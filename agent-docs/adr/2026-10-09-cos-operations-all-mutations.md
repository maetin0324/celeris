# ADR 2026-10-09: CoS の全 API 変更操作を監査付き操作層へ登録する

---
tasks: [01M4F24B0GAEVZQPP35830PA0F, 01M4F5KS8E1MXZDNJTRAVFJESW]
---

- 日付: 2026-10-09
- 状態: 決定済み（設計。registry・各領域・ctl・skill の実装と検証は後続 WorkUnit）
- 関連: [CoS チャットホーム ADR](2026-10-05-cos-chat-home.md) D3、[ADR-0079](0079-recursive-task-decomposition.md) D7/D13、[API v1](../../docs/api/v1/gui-api.md)

## D1 人の決定と原因

**CoS は API で人ができる変更操作を全て `/cos/operations` 経由で監査付きで行える。**
2026-10-08〜09 の CoS thread `01M4D7BWDEC0WH5JCEZKC8VW56` seq 36・38 の人の依頼と、
この task の execution plan decisions に対する人の回答を正本とする。UI に画面があるかは範囲に影響しない。
`POST /decisions/{id}/revise` の拒否で task `01M4D7RVKXY9A967ZXDADWVHQJ` が止まった事例も解消対象である。

CoS の chat run は claude-code の `bypassPermissions` で動いていた。原因は harness の権限モードでなく、
`crates/task-api/src/cos/operations.rs` の `ALLOWED` が 12 操作に限られていることである。
未登録の操作は 422 `cos_operation_not_allowed`、CoS credential で領域 API を直接呼ぶと
`cos/mod.rs` により 422 `cos_audit_context_required` になる。skill の変更だけでは解消しない。
既存 12 操作を維持し、D2 の例外を除き D3 の全行を登録する。直接の変更 API の拒否も維持する。

| decision key | 人の回答 | 確定する範囲 |
|---|---|---|
| promote | CoS にも許す（監査付き） | `POST /releases/{sha12}/promote` を admin に登録。selfdeploy の人専用記述との整合は ops-admin の仕事 |
| standing-rules | CoS にも許す（監査付き） | 作成・削除を projects に登録。D3 の旧「新しい standing permission は常に人へ」の一般基準をこの明示決定で更新 |
| secrets | 全て除外する | `/secrets/*`・account login・cluster connect・browser credential/attestation 系。metadata のみの取消・失効・削除も除外 |
| browser-control | 許す（監査付き） | control 接続・切断・agent begin/end・auth-section の 5 route を surface に登録 |
| daemon-wide | reload と notify/test は許し、console/instruct は除外 | 前者 2 本は admin、後者は surface の理由付き除外 |
| destructive | 他と同じ条件で許す | DELETE 全般に新しい expected_revision 必須化や一律 human_required を足さない。secrets 等の明示除外は優先 |

この決定は API 操作範囲の契約であり、この WorkUnit が本番 host を操作する許可ではない。
本番 promote・reload・再起動・本番 DB 変更はこの run で行わず、後続の反映も人が実行する手順と check を用意する
（ADR-0095 付記 D-d）。設計済みと実装済みを区別する。

## D2 除外する method/path と理由

以下が EXCLUDED の確定集合である。パスは `/api/v1` を省略する。取消・削除だけを許可する例外は作らない。
`browser-control=include` は secrets の一般除外に対する明示的な例外で、control の署名付き assertion 自体を
CoS が生成・偽装できるという意味ではない。既存の owner session・origin・lease・署名検証は残す。
control の auth-section は区間の状態変更であり、credential の登録や開封とは別の操作である。
Live View の grant/check/read は owner attestation を入力するため除外し、daemon の event 追記は含める。

撤去済み 9 route の除外は新しい権限縮小ではない。人にも変更できない既存 410 の入口を列挙したもので、
領域 API の 410 は維持し、CoS envelope からの要求だけを理由付き 422 と rejected 行で記録する。

