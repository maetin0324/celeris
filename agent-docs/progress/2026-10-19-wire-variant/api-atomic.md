---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 通常POST・GETの原子性と冪等性（api-atomic）
work_unit: api-atomic
status: done
updated: 2026-10-19
---
# api-atomic

## 変更
- task-core `store/org.rs`: `campaign_org_check_tx(conn, pin)`（transaction 内で現 org を `CampaignOrgPin::check_org` に照合）。
- task-core `store/decision_log.rs`: `task_campaign_resend_org_checked`（再判定と org 照合を同じ store lock で。`Same` でも drift は `Invalid`）。
- task-api `handlers/tasks.rs`: 既存の resend → transaction 内再判定 → `task_campaign_set_tx` は維持。新規作成は同じ transaction 内（set_tx の直前）で org 照合し、drift は 409 `campaign_org_drift`（task/events/key/識別は rollback で残らない）。同一冪等再送も drift なら 409。片欠けは 400、別 hash/構成脱落は既存 409。
- GET は `TaskDetail.campaign`（ops-campaign-view）。祖先矛盾は `task_campaign_set_tx` の `Invalid` が problem 応答。

## 試験（実 store、LLM なし）
- `crates/task-api/tests/campaign_variant_atomic.rs` 3 件: 正（201→GET→同一再送 200・件数不変）、drift（再送 409・新規 409・残留ゼロ）、片欠け 2 通り 400、別 content/脱落/pin 変更 409、node 脱落 4xx。残留は tasks/events/task_campaigns 件数で確認。
- `cargo test -p task-api --test campaign_variant_atomic --test campaign_ident` → 3 passed / 1 passed
- `cargo test -p task-core campaign` → 12 passed
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0

## 未解決・注意
- L1 は封印期待値を持たないので、この API 照合を非対象全体の適用証明にはしない（run 開始時の再照合は dispatch-pin 葉）。
- 祖先矛盾の API 経由の試験は task-core の既存試験（campaign_variant_tests）に依存。
- test-parallel.sh は統合葉に委ねた。
