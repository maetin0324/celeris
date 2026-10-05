---
tasks: [01M46XAR6KD97KMWBX2D6WW6TJ]
unit: schema
status: done
completed: 2026-10-05
---
# CoS chat の schema 登録と生成型の再生成

ADR 2026-10-05-cos-chat-home D2 の wire 型を `ApiV1Schema` に登録し、schema と gui/web の生成型を再生成した。

## 変更

- `crates/task-api/src/schema.rs`: `task_core::chat` の `ChatThread`・`ChatMessage`・`ChatRun`・`ChatAttachment`・`ChatCard`・`ChatEvent` と、request 6 種・response 9 種（一覧・詳細・stop・events・添付・reference を含む）を `ApiV1Schema` の欄に追加。
- `docs/api/v1/api-v1.schema.json`: `UPDATE_SCHEMA=1 cargo test -p task-api` で再生成。差分は追加のみ（`required` 末尾の `cron_run_result` のカンマ 1 行を除く）。
- `gui/app/celeris/types.ts`・`web/api/generated/types.ts`・`web/api/generated/schema.json`: `gen:types` で再生成。
- `docs/api/v1/gui-api.md`: §2 にチャットの endpoint 18 行（#178〜195）、§3.127 に REST の契約（冪等・上限・エラー・添付・停止）、§4 にチャット stream の event 表と再接続の規則を追記。
- `gui/docs/celeris-api-v1.md`: `scripts/sync-gui-docs.sh` で写した。

pnpm は `corepack pnpm@11.27.0 -C gui install --offline` と `corepack pnpm@12.6.0 -C web install --offline` を先に実行した（どちらも exit 0）。

## 確認

- `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema`: exit 0（再生成）。
- `cargo test -p task-api --lib schema::`: 2 passed（`committed_schema_matches_generated`、`schema_uses_defs_once_for_shared_types`）。rustfmt 後の再実行。
- `docs/api/v1/api-v1.schema.json` に `"ChatThread"` が 1 件入ることを確認。
- `node web/scripts/gen-types.mjs --check`: exit 0。
- `node gui/scripts/gen-types.mjs --check`: exit 0。
- `sh scripts/sync-gui-docs.sh --check`: exit 0（up to date）。
- `sh scripts/dev/check-doc-links.sh`: `check-doc-links: ok`、exit 0。写し先の ADR 相対リンクが `gui/docs/` から repo 内の `agent-docs/adr/2026-10-05-cos-chat-home.md` に解決することも確認。
- `rustfmt --edition 2024 crates/task-api/src/schema.rs`: 整形のみ適用。

## 未解決・申し送り

- §3.127 の endpoint は api-threads・api-attach・api-sse の実装どおり（18 本）。ADR の `/cos/inbox`・`/cos/threads/{t}/checkpoint`・`/cos/operations` 系は cos-run・受信箱の段で追加する。その時は §2 の表と §3 の番号を続けて足す。
- 503（CoS 無効時の新規送信）は ADR に書いてあるが、本段では `ChatState::enabled` の判定点を cos-run の段へ残しており、文書にも「cos-run の段で入れる」と書いた。
- `ChatEventData` は untagged の enum なので、schema 上は oneOf で表される。生成型（web）でも同じ形になる。
