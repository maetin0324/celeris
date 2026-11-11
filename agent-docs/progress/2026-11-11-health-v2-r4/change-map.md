---
title: "健全性v2 R4: Dispatcher の大きい関数の判断と副作用の区間"
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: running
updated: 2026-11-11
---
# 健全性v2 R4: Dispatcher の大きい関数の判断と副作用の区間

この文書は、`crates/task-dispatch/src/dispatcher/` の大きい 7 関数の本体を行番号で「判断」と「副作用」に分ける。コードは変えていない。行番号は WorkUnit cm-functions の base（`$CELERIS_WU_BASE`）の HEAD のもの。

- 判断: store・時計・config・memory 上の状態を読み、次の動作を決める区間。副作用なしで純粋関数に切り出せる。
- 副作用: DB 書き込み（`store.*` の書き込み）・event 追記・lease 操作・process や task の spawn・通知・file system・git の操作。Dispatcher の memory 上の map（`running`・`infra_backoff` など）への書き込みも副作用として数え、「（memory）」と付ける。
- 「判断＋副作用」は、判断の途中で副作用を持つ helper を呼ぶ区間。helper の位置を「呼ぶ helper」の欄に書く。
- owner 候補は次の 8 つから選ぶ。下の意味はこの文書での読みで、field の割り当て（cm-owners）と食い違えば cm-owners の結果を正とする。
  - execution-coordination: task・WorkUnit の状態遷移、計画、review の進行
  - run-supervisor: run の起動・終了・lease・process の監督
  - routing: lane・route・account・quota の選択と会計
  - project-orchestration: 案件・delivery・子 task・統合
  - host-maintenance: disk・掃除・host の保守
  - cluster-runtime: クラスタの接続・job
  - composition-root: 組み立て（config と port の注入）
  - test-hooks: 試験用の注入点

大きさ（行数は `fn` の行から閉じ括弧まで）:

| 関数 | 位置 | 行数 |
|---|---|---|
| finish_worker_result_with | `crates/task-dispatch/src/dispatcher/worker_finish.rs:337-1630` | 1294 |
| dispatch_one | `crates/task-dispatch/src/dispatcher/dispatch_run.rs:35-1145` | 1111 |
| spawn_review | `crates/task-dispatch/src/dispatcher/review_spawn.rs:347-1094` | 748 |
| on_review_finished | `crates/task-dispatch/src/dispatcher/review_verdict.rs:20-613` | 594 |
| reclaim_expired_leases | `crates/task-dispatch/src/dispatcher/leases.rs:217-436` | 220 |
| route_state | `crates/task-dispatch/src/dispatcher/route_select.rs:757-978` | 222 |
| resolve_quota_estimate | `crates/task-dispatch/src/dispatcher/quota_book.rs:119-332` | 214 |

## 関数: finish_worker_result_with

位置: `crates/task-dispatch/src/dispatcher/worker_finish.rs:337-1630`

worker run の終わり（`RunOutcome` か `AdapterError`）を受け、trigger・WorkUnit の遷移・checkpoint・quota・報告を決めて書く。判断の大半は 349-1281 にあり、書き込みは 1289 以降に集まる。ただし 364・1217-1253 の書き込みが判断の途中に挟まり、1390-1421 は書いた直後の DB を読み直して判断する。

| 区間 | 種類 | 読む状態 | 書く状態・外への作用 | 呼ぶ helper |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:349-361` | 判断 | `result` | なし（失敗理由・報告の種類・misplaced の判定） | `provider_failure_reason`, `reports::terminal_report` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:364` | 副作用 | 成果物の result.json の memory 欄 | node の memory file に追記（file system） | `absorb_memory`（`crates/task-dispatch/src/dispatcher/worker_finish.rs:1634-1651`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:367-368` | 判断 | `store.events_for` | なし | `cross_department_questions_of` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:371-597` | 判断 | `result`、`store.events_for`（540・578 の連続 requeue 数）、`config.max_requeues`・`max_infra_retries` | なし（`trigger`・`outcome_str`・`run_end`・`provider_outcome`・`cluster_wait` を決める） | `cluster_job_wait_for`（`crates/task-dispatch/src/dispatcher/cluster_job_wait.rs:150-202`）, `provider_failure_outcome`, `browser_prereq::worker_error_code` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:599-625` | 判断 | `check_failure` | なし | — |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:626-727` | 判断 | `store.events_for`、WU の作業ツリーの git 状態、成果物の checkpoint.json、`config.execution.limits.tree` | なし（WU の checkpoint を組み立てて `checkpoint_event` に置く） | `auto_leaf::counts`, `work_unit_checkpoint_site`, `checkpoint::gather_from`, `checkpoint::read_worker_checkpoint`, `task_core::merge_checkpoint` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:730-1003` | 判断 | `store.work_units_for`、`store.events_for`、`store.run_index_get`（988）、`config.execution` | なし（`execution_scheduler::decide` の結果から trigger・outcome・replan・integration repair の扱いを決める） | `execution_scheduler::decide`, `auto_leaf_decision`（`crates/task-dispatch/src/dispatcher/auto_leaf.rs:172-218`）, `merge_repair::retire_if_failed`, `integration_repair_result_trusted`, `integration_repair_exhaustion`（`crates/task-dispatch/src/dispatcher/worker_finish.rs:49-132`）, `counted_replans`, `plan_gate::stalled_replans`, `effective_max_replans`, `replan_exhausted_ask` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1004-1039` | 判断 | 成果物の decisions、`store.decisions_list` | なし（`wu_update` を決める） | `worker_decisions`（`crates/task-dispatch/src/dispatcher/tree_units.rs:369-528`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1040-1190` | 判断 | `store.events_for`、task の作業ツリーの git 状態、checkpoint.json、子 task の状態 | なし（atomic task の checkpoint・continuation の trigger を決める） | `checkpoint::gather`, `awaited_children`, `checkpoint_shows_progress`, `no_progress_streak`, `execution_scheduler::continuation_cap_question` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1194-1213` | 判断 | 成果物の decisions | なし | `worker_decisions` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1217-1253` | 副作用 | `store.run_index_get` | `store.node_session_touch`（CoS 会話）、`store.work_unit_session_touch` | — |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1256-1281` | 判断 | `cross_department` | なし（questions・metrics） | — |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1289-1351` | 副作用 | — | WU の変更を git commit、`store.work_unit_transition_with_events`、依存 WU の `store.work_unit_transition` | `commit_work_unit`（`crates/task-dispatch/src/dispatcher/work_units.rs:1139-1173`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1354-1387` | 副作用 | — | write set の記録、`store.work_units_apply`（決定の要求）、`store.run_index_finish` | `record_run_write_sets`（`crates/task-dispatch/src/dispatcher/write_set_record.rs:80-134`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1390-1421` | 判断＋副作用 | `store.work_units_for`（1395。直前に書いた行を読み直す） | `deferred_work_unit_trigger` が `store.append_event` | `execution_scheduler::settle_phase`, `deferred_work_unit_trigger`（`crates/task-dispatch/src/dispatcher/work_units.rs:1219-1369`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1425-1489` | 判断 | 木の token 使用量（`token_budget::tree_tokens_used`） | なし（token 予算切れの trigger と、追記する events の組み立て） | `token_budget::budget_scope`, `trigger_starts_another_run` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1493-1506` | 判断＋副作用 | account book、quota group | `quota_activity` を閉じる（memory）、group の他 member の event を追記 | `resolve_quota_estimate`（`crates/task-dispatch/src/dispatcher/quota_book.rs:119-332`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1507-1530` | 副作用 | — | account・source の cooldown、question の承認記録 | `record_account_failure`・`record_source_failure`（`crates/task-dispatch/src/dispatcher/route_failure.rs:76-152`）, `approvals::record_question_approval` |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1533-1552` | 副作用 | — | 段が Wait・Integrate のとき events を 1 件ずつ `store.append_event`、統合の開始 | `start_integration`（`crates/task-dispatch/src/dispatcher/phase_integration.rs:528-732`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs:1553-1628` | 副作用 | 遷移の結果 `outcome.next` | `store.apply_transition_with_events`、`infra_backoff`（memory）、会話の返事、run report、review の spawn、`pending_subjects`（memory） | `record_conversation_reply`, `reports::record_run_report`, `spawn_review` |

