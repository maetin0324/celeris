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

## 付記: 取り残し検出の不備（2026-10-11 本番 deploy）

### 起きたこと

50f349d84fea の本番 deploy で初回の sweep が done root task 218 件を `unintegrated` として受信箱に出した。

- 180 件は reason `repository default branch is missing`。agent-platform の `project_repos.default_branch` が NULL。うち 175 件は branch head が main の祖先で、誤検出。delivery（`crates/task-ops/src/delivery.rs` の `changes::default_branch(path, row.default_branch)`）は NULL のとき repo から既定 branch を解決するが、sweep（`crates/task-ops/src/stranded.rs` の `record.default_branch.is_none()`）は解決しない。解決の経路が 2 つあり食い違っていた。運用で `PATCH /api/v1/repos/01M2XPE3NZ1FZE061FKDY4A9PK` により `default_branch=main` を入れて回避した（データの書き換えのみ）。
- 38 件は reason `repository cannot be resolved`。benchfs の repository が `WorkspaceSpec::Remote`（pegasus・sirius）で、`crates/celeris/src/daemon/stranded_sweep.rs` は Remote の path を `None` にする。遠隔 repo の task は永久に受信箱に残る。
- sweep は状態が同じでも 5 分ごとに行を upsert して `checked_at`・`updated_at` を進める。受信箱 item の `at` は `checked_at` から作る（`crates/task-ops/src/inbox.rs` の `AttentionItem::Unintegrated`）ため、購読側には同じ item が新着に見えた。

### F1. default branch の解決は `task_ops::changes::resolve_default_branch` の 1 関数だけ

- 新しい関数 `task_ops::changes::resolve_default_branch(repo: &Path, configured: Option<&str>) -> Result<String, DefaultBranchError>` を唯一の解決経路にする。順序は次のとおり（既存の `default_branch` の規則を引き継ぐ）。
  1. 設定（`project_repos.default_branch`、前後の空白を除いて空でないもの）
  2. `git symbolic-ref --quiet --short refs/remotes/origin/HEAD` の `origin/` を除いた名前
  3. `refs/heads/main` が存在すれば `main`
  4. `refs/heads/master` が存在すれば `master`
  5. どれも無ければ `Err`（理由の文字列を持つ。例: `default branch cannot be resolved: no configured branch, origin/HEAD, main or master`）。**`"main"` を盲目に返さない**。
- 設定された名前は検証しない（設定が正）。設定した branch が存在しなければ、呼び出し元の ref 解決で従来どおり失敗する。
- 消すもの（同じ変更で）: 旧 `task_ops::changes::default_branch`（何も無ければ `"main"` を返す）、sweep の個別判定（`record.default_branch.is_none()` で `repository default branch is missing` とする分岐）と `BranchRecord.default_branch: Option<String>` を設定値のまま渡す経路、`crates/task-api/src/handlers/tasks.rs` の `repo.default_branch.unwrap_or_else(|| "main")`、`crates/task-dispatch/src/checkpoint.rs` の `default_branch(dir, None)`。
- 呼び出し元と、解決できないときの振る舞い:

