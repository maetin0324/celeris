---
task: review-sync-fix
wu: fallback-release
status: done
completed: 2026-10-03
---
# fallback-release: IntegrationRepair の fallback を branch の変化で解き merge candidate を記録する

- `crates/task-dispatch/src/dispatcher/review_spawn.rs`: `fallback_head(events, repo_id)` を追加。最新の IntegrationRepair 記録が `Exhausted{fallback:true}` なら、そのとき review した未同期 HEAD（`rollback_to_sha` か `before_sha`）を返す。review 前同期は「今の HEAD がその SHA と同じ、かつ target が HEAD の祖先でない」間だけ省く。それ以外は `integration repair fallback released for <repo>: …`（`FALLBACK_RELEASED_PREFIX`）を残して通常の `sync_onto_target` に戻り、`ReviewTargetSynced` と delivery の target/reviewed/merge candidate を記録する。判定は毎回 events と git から作る（memory に持たない）。event 欄・migration の追加なし。`Trigger::ReviewFail` は使わない。
- 試験（`dispatcher/tests/target_sync.rs`。`exhausted_fixture` / `exhausted_for` を使う共通の `fallback_then_rereview`: plan_issue による fallback → Done → `Trigger::Rereview`）:
  - `integration_repair_fallback_keeps_skipping_while_branch_unchanged`（(a)。dispatcher を作り直しても省く）
  - `integration_repair_fallback_released_when_target_merged`（(b)。main を merge → 候補 = merge 後の HEAD）
  - `integration_repair_fallback_released_when_head_moves`（(c)。HEAD を書き直す → rebase して候補を記録）
- ADR-0120 の末尾に「付記: fallback の解除（2026-10-03）」を追加。

## 証拠

- `cargo test -p task-dispatch --lib target_sync` → exit 0、23 passed / 0 failed
- `cargo test --workspace` → exit 0、合計 3766 passed / 0 failed / 13 ignored
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項

- fallback → merge-base 修復 → 配送成功 の通し試験（`integration_repair_fallback_delivers`）は並行 WU fallback-delivers の担当。

## 提案

- なし
