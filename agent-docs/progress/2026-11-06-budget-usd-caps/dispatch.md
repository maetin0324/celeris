---
title: 予算の USD 上限 — WorkUnit dispatch（task-dispatch の追従）
tasks: [01M6QDK2PMJTXF17Z8TNDKF85M]
status: done
updated: 2026-11-06
---
# WorkUnit dispatch: task-dispatch を USD 上限に追従

完了日: 2026-11-06。設計は [ADR 2026-11-06-budget-usd-caps](../../adr/2026-11-06-budget-usd-caps.md) D1・D2・D4。

## 変更
- `Dispatcher.budget_rates`（`[budget_rates]` の換算単価、既定は空）と入口 `Dispatcher::set_budget_rates` を足した。config からの読みと呼び出しは次の unit（config）。`DispatchConfig` は変えていない（campaign の構築を触らないため）。
- `tree_counters` に `&BudgetRates` を渡す（`planner_flow.rs`・`tree_units.rs`）。planner の `TreePlannerContext.tokens_left` は `usd_left`（残り micro-USD を USD にした値）。
- `tree_run_limit_hold`: `max_tree_usd` の決定の本文に、費用不明の run の数と解除の方法を足す（`with_unknown_cost_note`。ADR D2-4）。
- 案件の guard・再開（`project_guard.rs`・`project_resume.rs`・`project_wake.rs`）は `budget::usage*` に rates を渡す。危険な超過の保留文は USD と費用不明の run 数を書く。
- 試験: `project_recovery.rs`・`project_controller_replay.rs`・`long_horizon_pilot.rs` の上限と seed を USD（claude-sonnet-5、input 2 USD/100 万 token）にした。`tree_gate.rs` に cache を単価で数える・費用不明で止まる・`[budget_rates]` で数えられる試験、`project_recovery.rs` に費用不明（単価の無い model・usage の無い run）で案件が止まる試験を足した。`scripts/dev/test-project-controller-replay.sh` の試験名を `..._usd_cap` に直した。
- 残した token 名: `worker_finish.rs` の `BudgetKind::Tokens`（run の task `Budget.max_tokens` の終了種別）と `token_budget.rs`（変更しない対象）。

## 証拠（提出時の HEAD）
- `cargo nextest run -p task-dispatch`: 1413 件 pass、exit 0。
- `cargo clippy -p task-dispatch -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings` は `crates/celeris`（config 欄・`ApiSettings.budget_rates`。WorkUnit config の担当）だけで落ちる。`test-affected.sh` は celeris が通らないため config unit の後に走らせる。

## 本番への移行
この unit では無し（config は WorkUnit config）。

## 後続 unit への引き継ぎ
- celeris の daemon は `Dispatcher::set_budget_rates` と `ApiSettings.budget_rates` に同じ `[budget_rates]` を渡す。

## 再実行（check 不合格の修正）
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` が落ちていた。原因は task-dispatch の試験側の lint（未使用の `project_retention_last`・`external_wait`・`replan_each`、collapsible if、手動 strip_prefix、範囲 pattern、`Default` 後の field 代入、複雑な型）。すべて直し、提出時の HEAD で clippy・`cargo fmt --all -- --check` が exit 0、`cargo nextest run -p task-dispatch` が 1413 件通過。
- 範囲 check は `scripts/dev/test-project-controller-replay.sh` の変更が範囲外で落ちていた。この WU では変更を戻した。**closeout で要対応**: 同 script の `project_controller_replay_token_cap` を `project_controller_replay_usd_cap` に直す（試験名は USD 化済みで、script が旧名を期待している）。
