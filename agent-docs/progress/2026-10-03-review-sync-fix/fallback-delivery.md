---
task: review-sync-fix
wu: fallback-delivers
status: done
completed: 2026-10-03
---
# fallback-delivers: IntegrationRepair の fallback → merge-base 修復 → 配送成功の通し試験

## 変更の要点
- `crates/celeris/src/delivery/tests.rs` に `integration_repair_fallback_delivers` を追加。`merge_queued_delivery` / `cfg_for` / `target_advanced_events` と、`merge_base_repair_revalidates_and_delivers_only_the_repaired_branch` の修復完了・偽 prepare の手順を流用。
- 流れ: main が同じ file を変えて分岐 → `IntegrationRepairExhausted{fallback:true, before_sha=元の HEAD}` → 未同期 review（候補 NULL）で MergeQueued → `advance` の `check_candidate` が候補 NULL・target 非祖先で Fresh とし `[merge-base]` Blocked → 次の `advance` で `make_repair`（repair (merge_base) WU が Ready）→ 修復が main を merge して衝突解決 → 修復後の review で fallback が解け（HEAD が打ち切り時から動いた・main が祖先）、target/reviewed/merge candidate を記録 → `validate_candidate` 合格・`check_candidate` Fresh → `advance` が ff merge して Preparing（main = 修復後 HEAD、release = 先頭 12 桁）。
- 途中の確認: `ReviewTargetAdvanced` は 0 件（`MAX_TARGET_RESYNCS` 未満、`[needs-human]` にならない）、`review_fail` の遷移なし、task は Done で attempts 不変。
- 配送側（`crates/celeris/src/delivery.rs`）は変更不要だった。ADR-0118 D4 の stale 経路は未変更。
- dispatcher の review（`review_spawn` の `task_ops::delivery::begin`・同期による候補記録、`review_verdict` の MergeQueued 記録）は celeris の試験から動かせないので、同じ形で delivery 行に書く closure で再現した。同期するかは fallback-release の規則（HEAD が `before_sha` から動いた、または target が HEAD の祖先）で決める。dispatcher 側の解除そのものは `dispatcher/tests/target_sync.rs` の `integration_repair_fallback_released_when_target_merged` などが本物で確かめている。

## 証拠
- `cargo test -p celeris integration_repair_fallback_delivers` → 1 passed
- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace` → exit 0、3767 passed / 0 failed / 13 ignored
- `cargo clippy --workspace -- -D warnings` → exit 0

## 未解決事項
- dispatcher と celeris の delivery を 1 つの試験で本物同士つなぐ試験は無い（task-dispatch は celeris に依存できず、celeris 側から Dispatcher を組む試験基盤も無い）。必要なら e2e（tests/e2e）で daemon を立てる試験を別に置く。
