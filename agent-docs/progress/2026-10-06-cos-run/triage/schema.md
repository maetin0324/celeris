# CoS triage schema・生成型・gui-api.md

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

## 完了

- `crates/task-api/src/schema.rs` の `ApiV1Schema` に triage の wire 型 7 つを登録した（`cos_inbox_item` = `CosInboxItem`、`cos_inbox_list` = `CosInboxList`、`cos_resolve_request` = `ResolveBody`、`cos_resolve_response` = `ResolveResponse`、`cos_escalation_packet` = `EscalationPacket`、`cos_override_request` = `OverrideBody`、`cos_override_response` = `OverrideResponse`）。resolve-api・override-api 葉が「schema 段で行う」と残した型の登録がこれ。
- `OverrideResponse` は response の wire 型として `crates/task-api/src/cos/override_op.rs` に新設し、handler の `serde_json::json!` 組立を同名の struct 化にした。JSON の欄・順序は変更なし（`operation_id`・`state`・`action`・`new_revision`・`new_wait_id`・`remediation_task_id`・`paused_task_ids`）。
- `crates/task-api/src/cos/mod.rs` の `triage_view` を `pub` にした（schema.rs が `crate::cos::triage_view::CosInboxItem` を参照するため）。
- ロジックは変えていない。crates の変更は schema の derive・doc comment への追加と、上記の response 型化のみ。

## 再生成の流水（KB schema-regeneration）

1. `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` → 成功（1 passed）。`docs/api/v1/api-v1.schema.json` を再生成（`CosInboxItem`・`CosInboxList`・`ResolveBody`・`ResolveResponse`・`EscalationPacket`・`EscalationOption`・`OverrideBody`・`OverrideMode`・`OverrideResponse` が `$defs` に現れる）。
2. drift 確認: `cargo test -p task-core --lib committed_schema`（6 passed）・`cargo test -p task-worker --lib committed_schema`（1 passed）→ 差分なし。`event.schema.json`・protocol schema は再登録の型に含まれないため変更不要。
3. `corepack pnpm@12.6.0 -C web gen:types` → `web/api/generated/schema.json` と `types.ts` を更新。`node web/scripts/gen-types.mjs --check` → exit 0。
4. `corepack pnpm@11.27.0 -C gui gen:types` → `gui/app/celeris/types.ts` を更新（`cos_escalation`/`cos_fallback` の union 順序の並び替えを含む生成差分）。`pnpm install --offline` は不要だった（corepack が node_modules を立てた）。
5. `sh scripts/sync-gui-docs.sh` → `gui/docs/celeris-api-v1.md` を正本から写し直し。`--check` で up to date。

## docs/api/v1/gui-api.md

- §2 の一覧に 199〜201 の 3 行（`GET /cos/inbox`、`POST /cos/inbox/{i}/resolve`、`POST /cos/operations/{o}/override`）を追記し、見出しの count を `201 = … + CoS triage 3` にした。
- §3.129「CoS 受信箱の一次対応と代答の取消（ADR 2026-10-06-cos-inbox-triage / 2026-10-05-cos-chat-home D3・D6）」を新設。3 route の認証・入力・成功・409/403/422 の条件・override の 3 経路（reversible の新待ち・不可逆の remediation task・消費済みの pause）を記す。
- 冒頭の改訂履歴に 2026-10-06 の 1 項目（追加のみ。エンドポイント 199〜201、`NotificationKind` に `cos_escalation`/`cos_fallback`、migration 0053 以降）を追記した。

## 証拠

- `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` → 1 passed（再生成）。
- `cargo test -p task-api --lib committed_schema_matches_generated` → 1 passed（drift なし）。
- `cargo test -p task-core --lib committed_schema` → 6 passed・`cargo test -p task-worker --lib committed_schema` → 1 passed（他 crate の schema drift なし）。
- `node web/scripts/gen-types.mjs --check` → exit 0。
- `sh scripts/sync-gui-docs.sh --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh` / `check-adr-numbers.sh` / `check-doc-layout.sh scripts/dev/docs-layout.tsv` → 全て exit 0。
- `cargo test -p task-api --test cos_triage_override` → 8 passed（response 型化でも JSON の形は不変）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt -p task-api --check` → exit 0。

## 未解決

- なし。generate 系の生成物はすべて再生成で揃った（gui 用の手反映しは不要だった）。
