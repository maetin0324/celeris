---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: audit-api
status: done-in-branch
completed: 2026-10-05
---

# Phase 3 audit-api: routing audit に request 子 trace・実 source・escalation・遅延 outcome を結合する

run ごとの routing audit に次を結んだ。dispatch の `RoutingDecided`、`routing_features_recorded`（dispatch 時点）、`routing_request_decided`（proxy の子 trace。dispatch で未確定だった実際の model/source を含む）、escalation の理由（`escalation_audit`）、最新の（supersede されていない）`routing_outcome_recorded`。`GET /api/v1/tasks/{id}/routing` の run object に出る。規則の詳細は ADR 付記「Phase 3 audit-api unit の実装済み範囲」に書いた。

- task-core `routing_audit.rs`: `decision_id`・`escalation_audit`・`routing_features`・`routing_outcome`・`outcome_state`（`not_recorded` / `unreviewed` / `judged`）。
- task-ops `routing_audit.rs`: `routing_request_decided` を run_id か parent decision で run に結ぶ（推定はしない）。Phase 2 の同じ要求の子があればそこへ足す。子の `actual`（proxy log → 試した source → trace）、run の `actual_sources`、`request_trace_missing`。
- task-api: 応答型はそのまま `RunRoutingAudit` を返す（doc comment を更新）。API schema を再生成し、web の写しを更新した。
- task-core `model_router/tests.rs` の `routing_old_events_deserialize_without_optimizer`: trace に由来する新欄（`decision_id`・`outcome_state`）も比較の前に落とすようにした。

## 検証

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-api --test routing` | 4 passed（`routing_audit_links_run_request_and_actual_source`・`routing_is_empty_without_runs_and_404_for_unknown_tasks` を含む） |
| `cargo test -p task-ops --lib routing_audit` | 4 passed（新 `request_records_link_to_runs_with_actual_source`） |
| `cargo test -p task-core --lib routing_audit` | 3 passed（新 `joins_features_escalation_and_latest_outcome_per_run`、回帰 `joins_routing_usage_wall_retries_and_review_per_run`） |
| `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` → `cp docs/api/v1/api-v1.schema.json web/api/generated/schema.json` | 再生成後に UPDATE なしの全体試験で `committed_schema_matches_generated` が通る。`event.schema.json` の差分は 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0。3988 tests run: 3988 passed (1 slow), 12 skipped。doctest exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

## 未解決

- gui `types.ts` と web の生成型（`web/api/generated/types.ts` など）は再生成していない（surface 段の gui/web unit が担当）。
- proxy の trace を持たない `routing_request_decided` は子 trace にしない（`request_trace_missing` だけ残す）。子 trace の `trace` 欄を必須のままにして、gui の既存の読み方を変えないため。

## 提案

- `routing_request_decided` の trace を常に必須にするか、子 trace の `trace` を Option にして gui を合わせるかは、daemon-wire の sink 実装が固まった時点で決める。
