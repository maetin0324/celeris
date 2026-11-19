---
title: アーキ v4 H safety — 安全不変条件の試験（crate と filter を絞った再実行）と release gate の記録
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: running
updated: 2026-11-19
---
# safety（WorkUnit）

不変条件ごとに、`docs/architecture-map.md` の試験の列と、`CLAUDE.md`・review 方針（`scripts/dev/repo-health/profiles/celeris.toml` の `[review]` 節）が挙げる根拠の file から試験を選んだ。
試験は `cargo nextest list -p <crate> -E '<filter>'` で実在を確かめてから、`cargo nextest run -p <crate> -E '<filter>'` で走らせた。
workspace 全体・`--workspace`・doc test・e2e は走らせず、`CELERIS_USERNS_TESTS` は付けていない。fail は直さず fail と書く。
結果の表は [safety.tsv](safety.tsv)（列は `invariant result command passed note`）。

この節までは unit core（task-core・task-ops・task-dispatch・task-worker の 6 件）が書いた。
後の unit api（ACL・credential・CoS 監査・selfdeploy の権限・D・E・F の反例試験）と record（release gate の記録）が同じ 2 file に行と節を足す。

実行した tree: 作業ブランチの HEAD（基準は `$CELERIS_WU_BASE`、統合先は H の task branch）。この unit は crates/・scripts/・web/ を変えていない。
nextest の summary の `skipped` は filter から外れた試験の数で、実行時の skip ではない。各 filter の `--run-ignored only` の件数はどれも 0 だった。

## 結論（6 件）

| invariant | result | passed | crate |
|---|---|---|---|
| sqlite_source | pass | 169 | task-core 140・task-worker 29 |
| events_append_only | pass | 13 | task-core |
| lease_atomicity | pass | 51 | task-core 12・task-dispatch 39 |
| replay | pass | 14 | task-ops |
| project_controller_stop | pass | 14 | task-dispatch |
| rust_ts_canary | pass | 16 | task-dispatch |

fail は 0 件。注意点は 1 つ: sqlite_source の task-worker の db_guard の 29 件のうち 7 件は、`CELERIS_USERNS_TESTS` が無いと userns の検査の前に return して pass に数えられる（実質は未検査）。

## sqlite_source

- 守る条件: task の状態の真実は SQLite。worker・会話のメモリに持たない。状態は `transition` が許す遷移でだけ変わる。worker の run は DB を読むだけ。
- 根拠の file: `CLAUDE.md`（「禁止」の task-state-in-sqlite。review 方針の invariants `task-state-in-sqlite`）、`agent-docs/adr/0001-scope-and-principles.md`、`agent-docs/adr/0002-state-machine.md`、`agent-docs/adr/2026-11-16-taskstore-trait-removal.md`、`agent-docs/adr/0095-worker-runs-see-the-db-read-only.md`。
- 試験（architecture-map の「SQLite 永続化」「Task 型・状態機械」の行と ADR-0095 の db_guard）:
  - task-core `store::tests` 111 件、`store::legacy::tests` 7 件（file は `store/legacy_tests.rs`）、`store::client_open_tests` 6 件、`store::migration_harness_tests` 2 件、`transition::tests` 14 件（計 140 件）。
  - task-worker `db_guard::tests` 29 件。うち 7 件（`user_systemd_bus_is_hidden_from_launched_process`・`host_config_read_only_inside_namespace`・
    `the_db_and_its_wal_files_are_read_only_but_siblings_stay_writable`・`a_nested_user_namespace_cannot_undo_the_read_only_mount`・
    `the_guarded_process_has_no_capabilities_and_keeps_its_uid`・`probe_confirms_the_db_is_not_writable_inside`・`ssh_config_includes_still_parse_inside_the_namespace`）は
    `CELERIS_USERNS_TESTS` 無しで早期 return する。
