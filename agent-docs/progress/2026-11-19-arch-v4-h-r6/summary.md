---
title: アーキ v4 H R6 summary — gate・verify・promoted の要約を $SD_LOGS/release-records.jsonl に追記し、掃除の後も残る試験を足す
tasks: [01M7VJY8Q1MMAH87CSPQZWC7VY]
status: blocked
updated: 2026-11-20
---
# summary（WorkUnit）

完了日: 未完（範囲 check の不合格で計画の直しを申告した。下の「未解決」）

## 結論

- [ADR 2026-11-20 release-record-summary-log](../../adr/2026-11-20-release-record-summary-log.md) の D2〜D5 を実装した。
  `scripts/selfdeploy/lib.sh` に `sd_release_record <gate|verify|promoted> <正本の path>` を 1 つ置いた。この関数は正本の file を python で読み、
  要約を 1 行の JSON にして、`mkdir -p "$SD_LOGS"` の後に `>>` で `$SD_LOGS/release-records.jsonl` に追記する。
- 呼び出しは 5 か所:
  - `release.sh`: 通った gate は `mv -T "$STAGE" "$REL"` の後（`$REL/gate.json`）。release dir ができる前に ok の行を書かないためで、ADR D2 の時点と同じ。
    落ちた gate は `$FAILED_DIR/gate.json` を書く 2 か所の直後。
  - `verify.sh`: verify.json を書いた後。`ok` が偽でも追記する。
  - `promote.sh`: `sd_audit finish "$AUDIT_FILE" 0` が promoted.json を書いた後。promote.sh を通る rollback（`SD_OPERATION=rollback`）も含む。
- 追記に失敗しても（置き場に書けない・正本が読めない・python3 が無い）、`sd_log` の warning を出して 0 を返す（D4）。台本の exit code と正本の file は変わらない。
- 新しい試験 `scripts/selfdeploy/tests/release_record_summary_survives_prune.sh` を足した（D5）。
- `docs/ops/selfdeploy.md` §0 の置き場の図と「配備操作の監査情報」の新しい小節「gate・verify・昇格の要約の写し」に、置き場・形・
  「正本は release dir の file」・判断に読まないことを書いた。
- 既存の release・verify の試験 7 本に `unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR` を足した（下の「変えた既存の試験」）。この 7 本は計画の範囲 check の外にあり、check が落ちる。計画の直しを申告した（「未解決」）。
- `crates/`・policy の file は変えていない。本番の release.sh・verify.sh・promote.sh は走らせていない。要約の log を読む経路は足していない。

## commit（件名）

- 「summary: gate・verify・promoted の要約を release-records.jsonl に追記し、掃除の後も残る試験を足す」
- 「summary: 要約の行を書かない既存の試験 4 本の unset を戻し、範囲 check の不合格を記録する」

## 実装前 brief（ADR 1 節の要約）

1. 根本原因: gate.json・verify.json・promoted.json は release dir の中にしか無く、`prune_releases` が dir ごと消す。
2. 責務 owner: 3 つの file を書く場所（release.sh・verify.sh・promote.sh）。要約を作って追記する関数は lib.sh に 1 つだけ置く。
3. 既存の再利用先: `SD_LOGS`・`sd_ts`・`sd_log`・python3（promote の log と同じ置き場）。試験は release_parallel_test_gate.sh の fixture と同じ形にした。
4. 選択肢: 追記の log を採用した（B1）。3 file の写しを残す案（B2）、掃除で残す数を増やす案（B3）、DB に書く案（B4）は採らない（ADR 1.4）。
5. 新しい状態・分岐: 追記だけの写しの file を 1 つと、失敗を警告だけにする分岐を足した。flag・fallback・port は足していない。
6. 撤去対象: 無し（新しい追記）。
7. 不変条件: `unpromoted-release-no-takeover`（要約の log を判断に読まない）と、追記で台本の結果を変えないこと。どちらも新しい試験の場合 6・7 で確かめる。

## 新しい試験（`release_record_summary_survives_prune.sh`）

偽の cargo・pnpm・node・celerisctl と一時 dir の fixture で release.sh を 4 回走らせる。fixture は release_parallel_test_gate.sh と同じで、
`check-slot.sh` と `cos-skills.txt` も写す。verify と昇格は lib.sh の関数を fixture の verify.json に当てる。promoted.json は audit.sh の
`sd_audit begin`・`finish` で作った本物の形のものに当てる。冒頭で `CELERIS_BACKUPS_DIR`・`CELERIS_LOGS_DIR`・`CELERIS_CONFIG` を unset する。

