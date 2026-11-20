---
title: アーキ v4 H safety — 安全不変条件の試験（crate と filter を絞った再実行）と release gate の記録
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# safety（WorkUnit）

## 0. 結論

- 安全不変条件 10 件と arch_v4_counterexamples の計 11 行（[safety.tsv](safety.tsv)）: pass 11・fail 0・untested 0。passed は延べ 563 件
  （`cex_lease_restart` 12 件と `cex_same_tick` 3 件は lease_atomicity と arch_v4_counterexamples の両方に数えた）。
- 未達の不変条件: 無し。ただし 9 件は userns の env が無いと検査の前に return して pass に数えられる（実質は未検査）:
  sqlite_source の task-worker の `db_guard` 7 件と、credential の celeris-credentiald の `injection_peer_uid` 2 件。
- D・E・F の反例試験（arch_v4_counterexamples）: 81 passed、3 つの command がどれも exit 0。
- release gate（記録を読んだだけ。release.sh・verify.sh・promote.sh・status.sh は走らせていない）: いまの current は release 1680de5cd1b4
  （G の最後の commit「integrate wu/close (phase close)」2026-11-19T10:37:13Z）で、B〜G の最後の commit をすべて含む。
  この release は gate ok（17 段がどれも exit 0、nextest 8525 passed・fail 0）、verify ok（6 検査 ok）で、2026-11-19T11:30:18Z に昇格した。
  E・F・G を最初に含んだ release は gate・verify・昇格を記録で確かめた。B・C・D を最初に含んだ release（48b700ede57f・f85df0209504・eb1244bb5e8d）は
  promote ログで昇格を確かめたが、release の dir が掃除で消えていて gate・verify の記録は未確認（詳細は下の「release gate」節）。

不変条件ごとに、`docs/architecture-map.md` の試験の列と、`CLAUDE.md`・review 方針（`scripts/dev/repo-health/profiles/celeris.toml` の `[review]` 節）が挙げる根拠の file から試験を選んだ。
試験は `cargo nextest list -p <crate> -E '<filter>'` で実在を確かめてから、`cargo nextest run -p <crate> -E '<filter>'` で走らせた。
workspace 全体・`--workspace`・doc test・e2e は走らせず、`CELERIS_USERNS_TESTS` は付けていない。fail は直さず fail と書く。
結果の表は [safety.tsv](safety.tsv)（列は `invariant result command passed note`）。

この節までは unit core（task-core・task-ops・task-dispatch・task-worker の 6 件）が書いた。
後の unit api（ACL・credential・CoS 監査・selfdeploy の権限・D・E・F の反例試験）と record（「0. 結論」と release gate の記録）が同じ file に節を足した。record は safety.tsv の行を変えていない。

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

## api の結論（5 件）

ここから下は unit api が書いた（task-api・celeris-credentiald・task-worker・celeris・task-dispatch）。候補は `cargo nextest list -p <crate> -E '<filter>'` で実在を確かめてから走らせた。

| invariant | result | passed | crate |
|---|---|---|---|
| acl | pass | 28 | task-api |
| credential | pass | 78 | celeris-credentiald 54・task-worker 5・task-api 19 |
| cos_audit | pass | 16 | task-api |
| selfdeploy_authority | pass | 83 | celeris 80・task-api 3 |
| arch_v4_counterexamples | pass | 81 | task-api 14・task-dispatch 67 |

fail は 0 件、ignored は 0 件（各 filter の `--run-ignored only` の件数はどれも 0）。注意点は 1 つ: credential の celeris-credentiald の `injection_peer_uid` の 4 件のうち 2 件は、
`CELERIS_USERNS_TESTS` などの env が無いと早期 return して pass に数えられる（実質は未検査）。

## acl

- 守る条件: API の主体は認証済みの Bearer token から決め、本文や header の自己申告から決めない。未知の Bearer を人に倒さない。MCP の内部 token は読むだけで書けない。
  書き込みの endpoint は認証と guard（Host・Origin・Content-Type・本文の大きさ）を通る。
- 根拠の file: `agent-docs/adr/2026-10-27-api-principals.md`（review 方針の invariants `api-principal-from-token`。「検証」の項）、
  `agent-docs/adr/0013-taskd-api-and-gui-foundations.md`（architecture-map の router の行）。