- 件数: 169 passed（140 + 29）、fail 0、ignored 0。
- command:
  ```
  cargo nextest run -p task-core -E 'test(/^store::(tests|migration_harness_tests|client_open_tests|legacy::tests)::/) | test(/^transition::tests::/)'
  cargo nextest run -p task-worker -E 'test(/^db_guard::tests::/)'
  ```

## events_append_only

- 守る条件: events は追記専用で書き換え・削除しない。遷移と event は同じ transaction で書く。
- 根拠の file: `CLAUDE.md`（「禁止」の events の書き換え・削除。invariants `events-append-only`）、`agent-docs/adr/0001-scope-and-principles.md`。
- 試験（architecture-map の「Event（追記専用）」の行が挙げる `append_event_*`・`apply_transition_*_appends_*` と、追記専用の trigger を持つ表の試験）:
  - `store::tests::append_event_reads_back_in_seq_order`
  - `store::tests::apply_transition_accept_moves_draft_to_ready_and_appends_event`
  - `store::tests::apply_transition_appends_extra_event_atomically`
  - `store::tests::apply_transition_with_events_appends_transitioned_then_extras_in_order`
  - `store::tests::create_task_inserts_task_and_events_atomically`
  - `test(append_only)` の 8 件: `store::credential_wait::tests::secret_saved_event_is_append_only_and_deduplicated`、
    `store::project_wake::tests::project_wake_store_decisions_are_append_only`、`store::roadmap::tests::project_roadmap_store_rows_are_append_only`、
    `store::script_behavior_changes::tests::behavior_change_events_append_only`、`store::script_domains::tests::script_compare_log_appends_once_per_outcome_and_is_append_only`、
    `store::script_domains::tests::script_domain_events_table_is_append_only`、`store::script_domains::tests::script_generation_events_append_only`、
    `store::terminal_sessions::tests::append_only_rows`
- 件数: 13 passed、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p task-core -E 'test(/^store::tests::append_event_/) | test(/^store::tests::apply_transition_.*appends/) | test(/^store::tests::create_task_inserts_task_and_events_atomically$/) | test(append_only)'
  ```

## lease_atomicity

- 守る条件: lease は 1 つの持ち主だけが持つ。lease の取得と `Transitioned` event は同じ transaction で書く。回収は lease を比べてから解放する（compare-and-release）。
  持ち主の無い running の run 行は、持ち主が居ないと確かめてから回収する。
- 根拠の file: `agent-docs/adr/2026-10-03-ownerless-running-runs.md`（invariants `lease-single-holder`）、`agent-docs/adr/2026-11-15-lease-reclaim-compare-and-release.md`、
  `agent-docs/adr/0070-task-failure-visibility-and-handoff-safe-runs.md`、`agent-docs/adr/2026-11-11-dispatcher-state-ownership.md`。
- 試験（architecture-map の「実行中の run の表と lease の監督」「持ち主の居ない running の runs 行」の行）:
  - task-core `store::lease_reclaim_tests` 5 件、`store::tests` の `acquire_lease_*`（3）・`acquire_work_unit_lease_*`（2）・`renew_lease_extends_only_the_matching_running_lease`・
    `apply_transition_releases_lease_when_leaving_running`（計 12 件。`acquire_lease_is_exclusive_under_concurrency`・`acquire_lease_appends_transitioned_event_atomically` を含む）。
  - task-dispatch `dispatcher::tests::cex_lease_restart` 12 件、`dispatcher::tests::cex_same_tick` 3 件、`dispatcher::tests::ownerless_runs` 11 件、
    `dispatcher::tests::orphan_takeover` 6 件、`run_supervisor::tests` 7 件（計 39 件）。
- 件数: 51 passed（12 + 39）、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p task-core -E 'test(/^store::lease_reclaim_tests::/) | test(/^store::tests::(acquire_lease_|acquire_work_unit_lease_|renew_lease_|apply_transition_releases_lease_)/)'
  cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::(cex_lease_restart|cex_same_tick|ownerless_runs|orphan_takeover)::/) | test(/^run_supervisor::tests::/)'
  ```

## replay

