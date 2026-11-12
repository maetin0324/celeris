---
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: accepted
---
# ADR 2026-11-11: Dispatcher の状態の owner と、判断と副作用の分け方

- 状態: 採用（未実装）。実装は task 01M6TYVJT4S29ZMP6P0D4831JG（健全性 v2 R4）の unit own-cluster・own-maint・own-routing・
  own-hooks・own-quota・own-supervisor・own-orchestration・settle・admit・review-decide。反例試験は unit cex-lease・cex-review が先に固定する。
- 根拠の実測: [change-map.md](../progress/2026-11-11-health-v2-r4/change-map.md) と
  [change-map.json](../progress/2026-11-11-health-v2-r4/change-map.json)（field 116 件・co_change・equivalence 6 件）、
  owner 候補の規則は [owners.json](../progress/2026-11-11-health-v2-r4/owners.json)。
- 関係: [ADR-0082](0082-dispatcher-module-split.md)（dispatcher の module 分割。この ADR は状態の owner を足す）、
  [ADR 2026-10-03 lease を持たない run](2026-10-03-ownerless-running-runs.md)、
  [ADR 2026-11-06 予算の USD 上限](2026-11-06-budget-usd-caps.md)、
  [ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)、
  [ADR-0125](0125-deterministic-time-tests.md)（時間依存の試験の決定化）。

## 背景

`crates/task-dispatch/src/dispatcher.rs` の `pub struct Dispatcher` は field を 116 件持ち（うち `#[cfg(test)]` 10 件）、
`crates/task-dispatch/src/dispatcher/` の 200 余りの module が `&mut Dispatcher` を受けてどの field でも書ける。
書き手の module の owner 候補が 2 種類以上ある field は 12 件（owner_conflict）。大きい 7 関数
（finish_worker_result_with 1294 行・dispatch_one 1111 行・spawn_review 748 行・on_review_finished 594 行・
reclaim_expired_leases 220 行・route_state 222 行・resolve_quota_estimate 214 行）は判断の途中に書き込みを挟む。
field を下位の struct に包んで Dispatcher から互いに参照させるだけでは、どの module でも書ける状態は変わらない。

## D0 write-set の回答

人の決定 write-set は a（推奨どおり）。R4 の範囲に次を加える。

- ADR で決めた owner の module（`crates/task-dispatch/src/` 直下。下の D1・D8 の file）
- `docs/architecture-map.md` の owner の行の追加と、移した行の path の直し（G4 も地図を変えるので、それ以外の行は変えない）
- [ADR-0082](0082-dispatcher-module-split.md) の付記

加えないもの: `scripts/dev/repo-health/` と `agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json`
（profile の `policy_paths`）。この 2 つに要る変更は D8 の patch に書き、operator が policy だけの branch で main に入れる。
`crates/task-dispatch/src/review.rs` を変えるときは該当する関数の周りだけにする。`slices.json` は書き換えず、広げた範囲は進捗に書く。

## D1 境界と field の owner

owner は 8 つ。`Dispatcher` は組み立て（config と port の注入）と tick の順序だけを持つ。

| owner | module（`crates/task-dispatch/src/`） | 持つ状態 | field 数 |
|---|---|---|---:|
| `ExecutionCoordination` | `execution_coordination.rs` | task・WU の待ち、check・統合の process、review の同期、write set の予約 | 31 |
| `RunSupervisor` | `run_supervisor.rs` | 実行中の run の表（run の集合の正本）、review run、infra backoff、lease の照合 | 7 |
| `RoutingRuntime` | `routing_runtime.rs`（`routing_runtime/quota.rs`・`routing_runtime/quota_estimate.rs`） | account・source の cooldown、quota の窓、分類待ち、eval run | 14 |
| `ClusterRuntime` | `cluster_runtime.rs` | クラスタの接続・tunnel・job の poll | 16 |
| `HostMaintenance` | `host_maintenance.rs` | disk・scratch・build cache・repo target の掃除 | 9 |
| `ProjectOrchestration` | `project_orchestration.rs` | 案件の停滞の見張り、CoS chat の起動の待ち | 2 |
| `Dispatcher`（組み立て） | `dispatcher.rs` | config・store・port・registry | 22 |
| `TestHooks`・`ProbePorts` | `test_hooks.rs` | 試験の時計・遅延・停止点・ablation（`TestHooks`、`#[cfg(test)]`）と本番の既定を持つ probe（`ProbePorts`） | 15 |

### 境界の根拠（change-map.json の co_change）

数は `change-map.json` の `co_change`（2026-09-15 以降の merge でない commit 724 件、3 回以上の組 495 組）から取った。
owner ごとの合計は、組の両側を `owners.json` の `modules` で owner に写し、`dispatcher.rs` と `tests/` を含む組を除いて足した。