1. 通った release の gate の行が 1 行で、`file`・`sha`・`step_count`・`steps`・`cargo_test`（`nextest_passed` 7 など）が gate.json と一致する。
2. verify（ok）と promoted の行が足され、2 回目の追記の後も 1 行目がバイト単位で同じ。
3. 再 verify（ok が偽）で新しい行が足され、前の verify の行は直らない（D3）。
4. 落ちた gate の行（`ok` が false、`failed_step` が cargo-test、`file` が `.build/<sha12>/gate.json`）。
5. 次の release の掃除で 1 本目の release dir が消えた後も、その sha の gate・verify・promoted・verify の 4 行が残り、1 行目も変わらない。
6. 置き場に書けないとき、release.sh は exit 0 で gate.json も ok になり、warning が出る。読めない正本では関数が 0 を返して警告し、行は増えない（D4）。
7. 台本の行の順: release.sh に 3 か所あり、通った gate は `mv -T` の後。verify.sh は verify.json の後、promote.sh は `sd_audit finish … 0` の後に呼ぶ。
   台本（release・verify・promote・rollback・status）の注釈でない行と `crates/` に `release-records` が無い（D3）。

注釈に「既存の prune の試験は要約が残るかを見ないので足す」と、守る不変条件 (1)〜(5) を書いた。

## 変えた既存の試験（`unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR` の 3 行だけ）

理由: worker の run の環境は本番の `CELERIS_LOGS_DIR=/local/celeris/state/logs` を export している。release・verify の既存の試験は
`CELERIS_STATE_DIR` を一時 dir に向けるが `CELERIS_LOGS_DIR` を継ぐ。unset しないと、release.sh・verify.sh が偽の sha の要約の行を本番の
`release-records.jsonl` に足してしまう。unset すれば `$SD_LOGS` は一時の state の下（`backups/`）になる。promote_authorization_marker.sh
（159〜160 行）と promote_live_abort.sh（201〜202 行）が既に同じ unset をしている。試験の不変条件と期待値は変えていない。

どの試験が行を書くかを測った。base の版の試験 11 本を、`CELERIS_LOGS_DIR`・`CELERIS_BACKUPS_DIR` を `$TMPDIR` の一時 dir に向けて走らせ、
`release-records.jsonl` の行数を数えた（台本は HEAD の lib.sh・release.sh・verify.sh）:

| 試験 | 書いた行 | unset |
|---|---|---|
| release_parallel_test_gate | 6 | 残す |
| release_prune_stale_test_binaries | 6 | 残す |
| release_web_required | 6 | 残す |
| release_uses_scratch_lease | 2 | 残す |
| verify_web_app_start | 2 | 残す |
| browser_ledger_release_stages | 1 | 残す |
| release_web_bundle_requires_deps | 1 | 残す |
| bundle_includes_verify_helpers | 0 | 戻した |
| release_lock_bundled_scripts | 0 | 戻した |
| release_target_settled_after_build | 0 | 戻した |
| repo_health_bundle | 0 | 戻した |

行を書かない 4 本の unset は要らないので base に戻した。行を書く 7 本は unset を残した。
全部の試験を走らせた後も `/local/celeris/state/logs/release-records.jsonl` はできていない（`ls` で「No such file or directory」）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

### 基準 0: 新しい試験が引数なしで exit 0

- `bash scripts/selfdeploy/tests/release_record_summary_survives_prune.sh` → exit 0、`release_record_summary_survives_prune: ok`（この host で約 22 秒）。
- 反例（`$TMPDIR` の写しの木で台本を壊して走らせた）:
  - release.sh の通った gate の `sd_release_record gate "$REL/gate.json"` を消すと exit 1、`FAIL: expected 1 record after the first release, got 0`。
  - lib.sh の `>>` を `>` にすると exit 1、`FAIL: expected 2 records after verify, got 1`。

### 基準 1: release_parallel_test_gate.sh と promote・release notes の既存の試験

`for t in scripts/selfdeploy/tests/*.sh; do bash "$t"; done`（33 本）:

