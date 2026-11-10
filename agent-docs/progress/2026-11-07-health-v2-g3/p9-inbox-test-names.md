---
title: 健全性v2 G3 P-9 test-names — 受け入れ条件 0 の 6 件の試験を指定の名前にする
tasks: [01M71K2HHSJ20836QAMXJ0YMEW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G3 P-9 test-names — 受け入れ条件 0 の 6 件の試験を指定の名前にする

WorkUnit `test-names`。task の受け入れ条件 0 は 6 件の試験を関数名で grep するが、unit successors・event・inbox が入れた試験は名前が違い、最初の 1 件で落ちた。この unit は製品のコードを変えず、試験の名前と分け方だけを直す。基準は `$CELERIS_WU_BASE`。ADR (7) の付記と p9-inbox の進捗の試験名は unit `names-record` が直す。

## 変更
件名「P-9 test-names: 受け入れ条件 0 の 6 件の試験を指定の名前にする」の commit。変えた file は次の 2 つの試験 file とこの進捗 file だけ。

### 旧名 → 新名
| crate | 旧名 | 新名 |
|---|---|---|
| task-core | `dependency_reopen_keeps_dependent_with_retry_successor` | `dependency_reopen_keeps_retried_dependent` |
| task-core | `dependency_reopen_retry_skips_dependent_with_successor` | `dependency_reopen_retry_skips_retried_dependent` |
| task-core | `dependency_reopen_skips_unchanged_record` | `dependency_reopen_rereview_does_not_repeat_event` |
| task-ops | `dependents_reopened_inbox_item_lifecycle` | `dependency_reopen_inbox_lists_restored_and_kept`・`dependency_reopen_inbox_item_resolves_when_dependents_move`・`dependency_reopen_kept_only_event_has_no_inbox_item` の 3 件（旧名は消した） |

`dependency_reopen_records_restored_and_kept`（task-core）は名前を変えず、A の部分だけを残した。

### 移した assert
- `crates/task-core/src/store/dependency_reopen_tests.rs`:
  - `dependency_reopen_records_restored_and_kept` の後半（done の X を rereview → 合格 → reopen し、人が cancel した後続 `manual` と、ほかの依存先 F の失敗で cancel された Y があっても X に `DependentsReopened` を書かない部分。`y` が cancelled、`x` が reviewing・ready、`reopened_records(x)` が空）を、そのまま `dependency_reopen_rereview_does_not_repeat_event` の先頭に移した。その後に旧 `skips_unchanged_record` の A（kept だけの記録を 2 回目の rereview で重ねない）と Z（人が cancel した後続だけなら書かない）の assert が続く。
  - 足した assert（弱めたものは無い）: `dependency_reopen_rereview_does_not_repeat_event` で、X の rereview の直後にも `reopened_records(x)` が空であること（task の受け入れ条件 0 の 6 件目「done の A の Rereview では event を書かない」を rereview の時点で確かめる）。`dependency_reopen_keeps_retried_dependent` で、A の記録が restored `[C（依存先 [A]）]`・kept `[B（retried、後継 B'）]` の 1 件であること（task の受け入れ条件 0 の 1 件目「kept に retried と後継が入る」）。
- `crates/task-ops/src/inbox/dependents_reopened/tests.rs`:
  - 準備（A・E・B・C・D・X・Y・K・L の作成、E・A・X・K の失敗、cancel の連鎖の確かめ、C の retry、復帰の前に項目が無いこと、A・X・K の reopen、B・Y が ready で L が cancelled）を共有の helper `reopened()` に移した。準備の中の assert は 3 件のどれでも走る。
  - `dependency_reopen_inbox_lists_restored_and_kept`: A の項目の id の接頭辞、`blocking.tasks` が `[B]`、本文の B の id・「依存先 A」・「C（retry の後継 C2」・「D（ほかの依存先 E」、選択肢 `cancel:<B>` だけ、native の path `/api/v1/tasks/<B>/cancel`、`cancel_target` が B を返し D を返さないこと、`item_task_id` が A を返すこと。
  - `dependency_reopen_inbox_item_resolves_when_dependents_move`: X の項目が出ていて Y の cancel で消えること、A の項目が B の running で消え、`InfraRequeue` で ready に戻っても出ないこと、最後に項目が 1 件も無いこと。足した assert: B を動かす前に A の項目が出ていること。
  - `dependency_reopen_kept_only_event_has_no_inbox_item`: K の最新の `DependentsReopened` の restored が空で kept が 1 件、K の項目が出ないこと。足した assert: 同じ store の A には項目が出ていること（項目を作る経路が働いている中で K だけ出ないことを確かめる）。

### doc comment
改名・分割した 6 件の doc comment に、守る不変条件と、unit p9 の既存の `dependency_reopen_` の 5 件（`restores_cancelled_dependents_transitively`・`keeps_dependent_while_another_dependency_failed`・`keeps_manually_cancelled_dependent`・`returns_draft_dependent_to_draft`・`on_rereview_and_cancels_again_on_failure`）で検出できない理由を書いた。要点: 5 件は後続を retry せず（後継を持つ後続が無い）、依存先の retry の付け替えを通らず、依存先の側の events を読まず、done の依存先の rereview も 2 回目の rereview もせず、受信箱を組み立てない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 受け入れ条件 0 のコマンド（6 件の `fn` の grep と `cargo nextest run -p task-core -p task-ops -p task-api --no-tests=fail -E 'test(/dependency_reopen_/)'`）: exit 0、14 passed（task-core 11 件・task-ops 3 件）。
- 旧名 4 件（`dependency_reopen_keeps_dependent_with_retry_successor`・`dependency_reopen_retry_skips_dependent_with_successor`・`dependency_reopen_skips_unchanged_record`・`dependents_reopened_inbox_item_lifecycle`）の `grep -rnE … crates/`: 一致なし（exit 1）。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変えた path は 2 つの試験 file とこの進捗 file の 3 つだけ。製品のコードの差分は無い。
- `cargo fmt --all -- --check`: exit 0。`cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。先頭の check-health は verdict=pass、fail 級 0、worsened 0。new の 2 件（`dup.contract` の `UnavailableReason`、`dup.public-surface` の `fn judge`）は warning の候補で、この unit が変えていない file にある。nextest は 8177 passed・23 skipped。web は tsc -b 合格、vitest 104 file・712 件合格。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`: いずれも ok。

## 未解決
- 無し。

## 提案
- 無し。
