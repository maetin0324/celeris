---
title: "健全性v2 R4: Dispatcher の field 所有・重複処理・大きい関数の区間・反例試験の不足"
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R4: Dispatcher の field 所有・重複処理・大きい関数の区間・反例試験の不足

## 要約

コードは変えていない。数は `change-map.json`（下の再生成のコマンドの出力）から取った。所有の決定ではなく、R4 の分割の設計に使う実測である。

**field**: `crates/task-dispatch/src/dispatcher.rs` の `pub struct Dispatcher` の field は 116 件（うち `#[cfg(test)]` 10 件）。116 件すべてに owner 候補と根拠（`fields[].owner`・`owner_reason`）がある。規則は [owners.json](owners.json)、割り当ての経緯は [cm-owners.md](cm-owners.md)。

| owner 候補 | field 数 |
|---|---:|
| execution-coordination | 31 |
| composition-root | 22 |
| cluster-runtime | 16 |
| test-hooks | 15 |
| routing | 14 |
| host-maintenance | 9 |
| run-supervisor | 7 |
| project-orchestration | 2 |
| 合計 | 116 |

**owner_conflict**: 書き手の module の owner が 2 種類以上ある field は 12 件。うち 7 件（`awaiting_human`・`checking`・`infra_backoff`・`integrating`・`pending_subjects`・`reviewing`・`running`）は `dispatcher/leases`（run-supervisor）が書き手に入る。module 名は `crates/task-dispatch/src/` からの相対 path（`.rs` を省略）。

| field | 候補 owner | 書き手の owner（module） |
|---|---|---|
| `accounts_scan_cache` | routing | execution-coordination（dispatcher, dispatcher/snapshot）; routing（dispatcher/route_select） |
| `awaiting_human` | execution-coordination | execution-coordination（dispatcher/review_spawn）; run-supervisor（dispatcher/leases） |
| `checking` | execution-coordination | execution-coordination（dispatcher, dispatcher/work_units）; run-supervisor（dispatcher/leases） |
| `cos_chat_launch` | project-orchestration | project-orchestration（dispatcher/cos_chat/triage, dispatcher/project_retention, dispatcher/project_wake）; run-supervisor（dispatcher/cos_chat/launch） |
| `infra_backoff` | run-supervisor | execution-coordination（dispatcher/dispatch_run）; run-supervisor（dispatcher/leases, dispatcher/worker_finish） |
| `integrating` | execution-coordination | execution-coordination（dispatcher/phase_integration）; run-supervisor（dispatcher/leases） |
| `pending_subjects` | execution-coordination | execution-coordination（dispatcher/phase_integration, dispatcher/review_spawn, dispatcher/review_verdict）; run-supervisor（dispatcher/leases, dispatcher/worker_finish） |
| `reviewing` | run-supervisor | execution-coordination（dispatcher/review_spawn, dispatcher/review_verdict）; run-supervisor（dispatcher/leases, dispatcher/ownerless_runs） |
| `running` | run-supervisor | execution-coordination（dispatcher/dispatch_run）; run-supervisor（dispatcher/leases） |
| `scratch` | host-maintenance | execution-coordination（dispatcher）; host-maintenance（dispatcher/housekeeping） |
| `stagnation` | project-orchestration | execution-coordination（dispatcher, dispatcher/tree_units）; project-orchestration（dispatcher/project_wake） |
| `task_workspaces` | execution-coordination | execution-coordination（dispatcher/dispatch_run）; host-maintenance（dispatcher/housekeeping） |

owner_conflict が false でも競合が無いとは限らない（lock の中・別名経由・範囲外の型の変更は字句の測定で拾えない。`method.limits`）。

**equivalence**: 6 件（`change-map.json` の最上位の `equivalence`、元は `equivalence/<id>.json`）。verdict は 6 件とも different（経路全体は同じ処理ではない）。共有できる契約は次の範囲に限る。記録は [cm-equiv-run.md](cm-equiv-run.md) と [cm-equiv-flow.md](cm-equiv-flow.md)。