| method | path | 理由コード | 理由 |
|---|---|---|---|
| DELETE | `/accounts/{id}/login` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| POST | `/accounts/{id}/login` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| POST | `/accounts/{id}/login/code` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| DELETE | `/clusters/{id}/connect` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| POST | `/clusters/{id}/connect` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| POST | `/clusters/{id}/connect/code` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| DELETE | `/secrets/{id}` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| PUT | `/secrets/{id}` | `secret_operations` | 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。 |
| PATCH | `/milestones/{id}` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/milestones/{id}/cancel` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/milestones/{id}/decide` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/milestones/{id}/pause` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/milestones/{id}/resume` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/projects/{id}/milestones` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/projects/{id}/plan` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/projects/{id}/project-plan/{version}/decide` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |
| POST | `/browser/identities` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| DELETE | `/browser/identities/{id}` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/browser/identities/{id}/restore` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/browser/identities/{id}/revoke` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/browser/trusted-devices` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/browser/trusted-devices/verify` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| DELETE | `/browser/trusted-devices/{id}` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/console/instruct` | `console_instruction_chain` | 人の決定 daemon-wide=no-console。別の指示経路へ入り監査の鎖が二重になる。 |
| POST | `/cos/inbox/{i}/resolve` | `recursive_cos` | 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。 |
| POST | `/cos/operations` | `recursive_cos` | 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。 |
| POST | `/cos/operations/{o}/override` | `recursive_cos` | 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。 |
| POST | `/cos/threads/{t}/checkpoint` | `recursive_cos` | 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。 |
| POST | `/tasks/{id}/browser/live/{run}/{session}/check` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/tasks/{id}/browser/live/{run}/{session}/grant` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/tasks/{id}/browser/live/{run}/{session}/read` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/credential` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/decision` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/registered` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/revoke` | `browser_credential_attestation` | 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。 |
| POST | `/plans` | `removed_by_adr_0079` | ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。 |

`/cos/*` は上の現存 4 route だけでなく subtree 全体を再帰対象として拒否する（method に依らない）。
checkpoint・resolve は本来の CoS 制御 API として直接使う。人の override を操作 envelope に内包しない。
未知の `/cos/*`・外部 URL・scheme/host・`..`・未登録 method/path も既存の検証で拒否し、理由を記録する。
秘密の除外では受信した body の秘密・assertion・cookie・receipt を rejected payload/detail/card に保存しない。
要求 hash による冪等検査と秘密の保存は区別し、拒否前に監査 payload を伏せる。

## D3 全変更エンドポイントの領域割当（registry と子 task の共通契約）

棚卸し基準は base `409625f5` の `crates/task-api/src/handlers.rs` と merge される各 `routes()`、
および `docs/api/v1/gui-api.md` §3 の endpoint 表（browser control の補足表を含む）の和集合。
POST / PUT / PATCH / DELETE を method/path ごとに数えた。`put_route` 別名と browser control の BASE/format! を展開し、
`{a}`/`{id}` 等の引数名の差は同じ route として照合した。query（docs/page の path/etag 等）は
route key に含めず、共有処理の既存検査へ渡す。GET はこの変更表に含めない。
**142 行、router と文書で共通 128 行、router のみ 14 行、文書のみ 0 行**。
文書にない route も落とさない（cron、notifications、inbox answer、role assignment、site policy 等）。
実在しない project/milestone DELETE を人の回答の例示から新設しない。

A は監査付き許可、R は書込みをしない POST だが method の網羅性のため監査付き許可、X は D2 の理由付き除外。
foundation は A/R の既存登録を ALLOWED に、未実装行を PENDING に、X を EXCLUDED に置く。
verify では PENDING を消し、全 142 行が ALLOWED/EXCLUDED の一方だけに入ることを検査する。
R の操作では領域 write は不要だが operation・event・card は同一 transaction に記録する。
今後の route 追加では同時にこの範囲契約と registry・ctl・skill を更新する。

| registry 領域 | 担当 unit | 境界 | 全行 | 許可 A/R | 除外 X |
|---|---|---|---|---|---|
| tasks | ops-tasks | task・comments・execution・tree adopt・task changes・旧 plans | 22 | 21 | 1 |
| decisions | ops-decisions | decision・approval・inbox・knowledge・notifications | 11 | 11 | 0 |
| projects | ops-projects | project・milestone・旧 project plan・project docs・reports・standing rules | 23 | 15 | 8 |
| admin | ops-admin | provider・account・cluster・secret・org 設定・repo・skill・LLM・cron・release・daemon | 45 | 37 | 8 |
| surface | ops-surface | chat・添付・成果物ページ昇格・org messages・console・browser・cos の再帰除外 | 41 | 22 | 19 |