| 呼び出し元 | 解決できないとき |
|---|---|
| sweep（`crates/task-ops/src/stranded.rs`。`stranded_sweep.rs` は path と設定値を渡すだけ） | `unintegrated`、reason は `resolve_default_branch` の Err の文字列。設定 NULL でも origin/HEAD・main・master で解決できれば祖先判定に進む |
| delivery（`crates/task-ops/src/delivery.rs`） | `Skip::Report { reason: RefsUnresolvable }` と同じ扱いで、理由に Err の文字列を載せる。統合はしない |
| doc_gardener（`crates/celeris/src/doc_gardener.rs`） | その repo の監査を行わず `Err(String)` を返す（周期処理の既存の失敗記録に載る） |
| task-api changes（`crates/task-api/src/changes.rs`） | 変更一覧は `Missing` 扱い（差分なしではなく「解決できない」を返す）。merge は 409 の problem で理由を返す |
| task-api docs（`crates/task-api/src/docs.rs`） | その repository の docs 一覧を空にし、理由を応答の該当 repo に載せられなければ log に残す |
| task-api integrations（`crates/task-api/src/integrations.rs`）・handlers（`handlers/tasks.rs`） | その repository を `IntegrationContext.repositories` に入れない（merge は 409、詳細の promotion 計算は対象外）。`"main"` で埋めない |
| task-dispatch behind_target（`dispatcher/behind_target.rs`） | `DispatchError` を返し、behind は `null`（review sync はその repo を対象外） |
| task-dispatch housekeeping（`dispatcher/housekeeping.rs` の scratch seed） | その repo の seed を `commit: None` にする（seed を作らない） |
| task-dispatch run_context（`dispatcher/run_context.rs`） | その mount の docs 一覧を空にする |
| task-dispatch checkpoint（`checkpoint.rs`） | `(None, Vec::new())`（checkpoint に repo 状態を載せない） |
| celerisctl（`commands/docs_maintenance.rs`） | `--default-branch` の既定値 `main` を消し、省略時は `resolve_default_branch(repo, None)` で解決する。解決できなければ理由つきで非 0 終了 |

- 試験: 設定 NULL・origin/HEAD 無し・`main` のある repo で、取り込み済みの branch を sweep が `integrated` と判定する（`resolve_default_branch` の単体試験と sweep の試験）。どれも無い repo で `Err` と理由が返る。

### F2. 遠隔 repo（`WorkspaceSpec::Remote`）は sweep で判定しない: 新しい状態 `not_applicable`

- `TaskRepoIntegrationStatus` に `NotApplicable`（文字列 `not_applicable`）を足す。migration で `task_integrations.state` の CHECK に `'not_applicable'` を加える（SQLite は CHECK を変えられないので、表を作り直して行を写す。行は失わない）。
- sweep は Remote の repository を持つ record を ssh せず `not_applicable`、reason `remote repository: not checked by the sweep` で記録する。
- 理由: 5 分周期の sweep から TOTP を伴う ssh（pegasus）は不安定で、失敗のたびに偽の状態を作る。delivery も遠隔 repo を main に統合しないので、遠隔 repo について「main に入っていない」は取り残しの定義に当たらない。
- `not_applicable` は受信箱に出さない（`crates/task-ops/src/inbox.rs` の対象は `unintegrated` だけ）。API・celerisctl・web の統合状態表示には `not_applicable` として出す（`integration list --state not_applicable` で引ける）。merge・discard・hold・reopen は `not_applicable` から 409（遷移不可）。
- 既存の `unintegrated` 行（benchfs の 38 件）は次の sweep で `not_applicable` に移る。状態の変化は既存どおり event の追記で残し、過去の events は書き換えない。
- `discarded`・`held` の行は sweep が触らない既存の規則を保つ（遠隔 repo でも人の判断を上書きしない）。
- 遠隔で `git merge-base` を実行する判定は採らない。必要になれば別の ADR で、cluster の接続がある時だけ走る人起動の確認として決める。

### F3. 状態が変わらない再 sweep は行を書かない（受信箱 item の同一性）

- sweep は前回の行と `state`・`branch`・`head_sha`・`reason` がすべて同じなら upsert しない。`checked_at`・`updated_at` は変わらない。
- 受信箱 item の `at`（`checked_at` 由来）と id はこの行から作るので、状態が変わらない限り変わらない。購読側に新着として見えない。
- 4 欄のどれかが変われば従来どおり upsert し、`checked_at`・`updated_at` を進める（新着として見えるのは状態が変わった時だけ）。
- 「最後に確かめた時刻」を別に持つ欄は足さない（必要が出たら別に決める）。
- 試験: 同じ repo 状態で sweep を 2 回走らせ、受信箱 item の id と `at`、行の `checked_at` が同じであること。head が変わった時は `at` が進むこと。

### 実装との突き合わせ（F1〜F3、2026-10-11 close 段）

統合後の task branch（`e83fb32a`）のコードを読み、試験名は `grep -rn 'fn stranded_…' crates` で実在を確かめた。決定本文（F1〜F3）は変えていない。食い違いは各項の「ずれ」に書く。