| id | verdict | 共有できる契約（1 行） |
|---|---|---|
| continuation-resume | different | 保存 session の互換性・拒否・context 上限の事実から Resume か Fresh を返す純粋な判定まで（session の解決 `resolve_continuation_session` は既に共通）。 |
| lease-renew | different | `renew_lease(task_id, run_id, ttl) -> Result<bool, StoreError>` の store 操作まで（既に共通）。Ok(false) と Err を同じに扱う wrapper は lease 回収の安全条件を壊す。 |
| retry-backoff | different | infra 障害の「連続回数と上限から再試行するか」の判定と既存の `infra_backoff_delay(n)` まで（worker 完了と lease 回収に同じ判定が重複）。 |
| review-result | different | 決定的 check が不合格なら成功にしない判定と、check 結果から失敗一覧を作る処理まで（check の実行は既に共通）。 |
| run-close | different | run id・usage・metrics・明示した終端から終了記録を組み立てる純粋な処理まで。lease の検証・状態遷移・transaction の境界は一律の close(run) に入れない。 |
| wu-failure | different | 失敗の署名・同じ失敗の回数・replan の残り・replan 禁止の理由から Replan か Ask と理由を返す上限判定まで（似た判定が 3 か所）。 |

**関数と反例試験**: 大きい 7 関数の判断と副作用の区間は下の `## 関数: <名>` の 7 節（finish_worker_result_with・dispatch_one・spawn_review・on_review_finished・reclaim_expired_leases・route_state・resolve_quota_estimate）にある。各節の owner 候補は、その関数の module の owners.json の候補と 7 件とも一致する。lease・restart・同時完了・review/integration・権限/予算の反例試験の不足は末尾の `## 反例試験の不足` の 5 節にある。記録は [cm-functions.md](cm-functions.md) と [cm-test-gaps.md](cm-test-gaps.md)。

**再生成**（repo の root で。build・試験は走らせない。1 回 20 秒ほど）:

```sh
python3 agent-docs/progress/2026-11-11-health-v2-r4/change-map-measure.py --out agent-docs/progress/2026-11-11-health-v2-r4/change-map.json
```

`equivalence/*.json` の形か参照（file・行・関数）が壊れていれば、出力を書かずに exit 1 になる。作業ツリーが HEAD と同じなら同じ bytes を書く。

## 関数の節の読み方

下の `## 関数:` の 7 節は、`crates/task-dispatch/src/dispatcher/` の大きい 7 関数の本体を行番号で「判断」と「副作用」に分ける。コードは変えていない。行番号は WorkUnit cm-functions の base（`$CELERIS_WU_BASE`）の HEAD のもの。

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

## 反例試験の不足

cm-test-gaps による静的調査（2026-11-11、基準 `$CELERIS_WU_BASE`）。以下は試験本文の入力・操作順・assertion と、上の判断／副作用区間を照合した結果であり、試験の実行結果ではない。「不足」は挙げた試験群で固定されていない具体的な組合せを指す。製品の不具合が再現したという意味ではなく、別 crate・release gate を含む全試験の網羅性も主張しない。再現方法は後続の試験設計案で、今回は hook も試験も追加しない。

### lease

- 既存の試験: `crates/task-core/src/store/tests.rs:275` の `acquire_lease_is_exclusive_under_concurrency` は、同一の in-memory store に対する2スレッドの取得を Barrier で揃え、成功が片方だけであることを確認する。`crates/task-core/src/store/tests.rs:861` の `renew_lease_extends_only_the_matching_running_lease` は、保持者の延長、別 run・非 Running・存在しない task の拒否、不要な遷移 event が増えないことを順次確認する。
- 回収側の試験: `crates/task-dispatch/src/dispatcher/tests/work_units.rs:898` の `a_lease_expiry_requeue_closes_the_runs_row` は、期限切れの WU を tick で回収し、終了 event と run 行の HarnessError・終了時刻を確認する。`crates/task-dispatch/src/dispatcher/tests/integration_ready_race.rs:279` の `expired_lease_during_stage_integration_is_extended_not_reclaimed` は、統合中の lease を延長し、Running を維持して最終的に Done へ進めることを確認する。
- 守る不変条件: 単一保持者と、現在の保持者を古い情報で回収しないこと。根拠は `CLAUDE.md`（状態の正本は SQLite）、`agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md`（並列 WU の lease）、`agent-docs/adr/2026-10-20-integration-check-process-group-and-lease.md`（統合中の延長）。対象境界は `reclaim_expired_leases` の読み出し 218–237 と renew 244–303、回収の永続化 402–433。
- 不足する反例: Running の run A の期限切れ snapshot を回収側が取得した直後、別の接続が A を renew する順序、および A を終了させ run B が取得した後に古い回収側が再開する順序。期待結果は、先に確定した renew／B の lease を古い snapshot が消さず、B を止めず、A の requeue を B に適用しないこと。逆順（回収が先に確定）は旧 A の renew が拒否されること。既存の取得競合と逐次 renew 試験だけでは、この読み出しと適用の間の変更を固定していない。
- 決定的な再現方法: 一時 DB を2接続で開き、回収側の snapshot 取得後に試験専用 hook で停止し、他方の commit 完了を出来事待ちして再開する。期限は注入時計と fixture の lease 時刻で揃える。`tokio::time::pause` だけでは SQLite 側の実時計を進められないので、時計の対応も明示する。CPU 負荷や固定 sleep に依存しない。

