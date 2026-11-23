# Celeris の MCP サーバー（ADR-0056 D1/D2/D4/D5、Phase 78）

外部エージェント（ChatGPT・Claude Code・Codex・opencode 等）が Celeris を操作するための MCP
（Model Context Protocol）サーバー。実装は独立クレート `crates/celeris-mcp`。`celeris`（daemon）が
`[mcp]` を読んで起動する（`[llm_proxy]` と同じ形。`POST /reload` の対象外＝再起動が要る）。

守るべき境界（ADR-0056）: 外部エージェントは**人ではない**。案件・タスクの直接作成はできず、CoS に
渡すだけ（`console_instruct`）。知識は候補として `_inbox` に入るだけ（`knowledge_propose`）。組織の
`tools` / `permissions` / `review` は外から触れない。

## 1. 転送（Streamable HTTP）

MCP 仕様 2025-06-18 の Streamable HTTP。`POST /mcp` に JSON-RPC 2.0 を 1 件、応答は JSON
（`Accept: text/event-stream` を送っても JSON を返す。SSE 応答は実装していない — Phase 78 の逸脱。
`GET /mcp` は **405**（サーバー起点のストリーム購読は実装していない）。

`initialize` で `Mcp-Session-Id` ヘッダが発行され、以後の全メソッドで必須（無ければ 400、知らない
セッションは 404）。API（`7710`）とは別のポート（既定 `127.0.0.1:18200`）。TLS は持たない。**公開経路は
Celeris の外**（ChatGPT の Secure MCP tunnel、Tailscale、Cloudflare Tunnel）で用意する。

## 2. 設定（`[mcp]` / `[[mcp.listeners]]`）

口は複数持てる。それぞれ `auth = "token"`（既定。`Authorization: Bearer <token>` を検査）か
`auth = "none"`（**loopback だけ**。その口に来た要求はすべて `client = "<id>"` で固定したクライアント
として扱う）。

```toml
[mcp]
rate_limit_per_min = 60   # 既定 60（クライアントごと）

[[mcp.listeners]]
listen = "127.0.0.1:18200"   # Claude Code / Codex / LAN の客（Tailscale 越し含む）。Bearer 必須
auth = "token"

[[mcp.listeners]]
listen = "127.0.0.1:18201"   # ChatGPT Secure MCP tunnel の手元側だけが叩く。認証なし・客は chatgpt に固定
auth = "none"
client = "chatgpt"
```

1 口だけなら糖衣で書ける（`[mcp] listen` / `auth`。`[[mcp.listeners]]` と同時に書いた場合は両方が
有効になる）:

```toml
[mcp]
listen = "127.0.0.1:18200"
auth = "token"
```

`auth = "none"` を loopback 以外の `listen` に書くと**起動時の設定エラー**になる（`Config::validate`）。
`client = "..."` は `auth = "none"` のときだけ有効（`auth = "token"` の口に書くのも設定エラー）。

配備: migration `0024_mcp.sql`（`mcp_clients` / `mcp_calls`）。schema version 24。停止 → 起動が要る
（他の migration と同じ）。

## 3. クライアントの発行（`celerisctl mcp client`）

DB を直接開く管理系（`knowledge rerun` と同じ）。

```
$ celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client add chatgpt --scope skills:write
id: chatgpt
name: chatgpt
scopes: knowledge:read,knowledge:propose,tasks:read,console:instruct,org:read,skills:write
token: <64+ 文字の値。この 1 回しか出ない>
(この値は 2 度と表示されません。DB にはハッシュしか残りません。安全な場所に控えてください)
```

- **`<name>` がそのまま `id`（かつ主キー）になる**（`add` は名前 1 つしか取らないため。§2 の
  `[[mcp.listeners]] client = "chatgpt"` は `mcp client add chatgpt` で作った客をそのまま指す。
  同じ名前で 2 回 `add` するとエラー）。
- `--scope` を省略すると既定（`knowledge:read,knowledge:propose,tasks:read,console:instruct,org:read`）。
  `org:write` / `skills:write` は明示が要る。
- `--no-token` で「トークンを持たない客」を作る（`auth = "none"` の口に `client = "<id>"` で固定する
  専用。この客は `auth = "token"` の口では**絶対に**認証できない — トークンの値がそもそも無い）。
- `celerisctl mcp client ls` — 一覧（id / name / scopes / last_used_at。トークンは出ない）。
- `celerisctl mcp client scopes <id> --scope a,b,…` — scope を置き換える（トークンはそのまま。追加ではなく
  全体を書く）。次の要求から効く（scope は要求ごとに DB から読む）。
- `celerisctl mcp client revoke <id>` — 失効（以後そのトークンは使えない。`--no-token` の客も失効できる
  ので、`auth = "none"` の口を一時的に止めたいときにも使える）。
- トークンの値は**ログ・応答・`agent-docs/PROGRESS.md` のどこにも出さない**（celeris 側の DB にはハッシュ
  （SHA-256）だけが残る）。

## 4. スコープ