判断と副作用が交互に入る所:

- `crates/task-dispatch/src/dispatcher/worker_finish.rs:1292`: `commit_work_unit` の git commit の結果（HEAD の sha）が `updated_wu.head_commit` と `WorkUnitCommitted` event（1484-1486）に入る。commit が失敗すると WU の遷移は進むが commit event は無い。
- `crates/task-dispatch/src/dispatcher/worker_finish.rs:1395-1396`: 1317・1336 で書いた WU 行を `store.work_units_for` で読み直し、`settle_phase` で段の進み方を決める。判断の入力が直前の書き込みの結果になっている。
- `crates/task-dispatch/src/dispatcher/worker_finish.rs:1411`: `deferred_work_unit_trigger` は event を追記しつつ trigger と questions を返す（判断の helper が書き込みも持つ）。
- `crates/task-dispatch/src/dispatcher/worker_finish.rs:1495`: `resolve_quota_estimate` の中で quota group を閉じ（memory）、他 member の event を直接追記する。自分の event は 1553 の遷移と同じ transaction に入るので、1 つの group の event が 2 つの書き方に分かれる。
- `crates/task-dispatch/src/dispatcher/worker_finish.rs:1317-1387` の書き込み（WU 遷移・run index の終了）は 1553 の task 遷移より前に別々に commit される。1553 が `InvalidTransition`（1624）で落ちても、WU 行と run index は先に進んでいる。
- `crates/task-dispatch/src/dispatcher/worker_finish.rs:1617-1621`: 遷移後の状態が Reviewing なら `spawn_review` を呼び、その戻り値で `pending_subjects` に残すかを決める。
- `crates/task-dispatch/src/dispatcher/worker_finish.rs:1324-1327`: `#[cfg(test)]` の `test_halt_after_wu_transition` が本体の途中にある（test-hooks）。

純粋な入力→提案に切り出す案:

- `RunFinishInput { task, current_wu, run: RunOutcomeView, check_failure, events: Vec<Event>, units: Vec<WorkUnitRow>, checkpoint_site: CheckpointSnapshot, worker_decisions: Option<WorkerDecisions>, children_pending: bool, token_budget: TokenBudgetView, limits: FinishLimits }`
- → `RunFinishProposal { trigger, outcome: String, run_end, checkpoint: Option<Checkpoint>, wu_update: Option<WuUpdate { row, reason, dependents }>, commit_work_unit: bool, settle: Option<PhaseSettle>, events: Vec<Event>, questions: Vec<String>, infra_backoff_n: Option<u32>, after: AfterTransition { report, spawn_review } }`
- 349-1281 と 1425-1489 をこの関数にする。`settle_phase` は DB を読み直さず、`units` の中の `wu_update.row` を差し替えた列から決める（1395 の読み直しを消す）。`deferred_work_unit_trigger` の event 追記は提案の `events` に入れ、実行側が 1 つの transaction で書く。
- 実行側は「git commit → WU 遷移と task 遷移を同じ transaction → memory（backoff・quota）→ 報告・review の spawn」の順に、提案だけを見て書く。

owner 候補: run-supervisor（run の終わりの分類と trigger。349-597・1040-1190・1553-1628）。WU の継続・replan の判断（626-1039・1390-1421）は execution-coordination、quota と cooldown（1493-1517）は routing に分ける。1324-1327 は test-hooks。

## 関数: dispatch_one

位置: `crates/task-dispatch/src/dispatcher/dispatch_run.rs:35-1145`（`crates/task-dispatch/src/phase_timing.rs:56` の同名 helper ではない）

