---
tasks: [01M7XQ9QE8STND208NXG90F743]
---
# ADR 2026-11-21: 案件の CoS Chat に discussion mode を足し、discuss・propose の run の CoS operation を API で拒否する

- 日付: 2026-11-21
- 状態: Accepted
- 関係: 戦略層 v1 子 3（task 01M7XQ9QE8STND208NXG90F743）。子 0 の設計 brief（`agent-docs/progress/2026-11-21-strategy-v1-0/design-brief.md`）の
  §1（状態と権限の分離）・X5・脅威 T2・T5。子 2 の [ADR 2026-11-21-strategy-evidence-contract](2026-11-21-strategy-evidence-contract.md)
  （`celeris.strategy/1`・D2.1・D1.2 の `chat` 参照・D4 の `strategy_gate`）。[ADR 2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md)
  （CoS の変更はすべて `/cos/operations`）、[ADR 2026-10-27-relayed-instructions](2026-10-27-relayed-instructions.md)（`author`・relay）、
  [ADR 2026-10-27-api-principals](2026-10-27-api-principals.md)（`Principal`）、[ADR 2026-11-04-project-controller](2026-11-04-project-controller.md)（goal_guard・authority_gate・D12 human_epoch・§A4 intent）。

## 背景

- 人は、案件の CoS thread で ChatGPT のように議論したい（質問・仮説の批判・案の切り出し・過去の議論の要約）。議論の途中で CoS が task を起票したり
  Roadmap・docs・knowledge を変えたりしないことを、prompt の指示ではなく権限で保証することを求めた（子 0 の X5・T2）。
- 今の CoS run は、`/cos/operations` の allowlist（111 の action）を、run の入力が人の発言でも議論でも同じ条件で呼べる。
  `chat/intent.rs` の型の推定（consult・read・delegate）は prompt の hint で、認可に効かない（同 file の冒頭の注記）。
- 長い議論の要約と案は、元の発言を引く。今の chat の message は書き換えも撤回もできず、revision も持たない。編集・撤回で要約と案を失効させる
  手段が無い（T5）。

## 決定

### D1: mode の欄、置き場所、run の mode の導出

#### D1.1 欄と値

- 型 `task_core::chat::ChatDiscussionMode`（wire は snake_case の `discuss`・`propose`・`execute`）。既存の `ChatSendMode`（`queue`・`interrupt`。
  run の queue の扱い）とは意味が違うので別の型・別の欄名にする（変更の規約 8）。
- 要求の欄名は `discussion_mode`。次の 3 つの要求に任意の欄（`#[serde(default)] Option<ChatDiscussionMode>`）として足す。
  - `ChatPostMessageRequest`（`POST /chat/threads/{t}/messages`、人・operator の token）
  - `ChatRelayRequest`（`POST /chat/threads/{t}/relays`、operator が人の発言を中継する。人が選んだ mode を運ぶ）
  - MCP の `console_instruct` の引数（`chat_mcp_instruct`。欄は同じ名前。MCP の client に discuss を強いるかは子 4 が scope で決める）
- 置き場所は `chat_messages.metadata_json` の key `discussion_mode`。`discuss` と `propose` のときだけ書く。欄を省いた要求と `execute` の要求は
  key を書かない。したがって mode を省いた送信は、`metadata_json`・応答・`chat_events`・SSE が今と 1 byte も変わらない。
- wire の `ChatMessage` に `discussion_mode: Option<ChatDiscussionMode>`（`skip_serializing_if = "Option::is_none"`）を足す。key が無い message は
  欄を出さない。
- 新しい列・表・migration は足さない。`metadata_json` は既に `relay`・`interrupt` を持ち、索引 `idx_chat_messages_relay` と同じ書き方で読める。

#### D1.2 run の mode の導出（保存しない）

- 1 つの message の mode: `metadata_json.discussion_mode` があればその値、無ければ `execute`。
- 厳しさの順: `discuss` > `propose` > `execute`。
- run `r`（`chat_runs.input_message_id` = `i`）の入力の集合 `E(r)`: 同じ thread の `role='user'`・`state!='cancelled'` の message のうち、
  `prev < seq <= i.seq` のもの。`prev` は、同じ thread で `r` より前に始まり `state='completed'` で終わった run の入力 message の seq の最大（無ければ 0）。
  `i` が `role='system'`（daemon の triage・review・要求）でも同じ式を使い、`i` 自身の mode は `execute` とする。
- run の mode = `E(r) ∪ {i}` の mode のうち最も厳しいもの。失敗した run の入力が次の run に持ち越されたときも、議論の指示は緩まない。
  interrupt で先に取られた run では、まだ queue にある前の message も `E(r)` に入る。これは厳しい側にだけ働く（fail closed）。
- 導出は `task_core::chat` の 1 つの関数 `discussion_mode_of_run(conn, thread_id, run_id) -> Result<RunDiscussionMode, ChatError>` が行う。
  戻り値は mode と、mode を決めた message の `{message_id, author}` の列（監査に書く。D7）。値は `chat_runs` にも `metadata_json` にも保存せず、
  呼ぶたびに `chat_messages`（`UNIQUE(thread_id, seq)` の索引）と `chat_runs` から導く。状態の正本は人の message の `metadata_json` だけである（規約 4）。