| スコープ | 許すこと |
|---|---|
| `knowledge:read` | `knowledge_list` / `knowledge_search` / `knowledge_get` / `resources/read`（`celeris://knowledge/*`） |
| `knowledge:propose` | `knowledge_propose`（`_inbox` に候補を置く） |
| `tasks:read` | `tasks_list` / `tasks_get` / `projects_list` / `projects_get` |
| `tasks:interact`（Phase 101） | `task_comment` / `task_answer`（`POST /tasks/{id}/comments` / `/answer` と同じ）、`task_decompose`（Phase F6。`POST /tasks/{id}/execution/decompose` と同じ）、`decision_list`（ADR-0079 R3a。`GET /decisions` と同じ。読むだけ） |
| `tasks:control`（Phase 101） | `task_retry` / `task_cancel`（`POST /tasks/{id}/retry` / `/cancel` と同じ） |
| `tasks:decide`（Phase 101） | `task_approve` / `task_reject`（`POST /tasks/{id}/approve` / `/reject` と同じ）、`decision_answer`（ADR-0079 R3a。`POST /decisions/{id}/answer` と同じ）、`task_plan_gate`（ADR-0079 R3b。`POST /tasks/{id}/execution/plan-gate` と同じ）。人の決定・計画・task の承認に答える道具はこの scope だけ（ADR 2026-11-22-external-llm-discussion-exchange D6） |
| `console:instruct` | `console_instruct` / `console_reply` |
| `org:read` | `org_list` / `org_get` |
| `org:write` | `org_create_node` / `org_mount_skill` / `org_unmount_skill` |
| `skills:read` | `skills_list` / `skills_get` |
| `skills:write` | `skills_put` |
| `db:read` | `db_schema` / `db_query`（DB を読み取り専用で問い合わせる。ADR 2026-10-23-mcp-read-only-db-and-api） |
| `api:read` | `api_get`（`/api/v1` への GET を daemon が代行する。同 ADR） |
| `discussion:read` | `discussion_snapshot` / `discussion_evidence_search`（案件 thread のうち人が共有した範囲と、その client 自身の propose だけ。ADR 2026-11-22-external-llm-discussion-exchange） |
| `discussion:propose` | `discussion_propose`（未承認の案を 1 件記録する。同 ADR） |

`tools/list` は**持っているスコープの道具だけ**を返す（無い道具は一覧にすら出ない）。スコープが無い
道具を `tools/call` で呼んでも JSON-RPC `-32601`（`method not found` と同じ見え方。「その道具は無い」
という以上の情報を返さない）。

## 5. 道具（tools）と resources

名前は `<領域>_<動詞>`。`limit` は既定 20・上限 100。詳細な引数は `tools/list` の `inputSchema`
（各道具の Rust の `*Args` 構造体から自動生成）を見ること。

- **知識**: `knowledge_list { scope?, tag?, limit? }`、`knowledge_search { query, scope?, limit? }`、
  `knowledge_get { path }`（`_retired` は not_found）、
  `knowledge_propose { title, body, scope, path?, op?, tags?, sources?, confidence? }`（`_inbox` に置く。出典に
  `mcp:<client_id>` を必ず足す。秘密を含む本文は拒否）。
  - **置き場のガード**（Phase K-1。`docs/guides/knowledge.md` §2.1。`celerisctl knowledge record` と知識整理 run と
    同じ関数）: `scope` は `user` / `environment` / `experience` / `project:<slug>`。**案件 ID を渡すと
    slug に直す**（`project:01M2…` → `project:agent-platform`）。知らない案件・`environment/` 直下・ULID の段・
    scope と `path` の食い違い・日本語だけの題名で `path` が無い、は `-32002 rejected` で、`message` に
    正しい書き方（分類の一覧・知っている slug）が入る。クライアントはそれを見て直してもう一度呼ぶ。
  - 同じ scope に同じ題名のページがある・人についての事実（`user/profile|expertise|preferences|goals`）は、
    新しいページを作らずそのページへの候補になる（出力の `target` と `op: "append"`、向け直した理由の
    `redirect: {kind: same_title | user_canonical, from}`）。`op: "merge"` を付けると「本文は
    `knowledge_get` で読んだ既存ページを統合した完全な版」で、accept で上書きする。
  - `tools/list` の `knowledge_propose` の `description` の後半には、その時点の置き場（`environment` の
    分類と、案件の `project:<slug>` = 題名（id）の一覧）が載る（呼ぶたびに DB と KB から組み直す）。
  - `knowledge_list` / `knowledge_search` の `scope` に `project:<案件 ID>` を渡しても slug として引く。
- **タスク・案件**（読むだけ）: `tasks_list { status?, project_id?, limit? }`、`tasks_get { id }`、
  `projects_list { status?, limit? }`、`projects_get { id }`。