1 つの task（か WU）を走らせるかを決め、走らせるなら lease を取り、event を書き、worker を spawn する。admission の判断（41-382）の途中に、担当の割り当て・gate の書き込み・cluster の接続・作業場所の用意が入っている。689 の lease 取得から後は副作用が続く。

| 区間 | 種類 | 読む状態 | 書く状態・外への作用 | 呼ぶ helper |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:41-86` | 判断 | 知識の入力、`running`、`just_aborted`、run load、`disk_watch` | 待ちの記録（memory） | `knowledge_curation::inputs_ready`, `is_cos_task`, `run_load().admits`, `is_eligible`, `held_by_disk_critical`（`crates/task-dispatch/src/dispatcher/maintenance.rs:93-95`）, `note_dispatch_wait`（`crates/task-dispatch/src/dispatcher.rs:2642-2644`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:89-104` | 副作用 | org、task.routing、計画 | 担当の割り当て、`store.update_task`（execution gate・plan invalid） | `assign_if_needed`（`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1472-1561`）, `execution_gate_if_needed`（`crates/task-dispatch/src/dispatcher/work_units.rs:1768-1830`）, `apply_plan_invalid_atomic`（`crates/task-dispatch/src/dispatcher/tree_units.rs:567-612`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:108-122` | 判断＋副作用 | WU 行、events、決定、木の counter | `wu_dispatch_gate` が `store.apply_transition_with_events`・`work_unit_transition`、`design_review_hold` が設計 verdict、`tree_limit_hold_for` が `append_event` | `wu_dispatch_gate`（`crates/task-dispatch/src/dispatcher/work_units.rs:413-614`）, `decision_self_hold`, `design_review_hold`（`crates/task-dispatch/src/health_review/design_gate.rs:192-247`）, `tree_limit_hold_for`（`crates/task-dispatch/src/dispatcher/tree_units.rs:795-848`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:123-143` | 判断→副作用 | gate | 段の統合の開始、final review の開始 | `start_integration_from_ready`, `start_final_review_from_ready` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:153-214` | 判断 | task.routing、`config.execution` | なし（planner か・tier・budget を局所の task に書く） | — |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:217-251` | 判断 | `task.attempts`・`updated_at`、`infra_backoff`、時計 | 待ちの記録、`infra_backoff.remove`（memory） | `retry_backoff`, `now_utc` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:254-259` | 副作用 | browser の前提 | `store.apply_transition_with_events` | `browser_prereq_hold`（`crates/task-dispatch/src/dispatcher/browser_prereq.rs:73-96`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:262-285` | 判断 | write set の予約 | 保留の記録・待ちの記録（memory） | `write_reservation_for`, `write_set_blocker`, `note_write_set_hold` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:291-315` | 判断＋副作用 | config のクラスタ | `warned_unroutable`・`unroutable`（memory）、道具の無い担当の task を block（store） | `resolve_cluster`, `block_task_missing_cluster_tool` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:316-382` | 判断＋副作用 | `cluster_cooldown`・`cluster_connected`・使用数 | ssh の自動接続（外部 process）、接続状態の更新、クラスタを使えない記録 | `try_auto_connect_cluster`（`crates/task-dispatch/src/dispatcher/cluster.rs:687-698`）, `set_cluster_connected`, `mark_cluster_unavailable`（`crates/task-dispatch/src/dispatcher/cluster.rs:583-672`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:383-470` | 副作用 | workspace の root、WU の準備失敗 | task dir の用意（file system と store）、WU の worktree の作成、scope の snapshot、`wu_prepare_failures`（memory） | `pin_task_root`（`crates/task-dispatch/src/dispatcher/workspaces.rs:228-239`）, `task_workspaces_for`, `prepare_work_unit_workspace`（`crates/task-dispatch/src/dispatcher/work_units.rs:889-1002`）, `on_work_unit_prepare_failed`, `wu_base::scope_env` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:474-487` | 判断 | 知識の接続先 | なし（局所の task の harness・tier・budget） | `knowledge_fallback_reason`（`crates/task-dispatch/src/dispatcher.rs:2556-2572`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:495-551` | 判断＋副作用 | config、org、events | lane assist の保留・分類待ちの記録（store） | `decide_lane`（`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1391-1463`）, `decide_lane_for_work_unit`, `defer_for_classification`・`apply_lane_assist`（`crates/task-dispatch/src/dispatcher/lane_assist.rs:234-377`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:555-568` | 判断 | task | なし（`RouteInput`・run id） | `harness_request_for` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:571-600` | 副作用 | lane の決定 | `store.record_route_features` | — |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:602-641` | 判断 | route registry、account、quota、routing override | `route_state` の account 一覧の cache（memory） | `select_route_with`（`crates/task-dispatch/src/dispatcher/route_select.rs:992-1015`）, `compare_route_priority`（`crates/task-dispatch/src/dispatcher/script_compare.rs:179-190`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:642-665` | 判断＋副作用 | route の結果 | route を拒んだ記録（store） | `route_adapter`, `route_launch_target`, `note_route_refusal`（`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1621-1657`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:667-688` | 判断＋副作用 | credential、campaign の pin | credential の保留（memory）、variant task の block（store） | `credential_gate_hold`（`crates/task-dispatch/src/dispatcher/credential_gate.rs:62-78`）, `variant_run_pin`, `block_variant_task` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:689-720` | 副作用 | — | `store.acquire_lease`（段の lease）、`store.acquire_work_unit_lease` | — |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:721-803` | 副作用 | — | `WorkerStarted`、`store.record_route_decision`、`RoutingDecided`、知識 fallback の progress | — |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:805-824` | 判断 | config | なし（RunLimits・remote・container の決定） | `remote_ssh_settings`, `container_decision` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:825-841` | 判断＋副作用 | events、comments、子 task、org | 見つからない skill の progress event | `run_extras`（`crates/task-dispatch/src/dispatcher/run_context.rs:25-439`）, `token_budget::run_watch` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:849-883` | 副作用 | session の行 | session の retire・event の追記 | `resolve_continuation_session`（`crates/task-dispatch/src/dispatcher/continuation_session.rs:31-183`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:888-923` | 副作用 | events | `store.run_index_start`（worker）、WU の run 開始（`work_unit_transition`・`run_index_start`） | `start_work_unit_run`（`crates/task-dispatch/src/dispatcher/work_units.rs:1554-1622`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:925-975` | 判断 | WU、worktree | なし（`extras` の env・target・direct route） | `work_unit_scope_env`, `cargo_target_fallback`, `is_tree_node` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:981-1042` | 判断＋副作用 | org、runs | `store.run_index_start`（planner） | `execution_planner_context`, `resolve_profile` |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:1045-1108` | 副作用 | events | route 選択の記録、`task_workspaces`（memory）、`quota_begin`（memory）、write base の記録、routing context の記録 | `record_route_selection`, `quota_begin`, `capture_run_write_bases`, `record_routing_context`（`crates/task-dispatch/src/dispatcher/routing_context.rs:255-274`） |
| `crates/task-dispatch/src/dispatcher/dispatch_run.rs:1109-1143` | 副作用 | — | worker の spawn（tokio task と子 process）、write set の予約・`running.insert`（memory） | `spawn_worker`（`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1289-1376`）, `reserve_write_set` |

判断と副作用が交互に入る所:

- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:89-104` で担当と gate を書いた後に、217-251（backoff）・262-285（write set）・316-382（クラスタ）の判断で `Ok(false)` を返すことがある。書き込みは残り、この tick の dispatch はしない。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:108-112`: `wu_dispatch_gate` が WU の遷移を書き、その戻り値（`WuDispatchGate`）で 123-143 の分岐が決まる。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:343-380`: ssh の自動接続の結果で、続けるか・クラスタを使えないと記録して止まるかが決まる。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:415-469`: worktree の用意と scope の snapshot の成否で、続けるか・準備失敗として止まるかが決まる。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:571-605`: route の特徴量を store に書いた結果（hash）が、route 選択の入力 `RouteContext.features` になる。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:694-720`: lease を取れたかで続けるかが決まる。取った後の 722・752・773 の `?` で失敗すると、lease は取ったまま run は起きない（lease 切れの回収を待つ）。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs:868-882`: continuation session の解決（session の retire と event の追記）の結果で `extras.session` が決まり、900 の run index の `session_id` に入る。