- CoS の prompt（`crates/task-worker/src/cos_chat.rs`）には、同じ関数が返した mode を 1 行で示す。保証は D2 の API の拒否で、prompt の行は説明である。
  `intent.rs` の型の推定は意味が違う（人が選ばない hint）ので残し、mode と 1 つにまとめない。

### D2: CoS operation の mode 別の許可と拒否

#### D2.1 規則

- discuss と propose の run は、`/cos/operations` の 111 の action をすべて拒否する。discuss は read と会話だけで、Celeris の状態への副作用を 0 にする。
  propose が残す物は、返事に付けた案の file だけ（D3）で、これは operation を使わない。したがって propose も operation を 1 つも要しない。
- execute の run は今と同じ。`bind_run_policy`・`instructed_by`・human_required・`basis`・`goal_guard::check`・`strategy_gate`・`authority_gate`・
  domain の検査を、今の順のまま通す（D4）。
- 表は base（このブランチの開始時の commit）の `crates/task-api/src/cos/ops/{tasks,decisions,projects,admin,surface}.rs` の `ALLOWED` から機械的に作った
  （111 行）。gate は表を写さず、`operations::allowed()` が返す全行に同じ規則を当てる。action を足すと、足した action も自動で discuss・propose で拒否される
  （許可を足すには、この ADR を改めて決める）。
- 依頼が名指しした 3 つの系（Task の起票、roadmap と案件の変更、docs・knowledge の公開系）は、表の「分類」の列で示す。

#### D2.2 `/cos/operations` の外の CoS credential の口

| 口 | discuss | propose | execute | 理由 |
|---|---|---|---|---|
| `POST /cos/inbox/{i}/resolve` | 拒否 403 | 拒否 403 | 今と同じ | operation を実行する口。triage の run は入力が system なので execute（D1.2）で、今と変わらない |
| `POST /cos/threads/{t}/checkpoint` | 許可 | 許可 | 許可 | CoS 自身の thread の要約の cursor。domain の状態ではない。編集・撤回との関係は D5.4 |
| `POST /cos/knowledge-use` | 許可 | 許可 | 許可 | 読んだ knowledge の記録（observability）。domain の状態を変えない |
| `POST /cos/review-report` | — | — | — | 実体は action `chat.review_report`（表の行）。review の thread の run だけが使う |
| `GET` の口（knowledge の tree・page、inbox、operation、review-thread） | 許可 | 許可 | 許可 | 読むだけ |

CoS の harness の道具（shell など）で Celeris の外に起きる副作用は、この ADR の保証の外である（既存の CoS の tool policy のまま）。discuss の
「副作用 0」は「`cos_operations` に `applied` の行が増えず、tasks・roadmap・knowledge・docs・org の行と file が変わらない」と定義し、試験もこの定義で数える。

#### D2.3 拒否の status・code・監査

- status 403、problem の code `discussion_mode_forbidden`、detail は `run mode is <mode>; <action> needs an execute-mode human message`。
- 置く所: `create_operation` の blocking 区間で、idempotency の照合と `bind_run_policy` の後、`matched.is_none()` の分岐の後、`instructed_by` の解釈の前。
  未登録の path は今の理由（422）のまま拒否する。`instructed_by` で人の発言を引いても discuss・propose の拒否は外れない（人がその発言で議論を選んだため）。
  `/cos/inbox/{i}/resolve` も同じ関数を同じ位置で呼ぶ。
- 監査: 既存の `OperationAudit::reject(store, "api", &path, problem)` で `cos_operations` に `state='rejected'` の行と理由つきの監査 event を同じ
  transaction で書く。`audit.payload` に `discussion_mode: {mode, decided_by: [{message_id, author}]}` を入れる（D7）。新しい表・event の種類は足さない。
- 同じ idempotency key で既に記録された operation の再送は、今と同じく記録を返す（新しい副作用は無い）。

#### D2.4 action ごとの表

