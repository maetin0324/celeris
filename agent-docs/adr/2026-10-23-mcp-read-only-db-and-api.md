# ADR 2026-10-23: MCP に読み取り限定の DB 問い合わせ（db_query）と API の GET（api_get）を足す

## 背景
ChatGPT（Remote Desktop Commander 経由。専用 Linux ユーザー `chatgpt-rdc` → `scripts/rdc/celeris-chat` →
`celerisctl mcp call` → `127.0.0.1:18200/mcp`）は、Celeris を MCP の固定ツールでしか見られない。
ADR-0056 Phase 101 追記と `docs/guides/mcp.md` §8 は「DB の直読みも汎用 HTTP（curl）も渡さない」と決めていた。
2026-10-23 に利用者が「ルーティング（Jev の入出力・lane・evidence）などの調査を ChatGPT 側でも本番データで行いたい。
読み取りの範囲で sqlite を自由に見せ、celeris の API を自由に呼べるようにしたい」と依頼した。固定ツールは
タスク・知識・組織の要約しか返さず、`decision_log_*`・`routing_evidence`・`events` の生の行は見えない。

## 決定
ADR-0056 Phase 101 追記の「DB 直読み禁止・汎用 HTTP 禁止」を、**MCP の中で読み取りに限って解く**決定で置き換える。
`chatgpt-rdc` の Linux ユーザーに DB ファイルの権限や curl を渡す方針には戻さない（境界は引き続き Linux ユーザーと
MCP client の scope）。

- D1: scope を 2 つ足す。`db:read`（`db_schema` / `db_query`）と `api:read`（`api_get`）。どちらも
  `McpScope::DEFAULT` に入れない。付与は人が `celerisctl mcp client scopes`（新設。scope の置き換え）か再発行で行う
  （credential・scope は人だけが扱う）。
- D2: `db_query { sql, params?, limit? }`。daemon の store とは**別の接続**を呼び出しごとに開く
  （`file:<db>?mode=ro` + `SQLITE_OPEN_READ_ONLY`、`PRAGMA query_only=ON`、busy_timeout 2 秒）。
  - SQL は 1 文だけ（`rusqlite::Batch` で 2 文目があれば拒否）。準備した文が `readonly()` でなければ拒否。
  - authorizer（`sqlite3_set_authorizer`）で次だけを許す: `SELECT`、`READ`、`RECURSIVE`（WITH RECURSIVE）、
    関数呼び出し（`load_extension`・`fts3_tokenizer`・`readfile`・`writefile`・`edit` を除く）、
    PRAGMA は `table_info` / `table_xinfo` / `index_list` / `index_info` / `index_xinfo` / `foreign_key_list` を値の
    代入なしで。それ以外（INSERT/UPDATE/DELETE、ATTACH/DETACH、他の PRAGMA、CREATE/DROP（temp を含む）、
    ALTER、ANALYZE、REINDEX、トランザクション・SAVEPOINT、仮想表の作成）は拒否。
  - 上限: 行数は既定 500・最大 5000（`limit+1` 行を読んで `truncated` を返す）。結果の JSON は 1 MiB まで
    （超えた行で切り、`truncated_reason: "bytes"`）。値の長さは `SQLITE_LIMIT_LENGTH` で 16 MiB まで。
    実行時間は 10 秒（progress handler が 1000 VM 命令ごとに締切を見て中断）。同時実行は 2 本まで（超えたら
    `-32002`）。BLOB は中身を返さず `{"blob_bytes": n}`。
  - WAL の読み取り接続は書き込みを止めない（読み手は自分の snapshot を読むだけで、daemon の書き込みは待たない）。
    時間上限で読み取り transaction の長さも 10 秒に抑え、checkpoint を長く阻まない。
- D3: 秘密の列は authorizer の `SQLITE_IGNORE` で **NULL に置き換える**（WHERE・JOIN の中でも NULL として評価
  されるので、値の推測にも使えない）。伏せる列（`crates/celeris-mcp/src/tools/db.rs` の `MASKED_COLUMNS` が正本。
  `db_schema` の出力の `masked` にも同じ一覧が出る）:

  | 表 | 列 | 理由 |
  |---|---|---|
  | `mcp_clients` | `token_hash` | MCP の Bearer token の hash |
  | `cos_run_credentials` | `token_hash` | CoS run の credential token の hash |
  | `browser_trusted_devices` | `secret_hash`, `prev_secret_hash` | GUI/web の信頼端末の秘密（ログインの代わり）の hash |
  | `browser_identities` | `sealed_blob` | 保存した browser の login 状態（cookie 等）の封印 blob |
  | `browser_approvals` | `owner_session_hash`, `nonce` | 承認の持ち主 session の hash・再利用防止の nonce |
  | `browser_waits` | `resume_key` | 待ちの再開に使う鍵 |
  | `browser_control_actions` | `command_json` | 操作の引数（入力した文字列を含み得る） |
  | `browser_live_events` | `body_json` | Live View の出来事の本文（画面の内容を含み得る） |
  | `node_sessions` | `session_id` | harness の会話 session の再開鍵 |

  表ごと伏せるものは無い（上の表の他の列は id・時刻・状態で、秘密ではない）。DB に**無い**秘密: API の Bearer
  token（`[api] token_file`）、credentiald の保存 credential の値（credentiald の別 store。DB の
  `browser_credentials`・`credential_waits` は id と状態だけ）、web gateway の session と password（`web/server` の
  env・file）、MCP token の値（DB は hash だけ）。