- 試験（task-api）:
  - `--test auth_and_guards` 11 件（`bearer_token_is_required_when_configured`・`host_header_is_checked_against_the_allow_list`・
    `post_with_origin_is_forbidden_and_changes_nothing`・`post_requires_json_content_type`・`bodies_over_one_mebibyte_are_rejected_with_413`・
    `no_token_means_no_authentication` ほか）
  - lib の `principal::tests` 3 件（`caller_requires_a_principal_from_the_guard`・`token_debug_is_redacted`・`token_principals_parse_and_render`）、
    `middleware::tests` 3 件、`handlers::tests::resolve_principal_distinguishes_config_tokens_and_mcp_clients` 1 件
  - `browser_eval_surface_suite` の `api_principals::` 10 件（`api_principals_two_tokens_are_recorded_as_different_actors`・
    `api_principals_unknown_bearer_and_spoofed_headers_are_not_trusted`・`api_principals_mcp_token_reads_with_a_client_id_and_never_writes`・
    `api_principals_cos_runs_are_recorded_with_their_thread` ほか）
- 件数: 28 passed、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p task-api -E 'binary(auth_and_guards) | (kind(lib) & test(/^(principal|middleware)::tests::|^handlers::tests::resolve_principal_/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::/))'
  ```

## credential

- 守る条件: credential の値は broker（`celeris-credentiald`）の外に出さず、相手 process の uid を確かめて渡す。worker・API の応答・event・config に値を出さない。
- 根拠の file: `agent-docs/adr/0080-browser-phase2-policy-broker-approval.md`（invariants `credential-stays-in-broker`）、
  architecture-map の `celeris-credentiald`・Browser capability・Browser credential login の行、review 方針の credential class（`crates/task-api/src/handlers/secrets.rs`）、
  `agent-docs/adr/0126-test-daemons-without-userns.md`（peer uid の実 process の試験は opt-in）。
- 試験:
  - celeris-credentiald の `--test broker` 17 件・`--test injection_ipc` 19 件・`--test injection_peer_uid` 4 件・`--test prod_admission` 14 件（計 54 件）。
    `injection_peer_uid` の `real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch` は `CELERIS_USERNS_TESTS` 無しで SKIPPED を出して return し、
    `inner_peer_uid_probe` は probe の socket の env が無いと return する（どちらも pass に数えられる）。`same_uid_control_is_worker_not_allowed`・`probe_line_round_trips` は実行される。
  - task-worker の `browser_credential::tests` 5 件（`auth_login_uses_name_and_item_reference`・`origin_probe_is_explicitly_allowed_but_observation_is_denied` ほか）。
  - task-api の lib `secrets::tests` 7 件（`fingerprint_is_first_8_hex_chars_of_sha256_and_does_not_reveal_the_value`・`secret_ids_reject_path_traversal_and_empty` ほか）、
    `run_worker_suite` の `secrets_admin::` 12 件（`get_lists_items_with_used_by_and_never_the_value`・`put_records_a_secret_stored_event_without_the_value`・
    `secret_values_never_appear_in_config_or_secrets_responses`・`all_three_endpoints_require_a_token_even_on_loopback` ほか）。
- 件数: 78 passed（54 + 5 + 19）、fail 0、ignored 0。うち 2 件は上の早期 return。
- command:
  ```
  cargo nextest run -p celeris-credentiald -E 'binary(broker) | binary(injection_peer_uid) | binary(prod_admission) | binary(injection_ipc)'
  cargo nextest run -p task-worker -E 'test(browser_credential)'
  cargo nextest run -p task-api -E '(kind(lib) & test(/^secrets::tests::/)) | (binary(run_worker_suite) & test(/^secrets_admin::/))'
  ```

## cos_audit

- 守る条件: CoS の変更操作は `/cos/operations` の監査付きの操作層だけを通り、監査と業務の書き込みは同じ transaction に入る。直接の mutation・未登録の操作・
  許可リストの外の path・本文の主体の自己申告は理由付きで拒否する。
- 根拠の file: `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md`（invariants `cos-audit-same-tx`）、`agent-docs/adr/2026-10-27-api-principals.md` D3、
  architecture-map の「CoS チャットの REST/SSE と cos API」の行。
- 試験（task-api）:
  - `--test cos_operations` 8 件（`cos_chat_ops_api_task_create_writes_domain_audit_and_card_together`・`cos_chat_ops_api_rejects_unregistered_operations_regardless_of_skill`・
    `cos_chat_ops_api_rejects_paths_outside_the_allowlist_with_reasoned_events`・`cos_chat_ops_api_idempotency_same_hash_same_operation_different_hash_409` ほか）
  - lib の `cos::operations::skill_table_tests::cos_operator_skill_table_matches_allowed` 1 件
  - `cos_delivery_suite_a` の `cos_auth::` 7 件（`cos_chat_ops_auth_get_allowed_and_direct_mutation_is_422_with_audit`・`cos_chat_ops_auth_cos_shape_never_falls_back_to_admin`・
    `cos_chat_ops_auth_expired_revoked_and_unknown_are_401` ほか）
  - 監査と書き込みを 1 つの transaction に入れる反例（`tx_boundary_contract` の `cos_record_writes_audit_and_reference_in_one_transaction`・
    `cos_project_cancel_writes_audit_and_cascade_in_one_transaction`、`org_write_contract` の `cos_org_writes_commit_row_browser_event_operation_and_audit_together`）は arch_v4_counterexamples で走らせた。
- 件数: 16 passed、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p task-api -E 'binary(cos_operations) | (kind(lib) & test(/^cos::operations::/)) | (binary(cos_delivery_suite_a) & test(/^cos_auth::/))'
  ```

