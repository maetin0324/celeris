---
title: 健全性v2 R5 old-impl — 消した仕組み・旧い API や挙動を固定する試験の候補
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 old-impl — 消した仕組み・旧い API や挙動を固定する試験の候補

完了日: 2026-11-10

WorkUnit `old-impl`。task-api・celeris・celerisctl の試験から old_impl の候補を 3 件挙げ、
[`candidates/old-impl.json`](candidates/old-impl.json) に書いた（形は [`contracts.py`](contracts.py) の `check_old_impl`、手順は [`fill-guide.md`](fill-guide.md) 5 節）。
試験・`contracts.py`・crates/ は変えていない。基準は `$CELERIS_WU_BASE`。

## 結論
- 採った候補は 3 件、member は 4 試験。どれも `invariant=false`。replay の契約の試験と、消した欄・入口を拒否する今の契約の試験は入れていない。
- 走査した 1864 試験のうち 874 件が `invariant=true` で、候補にできない。消した仕組みを固定している試験の大きな塊（celeris の `tests/notify.rs`）がこの側にある（「提案」の 1）。

| id | members | 根拠 | 案 | mutation_target |
|---|---|---|---|---|
| `old-incremental-backup-measurement` | `db_maintenance/tests.rs::incremental_backup_progress_restarts_after_other_connection_writes` | 件名「定期DBバックアップの再開始ループと中断遅延を解消」 | delete | `crates/celeris/src/db_maintenance.rs` |
| `old-celerisctl-plan-legacy-path` | `plan.rs::tests::run_inserts_plan_task`・`run_with_blank_goal_returns_error` | ADR-0079（`POST /plans` は R5a で 410） | delete | `crates/celerisctl/src/commands/execution.rs` |
| `old-console-milestone-review-block` | `tests/console.rs::approvals_milestones_and_questions_carry_their_state` | ADR-0079 R5a 9（Console の `milestone_review` は R5b/R6 の回収で外す） | merge | `crates/task-api/src/console.rs` |

各候補の理由:
1. `old-incremental-backup-measurement`: 試験は rusqlite の `Backup::step(1)` が別接続の書き込みで最初からやり直すことを計測するだけで、celeris の関数を呼ばない。本体の `backup_once` は `VACUUM INTO` に置き換え済み（`db_maintenance.rs` の注釈「The old incremental backup can restart …」）。残す試験は VACUUM の並行書き込み（`vacuum_backup_finishes_with_a_concurrent_writer_and_integrity_ok`）と中断（`vacuum_progress_handler_interrupts_an_active_copy`）。
2. `old-celerisctl-plan-legacy-path`: `celerisctl plan` は DB に直接 `kind = plan` の分解 task を作る。stderr の注記 `ADR_0079_NOTE` が自ら「the legacy path (POST /plans is 410 since R5a)」と書く。破壊的変更の方針（ADR 2026-10-11）は旧い入口を残さないので、コマンドを消す変更で 2 試験も消す。新しい経路は `execution plan set`（plan/3）で、`execution_tests.rs` の `set_adopts_a_v3_plan_with_the_configured_tree_limits`・`set_rejects_an_invalid_plan` が守る。
3. `old-console-milestone-review-block`: Console は `proposed` の途中目標を `ConsoleBlock::Milestone` として出し、`task_ops::milestone_review::review_state` の返事を添える。ADR-0038 の判定の対話は ADR-0079 で廃止され、R5a 9 はこの読み手を「R5b / R6 の回収で外す」と書いた。試験は認可・質問のブロックも確かめるので、消さずに milestone の assert だけ外す（action は merge）。凍結した途中目標を読む契約は `?include_frozen=true` の GET で、`project_plan.rs::project_detail_hides_frozen_milestones_by_default` が守る。

