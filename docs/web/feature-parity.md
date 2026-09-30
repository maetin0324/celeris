---
tasks: [01M3MS2JRDJ4GM0D9VN9PJCB6B]
---
# web/ の feature parity matrix（gui/ の全 route）

ADR-0081（`docs/adr/0081-web-spa-frontend.md`）の移行 gate。`gui/app/routes.ts`（main `06e9a03cffe8`）の
**全 42 route を 1 行ずつ**並べ、その route の主要操作・認証・通知・SSE・file viewer・mobile 要件・既存の検査と、
新 `web/` でそれを閉じる Phase・slice・確認方法・状態を書く。**すべての行が「完了」になるまで gui/ から web/ への
配信切替をしない**（ADR-0081 D8）。gui/ の削除は別タスクで人の承認が要る。

## 使い方

- 各 Phase のタスクは、担当 slice の行の「状態」を `未着手` → `実装中` → `完了（<commit>）` に書き換える。
  `完了` にしてよいのは「確認方法」のテストが web/ にあり、通ったときだけ。未検査のまま完了にしない。
- 「確認方法」のテスト名は **これから web/ に作るテストの名前を先に決めたもの**。ファイルは
  `web/e2e/parity/<slice>.spec.ts`、テストの題は `parity: <route>` で始める。実行は
  `pnpm -C web e2e parity/<slice>.spec.ts -g "parity: <route>"`。mobile の行は、これに加えて
  `pnpm -C web mobile-audit`（幅 360/390/412 と 1440、ADR-0081 D7）の対象一覧にその path が入っていること。
- search param で切り替わる画面（tab など）は、その route の行の「主要操作」に書き、確認方法で全値を開く。
- 表の行数の検査: `grep -c '^| R[0-9][0-9] |' docs/web/feature-parity.md` が `gui/app/routes.ts` の
  `index(` + `route(` の数（42）と一致すること。path の集合の一致も確かめる（Phase 0 の証跡は
  run の artifacts の `check-parity-rows.py`）。

### Phase と slice（タスクへの分割は `docs/web/implementation-plan.md`）

| Phase | 内容 | slice |
|---|---|---|
| 1 | `web/server/` の gateway（Express 5）: 静的配信、auth/session、Host、CSRF、security headers、`/api/*`・`/events`・file の relay | `gateway`、`gateway-auth`、`gateway-relay` |
| 2 | shell（ナビ・バナー・Console の置き場）、TanStack Router/Query、SSE → invalidate の対応表、404 | `shell`、`realtime` |
| 3 | 中核の画面: Console、受信箱と認可、タスク、run とファイル、報告 | `console`、`inbox`、`tasks`、`task-detail`、`runs-files`、`reports` |
| 4 | 管理の画面: 案件とボード、組織、知識、運用設定 | `projects`、`org`、`knowledge`、`ops`、`help` |
| 5 | 横断 gate（遅延・security・mobile/a11y の全画面検査）| `latency-gate`、`security-gate`、`mobile-gate` |
| 6 | 並行運用と配信切替（人の判断） | `cutover` |
| 7 | gui/ の削除（別タスク・人の承認が要る） | `gui-removal` |

凡例: **認証** = `要`（未認証は画面なら 302 `/login?next=`、resource なら 401。`gui/app/auth.server.ts:157-203`）/
`不要`（`PUBLIC_PATHS` = `/login` `/logout` `/healthz`）。**SSE** = `全再検証` は root の `useCelerisStream`
（`task.event`/`daemon`/`reset` のどれでも表示中の全 loader を 250 ms スロットルで再実行）に依存するという意味。
web/ ではこれを「その行の domain の key だけ invalidate」に置き換える（ADR-0081 D6）。**mobile** の `audit` は
現行 `pnpm mobile-audit`（393×851 のみ、`gui/scripts/lib/celeris-fixture.mjs:56-110`）の対象、`未 audit` は対象外
（11 本。web/ では mobile-gate で必ず対象に入れる）。全画面に共通の要件（44×44 のタップ領域、ページ全体の横溢れ
なし、360〜412 と 1440 の幅）は横断要件 X10 に書き、行には画面固有のものだけ書く。

