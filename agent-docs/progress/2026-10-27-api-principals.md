---
title: API の主体を token ごとに区別する
updated: 2026-10-27
status: done
tasks: [01M5XXF7Q3N2619D5RQA5J7C7N]
---
# API の主体を token ごとに区別する

完了日: 2026-10-27。
基準: `git merge-base HEAD main`。

## 変更

- [ADR](../adr/2026-10-27-api-principals.md)。
- `[api] token_file` を削除（読み込みエラー）し `[[api.tokens]]`（`principal = "human:<name>" | "operator:<name>"`、
  `token_file`）を追加。重複・空・CoS 接頭辞の token を拒否。llm-proxy は `[llm_proxy] token_file`。
- guard が bearer から principal を決め request extension に載せる。`human:<name>` / `operator:<name>` /
  `mcp:<client_id>`（起動ごとの MCP 専用 token + `X-Celeris-Mcp-Client`、変更系は 403）/ `cos:<thread_id>`。
- handler の `by`・`actor` を principal に変更（ADR の実装状況の一覧）。
- BFF は `CELERIS_WEB_API_TOKEN_FILE` だけを読む（旧 `CELERIS_API_TOKEN_FILE` が設定されていれば起動エラー）。
- db guard は全 token file と proxy token を保護（`DaemonPaths.token_files`）。
- 試験: `crates/task-api/tests/api_principals.rs`（2 本の token・MCP・2 つの CoS thread・偽の名乗り・未知 bearer）、
  `handlers/tests.rs` の `resolve_principal_distinguishes_config_tokens_and_mcp_clients`、config の新形式の試験。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-api --test api_principals` → 4 passed。
- 必須検査の結果は報告本文と result.json に記す（test-affected / clippy / fmt）。

## 本番への移行

operator が昇格前に読む手順。**この run は本番 config・DB・service・credential を変更していない。**
token の値は shell 引数・log・画面に出さない。

1. **告知（必須）**: operator は変更の直前に利用者へ「共有 API token を web（human:web）と operator に分ける。
   llm-proxy の token は独立させる（値は今の api.token を引き継ぐ）。切替中は API と web が一時的に使えない」と知らせる。
2. バックアップ（0600）: `~/.config/celeris/config.toml`、`api.token`、`web.env`、live の
   `~/.config/systemd/user/celeris-web@.service`。旧 token は消さない（下の 3 で proxy 用に残る）。
3. token の用意（`cd ~/.config/celeris`、`umask 077`）:
   - `cp -p api.token llm-proxy.token`（proxy の bearer を変えない。LDR の secret・langmem の api_key_secret はそのまま通る）
   - `head -c 32 /dev/urandom | base64 | tr -d '\n' > web.api-token.new && mv -n web.api-token.new web.api-token`
   - `head -c 32 /dev/urandom | base64 | tr -d '\n' > api.token.new && mv api.token.new api.token`（operator の新 token。
     selfdeploy の `SD_API_TOKEN_FILE` と celerisctl はこの path を読み続ける）
4. 新 config を `config.toml.new` に作る: `[api]` の `token_file` 行を消し、次を足す。

   ```toml
   [[api.tokens]]
   principal = "operator:claude-code"
   token_file = "api.token"

   [[api.tokens]]
   principal = "human:web"
   token_file = "web.api-token"

   [llm_proxy]            # 既存の節に 1 行足す
   token_file = "llm-proxy.token"
   ```

5. 検証: `CELERIS_CONFIG=~/.config/celeris/config.toml.new scripts/selfdeploy/verify.sh <sha12>`。
   N-1（現行 release）は新形式の config を読めず起動に失敗する見込み（旧形式は新 binary が拒否するため、両方が読める
   config は無い）。verify がこれで落ちる場合の扱いは、operator が利用者に知らせて決める。
6. web の env: live unit と `web.env` から `CELERIS_API_TOKEN_FILE` を消し、
   `CELERIS_WEB_API_TOKEN_FILE=%h/.config/celeris/web.api-token` にする（repo の `deploy/systemd/celeris-web@.service` と同じ）。
   旧 env が残っていると BFF は起動しない。
7. 切替: `mv config.toml.new config.toml` の直後に通常の昇格（`promote.sh <sha12>`）、web を再起動。
8. 確認: `GET /api/v1/health`、web の画面、`celerisctl` の読み取り、MCP の `api_get`。確認用 task に web と
   operator からコメントを 1 件ずつ書き、`GET /api/v1/tasks/<id>/comments` の author が `human:web` /
   `operator:claude-code` に分かれること。CoS の操作は `cos:<thread_id>`。
9. 戻し: 利用者に知らせ、旧 release と 2 のバックアップ（config・api.token・web.env・unit）を一緒に戻す。
   新形式 config のまま旧 binary を起動しない。DB の events は書き換えない（既存の `human` 名義はそのまま）。

## 未解決

- phase-gate・cancel・task pause/resume（人の経路）・POST /tasks・project 系など、task-ops の中で `"human"` を
  固定している経路は今回変えていない（主体を引数に通す変更が task-ops に広がるため）。次の task で principal を通す。
- celerisctl `routing stage set` は DB を直接書くので actor は `human` のまま。
- web BFF の全経路（relay・SSE・Console・Browser Live）は同じ `daemonTokenFile` 1 本を使う構造で、経路ごとの
  新しい試験は足していない。

## 提案

- 既存 events の `human` 名義（棚卸しで operator と判明した分）は書き換えず、研究評価では日付で区切って扱う。
