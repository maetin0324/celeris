# ADR: done task の変更を取り残さない

- Date: 2026-10-11
- Status: Implemented（実装済み）
- tasks: [01M4M8D4ANYGCWVZ0A0WSW53X7]
- Related: [ADR 2026-10-11-breaking-changes-by-default](2026-10-11-breaking-changes-by-default.md)

## Context

2026-10-09 に done・review 合格となった 01M4D7RVKX の変更は、10-11 に人が気付いて手動で main に merge するまで task branch に残り、本番にも出なかった。01M4F3V643 も done 後の 130 commit が main に無い。報告された両 task の event には `phase_integrated {merged:[], head:""}` はあるが、delivery event は無い。

[調査記録](../progress/2026-10-11-stranded-done-branches.md) が示すコード上の根因は、`review_spawn.rs` が `delivery::begin` を既存 acceptance の `Check::Reviewer` から reviewer run を作れた場合だけ呼ぶ点である。計画の `StartIntegration` は親 branch への WU 統合であり main への配送ではない。加えて `delivery::prepare` は project 無し・対象外 project・support・tree child を `Skip::Silent` とし、delivery tick は既存 delivery row しか走査しない。個別 task の acceptance と project_id は取得できていないため、2 件の具体的な経路は推定である。

## Decision

### D1. 完了状態の不変条件と保存

対象は repository を変更した root task（atomic または計画付き）である。done になる時点までに、その task の記録済み branch head は次のいずれかでなければならない。

1. **integrated**: 対象 repo の main の祖先である。
2. **discarded**: 人または明示的に認可された運用操作が変更を捨てた。
3. **held**: 人または認可された運用操作が保留理由を付けた。

判定不能・repository の解決不能・branch/head 消失は integrated と見なさず、未解決として inbox に残す。`task.repos` を持たない task の repository は D3 の規則で解決する。done は作業完了を表し続け、integration 状態は別の durable state とする。repository ごとの task integration 表を設け、task_id/repo_id を一意キー、branch/head SHA、状態、理由、更新時刻、処理者を保存する。migration は 기존 task/event を保全し、events は append-only のまま `integration_detected`、`integration_merged`、`integration_discarded`、`integration_held`、`integration_reopened` 等を追記する。event は監査履歴、表は現在状態の索引とする。

### D2. Delivery と統合経路

review 合格の root task は reviewer run の有無に依存せず delivery を開始する。plan の `StartIntegration` は main delivery の代用にしない。既存 delivery policy が task を除外する場合も silent skip で閉じず、除外理由を durable に記録して状態を `held` 相当の要確認として inbox に出す。tree child は親への統合を保ち、root 対象とは別に扱う。

delivery は自動 merge を試みる。成功したら main への到達を git で確かめて integrated にする。衝突、権限不足、repo 不明など自動処理不能なら integration request を inbox に作り、人が merge／discard／hold を選べるようにする。merge 操作も同じ git 到達確認を通す。discard は理由必須、hold は理由必須で、held は明示操作で再開できる。agent-platform 自身の repository も例外にしない。main への merge と selfdeploy release の promote は別状態とし、main に入ったが本番 release に未反映なら **promotion pending** と表示する。通常の統合処理は本番 host を変更しない。

### D3. LLM を使わない定期検出

daemon の定期 sweep は done の root task を列挙し、task ごとに対象 repository を次の規則で解決してから git を実行する。

- **`task.repos` を持つ task**: その repository ごとに `refs/heads/celeris/<task id>` の head を読み、対象にする（branch が無ければ head 消失として未解決）。
- **`task.repos` が空の task**（project も repos も持たない task。本番 DB では done root 763 件中 548 件、01M4D7RVKX・01M4F3V643 もこの形で、実体は `<workspace>/repos/<name>` の worktree と登録 repo 側の `celeris/<task id>` branch）: store に登録された Local の repository（全案件の repo 一覧）ごとに `git for-each-ref refs/heads/celeris/` を 1 回だけ読み、`celeris/<task id>` がある repository をその task の対象にする（task×repo ごとに rev-parse しない）。どの登録 repository にも branch が無い task は repository を変えていない扱いで、何も記録しない。