## route の表（42 行）

| # | route | 種類 | 主要操作（mutation / form action、URL state） | 認証 | 通知 | SSE | file viewer | mobile 要件 | 既存の検査 | 閉じる Phase / slice | 確認方法 | 状態 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| R01 | `/` | 画面 | Console（CoS）への送信 = `POST` instruct（202）、新しい会話（R39）、返事の block の積み上げ | 要 | 共通（X9） | 全再検証 + `/console/stream`（R38）の block 差分 | なし | audit。**下端 fixed の composer**（safe-area、キーボード表示時に隠れない）、IME 変換中に送信しない | unit `home.route.test` `console*.test` `useConsoleStream.test`、e2e `g0` `g13`、mobile-audit `home` | 3 / `console` | `parity: / Console の送信・返事・IME` | 完了（cd421e3d） |
| R02 | `/inbox` | 画面 | 区画（承認待ち・質問・draft・注意: `failed` `cluster_unavailable` `phase_checkpoint` `requeue_limit_near` `unroutable`）ごとの approve / reject / answer / cancel（`expected_status`・note・answer、複数 `task_id` を直列）。結果は fetcher に残す | 要 | 共通（X9）、root のバッジ件数（counts） | 全再検証 | 承認の成果物 preview（`ApprovalArtifactPreview` → R41）、Markdown | audit。項目ごとの操作ボタン・結果表示が縦に並んでも横溢れしない | unit `inbox.action.test` `inbox.loader.test`、e2e `g1` `g2`（note 付き承認・2 ページ競合 409）`g7`（cluster_unavailable → /clusters）、`check-human-review.mjs`、mobile-audit `inbox` | 3 / `inbox` | `parity: /inbox 区画表示と承認・409 再取得` | 完了（6dfb3445） |
| R03 | `/healthz` | resource | GET のみ（GUI の release・版の応答。`release.sh` の新旧切替が参照） | 不要 | なし | なし | なし | — | unit `server-healthz.test`、e2e `g5-release` | 1 / `gateway` | `parity: /healthz 未認証 200 と版` | 完了（96717f8） |
| R04 | `/login` | 画面 | パスワード送信 → 署名 cookie 発行 → `next`（`safeNextPath`）へ。失敗は 1 s 待ち | 不要 | なし | なし（celeris を呼ばない） | なし | **未 audit**。パスワード欄の autocomplete、キーボード表示時にボタンが隠れない | unit `auth.test`、e2e `g5`（非 loopback の認証） | 1 / `gateway-auth` | `parity: /login 成功・失敗・next・daemon 停止中` | 完了（8eae18e） |
| R05 | `/logout` | resource | `POST` で cookie を消して `/login` へ | 不要 | なし | なし | なし | — | unit `auth.test` | 1 / `gateway-auth` | `parity: /logout cookie 消去と CSRF` | 完了（d8d375c） |
| R06 | `/org/secretary` | 画面（302） | 旧 URL。302 で `/org/cos` へ | 要 | なし | なし | なし | — | e2e `g0` | 4 / `org` | `parity: /org/secretary → /org/cos` | 未着手 |
| R07 | `/org` | 画面 | 課・部の `org_create` / `org_patch` / `org_delete`、`skill_mount` / `skill_unmount`。`?selected=` で右の詳細 | 要 | 共通（X9） | 全再検証 | profile・skill の Markdown | audit（`org`、`org?selected=`）。木と詳細が縦積みになる | unit `org.test` `org-tree.test` `skills.test`、e2e `g13`、mobile-audit `org` `org-detail` | 4 / `org` | `parity: /org 木・選択・作成・変更・削除・skill` | 未着手 |
| R08 | `/org/:id` | 画面 | その人の Console（R01 と同じ部品。`id=cos` もここ） | 要 | 共通（X9） | 全再検証 + `/console/stream` | なし | audit（`org-node`）。**fixed composer**（R01 と同じ） | unit `console*.test`、mobile-audit `org-node` | 3 / `console` | `parity: /org/:id Console の送信と宛先` | 完了（cd421e3d） |
| R09 | `/projects` | 画面 | 案件の作成（成功は `/projects/:id` へ redirect、失敗は fetcher）、一覧の絞り込み | 要 | 共通（X9） | 全再検証 | なし | audit | unit `projects.test` `project-index.test` `workspace-form.test`、e2e `g13`、mobile-audit `projects` | 4 / `projects` | `parity: /projects 一覧・作成・422 表示` | 完了（35be337a） |
| R10 | `/projects/:id` | 画面 | `project_edit` `project_status` `project_pause` `project_resume` `project_cancel` `project_archive` `project_unarchive` `project_plan` `project_plan_decide` `project_workspace_save` `project_workspace_clear`、`milestone_create` `milestone_status` `milestone_pause` `milestone_resume` `milestone_cancel` `milestone_decide`、`repo_create` `repo_patch` `repo_delete` `repo_primary`、`task_create` | 要 | 共通（X9） | 全再検証（project に属する task の event。`project_id` はイベントに無いことが多い → ADR-0081 D6 の解決表） | 成果物一覧（`ArtifactsList` → R41）、Markdown、仕事の木（`WorkTree`） | audit（`project-detail`）。計画 DAG と仕事の木が横溢れしない | unit `projects.detail.test` `projects.repos.test` `repos-admin.test` `project-plan*.test` `milestone-review.test` `work-tree.test`、e2e `g13`、mobile-audit `project-detail` | 4 / `projects` | `parity: /projects/:id 全 intent・計画・木` | 実装中 |
| R11 | `/projects/:id/docs` | 画面 | 文書の `init` / `save` / `delete` | 要 | なし | 全再検証（route は既定の判断） | 文書の Markdown 表示と編集 | audit（`project-docs`）。編集欄が幅に収まる | unit `docs.test`、mobile-audit `project-docs` | 4 / `projects` | `parity: /projects/:id/docs 初期化・保存・削除` | 未着手 |
| R12 | `/projects/:id/docs/maintenance` | 画面 | 文書保守の起動（`POST /projects/:id/docs/maintenance`） | 要 | なし | 全再検証 | なし | audit（`project-docs-maintenance`） | unit `docs-maintenance.test`、mobile-audit `project-docs-maintenance` | 4 / `projects` | `parity: /projects/:id/docs/maintenance 起動と結果` | 未着手 |
| R13 | `/board` | 画面 | 案件を選んで 6 列表示。絞り込みは URL（search param）。カードの `edit` | 要 | 共通（X9） | 全再検証 | なし | audit。6 列は横スクロールを列の枠内に閉じる（ページは横溢れしない） | unit `board.test` `board.loader.test`、`check-resume-recovery.mjs`、mobile-audit `board` | 4 / `projects` | `parity: /board 列・URL 絞り込み・編集・復帰` | 未着手 |
| R14 | `/knowledge` | 画面 | 知識の検索・閲覧（URL）と `save` | 要 | なし | 全再検証 | Markdown 表示 | audit | unit `knowledge.test`、mobile-audit `knowledge` | 4 / `knowledge` | `parity: /knowledge 検索・閲覧・保存` | 未着手 |
| R15 | `/knowledge/inbox` | 画面 | 候補の `accept` / `reject` | 要 | なし | 全再検証 | 候補の Markdown | audit（`knowledge-inbox`） | unit `knowledge.test`、mobile-audit `knowledge-inbox` | 4 / `knowledge` | `parity: /knowledge/inbox 採用・却下` | 未着手 |
| R16 | `/knowledge/skills` | 画面 | `skill_put` / `skill_delete`。`?create=1`（作成フォーム）、`?name=`（詳細）、`&edit=1`（編集） | 要 | なし | 全再検証 | skill 本文の Markdown・雛形 | audit（`knowledge-skills` `knowledge-skill-create` `-detail` `-edit`）。files 入力・インラインの検証エラー | unit `skills.test`、mobile-audit 4 本 | 4 / `knowledge` | `parity: /knowledge/skills 一覧・create・name・edit・削除` | 未着手 |
| R17 | `/reports` | 画面 | `reports_read`（既読）、`reports_notified`、`notify_test`（通知の試験）。`?filter=` `?level=`。ブラウザ通知の許可（`NotificationsEnable`） | 要 | **通知の中心**: 報告の到着 → Notification API（X9）、未読バッジ | 全再検証（`daemon` の reports は SSE の生 snapshot では空 → `GET /daemon` を取り直す、ADR-0081 D5） | なし | audit。展開行（R18）が縦に伸びる | unit `reports.test` `notify.test`、e2e `g13`、mobile-audit `reports` | 3 / `reports` | `parity: /reports 絞り込み・既読・通知試験・展開` | 完了（cafcaf76） |
| R18 | `/reports/:id` | resource | GET: 行の展開、`sources_expanded` の追い掛け | 要（現行は 302、web は 401 → X1） | なし | なし | なし | — | unit `reports.test` | 3 / `reports` | `parity: /reports/:id 展開の取得` | 完了（cafcaf76） |
| R19 | `/approvals` | 画面 | `approval_decide`、常設ルールの `standing_rule_create` / `standing_rule_delete` | 要 | 共通（X9）、root の承認待ちバッジ（`approvals_pending`） | 全再検証（バッジは `GET /daemon` の値。SSE の生 snapshot は 0） | 成果物の Markdown | audit | unit `approvals.test`、e2e `g13`、mobile-audit `approvals` | 3 / `inbox` | `parity: /approvals 判定・常設ルール・バッジ` | 完了（6e9eb5d4） |
| R20 | `/artifacts` | 画面 | 絞り込み（`?project=` 等、GET の Form）だけ | 要 | なし | 全再検証 | 成果物一覧（`ArtifactsList` → R41） | **未 audit** | unit `artifacts.test` `artifacts.route.test` `artifact-view.test`、e2e `g13` | 3 / `runs-files` | `parity: /artifacts 絞り込みと開く` | 完了（127ddb4） |
| R21 | `/tasks` | 画面 | 絞り込み・並び・検索（`?q=` `?status=` `?limit=` `?order=`、GET の Form）、続きの読み込み（fetcher） | 要 | なし | 全再検証 | なし | **未 audit**。表が幅 360 で横溢れしない | unit `tasks.loader.test`、e2e `g1` `g4` `g7` `g13`、e2e `g5-a11y`（CSP・axe） | 3 / `tasks` | `parity: /tasks 絞り込み・検索・続き` | 完了（753e272f） |
| R22 | `/tasks/new` | 画面 | タスク作成（受け入れ条件の型 `command` `artifact_exists` `reviewer` `human`）。成功は `/tasks/:id` へ | 要 | なし | 全再検証 | なし | **未 audit**。条件の行の追加・削除ボタン | unit `tasks.new.test`、e2e `g2` `g7`、e2e `g5-a11y` | 3 / `tasks` | `parity: /tasks/new 作成・条件 4 型・422` | 完了（9196395e） |
| R23 | `/tasks/:id` | 画面 | approve / reject / answer / cancel、`retry`、`edit`、`comment`、`reopen`、`rereview`、`promote`、`phase_gate`、`execution_decompose`。`?tab=overview\|timeline\|changes\|files\|artifacts` | 要 | 共通（X9） | 全再検証（その task の `task_id` で絞れる） | 成果物（`ArtifactView` → R41）、Markdown、files tab（R24 と同じ部品）、changes tab（R25） | audit（5 tab）。判断パネル・routing パネル（fixed 要素あり）がスマホで操作できる | unit `tasks.detail.*.test` `tasks.manage.action.test` `task-*.test` `lifecycle.test` `execution-mode.test`、e2e `g1` `g2` `g3` `g7` `g13` `g5-a11y`、`check-human-review.mjs` `check-task-routing.mjs` `check-resume-recovery.mjs`、mobile-audit `task-*` 5 本 | 3 / `task-detail` | `parity: /tasks/:id 5 tab と全 intent・409` | 完了（cfe6b40） |
| R24 | `/tasks/:id/files` | 画面 | 作業ツリーの閲覧（パス・ファイルの選択は URL） | 要 | なし | 全再検証 | **作業ツリーの file viewer**（path 検査 → X6） | **未 audit**。長い path・長い行 | unit `task-files.test` | 3 / `runs-files` | `parity: /tasks/:id/files 木と本文・不正 path` | 完了（127ddb4） |
| R25 | `/tasks/:id/changes` | 画面 | 変更の取り込み `integrate`、`pr_merge` | 要 | なし | 全再検証 | 差分の表示 | **未 audit**。差分の横溢れを枠内に閉じる | unit `task-changes.test` | 3 / `task-detail` | `parity: /tasks/:id/changes 差分・取り込み・merge` | 完了（cfe6b40） |
| R26 | `/tasks/:id/runs/:runId` | 画面 | run ログの閲覧（会話形式、`/files/.../stdout?offset=` の追記を追う） | 要 | なし | 全再検証（実行中 run は offset の追い掛け） | **stdout.jsonl・result.json の viewer**（R40） | **未 audit**。`check-run-log.mjs` は 360/390/412/1440 | unit `run-log.test`、e2e `g3`（行数が `wc -l` と一致）、`check-run-log.mjs` | 3 / `runs-files` | `parity: /tasks/:id/runs/:runId 会話表示と追記` | 完了（8d35dbe） |
| R27 | `/tasks/:id/runs/:runId/events` | resource | GET: Console の progress の「すべて見る」 | 要 | なし | なし | なし | — | unit `tasks.runs.events.route.test` | 3 / `console` | `parity: runs/:runId/events 全行の取得` | 完了（cd421e3d） |
| R28 | `/plans/new` | 画面 | 計画の作成（ADR-0079 で `POST /plans` は 410。`POST /tasks` で root task を作り、成功は `/tasks/:id` へ） | 要 | なし | 全再検証 | なし | **未 audit** | unit `plans.new.test`、e2e `g2` | 3 / `tasks` | `parity: /plans/new 作成と失敗表示` | 完了（f4a22a16） |
| R29 | `/daemon` | 画面 | `replay`（結果は fetcher） | 要 | なし | 全再検証（SSE `daemon` の生 snapshot と `GET /daemon` の差 → ADR-0081 D5） | なし | **未 audit** | unit `daemon.test`、e2e `g2`（replay 0 mismatch）`g4`、e2e `g5-a11y` | 4 / `ops` | `parity: /daemon 状態・replay` | 未着手 |
| R30 | `/providers` | 画面 | provider の `create` / `patch` / `delete` / `check` | 要 | なし | 全再検証（dispatcher tick ごとの snapshot） | なし | **未 audit** | unit `providers.test` `providers-admin.test`、e2e `g4` `g8` `g9`、e2e `g5-a11y` | 4 / `ops` | `parity: /providers 追加・変更・削除・確認` | 未着手 |
| R31 | `/accounts` | 画面 | account の `create` / `delete` / `check`、ログイン `login_start` / `login_code` / `login_cancel`（デバイス認証）、`secret_put` / `secret_delete`。LLM source・MCP クライアントの表示（R32 を開く） | 要 | なし | 全再検証 | なし | audit。**秘密値の入力欄**（表示しない・自動補完しない） | unit `accounts*.test` `secrets-admin.test` `llm-sources.test` `mcp.test`、e2e `g8` `g9`、mobile-audit `accounts` | 4 / `ops` | `parity: /accounts 追加・ログイン・secret・削除` | 未着手 |
| R32 | `/mcp/clients/:id/calls` | resource | GET: MCP クライアントの直近の呼び出し（カードを開いたとき） | 要 | なし | なし | なし | — | unit `mcp.test` | 4 / `ops` | `parity: mcp/clients/:id/calls 取得` | 未着手 |
| R33 | `/clusters` | 画面 | `cluster_connect` / `cluster_connect_code` / `cluster_connect_cancel`、`cluster_work_dir_save` / `cluster_work_dir_clear` | 要 | なし | 全再検証 | なし | audit | unit `clusters.test`、e2e `g7`、mobile-audit `clusters` | 4 / `ops` | `parity: /clusters 接続・作業ディレクトリ` | 未着手 |
| R34 | `/releases` | 画面 | `release_promote`（非同期の昇格、結果と失敗の表示） | 要 | なし | 全再検証 | なし | audit | unit `releases.test`、mobile-audit `releases` | 4 / `ops` | `parity: /releases 昇格と失敗表示` | 未着手 |
| R35 | `/graph` | 画面 | `?root=` `?depth=`（GET の Form）で依存グラフ | 要 | なし | 全再検証 | なし | **未 audit**。SVG が枠内に収まる | unit `graph-layout.test`、e2e `g3` `g7` | 3 / `tasks` | `parity: /graph root・depth` | 完了（6937011f） |
| R36 | `/help` | 画面 | 閲覧のみ（6 節の見出しと id、アンカー） | 要 | なし | なし（loader 無し） | なし | audit | e2e `g6`、mobile-audit `help` | 4 / `help` | `parity: /help 6 節とアンカー` | 未着手 |
| R37 | `/events` | resource（SSE） | `GET /stream` のバイト無加工の中継（`task_id`・`Last-Event-ID` を転送） | 要（401） | なし | **SSE の本体**: `hello` `heartbeat` `task.event` `daemon` `reset` | なし | — | unit `events.route.test` `useCelerisStream.test` `recovery.test` | 1 / `gateway-relay`（中継）+ 2 / `realtime`（invalidate 表） | `parity: /events 中継・再接続・Last-Event-ID・reset` | 完了（c353ac0b） |
| R38 | `/console/stream` | resource（SSE） | Console の block の SSE 中継 | 要（現行は 302、web は 401 → X1） | なし | Console の block 差分 | なし | — | unit `console.stream.route.test` `useConsoleStream.test` | 3 / `console` | `parity: /console/stream 中継と再接続` | 完了（bd00a32c） |
| R39 | `/console/new-conversation` | resource | `POST` 新しい会話 | 要 | なし | なし | なし | — | unit `console.server.test` | 3 / `console` | `parity: /console/new-conversation 開始` | 完了（bd00a32c） |
| R40 | `/files/tasks/:id/runs/:runId/:name` | resource（file） | run のファイルの中継（Range / `offset` / `length` / `download`、許可リストのヘッダ、`nosniff`） | 要（401） | なし | なし | **file relay 本体**（R26 が使う） | — | unit `files.route.test` | 1 / `gateway-relay` | `parity: files runs 中継・Range・offset・不正 name` | 完了（3bc52d5） |
| R41 | `/files/tasks/:id/artifacts/:idx` | resource（file） | 成果物の中継（同上） | 要（401） | なし | なし | **file relay 本体**（R02 R10 R20 R23 が使う） | — | unit `files.route.test` `artifacts.route.test` | 1 / `gateway-relay` | `parity: files artifacts 中継・download・不正 idx` | 完了（3bc52d5） |
| R42 | `*` | 画面（404） | 未定義パスも middleware（Host・認証・CSRF・header）を通して 404 | 要 | なし | なし | なし | 404 画面からナビへ戻れる | e2e `g5`（`/no-such-page`） | 2 / `shell` | `parity: * 未定義パスの 404 と header` | 完了（8bc8805） |