#### F1（default branch の一本化）

- 関数: `task_ops::changes::resolve_default_branch(repo: &Path, configured: Option<&str>) -> Result<String, DefaultBranchError>`（`crates/task-ops/src/changes.rs`）。順序は 設定（trim して空でないもの）→ `git symbolic-ref --quiet --short refs/remotes/origin/HEAD` の `origin/` を除いた名前 → `refs/heads/main` → `refs/heads/master` → `Err("default branch cannot be resolved: no configured branch, origin/HEAD, main or master")`。`"main"` を盲目に返す経路は無い。
- 旧経路の撤去: `pub fn default_branch(` は crates に 0 件。sweep の `repository default branch is missing` 分岐も無い。`crates/task-api/src/handlers/tasks.rs` と `crates/task-dispatch/src/checkpoint.rs` に `"main"` の埋めは無い（`crates/task-api/src/docs.rs` に残る `"main"` は新規 docs repo の初期 branch で、解決の fallback ではない）。
- `resolve_default_branch` を呼ぶのは次の 13 file（`grep -rl resolve_default_branch crates`）: sweep（`crates/celeris/src/daemon/stranded_sweep.rs` の `default_branch_for` が path と設定値を渡し、結果を `BranchRecord.default_branch: Result<String, DefaultBranchError>` に入れる。`crates/task-ops/src/stranded.rs` の `sweep` は `Err` の文字列を reason にして `unintegrated`）、delivery（`Skip::Report { RefsUnresolvable }`、detail に理由）、doc_gardener（`Err(String)`）、task-api changes（一覧は `missing: true`、diff・merge は 409 `default_branch_unresolvable`）、task-api integrations（取り残しの merge は 409 `default_branch_unresolvable`）、handlers/tasks.rs（`IntegrationContext.repositories` に入れず warn log）、task-api docs、task-dispatch behind_target・housekeeping（seed の `commit: None`）・run_context（docs 一覧を空）・checkpoint（`(None, Vec::new())`）、celerisctl `docs_maintenance`（`--default-branch` の既定 `main` を消し、省略時は解決、失敗で非 0）。
- 試験:
  - `crates/task-ops/src/changes/tests.rs`: `stranded_default_branch_is_configured_then_detected`、`stranded_default_branch_prefers_origin_head`、`stranded_default_branch_unresolvable_is_an_error_with_reason`
  - `crates/celeris/tests/stranded_default_branch.rs`: `stranded_default_branch_null_with_main_is_integrated`、`stranded_default_branch_null_with_origin_head_is_integrated_for_repoless_task`、`stranded_default_branch_unresolvable_is_unintegrated_with_reason`
- ずれ:
  - task-api docs: 決定は「その repository の docs 一覧を空にし理由を載せる」。応答に repo ごとの理由欄が無いため、既存の `docs_unavailable` problem に理由を載せて返す。
  - task-dispatch behind_target / review sync: 決定は「`DispatchError` を返す」。実装は `review_target_ref` が `Ok(None)` を返してその repo だけを対象外にする（他の repo の sync を止めないため）。behind は `null` で決定と同じ。
  - 新しいエラーコード `default_branch_unresolvable`（409）は `docs/api/v1/api.md` のエラーコード表に載っていない（resolve 葉の範囲外。進捗ファイルの「提案」）。

#### F2（遠隔 repo は `not_applicable`）