- **タスクの操作**（Phase 101。外部エージェント（ChatGPT・RDC 経由）の**書き込み**は、scope で許した
  この操作だけにする（HTTP API は `api_get` の GET しか代行しない）。いずれも `task-api` の HTTP ハンドラと**同じ** `task-ops` の
  関数を呼ぶ。書き込みの主体は `mcp:<client_id>`）:
  - `task_comment { id, text }`（scope `tasks:interact`。`POST /tasks/{id}/comments` と同じ効き方 —
    `running`/`reviewing` は割り込んで `ready` に戻す、`blocked` は回答として渡す、それ以外は記録するだけ。
    コメントの `author` は `mcp:<client_id>`）。
  - `task_answer { id, answer, expected_status? }`（scope `tasks:interact`。`POST /tasks/{id}/answer` と
    同じ。フィールド名は既存の `AnswerBody.answer` に合わせてある）。
  - `task_decompose { id, mode, note? }`（scope `tasks:interact`。ADR-0072「Phase F6 実装時の決定」P1/P6。
    `POST /tasks/{id}/execution/decompose` と同じ。`mode = "compound"` で起票済みの `draft`/`ready`/`blocked` の
    タスクを分解の経路に入れる（次の dispatch が planner run。計画を既に持つタスクには replan の依頼）、
    `"atomic"` で直接実行に戻す。`execution_hint_set` の `source` は `mcp:<client_id>`。`running`/`reviewing`/
    終端・gate の対象外は `-32602`（終端は `task_retry` の `execution` を使う）。run を止めない・複製しない・
    承認しない操作なので、`task_answer` と同じ `tasks:interact` に置いた（§8.2 の推奨 scope のまま使える）。
    例: `{"name":"task_decompose","arguments":{"id":"01M3…","mode":"compound","note":"工程に分けて"}}`）。
  - `decision_list { open?, root_id?, task_id? }`（scope `tasks:interact`。ADR-0079 D7 / Phase R3a。
    `GET /decisions` / `GET /tasks/{id}/decisions` と同じ `task_ops::decision`。既定は未回答だけ（`open` 省略 =
    `true`）。`root_id` で 1 つの木、`task_id` でその task の subtree に絞る（同時には使えない）。各要素は
    `DecisionView`（`decision.id`・`path`〈root › 段階 › unit のパンくず〉・`question`・`options`・`recommended`・
    `cost_of_reversal`・`needed_before`〈止めている unit / `stage:<key>` / `self`〉・`kind`）。
  - `decision_answer { id, option?, note? }`（scope `tasks:decide`。`POST /decisions/{id}/answer` と同じ。
    人の決定への回答なので `task_approve` と同じ `tasks:decide` に置く（ADR 2026-11-22-external-llm-discussion-exchange D6。
    `tasks:interact` だけの client は `decision_list` で読めるが答えられない））。`option` は決定の
    `options[].key` のどれか。`kind = choice` の決定だけ `option` を省いて `note` に自由記述で答えられる
    （記録される `option` は `other`）。`DecisionAnswered.by` は `mcp:<client_id>`。効き目は決定の種類と選択肢で
    決まる（ADR-0079 付記「R3a 実装時の逸脱・明確化」の表: 待っていた unit を進める / limit の `raise-once`・
    `replan`・`withdraw` / plan_invalid の `replan`・`atomic`・`cancel`）。回答済み・取り下げ済み・選択肢の外は
    `-32602`、無い id は `-32001`。取り下げ（`withdraw`）と revise は MCP には出さない（人が GUI / API で行う）。
    例: `{"name":"decision_answer","arguments":{"id":"01M4…","option":"org-vault","note":"まず試験用で"}}`。
  - `task_plan_gate { task_id, action, note? }`（scope `tasks:decide`。ADR-0079 D8 / Phase R3b。
    `POST /tasks/{id}/execution/plan-gate` と同じ `task_ops::plan_gate::plan_gate`）。root の計画の承認待ち
    （`awaiting_plan_approval`）に `approve` / `replan`（`note` 必須。planner への指示）/ `withdraw`（task と subtree の
    中止）で応える（`decision` は `action` の別名）。記録の主体は `mcp:<client_id>`（replan の `ExecutionHintSet.source` は
    `mcp:<client_id> (plan-gate)`）。承認待ちでない・note が無い replan は `-32602`、無い task は `-32001`。計画の決定への
    回答は別（`decision_answer`）。例: `{"name":"task_plan_gate","arguments":{"task_id":"01M4…","action":"approve"}}`。
  - `task_retry { id, execution? }`（scope `tasks:control`。`POST /tasks/{id}/retry`（`accept=false`）と同じ。
    `failed`/`cancelled` のタスクを複製して新しい `draft` を作る。`execution: "compound" | "atomic"` で複製先の
    実行の形を明示できる（`source` は `mcp:<client_id>`）。複製先は元の gate の判定を持たず、今の設定で判定し直す）。
  - `task_cancel { id, reason? }`（scope `tasks:control`。`POST /tasks/{id}/cancel` と同じ。`reason` を
    渡すと、取り消す前に「人を起こさない」コメント（`author = mcp:<client_id>`）として記録する —
    `gate::cancel` 自体には理由を運ぶ欄が無いため）。
  - `task_approve { id, note? }`（scope `tasks:decide`。`POST /tasks/{id}/approve` と同じ。承認タスクの
    `Event::ApprovalDecided.by` に `mcp:<client_id>` が入る）。
  - `task_reject { id, reason }`（scope `tasks:decide`。`POST /tasks/{id}/reject` と同じ。`reason` は
    必須・空白不可。`by` は `mcp:<client_id>`）。
- **CoS チャット**: `console_instruct { text, project_id?, thread_id?, relay? }`。`thread_id` を省略すると
  呼び出しごとに通常の会話（`kind=human`）を作る。題名は `<client_id>: <本文の先頭行、最大60文字>`、
  発言の metadata の `author` は `mcp:<client_id>`。`project_id` はスレッドに保持する。
  続きは返値の `thread_id` を指定する。存在しない・archived・legacy のスレッド、またはスレッドと異なる
  `project_id` は `invalid_params`。返値は `{ message_id, task_id, thread_id }`。
  `task_id` は入力メッセージ ID と同じ互換の受付 ID（実行タスクは作らない）なので、従来の prompt は
  そのまま `console_reply` に渡せる。legacy スレッドや REST の Console 入力には投入しない。
  `console_reply { task_id?, thread_id?, wait_secs? }` は ID を**どちらか一つ**指定する。
  `task_id` はその入力の返信、`thread_id` は最新の人の発言の返信を返す（古い完了済みの返信へ戻らない）。
  `wait_secs` は既定0、上限60秒。run が completed なら `{ state: "done", reply, actions: [] }`、
  failed なら `{ state: "failed", reply? }`、stopped/interrupted または入力が cancelled なら
  `{ state: "cancelled" }`、それ以外は `{ state: "pending" }`。新 CoS は actions を実行しない。
- **組織**: `org_list {}`、`org_get { node_id }`（実効 profile。`skills_mounts` を含む）、
  `org_create_node { parent_id, id, name, profile? }`（`profile` は `skills` / `knowledge` /
  `skills_mounts` / `harnesses` / `model` / `policy` / `run` だけ反映される。`tools` / `permissions` /
  `review` を送っても**無視される**）、`org_mount_skill { node_id, skill }` /
  `org_unmount_skill { node_id, skill }`。