| owner の組 | 共変更の commit 数（組の合計） |
|---|---:|
| execution-coordination ↔ execution-coordination | 115 |
| execution-coordination ↔ run-supervisor | 55 |
| execution-coordination ↔ routing | 26 |
| run-supervisor ↔ run-supervisor | 22 |
| project-orchestration ↔ run-supervisor | 15 |
| execution-coordination ↔ host-maintenance | 15 |
| routing ↔ run-supervisor | 13 |
| host-maintenance ↔ project-orchestration | 12 |
| project-orchestration ↔ project-orchestration | 8 |
| execution-coordination ↔ project-orchestration | 6 |
| host-maintenance ↔ routing | 3 |
| host-maintenance ↔ host-maintenance | 3 |

- 最も多い跨ぎは execution-coordination ↔ run-supervisor の 55 回。owner_conflict 12 件のうち 7 件（`running`・`reviewing`・
  `infra_backoff`・`awaiting_human`・`checking`・`integrating`・`pending_subjects`）は `dispatcher/leases`（run-supervisor）が書き手に入る。
  そこで run の集合は `RunSupervisor` だけが持ち、`ExecutionCoordination` は run の集合を持たない（下の規則）。
- lease の側の組: `dispatcher.rs` + `leases.rs` 12 回、`leases.rs` + `ownerless_runs.rs` 6 回、`leases.rs` + `planner_flow.rs` 4 回、
  `leases.rs` + `worker_finish.rs` 3 回。lease の回収と照合（`ownerless_runs.rs`）を同じ owner に置く。
- routing の側の組: `dispatcher.rs` + `route_select.rs` 10 回、`dispatch_run.rs` + `route_select.rs` 7 回。routing は dispatch から
  入力を受けて提案を返す側に置く。
- cluster の file は `dispatcher.rs` との組（`cluster.rs` 3 回・`cluster_job_wait.rs` 3 回）にしか出ない。他の owner と共変更しないので、
  最初に分ける。
- 共変更は file 単位の相関で、所有の証明ではない（change-map の `method.limits`）。field の owner は書き手と読み手の module
  （`fields[].writers`・`readers`）で決め、共変更は境界の切り方の確かめに使う。

### run の集合と lease の照合

- `running`（`HashMap<RunKey, RunEntry>`）は run の集合の正本で、`RunSupervisor` に置く。dispatch_run の登録・leases の削除・
  worker_finish の削除はどれも `RunSupervisor` の method を通す。
- lease の照合の状態 `ownerless_reconciled_at`・`lost_review_watch`・`terminal_records_reconciled_at` も `RunSupervisor` に置く。
- `ExecutionCoordination` は run の集合を持たない。`checking`（WU の check の process）・`integrating`（統合の process）・
  `awaiting_human`・`pending_subjects` は task・WU を鍵にした待ちで、run の表ではない。lease の回収でこれらを消すときは、
  `RunSupervisor` の回収の提案が返す鍵（`RunKey`・`TaskId`）を `ExecutionCoordination` の method に入力として渡す。

### field → owner の表

change-map.json の `fields` 116 件すべて。「change-map の候補」は `fields[].owner`。owner はすべて候補どおりで、割り当ては変えていない。
候補 test-hooks の 15 件は、`#[cfg(test)]` の 10 件を `TestHooks`（うち phase_effect_ab の 4 件はその中の評価用 fixture `PhaseAblation`）に、
本番の既定（実時計・実 ssh・実 knowledge の probe）を持つ 5 件を `ProbePorts` に置く。`ProbePorts` は owner が引数で受ける。

