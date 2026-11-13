# ADR-0082: dispatcher.rs を責務別の子モジュールへ分け、Dispatcher を facade にする

---
tasks: [01M3QEQPP31ZB29RH6YGFGTAPF]
---

- Date: 2026-09-29
- Status: Accepted
- 適用先: `crates/task-dispatch/src/dispatcher.rs` と `crates/task-dispatch/src/dispatcher/`

## 文脈

`dispatcher.rs` は production だけで約 17,000 行あり、ほぼ全体が 1 つの `impl Dispatcher` である。
エージェントが局所変更をするとき、関係の無い塊まで読み込むことになり、並行する作業の衝突も起きやすい。
test は既に `dispatcher/tests/` に外出し済みである。

## 決定

### D1. facade と子モジュール

- `dispatcher.rs` は facade とする。残すもの: `Dispatcher` struct と private な補助型（`RunEntry`、`ReviewEntry`、`Completion` など）、`new` と公開の setter/getter、`now_utc` / `monotonic_now`、`tick`、`drain_completions`、`dispatch_ready`、`is_idle`、private な `mod` 宣言、明示した `pub use`。
- 子モジュールは `dispatcher/<name>.rs` に置き、それぞれ `impl Dispatcher` ブロックか free fn を持つ。

### D2. 層（依存の向き）

L0（facade の型と free helper）← L1 ← L2 ← L3 ← L4 ← facade の `tick`。

| 層 | 子モジュール |
|---|---|
| L1 | `cluster`（cluster/tunnel 接続・probe）、`housekeeping`（disk guard・scratch・後片付け）、`snapshot`、`quota_book`（quota の見積り・release）、`provider_select`、`workspaces`、`run_context`、`worker_task` |
| L2 | `work_units`、`tree_units`、`child_tasks`、`review_spawn` |
| L3 | `worker_finish`、`planner_flow`、`review_verdict`、`phase_integration`、`leases` |
| L4 | `dispatch_run` |
| 後回し | `sinks`（StoreSink / ReviewerSink） |

- 横の呼び出しは L3 内の `worker_finish` → `phase_integration::start_integration` の 1 本だけ許す。
- `worker_task` は `Dispatcher` に依存しない。

### D3. 規則

- 移動だけ。関数の本体は変えない。移動の commit と中身を変える commit を分ける。
- 可視性は `pub(super)` か既存の `pub(crate)` まで。新しい `pub`・trait・crate は足さない。
- 公開パス（`task_dispatch::dispatcher::ClusterConnector` など、lib.rs の再公開、`crate::dispatcher::provider_failure_outcome`）は facade で明示的に `pub use` し直す。`*` での再公開はしない。
- モジュール名は crate 直下（`review`、`integration`、`scratch_gc` など）とも `task_core` のモジュール名（`tree`、`quota` など）とも重ねない。test が `use super::*` と `use task_core::*` を併用するため。
- エラー・イベントの文字列、schema、設定は 1 byte も変えない。
- 各段で `cargo test -p task-dispatch` の passed 件数が変わらないことを確かめる。

### D4. 守る invariant

1. dispatcher から LLM を呼ばない。
2. `tick` の段階の順序を facade にそのまま残す（`disk_ready` の判定は drain より前）。
3. 同じ transaction に入れる組（`CheckpointSaved`+`WorkerFinished`、`QuotaEstimated`+`WorkerFinished`、provider cooldown+遷移、`DecisionRequested`+blocked、Plan の子の挿入+ReviewPass、子の作成+Created/ApprovalRequested、木の子の一括作成、`ExecutionGated`+routing）を helper に分けても割らない。
4. `release_quota_if_tracked` を早期 return するすべての経路で呼ぶ。
5. lease と lock の順（`run_holds_lease` の確認後に適用、`spawn_review` の per-task lock は verdict 保存まで保持、lease 取得と `running.insert` は同じ流れ、`stop_run` は kill_tree → abort）を保つ。
6. `RunEntry.handle` と `ReviewEntry.handle` の名前と型を変えない（test が直接 await する）。
7. 本番コードの直接の `OffsetDateTime::now_utc()` を `self.now_utc()` に置き換えない（test の時計の意味が変わる）。
8. cluster の hook は `run_cluster_hooks_off_async` で包んで呼ぶ形を保つ。