- **skills**（KB の `skills/<name>/SKILL.md`。mount された skill が run に届く）:
  `skills_list {}`、`skills_get { name }`、
  `skills_put { name, skill_md, files? }`（frontmatter に `name` / `description` 必須。名前は
  `[a-z0-9-]{1,64}`。出典 `mcp:<client_id>` を frontmatter に残す。mount されるまで何にも効かない）。

- **DB の読み取り**（scope `db:read`。ADR 2026-10-23-mcp-read-only-db-and-api D2/D3）:
  `db_schema {}`（表・view・列・index、伏せた列の一覧 `masked`、上限）、
  `db_query { sql, params?, limit? }`（1 文の SELECT。`?1` 等に `params` を結ぶ。返り値は
  `{columns, rows, row_count, truncated, truncated_reason?, masked?, elapsed_ms}`）。daemon の store とは別の
  読み取り専用接続（`mode=ro`・`query_only`）を呼び出しごとに開き、authorizer で SELECT・読み取りの関数・
  `table_info` 等の PRAGMA だけを許す（書き込み・ATTACH・他の PRAGMA・temp 表・`load_extension` は `-32002`）。
  行数は既定 500・上限 5000、結果 1 MiB、10 秒で打ち切り、同時 2 本。秘密の列（`mcp_clients.token_hash`、
  `node_sessions.session_id` など。一覧は ADR と `db_schema` の `masked`）は WHERE の中でも NULL として読まれる。
- **API の GET**（scope `api:read`。同 ADR D5）: `api_get { path, query?, method? }`。daemon が `[api] listen` の
  loopback に daemon が起動ごとに作る MCP 専用 token と客の id（`X-Celeris-Mcp-Client`）で GET する（客に token は渡らない。API 側の主体は `mcp:<client_id>`、変更系は 403。ADR 2026-10-27-api-principals D4）。`path` は `/api/v1/` の下だけ、
  `query` は object（値は文字列・数・真偽）。`method` は省略か `GET`。secret・保存 credential・browser の
  identity と Live View・credential の受け渡し・TOTP 接続・login・添付の生 bytes・SSE は拒否（`-32002`。一覧は ADR）。
  返り値は `{status, content_type, body}`（4xx/5xx もそのまま返す）。応答 1 MiB・30 秒まで。

- **外部 LLM との議論**（ADR 2026-11-22-external-llm-discussion-exchange D1。ChatGPT・Muse など MCP の客は同じ契約）:
  対象は `project_id`（案件の固定の CoS thread）か `thread_id`（`kind=project` の thread）。両方なら一致しなければ、
  別案件・統括の thread なら `-32602`（`thread_project_mismatch`・`thread_kind_not_project`）。
  - `discussion_snapshot { project_id?, thread_id?, after_seq?, limit? }`（scope `discussion:read`）: 人が
    `exchange/shares` で共有した message（共有した版のまま。撤回・共有後の編集・取り消し・期限切れは `omitted`）と、
    この客自身の propose だけを `celeris.discussion-snapshot/1` で返す。各 item は `citation`・`content_hash`・`source`・
    `actor`・`consent`・`references`・`confidence`・`decision_state` を持ち、秘密の形は `[masked]`。共有が無ければ
    `items: []`。既定 50 件・上限 200 件、続きは `next_after_seq`。要約・戦略の添付 file は今は MCP からは返さない
    （`McpState` は CoS の workspace を読まない。ADR の D1.7）。
  - `discussion_evidence_search { project_id?, thread_id?, query, limit? }`（scope `discussion:read`）: snapshot と
    同じ範囲（マスク後の本文）だけを語で探す（空白で区切った語がすべて含まれるもの、大文字小文字を区別しない）。
    `query` は 1〜512 文字。返り値は `celeris.discussion-evidence/1` の `hits[]`（`citation`・`snippet` 400 文字まで・
    `references`・`confidence`・`decision_state`・`actor`・`source`・`content_hash`）。knowledge は探さない。
  - `discussion_propose { project_id?, thread_id?, idempotency_key, source_label, body, references?, confidence, decision_state }`
    （scope `discussion:propose`）: 案件 thread に `role=user`・author `mcp:<client_id>`・`discussion_mode: discuss` の
    message を 1 件作り、`metadata_json.exchange` に envelope（`actor = mcp:<client_id>`、consent は客の scope）を
    書く。人の発言・承認には数えない。`decision_state` は `discussion` か `proposed` だけ（他は `-32002`）。
    `actor`・`consent`・`approval` を引数に書くと `-32602`。`kind: chat` の参照はこの客の snapshot に今ある item の
    `thread_id`・`message_id`・`revision`・`sha256` と一致しなければ `-32002`（`citation_invalid`）、`kind: url` は
    文字列として保存するだけで取りに行かない。同じ `idempotency_key` と本文は 1 件（`duplicate: true`）、別の本文は
    `-32002`（`R7:idempotency_conflict`）。key は客ごとの名前空間（`mcp:<client_id>:<key>`）。秘密を含む本文は `-32002`
    （`exchange_secret`。本文は返さない）。返り値 `{message_id, thread_id, project_id, seq, content_hash, decision_state, duplicate}`。
  - 3 道具の引数・本文は `mcp_calls.request` に残さない。MCP の客は共有も取り込みもできない（人の REST だけ）。

**resources**（読むだけの客のため。`resources/read { uri }` は対応する `*_get` と同じものを返す）:
`celeris://knowledge/<path>`、`celeris://tasks/<id>`、`celeris://projects/<id>`、
`celeris://org/<node_id>`、`celeris://skills/<name>`。`resources/list` は**列挙できるもの**（知識の
索引・組織・skills）だけを返す（タスク・案件は件数が大きいので、`tasks_list` / `projects_list` で id を
知ってから `resources/read` で読む）。

