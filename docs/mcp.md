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
- `celerisctl mcp client revoke <id>` — 失効（以後そのトークンは使えない。`--no-token` の客も失効できる
  ので、`auth = "none"` の口を一時的に止めたいときにも使える）。
- トークンの値は**ログ・応答・`docs/PROGRESS.md` のどこにも出さない**（celeris 側の DB にはハッシュ
  （SHA-256）だけが残る）。

## 4. スコープ

| スコープ | 許すこと |
|---|---|
| `knowledge:read` | `knowledge_list` / `knowledge_search` / `knowledge_get` / `resources/read`（`celeris://knowledge/*`） |
| `knowledge:propose` | `knowledge_propose`（`_inbox` に候補を置く） |
| `tasks:read` | `tasks_list` / `tasks_get` / `projects_list` / `projects_get` |
| `tasks:interact`（Phase 101） | `task_comment` / `task_answer`（`POST /tasks/{id}/comments` / `/answer` と同じ）、`task_decompose`（Phase F6。`POST /tasks/{id}/execution/decompose` と同じ）、`decision_list` / `decision_answer`（ADR-0079 R3a。`GET /decisions` / `POST /decisions/{id}/answer` と同じ） |
| `tasks:control`（Phase 101） | `task_retry` / `task_cancel`（`POST /tasks/{id}/retry` / `/cancel` と同じ） |
| `tasks:decide`（Phase 101） | `task_approve` / `task_reject`（`POST /tasks/{id}/approve` / `/reject` と同じ） |
| `console:instruct` | `console_instruct` / `console_reply` |
| `org:read` | `org_list` / `org_get` |
| `org:write` | `org_create_node` / `org_mount_skill` / `org_unmount_skill` |
| `skills:read` | `skills_list` / `skills_get` |
| `skills:write` | `skills_put` |

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
  - **置き場のガード**（Phase K-1。`docs/knowledge.md` §2.1。`celerisctl knowledge record` と知識整理 run と
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
- **タスクの操作**（Phase 101。外部エージェント（ChatGPT・RDC 経由）向けに、汎用 curl で HTTP API を
  叩かせず、scope で許した操作だけをさせる。いずれも `task-api` の HTTP ハンドラと**同じ** `task-ops` の
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
  - `decision_answer { id, option?, note? }`（scope `tasks:interact`。`POST /decisions/{id}/answer` と同じ。
    `task_answer` と同じ重さ: 待っている仕事を進めるだけで、run を止めない・複製しない）。`option` は決定の
    `options[].key` のどれか。`kind = choice` の決定だけ `option` を省いて `note` に自由記述で答えられる
    （記録される `option` は `other`）。`DecisionAnswered.by` は `mcp:<client_id>`。効き目は決定の種類と選択肢で
    決まる（ADR-0079 付記「R3a 実装時の逸脱・明確化」の表: 待っていた unit を進める / limit の `raise-once`・
    `replan`・`withdraw` / plan_invalid の `replan`・`atomic`・`cancel`）。回答済み・取り下げ済み・選択肢の外は
    `-32602`、無い id は `-32001`。取り下げ（`withdraw`）と revise は MCP には出さない（人が GUI / API で行う）。
    例: `{"name":"decision_answer","arguments":{"id":"01M4…","option":"org-vault","note":"まず試験用で"}}`。
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
- **Console**: `console_instruct { text, project_id? }`（人の発言と同じ経路で CoS に渡す。発言の
  `author` は `mcp:<client_id>`。返値は `message_id` / `task_id`）、
  `console_reply { task_id, wait_secs? }`（`wait_secs` 上限 60。対話 run が終わっていれば `state: "done"`
  + `reply` + `actions[]`、失敗なら `state: "failed"`、終わっていなければ `state: "pending"`）。
- **組織**: `org_list {}`、`org_get { node_id }`（実効 profile。`skills_mounts` を含む）、
  `org_create_node { parent_id, id, name, profile? }`（`profile` は `skills` / `knowledge` /
  `skills_mounts` / `harnesses` / `model` / `policy` / `run` だけ反映される。`tools` / `permissions` /
  `review` を送っても**無視される**）、`org_mount_skill { node_id, skill }` /
  `org_unmount_skill { node_id, skill }`。
