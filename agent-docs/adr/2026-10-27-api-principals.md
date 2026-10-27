# ADR: API の主体を認証済み token ごとに区別する

- Date: 2026-10-27
- Status: Accepted
- Task: 01M5XXF7Q3N2619D5RQA5J7C7N

## 背景

管理 API の単一 Bearer を web gateway と外部 operator が共有し、API の変更を一律に human と記録している。
CoS の操作は cos_operations に thread が残るが、ドメイン event の by は cos に丸められる。
既存の監査記録は書き換えず、新しい要求から識別可能にする。

## 決定

1. `[api] token_file` を削除し、`[[api.tokens]]` の `principal` と `token_file` に置き換える。
   principal は `human:<name>` または `operator:<name>`。name は小文字英数字で始まる
   `[a-z0-9._-]` の 1〜64 文字。web は `human:web`、外部 operator は個別の名前を使う。
   未知の欄・空 token・同じ principal・同じ token の重複・CoS の予約 prefix を持つ token は起動時に拒否する。
   相対 token_file は config のディレクトリ基準。値を Debug・log・エラーに出さない。
2. guard が Bearer を照合し、`Principal` を request extension に載せる。主体を HTTP 本文や外部の
   actor header の自己申告から決めない。各 handler はこの主体を by / actor / source に伝える。
   CoS のポリシー上の区分（人か CoS か）と監査用の主体文字列を分け、既存の認可を広げない。
   token 無しの loopback は `human:local`、管理操作は従来どおり拒否する。不正な Bearer を人へフォールバックしない。
3. CoS run credential は既存の DB 検証を維持し、検証した thread_id から `cos:<thread_id>` を作る。
   run_id は extension と従来の cos_operations に保持する。CoS の直接 mutation 拒否は維持する。
   共通 operation 関数へは、通常 API の Caller または OperationAudit の検証済み thread から主体を渡す。
4. MCP の `api_get` は管理 token を共有しない。daemon が起動時に暗号学的乱数の内部 Bearer を生成し、
   API と MCP にだけ渡す。MCP は認証済み AuthedClient の id を `X-Celeris-Mcp-Client` に載せる。
   API は内部 Bearer が一致した場合だけこの header を使い、`mcp:<client_id>` とする。
   内部 Bearer の mutation は拒否する。既存の MCP scope・GET の path 拒否は維持する。
   MCP の専用 mutation ツールも既存の認証済み client id を用いた記録を維持する。
   ADR 2026-10-23-mcp-read-only-db-and-api D5 の管理 token 共有を、この内部認証に置き換える。
5. BFF は web 専用 token ファイル 1 本だけを読み、operator の token や daemon config の全 token 一覧を読まない。
   起動用 env は `CELERIS_WEB_API_TOKEN_FILE` に分離し、配備の web 起動スクリプトも同時に更新する。
   relay・SSE・Console・Browser Live の全経路が同じ web token を注入することを試験する。
6. 単一 API token の廃止に伴う別用途への流用も除去する。LLM proxy は独立した
   `[llm_proxy] token_file` を使い、langmem の proxy 接続もそれを参照する。
   本番 DB 保護判定は全 API token ファイルと proxy token を保護対象に含め、最初の 1 本だけに縮めない。
   CLI の server 用 token 上書き・release staging config・試験 fixture・現行文書も新形式へ更新する。
   CLI が API を呼ぶ際の「接続先 token ファイル」は単一のままでよい（その client 自身の principal の token）。
7. 本番変更はこの worker から行わない。operator は変更の直前に利用者へ変更内容と停止影響を知らせる。
   昇格前に task の進捗ファイルの「本番への移行」を確認し、告知・config と各 client の切替・確認・戻し方を
   一組で実施する。告知を省いた自動移行は作らない。

## 検証

- 2 本以上の管理 token で同じ操作を行い、DB の event に human:web / operator の別名が残る。
- MCP の認証済み client と CoS run の thread が request principal に反映され、別 client / thread が区別される。
- 未知 Bearer、偽の主体 header、内部 MCP token の mutation、失効 CoS credential を拒否する。
- 旧 config 欄、重複 token / principal、空 token を秘密を表示せずに拒否する。
- BFF の各転送経路、web 型検査・unit、影響 crate と逆依存、workspace clippy、fmt を実行する。
- 全体 nextest・doc test・web e2e・実 browser/userns は release gate のみ。

## 実装状況

- config `[[api.tokens]]`・`[llm_proxy] token_file`（`crates/celeris/src/config/api.rs`・`mod.rs`）、CLI の token 選択
  `ApiConfig::client_token`（`$CELERIS_API_PRINCIPAL` → 最初の operator → 最初の token）、verify の `--token-file` は
  `[[api.tokens]]` を `operator:verify` 1 本に置き換える。
- principal と guard（`crates/task-api/src/principal.rs`・`middleware.rs`）、MCP 専用 token（`daemon/run.rs` →
  `ApiSettings.mcp_token` と `celeris_mcp::tools::api::ApiTarget`）、BFF の `CELERIS_WEB_API_TOKEN_FILE`。
- by を principal にした handler: task の approve/reject/answer/comment/retry/patch、decisions の answer/withdraw/revise、
  execution の plan adopt / PUT / tree adopt / plan-gate / decompose、integrations、routing stage、browser site policy、
  CoS の各 op（`OperationAudit::by` = `cos:<thread_id>`）。
- 主体欄を持たない既存 event（phase-gate・cancel・POST /tasks 等）への新規監査欄追加は含めない。
  既存の by / actor / source は principal を渡す。

## main 同期時の追記: 昇格 API の監査

main に加わった配備監査にも認証済み principal を渡す。通常の昇格 API は human を
`human:<name>`、operator を配備監査 D2 の語彙 `external_operator:<name>` に変換する。
MCP・CoS・未認証はこの通常 API の昇格主体として拒否する。CoS は既存の operation 経路で
thread 種別から `cos_general` / `cos_project` を決める。固定 human の constructor は除去する。

一時停止・再開の通常 API も Caller を `plan_set_paused` へ渡し、既存 Edited.by の固定 human を除く。
