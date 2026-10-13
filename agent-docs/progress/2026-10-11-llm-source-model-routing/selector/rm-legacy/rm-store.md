---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
unit: rm-store
status: blocked (plan_issue: web view が範囲外)
date: 2026-10-13
---
# rm-store: store の shadow 予約と表を消す

## したこと
- `crates/task-core/src/store/routing_shadow.rs`・`routing_shadow_tests.rs` を削除。`SqliteStore` の shadow 予約の method（予約・確定・利用量・監査照会）と `ShadowReservationAudit` が無くなった。
- migration `0075_drop_routing_shadow_reservations.sql`: `idx_routing_shadow_reservations_day` と `routing_shadow_reservations` を drop。0049 は書き換えない。events は触らない。
  - 採番: `git for-each-ref refs/heads refs/remotes` の全 head の `crates/task-core/migrations/` を走査し、最大は 0074（他 branch に 0075 以上なし）。
  - `SCHEMA_VERSION` 74→75、固定値の試験（store::tests・routing_log_tests・routing_evidence・route_records・delivery・cluster_job・cron）を追随。旧 DB 再現試験の `DROP TABLE routing_shadow_reservations` は新しい DB に表が無いので外した。
  - 新試験 `store::routing_log_tests::migration_0075_drops_shadow_reservations_and_keeps_shadow_events`: 版 74 の DB に予約行と旧 `routing_shadow_recorded` event を入れ、開くと表・index が消え event は読めることを確かめる。
- task-ops `RequestLog::shadow_reservation` と `RoutingShadowAudit.reservation` を削除（予約の表が無いので監査で表示できない）。旧 event の `reservation_id` は history 型で読める。
- task-api 試験 `routing_shadow_audit_keeps_primary_outcome_separate` から予約の作成を外し、`reservation` 欄が出ないことを確かめる形にした。
- `UPDATE_SCHEMA=1` で docs/api/v1/api-v1.schema.json、`gen:types` で web/api/generated を再生成（`ShadowReservationAudit` と `reservation` が消えた）。

## 証拠
- `cargo test -p task-core --lib` → 941 passed
- `TMPDIR=<短い dir> cargo test -p task-api && cargo test -p task-ops --lib && cargo test -p celerisctl --test no_migrate` → exit 0（84 結果行・1201 passed）。長い TMPDIR だと browser_live_cross 3 件が SUN_LEN で落ちる（環境要因、変更と無関係）
- `cargo check --workspace --all-targets && cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- gen:types 後の `web/api/generated` は再生成と一致

## 未解決（plan_issue）
- `web/features/tasks/routing-audit-view.tsx`（227・244–251 行）と `routing-audit-view.test.tsx`（127・136・141 行）が `RoutingShadowAudit.reservation` を表示・試験している。生成型から欄が消えたので `pnpm -C web typecheck` が TS2339 で落ちる。web/features/ は本葉の許可範囲外。
- 直し方: ShadowRow の予約表示（「上限消費 …」の 2 分岐）を消し、test の 3 行の期待を外す。範囲に `web/features/tasks/` を足した修正葉（check: `corepack pnpm@12.6.0 -C web typecheck && corepack pnpm@12.6.0 -C web test`）を置く。
