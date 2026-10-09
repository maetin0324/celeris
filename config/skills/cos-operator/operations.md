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

`request.path` に使えるのはこの表の行だけ。API の変更操作はこの表か下の「除外する操作」のどちらか一方にある。
それ以外（外部 URL・任意の proxy・存在しない path）は
422 `cos_operation_not_allowed` で拒否され、拒否も記録される。表と `ALLOWED` の一致は試験
`cos_operator_skill_table_matches_allowed`（task-api）が固定している。

### tasks

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| 起票 | `POST /api/v1/tasks` | task.create | `celerisctl add --title … --objective … --check-cmd …`。本文は下の「起票」。添付は `attachment_ids`（[attachments.md](attachments.md)） |
| コメント | `POST /api/v1/tasks/<id>/comments` | comment.create | `{"body":"…"}` |
| 成果の取り込み（merge・PR・破棄） | `POST /api/v1/tasks/<id>/changes/<repo>/integrate` | task.integrate | `{"method":"merge"\|"pr"\|"discard","confirm":true,"note":"…"}`（discard は confirm 必須）。外部効果（git・GitHub）: pending → applied、再送は再実行しない |
| PR を Celeris で merge | `POST /api/v1/tasks/<id>/changes/<repo>/pr/merge` | task.pr_merge | body は `{}`。開いた PR だけ（他は 409）。外部効果 |
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
| KB ページの編集（正本） | `PUT /api/v1/knowledge/page` | knowledge.page_put | `{"path":"…","body":"…","etag":"…","message":"…"}`。外部効果（KB の git）: pending → applied。etag 不一致は 409・rejected。`_inbox/` は 403 |

### projects

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| 案件の作成 | `POST /api/v1/projects` | project.create | `{"title":"…","request":"…","workspace":{…}}`（workspace は任意）。作成後に秘書の最初の返事が起きる |
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
| 文書リポジトリの用意 | `POST /api/v1/projects/<id>/docs/init` | docs.init | body は `{}`。外部効果（git）扱い: pending → applied、再送は再実行しない |
| 文書ページの作成・更新 | `PUT /api/v1/projects/<id>/docs/page` | docs.page_put | `{"path":"docs/x.md","body":"…","etag":"…","message":"…"}`。既存ページは etag 必須（違えば 409、rejected で記録） |
| 文書ページの削除 | `DELETE /api/v1/projects/<id>/docs/page` | docs.page_delete | `{"path":"…","etag":"…"}` を body で送る（operation の path に query は付けられない） |
| 文書整理（audit・adopt・approve・apply） | `POST /api/v1/projects/<id>/docs/maintenance` | docs.maintenance | `{"op":"audit"}` など。記録の action は `docs.maintenance_<op>`。apply は人の approve 済みの plan だけ |

### admin