## selfdeploy_authority

- 守る条件: 昇格していない release の daemon は本番を引き継がず、DB を作らず・移行せず・handoff を求めない（exit 4）。昇格は verify 済みで current でも昇格中でもない release だけで、
  token を要する。昇格の監査は web と外部 operator を区別する。
- 根拠の file: `docs/ops/selfdeploy.md`（invariants `unpromoted-release-no-takeover`。§4 昇格は人だけ、§4c CoS の `release.promote`）、architecture-map の self-deploy の行。
- 試験:
  - celeris の lib `instance::tests` 29 件（`two_actives_never_coexist_when_the_old_one_is_stale_but_alive`・`a_stale_or_future_promote_lock_is_rejected_even_with_a_live_pid`・
    `a_fresh_matching_marker_authorizes_and_stale_or_foreign_ones_do_not` ほか）、`releases::tests` 21 件
    （`promotion_is_refused_for_unknown_unverified_current_and_running_releases`・`sha12_validation_rejects_path_traversal` ほか）、`releases::audit::tests` 2 件
  - celeris の `--test unpromoted_release` 10 件（`the_binary_exits_four_for_an_unpromoted_release`・`an_unpromoted_release_does_not_migrate_an_old_schema_db`・
    `an_unpromoted_release_does_not_request_a_handoff`・`an_unpromoted_release_without_current_never_creates_the_db` ほか）、`--test instance_handoff` 8 件
  - celeris の `cos_delivery_suite` の `releases_api::` 10 件（`promote_requires_a_token`・`promoting_is_409_when_unverified_already_current_or_already_promoting` ほか）
  - task-api の lib `releases::tests` 2 件（`cos_actor_follows_the_thread_kind_and_refuses_legacy_or_unreadable`・`human_policy_ref_names_the_requested_release`）、
    `browser_eval_surface_suite` の `api_principals::promotion_audit_distinguishes_web_and_external_operator` 1 件
- 件数: 83 passed（80 + 3）、fail 0、ignored 0、skip 0。
- command:
  ```
  cargo nextest run -p celeris -E '(kind(lib) & test(/^(instance|releases)::/)) | binary(unpromoted_release) | binary(instance_handoff) | (binary(cos_delivery_suite) & test(/^releases_api::/))'
  cargo nextest run -p task-api -E '(kind(lib) & test(/^releases::tests::/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::promotion_audit_/))'
  ```

## arch_v4_counterexamples

- 守る条件: D・E・F の反例が再発しない。org の書き込みは plan と apply の 1 本で、失敗すれば org は変わらない。HTTP と CoS の書き込みは adapter が開く 1 つの transaction に入る。
  dispatcher の WU 領域・CoS 領域の判断の file は store・設定に触らず（`store_access_guard`）、状態の owner の struct は試験用の `#[cfg(test)]` field を持たない（`structure_guard`）。
  遅れた完了・lease の再起動・同じ tick の競合・review の権限で二重の書き込みや誤った遷移を作らない。write set の重なる run を同時に走らせない。