| field | change-map の候補 | owner | cfg(test) | 書き手の扱い |
|---|---|---|:-:|---|
| `accepting_new_work` | execution-coordination | `ExecutionCoordination` |  |  |
| `awaiting_children` | execution-coordination | `ExecutionCoordination` |  |  |
| `awaiting_human` | execution-coordination | `ExecutionCoordination` |  | leases の削除は `RunSupervisor` が返す鍵を入力に受けて行う |
| `browser_ledger` | execution-coordination | `ExecutionCoordination` |  |  |
| `browser_prereq_rescan` | execution-coordination | `ExecutionCoordination` |  |  |
| `checking` | execution-coordination | `ExecutionCoordination` |  | 同上（WU の check の process。run の集合ではない） |
| `cluster_waiting` | execution-coordination | `ExecutionCoordination` |  |  |
| `credential_busy` | execution-coordination | `ExecutionCoordination` |  |  |
| `credential_held` | execution-coordination | `ExecutionCoordination` |  |  |
| `dispatch_waits` | execution-coordination | `ExecutionCoordination` |  |  |
| `integrated_deps` | execution-coordination | `ExecutionCoordination` |  |  |
| `integrating` | execution-coordination | `ExecutionCoordination` |  | 同上（統合の process。run の集合ではない） |
| `last_tick_gap_ms` | execution-coordination | `ExecutionCoordination` |  |  |
| `last_tick_started` | execution-coordination | `ExecutionCoordination` |  |  |
| `liveness_checked_at` | execution-coordination | `ExecutionCoordination` |  |  |
| `max_concurrency` | execution-coordination | `ExecutionCoordination` |  |  |
| `pending_subjects` | execution-coordination | `ExecutionCoordination` |  | 同上。worker_finish の書き込みも `ExecutionCoordination` の method を通す |
| `review_resync_halted` | execution-coordination | `ExecutionCoordination` |  |  |
| `review_sync_queue` | execution-coordination | `ExecutionCoordination` |  |  |
| `review_sync_seq` | execution-coordination | `ExecutionCoordination` |  |  |
| `run_write_bases` | execution-coordination | `ExecutionCoordination` |  |  |
| `rx` | execution-coordination | `ExecutionCoordination` |  |  |
| `stall_watch` | execution-coordination | `ExecutionCoordination` |  |  |
| `stalled_tasks` | execution-coordination | `ExecutionCoordination` |  |  |
| `task_workspaces` | execution-coordination | `ExecutionCoordination` |  | housekeeping の削除は `ExecutionCoordination` の method を通す（鍵は `HostMaintenance` が返す） |
| `ticks` | execution-coordination | `ExecutionCoordination` |  |  |
| `tx` | execution-coordination | `ExecutionCoordination` |  |  |
| `write_reservations` | execution-coordination | `ExecutionCoordination` |  |  |
| `write_set_hold_seq` | execution-coordination | `ExecutionCoordination` |  |  |
| `write_set_waits` | execution-coordination | `ExecutionCoordination` |  |  |
| `wu_prepare_failures` | execution-coordination | `ExecutionCoordination` |  |  |
| `infra_backoff` | run-supervisor | `RunSupervisor` |  | dispatch_run の読み取り・削除と worker_finish の書き込みは `RunSupervisor` の method を通す |
| `just_aborted` | run-supervisor | `RunSupervisor` |  |  |
| `lost_review_watch` | run-supervisor | `RunSupervisor` |  | lease の照合の状態 |
| `ownerless_reconciled_at` | run-supervisor | `RunSupervisor` |  | lease の照合の状態 |
| `reviewing` | run-supervisor | `RunSupervisor` |  | review run の表。review_spawn・review_verdict の書き込みは `RunSupervisor` の method を通す |
| `running` | run-supervisor | `RunSupervisor` |  | run の集合の正本。dispatch_run の登録は `RunSupervisor` の method を通す |
| `terminal_records_reconciled_at` | run-supervisor | `RunSupervisor` |  | lease の照合の状態 |
| `account_books` | routing | `RoutingRuntime` |  |  |
| `account_limits` | routing | `RoutingRuntime` |  |  |
| `accounts_scan_cache` | routing | `RoutingRuntime` |  | snapshot の書き込みは `RoutingRuntime` の method を通す |
| `classification_board` | routing | `RoutingRuntime` |  |  |
| `classification_deferred_since` | routing | `RoutingRuntime` |  |  |
| `classification_wait` | routing | `RoutingRuntime` |  |  |
| `eval_runs` | routing | `RoutingRuntime` |  |  |
| `knowledge_probe_cache` | routing | `RoutingRuntime` |  |  |
| `login_pending_accounts` | routing | `RoutingRuntime` |  |  |
| `quota_activity` | routing | `RoutingRuntime` |  |  |
| `quota_calibration` | routing | `RoutingRuntime` |  |  |
| `source_cooldowns` | routing | `RoutingRuntime` |  |  |
| `unroutable` | routing | `RoutingRuntime` |  |  |
| `warned_unroutable` | routing | `RoutingRuntime` |  |  |
| `cluster_conn` | cluster-runtime | `ClusterRuntime` |  |  |
| `cluster_connected` | cluster-runtime | `ClusterRuntime` |  |  |
| `cluster_cooldown` | cluster-runtime | `ClusterRuntime` |  |  |
| `cluster_disconnect_info` | cluster-runtime | `ClusterRuntime` |  |  |
| `cluster_job_polls` | cluster-runtime | `ClusterRuntime` |  |  |
| `cluster_login_needed` | cluster-runtime | `ClusterRuntime` |  |  |
| `connect_pending_clusters` | cluster-runtime | `ClusterRuntime` |  |  |
| `container_probe` | cluster-runtime | `ClusterRuntime` |  |  |
| `last_cluster_command_probe` | cluster-runtime | `ClusterRuntime` |  |  |
| `last_cluster_liveness` | cluster-runtime | `ClusterRuntime` |  |  |
| `last_cluster_tunnel_refresh` | cluster-runtime | `ClusterRuntime` |  |  |
| `tunnel_events` | cluster-runtime | `ClusterRuntime` |  |  |
| `tunnel_probe_state` | cluster-runtime | `ClusterRuntime` |  |  |
| `tunnel_prober_stop` | cluster-runtime | `ClusterRuntime` |  |  |
| `tunnel_state` | cluster-runtime | `ClusterRuntime` |  |  |
| `warned_cluster_tool` | cluster-runtime | `ClusterRuntime` |  |  |
| `disk_low` | host-maintenance | `HostMaintenance` |  |  |
| `disk_ready` | host-maintenance | `HostMaintenance` |  |  |
| `disk_watch` | host-maintenance | `HostMaintenance` |  |  |
| `health_inventory` | host-maintenance | `HostMaintenance` |  |  |
| `removing_build_caches` | host-maintenance | `HostMaintenance` |  |  |
| `removing_repo_targets` | host-maintenance | `HostMaintenance` |  |  |
| `repo_target_gc_at` | host-maintenance | `HostMaintenance` |  |  |
| `scratch` | host-maintenance | `HostMaintenance` |  | dispatcher.rs の書き込みは `HostMaintenance` の method を通す |
| `script_compare` | host-maintenance | `HostMaintenance` |  |  |
| `cos_chat_launch` | project-orchestration | `ProjectOrchestration` |  | cos_chat/launch の書き込みは `ProjectOrchestration` の method を通す |
| `stagnation` | project-orchestration | `ProjectOrchestration` |  | tree_units の書き込みは `ProjectOrchestration` の method を通す |
| `cluster_command_probe` | composition-root | `Dispatcher`（組み立て） |  |  |
| `cluster_connector` | composition-root | `Dispatcher`（組み立て） |  |  |
| `cluster_job_poller` | composition-root | `Dispatcher`（組み立て） |  |  |
| `cluster_master_watcher` | composition-root | `Dispatcher`（組み立て） |  |  |
| `config` | composition-root | `Dispatcher`（組み立て） |  |  |
| `credential_gates` | composition-root | `Dispatcher`（組み立て） |  |  |
| `credential_resumer` | composition-root | `Dispatcher`（組み立て） |  |  |
| `dispatch_routing` | composition-root | `Dispatcher`（組み立て） |  |  |
| `eligible` | composition-root | `Dispatcher`（組み立て） |  |  |
| `harness_adapters` | composition-root | `Dispatcher`（組み立て） |  |  |
| `llm_route_registry` | composition-root | `Dispatcher`（組み立て） |  |  |
| `orphan_takeover` | composition-root | `Dispatcher`（組み立て） |  |  |
| `publisher` | composition-root | `Dispatcher`（組み立て） |  |  |
| `routing_context_registry` | composition-root | `Dispatcher`（組み立て） |  |  |
| `routing_mode` | composition-root | `Dispatcher`（組み立て） |  |  |
| `store` | composition-root | `Dispatcher`（組み立て） |  |  |
| `target_sweep` | composition-root | `Dispatcher`（組み立て） |  |  |
| `target_sweep_scope` | composition-root | `Dispatcher`（組み立て） |  |  |
| `tunnel_forward_ensurer` | composition-root | `Dispatcher`（組み立て） |  |  |
| `tunnel_listener_probe` | composition-root | `Dispatcher`（組み立て） |  |  |
| `tunnel_probe` | composition-root | `Dispatcher`（組み立て） |  |  |
| `variant_observation` | composition-root | `Dispatcher`（組み立て） |  |  |
| `before_integration_start` | test-hooks | `TestHooks` | † | 時計・遅延・停止点 |
| `cluster_liveness_probe` | test-hooks | `ProbePorts` |  | 本番の既定を持つ注入 port（`#[cfg(test)]` ではない） |
| `cluster_ssh_command_override` | test-hooks | `ProbePorts` |  | 本番の既定を持つ注入 port（`#[cfg(test)]` ではない） |
| `eval_version_probe` | test-hooks | `ProbePorts` |  | 本番の既定を持つ注入 port（`#[cfg(test)]` ではない） |
| `knowledge_probe` | test-hooks | `ProbePorts` |  | 本番の既定を持つ注入 port（`#[cfg(test)]` ではない） |
| `now_unix_fn` | test-hooks | `ProbePorts` |  | 本番の既定を持つ注入 port（`#[cfg(test)]` ではない） |
| `test_cos_reserved_sources` | test-hooks | `TestHooks` | † | 時計・遅延・停止点 |
| `test_disable_stale_priority` | test-hooks | `TestHooks` の `PhaseAblation`（評価用 fixture） | † | phase_effect_ab::stale_priority。owner には引数で渡す |
| `test_disable_write_set_gate` | test-hooks | `TestHooks` の `PhaseAblation`（評価用 fixture） | † | phase_effect_ab::write_set。owner には引数で渡す |
| `test_halt_after_wu_transition` | test-hooks | `TestHooks` | † | 時計・遅延・停止点 |
| `test_now` | test-hooks | `TestHooks` | † | 時計・遅延・停止点 |
| `test_policy_clock` | test-hooks | `TestHooks` | † | 時計・遅延・停止点 |
| `test_skip_pre_review_sync` | test-hooks | `TestHooks` の `PhaseAblation`（評価用 fixture） | † | phase_effect_ab::review_sync。owner には引数で渡す |
| `test_sync_conflict_as_review_fail` | test-hooks | `TestHooks` の `PhaseAblation`（評価用 fixture） | † | phase_effect_ab::review_sync。owner には引数で渡す |
| `test_worker_start_delay` | test-hooks | `TestHooks` | † | 時計・遅延・停止点 |