## 6. 監査と流量制限

- すべての `tools/call` を `mcp_calls`（`client_id` / `tool` / `ok` / `error_kind` / `latency_ms` /
  `at` / `request`）に残す。結果の本文は残さない。引数は `db_query`（SQL 本文と params）と `api_get`（path と
  query）だけを `request` に 4000 文字まで残し、他の道具は残さない。`console_instruct` はチャットに発言を保存するので二重には
  書かない。
- クライアントごとに 1 分あたり `[mcp] rate_limit_per_min`（既定 60）。超えたら JSON-RPC エラー
  `-32000` + `data.retry_after`（秒）。
- 管理 API（`docs/api/v1/api.md` §3.110〜3.111）: `GET /mcp/clients`（トークンは出ない）、
  `GET /mcp/calls?client=`（直近 100 件）。

## 7. 接続手順

### 7.1 ChatGPT の Secure MCP tunnel

ChatGPT のコネクタ（Secure MCP tunnel）は手元のマシンから outbound 443 でトンネルを張り、その先の
エージェントが同じマシンの MCP を叩く。**Bearer ヘッダを足す機能が無い**（2026-09-21 人の確認）ので、
`auth = "none"` の専用の口を用意し、その口に来る要求はすべて `client = "chatgpt"` として扱う
（§2 の設定例）。トンネルの手元側プロセスが `http://127.0.0.1:18201/mcp` を叩くように設定する
（トンネルのセットアップ自体は ChatGPT 側の手順に従う。celeris 側はポートを開けて待つだけ）。

```
$ celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client add chatgpt \
    --scope knowledge:read,knowledge:propose,tasks:read,console:instruct
```

（`--no-token` は不要。`auth = "none"` の口は元々トークンを見ない。このクライアントに `auth = "token"`
の口からアクセスさせたいなら、別途トークン付きで作り直す）

### 7.2 Claude Code（リモート HTTP）

```
$ claude mcp add --transport http celeris http://127.0.0.1:18200/mcp \
    --header "Authorization: Bearer <celerisctl mcp client add で出たトークン>"
```

（CLI のフラグは `claude mcp add --help` で確認する）。

### 7.3 Codex（`mcp_servers` 設定）

Codex CLI の設定ファイル（`~/.codex/config.toml` 等）に:

```toml
[mcp_servers.celeris]
url = "http://127.0.0.1:18200/mcp"
headers = { Authorization = "Bearer <トークン>" }
```

（設定のキーは手元の Codex CLI の設定資料で確認する）。

### 7.4 stdio ↔ HTTP の橋（`celerisctl mcp stdio`）

stdio でしか MCP を話せない CLI エージェント（opencode 等）向け。中身は同じサーバーへの普通の
HTTP 要求で、`Mcp-Session-Id` を橋の中で覚えて次の行に載せる（1 行 = 1 つの JSON-RPC メッセージ、
newline-delimited JSON）。

```
$ echo "<トークン>" > /tmp/celeris-mcp-token
$ celerisctl mcp stdio --base-url http://127.0.0.1:18200 --token-file /tmp/celeris-mcp-token
```

`--token-file` を省略すると環境変数 `CELERIS_MCP_TOKEN` を見る。`auth = "none"` の口を橋渡しするだけ
なら、どちらも省略してよい（例: `celerisctl mcp stdio --base-url http://127.0.0.1:18201`）。

### 7.5 1 回だけの呼び出し（`celerisctl mcp call`。Phase 101）

stdio の橋すら持たない、シェルから 1 コマンドだけ叩ければよい客（`scripts/rdc/celeris-chat` 経由の
ChatGPT 等）向け。**DB は開かない**（`stdio` と同じ `reqwest::blocking` の HTTP クライアントを再利用し、
`initialize` → `tools/call` を 1 回だけ行う）。

```
$ celerisctl mcp call --base-url http://127.0.0.1:18200 --token-file /tmp/celeris-mcp-token \
    task_comment '{"id":"<task ulid>","text":"確認しました"}'
```

- `TOOL`（道具の名前）と `JSON_ARGS`（引数。省略時は `{}`）の 2 つの位置引数。`--list` を付けると
  `tools/list` の名前と description の一覧だけを出す（`TOOL`/`JSON_ARGS` は不要）。
- `--base-url`（既定 `http://127.0.0.1:18200`）、`--token-file`（省略時は環境変数 `CELERIS_MCP_TOKEN`）。
- 出力: `structuredContent` があればそれを整形 JSON で、無ければ `content[0].text`（JSON なら整形、
  そうでなければそのまま）を標準出力へ。
- エラー: 引数不正（`JSON_ARGS` が JSON として読めない、`TOOL` も `--list` も無い）は **exit code 2**。
  JSON-RPC のエラー・HTTP のエラー（401 等）・接続不可は理由を stderr に出して **exit code 1**。

## 8. Remote Desktop Commander 経由（ChatGPT）（Phase 101、ADR 2026-10-23-mcp-read-only-db-and-api）

ChatGPT からは Remote Desktop Commander（RDC）というリモート MCP で home-dev のシェルを叩ける。
ChatGPT の操作は **MCP client（`chatgpt-rdc-2`）の scope で許される範囲だけ**にする。DB と HTTP API は、
MCP の中の読み取り限定の道具（`db_query` / `db_schema`、`api_get`）で読む。専用ユーザーに DB ファイルの
権限・API の token・curl は渡さない（境界は Linux ユーザーと MCP client の scope）。