領域は route の先頭だけで決めない。`projects/.../repos` は admin、`org/.../messages` は surface、
`org/.../browser-settings` は admin、task 下の browser と artifacts/promote は surface、project docs は projects。
以下の各行が唯一の所有者を示す。action の domain は D4 の資源名であり registry 領域名とは別である。

### tasks

| method | path | 判定 | 根拠となる router module | API 表 |
|---|---|---|---|---|
| POST | `/plans` | X: `removed_by_adr_0079` | `handlers.rs` | 共通 |
| POST | `/tasks` | A | `handlers.rs` | 共通 |
| PATCH | `/tasks/{id}` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/accept` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/answer` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/approve` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/cancel` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/changes/{repo}/integrate` | A | `changes.rs` | 共通 |
| POST | `/tasks/{id}/changes/{repo}/pr/merge` | A | `changes.rs` | 共通 |
| POST | `/tasks/{id}/comments` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/execution-plan` | A | `execution.rs` | 共通 |
| PUT | `/tasks/{id}/execution-plan` | A | `execution.rs` | 共通 |
| POST | `/tasks/{id}/execution/decompose` | A | `execution.rs` | 共通 |
| POST | `/tasks/{id}/execution/phase-gate` | A | `execution.rs` | 共通 |
| POST | `/tasks/{id}/execution/plan-gate` | A | `execution.rs` | 共通 |
| POST | `/tasks/{id}/pause` | A | `lifecycle.rs` | 共通 |
| POST | `/tasks/{id}/reject` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/reopen` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/rereview` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/resume` | A | `lifecycle.rs` | 共通 |
| POST | `/tasks/{id}/retry` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/tree/adopt` | A | `execution.rs` | 共通 |

### decisions

| method | path | 判定 | 根拠となる router module | API 表 |
|---|---|---|---|---|
| POST | `/approvals/{id}/decide` | A | `approvals.rs` | 共通 |
| POST | `/decisions/{id}/answer` | A | `decisions.rs` | 共通 |
| POST | `/decisions/{id}/revise` | A | `decisions.rs` | 共通 |
| POST | `/decisions/{id}/withdraw` | A | `decisions.rs` | 共通 |
| POST | `/inbox/items/{id}/answer` | A | `inbox_notifications.rs` | router のみ |
| POST | `/knowledge/inbox` | A | `knowledge.rs` | 共通 |
| POST | `/knowledge/inbox/{id}/accept` | A | `knowledge.rs` | 共通 |
| POST | `/knowledge/inbox/{id}/reject` | A | `knowledge.rs` | 共通 |
| PUT | `/knowledge/page` | A | `knowledge.rs` | 共通 |
| POST | `/notifications/read-all` | A | `inbox_notifications.rs` | router のみ |
| POST | `/notifications/{id}/read` | A | `inbox_notifications.rs` | router のみ |

### projects