- 根拠の file: `agent-docs/adr/2026-11-18-org-write-shared-use-case.md`、`agent-docs/adr/2026-11-18-knowledge-lifecycle-tx-boundary.md`、
  `agent-docs/adr/2026-11-04-project-controller.md` D12（invariants `lifecycle-human-epoch-same-tx`）、`agent-docs/adr/2026-11-18-dispatcher-store-access-pilot.md` D1、
  `agent-docs/adr/2026-11-11-dispatcher-state-ownership.md` D6、`agent-docs/adr/0130-write-set-parallelism-and-behind.md`、
  architecture-map の「組織・案件・報告」「案件の lifecycle」「実行の調整の状態」の行。
- 試験:
  - task-api `cos_delivery_suite` の `org_write_contract::` 6 件（`org_writes_need_an_admin_principal_and_cos_writes_need_the_cos_run`・
    `cos_org_write_failing_inside_the_transaction_leaves_the_org_unchanged`・`http_browser_write_failing_after_the_row_leaves_the_org_unchanged` ほか）
  - task-api `cos_delivery_suite` の `tx_boundary_contract::` 8 件（`busy_store_fails_project_cancel_without_change_and_retry_succeeds`・
    `busy_store_fails_record_without_residue_and_retry_succeeds`・`non_admin_record_and_project_cancel_are_refused_without_writes` ほか）
  - task-dispatch 67 件:
    - `cex_` 40 件: `dispatcher::tests::cex_review_authority` 19・`cex_lease_restart` 12・`cex_late_completion` 6・`cex_same_tick` 3
    - `structure_guard` 14 件: `dispatcher::tests::structure_guard` 6（`store_access_guard_work_units`・`store_access_guard_cos_chat`・`store_access_guard_detects_each_violation` を含む）、
      各 module の `structure_guard_*` 8（cluster_runtime 1・host_maintenance 1・probe_ports 1・project_orchestration 1・routing_runtime 2・execution_coordination 1・run_supervisor 1）
    - `write_set_` 13 件: `dispatcher::tests::write_set_gate` 7・`write_set_record` 3・`dispatch_admission::tests::admit_write_set_hold_wins_over_the_cluster_resolution`・
      `phase_effect_ab::write_set::phase_effect_ab_write_set_gate_avoids_conflict_and_repair`・`execution_coordination::tests::write_set_blockers_come_from_the_running_keys_passed_in`
  - `cex_lease_restart` 12 件と `cex_same_tick` 3 件は lease_atomicity の行でも走らせた。
- 件数: 81 passed（6 + 8 + 67）、fail 0、ignored 0、skip 0。3 つとも exit 0。
- command（task の objective の形のまま）:
  ```
  cargo nextest run -p task-api --test cos_delivery_suite org_write_contract
  cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract
  cargo nextest run -p task-dispatch -E 'test(store_access_guard) | test(structure_guard) | test(cex_) | test(write_set_)'
  ```

## release gate

ここから下は unit record が書いた。release gate は記録を読んだだけで、`release.sh`・`verify.sh`・`promote.sh`・`status.sh` は走らせていない。本番の file・DB・systemd には触れていない。
読んだのは `docs/ops/selfdeploy.md` §0〜§4 が書く記録（`~/.local/celeris/releases/<sha12>/` の manifest.json・gate.json・verify.json・promoted.json・promote_failed.json、
`releases/.build/<sha12>/gate.json`、current と previous の symlink）と、`promote.sh` が書く `/local/celeris/state/logs/promote-*.log` で、読んだのは 2026-11-19。
この節の sha12 はどれも `git merge-base --is-ancestor <sha12> HEAD` が 0（exit 0）だった。

### 読んだ command と出力の要点