| 操作 | `request` の method と path | action | celerisctl・本文の要点 |
|---|---|---|---|
| モデルの割り当て（source × tier） | `PUT /api/v1/llm/models/assignments/<source>/<tier>` | model_assignment.put | `{"model_id":"…","note":"…"}`。model は catalog にあるもの |
| 割り当ての解除 | `DELETE /api/v1/llm/models/assignments/<source>/<tier>` | model_assignment.delete | body なし（無ければ 404） |
| 割り当て変更の影響の下見 | `POST /api/v1/llm/models/assignments/preview` | model_assignment.preview | `{"source","tier","model_id"?}`。書き込まない（影響だけ記録） |
| 役割の構成員の置き換え | `PUT /api/v1/llm/models/assignments/roles/<tier>` | model_role.replace | `{"members":[{"source","model_id","priority"}]}`。全 source の構成員を丸ごと置き換える。空で役割を止める |
| 役割の置き換えの下見 | `POST /api/v1/llm/models/assignments/roles/<tier>/preview` | model_role.preview | 本文は置き換えと同じ。書き込まない |
| モデルの上書き | `PUT /api/v1/llm/models/<source>/<model_id>/override` | model_override.put | `{"disabled","tier","alias","note"}`（全て任意） |
| モデルの上書きの削除 | `DELETE /api/v1/llm/models/<source>/<model_id>/override` | model_override.delete | body なし（無ければ 404） |
| 組織のノードの作成 | `POST /api/v1/org` | org.create | `{"id","name","kind","parent_id"?,"genre"?,"brief"?,"profile"?,"position"?}`。id の重複は 409 |
| 組織のノードの更新 | `PATCH /api/v1/org/<id>` | org.update | 変える欄だけ。`profile` は丸ごと差し替え |
| 組織のノードの削除 | `DELETE /api/v1/org/<id>` | org.delete | body なし。未終了の task か子ノードがあれば 409 |
| browser 設定の変更 | `PATCH /api/v1/org/<id>/browser-settings` | org.browser_settings | `{"allowed_domains","approval_actions","credential_policy_ids",…}`。grant の無いノードは 422 |
| skill の mount | `POST /api/v1/org/<id>/skills` | org.skill_mount | `{"skill":"<name>"}` |
| skill の unmount | `DELETE /api/v1/org/<id>/skills/<skill>` | org.skill_unmount | body なし |
| skill の作成・更新 | `PUT /api/v1/skills/<name>` | skill.put | `{"skill_md":"…","files":[{"path","content"}]}`。KB の git 書き込みなので外部効果扱い（pending → applied） |
| skill の削除 | `DELETE /api/v1/skills/<name>` | skill.delete | body なし。mount されていれば 409。外部効果扱い |
| 案件へのリポジトリの追加 | `POST /api/v1/projects/<id>/repos` | repo.create | `{"location":{"kind":"local","path":"…"},"name"?,"kind"?,"default_branch"?,"is_primary"?}`。最初の 1 件は primary |
| リポジトリの更新 | `PATCH /api/v1/repos/<id>` | repo.update | 変える欄だけ（1 つ以上） |
| リポジトリの削除 | `DELETE /api/v1/repos/<id>` | repo.delete | body なし。未終了の task が使っていれば 409 |
| クラスタの作業ディレクトリ | `PUT /api/v1/clusters/<id>/settings` | cluster.settings_put | `{"work_dir":"/abs/path"}`。`null` で設定ファイルの値に戻す |
| provider の作成 | `POST /api/v1/providers` | provider.create | `{"id","adapter","kind"?,"llm_source"?,"model"?,…}`。`env` に API key などの秘密の値を入れない（422 `secret_operations`、本文は記録されない） |
| provider の更新 | `PATCH /api/v1/providers/<id>` | provider.update | 変える欄だけ。秘密の値は同じく不可 |
| provider の削除 | `DELETE /api/v1/providers/<id>` | provider.delete | body なし（無ければ 404） |
| provider の疎通確認 | `POST /api/v1/providers/<id>/check` | provider.check | body は `{}`。daemon 経由の外部効果（pending → applied） |
| アカウントの作成 | `POST /api/v1/accounts` | account.create | `{"id","adapter"?}`（adapter 既定 claude-code）。ログインは人（除外） |
| アカウントの退避 | `DELETE /api/v1/accounts/<id>` | account.delete | `{"adapter"?}`（query の代わりに body）。daemon が移す外部効果。使用中は 409 |
| アカウントの確認 | `POST /api/v1/accounts/<id>/check` | account.check | `{"adapter"?}`。外部効果 |
| モデルの発見 | `POST /api/v1/llm/models/discover` | model_catalog.discover | `{"source"?}`。外部効果（daemon の発見を 1 回走らせる） |
| 通知のテスト送信 | `POST /api/v1/notify/test` | notify.test | body なし。Discord に 1 通送る外部効果 |
| 設定の再読み込み | `POST /api/v1/reload` | daemon.reload | body は `{}`。外部効果。設定の誤りは 400 で rejected |
| replay の検査 | `POST /api/v1/replay` | daemon.replay | body は `{}`。同時に 1 つだけ（実行中は 409） |
| release の昇格 | `POST /api/v1/releases/<sha12>/promote` | release.promote | body なし。人の依頼があり、`verify.json.ok` と `live_ok` を確かめた後だけ（[production.md](production.md)）。外部効果 |
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
| chat thread の作成 | `POST /api/v1/chat/threads` | chat.thread_create | `{"title","project_id"?,"client_thread_id"}`。同じ client_thread_id・同じ内容は同じ thread |
| chat thread の改名・アーカイブ | `PATCH /api/v1/chat/threads/<t>` | chat.thread_update | `{"title"?,"status"?:"open"\|"archived","expected_revision"}`。受信箱 thread はアーカイブ不可 |
| chat thread への書き込み | `POST /api/v1/chat/threads/<t>/messages` | chat.message_post | `{"client_message_id","text","attachment_ids":[],"reply_to_id":null,"mode":"queue","resume_queue":false}`。CoS の書き込みは完了済みの assistant 発言として入り、run を起こさない（`mode=interrupt`・`resume_queue=true` は 422） |
| 待ち行列の入力の取消 | `DELETE /api/v1/chat/threads/<t>/messages/<m>` | chat.message_cancel | body なし。queued の user 入力だけ |
| chat run の停止 | `POST /api/v1/chat/threads/<t>/stop` | chat.run_stop | `{"run_id"}`。待ち行列は一時停止になる |
| 待ち行列の再開 | `POST /api/v1/chat/threads/<t>/resume-queue` | chat.queue_resume | `{"expected_revision"}` |
| chat 添付のアップロード | `POST /api/v1/chat/threads/<t>/attachments` | chat.attachment_upload | `{"client_upload_id","name","content_base64"}`。外部効果（file）。記録に中身は残らない |
| chat 添付の削除 | `DELETE /api/v1/chat/attachments/<a>` | chat.attachment_delete | body なし。参照のある添付は 409。外部効果（file） |
| 互換 console の新しい会話 | `POST /api/v1/console/new-conversation` | console.new_conversation | `{"scope":"all"\|"project:<id>"\|"node:cos"}`（省略は all） |
| 組織のノードへ話しかける | `POST /api/v1/org/<id>/messages` | org.message | `{"text","project_id"?}`。対話用 task を作り run を起こす外部効果。CoS ノード自身へは 422 `cos_self_chain` |
| 成果物を文書へ昇格 | `POST /api/v1/tasks/<id>/artifacts/promote` | artifact.promote | `{"name","path","title"?,"overwrite"?}`。外部効果（docs git） |
| browser site policy の作成・置換 | `PUT /api/v1/browser/site-policies/<policy_id>` | browser.site_policy_put | `{"exact_origin","login_url","password_selector","submit_selector"?}`。外部効果 |
| browser site policy の削除 | `DELETE /api/v1/browser/site-policies/<policy_id>` | browser.site_policy_delete | body なし。参照中は 409。外部効果 |
| task の browser policy | `PUT /api/v1/tasks/<id>/browser/policy` | browser.task_policy_put | 本文は policy の JSON。外部効果 |
| browser の待ちを開く | `POST /api/v1/tasks/<id>/browser/requests` | browser.request_open | `NewBrowserWait`。credential・credential_policy_id・trusted_login を含む待ちは除外（422 `browser_credential_attestation`）。外部効果 |
| browser の制御（pause・takeover 等） | `POST /api/v1/tasks/<id>/browser/control/<run>/<session>` | browser.control | `{"assertion","command","expected_version","idempotency_key"}`。owner session の署名付き assertion・origin・lease の検査は route と同じ（記録では assertion を伏せる）。外部効果 |
| browser の人の切断 | `POST /api/v1/tasks/<id>/browser/control/<run>/<session>/disconnect` | browser.control_disconnect | `{"assertion"}`。外部効果 |
| browser の agent 操作の開始 | `POST /api/v1/tasks/<id>/browser/control/<run>/<session>/agent/begin` | browser.agent_begin | body なし。daemon 認証のある構成だけ。外部効果 |
| browser の agent 操作の終了 | `POST /api/v1/tasks/<id>/browser/control/<run>/<session>/agent/end` | browser.agent_end | body なし。外部効果 |
| browser の認証区間 | `POST /api/v1/tasks/<id>/browser/control/<run>/<session>/auth-section` | browser.auth_section | `{"active":true\|false}`。外部効果 |
| browser の live event | `POST /api/v1/tasks/<id>/browser/live/<run>/<session>/events` | browser.live_event | `{"kind":"status"\|"tabs"\|"url"\|"console",…}`。run が動いている間だけ。外部効果 |

