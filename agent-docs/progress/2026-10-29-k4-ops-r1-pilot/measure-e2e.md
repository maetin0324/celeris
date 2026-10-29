---
task: 01M62V0FQMMYQ1S83GBXQP7A6J
unit: measure-e2e
status: done
completed: 2026-10-29
---
# measure-e2e: 生成設定の一時 daemon で CoS 経路を stub で通す

結論: `crates/celeris/tests/ops_r1_measure.rs` が、生成した daemon.toml の実 `celeris` daemon（stub harness・無 LLM）で「人の message → CoS の起票 → worker run → CoS の報告 message」を通し、health の `cos_policy_version` = "6" を確かめる。daemon・task-*・web のコードは変えていない。生成 script に `[harness_selection]` が足りず CoS が起票した task が route されなかったので、script を直した（下の「見つかった不足」）。基準は `$CELERIS_WU_BASE` と `git merge-base HEAD main`（= release 94f86928dc13）。

## 試験の中身
- `scripts/campaign/ops-r1-measure-config.sh --harness stub --models cos-opus` で一時 state に daemon.toml を生成する。合成の release dir（`bin/celeris` は `CARGO_BIN_EXE_celeris` への symlink、`manifest.json` は sha12 だけ）を渡す。生成物に `[cos] enabled = true`・`policy_version = "6"`・並列 1・stub harness があり、`kb/skills/{cos-operator,cos-inbox-triage}` が配送されたことも確かめる。
- 試験だけの追加: daemon.toml の末尾に人の token（`human:ops-r1-e2e`）を 1 つ足す。人の message は人の principal で送る。
- 実 `celeris` を `env_clear`（PATH・HOME・TMPDIR・stub の記録 dir だけ）・loopback の空き port・一時 DB で起こす（`cap_probe_real_daemon.rs` の型）。
- stub（sh）が 3 役を受ける。CoS run は `CELERIS_COS_RUN_CREDENTIAL` を持つので env で分ける。人の依頼の目印を含む最初の CoS run だけが `POST $CELERIS_API_URL/cos/operations`（`task.create`、`policy_version` は `CELERIS_COS_POLICY_VERSION`）で起票し、他の CoS run は報告を返す。worker run は done を返す。
- 待ちは API・DB の poll（200 ms の sleep、上限 180 秒の保険）。通常は 9 秒で終わる。CPU を焼く待ちは無い。
- assert:
  1. `GET /api/v1/health` の `cos_policy_version` が "6"。
  2. 人の message（`POST /chat/threads/{t}/messages`、202・role=user）。
  3. 起票が CoS 由来: `/cos/operations` の応答が `actor = "cos"`・`action = "task.create"`・`state = "applied"`・`policy_version = "6"`・案件 CoS thread・`target_id` = 起票された task。DB の `cos_operations` に 1 行あり、その CoS run の入力 message が人の message。
  4. worker run: stub がこの task の RunRequest を worker 役で受け、`runs` に role=worker・status=completed の行がある。task は done、`project_id` は試行案件。
  5. 報告: 案件 CoS thread に assistant message（`OPS_R1_E2E_REPORT: …`）が残り、その run の入力は `root-done:<task_id>:…`（root 完了の報告）。人の message への CoS の返事（起票の知らせ）も同じ thread に残る。

## 見つかった不足（直した）
- 生成した config には `[harness_selection]` が無く、CoS が起票した task が `route refused: harness selection failed: no eligible harness is declared` で止まった。CoS run は harness を明示するので通るが、通常の task は genre・role・execution class の規則か既定が要る。実測定（`--harness real`）でも同じく止まる。
- 直し: script が `[harness_selection] rules = [{ required_capabilities = [], harness = "claude_code" }]` と `execution_class_defaults = { coding/planner/review = ["claude_code"] }` を書く（通常運用 config の最後の「条件なし → claude_code」と同じ形）。offline 試験 `test_stub_config_fields` にこの節の assert を足した。