（116 件。† は `#[cfg(test)]`。）

## D2 owner の規則

1. owner の struct の field は private にする。他の module は owner の method を通して読み書きする。
2. owner の method は `Dispatcher` を受け取らない（`&Dispatcher`・`&mut Dispatcher` を引数にしない）。要る値は引数で受ける。
3. owner の module は `crates/task-dispatch/src/` 直下に置き、`dispatcher` module を `use` しない。
4. 移した処理の旧い本体は同じ変更で消す。`Dispatcher` の側には、owner の method を呼ぶ薄い入口だけを残す。
5. 200 行を超える関数を丸ごと owner に移さない。先に D3 の判断と副作用に分け、判断を純粋関数、副作用を owner の method にする。
6. 試験を弱めない。移した処理の試験は同じ assertion のまま owner を通して走らせる。試験を消すなら、代わりの試験が同じ不具合を検出できることを進捗に書く。
7. owner の struct を足した unit は、`structure_guard_<owner>` の名の試験を足す。試験は source を読み、field が private であること、
   method が `Dispatcher` を受けないこと、struct に `#[cfg(test)]` の field が無いことを確かめる。
8. owner を足した unit は、`dispatcher.rs` の module map（ADR-0082）から移した module の旧い行を消し、ADR-0082 に付記を足し、
   `docs/architecture-map.md` に owner の行を足して移した行の path を直す。record まで持ち越さない。