- `ls -la ~/.local/celeris/releases/` → release の dir は 4 つ（1680de5cd1b4・1c69c955f0fb・7014d24222c4・bb772ffca8ec）。`releases` は `/local/celeris/state/releases` への symlink。
- `readlink /local/celeris/state/current /local/celeris/state/previous` → `releases/1680de5cd1b4`・`releases/7014d24222c4`（`~/.local/celeris/current`・`previous` はこの 2 つへの symlink）。
- `python3 -I -m json.tool ~/.local/celeris/releases/<sha12>/{manifest,gate,verify,promoted}.json`（4 dir）→ 下の表。
- `ls ~/.local/celeris/releases/*/promote_failed.json` → 無し（exit 2）。
- `ls -lt ~/.local/celeris/releases/.build/`（`/var/lib/celeris/release-build/.build` への symlink）→ gate が落ちた sha の dir は最も新しいもので 2026-11-12。2026-11-17 以降の sha の dir は無い（`tree` は作業ツリー）。
- `grep -E 'release=|current  ->|promoted [0-9a-f]{12}' /local/celeris/state/logs/promote-2026111{7,8,9}-*.log` → 下の昇格の連鎖。`ls /local/celeris/state/logs | grep rollback` → 無し。
- `grep -E '^\s+Summary \[' ~/.local/celeris/releases/<sha12>/gate-logs/cargo-test.log` → nextest の件数。

### B〜G の最後の commit

main の履歴（`git log --first-parent main`）と各 task の進捗ファイルから特定した。D・E・F は task branch に main を取り込む merge の commit の後に main へ入った。祖先の判定には task 自身の最後の統合の commit を使った（後の merge の commit を含む release はこの commit も含む）。

| task | 進捗ファイル | 最後の commit（件名・日時 UTC） | 特定の根拠 |
|---|---|---|---|
| B（01M7MZTKAZQY72EX2R4RQ1WVWF） | `2026-11-17-arch-v4-b.md` | 「integrate wu/closeout (phase closeout)」2026-11-17T21:45:07Z | この commit が B の進捗を done にする。次の commit から C の前提の task |
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | `2026-11-17-arch-v4-c.md` | 「integrate wu/loc-fix (phase fix)」2026-11-18T10:12:48Z | C の進捗と `loc-fix.md` を変える。D の進捗が before を「C の統合の後」のこの件名の commit と書く |
| D（01M7MZTKB4RYM80S6CT3RST2N9） | `2026-11-18-arch-v4-d.md` | 「integrate wu/close (phase close)」2026-11-18T19:55:42Z | D の進捗を done にする。後に「Merge commit '…' into celeris/01M7MZTKB4RYM80S6CT3RST2N9」2026-11-18T20:08:59Z |
| E（01M7MZTKB6GQ241F6JJYSPJNS1） | `2026-11-18-arch-v4-e.md` | 「integrate wu/close (phase close)」2026-11-18T22:21:03Z | E の進捗を done にする。後に「Merge commit '…' into celeris/01M7MZTKB6GQ241F6JJYSPJNS1」2026-11-18T22:31:07Z |
| F（01M7MZTKB8A4NJQEEA35Q4G5JV） | `2026-11-18-arch-v4-f.md` | 「integrate wu/close (phase close)」2026-11-19T04:18:27Z | F の進捗を done にする。後に「Merge commit '…' into celeris/01M7MZTKB8A4NJQEEA35Q4G5JV」2026-11-19T04:30:48Z |
| G（01M7MZTKBKXKEVBWS87PS5J9NK） | `2026-11-19-arch-v4-g.md` | 「integrate wu/close (phase close)」2026-11-19T10:37:13Z | G の進捗を done にする。G fix（01M7SJ2W618PYF6NAY721RJE07）の「integrate wu/fix (phase fix)」2026-11-19T10:28:48Z の後。main の先頭 |

### 昇格の連鎖（2026-11-17〜11-19）と B〜G を含むか

「含む」は `git merge-base --is-ancestor <B〜G の最後の commit> <release の commit>` が 0 のもの。昇格は promote ログの `promoted <sha12> (mode=live)` の行と、残っている release の promoted.json（status succeeded・exit_code 0）で確かめた。
gate・verify の「記録 ok」は gate.json・verify.json の `ok: true`。「未確認」は release の dir も `.build/<sha12>/` も無く、gate.json・verify.json を読めないもの。