```
ChatGPT
  └─ RDC device agent（専用 Linux ユーザー chatgpt-rdc、権限最小）
       └─ scripts/rdc/celeris-chat（薄い入口。celerisctl mcp call をそのまま呼ぶだけ）
            └─ 127.0.0.1:18200/mcp（token 付き。auth = "token"）
                 └─ Celeris MCP（crates/celeris-mcp）
                      ├─ db_query / db_schema → DB を読み取り専用の別接続で（db:read）
                      ├─ api_get → daemon が /api/v1 に GET（api:read）
                      └─ discussion_* → 人が共有した案件の議論と未承認の案（discussion:read・discussion:propose。§9）
```

ChatGPT 向けの使い方（よく使う問い合わせ例）は `docs/guides/chatgpt-rdc.md`。setup スクリプトが専用ユーザーの
`~/CELERIS-CHAT.md` に写す。

### 8.1 専用 Linux ユーザーの権限最小化

RDC の device agent は `chatgpt-rdc` という**専用の**ユーザーで動かす（rmaeda 本人のアカウントでは
動かさない）。そのユーザーに与えるのは:

- `scripts/rdc/celeris-chat` を実行できること（と、そのスクリプトが読む `celerisctl` バイナリ・
  token ファイル）。
- **それ以外は何も無い**: `sudo` 無し、SSH 鍵無し、`~/.config/celeris` / `~/.local/celeris` の DB
  （`celeris.sqlite3`）・API の token への権限無し。DB と API は MCP の `db_query` / `api_get` を通して
  読むだけで、秘密の列・path は MCP の側で伏せる・拒否する。
- token ファイル（`CELERIS_MCP_TOKEN_FILE`、既定 `$HOME/.config/celeris/mcp-token`）だけを、
  `chatgpt-rdc` から読める権限（他人には読ませない。`0400` 等）で置く。

`celerisctl` バイナリは `scripts/rdc/setup-chatgpt-rdc.sh` が専用ユーザーの `~/.local/bin/` に写す
（`celerisctl mcp call` は DB を開かない）。

### 8.2 推奨 scope

RDC の ChatGPT の client（本番の id は `chatgpt-rdc-2`。ADR 2026-11-22-external-llm-discussion-exchange D6）:

```
$ celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client add chatgpt-rdc-2 \
    --scope knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read,discussion:read,discussion:propose
# 既にある client の scope を置き換えるとき（token はそのまま。scope は置き換えなので全部書く）
$ celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client scopes chatgpt-rdc-2 \
    --scope knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read,discussion:read,discussion:propose
```

- 与える: `knowledge:read`、`knowledge:propose`、`tasks:read`、`tasks:interact`（コメント・`task_answer`・
  `task_decompose`・`decision_list`）、`console:instruct`、`db:read`（DB の読み取り）、`api:read`（API の GET）、
  `discussion:read`・`discussion:propose`（§5 の議論の道具）。
- **与えない**: `tasks:decide`（`task_approve`・`task_reject`・`decision_answer`・`task_plan_gate`。人の決定・計画・
  task の承認に答える道具）、`tasks:control`（やり直し・取り消し）、`org:write`、`skills:write`。これらは人が GUI から
  行う操作で、外部の LLM に渡さない。ChatGPT の発言は人の承認にならない（MCP の記録の主体は `mcp:<client_id>`）。
- `db:read`・`api:read` は運用の分析のために残す（人の決定 `ext-client-scope` の答え b）。DB と API の全読みなので、
  未発表の研究結果など機微な内容も ChatGPT に出うる（人が受け入れた運用）。`discussion_snapshot` の共有の範囲の制限は、
  この 2 つを持たない client だけを守る。

Muse など、RDC 以外の MCP 対応の外部 LLM の client（HTTP の token か `celerisctl mcp stdio`）:

```
$ celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client add muse \
    --scope discussion:read,discussion:propose
```

- 与えるのは `discussion:read`・`discussion:propose` だけ。`console:instruct`・`db:read`・`api:read`・`knowledge:read`・
  `tasks:*` を渡さない（人が共有した議論を読み、未承認の案を記録するだけにする）。
- scope の付与・token の発行は人が行う（credential は人だけが扱う）。
- この口は `auth = "token"`（§2 の 18200）を使う（`auth = "none"` の 18201 は同じホストの ChatGPT の
  Secure MCP tunnel 専用。RDC とは別経路）。

### 8.3 `celeris-chat` の呼び方

`chatgpt-rdc` のシェルからはこのスクリプトだけを呼ばせる（§7.5 の `celerisctl mcp call` の薄い
ラッパ。判断や検証は持たない）:

```
$ celeris-chat --list
$ celeris-chat tasks_list '{"status":"blocked","limit":5}'
$ celeris-chat knowledge_search '{"query":"pegasus"}'
$ celeris-chat db_schema
$ celeris-chat db_query '{"sql":"SELECT kind, count(*) FROM events GROUP BY kind ORDER BY 2 DESC","limit":50}'
$ celeris-chat api_get '{"path":"/api/v1/routing/evidence"}'
$ celeris-chat console_instruct '{"text":"調査結果をタスクにして"}'
# 続きは console_instruct に返値の thread_id を指定。返信は task_id または thread_id で取得。
$ celeris-chat console_reply '{"task_id":"<上の task_id>","wait_secs":30}'
$ celeris-chat task_comment '{"id":"<task ulid>","text":"見ました。続けてください"}'
# 人が共有した案件の議論を読み、未承認の案を記録する（流れは §9.3）
$ celeris-chat discussion_snapshot '{"project_id":"<案件の id>"}'
```

環境変数（`scripts/rdc/celeris-chat` が読む）: `CELERIS_BIN`（既定 `$HOME/.local/celeris/current/bin/celerisctl`）、
`CELERIS_MCP_URL`（既定 `http://127.0.0.1:18200`）、`CELERIS_MCP_TOKEN_FILE`（既定
`$HOME/.config/celeris/mcp-token`）。`chatgpt-rdc` の環境で上書きしてよい。