### restart

- 既存の試験: `crates/task-dispatch/src/dispatcher/tests/ownerless_runs.rs:260` の `handoff_shutdown_closes_the_old_reviewer_run_and_the_new_active_rereviews` は、旧 reviewer が lock を持つ間は新側が起動せず、shutdown で旧 run が Cancelled・終了 event 1件となり、引き継ぎ後の review が Done、running 行が空になることを確認する。`crates/task-dispatch/src/dispatcher/tests/ownerless_runs.rs:487` の `ownerless_runs_are_reconciled_again_after_the_interval` は注入時計で周期前後の回収を確認する。同ファイル :515 の `an_ownerless_run_is_kept_while_another_instance_lives_until_its_deadline` は、生存する他 instance があっても期限後には閉じることを確認するが、その後に旧側の完了を届けない。
- 回復時の保護: `crates/task-dispatch/src/dispatcher/tests/orphan_work_units.rs:349` の `stale_recovery_cannot_overwrite_a_new_run_replan_or_task_cancel` は、保存しておいた WU の読み取り値に対して新 run・replan・cancel を先に確定し、古い `recover_work_unit` が行と event を変更しないことを確認する。これは WU 回復の比較更新の試験であり、reviewer 完了経路全体の重複適用の試験ではない。
- 守る不変条件: run を取り残さず、遅延結果で終端から再開したり別 run の結果を上書きしたりしない。根拠は `agent-docs/adr/2026-10-03-ownerless-running-runs.md` D1–D3・D5、`CLAUDE.md` の SQLite 正本・events 追記専用。D3 は期限回収後の旧保持者による WorkerFinished の追記を明示的に許すため、全経路で終了 event が1件だけという条件にはしない。
- 不足する反例: 新 active が期限切れ reviewer A を ownerless として閉じ、reviewer B を開始した後、まだ生きていた旧 draining instance から A の合格結果が届く。期待結果は、A の run 行が Running に戻らず、B の登録・quota を A が解放せず、古い候補の合格で delivery を進めないこと。続いてもう一度 dispatcher を作り直し、B の正当な終了後に running 行が残らないことも確認する。対象境界は `on_review_finished` の memory 除去 26、quota 73–93、状態確認 126–140、delivery 保存 358–389 と task 遷移 502–560。
- 決定的な再現方法: 既存の ownerless fixture の pid 生存関数と注入時計を再利用し、旧 reviewer の完了送信を Notify の試験専用 gate で保留する。新側の回収・B 起動の出来事を待ってから旧側を解放し、再起動後まで run ID 別の索引・候補・quota を照合する。CPU 負荷は使わない。

### 同時完了