| method | path | 判定 | 根拠となる router module | API 表 |
|---|---|---|---|---|
| PATCH | `/milestones/{id}` | X: `removed_by_adr_0079` | `handlers.rs` | 共通 |
| POST | `/milestones/{id}/cancel` | X: `removed_by_adr_0079` | `lifecycle.rs` | 共通 |
| POST | `/milestones/{id}/decide` | X: `removed_by_adr_0079` | `milestones.rs` | 共通 |
| POST | `/milestones/{id}/pause` | X: `removed_by_adr_0079` | `lifecycle.rs` | 共通 |
| POST | `/milestones/{id}/resume` | X: `removed_by_adr_0079` | `lifecycle.rs` | 共通 |
| POST | `/projects` | A | `handlers.rs` | 共通 |
| PATCH | `/projects/{id}` | A | `handlers.rs` | 共通 |
| POST | `/projects/{id}/archive` | A | `lifecycle.rs` | 共通 |
| POST | `/projects/{id}/cancel` | A | `lifecycle.rs` | 共通 |
| POST | `/projects/{id}/docs/init` | A | `docs.rs` | 共通 |
| POST | `/projects/{id}/docs/maintenance` | A | `docs.rs` | 共通 |
| DELETE | `/projects/{id}/docs/page` | A | `docs.rs` | 共通 |
| PUT | `/projects/{id}/docs/page` | A | `docs.rs` | 共通 |
| POST | `/projects/{id}/milestones` | X: `removed_by_adr_0079` | `handlers.rs` | 共通 |
| POST | `/projects/{id}/pause` | A | `lifecycle.rs` | 共通 |
| POST | `/projects/{id}/plan` | X: `removed_by_adr_0079` | `project_plan.rs` | 共通 |
| POST | `/projects/{id}/project-plan/{version}/decide` | X: `removed_by_adr_0079` | `project_plan.rs` | 共通 |
| POST | `/projects/{id}/resume` | A | `lifecycle.rs` | 共通 |
| POST | `/projects/{id}/unarchive` | A | `lifecycle.rs` | 共通 |
| POST | `/reports/notified` | A | `reports.rs` | 共通 |
| POST | `/reports/read` | A | `reports.rs` | 共通 |
| POST | `/standing-rules` | A | `approvals.rs` | 共通 |
| DELETE | `/standing-rules/{id}` | A | `approvals.rs` | 共通 |

### admin

| method | path | 判定 | 根拠となる router module | API 表 |
|---|---|---|---|---|
| POST | `/accounts` | A | `handlers.rs` | 共通 |
| DELETE | `/accounts/{id}` | A | `handlers.rs` | 共通 |
| POST | `/accounts/{id}/check` | A | `handlers.rs` | 共通 |
| DELETE | `/accounts/{id}/login` | X: `secret_operations` | `handlers.rs` | 共通 |
| POST | `/accounts/{id}/login` | X: `secret_operations` | `handlers.rs` | 共通 |
| POST | `/accounts/{id}/login/code` | X: `secret_operations` | `handlers.rs` | 共通 |
| DELETE | `/clusters/{id}/connect` | X: `secret_operations` | `handlers.rs` | 共通 |
| POST | `/clusters/{id}/connect` | X: `secret_operations` | `handlers.rs` | 共通 |
| POST | `/clusters/{id}/connect/code` | X: `secret_operations` | `handlers.rs` | 共通 |
| PUT | `/clusters/{id}/settings` | A | `handlers.rs` | 共通 |
| POST | `/cron-jobs` | A | `cron_jobs.rs` | router のみ |
| DELETE | `/cron-jobs/{id}` | A | `cron_jobs.rs` | router のみ |
| PATCH | `/cron-jobs/{id}` | A | `cron_jobs.rs` | router のみ |
| POST | `/cron-jobs/{id}/pause` | A | `cron_jobs.rs` | router のみ |
| POST | `/cron-jobs/{id}/resume` | A | `cron_jobs.rs` | router のみ |
| POST | `/cron-jobs/{id}/run` | A | `cron_jobs.rs` | router のみ |
| POST | `/llm/models/assignments/preview` | R | `model_assignments.rs` | 共通 |
| PUT | `/llm/models/assignments/roles/{tier}` | A | `model_assignments.rs` | router のみ |
| POST | `/llm/models/assignments/roles/{tier}/preview` | R | `model_assignments.rs` | router のみ |
| DELETE | `/llm/models/assignments/{source}/{tier}` | A | `model_assignments.rs` | 共通 |
| PUT | `/llm/models/assignments/{source}/{tier}` | A | `model_assignments.rs` | 共通 |
| POST | `/llm/models/discover` | A | `model_catalog.rs` | 共通 |
| DELETE | `/llm/models/{source}/{model_id}/override` | A | `model_catalog.rs` | 共通 |
| PUT | `/llm/models/{source}/{model_id}/override` | A | `model_catalog.rs` | 共通 |
| POST | `/notify/test` | A | `notify.rs` | 共通 |
| POST | `/org` | A | `handlers.rs` | 共通 |
| DELETE | `/org/{id}` | A | `handlers.rs` | 共通 |
| PATCH | `/org/{id}` | A | `handlers.rs` | 共通 |
| PATCH | `/org/{id}/browser-settings` | A | `handlers.rs` | router のみ |
| POST | `/org/{id}/skills` | A | `skills.rs` | 共通 |
| DELETE | `/org/{id}/skills/{skill}` | A | `skills.rs` | 共通 |
| POST | `/projects/{id}/repos` | A | `repos.rs` | 共通 |
| POST | `/providers` | A | `handlers.rs` | 共通 |
| DELETE | `/providers/{id}` | A | `handlers.rs` | 共通 |
| PATCH | `/providers/{id}` | A | `handlers.rs` | 共通 |
| POST | `/providers/{id}/check` | A | `handlers.rs` | 共通 |
| POST | `/releases/{sha12}/promote` | A | `releases.rs` | 共通 |
| POST | `/reload` | A | `handlers.rs` | 共通 |
| POST | `/replay` | R | `handlers.rs` | 共通 |
| DELETE | `/repos/{id}` | A | `repos.rs` | 共通 |
| PATCH | `/repos/{id}` | A | `repos.rs` | 共通 |
| DELETE | `/secrets/{id}` | X: `secret_operations` | `handlers.rs` | 共通 |
| PUT | `/secrets/{id}` | X: `secret_operations` | `handlers.rs` | 共通 |
| DELETE | `/skills/{name}` | A | `skills.rs` | 共通 |
| PUT | `/skills/{name}` | A | `skills.rs` | 共通 |

