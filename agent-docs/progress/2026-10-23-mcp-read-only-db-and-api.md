---
title: chatgpt-rdc に読み取り限定の db_query と api_get を MCP ツールとして渡す
tasks: [01M5MWG7FB4V940XSF0C4F93CM]
status: done
updated: 2026-10-23
completed: 2026-10-23
---
# chatgpt-rdc の読み取り限定 DB 問い合わせと API GET

ADR: `agent-docs/adr/2026-10-23-mcp-read-only-db-and-api.md`（ADR-0056 Phase 101 追記の「DB 直読み・汎用 HTTP 禁止」を置き換え。
0056 の該当箇所に置き換えの付記を書いた）。

## 変更
- `crates/task-core`: `McpScope::DbRead`（`db:read`）・`McpScope::ApiRead`（`api:read`）。どちらも既定 scope に入れない。
  `McpCall.request` と migration 0093（`mcp_calls.request`。SCHEMA_VERSION 93）。`McpClientStore::mcp_client_set_scopes`。
- `crates/celeris-mcp/src/tools/db.rs`: `db_schema` / `db_query`。呼び出しごとに `file:<db>?mode=ro` + `query_only` の別接続、
  authorizer（SELECT・READ・RECURSIVE・関数（`load_extension` 等を除く）・読み取りの PRAGMA 7 種だけ）、1 文だけ、`readonly()`、
  行数 500/5000、結果 1 MiB、値 16 MiB、10 秒（progress handler）、同時 2 本。秘密の列 11 個は `SQLITE_IGNORE` で NULL
  （`MASKED_COLUMNS`。ADR D3 の表）。
- `crates/celeris-mcp/src/tools/api.rs`: `api_get`。`/api/v1/` の下・traversal なし・GET だけ・拒否リスト（`DENIED_PATHS` /
  `DENIED_SEGMENTS`。ADR D5 の表）。応答 1 MiB・30 秒、redirect を追わない。daemon（`crates/celeris/src/daemon/services.rs`）が
  `[api] listen` と `[api] token_file` から宛先を入れる。
- `crates/celeris-mcp/src/rpc.rs`・`tools/mod.rs`: `db_query` の SQL と params、`api_get` の path と query を `mcp_calls.request` に
  4000 文字まで記録（結果は残さない）。流量制限は既存の `rate_limit_per_min`。
- `celerisctl mcp client scopes <id> --scope …`（token を変えずに scope を置き換える）。
- 文書: `docs/guides/mcp.md` §3〜§8（scope 表・道具・監査・§8 を新方針に書き換え、旧い「DB 直接アクセス無し・汎用 curl を叩かせない」の
  説明を削除）、`docs/api/v1/api.md`（`McpCall.request`）、ChatGPT 向けの使い方 `docs/guides/chatgpt-rdc.md`（新規。skill の置き場が
  無かったので docs/guides に置き、setup スクリプトが専用ユーザーの `~/CELERIS-CHAT.md` に写す）。
- `scripts/rdc/celeris-chat`（説明と例）、`scripts/rdc/setup-chatgpt-rdc.sh`（使い方の写し・scope 付与の手順・動作確認の例）。
- web: 生成型（`McpCall.request`）と MCP client 画面の呼び出し履歴に `request` を表示。

## 試験
- `crates/celeris-mcp/src/tools/db_tests.rs`: 書き込み 12 種・ATTACH・PRAGMA 書き込み・`load_extension`・2 文目の拒否、読み取りの PRAGMA、
  秘密の列が WHERE・副問い合わせでも NULL、行数・結果サイズ・時間（終わらない再帰 CTE を 200 ms で中断）の上限、読み取り中も
  daemon の WAL 書き込みが待たない、伏せる列が schema に実在、`docs/guides/chatgpt-rdc.md` の SQL 例が全部通る。
- `crates/celeris-mcp/src/tools/api_tests.rs`: GET 以外・`/api/v1` 外・traversal・拒否リストの拒否、query の型。
- `crates/celeris-mcp/tests/mcp_integration.rs`: scope の無い client には一覧に出ず `-32601`、db_query の伏せ字・書き込み拒否と
  `mcp_calls.request`、api_get が偽 API（loopback）に daemon の token で GET し、拒否した要求は偽 API に届かない。
- `crates/celerisctl/tests/mcp_client_scopes.rs`: scope の置き換えで token hash が変わらない。

## 検査（提出時の HEAD で実行）
- `cargo fmt --all -- --check`: exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0
- `bash scripts/dev/test-affected.sh`: 結果は完了報告に記載

## 本番への移行（人が行う）
scope と token は人だけが扱う。daemon の再起動（この変更を含む release の昇格）の後に:

1. 既にある `chatgpt-rdc` client に scope を足す（token はそのまま。scope は置き換えなので全部書く）:
   ```
   ~/.local/celeris/current/bin/celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client scopes chatgpt-rdc \
     --scope knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read
   ~/.local/celeris/current/bin/celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client ls   # scopes に db:read,api:read
   ```
   作り直す場合は `mcp client revoke chatgpt-rdc` の後に `mcp client add chatgpt-rdc --scope …`（同じ scope）で token を
   発行し、600 のファイルに保存して 2. に渡す。
2. 専用ユーザーの celerisctl・celeris-chat・使い方を新しくする: `bash scripts/rdc/setup-chatgpt-rdc.sh [TOKEN_FILE]`
   （token を作り直したときだけ TOKEN_FILE を渡す）。
3. 確認: `sudo -iu chatgpt-rdc -- celeris-chat db_query '{"sql":"SELECT count(*) FROM tasks"}'` と
   `sudo -iu chatgpt-rdc -- celeris-chat api_get '{"path":"/api/v1/health"}'` が JSON を返し、
   `GET /api/v1/mcp/calls?client=chatgpt-rdc` の行に `request` が出る。
- `[api] listen` が無い構成では `api_get` は「API の宛先が設定されていません」を返す（`db_query` は動く）。
- DB の migration 0093 は daemon の起動時に当たる（`mcp_calls` に NULL 可の列を足すだけ）。

## 未解決事項
- 実機（RDC 経由の ChatGPT）での確認は未実施（人の端末と RDC の対が要る）。上の 3. の手順で確認する。

## 提案
- `Event` に秘密の値を運ぶ欄を足す変更では、ADR 2026-10-23-mcp-read-only-db-and-api の D3/D4 の表と `MASKED_COLUMNS` を同時に
  見直す（reviewer の確認項目に入れるとよい）。