不採用: field を owner 別の struct に包み、`Dispatcher` から `self.cluster.conn` のように互いに参照させるだけの案（梱包だけ）。
どの module でも `&mut Dispatcher` から全 owner の状態を書ける形が残り、owner_conflict 12 件が消えない。

## D3 判断と副作用の分け方

対象は change-map の大きい 7 関数。判断は「入力の struct → 提案の struct」の純粋関数にし、副作用は提案を受けた owner の method が行う。
transaction の範囲は今と同じにする。今 1 つの transaction で書くものを分けず、今別々に commit するものを 1 つにまとめない。

| 関数（今の位置） | 判断（純粋関数） | 副作用の owner |
|---|---|---|
| finish_worker_result_with（`dispatcher/worker_finish.rs`） | `run_settlement::settle(RunFinishInput) -> RunFinishProposal`（今の 349-1281 と 1425-1489）。段の進み方は WU の書き込みの後にもう 1 回の純粋関数（`settle_phase` の入力を作る）で決める | run の終わりと backoff は `RunSupervisor`、WU の遷移と commit は `ExecutionCoordination`、quota と cooldown は `RoutingRuntime` |
| dispatch_one（`dispatcher/dispatch_run.rs`） | `dispatch_admission::admit(AdmissionInput) -> AdmissionProposal`。write set・credential・予算・quota・クラスタ待ち・auto_leaf の既存の判定の結果を組み合わせるだけで、判定を作り直さない | lease の取得と worker の spawn は `RunSupervisor`、担当と gate の書き込みは `ExecutionCoordination` |
| spawn_review（`dispatcher/review_spawn.rs`） | `review_decision::plan_review`。`crates/task-dispatch/src/review.rs` の既存の関数を使い、同じ判定を写さない | review run の登録は `RunSupervisor`、同期と遷移は `ExecutionCoordination` |
| on_review_finished（`dispatcher/review_verdict.rs`） | `review_decision::judge_review`（決定的 check が不合格なら成功にしない） | delivery の保存と task の遷移は `ExecutionCoordination` |
| reclaim_expired_leases（`dispatcher/leases.rs`） | `run_supervisor` の回収の判定（期限切れの lease・手元の run の鍵・時刻 → 回収する鍵と trigger） | lease の解放と run の終了記録は `RunSupervisor`、待ちの削除は `ExecutionCoordination`（鍵を入力に受ける） |
| route_state（`dispatcher/route_select.rs`） | `routing_runtime` の route の状態の判定 | cooldown・limits の更新は `RoutingRuntime` |
| resolve_quota_estimate（`dispatcher/quota_book.rs`） | `routing_runtime::quota_estimate` の見積り | quota の窓と group の閉じ方は `routing_runtime::quota`（group の他 member の event の追記は今と同じく別の書き込み） |

## D4 等価性から共有するものと、しないもの

change-map の equivalence 6 件は verdict がすべて different で、経路全体は共有しない。共有するのは次の純粋な判定だけ。