| release | release の commit（件名・日時 UTC） | 昇格（UTC、from） | 含む task | gate | verify |
|---|---|---|---|---|---|
| 48b700ede57f | 「integrate wu/closeout (phase closeout)」2026-11-17T21:45:07Z | 2026-11-17T22:28:42Z、from f549b50b7ccc | B | 未確認 | 未確認（promote ログに `verify.live_ok=true`） |
| b2fe0a5e920f | 「repair: PathClass.invariants の進捗に test-affected の結果を書く」2026-11-18T02:25:45Z | 2026-11-18T03:16:41Z | B | 未確認 | 未確認（同） |
| f85df0209504 | 「integrate wu/loc-fix (phase fix)」2026-11-18T10:12:48Z | 2026-11-18T12:18:41Z | B・C | 未確認 | 未確認（同） |
| ed43a4abb7bf | 「integrate wu/fix-test (phase fix)」2026-11-18T13:42:24Z | 2026-11-18T15:25:24Z | B・C | 未確認 | 未確認（同） |
| 3033e0228853 | 「Merge commit '…' into celeris/01M7Q70NDQAK1KKXHY6WT027QZ」2026-11-18T16:18:10Z | 2026-11-18T17:17:11Z | B・C | 未確認 | 未確認（同） |
| eb1244bb5e8d | 「Merge commit '…' into celeris/01M7MZTKB4RYM80S6CT3RST2N9」2026-11-18T20:08:59Z | 2026-11-18T21:10:04Z | B・C・D | 未確認 | 未確認（同） |
| 1c69c955f0fb | 「Merge commit '…' into celeris/01M7MZTKB6GQ241F6JJYSPJNS1」2026-11-18T22:31:07Z | 2026-11-18T23:25:35Z、from eb1244bb5e8d | B〜E | 記録 ok（nextest 8515 passed） | 記録 ok |
| bb772ffca8ec | 「進捗: rejected-worker-decisions」2026-11-19T00:08:00Z | 2026-11-19T01:25:29Z、from 1c69c955f0fb | B〜E | 記録 ok（nextest 8517 passed） | 記録 ok |
| 7014d24222c4 | 「Merge commit '…' into celeris/01M7MZTKB8A4NJQEEA35Q4G5JV」2026-11-19T04:30:48Z | 2026-11-19T05:35:55Z、from bb772ffca8ec | B〜F | 記録 ok（nextest 8525 passed） | 記録 ok |
| 1680de5cd1b4 | 「integrate wu/close (phase close)」2026-11-19T10:37:13Z | 2026-11-19T11:30:18Z、from 7014d24222c4。いまの current | B〜G | 記録 ok（nextest 8525 passed） | 記録 ok |

残っている 4 release の記録の中身（4 つとも同じ形）:

- manifest.json: `gate_ok: true`、`schema_version: 126`、web の 7 段が ok、browser_ledger ok。
- gate.json: `ok: true`・`failed_step` 空。17 段（cargo-workspace-clean・cargo-fmt-check・control-ts-install・control-ts-bundle・script-bundle-verify・cargo-test・
  script-bundle-tests・cargo-clippy・source-size-report・cargo-build・web の 7 段）がどれも exit 0。cargo_test は `nextest_exit 0`・`doctest_exit 0`・`failed 0`・`userns: true`。
  nextest の件数（gate-logs/cargo-test.log の Summary 行）: 1c69c955f0fb 8515 passed・bb772ffca8ec 8517 passed・7014d24222c4 と 1680de5cd1b4 は 8525 passed、どれも 22 skipped・fail 0。
- verify.json: `ok: true`・`live_ok: true`。検査 1（start-and-migrate）・2（counts-match）・3（main-gets）・5（n-1-compat）・6（smoke）・4d（web-app-start）の 6 件がどれも ok。
- promoted.json: `status: succeeded`・`exit_code 0`・`mode: live`。actor は 4 件とも `external_operator:claude-code-ops-session`、`policy_ref: memory:promote-without-asking`。
  `included.tasks` は 1c69c955f0fb が E（01M7MZTKB6GQ241F6JJYSPJNS1）、bb772ffca8ec が 01M7RFVV60RQTDKYEWAEH4YYGT、7014d24222c4 が F（01M7MZTKB8A4NJQEEA35Q4G5JV）、1680de5cd1b4 が G（01M7MZTKBKXKEVBWS87PS5J9NK）。

### B〜G ごとの結果

