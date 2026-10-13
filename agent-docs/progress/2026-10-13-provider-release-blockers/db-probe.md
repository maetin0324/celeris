# worker_db_read_only の run_db 調査

---
tasks: [01M4V68MRJWG2RPPQGK0ABZDXH]
---

## 結論

- 原因: `run_db=/local/celeris/data/db/celeris.sqlite3` は、試験の子 celeris（daemon）が設定した値ではなく、外側の run（この worker を起こした daemon の run）の環境から継いだ `CELERIS_RUN_DB` である。
- raw write 先（`raw_db_write_exit=0` / `raw_wal_write_exit=0`）は試験の一時 DB（`tempfile` 配下の `celeris.sqlite3` とその `-wal`）で、本番 DB ではない。
- injected follow-up は外側の follow-up へ流れていない。一時 DB に書かれた。
- 本番影響: なし。本番 DB は読まず・書かなかった。本番 DB の確認手順は不要と判定し、「本番への移行」節には書かない。

## 原因と値の出所（file:line）

- 試験の stub: `tests/e2e/tests/worker_db_read_only.rs:152-154` で一時 DB（`root/celeris.sqlite3`）を作り、stub の `DB='{db}'` に埋める。`:189-195` の `ls` / `show` / `add --title injected` / raw append は全て `$DB`（一時 DB）を書き先にする。`:191` の `echo "run_db=${CELERIS_RUN_DB:-}"` は環境の値をそのまま出す。
- daemon が設定する値: `crates/task-dispatch/src/dispatcher/followups.rs:18-42` の `followups_env` は、`CELERIS_TASK_ID`・`CELERIS_RUN_ID`・`CELERIS_FOLLOWUPS_FILE` を常に足す。`CELERIS_RUN_DB` は `run_db` が `Some` のときだけ足す（`:35-36`）。`run_db` は `crates/task-dispatch/src/dispatcher/worker_task.rs:594` で `task_worker::db_guard::installed()` から取る。
- db_guard が入らない経路: `crates/celeris/src/daemon/bootstrap.rs:136-147` は、本番に当たらない daemon（`Exempt`）と guard probe 失敗（`:142` の warn「running without it」）で `install(None)` になる。このとき `followups_env` は `CELERIS_RUN_DB` を足さない（修正前）。
- 継承: `crates/task-dispatch/src/dispatcher/worker_task.rs:54-62` で `followups_env` を `adapter.with_env(env)` で重ねる。`claude_code` の子 process は `crates/task-worker/src/claude_code.rs:522` の `Command::new` で親環境を継ぐ（`env_clear` 無し）。よって外側の run の `CELERIS_RUN_DB` が残った。`CELERIS_FOLLOWUPS_FILE`・`CELERIS_TASK_ID`・`CELERIS_RUN_ID` も同じ経路で残り得た。
- 外側の値の証拠: この closeout run の環境にも `CELERIS_RUN_DB=/local/celeris/data/db/celeris.sqlite3` が入っている（`env | grep ^CELERIS_`）。試験の出力の値と一致する。

## なぜ h3-userns の run で出たか

- `h3-userns` の全体検査は `userns:false`（`unshare -Ur true` 不可）だった（`h3-userns.md` の記録）。そのため daemon の db_guard probe は失敗するか Exempt になり、`install(None)`（`bootstrap.rs:142`・`:147`）。修正前は `CELERIS_RUN_DB` を足さないので、外側の run の値（本番 path）が子の stub に届いた。
- 出力の `raw_db_write_exit=0` は一時 DB への書き込みであり、本番 DB への書き込みではない（guard が無いので一時 DB は書ける）。

## 外側の follow-up への流出の有無（判定: 流出なし）

- `crates/celerisctl/src/main.rs:268-278` の `followups_target` は、`CELERIS_FOLLOWUPS_FILE` と `CELERIS_RUN_DB` が両方あり、かつ `--db` 解決後の書き先が `CELERIS_RUN_DB` と同じときだけ follow-up file を返す。stub の `add` は `--db "$DB"`（一時 DB）を明示するので、`target`（一時 DB）と `run_db`（本番 path）が一致せず、`crates/celerisctl/src/commands/add.rs:213-220` の queue には入らず、一時 DB に `injected` を作った。
- 読み取りの確認: `/local/celeris/data/workspaces/01M4T94XWN31GW9SENK41GAP8F/` の下に `followups*.json` は無い（`find` で検索、該当は repo 内のソースのみ）。`runs/*/stdout.jsonl` に `queued follow-up` と `title … injected` の一致は無い（h3-userns の run 群）。`injected` の文字列は他の run のプロンプト・計画文書の中にだけ出る。
- 本番 DB は開いていない（読みも書きもしていない）。

## 修正

- 本体: `crates/task-dispatch/src/dispatcher/followups.rs:37-41`。db_guard が無いとき `CELERIS_RUN_DB` に空文字を明示する。adapter が親環境を重ねても空が勝つ。`celerisctl` の `run_db()`（`crates/celerisctl/src/main.rs:260-264`）は空値を無効として扱うので、外側の DB を run の DB と誤認しない。
  - 決定的な試験（userns 不要）: `crates/task-dispatch/src/dispatcher/followups.rs` の `tests::inherited_run_db_is_cleared_when_db_guard_is_unavailable`（`db_guard` が `None` のとき `CELERIS_RUN_DB` が `""` になることを確かめる）。
- 試験: `tests/e2e/tests/worker_db_read_only.rs:263-268` で子 celeris の起動時に `CELERIS_RUN_DB`・`CELERIS_FOLLOWUPS_FILE`・`CELERIS_TASK_ID`・`CELERIS_RUN_ID` を `.env_remove(...)` で消す（既存の `WORKER_DB_GUARD_ENV`・`CELERIS_CONFIG` と同じ形）。

## 残る注意（この WU では直していない）

- `followups_env` は `with_env` を実装する adapter にだけ届く（`crates/task-worker/src/adapter.rs:308` の既定は `None`）。`browser_launcher_run.rs` は `with_env` を持たず、子の環境も `env_clear` で作られるため、今回の経路（親環境の継承）には当たらない。新しい adapter が `with_env` を持たず親環境を継ぐなら、同じ空値の明示が要る。

## 検証（この葉）

- `CELERIS_USERNS_TESTS=1 cargo test -p e2e --test worker_db_read_only -- --exact a_codex_worker_run_cannot_write_the_daemon_db_but_can_read_it_with_celerisctl` → `test result: ok. 1 passed`（userns が作れる host。`unshare -Ur true` は exit 0）。
- `cargo test -p task-dispatch --lib -- --exact dispatcher::followups::tests::inherited_run_db_is_cleared_when_db_guard_is_unavailable` → `1 passed`。
- 試験は userns 試験（ADR-0126 A3）なので、`CELERIS_USERNS_TESTS=1` 無しでは `SKIPPED` で終わる。全体検査は `agent-docs/progress/2026-10-13-provider-release-blockers.md` の証拠節を参照。