## 調べた ADR
`agent-docs/adr/2026-10-11-breaking-changes-by-default.md` 以後の 222 本から、本文に「消す・廃止・置き換え・削除・撤去」が 3 回以上ある 44 本を読み、消した名前を target 3 crate で git grep した。主に読んだもの:
- `2026-10-11-llm-source-harness-selection.md`（provider・`providers.d`・`tier_models`・`worker_hint.adapter`・`in_use_cos`・`adapter_policy = model_family`）
- `2026-10-11-llm-source-model-routing-evaluation.md` D4（`[model_routing]`・estimator・shadow）、`2026-10-11-task-routing-features.md` D10（`routing export`・`routing_replay`）
- `2026-10-11-task-wait-reasons.md` D4・D6（`DaemonSnapshot.unroutable` ほか 3 欄）
- `2026-10-22-cos-fixed-threads.md` D7（`legacy_thread()`・`console.new_conversation`・受信箱スレッド）
- `2026-10-15-root-done-cos-summary.md` D7・D8（`TaskReady`・`notify::scan` の死んだ経路）
- `2026-10-16-project-auto-integrate.md`（`delivery_projects`）、`2026-10-20-continuation-limit-by-progress.md`（`max_continuations_per_work_unit`）
- `2026-10-14-eval-trial-scratch-target.md`（`/var/tmp/agent-platform-build`）、`2026-10-27-api-principals.md`（`[api] token_file`）
- `2026-11-04-project-controller.md`、`2026-11-02-ts-control-layer-boundary.md`、`2026-11-05-phase4-p4d-retention.md`、`2026-10-30-ops-r2-isolation-principal.md`、`2026-10-11-retire-gui.md`
- 置き換え済みと付記した ADR: `0028-genre-manifest.md`・`0038-milestone-review-dialogue.md`・`0044-task-management.md`・`0048-console.md`・`0051-supervised-delivery.md`・`0074-…`・`0077-…`・`0121-…`、と `0079-recursive-task-decomposition.md` の R5a 9（読み取りとして残したもの）

## 調べた commit の件名
`git log --format=%s HEAD` のうち「消す・削除・撤去・廃止・置き換え・remove・drop・retire・legacy・旧」を含む件名は 292 件あり、新しい順の 150 件を見た。target 3 crate に関わる主なもの:
- 「wu/rm-core: [model_routing] と config の旧経路・RoutingCatalog・task-core model_router の旧部分を 1 つの変更で消す」
- 「wu/rm-dispatch: dispatcher の routing_shadow・routing_enforce・tier binding の解決を消す」
- 「wu/hist: routing_shadow_recorded の履歴型を読み取り専用 module へ移し、旧形式 JSON fixture 試験を置く」
- 「wu/prune-select: task-dispatch の旧 provider 選択器（provider_select.rs）を消す」
- 「wu/api-tests: task-api の結合試験を新路に書き直し旧 accounts_admin.rs を消す」
- 「wu/run-selection: runs 表の選択欄を task detail の runs に selection として出す（旧 run は legacy_unresolved）」
- 「CoS の旧スレッド経路を消す: Console の入力は固定スレッドへ、legacy は読むだけ」
- 「案件の状態の要約を構造化して永続化し、secretary_summary を消す」
- 「config: celeris config を USD 欄にし旧欄を読み込みエラーにする（max_usd_per_day・max_tree_usd・budget_rates）」
- 「phase impl-b: continuation の上限を進捗で判定する（固い上限 continuation_hard_limit、旧欄は読み込みエラー）」
- 「定期DBバックアップの再開始ループと中断遅延を解消」

`git grep` で target 3 crate に名前が残っていないと確かめたもの: `in_use_cos`・`new_conversation`・`routing_replay`・`model_assignments`・`tier_models`・`TierModels`・`routing_enforce`・`RoutingCatalog`・`HeuristicEstimator`・`enforce_routes`・`dependency_reopen`・`/var/tmp/agent-platform-build`。

## 採らなかった候補と理由
replay の契約（DB に残る旧い shape の event・行を読む）:
- `celeris/tests/legacy_event_startup.rs::startup_loads_legacy_routing_decided_event`、`task-api/tests/legacy_routing_events.rs::accounts_list_survives_legacy_routing_events_without_evidence_mode`
- `task-api/tests/routing.rs::routing_shadow_audit_keeps_primary_outcome_separate`（消した `routing_shadow_recorded` の event を読む）
- `task-api/tests/llm_routes_admin.rs::run_selection::task_runs_expose_persisted_selection_and_legacy_unresolved`（旧 run の行）
- `task-api/tests/chat_api.rs::chat_api_legacy_thread_is_read_only`（`kind='legacy'` の行は読むだけ）
- `celeris/src/daemon/api_chat_tests.rs::chat_wiring_daemon_store_open_runs_legacy_migration`（旧 Console の行の migration）
- `celeris/tests/cos_chat_triage_notify.rs` の `cos_chat_triage_cutover_supersedes_legacy_pending_and_is_idempotent`・`cos_chat_triage_only_escalation_and_fallback_are_sendable`、`celeris/tests/cos_chat_triage_scenarios.rs::cos_chat_triage_unified_only_cos_escalation_reaches_the_webhook`（旧 kind の未送信行を送らない）
- `task-api/tests/project_roadmap.rs::project_roadmap_ignores_task_milestone_and_phase`（凍結の履歴の `milestone_id` を持つ root）
- `celerisctl/src/commands/scratch_tests.rs::gc_dry_run_lists_without_removing`、`task-api/src/handlers/tests.rs::metrics_scratch_matches_the_status_schema`（disk に残る旧い置き場を片付ける）

