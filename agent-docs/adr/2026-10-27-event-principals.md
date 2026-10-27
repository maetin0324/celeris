# ADR: 判断・操作の event に実際の主体を記録する

- Date: 2026-10-27
- Status: Accepted
- Task: 01M5XXF7Q58YE3PEHHND8DBWT5
- 依存: ADR 2026-10-27-api-principals（A1。要求の主体 `Principal`）

## 背景

A1 で API の要求ごとに主体（`human:<name>` / `operator:<name>` / `mcp:<client>` / `cos:<thread>`）が
決まるようになった。しかし event の側に主体の欄が無いものが残っていた。

- `Answered`・`IntegrationAnswered`・`Retried` に主体の欄が無い。`Created` に作成者が無い。
- 計画の承認待ち（plan-gate）の判断は `WorkerProgress` の自由文に「（by、計画 id）」を埋めていただけ。
- task comment は CoS・MCP の書き込みも `author_kind = human` になり、comment から作る `Answered` も
  人の回答に見えた。本番の `task_comments` には `author_kind = human` で author が `cos` の行が 15 件、
  `mcp:chatgpt-rdc` の行が 1 件あった。

研究評価では「人がした判断」と「CoS・外部 AI がした判断」を区別して数える必要がある。

## 決定

1. **event の主体欄（D1）**: `Answered.by`・`IntegrationAnswered.by`・`Retried.by`・`Created.created_by` を足す。
   値は A1 の主体文字列。導入前の event には無いので `Option` で読み、無ければ省略する（events は追記専用。
   旧い event は書き換えない）。
2. **主体の決め方（D2）**: API・CoS operations・MCP の入口は A1 の `Caller.actor()` / `OperationAudit::by()` /
   `mcp:<client_id>` を ops 層へ渡す。daemon 自身が作る記録（承認の task・support task・統合依頼の置き換えや
   終端での回収）は `daemon`（`task_core::DAEMON_ACTOR`）、worker の run が宣言した後続 task は
   `worker:<run_id>`、celerisctl が DB へ直接書く操作は `human:cli` とする。`TaskStore::create_task` は
   `daemon` を書き、入口の主体を持つ呼び出しは `create_task_by` を使う。
3. **plan-gate の判断を構造化（D2）**: `POST /tasks/{id}/execution/plan-gate` の承認・replan・取り下げは
   `Event::PlanGateDecided { plan_id, action, by, note }` を同じ transaction で積む。`WorkerProgress` の自由文に
   by を埋める旧い記録は消す。承認に付けたメモの `Answered` にも `by` を書く。
4. **comment の書き手の種別（D3）**: `CommentAuthorKind` を `human | node | system | cos | mcp | operator` にする。
   人の入口の comment は主体から種別を決める（`CommentAuthorKind::from_actor`）。CoS・MCP・operator の comment も
   人の指示と同じく割り込み・回答として効く（`is_instruction`）。comment が回答になったときの
   `Answered.by` は comment の主体と同じ。
5. **既存行の修正（D3）**: migration 0097 が `task_comments` を作り直して CHECK を広げ、`author_kind = human` の
   行を author（`cos` / `cos:*` → cos、`mcp:*` → mcp、`operator:*` → operator）から直す。events は触らない。

## 範囲外

- `Edited`・`Approved` など、すでに `by` を持つ event の値（`human` の丸め）の見直しはこの ADR では扱わない。

## 結果

- 評価の集計は event の `by` / `created_by` と comment の `author_kind` で人と AI の判断を分けられる。
- 旧い event（欄なし）は「主体不明」として扱う。