- 状態: `TaskRepoIntegrationStatus::NotApplicable`（`crates/task-core/src/integrations.rs`、文字列 `not_applicable`）。
- migration: `crates/task-core/migrations/0070_task_integration_not_applicable.sql`。`task_integrations_new` を CHECK に `'not_applicable'` を足して作り、全行を `INSERT … SELECT` で写してから旧表を DROP・RENAME し、index `idx_task_integrations_disposition` を作り直す。行は失わない。0068・0069 は他 branch が使うため飛ばした。
- sweep: `crates/celeris/src/daemon/stranded_sweep.rs` が `WorkspaceSpec::Remote` の repository に `not_applicable: true` を立て、ssh も git も実行しない。`crates/task-ops/src/stranded.rs` の `sweep` は `not_applicable` の record を `NotApplicable` で記録する。`Discarded`・`Held` の行は従来どおり先に飛ばす（遠隔でも人の判断を上書きしない）。
- 受信箱: `crates/task-ops/src/inbox.rs` は `task_integration_list(Some(Unintegrated))` だけを読む。`not_applicable` は出ない。
- 409: `stranded.rs` の `merge`・`reopen`・`set_disposition`（discard・hold）は `NotApplicable` で `StoreError::Invalid` を返し、`crates/task-api/src/integrations.rs` の `invalid_problem` が 409 `invalid_transition` にする。celerisctl の `integration list --state not_applicable` は受け付ける。web は `対象外（遠隔 repo）` を操作ボタン無しで出す。
- 試験: `stranded_remote_migrates_unintegrated_row_to_not_applicable`、`stranded_remote_actions_reject_not_applicable_rows`（ともに `crates/task-ops/src/stranded.rs`）、`stranded_remote_not_applicable_state_is_hidden_from_inbox`（`crates/task-ops/src/inbox/tests.rs`）。
- ずれ（解消: 決定本文を実装に合わせて改訂した。final-verify 段で確認）:
  - 表の名前: 決定本文の `task_repo_integration_states` は `task_integrations`（`0067_task_repo_integration_states.sql` が作った表）に直した。
  - reason の文字列: 決定本文の文字列を `remote repository: not checked by the sweep` に直した。merge の拒否理由は `remote repository was not checked by the sweep`、reopen・discard・hold は `not-applicable integrations cannot be changed locally`。
  - event（解消済み）: 決定どおり `unintegrated` 等から `not_applicable` への遷移は `Event::IntegrationNotApplicable` （`crates/task-core/src/model.rs`、serde 名 `integration_not_applicable`、`repo_id`・`branch`・`head_sha`・`reason`・`actor` を持つ）として追記する。追記は `crates/task-core/src/store/task_integration_state.rs` の `task_integration_upsert_impl` が行い、状態が同じなら追記しない（既存の規則）。試験: `stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat`（`crates/task-ops/src/stranded.rs`）。移行で書き換わった行の過去の events は書き換えていない。event 名は `crates/task-api/src/query.rs` の `EVENT_TYPES` と web の `event-kinds.ts`・`invalidation-map.ts` に追従済み。
  - repos 空の task（`repoless_records`）は Local の repository だけを走査し、Remote の repository の branch は record を作らない（`not_applicable` の行もできない）。`task.repos` を持つ task だけが `not_applicable` で記録される。

#### F3（状態が変わらない再 sweep は書かない）

- 実装: `crates/task-ops/src/stranded.rs` の `sweep` は `store.task_integration_get` で前回の行を読み、`state`・`branch`・`head_sha`・`reason` のどれかが違う（または行が無い）ときだけ `task_integration_upsert` を呼ぶ。同じなら `checked_at`・`updated_at` は変わらない。戻り値の件数も書いた行だけを数える。
- 受信箱 item の `at` は従来どおり `checked_at` から作る（`AttentionItem::Unintegrated`）。通知 feed も同じ inbox view を元にする。
- 試験: `stranded_identity_preserves_attention_item_id_and_at_until_head_changes`（`crates/task-ops/src/inbox/tests.rs`。5 分あけた 2 回の sweep で item の id と `at` が同じ、head を変えると `at` が進む）。
- ずれ: 無し。

## 付記: 衝突しない進みは自動で取り込み直す（2026-10-27）

D2 の「衝突・権限不足など自動処理不能なら integration request」は、人が気づいて押すまで残る前提だった。
[ADR 2026-10-27-done-task-auto-reintegrate](2026-10-27-done-task-auto-reintegrate.md) により、auto_integrate の案件で main が進んだだけの
`unintegrated` 行は sweep の後に `task_ops::stranded::reintegrate::run` が git だけで判定し、clean なら再 review 経路に戻す。
衝突・回数の上限だけが従来どおり受信箱 `unintegrated` と integration request に残る。手動の `merge`・`hold`・`discard`・`reopen` は変えない。