### surface

| method | path | 判定 | 根拠となる router module | API 表 |
|---|---|---|---|---|
| POST | `/browser/identities` | X: `browser_credential_attestation` | `browser_identity.rs` | 共通 |
| DELETE | `/browser/identities/{id}` | X: `browser_credential_attestation` | `browser_identity.rs` | 共通 |
| POST | `/browser/identities/{id}/restore` | X: `browser_credential_attestation` | `browser_identity.rs` | 共通 |
| POST | `/browser/identities/{id}/revoke` | X: `browser_credential_attestation` | `browser_identity.rs` | 共通 |
| DELETE | `/browser/site-policies/{policy_id}` | A | `browser_site_policies.rs` | router のみ |
| PUT | `/browser/site-policies/{policy_id}` | A | `browser_site_policies.rs` | router のみ |
| POST | `/browser/trusted-devices` | X: `browser_credential_attestation` | `browser_trusted_devices.rs` | 共通 |
| POST | `/browser/trusted-devices/verify` | X: `browser_credential_attestation` | `browser_trusted_devices.rs` | 共通 |
| DELETE | `/browser/trusted-devices/{id}` | X: `browser_credential_attestation` | `browser_trusted_devices.rs` | 共通 |
| DELETE | `/chat/attachments/{a}` | A | `chat/attachments.rs` | 共通 |
| POST | `/chat/attachments/{a}/references` | A | `chat/attachments.rs` | 共通 |
| POST | `/chat/threads` | A | `chat/mod.rs` | 共通 |
| PATCH | `/chat/threads/{t}` | A | `chat/mod.rs` | 共通 |
| POST | `/chat/threads/{t}/attachments` | A | `chat/attachments.rs` | 共通 |
| POST | `/chat/threads/{t}/messages` | A | `chat/mod.rs` | 共通 |
| DELETE | `/chat/threads/{t}/messages/{m}` | A | `chat/mod.rs` | 共通 |
| POST | `/chat/threads/{t}/resume-queue` | A | `chat/mod.rs` | 共通 |
| POST | `/chat/threads/{t}/stop` | A | `chat/mod.rs` | 共通 |
| POST | `/console/instruct` | X: `console_instruction_chain` | `console.rs` | 共通 |
| POST | `/console/new-conversation` | A | `console.rs` | 共通 |
| POST | `/cos/inbox/{i}/resolve` | X: `recursive_cos` | `cos/inbox.rs` | 共通 |
| POST | `/cos/operations` | X: `recursive_cos` | `cos/operations.rs` | 共通 |
| POST | `/cos/operations/{o}/override` | X: `recursive_cos` | `cos/override_op.rs` | 共通 |
| POST | `/cos/threads/{t}/checkpoint` | X: `recursive_cos` | `cos/mod.rs` | 共通 |
| POST | `/org/{id}/messages` | A | `handlers.rs` | 共通 |
| POST | `/tasks/{id}/artifacts/promote` | A | `docs.rs` | 共通 |
| POST | `/tasks/{id}/browser/control/{run}/{session}` | A | `browser_control.rs` | 共通 |
| POST | `/tasks/{id}/browser/control/{run}/{session}/agent/begin` | A | `browser_control.rs` | 共通 |
| POST | `/tasks/{id}/browser/control/{run}/{session}/agent/end` | A | `browser_control.rs` | 共通 |
| POST | `/tasks/{id}/browser/control/{run}/{session}/auth-section` | A | `browser_control.rs` | 共通 |
| POST | `/tasks/{id}/browser/control/{run}/{session}/disconnect` | A | `browser_control.rs` | 共通 |
| POST | `/tasks/{id}/browser/live/{run}/{session}/check` | X: `browser_credential_attestation` | `browser_live.rs` | 共通 |
| POST | `/tasks/{id}/browser/live/{run}/{session}/events` | A | `browser_live.rs` | 共通 |
| POST | `/tasks/{id}/browser/live/{run}/{session}/grant` | X: `browser_credential_attestation` | `browser_live.rs` | 共通 |
| POST | `/tasks/{id}/browser/live/{run}/{session}/read` | X: `browser_credential_attestation` | `browser_live.rs` | 共通 |
| PUT | `/tasks/{id}/browser/policy` | A | `browser.rs` | 共通 |
| POST | `/tasks/{id}/browser/requests` | A | `browser.rs` | 共通 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/credential` | X: `browser_credential_attestation` | `browser.rs` | 共通 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/decision` | X: `browser_credential_attestation` | `browser.rs` | 共通 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/registered` | X: `browser_credential_attestation` | `browser.rs` | 共通 |
| POST | `/tasks/{id}/browser/waits/{wait_id}/revoke` | X: `browser_credential_attestation` | `browser.rs` | 共通 |

## D4 action 名の規則

action は **`<domain>.<verb>`**（小文字、語内区切りは underscore、dot は 1 個）。
domain は対象資源の単数名、verb は何をするかを表す動詞。同じ資源内の異なる操作には異なる名前を付ける。
registry の tasks / decisions / projects / admin / surface は実装の所有領域であり、action の domain を 5 語に制限しない。
既存 12 名は互換性のため変更しない:
`task.create`・`comment.create`・`decision.answer`・`approval.decide`・`execution.phase_gate`・
`question.answer`・`execution.plan_gate`・`project.update`・`knowledge.record`・`knowledge.reject`・
`attachment.reference`・`inbox.answer`。

| route の種類・例 | 命名 |
|---|---|
| 資源を作成／編集／削除 | `<resource>.create` / `.update` / `.delete`（PUT upsert は `.put` も可。registry 内で一貫させる） |
| task の状態変更 | `task.reopen` / `task.retry` / `task.pause` / `task.resume` / `task.cancel` |
| decision の訂正・撤回 | `decision.revise` / `decision.withdraw` |
| execution plan POST / PUT | `execution.adopt_plan` / `execution.put_plan`（method 別に一意） |
| LLM の source/tier と role/tier | `model_assignment.put` / `.delete` / `.preview`、`model_role.replace` / `.preview` |
| knowledge 正本と candidate | `knowledge.put_page` / `.accept`、既存 `.record` / `.reject` |
| browser control | `browser_control.command` / `.disconnect` / `.agent_begin` / `.agent_end` / `.auth_section` |
| daemon・本番昇格 | `daemon.reload` / `notify.test` / `release.promote` |
| chat・添付 upload | `chat_thread.create` / `.update`、`chat_message.create` / `.delete`、`attachment.upload` |

単一 route の body が選ぶ既存 command（control command、docs maintenance action、changes integrate mode 等）は
資源の既存 validation を保持し、詳細は監査 payload/result に記録する。別 method/path は別 registry entry とする。
action は path から ad hoc に生成せず、領域 registry に明示する。新規 action の重複、既存名の変更、
領域をまたいだ同一 method/path は網羅性試験で拒否する。EXCLUDED に許可 action を付けて dispatch しない。

## D5 実装の形と維持する検証

`operations.rs` は envelope の認証・path/body 検証・冪等検査・領域 dispatch を受け持つ。
`cos/ops/{tasks,decisions,projects,admin,surface}.rs` に各領域の ALLOWED/EXCLUDED/PENDING と dispatch を置く。
PENDING は移行中だけ存在し、未登録を黙って成功させない。除外の理由は D2 と一致させる。

領域 handler と CoS dispatch は同じ共有の操作関数を呼ぶ。通常 handler は監査 context 無しの `Applied::Direct`、
CoS 経路は credential から作った `OperationAudit` を渡し `Applied::Audited` を返す。
thread/run/operation/actor を body から自己申告させない。DB の領域 write・cos_operations 行・監査 envelope event・
代答等のカードを `OperationAudit::apply` の 1 transaction に入れる（独立した store write を先に commit しない）。
既存の domain event にも actor=cos と操作への相関を通す。全 route の処理を HTTP 自己呼出しで代理して認証を迂回しない。

ファイル・git・upload・daemon channel・release promote などの副作用は SQLite transaction では rollback できない。
共有操作関数で安全な staging/既存の冪等処理と監査の開始・結果を結び、失敗・中断から再送しても二重適用せず、
副作用前に除外・認可・revision を検査する。DB write と監査行の原子性を外部副作用の原子性と取り違えない。
成功を確かめる前に applied を記録しない。副作用途中の失敗も監査の理由と結果に残す。
この条件を満たす具体的な整理は各領域 unit が実装・検証する。

- `expected_revision` は envelope の既存必須フィールド（版がなければ null）と領域固有の検査を維持。
  stale revision は既存どおり 409。DELETE だけに新しい版必須条件を足さない。
- idempotency key は credential の thread に scope し、同じ key/hash は同じ operation、異なる hash は 409。
  body の revision/key（browser の expected_version 等）も既存どおり検査する。
- `human_required`、明示 human check、対象の認可・confidence・人の指示の既存例外規則を保持。
  許可範囲に入っていることだけで人待ちを解除しない。inbox.answer の委譲先にも同じ検査を通す。
- browser control 許可でも既存 assertion の検証を省略せず、監査には署名 token の生値を置かない。
  CoS が assertion を持たないときに代理で署名する経路は作らない。
- upload は multipart の内容を JSON へ無制限に展開しない。既存のサイズ・hash・path containment・
  client_upload_id を維持し、envelope から共有 upload 処理を使う安全な入力形は surface unit が定義する。
- 除外は **422 `cos_operation_not_allowed`** と理由コード／detail を返し、**rejected の cos_operations 行と理由付き監査 event**
  を残す。領域の write は一切しない。未知操作にも同じ拒否記録が付く。

## D6 実装と検証の受け入れ

foundation は D3 の全行を一意に分類する試験と、監査経由／直接呼出しを比較する共通 helper を用意する。
5 領域の unit は自分の ALLOWED/EXCLUDED/PENDING を更新する。ctl-wrap と skill-docs はその registry に合わせる。
最終 verify は PENDING が空で ALLOWED か EXCLUDED のどちらか一方だけに属することを router と文書の和集合で検査する。

各許可操作の代表で `/cos/operations` 経由の applied・監査行・event と、直接呼出しの 422 を確かめる。
必須例は decision revise/withdraw、task edit/reopen/retry/pause/resume、execution-plan PUT、
LLM assignments PUT/DELETE、knowledge accept。除外各理由では 422・rejected 行・副作用無し・秘密値非保存を確かめる。
人の回答を変更せず promote・standing-rules・browser control・DELETE の登録を確認する。
既存の revision conflict・idempotency conflict・human_required と browser の署名／lease 検証の退行も検査する。

後続の workspace 検査は `cargo clippy --workspace -- -D warnings` と `bash scripts/dev/test-parallel.sh`。
この ADR WorkUnit の検査は文書リンク・ADR 採番・progress front matter・コード差分無しであり、
ここでは API を実装済みとはしない。[旧 D3 の差分節](2026-10-05-cos-chat-home.md) は本決定への参照で設計判断を解消し、
実装完了の追記は skill-docs/verify が行う。

## 付記: 実装（2026-10-09、task 01M4F5KS8E1MXZDNJTRAVFJESW）

D6 の必須例を先に登録した。各操作は handler と CoS dispatch が同じ共有関数を通り、検証（読むだけ）→
`OperationAudit::apply` の transaction 内の書き込み → commit 後の冪等な後始末、の形にした（既存 12 操作と同じ）。

| action | method / path | 共有の関数（検証 → tx 内の書き込み） | commit 後の後始末 |
|---|---|---|---|
| `decision.revise` | POST `/decisions/{id}/revise` | ta `decisions::revise_op` → to `decision::plan_revise` → tc `decision_resolve_apply_tx`（期待 `answered`） | 既にある子への訂正コメント（`finish_answer`） |
| `decision.withdraw` | POST `/decisions/{id}/withdraw` | ta `decisions::withdraw_op` → to `decision::plan_withdraw` → 同上（期待 `open`） | `self` の節点の中止 |
| `task.update` | PATCH `/tasks/{id}` | ta `task_actions::patch_task_op` → to `edit::plan_edit` → tc `edit_task_tx`・`set_task_expected_write_paths_tx` | unroutable な blocked の解除（`finish_edit`） |
| `task.reopen` | POST `/tasks/{id}/reopen` | ta `reopen_op` → to `comment::plan_reopen` → tc `apply_transition_tx(Reopen)` | routing 結果の追記（`finish_reopen`） |
| `task.retry` | POST `/tasks/{id}/retry` | ta `retry_op` → to `retry::plan_retry` → tc `retry_task_tx` | `execution` の明示（`finish_retry`） |
| `task.pause` / `task.resume` | POST `/tasks/{id}/pause`・`resume` | ta `lifecycle::task_pause_op` → to `lifecycle::plan_set_paused` → tc `edit_task_tx` | なし |
| `execution.put_plan` | PUT `/tasks/{id}/execution-plan` | ta `execution::put_plan_audited` → to `execution::plan_replan` → tc `execution_plan_replan_tx` | なし |
| `model_assignment.put` / `.delete` | PUT・DELETE `/llm/models/assignments/{source}/{tier}` | ta `model_assignments::{put,delete}_assignment_audited` → tc `model_role_assignment_{set,delete}_tx` | なし |
| `knowledge.accept` | POST `/knowledge/inbox/{id}/accept` | ta `knowledge::accept_op`（`reject_op` と同じく git の取り込みを tx の中で行い、失敗は rejected） | なし |

- 領域 event の書き手は `cos`（`Edited.by`・`DecisionAnswered.by`・`DecisionWithdrawn.reason` の接頭辞・割り当ての
  `updated_by`）。人の経路は従来どおり `human` / `admin`。
- `execution.put_plan` は active な計画がある task の replan だけ。計画の無い task への PUT は初回の採用
  （`adopt_human_plan`。子 task の作成・決定の要求を伴う）なので `execution.adopt_plan`（POST）と同じく登録待ちで、
  理由付きの 422 と rejected 行で記録する。
- path の placeholder は英数字・`-`・`_` に加えて `.`・`:` も 1 segment として受ける（model id・
  `openai-compatible:<id>` の source）。`.`・`..` だけの segment は従来どおり拒否する。
- celerisctl: CoS credential のとき `retry`・`answer`・`execution phase-gate`・`execution plan replan` を
  `/cos/operations` に包む（`cos_mapped`。試験 `cos_mapped_subcommands_match_registered_operations`）。
  他の変更サブコマンドは従来どおり拒否し、`api-request` で登録済みの path を送る。
- 試験: ta `tests/cos_ops_mutations.rs`（直接呼び出しの 422・rejected 行、`/cos/operations` 経由の applied・監査 event・
  カード、領域 write の書き手、競合・catalog 外・初回採用の理由付き拒否）。除外は既存の `tests/cos_ops_registry.rs`。
- 残り: 各領域の `PENDING`（tasks 10・decisions 3・projects 14・admin 35・surface 21 本）は同じ形で登録する
  後続の子 task に分けた（進捗 `agent-docs/progress/2026-10-09-cos-operations-all-mutations.md`）。