| id | 共有する | 共有しない |
|---|---|---|
| continuation-resume | 保存 session の互換性・拒否・context 上限から Resume か Fresh を返す判定 | session の解決（既に共通） |
| retry-backoff | infra 障害の連続回数と上限から再試行するかの判定と `infra_backoff_delay(n)`（worker 完了と lease 回収で 1 つ） | 回収側の lease の検証 |
| review-result | 決定的 check が不合格なら成功にしない判定と、check 結果から失敗一覧を作る処理 | check の実行（既に共通） |
| run-close | run id・usage・metrics・終端から終了記録を組み立てる処理 | lease の検証・状態遷移・transaction の境界（一律の close(run) は作らない） |
| wu-failure | 失敗の署名・同じ失敗の回数・replan の残り・禁止の理由から Replan か Ask を返す上限判定（似た判定 3 か所を 1 つに） | replan の実行 |
| lease-renew | 無し。`renew_lease` の store 操作は既に共通 | Ok(false) と Err を同じに扱う wrapper（回収の安全条件を壊す） |

分けたままにするもの: security 境界の独立の検証（credential gate・db guard・browser の policy broker は、それぞれが自分で検証する）、
eval の試行（`eval_trials`）と CoS chat（`cos_chat`）の run の起動と終わり（試行の隔離と thread の session という別の契約を持つ）。

## D5 接続の契約

owner の間は既存の `Trigger`・`Event` と `execution_scheduler::decide` だけでつなぐ。全 domain に共通の event 基盤・command bus は作らない。
判断の関数は提案（次の `Trigger`、追記する `Event` の列、owner への指示）を返し、`Dispatcher` の薄い入口が提案を owner の method に順に渡す。
`dispatch_admission` は既存の判定の結果を組み合わせるだけで、新しい判定の型を作らない。

## D6 ablation と試験用フックの隔離

- phase_effect_ab の 4 flag（`test_skip_pre_review_sync`・`test_sync_conflict_as_review_fail`・`test_disable_write_set_gate`・
  `test_disable_stale_priority`）は評価用 fixture `PhaseAblation` に移し、`TestHooks` の中に置く。
- 時計・遅延（`test_now`・`test_policy_clock`・`test_worker_start_delay`）、停止点（`before_integration_start`・`test_halt_after_wu_transition`）、
  `test_cos_reserved_sources` は `TestHooks` に置く。
- `Dispatcher` の `#[cfg(test)]` の field は `TestHooks` の 1 個だけにする（今は 10 個）。
- `write_set_gate.rs` の `test_disable_write_set_gate` と `stale_priority.rs` の `test_disable_stale_priority` は、`Dispatcher` の薄い入口が
  `TestHooks` から読んで引数で owner の関数に渡す。owner の struct に `#[cfg(test)]` の flag を足さない。本番の build では入口が
  「ablation なし」の値を渡す。
- 新しい試験用の flag を `Dispatcher` に足さない。足すなら `TestHooks` か `PhaseAblation` に足す。

## D7 反例試験・移行順・戻し方

### 反例試験（分割の前に今の実装に対して固定する）

change-map の「反例試験の不足」の 5 節に対応する。各試験の doc comment に、守る不変条件と既存の試験で検出できない理由を書く。

| 範囲 | 反例 | unit |
|---|---|---|
| lease | 回収側が期限切れ snapshot を取った直後に別の接続が renew する。run A を終えて run B が取った後に古い回収側が再開する。現在の保持者を回収しない | cex-lease |
| restart | 新 active が reviewer A を ownerless として閉じ reviewer B を始めた後、旧 draining instance から A の合格が届く。A の run 行は Running に戻らない | cex-lease |
| 同時完了 | run A の Done の処理待ちと lease の期限が同じ tick に来て、人のコメントか cancel が先に確定する。WU 遷移の直後と task 遷移の直前に人の操作を挟む | cex-lease |
| review/integration | review A の合格の後、verdict の保存の前に main が進む。段の repair か人の replan が先に確定した後に、旧 review と旧統合の完了が遅れて届く | cex-review |
| 権限/予算 | credential の resume の claim の後、同じ木の別 run の終わりで USD 上限か費用不明になる。credential の解消だけで予算を迂回しない。quota group の 2 つの終わりが続けて届く | cex-review |

再現は注入時計・出来事待ち・試験専用の gate で決定的に行う（ADR-0125）。CPU に負荷をかけない。反例が今の実装で落ちたら、その unit では本体を直さず、
進捗に書いて repair か replan に回す。

### 移行順

1. cex-lease・cex-review: 反例試験を今の実装に対して固定する。
2. own-cluster・own-maint・own-routing・own-hooks: 他の owner と共変更の少ない状態から分ける（cluster は `dispatcher.rs` との組だけ）。
3. sync-policy: operator が D8 の patch を main に入れた後、その main を取り込む。
4. own-quota・own-supervisor・own-orchestration: lease と予算の状態を移す（D8 の高 risk の class が新しい file に当たってから）。
5. settle・admit・review-decide: D3 の判断を純粋関数に分ける。
6. record: 実測を取り直して進捗と ADR の状態を締める。