### 8.4 注意

`auth = "none"` の口（§2 の 18201。ChatGPT の Secure MCP tunnel 専用）は、**同じホストの任意の
プロセスから叩ける**（loopback なら誰でも `curl http://127.0.0.1:18201/mcp` を叩ける、という意味。
認証はトンネルの配置だけで担保している）。RDC の device agent を celeris と**同居させるホスト**では、
この口は使わない（`chatgpt-rdc` から `18201` を直接叩けてしまうと、§8.2 で絞った scope を素通りして
`chatgpt`（Secure MCP tunnel 用）のクライアントとして振る舞えてしまう）。RDC 経由は必ず `auth =
"token"` の口（§2 の 18200）と、§8.2 で絞った専用クライアント（`chatgpt-rdc-2`）のトークンを使うこと。

### 人の指示の取り次ぎ

利用者が MCP client に伝えた指示は `console_instruct` の `text` に原文を書き、
`relay: {"said_at":"2026-10-27T04:00:00Z","task_ids":["<task id>"],"expires_at":"2026-10-28T00:00:00Z"}` を付ける。
時刻は実際の発言時刻、task と期限は利用者が指定した範囲に合わせる（省略すると案件全体・期限なし）。
`project_id` または `thread_id` で対象の固定スレッドを指定する。
投稿主体は認証済み client の `mcp:<client_id>`。CoS は返値の `message_id` を `instructed_by` として参照できる。
自分の判断は人の指示として取り次がない。範囲と監査の詳細は[取り次ぎ ADR](../../agent-docs/adr/2026-10-27-relayed-instructions.md)。

## 9. 外部 LLM（ChatGPT・Muse など）と議論する 3 つの経路（ADR 2026-11-22-external-llm-discussion-exchange）

議論の正本は Celeris の案件の固定の CoS thread である。外部 LLM は、人が共有した発言と、その client 自身が記録した案だけを
読み、未承認の案（draft）だけを記録できる。どの経路も同じ形（`celeris.discussion-snapshot/1` の item。`citation`・
`content_hash`・`source`・`actor`・`consent`・`confidence`・`decision_state`）を使う。

| 経路 | 使う場面 | 読む | 書く |
|---|---|---|---|
| (a) MCP の client | Muse など MCP を話す client（HTTP の token か §7.4 の `celerisctl mcp stdio`）。ChatGPT の custom remote MCP（§7.1）が使える環境 | `discussion_snapshot`・`discussion_evidence_search`（§5） | `discussion_propose`（§5） |
| (b) RDC | ChatGPT から RDC の端末の `celeris-chat` を呼ぶ（§8）。今の home-dev の経路 | 同じ道具を `celeris-chat` で | 同じ道具を `celeris-chat` で |
| (c) Markdown/JSON の交換 | MCP も RDC も使えない ChatGPT（プランを問わない）・その他の LLM | 人が REST で書き出して貼り付ける | 人が選んだ要約・引用を REST で 1 件ずつ取り込む |

- ChatGPT の custom remote MCP は optional である。無い環境でも (c) で使え、RDC がある環境では (b) を使う。外部に公開する
  MCP（tunnel・公開の listener・本番の client の発行）は人が承認してから人が配置する。
- scope は §8.2: RDC の ChatGPT の client（`chatgpt-rdc-2`）と、Muse など RDC 以外の client（`discussion:read`・
  `discussion:propose` だけ）。どちらにも `tasks:decide` を渡さないので、外部 LLM は人の決定・計画・task の承認に答えられない。
- 外部 LLM の発言（propose・取り込んだ文）は、人の発言にも承認にも数えない。propose の記録の主体は `mcp:<client_id>`、
  取り込みの主体は取り込んだ人だが、どちらも `discussion_mode: discuss` の発言になり、それを読んだ CoS の run は task の起票・
  Roadmap の変更・決定への回答・publish の operation を拒否される。案を採用するのは人で、Celeris の Web で行う。

### 9.1 人が共有する（3 経路に共通）

既定では何も外に出ない。人が案件 thread の発言を選んで共有した範囲だけが、snapshot・検索・書き出しに出る。共有・取り消し・
書き出し・取り込みの 4 route（`docs/api/v1/api.md` の 242〜245）は人の API token（主体 `human:<name>`。web gateway の token など）
だけが呼べ、MCP の client・operator・CoS の run・認証の無い要求は 403 `exchange_forbidden` になる。今は Web の画面と
`celerisctl` の subcommand が無いので、`curl` で呼ぶ。

```bash
# CELERIS_API に daemon の base URL（例 http://127.0.0.1:<api-port>）、CELERIS_API_TOKEN に人の API token を入れておく。
# token の値を log・成果物・chat に書かない。
T=<案件の thread の id>     # Web の chat 画面か GET $CELERIS_API/api/v1/chat/threads の project_id で探す
# 共有する発言の id は GET $CELERIS_API/api/v1/chat/threads/$T/messages で見る
curl --fail-with-body -X POST -H "Authorization: Bearer $CELERIS_API_TOKEN" -H 'content-type: application/json' \
  --data '{"message_ids":["<message id>"],"scope":"selected_excerpt","expires_at":"2026-12-01T00:00:00Z"}' \
  "$CELERIS_API/api/v1/chat/threads/$T/exchange/shares"          # → 201 {"share_id": …, "revoked": false}
# 取り消す
curl --fail-with-body -X POST -H "Authorization: Bearer $CELERIS_API_TOKEN" \
  "$CELERIS_API/api/v1/chat/threads/$T/exchange/shares/<share_id>/revoke"
```