## 除外する操作（人の決定。CoS は送らない）

次の変更操作は `/cos/operations` に送れない（`crates/task-api/src/cos/ops/*.rs` の `EXCLUDED`）。送ると 422
`cos_operation_not_allowed` と下の理由が返り、拒否も理由付きで記録される。人に web の画面で行うよう頼む。

| 系列 | path | 理由 |
|---|---|---|
| `secret_operations` | `/secrets/<id>`（PUT・DELETE）、`/accounts/<id>/login*`、`/clusters/<id>/connect*` | 秘密の値を扱う（取消・削除も含め全て除外） |
| `browser_credential_attestation` | `/browser/identities*`、`/browser/trusted-devices*`、`/tasks/<id>/browser/waits/<w>/{credential,decision,registered,revoke}`、`/tasks/<id>/browser/live/<run>/<session>/{check,grant,read}` | credential・封緘 state・receipt・owner attestation／信頼端末を扱う |
| `console_instruction_chain` | `POST /console/instruct` | 別の指示経路へ入り監査の鎖が二重になる |
| `recursive_cos` | `/cos/operations`・`/cos/operations/<o>/override`・`/cos/inbox/<i>/resolve`・`/cos/threads/<t>/checkpoint` | CoS 制御 API の再帰。受信箱の解決は envelope の外で直接呼ぶ |
| `removed_by_adr_0079` | `POST /plans`、`/milestones/*`、`/projects/<id>/{milestones,plan,project-plan/<v>/decide}` | 撤去済みの 410 の互換入口 |

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
既存の未実行・blocked の task（子も含む）は `PATCH /api/v1/tasks/<id>` に
`{"project_id":"<案件 ID>","repos":["agent-platform"]}` を送れば修復できる。blocked の解除は別操作。
CoS も上の表の `PATCH /api/v1/tasks/<id>`（task.update）で同じ修復を監査付きで送れる。

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
