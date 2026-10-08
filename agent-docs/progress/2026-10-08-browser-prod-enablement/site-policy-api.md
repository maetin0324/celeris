---
task: browser-prod-enablement
wu: site-policy-api
status: done
completed: 2026-10-08
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

# site-policy-api: site policy の DB 正本・API、grant の credential 設定 API（ADR 2026-10-08 D3）

## したこと
- migration **0060** `browser_site_policies`（正本）と `browser_site_policy_events`（追記専用の監査）。
  全ブランチを走査して 0059 が `celeris/01M4CAKADG…`（cos_inbox_item_summary、main 未取り込み）で使われていたので 0060 を取り、
  59 を `RESERVED_VERSIONS` に足した（その branch を取り込むときに RESERVED から外して両方登録する）。`SCHEMA_VERSION = 60`。
- task-core `store/site_policies.rs`: list / get / upsert / delete（grant と未解決 wait の参照があれば `InUse`）/ seed（無い id だけ `source=config`）。
- task-api `browser_site_policies.rs`: `GET /api/v1/browser/site-policies`、`PUT|DELETE /api/v1/browser/site-policies/{policy_id}`（admin token）。
  422 `site_policy_invalid`（ADR-0110 D2 の検証、`reason`）、409 `site_policy_in_use`（`node_ids`・`wait_ids`）、404 `site_policy_not_found`。
- broker control: `UnixCredentialBrokerControl.site_policies` を `SitePolicies(Arc<dyn SitePolicyLookup>)` に替え、本番は
  `StoreSitePolicies`（DB を登録のたびに読む）。`credential_policy()` に判定を切り出した。daemon は config の種を起動時に検証→seed。
- `PATCH /org/{id}/browser-settings`: `credential_use: bool` を追加（Phase 1 集合を実体化して `credential_use` を足す/外す）。
  `credential_policy_ids` は実在しない id で 422 `unknown_site_policy`。承認方針は不変（`ALWAYS_APPROVED`）。
- `UPDATE_SCHEMA=1` で `docs/api/v1/api-v1.schema.json` 再生成、web（`pnpm -C web gen:types`）・gui（`node scripts/gen-types.mjs`。pnpm 版不一致のため直接）の生成型更新。
  `docs/api/v1/gui-api.md` §3.132 を追加。
- 版数固定の試験（`SCHEMA_VERSION, 58` 12 箇所・chat の `assert_eq!(version, 58)`・delivery.rs の版数一覧に 60）を更新。

## ADR との差（付記の提案）
- ADR は `Event::BrowserSitePolicyChanged` を書くが、site policy には task_id が無く events 表は task 単位なので、
  `org_browser_events`（0051）と同じく別表 `browser_site_policy_events` に追記した（並行する ledger-gate 葉の Event 追加との衝突も避ける）。
  close-out で ADR に付記すること。

## 証拠
- `cargo test -p task-api --test browser_site_policy_db` → 5 passed（`browser_site_policy_db_api_changes_reach_broker_without_restart` ほか）
- `cargo nextest run -p task-core` → 868 passed（`browser_site_policy_db_delete_refuses_open_wait_reference`、migration 版数の試験を含む）
- `bash scripts/dev/test-parallel.sh` → exit 0、4720 passed, 13 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0
- `pnpm -C web typecheck` → exit 0、gui `tsc --noEmit` → exit 0

## 未解決
- web の `/browser/settings` 画面（web-site-policy 葉）と doctor の config/DB 差の WARN（preflight 葉）は後続。
- `POST /org`・`PATCH /org/{id}` の profile 経由の `credential_policy_ids` は実在検査していない（ADR は browser-settings だけを定める）。