### 戻し方

unit ごとの commit を `git revert` で戻す。新旧を切り替える flag・互換の分岐は置かない（ADR 2026-10-11 破壊的変更）。
policy の patch を戻すときは、operator が policy だけの branch で revert する。

## D8 lease・budget の判断の移動先と policy の path

移動先（後の unit の check がこの名を固定する）:

- lease: `crates/task-dispatch/src/run_supervisor.rs`・`crates/task-dispatch/src/run_settlement.rs`
- budget: `crates/task-dispatch/src/routing_runtime/quota.rs`・`crates/task-dispatch/src/routing_runtime/quota_estimate.rs`・
  `crates/task-dispatch/src/run_settlement.rs`・`crates/task-dispatch/src/dispatch_admission.rs`

profile の高 risk の class と capability-map.json の owner path に、この file を足す必要がある。2 つとも profile の `policy_paths` で、
候補 branch で変えると rule policy.changed が落ちる。足す内容は git apply で当たる patch
[policy-paths.diff](../progress/2026-11-11-health-v2-r4/policy-paths.diff) と説明
[policy-paths.md](../progress/2026-11-11-health-v2-r4/policy-paths.md) に書いた。足すだけで、今の path は消さない。

旧い file を消すときの rule arch-map（`scripts/dev/repo-health/rules/arch_map.py`）の当たり方は policy-paths.md に試算を書いた。要点:

- 地図の行が path を名指しする file（`dispatch_run.rs`・`worker_finish.rs`・`review_spawn.rs`・`review_verdict.rs`・`token_budget.rs`・
  `housekeeping.rs`・`ownerless_runs.rs`）は、地図を直さずに消すと arch-map.wrong_path に当たる。消す unit が同じ変更で地図の行を直す。
- capability-map.json の owner path（`dispatcher/leases.rs`・`dispatcher/ownerless_runs.rs`・`dispatcher/quota_book.rs`・
  `dispatcher/token_budget.rs`・`dispatcher/dispatch_run.rs`・`dispatcher/route_select.rs` ほか）は、地図の 59 行目の
  `crates/task-dispatch/src/dispatcher/` が残る間は消しても当たらない。この dir を地図から外すと arch-map.wrong_owner と missing_invariant に当たる。
- そこで capability-map.json の owner path と profile の高 risk の正規表現が名指しする file は、policy から旧い path を撤去するまで
  `Dispatcher` の薄い入口（owner の method を呼ぶだけ）として残す。旧い本体は D2 の 4 のとおり消す。

## 付記 2026-11-12: own-cluster の実装（ClusterRuntime）

- `ClusterRuntime`（`crates/task-dispatch/src/cluster_runtime.rs`）は D1 の表の cluster-runtime の 16 field に加えて、表で
  `Dispatcher`（組み立て）とした 7 件（`cluster_command_probe`・`cluster_connector`・`cluster_job_poller`・`cluster_master_watcher`・
  `tunnel_forward_ensurer`・`tunnel_listener_probe`・`tunnel_probe`）と `ProbePorts` とした 2 件（`cluster_liveness_probe`・
  `cluster_ssh_command_override`）も持つ（計 25 field）。理由: unit own-cluster の受け入れ条件が「cluster の field が `Dispatcher`
  から消え `ClusterRuntime` 1 個になる」で、この 9 件の port を読むのは `ClusterRuntime` の method だけだから。port は
  `Dispatcher` の setter（celeris の配線は変えない）が `ClusterRuntime` の setter に渡す。
- `cluster_waiting` は表どおり `ExecutionCoordination` の範囲で、`Dispatcher` に残した（その owner を作る unit が移す）。
- クラスタ job の durable wait のうち、wait の行・task と WU の遷移（`finish_cluster_job_wait`・`time_out_cluster_job_wait`・再開）は
  task・WU の待ちなので `dispatcher/cluster_job_wait.rs` に残した。`ClusterRuntime` は poll のスレッドと結果の受け口
  （`take_job_poll`・`start_job_poll`）と poll の要求の組み立て（`job_poll_request`）を持つ。
- owner の外の値（store・`[[clusters]]`・cooldown・`--mode verify`・単調時計・tick の間隔）は `ClusterInputs` で受ける。単調時計は
  `Dispatcher::monotonic_clock` が返す closure で、`monotonic_now` と同じ時計（試験の差し替えを含む）を使う。
- 試験 `structure_guard_cluster_runtime` が D2 の 1〜3・7 を source から確かめる。

## 付記 2026-11-12: own-maint の実装（HostMaintenance）