純粋な入力→提案に切り出す案:

- admission: `DispatchAdmissionInput { task, gate: WuDispatchGate, running_for_task, load: RunLoadView, aborted: bool, disk: DiskView, backoff: BackoffView { attempts, updated_at, infra_until }, write_sets: WriteSetView, cluster: ClusterView, now, limits: DispatchLimits }` → `AdmissionProposal = Hold(TaskWait) | Integrate(WorkUnitRow) | FinalReview | Skip | Launch(LaunchPlan { run_key, planner: bool, budget, lane: Option<LaneDecision> })`。41-86・153-251・262-285・316-330 をここに入れる。
- launch: `RunLaunchInput { plan: LaunchPlan, route: RouteOutcome, adapter, extras_view, session: Option<SessionPick> }` → `RunLaunchProposal { lease: LeaseRequest, events: Vec<Event>, run_row: RunRow, quota: QuotaBegin, write_reservation, spawn: SpawnSpec }`。555-568・805-824・925-1042 の判断をここに入れる。
- 実行側は「割り当て・gate の書き込み → admission → 作業場所の用意 → route → lease → events と run index を 1 transaction → spawn」の順に並べ、admission が Hold なら書き込みをしない順にする。

owner 候補: execution-coordination（admission と gate。41-285）。route と lane（495-688）は routing、クラスタ（291-382）は cluster-runtime、lease・spawn（689-1143）は run-supervisor に分ける。

## 関数: spawn_review

位置: `crates/task-dispatch/src/dispatcher/review_spawn.rs:347-1094`

Reviewing の task の review を起こす。file lock を取り、token 予算・人の承認・reviewer の選択を判断し、repo ごとに target へ rebase し、delivery の行と event を書き、reviewer run を tokio task で spawn する。repo ごとの同期（487-711）は git の操作と判断が 1 行ごとに交互に入る。

| 区間 | 種類 | 読む状態 | 書く状態・外への作用 | 呼ぶ helper |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:353-364` | 判断 | `accepting_new_work`・`disk_ready`、`store.get` | なし | `held_by_disk_critical` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:365-388` | 副作用 | task dir | `runs/` の作成、lock file の open・`try_lock`（file system） | `task_dir` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:390-395` | 判断 | `store.get`（lock の後に読み直す） | なし | — |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:398-412` | 判断→副作用 | token 使用量、resync の回数 | `store.apply_transition(TokenBudgetExhausted)`、resync 停止の event・notice | `token_budget::exhausted`, `target_resync_exhausted`（`crates/task-dispatch/src/dispatcher/review_spawn.rs:1250-1361`） |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:414-424` | 判断 | events、子 task | `awaiting_human`（memory） | `resolve_human_approvals`（`crates/task-dispatch/src/dispatcher/child_tasks.rs:349-394`） |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:426-463` | 判断＋副作用 | 案件の `auto_integrate`、route | reviewer の route 選択の記録・event | `needs_reviewer_run`, `pick_reviewer`（`crates/task-dispatch/src/dispatcher/review_spawn.rs:1368-1581`） |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:469-484` | 副作用 | workspace の種類 | 子 task の branch を git commit、remote のときの progress event | `commit_child_branch`（`crates/task-dispatch/src/dispatcher/phase_integration.rs:345-371`） |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:487-711` | 判断と副作用の交互 | worktree の git 状態、events（fallback head） | behind-target の観測の記録（544・548）、target への rebase（546）、integration repair の開始（581）、`IntegrationRepairExhausted`（592-604・633-646）、progress event、stale の延期（682） | `review_target_ref`, `fallback_head`, `observe_behind_target`（`crates/task-dispatch/src/dispatcher/behind_target.rs:12-33`）, `task_ops::changes::sync_onto_target`, `try_integration_repair`（`crates/task-dispatch/src/dispatcher/review_verdict.rs:772-846`）, `defer_stale_review`（`crates/task-dispatch/src/dispatcher/review_spawn.rs:1101-1137`） |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:714-726` | 判断＋副作用 | 合格の記録 | reviewer を省く理由の event | `reviewer_reuse`（`crates/task-dispatch/src/dispatcher/review_spawn.rs:1142-1221`） |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:727-738` | 判断 | 決定・回答、policy 違反 | なし | `health_job`, `review_human_inputs`, `review_policy_violations` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:745-756` | 副作用 | `config.delivery` | delivery の行を始める | `task_ops::delivery::begin` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:757-819` | 判断と副作用の交互 | delivery の行、events、WU 行 | stale の延期、`store.delivery_save`（CAS）、`ReviewTargetSynced`、`IntegrationRepairResolved` | `pending_integration_repair` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:824-850` | 判断 | task の種類、events、クラスタ | なし（plan check・aggregate・remote 設定・成果物の一覧） | `plan_depth`, `has_aggregate_transition`, `cluster_of`, `artifacts_for_run` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:852-914` | 副作用 | runs | `WorkerStarted{role: reviewer}`、`store.run_index_start`、route 選択の記録、`quota_begin`（memory） | `record_route_selection`, `quota_begin` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:915-995` | 判断 | config、workspace、repo の git（候補の必須検査を読む） | なし（`repo_checks` と review 用の task の写し） | `required_checks::candidate_required_checks`, `workspace_config::load`, `review::review_view` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:996-1077` | 副作用 | — | tokio task の spawn（review の検査と reviewer の process、sync guard の再確認、`tx.send(Completion::Review)`） | `review_task`, `health.attach` |
| `crates/task-dispatch/src/dispatcher/review_spawn.rs:1078-1093` | 副作用 | — | `reviewing.insert`（memory） | — |

