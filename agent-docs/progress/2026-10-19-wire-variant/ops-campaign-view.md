---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: TaskDetailに解決済campaignを公開（ops-campaign-view）
work_unit: ops-campaign-view
status: done
updated: 2026-10-19
---
# ops-campaign-view

## 変更（crates/task-ops のみ）
- `TaskDetail.campaign: Option<CampaignIdentity>`（`skip_serializing_if = none`）。`task_detail` が既存 `store.task_campaign_resolve` を呼び、全祖先を検査。`StoreError::Invalid`（矛盾・循環・保存値不正）は `OpsError::Store` で返し、None に丸めない。`Task` 本体に欄は増やしていない。
- `TaskDetail` の構築箇所は `task_detail` の 1 箇所のみ（所有 literal はそれ）。
- 試験（view/tests.rs）: root と子孫で同じ識別（variant・構成 pin 込み）、campaign 無し task は欄が JSON に出ない、祖先矛盾（別 connection で直接挿入）は `Store(Invalid)`（root 自身は読める）。

## 証拠
- `cargo test -p task-ops task_detail` → 20 passed（新規 2 件含む）
- `cargo test -p task-ops` → 556 passed / 0 failed
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0

## 未解決・次
- `cargo test -p task-api` の `committed_schema_matches_generated` は schema drift で落ちる（想定どおり）。api-schema WU が `UPDATE_SCHEMA=1` で再生成する。本 WU では生成物に触れていない。