- **skills**（KB の `skills/<name>/SKILL.md`。Phase 79 で run に届く。Phase 78 では置き場だけ）:
  `skills_list {}`、`skills_get { name }`、
  `skills_put { name, skill_md, files? }`（frontmatter に `name` / `description` 必須。名前は
  `[a-z0-9-]{1,64}`。出典 `mcp:<client_id>` を frontmatter に残す。mount されるまで何にも効かない）。

**resources**（読むだけの客のため。`resources/read { uri }` は対応する `*_get` と同じものを返す）:
`celeris://knowledge/<path>`、`celeris://tasks/<id>`、`celeris://projects/<id>`、
`celeris://org/<node_id>`、`celeris://skills/<name>`。`resources/list` は**列挙できるもの**（知識の
索引・組織・skills）だけを返す（タスク・案件は件数が大きいので、`tasks_list` / `projects_list` で id を
知ってから `resources/read` で読む）。

## 6. 監査と流量制限

- すべての `tools/call` を `mcp_calls`（`client_id` / `tool` / `ok` / `error_kind` / `latency_ms` /
  `at`）に残す（引数と結果の本文は残さない）。`console_instruct` は Console にも出るので二重には
  書かない。
- クライアントごとに 1 分あたり `[mcp] rate_limit_per_min`（既定 60）。超えたら JSON-RPC エラー
  `-32000` + `data.retry_after`（秒）。
- 管理 API（`docs/gui/api.md` §3.110〜3.111）: `GET /mcp/clients`（トークンは出ない）、
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

（Claude Code CLI のバージョンによってフラグ名が変わることがある。`claude mcp add --help` で確認。
このセッションでは実機の Claude Code CLI での検証はできていない — ADR-0009 P-34、`docs/PROGRESS.md`
の実機節を参照）。

### 7.3 Codex（`mcp_servers` 設定）

Codex CLI の設定ファイル（`~/.codex/config.toml` 等）に:

```toml
[mcp_servers.celeris]
url = "http://127.0.0.1:18200/mcp"
headers = { Authorization = "Bearer <トークン>" }
```

（キー名は Codex CLI のバージョンに依存する。この設定例も実機未検証）。

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

## 8. Remote Desktop Commander 経由（ChatGPT）（Phase 101）

ChatGPT からは Remote Desktop Commander（RDC）というリモート MCP で home-dev のシェルを叩ける。
汎用 `curl` で HTTP API を直接叩かせず、**MCP client の scope で許される操作だけ**をさせるための構成
（ADR-0056 Phase 101 追記）。

```
ChatGPT
  └─ RDC device agent（専用 Linux ユーザー chatgpt-rdc、権限最小）
       └─ scripts/rdc/celeris-chat（薄い入口。celerisctl mcp call をそのまま呼ぶだけ）
            └─ 127.0.0.1:18200/mcp（token 付き。auth = "token"）
                 └─ Celeris MCP（crates/celeris-mcp）
```

### 8.1 専用 Linux ユーザーの権限最小化

RDC の device agent は `chatgpt-rdc` という**専用の**ユーザーで動かす（rmaeda 本人のアカウントでは
動かさない）。そのユーザーに与えるのは:

- `scripts/rdc/celeris-chat` を実行できること（と、そのスクリプトが読む `celerisctl` バイナリ・
  token ファイル）。
- **それ以外は何も無い**: `sudo` 無し、SSH 鍵無し（クラスタにもどこにも入れない）、`~/.config/celeris` /
  `~/.local/celeris` の DB（`celeris.sqlite3`）への直接アクセス無し（読み書きとも不可。celeris の状態は
  MCP のツール経由でしか見えない・変えられない）。
- token ファイル（`CELERIS_MCP_TOKEN_FILE`、既定 `$HOME/.config/celeris/mcp-token`）だけを、
  `chatgpt-rdc` から読める権限（他人には読ませない。`0400` 等）で置く。