## 結果

- 局所変更の読む範囲が子モジュール 1 つに縮む。巨大関数（`finish_worker_result`、`dispatch_one` など）の中身の分割は移動の完了後に別の段で行う。
- 子モジュールは facade の private な型に `super::` 経由で触れるため、型の field の可視性は原則変えずに済む。

## 付記 2026-11-12: cluster の状態の owner を facade の外へ出した（R4 own-cluster）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D2 に従い、cluster・tunnel・job の poll の
状態（25 field）と処理を `crates/task-dispatch/src/cluster_runtime.rs` の `ClusterRuntime` に移した。`ClusterRuntime` は
`dispatcher` module の外にあり、`Dispatcher` を引数に受けない。module map の `cluster` の行は「`ClusterRuntime` を呼ぶ入口と
クラスタの解決」に、`cluster_job_wait` の行は「durable wait の行と task・WU の再開」に直し、`cluster_runtime` の行（層は owner）を足した。
子 module の型の可視性を変えない（上の「結果」）という前提は、`dispatcher` の外の owner には当てはまらない。owner の型は
`pub(crate)` で、field は private のまま method で読む。

## 付記 2026-11-12: disk・scratch・target の保守の状態の owner を facade の外へ出した（R4 own-maint）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D2 に従い、disk guard・disk 監視・scratch pool の
GC・build cache と repo target の掃除・中止された作業場所の片付けの状態（10 field）と処理を
`crates/task-dispatch/src/host_maintenance.rs` の `HostMaintenance` に移した。`HostMaintenance` は `dispatcher` module の外にあり、
`Dispatcher` を引数に受けない。module map の `housekeeping` の行は「`HostMaintenance` を呼ぶ入口と、verify の判定・承認と通知 feed の
照合・検査の `CARGO_TARGET_DIR`」に直し、`maintenance` の行（保守 executor と disk 監視の設定の入口）と `host_maintenance` の行
（層は owner）を足した。

## 付記 2026-11-12: 経路選択の状態の owner を facade の外へ出した（R4 own-routing）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D2・D3 に従い、経路選択の状態（15 field）と処理を
`crates/task-dispatch/src/routing_runtime.rs` の `RoutingRuntime` に移した。`RoutingRuntime` は `dispatcher` module の外にあり、
`Dispatcher` を引数に受けない。`route_state` の判断は `routing_runtime/route_state.rs` の純粋関数 `build_route_state` に分け、
account 一覧の走査 cache の更新と入力の収集は `RoutingRuntime::route_state` が行う。module map に `routing_runtime` の行（層は owner）を足し、
`route_failure` の行を「`RoutingRuntime` への cooldown の記録」に直した。移した module の旧い行は無かった。

## 付記 2026-11-13: quota の状態と見積りの判断を `RoutingRuntime` へ出した（R4 own-quota）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D3 に従い、`quota_activity`・`quota_calibration` を
下位 owner `routing_runtime/quota.rs` の `QuotaBook` に、`eval_runs`・`knowledge_probe_cache` を `RoutingRuntime` に移した。
`resolve_quota_estimate`（約 210 行）の判断は `routing_runtime/quota_estimate.rs` の純粋関数 `propose_estimate` に分け、適用
（較正の更新と、閉じたグループの他の member の event の追記）は `RoutingRuntime::settle_quota` が行う。
`quota_book` の行は、run 単位の入口（`quota_begin`・`release_quota_if_tracked`。終了経路は `RoutingRuntime::settle_quota` を直接呼ぶ）だけを持つ行に直した
（module は入口として残るので行は消さない）。