- 守る条件: events を replay した結果は現在の投影と一致する。
- 根拠の file: `agent-docs/adr/0001-scope-and-principles.md`（architecture-map の「Event からの replay/整合性検査」の行）。
- 試験: task-ops `replay::tests` 全 14 件（`replay_reports_zero_mismatches_when_events_match_current_state`・`replay_detects_status_drift_from_events`・
  `replay_matches_live_after_blocked_repair_superseded`・`rebuild_work_units_and_runs_matches_the_scheduler_written_index_for_the_e2_fixtures`・
  `check_and_apply_execution_fixes_a_corrupted_index` ほか）。
- 件数: 14 passed、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p task-ops -E 'test(/^replay::tests::/)'
  ```

## project_controller_stop

- 守る条件: 案件の mode `off`・phase `paused`・`cancelled`・案件の pause・cancel のとき、controller の効果は実行されず `suppressed` になる。
  人の操作と競合した判断は効果を出さない。pause・cancel・未回答の問い・危険な予算超過は時間や回答で自動に解かない。不正な提案が続けば止まる。
- 根拠の file: `agent-docs/adr/2026-11-04-project-controller.md`（D3・D9、契約の 10「pending の効果の門と再試行」、A6 の E2E の試験名）、
  architecture-map の「Project Controller（wake・lease・判定・効果・guard）」の行。
- 試験（task-dispatch）:
  - `dispatcher::tests::project_wake::effects::project_wake_pending_{off,paused,cancelled,shadow}`（4）
  - `dispatcher::tests::project_wake::project_wake_human_races_{apply,cancel,pause}`（3）、`project_wake_off_is_inert`（1）
  - `dispatcher::tests::project_wake::project_cos::recovery::project_recovery_k37_{dangerous_budget_overrun_stays_blocked,pause_and_cancel_are_not_lifted_by_time_or_answers,unresolved_human_required_stays_blocked}`（3）
  - `dispatcher::tests::project_wake::replay::project_controller_replay_{paused_project,cancelled_project,invalid_proposals_stop}`（3）
- 件数: 14 passed、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::project_wake::effects::project_wake_pending_/) | test(/^dispatcher::tests::project_wake::project_wake_(human_races_|off_is_inert$)/) | test(/project_recovery_k37_/) | test(/^dispatcher::tests::project_wake::replay::project_controller_replay_(paused_project|cancelled_project|invalid_proposals_stop)$/)'
  ```

## rust_ts_canary

- 守る条件: canary の対象は単位ごとに決定的に選ぶ。TS 側は検証した TS の結果だけを使い、Rust 側も比較を続ける。拒否・失敗・効果の前の fallback・予算超過は Rust に倒れ（fail-open にしない）、
  効果の未検証は止めて rollback する。
- 根拠の file: `agent-docs/adr/2026-11-02-ts-control-layer-boundary.md`（S5-3 canary の選び方、S5-4 mode の遷移と権限、停止基準の表）。
- 試験（task-dispatch の `test(canary)` の全 16 件）:
  - `dispatcher::tests::script_canary` 7 件（`canary_denied_never_fails_open`・`canary_fallback_before_effect_uses_rust`・`canary_rust_side_still_compares`・
    `canary_selection_is_deterministic_per_unit`・`canary_ts_side_uses_validated_ts_result`・`rollback_write_failure_treated_as_rust`・`tick_budget_200ms_falls_back_after_budget`）
  - `dispatcher::tests::cos_chat_triage::cos_digest_script::canary` 3 件（`canary_digest_no_double_message_or_effect`・`canary_digest_pass_selection_matches_record`・`canary_effect_unverified_stops_and_rolls_back`）
  - `dispatcher::tests::cos_chat_triage::cos_digest_script::cos_digest_behavior_change` の canary 4 件
  - `dispatcher::tests::script_compare::script_compare_gate::script_compare_gate_canary_splits_by_task_id`（1）、`dispatcher::tests::route_select::route_priority_canary_override_reorders_only_its_harness`（1）
- 件数: 16 passed、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p task-dispatch -E 'test(canary)'
  ```