`celerisctl` バイナリ自体（既定 `$HOME/.local/celeris/current/bin/celerisctl`）は rmaeda のホーム下に
あるため、`chatgpt-rdc` からは既定では読めない。専用ユーザーのホーム配下にコピーするか、
`~/.local/celeris` に対して専用ユーザーへの読み取り権を足すか、`CELERIS_BIN` で別の場所を指すかは
運用側（親）が決める（`scripts/rdc/celeris-chat` の先頭コメントにも同じ注意を書いてある）。

### 8.2 推奨 scope

```
$ celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client add chatgpt-rdc \
    --scope knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct
```

- 与える: `knowledge:read`、`knowledge:propose`、`tasks:read`、`tasks:interact`、`console:instruct`。
- **与えない**: `tasks:control`（やり直し・取り消し）、`tasks:decide`（承認・却下）、`org:write`、
  `skills:write`。これらは人が GUI から行う決定的な操作で、ChatGPT に渡す理由がない（欲しくなったら
  この節の scope を明示して広げる。既定では絞る）。
- この口は `auth = "token"`（§2 の 18200）を使う（`auth = "none"` の 18201 は同じホストの ChatGPT の
  Secure MCP tunnel 専用。RDC とは別経路）。

### 8.3 `celeris-chat` の呼び方

`chatgpt-rdc` のシェルからはこのスクリプトだけを呼ばせる（§7.5 の `celerisctl mcp call` の薄い
ラッパ。判断や検証は持たない）:

```
$ celeris-chat --list
$ celeris-chat tasks_list '{"status":"blocked","limit":5}'
$ celeris-chat knowledge_search '{"query":"pegasus"}'
$ celeris-chat console_instruct '{"text":"調査結果をタスクにして"}'
$ celeris-chat console_reply '{"task_id":"<上の task_id>","wait_secs":30}'
$ celeris-chat task_comment '{"id":"<task ulid>","text":"見ました。続けてください"}'
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
"token"` の口（§2 の 18200）と、§8.2 で絞った専用クライアント（`chatgpt-rdc`）のトークンを使うこと。

## 9. 実機での確認手順（このセッションでは未実施。ADR-0009 P-34）

```
$ celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client add chatgpt

$ TOKEN=<上で出たトークン>
$ curl -sS -X POST http://127.0.0.1:18200/mcp \
    -H "content-type: application/json" -H "authorization: Bearer $TOKEN" \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' -D -
# Mcp-Session-Id: <セッション id> がヘッダに出る

$ SESSION=<上のヘッダの値>
$ curl -sS -X POST http://127.0.0.1:18200/mcp \
    -H "content-type: application/json" -H "authorization: Bearer $TOKEN" \
    -H "mcp-session-id: $SESSION" \
    -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'

$ curl -sS -X POST http://127.0.0.1:18200/mcp \
    -H "content-type: application/json" -H "authorization: Bearer $TOKEN" \
    -H "mcp-session-id: $SESSION" \
    -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
          "name":"knowledge_propose",
          "arguments":{"title":"test","body":"実機確認","scope":"experience"}
        }}'
# 応答の content[0].text の JSON に "path": "_inbox/...md" が入る

$ curl -sS -X POST http://127.0.0.1:18200/mcp \
    -H "content-type: application/json" -H "authorization: Bearer $TOKEN" \
    -H "mcp-session-id: $SESSION" \
    -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
          "name":"console_instruct",
          "arguments":{"text":"実機確認: 今の時刻を一言で答えて"}
        }}'
# task_id を控え、CoS が返事するまで数十秒待ってから:
$ curl -sS -X POST http://127.0.0.1:18200/mcp \
    -H "content-type: application/json" -H "authorization: Bearer $TOKEN" \
    -H "mcp-session-id: $SESSION" \
    -d '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
          "name":"console_reply",
          "arguments":{"task_id":"<上の task_id>","wait_secs":30}
        }}'
```

結果は `docs/PROGRESS.md` の Phase 78 節に追記する（トークンの値は書かない）。