今の契約（消した欄・入口を拒否する、または今も使う名前）:
- 消した欄の読み込みエラー: `celeris/src/config/project_controller.rs::tests::legacy_token_cap_is_rejected_on_load`、`celeris/src/config/script.rs::tests::config_active_bundle_sha256_is_rejected`
- 消した入口の 410・不在: `task-api/tests/project_plan.rs::project_plan_endpoints_are_gone`、`task-api/tests/project_roadmap.rs::project_roadmap_legacy_milestones_stay_gone`、`celerisctl/src/commands/projects.rs::tests::projects_plan_subcommand_is_removed`、`task-api/tests/task_wait_api.rs` の `awaiting_children` の不在の assert
- 今の仕組み: `task-api/tests/long_horizon_pilot.rs::long_horizon_pilot_api_off_restores_the_legacy_cos_launch`（案件 controller の mode `off`）、`celeris/src/daemon/routing_guardrail_tests.rs`・`celeris/tests/long_run_soak.rs::long_run_soak_shadow`（今の `[routing] mode` の shadow/enforce）、`celeris/tests/project_cos_shadow.rs`（TS の案件 decider）
- 今の名前: `celerisctl/src/commands/worker_tests.rs::normalize_outcome_maps_adapter_errors_to_provider_failure`・`celerisctl/tests/worker_run.rs::error_with_provider_failure_exits_four_and_preserves_provider_failure`（`provider_failure` は今の worker protocol の欄）、`celerisctl/src/commands/execution_tests.rs::tree_adopt_needs_the_tree_and_put_is_an_alias_of_set`（`put` は ADR-0079 R5b-prep が置いた別名）
- 今の道具: `celerisctl/tests/org_migrate_v2.rs::config_to_harnesses_prints_the_new_shape_from_the_legacy_one`（`[[roles]]`・`[[genres]]` は今も読める設定で、変換器は ADR-0046 D3 の現行の道具）
- 意図して残す欄: `task-api/tests/routing_source_state.rs::routing_source_api_reports_freshness_and_cost_components` の「旧欄は残る」（`last_hour_requests`・`reachable` を消す ADR は無い）
- 消すと決めていない: `celeris/src/db_maintenance` 以外の計測試験（`tick_scale_baseline`・`script_ops_measure_tests`）は今の実装を測る

`invariant=true` で候補にできないもの:
- `celeris/tests/notify.rs` の 34 試験（`notify::scan`・`notify::schedule`・`notify::select_batch` を呼ぶ helper を使う）。この 3 関数は daemon から呼ばれず（daemon は `select_routes_batch` だけを使う）、ADR 2026-10-15-root-done-cos-summary D7・D8 が「死んだ経路として残してある」と書く。architecture-map の L122・L154 が file ごと関連試験に挙げるので `invariant=true` になる。
- `celeris/src/config/tests.rs::coding_harness_default_config_rejects_fixed_adapter_and_unknown_policy`。`adapter_policy = model_family` は ADR 2026-10-11-llm-source-harness-selection が廃止すると書き、今は config の検証だけが読む（dispatch は読まない）。architecture-map L146 が file ごと挙げる。
- `celeris/tests/script_runtime.rs::script_runtime_daemon_shutdown_count_skips_unreaped_actor`（修正前の数え方を前提の確認に使う。不具合の再現で、旧実装の固定ではない）

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --scan > "$TMPDIR/scan.json"` → `tests=1864 excluded=2 git_grep_lines=1866 match=yes`、`invariant=true` は 874 件
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check-fragment agent-docs/progress/2026-11-10-health-v2-r5/candidates/old-impl.json` → `ok`、exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は `candidates/old-impl.json` と `old-impl.md` の 2 つだけ、exit 0
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`、exit 0
- `sh scripts/dev/progress-index.sh --check` → `ok`、exit 0

## commit
- 「wu/old-impl: old_impl の候補 3 件（旧 backup 計測・celerisctl plan・Console の途中目標）と進捗」

## 未解決事項
- 無し。

## 提案
1. architecture-map の関連試験の列が `celeris/tests/notify.rs` と `celeris/src/config/tests.rs` を file ごと挙げるので、死んだ経路（`notify::scan`・`schedule`・`select_batch`）や廃止した `adapter_policy` を固定する試験も `invariant=true` になり、old_impl にできない。地図の行を試験関数の単位に絞るか、死んだ経路を消す task を別に起こすことを提案する（どちらも R5 の write set の外）。
2. `celerisctl projects show` は `store.milestone_list` で凍結した途中目標を出し続ける（試験 `run_show_reads_the_project_and_its_milestones` は出力を確かめないので候補にしていない）。Console の途中目標の節を消すときに同じ変更で外すとよい。