- exit 0 は 30 本。release_parallel_test_gate・release_notes_promote・promote_authorization_marker・promote_backup_retention・
  promote_handoff_settled・promote_live_abort・promote_web_follows_release・release_prune_production_scale・
  release_prune_stale_test_binaries・prune_tests_stay_in_tmp・verify_web_app_start・新しい試験などを含む。
- exit 1 は 3 本: browser_ledger_credential_evidence・browser_ledger_release_stages・release_web_bundle_requires_deps。
  変更の前の HEAD（`git archive HEAD scripts tools/nextest` で出した木）でも 3 本とも同じ `FAIL:` の行で exit 1 になる。原因は fixture が
  `scripts/dev/lib/check-slot.sh` を写さず、`test-parallel.sh: line 55: …/check-slot.sh: No such file or directory` で gate が cargo-test で落ちること
  （counts が release_parallel_test_gate.sh で直したのと同じ不足）。この変更とは関係なく、この WorkUnit の範囲の外なので直していない（「提案」）。

### 基準 2: crates/ を変えず、crates/ に release-records の参照が無い

- `git diff --name-only $CELERIS_WU_BASE HEAD -- crates/` → 空。
- `git grep -n release-records -- crates | wc -l` → 0（新しい試験の場合 7 も同じことを確かめる）。

### 基準 3: docs/ops/selfdeploy.md

- §0 の置き場の図に `$SD_LOGS/release-records.jsonl` の行を足した。「配備操作の監査情報」に小節「gate・verify・昇格の要約の写し」を足し、
  置き場・追記の時点・欄・「**正本は release dir の file**」・判断に読まないこと・失敗は警告だけであることを書いた。

### repo の検査

- `bash scripts/dev/test-affected.sh` → exit 0（`test-affected: 試験対象の変更なし（docs のみ等）`。python の試験 126 件 OK、repo_health_gate の 7 件 OK）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash -n` で lib.sh・release.sh・verify.sh・promote.sh の構文 → ok。shellcheck はこの host に無く、走らせていない。

## 本番への移行

- config・API・CLI・DB の変更は無い。人の作業は要らない。
- この変更を含む release の後、`release.sh`・`verify.sh` を回すと `/local/celeris/state/logs/release-records.jsonl`（`paths.env` の
  `CELERIS_LOGS_DIR`）に行が増える。web の昇格（`POST /promote`）は `current` に同梱の promote.sh を走らせるので、promoted の行は
  この変更を含む版が `current` になった後の昇格から増える。
- 確かめ方（人が実行する。任意）: 次の release の後に `tail -n 3 /local/celeris/state/logs/release-records.jsonl`。
- 掃除で既に消えた release の記録は戻らない。

## 未解決

- 範囲 check が落ちる（計画の直しを申告した）。この unit の範囲 check の正規表現は `scripts/selfdeploy/tests/` のうち
  `release_record_summary_survives_prune.sh` と `fixtures/` しか許さない。上の 7 本の既存の試験の unset は範囲の外なので check が exit 1 になる。
  unset を外せば check は通るが、worker の環境でその試験を走らせるたびに偽の行が本番の `/local/celeris/state/logs/release-records.jsonl` に
  足される（`/local` の本番の state を試験が書き換える）。台本の側（lib.sh）では、試験が一時 dir に向けた `CELERIS_STATE_DIR` と本番の
  `paths.env` から継いだ `CELERIS_LOGS_DIR` を見分ける決まりが無い。見分ける推測の分岐を足すと、承認の無い分岐になる。
  直し方: この unit の範囲 check に次の 7 本を足す。
  `scripts/selfdeploy/tests/(browser_ledger_release_stages|release_parallel_test_gate|release_prune_stale_test_binaries|release_uses_scratch_lease|release_web_bundle_requires_deps|release_web_required|verify_web_app_start)\.sh`
- 上の 3 本の既存の試験（browser_ledger_credential_evidence・browser_ledger_release_stages・release_web_bundle_requires_deps）は、
  base の時点で fixture の不足（`check-slot.sh`）により落ちている。

## 提案

- 上の 3 本の fixture に `scripts/dev/lib/check-slot.sh`（と、要れば `cos-skills.txt`）を写す小さな task を起こす。release の試験の偽の木は
  台本が読む file が増えるたびに壊れるので、写す file の一覧を 1 つの fixture（例: `tests/fixtures/release-tree.sh`）にまとめると、同じ不足が繰り返さない。
