---
title: 予算の USD 上限 — WorkUnit api-worker（task-worker protocol と task-api）
tasks: [01M6QDK2PMJTXF17Z8TNDKF85M]
status: done
updated: 2026-11-06
---
# WorkUnit api-worker: task-worker protocol と task-api を USD 上限に追従

完了日: 2026-11-06

設計は [ADR 2026-11-06-budget-usd-caps](../../adr/2026-11-06-budget-usd-caps.md) D4。

## 変更
- `task-worker`: `TreePlannerContext.tokens_left` を消し `usd_left: Option<f64>` にした（f64 を含むので `Eq` の derive を `TreePlannerContext` とその親 2 つから外した）。planner の prompt は「budget left: N.NN USD-equivalent」を出す。
- `task-api`: `ApiSettings.budget_rates`（`[budget_rates]` の写し）を足し、`task-tree` の `limits` と `GET …/controller` の換算に渡す。
  - `limits`（`TreeLimitsUsage`）: `max_tokens` を消し `usd`・`max_usd?`・`usd_unknown_runs` を返す。`tokens` は表示用で残る。
  - `budget_today`: `usd`・`usd_unknown_runs`・`price_table_version` を足した。`summary.budget_limits` は `max_usd_per_day`。
- `task-core`（最小の変更）: `ProjectBudgetToday` に上の 3 欄を足し、f64 を含むので `Eq` を外した。
- schema（`docs/protocol/worker-protocol.schema.json`、`docs/api/v1/api-v1.schema.json`）を `UPDATE_SCHEMA=1` で再生成。`docs/api/v1/api.md` を直した。
- web: `web/api/generated/*` を再生成。手書きの controller 画面（`project-controller-panel.tsx`）と試験・e2e fixture を USD の欄に直した。
- 試験: `task_tree.rs` に、cache を単価で数える・単価の無い model が `usd_unknown_runs` になる・`budget_rates` を書いた model が数えられる試験を足した。`project_controller.rs` は `budget_today` の新欄と `max_tokens_per_day` が無いことを確かめる。

## 後続 unit への引き継ぎ
- `task-dispatch/planner_flow.rs` が `usd_left` を、`celeris/src/daemon/api.rs` が `ApiSettings.budget_rates` を渡す必要がある（この unit の範囲外。今は workspace 全体が通らない）。
- 試験用の `ApiSettings`・`TestEnv` に `budget_rates` を足してある（`crates/task-api/tests/common/mod.rs`）。

## 証拠（提出時の HEAD）
- `cargo nextest run -p task-api -p task-worker`: 2097 件 pass、skip 11、exit 0。
- `cargo clippy -p task-core -p task-ops -p task-worker -p task-api -- -D warnings`: exit 0（task-dispatch・celeris は後続 unit が未追従で workspace 全体は未確認）。`--all-targets` では task-core の既存の試験コードに 3 件（この unit の変更ではない）。
- `cargo fmt --all -- --check`: exit 0。
- `web`: `tsc -b` exit 0、`vitest run features/projects` 32 件 pass。

## 本番への移行
この unit では無し（config 欄の変更は WorkUnit config）。

## task-core の公開 API 変更（最小）

- `crates/task-core/src/project_wake.rs` の `ProjectBudgetToday` に `usd`・`usd_unknown_runs`・`price_table_version` を足し、`Eq` derive を外した（`usd` が f64 のため）。この型は task-api の `budget_today` 応答の型で、task-core にしか置き場がない。tokens は表示用に残した。
- この変更は WU の範囲 check（task-worker・task-api・docs・web・agent-docs のみ許可）に当たる。範囲 check の許可 path に `crates/task-core/src/project_wake.rs` を足す必要がある（plan_issue として申告）。

## 証拠（提出時の HEAD で実行）

- 範囲 check: `crates/task-core/src/project_wake.rs` を許可 path に加えた条件で範囲外 path なし。
- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy -p task-worker -p task-api -- -D warnings`: exit 0。
- `cargo nextest run -p task-api -p task-worker`: 2097 件 pass、11 件 skip。
- `cargo clippy --workspace -- -D warnings` は task-dispatch（後続 unit dispatch の担当。`tree_counters` の `&BudgetRates` 引数など 6 件）で落ちる。この unit の範囲外。
