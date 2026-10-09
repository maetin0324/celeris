# cos-operator: 操作の表と本文の形

`/cos/operations` に送る操作の一覧と本文の形。起票・回答・決定・承認・gate・コメント・KB・pin の
本文を書くときに読む。経路・idempotency・409 の規則は Core と SKILL.md にある。

## 共通の形

`POST /api/v1/cos/operations` の本文。認証ヘッダは run credential（値を prompt・ログ・チャットに書き写さない）。

```sh
curl -sS -X POST "$API/api/v1/cos/operations" -H "Authorization: Bearer $CELERIS_RUN_TOKEN" \
  -H 'Content-Type: application/json' -d @op.json
```

```json
{"idempotency_key":"<thread 内で一意>","expected_revision":null,
 "reason":"<空でない理由>","policy_version":"4",
 "request":{"method":"POST","path":"<下の表の path>","body":{}}}
```

## 登録済みの操作（`crates/task-api/src/cos/ops/*.rs` の `ALLOWED` の連結と同じ）

`request.path` に使えるのはこの表の行だけ。これ以外（外部 URL・任意の proxy・`/cos/*` 自身・未登録の path）は
422 `cos_operation_not_allowed` で拒否され、拒否も記録される。表と `ALLOWED` の一致は試験
`cos_operator_skill_table_matches_allowed`（task-api）が固定している。

### tasks

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| 起票 | `POST /api/v1/tasks` | task.create | `celerisctl add --title … --objective … --check-cmd …`。本文は下の「起票」。添付は `attachment_ids`（[attachments.md](attachments.md)） |
| コメント | `POST /api/v1/tasks/<id>/comments` | comment.create | `{"body":"…"}` |
| 段の確認（phase gate） | `POST /api/v1/tasks/<id>/execution/phase-gate` | execution.phase_gate | `{"action":"continue"\|"replan"\|"withdraw","note":"…"}` |
| task の質問への回答 | `POST /api/v1/tasks/<id>/answer` | question.answer | `celerisctl answer <task-id> <答え>` |
| task の accept | `POST /api/v1/tasks/<id>/accept` | task.accept | body は `{}` |
| task の approve | `POST /api/v1/tasks/<id>/approve` | task.approve | `{"note":"…"}` |
| task の reject | `POST /api/v1/tasks/<id>/reject` | task.reject | `{"note":"…"}` |
| task の cancel | `POST /api/v1/tasks/<id>/cancel` | task.cancel | `{"expected_status":"…"}`（省略可） |
| 計画の確認（plan gate） | `POST /api/v1/tasks/<id>/execution/plan-gate` | execution.plan_gate | `{"action":"continue"\|"replan"\|"withdraw","note":"…"}`。replan は gate の `replan` で planner に返す |
| task の編集（受け入れ条件・題・担当など） | `PATCH /api/v1/tasks/<id>` | task.update | 変える欄だけを送る（`PATCH /tasks/{id}` の schema。`expected_status` で競合を防ぐ）。`Edited.by` は `cos` |
| task の再開（done/failed → ready） | `POST /api/v1/tasks/<id>/reopen` | task.reopen | `{"expected_status":"failed"}` か `{}` |
| task のやり直し（failed/cancelled を複製） | `POST /api/v1/tasks/<id>/retry` | task.retry | `{"accept":true}`（省略なら draft）。`workspace`・`execution` も人の経路と同じ |
| task の再レビュー依頼 | `POST /api/v1/tasks/<id>/rereview` | task.rereview | `{"expected_status":"done"}` |
| task の subtree の一時停止 | `POST /api/v1/tasks/<id>/pause` | task.pause | body は `{}`。走っている run は止めない |
| 一時停止の解除 | `POST /api/v1/tasks/<id>/resume` | task.resume | body は `{}` |
| 初回の実行計画採用 | `POST /api/v1/tasks/<id>/execution-plan` | execution.plan_adopt | 計画 JSON。active な計画が無い task 用。`celerisctl execution plan set` |
| 実行計画の差し替え（replan） | `PUT /api/v1/tasks/<id>/execution-plan` | execution.put_plan | 本文は計画 JSON（`celeris.execution-plan/*`）。active な計画がある task だけ |
| execution を compound / atomic に切替 | `POST /api/v1/tasks/<id>/execution/decompose` | execution.decompose | `{"mode":"compound\|atomic","note":"…"}` |
| 計画 unit への task 採用 | `POST /api/v1/tasks/<id>/tree/adopt` | tree.adopt | `{"task_id":"…","stage":"…","unit_key":"…"}`。`celerisctl tree adopt` |