## 付記 2026-11-13: 実行中の run の表と lease の監督を `RunSupervisor` へ出した（R4 own-supervisor）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D3・D4 に従い、D1 の表で unit 列が own-supervisor の
7 field（`running`・`reviewing`・`infra_backoff`・`just_aborted`・`lost_review_watch`・`ownerless_reconciled_at`・`terminal_records_reconciled_at`）を
`crates/task-dispatch/src/run_supervisor.rs` の `RunSupervisor`（private field）に移した。`RunKey`・`RunEntry`・`ReviewEntry` も owner が
`dispatcher` module を使わないよう同じ file に移した。手元の run の問い合わせ（`runs`・`run_keys`・`holds_run_id`・`runs_for_task` ほか）、
abort（`abort_runs`・`take_run`）、shutdown の割り込み（`take_runs`・`take_reviews`）は `RunSupervisor` の method にした。
`reclaim_expired_leases` の判断は純粋関数 `reclaim_reason`・`reclaim_keys`・`reclaim_verdict` に分け、lease の解放と終了記録と backoff の期限は
`RunSupervisor::release_lease` が行う。WU・統合の照合と待ち（`checking`・`integrating`・`pending_subjects`）は `leases.rs` の入口に残した
（own-coord-b が `ExecutionCoordination` に移す）。D4 retry-backoff の `run_supervisor::infra_retry_after` を worker 完了と lease の回収・shutdown が使う。
module map に `run_supervisor` の行（層は owner）を足し、`leases` の行を入口に直した。移した module の旧い行は無かった。

## 付記 2026-11-13: write set・同期・依存・資格の待ちの状態を `ExecutionCoordination` へ出した（R4 own-coord-a）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D2・D6 に従い、D1 の表で unit 列が own-coord-a の
14 field（`write_reservations`・`write_set_waits`・`write_set_hold_seq`・`run_write_bases`・`review_sync_queue`・`review_sync_seq`・
`review_resync_halted`・`integrated_deps`・`wu_prepare_failures`・`credential_held`・`credential_busy`・`browser_ledger`・`browser_prereq_rescan`・
`cluster_waiting`）を `crates/task-dispatch/src/execution_coordination.rs` の `ExecutionCoordination`（private field）に移した。
`WriteReservation`・`WriteSetWait`・`reservations_overlap`・`RunWriteBase`・`ReviewSyncWait`・`WuPrepareFailures` も owner が `dispatcher` module を
使わないよう同じ file に移した。write set の blocker の絞り込みと予約の刈り込みは `RunSupervisor::run_keys` を引数で受け、owner は run の集合を持たない。
`write_set_gate.rs` には store の hint から予約を作る入口と、`TestHooks` の ablation を読んで `admit_write_set` に引数で渡す入口だけを残した。
`stale_priority.rs`・`browser_prereq.rs`・`credential_gate.rs`・`write_set_record.rs`・`review_spawn.rs` は owner の method を呼ぶ。
module map に `execution_coordination` の行（層は owner）を足した。移した module の旧い行は無かった（`write_set_gate` などは map に行を持たない）。

## 付記 2026-11-13: task・WU の待ち・check と統合の process・tick の状態を `ExecutionCoordination` へ出した（R4 own-coord-b）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D2・D3・D6 に従い、D1 の表で unit 列が own-coord-b の
16 field（`accepting_new_work`・`awaiting_children`・`awaiting_human`・`checking`・`dispatch_waits`・`integrating`・`last_tick_gap_ms`・
`last_tick_started`・`liveness_checked_at`・`max_concurrency`・`pending_subjects`・`rx`・`stall_watch`・`stalled_tasks`・`ticks`・`tx`）を
`Dispatcher` から `crates/task-dispatch/src/execution_coordination.rs` の `ExecutionCoordination` へ移した。値の型 `IntegrationEntry`・`CheckingEntry`・
`AwaitingChildren` も同じ file に移した。review の持ち越し（`ReviewSubject`）と完了の channel の値（`Completion`）は dispatcher の型なので、
owner は型引数 `ExecutionCoordination<S, C>` で受ける。lease の回収・abort の待ちの削除（`leases.rs`・`ownerless_runs.rs`）は、
`RunSupervisor` が手放した review の鍵を受ける `release_reclaimed_review` と、drain の `abort_in_hand` を通す。stale 優先の ablation は
`stale_priority.rs` の `stale_priority_disabled` が `TestHooks` から読み、`enter_review_sync_queue` に引数で渡す。module map の
`execution_coordination` の行を直した。移した module の旧い行は無かった（`leases`・`tree_units`・`snapshot` などの行は入口として残る）。
`ExecutionCoordination` の method が repo-health の閾値（型 1 つに 60）を超えないよう、WU の check と段の統合の process（`checking`・
`integrating`）を下位 owner `ProcessTable`、停滞の見張り（`stall_watch`・`stalled_tasks`・`liveness_checked_at`）を下位 owner
`LivenessWatch` に置いた（同じ file、`ExecutionCoordination` の private な field としてだけ作る。`RoutingRuntime` の中の `QuotaBook` と同じ形）。
入口は `processes()`・`liveness()`（と `_mut`）から method を呼ぶ。write set の blocker の判断は純粋関数 `write_set_blocker` にした。