解決した head が repo の既定 branch の祖先なら integrated、そうでなければ integration state を unintegrated にして inbox item を upsert する。git は store の lock の外で読む。LLM 判定・要約を検出条件に使わない。既存 delivery row の tick に依存せず全 done task を走査するため、初回 sweep で 01M4F3V643 等、project も repos も無い過去の取り残しも検出する。main 更新・branch 消失・対象 repo の解決不能を含む再走査は冪等であり、状態変化は event に記録する。

### D4. 受信箱（inbox）操作/API と可視化

受信箱（inbox）item から merge／discard／hold を行う操作を task API と `celerisctl` の両方に提供する。操作は認可・入力理由を検証し、DB 状態と event を一貫して更新する。merge は対象 branch/head と repo を再検査し、既に main に含まれていれば冪等に integrated とする。discard/hold の再操作履歴は上書きせず event に残す。

web の task 詳細・一覧および inbox は次の状態を表示する: integrated（取り込み済み）、unintegrated（未取り込み／要対応）、discarded（捨てた）、held（保留）、promotion pending（昇格待ち）。理由・対象 repo・branch/head・最後の確認時刻も詳細で確認でき、integration 操作は inbox から実行できる。

### D5. 同時に削除する旧経路

[ADR 2026-10-11-breaking-changes-by-default](2026-10-11-breaking-changes-by-default.md) に従い、新経路と同じ変更で以下を削除する。

- `reviewer_run.is_some()` のときだけ `delivery::begin` する条件付き起動。
- project 無し／policy 対象外等をイベントも状態も残さず終える `Skip::Silent`。
- deliveries row が無い done task を永久に見落とす、既存 row 限定の tick を唯一の検出経路とする前提。
- 実装で置換される旧い integration 状態・API・CLI・web 表示・config 分岐・それだけを守る試験。

置換後も tree child を root delivery 対象にしない規則は維持するが、理由を記録可能にする。

### D6. 決定的な試験

一時 git repo に main と task branch を作り、done の root task と repo/worktree 情報を fixture 化する。

1. main に無い branch head の task を作り、sweep により unresolved 状態と inbox item が作られることを確認する。LLM・外部サービスは使わない。
2. 2 回目の sweep が item/event を重複作成せず、既存の stranded task も初回 sweep で拾えることを確認する。
3. inbox merge 操作で git merge が行われ、head が main の祖先となり integrated 表示へ変わることを確認する。
4. 別 task を discard して理由と履歴を確認し、held は理由付きで残り明示的に再開可能であることを確認する。
5. agent-platform の統合後も promotion pending は main の統合状態と独立して表示されることを確認する。

## Consequences

完了 task の変更が状態なしで取り残されなくなり、初回走査で過去分も可視化される。状態表・migration・daemon sweep・inbox/API/web が必要になる。done と本番反映を分離するため、運用者は integrated と promotion pending を区別できる。

## 付記: 実装との突き合わせ（2026-10-11）

Status は Proposed のまま。以下は実装（views・routes・routes-wire・cli・e2e の各 WorkUnit）を読んで確かめた事実と、決定からのずれである。決定本文は変えていない。

### 実装した経路と欄

- route（`crates/task-api/src/integrations.rs:19-30`）:
  - `GET /api/v1/integrations?state=`（`unintegrated|integrated|discarded|held`）
  - `GET /api/v1/tasks/{id}/integrations`
  - `POST /api/v1/tasks/{id}/integrations/{repo}/merge`
  - `POST /api/v1/tasks/{id}/integrations/{repo}/discard`（body `{"reason": "…"}`、空白のみは不可）
  - `POST /api/v1/tasks/{id}/integrations/{repo}/hold`（同上）
  - `POST /api/v1/tasks/{id}/integrations/{repo}/reopen`
  - `{repo}` は repository の名前（受信箱の `operation_path` は `/api/v1/tasks/{task}/integrations/{repo_name}/`）。