判断と副作用が交互に入る所:

- `crates/task-dispatch/src/dispatcher/review_spawn.rs:381-395`: lock を取れたかで続けるかが決まり、取った後に task を読み直して状態を判断し直す。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs:546-679`: `sync_onto_target`（git rebase）の結果の種類で、同期した・integration repair を起こして return・repair 上限で同期を省く・abort 失敗で止まる・省く、のどれかを決める。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs:680-692`: rebase の後に target を読み直し、進んでいれば stale として延期する。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs:757-819`: delivery の行の base・head と同期の snapshot を比べて延期するか決め、`delivery_save` の CAS の結果で失敗にするか決める。`ReviewTargetSynced` を追記した直後に events を読み直し（803）、integration repair を解けたか判断する。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs:410-463` の helper（`target_resync_exhausted`・`pick_reviewer`）は判断を返しつつ event も書く。

純粋な入力→提案に切り出す案:

- 入口: `ReviewAdmissionInput { task, accepting: bool, disk_ready: bool, token_budget: Option<(u64, u64)>, resync_exhausted: bool, approvals: Option<HumanApprovals>, needs_reviewer: bool }` → `ReviewAdmission = Skip | Hold | Fail(Trigger, Event) | Proceed { needs_reviewer }`。
- repo ごとの同期: git の操作は実行側に残し、結果の分類を純粋関数にする。`RepoSyncInput { repo, fallback_head, head, target_contained, outcome: SyncOutcome, target_after: Option<String>, repair_budget }` → `RepoSyncProposal = Synced(ReviewSync) | Skip { events } | Repair(RepairRequest) | Halt { events } | Stale(StaleDeferral)`。
- 起動: `ReviewLaunchInput { task, synced: Vec<ReviewSync>, delivery: Option<DeliveryRow>, events, reviewer: Option<ReviewerPick> }` → `ReviewLaunchPlan { delivery_update: Option<DeliveryRow>, events: Vec<Event>, run_row: Option<RunRow>, repo_checks: Vec<RequiredRepoChecks>, extras: ReviewExtras }`。

owner 候補: execution-coordination（review の進行。353-463・824-995）。repo の同期と delivery（487-819）は project-orchestration、reviewer の route（426-463・852-914）は routing、spawn（996-1093）は run-supervisor に分ける。

## 関数: on_review_finished

位置: `crates/task-dispatch/src/dispatcher/review_verdict.rs:20-613`

review の結果（`ReviewOutcome`）を受け、reviewer run の終わり・quota・cooldown・判定・delivery・子 task・計画の展開・task の遷移・報告を書く。early return が 94-96・126-140・182-207・282-319・459-490・494-500 にあり、それぞれが自分の分の event を書いて抜ける。