| # | registry | action | method と path | 分類 | discuss | propose | execute |
|---|---|---|---|---|---|---|---|
| 1 | tasks | `task.create` | `POST /api/v1/tasks` | **Task の起票** | 拒否 403 | 拒否 403 | 今と同じ |
| 2 | tasks | `task.integrate` | `POST /api/v1/tasks/{id}/changes/{repo}/integrate` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 3 | tasks | `task.pr_merge` | `POST /api/v1/tasks/{id}/changes/{repo}/pr/merge` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 4 | tasks | `comment.create` | `POST /api/v1/tasks/{id}/comments` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 5 | tasks | `execution.phase_gate` | `POST /api/v1/tasks/{id}/execution/phase-gate` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 6 | tasks | `question.answer` | `POST /api/v1/tasks/{id}/answer` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 7 | tasks | `execution.plan_gate` | `POST /api/v1/tasks/{id}/execution/plan-gate` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 8 | tasks | `execution.plan_adopt` | `POST /api/v1/tasks/{id}/execution-plan` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 9 | tasks | `task.update` | `PATCH /api/v1/tasks/{id}` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 10 | tasks | `task.reopen` | `POST /api/v1/tasks/{id}/reopen` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 11 | tasks | `task.retry` | `POST /api/v1/tasks/{id}/retry` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 12 | tasks | `task.accept` | `POST /api/v1/tasks/{id}/accept` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 13 | tasks | `task.approve` | `POST /api/v1/tasks/{id}/approve` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 14 | tasks | `task.reject` | `POST /api/v1/tasks/{id}/reject` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 15 | tasks | `task.cancel` | `POST /api/v1/tasks/{id}/cancel` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 16 | tasks | `task.rereview` | `POST /api/v1/tasks/{id}/rereview` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 17 | tasks | `integration.integrate_task` | `POST /api/v1/tasks/{id}/integration-requests/{request}/integrate-task` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 18 | tasks | `execution.decompose` | `POST /api/v1/tasks/{id}/execution/decompose` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 19 | tasks | `task.pause` | `POST /api/v1/tasks/{id}/pause` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 20 | tasks | `task.resume` | `POST /api/v1/tasks/{id}/resume` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 21 | tasks | `execution.put_plan` | `PUT /api/v1/tasks/{id}/execution-plan` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 22 | tasks | `tree.adopt` | `POST /api/v1/tasks/{id}/tree/adopt` | Task の変更・実行 | 拒否 403 | 拒否 403 | 今と同じ |
| 23 | decisions | `decision.answer` | `POST /api/v1/decisions/{id}/answer` | 人の決定・承認への回答 | 拒否 403 | 拒否 403 | 今と同じ |
| 24 | decisions | `approval.decide` | `POST /api/v1/approvals/{id}/decide` | 人の決定・承認への回答 | 拒否 403 | 拒否 403 | 今と同じ |
| 25 | decisions | `knowledge.record` | `POST /api/v1/knowledge/inbox` | knowledge の受信箱 | 拒否 403 | 拒否 403 | 今と同じ |
| 26 | decisions | `knowledge.page_put` | `PUT /api/v1/knowledge/page` | **knowledge（公開系）** | 拒否 403 | 拒否 403 | 今と同じ |
| 27 | decisions | `knowledge.reject` | `POST /api/v1/knowledge/inbox/{id}/reject` | knowledge の受信箱 | 拒否 403 | 拒否 403 | 今と同じ |
| 28 | decisions | `inbox.answer` | `POST /api/v1/inbox/items/{id}/answer` | 人の決定・承認への回答 | 拒否 403 | 拒否 403 | 今と同じ |
| 29 | decisions | `decision.revise` | `POST /api/v1/decisions/{id}/revise` | 人の決定・承認への回答 | 拒否 403 | 拒否 403 | 今と同じ |
| 30 | decisions | `decision.withdraw` | `POST /api/v1/decisions/{id}/withdraw` | 人の決定・承認への回答 | 拒否 403 | 拒否 403 | 今と同じ |
| 31 | decisions | `knowledge.accept` | `POST /api/v1/knowledge/inbox/{id}/accept` | **knowledge（公開系）** | 拒否 403 | 拒否 403 | 今と同じ |
| 32 | decisions | `notification.read_all` | `POST /api/v1/notifications/read-all` | 通知の既読 | 拒否 403 | 拒否 403 | 今と同じ |
| 33 | decisions | `notification.read` | `POST /api/v1/notifications/{id}/read` | 通知の既読 | 拒否 403 | 拒否 403 | 今と同じ |
| 34 | projects | `project.create` | `POST /api/v1/projects` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 35 | projects | `project.update` | `PATCH /api/v1/projects/{id}` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 36 | projects | `project.cancel` | `POST /api/v1/projects/{id}/cancel` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 37 | projects | `project.pause` | `POST /api/v1/projects/{id}/pause` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 38 | projects | `project.resume` | `POST /api/v1/projects/{id}/resume` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 39 | projects | `project.archive` | `POST /api/v1/projects/{id}/archive` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 40 | projects | `project.unarchive` | `POST /api/v1/projects/{id}/unarchive` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 41 | projects | `project.status_put` | `PUT /api/v1/projects/{id}/status` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 42 | projects | `project.controller_put` | `PUT /api/v1/projects/{id}/controller` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 43 | projects | `project.controller_wake` | `POST /api/v1/projects/{id}/controller/wake` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 44 | projects | `project.roadmap_propose` | `POST /api/v1/projects/{id}/roadmap/proposals` | **roadmap の変更** | 拒否 403 | 拒否 403 | 今と同じ |
| 45 | projects | `authority.grant_revoke` | `POST /api/v1/projects/{id}/authority/grants/{g}/revoke` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 46 | projects | `authority.check` | `POST /api/v1/projects/{id}/authority/check` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 47 | projects | `authority.grant_propose` | `POST /api/v1/projects/{id}/authority/proposals` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 48 | projects | `standing_rule.create` | `POST /api/v1/standing-rules` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 49 | projects | `standing_rule.delete` | `DELETE /api/v1/standing-rules/{id}` | **案件の変更**・権限 | 拒否 403 | 拒否 403 | 今と同じ |
| 50 | projects | `docs.init` | `POST /api/v1/projects/{id}/docs/init` | **docs（公開系）** | 拒否 403 | 拒否 403 | 今と同じ |
| 51 | projects | `docs.page_put` | `PUT /api/v1/projects/{id}/docs/page` | **docs（公開系）** | 拒否 403 | 拒否 403 | 今と同じ |
| 52 | projects | `docs.page_delete` | `DELETE /api/v1/projects/{id}/docs/page` | **docs（公開系）** | 拒否 403 | 拒否 403 | 今と同じ |
| 53 | projects | `docs.maintenance` | `POST /api/v1/projects/{id}/docs/maintenance` | **docs（公開系）** | 拒否 403 | 拒否 403 | 今と同じ |
| 54 | admin | `model_override.put` | `PUT /api/v1/llm/models/{source}/{model_id}/override` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 55 | admin | `model_override.delete` | `DELETE /api/v1/llm/models/{source}/{model_id}/override` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 56 | admin | `org.create` | `POST /api/v1/org` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 57 | admin | `org.update` | `PATCH /api/v1/org/{id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 58 | admin | `org.delete` | `DELETE /api/v1/org/{id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 59 | admin | `org.browser_settings` | `PATCH /api/v1/org/{id}/browser-settings` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 60 | admin | `org.skill_mount` | `POST /api/v1/org/{id}/skills` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 61 | admin | `org.skill_unmount` | `DELETE /api/v1/org/{id}/skills/{skill}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 62 | admin | `skill.put` | `PUT /api/v1/skills/{name}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 63 | admin | `skill.delete` | `DELETE /api/v1/skills/{name}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 64 | admin | `repo.create` | `POST /api/v1/projects/{id}/repos` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 65 | admin | `repo.update` | `PATCH /api/v1/repos/{id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 66 | admin | `repo.delete` | `DELETE /api/v1/repos/{id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 67 | admin | `local.settings_put` | `PUT /api/v1/clusters/local/settings` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 68 | admin | `cluster.settings_put` | `PUT /api/v1/clusters/{id}/settings` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 69 | admin | `account.create` | `POST /api/v1/llm/sources/{source_id}/accounts` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 70 | admin | `account.delete` | `DELETE /api/v1/llm/sources/{source_id}/accounts/{account_id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 71 | admin | `account.check` | `POST /api/v1/llm/sources/{source_id}/accounts/{account_id}/check` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 72 | admin | `account.limit_set` | `PATCH /api/v1/llm/sources/{source_id}/accounts/{account_id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 73 | admin | `model_catalog.discover` | `POST /api/v1/llm/models/discover` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 74 | admin | `notify.test` | `POST /api/v1/notify/test` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 75 | admin | `daemon.reload` | `POST /api/v1/reload` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 76 | admin | `daemon.replay` | `POST /api/v1/replay` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 77 | admin | `release.promote` | `POST /api/v1/releases/{sha12}/promote` | **公開系** | 拒否 403 | 拒否 403 | 今と同じ |
| 78 | admin | `cron_job.create` | `POST /api/v1/cron-jobs` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 79 | admin | `cron_job.update` | `PATCH /api/v1/cron-jobs/{id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 80 | admin | `cron_job.delete` | `DELETE /api/v1/cron-jobs/{id}` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 81 | admin | `cron_job.pause` | `POST /api/v1/cron-jobs/{id}/pause` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 82 | admin | `cron_job.resume` | `POST /api/v1/cron-jobs/{id}/resume` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 83 | admin | `cron_job.run` | `POST /api/v1/cron-jobs/{id}/run` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 84 | admin | `script_domain.mode_set` | `PUT /api/v1/script/domains/{domain}/mode` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 85 | admin | `script_generation.register` | `POST /api/v1/script/generations` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 86 | admin | `script_generation.retire` | `POST /api/v1/script/generations/{id}/retire` | 管理 | 拒否 403 | 拒否 403 | 今と同じ |
| 87 | surface | `attachment.reference` | `POST /api/v1/chat/attachments/{id}/references` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 88 | surface | `chat.thread_create` | `POST /api/v1/chat/threads` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 89 | surface | `chat.thread_update` | `PATCH /api/v1/chat/threads/{t}` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 90 | surface | `chat.message_post` | `POST /api/v1/chat/threads/{t}/messages` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 91 | surface | `chat.message_cancel` | `DELETE /api/v1/chat/threads/{t}/messages/{m}` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 92 | surface | `chat.run_stop` | `POST /api/v1/chat/threads/{t}/stop` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 93 | surface | `chat.thread_read` | `POST /api/v1/chat/threads/{t}/read` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 94 | surface | `chat.relay_forward` | `POST /api/v1/chat/threads/{t}/forwards` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 95 | surface | `chat.proposal_forward` | `POST /api/v1/chat/threads/{t}/proposals` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 96 | surface | `chat.queue_resume` | `POST /api/v1/chat/threads/{t}/resume-queue` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 97 | surface | `chat.attachment_upload` | `POST /api/v1/chat/threads/{t}/attachments` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 98 | surface | `chat.attachment_delete` | `DELETE /api/v1/chat/attachments/{a}` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 99 | surface | `chat.review_report` | `POST /api/v1/cos/review-report` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 100 | surface | `org.message` | `POST /api/v1/org/{id}/messages` | chat の面 | 拒否 403 | 拒否 403 | 今と同じ |
| 101 | surface | `artifact.promote` | `POST /api/v1/tasks/{id}/artifacts/promote` | **公開系** | 拒否 403 | 拒否 403 | 今と同じ |
| 102 | surface | `browser.site_policy_put` | `PUT /api/v1/browser/site-policies/{policy_id}` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 103 | surface | `browser.site_policy_delete` | `DELETE /api/v1/browser/site-policies/{policy_id}` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 104 | surface | `browser.task_policy_put` | `PUT /api/v1/tasks/{id}/browser/policy` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 105 | surface | `browser.request_open` | `POST /api/v1/tasks/{id}/browser/requests` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 106 | surface | `browser.control` | `POST /api/v1/tasks/{id}/browser/control/{run}/{session}` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 107 | surface | `browser.control_disconnect` | `POST /api/v1/tasks/{id}/browser/control/{run}/{session}/disconnect` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 108 | surface | `browser.agent_begin` | `POST /api/v1/tasks/{id}/browser/control/{run}/{session}/agent/begin` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 109 | surface | `browser.agent_end` | `POST /api/v1/tasks/{id}/browser/control/{run}/{session}/agent/end` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 110 | surface | `browser.auth_section` | `POST /api/v1/tasks/{id}/browser/control/{run}/{session}/auth-section` | browser | 拒否 403 | 拒否 403 | 今と同じ |
| 111 | surface | `browser.live_event` | `POST /api/v1/tasks/{id}/browser/live/{run}/{session}/events` | browser | 拒否 403 | 拒否 403 | 今と同じ |

計 111 行（discuss で許可 0・propose で許可 0・execute は 111 行すべて既存の検査のまま）。太字の分類は依頼が名指しした系で、discuss で必ず拒否する。

### D3: propose の案の保存先

- 置き場所は CoS thread の workspace の file `strategy/<strategy_id>/v<version>.json`（`celeris.strategy/1`）。run の終わりに dispatcher が既存の
  `chat_message_attach_workspace_files` で返事の message に添付する。添付は `chat_attachments` の行（Celeris が blob から計算した `sha256`）と、
  message からの `chat_attachment_refs`（参照がある間は GC で消えない）になる。`knowledge.record` は使わない。knowledge の受信箱の候補は日次の整理を
  通ると正本に適用されるので、未承認の案の置き場所にすると「propose は未承認の案だけ」が保てないからである。置き場所は 1 つにする（規約 4）。
- 文書の欄は子 2 の D1.1 のとおり。chat の案では次を固定する。
  - `decision_state` は `discussion` か `proposed`（子 2 D2.1。`approved`・`rejected` は拒否）。
  - `source = {kind: "celeris_cos_thread", label, thread_id: <この thread>}`、`actor = {kind: "cos", id: "cos:<thread_id>"}`、`project_id` は thread の案件。
  - 版: 同じ thread で同じ name に違う sha256 の添付があれば衝突として拒否する（子 2 D1.4 と同じ規則）。`supersedes` は v1 で `null`、v2 以降は
    前の版の chat の添付 `{thread_id, message_id, name, sha256}`（子 2 の task の成果物の形 `{task_id, name, sha256}` と並ぶ 2 つ目の形）。
  - 引用: `references[]` の `chat` の参照に `revision`（1 以上の整数）と `sha256`（引用した版の本文の SHA-256、64 桁の小文字 hex）を必須にする
    （子 2 D1.2 の `chat` 参照に欄を足す。子 2 の検査器・試験を同じ変更で直す。本番に `chat` 参照を持つ文書はまだ無い）。
- 検査: 添付の前に、子 2 の `task_core::strategy` の純粋関数と D6 の引用の検査を当てる。通らない file は添付せず、run の status に code を出す。
  discuss の run の `strategy/` の file は添付しない（status `discussion_mode_forbidden`）。execute の run は propose と同じく添付できる。
- 案は実行の入口を通らない。`strategy_gate`（子 2 D4）は task の成果物と人の決定だけを承認とみなすので、chat の案を `strategy` の参照に書いた
  `task.create` は `strategy_source_missing` で拒否される。承認に進めるには、人が execute で依頼し、task の run が成果物 `strategy/<id>/v<n>.json` を作って
  決定 `strategy-approve-v<n>` を出す（子 2 D2.2 の既存の経路）。Web で chat の案を直接採用・却下する口は子 7 が決める。

### D4: execute は既存の検査をすべて通す

execute の run の operation は、今の `create_operation` の順のまま通す。足す検査は無く、外す検査も無い。

1. `bind_run_policy`（run に渡した policy の版）。
2. `instructed_by`（run の入力の人の発言か、scope の内の relay）。human_required は人の発言の引用でだけ外れる。
3. `basis`（人の待ちに CoS が自分の判断で答えるとき）。
4. `goal_guard::check`（人の目的・受け入れ条件・controller の起票は人の発言の引用が要る）。
5. `strategy_gate`（`strategy` の欄があるとき。子 2 D4）。
6. `authority_gate`（本番・外部公開・不可逆・API の認可・外部の効果は、人の発言の引用か authority ledger の許可）。
7. domain の検査（task の予算の既定 `max_wall_secs`・`max_turns`、案件の予算、quota、担当の決定は既存の dispatch と admission が行う）。

試験 `discussion_mode_execute_*` は、`discussion_mode: execute` の送信と欄を省いた送信で、応答・`cos_operations` の行・監査が同じことを確かめる。

### D5: message の編集・撤回

#### D5.1 REST と権限

- 編集 `PATCH /api/v1/chat/threads/{t}/messages/{m}`、本文 `{text, expected_revision}`（`deny_unknown_fields`、`text` は既存の `CHAT_MESSAGE_TEXT_MAX_BYTES` まで）。
- 撤回 `POST /api/v1/chat/threads/{t}/messages/{m}/retract`、本文 `{expected_revision, reason}`（`reason` は 500 文字まで、空でもよい）。
- 前の版の読み出し `GET /api/v1/chat/threads/{t}/messages/{m}/revisions`（`chat_events` の `message_revised` を古い順に返す。読む権限は thread の読み出しと同じ）。
- 書けるのは `Principal::Human` で、`caller.actor()` が message の `author` と一致するときだけ。`operator:`・`mcp:`・`cos:`・認証の無い要求、
  別の人、relay の message（author は operator）は 403 `chat_message_edit_forbidden`。CoS の run は `/cos/operations` にこの 2 つの action を持たない
  （allowlist に足さない）。
- 対象は `role='user'` の message だけ。`assistant`・`system` は 403 `chat_message_edit_forbidden`。
- 状態: `running` は 409（run が読んでいる）。`cancelled` は 409。`queued` は編集できる（まだ届いていない）。撤回は届いた message（`completed`・`failed`・
  `interrupted`）だけで、`queued` は既存の取り消し（`DELETE …/messages/{m}`）を使う（409）。
- `expected_revision` が今の revision と違えば 409（`chat_conflict`）。
- 編集は mode を変えない（mode を変えるときは撤回して送り直す）。編集・撤回は `human_epoch`（project-controller D12）を上げない。新しい指示ではないからである。

#### D5.2 revision と本文の記録

- message の revision は `metadata_json.revision`（無ければ 1）。編集・撤回のたびに 1 上げる。wire の `ChatMessage` に `revision`（1 のときは出さない）、
  `edited: Option<{by, at}>`、`retracted: Option<{by, at, reason}>` を足す。
- 編集は `chat_messages.text` を新しい本文に置き換える。撤回は `text` を空にし、`metadata_json.retracted` を書く。FTS5 は既存の trigger
  `chat_search_message_update`（`AFTER UPDATE OF thread_id,text`）で追従するので、撤回した本文は履歴検索に出ない。
- 前の本文は同じ transaction で `chat_events` に 1 行追記する。type は `message_revised`（`ChatEventType::MessageRevised`、data は
  `ChatMessageRevisedData {message_id, kind: edit|retract, revision, previous_revision, previous_text, previous_sha256, by, reason, checkpoint_reset}`）。
  続けて既存の `message` event で新しい message を流す（SSE の client は既存の処理で表示を直す）。`chat_events.type` に CHECK は無く、
  retention（`chat_events_retention`）が消すのは `text_delta` と `tool` だけなので、前の本文は消えない（events は追記だけ）。

#### D5.3 添付

編集で添付は変えない。撤回した message の添付の参照（`chat_attachment_refs`）は残す（データを失わない）。

#### D5.4 checkpoint と session

- 編集・撤回した message の seq が `chat_threads.summary_through_seq` 以下なら、同じ transaction で `summary=''`・`summary_through_seq=0` に戻す。
  消した要約の全文と cursor は `message_revised` の `checkpoint_reset: {previous_through_seq, previous_summary}` に残す。走っている run の checkpoint は
  既存の `expected_summary_through_seq` の照合で 409 になる。
- 編集・撤回した message の seq が live の CoS session の `delivered_through_seq` 以下なら、同じ transaction で session を retire する（既存の `retire_conn`）。
  次の run は fresh の session で始まり、harness の会話の記憶に前の本文が残らない。

### D6: 要約と案の出典、失効の導出

- 引用の形 `ChatCitation {thread_id, message_id, revision, sha256}` を `task_core::chat` に 1 つだけ置く。子 2 の `chat` 参照（D3）と要約の引用が同じ型・
  同じ検査を使う（規約 2）。
- 要約は CoS の返事に添える workspace の file `discussion/summary/<name>.json`（schema `celeris.discussion-summary/1`:
  `{schema, project_id, thread_id, through_seq, body, citations[]}`。`body` は Markdown で 16,384 byte まで、`citations` は 1〜256 件）。
  長い履歴の要約の本文は file の `body` で、返事の本文は短い案内にする。
- 添付のとき、dispatcher は要約と案の file の引用を検査する: thread が同じか、案件の無い統括の thread、message がある、`revision` と `sha256` が
  その時の message と一致する。通らない file は添付しない。通った file の引用の列は、添付と同じ transaction で返事の `metadata_json.citations` に
  1 回だけ書く（file の blob が正本で、`citations` はそこから一度だけ導いた索引。どちらも以後書き換えない）。
- 失効の状態は保存しない。`GET …/messages` と SSE の `message` event を組むとき、`citations` の各行に、引いた message の今の revision・本文の sha256・
  `retracted` から状態 `current`・`edited`・`retracted`・`missing` を導いて付ける（`ChatCitationView`）。1 つでも `current` でない要約・案は失効と表示する。
- 子 2 の `strategy_gate` の `chat` 参照の検査も同じ関数を使う。`edited` は `strategy_stale`、`retracted`・`missing` は `strategy_source_missing`。

### D7: actor の区別と監査

| actor | 記録の場所 | 値 |
|---|---|---|
| 人 | message の `author` | `human:<name>`（人の API token の `Principal::Human`） |
| 中継した operator | message の `author` と `relay.relayed_by` | `operator:<name>` |
| MCP の client | message の `author` | `mcp:<client_id>` |
| CoS | `cos_operations`・監査 event の `by`、案の文書の `actor` | `cos:<thread_id>`（`OperationAudit::by`）と `run_id` |
| 編集・撤回した人 | `message_revised` の `by`、`ChatMessage.edited.by`・`retracted.by` | `human:<name>` だけ |

- mode の拒否の監査は、拒否した CoS の run（`cos:<thread>`・`run_id`）と、mode を決めた message の `{message_id, author}` の列を持つ。人が選んだのか、
  operator・MCP の client の入力で厳しくなったのかを監査から区別できる。
- MCP の client と operator の入力は人の承認にならない（子 2 D2.2 の規則のまま）。mode は「させない」向きにだけ効き、承認や許可を足さない。

### D8: 試験

#### D8.1 名前と守る不変条件

| WU | 試験 | 守る不変条件 |
|---|---|---|
| mode-gate | `discussion_mode_absent_post_is_unchanged` | 欄を省いた送信と `execute` の送信で、`metadata_json`・応答・`chat_events` が base と同じ（回帰） |
| mode-gate | `discussion_mode_unknown_value_is_422` | 決めた 3 語の外は 422 |
| mode-gate | `discussion_mode_run_takes_strictest_input` | `E(r)` の最も厳しい mode を採る |
| mode-gate | `discussion_mode_system_input_run_is_execute` | triage・review の run は今と同じ |
| mode-gate | `discussion_mode_discuss_denies_every_registered_op` | `allowed()` の全行が 403・`rejected` の行・domain の行数と file が不変 |
| mode-gate | `discussion_mode_propose_denies_every_registered_op` | 同上（propose） |
| mode-gate | `discussion_mode_discuss_task_create_is_403_and_creates_nothing` | Task 起票の負の試験。tasks の行数が増えない |
| mode-gate | `discussion_mode_discuss_roadmap_propose_is_403` | roadmap の版が増えない |
| mode-gate | `discussion_mode_discuss_project_update_is_403` | Project.request が変わらない |
| mode-gate | `discussion_mode_discuss_publish_ops_are_403` | `docs.page_put`・`knowledge.page_put`・`knowledge.accept`・`artifact.promote`・`release.promote` |
| mode-gate | `discussion_mode_discuss_instructed_by_cannot_waive` | 人の発言の引用でも外れない（T2） |
| mode-gate | `discussion_mode_discuss_inbox_resolve_is_403` | `/cos/inbox/{i}/resolve` |
| mode-gate | `discussion_mode_reject_audit_names_deciding_messages` | 監査に mode と決めた message・author |
| mode-gate | `discussion_mode_relay_and_mcp_instruct_carry_mode` | relay・MCP の入力も mode を運ぶ |
| mode-gate | `discussion_mode_execute_matches_absent` | execute は今の検査の順と結果のまま（goal_guard の 422、authority_gate） |
| mode-gate | `discussion_mode_external_text_cannot_lift_mode` | 本文・添付の指示文（prompt injection）で mode も権限も変わらない（T2） |
| summary-cite | `discussion_mode_propose_attaches_valid_strategy_draft` | 案は添付として残り sha256 を持つ |
| summary-cite | `discussion_mode_discuss_refuses_strategy_draft` | discuss の run の案は添付されない |
| summary-cite | `discussion_mode_propose_refuses_approved_state_draft` | `approved` を書いた案は拒否 |
| summary-cite | `discussion_mode_propose_same_name_other_sha_refused` | 同じ版の 2 つの sha256 は衝突（T5） |
| edit-retract | `chat_message_edit_by_author_keeps_previous_body_in_chat_events` | 前の本文が `message_revised` に残る |
| edit-retract | `chat_message_edit_forbidden_for_non_author_principals` | operator・mcp・cos・別の人・認証無しは 403 |
| edit-retract | `chat_message_edit_running_is_409` / `chat_message_edit_stale_revision_is_409` | 走っている入力と古い revision |
| edit-retract | `chat_message_edit_assistant_or_system_is_403` | 人の発言だけ |
| edit-retract | `chat_message_retract_empties_text_and_search` | 撤回の本文は FTS5 に出ない |
| edit-retract | `chat_message_edit_resets_checkpoint_covering_seq` | 要約の cursor を戻し、前の要約を event に残す |
| edit-retract | `chat_message_edit_retires_session_that_saw_message` | 次の run は fresh |
| edit-retract | `chat_message_edit_previous_body_survives_retention` | retention が `message_revised` を消さない |
| edit-retract | `chat_message_edit_does_not_bump_human_epoch` | controller の human_epoch は不変 |
| summary-cite | `chat_discussion_summary_citation_current` | 引用の状態が `current` |
| summary-cite | `chat_discussion_summary_stale_after_edit` / `chat_discussion_summary_stale_after_retract` | 編集・撤回で失効（T5） |
| summary-cite | `chat_discussion_summary_citation_other_thread_refused` | 別の案件の thread の発言は引けない（T4） |
| summary-cite | `chat_discussion_summary_citation_wrong_revision_refused` | 添付の時に revision・sha256 が違えば拒否 |
| summary-cite | `chat_discussion_summary_state_is_not_stored` | `metadata_json.citations` に状態が無い |
| summary-cite | `chat_discussion_summary_strategy_ref_edited_is_stale` | 子 2 の gate も同じ判定（`strategy_stale`・`strategy_source_missing`） |

#### D8.2 変更前に落ちること

- `discussion_mode_*`: base の `ChatPostMessageRequest` は `deny_unknown_fields` で `discussion_mode` を知らないので、送信が 422 になり試験は落ちる。
  送信を経ずに store に `metadata_json.discussion_mode` を直接置いた版の試験（`discussion_mode_discuss_task_create_is_403_and_creates_nothing` の補助）では、
  base の `/cos/operations` は `task.create` を通して task を作るので落ちる。
- `chat_message_edit_*`: base に `PATCH …/messages/{m}`・`…/retract` が無い（405）ので落ちる。
- `chat_discussion_summary_*`: base の message は revision と `citations` を持たないので落ちる。

#### D8.3 回帰

- `discussion_mode_absent_post_is_unchanged` と `discussion_mode_execute_matches_absent`。
- 既存の試験（`crates/task-api/src/cos/ops/tests.rs`・`crates/task-api/tests/cos_*`・`crates/task-core/src/chat/*tests.rs`・`strategy_gate`・web の chat の unit 試験）を
  変えずに通す。`ChatPostMessageRequest {` の struct literal 65 か所・`ChatRelayRequest {` 20 か所には `discussion_mode: None` を足すだけで、
  主張は変えない（変更の規約 10: 弱める試験は無い）。

### D9: web の変更（既存の CoS chat 画面の延長。新しい chat の実装は作らない）

| file | 変更 |
|---|---|
| `web/features/chat/composer/chat-composer.tsx` | 送信の横に mode の選択（議論・案・実行）。既定は「実行」で、そのときは欄を送らない（今と同じ） |
| `web/features/chat/data/send.ts`・`session.ts`・`reducer.ts`・`client.ts` | `discussion_mode` を送る。`message_revised` の SSE で表示を直す。編集・撤回・revisions の client |
| `web/features/chat/messages/message-item.tsx`・`parts.tsx` | mode の印、「編集済み（rev n）」「撤回済み」、自分の発言の編集・撤回の操作、前の版の表示 |
| `web/features/chat/messages/workspace-links.ts` | 案・要約の file に引用の状態（current・edited・retracted・missing）と、引いた message への移動 |
| `docs/api`（openapi・型の生成） | 欄・3 つの口・event の型 |

### D10: 足さないもの

- 新しい DB 表・migration・Manager・trait・Agent サービス・状態機械は足さない。足す型は wire の enum 1 つ（`ChatDiscussionMode`）、event の data 1 つ、
  引用の型 2 つ（`ChatCitation`・`ChatCitationView`）、要求の型 2 つ（編集・撤回）で、置き場所は既存の `chat_messages.metadata_json`・`chat_events`・
  `chat_attachments` である。
- 承認の経路は足さない（子 2 の決定のまま）。Project.request と承認済み Roadmap は書かない（discuss・propose では operation がすべて拒否される）。
- prompt だけの制限は今 0 件（子 0 X5 の `git grep`）で、消す物は無い。

## 実装前 brief

1. 根本原因: CoS operation の認可が run の入力の種類（議論か指示か）を見ない。message に revision が無く、引用の失効を導けない。
2. 責務 owner: mode の記録と導出、編集・撤回、引用は `task_core::chat`（store・human_post）。拒否は `task_api::cos::operations::create_operation` と
   `cos::inbox::resolve`。REST は `task_api::chat`。案・要約の添付の検査は dispatcher の chat の run の終わり（`chat_message_attach_workspace_files` を呼ぶ所）。
3. 既存の再利用先: `OperationAudit::reject`、`operations::allowed()`、`metadata_json`、`chat_events`、`chat_attachments`、FTS5 の trigger、`retire_conn`、
   `chat_thread_checkpoint_at` の cursor、子 2 の `task_core::strategy` と `strategy_gate`。
4. 選択肢: (a) mode を thread に持つ — 発言ごとに選べず、議論から実行への切り替えが残りの発言にも効くので不採用。(b) mode を run に保存する — 正本が 2 つに
   なるので不採用。(c) 人の message に持ち、run の mode は導く — 採用。案の置き場所は (i) `knowledge.record`（日次の整理で正本に入るので不採用）、
   (ii) 返事の workspace file（採用）。
5. 新しい状態・分岐・fallback・port: `metadata_json` の key `discussion_mode`・`revision`・`edited`・`retracted`・`citations`、`chat_events` の type
   `message_revised`、`create_operation` の拒否の分岐 1 つ。fallback と新旧切替の flag は無い。
6. 撤去対象: 無し（置き換える仕組みが無い）。
7. 不変条件: D8.1 の表。

## WorkUnit への割り当て（計画のまま）

| WorkUnit | 書く場所 |
|---|---|
| mode-gate | `crates/task-core/src/chat/`（型・導出）、`crates/task-api/src/chat/mod.rs`、`crates/task-api/src/cos/operations.rs`・`cos/inbox.rs`、`crates/celeris-mcp/src/tools/console.rs`、`crates/task-worker/src/cos_chat.rs`（1 行） |
| edit-retract | `crates/task-core/src/chat/`（編集・撤回の tx、event の data）、`crates/task-api/src/chat/mod.rs`（3 つの口） |
| summary-cite | `crates/task-core/src/chat/`（`ChatCitation`）、`crates/task-core/src/strategy*`（`chat` 参照の欄）、`crates/task-api/src/cos/strategy_gate*`、dispatcher の添付の検査 |
| web-discuss | D9 の file |
