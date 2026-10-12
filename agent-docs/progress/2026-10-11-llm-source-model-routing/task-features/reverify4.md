# task-features reverify4: 全体検証（前回の branch 引継ぎ・書き込み集合の抽出の後）

tasks: [01M4P7CTS42KTCR8HS4JF0252R]

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: task-features（再試行）/ WU key: reverify4（reverify3 は superseded）
- 基点: `CELERIS_WU_BASE` = de311ba7（WU の範囲基点コミット。中身は HEAD と同一で、`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` は clean）
- HEAD: 144529b3（doclink-fix の後）。前回 branch `celeris/01M4NM332S384YYC4VC5DB7KCY`（816a6a5b）は祖先に含まれる（merge-prev 済み）
- コードは変えていない。変えたのは `agent-docs/progress/2026-10-11-llm-source-model-routing/task-features.md`（再試行の節）と本ファイル。

## 検証（この run）

| 検査 | コマンド | 結果 |
|---|---|---|
| 全試験 | `bash scripts/dev/test-parallel.sh`（`TMPDIR=/tmp/rv4`） | exit 0。`CELERIS_TEST_SUMMARY`: nextest 5152 passed / 0 failed / 14 ignored、binaries 178（nextest 168・doctest 10）、nextest_exit 0 / doctest_exit 0、nextest 103.8 秒 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0、`Finished dev profile`、警告 0 |
| 文書参照 | `sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（`check-adr-numbers: ok (183 files)`） |
| 進捗索引 | `sh scripts/dev/progress-index.sh --check` | exit 0（`progress-index --check: ok`） |
| route 系の試験 | `cargo test -p task-core --lib route_` | exit 0。36 passed / 0 failed（書き込み集合の 5 本、task／WU の deps・retry・repair・budget の試験、`route_record_schema_pinned` を含む） |
| 範囲 | `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` | clean（差分ゼロ） |

## 試験の環境（TMPDIR を短くした理由）

- 1 回目は run の作業 tmp（`…/runs/01M4PZP298B67XV075WJ5T3C7T/tmp`、94 字）のまま `test-parallel.sh` を回し、exit 100。失敗 216 件の大半は `credentiald`・`task-api`（browser_live／browser_e2e）・`celerisctl` の socket 試験で、ログの 158 箇所が `SUN_LEN`（unix socket の path が 108 バイトを超えた）。コードの失敗ではない（前回 reverify2 の記録と同じ現象）。
- 2 回目は `TMPDIR=/tmp/rv4`（短い path）で全件 exit 0。`/tmp` にはテストの一時 socket だけを置き、リポジトリの写しや build 出力は置いていない。

## 試験の対象（書き込み集合）

- `route_extract_write_set_work_unit_prior_attempt_in_brief_and_aux`（WU。`run_write_sets`（work_unit_id 一致）と `work_unit_write_sets` の両方の path が brief の「書き込み集合」節と aux の `write_set` に入る）
- `route_extract_write_set_task_prior_attempt_in_brief_and_aux`（task。`run_write_sets`（task_id 一致）だけを読み、WU の集合は読まない）
- `route_extract_write_set_work_unit_without_prior_attempt_is_missing`／`route_extract_write_set_task_without_prior_attempt_is_missing`（前の試行が無いときは「範囲の記述なし」と `write_set` の欠測）
- `route_extract_write_set_excludes_sets_recorded_after_decision_time`（判断時点より後の行を読まない）

以上は全て上の全体 run に含まれ、0 failed。

## 残り（この WU の範囲外、進捗の「残り」節に書いた）

- label 側の `changed_lines`・`changed_files_n`・`changed_crates_n`・`quota_used`・`retries_after`・`repaired` は未計測（null＋missing）。ADR の実装付記で selector／rollout 段の未解決事項と明記されている。突き合わせ表（`task-features/field-audit.md`）もこの扱いで記録している。
