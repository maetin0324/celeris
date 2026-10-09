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

## 登録済みの操作（`crates/task-api/src/cos/operations.rs` の `ALLOWED` と同じ）

`request.path` に使えるのはこの表の行だけ。これ以外（外部 URL・任意の proxy・`/cos/*` 自身・未登録の path）は
422 `cos_operation_not_allowed` で拒否され、拒否も記録される。表と `ALLOWED` の一致は試験
`cos_operator_skill_table_matches_allowed`（task-api）が固定している。

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| 起票 | `POST /api/v1/tasks` | task.create | `celerisctl add --title … --objective … --check-cmd …`。本文は下の「起票」。添付は `attachment_ids`（[attachments.md](attachments.md)） |
| コメント | `POST /api/v1/tasks/<id>/comments` | comment.create | `{"body":"…"}` |
| 決定（decision）への回答 | `POST /api/v1/decisions/<id>/answer` | decision.answer | `{"option":"…","note":"…"}` |
| 承認・認可 | `POST /api/v1/approvals/<id>/decide` | approval.decide | `celerisctl approve <task-id> --note …` |
| 段の確認（phase gate） | `POST /api/v1/tasks/<id>/execution/phase-gate` | execution.phase_gate | `{"action":"continue"\|"replan"\|"withdraw","note":"…"}` |
| task の質問への回答 | `POST /api/v1/tasks/<id>/answer` | question.answer | `celerisctl answer <task-id> <答え>` |
| 計画の確認（plan gate） | `POST /api/v1/tasks/<id>/execution/plan-gate` | execution.plan_gate | `{"action":"continue"\|"replan"\|"withdraw","note":"…"}`。replan は gate の `replan` で planner に返す |
| 案件の更新 | `PATCH /api/v1/projects/<id>` | project.update | 変える欄だけを送る |
| KB 候補の作成 | `POST /api/v1/knowledge/inbox` | knowledge.record | `celerisctl knowledge record --title … --scope project:<slug> --source …`（CoS credential では API 経由）。下の「KB 候補」 |
| KB 候補の却下 | `POST /api/v1/knowledge/inbox/<id>/reject` | knowledge.reject | body は `{}`（理由は operation の `reason` に書く） |
| 添付の pin（既存の owner へ後から） | `POST /api/v1/chat/attachments/<id>/references` | attachment.reference | [attachments.md](attachments.md) |
| 受信箱の件への回答（relay） | `POST /api/v1/inbox/items/<id>/answer` | inbox.answer | Core の「受信箱の件への回答（relay）」。`instructed_by` を付ける |

### 登録されていない操作（CoS は送らない）

次は `/cos/operations` に登録されていない。送ると 422 になる。必要なら人に web の画面で行うよう頼むか、
運用者向けの task を起票する。登録を足すのは権限の変更なので人の決定。

- 実行計画の直接の差し替え（`PUT /api/v1/tasks/<id>/execution-plan`）。現計画は `celerisctl execution plan show <task-id>` で読める。直したいときは plan/phase gate の `replan`
- task・案件の pause / resume（`POST /api/v1/tasks/<id>/pause`・`…/resume`、`POST /api/v1/projects/<id>/pause`）
- 永続の認可（`/api/v1/standing-rules`）の作成・変更。読むのは `GET /api/v1/standing-rules`。新しい standing permission は security として人に回す

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