補足:
- 行ごとの SSE の invalidate 範囲（どのイベントでどの key を捨てるか）は ADR-0081 D6 の表が正。parity の確認では、
  その行の画面を開いたまま無関係な task の `worker_progress` を流しても、その画面の key が再取得されないことも見る。
- root が持つもの（ナビのバッジ counts・`reportsLive`・`approvalsPending`、celeris 断のバナーと 5 秒ごとの再確認、
  復帰時の再同期）は route ではないので横断要件 X9・X12・X13 に置く。

## 横断要件の表

| # | 要件 | 現行の実装と検査（gui/） | web/ での要件 | 閉じる Phase / slice | 確認方法 | 状態 |
|---|---|---|---|---|---|---|
| X1 | 認証・session | `gui/app/auth.server.ts`: パスワード SHA-256 を `timingSafeEqual`、失敗 1 s、HMAC 署名 cookie `__celeris_gui_session`（httpOnly・SameSite=Strict・https で Secure・24 h）。`PUBLIC_PATHS` 以外は画面 302、`/events` `/files/*` は 401。非 loopback bind でパスワード無しなら起動拒否（`gui/server.js:49-55`）。unit `auth.test`、e2e `g5` | 同じ cookie 属性と公開パス。`/api/*`・`/events`・`/files/*` はすべて 401（SPA は 401 を受けて `/login?next=` へ）。`next` は同一オリジンの絶対パスだけ。非 loopback + パスワード無しは起動しない。daemon 停止中も login できる | 1 / `gateway-auth` | `parity-x: auth cookie 属性・401・next・非 loopback` | 完了（8eae18e） |
| X2 | CSRF | Origin 一致 / `Sec-Fetch-Site`（`security.server.ts:202-268`）。Express 層で 403（`server/app.ts:164`）+ root middleware。unit `security.csrf.test` | 変更系（POST/PATCH/PUT/DELETE）の `/api/*`・`/logout`・`/login` を gateway で検査して 403。GET に副作用を持たせない | 1 / `gateway` | `parity-x: CSRF 別 origin の変更は 403` | 完了（96717f8） |
| X3 | Host 検査 | Express 層（`server.js:74-109`、静的配信にも効く）+ root middleware。e2e `g5`（`Host: evil.example` → 400） | asset・HTML・API・file・stream・404 の全経路で許可外の Host を 400 | 1 / `gateway` | `parity-x: Host 全経路で 400` | 完了（96717f8） |
| X4 | CSP・security headers | HTML は nonce 付き CSP・nosniff・no-referrer・`X-Frame-Options: DENY`・既定 no-store（`security.server.ts:285-296`）、静的・302/401 は `default-src 'none'`（`server.js:91-102`）。e2e `g5` `g5-a11y`（全ページに CSP） | SPA の HTML に CSP（inline script なし、`connect-src 'self'`）、全応答に nosniff・no-referrer・frame 拒否。API と HTML は no-store、hash 付き asset だけ長期 cache | 1 / `gateway` | `parity-x: 全応答の security header` | 完了（96717f8） |
| X5 | daemon token の秘匿 | token は `client.server.ts` の `#token` だけ、root loader は URL だけ返す（未認証なら返さない）。e2e `g5.spec.ts:253-` | token は gateway の process だけが持つ。HTML・bundle・`/api/*` の応答・エラー文・ログに出さない。client は daemon に直接つながない | 1 / `gateway-relay` | `parity-x: token が HTML・bundle・エラーに出ない` | 完了（77d10ac） |
| X6 | file path の安全 | `files.runs.ts:54-100`・`files.artifacts.ts`: 中継ヘッダの許可リスト、`nosniff` の付け直し、Range / offset / length / download の転送。path の解釈は daemon | `:name` `:idx` を gateway で検査（`..`・区切り文字・NUL を拒否）、daemon の応答ヘッダは許可リストだけ転送、`Content-Disposition` は daemon の指定か attachment。HTML の成果物は同一オリジンで実行させない（`nosniff` と CSP sandbox） | 1 / `gateway-relay` | `parity-x: file 不正 path・header 許可リスト・HTML を実行しない` | 完了（3bc52d5） |
| X7 | timezone | `app/lib/time-zone.ts`（表示の時刻帯）、unit `time-zone.test` `clock.test` `time-delta.test` | SSR が無くなるので client の時刻帯で表示し、相対時刻は同じ規則。hydration の差分が無くなる分、サーバ時刻との差（`hello.now`）で相対表示を補正 | 2 / `shell` | `parity-x: timezone 表示と相対時刻` | 完了（89416a54） |
| X8 | 秘密・機密を永続化しない | SSR なので client の保存は無い | TanStack Query の persist を使わない。localStorage / IndexedDB に server state・secret・token・報告本文を書かない（UI の好みだけ可） | 2 / `shell` | `parity-x: storage に機密が無い`（全画面を開いた後の storage を検査） | 完了（89416a54） |
| X9 | ブラウザ通知 | `components/NotificationsWatcher.tsx`（root loader の `reportsLive` → Notification API、判定 `lib/reports.ts`）、`NotificationsEnable`、R17 の `notify_test`。unit `notify.test` `reports.test` | 報告の到着を shell で見張る。値の元は `GET /daemon`（補完済み）で、SSE の生 snapshot の `reports: None` で「無い」と判断しない。同じ報告を二度通知しない（タブ間も含む） | 3 / `reports` | `parity-x: 通知 1 回だけ・生 snapshot で消えない` | 完了（44cf8508） |
| X10 | mobile・a11y | mobile-audit（393×851、ADR-0055 D1 の規則: 44×44、横溢れ、`a11y-name`、`a11y-structure`、`focus-order`）、e2e `g5-a11y`（axe の critical/serious 0）、`check-*.mjs` の 360/390/412/1440 撮影 | 全 42 行の画面（resource を除く）を 360/390/412/1440 で監査（未 audit の 11 本も含む）。タップ領域 44×44、ページの横溢れなし、axe critical/serious 0、ダイアログの focus trap、遷移時の見出し focus | 5 / `mobile-gate` | `pnpm -C web mobile-audit`（全画面・4 幅）と `parity-x: axe 全画面` | 未着手 |
| X11 | 遅延の表示と遷移の独立 | loader は全部 blocking、root は `/health` → `/inbox` → `/daemon` を直列、client の timeout 15 s | 遷移（URL と枠）は daemon を待たない。1 s 超で待機表示、5 s 超で「時間がかかっています」と再試行、失敗を 0 件に置き換えない（ADR-0081 D7）。daemon に 5/10 s の遅延を入れて測る | 5 / `latency-gate` | `parity-x: 遅延 10 s で遷移が止まらない`（baseline と同じ手順） | 未着手 |
| X12 | SSE の再接続・再同期 | `useCelerisStream`（CLOSED なら 5 s 後に張り直し）、`useResumeRevalidate`・`recovery.ts`（visibilitychange / pageshow / online / focus）。`check-resume-recovery.mjs` | `Last-Event-ID` で続きから、`reset` で全 invalidate、復帰時は表示中の key だけ再取得。イベントの burst で request が増え続けない | 2 / `realtime` | `parity-x: SSE 再接続・reset・復帰・burst` | 完了（c353ac0b） |
| X13 | celeris 断の表示 | root の `health`・`unavailable` バナー、5 s ごとに root だけ再確認。e2e `g0`（停止中のバナーと復旧） | shell にバナー。daemon 断でも shell・ナビ・login は出る。復旧したら表示中の key を取り直す | 2 / `shell` | `parity-x: daemon 停止中のバナーと復旧` | 完了（bcfe6276） |
| X14 | 409 / 422 の扱い | `revalidateAfterActionErrors`（4xx の後も再検証）、操作の結果は fetcher に載せて SSE の再検証で消さない（監査 H1） | mutation の 409 は該当 key を invalidate して「状態が変わりました」、422 は celeris の文言を欄の横に。結果の表示は再取得で消えない。確定まで同じ操作を二重送信しない | 3 / `inbox`（共通部品） | `parity-x: 409 再取得・422 表示・二重送信なし` | 完了（6dfb3445） |
| X15 | 配布・起動 | `pnpm release`（`e2e g5-release`: 展開して `pnpm install --prod --offline` だけで動く）、`/healthz` による新旧切替 | web/ も同じ形で配布でき、`/healthz` で切替できる。gui/ と並行して動かせる | 6 / `cutover` | `parity-x: web の配布物が offline install で動く` | 未着手 |
| X16 | 型と境界 | `gui/scripts/gen-types.mjs`（`docs/api/v1/api-v1.schema.json` → `types.ts`） | 型は同じ schema から生成し差分ゼロ。`web/` は `gui/` を import しない。client に server・token の処理が入らない | 1 / `gateway` | `parity-x: 型の再生成差分ゼロ・gui import なし` | 完了（8ab4362） |

## この表の範囲外

- gui/ の削除・rename（Phase 7、別タスクで人の承認が要る）。
- celeris の API の変更（`EventRow.project_id` の追加など）。必要なら実装計画の「人の判断点」に置く。