### decisions

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| 決定（decision）への回答 | `POST /api/v1/decisions/<id>/answer` | decision.answer | `{"option":"…","note":"…"}` |
| 承認・認可 | `POST /api/v1/approvals/<id>/decide` | approval.decide | `celerisctl approve <task-id> --note …` |
| KB 候補の作成 | `POST /api/v1/knowledge/inbox` | knowledge.record | `celerisctl knowledge record --title … --scope project:<slug> --source …`（CoS credential では API 経由）。下の「KB 候補」 |
| KB 候補の却下 | `POST /api/v1/knowledge/inbox/<id>/reject` | knowledge.reject | body は `{}`（理由は operation の `reason` に書く） |
| 受信箱の件への回答（relay） | `POST /api/v1/inbox/items/<id>/answer` | inbox.answer | Core の「受信箱の件への回答（relay）」。`instructed_by` を付ける |
| 決定の答えの訂正（回答済みの choice） | `POST /api/v1/decisions/<id>/revise` | decision.revise | `{"option":"…","note":"…"}`。既に作られた子は作り直さずコメントで届く |
| 決定の取り下げ（open のもの） | `POST /api/v1/decisions/<id>/withdraw` | decision.withdraw | `{"reason":"…"}` か `{}`。止めていた unit を取り下げる |
| 通知を既読化 | `POST /api/v1/notifications/<id>/read` | notification.read | body は `{}` |
| 通知を全て既読化 | `POST /api/v1/notifications/read-all` | notification.read_all | body は `{}` |
| KB 候補の取り込み（正本へ） | `POST /api/v1/knowledge/inbox/<id>/accept` | knowledge.accept | `{}` か `{"path":"…","overwrite":false}` |

### projects

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| 案件の更新 | `PATCH /api/v1/projects/<id>` | project.update | 変える欄だけを送る |
| 案件の中止 | `POST /api/v1/projects/<id>/cancel` | project.cancel | body は `{}`。非終端の task を全て cancelled にする（`result.cancelled_tasks`） |
| 案件の一時停止 | `POST /api/v1/projects/<id>/pause` | project.pause | body は `{}`。終端・停止中は 409 |
| 案件の再開 | `POST /api/v1/projects/<id>/resume` | project.resume | body は `{}`。停止中でなければ 409 |
| 案件のアーカイブ | `POST /api/v1/projects/<id>/archive` | project.archive | body は `{}`。終端の案件だけ |
| アーカイブの解除 | `POST /api/v1/projects/<id>/unarchive` | project.unarchive | body は `{}` |
| 永続の認可の追加 | `POST /api/v1/standing-rules` | standing_rule.create | `{"rule":"…","node_id":"…"}`（node_id 省略で全員向け）。人の決定で CoS にも許可 |
| 永続の認可の削除 | `DELETE /api/v1/standing-rules/<id>` | standing_rule.delete | body なし。無ければ 404 |
| 報告を既読化（旧入口） | `POST /api/v1/reports/read` | report.read | `{"ids":["…"]}`。対応する通知も既読。新しくは notification.read を使う |
| 通知時刻を進める（旧入口） | `POST /api/v1/reports/notified` | report.notified | body は `{}`。API のメモリの値で、再起動で戻る |

### admin

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| モデルの割り当て（source × tier） | `PUT /api/v1/llm/models/assignments/<source>/<tier>` | model_assignment.put | `{"model_id":"…","note":"…"}`。model は catalog にあるもの |
| 割り当ての解除 | `DELETE /api/v1/llm/models/assignments/<source>/<tier>` | model_assignment.delete | body なし（無ければ 404） |
| 定期実行の作成 | `POST /api/v1/cron-jobs` | cron_job.create | `{"name","schedule","timezone","template",…}`（`docs/api/cron-jobs.md`）。名前の重複は 409 |
| 定期実行の更新 | `PATCH /api/v1/cron-jobs/<id>` | cron_job.update | `<id>` は ULID か name。変える欄だけ。有効/無効は pause・resume で変える |
| 定期実行の削除 | `DELETE /api/v1/cron-jobs/<id>` | cron_job.delete | body なし。履歴も消え、作った task は残る |
| 定期実行の一時停止 | `POST /api/v1/cron-jobs/<id>/pause` | cron_job.pause | body は `{}` |
| 定期実行の再開 | `POST /api/v1/cron-jobs/<id>/resume` | cron_job.resume | body は `{}` |
| 定期実行の手動実行 | `POST /api/v1/cron-jobs/<id>/run` | cron_job.run | body は `{}`。外部効果扱い（pending → applied）。同じ idempotency_key の再送は再実行しない |

### surface

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| 添付の pin（既存の owner へ後から） | `POST /api/v1/chat/attachments/<id>/references` | attachment.reference | [attachments.md](attachments.md) |

