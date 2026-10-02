---
tasks: [01M3Y4FZD5HH800XATG3K5KQ28]
---

# 時間依存試験の調査一覧

方式の記号は [ADR-0125](../adr/0125-deterministic-time-tests.md) に従う。(a) 注入時計・停止時計、(b) 状態または event の到着待ち、(c) `SIGSTOP` / `SIGCONT` による競合の固定、(d) 試験専用の同期フック。待機上限は成功条件ではなく異常時の保険とする。

## 対象の 6 試験

### `a_wait_parks_the_task_polls_and_resumes_as_a_continuation`

- ファイル: `crates/task-dispatch/src/dispatcher/tests/cluster_job_wait.rs`
- 原因: `tick_until` の 200 / 400 tick と各 tick 後の 20 ms sleep が、非同期 poller と worker の完了速度を仮定する。poll 間隔の時計は注入済みでも、結果の到着は実 scheduler に依存する。
- 方式: **(a) + (b)**。`test_now` で poll 時刻を進め、`Blocked`、poll event、`Satisfied`、`Done` を保存状態・event で待つ。slot / attempts の解放、poll の重複禁止、continuation の前置きも検査する。詳細とフック点は [ADR-0125 §1](../adr/0125-deterministic-time-tests.md#1-a_wait_parks_the_task_polls_and_resumes_as_a_continuation)。

### `every_cargo_path_uses_the_scratch_target_dir`

- ファイル: `crates/task-dispatch/src/dispatcher/tests/build_cache.rs`
- 原因: `run_until(..., 800, Done)` が並列 WU、checks、統合、review の所要時間を tick 数で測る。高負荷では `Reviewing` のまま予算を使い切る。
- 方式: **(b)**。各 run / check / review の記録と最終 `Done` を待つ。`Reviewing` を成功とせず、全 `CARGO_TARGET_DIR` の owner 別 scratch 対応を保持する。詳細とフック点は [ADR-0125 §2](../adr/0125-deterministic-time-tests.md#2-every_cargo_path_uses_the_scratch_target_dir)。

### `command_checks_are_re_executed_in_workspace`

- ファイル: `crates/task-dispatch/src/review/tests.rs`
- 原因: 3 秒 timeout で実 shell を起動し、timeout 時は 6 秒で再試行する。高負荷では短い正常コマンドまで timeout になり得る。
- 方式: **(d)**。通常の workspace 内実行は実 `LocalWorkspace` で確認し、timeout / 再試行枝は `Workspace::exec` の試験用実装から明示的な timeout 結果を返す。終了コード、欠落ファイル、3 秒→6 秒の再試行契約を残す。詳細とフック点は [ADR-0125 §3](../adr/0125-deterministic-time-tests.md#3-command_checks_are_re_executed_in_workspace)。

### `real_broker_browser_injection_receipt_and_origin_guards`

- ファイル: `crates/task-worker/tests/browser_injection_wire.rs`
- 原因: runtime の起動直後に CDP `Target.createTarget` を送り、応答を得られず `page target: SinkFailed` になる。runtime ready と CDP ready は別であり、現エラーだけでは起動遅延・早期終了・pipe 故障を区別できない。
- 方式: **(b) + (d)**。`Browser.getVersion` の有効な応答を CDP ready として待ち、試験用 feature のみで CDP 応答上限を調整する。`SinkFailed` は成功・skip にせず、receipt と origin / iframe 拒否を維持する。詳細とフック点は [ADR-0125 §4](../adr/0125-deterministic-time-tests.md#4-real_broker_browser_injection_receipt_and_origin_guards)。

### `controller_kill_leaves_no_runtime_processes`

- ファイル: `crates/task-worker/tests/browser_runtime_isolated.rs`
- 原因: controller kill 後の PID 生存判定に subreaper 配下の zombie が混ざり得る。ただし現コードは既に `/proc/<pid>/stat` の `Z` / `X` と starttime を区別するため、再発時は state を確認して実生存と分ける。info-fd 報告と `PR_SET_PDEATHSIG` 準備の間にも競合窓がある。
- 方式: **(c) + (d)、終了判定は (b)**。準備境界を試験用フックで露出し stutter で順序を固定する。controller 終了後に同一 process の消滅または zombie 化を確認し、PID / starttime / PPid を診断する。明示的な isolation skip 規則は維持する。詳細とフック点は [ADR-0125 §5](../adr/0125-deterministic-time-tests.md#5-controller_kill_leaves_no_runtime_processes)。

### `tick_prunes_the_oldest_terminal_workspace_and_records_an_event`

- ファイル: `crates/task-dispatch/src/dispatcher/tests/cleanup_and_disk.rs`
- 原因: `prune_one_workspace` は削除を別スレッドに逃がし、削除が終わってから `store.append_event` で `WorkspacePruned` を積む（housekeeping.rs 435-477 行）。元の試験は `tick()` 後に「`target/` が消えるまで」だけを `for _ in 0..100 { … sleep(20ms) }`（約 2 秒）で待ち、その直後に `events_for` を読んでいた。削除とイベント追記は別の処理なので、高負荷で `target/` の unlink は終わっていてもイベントの store 書き込み（sqlite の lock 取得含む）がまだ終わっていない窓があり、`cleanup_and_disk.rs:97` の assert が先に落ちる（2026-10-02 に他 task の統合検査で再現）。
- 方式: **(b)**。固定回数ループを削除し、`target/` の消滅と `store.events_for` に期待の `WorkspacePruned { removed: ["repos/benchfs/target"] }` が現れることの両方を 1 つの待ちループで確認する（`wait_for_prune`）。[`STATE_WAIT_GUARD`]（60 秒）を壊れたときに止まる保険にし、超えたら `panic!` で `target_dir.exists()` と現在の `events` を出す。`target/` 消滅・repo dir 残存・`removed` の中身の assert は元のまま弱めていない。
- `workspace_prune_after_secs_zero_disables_pruning`（同 file）の 50ms sleep は直さない: `workspace_prune_after_secs == 0` のとき `prune_one_workspace` は削除スレッドを一切立てずに即 return する（housekeeping.rs の `if ... == 0 { return; }`）ので、待っても届かない非同期処理が無く、負荷で偽の失敗を生む経路がない。

## その他の一覧

調査には次の `git grep` を用い、命中箇所の所属する試験と周囲の待機条件を確認した。`run_until_idle` は単なる dispatcher 駆動にも大量に使われるため、それだけでは採用せず、状態・event を固定 tick 数で待つ呼び出しを採った。表の「形」は現在の実装であり、方式は変更時の推奨である。

```sh
git grep -nE 'Duration::from_millis\([0-9]+\)|Duration::from_secs\([0-9]+\)|run_until\(|tick_until|run_until_idle\(' -- crates tests
git grep -nE 'deadline|timeout\(Duration::from_(millis|secs)|Instant::now\(\).*Duration::from_(millis|secs)|run_until\(|tick_until|for _ in 0\.\.[0-9]+' -- 'crates/**/tests/**' 'crates/**/tests.rs' 'tests/**'
git grep -nE 'Instant::now\(\)|time::timeout\(|timeout\(Duration::from_(millis|secs)|for _ in 0\.\.[0-9]+' -- crates/task-worker/tests tests/e2e/tests
```

| ファイル | 試験名 | 形 | 推奨方式 (a)〜(d) | 印 |
| --- | --- | --- | --- | --- |
| `crates/task-dispatch/src/dispatcher/tests/cluster_job_wait.rs` | `a_timed_out_wait_asks_a_human_and_the_answer_resumes` | `tick_until` 200 / 400、20 ms sleep で wait / answer を待つ | (a) + (b) | 直す候補 |
| 同上 | `cancelling_a_waiting_task_cancels_the_wait_without_qdel` | `tick_until` 200 / 400 で poll / cancel を待つ | (b) |  |
| 同上 | `a_wait_on_an_unknown_cluster_is_a_retryable_failure` | `tick_until` 400 で retryable failure を待つ | (b) |  |
| 同上 | `a_leaf_unit_waits_alone_and_liveness_names_the_wait` | `tick_until` 800 で leaf wait / liveness を待つ | (a) + (b) |  |
| `crates/task-dispatch/src/dispatcher/tests/build_cache.rs` | `parallel_work_units_get_their_own_cargo_target_dir_and_it_is_removed_when_done` | `run_until` 800 / 200 の後、削除 thread を 10 秒 deadline で待つ | (b) | 直す候補 |
| `crates/task-dispatch/src/dispatcher/tests/orphan_takeover.rs` | `an_orphaned_run_with_a_done_result_is_finalised` | `run_until` 300 で store 状態を待つ | (b) |  |
| 同上 | `an_orphaned_parallel_work_unit_run_is_reconciled_without_waiting_for_its_lease` | `run_until` 300 で gate 状態を待つ | (b) |  |
| `crates/task-dispatch/src/dispatcher/tests/tree_approval.rs` | `human_replan_whose_first_attempt_is_invalid_gets_its_second_attempt` | `run_until_quiet` 800 と 300 回×20 ms の状態待ち | (b) | 直す候補 |
| `crates/task-dispatch/src/dispatcher/tests/tree_gate.rs` | `plan_limits_raise_limit_decisions_and_hold_only_the_excess` | `run_until_quiet_approving` 1500 tick で decision を待つ | (b) |  |
| `crates/task-dispatch/src/dispatcher/tests/tree_replan.rs` | `child_infra_failure_is_not_a_question` | `run_until_quiet` 1500 tick と固定 10 tick で状態を待つ | (b) |  |
| `crates/task-dispatch/src/dispatcher/tests/work_units.rs` | `a_pending_human_replan_runs_the_planner_before_retrying_the_integration` | `run_until` 600 で WU 状態を待つ | (b) |  |
| 同上 | `terminal_work_unit_target_is_reclaimed_on_the_next_tick` | 削除完了を 10 秒 deadline + sleep で待つ | (b) |  |
| `crates/celeris/tests/browser_startup_reap.rs` | `startup_reaps_dead_instance_and_preserves_live_instance` | runtime 終了を 3 秒 deadline + 20 ms sleep で待つ | (b) | 直す候補 |
| `crates/task-api/tests/stream.rs` | `serve_over_loopback_tcp_streams_events_and_closes_on_shutdown` | SSE の EOF と server join を各 5 秒 deadline で待つ | (b) | 直す候補 |
| `crates/task-api/tests/browser_h3_injection.rs` | `production_h3_injects_once_without_exposure`（`start_fixture`） | TLS fixture の listen を 10 秒 deadline + sleep で待つ | (b) |  |
| `crates/task-worker/tests/browser_h3_wire.rs` | `real_broker_browser_injection_receipt_and_origin_guards`（`fixture` / `start_broker`） | TLS listen は 10 秒、broker socket は 200 回×10 ms | (b) |  |
| `crates/task-worker/tests/browser_injection_attacks.rs` | `real_browser_injection_attack_matrix`（`fixture` / `start_broker`） | TLS listen は 10 秒、broker socket は 200 回×10 ms | (b) |  |
| `crates/celeris-credentiald/tests/injection_ipc.rs` | `success_reply_is_receipt_only_and_secret_reaches_only_the_sink` / `auth_section_is_required_and_bound_to_its_lease` など（共通 `Fx::new`） | 2 socket の出現を 200 回×10 ms だけ待つ | (b) | 直す候補 |
| `crates/task-worker/src/process_group/tests.rs` | `kill_tree_terminates_the_whole_group_including_grandchildren` | grandchild の消滅を固定 100 回の sleep で待つ | (b) + (c) |  |
| `crates/task-dispatch/tests/unified_kill.rs` | `cancel_kills_the_worker_process_group_including_grandchildren` / `interrupt_kills_the_worker_process_group_including_grandchildren` | PID file 400 回、消滅 200 回の sleep 待ち | (b) + (c) |  |
| `crates/task-worker/tests/browser_egress_process.rs` | `independent_proxy_refuses_worker_selected_private_or_proxy_destinations` | 子 process 終了を 8 秒 deadline + 10 ms sleep で待つ | (b) |  |
| `crates/scratch-cache/src/tests.rs` | `put_returns_before_the_l2_write` / `corrupt_l2_entry_is_discarded` / `l2_gc_evicts_lru` | 共通 `wait_until` が L2 の状態を 10 秒 deadline + 20 ms sleep で待つ | (b) |  |
| `crates/scratch-cache/tests/webdav.rs` | `sccache_request_sequence_round_trips` / `l2_hit_through_http_is_promoted` | flusher の L2 書き込みを 10 秒 deadline で待つ | (b) |  |

除外した例: `Duration::from_millis` / `from_secs` のうち本体設定を fixture に渡す `kill_grace` や cache の interval、`crates/scratch-cache/tests/webdav.rs::sccache_request_sequence_round_trips` の PUT が 2 秒未満という性能条件は状態待ちではない。`crates/task-dispatch/tests/unified_kill.rs::the_wall_clock_timeout_kills_the_worker_process_group_including_grandchildren` は timeout 発火自体が検査対象であり、上表の通常の終了待ちと同一視しない。`run_until_idle` だけを呼ぶ多数の dispatcher 試験も、固定 tick が状態待ちの失敗条件だと確認できたもの以外はこの表に含めない。
