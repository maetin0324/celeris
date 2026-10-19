---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 既存matching前の専用構成・assignee検査（dispatch-pin）
work_unit: dispatch-pin
status: done
updated: 2026-10-19
---
# dispatch-pin

## 変更（task-dispatch のみ）
- `dispatcher/variant_pin.rs`（新）: `check_assignee`（割当先が pin の `nodes` に含まれ、node が根＋単一課なら `coding_node` であること）、`Dispatcher::variant_org_pin`（`task_campaign_resolve` の `variant_org`。解決 `Err` は黙って通常扱いにせず理由にする）、`block_variant_task`（`ready → blocked`＋`QuestionRaised`「variant pin check failed; not assigned: …」。approval は作らない）。
- `dispatch_run.rs` `assign_if_needed`: `org_list()` 取得後、(1) pin 解決 (2) `CampaignOrgPin::check_org(&org)` (3) 既存 `task_ops::matching::decide` をそのまま呼ぶ (4) `Assigned` の node または `NotApplicable` での既存 assignee（retry）に `check_assignee`。失敗は blocked＋理由 event で割当てない。variant なしの task は経路が変わらない。共有 org は読むだけ。
- 全 node digest（`org_effective_digest`）は観測証拠で、非対象不変の証明には使っていない（campaign-proof が封印期待値と照合する）。run 開始時の再照合・RunContext への記録は worker-pin / dispatch-lineage の範囲で、本葉では未実装。

## 試験（`dispatcher/tests/variant_pin.rs`、8 件）
正: spec-off が単一課へ割当、spec-on は matching 通り割当で org 不変、retry の既存 assignee が coding_node なら保持、子が祖先の pin を継ぐ、variant なしは従来どおり。
負: node 追加 drift、coding 課 profile drift、孫の drift、retry assignee が単一課外 → いずれも blocked＋理由 event で assignee なし。

## 証拠
- `cargo test -p task-dispatch variant_pin` → 8 passed
- `cargo test -p task-dispatch` → 957 passed、0 failed
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` では既存の `tests/trial_context.rs:223,231` の type_complexity が出る。本葉の変更外）
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` は統合葉に委ねた（未実行）

## 未解決事項
- blocked の理由 event は `QuestionRaised`（既存 variant）を使う。専用の event 種を足すかは未決（wire 変更になるため見送り）。