- 認可は既存の人の操作と同じ admin bearer（`require_admin`）で、actor は `human`。不正な遷移は 409 の problem。
- CoS の operation registry では 4 mutation route を `EXCLUDED`（理由 `integration_human_decision`、`crates/task-api/src/cos/ops/tasks.rs:73-` 付近）。agent からは実行させない。
- 欄（`crates/task-ops/src/view.rs`）: TaskSummary/TaskDetail に `integrations: Option<Vec<IntegrationView>>` と `promotion: Option<PromotionView>`。`IntegrationView` は `repo_id`・`repo`・`state`・`reason`・`actor`・`branch`・`head`・`checked_at`。`PromotionView` は `status`（`not_applicable|promotion_pending|promoted`）と `release_sha`。前回の記録にあった `integration_status[]` という名前ではない。
- promotion は全 state の行で計算する（e2e の変更）。ADR の「統合状態と独立して表示」に合わせた。
- 配線: `[selfdeploy] releases_dir` → `ApiState.integration_releases_dir`（`crates/task-api/src/lib.rs:241-242`）。repo path と default branch は project の repository 行と worktree marker から得る。project の無い task では、task の integration 行が指す登録 repository（sweep が `celeris/<task id>` branch から解決したもの）から得る（`resolve_repo`・`attach_task_integrations`）。
- celerisctl（`crates/celerisctl/src/commands/integration.rs`）: `integration list [--state] [--task] [--json]`、`merge <task> <repo>`、`discard|hold <task> <repo> --reason <r>`（`--reason` は必須）、`reopen <task> <repo>`。
- schema と生成型: `docs/api/v1/api-v1.schema.json` に `IntegrationView`・`PromotionStatus` がある。`node web/scripts/gen-types.mjs --check` は exit 0 で web の生成型と一致（2026-10-11 に確認）。

### repos 未設定 task の解決（D3、sweep-repoless）

- `crates/celeris/src/daemon/stranded_sweep.rs` の `run` は done root task を `task.repos` の有無で分ける。`task.repos` を持つ task は従来どおり repo ごとに `rev-parse refs/heads/celeris/<id>`。空の task は `repoless_records` が `project_list` → `repo_list` で全登録 repository を集め、Local のものだけ `git for-each-ref --format='%(refname) %(objectname)' refs/heads/celeris/` を 1 回読み、`celeris/<task id>` があればその `repo_id` で `BranchRecord` を作る。Remote の repository と branch の無い task は記録しない。repos 空の done root task が 1 件も無ければ git を読まない。
- API: `crates/task-api/src/integrations.rs` の `resolve_repo` と `crates/task-api/src/handlers/tasks.rs` の `attach_task_integrations` は、project の repository に加えて task の integration 行が指す repository を名前解決に使う。project の無い task でも `/tasks/{id}/integrations/{repo}/{merge,discard,hold,reopen}` と task 詳細・一覧の `integrations` が働く。merge は従来どおり worktree marker の repo を使う。
- 試験（`crates/celeris/tests/integration_api.rs`）: `integration_api_repoless_done_task_detected_in_inbox_and_detail_then_discarded`（検出・受信箱・詳細・discard で受信箱から消える）、`integration_api_repoless_task_without_branch_records_nothing`、`integration_api_repoless_branch_in_main_is_integrated`。

### 決定からのずれ（人の判断が要る）

1. **受信箱の操作先**（D4）: `Unintegrated` item の `answer.native` は merge の 1 件だけ（`crates/task-ops/src/human_inbox.rs:1068`）。discard と hold の選択肢には native が無く、web は `operation_path` に `discard`/`hold` を足して route を呼ぶ必要がある。旧い説明文だけの `/integration/{merge,discard,hold}` の URL は `human_inbox.rs` から消えている（検索で 0 件）。
2. **merge の意味**（D2・D4）: API merge は task branch head を既定 branch に実際に取り込む。既に祖先なら冪等に integrated を返し、ff 可能なら fast-forward、そうでなければ detached の一時 worktree で `git merge --no-ff` してから既定 branch を進める。既定 branch の checkout が dirty なら `409 checkout_busy` とし、衝突時は既定 branch を変えず integration request と理由を記録して `integrated: false` を返す。取り込み後に `git merge-base --is-ancestor` で head が既定 branch の祖先であることを確認してから integrated と `IntegrationMerged` event を記録する。origin へ push はしない。
3. **discard の確認**: 実装は reason の必須だけで、確認フラグは無い。決定は確認を求めていない。

### 未実施

- 受信箱の discard/hold の操作先を item に載せる変更は crate の変更になるため、この WorkUnit では行わない（次の WorkUnit の候補）。
- web の操作画面は、記録上 views・routes・cli・e2e のいずれでも作っていない。