## 付記 2026-11-13: 案件の停滞の見張り・CoS chat の起動の状態・残りの保守と probe の port を owner へ出した（R4 own-orchestration）

[ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md) D1・D2・D6 に従い、D1 の表で unit 列が own-orchestration の
7 field を `Dispatcher` から表の owner 列の置き場へ移した。

- `cos_chat_launch`・`stagnation` は新しい `crates/task-dispatch/src/project_orchestration.rs` の `ProjectOrchestration` へ。停滞の判定の状態の型
  `StagnationWatch` も `tree_units.rs` から同じ file に移し、field を private にした。CoS chat の起動の状態（`CosChatLaunch`）は dispatcher の型なので、
  owner は型引数 `ProjectOrchestration<L>` で受ける。`tree_units` の書き込み（閾値の差し替え・判定の開始・結果の記録）と `cos_chat/launch` の書き込み
  （差し込み・verify での取り外し・tick の間の取り出しと戻し）は owner の method（`configure_stagnation`・`begin_stagnation_scan`・`record_stagnation`・
  `install_launch`・`remove_launch`・`take_launch`・`restore_launch`）を通す。判定をするかの判断は既存の純粋関数 `task_ops::stagnation::scan_due`、
  判定は `task_ops::stagnation::detect` のまま。番号（generation）の進め方と、起動後にメモリの待ちが揃うまで判定しない条件は変えていない。
- `health_inventory`・`script_compare` は既存の `HostMaintenance` へ。Busy の印の型 `InventoryRuns` は `health_review/inventory.rs` から
  `host_maintenance.rs` に移した（`inventory.rs` は re-export と `port` の関数だけを持つ）。TS の比較の状態（`ScriptCompareState`）は dispatcher の型なので、
  owner は型引数 `HostMaintenance<C>` で受け、入口は `script_compare()`・`script_compare_mut()` から読む。`HostMaintenance` の struct に
  `#[cfg(test)]` の field は無い。`ScriptCompareState` の中の試験専用の field `next_digest_pass_id` は今の場所に残した（`TestHooks` へ移すと、
  repo-health の size 規則の test mask がそれまで数えていなかった `script_compare.rs` の `impl Dispatcher` の 12 method を数え始め、
  `Dispatcher` の method 数が 382 から 394 に見かけ上増えて fail になるため。進捗の提案に書いた）。
- `eval_version_probe`・`knowledge_probe`・`now_unix_fn` は新しい `crates/task-dispatch/src/probe_ports.rs` の `ProbePorts` へ（本番の既定を持つ
  注入 port。`#[cfg(test)]` ではない）。`KnowledgeProbe` の定義もここに移し、`dispatcher::KnowledgeProbe` は re-export で同じ path のまま。owner は
  `ProbePorts` を持たず、`Dispatcher` の入口が読んだ値を引数で渡す。
- 試験 `structure_guard_project_orchestration`・`structure_guard_probe_ports` が D2 の 1〜3 を source から確かめ、`OWNER_STRUCTS` に
  `ProjectOrchestration<L>`・`StagnationWatch`・`ProbePorts`・`InventoryRuns` を足した（`HostMaintenance` は `HostMaintenance<C>` に直した）。
- module map に `project_orchestration`・`probe_ports` の行を足し、`host_maintenance`・`tree_units` の行を直した。移した module の旧い行は無かった
  （`tree_units`・`cos_chat`・`script_compare`・`cron_tick` は入口として残る）。