- 前回提出の check で試験が 1 回落ちた（`assertion left == right: left "" right "200"`）。原因は試験側の競合: daemon が起票を commit すると task は API に見えるが、stub の `curl -w '%{http_code}' >status` はまだ応答を書き終えていない（shell の redirect で status は空のまま開かれている）。直し: task を見つけた後に、status file が空でなくなるまでも出来事待ちで待つ（`-w` は本文の後に書かれるので、そのとき response.json も書き終わっている）。daemon のコードは変えていない。

## 証拠（提出時の HEAD で実行）
- 条件 0: `cargo nextest run -p celeris --test ops_r1_measure` → exit 0、`1 test run: 1 passed`（7〜9 秒）。競合の直しの後に 4 回続けて実行し 4 回とも pass。
- 条件 1: 上の assert 1〜5（試験の本文）。値は一度 `--no-capture` で確かめた: runs = `[["worker","completed"]]`、報告 run の入力 = `root-done:<task_id>:16`、報告 text = `OPS_R1_E2E_REPORT: 依頼の task が完了しました。`。
- 条件 2: `git diff $CELERIS_WU_BASE -- crates/celeris/src 'crates/task-*' web | wc -l` → 0。`git merge-base HEAD main` からも 0。`sh "$CELERIS_WU_SCOPE_PATHS"` → `crates/celeris/tests/ops_r1_measure.rs`・`scripts/campaign/ops-r1-measure-config.sh`・`scripts/campaign/test_ops_r1_measure_config.py`（と本ファイル・docs/ops）。
- 条件 3:
  - `bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celerisctl、`820 tests run: 820 passed`。文書検査 architecture-map・progress-index の self-test も OK）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
  - `cargo fmt --all -- --check` → exit 0。
  - `python3 -I scripts/campaign/test_ops_r1_measure_config.py` → `Ran 9 tests ... OK`。

## 本番で使う手順（人が実行）
1. config 生成（実 harness）: `sh scripts/campaign/ops-r1-measure-config.sh --state <測定 state> --release-dir /local/celeris/state/releases/94f86928dc13 --models <決定の key> --claude-accounts <claude の account dir> --clusters-from ~/.config/celeris/config.toml --out <daemon.toml>`。生成物に `[harness_selection]` があることを `grep -n harness_selection <daemon.toml>` で確かめる。
2. 起動: `scripts/campaign/measure-launcher.sh start`（`DAEMON_CONFIG=<daemon.toml>`）。確認: `curl -s http://127.0.0.1:<port>/api/v1/health` の `cos_policy_version` が `"6"`。
3. 案件と案件 CoS thread: `docs/ops/ops-r1-measure-daemon.md` §3 の 2 つの curl（`setup/project.json` で `POST /api/v1/projects`、続けて `POST /api/v1/chat/threads {project_id}`）。
4. 人の依頼を thread に入れ、`GET /api/v1/chat/threads/<t>/messages` で CoS の返事、`GET /api/v1/tasks?project=<project_id>` で起票、task done の後に thread の報告 message を確かめる。

## 未解決事項
- 人の token: 生成 config の token は `operator:ops-r1` だけ。この token で送った message の author は `operator:ops-r1` になり、`celeris-campaign ops-r1-intake` は author が `human:` の message だけを記録する。試験は人の token を末尾に足して通した。実測定で人がどの経路（web gateway を測定 daemon に向ける・人の token を config に足す）で送るかは未決定。
- 試行 1 回の上限: stub CoS の起票 body の `max_retries: 0` は create_task_op に受理された（201 相当の applied）。実 LLM の CoS が書く body の上限は org profile 側で固定する想定のまま（未確認）。

## 提案
- `ops-r1-measure-config.sh` に `--human-principal <human:name>` を足し、`<state>/human.token` と 2 つ目の `[[api.tokens]]` を書く。人の依頼の経路を決めた後に行う。