- 既存の試験: `crates/task-dispatch/src/dispatcher/tests/tick_and_dispatch.rs:663` の `a_human_comment_interrupts_the_running_run_and_the_next_run_carries_it` は、稼働中の worker への人のコメントが Ready への割り込みとなり、attempts を増やさず次 run の前置きに載ることを確認する。`crates/task-dispatch/src/dispatcher/tests/work_units.rs:997` の `a_run_aborted_by_cancel_closes_its_runs_row` は、遅延中の worker を cancel して run 行の Cancelled・終了時刻と終了 event を確認する。両者は完了通知を同じ tick の処理待ちキューへ揃えてはいない。
- 既存の回復保護: `crates/task-dispatch/src/dispatcher/tests/orphan_work_units.rs:386` の `local_run_is_not_recovered_and_task_cancel_does_not_requeue` は、手元にある run を索引だけから回収しないことと、その後の cancel で WU が Cancelled、running map が空になることを確認する。前節の :349 の比較更新試験もあるが、worker 終了処理の副作用を通した比較ではない。
- 守る不変条件: `CLAUDE.md` の「状態は SQLite」「worker の自己申告だけで完了にしない」「events は追記専用」。人の割り込みは `agent-docs/adr/0044-task-management.md`、WU の回復は `agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md` の Ready task の孤児回収の付記。対象境界は `finish_worker_result_with` の WU 遷移 1289–1351、run 終了 1354–1387、段の再判定 1390–1421、task 遷移 1553–1628。
- 不足する反例: run A の Done 通知を処理待ちにし、lease も期限に到達させた tick に、(a) 人のコメント、(b) cancel をそれぞれ確定させる。さらに WU 遷移直後と task 遷移直前の各境界で人の操作を挟む。期待結果は、cancel 後に Done／review／統合へ進まないこと、コメント後の新 run B を A の終了で閉じず、次 run がコメントを受け取ること、回収と終了で retry・report・quota の処理を二重適用しないこと。逆順でも、既に確定した状態に対する人の操作の契約どおりとなること。単独の cancel／コメント試験ではこの組合せを保証しない。
- 決定的な再現方法: fake adapter の完了送信を gate で制御し、キュー投入済みを通知で確認してから tick を呼ぶ。lease 時刻は注入時計で固定し、途中の窓は既存 `test_halt_after_wu_transition` と同種の試験専用 hook で固定する。順序ごとに run ID、WU、task、終了理由、起動回数を検査する。CPU 負荷や偶然の同時終了は使わない。

### review/integration

- 既存の試験: `crates/task-dispatch/src/dispatcher/tests/target_sync.rs:309` の `integration_repair_resumes_review_on_latest_target` は、repair 後に target を進め、最新 target を含む候補で check と reviewer が動き、attempts を保って Done になることを確認する。同ファイル :1336 の `reviewer_pass_is_reused_when_target_advances_without_overlap` と :1422 の `reviewer_runs_again_when_target_advances_touching_the_task_files` は、main の進行と候補の差分が非重複なら合格を再利用し、重複なら reviewer を再実行することを確認する。非重複の場合も新候補への必須 check の束縛を確認している。
- 段の順序: `crates/task-dispatch/src/dispatcher/tests/work_units.rs:1928` の `a_pending_human_replan_runs_the_planner_before_retrying_the_integration` は、人の replan 要求後、planner より前に統合を再試行しないこと、変更後の check で統合が追加1回だけ走ることを確認する。`crates/task-dispatch/src/dispatcher/tests/integration_ready_race.rs:71` の `ready_wins_before_integration_start_leaves_pending_and_retries` は、統合開始前の hook で Ready への遷移を挟み、Pending のまま再試行できることを確認する。
- 守る不変条件: `CLAUDE.md` の完了判定、`agent-docs/adr/0118-review-target-sync-and-merge-candidate.md` D3–D5 の候補と verdict の束縛、`agent-docs/adr/0120-pre-review-sync-integration-repair.md` D2–D4 の repair 後の再同期と回数上限、`agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md` の統合・replan の順序。再利用の条件は `agent-docs/adr/2026-10-27-review-resync-halt-visible-and-reviewer-reuse.md` に従う。
- 不足する反例: review A が合格し完了キューへ入った後、verdict 保存前に main が進む。併せて段の repair／人の replan が先に確定した状態を用意し、旧 review A と旧 integration の完了を遅れて届ける。期待結果は、旧候補を無条件に MergeQueued にせず、現行の計画を旧完了で Done にせず、repair を重複作成せず、必要な再同期・check を経てから次段へ進むこと。既存試験は各々の分岐と順序を確認するが、完了通知の滞留後の計画変更と候補変更を組み合わせてはいない。
- 決定的な再現方法: `spawn_review` の非同期検査・送信（996–1077）と `on_review_finished` の delivery 保存（358–389）／task 遷移（502–560）の間を試験専用 gate で区切る。送信完了を出来事待ちし、一時 Git repo の main 更新・replan 確定後に処理を進める。repair 起票と段統合の event 順・候補の束縛を照合する。main を繰り返し更新する負荷試験にはしない。