- D4: `events.payload` は伏せない。`Event` の型（`crates/task-core/src/model/events.rs`）には token・password・
  cookie の値を運ぶ欄が無い（`credential_id` は id）。人や worker が書いた自由文（コメント・回答・stdout の末尾）に
  秘密が入り得るのは `tasks_get`・GUI と同じで、db_query で新たに増える露出ではない。秘密の値を運ぶ欄を Event に
  足すときはこの ADR の表に足す（その変更の review で見る）。
- D5: `api_get { path, query?, method? }`。daemon の中から `[api] listen` の loopback（`0.0.0.0` は `127.0.0.1`
  に直す）へ、`[api] token_file` の token で GET を代行する（llm-proxy と同じ経路）。`chatgpt-rdc` に token は渡らない。
  - `method` は省略か `GET` だけ。`path` は `/api/v1/` で始まり、`?`・`#`・`%`・`\`・制御文字・空の段・`.`/`..` の段を
    含まない（query は `query` に object で渡す。値は文字列・数・真偽）。
  - 拒否リスト（`crates/celeris-mcp/src/tools/api.rs` の `DENIED_PATHS` / `DENIED_SEGMENTS` が正本）:

    | path | 理由 |
    |---|---|
    | `/api/v1/secrets/**` | secret の目録と値 |
    | `/api/v1/browser/credentials/**` | 保存 credential |
    | `/api/v1/browser/identities/**` | 保存した login 状態（cookie） |
    | `/api/v1/tasks/*/browser/live/**` | Live View の映像・出来事・読み取り |
    | `/api/v1/tasks/*/browser/waits/*/credential` | credential の受け渡し |
    | `/api/v1/clusters/*/connect/**` | TOTP の接続 prompt |
    | `/api/v1/llm/sources/*/accounts/*/login/**` | アカウントの login 手続き（code） |
    | `/api/v1/chat/attachments/*/content`, `/preview` | 添付の生 bytes |
    | `/api/v1/stream`, `/api/v1/console/stream`, `/api/v1/chat/threads/*/stream` | SSE（終わらない応答） |
    | 段が `secret(s)`・`credential(s)`・`token(s)`・`cookie(s)`・`password(s)`・`login` のどれか | 将来の秘密系 endpoint の既定拒否 |

  - 応答は 1 MiB まで（超えたら `-32002`）、30 秒で打ち切り。JSON は値、他の text は文字列で `{status, body}` を返す
    （4xx/5xx もそのまま返す）。text でない本文は拒否。
- D6: 監査。`mcp_calls` に `request` 列を足す（migration 0093）。`db_query` は SQL 本文と params、`api_get` は
  path と query を 4000 文字まで記録し、結果は記録しない（ADR-0056 D4「引数と結果の本文は残さない」はこの 2 つの
  ツールの**要求**についてだけ緩める）。他のツールの `request` は NULL のまま。流量制限は既存の
  `[mcp] rate_limit_per_min` に乗る。
- D7: 置き換えた文書。ADR-0056 Phase 101 追記の該当箇所に「この ADR で置き換えた」と付記し、`docs/guides/mcp.md` §8 の
  「DB 直読み禁止・汎用 HTTP 禁止」の説明を消して新方針を書く。ChatGPT 向けの使い方（問い合わせ例つき）は
  `docs/guides/chatgpt-rdc.md`（setup スクリプトが専用ユーザーの `~/CELERIS-CHAT.md` に写す）。

## 結果
- ChatGPT は `celeris-chat db_query '{"sql":"…"}'` と `celeris-chat api_get '{"path":"/api/v1/…"}'` で本番の
  決定ログ・evidence・events を読める。書き込み経路は増えない。
- 新しい秘密の列・endpoint を足す変更は、この ADR の表と `MASKED_COLUMNS` / `DENIED_*` を同時に更新する。
  `db_schema` の試験が「伏せる列が実在する」ことを固定する（列名の打ち間違いで素通りしない）。