| 区間 | 種類 | 読む状態 | 書く状態・外への作用 | 呼ぶ helper |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:26` | 副作用 | — | `reviewing.remove`（memory） | — |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:30-51` | 判断→副作用 | 用意した reviewer run と実際の run | quota の解放（memory）、起動しなかった run を閉じる event と run index | `release_quota_if_tracked`（`crates/task-dispatch/src/dispatcher/quota_book.rs:56-80`）, `close_aborted_run`（`crates/task-dispatch/src/dispatcher/leases.rs:478-525`） |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:53-69` | 判断 | entry、outcome | なし（`WorkerFinished{role: reviewer}` の組み立て） | `wall_ms_since` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:73-93` | 判断＋副作用 | account book、quota group | quota group を閉じる（memory）、group の他 member の event を追記 | `started_model_of`, `resolve_quota_estimate`（`crates/task-dispatch/src/dispatcher/quota_book.rs:119-332`） |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:94-96` | 判断 | `store.get` | なし | — |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:100-125` | 副作用 | `store.org_list` | 部署の lead session の使用量（`store.node_session_touch`） | `department_of` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:126-140` | 判断→副作用 | `task.status` | 破棄のときの event 追記と run index の終了 | `finish_reviewer_run_index` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:142-179` | 判断→副作用 | 受け入れ条件、verdicts、必須検査の束縛 | `ReviewerPassed` | — |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:182-207` | 判断→副作用 | `target_stale` | stale の延期、event・run index、`pending_subjects`（memory） | `defer_stale_review` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:210-226` | 副作用 | 必須検査の結果 | `RequiredChecksEvaluated` | — |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:228-258` | 副作用 | provider の失敗の分類 | account・source の cooldown | `record_account_failure`・`record_source_failure`（`crates/task-dispatch/src/dispatcher/route_failure.rs:76-152`） |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:265-319` | 判断→副作用 | events（連続延期・連続インフラ失敗）、`config.max_reviewer_retries`・`max_requeues` | 延期の progress・`WorkerFinished`・cooldown の event、run index、`pending_subjects` | `consecutive_reviewer_infra_failures`, `consecutive_reviewer_requeues` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:320-349` | 判断 | 受け入れ条件 | なし（上限に達したとき未判定の reviewer 条件を不合格で補う） | — |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:351-355` | 判断 | 子 task、events | なし（`outcome` を policy で直す、食い違いの検出） | `policy_review_gate`（`crates/task-dispatch/src/health_review/policy_review.rs:255-304`）, `review_discrepancy` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:358-389` | 判断→副作用 | `store.delivery_get` | `store.delivery_save`（MergeQueued か Blocked） | — |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:393-457` | 判断＋副作用 | verdicts | reviewer run の run index の終了（421）、events の組み立て | `finish_reviewer_run_index` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:459-490` | 判断→副作用 | 子 task の状態 | event 追記と `awaiting_children`（memory）、失敗した子の escalation（承認の記録）、aggregate run の予約 | `pending_children`, `escalate_failed_children`（`crates/task-dispatch/src/dispatcher/child_tasks.rs:145-229`）, `needs_aggregate_run`, `schedule_aggregate_run` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:494-500` | 副作用 | WU 行 | 局所 repair か人の判断待ち（task_ops 経由で store に書く） | `try_review_repair`（`crates/task-dispatch/src/dispatcher/review_verdict.rs:633-737`） |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:502-560` | 判断＋副作用 | org、案件の workspace・repos | 計画の展開（子 task の挿入）、`store.complete_plan` か `apply_transition_with_events` | `fix_plan_for_harness`, `materialize_logging` |
| `crates/task-dispatch/src/dispatcher/review_verdict.rs:561-611` | 副作用 | 遷移の結果 `outcome.next`、成果物の result.json | run report（Done・Failed） | `reports::record_run_report`, `task_worker::read_result_report_kind` |

判断と副作用が交互に入る所:

- `crates/task-dispatch/src/dispatcher/review_verdict.rs:81`: quota の見積もり（group を閉じる memory の書き込みと他 member の event 追記）が、94 の task の存在確認より前にある。task が消えていても quota group は閉じる（コメントの意図どおり）。
- 「reviewer の `WorkerFinished` と quota の event を追記し run index を閉じる」処理が `crates/task-dispatch/src/dispatcher/review_verdict.rs:129-138`・`193-202`・`301-313`・`421-427` の 4 か所に写されている。early return ごとに end の種類だけが違う。
- `crates/task-dispatch/src/dispatcher/review_verdict.rs:166` の `ReviewerPassed` は 182 の stale 判定より前に書く（stale でも合格の束縛は残す設計）。
- `crates/task-dispatch/src/dispatcher/review_verdict.rs:388` の `delivery_save` と 421 の run index の終了は、502-560 の task 遷移より前に別々に commit される。遷移が `InvalidTransition`（607）で落ちても、delivery の行は MergeQueued か Blocked に進んでいる。
- `crates/task-dispatch/src/dispatcher/review_verdict.rs:483-488`: `escalate_failed_children` と `needs_aggregate_run` は判断の結果を返しつつ、内部で書き込み（承認の記録・aggregate の予約）をする。

純粋な入力→提案に切り出す案:

- `ReviewVerdictInput { task: Option<Task>, entry: Option<ReviewEntryView>, outcome: ReviewOutcome, events: Vec<Event>, delivery: Option<DeliveryRow>, pending_children: usize, failed_children: Vec<ChildView>, needs_aggregate: bool, limits: ReviewLimits }`
- → `ReviewVerdictProposal { close_unstarted: Option<RunId>, quota: Option<QuotaRequest>, cooldown: Option<CooldownReport>, reviewer_run_end: Option<(RunEnd, RunIndexStatus)>, events: Vec<Event>, delivery_update: Option<DeliveryRow>, apply: VerdictApply, report: Option<TerminalReport> }`
- `VerdictApply = Discard | DeferStale(StaleDeferral) | DeferSupply | AwaitChildren | EscalateChildren | Aggregate | Repair | CompletePlan(Plan) | Transition(Trigger)`
- 4 か所の「reviewer run を閉じる」写しは `reviewer_run_end` の 1 つにまとまる。実行側が `events`・`delivery_update`・遷移を同じ transaction で書けば、388 と 502-560 の分離が消える。

owner 候補: execution-coordination（判定の適用と task 遷移。126-226・320-355・393-560）。quota と cooldown（73-93・228-258）は routing、delivery・子 task・計画の展開（358-389・459-490・502-542）は project-orchestration に分ける。

## 関数: reclaim_expired_leases

位置: `crates/task-dispatch/src/dispatcher/leases.rs:217-436`

Running の task ごとに lease を調べ、切れたか持ち主の daemon が居ないなら、生きている run・WU の検査・段の統合があれば延長し、無ければ run を止めて infra requeue（か失敗）に遷移させる。task ごとの loop の中で判断と副作用が交互に入る。

| 区間 | 種類 | 読む状態 | 書く状態・外への作用 | 呼ぶ helper |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/leases.rs:218-237` | 判断 | `store.list(Running)`、lease、時計、instance の一覧（持ち主の daemon が居るか） | なし（孤児かどうか） | `is_eligible`, `is_phase_lease_holder`, `holds_task_in_hand`（`crates/task-dispatch/src/dispatcher/leases.rs:726-732`）, `lease_holders_gone`（`crates/task-dispatch/src/dispatcher/leases.rs:738-760`） |
| `crates/task-dispatch/src/dispatcher/leases.rs:238-240` | 副作用 | — | 孤児の引き継ぎの event | `note_orphan_takeover`（`crates/task-dispatch/src/dispatcher/leases.rs:763-785`） |
| `crates/task-dispatch/src/dispatcher/leases.rs:244-265` | 判断→副作用 | `running`、process group が生きているか | `store.renew_lease` | `task_worker::process_group::group_alive` |
| `crates/task-dispatch/src/dispatcher/leases.rs:268-286` | 判断→副作用 | `checking` | `store.renew_lease`（WU の検査中） | — |
| `crates/task-dispatch/src/dispatcher/leases.rs:289-303` | 判断→副作用 | `integrating` | `store.extend_task_lease`（段の統合中） | `integration_ttl`（`crates/task-dispatch/src/dispatcher/phase_integration.rs:260`） |
| `crates/task-dispatch/src/dispatcher/leases.rs:305-319` | 副作用 | `running` | `running.remove`（memory）、process group の停止（kill） | `stop_run`（`crates/task-dispatch/src/dispatcher/leases.rs:454-463`） |
| `crates/task-dispatch/src/dispatcher/leases.rs:323-336` | 判断 | `store.work_units_for` | なし（WU の run id の一覧） | — |
| `crates/task-dispatch/src/dispatcher/leases.rs:340-349` | 副作用 | 成果物の result.json | 終端の結果で run を確定（終了の経路を呼ぶ） | `finalise_from_result_json`（`crates/task-dispatch/src/dispatcher/worker_finish.rs:1892-1930`） |
| `crates/task-dispatch/src/dispatcher/leases.rs:350-401` | 判断 | `store.events_for`（連続 infra requeue 数）、`config.max_infra_retries` | なし（trigger・outcome・error class・`WorkerFinished` の列） | `consecutive_infra_requeues` |
| `crates/task-dispatch/src/dispatcher/leases.rs:402-433` | 副作用 | 遷移の結果 | `store.apply_transition_with_events`、`infra_backoff`（memory）、WU と段の統合の照合、decision log | `reconcile_work_unit_run`（`crates/task-dispatch/src/dispatcher/phase_integration.rs:228-246`）, `reconcile_integration`, `record_decision_log` |