### 権限/予算

- 既存の試験: `crates/task-dispatch/src/dispatcher/tests/tree_gate.rs:1271` の `tree_usd_limit_prices_cache_tokens_by_their_rates` は単価に従う合算と上限到達時の決定要求1件を、同ファイル :1301 の `tree_usd_limit_stops_on_unknown_cost_runs_unless_a_rate_is_configured` は費用不明での停止と単価設定後の解除を確認する。`crates/task-dispatch/tests/policy_close.rs:323` の `unknown_quota_or_cost_is_not_treated_as_zero_or_available` は quota・費用・予算の Unknown を Available／ゼロにせず、Unknown しかなければ Hold とする純粋な policy を確認する。これは実際の worker 非起動の確認ではない。
- quota 予約: `crates/task-dispatch/src/dispatcher/tests/route_select.rs:1424` の `cos_reservation_is_traced_and_never_a_slot_wait` は、予約済み候補の拒否理由と、予約と同時実行枠の混在を単なる slot 待ちにしないことを確認する。入力は構築済みの RouteState であり、`route_state` の収集から `dispatch_one` の起動までの更新は挟まない。
- credential 待ち: `crates/task-dispatch/src/dispatcher/tests/credential_gate.rs:153` の `missing_credential_blocks_only_that_unit_and_leaves_the_task_and_siblings` は対象 WU だけの Blocked を確認する。`crates/task-dispatch/src/dispatcher/tests/credential_e2e.rs:547` の `credential_wait_notifies_through_the_outbox_without_any_run_over_full_ticks` は注入時計で tick を回し、待機中の通知集約と run／chat 起動の抑止を確認する。`crates/task-core/src/store/credential_wait_tests.rs:255` の `cancelled_task_is_never_resumed` は cancel 確定後の検証／resume claim を拒否する。`crates/task-core/src/store/credential_wait_tests.rs:209` の `claim_resume_succeeds_exactly_once_under_concurrency` は8スレッドの claim の成功が1件であることを確認する。claim 済みで実起動前の窓はこれらの試験にはない。
- 守る不変条件: `CLAUDE.md` の認可・SQLite 正本、`agent-docs/adr/2026-11-06-budget-usd-caps.md` D2（上限設定時の費用不明は停止、既に起動した run 分の超過は許容）、`agent-docs/adr/2026-10-11-llm-source-harness-selection.md`（source/account と quota）、`agent-docs/adr/2026-10-15-credential-wait-skip-triage.md`（待機によって worker／CoS を起こさない）。USD 費用不明と quota 推定の unknown は別の量として検査する。
- 不足する反例: (a) credential の resume claim が成功した後、同じ木の別 run が終わって USD 上限到達または費用不明となり、次の dispatch に進む。期待結果は credential の解消だけで予算を迂回せず、worker を起こさず理由を残すこと。cancel を claim 後に確定する派生例でも起動しないこと。(b) 同じ account の worker と reviewer が重なり、一方の終了で quota group が Pending、他方の終了で Closed になる途中に task の cancel／再起動を挟む。期待結果は予約を二重解放せず、残る不明分を利用可能なゼロと扱わず、次の route が過大な空きを見ないこと。単独の gate／policy／claim の assertion ではこの連鎖を固定していない。
- 決定的な再現方法: (a) claim 完了と次 tick の間で出来事待ちし、別 run の完了を fixture で永続化して起動回数ゼロを照合する。(b) fake adapter の2つの終了 gate を順に解放し、`resolve_quota_estimate` の group 更新 177 と他 member の event 追記 314–316、自分の event を保存する worker／review 終了経路の境界を試験専用 hook で固定する。quota の観測値・時計も注入し、再起動後の route と記録を確認する。実 credential・外部通信・CPU 負荷は使わない。