| task | 最初に含んだ release | その release の gate | verify | 昇格 | いまの current（1680de5cd1b4） |
|---|---|---|---|---|---|
| B | 48b700ede57f（「integrate wu/closeout (phase closeout)」2026-11-17） | 未確認 | 未確認 | した（2026-11-17T22:28:42Z） | 含む。gate ok・verify ok・昇格済み |
| C | f85df0209504（「integrate wu/loc-fix (phase fix)」2026-11-18） | 未確認 | 未確認 | した（2026-11-18T12:18:41Z） | 含む。同上 |
| D | eb1244bb5e8d（D の branch への merge、2026-11-18） | 未確認 | 未確認 | した（2026-11-18T21:10:04Z） | 含む。同上 |
| E | 1c69c955f0fb（E の branch への merge、2026-11-18） | ok | ok | した（2026-11-18T23:25:35Z） | 含む。同上 |
| F | 7014d24222c4（F の branch への merge、2026-11-19） | ok | ok | した（2026-11-19T05:35:55Z） | 含む。同上 |
| G | 1680de5cd1b4（「integrate wu/close (phase close)」2026-11-19） | ok | ok | した（2026-11-19T11:30:18Z） | この release 自身 |

B・C・D の最初の release の gate・verify を「未確認」とした理由: `~/.local/celeris/releases/<sha12>/` と `releases/.build/<sha12>/` がどちらも無く、gate.json・verify.json を読めない。
release.sh の掃除が current・previous・検証済みの新しい 3 件だけを残す（`scripts/selfdeploy/release.sh` の「掃除」の節）ので、その後の release を作ったときに消えたとみられる。
ただし、次の 3 つから gate と verify は ok だったと推定できる（記録そのものは読めていない）:
promote ログに 3 件とも `promoted <sha12> (mode=live)` があり、`promote.sh` は verify.json の `ok` が真でなければ止まる（同じ script の「is not ok — refusing to promote」）。
`verify.sh` は gate.json の `ok` が真でなければ止まる（「is not ok; refusing to verify」）。どちらの検査も 3 つの release の commit の版の script にある（`git show <commit>:scripts/selfdeploy/promote.sh`）。
`release.sh` は gate が落ちると release の dir を作らない（`docs/ops/selfdeploy.md` §2）。
B・C・D は後の 4 release（1c69c955f0fb 以降）にも含まれ、その gate・verify・昇格は記録で確かめた。

人が確かめる読み取りだけの command（B・C・D の最初の release）:

```
ls -la ~/.local/celeris/releases/48b700ede57f ~/.local/celeris/releases/f85df0209504 ~/.local/celeris/releases/eb1244bb5e8d ~/.local/celeris/releases/.build/
grep -vE 'preview:' /local/celeris/state/logs/promote-20261117-222401.log /local/celeris/state/logs/promote-20261118-121753.log /local/celeris/state/logs/promote-20261118-210838.log
git -C ~/workspace/agent-platform show 48b700ede57f:scripts/selfdeploy/promote.sh | grep -n 'refusing to promote'
```

gate.json の写しが別の場所（`/local/celeris/state/backups`・release を作った run の成果物）に残っているかは調べていない（未確認）。

### 気づいた点（直していない）

- gate.json の `cargo_test.passed` は 4 release とも 3、`ignored` は 24 で、nextest の通過数（8515〜8525）を含まない。値は doc test の通過数 3（cargo-test.log の `test result:` 14 行の和）と、
  nextest の skipped 22 と doc test の ignored 2 の和に一致する。nextest の件数は gate-logs/cargo-test.log の Summary 行にだけある。
- gate.json の `cargo_test.userns` は 4 release とも true。上の 9 件（userns の env が無いと return する試験）が release gate で検査まで進んだかは、cargo-test.log が通過した試験を 1 件ずつ出さないので未確認。

## 提案

- release.sh の gate.json の `cargo_test` に nextest の passed・skipped を入れる（いまの `passed`・`ignored` は doc test の値と nextest の skipped の和に見える）。読む側（release の記録を読む検査・報告）が gate log を grep しなくて済む。
- release の掃除で消える gate.json・verify.json・promoted.json の要約を、`/local/celeris/state/logs` の promote ログのように消えない場所へ 1 行ずつ残す。B・C・D のように後から gate・verify を記録で確かめられない release を無くす。