判断と副作用が交互に入る所:

- `crates/task-dispatch/src/dispatcher/leases.rs:239` の孤児の event は、244-303 の延長の判断より前に書く。run が生きていて延長（`continue`）した場合も、孤児の event は残る。
- `crates/task-dispatch/src/dispatcher/leases.rs:252-263`・`275-284`: `renew_lease` の戻り値（true・false・Err）で、延長して次へ・回収へ進む・次の tick に回す、が決まる。
- `crates/task-dispatch/src/dispatcher/leases.rs:317` で process を止めてから、342 で result.json による確定を判断し、402 で遷移する。遷移が `InvalidTransition`（429）で落ちても、process は止まり `running` から消えている。326・356 の `?` で store の読み出しが落ちても同じ。
- `crates/task-dispatch/src/dispatcher/leases.rs:342-349`: 確定の経路を 1 本でも通ったら、この task の残りの回収を次の tick に回す（副作用の結果で判断が変わる）。

純粋な入力→提案に切り出す案:

- `LeaseSweepInput { task, lease, now, holders_gone: bool, in_hand: bool, alive_run: Option<RunId>, checking_run: Option<RunId>, integrating: bool, wu_runs: Vec<RunId>, run_since: Option<OffsetDateTime>, infra_n: u32, limits: LeaseLimits }`
- → `LeaseAction = Skip | Renew { run_id, ttl } | ExtendTask { ttl } | Reclaim(ReclaimPlan { orphan_note: Option<Event>, stop: Vec<RunKey>, finalise_candidates: Vec<RunId>, trigger, finished: Vec<Event>, backoff_until: Option<OffsetDateTime>, wu_reason, reconcile_integration: bool })`
- 実行側は `Renew`・`ExtendTask` の戻り値が false のときだけ `Reclaim` を求め直す。孤児の event は `Reclaim` の中に入れ、延長したときは書かない。

owner 候補: run-supervisor（lease と process の監督の全体）。WU・段の統合の照合（416-425）は execution-coordination の port を呼ぶ形にする。

## 関数: route_state

位置: `crates/task-dispatch/src/dispatcher/route_select.rs:757-978`（`crates/celerisctl/src/commands/worker.rs:170` の同名関数とは別）

走っている run と reviewer run の (source, account) の使用数、account の残量・cooldown・認証失敗・CoS の予約、account ごとの上限を集め、`RouteState`（値）を返す。ほぼ全体が判断で、副作用は account の一覧の file system 走査とその cache だけ。

| 区間 | 種類 | 読む状態 | 書く状態・外への作用 | 呼ぶ helper |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/route_select.rs:758-766` | 判断 | route registry、時計（`now_unix_fn`）、`[cos]` の config | なし | — |
| `crates/task-dispatch/src/dispatcher/route_select.rs:767-797` | 判断 | `running`、`reviewing` | なし（source・account ごとの使用数） | — |
| `crates/task-dispatch/src/dispatcher/route_select.rs:798-820` | 判断 | CoS chat の route の予約、source の account 一覧 | なし | `account_adapter_for_source_kind`, `source_account_pool` |
| `crates/task-dispatch/src/dispatcher/route_select.rs:821-836` | 副作用 | `config.accounts` の root、account の dir | account の dir の走査（file system の読み）と `accounts_scan_cache.insert`（memory） | `scan_accounts` |
| `crates/task-dispatch/src/dispatcher/route_select.rs:837-931` | 判断 | account book（Mutex の lock）、時計 | なし（account の並び、source の残量、cooldown・認証失敗・CoS 予約の集合） | `evaluate`, `five_hour_utilization`, `remaining_fraction` |
| `crates/task-dispatch/src/dispatcher/route_select.rs:933-946` | 判断 | `source_cooldowns` | なし | — |
| `crates/task-dispatch/src/dispatcher/route_select.rs:947-963` | 判断 | `account_limits`、既定の上限 | なし | `capacity::effective_account_limit`, `capacity::account_run_limit` |
| `crates/task-dispatch/src/dispatcher/route_select.rs:964-972` | 判断（test-hooks） | `test_cos_reserved_sources`（`#[cfg(test)]`） | なし | — |
| `crates/task-dispatch/src/dispatcher/route_select.rs:973-977` | 判断 | — | なし | — |