## 登録されていない操作（CoS は送らない）

未登録の変更操作は `/cos/operations` に送れない。送ると 422 `cos_operation_not_allowed` になり、拒否も理由付きで記録される。

- **除外（人の決定。今後も登録しない）**: 秘密の値を扱う操作（`/secrets/*`・`/accounts/<id>/login*`・
  `/clusters/<id>/connect*`・browser の credential/attestation 系）、`/console/instruct`、`/cos/*` 自身、撤去済みの入口。
  人に web の画面で行うよう頼む。
- **登録待ち（許可範囲は ADR 2026-10-09 で決定済み。領域別に実装中）**: 上の表に無い変更操作
  （例: task の integrate・PR merge、案件の作成・docs・org・provider・release promote・
  `/reload`・browser 操作など）。必要なら人に web の画面で行うよう頼むか、運用者向けの task を起票する。
  一覧はエラー本文の `pending in <領域>` と `crates/task-api/src/cos/ops/*.rs` の `PENDING` にある。

## 監視（読み取り）

`celerisctl ls` / `show <id>` / `log <id>`、`GET /api/v1/tasks/<id>`・`/timeline`・`/events`、`GET /api/v1/inbox`、
`GET /api/v1/notifications`、`GET /api/v1/cos/inbox`、`GET /api/v1/cos/operations/{o}`。

## 起票（`POST /api/v1/tasks`）

`title`・`objective`・`acceptance` は必須。`acceptance` は 1 件以上で、欄名は `acceptance`（`criteria`・`checks` ではない）。
各要素は `type` つき。`human` だけの条件は 422 なので `reviewer` か `command`・`artifact_exists` を入れる。
担当・model は書かない（割り当ては Celeris が決める）。

**リポジトリを使う task は `project_id` と `repos` を付けて起票する。** `genre: "coding"` の task、
または objective・acceptance が repo 内の path や `cargo`・`pnpm`・`git` 等を前提にする task が対象。
`GET /api/v1/projects` と `GET /api/v1/projects/<id>/repos` で案件 ID と登録 repo 名を確認し、
`repos` は名前の配列（例: `["agent-platform"]`）で送る。thread の案件から自動で付くと考えない。
案件や repo を特定できなければ起票前に確認する。workspace の path だけを書いて代用しない。
不足は 422 `repository_required` で、task は作られない。本文を補って新しい idempotency_key で送る。
既存の未実行・blocked の task（子も含む）は、人が `PATCH /api/v1/tasks/<id>` に
`{"project_id":"<案件 ID>","repos":["agent-platform"]}` を送れば修復できる。blocked の解除は別操作。
PATCH は現在 CoS の操作表に無いので、CoS が直接実行する別経路を探さない。

```json
{"idempotency_key":"create-screen-fix-1","expected_revision":null,
 "reason":"人がチャットで画面の修正を依頼した（seq 12）","policy_version":"4",
 "request":{"method":"POST","path":"/api/v1/tasks","body":{
   "title":"画面修正","objective":"依頼の全文",
   "genre":"coding","project_id":"<確認した案件 ID>","repos":["agent-platform"],
   "acceptance":[{"type":"reviewer","text":"添付の screenshot の崩れが直っている"}]}}}
```

`acceptance` の要素: `{"type":"reviewer","text":"…"}`・`{"type":"command","cmd":"…","expect_exit":0}`・
`{"type":"artifact_exists","name":"…"}`・`{"type":"knowledge_page","path":"…"}`・`{"type":"human","text":"…"}`。

## KB 候補（`POST /api/v1/knowledge/inbox`）

```json
{"idempotency_key":"kb-fern03-manual-1","expected_revision":null,
 "reason":"人がチャットで手順を KB に残すよう依頼した（seq 14）","policy_version":"4",
 "request":{"method":"POST","path":"/api/v1/knowledge/inbox","body":{
   "title":"fern03 の使い方","scope":"project:agent-platform",
   "body":"要点（Markdown）","sources":["message:<message id>"],
   "tags":["fern03"],"confidence":"high","attachment_ids":[]}}}
```

- **scope は `user` / `environment` / `environment/<分類>` / `experience` / `project:<slug>` のどれか**。
  `projects/<slug>` は KB の置き場の名前で、scope には書かない。KB の根へ直接書かない。
- 検索・読取りは `celerisctl knowledge search <語>` / `get <path>`。
- 422 `validation`（scope・sources・本文）は本文を直して別の key で送り直す。

## 本文の形が分からないとき

`request.body` は指定先の既存 JSON schema で検証される。推測で送らず、`docs/api/v1/` の schema
（[gui-api.md](../../../docs/api/v1/gui-api.md)）を読む。422 `validation` は本文の形の誤り。