- `expires_at` は必須で、共有した時刻から 30 日以内。共有は発言のその時の版に固定され、後で編集・撤回した発言は出なくなる
  （snapshot の `omitted` に理由だけが出る）。もう一度共有すれば今の版が出る。
- 秘密の形（token・password・cookie・webhook URL など）を含む発言は 422 `exchange_secret` で共有できない。発言を直してから共有する。
  出すときにも秘密の形は `[masked]` に置き換わる。
- knowledge のページと、CoS の返事に添付した要約・戦略の file は、今はどの経路からも外に出ない。

### 9.2 (a) MCP の client（Muse など）

人が §8.2 の scope で client を発行し、token を client の設定に入れる（token の発行と保管は人が行う）。client は §5 の 3 道具を
呼ぶ。流れは (b) と同じで、`celeris-chat <道具> '<JSON>'` の代わりに MCP の `tools/call` を使う。ChatGPT の custom remote MCP を
使うときも同じ 3 道具を使う（§7.1。`auth = "none"` の口の client の scope は人が決める）。

### 9.3 (b) RDC（ChatGPT → `celeris-chat`）

1. 人が 9.1 で発言を共有する。
2. ChatGPT が `celeris-chat discussion_snapshot '{"project_id":"<案件の id>"}'` を呼ぶ。返り値の `thread_id`・`project_id`・
   各 item の `citation`（`thread_id`・`message_id`・`revision`・`sha256`）が、以後の会話の鍵になる。
3. 出典を探すときは `celeris-chat discussion_evidence_search '{"project_id":"<案件の id>","query":"<語>"}'`。
4. 人が ChatGPT の中で採用したい案を選んだら、ChatGPT が `celeris-chat discussion_propose` で、snapshot の `thread_id` と、
   引く item の `citation` に `"kind":"chat"` を足した参照を付けて記録する。同じ `idempotency_key` で送り直しても 1 件になる。
5. 次の `discussion_snapshot` に、その案が `kind: "proposal"`・`actor.id: "mcp:chatgpt-rdc-2"`・`references` 付きで出る。
   続きは同じ `thread_id` を渡す。ChatGPT の新しい会話からでも、`project_id` だけで同じ thread に戻る。

ChatGPT 向けの呼び方の例は `docs/guides/chatgpt-rdc.md` §5。この流れは試験
`crates/celerisctl/tests/rdc_celeris_chat.rs`（`rdc_celeris_chat_snapshot_propose_continue`）が、`scripts/rdc/celeris-chat` →
`celerisctl mcp call` → 試験の MCP server で、setup スクリプトの scope のまま確かめる。

### 9.4 (c) Markdown/JSON の交換（MCP の無い client）

書き出し（人が共有した範囲を外部 LLM に貼る）:

```bash
curl --fail-with-body -H "Authorization: Bearer $CELERIS_API_TOKEN" \
  "$CELERIS_API/api/v1/chat/threads/$T/exchange/export?format=markdown&share_id=<share_id>" > discussion.md
```

- `format=json` なら同じ内容の JSON。`share_id` を省くと有効な共有のすべて。読むだけで、記録は作らない。
- 人は `discussion.md` の全体か一部を ChatGPT などに貼る。出典を示すときは各 item の `citation` を残す。

取り込み（人が外部 LLM との議論から選んだ要約・引用を 1 件だけ戻す）:

```bash
BODY='ChatGPT との議論で人が選んだ要約: H2 を先に確かめる'
HASH=$(printf '%s' "$BODY" | sha256sum | cut -d' ' -f1)     # 本文の SHA-256（末尾の改行を含めない）
cat > import.md <<MD
---
schema: celeris.discussion-envelope/1
project_id: <案件の id>
source: {kind: markdown_import, label: chatgpt}
source_revision: <書き出しの source_revision をそのまま。例 {request_sha256: …, roadmap_version: 3}>
timestamp: 2026-11-22T03:10:00Z
content_hash: sha256:$HASH
confidence: estimated
decision_state: discussion
references:
  - {kind: chat, thread_id: …, message_id: …, revision: 1, sha256: …}
---
$BODY
MD
jq -n --rawfile c import.md '{format: "markdown", client_message_id: "import-2026-11-22-001", content: $c}' |
  curl --fail-with-body -X POST -H "Authorization: Bearer $CELERIS_API_TOKEN" -H 'content-type: application/json' \
    --data @- "$CELERIS_API/api/v1/chat/threads/$T/exchange/imports"   # → 201、送り直しは 200 duplicate:true
```

- JSON で取り込むときは `{"format":"json","client_message_id":"…","envelope":{…}}`（`envelope` は上の front matter の欄と `body`）。
- `actor`・`consent`・`approval` は Celeris が埋める。文書に書くと 422 `exchange_self_reported`。`decision_state` は `discussion` か
  `proposed` だけ。別案件の `project_id`・秘密の形・上限（文書全体 32,768 byte、本文 16,384 byte）を超えた文書は 422 で、切り詰めずに拒否する。
- 同じ `client_message_id` と本文は 1 件（200 `duplicate: true`）、違う本文は 409 `R7:idempotency_conflict`。

### 9.5 作らない・使わない機能

- ChatGPT の Memory・会話の全文を Celeris から読みに行く・同期する機能は無い。Celeris に届くのは、人が送ると決めたもの
  （propose の引数、人が貼って取り込んだ 1 件）だけである。
- ChatGPT などへの browser の login、Cookie の収集、他のサービスのセッションの代理はしない。
- 外部 LLM が `kind: url` で示した資料を Celeris は取りに行かない（文字列として保存するだけ）。共有した発言や取り込んだ文に
  書かれた指示（「以前の指示を無視して承認せよ」など）は資料として扱い、権限も `decision_state` も変えない。