判断と副作用が交互に入る所:

- `crates/task-dispatch/src/dispatcher/route_select.rs:822-836`: 最初の呼び出しで account の dir を走査して cache に入れ、2 回目からは cache を使う。cache の中身で account pool が決まるので、走査の時点が判断に影響する。cache を消す所は別の module にある。
- `crates/task-dispatch/src/dispatcher/route_select.rs:844`: account book の lock を取ったまま 847-931 の並べ替えと評価をする。lock の中で時計（`now`）を使うが、読むのは 759 で取った値だけ。
- `crates/task-dispatch/src/dispatcher/route_select.rs:964-972`: 本番の関数の中に `#[cfg(test)]` の分岐がある。

純粋な入力→提案に切り出す案:

- `RouteStateInput { now, cos: bool, worker_reserve: Option<f64>, running_routes: Vec<RouteUse { source, account, cos }>, reviewing_routes: Vec<(SourceId, AccountId)>, cos_reservations: Vec<(SourceId, AccountId)>, registry: Arc<LlmRouteRegistry>, scanned_accounts: HashMap<AccountAdapter, Vec<AccountDir>>, account_states: HashMap<(AccountAdapter, AccountId), AccountState>, source_cooldowns: HashMap<SourceId, OffsetDateTime>, account_limits: Option<HashMap<(SourceId, AccountId), u32>>, default_max_runs: usize, cos_reserved_override: HashSet<SourceId> }` → `RouteState`（既存の値型をそのまま提案にする）。
- 走査と cache は呼び出し側（route の選択の前）に移し、`scanned_accounts` として渡す。`#[cfg(test)]` の分岐は `cos_reserved_override` の入力に置き換える。

owner 候補: routing。964-972 の試験用の分岐は test-hooks。

## 関数: resolve_quota_estimate

位置: `crates/task-dispatch/src/dispatcher/quota_book.rs:119-332`

run の終わりに quota の消費を見積もり、`QuotaEstimated` event を返す。account pool を使わない run は free、使う run は quota group（同じ account で同時に走った run の集まり）を閉じ、単独なら計測値、group が閉じたら按分、閉じていなければ unknown を返す。group が閉じたときは他 member の event をここで直接追記する。

| 区間 | 種類 | 読む状態 | 書く状態・外への作用 | 呼ぶ helper |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/quota_book.rs:129-135` | 判断 | usage、model | なし（重み付き token の計算） | `task_core::output_input_ratio`, `quota::weighted_tokens` |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:137-158` | 判断 | account・adapter が無い | なし（free の event） | `quota::free_window`, `representative_method` |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:160-176` | 判断 | account book の usage（Mutex の lock） | なし（終了後の usage と group の member） | `account_book`, `accounts::quota_source_label` |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:177` | 副作用 | quota group | `quota_activity.end` で group から抜ける・閉じる（memory） | `QuotaActivity::end`（`crates/task-dispatch/src/accounts.rs:735`） |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:179-201` | 判断 | 較正値 | なし（単独の run の窓ごとの判定） | `quota_calibration.calibration`, `decide_quota_window`（`crates/task-dispatch/src/dispatcher/quota_book.rs:85-108`） |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:202-209` | 副作用 | 計測した窓 | `quota_calibration.record`（memory） | `QuotaCalibration::record`（`crates/task-dispatch/src/accounts.rs:798`） |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:210-266` | 判断 | — | なし（単独の event、group が閉じていないときの unknown の event） | `quota::decide_window` |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:267-313` | 判断 | group の before・after、member | なし（member ごとの按分の event） | `quota::snapshot`, `decide_quota_window` |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:314-316` | 副作用 | — | 他 member の event を `store.append_event` | — |
| `crates/task-dispatch/src/dispatcher/quota_book.rs:318-331` | 判断 | — | なし（自分の event が無いときの unknown） | — |

判断と副作用が交互に入る所:

- `crates/task-dispatch/src/dispatcher/quota_book.rs:177-178`: `quota_activity.end` の結果（Exclusive・Pending・Closed）で分岐が決まる。memory の group 状態はこの時点で変わる。
- `crates/task-dispatch/src/dispatcher/quota_book.rs:206-208`: 計測した窓の較正値を、呼び出し側の遷移が commit される前に memory へ書く。呼び出し側の遷移（`crates/task-dispatch/src/dispatcher/worker_finish.rs:1553-1556`）が落ちても較正値は更新されたまま。
- `crates/task-dispatch/src/dispatcher/quota_book.rs:314` は他 member の event をここで直接書き、自分の event は返して呼び出し側の transaction（`crates/task-dispatch/src/dispatcher/worker_finish.rs:1505`、`crates/task-dispatch/src/dispatcher/review_verdict.rs:432-434`）で書く。1 つの group の event が 2 つの書き方に分かれる。

純粋な入力→提案に切り出す案:

- `QuotaEstimateInput { task_id, run_id, work_unit_id, account: Option<AccountId>, adapter: Option<AccountAdapter>, provider, model, usage: Usage, after: Option<UsageSnapshot>, group: QuotaEndOutcome, calibration: HashMap<QuotaWindow, Calibration> }` → `QuotaEstimateProposal { own: Event, member_events: Vec<(TaskId, Event)>, calibration_records: Vec<CalibrationRecord { source, window, used_pct, weighted_tokens }> }`
- `quota_activity.end` と account book の読み出しは呼び出し側に残し、その結果を `group`・`after` として渡す。`member_events` と `calibration_records` は呼び出し側が遷移と同じ順で書く。

owner 候補: routing（quota の会計）。