- `HostMaintenance`（`crates/task-dispatch/src/host_maintenance.rs`）は 10 field を持つ: D1 の表の host-maintenance の
  `disk_low`・`disk_ready`・`disk_watch`・`removing_build_caches`・`removing_repo_targets`・`repo_target_gc_at`・`scratch` と、表で
  `Dispatcher`（組み立て）とした `target_sweep`・`target_sweep_scope`、表で `ExecutionCoordination` とした `task_workspaces`。
  理由: unit own-maint の objective がこの 3 件を移す field に挙げ、受け入れ条件が「挙げた field が `Dispatcher` から消え
  `HostMaintenance` 1 個になる」だから。`target_sweep`・`target_sweep_scope` を読むのは保守 executor と repo target の掃除だけで、
  `Dispatcher` の setter（celeris の配線は変えない）が `HostMaintenance` の setter に渡す。`task_workspaces` を書くのは run の起動
  （`remember_task_workspaces`）と中止の片付けだけ。ExecutionCoordination を作る unit が、表どおりに移すかをそこで決める。
- `disk_ready` は表どおり `HostMaintenance` に置いた。facade の `tick` は `refresh_disk_ready` を drain より前に呼び、他の module は
  `disk_ready()` で読む。
- 表で host-maintenance の `health_inventory`（cron の棚卸しの Busy の印）と `script_compare`（TS の比較の状態）は、unit own-maint の
  objective に挙がっていないので `Dispatcher` に残した。
- 実行中の run と判定中の review の作業場所を消さない条件は、`Dispatcher::tasks_in_flight`（`running` の鍵の task と `reviewing` の鍵）を
  入口が組んで `HostMaintenance::cleanup_cancelled_worktrees` に渡す。片付けの判断は純粋関数 `plan_workspace_cleanup`
  （走っている → 触らない、中止 → 消す、終端 → 残して未コミットを 1 行記録、task が無い → 記録だけ落とす）。
- owner の外の値（store・`[scratch]`・`shared_build_cache`・build cache と releases の dir・`min_free_disk_mb`・workspace の root・
  prune の秒数・`--mode verify`・注入時計の値）は `MaintenanceInputs` で受ける。保守 executor の失敗は `MaintenanceError`
  （store と scratch pool の lock）で返し、入口が今と同じ `DispatchError` の variant に写す。
- 試験 `structure_guard_host_maintenance` が D2 の 1〜3・7 を source から確かめる。

## 付記 2026-11-12: own-routing の実装（RoutingRuntime）

- `RoutingRuntime`（`crates/task-dispatch/src/routing_runtime.rs`）は 15 field を持つ: D1 の表の routing の `account_books`・
  `account_limits`・`accounts_scan_cache`・`classification_board`・`classification_deferred_since`・`classification_wait`・
  `login_pending_accounts`・`source_cooldowns`・`unroutable`・`warned_unroutable` と、表で `Dispatcher`（組み立て）とした
  `dispatch_routing`・`routing_context_registry`・`routing_mode`、表に無い `llm_route_registry`・`harness_adapters`。
  理由: unit own-routing の objective がこの 15 件を移す field に挙げ、受け入れ条件が「挙げた field が `Dispatcher` から消え
  `RoutingRuntime` 1 個になる」だから。`eval_runs`・`knowledge_probe_cache`・`quota_activity`・`quota_calibration` は表では routing だが、
  objective が own-quota などに任せているので `Dispatcher` に残した。
- `route_state`（約 220 行）は D3 に従い、判断を `routing_runtime/route_state.rs` の純粋関数 `build_route_state(RouteStateInputs) -> RouteState`
  に分けた。走っている run・review の `(source, account)` は `Dispatcher::route_holds` が集めて渡す（run の表の owner は別 unit が作る）。
  `RoutingRuntime::route_state` が走査 cache を満たし、帳簿の lock を借りて関数を呼ぶ。`RouteState`・`account_adapter_for_source_kind`・
  `source_account_pool`・`DispatchRoutingSettings`・`ClassificationBoard` は `dispatcher` の外の `routing_runtime` に移した（owner が
  `dispatcher` module を使わないため）。公開 API の path は `task_dispatch::` 直下のままで変わらない。
- `ClassificationBoard` の `BoardItem` が持つ `CredentialGate` は `credential_gate.rs`（高 risk の credential の path）の型で、この unit は
  その file を変えないので、crate 直下の re-export（`crate::CredentialGate`）で参照する。型を移すかは credential の owner を決める unit に任せる。
- 試験の test-only の `test_cos_reserved_sources` は `Dispatcher::route_state` の `#[cfg(test)]` の後処理に残した（own-hooks が移す）。
  試験の field 直参照（`source_cooldowns`・`account_limits`）は `#[cfg(test)]` の method（`insert_source_cooldown_for_test`・
  `set_stored_limit_for_test`）に置き換えた。
- 試験 `structure_guard_routing_runtime` が D2 の 1〜3・7 と、純粋 module が `Dispatcher`・store・時計・lock の語を持たないことを source から確かめる。
