# celeris HTTP API v1 仕様

実行・計画・木・決定の要求のエンドポイントは §3.125 にある。

- 状態: **Accepted**（人間の決定 H1 / H5〜H7。celeris 側の ADR-0013、GUI 側の ADR-GUI-0001）。改訂日 2026-09-14
- 改訂: 2026-09-21 Phase 82（ADR-0056 D3 続き、skills を GUI から見る・作る・mount する）
  — **追加のみ。v1 のまま**。エンドポイント 101〜106: `GET /skills`・`GET /skills/{name}`・
  `PUT /skills/{name}`・`DELETE /skills/{name}`・`POST /org/{id}/skills`・
  `DELETE /org/{id}/skills/{skill}`（§3.112〜3.117）。mount / unmount は celeris-mcp の
  `org_mount_skill`/`org_unmount_skill` と**同じ** `task_ops::knowledge::set_skill_mount` を呼ぶので
  挙動は同一。スキーマ・DB マイグレーションの変更は無い。
- 改訂: 2026-09-21 Phase 78（ADR-0056 D1/D2/D4/D5、外部エージェントが Celeris を操作する MCP サーバー）
  — **追加のみ。v1 のまま**。エンドポイント 99〜100: `GET /mcp/clients` / `GET /mcp/calls?client=`
  （§3.110〜3.111。MCP クライアント表と呼び出しログの観測。トークンの値は出ない）。MCP サーバー本体
  （`POST /mcp`。JSON-RPC 2.0 + MCP Streamable HTTP）は `[mcp] listen` の既定 `127.0.0.1:18200` の
  別ポートで、この `/api/v1` 契約には含まれない（`docs/guides/mcp.md` 参照）。DB のスキーマ版数は **24**
  （migration 0024: `mcp_clients` / `mcp_calls`）。`Message.metadata` に `author`（`mcp:<client_id>`。
  MCP の `console_instruct` が付ける）、`Profile`/`EffectiveProfile` に `skills_mounts`（ADR-0056 D3
  のデータモデルのみ。届け方は Phase 79）が増えた。`GET /console` の `human` ブロックに `author` が
  増えた（§3.98）。
- 改訂: 2026-09-21 Phase 67（ADR-0054 D1、ノードごとの継続セッションと resume）— **追加のみ。v1 のまま**。
  エンドポイント 98: `POST /console/new-conversation`（§3.109。CoS の継続セッションを捨てる。**管理系**、
  204、本文なし）。DB のスキーマ版数は **23**（migration 0023: `node_sessions`。ノードごとの継続セッション
  の目印。本文・トークンの値は書かない）。CoS の対話・部門長のレビュー run 自体の挙動（`--resume` 等、
  前置きの差分化）は `GET /console` / `POST /console/instruct`（§3.98 / §3.107）の応答の形を変えない
  （継続かどうかはサーバ内部の判断で、API の応答契約には出ない）。
- 改訂: 2026-09-21 Phase 65（ADR-0053 D1/D4、LLM source のローカル OpenAI 互換プロキシ）— **追加のみ。
  v1 のまま**。エンドポイント 97: `GET /llm/sources`（§3.108。供給元ごとの到達性・アカウントの残量・
  cooldown・直近 1 時間の要求/token 数。読み取りだが認証は必要。`[llm_proxy]` が無効なら 409
  `llm_proxy_unavailable`）。GUI の表示は Phase 66。プロキシ自体（`POST /v1/chat/completions` 等）は
  `127.0.0.1:18100` の別ポートで、この `/api/v1` 契約には含まれない（`docs/guides/llm-source.md` 参照）。
  DB のスキーマ版数は **22**（migration 0022: `llm_proxy_requests`。プロキシの要求記録。本文は書かない）。
- 改訂: 2026-09-21 バグ報告の対応（昇格の成否が画面に出なかった）— **追加のみ。v1 のまま**。
  `GET /releases` の `items[]` に `promote_failed`（`promote_failed.json`。直近の昇格の試みが
  失敗した記録）が増えた。§3.66〜3.67。
- 改訂: 2026-09-20 Phase 59（ADR-0046、組織 = Agent Profile の継承木）— **追加のみ。v1 のまま**。
  (1) 組織のノードが `profile` を持つようになった（`OrgNode.profile`、`GET /org` の `effective_profiles[]`、
  `POST /org` と `PATCH /org/{id}` の `profile`。§3.42〜3.44）。
  (2) タスクに `skills` / `mode` が増え、`harness`（= `Task.genre`）が編集できるようになった
  （`POST /tasks`、`PATCH /tasks/{id}`。§3.74）。
  (3) イベント種別 `assigned`（matching が担当を決めた。`{node, score, reason}`）が増えた。
  DB のスキーマ版数は **16**（migration 0016: `org_nodes.profile_json` / `tasks.skills_json` / `tasks.mode`）。
  設定は `[[genres]]` + `[[roles]]` から `[[harnesses]]` に移った（互換の読み込みは残る。GUI からは
  `GET /config` の `genres[]` がそのままハーネスの一覧として見える）
- 改訂: 2026-09-19 Phase 57（ADR-0044 B3、文書）— **追加のみ。v1 のまま。DB は変わらない**（正本は
  git のファイル）。案件の文書が GUI から読み書きできるようになった: エンドポイント 81〜86
  （`GET /projects/{id}/docs`、`GET|PUT|DELETE /projects/{id}/docs/page`、
  `POST /projects/{id}/docs/init`、`POST /tasks/{id}/artifacts/promote`。§3.92〜3.97）。
  `GET /tasks/{id}/timeline` に `kind = "doc"`（逆リンク）が増えた。エラーコードに
  `docs_unavailable`（409）・`etag_mismatch`（409）・`page_exists`（409）・`page_not_found`（404）が増えた。
  **変更系は管理系**（`token_file` 未設定でも 401）で、**組織の「人」は呼べない**
  （ワーカーは自分の worktree のブランチに文書を書き、人が取り込む）
- 改訂: 2026-09-19 Phase 55（ADR-0044 B2 = D6、中止・一時停止・アーカイブ / §5 Phase 53 追記）—
  **追加 + 認可の破壊的変更**。(1) 案件・途中目標を止められるようになった: エンドポイント 73〜80
  （`POST /projects/{id}/{cancel|pause|resume|archive|unarchive}`、
  `POST /milestones/{id}/{cancel|pause|resume}`。§3.84〜3.91）、`ProjectStatus` に `cancelled`、
  `MilestoneStatus` に `paused` / `cancelled`、`Project.{archived_at, paused_from}`、
  `Milestone.paused_from`、`GET /projects?archived=` と `GET /tasks?archived=`、
  `Event::Transitioned.reason` に `project_cancelled` / `milestone_cancelled`。
  DB のスキーマ版数は **15**（migration 0015: `projects.archived_at` / `projects.paused_from` /
  `milestones.paused_from`）。(2) **変更を伴うエンドポイントを 1 つ残らず管理系に揃えた**
  （§1.3。`POST /tasks`、`approve` / `reject` / `answer` / `cancel` / `retry`、`POST /plans`、
  `POST /replay`、`PATCH /projects/{id}`、`POST /projects/{id}/milestones`、`PATCH /milestones/{id}` が
  トークン必須になった。**v1 の破壊的変更**。GUI は BFF がトークンを持つので画面は変わらない）。
  (3) 走っている run の**止め方が 1 つになった**: `cancel` / 人のコメントによる割り込み /
  タイムアウト / リース喪失 / drain のどれも、ワーカーの**プロセスグループ**へ SIGTERM →
  `kill_grace_secs` → SIGKILL（ハーネスが起こした孫まで消える）
- 改訂: 2026-09-19 Phase 54（ADR-0043 A2、変更の取り込み）— **追加のみ。v1 のまま**。タスクが
  作ったブランチを**人が**見て取り込めるようになった: エンドポイント 68〜72
  （`GET /tasks/{id}/changes`、`GET /tasks/{id}/changes/{repo}/diff`、
  `POST /tasks/{id}/changes/{repo}/integrate`、`POST /tasks/{id}/changes/{repo}/pr/merge`、
  `GET /projects/{id}/integrations`。§3.79〜3.83）。取り込みの記録は `task_integrations`
  （DB のスキーマ版数は **14**。migration 0014）。エラーコードに `default_branch_busy`（409）と
  `pr_unavailable`（409）が増えた。**取り込みの変更系は管理系**（`token_file` 未設定でも 401）で、
  **組織の「人」＝ワーカーからは呼べない**（ワーカープロトコルには出していない）。
  `config.toml` に `[github]`（`gh` / `merge_method`）が増えた
- **マージ（Phase 54）**: Phase 53（ADR-0044 B1）と並行で進んだ枝なので、番号を整えた。
  **ADR-0043 A2 は §3.79〜3.83（エンドポイント 68〜72）**、B1 の §3.74〜3.78（63〜67）はそのまま。
  migration は **0013 = B1（`task_comments`）、0014 = A2（`task_integrations`）**で `SCHEMA_VERSION = 14`。
  `GET /tasks/{id}/timeline`（§3.78）の `integration` は A2 の記録から**実際に出るようになった**
- 改訂: 2026-09-19 Phase 53（ADR-0044 B1、タスク管理）— **追加のみ。v1 のまま**。人がタスクを手で触れる
  ようになった: エンドポイント 63〜67（`PATCH /tasks/{id}`、`GET|POST /tasks/{id}/comments`、
  `POST /tasks/{id}/reopen`、`GET /tasks/{id}/timeline`。§3.74〜3.78）、`Task.labels[]` / `Task.category`、
  `TaskSummary.{labels, category, priority_label, project_id, milestone_id}`、`TaskDetail.priority_label`、
  `GET /tasks` の絞り込み（`labels` / `categories` / `milestone_id` / `tiers` / `priorities` / `q`）、
  イベント種別 `edited`、`Action::{Edit, Reopen}`、`RunOutcomeKind::Interrupted`。
  `POST /tasks` の既定が `ready`（人の経路だけ。`celerisctl add` と計画・委譲は従来どおり `draft`）、
  `priority` の既定が **P2（= 10）**。DB のスキーマ版数は 13（migration 0013: `task_comments` と
  `tasks.labels_json` / `tasks.category`）
- **マージ（Phase 52 + 53）**: 両方の節が同じ番号を取っていたので、**ADR-0043 A1 が §3.68〜3.73
  （エンドポイント 57〜62）、ADR-0044 B1 が §3.74〜3.78（エンドポイント 63〜67）**に整えた。
  `PATCH /tasks/{id}`（§3.74）は ADR-0043 D2 の **`repos`** も受ける
- 改訂: 2026-09-19 Phase 52（ADR-0043 A1、ワークスペース）— **追加のみ。v1 のまま**。案件が「リポジトリ」を
  複数持てるようになった（`project_repos`）: エンドポイント 57〜60（`GET|POST /projects/{id}/repos`、
  `PATCH|DELETE /repos/{id}`）、`ProjectDetail.repos[]`、`POST /tasks` の `repos`（名前の配列）、
  `Task.repos[]`（`{repo_id, name}`）。タスクの作業ツリーが GUI から読めるようになった:
  エンドポイント 61〜62（`GET /tasks/{id}/tree`、`GET /tasks/{id}/tree/file`。読み取り。トークン不要）。
  **`Project.workspace` は primary のリポジトリの `location` の写し**（GUI の後方互換。従来どおり読める）。
  DB のスキーマ版数は 12（migration 0012: `project_repos` と `tasks.repos_json`）。
  worktree のブランチ接頭辞の既定が `celeris/` → **`celeris/`**、`workspace_root` の既定が
  **`~/.local/celeris/workspaces`** に変わった（ADR-0042 D3。どちらも `config.toml` で変えられる）。
  worktree は**終端では消えなくなり**、**中止（`POST /tasks/{id}/cancel`）で worktree とブランチが消える**（ADR-0043 D2）
- 改訂: 2026-09-19 Phase 51（ADR-0041 D5）— **型は変わらない**。`mode = "verify"` のプロセスに限り、
  `GET /config` の `roles[]` / `genres[]` / `providers[]` に組み込みの `smoke`（`adapter = "fake"`）が
  1 つずつ増え、`reviewer` が `{adapter: "fake", tier: "standard"}` になる（`Config::apply_verify_smoke`。
  設定ファイルに同じ id があっても上書きする）。`mode = "normal"`（本番）の応答は 1 バイトも変わらない。
  `POST /tasks` はそのプロセスで `{"genre":"smoke","role":"smoke"}` を受け付け、verify の celeris はその
  タスク**だけ**を dispatch する（§3.1、`docs/ops/selfdeploy.md` の検査 6）
- 改訂: 2026-09-19 Phase 50（ADR-0041 D3/D4）— `GET /releases` の `items[]` に `promoted_at` / `on_main` /
  `changes`、`POST /releases/{sha12}/promote` の応答に `script_from` を追加（追加のみ。v1 のまま）。
  昇格に使う `promote.sh` は**いま動いている版のもの**に変わった（§3.67）
- 改訂: 2026-09-18 Phase 38（ADR-0028 追記）— `GET /config` の `genres[].{input_artifacts, output_artifacts}` の
  各要素は `"名前"` に加えて **`"名前: 説明"`** の形も取る（型は `Vec<String>` のまま。追加のみ、v1 のまま）。
  **GUI は `:` の前を成果物の名前として扱い、後ろを説明として出すこと**（`ConfigView` / `GenreConfigView` の
  型は変えていない。値の文字列に説明が付くだけ）。
- 改訂: 2026-09-18 Phase 31（実機の事故: 失敗した仕事をやり直す手段が無かった）— `POST /tasks/{id}/retry`
  （§3.63）、イベント種別 `retried`、`task_ops::actions` に `Action::Retry` を追加（追加のみ。v1 のまま）
- 改訂: 2026-09-17 Phase 27（Phase 24/25 の監査対応、GUI-R3/R4、ADR-0034 D7）— `TaskSummary.assignee` /
  `TaskSummary.conversation`、`ProjectTaskView.conversation`、`Message.task_id` を追加（追加のみ。v1 のまま）。
  **`POST /projects` を管理系に変更**（`token_file` 未設定でも 401。破壊的変更はここだけ）。
  `POST/PATCH /org` の `genre` は `[[genres]]` にあるものだけ（無い id は 422）。
  `GET /approvals?pending=false` が「決定済みだけ」に絞り込むようになった（R5。以前は全件）。
  DB のスキーマ版数は 7（migration 0007: `messages.task_id` の追加と `reports.project_id` の NULL 可）
- 改訂: 2026-09-17 Phase 18（ADR-0028 D1/D4、分野を能力レジストリに）— `GET /config` の
  `genres[].{capabilities, input_artifacts, output_artifacts}` を追加（3 つとも任意、空なら省略。追加のみ。v1 のまま）
- 改訂: 2026-09-17 Phase 16（ADR-0027 D1、分野）— `POST /tasks` の `genre`、`TaskSummary.genre` / `TaskDetail.genre`、
  `GET /tasks` の `genre=` クエリ、`GET /config` の `genres[]` を追加（追加のみ。v1 のまま）
- 改訂: 2026-09-16 実機確認の反映 — エンドポイント 33（`runs/{run_id}/prompt`）、`RunFiles.prompt`、
  `ProviderCheckResponse.detail` / `last_check.detail` を追加（追加のみ。v1 のまま）
- 改訂: 2026-09-16 ADR-0023（run の指示の保存・子待ちの可視化）— エンドポイント 32（`runs/{run_id}/request`）、`RunFiles.request`、
  `DaemonSnapshot.awaiting_children[]` を追加（追加のみ。v1 のまま）
- 改訂: 2026-09-16 ADR-0022（疎通確認の記録）— `GET /providers` の `last_check` を追加（追加のみ。v1 のまま）
- 改訂: 2026-09-16 ADR-0021（委譲した子の失敗）— イベント種別 `question_raised`、`GET /config` の `delegation.on_child_failure` を追加（追加のみ。v1 のまま）
- 改訂: 2026-09-15 Phase 10（ADR-0016 役割と委譲）— `POST /tasks` の `role` / `aggregate`、`TaskDetail.role` / `delegated[]`、`GET /config` の `roles[]` / `delegation`、イベント種別 `delegated` を追加（全て追加のみ。v1 のまま）
- 提供者: **celeris**（crate `task-api`、axum）。celeris のデーモンプロセス内で、`config.toml` に `[api]` 節があるときだけ動く
- 利用者: `celeris-gui` の BFF（Remix = React Router framework mode のサーバ側 loader / action）と `curl`。**ブラウザは直接呼ばない**
- 正の型定義: Rust（`task-core` / `task-ops` / `task-api`、`serde` + `schemars`）。JSON Schema を `docs/api/v1/api-v1.schema.json` にコミットし、
  テストで生成一致を検証する（§7）。`celeris-gui` はこのファイルから TypeScript の型を生成する
- 実装の順序: celeris の Phase 9a（基盤: task-ops / WAL / events の id / スキーマ版数 / ProviderThrottled / list_page）→ 9b（本仕様）。
  GUI の G フェーズは 9b 完了後に始める（DESIGN-GUI §10）

本文書は celeris の実装者が**これだけで実装とテストを書ける**ことを目標にする。各エンドポイントの要求・応答の型、ステータスコード、境界条件を書く。
派生値（受信箱、詳細、run の要約、プロバイダの集計）の**計算規則は celeris 側（`task-ops` / `task-api`）にある**。GUI は再計算しない。

---

## 1. 全体

### 1.1 有効化と設定（`config.toml`）

```toml
[api]
listen = "127.0.0.1:7710"      # これを書いたときだけ API が動く（Option<SocketAddr>。無ければ無効）。推奨ポートは 7710（GUI は 7700）
# token_file = "secrets/api.token"       # 非 loopback で listen するとき必須。指定があれば loopback でも要求する。相対パスは設定ファイル基準
# allowed_hosts = ["celeris.lab.example"]  # Host 許可リストへの追加（localhost / 127.0.0.1 / [::1] とポート付きの形は常に許可）
```

- `listen` が無ければ API は動かない（既定は無効。デーモンに暗黙のネットワーク口を開けない）。設定の型は `crates/celeris/src/config/api.rs` の `ApiConfig{listen, token_file, allowed_hosts}`、task-api が受け取るのは `task_api::ApiSettings`（`crates/task-api/src/lib.rs`）。
- `listen` が loopback（`127.0.0.0/8`、`::1`）以外で `token_file` が無い → `ConfigError::Invalid`（`[api] listen = … is not a loopback address; token_file is required`。起動時 exit 2）。
- `token_file` の内容（前後の空白を除いた 1 行）がトークン。ファイルが読めない・空 → 設定エラー。トークンはログにも API にも出さない（task-api は SHA-256 の値だけを持つ）。
- SSE の同時接続数の上限は task-api の定数 `MAX_STREAMS = 16`（設定キーにしない）。
- API は**自分専用の `SqliteStore` 接続**を持ち、DB 呼び出しは `spawn_blocking` で行う（ディスパッチャの接続と Mutex を共有しない。ADR-0013 D3）。
- `providers_include = "providers.d/*.toml"`（トップレベル、`[[providers]]` と併用可。ADR-0017 M1）を設定すると、
  §3.24〜3.28 のプロバイダ管理エンドポイントが `providers.d/<id>.toml`（1 アカウント 1 ファイル、ファイル名昇順で
  `[[providers]]` の後ろに連結）を読み書きできるようになる。未設定なら 5 本とも 409。
- `[secrets] dir = "secrets"`（ADR-0030 D1。GUI から API キー等を預かる置き場所）を設定すると、§3.36〜3.38 の
  秘密管理エンドポイントが `dir` 配下に 1 秘密 1 ファイル（0600）で読み書きできるようになる。未設定なら 3 本とも
  409 `secrets_unavailable`。値を環境変数として run に流し込む `env_from_secrets` は `[adapters.<種別>]` と
  `[[providers]]` の行の両方に書ける（§3.36 参照）。

### 1.2 プロトコルと共通規約

| 事項 | 決め |
|---|---|
| ベースパス | `/api/v1`。以下のパスは全てこれに続く |
| 転送 | HTTP/1.1。要求・応答とも `application/json; charset=utf-8`。サーバ → クライアントの通知は SSE（`text/event-stream`）。WebSocket / gRPC / HTTP/2 は使わない |
| CORS | **出さない**。`Access-Control-*` ヘッダは一切付けない。プリフライト（`OPTIONS`）は認証より前に 405 `method_not_allowed`（Host 検査の後） |
| 共通応答ヘッダ | `Cache-Control: no-store`、`X-Content-Type-Options: nosniff`、`X-Request-Id: <ULID>`（Problem の `instance` と同じ） |
| ID | `TaskId` / `run_id` = ULID 文字列（26 文字、Crockford base32 `^[0-9A-HJKMNP-TV-Z]{26}$`）。イベントの `id` = 64 bit 整数（DB 全体で単調）。`seq` = 64 bit 整数（タスク内で 0 始まり） |
| 時刻 | RFC 3339、UTC、`Z` 終端、秒以下は任意桁（`time::serde::rfc3339` の出力そのまま）。`events.ts` / `created_at` / `updated_at` も同じ |
| 列挙 | `Status` / `TaskKind` / `Tier` は `snake_case` の文字列。`Check` / `Event` / `WorkspaceSpec` は tagged（`type` / `type` / `kind`）。**task-core の serde 表現そのまま**（§6） |
| ページング | keyset。`limit` の既定と最大はエンドポイントごと（`GET /tasks` は既定 100・最大 500、`GET /events` 系は既定 500・最大 5,000）で、最大を超えれば最大に丸め、`0` は 400。不透明な `cursor`（応答の `next_cursor`）。並び順はエンドポイントごとに固定 |
| 要求本文 | `POST` / `PUT` / `PATCH` は本文が空でも `Content-Type: application/json` 必須（無ければ 415。`DELETE` は検査しない）。上限 1 MiB（`MAX_BODY_BYTES`。`Content-Length` が超えれば middleware が、読みながら超えればハンドラが 413）。本文を取らない操作の多くは空の本文を `{}` とみなす。未知のフィールドは 400（`deny_unknown_fields`） |
| 楽観的検査 | 変更系は `expected_status` を受け取れる。現在の `status` と違えば 409 `conflict`（状態は変えない） |
| 冪等性 | 変更系は冪等ではない（同じ承認を 2 回送れば 2 回目は 409 `invalid_transition`）。`expected_status` を付けるのが正 |

### 1.3 認証（Bearer）

- `token_file` が設定されていれば全エンドポイント（`GET /health` を除く）で `Authorization: Bearer <token>` を要求する（scheme は大文字小文字を区別しない）。無い・違えば 401 `unauthorized`（`WWW-Authenticate: Bearer realm="celeris"`）。比較は SHA-256 の値どうしを定数時間で行う。
- `token_file` が無い（= loopback のみ）場合は認証しない。**ただし管理系エンドポイントは例外**で、`token_file` が無くても常に 401 にする（ADR-0017 D1: loopback でも管理操作にはトークンを要求する）。管理 API を使うには `token_file` の設定が要る。
- **Phase 55（ADR-0044 §5 Phase 53 追記）から、人の変更操作のエンドポイントは管理系**（ハンドラが `middleware::require_admin` を呼ぶ）。読み取り（`GET`）は従来どおりで、`token_file` が無ければ loopback から素通しのまま。
  - 例外: browser の Live View（`/tasks/{id}/browser/live/…`）と制御（`/tasks/{id}/browser/control/…`）は `require_admin` を使わず、middleware の Bearer に加えて GUI 署名の assertion（人の操作）か daemon の bearer（worker）で認可する。`token_file` が無い構成では制御は 403 `browser_control_disabled`、Live View の追記も拒否する（ADR-0099 D3）。
  - この Phase で管理系に揃えたもの（それまでトークン不要だった）: `POST /tasks`、`POST /tasks/{id}/{approve|reject|answer|cancel|retry}`、`POST /plans`、`POST /replay`、`PATCH /projects/{id}`、`POST /projects/{id}/milestones`、`PATCH /milestones/{id}`。**v1 の破壊的変更**（冒頭の変更点一覧）。
  - GUI は BFF がトークンを持つ（`CELERIS_API_TOKEN_FILE`）ので画面は変わらない。`celerisctl` は HTTP API を使わず SQLite を直接開くので影響しない。組織の「人」（ワーカー）はそもそも API を叩かない（SPEC §3.6）。
- `GET /health` は常に無認証（版とスキーマ版数だけを返す。G0 の疎通確認用）。ただし Host 検査は受ける。

### 1.4 Host 検査・Origin・CSRF

- 全要求で `Host` ヘッダと（absolute-form の）URI の authority の両方を許可リスト（`localhost`、`127.0.0.1`、`[::1]`、`listen` のホスト、`allowed_hosts`。ポートは無視、大文字小文字は区別しない）と照合し、外れれば 400 `host_not_allowed`。`Host` が無い・複数ある・ASCII でないときも 400。DNS rebinding 対策。検査の順は Host → `OPTIONS` の 405 → Bearer → Origin / Content-Type / 本文サイズ（`crates/task-api/src/middleware.rs`）。
- 変更系（`POST`/`PUT`/`PATCH`/`DELETE`）に `Origin` ヘッダが付いていれば 403 `origin_forbidden`。ブラウザから直接呼ばれる設計ではないので、`Origin` の存在自体を「想定外の呼び出し」とみなす（`curl` と Node の `fetch` は `Origin` を送らない）。Content-Type / 本文サイズの検査は本文を伴う `POST`/`PUT`/`PATCH` だけ（`DELETE` は本文を取らない）。
- `Content-Type: application/json` の要求（1.2）と合わせて、フォーム送信型の CSRF は成立しない。

### 1.5 エラー

`application/problem+json`（RFC 9457）。本体は `Problem`:

```json
{"type":"urn:celeris:problem:invalid_transition","title":"invalid transition","status":409,
 "detail":"task 01J… (kind=execute, status=done) cannot be approved","code":"invalid_transition",
 "instance":"urn:celeris:request:01J…","task_status":"done","kind":"execute","trigger":"approve"}
```

| `code` | HTTP | 意味と付加フィールド |
|---|---|---|
| `bad_request` | 400 | JSON 構文誤り、未知フィールド、クエリの型誤り、ULID でない id、`cursor` の解読失敗 |
| `host_not_allowed` | 400 | 1.4 |
| `unauthorized` | 401 | 1.3 |
| `origin_forbidden` | 403 | 1.4 |
| `path_forbidden` | 403 | ファイル系: ワークスペース外・symlink 越え・不正な `run_id`（§3.8） |
| `task_not_found` | 404 | タスクが無い |
| `run_not_found` / `artifact_not_found` / `file_not_found` | 404 | run ディレクトリ / 成果物の添字 / ファイルが無い |
| `not_found` | 404 | 未定義のパス |
| `org_node_not_found` / `project_not_found` / `milestone_not_found` | 404 | 組織のノード / 案件 / 途中目標が無い（ADR-0033、§3.42〜3.49。ULID でない案件・途中目標の id もここ） |
| `report_not_found` | 404 | 報告が無い（ADR-0033 D3、§3.50〜3.53。ULID でない id もここ） |
| `approval_not_found` / `standing_rule_not_found` | 404 | 認可 / 永続の認可が無い（ADR-0033 D5、§3.56〜3.60。ULID でない id もここ） |
| `method_not_allowed` | 405 | |
| `conflict` | 409 | `expected_status` 不一致。`expected`, `actual` |
| `invalid_transition` | 409 | 状態機械または task-ops の写像が拒否。`task_status`, `kind`, `trigger`（`InvalidTransition{status, kind, trigger}` の写し。`trigger` は `Trigger::name()`） |
| `org_node_exists` | 409 | `POST /org` の `id` が既にある（更新は `PATCH /org/{id}`） |
| `org_node_in_use` | 409 | 消そうとした組織のノードが未終了のタスクを抱えている、または子を持つ（ADR-0033 D1） |
| `release_not_found` | 404 | ADR-0040 D6: `POST /releases/{sha12}/promote` の sha12 が `[selfdeploy] releases_dir` に無い（sha12 の形でないときも同じ） |
| `release_not_promotable` | 409 | ADR-0040 D6: 昇格を受け付けられない（`verify.json` が無い／`ok` でない、既に `current`、既に昇格中、`scripts/promote.sh` が無い、`[selfdeploy]` が無い）。`detail` に理由の一行 |
| `default_branch_busy` | 409 | ADR-0043 D5（§3.81）: 人のチェックアウトが取り込み先のブランチを出したまま未コミットの変更を持っている。`detail` は `"<default_branch> が編集中"`。**何も触っていない**ので、人が片付けてからもう一度押す |
| `pr_unavailable` | 409 | ADR-0043 D5（§3.81 / §3.82）: PR の経路が使えない（`origin` リモートが無い、`gh` が PATH に無いか認証されていない、merge しようとした PR が開いていない）。`detail` に理由の一行 |
| `docs_unavailable` | 409 | ADR-0044 D7（§3.92〜3.97）: 案件の文書の根が使えない（まだ無い＝`POST /projects/{id}/docs/init` で用意する、primary がリモート、`$HOME` が無い、`git` が動かない、置き場が空でない）。`detail` に理由の一行 |
| `etag_mismatch` | 409 | ADR-0044 D7（§3.95 / §3.96）: 読んでから誰かがそのページを直した（ページがあるのに `etag` を付けなかったときも同じ）。拡張フィールド `etag` にいまの値。再読み込みしてから直す |
| `page_exists` | 409 | ADR-0044 D7（§3.97）: 昇格の宛先にもうページがある。`overwrite: true` なら上書きできる |
| `page_not_found` | 404 | ADR-0044 D7（§3.93 / §3.96）: そのパスのページが default_branch に無い |
| `removed_by_adr_0079` | 410 | ADR-0079 R5a で撤去した入口（§3.125.8）。拡張フィールド `adr`（`"ADR-0079"`）と `instead`（代わりの入口） |
| `payload_too_large` | 413 | 本文 > 1 MiB |
| `unsupported_media_type` | 415 | 変更系で `Content-Type` が JSON でない |
| `range_not_satisfiable` | 416 | ファイル系の `Range` / `offset` がサイズを超える。`Content-Range: bytes */<size>` |
| `validation` | 422 | task-ops の検証失敗。`errors: [{field?, message}]`。`message` は `celerisctl` と同じ文言（§5.6）。`field` は task-api が文言から推定できるときだけ（`acceptance` / `depends_on` / `goal` / `title` / `objective` / `parent`。`problem::validation_field`） |
| `too_many_streams` | 503 | SSE 接続数が 16 を超えた。`Retry-After: 5` |
| `db_busy` | 503 | `SQLITE_BUSY`（busy_timeout 超過）。`Retry-After: 1` |
| `replay_in_progress` | 503 | 別の `POST /replay` が走っている。`Retry-After: 5` |
| `standby` | 503 | ADR-0040 D4: いまこのプロセスは `standby`（または `draining`）なので、ディスパッチャの状態を要する管理系（`POST /reload`、`POST /providers/{id}/check`、クラスタ接続、アカウントの確認・削除・ログイン中継、`POST /notify/test`）を受けられない。`detail` は `"standby"`、`Retry-After: 2`。窓は 1〜2 tick（昇格の引き継ぎ中）なので、GUI はその間だけ「切り替え中」を出して再送すればよい。読み書きの通常のエンドポイントはそのまま動く |
| `internal` | 500 | その他（`detail` にエラー文。スタックやパスは出さない） |

`task_ops::OpsError` からの写像（Phase 9a の型）:

| `OpsError` | HTTP / `code` | 付加フィールド |
|---|---|---|
| `NotFound(id)` | 404 `task_not_found` | — |
| `InvalidState{id, context, action}` | 409 `invalid_transition` | `detail` = `Display`（`task <id> (<context>) cannot be <action>`）、`task_status` / `kind` は現在のタスクから、`trigger` は操作名（`approve` 等） |
| `Validation(msg)` | 422 `validation` | `errors: [{field?, message: msg}]` |
| `Conflict{expected, actual}` | 409 `conflict` | `expected`, `actual` |
| `ProjectNotFound(id)` / `MilestoneNotFound(id)` | 404 `project_not_found` / `milestone_not_found` | — |
| `InvalidLifecycle{..}` | 409 `invalid_transition` | `trigger` |
| `DecisionNotFound(id)` | 404 `decision_not_found` | — |
| `DecisionNotOpen{status, ..}` | 409 `decision_not_open` | `decision_status` |
| `TreeAdopt{conflict, code, ..}` | 409（`conflict`）/ 422 の `adopt_*`（§3.125.4） | — |
| `ProjectPlan*`（案件計画。R5a 以降は入口が 410 なので出ない） | 404 `project_plan_not_found` / 409 `project_plan_already_decided` / `project_plan_in_flight` / `project_plan_stale` | — |
| `Store(InvalidTransition{status, kind, trigger})` | 409 `invalid_transition` | `task_status`, `kind`, `trigger` |
| `Store(Sqlite(busy))` | 503 `db_busy` | — |
| `Store(その他)` | 500 `internal` | — |

`StoreError::InUse` は種類ごとに 409（`repo_in_use`・`execution_plan_in_use`・`project_slug_in_use` など）。`StoreError::SchemaTooNew` は起動時に起きるので API のエラーにはならない（celeris が exit 2）。

上の表は全体に共通のコード。エンドポイント固有のコード（`provider_*`・`account_*`・`secret_*`・`cluster_*`・`knowledge_*`・`skill_*`・`browser_*`・`execution_plan_*`・`adopt_*` など）は §3 の各節にある。正本は `crates/task-api/src/problem.rs` と各ハンドラ。

---

## 2. エンドポイント一覧（174 = 表 168 + browser 制御 6）

`crates/task-api/src` の `.route(…)` の全パス（146 本）をメソッドごとに 1 行で並べる（174 行。パスは `/api/v1` を除いた形）。
番号は追加の順で、§3 の見出しや改訂履歴の「エンドポイント N」はこの番号を指す。#108 以降は 2026-10-02 に router と照らして足した行。
応答型は `docs/api/v1/api-v1.schema.json` の `$defs` の名前（`{…}` は名前の無い JSON の形）。ステータスを書いていない行は 200。
browser 系（#151〜174）の流れは §3.118〜3.124 と `docs/guides/browser-capability.md`。

| # | メソッド | パス | 目的 | 応答型 | 出所 |
|---|---|---|---|---|---|
| 1 | GET | `/health` | 版、スキーマ版数、DB の journal_mode | `Health` | task-api |
| 2 | GET | `/inbox` | 承認待ち / 質問 / draft / 注意 | `Inbox` | task-ops（+ スナップショット） |
| 3 | GET | `/tasks` | 一覧（フィルタ・keyset ページング） | `TaskList` | store `list_page` + task-ops |
| 4 | POST | `/tasks` | `celerisctl add` 相当 | 201 `Task` | task-ops |
| 5 | GET | `/tasks/{id}` | 詳細（`celerisctl show --json` と同一） | `TaskDetail` | task-ops |
| 6 | GET | `/tasks/{id}/events` | そのタスクのイベント（`after_seq`） | `EventsPage` | store `events_for` |
| 7 | GET | `/tasks/{id}/runs` | run の要約一覧 | `RunList` | task-ops + ファイル存在 |
| 8 | GET | `/tasks/{id}/runs/{run_id}/stdout` | `runs/<run_id>/stdout.jsonl` | バイト列 | ファイル |
| 9 | GET | `/tasks/{id}/runs/{run_id}/stderr` | `runs/<run_id>/stderr.log` | バイト列 | ファイル |
| 10 | GET | `/tasks/{id}/runs/{run_id}/result` | `runs/<run_id>/result.json` | `application/json` | ファイル |
| 11 | GET | `/tasks/{id}/artifacts` | `ArtifactProduced` の一覧 + 現在の sha256 | `ArtifactList` | events + ファイル |
| 12 | GET | `/tasks/{id}/artifacts/{idx}` | 成果物本体 | バイト列 | ファイル |
| 13 | POST | `/tasks/{id}/approve` | `celerisctl approve`（draft → Accept / Approval → Approve） | `TransitionResult` | task-ops |
| 14 | POST | `/tasks/{id}/reject` | `celerisctl reject` | `TransitionResult` | task-ops |
| 15 | POST | `/tasks/{id}/answer` | `celerisctl answer` | `TransitionResult` | task-ops |
| 16 | POST | `/tasks/{id}/cancel` | `celerisctl cancel` | `TransitionResult` | task-ops |
| 17 | POST | `/plans` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: `celerisctl plan` 相当 | 410 `Problem`（旧 201 `Task`） | task-ops |
| 18 | POST | `/replay` | `celerisctl replay`（読み取りのみ） | `ReplayReport` | task-ops |
| 19 | GET | `/graph` | DAG（`depends_on` の辺、`parent_id` の入れ子） | `Graph` | task-ops |
| 20 | GET | `/events` | 全タスク横断のイベント（`after_id`。ポーリング / `curl` 用） | `EventsPage` | store `events_since` |
| 21 | GET | `/stream` | SSE（§4） | `text/event-stream` | store `events_since` + スナップショット |
| 22 | GET | `/providers` | 定義 + 稼働状況 + 集計 | `Providers` | 設定 + スナップショット + task-api の集計 |
| 23 | GET | `/daemon` | ディスパッチャのメモリ上のスナップショット | `DaemonView` | `tokio::sync::watch` |
| 24 | GET | `/config` | `config.toml` の要約（秘密は出さない） | `ConfigView` | 設定（celeris が起動時に渡す） |
| 25 | GET | `/schema` | `api-v1.schema.json` の内容 | `application/schema+json` | `include_str!` |
| 26 | GET | `/clusters` | `[[clusters]]` の定義 + 接続の有無 + cooldown（ADR-0018、Phase 12） | `Clusters` | 設定 + スナップショット |
| 27 | POST | `/providers` | `providers.d/<id>.toml` を作る（ADR-0017、Phase 11。**管理系: トークン必須**） | 201 `ProviderConfigView`（`Location`） | celeris（ファイル書き込みのみ） |
| 28 | PATCH | `/providers/{id}` | 並列度・tier・model・env を変更する（**管理系**） | 200 `ProviderConfigView` | celeris（ファイル書き込みのみ） |
| 29 | DELETE | `/providers/{id}` | `providers.d/<id>.toml` を消す（**管理系**） | 200 `{}` | celeris（ファイル削除のみ） |
| 30 | POST | `/providers/{id}/check` | そのアカウントの env で短い疎通確認を 1 回行う（**管理系**） | 200 `ProviderCheckResponse` | celeris（`task-worker` 経由。タスク・イベントには残らない） |
| 31 | POST | `/reload` | 設定と `providers.d/` を読み直し、稼働中のプロバイダ選定・アダプタを差し替える（**管理系**） | 200 `ReloadResult` | celeris（`Dispatcher` の差し替え） |
| 32 | GET | `/tasks/{id}/runs/{run_id}/request` | `runs/<run_id>/request.json`（ワーカーに渡した `RunRequest`。ADR-0023 D2） | `application/json` | ファイル |
| 33 | GET | `/tasks/{id}/runs/{run_id}/prompt` | `runs/<run_id>/prompt.txt`（claude-code / codex に実際に渡した文面。ADR-0023 M1） | `text/plain` | ファイル |
| 34 | GET | `/secrets` | GUI から預かる秘密（API キー等）の一覧。値は含まない（ADR-0030、Phase 20。**管理系**） | `SecretList` | celeris（ファイル読み取りのみ） |
| 35 | PUT | `/secrets/{id}` | 秘密の作成／置き換え（0600）（**管理系**） | 200 `SecretPutResult` | celeris（ファイル書き込みのみ） |
| 36 | DELETE | `/secrets/{id}` | 秘密の削除（**管理系**） | 200 `{}` | celeris（ファイル削除のみ） |
| 37 | POST | `/clusters/{id}/connect` | 接続を開始する（コード不要で張れれば即完了。ADR-0032、Phase 22。**管理系**） | 200 `ClusterConnectStart` | celeris（`ssh -M -N` の起動・借用） |
| 38 | POST | `/clusters/{id}/connect/code` | 進行中の接続に検証コード（TOTP 等）を渡す（**管理系**） | 200 `ClusterConnectResult` | celeris（`SSH_ASKPASS` 経由の中継） |
| 39 | DELETE | `/clusters/{id}/connect` | 進行中の接続を取り消す、または張った接続を切る（**管理系**） | 200 `{}` | celeris（子プロセスの終了） |
| 40 | GET | `/org` | 組織（一つ、役割の木）の全ノード（ADR-0033 D1、Phase 23） | `OrgList` | store `org_list` |
| 41 | POST | `/org` | 役職を足す（**管理系**） | 201 `OrgNode`（`Location`） | store `org_upsert` |
| 42 | PATCH | `/org/{id}` | 役職を変える・付け替える（**管理系**） | 200 `OrgNode` | store `org_upsert` |
| 43 | DELETE | `/org/{id}` | 役職を消す。仕事を抱えていたら 409（**管理系**） | 204 | store `org_delete` |
| 44 | GET | `/projects` | 案件の一覧（新しい順。ADR-0033 D2） | `ProjectList` | store `project_list` |
| 45 | POST | `/projects` | 案件を投げる（`status = proposed`。直後に秘書の run が起きるので**管理系**。Phase 27） | 201 `Project`（`Location`） | store `project_create` |
| 46 | GET | `/projects/{id}` | 案件 + 途中目標 + 仕事の木 | `ProjectDetail` | store（`project_id` で絞った `tasks`） |
| 47 | PATCH | `/projects/{id}` | 案件の状態を変える | 200 `Project` | store `project_set_status` |
| 48 | POST | `/projects/{id}/milestones` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: 途中目標を足す（`seq` はストアが採番） | 410 `Problem`（旧 201 `Milestone`（`Location`）） | store `milestone_create` |
| 49 | PATCH | `/milestones/{id}` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: 途中目標の状態を変える（SPEC §7 のアジャイル） | 410 `Problem`（旧 200 `Milestone`） | store `milestone_set_status` |
| 50 | GET | `/org/{id}/messages` | そのノードとのやり取り（古い順。ADR-0033 D4、Phase 24） | `MessageList` | store `message_list` |
| 51 | POST | `/org/{id}/messages` | そのノードに話しかける（**管理系**） | 202 `MessageAccepted` | `task_ops::conversation::start` |
| 52 | GET | `/notify` | Discord への通知の設定と直近の送信（ADR-0037、Phase 39。URL は出さない） | `NotifyView` | 設定 + store `notification_recent` |
| 53 | POST | `/notify/test` | テスト送信を 1 回（**管理系: `token_file` 未設定でも 401**） | 200 `NotifyTestResult` | celeris（`[secrets]` の webhook へ POST） |
| 54 | POST | `/milestones/{id}/decide` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: 途中目標の判定（`ok` / `discuss` / `ng`。ADR-0038 D2、Phase 41）（**管理系**） | 410 `Problem`（旧 202 `MilestoneDecided`） | `task_ops::milestone_review::decide` |
| 55 | GET | `/releases` | リリース一覧・検証状態・`current`/`previous`・引き継ぎの進行（ADR-0040 D6、Phase 48） | `Releases` | `[selfdeploy] releases_dir` のファイル + store `instance_list` |
| 56 | POST | `/releases/{sha12}/promote` | そのリリースへ昇格する（`promote.sh` を起こして 202）（**管理系: `token_file` 未設定でも 401**） | 202 `ReleasePromoteAccepted` | celeris（`<release>/scripts/promote.sh` を detached で起動） |
| 57 | GET | `/projects/{id}/repos` | その案件のリポジトリ（primary が先頭。ADR-0043 D1、Phase 52） | `RepoList` | store `repo_list` |
| 58 | POST | `/projects/{id}/repos` | リポジトリを足す（**管理系: `token_file` 未設定でも 401**） | 201 `ProjectRepo` | store `repo_create` |
| 59 | PATCH | `/repos/{id}` | リポジトリを変える（名前・場所・`run`・primary。**管理系**） | 200 `ProjectRepo` | store `repo_update` |
| 60 | DELETE | `/repos/{id}` | リポジトリを消す。未終端のタスクが使っていたら 409（**管理系**） | 204 | store `repo_delete` |
| 61 | GET | `/tasks/{id}/tree` | タスクの作業ツリーの一覧（ADR-0043 D6） | `TreeView` | `worktree.json` + ファイル |
| 62 | GET | `/tasks/{id}/tree/file` | そのファイルの本文（テキスト 512 KiB まで） | `TreeFileView` | ファイル |
| 63 | PATCH | `/tasks/{id}` | タスクを編集する（題名・目的・受け入れ条件・優先度・ラベル・種類・リポジトリ・担当・役割・tier・アダプタ・途中目標・依存・予算。ADR-0044 D1 + ADR-0043 D2、Phase 53）（**管理系**） | 200 `TaskEditResult` | `task_ops::edit::edit_task` |
| 64 | GET | `/tasks/{id}/comments` | そのタスクのコメント（古い順。ADR-0044 D2） | `CommentList` | store `comments_for` |
| 65 | POST | `/tasks/{id}/comments` | 人のコメント。状態に応じて**割り込み**・回答・記録になる（**管理系**） | 201 `CommentResult` | `task_ops::comment::post_human_comment` |
| 66 | POST | `/tasks/{id}/reopen` | 終端のタスクを同じ worktree のまま再開する（`done`/`failed` → `ready`）（**管理系**） | 200 `TransitionResult` | `task_ops::comment::reopen` |
| 67 | GET | `/tasks/{id}/timeline` | 起きたこと 1 本（イベント・コメント・認可・報告・委譲・リリース・取り込み。ADR-0044 D5） | `Timeline` | store + `[selfdeploy] releases_dir` |
| 68 | GET | `/tasks/{id}/changes` | リポジトリごとの差分の要約と取り込みの記録（ADR-0043 D5、Phase 54） | `ChangesView` | `worktree.json` + `git` + store |
| 69 | GET | `/tasks/{id}/changes/{repo}/diff` | 1 ファイルの unified diff（200 KiB で切る） | `ChangeDiffView` | `git diff` |
| 70 | POST | `/tasks/{id}/changes/{repo}/integrate` | 取り込む（`merge` / `pr` / `discard`）（**管理系: `token_file` 未設定でも 401**） | 200 `IntegrateResult` | `git` / `gh` + store |
| 71 | POST | `/tasks/{id}/changes/{repo}/pr/merge` | その PR を Celeris から merge する（**管理系**） | 200 `IntegrateResult` | `gh pr merge` + store |
| 72 | GET | `/projects/{id}/integrations` | その案件の PR と取り込み（タスク × リポジトリごとに最新の 1 件） | `ProjectIntegrations` | store + `gh pr view` |
| 73 | POST | `/projects/{id}/cancel` | 案件を中止し、属する非終端タスクと途中目標を連鎖で `cancelled` にする（ADR-0044 D6、Phase 55。**管理系: `token_file` 未設定でも 401**） | 200 `ProjectLifecycle` | `task_ops::lifecycle` |
| 74 | POST | `/projects/{id}/pause` | 案件を一時停止する（属するタスクは dispatch されない。**管理系**） | 200 `ProjectLifecycle` | `task_ops::lifecycle` |
| 75 | POST | `/projects/{id}/resume` | 一時停止を解く（`paused_from` へ戻す。**管理系**） | 200 `ProjectLifecycle` | `task_ops::lifecycle` |
| 76 | POST | `/projects/{id}/archive` | 終端の案件をアーカイブする（一覧から既定で隠れる。**管理系**） | 200 `ProjectLifecycle` | `task_ops::lifecycle` |
| 77 | POST | `/projects/{id}/unarchive` | アーカイブを解除する（**管理系**） | 200 `ProjectLifecycle` | `task_ops::lifecycle` |
| 78 | POST | `/milestones/{id}/cancel` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: 途中目標を中止し、属する非終端タスクを連鎖で `cancelled` にする（**管理系**） | 410 `Problem`（旧 200 `MilestoneLifecycle`） | `task_ops::lifecycle` |
| 79 | POST | `/milestones/{id}/pause` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: 途中目標を一時停止する（**管理系**） | 410 `Problem`（旧 200 `MilestoneLifecycle`） | `task_ops::lifecycle` |
| 80 | POST | `/milestones/{id}/resume` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: 一時停止を解く（**管理系**） | 410 `Problem`（旧 200 `MilestoneLifecycle`） | `task_ops::lifecycle` |
| 81 | GET | `/projects/{id}/docs` | 案件の文書のツリー（`?q=` は `git grep -il`。ADR-0044 D7、Phase 57） | `DocsTree` | `git ls-tree` / `git log` |
| 82 | GET | `/projects/{id}/docs/page` | ページ 1 枚（raw / html / front matter / 履歴 / etag） | `DocPage` | `git show` / `git log` |
| 83 | POST | `/projects/{id}/docs/init` | 文書リポジトリを用意する（無い案件だけ。**管理系**） | 200 `DocsInitResult` | `git init` + store |
| 84 | PUT | `/projects/{id}/docs/page` | ページを既定のブランチに直接コミットする（**管理系**） | 200 `DocPageResult` | 一時 worktree + `git commit` |
| 85 | DELETE | `/projects/{id}/docs/page` | ページを消す（**管理系**） | 200 `DocPageResult` | 一時 worktree + `git commit` |
| 86 | POST | `/tasks/{id}/artifacts/promote` | 成果物をページに昇格する（**管理系**） | 200 `DocPageResult` | 成果物 + 一時 worktree |
| 87 | GET | `/console` | 全案件の流れ（正規化したブロック、時刻順、カーソル付き。ADR-0048 D1、Phase 60a） | `ConsolePage` | events + messages + approvals + milestones + reports |
| 88 | GET | `/console/stream` | 同じブロックを SSE で流す（`event: console.block`） | SSE | 同上 |
| 89 | GET | `/tasks/{id}/runs/{run_id}/events` | その run のイベントだけ（折り畳んだ進行を開いたとき） | `EventsPage` | store `event_rows_for` |
| 90 | GET | `/knowledge/tree` | 知識ベースのツリー（`?scope=` / `?q=`。ADR-0047、Phase 61） | `KnowledgeTree` | `index.json` + `grep` |
| 91 | GET | `/knowledge/page` | ページ 1 枚（raw / html / front matter / 履歴 / etag） | `KnowledgePage` | ファイル + 履歴 |
| 92 | PUT | `/knowledge/page` | ページを 1 件 1 コミットで書く（**管理系**） | 200 `KnowledgePageResult` | ファイル + コミット |
| 93 | GET | `/knowledge/inbox` | `_inbox/` の候補（出典・取り込み先つき） | `KnowledgeInbox` | ファイル |
| 94 | POST | `/knowledge/inbox/{id}/accept` | 候補を正本に取り込む（**管理系**） | 200 `KnowledgePageResult` | ファイル + コミット |
| 95 | POST | `/knowledge/inbox/{id}/reject` | 候補を捨てる（**管理系**） | 200 `KnowledgeRejectResult` | ファイル + コミット |
| 96 | POST | `/console/instruct` | CoS への指示（Console から。ADR-0048 D3、Phase 60b）（**管理系**） | 202 `ConsoleInstructAccepted` | `crate::console` + `task_ops::conversation` |
| 97 | GET | `/llm/sources` | LLM source ごとの到達性・残量・cooldown・直近 1 時間の要求数（ADR-0053 D4。`[llm_proxy]` が無効なら 409） | `LlmSourcesView` | celeris の `LlmSourcesReader` |
| 98 | POST | `/console/new-conversation` | CoS の継続セッションを捨てる（ADR-0054 D1）（**管理系**） | 204 | store `node_sessions` |
| 99 | GET | `/mcp/clients` | MCP クライアントの一覧（トークンの値は出ない。ADR-0056 D4） | `McpClientsView` | store `mcp_clients` |
| 100 | GET | `/mcp/calls` | MCP の呼び出しログ（`?client=`） | `McpCallsView` | store `mcp_calls` |
| 101 | GET | `/skills` | skill の一覧（name / description / updated / mounted_by。ADR-0056 D3 続き、Phase 82） | `SkillList` | KB `skills/` + org |
| 102 | GET | `/skills/{name}` | `SKILL.md` 本文と付属ファイルの一覧 | `SkillDetailView` | KB `skills/<name>/` |
| 103 | PUT | `/skills/{name}` | skill を作る・更新する（**管理系**） | 200 `SkillPutResult` | ファイル + コミット |
| 104 | DELETE | `/skills/{name}` | skill を消す（mount されていれば 409 `skill_mounted`。**管理系**） | 204 | ファイル + コミット |
| 105 | POST | `/org/{id}/skills` | ノードに skill を mount する（**管理系**） | 200 `OrgNode` | store |
| 106 | DELETE | `/org/{id}/skills/{skill}` | ノードから skill を unmount する（**管理系**） | 200 `OrgNode` | store |
| 107 | GET | `/tasks/{id}/routing` | なぜその担当・harness・lane・model になったか（run ごとの監査と routing の出自。ADR-0069 D5） | `TaskRoutingView` | `task_ops::routing_audit` |
| 108 | POST | `/tasks/{id}/accept` | `draft` だけを `ready` にする（ADR-0070 D2 追記。§3.125.6）（**管理系**） | `TransitionResult` | `task_ops::gate::accept` |
| 109 | POST | `/tasks/{id}/retry` | `failed` / `cancelled` のタスクを複製してやり直す（§3.63・§3.125.6）（**管理系**） | 201 `RetryResult`（`Location`） | `task_ops::retry` |
| 110 | POST | `/tasks/{id}/rereview` | `done`（または最終レビューの不合格だけで `failed`）の task の最終レビューをやり直す（**管理系**） | `TransitionResult` | `task_ops::comment::rereview` |
| 111 | POST | `/tasks/{id}/pause` | task の subtree を一時停止する（ADR-0079 D13。§3.125.9）（**管理系**） | `TaskPauseResult` | `task_ops::lifecycle::pause_task` |
| 112 | POST | `/tasks/{id}/resume` | subtree の一時停止を解く（§3.125.9）（**管理系**） | `TaskPauseResult` | `task_ops::lifecycle::resume_task` |
| 113 | GET | `/accounts` | アカウントのプール（claude-code / codex。ADR-0024・ADR-0025。§3.29） | `AccountList` | `[accounts]` のディレクトリ + スナップショット |
| 114 | POST | `/accounts` | アカウントを足す（§3.30）（**管理系**） | 201 `AccountView`（`Location`） | celeris（ディレクトリ作成） |
| 115 | DELETE | `/accounts/{id}` | アカウントを消す（使用中は 409。§3.31）（**管理系**） | 200 `{}` | celeris（`AdminRequest`） |
| 116 | POST | `/accounts/{id}/check` | アカウントの疎通確認（§3.32）（**管理系**） | `AccountCheckResponse` | celeris（`AdminRequest`） |
| 117 | POST | `/accounts/{id}/login` | ログインの中継を始める（§3.33）（**管理系**） | `AccountLoginStart` | celeris（`AdminRequest`） |
| 118 | POST | `/accounts/{id}/login/code` | ログインのコードを渡す（§3.34）（**管理系**） | `AccountLoginResult` | celeris（`AdminRequest`） |
| 119 | DELETE | `/accounts/{id}/login` | 進行中のログインを取り消す（§3.35）（**管理系**） | 200 `{}` | celeris（`AdminRequest`） |
| 120 | PUT | `/clusters/{id}/settings` | クラスタの `work_dir` を設定する（**管理系**） | `ClusterSettingsView` | store |
| 121 | GET | `/org/{id}/memory` | そのノードの覚え書き（`?project=`。`[memory] dir` が無ければ 409。§3.62） | `MemoryView` | `[memory] dir` のファイル |
| 122 | GET | `/reports` | 報告の一覧（ADR-0033 D3。§3.50） | `ReportList` | store |
| 123 | GET | `/reports/{id}` | 報告 1 件（§3.51） | `ReportDetail` | store |
| 124 | POST | `/reports/read` | 報告を既読にする（§3.52）（**管理系**） | `ReportsReadResult` | store |
| 125 | POST | `/reports/notified` | 通知済みの時刻を進める（§3.53）（**管理系**） | `ReportsNotifiedResult` | store |
| 126 | GET | `/approvals` | 認可の一覧（`?pending=&project=&node=`。ADR-0033 D5。§3.56） | `ApprovalList` | store |
| 127 | POST | `/approvals/{id}/decide` | 認可を決める（§3.57）（**管理系**） | `ApprovalDecideResult` | store + task-ops |
| 128 | GET | `/standing-rules` | 永続の認可の一覧（`?node=`。§3.58） | `StandingRuleList` | store |
| 129 | POST | `/standing-rules` | 永続の認可を足す（§3.59）（**管理系**） | 201 `StandingRule` | store |
| 130 | DELETE | `/standing-rules/{id}` | 永続の認可を消す（§3.60）（**管理系**） | 204 | store |
| 131 | POST | `/projects/{id}/plan` | **ADR-0079 R5a で撤去（410 `removed_by_adr_0079`。§3.125.8）**。旧: 案件の分解・案件計画（§3.61） | 410 `Problem`（旧 202 `ProjectPlanAccepted`） | `crate::project_plan` |
| 132 | POST | `/projects/{id}/project-plan/{version}/decide` | **ADR-0079 R5a で撤去（410。§3.125.8）**。旧: 案件計画の版の承認・却下（ADR-0074 D3.3） | 410 `Problem`（旧 202 `ProjectPlanDecided`） | `crate::project_plan` |
| 133 | GET | `/projects/{id}/docs/maintenance` | repository docs の監査・提案・方針・保存済みの報告（ADR-0068。`docs/guides/repository-documentation-maintenance.md`） | `{audit, proposal, policy, saved_report}` | `task_ops::docs_maintenance` |
| 134 | POST | `/projects/{id}/docs/maintenance` | docs 整理の監査・方針の採用・計画の承認・適用（本文 `MaintenanceAction`: `audit` / `adopt` / `approve` / `apply`）（**管理系**） | 動作ごとの JSON（`{policy}` / `{approved, plan}` / `{sha, worktree, merged, task_id}` など） | `task_ops::docs_maintenance` |
| 135 | GET | `/tasks/{id}/execution` | 計画・WU・run・metrics・ExecutionPhase をまとめた実行の詳細（ADR-0072 D19。§3.125.1） | `TaskExecutionView` | `task_ops::view` + `task_ops::execution` |
| 136 | GET | `/tasks/{id}/execution-plan` | 有効な ExecutionPlan と WU・版の履歴（§3.125.2） | `ExecutionPlanView` | `task_ops::execution::active_plan` |
| 137 | POST | `/tasks/{id}/execution-plan` | 人の計画を採用する（新規だけ。§3.125.3）（**管理系**） | 201 `ExecutionPlanView` | `task_ops::execution::adopt_human_plan` |
| 138 | PUT | `/tasks/{id}/execution-plan` | 人の計画の採用、有効な計画があれば人の replan（§3.125.3）（**管理系**） | 201 / 200 `ExecutionPlanView` | `task_ops::execution::{adopt_human_plan, replan}` |
| 139 | POST | `/tasks/{id}/tree/adopt` | 採用済みの /3 の計画の unit に既存の task を結ぶ（ADR-0079 D15。§3.125.4）（**管理系**） | `AdoptionOutcome` | `task_ops::tree_adopt::adopt` |
| 140 | POST | `/tasks/{id}/execution/phase-gate` | 途中確認中の task を判定する（`continue` / `replan` / `withdraw`。§3.125.11）（**管理系**） | `TransitionResult` | `task_ops::phase_gate` |
| 141 | POST | `/tasks/{id}/execution/plan-gate` | root の計画の人の承認（ADR-0079 D8。§3.125.12）（**管理系**） | `TransitionResult` | `task_ops::plan_gate` |
| 142 | POST | `/tasks/{id}/execution/decompose` | 実行の形（`compound` / `atomic`）を人が決め直す（§3.125.7）（**管理系**） | `DecomposeResult` | `task_ops::regate::set_execution_mode` |
| 143 | GET | `/tasks/{id}/task-tree` | 再帰的な task の木と roll-up（`?root=`。ADR-0079 D11。§3.125.10） | `TaskTreeView` | `task_ops::tree_view::task_tree` |
| 144 | GET | `/metrics/execution` | 期間で集計した gate の判定分布・completion rate など（`?since=&group_by=`。§3.125.5） | `ExecutionMetricsSummary` | `task_api::stats` |
| 145 | GET | `/metrics/scratch` | scratch pool の観測値（ADR-0075 D6。`celerisctl scratch status --json` と同じ形。まだ無ければ 404 `scratch_unavailable`） | `ScratchStatus` | スナップショット |
| 146 | GET | `/decisions` | 決定の要求の一覧（`?open=&root_id=`。ADR-0079 D7。§3.125.13） | `DecisionList` | `task_ops::decision::list` |
| 147 | GET | `/tasks/{id}/decisions` | その task の subtree の決定（`?open=`。§3.125.13） | `DecisionList` | `task_ops::decision::for_subtree` |
| 148 | POST | `/decisions/{id}/answer` | 決定に答える（§3.125.13）（**管理系**） | `DecisionOutcome` | `task_ops::decision::answer` |
| 149 | POST | `/decisions/{id}/withdraw` | 決定を取り下げる（§3.125.13）（**管理系**） | `DecisionOutcome` | `task_ops::decision::withdraw` |
| 150 | POST | `/decisions/{id}/revise` | 回答済みの決定の答えを変える（§3.125.13）（**管理系**） | `DecisionOutcome` | `task_ops::decision::revise` |
| 151 | GET | `/tasks/{id}/browser/policy` | task の browser policy（ADR-0080） | `{policy}` | store |
| 152 | PUT | `/tasks/{id}/browser/policy` | task の browser policy を設定する（**管理系**） | `{updated: true}` | store |
| 153 | POST | `/tasks/{id}/browser/requests` | 登録待ち・承認待ちを開く（§3.118）（**管理系**） | 201 / 200 `BrowserRequestResult` | `crate::browser` |
| 154 | GET | `/tasks/{id}/browser/waits` | その task の wait（§3.119） | `BrowserWaitList` | store |
| 155 | GET | `/browser/waits` | 人の対応待ちの wait（§3.120） | `BrowserPendingList` | store |
| 156 | POST | `/tasks/{id}/browser/waits/{wait_id}/credential` | 手動登録の credential を broker に渡す（§3.121）（**管理系** + attestation） | `BrowserWaitResult` | `crate::browser` + credential broker |
| 157 | POST | `/tasks/{id}/browser/waits/{wait_id}/registered` | broker の receipt で登録済みにする（§3.122）（**管理系** + attestation） | `BrowserWaitResult` | `crate::browser` |
| 158 | POST | `/tasks/{id}/browser/waits/{wait_id}/decision` | 使用の承認・拒否（§3.123）（**管理系** + attestation） | `BrowserWaitResult` | `crate::browser` |
| 159 | POST | `/tasks/{id}/browser/waits/{wait_id}/revoke` | 未消費の承認を失効させる（§3.124）（**管理系** + attestation） | `BrowserWaitResult` | `crate::browser` |
| 160 | GET | `/browser/identities` | Browser Identity の一覧（`?project_id=` 必須。ADR-0101） | `{identities}` | `crate::browser_identity` |
| 161 | POST | `/browser/identities` | Browser Identity を登録する（**管理系**） | 201 `{identity}` | `crate::browser_identity` |
| 162 | DELETE | `/browser/identities/{id}` | Browser Identity を消す（**管理系**） | `{identity}` | `crate::browser_identity` |
| 163 | POST | `/browser/identities/{id}/revoke` | Browser Identity を失効させる（**管理系**） | `{identity}` | `crate::browser_identity` |
| 164 | POST | `/browser/identities/{id}/restore` | identity を開封して稼働中の隔離 session の controller に渡す（ADR-0101 D3 / ADR-0108 D5。attestation 必須）（**管理系**） | 204 | `crate::browser_identity` |
| 165 | POST | `/tasks/{id}/browser/live/{run}/{session}/grant` | Live View の閲覧許可を得る（GUI 署名の assertion） | `GrantResponse` | `crate::browser_live` |
| 166 | POST | `/tasks/{id}/browser/live/{run}/{session}/check` | 閲覧許可を確かめる | `CheckResponse` | `crate::browser_live` |
| 167 | POST | `/tasks/{id}/browser/live/{run}/{session}/read` | Live View の event を読む | `ReadResponse` | store `browser_live_after` |
| 168 | POST | `/tasks/{id}/browser/live/{run}/{session}/events` | Live View の event を追記する（daemon bearer） | `EventResponse` | store `browser_live_append` |

browser の制御（`crate::browser_control`）の 6 本は、route を定数 `BASE`（`/api/v1/tasks/{id}/browser/control/{run}/{session}`）と `format!` で組み立てて登録している（`browser_control.rs` の `routes()`）。詳細は `docs/guides/browser-capability.md`。

| メソッド | パス | 目的 | 応答型 | 出所 |
|---|---|---|---|---|
| GET | `/tasks/{id}/browser/control/{run}/{session}` | browser の制御状態（ADR-0099 D3。worker が参照） | `ControlStatus` | store `browser_control` |
| POST | `/tasks/{id}/browser/control/{run}/{session}` | pause・takeover・renew・resume・stop（署名付き assertion） | `ControlOutcome` | store `browser_control_mutate` |
| POST | `/tasks/{id}/browser/control/{run}/{session}/disconnect` | 人の操作の接続を切る | `ControlStatus` | store `browser_control_mutate` |
| POST | `/tasks/{id}/browser/control/{run}/{session}/agent/begin` | agent の操作の開始を記録する（daemon bearer） | `ControlStatus` | store `browser_control_mutate` |
| POST | `/tasks/{id}/browser/control/{run}/{session}/agent/end` | agent の操作の終わりを記録する（daemon bearer） | `ControlStatus` | store `browser_control_mutate` |
| POST | `/tasks/{id}/browser/control/{run}/{session}/auth-section` | 認証の区間を記録する | `ControlStatus` | store `browser_control_mutate` |

---

## 3. 各エンドポイント

記法: `→` は成功応答。エラーは §1.5 の共通分に加え、各項に書いたもの。

### 3.1 `GET /health` → 200 `Health`

```json
{"api_version":"1","schema_version":37,"celeris_version":"0.9.0","instance_id":"01J…",
 "started_at":"…","now":"…","db":{"journal_mode":"wal","busy_timeout_ms":5000,"filesystem":"ext4","device":"/dev/sda1"},
 "release":"a1b2c3d4e5f6","mode":"normal","role":"active"}
```

- `api_version` は `"1"` 固定。互換性を壊す変更は `/api/v2` で行う（ADR-0013 D8）。
- `schema_version` は `schema_migrations` の最大版数（= `task_core::SCHEMA_VERSION`）。
- `journal_mode` は `PRAGMA journal_mode` の実測値（`"wal"` でなければ設定不備。GUI は警告を出す）。
- `db.filesystem` / `db.device` は `/proc/self/mountinfo` から引いた DB のファイルシステム種別とマウントソース。
  判定できなければ省略される（ADR-0064 D1）。
- `release` はこのプロセスのリリース（`--release <sha12>` / 環境変数 `CELERIS_RELEASE` / 既定 `"dev"`）、
  `mode` は `"normal"` か `"verify"`（`--mode`）、`role` は `"active"` / `"standby"` / `"draining"` / `"verify"`。
  昇格（`promote.sh`）と検証（`verify.sh`）は「どの版がどの役割で動いているか」をここだけで判定する。
  `role` が `standby` / `draining` の間はディスパッチャの状態を要する管理系が 503 `standby`（§1.5。ADR-0040 D3/D4）。
- `mode == "verify"` のプロセスは、**`genre = "smoke"` かつアダプタが `fake` のタスクだけ**を dispatch する
  （他の ready は動かさない）。その役割・分野・プロバイダは verify モードが組み込みで足すもので、すべて偽のアダプタ
  （`adapter = "fake"`）。`GET /config` にだけ姿が出る（§3.21）。本番（`normal`）には何の影響も無い（ADR-0041 D5）。
- 無認証（1.3）。DB のパスは出さない（`GET /config` に出す）。

### 3.2 `GET /inbox` → 200 `Inbox`

計算規則は §5.1。クエリ無し（あれば 400）。応答は `task_ops::inbox::Inbox`:
`{approvals, questions, drafts, attention, browser_waits, decisions, counts}`（`counts` は各配列の件数と
DB 全体の status 別件数 `by_status`）。`attention[]` は `type` で区別する（`failed` / `requeue_limit_near` /
`unroutable` / `cluster_unavailable` / `phase_checkpoint` / `plan_approval` / `delivery_skipped`）。
`type: "unroutable"` はデーモンのスナップショット（3.20）から合成する。スナップショットがまだ無ければ出ない。

- `questions[].approval_id`: その質問に対応する**未決の** `approvals` の id（§3.56）。GUI はこれで
  `POST /approvals/{id}/decide`（3.57）へ直接リンクできる。行がまだ無い、または既に決定済みなら `null`。
- `browser_waits[]` は人の対応を待つ browser の wait（ADR-0080 D5）、`decisions[]` は未回答の決定の要求
  （回答は `POST /decisions/{id}/answer`。ADR-0079 D7）。

### 3.3 `GET /tasks` → 200 `TaskList`

| クエリ | 型 | 既定 | 意味 |
|---|---|---|---|
| `status` | `Status`、複数可（`?status=ready&status=running` または `status=ready,running`） | 全て | |
| `kind` | `TaskKind`、複数可 | 全て | |
| `genre` | 文字列（自由記述）、複数可（`kind` と同じ形） | 全て | `Task.genre` の完全一致（`ListFilter.genres`。ADR-0027 D1）。列挙型の検証はしない |
| `parent` | `TaskId` | — | 直接の子だけ（`ListFilter.parent_id`） |
| `project` | `ProjectId`（ULID） | — | その案件のタスクだけ（`ListFilter.project_id`。ADR-0033 D2）。ULID でなければ 400 |
| `root_only` | bool | false | `parent_id IS NULL` のものだけ。`parent` と AND で効く（同時指定は空になるだけで、エラーではない） |
| `q` | 文字列（最大 200 文字） | — | `title` / `objective` / **コメント本文**の部分一致（`ListFilter.text_contains` + `text_includes_comments`。SQLite の LIKE なので **ASCII の大文字小文字は区別しない**。`%` `_` はリテラル。ADR-0014 D2 / ADR-0044 D4）。200 文字超は 400、空文字は絞り込まない |
| `label` | 文字列（小文字 `[a-z0-9-]`）、複数可 | 全て | **AND**（指定したラベルを全部持つタスクだけ）。規則に合わない値は 400（ADR-0044 D4） |
| `category` | `feature`/`bug`/`research`/`ops`/`docs`/`other`、複数可 | 全て | OR。知らない値は 400（ADR-0044 D3/D4） |
| `assignee` | 文字列（`org_nodes.id`） | — | 完全一致（`ListFilter.assignee`）。空文字は絞り込まない |
| `milestone` | `MilestoneId`（ULID） | — | その途中目標のタスクだけ。ULID でなければ 400 |
| `tier` | `frontier`/`standard`/`cheap`、複数可 | 全て | `worker_hint.tier`（`json_extract`）。知らない値は 400 |
| `priority` | `P0`〜`P3`（大文字小文字は区別しない）または整数、複数可 | 全て | `Task.priority` の完全一致（ラベルは P0=30 / P1=20 / P2=10 / P3=0 に写す）。どちらでもなければ 400 |
| `order` | `dispatch` / `updated_desc` / `created_desc` | `updated_desc` | `ListOrder` と同じ: `dispatch` = `priority DESC, created_at ASC, id ASC`（`ready_tasks` と同じ）。`updated_desc` = `updated_at DESC, id DESC`。`created_desc` = `created_at DESC, id DESC`。他は 400 |
| `limit` | 1 以上の整数 | 100 | 0 は 400。500 を超える値は 500 に丸める |
| `archived` | bool（`1`/`0`/`true`/`false`） | `false` | `true` なら**アーカイブされた案件のタスクも返す**。既定はそれらを隠す（`ListFilter.hide_archived`。ADR-0044 D6）。案件に属さないタスクは常に見える。`GET /tasks/{id}`（個別）は既定でもそのまま見える |
| `cursor` | 不透明文字列 | — | 前応答の `next_cursor`（`Page<T>.next_cursor` をそのまま）。解読できない cursor は 400 |

- 知らないクエリキー、複数可でないキーの重複は 400。
- `task_ops::view::task_list`（`TaskStore::list_page(&ListFilter, ListOrder, cursor, limit) -> Page<Task>`）を使い、`Task` を `TaskSummary` に写す（`children` / `pending_children` / `backoff_until` の付加は task-ops）。
- `total` は `Page.total`（同じフィルタでの総件数。cursor に依らない）。
- `items[].children` / `pending_children` は `parent_id` で集計（`pending` = 非終端）。`backoff_until` は §5.3。
- `items[].role` は `Task.role`（ADR-0016 D1）。一覧の行に役割のラベルを出すための値で、`TaskDetail.role` と同じ（役割なしは `null`）。
- `items[].genre` は `Task.genre`（ADR-0027 D1）。`role` と同じ理由で一覧の行に出す値で、`TaskDetail.genre` と同じ（分野なしは `null`）。
- `items[].assignee` は `Task.assignee`（組織のノード id。ADR-0033 D2。担当なしは `null`）、
  `items[].conversation` は**対話用タスクか**（`true` なら人への返事のための run。§3.54〜3.55）。
  仕事の木やタスク一覧から対話用タスクを隠すのに使う（API 側の絞り込みは無い。GUI がこの真偽値で弾く）。
- `items[].support` は**裏方タスクの印**: `"milestone_review"` | `"conversation"` | `"plan"` | `"compaction"` |
  `"knowledge"` | `"doc_gardener"` | `"approval"` | `"review"` | `null`。決定的な優先順（`task_core::support_kind`）で 1 つだけ付く:
  途中目標レビュー（対話 + `milestone_id`）> 対話 > 計画（`kind == "plan"`）> 圧縮（`role == "report-compressor"`）>
  知識整理（`role == "knowledge"`）> `role == "doc-gardener"` > 承認（`kind == "approval"`）> 合成レビュー（`kind == "review"`）。
  人が見る本体の仕事は `null`。GUI はこれで仕事の木から裏方を一括で外せる（`conversation` は互換のため残す）。
- `items[].labels` / `items[].category` / `items[].priority_label` / `items[].project_id` /
  `items[].milestone_id` はボードのカードが要るもの（ADR-0044 D3/D4）。
  `priority_label` は `priority`（`i32`）を P0〜P3 に丸めた文字列（30 以上 = P0、20..30 = P1、
  10..20 = P2、10 未満 = P3）。`priority`（`i32`）は互換のため残す。
- `items[].is_root_task` は案件の root task か、`items[].paused` はそのタスク自身が subtree の一時停止中か
  （ADR-0079 D13）。`items[].actions` は今この状態で許される操作（§5.4）。
- 複数のフィルタを同時に書いたら **AND**（`label` どうしも AND、`category` / `tier` / `priority` /
  `status` / `kind` / `genre` は同じキーの中では OR）。
- `counts_by_status` は**フィルタに関係なく** DB 全体の status 別件数（0 件の status は現れない）。タイトルバーの件数表示用。
- 空のときは `{"items":[],"next_cursor":null,"total":0,"counts_by_status":{…}}`。

### 3.4 `POST /tasks` → 201 `Task`（`Location: /api/v1/tasks/{id}`。**管理系**）

`token_file` 未設定でも 401（`require_admin`。認可は本文の検証より先）。クエリは受けない。
要求本文は `task_ops::add::NewTaskSpec`（`celerisctl add` の引数と 1:1。`deny_unknown_fields`。§6.2）:

```json
{"title":"add CLI parsing","objective":"…",
 "acceptance":[{"type":"human","text":"reviewer is happy"},
               {"type":"command","cmd":"cargo test","expect_exit":0},
               {"type":"artifact_exists","name":"bench.json"},
               {"type":"knowledge_page","path":"design/cli.md"},
               {"type":"reviewer","text":"the diff is minimal"}],
 "kind":"execute","tier":"standard","adapter":null,"priority":0,
 "parent":null,"depends_on":["01J…"],
 "max_turns":10,"max_wall_secs":600,"max_retries":2,
 "workspace":null,"cluster":null,"workspace_mode":null,
 "role":"lead","genre":null,"aggregate":false,
 "project_id":null,"milestone_id":null,"assignee":null,"repos":[],
 "labels":["infra"],"skills":[],"mode":null,"category":"ops","status":"ready",
 "features":null,"execution":null,"pause_after":null,"stages_hint":[]}
```

- `acceptance[]` は `task_ops::add::CriterionSpec`（`Human{text}` / `Command{cmd, expect_exit}` / `ArtifactExists{name}` /
  `KnowledgePage{path}` / `Reviewer{text}`）を `#[serde(tag = "type", rename_all = "snake_case")]` で表したもの。
  `expect_exit` は省略可（既定 0）。
- 必須は `title` / `objective` / `acceptance`。省略可能なフィールドと既定は `celerisctl add` と同じ: `kind=execute`、
  `parent=null`、`depends_on=[]`、`max_retries=2`、`workspace=null`（→ `Local{path: "<task_id>"}`、相対）、
  `role=null`、`genre=null`、`aggregate=false`。
- `tier` / `max_turns` / `max_wall_secs` / `adapter` は任意。省略時は
  **タスクに書いた値 → `role` に一致する `[[roles]]` の既定 → `assignee` のノードの分野の `default_role` の既定 →
  `genre` の `default_role` の既定 → 全体の既定**（`tier=standard`、`max_turns=10`、`max_wall_secs=600`、`adapter=null`）
  の順で埋める（ADR-0016 D1 / M3、ADR-0027 D1、ADR-0033 D2）。`max_retries` に役割・分野の既定は無い（常に 2）。
- `role` は自由記述の役割名（ADR-0016 D1）。`[[roles]]` に無い名前でもエラーにせず、名前だけ保存する（既定も指示文も付かない）。
  状態機械は `role` を見ない。`GET /config` の `roles[]` が設定にある役割の一覧。
- `project_id` / `milestone_id` / `assignee` は任意（ADR-0033 D2）。`project_id` はその案件の
  仕事の木にタスクを載せる（`GET /projects/{id}` に出る）。`milestone_id` は `project_id` と同じ案件のもので
  あること。`assignee` は `GET /org` のノード id。違反（無い案件・`project_id` 無しの `milestone_id`・案件違いの途中目標・
  知らない `assignee`・そのタスクのハーネスを許さない `assignee`）は 422 `validation`。
- `genre` は分野の id（ADR-0027 D1）。**`[[genres]]` が 1 件でも設定されている celeris では常に検証する**
  （`celerisctl add --config` 無しのような「検証しない」緩さは API には無い）。
  `genre` を省略し `role` が指定されていれば、その役割を含む分野がちょうど 1 つだけあるとき、その分野を継ぐ
  （0 件・2 件以上は継がない）。それも無ければ `assignee` のノードの分野を継ぐ。`GET /config` の `genres[]` が設定にある分野の一覧
  （`{id, description, capabilities?, input_artifacts?, output_artifacts?, default_role, roles}`。
  `capabilities` / `input_artifacts` / `output_artifacts` は任意の自由記述で、空なら省略される。要素は `名前` でも
  `名前: 説明` でもよい（GUI は `:` の前を名前として扱う）。ADR-0028 D1）。
- `aggregate`（ADR-0016 D3）: true の親は、委譲した子が全て終端になった後に集約 run を 1 回だけ行い `artifacts/summary.md` を書く。
  応答の `Task` では **false のとき省略される**（`#[serde(skip_serializing_if)]`。`role` / `genre` / `project_id` /
  `milestone_id` / `assignee` も `null` のとき、`labels` / `skills` / `repos` は空のとき省略）。
- `acceptance` は**クライアントが並べた順**で保存する（並びに意味は無い）。
  `command` の `text` は `` `<cmd>` exits 0 ``（`expect_exit` に関わらずこの文）、`artifact_exists` の `text` は `artifact <name> exists`、
  `knowledge_page` の `text` は `knowledge base page <path> exists`。整形は task-ops が行う。
- `labels` / `category` / `priority` のラベル表記 / `status` は任意（ADR-0044 D1/D3）:
  - `labels`: 小文字の `[a-z0-9-]`、1〜64 文字、最大 8 個（重複は畳む）。違反は 422 `validation`。
  - `category`: `feature` / `bug` / `research` / `ops` / `docs` / `other`。**既定は `other`**
    （応答の `Task` では既定のとき省略される）。
  - `priority`: `"P1"` のようなラベルでも `20` のような整数でも書ける（P0=30 / P1=20 / P2=10 / P3=0）。
    **省略時は P2（= 10）**。`celerisctl add` は `--priority` の既定 0 を明示して渡すので従来どおり。
  - `status`: `"draft"` か `"ready"` だけ（他は 422）。
- `skills`（小文字 `[a-z0-9._-]`、最大 12 個。`assignee` を書かなければこれで担当が決まる。ADR-0046 D2/D5）、
  `mode`（`prototype` / `production` / `research`。既定 `production`。ADR-0046 D4）、
  `cluster` / `workspace_mode`（`WorkspaceSpec::Remote` と作業場所の扱い。ADR-0018 / ADR-0059 D1）、
  `features`（ADR-0069 D3）、`execution`（`atomic` / `compound`。人が書けば Complexity Gate の明示。ADR-0072 D13）、
  `pause_after`（ADR-0074 D2.1）、`stages_hint`（`[{title, scope}]`、最大 16 件。ADR-0079 D12）も任意。
- 初期 `status`（ADR-0044 D1）: `kind=approval` なら常に `ready`。それ以外は
  **`POST /tasks` では `ready`**（人は Go を出す側なので draft を挟まない）。`status: "draft"` を
  明示したときだけ Go 待ちの `draft` で始まる。`celerisctl add`・委譲・分解の子は `draft`
  （API のハンドラが `status` を省略時に `ready` で埋める。`task_ops::add` の既定は `draft`）。
  `Created` イベントと同一トランザクション（`task_ops::add::create_task_with_roles(store, spec, roles, genres, now) -> Task`）。
- 422 `validation`（`OpsError::Validation` の文言そのまま。`celerisctl add` も同じ関数を通る。主な検査をこの順で。ADR-0014 D3 / ADR-0027 D1）:
  - `stages_hint` の形（件数・空の `title`・長さ）
  - `genre` が `[[genres]]` に無い（`[[genres]]` が空でない設定に限る）→ `unknown genre: "<genre>"`
  - `genre` と `role` を両方指定し、`role` がその分野の `roles` に無い（同上）→ `role "<role>" is not one of genre "<genre>"'s roles`
  - `assignee` が組織のノードに無い → `assignee "<id>" is not an org node`
  - `project_id` が無い → `project <id> does not exist`、`project_id` 無しの `milestone_id` → `milestone_id requires project_id`、
    案件違い → `milestone <id> does not belong to project <id>`
  - `status` が `draft` / `ready` 以外、`labels` / `skills` の規則違反
  - `title` が空白だけ → `title must not be blank`（`field: "title"`）
  - `objective` が空白だけ → `objective must not be blank`（`field: "objective"`）
  - `acceptance` が空 → `at least one acceptance criterion is required (--accept, --check-cmd, --check-artifact, or --check-reviewer)`（`field: "acceptance"`）。
    `human` を含むのに `artifact_exists` も `knowledge_page` も無い → 422（ADR-0067 D2）
  - `parent` が存在しない → `parent <id> does not exist`（`field: "parent"`）
  - `depends_on[i]` が存在しない → `dependency <id> does not exist`、`failed` / `cancelled` → `dependency <id> has status Failed and cannot be depended on`（`Failed` / `Cancelled` は `{:?}` 表記。`field: "depends_on"`）
  - `repos` の違反（下記）
- 検証に失敗したら何も挿入しない。
- **`repos`** は任意（ADR-0043 D2）。そのタスクが使う案件のリポジトリを**名前で**並べる
  （`"repos": ["benchfs", "benchfs-paper"]`。名前は `GET /projects/{id}/repos` の `name`）。
  省略すると **親のタスクの `repos` → 案件の primary** を継ぐ。`repos[0]` がワーカーの
  カレントディレクトリになる。422 `validation` になるのは次のとき:
  - `project_id` を書かずに `repos` を書いた → `repos can only be used on a task that belongs to a project`
  - その案件に無い名前 → `task repo "<name>" is not one of this project's repositories`
  - リモートのリポジトリを他と混ぜた → `a task cannot mix a remote repository with other repositories yet (ADR-0043 D2)`
  - 手元の `workspace` とリモートのリポジトリを組み合わせた → `a local workspace cannot be combined with the remote repo "<name>"; …`
  応答の `Task.repos[]` は `{"repo_id": "01J…", "name": "benchfs"}` の配列（空なら省略される）。

### 3.5 `GET /tasks/{id}` → 200 `TaskDetail`

- `celerisctl show --json <id>` と**同じ型・同じ直列化**（task-ops の `TaskDetail` を compact な JSON で出す。ADR-0013 D12）。差は次の 3 点:
  API は `runs[].files` を埋める（celerisctl は `null`）、`timers.now` は応答時刻、celerisctl は `--config <config.toml>`
  （または `CELERIS_CONFIG`）を渡さない限り `workspace_dir` / `timers.backoff_until` / `timers.max_requeues` / `worktree` を
  設定の既定値で計算する（`--workspace-root` で基準だけ上書き可）。`--config` を渡せば API と同じ値になる。
- `workspace_dir` は `WorkspaceSpec::Local{path}` を `workspace_root` で絶対化した文字列（`canonicalize` はしない。存在しなくてもよい）。
  `Remote{cluster, path}` では**手元の写し** `workspace_root/<task_id>`（run のログ `runs/` と成果物はここ。クラスタ側のパスは `task.workspace.path`。ADR-0018 D1）。
- `cluster` は `WorkspaceSpec::Remote` の `cluster`（`[[clusters]] id`）。`Local` は `null`。
- `worktree` は `sync = "worktree"` のクラスタで動くタスクだけに出る（ADR-0019 D2）。
  `{project, dir, branch}` = 元のリポジトリ / クラスタ上の worktree のパス（既定 `<project>/.celeris-worktrees/<task_id>`、
  `worktree_root` があればその下）/ ブランチ `celeris/<task_id>`。**celeris は commit しない**ので、変更は worktree の作業ツリーに残る。
  GUI はここを「クラスタで結果を見る場所」として出す（`git -C <dir> diff`、`git -C <dir> commit`、`git worktree remove <dir>` は人の操作）。
- ローカルの作業場所（`kind = "local"`、`mode = "worktree"` 既定）が git リポジトリの
  タスクにも `worktree` が出る（`{project, dir, branch}` = 元のリポジトリ / `<workspace_root>/<task_id>/tree` /
  `celeris/<task_id>`）。このとき `workspace_dir` は `<workspace_root>/<task_id>`（run のログ `runs/` と成果物は
  作業ツリーの**外**にある）。celeris が用意した目印 `<workspace_dir>/worktree.json` があるタスクだけがこの扱いで、
  worktree を消した後も `runs/` と `artifacts/` は同じ場所から引ける（ADR-0041 D1）。
  `sync = "rsync"` / `"none"` のクラスタと worktree を切らない Local のタスクでは `null`。
  `celerisctl show --json` は `--config <config.toml>`（または `CELERIS_CONFIG`）を渡したときだけ `worktree` を出せる
  （`[[clusters]]` を知らないと worktree のパスが決まらないため）。
- `role` は `task.role` と同じ値を最上位にも出したもの（GUI の表示用。ADR-0016 D1）。役割が無ければ `null`。
- `genre` は `task.genre` と同じ値を最上位にも出したもの（`role` と同じ理由。ADR-0027 D1）。分野が無ければ `null`。
- `priority_label` は `task.priority` を P0〜P3 に丸めたもの（§3.3 と同じ規則。ADR-0044 D3）。
- `delegated[]` は、このタスクの run が `delegate` で作った子の履歴（`Event::Delegated` の出現順。ADR-0016 D2）。
  1 要素は `{run_id, ts, tasks: TaskRef[]}` で、`ts` はイベントの `ts`、`tasks` は子の**現在の**状態（既に存在しない ID は落とす）。
  委譲された子は `children[]` にも出る（`delegated[]` はどの run が作ったかを足すだけ）。
- `failure` は `status == failed` のときだけ値を持つ（分類・理由。ADR-0070 D1）。`execution` は gate・計画・WorkUnit の活動が
  あるときだけ出る（無ければ省略。ADR-0072 D20）。`is_root_task` / `paused_by`（一時停止で dispatch を止めている task。
  無ければ省略）は ADR-0079 D13、`cluster_job_wait`（待っているクラスタ job。無ければ省略）は ADR-0090 D5。
- `timers.now` は応答時刻。クライアントは `lease_expires_at - now` 等をこの `now` 基準で計算する（時計ずれ対策）。
- `runs[].files` は task-api が `<workspace_dir>/runs/<run_id>/` を `stat` して埋める（task-ops は `null`）。
- `actions` は今この状態で許される操作（§5.4）。GUI はボタンの表示にこれを使い、押した結果の 409 も正常系として扱う。
- 404 `task_not_found`。`{id}` が ULID でなければ 400。

### 3.6 `GET /tasks/{id}/events` → 200 `EventsPage`

| クエリ | 既定 | 意味 |
|---|---|---|
| `after_seq` | −1 | この `seq` より大きいものから（−1 未満は 400） |
| `limit` | 500（最大 5000。超える値は 5000 に丸める。0 は 400） | |
| `types` | 全て | `Event` の `type` 名をカンマ区切り（例 `transitioned,worker_finished`）。未知の名前は 400 |

`worker_progress` は `{run_id, msg}` に加えて
`kind`（`tool_use` / `tool_result` / `text` / `thinking` / `status`）/ `tool` / `summary` / `detail`（4 KiB まで）/
`truncated` / `error` を**あれば**持つ（**追加のみ**。付けないワーカー・導入前のイベントには無い。ADR-0048 D2）。
Console（§3.98）はこの形だけを見る。

`types` の語彙は `task_api::query::EVENT_TYPES`（50 種）: `created`、`transitioned`、`worker_started`、`worker_progress`、
`artifact_produced`、`worker_finished`、`review_verdict`、`approval_requested`、`approval_decided`、`approvals_withdrawn`、
`answered`、`provider_throttled`、`cluster_unavailable`、`delegated`、`question_raised`、`retried`、`edited`、`assigned`、
`browser_updated`、`browser_wait_opened`、`browser_wait_resolved`、`cluster_job_wait_started`、`cluster_job_wait_polled`、
`cluster_job_wait_finished`、`workspace_mode_downgraded`、`cluster_master_exited`、`workspace_pruned`、`routing_decided`、
`checkpoint_saved`、`execution_planned`、`work_unit_transitioned`、`work_unit_spec_overridden`、`work_unit_checks_failed`、
`execution_gated`、`execution_hint_set`、`repair_scheduled`、`quota_estimated`、`pause_points_resolved`、`phase_reported`、
`project_plan_proposed`、`project_plan_decided`、`child_task_created`、`child_adopted`、`unit_gate_overridden`、
`decision_requested`、`decision_answered`、`decision_withdrawn`、`plan_approval_requested`、`stall_detected`、`delivery_skipped`。
主なものの形:

- `approvals_withdrawn`（`{approval_ids, task_status, reason}`）: タスクが終端になり、未決の認可の要求
  （§3.56）を celeris が `withdrawn` で閉じた。`reason` は `task_terminal` か `reconcile`。状態は変えない。
- `cluster_unavailable`（`{cluster, host, reason}`。ADR-0018）。
- `delegated`（`{run_id, task_ids}`。ADR-0016 D2）。状態は変えないので `replay` は無視する。
- `question_raised`（`{run_id, text}`。ADR-0021）: ディスパッチャが人に出した質問。同じトランザクションの
  `transitioned{to: "blocked", reason: "child_failed"}` と対。状態は変えないので `replay` は無視する。
- `retried`（`{from}`）: `failed`/`cancelled` を複製してやり直した新しいタスクに付く。状態は変えないので `replay` は無視する（§3.63）。
- `edited`（`{fields, by}`。ADR-0044 D1）: 人が `PATCH /tasks/{id}` で変えた項目名の一覧。
- `assigned`（`{node, score, reason}`。ADR-0046 D5）: `assignee` が無いタスクの担当を matching が決定的に決めたときに
  1 件だけ付く。`node` は決まった担当の id、`score` はタスクの `skills` とその担当の実効 `skills` の重なりの件数、
  `reason` は人が読める理由の文。GUI のタスク画面の「なぜこの担当か」はこれを表示する。

`work_unit_committed` / `phase_integrated` / `work_units_serialized` は応答の `items[]` には現れるが、`types` の語彙には無い（指定すると 400）。

- `seq` 昇順。`has_more` が true なら最後の `seq` を `after_seq` に入れて続きを取る。
- `items[].id` はグローバル id（ADR-0013 D6）。`items[].ts` は `events.ts`。
- 404 `task_not_found`。

### 3.7 `GET /tasks/{id}/runs` → 200 `RunList`

§5.2 の規則で events から組み立て、`files`（`{stdout, stderr, result, request, prompt}` の有無）を埋める。
応答は `{"runs": [...]}`。events 上の出現順（`started_at` 昇順）。クエリは受けない。404 `task_not_found`。

### 3.8 ファイル系: `GET /tasks/{id}/runs/{run_id}/{stdout|stderr|result|request|prompt}`、`GET /tasks/{id}/artifacts/{idx}`

**パス解決（ユーザ入力のパスは受け取らない。ADR-0013 D11）**

1. run: `run_id` が `^[0-9A-HJKMNP-TV-Z]{26}$` に一致しなければ（ファイルシステムに触る前に）403 `path_forbidden`。
   成果物: `idx` が非負整数でなければ 400。タスクが無ければ 404 `task_not_found`。
2. `<ws>` = タスクの手元の作業場所（`task_ops::workspace::local_dir`）を `canonicalize`。`Local{path}` は `workspace_root` 基準
   （worktree を切ったタスクは `<workspace_root>/<task_id>`。ADR-0041 D1）、`Remote` は手元の写し `<workspace_root>/<task_id>`
   （ADR-0018 D1）。存在しなければ 404 `file_not_found`。
3. run: 対象 = `<ws>/runs/<run_id>/{stdout.jsonl|stderr.log|result.json|request.json|prompt.txt}`。
   `request.json` は**ワーカーに渡した `RunRequest`**（objective・役割の指示文・クラスタ用の追記・前回の判定・人の回答・子の結果。ADR-0023 D2）。
   `prompt.txt` は claude-code / codex に**実際に渡した文面**（`build_prompt` の結果。ADR-0023 M1。fake アダプタの run には無い）。
   どちらにも秘密は含まれない（`[[providers]].env` の値やトークンは `RunRequest` に入らない）。run のディレクトリごと無ければ 404 `run_not_found`。
4. 成果物: `idx` は `GET /tasks/{id}/artifacts` の `items[].idx`（`ArtifactProduced` の出現順、0 始まり）。範囲外 → 404 `artifact_not_found`。
   記録された `ArtifactRef.path` が空・絶対パス・`..` を含む → 403 `path_forbidden`。対象 = `<ws>` + `ArtifactRef.path`。
5. 対象を `canonicalize` し、`<ws>` の canonical パスで始まらなければ 403 `path_forbidden`（symlink でワークスペース外へ出るものを弾く）。ファイルでなければ（ディレクトリ等）403。存在しなければ 404 `file_not_found`。
6. ワークスペースの**外は絶対に出さない**が、中は信頼境界の内側とする。

**応答**

- `Content-Type` は拡張子から次の**閉じた表**で決める。それ以外は `application/octet-stream`。`text/html`、`image/svg+xml`、`application/javascript` 等の能動的な型は**決して返さない**。
  - `text/plain; charset=utf-8`: `.txt .log .jsonl .diff .patch .csv .tsv .toml .yaml .yml .rs .py .sh .ts .js .c .h .cpp .go .java .sql`（ソースは全て text/plain）
  - `application/json`: `.json`（`result` は常にこれ）
  - `text/markdown; charset=utf-8`: `.md`
  - `image/png` / `image/jpeg` / `image/gif` / `image/webp`: 対応する拡張子
- `Content-Disposition: inline; filename="<basename>"; filename*=UTF-8''<RFC 8187 でエスケープ>`（`?download=1`（または `true`）で `attachment`）。
- 成果物には `X-Celeris-Sha256: <記録値>` と `X-Celeris-Sha256-Current: <現在の値>`（計算は 64 MiB までで、超えるファイルは省略）。
- `X-Celeris-Size: <現在のバイト数>` と `Accept-Ranges: bytes` を常に付ける（追尾用）。
- 範囲: `Range: bytes=a-b`（`a-` / `-n` も可）に 206 + `Content-Range` で応える（単一範囲のみ。複数範囲・不正な形・開始がサイズ以上は 416。`bytes` 以外の単位は無視して全体を返す）。
  または `?offset=N&length=M`（`Range` と併用不可、併用は 400）。`offset == size` は **200 で空本体**（追尾で「新着なし」を表すため）。`offset > size` → 416。
- 受けるクエリは `offset` / `length` / `download` だけ（他は 400）。
- 本体はストリーミング。サイズ上限は設けない（追尾は `offset` で行う）。

### 3.9 `GET /tasks/{id}/artifacts` → 200 `ArtifactList`

`{"items": [...]}`。`ArtifactProduced` を出現順に並べ、`idx`、`run_id`、`ts`、`artifact`（`ArtifactRef`）、`exists`、`forbidden`、`size`、`sha256_current`、`sha256_matches`（記録値との一致。`exists=false` なら `null`）。パス検査（3.8）で `path_forbidden` に落ちるものは `exists=false, forbidden=true` として一覧には残す（GUI は警告表示、本体は 403）。ワークスペースやファイルが無いものは `exists=false, forbidden=false`。クエリは受けない。404 `task_not_found`。

### 3.10 `POST /tasks/{id}/approve` → 200 `TransitionResult`（**管理系**）

`token_file` 未設定でも 401（`require_admin`。§3.11〜3.13 も同じ。認可は本文の検証より先）。
本文 `DecisionBody{note?: string, expected_status?: Status}`（空本体は `{}` と同じ。ただし `Content-Type` は必要）。呼ぶのは `task_ops::gate::approve(store, id, note, expected_status)`。
応答 `TransitionResult{id, from, to, reason, cascaded: TaskRef[]}`。

- 写像は `celerisctl approve` と同じ: `status == draft`（kind 不問）→ `Trigger::Accept`（イベント追加無し）。`kind == approval && status == ready` → `Trigger::Approve` + `Event::ApprovalDecided{by:"human", approved:true, note}` を同一トランザクション。
- それ以外 → 409 `invalid_transition`（`OpsError::InvalidState`: `detail: "task <id> (kind=<kind>, status=<status>) cannot be approved"`（`kind` / `status` は `{:?}` 表記）、`trigger: "approve"`、`task_status`、`kind`）。
- `expected_status` があり現在と違う → 409 `conflict`（`OpsError::Conflict`。`expected` / `actual` を添える。写像より先に検査される）。
- `by` は `"human"` 固定（H9）。
- 404 `task_not_found`。

### 3.11 `POST /tasks/{id}/reject` → 200 `TransitionResult`（**管理系**）

本文 `DecisionBody`。`task_ops::gate::reject(store, id, note, expected_status)`: `kind == approval && status == ready` のみ `Trigger::Reject` + `ApprovalDecided{by:"human", approved:false, note}`。それ以外は 409 `invalid_transition`（`cannot be rejected`、`trigger: "reject"`）。`draft` の取り消しは `cancel`（ADR-0004 D2）。

### 3.12 `POST /tasks/{id}/answer` → 200 `TransitionResult`（**管理系**）

本文 `AnswerBody{answer: string, expected_status?}`（`answer` は必須。空本体は 400）。`answer` が空白のみ → 422 `validation`（`answer must not be blank`、`field: "answer"`。task-api が task-ops を呼ぶ前に検査する。CLI は clap が空文字を通すので挙動は CLI と同じにしない）。
`task_ops::gate::answer(store, id, answer, expected_status)`: `status != blocked` → 409 `invalid_transition`（`cannot be answered; only blocked tasks accept an answer`、`trigger: "answer"`）。
`blocked` でも、工程の後の途中確認（`awaiting_human`）と root の計画の承認待ちは質問ではないので 409 `invalid_transition`
（それぞれ `POST /tasks/{id}/execution/phase-gate`・`POST /tasks/{id}/execution/plan-gate` を使う。ADR-0074 D2.2 / ADR-0079 D8）。
`Trigger::Answer` + `Event::Answered{question, answer}`（`question` は §5.5 の `latest_question`。無ければ空文字列）を同一トランザクション。

- 遷移の後、このタスクの未決の `approvals`（§3.56）があれば `once` + 同じ `answer` の文言で決定済みにする
  （`approvals` が無ければ何もしない。決定的）。`POST /approvals/{id}/decide`（3.57）は既にこの経路（`gate::answer`）に
  相乗りしているので、どちらから答えても `GET /approvals?pending=true` から同じように消える。

### 3.13 `POST /tasks/{id}/cancel` → 200 `TransitionResult`（**管理系**）

本文 `CancelBody{expected_status?}`（空本体は `{}` と同じ）。`task_ops::gate::cancel(store, id, expected_status)`: 終端（`done|failed|cancelled`）→ 409 `invalid_transition`（`cannot be cancelled`、`trigger: "cancel"`）。伝播（Approval の子、`depends_on` の後続）はストアが同一トランザクションで行い、`cascaded` にその id を列挙する（§5.7）。
取り消しの後、そのタスクの browser の session をすべて止める（ADR-0099 D3）。

### 3.14 `POST /plans` → 410 `removed_by_adr_0079`（**管理系**）

廃止（ADR-0079 U-R6）。認可（`require_admin`）を通ると本文を見ずに 410 を返す（problem の `adr: "ADR-0079"`、
`instead: "POST /api/v1/tasks (a root task; name the stages in stages_hint)"`）。分解は root task の Complexity Gate と
planner が行う（`POST /tasks` で root task を作る。§3.4）。既存の `kind = plan` の行と子はそのまま読める。

### 3.15 `POST /replay` → 200 `ReplayReport`（**管理系**）

`token_file` 未設定でも 401（`require_admin`）。本文は空（`{}`。空本体も可。未知のフィールドは 400）。`task_ops::replay::replay(store) -> ReplayReport{tasks, mismatches}`（`mismatches[]` = `{task_id, field: "status"|"attempts", replayed, stored}`）。`celerisctl replay` と同じ規則（`Created` で初期化、`Transitioned` で上書き、`worker_error|lease_expired|review_fail` と `ready` へ戻る `child_failed` で attempts+1、`reopen` で attempts を 0 に戻す）で全タスクを再構築し、`tasks` との差分を返す。**DB は変更しない。** 数万イベントで数秒かかりうるので、`spawn_blocking` で行い、同時実行は 1 つ（2 つ目は 503 `replay_in_progress`、`Retry-After: 5`）。

### 3.16 `GET /graph` → 200 `Graph`

| クエリ | 既定 | 意味 |
|---|---|---|
| `root` | — | このタスクの祖先・子孫（`depends_on` と `parent_id` を両方向にたどる）だけ。無いタスクは 404 `task_not_found` |
| `depth` | 無制限 | `root` からの最大ホップ数 |
| `include_terminal` | true | false で `done\|failed\|cancelled` を除く（辺も除く） |

`nodes[] = {id, title, status, kind, parent_id, role}`（`role` は `Task.role`。ノードに役割のラベルを出すための値。役割なしは `null`）、
`edges[] = {from, to, kind: "depends_on"}`（`from` = 先行、`to` = 後続）。親子は `parent_id` で表し、辺にしない。レイアウトはクライアント。上限 5,000 ノード（超えたら 422 `validation`、`detail` で `root` の指定を促す）。

### 3.17 `GET /events` → 200 `EventsPage`

| クエリ | 既定 | 意味 |
|---|---|---|
| `after_id` | 0 | このグローバル id より大きいものから |
| `limit` | 500（最大 5000。超える値は 5000 に丸める。0 は 400） | |
| `task_id` | — | 1 タスクに絞る（ULID でなければ 400） |
| `types` | 全て | 3.6 と同じ |

`id` 昇順。`task_id` も `types` も無ければ `events_since(after_id, limit)` そのもの。SSE を使えないクライアント（`curl`、テスト）用。

### 3.18 `GET /stream`

§4。

### 3.19 `GET /providers` → 200 `Providers`

`items[]` は `[[providers]]` の順。定義（`id` / `adapter` / `tiers` / `concurrency` / `model` = 実効モデル / `env_keys` = **キー名だけ** /
`account_pool` / `account_id` / `tier_models` / `credential_refs`）は設定から、`in_use` と `in_use_cos` と `cooldown` と
`last_check` はスナップショット（無ければ `null`）、`stats` は §5.8 の集計。クエリは受けない。

`in_use_cos`: そのプロバイダで走っている **CoS の対話 run**（Console から CoS に話しかけた一言）の数。
CoS の対話 run は `max_concurrency` とアカウントプールのプロバイダの `concurrency` の外で走るので、`in_use` には含めず別に出す
（`in_use` + `in_use_cos` が `concurrency` を超えることがある。ADR-0089）。

`last_check`（ADR-0022 D2）は直近の `POST /providers/{id}/check` の結果 `{at, result, detail?}`
（`result` は §3.27 と同じ 4 値、`detail` は人が読む一行で、無ければ省略。ADR-0022 M1）。
**メモリだけに持つ観測値**で、celeris を再起動すると `null` に戻る（イベントにも DB にも残さない）。自動では走らないので、
値が入るのは人が `check` を叩いた後だけ。`reload` でプロバイダ表を差し替えても、同じ `id` の記録は残る。

ADR-0017 M4: `POST /reload` に成功すると、次の tick のスナップショットに乗った一覧（`providers.d/` を含む）を優先して返す。最初の tick が来る前だけ起動時に固定した一覧にフォールバックする。`GET /config` の `providers[]` も同じ規則。

### 3.20 `GET /daemon` → 200 `DaemonView`

```json
{"now":"…","snapshot":{"instance_id":"01J…","pid":1234,"hostname":"lab-01","started_at":"…","last_tick_at":"…","ticks":8812,"tick_ms":2000,
  "in_flight":[{"task_id":"01J…","run_id":"01J…","provider":"claude-a","kind":"worker","since":"…"}],
  "cooldowns":[{"provider":"claude-b","until":"…","reason":"throttled"}],
  "awaiting_human":["01J…"],"awaiting_children":["01J…"],"unroutable":[],
  "reports":null,"approvals_pending":0,"decisions_open":0,
  "providers":[{"id":"claude-a","adapter":"claude-code","tiers":["frontier","standard","cheap"],"concurrency":2,"model":"claude-sonnet-5","env_keys":[],"in_use":1,"in_use_cos":0,"account_pool":false}],
  "clusters":[],"accounts_root":null,"max_runs_per_account":null,"accounts_roots":{},"accounts":[]}}
```

- ディスパッチャが tick の最後に `DaemonSnapshot` を `tokio::sync::watch` に送り、API は最新値を読む（ADR-0013 D4）。DB には書かない。クエリは受けない。
- 最初の tick より前は `snapshot: null`。
- `reports`（秘書レベルの未読の報告と通知の判定）・`approvals_pending`（未決定の認可の件数）・`decisions_open`（未回答の決定の要求の件数）は
  ディスパッチャではなく **API が応答を組むときに埋める**（ADR-0033 D3/D5、ADR-0079 D7）。
- `clusters[]`（ADR-0018）、`accounts_root` / `max_runs_per_account` / `accounts_roots` / `accounts[]`（ADR-0024 / ADR-0025）、
  `scratch`（scratch pool の観測値。無ければ省略。`GET /metrics/scratch` と同じ。ADR-0075 D6）も載る。
- `awaiting_human[]` は承認待ちで延期中のタスク、`awaiting_children[]` は**委譲した子が終わるのを待っている親**
  （どちらも `reviewing` のままだが理由が違う。ADR-0023 D3）。GUI はこれを見て「判定中」と「部下待ち」を区別する
  （`reviewing` かつ子が非終端、という再計算を GUI 側でしない）。
- cooldown は `ProviderPolicy::cooldowns(now) -> Vec<Cooldown{provider, until: Instant, reason: CooldownReason}>` で取り、`Instant` を壁時計に直す。`reason` の語彙は `throttled | auth_failed | exhausted`（`Spawn` は `provider_failure_outcome` が `Exhausted` に写すので cooldown の理由としては現れない。`ProviderThrottled.reason` には `spawn` も入りうる）。
- `last_tick_at` が `now` から `3 × tick_ms` 以上古ければ GUI は「ディスパッチャが遅延」と表示する（API は判定しない）。
- API に繋がらないこと自体が「celeris 停止」を意味する（GUI 側で表示）。
- **`containers`**（ADR-0043 D3。古いスナップショットには無いので任意）: コンテナ実行の設定と、
  **起動時に 1 度だけ**調べた runtime の能力。

```json
"containers":{"preference":"auto","runtime":"docker","image_default":"celeris-worker:latest",
  "build_dir":"/home/u/.local/celeris/containers",
  "probes":[{"runtime":"podman","detail":"newuidmap: write to uid_map failed: Operation not permitted"},
            {"runtime":"docker","detail":"ok"}]}
```

  - `preference` は `[containers] runtime`（`"auto"` / `"podman"` / `"docker"`）。`"auto"` は podman を先に試す。
  - `runtime` は実際に使うもの。**無ければ（省略）`run = container` のタスクは dispatch されず `blocked`** になる
    （質問「コンテナ runtime が使えません…」）。GUI は設定画面で赤く出す。
  - `probes[]` は試した順の `<runtime> info` の結果（`detail` は成功なら `"ok"`、失敗なら理由の 1 行）。
  - これは**観測値**で、DB には書かないし `replay` の対象でもない（再起動すると調べ直す）。

### 3.21 `GET /config` → 200 `ConfigView`

`config.toml` の要約。`config_path`、`db`（絶対パス）、`workspace_root`、`tick_ms`、`max_concurrency`、`lease_grace_secs`、`idle_timeout_secs`、`kill_grace_secs`、`review_timeout_secs`、`error_cooldown_secs`、`retry_backoff_base_secs`、`retry_backoff_max_secs`、`max_requeues`、`plan_auto_accept`（`[plan] auto_accept`）、`reviewer{adapter, tier?}`（`tier` は `[reviewer] tier` を明示したときだけ。無ければ worker run の lane に合わせて動的に決まる）、
`providers[]{id, adapter, tiers, concurrency, model, env_keys, account_pool, account_id, tier_models, credential_refs}`、
`clusters[]{id, host, work_dir?, concurrency, sync, delete_on_push, has_setup, env_keys, rsync_excludes, auth, forwards}`（ADR-0018。`work_dir` は設定ファイルの値だけ）、
`roles[]{id, tier, adapter, max_turns, max_wall_secs, has_instructions}`（`[[roles]]` の順。ADR-0016 D1）、
`genres[]{id, description, capabilities?, input_artifacts?, output_artifacts?, default_role, roles}`（ADR-0027 D1 / ADR-0028 D1。
`capabilities` / `input_artifacts` / `output_artifacts` は自由記述の任意フィールドで、空なら省略される。
`[[genres]]` の順。`[[genres]]` を書かない設定では `[]`）、
`delegation{max_delegate_per_run, max_tree_depth, max_tree_runs, on_child_failure}`（ADR-0021。既定 8 / 5 / 100 /
`"retry_then_ask"`。`on_child_failure` は `"retry_then_ask"` か `"ignore"`）、`api{bind, auth_required, allowed_hosts}`。クエリは受けない。
`[[providers]].env` の**値**、`[adapters.*].env` の値、`[[clusters]].env` の値と `setup` の中身、`[[roles]].instructions` の**本文**（有無だけを `has_instructions` で出す）、`token_file` のパスと内容は出さない。

- `GET /health` の `mode` が `"verify"` のプロセスでは、`roles[]` / `genres[]` /
  `providers[]` の末尾に組み込みの **`smoke`**（`adapter = "fake"`、`tier`/`tiers` は `standard`）が 1 つずつ
  増え、`reviewer` が `{adapter: "fake", tier: "standard"}` になる。設定ファイルに同じ id があっても
  上書きされる（煙試験が本物の LLM を呼ぶ経路を設定で開けられないようにするため。ADR-0041 D5）。**型は変わらない**し、
  本番（`mode = "normal"`）の応答も変わらない。`task-api` は `celeris` crate に依存しないので、この型は task-api に置き、celeris が起動時に値を作って `ApiState` に渡す。

### 3.22 `GET /schema` → 200 `application/schema+json`

コミット済み `docs/api/v1/api-v1.schema.json` を `include_str!` で返す（開発・型生成の確認用。GUI の型生成はリポジトリのファイルから行い、この応答には依存しない）。クエリは受けない。

### 3.23 `GET /clusters` → 200 `Clusters`（ADR-0018。トンネルは ADR-0053 D3）

`items[]` は `[[clusters]]` の順。定義（`id` / `host` / `concurrency` / `sync` / `delete_on_push` / `has_setup` = `setup` の有無 / `env_keys` = **キー名だけ** / `rsync_excludes` / `auth`）は設定から、
`in_use`（そのクラスタで走っている run + 判定の数）/ `connected`（この tick の `ssh -O check` の結果 = 多重接続があるか）/ `cooldown_until` / `cooldown_remaining_secs` / `connect_pending` /
`tunnel_forwards` / `tunnel_login_needed` は スナップショットから（無ければ `in_use`・`connected` は `null`、`connect_pending`・`tunnel_login_needed` は `false`）。`env` の値と `setup` の中身は出さない（ADR-0018 D7）。
`cooldown_until` が過ぎていれば `cooldown_until`・`cooldown_remaining_secs` とも `null`。無認証の読み取り。

- `auth`: `"manual"`（既定）/ `"publickey"` / `"totp"`（ADR-0032 D1）。GUI はこれで「クラスタ」画面の案内を出し分ける
  （3.39〜3.41、§9）。
- `connect_pending`: GUI 発の接続（`POST /clusters/{id}/connect`）が celeris 側で進行中か（ADR-0032 D5）。
  **プロンプト文字列はここには出さない**（`POST /clusters/{id}/connect` の応答にだけ載る。ADR-0024/0025 の
  「URL とコードは action の戻り値にだけ置く」と同じ規律）。
- ディスパッチャは 1 tick に 1 回、設定の全クラスタに `ssh -o BatchMode=yes -O check <host>` を実行する（unix ソケットを見るだけ。ネットワークにも認証にも触れない）。
  接続が戻れば cooldown はその tick で解ける。`auth = "publickey"` のクラスタは、未接続を見つけると cooldown にする前に
  1 回だけ自動で接続を試みる（ADR-0032 D3）。
- GUI は `connected == false` のクラスタに、`auth` に応じた案内を出す（3.39〜3.41）。受信箱の `attention[].cluster_unavailable`（§5.1 (d)）と対。
- `tunnel_forwards[]`（ADR-0053 D3）: `[[clusters.forwards]]`
  （`listen` / `target`）と、観測。`up`（forward 越しに `GET <listen>/v1/models` が届くか＝
  `listener && target_healthy`）、`listener`（手元の `-O forward`/`ssh -N -L` の待ち受けが有るか）、
  `target_healthy`（listener 越しに target が `/v1/models` に応答するか）、`last_error`（直近の失敗理由）。
  観測が無ければ `up`/`listener`/`target_healthy` は**欄ごと省略**し、`last_error` も失敗が無ければ省略する。
  `forwards` を持たないクラスタは空配列。
  - **`listener == true` かつ `target_healthy == false`** は「転送（forward）はあるが先方が応答しない」
    （GUI はこれを 1 語のバッジとは別に、理由の文で示す）。celeris はこの状態では `-O forward` を
    **再発行しない**（listener は既に有るので無意味。ADR-0053 D3）。target の健康 probe 自体も
    `[[clusters.forwards]] probe_interval_secs`（既定 30 秒）の間隔でしか行わない。
  - **`listener == false`** は「転送そのものが無い」。celeris は次の tick で `-O forward` の(再)発行を
    試みる。
- `tunnel_login_needed`（ADR-0053 D3）: `[[clusters.forwards]]` を持つクラスタで、ssh master が落ち、
  **鍵認証を試しても**繋がらなかった状態（人の TOTP 入力が要る）。`GET /clusters` にはこれだけが出る
  （プロンプト文字列やコードは `POST /clusters/{id}/connect`/`connect/code` の応答にだけ載る。§3.39〜3.41
  と同じ経路で接続する）。この状態は Discord にも `cluster_login_needed`（§5.1）で 1 回だけ知らせる。
- `stats`（ADR-0078 D5）: ssh master の接続・切断の回数 `{last_24h, since_start}`。各値は `connects_totp` /
  `connects_publickey` / `connects_borrowed`（人や前の daemon が張った master を見つけた回数）/ `losses` /
  `losses_by_cause`（`check_failed` / `probe_failed` / `master_exited` / `explicit` → 回数）/ `key_auth_attempts` /
  `last_lost_at` / `last_lost_cause`。`last_24h` は DB の `cluster_connection_log` から（再起動をまたぐ）、
  `since_start` はこの daemon の起動以降（スナップショットが無ければ `null`）。
- `work_dir` / `work_dir_source`（ADR-0059 D6）: 実効の作業ディレクトリと出どころ。DB の上書き
  （`PUT /clusters/{id}/settings`）があれば `"settings"`、無ければ設定ファイルの `[[clusters]] work_dir` で `"config"`。
  どちらも無ければ両方とも欄ごと省略。

```json
{"items": [
  {"id": "pegasus", "host": "pegasus", "concurrency": 2, "sync": "rsync", "delete_on_push": false,
   "has_setup": false, "env_keys": [], "rsync_excludes": [], "auth": "totp",
   "in_use": 0, "connected": true, "cooldown_until": null, "cooldown_remaining_secs": null,
   "connect_pending": false,
   "tunnel_forwards": [{"listen": "127.0.0.1:18000", "target": "bnode150:18000", "up": false,
     "listener": true, "target_healthy": false,
     "last_error": "target bnode150:18000 did not answer /v1/models through the forward"}],
   "tunnel_login_needed": false,
   "stats": {"last_24h": {"connects_totp": 1, "connects_publickey": 0, "connects_borrowed": 0, "losses": 0,
                          "losses_by_cause": {}, "key_auth_attempts": 0, "last_lost_at": null, "last_lost_cause": null},
             "since_start": null},
   "work_dir": "/work/NBB/u", "work_dir_source": "settings"}
]}
```

### 3.24〜3.28 プロバイダ管理（ADR-0017。**すべて管理系: `token_file` 未設定でも 401**）

設計は ADR-0017。`config.toml` を直接書き換えず、`providers_include`（例: `providers_include = "providers.d/*.toml"`）が指す
ディレクトリに 1 アカウント 1 ファイル（`providers.d/<id>.toml`。`[[providers]]` の 1 行と同じ形）を読み書きする。
反映（実際の dispatch と `GET /providers`/`GET /config` への表示）は `POST /reload` を呼んだ**次の tick から**で、
実行中の run には影響しない。`providers_include` が設定されていない構成では、3.24〜3.26 は 409
`providers_admin_unavailable` を返す（3.27・3.28 は celeris への管理要求の経路が無い構成で同じ 409）。
3.27・3.28 は `active` のときだけ受け、`standby`/`draining` の間は 503 `standby`（§1.5）。

#### 3.24 `POST /providers` → 201 `ProviderConfigView`（`Location: /api/v1/providers/{id}`）

要求本文: `{"id": "acct-b", "adapter": "fake"|"claude-code"|"codex"|"acp"|"paperqa"|"local-deep-research", "tiers"?: [...], "concurrency"?: 1, "model"?: "", "env"?: {...}, "account_pool"?: false, "account_id"?: "…", "tier_models"?: {...}, "credential_refs"?: {...}}`
（省略したものは `[[providers]]` と同じ既定。`tiers` は 3 tier 全部、`concurrency` は 1）。`id` は 1〜64 文字の ASCII 英数字・`-`・`_`
（`providers.d/<id>.toml` のファイル名になるため、パス区切りは拒否）。`adapter` は既知の 6 種類のみ。
`id`・`adapter` の形が不正、`concurrency` が 0 は 400 `bad_request`。`id` が既にあれば 409 `provider_exists`。
応答・ログとも `env` は `env_keys`（キー名だけ）で、値は一切出さない。

- `tier_models`（ADR-0069 D4）: tier → `{name, model_id?, unavailable_reason?, reasoning_effort?}` の明示束縛。
  `adapter` が `"claude-code"`/`"codex"` 以外で空でなければ 400 `bad_request`。
- `account_id`: プールの中の特定アカウントに固定する。`account_pool = true` が要り（無ければ 400）、
  形はアカウント id と同じ規則（不正なら 400）。
- `credential_refs`: LLM の認証用環境変数（`ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` / `OPENAI_API_KEY` /
  `CODEX_API_KEY`）→ 秘密 id（§3.36〜3.38）。ファイルの `env_from_secrets` に書かれる。キーがこの 4 つ以外か、
  秘密 id の形が不正なら 400。
- `[secrets]` が設定されている構成では、`env` に上の 4 つのキーの値が平文で入っていると、書き込む前に
  秘密（id は `migrated-<ULID>`）へ移し、`env_from_secrets` からの参照に置き換える（値はファイルに残さない）。

**`command`/`args`（ADR-0026 D2: `adapter = "acp"` の行だけが持つ実行ファイル／引数の上書き）・`settings`
（ADR-0027 D3: `adapter = "paperqa"` の行だけが持つ PaperQA 設定ファイルの上書き）・`env_from_secrets`
（ADR-0030 D2）は本文に含められない**（含まれていたら値を見る前に 422 `invalid_provider`。実行するコマンド／設定を
HTTP から差し替えられないようにするため。`providers.d/<id>.toml` は人が直接編集する。ADR-0026 D7、ADR-0027 D3）。
`local-deep-research` の `[adapters.local_deep_research].settings`（ADR-0029 D1）は行ごとの上書きが無く、
そもそも `ProviderConfig` に対応するフィールドが無いので、この制約の対象外（本文に置けるフィールドは他アダプタと同じ）。

#### 3.25 `PATCH /providers/{id}` → 200 `ProviderConfigView`

要求本文は `{"tiers"?, "concurrency"?, "model"?, "env"?, "account_pool"?, "account_id"?, "tier_models"?, "credential_refs"?}`
（渡したフィールドだけ上書き。`id`/`adapter` は変更不可。空の本文は `{}` と同じ）。`account_id: ""` は固定を外す。
`credential_refs` は既存の認証用キーの参照を置き換え、それ以外の `env_from_secrets` は残す。検証と 400 の条件は 3.24 と同じ
（patch 後の組み合わせで判定する）。存在しない `id`（形が不正な `id` を含む）は 404 `provider_not_found`。
**`command`/`args`/`settings`/`env_from_secrets` は 3.24 と同じく本文に含められない**（422
`invalid_provider`。含まれていたらファイルには一切触れない）。ファイルに人が直接書いたこれらの値は、
これらを含まない PATCH では変更されずそのまま残る。

#### 3.26 `DELETE /providers/{id}` → 200 `{}`

`providers.d/<id>.toml` を削除する。存在しない `id`（形が不正な `id` を含む）は 404 `provider_not_found`。

#### 3.27 `POST /providers/{id}/check` → 200 `ProviderCheckResponse`

```json
{"result": "ok" | "auth_failed" | "throttled" | "spawn_failed", "checked_at": "…", "detail": "Confirmed ready; …"}
```

`detail`（ADR-0022 M1）は人が読むための一行（ワーカーの返答、または失敗の理由。無ければ欄ごと省略）。`GET /providers` の `last_check.detail` にも同じ値が出る。
**見ているのは「このアカウントで CLI が起動して応答するか」だけ**なので、ワーカープロトコル上のエラー（`Terminal::Error`）は
アカウントの問題ではなく `ok` として扱い、理由を `detail` に入れる。起動できない・認証切れ・枯渇はそれぞれ
`spawn_failed` / `auth_failed` / `throttled`。

そのアカウントの env で短い run（30 秒・3 ターンまで）を 1 回だけ行い、疎通を確かめる（ADR-0017 D2）。設定は毎回
読み直すので、`reload` 前に足した `providers.d/` の行も確かめられる。DB には
一切触れない（タスクにもイベント列にも残らない、観測値）。結果は**次の tick のスナップショット**にも載り、
`GET /providers` の `last_check` として読める（ADR-0022 D2。celeris の再起動で消える）。
**自動では走らない**（起動時の一括確認も定期実行もしない。1 回ごとに実際の API 呼び出しを消費するため。ADR-0022 D3）。存在しない `id` は 404 `provider_not_found`。設定の
再読込自体が失敗した（`providers.d/` の壊れた TOML 等）場合は 400 `bad_request`。40 秒以内に結果が返らなければ 500 `internal`。
**celeris（`task-worker` に依存する側）が実行し、task-api 自身はワーカーを起動しない**（DESIGN §5.10 の境界。ADR-0017 M2）。

#### 3.28 `POST /reload` → 200 `ReloadResult`（`{"reloaded": true}`）

`config.toml` と `providers_include` の指すディレクトリを読み直す。設定の検証に失敗したら 400 `bad_request`
（`detail` は `invalid config: …`）を返し、稼働中の状態には触れない（古い設定のまま動き続ける）。実行中の run はそれぞれ差し替え前のアダプタ・役割・
分野の写しを既に掴んでいるので、reload の影響を受けない。**反映は次に起動する run / 次 tick から**。

反映されるもの:

| 設定 | 反映先 | いつから効くか |
| --- | --- | --- |
| `[[providers]]` / `providers.d/` | `StaticPolicy`・アダプタ一式・`GET /providers`/`GET /config` の一覧 | 次 tick の dispatch から |
| `[[roles]]` / `[[genres]]` / `[delegation]` | `Dispatcher` の役割・分野・委譲設定（`RunContext`、委譲される子の budget） | 次に起動する run から（実行中の run は古い写しのまま） |
| `[reports]` | 報告の圧縮の閾値 | 次 tick から |
| `[notify]` | 通知の間隔・webhook の秘密 id・GUI base URL | 次 tick から |
| `[conversation]` | 対話が常に走る分野 | 次に始まる対話から |
| `[selfdeploy]` の `delivery_projects` / `delivery_default_departments` | 納品の方針 | 次 tick から |

**cooldown はメモリ上（`StaticPolicy` の内部状態）なので reload で消える**（ADR-0017 D1）。

再起動が要るもの（reload では触れない。変更しても黙って古いまま動き続ける）: `db` / `workspace_root` /
`[api]` / `[[clusters]]`、および秘密の `used_by`（§3.36）。**`[accounts]` だけは例外的にこの reload 自体を 400 で拒否する**（ADR-0024）:
`claude_dir` / `codex_dir` / `max_runs_per_account` / `check_model` のどれかが読み直した設定で変わっていれば、`detail` に
再起動が必要な旨を書いて拒否する（それ以外のフィールドの変更は反映されない）。

### 3.29〜3.35 アカウントのプール（claude-code / codex。ADR-0024・ADR-0025）

設計は ADR-0024（claude-code）と ADR-0025（codex を追加）。`[accounts] claude_dir` / `codex_dir` の下の 1
ディレクトリ（`<claude_dir>/<id>/` または `<codex_dir>/<id>/`）が 1 アカウントで、アカウントは `(adapter, id)`
で識別する（**id が同じでもアダプタが違えば別のアカウント**）。`account_pool = true` のプロバイダ
（claude-code か codex）の run は、残量（claude-code は stream-json の `rate_limit_event`、codex は
`token_count` の `rate_limits` が出す `five_hour` / `seven_day` の `utilization`）から選んだアカウントの環境変数
（claude-code は `CLAUDE_SECURESTORAGE_CONFIG_DIR`、codex は `CODEX_HOME`）で起動する（選び方は ADR-0024 D3。
アダプタが違っても同じ計算式を使う）。`[accounts]` はどちらか一方の根ディレクトリだけでもよい。
`[accounts]` が無い構成では、`GET /accounts` は `{"root": null, "roots": {}, "max_runs_per_account": 0, "items": []}`、管理系は 409
`accounts_unavailable`。**3.30 以降はすべて管理系**（`token_file` 未設定でも 401。ADR-0017 M3）。3.29 は無認証の読み取り。
`id` の規則はプロバイダと同じ（1〜64 文字の ASCII 英数字・`-`・`_`）。3.31〜3.35 は `?adapter=` クエリを受け取る
（省略時 `"claude-code"`。未知の値は 400 `bad_request`）。3.30 はクエリを受け取らず、本文の `adapter` で指定する。
3.31〜3.35 は `active` のときだけ受け、`standby`/`draining` の間は 503 `standby`（§1.5）。celeris への管理要求の経路が
無い構成も 409 `accounts_unavailable`。

#### 3.29 `GET /accounts` → 200 `AccountList`

```json
{"root": "/home/u/celeris/claude-accounts",
 "roots": {"claude-code": "/home/u/celeris/claude-accounts", "codex": "/home/u/celeris/codex-accounts"},
 "max_runs_per_account": 2,
 "items": [
   {"adapter": "claude-code", "id": "a", "dir": "/home/u/celeris/claude-accounts/a", "logged_in": true, "in_use": 1,
     "usage": {"five_hour": {"utilization": 0.14, "resets_at": "…"}, "seven_day": {"utilization": 0.24, "resets_at": "…"},
               "status": "allowed", "observed_at": "…", "source": "run"},
     "score": 0.81, "excluded_reason": null, "cooldown": null,
     "last_check": {"at": "…", "result": "ok", "detail": "ok"}, "login_pending": false,
     "stats": {"runs": 12, "done": 10, "error": 1, "input_tokens": 1234, "output_tokens": 567}},
   {"adapter": "codex", "id": "c", "dir": "/home/u/celeris/codex-accounts/c", "logged_in": false, "in_use": 0,
     "usage": null, "score": null, "excluded_reason": "not_logged_in", "cooldown": null,
     "last_check": null, "login_pending": false,
     "stats": {"runs": 0, "done": 0, "error": 0, "input_tokens": 0, "output_tokens": 0}}]}
```

- `root` は `roots["claude-code"]` の別名として残す（後方互換。ADR-0025 D6）。`roots` はアダプタ →
  設定されていればその絶対パス・無ければ `null`。
- `items[]` は `adapter` → `id` の順（`"claude-code"` が先）。`logged_in` はログイン済みを示すファイル
  （claude-code は `.credentials.json`、codex は `auth.json`）の有無（中身は読まない）。
- `usage` / `score` / `excluded_reason` / `cooldown` / `last_check` / `login_pending` / `in_use` はスナップショットの観測値（最初の tick 前は `null` / `false` / 0）。
  `usage` と `cooldown` はそのアダプタの根ディレクトリの `.celeris-usage.json` にも保存され、再起動後も残る（`last_check` も同じ）。
- `excluded_reason`（選ばれない理由）: `not_logged_in` / `at_capacity` / `cooldown` / `five_hour_exhausted` / `seven_day_exhausted` / `rejected`。選べるなら `null` で `score` が入る。
- `cooldown` は `{until, reason}`。`reason`: `auth_failed`（再ログインが必要）/ `throttled` / `exhausted`。
- `stats` は `WorkerStarted.account` と対応する `WorkerFinished` から、同じ `WorkerStarted.adapter` のものだけ集計する（§5.8 のプロバイダ集計と同じ規則。同じ id でもアダプタが違えば別集計）。
- 時刻はすべて RFC 3339。

#### 3.30 `POST /accounts` → 201 `AccountView`（`Location: /api/v1/accounts/{id}`）

要求本文 `{"id": "b", "adapter": "codex"}`（`adapter` 省略時 `"claude-code"`）。`id` の形が不正、または `adapter` が
`"claude-code"`/`"codex"` 以外なら 400 `bad_request`。`<root>/<id>/` を 0700 で作る
（`root` はそのアダプタの根ディレクトリ。設定されていなければ 409 `accounts_unavailable`）。既にあれば 409
`account_exists`。作っただけでは `logged_in: false`（3.33 でログインする）。

#### 3.31 `DELETE /accounts/{id}` → 200 `{}`

`?adapter=`（省略時 `claude-code`）。ディレクトリを `<root>/.removed/<id>-<unix秒>/` に移す（認証ファイルは消さない。人が後で片付ける）。
`in_use > 0` なら 409 `account_in_use`、無ければ 404 `account_not_found`。進行中のログインは止める。
**実装は celeris 側で行う**（`AdminRequest::AccountRemove` 経由。`in_use` はディスパッチャの権威ある値
`account_in_use`（running/reviewing を直接見る。`(adapter, id)` で判定）で判定するので、task-api のスナップショット経由のレースが無い）。

#### 3.32 `POST /accounts/{id}/check` → 200 `AccountCheckResponse`

`?adapter=`（省略時 `claude-code`）。

```json
{"result": "ok" | "auth_failed" | "throttled" | "spawn_failed", "checked_at": "…", "detail": "ok",
 "usage": {"five_hour": {…}, "seven_day": {…}, "status": "allowed", "observed_at": "…", "source": "check"}}
```

`detail` と `usage` は無ければ欄ごと省略。存在しない `id` は 404 `account_not_found`。
claude-code はそのアカウントで `claude -p … --max-turns 1 --model <[accounts].check_model>` を、codex は
`codex exec --json --skip-git-repo-check "Reply with exactly: ok"`（モデルは指定しない）を、それぞれ 1 回だけ
（60 秒まで）実行し、観測値（`rate_limit_event` / `token_count`）を記録する。codex は出力に 401 の行が出たら
再試行を待たずに打ち切って `auth_failed` にする（未ログインだと 10 回ほど再試行して約 40 秒かかるため）。
**自動では走らない**（ADR-0022 D3 と同じ理由）。celeris 側で実行する（task-api はプロセスを起動しない）。

#### 3.33 `POST /accounts/{id}/login` → 200 `AccountLoginStart`

`?adapter=`（省略時 `claude-code`）。アダプタで流儀が違う（ADR-0025 D5）:

- **claude-code**（`kind: "paste_code"`）: `{"kind": "paste_code", "url": "https://claude.com/cai/oauth/authorize?…", "expires_at": "…"}`。
  celeris が `claude auth login` を `CLAUDE_SECURESTORAGE_CONFIG_DIR=<dir>` で起動し、出力から認可 URL を取り出して返す
  （15 秒以内に出なければ 502 `login_failed`）。人はこの URL を自分のブラウザで開いて認可し、表示されたコードを
  3.34 で渡す。10 分で打ち切る。
- **codex**（`kind: "device_code"`）: `{"kind": "device_code", "url": "https://auth.openai.com/codex/device", "user_code": "ABCD-EFGHI", "expires_at": "…"}`。
  celeris が `codex login --device-auth` を `CODEX_HOME=<dir>` で起動し（標準入力は使わない）、URL と一回限りの
  コードを取り出して返す。人はこの URL を別のデバイスで開いて `user_code` を入力する（**GUI には貼り戻さない**）。
  子プロセスの終了を待って（最大 15 分）完了を判定する: `auth.json` ができていれば `logged_in: true` になる
  （GUI は tick ごとの再検証で気づく）。**`login/code`（3.34）は使わない**。

どちらも既に進行中のログインがあれば古い方を止めてやり直す。**`user_code`・URL はログに出さない**（3.34 も同様）。

#### 3.34 `POST /accounts/{id}/login/code` → 200 `AccountLoginResult`

claude-code のみ。要求本文 `{"code": "…"}`。`{"result": "ok" | "failed", "detail": "…"}`（`detail` は無ければ省略）。コードを `claude auth login` の標準入力に渡し、終了を 30 秒まで待つ。
exit 0 かつ `.credentials.json` ができたら `ok`。空・空白だけのコードは 422 `validation`（`errors[].field = "code"`）。
進行中のログインが無ければ 409 `login_not_started`。**コードはログにも応答にも出さない**。
`?adapter=codex` は本文を見る前に 409 `login_code_not_supported`（codex はデバイス認証だけで完結する。ADR-0025 D5）。

#### 3.35 `DELETE /accounts/{id}/login` → 200 `{}`

`?adapter=`（省略時 `claude-code`）。進行中のログインを止める（無ければ何もしない）。

#### プロバイダ管理への追加（3.19 / 3.24 / 3.25）

`ProviderView` / `ProviderConfigView` / `POST /providers` / `PATCH /providers/{id}` の本文に `account_pool: bool`（既定 `false`）を追加。
`true` は `adapter` が `"claude-code"` か `"codex"` で、かつ `[accounts]` にそのアダプタの根ディレクトリが設定されているときだけ有効
（ADR-0025 D1）。それ以外の `adapter` はもちろん、
**対応する根ディレクトリが設定されていない構成（`GET /accounts` の `roots["<adapter>"]: null`）で `account_pool = true` を
`POST`/`PATCH /providers` に渡した時点で** 422 `invalid_provider` を返す（reload を待たない）。
プール経由の run の失敗は、原因がアカウント側（throttled/auth_failed/exhausted）ならアカウントを cooldown に
しプロバイダは cooldown にしない。**Spawn 失敗（起動できない）はアカウントの責任ではないので、通常どおり
プロバイダを cooldown にする**（ADR-0024 D4）。

### 3.36〜3.38 秘密（API キー等）の管理（ADR-0030。**すべて管理系: `token_file` 未設定でも 401**）

設計は ADR-0030。ADR-0017 の「API キーを GUI から入力して保存しない」を上書きする（人間の依頼）。秘密は celeris が
`[secrets] dir` の下にファイルで持つ（1 秘密 = 1 ファイル、ファイル名 = id、中身 = 値 1 行・0600。ディレクトリは 0700）。**値を返す
API は無い**（作成・削除だけ）。`[secrets]` が未設定なら 3 本とも 409 `secrets_unavailable`。`id` の規則はプロバイダ
と同じ（1〜64 文字の ASCII 英数字・`-`・`_`。`PUT`/`DELETE` の id はパスにあるので、無効な形は
`PATCH`/`DELETE /providers/{id}` と同じ規約で 404 `secret_not_found` にする）。

**使い方（`env_from_secrets`）**: `[adapters.<種別>]` と `[[providers]]` の行の両方に、環境変数名 → 秘密 id の
対応 `env_from_secrets = { LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY = "tavily" }` を書ける（`[[providers]]` の行の
LLM 認証用キーは 3.24 の `credential_refs` からも書ける）。値は `build_adapters`
（アダプタを組み立てるとき）に読むので、鍵を入れ替えたら `POST /reload` が要る（GUI は保存後に自動で呼ぶ）。
優先順は celeris の環境 < `[adapters.*].env` < `[adapters.*].env_from_secrets` < 行の `env` < 行の
`env_from_secrets`。**秘密が見つからないのは設定エラーにしない**（`warn!` を出してその環境変数を渡さず、その層
は下の層の値に道を譲る。run はワーカー自身のエラー（鍵が無い）で失敗する）。`[[providers]].env` に平文で書く
従来の方法も残る（既存設定を壊さない）。

#### 3.36 `GET /secrets` → 200 `SecretList`

```json
{"dir": "/home/u/celeris/secrets",
 "items": [
   {"id": "tavily", "updated_at": "2026-09-17T01:00:00Z", "fingerprint": "a1b2c3d4",
    "used_by": [
      {"scope": "adapter", "name": "local-deep-research", "env": "LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY"},
      {"scope": "provider", "name": "ldr-tavily", "env": "LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY"}
    ]}]}
```

**値は返さない**。`fingerprint` は値（末尾の改行を除く）の sha256 の先頭 8 桁（16 進。人が「入れ替えた鍵が届いているか」を確かめる
ためで、値そのものは復元できない）。`updated_at` はファイルの mtime。`used_by` は celeris の**起動時の設定から導いた**もの
（`[adapters.*].env_from_secrets` と `[[providers]].env_from_secrets` を走査する。`POST /reload` では更新されない。
GUI は再計算しない）。**設定が参照している id は、まだ鍵を入れていなくても `items[]` に載る**
（`updated_at` と `fingerprint` が `null`。GUI が「鍵を入れる場所」を一覧に出せるようにするため）。したがって
`items[]` = ファイルとして存在する秘密 ∪ どこかの `env_from_secrets` が指している id。ファイルがある id を
先に id 昇順、続けて未設定の id を id 昇順で並べる。`dir` のディレクトリがまだ無ければファイルは 0 件として扱う。

#### 3.37 `PUT /secrets/{id}` → 200 `SecretPutResult`

要求本文 `{"value": "tvly-abc123..."}`。作成／置き換え（既にあれば上書き）、ファイルを 0600 で
atomic（一時ファイル→rename）に書く。空文字・空白だけの値は 422 `validation`。本文が JSON として読めない・型違いは
400 `bad_request`（`detail` は固定文で、本文の値を反射しない）。応答は
`{"id", "updated_at", "fingerprint"}`（**値は含まない**）。無効な id（パス区切りを含む等）は 404
`secret_not_found`（`PATCH`/`DELETE /providers/{id}` と同じ規約。本文を見る前に判定する）。

#### 3.38 `DELETE /secrets/{id}` → 200 `{}`

ファイルを削除する。無ければ 404 `secret_not_found`。

**ログ・応答のどこにも値は出ない**（`who = "admin"`, `op = "secret_put" | "secret_delete"`, `secret_id` だけ記録
する。ADR-0024 D5 と同じ規律）。

### 3.39〜3.41 クラスタへの接続を GUI から張る（ADR-0032。**すべて管理系: `token_file` 未設定でも 401**）

設計は ADR-0032。ADR-0018 D2/D7 の「celeris から対話的な認証は絶対に行わない・接続を張るのは人力」を、
`[[clusters]].auth` で opt-in する形に上書きする（既定 `"manual"` は従来どおり celeris が接続を張らない）。
実際の ssh の起動・`SSH_ASKPASS` を使った検証コードの中継は celeris 側（`AdminRequest::ClusterConnect*`）が行い、
task-api 自身は ssh を起動しない（DESIGN §5.10 の境界）。未知の `id`（`[[clusters]]` に無い）はどの操作も
404 `cluster_not_found`。3 本とも `active` のときだけ受け、`standby`/`draining` の間は 503 `standby`（§1.5）。
celeris への管理要求の経路が無い・応答が時間内に返らない（3.39・3.40 は 40 秒、3.41 は 10 秒）ときは 500 `internal`。

#### 3.39 `POST /clusters/{id}/connect` → 200 `ClusterConnectStart`

```json
{"kind": "connected"}
```
```json
{"kind": "needs_code", "prompt": "(rmaeda@130.158.241.2) Verification code: ", "expires_at": "2026-09-17T01:35:00Z"}
```

- `kind = "connected"`: コード不要で張れた（`auth = "publickey"` で鍵だけの接続が通った、または既に
  多重接続があった。ADR-0032 D2: 既にあれば新しく張らずに成功を返す）。`prompt`・`expires_at` は欄ごと省略。
- `kind = "needs_code"`: ssh がプロンプトを出した（`auth = "totp"`）。`prompt` は ssh が実際に出した文字列を
  そのまま返す（ユーザ名・ホスト名を含みうる）。人はこれを見て検証コードを入力し、3.40 に渡す。
  `expires_at`（300 秒後）を過ぎたセッションは celeris が自動で片付ける。進行中のセッションがあれば古い方を畳んでやり直す。
- `auth = "manual"` のクラスタへの `connect` は 409 `cluster_connect_not_supported`
  （人が `scripts/cluster-login.sh` で張る運用のまま。ADR-0032 D7）。
- 接続そのものの失敗（ssh の失敗、タイムアウト）は 502 `cluster_connect_failed`（`detail` に一行の手がかり）。
- **プロンプト文字列はログに出さない**（ユーザ名・ホスト名が入るため。ADR-0032 D4）。`GET /clusters` にも出さない
  （3.23）。

#### 3.40 `POST /clusters/{id}/connect/code` → 200 `ClusterConnectResult`

要求本文 `{"code": "123456"}`。`{"ok": true}`（`detail` は無ければ欄ごと省略）。

- 本文が JSON として読めない・型違いは 400 `bad_request`（`detail` は固定文で、本文の値を反射しない）。
- コードは `trim` して空、または制御文字を含めば ssh に渡さず 422 `validation`（`claude_account.rs::submit_code`
  と同じ注入防止。長さや文字種は制限しない）。この場合 `admin_tx` には何も送らない。
- 進行中のセッションが無ければ 409 `cluster_connect_not_started`。
- **コードが間違っていた（ssh が接続できなかった）場合は 422 ではなく 200 `{"ok": false, "detail": "…"}`**。
  422 は「celeris がコードを ssh に渡すことすら拒んだ」ときだけで、ssh の認証結果は `ok` で伝える
  （ADR-0032 D5。GUI は `ok: false` を握りつぶさずに画面へ出す）。`detail` に**コードは含まれない**。
- **コードは受け取ってもログにも応答にも出さない**（`who = "admin"`, `op = "cluster_connect_code"`, `cluster`
  だけ記録する。ADR-0032 D4）。

#### 3.41 `DELETE /clusters/{id}/connect` → 200 `{}`

進行中の接続セッションがあれば取り消し（ssh の子プロセスをプロセスグループごと落とす）、celeris が保持している
master も落とし、さらに人が張った master が残っている場合に備えて `ssh -o BatchMode=yes -O exit <host>` を呼ぶ。
接続が元から無い（`-O exit` が失敗する）場合も 200。

### 3.42〜3.49 組織・案件・途中目標（ADR-0033 D1/D2）

SPEC §3.2〜§3.3 の「組織（一つ、役割の木）」と「案件・仕事の木」を第一級のエンティティにしたもの。
既存の `tasks` は実行基盤として残り、案件の仕事の木は `tasks WHERE project_id = ?`（DAG は従来どおり
`parent_id` / `depends_on`）。**組織の編集（3.43〜3.45）、案件の作成と変更（3.46 の `POST`、3.48）、
途中目標の作成と変更（3.49）が管理系**（`token_file` 未設定でも 401。案件を作ると秘書の run が起きるので、
`POST /org/{id}/messages` と同じ規律）。読み取り（3.42・3.46 の `GET`・3.47）は通常の要求（トークンを設定した
celeris では、他の全要求と同じくトークンが要る）。

#### 3.42 `GET /org` → 200 `OrgList`

```json
{"items":[{"id":"secretary","parent_id":null,"name":"秘書","kind":"secretary","brief":"…",
           "position":0,"created_at":"…","updated_at":"…"},
          {"id":"research-survey","parent_id":"research","name":"関連研究調査課","kind":"section",
           "genre":"literature","brief":"…","position":6,"created_at":"…","updated_at":"…"}]}
```

- クエリは受けない（知らないクエリキーは 400）。
- 並びは `position` 昇順、同値なら `id` 昇順。**木は GUI が `parent_id` で組む**（API は入れ子にしない）。
- `kind` は `secretary`（根。1 つだけ）/ `department`（部）/ `section`（課）。
- `genre` は `[[genres]] id`（無ければ項目ごと出ない）。その「人」が仕事に使うハーネスの束（ADR-0027/0028）。
  書き込み（3.43 / 3.44）では**設定の `[[genres]]` にある id だけ**を受ける（無い id は 422 `validation`、
  `errors[0].field = "genre"`。分野を 1 つも設定していない celeris では検証しない）。
- 初期の形は `org_include` が指すファイル（`config/org.example.toml`）から、**DB の `org_nodes` が空のときだけ**
  蒔かれる。以後は DB が正で、設定を書き換えても反映されない（ADR-0033 D1）。

各ノードは `profile` を持ち、**子は親を継ぐ**（ADR-0046 D1）。応答には
そのノード自身の `profile`（空なら項目ごと出ない）と、**継いだ後**の `effective_profiles[]` の両方が入る。

```json
{"items":[{"id":"cos","parent_id":null,"name":"Chief of Staff","kind":"secretary",
           "profile":{"harnesses":{"allowed":["conversation","plan"],"default":"conversation"},
                      "knowledge":[{"kind":"kb","scope":"user"}],
                      "policy":["人に返す文は、人が数十秒で読める分量にする。"]},
           "position":0,"created_at":"…","updated_at":"…"},
          {"id":"software-engineering","parent_id":"engineering","name":"Software Engineering","kind":"section",
           "profile":{"skills":["rust","sqlite"],"tools":["gh"],"harnesses":{"default":"coding"}},
           "position":0,"created_at":"…","updated_at":"…"}],
 "effective_profiles":[{"node_id":"cos","chain":["cos"],"harnesses_allowed":["conversation","plan"],
                        "harness_default":"conversation","policy":["…"]},
                       {"node_id":"software-engineering","chain":["cos","engineering","software-engineering"],
                        "skills":["software","rust","sqlite"],"harnesses_allowed":["coding"],
                        "harness_default":"coding","tools":["gh"],"policy":["…","…"]}],
 "lead_sessions":[{"node_id":"engineering","turns":3,"approx_tokens":12345,"last_used_at":"…"}]}
```

`lead_sessions[]`（`NodeSessionSummary`）は部門長（`kind = "department"`）
の継続セッション（`node_sessions`、`kind = "lead"`）が**あるノードだけ**、`node_id` で
対応づけて渡す（無いノードは配列に出ない。空なら `lead_sessions` 自体を省略。ADR-0054 D1/D3）。組織画面はこれで
「継続中のセッション: turns / tokens / 最終使用」を部門長ノードに出す。CoS の対話セッションはここには
出ない（Console のチャット欄自身が状態を見せるため）。

- `effective_profiles[]` は `items[]` と**同じ並び**で、`node_id` で対応づく（空なら省略）。`chain` は根から葉までの
  ノード id（GUI の「どこから継いだか」）。
- 継ぎ方（ADR-0046 D1。GUI はこれを再実装しない。表示は `effective_profiles` をそのまま使う）:
  `skills` / `knowledge` / `skills_mounts` / `tools` / `harnesses.allowed` / `permissions.approvals` は**親と和**
  （根→葉の順、重複は落ちる）、`deny_tools` は和だが**常に勝つ**（実効の `tools` から引かれる）、
  `browser` / `run` / `model.tier` / `harnesses.default` / `review.*` は**子が勝つ**、`model.allowed_tiers` は**交わり**
  （空の親は制限なし）、`budget.max_lane` / `budget.max_attempts` は**最も厳しい値**（ADR-0069 D2）、
  `policy` は根→葉の順に**連結**。
- `profile` の項目はすべて任意。空の `profile` は応答に出ない。

#### 3.43 `POST /org` → 201 `OrgNode`（`Location: /api/v1/org/{id}`）（**管理系**）

要求本文 `{"id":"coding-poc","name":"PoC・R&D 課","kind":"section","parent_id":"coding","genre":"coding","brief":"…","position":4}`。
`parent_id` / `genre` / `brief` / `position` / `profile` は省略可（`brief` と `position` の既定は `""` / `0`）。
知らない欄・本文の解析の失敗は 400 `bad_request`。

- `id` は英小文字ケバブ（`[a-z0-9-]`、1〜64 文字、先頭末尾は `-` でない）。
- 既にある `id` は 409 `org_node_exists`（更新は 3.44）。
- 検証に落ちたら 422 `validation`: `id` の綴り違反、空白だけの `name`、秘書が 2 人、秘書に親がある、
  秘書以外に親が無い、親が存在しない、自分を祖先にする、種類の順序違反（`secretary` > `department` > `section`）。

任意で `profile` を受ける（省略時は空。ADR-0046 D1）。`profile` の検証に落ちたら
422 `validation`（`errors[0].field = "profile"`）:

- `tools` / `deny_tools` の語彙は `gh` / `tavily` / `exa` / `docker` / `cluster:<id>` だけ（それ以外は 422）。
- `harnesses.allowed[]` / `harnesses.default` / `review.harness` は**設定にある分野（ハーネス）id**か組み込み
  （`conversation` / `plan` / `reviewer` / `smoke` / `knowledge`）だけ（分野を 1 つも設定していない celeris では検証しない）。
- `skills[]` は小文字の `[a-z0-9._-]`、1〜64 文字。`skills_mounts[]` は小文字の `[a-z0-9-]`、1〜64 文字。
- `browser` は `BrowserCapability` の検証に通ること。
- `run` は `"host"` / `"container"`、`model.tier` と `model.allowed_tiers[]` と `review.tier` は
  `"frontier"` / `"standard"` / `"cheap"` だけ（知らない綴り・知らない欄は本文の解析で落ちて 400 `bad_request`）。

#### 3.44 `PATCH /org/{id}` → 200 `OrgNode`（**管理系**）

`{"name":…, "kind":…, "parent_id":…, "genre":…, "brief":…, "position":…, "profile":…}` のうち**書いた項目だけ**を変える
（空の本文は `{}` と同じ）。`genre` は設定の `[[genres]]` にある id だけ（無い id は 422 `validation`）。
`"genre": null` と書けば分野を外せる（書かなければ今の値のまま）。`parent_id` は付け替えだけで、`null` は
「書かない」と同じ（親を外せない）。無い id は 404 `org_node_not_found`。
検証は 3.43 と同じ（付け替え・`kind` の変更で木が壊れるなら 422。既存の子との種類の順序も見る）。

`profile` は**丸ごと差し替え**（部分更新はしない）。書かなければ今の値のまま、
`{}` を書けば空になる。検証は 3.43 と同じ。実効 profile（継いだ後）は `GET /org` の
`effective_profiles[]` で読む（`PATCH` の応答は**そのノード自身の** `profile` だけを返す。ADR-0046 D1）。

#### 3.45 `DELETE /org/{id}` → 204（**管理系**）

- そのノードを `assignee` に持つ**未終了のタスク**があれば 409 `org_node_in_use`（SPEC の「消すときに仕事を
  抱えていたら」。ADR-0033 D1）。
- 子ノードが残っていても 409 `org_node_in_use`（木を宙ぶらりんにしない）。
- 無い id は 404 `org_node_not_found`。

#### 3.46 `GET /projects` → 200 `ProjectList` / `POST /projects` → 201 `Project`（**`POST` は管理系**）

`POST` の要求本文 `{"title":"Pluvio の新テーマ","request":"Pluvio を基盤に用いた新たな研究テーマの模索、検証"}`
（応答に `Location: /api/v1/projects/{id}`）。
作られた案件は必ず `status = "proposed"`（秘書が理解確認・方針を返すまで人の返事待ち。
SPEC §7 / ADR-0033 D2）。空白だけの `title` / `request` は 422 `validation`。知らない欄は 400 `bad_request`。
一覧は `created_at` の降順。

`GET /projects` は**アーカイブされた案件を既定で隠す**。`?archived=1`
（`1`/`0`/`true`/`false`）で全部返す（他のクエリキーは 400）。`GET /projects/{id}`（個別）は既定でもそのまま見える。
`Project` の状態は `proposed` / `active` / `paused` / `done` / `cancelled`。`paused` / `cancelled` は
§3.84〜3.91 の専用のエンドポイントでだけ入る（ADR-0044 D6）。

任意で `workspace`（**案件の作業場所** = コードのある場所）を受ける（ADR-0039 D1）:

```json
{"title":"Pluvio の PoC","request":"…",
 "workspace":{"kind":"local","path":"~/workspace/rust/pluvio-poc"}}
{"title":"benchfs の検証","request":"…",
 "workspace":{"kind":"remote","cluster":"pegasus","path":"/work/NBB/rmaeda/workspace/rust/benchfs"}}
```

- `kind` は `local`（手元の普段のパス。SPEC §2.1）か `remote`（クラスタ側の作業ディレクトリ。ADR-0018 D1。
  celeris は写しを持ち、`sync` の設定で往復する）。
- `local` の `~` / `~/…` は celeris の `$HOME` で**展開して保存する**（`GET` は展開後の絶対パスを返す）。
  `remote` の `path` はクラスタ側なので展開しない。`remote` の `path` は省略可（省略・相対パスはクラスタの
  `work_dir` から解決する。ADR-0059 D6）。
- `remote` の `cluster` が `[[clusters]]`（`GET /clusters`）に無ければ 422 `validation`
  （`errors[].field = "workspace.cluster"`）。
- 省略すれば「作業場所なし」（`Project.workspace` は応答に出ない）。
- この作業場所は案件に属するタスクの**子タスク（分解・委譲）**が継ぐ（明示 > 案件 > 親。ADR-0039 D2）。
  コードを扱う案件では、GUI から必ず入れてもらうのがよい。

案件は**リポジトリを複数持てる**（3.68〜3.71。ADR-0043 D1）。
`POST /projects {workspace}` と `PATCH /projects/{id} {workspace}` は **primary のリポジトリを
作る／書き換える**（`"workspace": null` は primary を消す。未終端のタスクが使っていれば 409 `repo_in_use`）。
`Project.workspace` は primary の `location` の写し。`GET /projects/{id}` の応答には `repos[]`（primary が先頭）が入る。

`kind = "local"` も `kind = "remote"` も任意で `mode` を持てる（`"worktree"` | `"shared"`、**既定
`"worktree"`**。ADR-0041 D1 / ADR-0059 D1）。知らない値は 400 `bad_request`（本文の解析で落ちる）。

```json
{"workspace":{"kind":"local","path":"~/workspace/agent-platform","mode":"worktree"}}
```

- `local` の `"worktree"`（既定）: `path` が git リポジトリなら、celeris は**タスクごとに `git worktree` を切る**
  （ブランチは `celeris/<task_id>`。接頭辞は `[workspace] worktree_branch_prefix`）。run のログ（`runs/`）と
  成果物（`artifacts/`）は**作業ツリーの外**の `<workspace_root>/<task_id>/` に置かれ、ファイル系エンドポイント
  （§3.7〜§3.9）と `workspace_dir` もそちらを指す。置き場所・base・後片付けは ADR-0041 D1 / ADR-0043 / ADR-0079 D6。
- `local` の `"shared"`: `path` をそのまま作業ディレクトリにする（celeris 専用の使い捨てリポジトリ向け）。
- `path` が git リポジトリでなければ `"worktree"` でも `"shared"` と同じ。
- `remote` の `mode` はクラスタ側の同期方針（`"worktree"` = クラスタの `sync` に従う。ADR-0059 D1）。
- 省略したものは応答の JSON にも出ない。

#### 3.47 `GET /projects/{id}` → 200 `ProjectDetail`

```
GET /api/v1/projects/{id}?include_frozen=true
```

```json
{"project":{…Project…},
 "repos":[{…ProjectRepo…}],
 "milestones":[],
 "milestones_frozen":2,
 "milestones_frozen_open":1,
 "tasks":[{"id":"01J…","title":"調べる","status":"ready","parent_id":null,"depends_on":[],
           "assignee":"research-survey","milestone_id":null,"is_root_task":true,
           "conversation":false,"support":null}],
 "root_totals":{…ProjectRootTotals…}}
```

`project` には案件の作業場所（`workspace`。ADR-0039 D1。決めていない案件では出ない）も入る。
`repos` は案件のリポジトリ（primary が先頭。ADR-0043 D1）。
`tasks` は**仕事の木を描くのに必要な分だけ**（`created_at` 降順で最大 2,000 件。詳細は `GET /tasks/{id}`）。
`project_id` が一致するタスクだけが入り、他の案件・案件に属さないタスクは出ない。
ULID でない id・無い案件は 404 `project_not_found`。知らないクエリキーは 400。

- `is_root_task` が `true` の行は案件の root task（案件直下・木の子でない・対話でも裏方でもない。ADR-0079 D13）。
- `conversation` が `true` の行は**対話用タスク**（人への返事のための run。3.54 参照）なので、仕事の木からは
  隠してよい。`support`（§3.3 の `TaskSummary.support` と同じ規則）が `null` でない行は裏方なので、
  GUI は仕事の木から一括で外せる。
- `root_totals` は root task の数（状態ごと）とその subtree の合計（`tasks` と同じ 2,000 件の範囲。ADR-0079 D11）。

途中目標と案件計画は**凍結した履歴**（ADR-0079 D13）。既定では `milestones` は空で、件数だけ
`milestones_frozen`（全行）と `milestones_frozen_open`（そのうち終端でないまま凍結した行）に入る。
`?include_frozen=true` のときだけ、`milestones[]` に全行と `project_plan`（案件計画の DAG。無ければ省略）を
読み取り専用で返す。

`milestones[]` の各行は `Milestone` のフィールドが**そのまま平らに**出たうえで、
2 つが増える（どちらも無ければ省略。ADR-0038 D1 / D4）:

- `review`: その途中目標についての**秘書のレビューの返事**（`{message_id, text, at}`）。
- `proposal`: その返事が提案した**次の途中目標**（`proposed` の最新。`review` がある行にだけ付く）。

#### 3.48 `PATCH /projects/{id}` → 200 `Project`（**管理系**）

`{"status":…, "workspace":…, "slug":…, "title":…, "request":…}` のうち**書いたものだけ**を変える。
知らない欄・知らない `status` の値は 400 `bad_request`（本文の解析で落ちる）。

- `status`: `"proposed"` / `"active"` / `"done"`。**`paused` / `cancelled` は入れられない**
  （422 `validation`、`errors[0].field = "status"`。§3.84〜3.91 の専用のエンドポイントを使う。ADR-0044 D6）。
- `{"workspace":{"kind":"local","path":"~/workspace/rust/pluvio-poc"}}` — 作業場所（primary のリポジトリ）を
  設定・差し替える（`POST` と同じ検証・`~` の展開・422）。`{"workspace":null}` は作業場所を消す
  （未終端のタスクが使っていれば 409 `repo_in_use`）。
- `slug`: 知識ベースの置き場 `projects/<slug>/` の slug（小文字の `[a-z0-9-]`、1〜64 文字、`-` の連続・先頭末尾は
  不可、案件 ID の形は不可。違反は 422、他の案件と重複すれば 409 `project_slug_in_use`）。KB のディレクトリは動かさない
  （ADR-0044 D7）。
- `title`（前後の空白を除いて 1〜200 文字）/ `request`（同じく 1〜20,000 文字）。空・長すぎるものは 422。
  `request` を書き換えても run は起きない（ADR-0072）。
- `auto_advance` は廃止（書けば 422 `validation`、`errors[0].field = "auto_advance"`。ADR-0079 D13）。
- どれも書かない（`{}`）は 422 `validation`。
- ULID でない id・無い案件は 404 `project_not_found`。

#### 3.49 途中目標: `POST /projects/{id}/milestones` → 201 `Milestone` / `PATCH /milestones/{id}` → 200 `Milestone`

**廃止**（ADR-0079 D13）。どちらも管理系の認証（401）を通ったあと、本文を見ずに 410 `removed_by_adr_0079`
（`adr = "ADR-0079"`、`instead` に代わりの経路）を返す。途中目標は root task の段階で表す
（`POST /tasks` に `project_id` と `stages_hint`）。既存の途中目標は凍結され、読み取りは
`GET /projects/{id}?include_frozen=true`（3.47）。

### 3.50〜3.53 報告（ADR-0033 D3）

報告は**下から上へ**流れる。生成は決定的（run の `done` / `error` / `question` から celeris が 1 件作る。LLM は呼ばない）、
**圧縮だけが LLM**（親ノードに子の報告が既定 4 件（`[reports] compress_after`）たまるか、最古が既定 2 時間
（`[reports] compress_after_secs`）を過ぎたら「まとめの run」を 1 回起こし、その `done` が
親の報告になる。`sources` に子の id が入る）。**悪い知らせ（`bad_news`）は圧縮を待たず、各祖先に複製されて秘書まで届く**
（SPEC §2.4）。人が見るのは `level = 0`（秘書）の報告。

- **読み取り（3.50 / 3.51）は通常の認証**、**既読と通知（3.52 / 3.53）は管理系**（`token_file` 未設定でも 401）。

#### 3.50 `GET /reports` → 200 `ReportList`

```
GET /api/v1/reports?project=<ULID>&node=<org id>&level=<n>&unread=true&limit=50
```

- 新しい順（`created_at` 降順、同値は id 降順）。`limit` の既定は 50、1〜500 に丸める。知らないクエリキーは 400。
  ULID でない `project` は 400 `bad_request`。
- `level=0&unread=true` が**秘書レベルの未読**（GUI の「報告の流れ」の既定）。
- `Report`: `{id, project_id?, node_id, task_id?, kind, level, headline, body, sources[], read_at?, created_at}`。
  `kind` は `progress` / `result` / `bad_news` / `proposal` / `question`。`project_id` が無いものは「案件なし」
  （クラスタが落ちた等、案件に紐づかない悪い知らせ）。

#### 3.51 `GET /reports/{id}` → 200 `ReportDetail`

- `{report, sources_expanded[]}`。`sources_expanded` は `report.sources` の順に引いた元の報告（消えていたものは飛ばす）。
- 無い id・ULID でない id は 404 `report_not_found`。

#### 3.52 `POST /reports/read` → 200 `{updated}`（**管理系**）

- 本文 `{"ids": ["<report id>", …]}`。既に既読のものは触らない（`updated` は未読から既読に変わった件数）。
- ULID でない id は 404 `report_not_found`（無い ULID は数えずに飛ばす）。

#### 3.53 `POST /reports/notified` → 200 `{last_notified_at}`（**管理系**）

- GUI がブラウザ通知を出したときに呼ぶ（本文は取らない）。次の通知は**2 時間後**まで出ない（`bad_news` の未読を除く。
  SPEC §3.5「通知は数時間単位」）。
- `last_notified_at` は **API プロセスのメモリ**にある観測値で、DB には書かない（celeris を再起動すると「まだ通知していない」に戻る）。

#### `GET /daemon` への追加（3.20）

`DaemonSnapshot.reports`（`ReportsLive`。未読数を読めなければ `null`）:

```
reports: { unread_secretary: u32, unread_bad_news: u32, last_notified_at?: String, notify_now: bool }
```

`notify_now` は決定的に決まる: **`bad_news` の未読があれば即 true**、無ければ「未読があり、まだ通知していないか前回の通知から 2 時間以上経った」とき true。
この 1 フィールドだけはディスパッチャではなく**API が応答を組むときに埋める**（`last_notified_at` が API 側にあるため）。

#### 3.54〜3.55 対話（ADR-0033 D4）

SPEC §3.4「組織の木を見て誰に言うかを決め、その担当に直接言う。相手は人なので先週の議論の続きとして話せる」。
**新しいプロトコルは足していない**: 話しかけると `messages` に `role = "user"` の行が 1 つ入り、そのノードの
run を 1 回起こすための**対話用タスク**（`kind = "execute"`、受け入れ条件なし、`assignee` = そのノード、
`title` = `"対話: <本文の先頭 40 字>"`）が `ready` で 1 件できる。返事はその run の `artifacts/result.json` の
`summary` で、ディスパッチャが `role = "node"` の行として（`run_id` 付きで）足す。

#### 3.54 `GET /org/{id}/messages?project=<ULID>&limit=<n>` → 200 `MessageList`

```json
{"items":[{"id":"01J…","node_id":"secretary","project_id":"01J…","role":"user",
           "text":"この案件をお願いします","task_id":"01J…","created_at":"…"},
          {"id":"01J…","node_id":"secretary","project_id":"01J…","role":"node",
           "text":"理解の確認です。…","run_id":"01J…","task_id":"01J…","created_at":"…"}]}
```

- `task_id` は**その 1 往復を起こした対話用タスク**（`role = "user"` の行にも `role = "node"` の行にも
  同じ id。古い行には無い）。`metadata`（`author` や実行した操作の記録）は有るときだけ出る。

- 並びは**古い順**（`created_at` 昇順、同値は `id` 昇順）。`limit`（既定 50、上限 500、0 は 400）を超えるときは
  **新しい方**を残す（直近のやり取りを読むため）。知らないクエリキーは 400。
- `project` を書けばその案件のスレッド、書かなければ**案件に紐づかない雑談**だけ（混ざらない）。
- 無いノードは 404 `org_node_not_found`、ULID でない `project` は 404 `project_not_found`
  （存在しない案件の ULID は空の一覧）。
- 読み取りなので管理系ではない（トークンを設定した celeris では他の読み取りと同じくトークンが要る）。
- GUI はここをポーリングするか、`GET /stream` のイベントを見て引き直す（返事は同期では返らない）。

#### 3.55 `POST /org/{id}/messages` → 202 `MessageAccepted`（**管理系**）

要求本文 `{"text":"先週の続きで、隣接分野も見てほしい","project_id":"01J…"}`（`project_id` は省略可）。
応答は `{"message_id":"01J…","task_id":"01J…"}`。

- **202**（同期で返事を待たない）。返事が入ったかは 3.54 で見る。
- 人格を持つノードに指示を出す経路なので**管理系**（`token_file` 未設定でも 401。`POST /org` と同じ規律）。
- 無いノードは 404 `org_node_not_found`。空白だけの `text`・存在しない `project_id` は 422 `validation`。
- run の道具立ては決定的に決まる: ノードの `genre` に関係なく**対話用分野**（設定の `[conversation] genre`、
  既定 `"secretary"`）→ その分野の `default_role` → 役割の `tier` / `adapter`（役割が無ければ `standard`）。
  予算は対話用の小さい既定（`max_turns = 10` / `max_wall_secs = 300` / `max_retries = 1`）で、役割の既定より優先する。
- run のプロンプトには、ノードの `brief`・そのノードの長期記憶（ADR-0033 D6）・**この案件のこのノードとの
  直近のやり取り（既定 20 件）**が前置きされる。
- run が `error` に終わったときの返事は `"返事できませんでした: <理由>"`。これを書くのは**タスクが `failed` に
  落ちたときだけ**で、途中のやり直し（retryable / requeue）では書かない（1 通の問いに返事は 1 行）。
  `question` は本文をそのまま返事にすることに加え、**`approvals` に 1 件を作る**（§3.6）。
- **同じノード・同じ案件の対話は直列**: 未終了の対話タスクがあれば、新しい対話タスクの
  `depends_on` にそれが入る。つまり 2 通続けて送ると、2 通目は 1 通目の返事が終わるまで `ready` にならない
  （GUI は `GET /tasks/{id}` の `depends_on` で待ち行列を見せられる）。CoS（`cos`）だけは**案件をまたいで**直列
  （継続セッションが全体で 1 本のため。ADR-0054 D1/D2）。
- 対話用タスクは**報告を作らない**（返事は `messages` で読むもので、報告の流れには出ない）。
- **秘書の最初の返事**: `POST /projects`（3.46。こちらも管理系）で案件を作ると、その直後に秘書ノード
  （`kind = "secretary"`）へ `request` を本文とした対話が 1 回自動で起きる（SPEC §7）。秘書がいない構成
  （組織を種蒔きしていない）では何も起きない。失敗しても案件の作成は成功のまま。

### 3.56〜3.60 認可（ADR-0033 D5）

SPEC §3.6「少しでも聞くべきだとエージェントが判断したら、あなたに指示を仰ぐ。あなたはそれに対して
『今回だけ』か『同じようなことは今後ずっと』のどちらかの認可を出す。永続の認可は文字で記録してエージェントに
注入する」。既存の `Question` 終端（`Status::Blocked` / `answers[]`。ADR-0010）に接続する: run が質問で終わると
`approvals` に 1 件できる（宛先は `task.assignee`、無ければ秘書）。**新しいプロトコルは足していない**:
人が答えると、既存の「質問に答える」経路（`POST /tasks/{id}/answer` と同じ `answers[]`）でタスクが再開する。

- **読み取り（3.56 / 3.58）は通常の認証**、**決める・作る・消す（3.57 / 3.59 / 3.60）は管理系**
  （`token_file` 未設定でも 401）。

#### 3.56 `GET /approvals?pending=&project=&node=` → 200 `ApprovalList`

- 古い順（`created_at` 昇順、同値は id 昇順。答える順に並ぶキュー）。`pending` は**三値**:
  `pending=true` で未決定だけ（`decision` 無し）、`pending=false` で**決定済みだけ**（`decision` あり。
  「決めたものの履歴」）、省略すると全件。
  `project` / `node` と AND で効く。
- `Approval`: `{id, project_id?, node_id, task_id?, question, decision?, answer?, created_at, decided_at?}`。
  `decision` は `once` / `standing` / `denied` / `withdrawn`（未決定は無い）。
- **`withdrawn`（ADR-0033 D5）**: 認可元のタスク（`task_id`）が終端
  （`done` / `failed` / `cancelled`）になったので、**celeris が自動で閉じた**（人の決定ではない）。
  人が中止した・子として連鎖で中止された・依存先の失敗で `dependency_failed` になった・run が失敗した・
  完了した、のどれでも、その終端への遷移と**同じトランザクション**で、そのタスクの未決の行がすべて
  `decision = "withdrawn"`、`answer = "task <status>: 認可元のタスクが終わったため、celeris が自動で取り下げました"`、
  `decided_at` = 遷移の時刻になり、タスクに `Event::ApprovalsWithdrawn {approval_ids, task_status,
  reason: "task_terminal"}`（`type = "approvals_withdrawn"`）が 1 件つく。遷移で取りこぼした行は、ディスパッチャの tick の照合が
  同じ形で閉じる（`reason: "reconcile"`）。
  閉じた行は `pending=true`・`DaemonSnapshot.approvals_pending`・受信箱の `questions[].approval_id`・
  通知から消え、`pending=false`（決めたものの履歴）に残る。部をまたぐ委譲の判定では `withdrawn` を
  「まだ決まっていない」と読む（`denied` のように「もう聞かない」にはしない）。
- **部をまたぐ委譲の認可**（SPEC §3.1）は `question` が
  **`"cross-department: <委譲元> -> <委譲先>: <理由>"`** の固定の形で来る（`node_id` = 委譲元、
  `task_id` = 委譲しようとした親タスク）。`once` ならそのタスクの次の run で委譲が通り、`standing` なら
  以後ずっと通る（このとき `standing_rules.rule` には答えの文ではなく
  **`"cross-department: <委譲元> -> <委譲先>"`** が入る）。`denied` なら子は作られず、ワーカーには
  `answers[]` の「認めない: …」が見える。GUI は接頭辞 `cross-department: ` で「連携の認可」として
  見せ分けられる。

#### 3.57 `POST /approvals/{id}/decide` → 200 `ApprovalDecideResult`（**管理系**）

要求本文 `{"decision":"once"|"standing"|"denied","answer":"…","scope":"node"|"all"}`（`scope` は省略可、既定
`node`。`standing` のときだけ意味を持つ）。

- `once` → 既存の「質問に答える」経路（`answers[]`）でそのタスクを再開する。
- `standing` → 同じことをして、さらに `standing_rules` に 1 行追加する（`scope = "node"` ならそのノード宛て、
  `"all"` なら全員）。
- `denied` → 答えを `"認めない: <answer>"` にして再開する（ワーカーが自分で判断できるように）。
- 応答は `{approval, standing_rule?, transition?, note?}`。`standing_rule` は `decision = "standing"` のときだけ、
  `transition`（`TransitionResult`。3.12 `POST /tasks/{id}/answer` と同じ形）は `approval.task_id` のタスクに
  答えて再開したときだけ載る。
- **認可元のタスクの状態で分かれる（ADR-0033 D5）**。どれも**書く前に**判定する（半端に書かない）:
  - `blocked`（段階の途中確認 `awaiting_human`・計画の承認待ち `awaiting_plan_approval` ではない）:
    上のとおり答えて再開する。
  - **終端（`done` / `failed` / `cancelled`）またはタスクが無い**: 答える相手がいないので、**決定だけ記録して
    200**（`once` / `standing` / `denied` のどれでも。`standing` なら規則も足す）。`transition` は載らず、
    `note`（例 `"task … is already cancelled; decision recorded without resuming the task"`、タスクが無ければ
    `"task … no longer exists; decision recorded only"`）が載る。
    取り下げ済み（`withdrawn`）の行に人が答え直すのもこれ。
  - それ以外（`ready` / `running` / `reviewing` / `draft`、途中確認・計画の承認待ちの `blocked`）:
    409 `invalid_transition`（`task_status` / `kind` / `trigger: "decide"` 付き）、**何も書かない**（行は未決のまま）。
  - `task_id` の無い行: 決定だけ記録して 200（`transition` も `note` も載らない）。
- `decision = "withdrawn"` は celeris だけが書く。人が送ると 422 `validation`。
- `answer` が空白だけは 422 `validation`。無い id・ULID でない id は 404 `approval_not_found`。
  `scope` が `"node"`/`"all"` 以外、`decision` が上の 4 値以外、知らない欄（`deny_unknown_fields`）は 400。

#### 3.58 `GET /standing-rules?node=` → 200 `StandingRuleList`

- `node` を書けば**全員向け（`node_id = null`）+ そのノード向け**、書かなければ**絞り込み無し（全ノード分。
  GUI の一覧・編集用）**。古い順。
- `StandingRule`: `{id, node_id?, rule, created_at}`。`node_id` が無いものは全員向け。

#### 3.59 `POST /standing-rules` → 201 `StandingRule`（**管理系**）

要求本文 `{"node_id":"coding-poc","rule":"…"}`（`node_id` は省略すると全員向け）。GUI から直接、質問を経ずに
永続の認可を足すためのもの。`rule` が空白だけは 422 `validation`。

#### 3.60 `DELETE /standing-rules/{id}` → 204（**管理系**）

無い id・ULID でない id は 404 `standing_rule_not_found`。

#### `GET /daemon` への追加（3.20）

`DaemonSnapshot.approvals_pending`（`u32`。古いスナップショットには無いので既定は 0）: 未決定の認可の件数。
`reports` と同じ理由で**API が応答を組むときに埋める**（ディスパッチャの送るスナップショットでは常に 0）。

### 3.61〜3.62 GUI 監査対応の API（ADR-0033 D4 / D6）

3.61 は撤去済みで 410（ADR-0079 D13。`POST /milestones/{id}/decide` も同じ撤去理由で §3.125.8 にまとめてある）。
3.62 は SPEC §3.2 の「記憶は案件をまたぐ」を人が確認するための読み取り専用の窓口（ADR-0033 D6）。

#### 3.61 `POST /projects/{id}/plan` → 410 `removed_by_adr_0079`（**管理系**）

撤去済み（ADR-0079 D13。§3.125.8）。案件は計画を持たない。**本文も id も読まずに** 410 を返す（管理系のまま:
トークンが無ければ先に 401）。`instead` は「`POST /tasks` に `project_id` を付けて root task を作る（段階は
`stages_hint` で名付ける）」。要求・応答の型（`ProjectPlanBody` / `ProjectPlanAccepted`）は
`api-v1.schema.json` の互換のためにだけ残る。

#### 3.62 `GET /org/{id}/memory?project=<id>` → 200 `{notes, project, notes_path, project_path}`（読み取り）

```json
{"notes":"- 2026-09-17: pegasus は pjsub で投げる\n",
 "project":"- 2026-09-17: Pluvio は非同期ランタイム基盤らしい\n",
 "notes_path":"/var/lib/celeris/memory/secretary/notes.md",
 "project_path":"/var/lib/celeris/memory/secretary/projects/01J….md"}
```

- `<memory_dir>/<node_id>/notes.md`（案件をまたぐ記憶）と `projects/<project_id>.md`（案件の引き出し）の
  **全文**（前置き用の 8,000 字カット。ADR-0033 D6 とは別で、上限は切らない）。無ければ空文字列。
  `project` を書かない・空文字列なら `project` / `project_path` は `null`（`project` は案件の存在を確かめない。
  ファイルが正）。
- `[memory]`（`config.toml`）が設定されていなければ 409 `memory_unavailable`（ノードの確認より先）。
  無いノードは 404 `org_node_not_found`。知らないクエリは 400。
- **書き込み API は無い**（記憶は run の後にワーカーが書く。ADR-0033 D6）。人が直したければ
  `notes_path` / `project_path` のファイルを直接編集する。この応答がパスを返すのはそのため。

### 3.63 `POST /tasks/{id}/retry` → 201 `RetryResult`

経緯は [ADR-0070](../../../agent-docs/adr/0070-task-failure-visibility-and-handoff-safe-runs.md) を参照。要求本文 `RetryBody` は省略可。
`accept` の既定は `true`。`workspace` を指定すると複製先の作業場所を差し替える。
`execution` は `"compound"` / `"atomic"` の明示指定で、対象外のタスクには 422 を返す。
指定しなければ元の `execution_hint` を継ぎ、元の gate 判定は継がずに再判定する（§3.125.6）。

- `failed` または `cancelled` のタスクを**複製して新しいタスクを作る**（`Failed`/`Cancelled` を非終端に
  戻す状態機械の遷移は**足していない**。DESIGN の状態機械を壊さないため）。それ以外の状態は 409
  `invalid_transition`（`trigger: "retry"`）。無いタスクは 404 `task_not_found`。
- 複製するもの: `title` / `objective` / `acceptance` / `inputs` / `worker_hint` / `budget` / `role` /
  `genre` / `project_id` / `milestone_id` / `assignee` / `parent_id` / `workspace`。`depends_on` は
  **元と同じ**。`attempts` は 0 から。新しいタスクは既定で `ready` から始まる。`accept: false`
  を本文に付けたときだけ `draft` にする（別の遷移は経由しない。`task_ops::add::create_support_task` と同じ「その場で
  `ready` を書く」流儀）。新しいタスクに `Event::Created` + `Event::Retried{from: <元の id>}` を記録する。
- **元のタスクに依存していた未終端のタスク**（`draft` / `ready` / `blocked`。対話タスクは
  `DependencyFailed` の対象外なので `draft`/`ready` のまま残っていることがある。P-78）と、**その依存の
  失敗で `cancelled` になっていたタスク**（最後の `Transitioned` の `reason` が `"dependency_failed"`
  のもの。人が直接 `cancel` したものは対象外）の `depends_on` を、元の id から新しい id に張り替える。
  後者は `cancelled` → `draft` に戻し（`Event::Transitioned{from: "cancelled", to: "draft",
  reason: "retried"}` を記録）、前者は状態を変えずに `depends_on` だけ書き換える。
- 応答は `201 {"task_id": "01J…", "rewired": ["01J…", …]}`（`Location: /api/v1/tasks/{task_id}`）。
  `rewired` は張り替えたタスクの id（順不同）。
- 管理系。`token_file` が無くてもトークン無しの操作は 401。
- `task_ops::actions(task)`（§5.4）は `failed` / `cancelled` のタスクに `Action::Retry`（`"retry"`）を足す。
  受信箱の `failed` 項目、`GET /tasks/{id}` の `failed`/`cancelled` 表示、案件の仕事の木の失敗ノードは、
  みな `actions` にこれが立つのでボタンの表示に迷わない。

### 3.64〜3.65 通知（Discord）（ADR-0037）

「人の判断が要るとき」だけ Discord の webhook に 1 通投げる仕組みの、設定の確認とテスト送信。
**判定と送信は celeris の tick が決定的に行う**（LLM は関与しない）。API は台帳（`notifications` 表）を
読むだけで、送信は celeris に委譲する。

知らせるのは「人の判断が要る」出来事だけ（ADR-0037 D1）: `milestone_ready` / `approval_pending` /
`question_blocked` / `bad_news` / `secretary_reply` / `task_ready` / `cluster_login_needed`
（`cluster_login_needed` は ADR-0053 D3。クラスタの ssh master が落ち、鍵認証も失敗して
人の TOTP 入力が要る状態。`key` = クラスタ id。celeris が outage ごとに 1 回だけ台帳へ書くので、
同じ outage で 2 通目が来ることはない）。`result` / `progress` は**知らせない**（SPEC §3.5 の
数時間単位の流れは GUI の報告の仕事）。同じ `(kind, key)` は 1 回だけ送り、失敗したら次の tick で
再送する（最大 3 回。429 はここに数えない）。

通知条件の経緯は ADR-0037 を参照。現在の条件:

- `milestone_ready` は「動いているものが無く、人の手が要る」状態（ready/running/reviewing/blocked が
  0 件、done が 1 件以上）で鳴る。**全部が終端である必要はない** — Go 待ちの `draft` が残っていてもよい
  （むしろそここそが人の判断が要る瞬間）。`key` は `<途中目標 id>:<done の件数>` で、Go を出して
  また止まると done の件数が変わるので再び鳴る。
- `milestone_ready` / `approval_pending` / `question_blocked` / `bad_news` / `secretary_reply` のうち
  `milestone_ready` を除く 4 種は、**celeris の起動より前に作られた出来事は対象にしない**
  （backfill 禁止。GUI で既に見た昨日以前の履歴が起動直後に一斉送信されることはない）。
- 1 tick（`interval_secs`）に送るのは最大 1 通。`bad_news` が複数 pending なら 1 通に束ねる
  （台帳の行は個別に決着する）。

webhook の URL は**秘密**で、`[secrets]`（§3.36〜3.38 / ADR-0030）に id `discord-webhook`（既定。
`[notify] discord_webhook_secret` で変えられる）で登録する。**URL は応答にもログにも問題詳細にも出ない。**

#### 3.64 `GET /notify` → 200 `NotifyView`（読み取り）

```jsonc
{
  "configured": true,                 // その id の秘密が登録されていて、送れる状態か
  "secret_id": "discord-webhook",     // GUI はこの id で「API キー」画面への導線を出す
  "fingerprint": "3f9a1c02",          // 値の sha256 の先頭 8 桁（値は復元できない）。未登録なら省略
  "gui_base_url": "http://192.168.1.103:7700",  // 文面に付けるリンクの根。未設定なら省略
  "recent": [                          // 直近 10 件（新しい順）
    {
      "kind": "milestone_ready",
      "key": "01J…:2",                // 途中目標 id / 認可 id / タスク id / 報告 id / 案件 id
                                       // （`milestone_ready` は `<途中目標 id>:<done の件数>`）
      "project_id": "01K…",           // milestone_ready はその途中目標の
                                       // 案件、secretary_reply はその案件自身、他の種は省略（null 相当）
      "created_at": "2026-09-18T12:00:00Z",
      "sent_at": "2026-09-18T12:00:01Z",   // まだなら省略
      "attempts": 1,
      "ok": true,                      // 省略 = まだ決着していない（次の tick で再送）、false = 諦めた
      "error": "http status 404"       // 失敗の理由（**URL・ホスト名は入らない**）。無ければ省略
    }
  ]
}
```

- 通常の認証だけ（`token_file` があればトークン必須）。本文に URL は**絶対に含まれない**。
- `configured` が false のとき、GUI は「未設定」と出し、`secret_id` を添えて API キー画面へ導く。
- 送らずに畳んだ行（秘密が無い間に起きた出来事）は `ok: false`、`attempts: 0`、
  `error: "discord webhook is not configured"` で並ぶ（ADR-0037 D2:「秘密が無い間の出来事は通知しない」）。
- `project_id` は GUI がリンクを作るためだけの補助情報で、判定は celeris がその通知を作った時点で
  分かっている案件 id をそのまま台帳に書いたもの（応答時に途中目標から逆引きしない）。

#### 3.65 `POST /notify/test` → 200 `NotifyTestResult`（**管理系: `token_file` 未設定でも 401**）

要求本文は無し（`{}` でよい）。定型のテスト文を 1 通だけ送る。台帳（`notifications`）には残さない。

```jsonc
{ "ok": true, "detail": "the test message was delivered" }
```

- `ok: false` でも 200（送り先が 404 を返した等）。`detail` は**種別だけ**の短い文で、URL・ホスト名は入らない。
- 409 `notify_unavailable`: 秘密が登録されていない（`[secrets]` 自体が無い場合も含む）、または celeris に
  委譲できない構成。`detail` には id（既定 `discord-webhook`）だけを書く。
- 401 `unauthorized`: トークン無し（`token_file` を設定していない構成でも 401）。

### 3.66〜3.67 リリース（自己改善のデプロイ）（ADR-0040 D6）

`scripts/selfdeploy/release.sh` が作った**不変のリリース**（`~/.local/celeris/releases/<sha12>/`）を一覧し、
検証済みのものへ**人が**昇格する。設計は `agent-docs/adr/0040-self-improvement-deploy.md`、運用は
`docs/ops/selfdeploy.md`。読む先は `[selfdeploy] releases_dir`（既定 `releases`。設定ファイルのディレクトリ基準）。

| | |
|---|---|
| 何を読むか | `<releases_dir>/<sha12>/{manifest.json, gate.json, verify.json, changes.json, promoted.json}`、`promote.lock`、`<releases_dir>` の**親**の `current` / `previous` の symlink、`daemon_instances`（ADR-0040 D4）、`[selfdeploy] repo` の git（`on_main` のためだけ。ADR-0041 D3） |
| 何を書くか | `POST .../promote` のときだけ `<release>/promote.lock` と `<release>/promote.log`。**DB も設定も本番プロセスも、`[selfdeploy] repo` の git リポジトリも触らない** |
| 誰が昇格するか | **人だけ**（ADR-0040 D5）。この API か shell から。celeris の中に自動で呼ぶ経路は無い |

`.build` / `.cargo-target` のような `.` で始まる名前と、組み立て途中の `<sha12>.partial` は一覧に出ない。
`manifest.json` / `gate.json` が壊れていても一覧は落ちず、その 1 件が `gate_ok: false` と `problem` を持つ。

#### 3.66 `GET /releases` → 200 `Releases`（読み取り。**管理系ではない**）

```jsonc
{
  "current": "9ca90bd4f1c2",          // <releases_dir>/../current が指す sha12。無ければ null
  "previous": "448ae0c33b19",         // 同 previous。rollback 先
  "running": {                        // いまこの要求に答えているプロセス自身（GET /health と同じ値）
    "release": "9ca90bd4f1c2",        // --release / CELERIS_RELEASE / "dev"
    "role": "active",                 // active | standby | draining | verify
    "instance_id": "01K…"
  },
  "instances": [                      // daemon_instances（ADR-0040 D4）。started_at 昇順
    { "instance_id": "01K…", "release": "9ca90bd4f1c2", "pid": 1234, "role": "draining",
      "started_at": "2026-09-19T09:00:00Z", "heartbeat_at": "2026-09-19T10:00:00Z",
      "handoff_requested_at": "2026-09-19T09:59:00Z", "drained_at": null }
  ],
  "items": [                          // built_at の新しい順（読めなかったものは最後）
    {
      "sha12": "abcdef123456",
      "ref": "self/01M2…",            // release.sh に渡した ref。読めなければ null
      "built_at": "2026-09-19T08:00:00Z",
      "schema_version": 11,
      "gate_ok": true,                // gate.json の ok（cargo test / clippy / build / pnpm … が全部 exit 0）
      "verify": { "ok": true, "live_ok": true, "at": "2026-09-19T08:30:00Z" },  // null = 未検証
      "promoted_at": null,            // promoted.json（昇格に成功したときだけ）。null = 一度も昇格していない
      "on_main": false,               // git merge-base --is-ancestor <sha> main。null = 分からない
      "changes": {                    // changes.json（ADR-0041 D4）。null = 古いリリース
        "base": "9ca90bd4f1c2",       // ビルド時の current の sha12（null = current が無かった）
        "stale": false,               // base != いまの current（＝この差分はもう「いま」の話ではない）
        "commit_count": 3,
        "file_count": 12,
        "sensitive": ["scripts/selfdeploy/verify.sh"],   // 安全に関わる変更。空なら普通のリリース
        "commits": [{ "sha": "…40 桁…", "subject": "phase 50: …" }]   // 新しい順、最大 50 件
      },
      "is_current": false,
      "is_previous": false,
      "promoting": false,             // promote.lock の pid がまだ生きている
      "promote_failed": null,         // promote_failed.json（直近の昇格の試みが失敗したときだけ）。§3.67 の後注
      "problem": null                 // manifest/gate が読めなかったときだけ一行（普段は省略）
    }
  ]
}
```

- 通常の認証だけ（`token_file` があればトークン必須。`GET /notify` と同じ扱い）。
- `[selfdeploy]` が無い構成、`releases_dir` がまだ無い（初回）ときも **200**（`items: []`）。
- GUI は `verify` で状態を出し分ける: `null` =「未検証」、`ok && live_ok` =「検証済み（ライブ引き継ぎ）」、
  `ok && !live_ok` =「検証済み（停止 → 起動）」、`!ok` =「検証に落ちた」。
- 引き継ぎの進行は `instances` で見える（旧が `draining`、新が `active`。ADR-0040 D4）。
  昇格の最中は `GET /releases` を数秒ごとに読み直せばよい（SSE には載らない）。

`promoted_at`・`on_main`・`changes` の仕様（経緯は ADR-0041）:

- **`promoted_at`**: `promote.sh` が昇格に成功したときに書く `<release>/promoted.json` の
  `{promoted_at, mode, from}` の `promoted_at`。まだ昇格していないリリースは `null`。
- **`on_main`**: その sha が `[selfdeploy] repo`（既定 `~/workspace/agent-platform`。`~` は celeris の
  `$HOME` で展開）の `main` の**祖先**か。`git -C <repo> merge-base --is-ancestor <sha> main` の
  終了コードそのままで、`0` → `true`、`1` → `false`、それ以外（リポジトリが無い・`main` が無い・
  git が無い・時間切れ・その sha を知らない）は **`null`**。**celeris はこのリポジトリを読むだけ**で、
  checkout も fetch も merge もしない（反映は人がやる。ADR-0041 D3）。
  GUI は `current` の行が `on_main: false` のときだけ「本番は main に未反映: `git merge --ff-only <sha12>`」と出す。
- **`changes`**: `release.sh` が**ビルド時に**書いた `<release>/changes.json` の要約。
  `sensitive` は `scripts/selfdeploy/lib.sh` の `SD_SENSITIVE_PATTERNS`（`scripts/selfdeploy/`、`deploy/`、
  `crates/celeris/src/instance.rs`、`crates/celeris/src/releases.rs`、`crates/task-api/src/releases.rs`、
  `crates/task-core/migrations/`、`CLAUDE.md`、`gui/CLAUDE.md`、`.claude/`、`config/`、`agent-docs/adr/0040-`、
  `agent-docs/adr/0041-`）に**前方一致**したファイル。**判定は `release.sh` の側で済んでいて、API も GUI も
  その結果を運ぶだけ**（パターンを 2 か所に置かない）。`changes.json` が無い古いリリースでは `null`。

#### 3.67 `POST /releases/{sha12}/promote` → 202 `ReleasePromoteAccepted`（**管理系: `token_file` 未設定でも 401**）

要求本文は無し（`{}` でよい）。`promote.sh <sha12>` を **detached**（`setsid`、stdin は `/dev/null`、
stdout/err は `<release>/promote.log`）で起こし、`promote.lock` に pid を書いてすぐ返す。
**昇格の完了は待たない。**

**どちらの `promote.sh` を起こすか**（ADR-0041 D4）:
**いま動いている版**のもの（`<releases_dir>/<current>/scripts/promote.sh`）を使う。昇格は「動いている
本番を止めて／引き継いで新しい版に替える」作業で、その手順を知っているべきなのはいまの本番だから。
実装者が `scripts/selfdeploy/` を壊したリリースを作っても、その壊れた昇格スクリプトは走らない
（新しい昇格スクリプトは、それ自身が一度昇格されてから次の昇格で使われる）。`current` に `scripts/` が
無い（古いリリース、または初回）ときだけ昇格先のものを使う。どちらを使ったかは `script_from`。

```jsonc
{ "sha12": "abcdef123456",
  "log": "/home/…/celeris/releases/abcdef123456/promote.log",   // 中身は API では出さない
  "started_at": "2026-09-19T10:00:00Z",
  "script_from": "current" }   // "current" | "target"
```

- 404 `release_not_found`: その sha12 のディレクトリが無い（sha12 の形＝16 進 7〜40 桁でないときも同じ）。
- 409 `release_not_promotable`: `verify.json` が無い／`ok` でない、既に `current`、既に昇格中
  （`promote.lock` の pid が生きている）、`current` にも昇格先にも `scripts/promote.sh` が無い
  （どちらも 古いリリース）、`[selfdeploy]` が無い。`detail` に理由の一行。
- 401 `unauthorized`: トークン無し（`token_file` を設定していない構成でも 401）。
- **この要求に答えた celeris 自身が、その昇格で `draining` になって最後には終わる**（ADR-0040 D4 の
  ライブ引き継ぎ）。202 を返した後に同じプロセスの API が閉じるのは正常。GUI は `GET /releases` を
  読み直して `running.release` が新しい sha12 になるのを待つ（同じポートを新旧が `SO_REUSEPORT` で
  共有するので接続は切れない）。
- `promote.sh` は `verify.json.ok` を自分でも確かめる（`--force` は無い）。この API の 409 はその前段の
  早い拒否で、二重の防壁になっている。
- **`promote.sh` が detached で始まった後に失敗しても、この 202 は変わらない**（celeris は「起こせた」
  ことしか知らない）。経緯は ADR-0041 を参照。`promote.sh` は失敗時に
  `<release>/promote_failed.json` に `{failed_at, error}`（`error` はログの末尾 20 行）を書くようになった。
  `GET /releases` の `items[].promote_failed` はこれを写す（§3.66）。次の昇格の試みが始まる
  （この API が呼ばれる）と、そのリリースの `promote_failed.json` は消える — 古い失敗が残り続けない。
  GUI は `promoting` が偽で `promote_failed` が非 `null` のときだけ赤いバナーを出す。

### 3.68〜3.71 案件のリポジトリ（ADR-0043 D1。**変更系は管理系**）

案件は**リポジトリを複数持つ**（論文の `benchfs-paper` とコードの `benchfs`、git ではないデータの置き場）。
`is_primary` の 1 件が「主なリポジトリ」で、**`Project.workspace` はその `location` の写し**である
（GUI の後方互換。`PATCH /projects {workspace}` は primary を書き換える）。

#### 3.68 `GET /projects/{id}/repos` → 200 `RepoList`

- 読み取り（トークン不要）。並びは **primary が先頭**、あとは作った順
- 知らない案件は 404 `project_not_found`

#### 3.69 `POST /projects/{id}/repos` → 201 `ProjectRepo`

```json
{
  "name": "benchfs",
  "kind": "git",
  "location": {"kind": "local", "path": "~/workspace/rust/benchfs"},
  "default_branch": "main",
  "run": "auto",
  "is_primary": true
}
```

- `location` だけが必須。`name` を省略するとパスの末尾から slug を作る（`benchfs`）。
  `kind` を省略すると `<path>/.git` があれば `git`、無ければ `dir`（リモートは `git`）
- `location.kind = "local"` の `path` の `~` は celeris の `$HOME` で展開して保存する（ADR-0039 D5）。
  `remote` の `cluster` が `[[clusters]]` に無ければ 422 `validation`
- `name` は案件内で一意の slug（`[a-z0-9._-]`、1〜64 文字、先頭が `.` / `-` でないこと）。
  重複・不正な名前は 422。`<workspace_root>/<task_id>/repos/<name>/` というディレクトリ名になるため
- **案件の最初の 1 件は自動的に `is_primary = true`** になる（案件に主なリポジトリが無い状態を作らない）。
  `is_primary: true` を立てると、同じ案件の他の行の `is_primary` は落ちる
- `sync` は `location.kind = "remote"` のときだけ（`worktree`〈既定〉 / `rsync` / `none`）。
  **`sync: "none"` は 422**（ADR-0043 D7 のリモート (b) は未実装）
- `run` は `auto`（既定。`workspace.toml` の `[run] mode` に従う）/ `host` / `container`。
  `container` または `auto` で `[run] mode = "container"` なら container 実行を選ぶ。

#### 3.70 `PATCH /repos/{id}` → 200 `ProjectRepo`

- 書いたものだけ変える（1 つも書かなければ 422）。`default_branch` / `sync` は `null` を明示すると消える
- `is_primary: true` でこの行が primary になる（`false` は何もしない。primary を空にはできない）
- `kind` を `dir` にすると `default_branch` は落ちる。`location` を `local` にすると `sync` は落ちる
- 知らない id は 404 `repo_not_found`

#### 3.71 `DELETE /repos/{id}` → 204

- **未終端（`done` / `failed` / `cancelled` 以外）のタスクがそのリポジトリを使っていたら 409 `repo_in_use`**
- primary を消したら、残りのうち一番古いものが primary になる

### 3.72〜3.73 タスクの作業ツリーの閲覧（ADR-0043 D6。読み取り）

タスクの作業場所は `<workspace_root>/<task_id>/repos/<name>/`（git は worktree、`dir` は実体への
シンボリックリンク）。`<workspace_root>/<task_id>/worktree.json` がその目印で、この 2 つの
エンドポイントはそれだけを見る（git は起こさない）。

#### 3.72 `GET /tasks/{id}/tree?repo=&path=` → 200 `TreeView`

- `repo` を省略すると**先頭のリポジトリ**（= ワーカーのカレントディレクトリ）。そのタスクに無い名前は 404
- `path` はそのリポジトリの作業ツリーからの**相対パス**（省略・空は根）。
  `..` を含むもの・絶対パスは **403 `path_forbidden`**。解決した実体が作業ツリーの外に出るもの
  （シンボリックリンクでの脱出）も 403
- 作業ツリーを持たないタスク（`worktree.json` が無い）は 404 `file_not_found`
- `entries` はディレクトリが先、あとは名前順（決定的）。`kind` は `dir` / `file` / `other`

```json
{
  "repo": "benchfs",
  "path": "src",
  "repos": [
    {"name": "benchfs", "kind": "git", "dir": "/home/u/.local/celeris/workspaces/01J.../repos/benchfs",
     "branch": "celeris/01J...", "base": "9602b596826c..."},
    {"name": "data", "kind": "dir", "dir": "/home/u/.local/celeris/workspaces/01J.../repos/data"}
  ],
  "entries": [
    {"name": "bin", "path": "src/bin", "kind": "dir"},
    {"name": "lib.rs", "path": "src/lib.rs", "kind": "file", "size": 1234}
  ]
}
```

#### 3.73 `GET /tasks/{id}/tree/file?repo=&path=` → 200 `TreeFileView`

- `path` は必須（省略は 400 `bad_request`）。境界の規則は 3.72 と同じ
- ディレクトリを指したら 403 `path_forbidden`
- **テキストで 512 KiB 以下のときだけ `text` を返す**。超えたら `too_large: true`（`text` なし）、
  バイナリ（NUL を含む・UTF-8 でない）は `binary: true`（`text` なし）。どちらも `size` は返す

---

### 3.74〜3.78 タスク管理: 編集・コメント・再開・タイムライン（ADR-0044 B1）

`POST /tasks` と `PATCH /tasks/{id}` は次の項目も受け付ける（経緯は [ADR-0046](../../../agent-docs/adr/0046-organization-as-agent-profiles.md)）。

| 項目 | 型 | 意味 |
|---|---|---|
| `skills` | `string[]` | そのタスクに**必要な能力タグ**（小文字 `[a-z0-9._-]`、最大 12 個）。`assignee` が無いタスクの担当は、これとノードの実効 `skills` の重なりで決まる（ADR-0046 D5） |
| `mode` | `"prototype"` / `"production"` / `"research"` | 進め方。既定 `production`。前置きの規則とレビューの厳しさが変わる（ADR-0046 D4） |
| `harness` | `string`（`PATCH` は `null` で外せる） | 実行契約の id。**`Task.genre` 列がそのまま harness id**（応答の `Task` では従来どおり `genre` として出る） |

- `skills` の綴り違反・件数超過は 422 `validation`。`mode` / `harness` の知らない綴りは本文の解析で落ちて
  400 `bad_request`（`harness` は「設定に無い id」なら 422 `validation`）。
- `assignee` と `harness` の組み合わせが合わない（その担当が `harnesses.allowed` にその harness を持たない）
  ときは 422 `validation`（作成時も編集時も）。**profile を 1 つも書いていないノードは従来どおり通る**。
- `EditResult.fields` には `"skills"` / `"mode"` / `"harness"` が入りうる。

人がタスクに手を入れるための 5 本。**`PATCH /tasks/{id}` と `POST /tasks/{id}/comments`、
`POST /tasks/{id}/reopen` は管理系**（`token_file` 未設定でも 401）。読み取り 2 本は通常の認証だけ。

#### 3.74 `PATCH /tasks/{id}` → 200 `{task, fields}`（**管理系**）

要求本文は `task_ops::edit::TaskEdit`。**書いた項目だけ**が変わる。`null` を書ける項目
（`assignee` / `role` / `adapter` / `milestone_id` / `harness`）は `null` で消す、省略で据え置き。
`workspace` は `WorkspaceSpec`、`project_id` は案件を持たない未実行の root task への案件付与、
`pause_after` は次の計画採用時に解決する停止点の指定。これらも省略時は据え置く。

```json
{"title":"…","objective":"…","acceptance":[{"type":"human","text":"…"}],
 "priority":"P1","labels":["infra"],"category":"ops","repos":["benchfs","benchfs-paper"],
 "assignee":"infra-section","role":"implementer","tier":"frontier","adapter":null,
 "milestone_id":"01J…","depends_on":["01J…"],
 "max_turns":30,"max_wall_secs":1800,"max_retries":2,
 "expected_status":"ready"}
```

- 応答の `fields` は**実際に変わった項目の名前**。何も変わらなければ空配列で、イベントも積まない。
- 変わったときは `Event::Edited{fields, by: "human"}` を**同じトランザクション**で積む
  （`replay` はこのイベントを無視する。状態機械は通らない）。
- **終端（`done` / `failed` / `cancelled`）は原則 409 `invalid_transition`**。
  例外として `failed` の `workspace` だけの変更は受け付ける。`workspace` は
  `running` / `reviewing` / `done` / `cancelled` では変更できない。
  やり直すなら `POST /tasks/{id}/retry`、同じ worktree で続けるなら `POST /tasks/{id}/reopen`。
- **`running` / `reviewing` は受け付けるが、走っている run は止めない**（次の run から効く）。
  止めたければ `POST /tasks/{id}/comments`（D2 の割り込み）か `POST /tasks/{id}/cancel`。
- `expected_status` が現在と違えば 409 `conflict`（`expected` / `actual` 付き）。
- 422 `validation`: 1 つも項目を書いていない、空白だけの `title` / `objective`、空の `acceptance`、
  ラベルの規則違反（`[a-z0-9-]` でない・9 個以上）、知らない `assignee`、案件違い・案件なしの
  `milestone_id`、存在しない・`failed`/`cancelled` の `depends_on`、自分自身への依存、
  **依存の循環**（`depends_on would create a cycle: …`。作成時は新しい id が誰の `depends_on` にも
  入っていないので起きないが、編集では起こせる。循環した 2 件は `ready_tasks` が永久に返さない）、
  **`genre` の `roles` に無い `role`**（作成時と同じ規則。`genre` はこの API では変えられない）。
- **`tier` / `adapter` / 予算は再解決しない**。ADR-0033 D2 の解決順（タスク > 役割 > 担当 > 分野）は
  **作成時に 1 回**効いて `worker_hint` と `budget` に焼き付く。`assignee` や `role` を変えても
  それらは動かないので、変えたければ同じ本文に `tier` / `adapter` / `max_turns` …も書くこと
  （GUI の編集フォームは tier を担当の隣に並べている）。
- **`status` / `attempts` / リースは原則触らない**。ただし経路が無く `blocked` のタスクに
  `workspace` または `assignee` を設定し、実行可能になったときは `ready` に戻す。
  ストアは状態を、編集を書き戻すトランザクションの
  中で読み直した値で書く（編集フォームを開いている間にディスパッチャが run を始めていても、
  その run を壊さない）。応答の `task` はその読み直した状態を持つ。
- 知らないキーは 400 `bad_request`（`deny_unknown_fields`）。
- **`repos`（ADR-0043 D2）**: この案件のリポジトリを**名前で**差し替える
  （`project_repos.name`。§3.68）。解決の規則は `POST /tasks` と同じで、**そのタスクの案件の中**から引く
  （継承〈親 → primary〉は作成時だけの規則なので、`[]` を書けば「リポジトリを使わない」になる）。
  知らない名前・リモートと他のリポジトリの混在・案件に属さないタスクの空でない `repos` は 422 `validation`。
  **走っている run には効かない**（次の run の worktree から。`PATCH` は run を止めない）。

#### 3.75 `GET /tasks/{id}/comments` → 200 `CommentList`

`{"items":[{"id":"01J…","task_id":"01J…","author_kind":"human","author":null,
  "body":"…","run_id":null,"created_at":"2026-09-19T…Z"}]}` — **古い順**（`created_at`、同時刻は `id`）。
`author_kind` は `human`（人）/ `node`（組織の「人」。`author` にノード id）/ `system`（celeris）。
知らないタスクは 404 `task_not_found`。

#### 3.76 `POST /tasks/{id}/comments` → 201 `CommentResult`（**管理系**）

本文は `{"body":"…"}`（空白だけは 422、20,000 文字超は 422）。応答:

```json
{"comment":{…TaskComment…},"effect":"interrupted",
 "transition":{"id":"…","from":"running","to":"ready","reason":"comment","cascaded":[]},
 "can_reopen":false}
```

**人のコメントの効き方（ADR-0044 D2 の表。決定的）**:

| タスクの状態 | `effect` | 何が起きる |
|---|---|---|
| `running` / `reviewing` | `interrupted` | 新しいトリガ `Interrupt` で `ready` に戻す（**attempts 据え置き**、`Transitioned{reason:"comment"}`）。`WorkerFinished{outcome:"interrupted: comment"}` を同じトランザクションで積む。走っていた run はディスパッチャが次の tick で止める（`cancel` と同じ `abort_stale_runs` の経路）。**報告（ADR-0034）は作らない**。次の run の前置きの先頭に「**人からの割り込み**: …」として載る |
| `blocked` | `answered` | `POST /tasks/{id}/answer` と同じ（`Event::Answered`、`Trigger::Answer`、未決の `approvals` も決まる） |
| `ready` / `draft` | `stored` | 記録するだけ（次の run の前置きの「コメント」節に載る。`draft` は Go 待ちのまま） |
| `done` / `failed` / `cancelled` | `terminal` | 記録するだけ。`can_reopen` が真なら GUI は「再開」を出せる（`cancelled` は worktree が無いので偽） |

- ワーカーのコメントはこの API では作れない（ワーカー・プロトコルの `{"type":"comment"}` 行が
  `author_kind = node` で入る。`docs/protocol/worker-protocol.md` §4.7）。ワーカーのコメントは
  **人を起こさない**（状態も変えない）。対話 run とレビュー run にはコメントを書かせない。
- ADR-0044 D8: `interrupted` は `bad_news` にも `error_cooldown` にも数えない
  （`RunOutcomeKind::Interrupted`。§5.2）。
- `WorkerFinished{outcome:"interrupted: comment"}` は、**いま走っているワーカー run**
  （= リースの `worker_run_id`）にだけ付く。`reviewing` の割り込みでは付かない（直近のワーカー run は
  既に `done: …` で終わっており、そこに重ねるとその run の記録を壊すため）。遷移
  （`Transitioned{reason:"comment"}`）はどちらでも残る。
- `blocked` のタスクへのコメントは `POST /tasks/{id}/answer` と同じ状態変化を起こす。
  コメント・回答・中止・retry・タスク作成はいずれも管理系で、トークン未設定でも 401 を返す。

#### 3.77 `POST /tasks/{id}/reopen` → 200 `TransitionResult`（**管理系**）

本文は `{"expected_status":"done"}`（任意）。新しいトリガ `Reopen` で **`done` / `failed` → `ready`**、
**attempts は 0 に戻す**（`reason: "reopen"`）。worktree とブランチはそのままなので、続きから直せる。

- `cancelled` は**再開しない**（worktree もブランチも消してある。ADR-0044 D6）→ 409 `invalid_transition`。
  やり直すなら `POST /tasks/{id}/retry`（複製）。
- 非終端も 409 `invalid_transition`。`expected_status` 不一致は 409 `conflict`。

#### 3.78 `GET /tasks/{id}/timeline` → 200 `Timeline`

そのタスクに起きたことを **`at` の昇順で 1 本**にまとめる（同時刻は元の順を保つ安定ソート）。

```json
{"task_id":"01J…","items":[
  {"kind":"event","at":"…","seq":0,"event":{…Event…}},
  {"kind":"comment","at":"…","comment":{…TaskComment…}},
  {"kind":"approval","at":"…","approval":{…Approval…}},
  {"kind":"report","at":"…","report":{…Report…}},
  {"kind":"delegation","at":"…","run_id":"…","tasks":[{…TaskRef…}]},
  {"kind":"release","at":"…","sha12":"abcdef012345","commits":["…"]},
  {"kind":"integration","at":"…","action":"pr","detail":"gui: merged — PR #42 https://…"}]}
```

- `event`: `events` の 1 行（遷移・run・質問・回答・**編集**・**割り込み**）。ただし `Delegated` だけは
  `delegation` に畳む（同じことを 2 回出さない）。
- `approval`: そのタスクの認可（`at` は決まっていれば `decided_at`、まだなら `created_at`）。
- `report`: そのタスクが元になった報告（ADR-0034）。
- `delegation`: `Event::Delegated` の run と、作られた子の `TaskRef`。
- `release`: **そのタスクのブランチのコミット**（`worktree.json` の `branch` と `base` から
  `rev-list <base>..<branch>` で引く）が `~/.local/celeris/releases/*/changes.json` の `commits` に含まれる
  リリース。`worktree.json` が無い・**`base` が空**・git が動かない・`[selfdeploy]` が無いときは
  **何も出さない**（タイムラインは落ちない）。`base` が無いまま `rev-list <branch>` を引くと
  `main` の歴史まで「このタスクの変更」として並ぶので、そこは出さない側に倒す。
- `items[].event` は**新しい方から 2,000 件**まで（それより古いイベントは `GET /tasks/{id}/events`
  でページングして見る）。クエリパラメータは受け付けない。
- 並びは `at` を**時刻として**比べる（RFC 3339 の小数秒があるので、文字列比較では順が狂う）。
- `integration`（ADR-0043 D5 / A2 の取り込み: merge / PR / discard）は `task_integrations`（§3.79〜3.83）から
  出る。`action` は `merge` / `pr` / `discard`、`detail` は
  `<リポジトリ>: <行方>` + PR の番号と URL + 理由の一行。`at` は**人が押した時刻**（記録の `created_at`。
  PR の同期では動かない）。GUI は知らない `kind` を無視できるようにしておくこと。
- `doc`（ADR-0044 D7 の逆リンク）は **front matter の `tasks:` にこのタスクを持つページ**
  （`{"kind":"doc","at":"…","project_id":"01J…","path":"docs/research/fs.md","title":"調べたこと"}`）。
  案件の文書の根を `git grep -l "<タスク id>"` で絞ってから front matter を確かめるので、本文に id が
  出ただけのページは載らない。`at` はそのページの**最後のコミットの時刻**（読めなければ空）。
  文書の根が無い案件・git が動かないときは**何も出さない**（タイムラインは落ちない）。
- `knowledge`（ADR-0047 D4/D5）は、そのタスクの終端から起きた知識整理 run（`knowledge_runs`）を
  1 件（`state` は `scheduled` / `applied` / `failed`。`applied` のときだけ `ingested` / `inbox` / `discarded`）。
  `via` は ADR-0052 D2 による: `"langmem"` なら従来どおり Qwen で抽出、`"fallback:<adapter>"` なら
  Qwen に届かず tier `cheap` の汎用ハーネスで抽出した（GUI は後者に「cheap のハーネスで抽出」を出す）。
  run が 1 度も始まっていなければ `via` は付かない。
- 知らないタスクは 404 `task_not_found`。

---

### 3.79〜3.83 変更の取り込み（ADR-0043 D5。**変更系は管理系**）

タスクは `celeris/<task_id>` ブランチに変更を積む（ADR-0043 D2）。それを**人が**見て、`main` に
取り込むか、PR にするか、捨てる。**押せるのは人だけ**（SPEC §3.6）。ワーカープロトコル
（`docs/protocol/worker-protocol.md`）にはこの経路が無いので、組織の「人」が `main` を動かす道は無い。

見る先は §3.72 と同じ目印（`<workspace_root>/<task_id>/worktree.json`）で、`kind = "git"` の
リポジトリだけが対象（`dir` は出ない）。目印が無いタスクは 404 `file_not_found`。

celeris はここで **`git` と `gh` だけ**を、待ち時間の上限付きで起こす（読み取り 30 秒、書き込み 300 秒、
`gh pr view` 10 秒、`gh pr create` / `gh pr merge` 120 秒）。LLM もワーカーも起こさない。

#### 3.79 `GET /tasks/{id}/changes` → 200 `ChangesView`

- リポジトリごとに `base`（`merge-base(default_branch, head)`）/ `head` / `ahead`（`base..head` の
  コミット数）/ `files` / `stat` / `dirty` / `missing` / `origin` / `integration` を返す
- `files` は **base から「いまの作業ツリー」まで**（コミット済み + 未コミット + 追跡外）。
  `dirty` は `git status --porcelain` が空でないこと、`ahead` はコミットの数なので、
  **コミットが 1 つも無いタスクは `ahead = 0`**（GUI は「変更なし」）
- worktree が消えていてもブランチが残っていれば、元のリポジトリで `base..<branch>` を見る
  （そのとき `dirty` は常に `false`）。**どちらも無ければ `missing: true`、`ahead: 0`、`files: []`**
- `default_branch` は `project_repos.default_branch`、無ければ検出（`origin/HEAD` → `main` → `master`）
- **PR の同期はここでだけ行う**（ADR-0043 D5「常時同期はしない」）。`integration.state` が `open` で
  `pr_number` があれば `gh pr view <n> --json state,mergedAt,mergeable,reviewDecision,url` を 10 秒で呼び、
  `OPEN` / `MERGED` / `CLOSED` を `open` / `merged` / `closed` に写す。`merged` になったら**その場で
  worktree とローカルのブランチを片付ける**。`gh` が失敗したときは記録を**触らない**（`failed` にしない）
- `gh`（真偽）は `gh auth status` の結果（**プロセス内で 60 秒だけ覚える**）。`merge_method` は `[github] merge_method`

```json
{
  "task_id": "01J...",
  "gh": true,
  "merge_method": "merge",
  "repos": [
    {"repo": "benchfs", "branch": "celeris/01J...", "default_branch": "main",
     "base": "9602b596826c...", "head": "1f2e3d4c5b6a...", "ahead": 3,
     "files": [{"path": "src/lib.rs", "status": "M", "additions": 12, "deletions": 3},
               {"path": "notes.md", "status": "?", "additions": 8, "deletions": 0}],
     "stat": {"files": 2, "additions": 20, "deletions": 3},
     "dirty": true, "missing": false, "origin": true,
     "integration": {"id": "01J...", "task_id": "01J...", "repo": "benchfs", "method": "pr",
                     "state": "open", "pr_number": 42, "pr_url": "https://github.com/o/r/pull/42",
                     "created_at": "2026-09-19T10:00:00Z", "updated_at": "2026-09-19T10:00:00Z"}}
  ]
}
```

#### 3.80 `GET /tasks/{id}/changes/{repo}/diff?path=` → 200 `ChangeDiffView`

- `path` は必須（省略は 400 `bad_request`）。`..` と絶対パスは 403 `path_forbidden`
- そのタスクに無い `repo` は 404 `file_not_found`
- **200 KiB で切る**（切ったら `truncated: true`）。差分が無ければ `diff` は空文字列
- 追跡外のファイルは `git diff --no-index -- /dev/null <path>` の出力（「全部追加」の形）

#### 3.81 `POST /tasks/{id}/changes/{repo}/integrate` → 200 `IntegrateResult`（**管理系**）

要求本文 `IntegrateBody`: `{"method": "merge" | "pr" | "discard", "note"?: string, "confirm"?: bool}`。

- **`merge`**: 一時 worktree（`<workspace_root>/.integrate/<ulid>`）でブランチを `default_branch` に
  `rebase` し、成功したら `default_branch` を進める。`origin` へ push は**しない**
  - 人のチェックアウトが `default_branch` を出していて `git status --porcelain` が空でなければ
    **409 `default_branch_busy`**（`detail` は `"<default_branch> が編集中"`）。**記録も残さない**
  - 出していて綺麗なら `git -C <local.path> merge --ff-only <sha>`（人の作業ツリーも進む）
  - 別のブランチ（または detached）なら `git -C <local.path> update-ref refs/heads/<default_branch> <new> <old>`
    （**人の作業ツリーには触らない**）
  - 成功したら worktree を消してブランチを `git branch -D` し、`state = "done"`（`merged_at` も入る）
  - `rebase` が衝突したら `rebase --abort` して worktree もブランチも残し、`state = "conflict"` を記録して
    **「衝突の解消: <題名>」タスクを自動で作る**（応答の `child_task_id`。下の注記）
- **`pr`**: `origin` リモートが無い、または `gh` が使えない（PATH に無い・認証されていない）ときは
  **409 `pr_unavailable`**。そうでなければ `git push -u origin <branch>` →
  `gh pr create --base <default_branch> --head <branch> --title <タスクの題名> --body <生成>` →
  `state = "open"`（`pr_number` / `pr_url`）。本文は**決定的**（目的 / 受け入れ条件 / 最新の報告の要約 /
  `Celeris task <id>` と `[notify] gui_base_url` があればそのリンク）。LLM は使わない
- **`discard`**: `{"confirm": true}` が要る（無ければ 422 `validation`、`errors[0].field = "confirm"`）。
  worktree を消してブランチを `git branch -D` し、`state = "done"`
- `git` / `gh` が失敗したときは **200** を返し、記録が `state = "failed"` と `detail`（理由の一行）を持つ
  （GUI はそれをそのまま出す）。409 になるのは上の 2 つだけ
- `note` は記録の `detail` の先頭に入る（PR の本文には入れない）

**衝突の解消タスク**（ADR-0043 D5）: 親 = 元のタスク、担当（`assignee` / `role` / `genre` / `tier` / 予算）は
親と同じ、`status = "ready"`、前置きに衝突したファイルの一覧。受け入れ条件は `Check::Command` 3 本
（作業ツリーが clean / rebase が進行中でない / `default_branch` が `HEAD` の祖先）。
**親のブランチの上で**働かせるため、`workspace` は**親の worktree のパス + `mode = "shared"`** で
`repos` は空（ADR-0043 D5）。終わったら人がもう一度 `merge` を押す。

#### 3.82 `POST /tasks/{id}/changes/{repo}/pr/merge` → 200 `IntegrateResult`（**管理系**）

- 本文は空の JSON（`{}`）でよい
- そのリポジトリの最新の記録が `method = "pr"` かつ `state = "open"` でなければ **409 `pr_unavailable`**
- `gh pr merge <n> --<[github] merge_method> --delete-branch` → そのまま `gh pr view` で同期する。
  `merged` になったら worktree とローカルのブランチを片付ける
- `gh` が失敗したら 200 で `state = "failed"` と `detail`

#### 3.83 `GET /projects/{id}/integrations` → 200 `ProjectIntegrations`

- その案件のタスクの取り込みを、**タスク × リポジトリごとに最新の 1 件**だけ、新しい順に最大 200 件
- `state = "open"` のものは `gh pr view` で同期する（**1 回の呼び出しで 20 件まで**）
- 無い案件は 404 `project_not_found`

```json
{"items": [{"integration": {"id": "01J...", "task_id": "01J...", "repo": "benchfs", "method": "pr",
                            "state": "open", "pr_number": 42, "pr_url": "https://github.com/o/r/pull/42",
                            "created_at": "2026-09-19T10:00:00Z", "updated_at": "2026-09-19T10:05:00Z"},
            "task_title": "ワークスペース A2", "task_status": "done"}]}
```

---

### 3.84〜3.91 案件の中止・一時停止・アーカイブと旧途中目標 API

経緯は [ADR-0044](../../../agent-docs/adr/0044-task-management.md) と
[ADR-0079](../../../agent-docs/adr/0079-recursive-task-decomposition.md) を参照。
全 route が管理系で、トークンが無ければ 401。本文は空または `{}`、クエリは受け付けない。

#### 3.84〜3.88 案件の操作

| # | route | 成功時の応答 | 要点 |
|---|---|---|---|
| 3.84 | `POST /projects/{id}/cancel` | 200 `ProjectLifecycle` | 非終端のタスクを同期的に中止し、案件を `cancelled` にする |
| 3.85 | `POST /projects/{id}/pause` | 200 `ProjectLifecycle` | 案件を `paused` にし、新たな dispatch を止める |
| 3.86 | `POST /projects/{id}/resume` | 200 `ProjectLifecycle` | `paused_from` の状態に戻す |
| 3.87 | `POST /projects/{id}/archive` | 200 `ProjectLifecycle` | 終端の案件を一覧から隠す。再送は冪等 |
| 3.88 | `POST /projects/{id}/unarchive` | 200 `ProjectLifecycle` | アーカイブを解除する。再送は冪等 |

`ProjectLifecycle` は `project`、`cancelled_tasks[]`、`cancelled_milestones[]` を返す。
後二者は中止時だけ埋まる。未知の案件は 404 `project_not_found`、状態に合わない操作は
409 `invalid_transition`。`pause` は実行中の run を中断しない。

#### 3.89〜3.91 途中目標の旧 route

| # | route | 応答 |
|---|---|---|
| 3.89 | `POST /milestones/{id}/cancel` | 410 `Problem` |
| 3.90 | `POST /milestones/{id}/pause` | 410 `Problem` |
| 3.91 | `POST /milestones/{id}/resume` | 410 `Problem` |

3 本とも `removed_by_adr_0079` を返す。本文と id は読まない。代わりに task の subtree には
`POST /tasks/{id}/cancel`・`POST /tasks/{id}/pause`・`POST /tasks/{id}/resume`、
案件には上の 3.84〜3.86 を使う（§3.125.8〜9）。`MilestoneLifecycle` は互換用の型だけ残る。

---

### 3.92〜3.97 文書（ADR-0044 D7。**変更系は管理系**）

**正本は git のファイル**（DB には何も持たない）。案件の文書の根は

1. その案件の **primary リポジトリ**（`project_repos.is_primary`。ADR-0043 D1）が `kind = "git"` で
   **手元（`location.kind = "local"`）**にあれば、その `.config/celeris/workspace.toml` の
   `[outputs] docs`（既定 `docs`）
2. primary が `dir`、または案件にリポジトリが無ければ **`POST /projects/{id}/docs/init`（§3.94）で
   `~/workspace/<案件 slug>/` に作る**（`git init -b main` + `docs/README.md` の最初のコミット）。
   作ったら primary の `git` リポジトリとして登録する

見えるのは **`<docs>/**/*.md` の default_branch の中身**（人の作業ツリーの未コミットの変更は出ない）。
`path` は**リポジトリ相対**（`docs/research/xxx.md`）で、文書の根で始まっていなければ根の下だと解釈する
（`?path=research/xxx.md` も同じページ）。`..`・絶対パスは **403 `path_forbidden`**、`.md` で終わらない
パスは **422 `validation`**。文書の根がまだ無い案件の**読み取り**は **409 `docs_unavailable`**
（読み取りは何も作らない。作るのは管理系の §3.94 / §3.95 / §3.97 だけ）。

celeris はここで **`git` だけ**を、待ち時間の上限付きで起こす（読み取り 30 秒、書き込み 300 秒。§3.79 と同じ
枠組み）。LLM もワーカーも起こさない。人の編集は**一時 worktree でコミットして default_branch を
fast-forward** する（ADR-0043 D5 の `merge` と同じやり方）。author / committer は
`Celeris (human) <celeris@local>`。

組織の「人」（ワーカー）にはこの経路を**出していない**。ワーカーは自分の worktree のブランチに
`docs/` を書き、人が「変更」タブ（§3.81）で取り込む。

#### 3.92 `GET /projects/{id}/docs` → 200 `DocsTree`

- `root`（文書の根）・`repo`（リポジトリの名前）・`default_branch` と、ページの一覧（パスの昇順、最大 500。
  超えたら `truncated: true`）
- 1 件ごとに `path` / `title`（front matter の `title` → 1 行目の `# ` → ファイル名）/ `updated_at` /
  `last_commit`（`sha` / `at` / `author` / `subject`）
- `?q=` があれば **`git grep -i -l -F`**（大文字小文字を区別しない固定文字列。正規表現ではない）で絞る
- 無い案件は 404 `project_not_found`、文書の根が無ければ 409 `docs_unavailable`

```json
{"project_id": "01J...", "repo": "benchfs", "root": "docs", "default_branch": "main", "truncated": false,
 "items": [{"path": "docs/research/fs.md", "title": "調べたこと", "updated_at": "2026-09-19T11:00:00Z",
            "last_commit": {"sha": "…", "at": "2026-09-19T11:00:00Z", "author": "Celeris (human)",
                            "subject": "docs: docs/research/fs.md"}}]}
```

#### 3.93 `GET /projects/{id}/docs/page?path=` → 200 `DocPage`

- `raw`（front matter を含む Markdown のもと）、`html`（**サーバで描画**。`pulldown-cmark`、表・脚注・
  打ち消し線あり、**生 HTML は捨てる**）、`title` / `tags[]` / `tasks[]`（front matter）、
  `history`（直近 20 件、新しい順）、`etag`（**blob の sha**）
- 本文中の `celeris:task/<ULID>` は `/tasks/<ULID>` に、`[[相対パス.md]]` は文書タブへのリンクに開く
  （根の外に出るものはリンクにしない）
- 512 KiB を超えるページは `too_large: true` で `raw` / `html` が空
- 無いページは 404 `page_not_found`

#### 3.94 `POST /projects/{id}/docs/init` → 200 `DocsInitResult`（**管理系**）

- 本文は空の JSON（`{}`）でよい
- 文書の根が既にあれば**何もせず** `created: false`（その根を返す）
- 無ければ `~/workspace/<案件 slug>/` に作る（slug は案件の題名の ASCII 化。作れなければ案件の id）。
  既に同じ名前の git リポジトリがあればそれを使い、**中身は触らない**
- `$HOME` が分からない・`git` が動かない・置き場が空でないときは 409 `docs_unavailable`

#### 3.95 `PUT /projects/{id}/docs/page` → 200 `DocPageResult`（**管理系**）

```json
{"path": "docs/research/fs.md", "body": "# 調べたこと\n…", "etag": "<blob sha>", "message": "docs: 直した"}
```

- `etag` は **§3.93 が返した値**。ページが既にあるのに `etag` が無い / 違えば **409 `etag_mismatch`**
  （`etag` 拡張フィールドにいまの値が載る）。新しいページは `etag` を付けない
- `message` の既定は `docs: <path>`
- **人のチェックアウトが default_branch を出していて未コミットの変更があれば 409 `default_branch_busy`**
  （ADR-0043 D5 と同じ規則。何も触らない）
- 中身が同じなら新しいコミットを作らず `unchanged: true` を返す
- 文書の根が無い案件では**その場で作る**（§3.94 と同じ）

#### 3.96 `DELETE /projects/{id}/docs/page?path=&etag=` → 200 `DocPageResult`（**管理系**）

- `etag` の規則は §3.95 と同じ（無い / 違えば 409 `etag_mismatch`）。消えたページは 404 `page_not_found`
- 応答は `deleted: true`、`etag: null`

#### 3.97 `POST /tasks/{id}/artifacts/promote` → 200 `DocPageResult`（**管理系**）

```json
{"name": "answer.md", "path": "docs/research/fs.md", "title": "調べたこと", "overwrite": false}
```

- `name` はそのタスクの成果物（`ArtifactProduced` の `name`。同じ名前が複数あれば**いちばん新しいもの**）。
  無ければ 404 `artifact_not_found`、UTF-8 で読めなければ 422 `validation`
- 中身の先頭に front matter を**混ぜて**コミットする（`title` と `tasks: [<タスク id>]`。既にある
  front matter の `tags` は残し、同じタスクは 2 回足さない）
- 宛先が既にあれば **409 `page_exists`**（`overwrite: true` なら上書き）
- 案件に属さないタスクは 409 `docs_unavailable`、知らないタスクは 404 `task_not_found`
- コミットの規則（一時 worktree・fast-forward・`default_branch_busy`）は §3.95 と同じ
- 昇格したページはそのタスクの `GET /tasks/{id}/timeline` に `kind = "doc"` として出る（逆リンク）

### 3.98〜3.100 Console（ADR-0048 D1/D2。読み取り）

Console は「全案件の流れが一本で見える画面」。celeris は**正規化したブロック**だけを返し、GUI は
イベントの種類やアダプタごとの差を知らない。**読み取りだけ**で状態は変えない。入力（`POST /console/instruct`）と
CoS の `actions` は ADR-0048 D3（§3.107）。

#### 3.98 `GET /console?scope=&since=&limit=` → 200 `ConsolePage`

| クエリ | 既定 | 説明 |
|---|---|---|
| `scope` | `all` | `all` / `project:<ULID>` / `node:<org ノード id>`。形が違えば 400 |
| `since` | – | 前回の `next_cursor`（**不透明な文字列**。GUI は中身を解釈しない）。形が違えば 400 |
| `limit` | 100 | 1〜**200**（超えたら 200 に丸める。0 は 400） |

- `items` は**時刻の昇順**（新しいものが最後。GUI は下に足す）。
- `since` **無し**は「いちばん新しい `limit` 件」、`since` **有り**は「そのカーソルより後の `limit` 件」。
  同じブロックを 2 回返さない。
- `next_cursor` は最後のブロックの位置。1 件も無ければ渡した `since` をそのまま返す（増分取得に使える）。
- 引きはすべて上限付き（`events` は 1 回 4,000 件の窓、対話・認可・報告は `limit × 4`（最大 800））。
  窓より古いものは `GET /tasks/{id}/events` や `GET /reports` で見る。

ブロックは `kind` で 9 種（ADR-0048 D1 の 8 種 + ADR-0047 D5 の `knowledge`）。
どれも `at`（RFC 3339）と `cursor` を持つ:

| `kind` | 中身 | 由来 |
|---|---|---|
| `human` | 人の発言（`text` / `node_id` / `project_id` / `task_id`）。ADR-0056 D2: `author`（`mcp:<client_id>`。MCP の `console_instruct` が付けた発言だけ。人の発言は省略）で GUI は「外部（<client name>）」の帯を出せる | `messages`（`role = user`） |
| `reply` | CoS・部署ノードの返事（Markdown。`run_id` 付き）。CoS が `actions`（§3.107）を宣言していれば `actions_result`（`MessageMetadata`: `actions_executed[]` / `actions_failed[]`）。ADR-0054 D2: `state`（`streaming` \| `done`。省略時 `done`）・`thinking`（run 中の最新の思考 1 行。置き換え式）・`steps[]`（`{kind: tool_use\|tool_result, tool?, text, error?}`。run 中の道具の呼び出しを順番どおり） | `messages`（`role = node`）。`state = streaming` のときは対話 run の `Event::WorkerProgress` から合成（まだ `messages` に確定していない） |
| `task` | 開始・終了・失敗・中止・割り込みの 1 行（`task`: `from` / `to` / `reason` / `assignee` / `harness` / `tier` / `mode` / `elapsed_secs`） | `Event::Transitioned` |
| `progress` | run ごとに束ねたワーカーの進行。`progress`: `run_id` / `count` / `tool_count` / `last_status` / `started_at` / `updated_at` / `first[]` / `last[]` / `truncated`。見出し用に `title` / `assignee` / `harness` / `tier` | `Event::WorkerProgress`（ADR-0048 D2 の正規化） |
| `question` | ディスパッチャの質問（`text` / `answered` / `answer` / `run_id`） | `Event::QuestionRaised` + `Event::Answered` |
| `approval` | 認可 1 件（`Approval` をそのまま。`decision` / `answer` / `decided_at` 付き） | `approvals` |
| `milestone` | 途中目標の提案（`proposed` のものだけ）と秘書のレビューの返事 | `milestones` + `messages` |
| `report` | 報告（見出しと本文） | `reports` |
| `knowledge` | 知識整理 run の結果（ADR-0047 D4/D5）。`task_id` / `task_title` / `run_task_id` / `state`（`applied` \| `failed`。`scheduled` は出ない）/ `ingested` / `inbox` / `discarded` / `via`（ADR-0052 D2。`"langmem"` \| `"fallback:<adapter>"`。GUI は後者に「cheap のハーネスで抽出」を出す） | `knowledge_runs`（`state != scheduled`）+ 元のタスク |

範囲の効き方:

- `project:<id>`: そのタスク（`tasks.project_id`）・その案件についての対話（`messages.project_id`）・
  報告（`reports.project_id`）・その案件の途中目標。
- `node:<id>`: そのノードが担当のタスク（`tasks.assignee`）・そのノードとの対話（`messages.node_id`）・
  そのノードの報告（`reports.node_id`）。**途中目標は出ない**（途中目標は案件のもの）。
- 同じ質問が `approvals` にもあるときは **`approval` の側だけ**出す（同じことを 2 回出さない）。

`progress` の `first[]` / `last[]` は折り畳みの見出し用に**始めの 3 行と終わりの 3 行**だけ
（`truncated = true` なら間が省かれている）。全行は §3.100 で取る。1 行は
`{at, seq, kind?, tool?, text, error?}` で、`text` は `summary` があればそれ、無ければ `msg`。

**育つ返事（ADR-0054 D2）**: 対話 run（`task.conversation` あり。CoS の対話・ノードとの対話の
どちらも）の進行は、他のタスクのような折り畳んだ `progress` ではなく、**`reply`（`state = "streaming"`）**
として出る。`kind = thinking` の行は `thinking` を**置き換え**（積み上げない。最新の 1 行）、
`kind = text` の行は `text` に**そのまま追記**（アダプタが部分文字列で送るぶんだけ）、
`kind = tool_use` / `tool_result` の行は `steps[]` に**順番どおり追加**する（`tool_use` は `tool` +
`summary`、`tool_result` は折り畳み用に 1 行）。`kind = status` の行はここには出さない。
run が終わり `messages` に確定すると、**別の `reply` ブロック**（`state = "done"`、`thinking`/`steps` は
空）が届く。GUI は同じ `run_id` の `reply` を 1 つの吹き出しとして扱う（§3.99 の SSE の積み方を参照）。

#### 3.99 `GET /console/stream?scope=&since=`

`GET /stream` と同じ枠組み（認証・同時接続数 16・`heartbeat`・停止で閉じる）。`scope` は §3.98 と同じ。

```
event: hello
data: {"cursor":"00001789…000.12345.e12345","scope":"all","now":"…"}

event: console.block
data: {…§3.98 のブロック 1 件…}

event: heartbeat
data: {"now":"…"}
```

- `since` を渡さなければ**「今」から**（履歴は §3.98 で取る）。
- `progress` と、対話 run の育つ `reply`（`state = "streaming"`）は **run ごとに 1 秒に 1 回まで**にまとめて
  流す（1 回の道具呼び出しごとにフレームが飛ばない）。流れてくる `progress` ブロックは**その run の積み上げ**
  （`count` / `tool_count` はその接続で見た合計）。育つ `reply` は**その接続で見た増分だけ**（`text` は
  その回で届いた分だけ・GUI 側が同じ `run_id` の既存の吹き出しに追記する。`thinking` は最新の値で置き換え）。
- `task` / `human` / `question` / `approval` / `milestone` / `report` と、確定した `reply`
  （`state = "done"`）はまとめずにそのまま流す（対話・認可・報告は 1 秒ごとに見に行く）。
- `Last-Event-ID` は使わない（`id:` を付けない。再開は `since` で行う）。

#### 3.100 `GET /tasks/{id}/runs/{run_id}/events?after_seq=&limit=` → 200 `EventsPage`

折り畳んだ `progress` を開いたときに取る、その run の**全行**。`GET /tasks/{id}/events` の run 絞り込み版で、
`run_id` を持つイベント（`worker_started` / `worker_progress` / `artifact_produced` / `worker_finished` /
`review_verdict` / `question_raised` / `delegated`）だけを `seq` 昇順で返す（遷移は run に紐づかないので入らない）。
`limit` は既定 500・最大 5,000。知らないタスクは 404 `task_not_found`。

`worker_progress` の行には ADR-0048 D2 の `kind` / `tool` / `summary` / `detail` / `truncated` / `error` が
**あれば**入っている（付けないワーカー・導入前のイベントには無い）。

### 3.101〜3.106 知識ベース（ADR-0047。**変更系は管理系**）

**正本は `[knowledge] root`（既定 `~/.local/share/celeris/knowledge`）の Markdown**（DB には何も持たない）。案件の文書（§3.92〜3.97）と
同じ流儀だが、**正本は作業ツリーのファイルそのもの**なので、読み取りは常にファイルを読む（人が編集中の未コミットの
変更もそのまま見える）。書き込みは作業ツリーに書いてから**そのパスだけを 1 件 1 コミット**する。
author / committer は `Celeris (human) <celeris@local>`（`celerisctl knowledge record` が作る候補だけ
`Celeris (knowledge) <celeris@local>`）。

- `path` は **KB の根からの相対**（`environment/clusters/pegasus.md`）。`..`・絶対パスは **403 `path_forbidden`**、
  `.md` で終わらないパスは **422 `validation`**
- `etag` は**ページの中身の sha256**（文書の blob sha とは別物。GUI は opaque な値として扱う）
- `[knowledge] root` が設定されていなければ **409 `knowledge_unavailable`**。設定されていてもディレクトリが
  まだ無ければ、**読み取りは `initialized: false` を返すだけ**（何も作らない）、変更系は
  409 `knowledge_unavailable`（用意するのは `celerisctl knowledge init` だけ）
- 書き込みのあとは必ず `index.json` を作り直す（索引は**再生成できる派生物**。バージョン管理には入らない）
- `_inbox/` の候補は**索引にも検索にも出ない**。人が §3.105 / §3.106 で accept / reject する

組織の「人」（ワーカー）はこの HTTP ではなく **`celerisctl knowledge`**（ADR-0047 D3）で KB を読む。
前置きには索引（`path` / `title` / `tags`、最大 200 件）だけが載り、本文は載らない。

#### 3.101 `GET /knowledge/tree?scope=&q=` → 200 `KnowledgeTree`

- `root`（絶対パス）・`initialized`・`generated_at`（`index.json` を作った時刻）・`inbox_count`
- `items`: `path` / `title` / `tags[]` / `scope` / `sources[]` / `updated` / `confidence`
  （`?q=` が無ければパスの昇順、最大 500。超えたら `truncated: true`）
- `scopes`: **ページのあるディレクトリ**の一覧（`items[].path` の親ディレクトリを重複無し・名前順に並べたもの。
  画面のツリーの見出し）。front matter の `scope`（`project:pluvio` のようなラベル）はここには入らない
  — それは `items[].scope` の方。`?scope=` / `?q=` で絞っても **`scopes` は KB 全体のまま**（見出しは動かない）
- `?scope=` は **KB の相対パスの接頭辞**（`user` / `environment/clusters` / `projects/pluvio`）か
  front matter の `scope` の値（`project:pluvio`）
- `?q=` があれば **索引の `tags` / `title` と本文の全文一致**で絞り、
  **tag 一致数 → title 一致数 → 本文一致 → `updated` の新しい順**に並べる（ADR-0047 D3）

```json
{"root": "/home/u/knowledge", "initialized": true, "generated_at": "2026-09-20T01:00:00Z",
 "inbox_count": 2, "truncated": false, "scopes": ["environment/clusters", "user"],
 "items": [{"path": "environment/clusters/pegasus.md", "title": "pegasus の使い方",
            "tags": ["environment", "cluster", "pegasus"], "scope": "environment",
            "sources": ["human"], "updated": "2026-09-20", "confidence": "low"}]}
```

#### 3.102 `GET /knowledge/page?path=` → 200 `KnowledgePage`

- `raw`（front matter を含む Markdown のもと）、`html`（**サーバで描画**。`pulldown-cmark`、
  **生 HTML は捨てる**）、`title` / `tags[]` / `scope` / `sources[]` / `confidence` / `updated`、
  `history`（直近 20 件、新しい順）、`etag`
- 本文中の `celeris:task/<ULID>` は `/tasks/<ULID>` に、`[[相対パス.md]]` は `/knowledge?path=…` に開く
- 512 KiB を超えるページは `too_large: true` で `raw` / `html` が空
- 無いページは 404 `page_not_found`

#### 3.103 `PUT /knowledge/page` → 200 `KnowledgePageResult`（**管理系**）

```json
{"path": "environment/clusters/pegasus.md", "body": "---\ntitle: …\n---\n…", "etag": "<sha256>", "message": "knowledge: 直した"}
```

- `etag` は **§3.102 が返した値**。ページが既にあるのに `etag` が無い / 違えば **409 `etag_mismatch`**
  （`etag` 拡張フィールドにいまの値が載る）。新しいページは `etag` を付けない
- `message` の既定は `knowledge: <path>`
- 中身が同じなら新しいコミットを作らず `unchanged: true`
- `_inbox/` のパスは **403 `path_forbidden`**（候補は §3.105 / §3.106 で扱う）

#### 3.104 `GET /knowledge/inbox` → 200 `KnowledgeInbox`

- `items`: `id`（`_inbox/<id>.md` のファイル名から `.md` を取ったもの）/ `path` / `title` / `tags[]` /
  `scope` / `sources[]` / `confidence` / `created`（front matter のまま。RFC 3339 か `YYYY-MM-DD`）/ `body` / `html` /
  `target`（取り込み先。front matter の `path`、無ければ `scope` と題名からの既定）/ `target_exists`
- 新しい順（id の降順 = 記録した時刻の降順）

#### 3.105 `POST /knowledge/inbox/{id}/accept` → 200 `KnowledgePageResult`（**管理系**）

```json
{"path": "environment/clusters/pegasus.md", "overwrite": false}
```

- 本文は省略してよい（`{}` か空）。`path` を渡せばそこへ、渡さなければ候補の `target` へ移す
- `_inbox/` から消して宛先に書き、**両方のパスを 1 コミット**にする。`path:`（候補専用の鍵）は落ち、
  `updated` は今日になる
- 宛先が既にあれば **409 `page_exists`**（`overwrite: true` なら上書き）
- 知らない id は 404 `candidate_not_found`、`/` や `..` を含む id は 403 `path_forbidden`

#### 3.106 `POST /knowledge/inbox/{id}/reject` → 200 `KnowledgeRejectResult`（**管理系**）

- `_inbox/<id>.md` を消してコミットする（履歴には残る）。応答は `{"id": "…", "sha": "…"}`
- 知らない id は 404 `candidate_not_found`

### 3.107 `POST /console/instruct` → 202 `ConsoleInstructAccepted`（管理系。ADR-0048 D3）

Console の入力欄の文の入口。`POST /org/{id}/messages`（§3.47）と同じ経路に載せるだけで、返事は待たない
（`GET /console` / `GET /console/stream` で拾う）。

```json
{"text": "@software-engineering このバグを直して", "scope": null}
```

`InstructBody`: `text`（必須。空白だけは 422 `validation`）、`scope`（省略可。`all` / `node:<id>` /
`project:<id>`。形が違えば 400 `bad_request`）。

- **相手の決め方**（決定的。LLM は関与しない）:
  1. `scope = node:<id>` → そのノード。
  2. それ以外で `text` が `@<node-id> `（`@` の直後に空白でない 1 語、その後ろに空白）で始まる → その
     ノード。**`@<node-id> ` は送る本文から取り除く**（残りの本文だけがそのノードへの発言になる）。
  3. `scope = project:<id>` で `@mention` が無い → **CoS**（組織の根。`OrgKind::Secretary`）への対話を
     その案件に紐づける（`Message.project_id` / `Task.project_id` がその案件）。
  4. それ以外（省略・`all`、`@mention` 無し）→ CoS への対話（案件に紐づかない）。
- ノード宛て（1・2）は知らない id なら 404 `org_node_not_found`。CoS 宛て（3・4）は組織に CoS
  （`OrgKind::Secretary` のノード）が無ければ 404 `org_node_not_found`（`id: "cos"`）。
- 応答 `ConsoleInstructAccepted`: `message_id`（入った `role = "user"` の行）、`task_id`（起こした対話用
  タスク）、`node_id`（実際に話しかけた相手。ノード宛てならそのノード、CoS 宛てなら CoS の id）。
- CoS への対話 run の前置きには、通常の CoS 対話（§3.47 のノード一覧）に加えて**進行中の案件とその途中目標**
  （`proposed` / `active` の案件だけ）と、`actions` の宣言の仕方（JSON の形と目安）が入る。CoS が結果ファイルに
  `actions`（`create_task` / `propose_project` / `add_milestone` / `ask_human`）を宣言すると、taskd が**決定的に**
  実行する（LLM はしない）:
  - `create_task`: **`ready`**（人の Go 済み）のタスクを作る。`assignee` を省けば次 tick の matching（ADR-0046 D5）が
    決める。`project` / `milestone` は id（省略可）、`harness` はハーネス id（`[[harnesses]]` を設定していれば検証）、
    `acceptance` は 1 件以上必須。
  - `propose_project`: `proposed` の案件を作る。`repos[]` は**絶対パス**（1 件目が primary。名前・種類は
    `POST /projects/{id}/repos` と同じ既定から決める）。
  - `add_milestone`: 既存の案件の末尾に `proposed` の途中目標を足す。**ADR-0079 D12 で廃止**（「途中目標は root task の段階で表す」の理由付きで落ち、人に見える。代わりに `create_task.stages_hint`）。
  - `ask_human`: taskd 側では何も作らない（人への問いかけ自体が返事の本文）。「実行できた」として記録するだけ。
  - 検証に落ちた action（知らない harness / repos / 案件など）は**実行されない**。CoS の返事の Markdown に
    「実行できなかった action: …」の節が付き、`reply` ブロックの `actions_result.actions_failed[]` にも理由が残る。
  - 実行結果は `Message.metadata`（`MessageMetadata`）に残り、`GET /console` の `reply` ブロックが
    `actions_result` として運ぶ（§3.98 の表）。**同じ run の actions は 1 回だけ実行される**（冪等）。
  - CoS が `actions` を出すと run はそこで終わる（サブエージェントに一通り投げたら一旦止まる。ADR-0054
    D2）。作った `create_task` は `task` ブロックとして返事の直下に出て、その後の進行はそのタスク**自身**
    の run として折り畳みの `progress` で流れる（既定は折り畳み。§3.98）。
- **入力のキュー（ADR-0054 D2）**: 同じノード（**CoS は案件をまたいでも 1 つの列**。D1 の継続
  セッションが `project_id` に関わらず全体で 1 本のため。CoS 以外のノードは案件ごとに別の列）に未終了の
  対話用タスクがあれば、新しい `POST /console/instruct` は 202 を返しつつ、その対話用タスクを**依存**
  （`depends_on`）として作る。run は前の対話が終わるまで始まらない（202 の応答自体はすぐ返る。「投げた」
  ことと「run が始まった」ことは別）。
- **CoS の対話 run に許す読み取りの道具（ADR-0054 D2）**: `celerisctl knowledge search|get`・
  `celerisctl ls|show`（タスク）・`celerisctl projects ls|show`（案件）だけ。書く操作は
  §3.107 の `actions` 経由だけ（対話 run 自身は道具を使わない、という ADR-0033 D4 の原則の例外はこれだけ）。
  アダプタごとの実現（GUI からは見えない、taskd 内部の話）は claude-code が `--allowedTools`、codex が
  `sandbox_mode="read-only"`、ACP は道具単位の許可が無いため対話 run 中は道具の許可要求を一律拒否
  （ADR-0054 を参照）。

### 3.108 `GET /llm/sources` → 200 `LlmSourcesView`（ADR-0053 D4）

LLM source のローカル OpenAI 互換プロキシ（`crates/llm-proxy`。`127.0.0.1:18100`、`/api/v1` の外）が
使っている供給元の観測。判断（選択・cooldown）はプロキシの中で決定的に行われる。ここは**見えるように
するだけ**（GUI の表示は `/accounts` の「LLM source」節）。

- 通常の読み取り認証（`token_file` があれば Bearer 必須）。
- `[llm_proxy]` が無効（`enabled = false`、または `claude_oauth`/`codex_oauth`/`openai_compatible` が
  1 つも無い）なら **409 `llm_proxy_unavailable`**。

```json
{"sources": [
  {"id": "claude-oauth", "kind": "claude-oauth", "enabled": true,
   "accounts": [{"id": "acct-a", "logged_in": true, "remaining": 0.62,
                 "remaining_short": 0.62, "remaining_long": 0.81}],
   "last_hour_requests": 12, "last_hour_prompt_tokens": 3400, "last_hour_completion_tokens": 900},
  {"id": "openai-compatible:qwen", "kind": "openai-compatible", "enabled": true, "reachable": true,
   "accounts": [], "last_hour_requests": 40, "last_hour_prompt_tokens": 9000,
   "last_hour_completion_tokens": 5000}
],
"celeris_tiers": [
  {"tier": "frontier", "resolves_to": "claude-oauth"},
  {"tier": "standard", "resolves_to": "claude-oauth"},
  {"tier": "cheap", "resolves_to": "openai-compatible:qwen"}
]}
```

- `sources[].id`: `claude-oauth` / `codex-oauth` / `openai-compatible:<id>`（設定の `[[llm_proxy.sources.openai_compatible]] id`）。
- `reachable`: `openai-compatible` だけ `GET <base_url>/models` の probe 結果（60 秒キャッシュ）。
  `claude-oauth`/`codex-oauth` は到達性ではなくアカウントの残量で見るので、フィールド自体が省略される
  （`null` を書かない。省略 = 「この供給元には意味が無い」）。
- `accounts[].remaining`: 0.0〜1.0。短期・長期のうち**厳しい方**（残りが少ない方）。測れないとき
  （観測が古い・無い）はフィールドを省略する（値を捏造しない。ADR-0024 D3 と同じ規律）。
  `cooldown_until`/`cooldown_reason` も 429/401 を受けた直後だけ載る（このアカウントプールは CLI
  ワーカーの dispatch と**同じ帳簿**を共有するので、`GET /accounts` の cooldown とも一致する）。
- `accounts[].remaining_short` / `remaining_long`（ADR-0053 D4）: 短期枠（Claude の 5 時間 /
  Codex の週内相当）・長期枠（7 日）それぞれ単独の残り。`remaining` と同じ「測れないときは省略」の規律。
- `last_hour_*`: `llm_proxy_requests`（migration 0022）の直近 1 時間の集計。本文は記録しないので
  ここにも出ない。
- `celeris_tiers[]`（ADR-0053 D4）: `celeris/<tier>` が**今**どこに解決するか（`server.rs` の
  実際の選択と同じ決定的な計算を、副作用なしでなぞるだけ）。`resolves_to` は `sources[].id` と同じ形。
  選べる候補が無ければ `null`（`no_source_available` になる状態）。古いスナップショットには無いので
  省略時は空配列として扱う。

### 3.109 `POST /console/new-conversation` → 204（管理系。ADR-0054 D1）

CoS の**継続セッション**（`node_sessions`。§3.107 の対話 run が `--resume` 等で続けているもの）を捨てる。
GUI の「新しい会話」ボタンの入口。**薄い**: ディスパッチャには触らず、ストアの `node_sessions.retired_at`
を立てるだけ（DESIGN 原則 1）。

- 要求本文は無い（`{}` を送っても無視される）。
- 現役セッションが有っても無くても **204**（結果として「捨てた」状態にするだけなので、既に無かったことを
  エラーにしない）。
- 効果: 次に人が CoS（`GET /console` の `scope=all` / `project:<id>`。§3.98）に話しかけたときの対話 run は
  **新規セッション**（前置きは全量。§3.107 の「進行中の案件」等をもう一度渡す）から始まる。捨てなければ、
  逼迫（`[sessions] rollover_tokens`）かアカウント変更が起きるまで、前置きは差分だけ（`session_diff`）が
  続く。
- 部門長（engineering/research/operations の根ノード）のレビュー・切り分け run（ADR-0051）の継続セッション
  （`kind = lead`）はこの API の対象外（部署ごとに 1 本、GUI からの操作は今回のスコープに無い）。

### 3.110〜3.111 MCP サーバーの観測（ADR-0056 D4。読み取り）

外部エージェントが Celeris を操作する MCP サーバー（`crates/celeris-mcp`。`[mcp] listen` の既定
`127.0.0.1:18200`、`/api/v1` の外・別ポート）が持つクライアント表（`mcp_clients`）と呼び出しログ
（`mcp_calls`）の観測。判断（認証・スコープ・流量制限）は MCP サーバーの中で決定的に行われる。ここは
**見えるようにするだけ**（GUI の表示は「アカウント」画面の「MCP クライアント」節、後続の GUI Phase）。

- `GET /mcp/clients` → 200 `{"items": [McpClient]}`。`McpClient` は `id` / `name` / `scopes[]`
  （`"knowledge:read"` 等の文字列） / `created_at` / `last_used_at`（無ければ省略） /
  `revoked_at`（無ければ省略）。**トークンの値は出ない**（DB にも平文では無い。ハッシュも出さない）。
- `GET /mcp/calls?client=<id>` → 200 `{"items": [McpCall]}`。`client` は省略可（省略時は全クライアント）。
  新しい順、直近 100 件。`McpCall` は `id` / `client_id` / `tool` / `ok` / `error_kind`（成功なら省略） /
  `latency_ms` / `at`。**引数と結果の本文は残さない**（ADR-0056 D4）。`console_instruct` の呼び出しは
  Console（§3.98）にも出るので二重には書かない。
- どちらも読み取り専用。`token_file` があれば Bearer 必須。

### 3.112〜3.117 skills を GUI から見る・作る・mount する（ADR-0056 D3）

KB の専用ディレクトリ `skills/<name>/SKILL.md`（Claude Code の skills 形式。
frontmatter に `name` / `description` 必須）と `Profile.skills_mounts` を、MCP だけでなく GUI からも見て・
作って・mount できるようにする。**MCP の `skills_put` / `org_mount_skill` / `org_unmount_skill` と同じ
`task_ops::knowledge::{skills_put, set_skill_mount}` を呼ぶ**ので、挙動は MCP 経由でも GUI 経由でも同一。
`skills`（能力タグ、§3.44）とは別物（名前が近いが役割が違う）。

- KB（`[knowledge] root`）が未設定なら §3.101〜3.106 と同じ 409 `knowledge_unavailable`。ディレクトリは
  あるが skill が 1 つも無ければ `initialized: true` かつ `items: []`
- skill 名は `[a-z0-9-]{1,64}`。合わない名前は 422 `validation`
- `mounted_by`（§3.112・3.113）は**継いだ後**（`EffectiveProfile.skills_mounts`）で判定する。親ノードで
  mount すれば、その子ノードの id もここに現れる（GUI は継承を再計算しない。§3.44 と同じ規律）
- skill が 1 つでもどこかのノードに mount されていれば §3.115 の `DELETE` は 409 `skill_mounted` で断る
  （まず §3.117 で unmount してから消す）

#### 3.112 `GET /skills` → 200 `SkillList`

- `root`（絶対パス）・`initialized`
- `items[]`: `name` / `description`（frontmatter）/ `updated`（`SKILL.md` の最後のコミット時刻、RFC 3339。
  無ければ省略）/ `mounted_by[]`（mount している組織ノードの id、無ければ省略）

```json
{"root": "/home/u/knowledge", "initialized": true,
 "items": [{"name": "rust-review", "description": "Rust のコードレビューの手順",
            "updated": "2026-09-21T10:00:00Z", "mounted_by": ["coding"]}]}
```

#### 3.113 `GET /skills/{name}` → 200 `SkillDetailView`

- `name` / `skill_md`（frontmatter を含む本文）/ `files[]`（同じディレクトリの付属ファイルの相対パス。
  本文は運ばない ＝ `SKILL.md` 自身しか本文を持たない。ADR-0056 D3 P-79-a と同じ「索引だけ渡す」設計）/
  `updated` / `mounted_by[]`
- 無い skill は 404 `skill_not_found`

#### 3.114 `PUT /skills/{name}` → 200 `SkillPutResult`（**管理系**）

```json
{"skill_md": "---\nname: rust-review\ndescription: …\n---\n\n# rust-review\n…",
 "files": [{"path": "checklist.md", "content": "- fmt\n- clippy\n"}]}
```

- 作成・更新の両方（`skills/<name>/` が無ければ作る）。`skill_md` の frontmatter の `name` は URL の
  `{name}` と一致していること、`description` は空でないこと（違反は 422 `validation`。
  `task_ops::knowledge::SkillError` の文言をそのまま返す）
- frontmatter に `source:` が無ければ `source: gui` を足す（MCP 経由は `mcp:<client_id>`。ADR-0056 D3）。
  冪等（既に `source:` があれば触らない）
- `files[]` は省略してよい（既定 `[]`）。パスは相対で `..` を含めない（違反は 422 `validation`）
- 応答は `{"path": "skills/<name>/SKILL.md"}`

#### 3.115 `DELETE /skills/{name}` → 204（**管理系**）

- どこかの組織ノードに mount されていれば **409 `skill_mounted`**（`detail` に mount しているノード id
  の一覧）。無い skill は 404 `skill_not_found`
- `skills/<name>/` をまるごと消して 1 コミット

#### 3.116 `POST /org/{id}/skills` → 200 `OrgNode`（**管理系**）

```json
{"skill": "rust-review"}
```

- そのノードの `profile.skills_mounts` に skill 名を足す（重複は足さない。冪等）。応答は更新後の
  `OrgNode`（`PATCH /org/{id}` §3.44 と同じ形）
- 知らないノードは 404 `org_node_not_found`。skill 名の形が不正なら 422 `validation`
  （**KB に実在するかは検査しない** — ADR-0056 D3「mount が門」。まだ無い skill 名を先に mount しておける）

#### 3.117 `DELETE /org/{id}/skills/{skill}` → 200 `OrgNode`（**管理系**）

- そのノードの `profile.skills_mounts` から skill 名を外す（無くても 200。冪等）。応答は更新後の `OrgNode`
- 知らないノードは 404 `org_node_not_found`

---

### 3.118〜3.124 browser の人待ち: 登録依頼・承認（ADR-0080 D4/D5）

task は `blocked` のまま、wait の `reason`（`waiting_for_auth` / `waiting_for_approval`）が `BrowserRunState`
の `WAITING_FOR_AUTH` / `WAITING_FOR_APPROVAL` を運ぶ。wait の保存と同じトランザクションで task が
`running → blocked`（worker の lease を解放）になり、`browser_wait_opened` を追記する。解決・終端化は
`browser_wait_resolved`（固定の `code`）。**秘密は wait・event・応答のどこにも無い**。全応答
`Cache-Control: no-store`、エラーは固定コードだけ（要求の値を反射しない）。

| # | 接点 | 認可 | 本文 → 応答 |
| --- | --- | --- | --- |
| 3.118 | `POST /tasks/{id}/browser/requests` | 管理系（supervisor） | `NewBrowserWait` → 201 / 200（同じ `resume_key` の再送）`BrowserRequestResult` |
| 3.119 | `GET /tasks/{id}/browser/waits` | 読み取り | → `BrowserWaitList` |
| 3.120 | `GET /browser/waits` | 読み取り | 人の対応待ち（`pending`）→ `BrowserPendingList`（inbox の `browser_waits` と同じ形） |
| 3.121 | `POST /tasks/{id}/browser/waits/{wait_id}/credential` | 管理系 + attestation（`register`） | `BrowserCredentialBody`（username/password は broker の control IPC にだけ渡す）→ `BrowserWaitResult` |
| 3.122 | `POST /tasks/{id}/browser/waits/{wait_id}/registered` | 管理系 + attestation（`register`） | `BrowserRegisteredBody`（broker の receipt を照合）→ `BrowserWaitResult` |
| 3.123 | `POST /tasks/{id}/browser/waits/{wait_id}/decision` | 管理系 + attestation（`approve_once` / `deny`） | `BrowserDecisionBody` → `BrowserWaitResult` |
| 3.124 | `POST /tasks/{id}/browser/waits/{wait_id}/revoke` | 管理系 + attestation（`revoke`） | `BrowserRevokeBody`（未消費の承認を失効）→ `BrowserWaitResult` |

- human attestation: `{payload, signature}`。`payload` は `AttestationClaims` の JSON（actor・owner session
  hash・task/wait・`expected_version`・decision・policy hash・nonce・30 秒以内の `expires_at`）で、GUI 専用鍵の
  Ed25519 署名（hex）を付ける。daemon は公開鍵だけを持つ。bearer token だけでは登録・承認・拒否できない。
- 状態: 登録待ちは `registered`（task → `ready`。使用承認は兼ねない）か拒否・期限切れで `failed`。承認待ちは
  `approved`（task → `ready`、worker が同じ run/session で一度だけ消費）か `deny` / 期限切れで `failed`
  （`approval_denied` / `browser_wait_expired`、自動 retry なし）。cancel は wait を `cancelled` に閉じる。
- 期限: 登録待ちは既定・上限 24 時間、承認待ちは既定・上限 5 分。期限切れはディスパッチャの tick（起動直後を
  含む）が一度だけ終端化する。
- 未解決の wait がある間、一般の `answer` / 途中確認の再開では `ready` に戻せない（409 `invalid_transition`）。
  inbox の `questions` には出さず `browser_waits`（`counts.browser_waits`）に出す。
- エラー: 401 bearer なし、403 `attestation_invalid` / `attestation_replayed`、404 `browser_wait_not_found`、
  409 `browser_wait_version_conflict` / `browser_wait_state` / `task_not_running`、410 `browser_wait_expired`、
  422 `browser_body_invalid` / `browser_wait_invalid`（`field` だけ）/ `credential_receipt_invalid` /
  `credential_rejected`、503 `browser_unavailable`（attestation 鍵・broker が未設定）。

### 3.125 実行・計画・木・決定の要求（ADR-0072・ADR-0079）

ExecutionPlan と WorkUnit、再帰的な task の木（ADR-0079）、人への決定の要求、ADR-0079 R5a で撤去した入口をまとめる。
認証・本文の規約・エラーの形は §1 のとおり（変更系はすべて管理系）。型名は `api-v1.schema.json` の `$defs` を指す。

#### 3.125.1 `GET /tasks/{id}/execution` → 200 `TaskExecutionView`

タスクの実行詳細。`runs`（`RunSummary[]`）と `metrics`（`ExecutionMetrics`）は必須。次の欄は値が無ければ省略する（`null` は出さない）: `gate`（`ExecutionGateDecision`。gate の対象外か未判定なら無い）、`phase`（`ExecutionPhase`）、`plan`（`ExecutionPlanView`。計画の無い atomic タスクでは無い）、`phase_checkpoint`（工程の後の途中確認で止まっているときだけ）、`awaiting_children`（`phase = awaiting_children` のときだけ。空なら省略）、`plan_approval`（`PlanApprovalView`。`phase = awaiting_plan_approval` のときだけ）。`metrics.total_cache_read_tokens` は観測できた cached input tokens の合計で、未観測なら省略する。不明なタスクは 404。クエリは受け付けない。

#### 3.125.2 `GET /tasks/{id}/execution-plan` → 200 `ExecutionPlanView`

有効な計画と `work_units[]` を返す。`versions[]` は `ExecutionPlanVersionView` の版履歴（`version` 昇順、superseded を含む）。有効な計画が無ければ 404 `execution_plan_not_found`。`ExecutionPlanView` は `id`、`task_id`、`version`、`origin`、`status`、`plan`、`created_at`、`work_units` が必須で、`versions` は既定 `[]`。`plan` は採用した `ExecutionPlanSpec` をそのまま返す（/3 を内部の /2 の形に写したものではない）。`plan.schema` は `celeris.execution-plan/1`、`celeris.execution-plan/2`（`phases` と `work_units`）、`celeris.execution-plan/3`（ADR-0079 D2。`stages`・`units`・`decisions` を持ち、`work_units` は空。空の `stages` / `units` / `decisions` は出力しない）のいずれか。/3 でも `work_units[]`（`WorkUnitView`）は段階を工程として写した行（`phase` = 段階の key、統合 WU を含む）。クエリは受け付けない。

#### 3.125.3 `POST /tasks/{id}/execution-plan`・`PUT /tasks/{id}/execution-plan` → 201 `ExecutionPlanView`（管理系）

本文は `ExecutionPlanSpec`（知らない欄は 400）。型の上で必須なのは `schema` と `rationale` だけで、ほかの欄は省略すると空配列になる。schema ごとの必須欄は検証（`task_core::execution_plan::validate_with`）が決め、違反は 422:

- `celeris.execution-plan/1`: `work_units` が 1 件以上。`phases`・`children`・`stages`・`units`・`decisions` は空。
- `celeris.execution-plan/2`: `work_units` が 1 件以上、`phases` が 1 件以上で、各 WU に `phase` が要る。`children` は書けるが、この入口（人の計画）では子 Task を作らない（`adopt_plan` は子なしで採用する）。`stages`・`units`・`decisions` は空。
- `celeris.execution-plan/3`: `stages` と `units` が 1 件以上、`decisions` は任意。`work_units`・`phases`・`children` は空か省略（書くと 422）。`[execution.tree] enabled = true` が要る（下記）。

人が提案した計画（origin `human`）として検証・採用し、応答に `work_units` と `versions` を含む。`POST` は新規だけで、既に有効な計画があれば 409。`PUT` は有効な計画が無ければ `POST` と同じ（201）、**有効な計画があれば人の replan**（200。ADR-0079 付記「R5b-fix1」）: 本文は新しい版の計画の**全体**（差分の形は受け付けない）で、`task_ops::execution::replan`（origin human）を通す。done の WU は同じ key・`kind`・`phase`（/3 は段階）・`depends_on` で残す必要があり（消す・構造を変えると 422）、spec のほかの欄（`checks` など）は上書きできる（状態は `done` のまま、`work_unit_spec_overridden` の event）。応答には `replan`（`added` / `changed` / `removed` / `overridden_done`）が付く。クエリは受け付けない。管理トークンが無ければ 401、計画が無効なら 422。

`celeris.execution-plan/3`（ADR-0079 D2。`stages` / `units` / `decisions`）は daemon と同じ実効の上限（`[execution.tree]`）で検証する。`[execution.tree] enabled = false`（既定）なら 422（本文に `[execution.tree] enabled = true` を案内する `TreeDisabled`）。有効なら planner の計画と同じ経路を 1 トランザクションで通す（ADR-0079 付記「R5b-prep 実装時の逸脱・明確化」）:

- unit の gate（D4 (3)）: leaf ↔ kind task の上げ下げを採用する spec に当て、食い違いは `unit_gate_overridden` に残す（`adopt` の unit は構造上の理由で `kept_task`）。
- 採用の直後の止め: 計画の決定（計画と unit の `decisions`）は決定の要求（origin `human`、`raised_by.run_id` なし、path は root から）になり、`GET /decisions`・受信箱・Discord（人の計画の版ごとに `plan:<plan_id>:decisions` の 1 通）に出る。答えの無い決定を待つ leaf は `blocked(decision)`、kind task の unit は子を作らずに待つ。木の上限（`max_tree_leaves` など）を超える unit は `kind: limit` の決定で止まる。計画の上限（段階の数・段階あたりの unit・子 task・`max_depth`）の違反は人の計画では 422（planner の最後の試行のように緩めて採用しない）。**`adopt` の unit は `max_child_tasks_per_plan` に数えない**（子を作らない）。
- kind task の unit の `adopt: <task_id>`（D15、人の計画だけ）は同じトランザクションで結ぶ（§3.125.4 の `POST /tasks/{id}/tree/adopt` と同じ条件）。対象が `done` / `failed` なら unit は `done`（`child_adopted`）、まだ終端でなければ unit は結ばれずに待つ（後で `tree/adopt`）。条件に合わない unit があれば計画全体を 409 / 422 で拒否し、何も書かない（`code` は `adopt_*`）。
- root の計画の承認（D8 の `PlanGate`）は挟まない（書いた人の承認とみなす）。報告の流れに「計画を採用して進めます: …」を 1 件残し、承認が要る形（決定・`review: human`・上限に近い）だったなら理由も本文に書く。部をまたぐ子の認可の質問（ADR-0074 F4b）も出さない。

応答（`PUT` / `POST` のときだけ）には `adoptions[]`（`AdoptionOutcome`: `plan_id`、`unit_key`、`stage`、`task_id`、`adopted`、`task_status`、`unit_status`、`detail`。`adopt` の unit があるときだけ）と `decisions_raised`（出した計画の決定の数。0 なら省略）が付く。`GET` では出ない。`celerisctl execution plan set|put <task> --file <json> [--config <config.toml>]`（`--config` 省略時は `CELERIS_CONFIG`。どちらも無ければ木は無効）は同じ関数を呼ぶ。有効な計画の人の replan は `celerisctl execution plan replan <task> --file <json> [--reason <text>] [--config <config.toml>]`（`set|put` は新規だけ）。

#### 3.125.4 `POST /tasks/{id}/tree/adopt` → 200 `AdoptionOutcome`（管理系。ADR-0079 D15）

採用済みの /3 の計画の kind task の unit に、既存の task を木の子として後から結ぶ（人の計画の `adopt` の対象がその時点で終端でなかったとき）。本文は `AdoptRequest`: `task_id`（採用する task）、`stage`（unit の段階）、`unit_key`（unit の key）がすべて必須、知らない欄は拒否。条件: `[execution.tree] enabled`（無ければ 422 `tree_disabled`）、`{id}` が有効な /3 の計画を持つ（422 `adopt_no_tree_plan`）、unit があり（422 `adopt_unit_not_found`）kind task で（422 `adopt_unit_not_task`）同じ段階で（422 `adopt_stage_mismatch`）`adopt` にこの `task_id` が書かれている（422 `adopt_id_mismatch`）、対象は同じ案件（422 `adopt_other_project`）、`{id}` 自身でも祖先でもない（422 `adopt_ancestor`）、execute の仕事の task（対話・裏方でない。422 `adopt_target_kind`）、他の木に属さず自分も木の root でない（409 `adopt_target_in_tree`）、`done` か `failed`（`cancelled` は 409 `adopt_target_cancelled`、終端でなければ 409 `adopt_target_not_terminal`）、unit の行が `pending` / `ready` で子を持たない（409 `adopt_unit_not_open`）、`{id}` が終端でない（409 `adopt_owner_terminal`）。結ぶと 1 トランザクションで unit を `done`（`child_task_id` = 対象、`work_unit_transitioned{reason: child_adopted}` と `child_adopted{plan_id, unit_key, stage, child_task_id}` を `{id}` に）、依存が満たされた unit を `ready` に、対象の `tree` = `{root_id, depth, parent_unit}`（`base_commit` なし）と、`parent_id` が無ければ `{id}`（あれば書き換えない）を書き、対象に `edited{fields: ["tree", ("parent_id")], by: "human"}` を積む。対象の状態・履歴・ブランチ・作業場所は変えない。段階の統合は対象のブランチ `celeris/<task_id>` を任意の項目として扱い、既に main か親のブランチに入っていれば `skipped`、無ければ飛ばす。競合（同時の変更）は 409 `adopt_conflict`、無い task は 404、トークン無しは 401。`celerisctl tree adopt <root> --task <id> --stage <s> --unit <key> [--config <config.toml>]` も同じ。MCP には出していない。

#### 3.125.5 `GET /metrics/execution?since=&group_by=` → 200 `ExecutionMetricsSummary`

`since` は RFC 3339 の時刻（省略可）。`group_by` は `gate_mode`（既定）、`genre`、`assignee`、`lane`、`depth`（ADR-0079 R4a）のいずれか。応答は `group_by`、`total_tasks`、`groups[]` が必須で、`since` は指定したときだけ出る。`accounts_now[]` は schema 上は省略可能だが、応答では常に出る（`[llm_proxy]` が無効なら `[]`）。各 group は `ExecutionMetricsGroup`。不正な日時・group_by は 400。

`group_by=depth`（ADR-0079 D11 / U-R7「深さ別の review run 数と費用」）: `key` は task の層（`"1"` = root と木の無い task、`"2"` = 子、`"3"` = 孫）。各 group にだけ `rollup`（`RollupMetrics`）が付く: その深さの task の**自分の分**の和で、`tasks`、`runs_by_role`（`worker` / `planner` / `reviewer` / `wrap_up`。reviewer を含む）、`runs`（reviewer を除く）、`reviewer_runs`、`reviewer_cost_usd`、`runs_in_flight`、`input_tokens` / `output_tokens` / `tokens`（cache を除く）、`cost_usd`、`cost_usd_complete`、`quota[]`（`QuotaUse`。(source, account, window) ごと）、`first_run_started_at` / `last_run_finished_at` / `wall_ms`（壁時計: 最初の run の開始 → 最後の run の終わり）、`busy_ms`（終わった run の長さの和）、`leaves_total` / `leaves_done`、`child_tasks_total` / `child_tasks_done`、`open_decisions`。run・定価は `runs` の索引、quota は `quota_estimated` のイベントから決定的に数える（LLM は使わない）。他の `group_by` には `rollup` は出ない（互換）。

#### 3.125.6 `POST /tasks/{id}/accept` → 200 `TransitionResult` / `POST /tasks/{id}/retry` → 201 `RetryResult`（管理系）

**accept**: `draft` を `ready` にする。本文は省略可能な `ReopenBody`（`expected_status` のみ。楽観的競合検出）。既に draft でない場合は状態競合。これは `approve`（人の承認チェック）とは異なる操作。クエリは受け付けない。

**retry**（§3.63 の現行の形）: `failed` または `cancelled` のタスクを複製し、新しいタスクの `task_id` と `rewired` を返す。本文は省略可能な `RetryBody`。`accept` の既定は **`true`** で、新しいタスクは `ready` で始まる。`false` の場合だけ `draft`。`workspace` を指定すると複製先の作業場所を差し替える。`execution?: "compound" | "atomic"`（ADR-0072）は複製先の実行の形の人の明示で、複製先に `execution_hint = {mode, explicit: true}` と `execution_hint_set` イベント（`source: "human"`）を残す。省略時は元の `execution_hint` を引き継ぐ。**どちらの場合も元の gate の判定（`routing.execution`）は引き継がず**、複製先の最初の dispatch で今の `[execution] gate` の設定で判定し直し、複製先自身の `execution_gated` を残す。gate の対象外のタスクに `execution` を書くと、複製せずに 422。応答の `Location` は新しいタスクの URL。クエリは受け付けない。

#### 3.125.7 `POST /tasks/{id}/execution/decompose` → 200 `DecomposeResult`（管理系）

起票済みのタスクの実行の形を人が決め直す（ADR-0072）。本文は `DecomposeRequest`: `mode`（`ExecutionMode`: `compound` = 計画を作らせる / `atomic` = 直接実行）が必須、`note` は任意（2,000 文字まで）。`routing.execution_hint = {mode, explicit: true}` を書き、前の gate の判定（`routing.execution`）を消し、`execution_hint_set` イベント（`source: "human"`、`previous`、`previous_decision`、`note`、`replan`）を残す。次の dispatch で gate が `human/explicit` として判定し直し（新しい `execution_gated`）、`compound` なら planner run が ExecutionPlan を作る（`gate = "shadow"` でも人の明示の compound は採用される。`gate = "off"` では効かない）。計画を既に持つタスクへの `compound` は replan の依頼（`replan: true`。次の dispatch が replan の planner run、`max_replans` の範囲。`note` は planner の「起こした理由」）。応答は `task`（`Task`）、`mode`、`replan` が必須、`previous_decision`（`ExecutionGateDecision`）は消した判定があるときだけ。受け付ける状態は `draft` / `ready` / `blocked`（`blocked` は `ready` に戻った次の dispatch から効く）。管理トークンが無ければ 401、JSON の構文・型が不正（知らない `mode` を含む）なら 400、gate の対象外（`kind != execute`・対話・support-task・`routing` の無い旧タスク・固定パイプラインの harness・`workspace_mode = shared`）と長すぎる `note` は 422、不明なタスクは 404、`running` / `reviewing`（走っている run は止めない）・終端（`retry` の `execution` を使う）・計画を持つタスクの `atomic` は 409 `invalid_transition`。クエリは受け付けない。

#### 3.125.8 撤去した入口 → 410 `removed_by_adr_0079`（ADR-0079 D13 / U-R6）

案件は計画を持たず、途中目標は root task の段階で表す（既存の途中目標の行は凍結）。次の入口は**本文も id も読まずに** 410 Gone を返す（管理系のまま: トークンが無ければ先に 401）。problem は `type: "urn:celeris:problem:removed_by_adr_0079"`、`code: "removed_by_adr_0079"`、`detail`（例 `ADR-0079: 案件は計画を持たない。root task を作る`）に、`adr: "ADR-0079"` と `instead`（代わりの入口の短い説明）を添える。要求・応答の型（`ProjectPlanBody` / `ProjectPlanDecided` / `MilestoneCreateBody` / `MilestonePatchBody` / `MilestoneDecideBody` / `MilestoneLifecycle` / `NewPlanSpec` など）は `api-v1.schema.json` の互換のためにだけ残す。

| 入口 | 以前 | 代わり |
|---|---|---|
| `POST /projects/{id}/plan`（`mode: decompose` / `milestones` とも） | 202 `ProjectPlanAccepted`（案件の分解 task / 案件計画 run） | `POST /tasks` に `project_id`（root task）。段階の名指しは `stages_hint` |
| `POST /projects/{id}/project-plan/{version}/decide` | 202 `ProjectPlanDecided` | 木の中の決定は `POST /decisions/{id}/answer`、root 計画の承認は `POST /tasks/{id}/execution/plan-gate` |
| `POST /projects/{id}/milestones` | 201 `Milestone` | root task の段階（`stages_hint`、計画の `review: human`） |
| `PATCH /milestones/{id}` | 200 `Milestone` | 同上 |
| `POST /milestones/{id}/decide` | 202 `MilestoneDecided`（ADR-0038 の判定） | 段階の `review: human`（`POST /tasks/{id}/execution/phase-gate`） |
| `POST /milestones/{id}/{cancel,pause,resume}` | 200 `MilestoneLifecycle` | `POST /tasks/{id}/{cancel,pause,resume}`（subtree）・`POST /projects/{id}/{cancel,pause,resume}` |
| `POST /plans`（ADR-0028 の Plan kind） | 201 `Task`（`kind = plan`） | `POST /tasks`（root task。分解は gate と planner） |

既存の `kind = plan` の行とその子、途中目標の行、`tasks.milestone_id` はそのまま読める（`GET /projects/{id}?include_frozen=true`、`GET /tasks?milestone=`）。`celerisctl projects plan approve|reject` は削除した。

#### 3.125.9 `POST /tasks/{id}/pause` / `POST /tasks/{id}/resume` → 200 `TaskPauseResult`（管理系。ADR-0079 D13）

task の **subtree の一時停止**。本文は `{}` か空（未知の欄は 400）。`pause` は task に `paused_at` を入れ `Event::Edited{fields: ["paused_at"], by: "human"}` を残す（状態機械は触らない。replay の状態・attempts は変わらない）。以後、その task と子孫（`parent_id` の鎖と、採用で `parent_id` を書き換えない木の子〈`tree.parent_unit`〉）は `ready_tasks` に返らず dispatch されない（一時停止の後に作られた子も止まる）。**走っている run は終わるまで走る**（案件の一時停止と同じ意味）: その後 `ready` に戻っても起きず、`running` の task の並列 WU の 2 本目以降も起きない。最終レビュー（`reviewing`）と人の操作（回答・承認・中止）は止めない。`resume` は `paused_at` を消す（祖先がまだ一時停止中なら子孫は止まったまま）。応答は `task`（`TaskRef`）、`paused_at?`（RFC 3339。`resume` の後は省略）、`subtree[]`（非終端の子孫の `TaskRef`。自分は含まない）。終端の task・既に一時停止中・対話 task の `pause` と、一時停止中でない task の `resume` は 409 `invalid_transition`、不明な task は 404、トークンが無ければ 401。

案件の `POST /projects/{id}/pause|cancel` も root task の subtree に効く: 案件が paused / cancelled / archived なら、`project_id` を持たない子孫も祖先の案件で止まる（`ready_tasks` と並列 WU の判定）。案件の中止は属する task を `project_cancelled` で中止し、木の子へは `parent_cancelled` で連鎖する（ADR-0079 R1b）。

`TaskSummary`（`GET /tasks` の `items[]`）には `is_root_task` と `paused`（この task 自身の `paused_at` の有無）、`TaskDetail`（`GET /tasks/{id}`）には `is_root_task` と `paused_by?`（dispatch を止めている task: 自分か `paused_at` を持つ一番近い祖先）が付く。`Task.paused_at` は `Task` の JSON にも出る（無ければ省略）。

ADR-0090 D5: `TaskDetail.cluster_job_wait?: ClusterJobWaitView` — この task が待っているクラスタ job（`cluster_job_waits` の `waiting` の行。無ければ省略）。欄は `wait_id`、`work_unit_id?`（WU の run の wait）、`run_id`、`cluster`、`scheduler`（`pbs` | `slurm`）、`jobs[]`（`ClusterJobStatus`: `job_id`、`state` = `queued` | `held` | `running` | `exiting` | `finished` | `gone` | `unknown`、`exit_status?`、`raw_state?`。申告の順、まだ poll していない job は `unknown`）、`status_line`（`42634 (R) 42635 (Q)`）、`poll_secs`、`created_at`、`deadline`、`last_polled_at?`、`next_poll_at?`（`last_polled_at + poll_secs`）、`summary?`。wait の間の task は `blocked`（直前の遷移の reason `waiting_for_cluster_jobs`）で、受信箱の質問には出ず、`POST /tasks/{id}/answer` は 409 `invalid_transition`（trigger `cluster_job_wait_pending`）。すべての job が終われば daemon が `cluster_job_resume` で `ready` に戻す。上限（`deadline`）を過ぎると wait は `timed_out` になり、質問（延長／job の取り消し／取り下げ）が受信箱に出る。v2 / v3 の計画の unit の wait では task は `ready` のままで、unit が `blocked(cluster_jobs)`（`WorkUnitBlockedReason::ClusterJobs`）になる。`runs[].status` / `WorkerFinished.end` に `waiting` が加わった。events の `types` は `cluster_job_wait_started` / `cluster_job_wait_polled`（状態が変わった poll だけ）/ `cluster_job_wait_finished`（`state` = `satisfied` | `timed_out` | `cancelled`）を受ける。

#### 3.125.10 `GET /tasks/{id}/task-tree?root=` → 200 `TaskTreeView`

ADR-0079 D11: 再帰的な task の木と roll-up（読み取り。トークン不要）。ADR の `GET /tasks/{id}/tree` は ADR-0043 D6 の作業ツリーの閲覧（`TreeView`）が既に使っているため、パスは `task-tree`（ADR-0079 付記「R4a 実装時の逸脱・明確化」）。既定は問い合わせた task を根にした subtree、`root=true` なら木の root から。応答は `root_id`（木の root）、`subtree_root`（この view の根）、`tree_enabled`（`[execution.tree] enabled`）、`nodes[]`（`TaskTreeNode`。前順 = 親が子より先、先頭が view の根）、`totals`（view の根の subtree の合計。`nodes[0].subtree` と同じ）が必須で、`limits`（`TreeLimitsUsage`: `leaves` / `max_leaves`、`runs` / `max_runs`〈reviewer を除く〉、`replans` / `max_replans`、`tokens` / `max_tokens?`、`open_decisions` / `max_open_decisions`。`max_*` は `raise-once` / `replan` の回答の余裕を当てた値）は view の根が木の root のときだけ。各節点は `id`、`title`、`status`、`phase?`（`TreeNodePhase`: `planning` / `executing` / `repairing` / `verifying` / `awaiting_human` / `awaiting_children` / `awaiting_plan_approval` / `held_on_decision`〈節点の `self` の決定、または答えを待つ `blocked(decision)` の unit〉/ `blocked_infra`〈子の基盤の失敗の unit〉。終端・待ちの無い task は省略）、`depth`（root = 1）、`parent_id?`（view の根では省略）、`parent_unit_key?` / `parent_stage?`（この節点を作った親の unit）、`plan_version?`、`open_decisions`（この節点が出した未回答の決定）、`stall?`（ADR-0079 D10。`TreeNodeStall`: `reason`、`since?`、`detail`。節点の最後の event が `StallDetected` で終端でないときだけ = D10 の「理由なく止まっています」。何か event が積まれれば消える）、`children[]`（作られた順）、`units[]`（`TreeUnitView`: `key`、`stage?`、`kind`、`title`、`status`、`blocked_reason?`、`child_task_id?`。統合 WU と superseded を含む履歴）、`own`（自分の分）、`subtree`（自分と子孫の合計）。`own` / `subtree` は `RollupMetrics`（§3.125.5 の `group_by=depth` と同じ形）で、件数・トークン・定価・quota は和、`cost_usd_complete` は論理積、壁時計は最小の開始と最大の終わりなので、root の `subtree` は各節点の `own` の和と一致する。木の無い task（`[execution.tree] enabled = false` の旧い task を含む）は 1 節点（深さ 1）の木。不明な task は 404 `task_not_found`、知らないクエリ・真偽値でない `root` は 400。`GET /tasks/{id}/execution` の `metrics` は自分の分のまま（互換）。

#### 3.125.11 `POST /tasks/{id}/execution/phase-gate` → 200 `TransitionResult`（管理系）

途中確認中の Task を判定する。本文は `PhaseGateRequest` で、`action`（`PhaseGateAction`: `continue`、`replan`、`withdraw`）が必須。`note` は `continue` では任意の次工程への指示、`replan` では空白以外の文字が必要。応答の `id`、`from`、`to`、`reason` は必須で、`cascaded[]` は既定で空配列。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の replan note は 422、不明な Task は 404、Task が `awaiting_human` でなければ 409 `invalid_transition`。クエリは受け付けない。

#### 3.125.12 `POST /tasks/{id}/execution/plan-gate` → 200 `TransitionResult`（管理系。ADR-0079 D8）

root の /3 の計画が人の承認を待っている Task（`blocked` で直前の遷移の reason が `awaiting_plan_approval`。`GET /tasks/{id}/execution` の `phase = awaiting_plan_approval`、`plan_approval`: `PlanApprovalView`〈`plan_id`、`reasons`、`summary`〉）を判定する。承認を求めるのは、計画の採用の時点で決定を含む（`decisions:<key>,…`）・`review: human` の段階がある（`review_human:<stage>`）・上限の `approval_near_limit_ratio`（既定 0.8）以上（`near_limit:<設定名>:<値>/<上限>`。段階数・段階あたりの unit・子 task・見込みの leaf〈leaf + 子 task × 4〉・見込みの木の run）のどれかのとき（root の replan の版にも同じ規則。子の計画は求めない）。本文は `PlanGateRequest`: `action`（`PlanGateAction`: `approve` / `replan` / `withdraw`。`decision` は `action` の別名）が必須、`note` は `approve` では任意（次の run に「計画の承認（ADR-0079 D8）」として渡る）、`replan` では空白以外の文字が必要（planner への指示。2,000 文字まで）。`approve` は `PhaseResume{plan_approve}`（reason `plan_approved`）で unit が起き始める（決定への回答は別。答えの無い決定に依存する unit は待つ）、`replan` は reason `plan_replan` と `ExecutionHintSet{replan: true, source: "human (plan-gate)"}` で次の dispatch が replan の planner run（`max_replans` に数える。新しい版にも同じ承認の規則）、`withdraw` は `Cancel`（subtree に連鎖）。応答の `id`、`from`、`to`、`reason` は必須。管理トークンが無ければ 401、JSON の構文・型が不正（知らない `action`）なら 400、空の replan note・長すぎる note は 422、不明な Task は 404、Task が `awaiting_plan_approval` でなければ 409 `invalid_transition`。承認待ちの Task への `POST /tasks/{id}/answer` も 409。クエリは受け付けない。MCP では `task_plan_gate`（scope `tasks:interact`、`by = mcp:<client_id>`）。

`GET /inbox`: 承認待ちの root は `questions` ではなく `attention[]` の `type: "plan_approval"`（`task`〈`actions` に `plan_gate`、`answer` は無い〉、`plan_id`、`plan_version`、`reasons`、`summary`、`stages[]`〈`key`・`title`・`review_human`・`units[]`〉、`decision_ids[]`〈同じ節点の未回答の決定。`decisions[]` の節にも出る〉、`at`）に出る（`counts.attention` に数える）。通知は `plan_approval`（key `plan:<plan_id>:approval`、その計画の決定を 1 通に束ねる。`decision_requested` の `plan:<plan_id>:decisions` は鳴らさない）。承認の要らない root の計画は報告の流れ（`GET /reports`）に `kind: progress` の「計画を採用して進めます: <段階の一覧>」を 1 件残すだけで、通知しない。

#### 3.125.13 決定の要求（ADR-0079 D7）

人への決定の要求（`DecisionRequest`。計画の `decisions`・worker の `result.json` の `decisions`・daemon の `leaf_too_large` / `limit` / `plan_invalid`）の一覧と回答。`[execution.tree] enabled = false`（既定）では決定が作られないので、一覧は空（404 ではない）、回答は 404 になる。効き目（選択肢 → 効き目の表）は ADR-0079 付記「R3a 実装時の逸脱・明確化」。MCP では `decision_list` / `decision_answer`（scope `tasks:interact`、`docs/guides/mcp.md`）。

##### `GET /decisions?open=&root_id=` → 200 `DecisionList`

`items[]` は `DecisionView`（`decision`: `DecisionRequest`、`task_id` = 決定を出した節点、`root_id`、`created_at`、`answered_at?`、`effect?` = 回答済みならその効き目 `DecisionEffect`: `resume` / `raise_once` / `replan` / `atomic` / `withdraw`）。`created_at` 昇順。`open=true` は未回答だけ、`false` は回答済み・取り下げ済みだけ、省略は全件。`root_id` で 1 つの木（root task の id）に絞る。不正な `open` / `root_id` は 400。

##### `GET /tasks/{id}/decisions?open=` → 200 `DecisionList`

その task の subtree（その task が出した決定と、`path` にその task を含む子孫の決定）。不明な Task は 404 `task_not_found`。

##### `POST /decisions/{id}/answer` → 200 `DecisionOutcome`（管理系）

本文は `DecisionAnswerBody`: `option?`（決定の `options[].key` のどれか）、`note?`（2,000 文字まで。依存する仕事の入力に固定の書式で入る）。`kind = choice` の決定だけ `option` を省いて `note` に自由記述で答えられる（記録される `option` は `other`）。1 トランザクションで `DecisionAnswered{by: "human"}`、表の更新、待っていた unit の再評価（`blocked(decision)` → `pending` / `ready`、取り下げなら `cancelled`）、効き目の event（replan の依頼 = `ExecutionHintSet{replan: true}`、atomic の run の `self` への答え = `Answered`）を書く。`needed_before: [self]` の取り下げは続けて節点を中止する。応答は `decision`（回答後の `DecisionView`）、`effect`、`resumed[]`、`cancelled[]`（unit の key）、`replan_requested`、`cancelled_task?`。管理トークンが無ければ 401、JSON が不正なら 400、無い id は 404 `decision_not_found`、`open` でない・決定を出した節点が終端なら 409 `decision_not_open`（`decision_status` を添える）、選択肢の外・daemon の決定で `option` 無し・note が長すぎるなら 422。

##### `POST /decisions/{id}/withdraw` → 200 `DecisionOutcome`（管理系）

人が決定を取り下げる（`DecisionWithdrawn`）。本文は省略可能な `DecisionWithdrawBody`（`reason?`）。効き目は選択肢の `withdraw` と同じ（止めていた unit〈`needed_before` の unit・`stage:<key>` の unit・その決定を `needs_decisions` に持つ unit〉と、それに依存する未着手の unit を `cancelled`。`needed_before: [self]` なら節点を中止）。`open` でなければ 409、無い id は 404。

##### `POST /decisions/{id}/revise` → 200 `DecisionOutcome`（管理系）

回答済みの `choice` の決定の答えを変える（新しい `DecisionAnswered`。最後の回答が有効）。本文は `DecisionAnswerBody`。これから作られる子・これから走る leaf は新しい答えを読む。既に作られた非終端の子は作り直さず、node のコメント（人を起こさない）で新しい答えを届け、応答の `notified_children[]` に並ぶ。未回答・取り下げ済み・daemon の決定（回答の時点で効き目を当てたもの）は 409。

##### `GET /inbox` の `decisions` と `counts.decisions`

受信箱に `decisions[]`（`DecisionInboxItem`: `id`、`key`、`kind`、`task_id`、`root_id`、`path`〈パンくず〉、`question`、`options`、`recommended`、`cost_of_reversal`、`cost_note?`、`needed_before`、`origin`、`created_at`、`age_secs`）と `counts.decisions` が付く。未回答で、決定を出した節点が終端でないものだけ（古い順）。`GET /daemon` の `snapshot.decisions_open` は同じ件数（API が応答を組むときに埋める）。

#### 3.125.14 案件の詳細と編集の現行の形（§3.47・§3.48 への追加。ADR-0079 D13 / ADR-0072 F6）

##### `GET /projects/{id}?include_frozen=` → 200 `ProjectDetail`

案件詳細。`project`（`Project`）、`milestones[]`（`MilestoneView`）、`tasks[]`（`ProjectTaskView`）は必須で、`repos[]`（`ProjectRepo`）は既定で空配列。

**ADR-0079 D13 / U-R8: 途中目標は凍結した履歴**。既定（`include_frozen` 省略・`false`）では `milestones` は空配列で、`project_plan` も出ない。`milestones_frozen`（`u32`、既定 0）はこの案件の途中目標の行の数（隠していても数える。GUI の「以前の途中目標 N 件」用）。`milestones_frozen_open`（`u32`、既定 0、ADR-0079 R6-4）はそのうち終端（`reached` / `redesigned` / `cancelled`）でないまま凍結した行の数。`?include_frozen=true` のときだけ全行を読み取り専用で返し（`MilestoneView`: 秘書のレビューの返事と提案を添えたもの）、案件計画の版があれば `project_plan`（`ProjectPlanDagView`: `nodes[]`〈`PlanDagNode`〉が必須、`current_version` と `pending`〈`PlanDagProposal`〉は省略可能）も返す。行は消さず状態も変えない（書き込みの入口は §3.125.8 の 410）。真偽値でない `include_frozen` と知らないクエリは 400。

`tasks[]` の各行には `is_root_task`（`boolean`、既定 `false`。`task_core::is_root_task`: 案件直下〈`parent_id` なし〉・木の子〈`tree.parent_unit`〉でない・対話でも裏方〈`support_kind`〉でもない）が付く。案件ページの root task の一覧はこれで絞る。`root_totals`（`ProjectRootTotals`。ADR-0079 D11）は同じ述語の root task の `root_tasks`（数）、`by_status`（状態ごとの数。0 件の状態は出ない）、`totals`（root task ごとの subtree の roll-up の和。`RollupMetrics`。run・reviewer の run・トークン・定価・leaf・未回答の決定・壁時計。**quota は数えない**〈events を読まない。quota は `task-tree` と `metrics/execution` で見る〉）。範囲は `tasks[]` と同じ上限（2,000 件）。`project.auto_advance` は `boolean` で常に読める（R5a からは書けず、読まない列）。`project.slug` は知識ベースでのこの案件の置き場 `projects/<slug>/`（ADR-0047。作るときに題名 → primary リポジトリの名前 → id の末尾から決まり、案件の間で一意）。管理トークンは不要。不明な案件は 404。

##### `PATCH /projects/{id}` → 200 `Project`（管理系）

本文は `ProjectPatchBody`。`auto_advance` は **ADR-0079 D13 で廃止**: 値が `true` でも `false` でも（他の欄と一緒でも）422 `validation`（`field: "auto_advance"`。列 `projects.auto_advance` は残すが書かない・読まない）。同じ本文には `status` と `workspace` も指定できる。`slug?: string` は知識ベースの置き場 `projects/<slug>/` の slug を変える（小文字の `[a-z0-9-]`、1〜64 文字、先頭・末尾・連続の `-` と案件 ID の形は不可 → 422。他の案件が使っていれば 409 `project_slug_in_use`）。**KB のディレクトリは動かさない**（`projects/<旧>/` は人が動かす）。`title?: string` は案件の名前、`request?: string` は案件の説明（依頼文。GUI の「依頼文」）を変える（ADR-0072P3。前後の空白を除いて保存し、空は 422、`title` は 200 文字・`request` は 20,000 文字まで）。値が変わった欄だけを書き、管理系のログに `op = "project_updated"` と変えた欄の名前を残す（案件には events の列が無い）。説明を変えても CoS への再依頼にはならない。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の変更指定や許されない状態変更は 422、不明な案件は 404。クエリは受け付けない。

#### 3.125.15 `POST /tasks` の `stages_hint`（ADR-0079 D12）

`NewTaskSpec.stages_hint?: StageHint[]`（`{title: string, scope?: string}`。未知の欄は 400）。人（API・CLI）と CoS（`create_task.stages_hint`）が名指しした段階の名前と範囲で、そのまま `Task.routing.stages_hint` に入り、root の planner への入力になる（構造の強制ではない。子は継がない）。16 件まで、`title` は空白以外の 1〜120 文字、`scope` は 2,000 文字まで（違反は 422）。省略時は空で、`routing` の JSON にも出ない。


### 3.126 §2 の追加 route

以下は §2 の一覧にある route のうち、上の節に独立した説明が無かったもの。
読み取りは通常の認証、管理系は `token_file` が無くても bearer token を要求する。

#### 3.126.1 `GET /tasks/{id}/routing` → 200 `TaskRoutingView`

`task_id`、`assignee`、`routing`、`runs[]` を返す。run ごとの routing 監査はイベントから組み立てる。
クエリは受け付けず、不明な task は 404 `task_not_found`。`routing.rs` を参照。

#### 3.126.2 `POST /tasks/{id}/rereview` → 200 `TransitionResult`（管理系）

省略可能な `ReopenBody.expected_status` を受け付ける。reviewer 条件を持つ通常の task の
最終レビューをやり直す。`failed` の場合は直前の実装 run が成功し、最終レビューの不合格だけが
失敗の理由である必要がある。不明な task は 404、状態競合は 409、対象外は 422。

#### 3.126.3 `PUT /clusters/{id}/settings` → 200 `ClusterSettingsView`（管理系）

本文 `ClusterSettingsPutBody.work_dir` は絶対パス・`~`・`~/…` のいずれか。
`null` または省略で DB の上書きを消す。応答は `cluster_id`、`work_dir`、`updated_at`。
不明な cluster は 404、不正な path は 422。クエリは受け付けない。

#### 3.126.4 `GET /projects/{id}/docs/maintenance` → 200 JSON

案件の文書リポジトリの `audit`、`proposal`、`policy`、`saved_report` を返す。
文書の根が使えない場合は Problem を返す。クエリは受け付けない。

#### 3.126.5 `POST /projects/{id}/docs/maintenance` → 200 JSON（管理系）

本文は `op` で区別する `MaintenanceAction`。`audit` は監査と提案、`adopt` は `policy`、
`approve` と `apply` は `plan` が必要。応答は順に `{audit,proposal}`、`{policy}`、
`{approved,plan}`、`{sha,worktree,merged,task_id}`。適用は隔離 worktree に置き、
検証用 task を作る。クエリは受け付けず、実行できなければ 409 `docs_maintenance`。

#### 3.126.6 `GET /metrics/scratch` → 200 `ScratchStatus`

直近の daemon snapshot の `scratch` をそのまま返す。未公開または shared build cache が
無効なら 404 `scratch_unavailable`。クエリは受け付けない。

#### 3.126.7 `GET /tasks/{id}/browser/policy` → 200 `{policy}`

保存した `BrowserTaskPolicy`（無ければ `null`）を返す。不明な task は 404。
クエリは受け付けず、応答は `Cache-Control: no-store`。

#### 3.126.8 `PUT /tasks/{id}/browser/policy` → 200 `{updated: true}`（管理系）

本文は `BrowserTaskPolicy` の JSON。型や内容が不正なら 422 `browser_body_invalid`、
状態が変更を許さなければ 409 `browser_policy_state`。クエリは受け付けない。

#### 3.126.9 `GET /browser/identities` → 200 `{identities: IdentityView[]}`

`project_id` クエリが必須で、欠落・余分なクエリは 400 `identity_query_invalid`。
封緘した state は返さず metadata だけを返す。封緘の設定が無ければ 503 `identity_unavailable`。

#### 3.126.10 `POST /browser/identities` → 201 `{identity: IdentityView}`（管理系）

本文 `IdentityRegisterInput` は `identity_id`、`project_id`、`origin`、`demand_confirmed_by`、
`state` と任意の `ttl_secs`。需要を確認した人が無ければ拒否する。本文不正は 400
`identity_body_invalid`。state は封緘して保存し、応答には含めない。

#### 3.126.11 `DELETE /browser/identities/{id}` → 200 `{identity: IdentityView}`（管理系）

対象を削除し、削除後の metadata を返す。不明な id は 404 `identity_not_found`。

#### 3.126.12 `POST /browser/identities/{id}/revoke` → 200 `{identity: IdentityView}`（管理系）

対象を失効させ、失効後の metadata を返す。不明な id は 404 `identity_not_found`。

#### 3.126.13 `POST /browser/identities/{id}/restore` → 204（管理系）

本文は `project_id`、`origin`、任意の `session_id`。稼働中の隔離 session とその場の
attestation が一致した場合だけ、開封した state を controller に渡す。条件を満たさなければ
403 `isolation_required` などの固定コードで拒否し、平文は返さない。

#### 3.126.14 `POST /tasks/{id}/browser/live/{run}/{session}/grant` → 200 `GrantResponse`

本文は GUI 署名の `{assertion}`。閲覧を許可すると `{grant_id,expires_at}` を返す。
通常の bearer に加えて署名と owner session を検証する。

#### 3.126.15 `POST /tasks/{id}/browser/live/{run}/{session}/check` → 200 `CheckResponse`

本文は `{assertion,grant_id}`。閲覧可能なら `{connected:true}`。同じ許可条件を毎回検証する。

#### 3.126.16 `POST /tasks/{id}/browser/live/{run}/{session}/read` → 200 `ReadResponse`

本文は `{assertion,grant_id}`、クエリは任意の `after`。`plan` と `events[]` を返す。
許可と session を検証してから最大 1,000 件を読む。

#### 3.126.17 `POST /tasks/{id}/browser/live/{run}/{session}/events` → 200 `EventResponse`

daemon bearer が使う。本文は `kind` が `status` / `tabs` / `url` / `console` の
イベント。run が稼働中であることを確認し、秘密を除去して記録した `seq` を返す。

#### 3.126.18 browser control の 6 route

`{run}` と `{session}` は稼働中 browser session を指定する。人の変更系は GUI 署名の
assertion で owner session を確認し、worker の route は daemon bearer を要求する。

| route | 成功時の応答 | 本文・要点 |
|---|---|---|
| `GET /tasks/{id}/browser/control/{run}/{session}` | 200 `ControlStatus` | worker が phase・version・lease・操作可能状態を読む |
| `POST /tasks/{id}/browser/control/{run}/{session}` | 200 `ControlOutcome` | `assertion`、`command`（pause / takeover / renew / resume / stop）、`expected_version`、`idempotency_key` |
| `POST /tasks/{id}/browser/control/{run}/{session}/disconnect` | 200 `ControlStatus` | `{assertion}`。人の接続を切る |
| `POST /tasks/{id}/browser/control/{run}/{session}/agent/begin` | 200 `ControlStatus` | worker が操作開始を記録する |
| `POST /tasks/{id}/browser/control/{run}/{session}/agent/end` | 200 `ControlStatus` | worker が操作終了を記録する |
| `POST /tasks/{id}/browser/control/{run}/{session}/auth-section` | 200 `ControlStatus` | worker が `{active?: boolean}` で認証区間を始める・終える。既定 `true` |

署名や bearer が使えない構成は 403、状態や版の競合は 409、本文の不正は 422 を返す。


## 4. SSE `GET /stream`

```
event: hello
data: {"cursor":12345,"now":"…","daemon":{…DaemonSnapshot…}}

event: task.event
id: 12346
data: {"id":12346,"task_id":"01J…","seq":7,"ts":"…","event":{"type":"transitioned","from":"ready","to":"running","reason":"dispatch"}}

event: daemon
data: {…DaemonSnapshot…}

event: heartbeat
data: {"now":"…"}

event: reset
data: {"reason":"cursor_too_old","cursor":20000}
```

| 事項 | 決め |
|---|---|
| 応答ヘッダ | `Content-Type: text/event-stream; charset=utf-8`、`Cache-Control: no-store`、`X-Accel-Buffering: no`。`hello`（と必要なら `reset`）は接続直後に送る |
| 再開 | `Last-Event-ID` ヘッダ（`EventRow.id`）または `?after_id=`（ヘッダが優先）。省略時は「今」（`TaskStore::latest_event_id()`）から（過去は送らない）。`hello.cursor` が送信開始位置（この id より後を送る）。数値でない `Last-Event-ID` は 400 `bad_request` |
| 取りこぼし | `最新 id − 要求 id > 10,000`（`STREAM_RESET_THRESHOLD`）なら `reset{reason: "cursor_too_old"}`、要求 id が最新より大きい（DB が入れ替わった）なら `reset{reason: "cursor_ahead"}` を `hello` の直後に送り、`cursor` = 最新 id から続ける。クライアントは全体を再取得する |
| `task.event` | 接続ごとの購読ループが `events_since(cursor, 1000)`（`STREAM_BATCH`）を **250 ms 間隔**（`STREAM_POLL_INTERVAL`）でポーリングし、1,000 件読めたら追いつくまで続けて読む。`id:` 行に `EventRow.id`、`data` は `EventRow`。`?task_id=` で 1 タスクに絞る（`hello.cursor` は絞らない）。DB エラーは次のポーリングで再試行する |
| `daemon` | デーモンのスナップショット（`watch`）が変わるたび（= 毎 tick）に `DaemonSnapshot`。値が `null` の間は送らない（`hello.daemon` は `null` になりうる）。`?task_id=` があっても送る |
| `heartbeat` | 15 秒ごと（`STREAM_HEARTBEAT_INTERVAL`）。`StreamHeartbeat{now}` |
| クエリ | `after_id` と `task_id` だけ。他は 400 |
| 接続数 | 定数 16（`MAX_STREAMS`、設定キーにしない）。超過は 503 `too_many_streams` |
| 認証 | 他の読み取りと同じ（Bearer / Host） |
| 終了 | クライアントが切ればサーバは購読ループを止める。celeris の停止時は接続を閉じる |

`data` は 1 行の JSON（`event: <name>\n[id: <id>\n]data: <json>\n\n`）。型は `StreamHello` / `EventRow` / `DaemonSnapshot` / `StreamHeartbeat` / `StreamReset`（§6）。
`GET /console/stream` は同じ枠組み（`hello` / `heartbeat` に加えて `console.block`）を使う別の stream（§3.99）。

クライアント（BFF）の規約: `task.event` を受けたら該当画面のデータを**再取得**する（イベント本体から状態を組み立てない。真実は DB）。`celerisctl` による書き込みも同じ経路で流れる（in-process 通知は使わない。ADR-0013 D6）。

---

## 5. 派生値の計算規則（task-ops / task-api）

**`crates/task-ops`** の関数として実装し、`celerisctl show --json` / `celerisctl` の各コマンド / `task-api` / ディスパッチャが同じ関数を使う。GUI は結果を表示するだけ（規則を再実装しない）。関数はモジュールの下にある（`lib.rs` から再輸出していない）。

### 5.1 受信箱（`task_ops::inbox::inbox(store, snapshot, ctx: &ViewContext, now, evidence)`）

`Inbox { approvals, questions, drafts, attention, browser_waits, decisions, counts }`。`evidence` は task-api が渡す関数で、`<ws>/runs/<run_id>/result.json` の `evidence[]` を読む（読めなければ `[]`）。

| 区画 | 抽出 | 各項目の埋め方 | 並び |
|---|---|---|---|
| `approvals[]` | `kind == approval && status == ready` | `parent` = `parent_id` のタスク（無ければ `null`）。`criterion_idx` / `attempt` は title を `Approval needed: <title> — criterion <idx> (attempt <n>)` として解析（`view::parse_human_approval_title`）。解析できなければ `null`。`criterion_text` = 親の `acceptance[idx].text`（無ければ approval の `objective`）。`requested_at` = `ApprovalRequested` の `ts`（無ければ `created_at`）。`last_run` = 親の `last_run_id` の `RunSummary`。`evidence` = その run が `done` のときだけ。`other_verdicts` = 親の同 run の `ReviewVerdict`。`artifacts[]` = `ApprovalArtifact{idx, ...ArtifactRef}`（`ArtifactRef` を flatten。`idx` は `GET /tasks/{parent}/artifacts/{idx}` の添字）。`knowledge_pages[]` = 親の `Check::KnowledgePage`。`previous_decisions` = 親の他の Approval 子（同じ `criterion_idx`）の `ApprovalDecided` | `requested_at` 昇順 |
| `questions[]` | `status == blocked` のうち、browser の wait・クラスタの job 待ち・工程の途中確認（phase gate）・計画の承認（plan gate）で止まっているものを除く | `question` = §5.5。`asked_at` = その `WorkerFinished`（または `QuestionRaised`）の `ts`。`run_id` = 同。`previous` = `answers_from_events`。`approval_id`（そのタスクに保留中の承認があるとき） | `asked_at` 昇順 |
| `drafts[]` | `status == draft` を `parent_id` でまとめる。案件計画の提案は別のグループ（`project_plan: {project_id, version, supersedes}`） | `parent` = Plan 等（`null` = 根）。`plan_summary` = 親の直近 `WorkerFinished.outcome` が `done: ` 始まりならその後ろ。`drafts` = `TaskSummary` | 親の `created_at` 昇順 → 案件計画の提案 → 根 |
| `attention[]` | `type` で分かれる tagged union（`snake_case`）: `failed`（`updated_at >= now − 24h`）、`requeue_limit_near`（`ready && max_requeues > 0 && consecutive_requeues > 0 && consecutive_requeues >= max_requeues − 1`）、`unroutable`（スナップショット）、`cluster_unavailable`（`WorkspaceSpec::Remote` で終端でないタスクの `ClusterUnavailable` が `ts >= now − 24h` にあるクラスタ。**クラスタごとに 1 件**。`clusters[].connected == true` なら出さない）、`phase_checkpoint`（工程の途中確認待ち。ADR-0074）、`plan_approval`（root の計画の承認待ち。ADR-0079 D8）、`delivery_skipped`（取り込みを見送った root。ADR-0051） | `failed`: `reason` = 直近 `WorkerFinished.outcome` と直近 run の fail の `ReviewVerdict.reason` を `; ` で結合、`class`（`infra` / `work`）、`delivered_release`。`requeue_limit_near`: `count` / `max`。`unroutable`: `hint` = `worker_hint`、`at` = `last_tick_at`。`cluster_unavailable`: `cluster` / `host`（空なら `clusters[].host`）/ `at` = 最新の `ts` / `tasks` = 該当タスク数 | `at` 降順 |
| `browser_waits[]` | 人の対応（credential の登録・一回だけの承認・拒否）を待つ browser の wait（ADR-0080 D5） | `task_ops::browser::BrowserWaitItem` | |
| `decisions[]` | 未回答の決定の要求（ADR-0079 D7）。回答は `POST /decisions/{id}/answer` | `task_ops::decision::DecisionInboxItem` | |
| `counts` | 各区画の要素数 + `by_status`（status 名 → 件数、DB 全体） | `drafts` は **draft タスクの件数**（グループ数ではない） | |

### 5.2 run の要約（`task_ops::view::runs(rows: &[EventRow]) -> Vec<RunSummary>`）

- `WorkerStarted{run_id, adapter, model, provider}` で開始（`started_at` = `ts`）。同じ `run_id` の `WorkerProgress` を `progress` に、`ArtifactProduced` を `artifacts` に、`ReviewVerdict` を `verdicts` に数える。
- `WorkerFinished{run_id, outcome, end, usage}` で終了（`finished_at` = `ts`）。`RunOutcomeKind` は、構造化された `end` が `Yielded` / `BudgetExhausted` なら `continued`、それ以外は `outcome` の**接頭辞**で決める（ディスパッチャの文字列と対）:
  - `done: ` → `done`
  - `question: ` → `question`
  - `requeue: ` / `infra_requeue: ` → `requeue`
  - `lease_expired`（完全一致）→ `lease_expired`
  - `interrupted: ` → `interrupted`（人のコメントで止めた run。ADR-0044 D2）
  - `continue: ` → `continued`（予算切れ・yield の続き。ADR-0072 D9/D11）
  - それ以外（`error(retryable=…): …`）→ `error`
- `interrupted` と `continued` は失敗ではないので、`bad_news`（ADR-0034）にも `error_cooldown` にも §5.8 の集計にも数えない。
- `outcome_text` は接頭辞を除いた残り（`error` は文字列全体、`lease_expired` は `null`）。
- `WorkerFinished` が無い run は `finished_at = null, outcome = null`（実行中、または回収前）。
- Reviewer run も `WorkerStarted` / `WorkerFinished`（`role: "reviewer"`）を持つので一覧に現れ、`RunSummary.role` が `reviewer` になる（ADR-0014 D1。`role` の無いイベントは `worker`）。Reviewer run の進捗は対象 run に `reviewer run <id>: ` 接頭辞で付き、`reviewer run requeued: ` で始まるものは対象 run の `reviewer_deferrals` に数える。

### 5.3 タイマー（`task_ops::view::timers(task, rows, ctx: &ViewContext, now)`）

- `lease_expires_at` = `task.lease.expires_at`（`running` のとき。`renew_lease` はイベントを出さないので、GUI は `running` の詳細を 5 秒ごとに再取得する）。
- `backoff_until` = `status == ready && attempts > 0` のとき `updated_at + retry_backoff(base, max, attempts)`（`min(base·2^(attempts−1), max)`。`base = 0` なら `null`）。過去なら `null`。
- `consecutive_requeues`（`Transitioned` を新しい順に見て `requeue` を数え `dispatch` は読み飛ばす）/ `consecutive_reviewer_requeues`（最後の `Transitioned` 以降の `REVIEWER_REQUEUED_PREFIX` を数える）は `task_ops::derive` にある。`retry_backoff` / `artifacts_for_run` / `last_run_id` / `latest_question` / `answers_from_events` も同じモジュール。
- `max_requeues` は設定値（`ViewContext`）。

### 5.4 可能な操作（`task_ops::view::actions(task)` / `actions_with_events(task, events)`）

`actions(task)`: `approve`: `status == draft` または `kind == approval && status == ready`。`reject`: `kind == approval && status == ready`。`answer`: `status == blocked`。`cancel`: 非終端。`retry`（§3.63）: `status == failed` または `cancelled`。`edit`（ADR-0044 D1。§3.74）: 非終端。`reopen`（ADR-0044 D2。§3.77）: `status == done` または `failed`（`cancelled` には付かない）。

`actions_with_events(task, events)` は上に events が要る 3 つを足す: `rereview`（`done`、または直前の遷移が `review_fail` の `failed`。ADR-0070 D2）、`phase_gate`（`blocked(awaiting_human)`。ADR-0074 D2.4）、`plan_gate`（`blocked(awaiting_plan_approval)`。ADR-0079 D8）。後の 2 つのときは `answer` を出さない。

`TaskDetail.actions` と受信箱の `attention[]` の `task.actions` は `actions_with_events`、`TaskRef` / `TaskSummary` の `actions` は `actions(task)`（ADR-0015 D4）。GUI は `actions` を見るだけでよい。一括承認の API は無い（GUI が 1 件ずつ `POST /tasks/{id}/approve` を呼ぶ。原子性は無い）。

### 5.5 質問文（`task_ops::derive::latest_question(events)`）

events を後ろから見て最初に見つかった質問。次の 2 つを同じように扱う（Reviewer run の `WorkerFinished` は飛ばす）:
- `WorkerFinished{outcome}` が `"question: "` で始まるもの（ワーカーが聞いた。接頭辞を除いた文字列）
- `QuestionRaised{text}`（ディスパッチャが聞いた。ADR-0021 D2）

無ければ空文字列。

### 5.6 検証（`task_ops::add::create_task` / `task_ops::plan::create_plan` の中）

文言は §3.4 / §3.14 のとおり（`OpsError::Validation` → 422）。`title` / `objective` の空白、`parent` / `depends_on` / `project` の存在を検査する（ADR-0014 D3）。`celerisctl add` / `plan` も同じ関数を通す。テーブル駆動テストは `crates/task-ops/src/add/tests.rs` / `plan/tests.rs`。

### 5.7 `TransitionResult.cascaded`（`task_ops::gate`。`#[serde(default)]`）

`apply_transition` の前の最新 event id を控え、後で増分を読んで、遷移対象以外の `task_id` に付いた `Transitioned{to: cancelled}` のうち `reason` が `cancel` / `dependency_failed` / `parent_cancelled` のものを `TaskRef` にして返す（同時に他の書き込みが挟まっても過剰には含まれない）。

### 5.8 プロバイダの集計（`task_api::stats`。task-ops ではなく task-api のメモリ）

- 最初の `GET /providers`（または `GET /accounts`）で `events_since(0, 5000)` を繰り返して全イベントを走査し、以後は要求のたびに増分だけ読む。**メモリ内の観測値**で、真実ではない（再起動で再計算）。
- `WorkerStarted{run_id, provider}` で run 表に `provider`（`null` なら `"unknown"`）を登録し、`WorkerFinished{run_id, outcome, usage}` で閉じる。分類は §5.2（`continued` / `interrupted` はどの区分にも数えない）。`input_tokens` / `output_tokens` は `usage` の和（`null` は 0）。
- `runs` は `WorkerStarted` の数（実行中を含む）。`by_day` は `WorkerFinished.ts` の UTC 日付で今日を含む直近 30 日、`by_day[].runs` はその日に終わった run の数。
- Reviewer run（`role: reviewer`）も同じ規則で集計に**含める**（ADR-0014 D1）。
- アカウント別（`GET /accounts` の `stats`）は `WorkerStarted.account` と `adapter` の組で同じ規則。

---

## 6. 型

### 6.1 型の出所

型の正は Rust の定義と、そこから生成した `docs/api/v1/api-v1.schema.json`（§7）。この文書には欄の一覧を写さない（写しは古くなる）。欄を知りたいときは schema の `$defs/<型名>` か下のファイルを読む。

| 出所 | 主な型 |
|---|---|
| `crates/task-core/src/`（`model.rs`・`store/` ほか） | `Task`, `TaskId`, `TaskKind`, `Status`, `Tier`, `WorkerHint`, `WorkspaceSpec`, `Check`（`KnowledgePage` を含む）, `Criterion`, `ArtifactRef`（`declared`、既定 `true`）, `Usage`, `Event`, `EventRow`, `ListFilter`, `Page<T>`、案件・リポジトリ・取り込み・delivery・routing の監査などの永続の型（`repos.rs`, `integrations.rs`, `delivery.rs`, `routing_audit.rs` …） |
| `crates/task-ops/src/view.rs` | `TaskRef`, `TaskSummary`, `TaskList`, `TaskDetail`, `Timers`, `CriterionView`, `VerdictView`, `RunSummary`, `RunFiles`, `RunOutcomeKind`, `Action`, `ViewContext` |
| `crates/task-ops/src/inbox.rs` | `Inbox`, `InboxCounts`, `ApprovalItem`, `ApprovalArtifact`, `EvidenceView`, `QuestionItem`, `DraftGroup`, `AttentionItem` |
| `crates/task-ops/src/daemon.rs` | `DaemonSnapshot`, `InFlight`, `InFlightKind`, `CooldownView`, `ProviderLive`, `AccountLive` ほか（task-dispatch が作り task-api が読むので、両者が依存する task-ops に置く。ADR-0013 D3/D4） |
| `crates/task-ops/src/` のその他（`add.rs`, `plan.rs`, `gate.rs`, `graph.rs`, `replay.rs`, `retry.rs`, `decision.rs`, `tree_view.rs`, …） | `NewTaskSpec`, `NewPlanSpec`（`POST /tasks` / `POST /plans` の本文そのもの。`deny_unknown_fields`）, `TransitionResult`, `Graph`, `ReplayReport`, `RetryResult`, `DecisionInboxItem` ほか |
| `crates/task-api/src/types.rs` | HTTP の要求・応答の包み（`Health`, `Problem`, `DecisionBody`, `AnswerBody`, `CancelBody`, `ReopenBody`, `EventsPage`, `RunList`, `ArtifactList`, `Providers`, `DaemonView`, `ConfigView`, `StreamHello`, `StreamHeartbeat`, `StreamReset`, `AccountList`, `SecretList`, `TaskRoutingView` …） |
| `crates/task-api/src/` のその他（`approvals.rs`, `browser.rs`, `knowledge.rs`, `skills.rs`, `docs.rs`, `project_plan.rs`, `reports.rs`, `console.rs`, …） | その module の endpoint だけが使う要求・応答 |

### 6.2 表記の約束

- 全ての公開型が `JsonSchema` を derive し、`crates/task-api/src/schema.rs` の `ApiV1Schema`（1 フィールド = 1 公開型）から辿れる。
- 列挙は serde の表現そのまま（§1）。tagged union（`Event`, `Check`, `AttentionItem` は `type`、`WorkspaceSpec` は `kind`）。
- 要求本文の型は多くが `#[serde(deny_unknown_fields)]`（未知フィールドは 400）。応答の任意の欄は `skip_serializing_if` で省かれることがある（GUI は欠落と `null` を同じに扱う）。

---

## 7. スキーマファイルと生成

| 事項 | 決め |
|---|---|
| ファイル | `docs/api/v1/` に 2 つ。**(1) `event.schema.json`**（ルート `EventRow`。task-core の `store/tests.rs` が一致を検証。`UPDATE_SCHEMA=1 cargo test -p task-core` で再生成）。**(2) `api-v1.schema.json`**（ルート `ApiV1Schema`、`Event` / `EventRow` / `Task` / `DaemonSnapshot` / `Status` などの共有型は `$defs` に 1 回だけ現れる）。**GUI の型生成は (2) だけを読む**（(1) は celeris 自身の契約・テスト用） |
| 生成 | `UPDATE_SCHEMA=1 cargo test -p task-api`（`schemars::schema_for!(ApiV1Schema)`、整形は `serde_json::to_string_pretty`、末尾改行 1 つ）。task-core（`event.schema.json` ほか）と task-worker（`docs/protocol/`）の schema も同じ `UPDATE_SCHEMA=1 cargo test -p <crate>` で再生成する |
| 一致テスト | `crates/task-api/src/schema.rs` の `committed_schema_matches_generated`（生成結果 == コミット済みファイル。差分があれば `schema drift: run UPDATE_SCHEMA=1 cargo test -p task-api` で失敗）と `schema_uses_defs_once_for_shared_types`（共有型が `$defs` にあり `$dynamicRef` を使わない） |
| `GET /schema` | コミット済みのファイル（`API_V1_SCHEMA_JSON`、`include_str!`）をそのまま返す |
| 方言 | schemars 1.x の既定（JSON Schema 2020-12、`$defs`）。`$dynamicRef` は使わない |
| GUI 側 | `web/`: `pnpm gen:types`（`web/scripts/gen-types.mjs`。schema から `web/api/generated/` を生成し、`--check` で差分を検査）。旧 `gui/`: `pnpm gen:types`（`gui/scripts/gen-types.mjs` が `json2ts --additionalProperties=false` で `app/celeris/types.ts` を生成）。どちらも生成物をコミットする |
| 文書の写し | `scripts/sync-gui-docs.sh` がこのファイルを `gui/docs/celeris-api-v1.md` に写す（`--check` でずれを検査。ADR-0020 D4） |
| 互換性 | フィールドの追加（任意）は v1 のまま。削除・型変更・意味変更は `/api/v2` |

---

## 8. 試験の置き場所

HTTP 越しの試験は `crates/task-api/tests/`（fake の `SqliteStore` と `tempfile` だけで動く。ネットワークは loopback のみ）、派生値の規則の試験は `crates/task-ops/src/*/tests.rs` に置く。`GET /tasks/{id}` の本体は、同じ `ViewContext` で呼んだ `task_ops::view::task_detail` の compact な直列化と byte 単位で一致する（`timers.now` と API が埋める `runs[].files` を除く。`celerisctl show --json` も同じ関数。§3.5）。

---

## 9. 要求の検査と細部の挙動

GUI はこれを契約として扱ってよい（ADR-0013 の実装メモ）。

**要求の検査**（`crates/task-api/src/middleware.rs`）
- 順序: Host → `OPTIONS` の 405 → 認証（`/health` を除く。`/health` は無認証で版と `journal_mode` だけを返す）→ `POST` / `PUT` / `PATCH` / `DELETE` の Origin（403）→ `POST` / `PUT` / `PATCH` の Content-Type（415）と本文サイズ（413）→ ルーティング。
  - 認証が有効な構成では、未定義のパスも 404 より先に 401 になる。
  - `Content-Type` の無い `POST` / `PUT` / `PATCH` は、未定義のパスでも 415 になる。
- 400 `bad_request` / `host_not_allowed` になるもの:
  - Host ヘッダが複数ある要求、absolute-form の URI で authority が許可されない要求。
  - **未知のクエリパラメータ**と、単一値のキーの重複（全エンドポイント）。`status` / `kind` などの一覧のキーは繰り返し指定・カンマ区切りが可。
  - 数値でない `Last-Event-ID`。
- 空文字の `q=` / `cursor=` は指定無しとして扱う。`q` は `title` と `objective`（とコメント本文）を対象にする（ADR-0014 D2、ADR-0044 D4）。

**存在しない id**
- `/events?task_id=<存在しない id>` → 200 で空のページ。
- `/graph?root=<存在しない id>` → 404 `task_not_found`。
- 422 の `errors[].field` は、推定できるときだけ（`title` / `objective` / `acceptance` / `parent` / `depends_on` / `goal` / `answer`）。

**デーモンのスナップショット**
- `DaemonSnapshot.in_flight[]` の `kind: "reviewer"` の `run_id` は **Reviewer run 自身の id**（`WorkerStarted{role: reviewer}` と同じ。ADR-0014 D1）。
- `DaemonSnapshot.unroutable[]` は「設定に合うプロバイダ／クラスタが無い」タスクだけ。クラスタの多重接続待ち（cooldown 中を含む）のタスクはここには**入らない**（受信箱の `attention[].cluster_unavailable` が代わりに知らせる。ADR-0018 実装メモ M8）。

**SSE**
- 送る順は `hello` →（必要なら）`reset` → `task.event` …（§4）。

**ファイル**（`crates/task-api/src/files.rs`）
- Remote ワークスペースの `runs[].files` とファイル系エンドポイントは、手元の写し `workspace_root/<task_id>` を見る。写しが無ければ 404 `file_not_found`（`workspace directory does not exist`）。64 MiB を超える成果物は `sha256_current` と `sha256_matches` が `null`。
- `run_id` の形式検査は、ワークスペースの解決より先に行う（不正な `run_id` は、ワークスペースが無くても 403）。
- run ディレクトリが無ければ 404 `run_not_found`、ファイルが無ければ 404 `file_not_found`、ディレクトリなら 403。
- 成果物のパスは、空・絶対パス・`..` を含むものを字句的に 403 にしてから canonicalize する。
- クエリは `offset` / `length` / `download` だけ。
- 範囲指定:
  - `Range` の開始がサイズ以上なら 416（空ファイルを含む）。複数範囲・`end < start`・`bytes=-0` も 416。
  - `bytes` 以外の単位は無視して全体を 200 で返す。
  - `Range` と `offset` / `length` を同時に指定すると 400。
  - `offset` / `length` は 200（`offset == size` は空の本文）。`offset > size` は 416。

**運用ログ（ADR-0015）**
- API は要求ごとに `method` / `path` / `status` / `duration_ms` / `request_id`（= `X-Request-Id`）を記録する。既定は `debug`、**1 秒以上かかった要求は `warn`**（`slow api request`）。`GET /stream` は長時間つなぐのが正常なので警告の対象外。
- デーモンは `max(1 秒, tick_ms × 2)` を超えた tick を `warn`（`slow tick`）で記録する。
- DB がネットワークファイルシステム（NFS など）上にあると起動時に `warn`。SQLite の WAL はローカルディスクを前提にしている（ADR-0013 D5）。

**起動**
- celeris は `[api]` の `token_file` が読めない・空なら exit 2（設定の誤り）。DB が知らない新しい版数でも exit 2。
- API の DB 接続を開けない・bind できない場合は起動に失敗する（API 無しで動き続けない）。
- `celeris_version` は celeris crate の版（`CARGO_PKG_VERSION`）。

### 部署レビューからデプロイ準備への引き渡し（ADR-0051）

`GET /tasks/{id}/changes` の省略可能な `delivery` は自己改善案件の進行状態（`task_core::delivery::Delivery`）。
`state` は `reviewing | merge_queued | merging | preparing | ready | blocked`。
ほかに `task_id, project_id, repo_id, repo, branch, base, head, default_branch, department,
review_run, worker_run, criterion_idx, decision, detail, release, prepare_pid, notification`
と、push したときの `pushed_at` / `push_error` を持つ。
`decision` は既存Reviewer runのマージ判定。`release` は検証対象sha12。
`ready` はビルドとsnapshot検証の成功であり、本番昇格とは異なる。
既存 `/releases/{sha12}/promote` だけが人のデプロイ操作を受け付ける。

管理系 `POST /tasks/{id}/rereview` は `ReopenBody {expected_status?}` を受け取り（不一致は 409）、
Reviewer条件がある `done` の仕事、または直前の遷移が `review_fail` の `failed` を reviewing へ戻す（それ以外は 422）。返却は `TransitionResult`。
実装runは再実行せず、既存成果のレビューを再実行する。認証、404 は他の管理操作と同じ。

### タスクの routing の監査（ADR-0069 D5）

`GET /tasks/{id}/routing` → 200 `TaskRoutingView {task_id, assignee?, routing?, runs[]}`（読み取り。認証は他の
読み取りと同じ）。`routing` は `Task.routing`（`tier_source`・`assignee_explicit`・CoS/計画/委譲が書いたが
捨てた担当 `dropped_assignee`・`features` の上書き）。`runs[]` はワーカー run ごとの `RoutingAudit`（古い順:
`task_id, run_id, org_node, harness, adapter, provider, account, lane, model, reasoning_effort, features, rule_id,
policy_version, reasons, escalation, outcome, cost_usd, input_tokens, output_tokens, wall_ms, retries, review`）。
各 run の `escalation` がエスカレーションの履歴。run が無いタスクは `runs: []`、知らないタスクは 404、
クエリパラメータは 400。
