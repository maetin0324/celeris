# reverify5: label 段の統合後検証（labels-db・labels-diff の後の HEAD）

tasks: [01M4P7CTS42KTCR8HS4JF0252R]

判定: **fail**。`bash scripts/dev/test-parallel.sh` が exit 100（nextest の失敗 3 件）。clippy・文書検査 3 本は exit 0。crates/・web/ は変更していない（`git diff --stat HEAD -- crates web` 空）。

検証対象 HEAD: `1d2f06c0`（labels-diff の統合後）。

## 1. D3(b) label 6 欄の実装確認（コードを読んで確かめた）

`crates/task-core/src/route_extract.rs::outcome_labels_for_run`（DB 抽出）と `crates/task-ops/src/routing_labels.rs::outcome_labels_for_run`（changed_* の git 計算）を読んだ。前回の reviewer 指摘（6 欄が null＋missing 固定）は解消している。

| 欄 | 抽出元（実装） | 試験 |
|---|---|---|
| `quota_used` | `quota_estimated` event の最終 run の窓（値があるもの） | `route_outcome_quota_used_from_quota_estimated` |
| `retries_after` | 同 WU の後続 `worker` run 数（WU なし task は task 直下の run 数） | `route_outcome_retries_after_and_repaired_task_and_wu` |
| `repaired` | 開始後に作られた `kind='repair'` WU の有無 | 同上 |
| `changed_lines`・`changed_files_n`・`changed_crates_n` | task-ops: `run_write_sets`、WU は `work_unit_write_sets` への fallback。complete 行の base..head `git diff --numstat` | `routing_outcome_git_numstat_counts_lines_files_crates`、`route_extract_outcome_reports_missing_metrics_without_git` |

task／WU 両 subject の突き合わせは [field-audit.md](field-audit.md) の D3(b) 節（この WU では内容を変えず、欄の存在と試験名を確認）。

## 2. 検査の結果（HEAD 1d2f06c0）

- `bash scripts/dev/test-parallel.sh`
  - 1 回目: `TMPDIR` を run の作業 tmp（93 字）のまま。nextest 5048 passed / **108 failed**、exit 100。SUN_LEN（`path must be shorter than SUN_LEN`）と `timed out waiting for fixture readiness` が主因。
  - 2 回目: `TMPDIR=/local/celeris/tmp-reverify5`（31 字）。5125 passed / 31 failed、exit 100。credentiald・browser launcher の daemon 起動待ちが 60 秒で切れた。`deep_session_dir()` の固定 path（別 workspace の長い path）が SUN_LEN を超える。
  - 3 回目: 同じ `TMPDIR`、1 回だけの再実行（負荷による timeout の規則）。5125 passed / 31 failed、exit 100。2 回目と同じ失敗集合。
  - 4 回目: `TMPDIR=/local/rv5`（9 字）。**5153 passed / 3 failed** / 14 ignored、doctest exit 0、nextest exit 100。失敗は `task-api::browser_e2e` の次の 3 件: `approved_credential_requires_conformance_and_does_not_consume_approval`、`denied_approval_fails_the_task_without_a_lease`、`registered_credential_does_not_open_approval_for_uncertified_backend`。いずれも `POST …/browser/waits/{id}/credential` が 503 `browser_unavailable`（"credential broker is not configured"）を返し、期待 200。
  - 5 回目: 4 回目と同じ条件で 1 回再実行。同じ 3 件が再び失敗（5153 passed / 3 failed）。
- 失敗の切り分け: 失敗しているのは credentiald・browser launcher・task-api browser の試験で、この branch 上でその crate のコードは reverify4（`8657edc4`、5152 passed）以降変わっていない（`git diff --stat 8657edc4 HEAD -- crates` は task-core・task-ops の 3 file のみ）。一方、reverify4 は `TMPDIR=/tmp/rv4` で全件通っている。
- 未解決: browser_e2e の 3 件が 503 になる原因は特定できていない。readiness 待ち（control socket と resolve.sock の存在、上限 60 秒）は通っているので、後続の broker 呼び出し（`BrokerFailure::Unavailable`）が失敗している。試験 crate の変更なしでは詰め切れないため、報告する。
- `cargo clippy --workspace -- -D warnings`: exit 0（`Finished dev profile … in 34.14s`、警告 0）。
- `sh scripts/dev/check-doc-links.sh`: exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/check-adr-numbers.sh`: exit 0（`check-adr-numbers: ok (183 files)`）。
- `sh scripts/dev/progress-index.sh --check`: exit 0（`progress-index --check: ok`）。
- `git diff --stat HEAD -- crates web`: 空（コードは変えていない）。

## 3. 次にすること（提案）

- 3 件の `browser_e2e` の broker 503 を、この task の外（browser 系の担当）で調べ直す。原因が環境（broker の起動順・socket の権限・host 負荷）か、試験の書き方（ready 判定の後の別 socket）かを切り分ける。
- test-parallel の TMPDIR 規則を `leaf-check-rules-and-pitfalls` に合わせて直す。現在は短い base でも browser_e2e の 3 件が落ちるため、「短い base で通る」は成り立たない。
- 上の 3 件が解けるまで、この task の受け入れ 0 は未達。label 段（labels-db・labels-diff）の内容は上の 1 節のとおり完成している。
