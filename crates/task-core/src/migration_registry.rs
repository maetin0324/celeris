//! migration の登録（ADR-0013 D5）: 版数と SQL 本体の組の表 [`MIGRATIONS`] と、飛ばす版数
//! [`RESERVED_VERSIONS`]。版数から SQL を引く [`migration_sql`] は表を引くだけ。適用の処理と
//! `SCHEMA_VERSION` は `store/migrations.rs` が持つ（`scripts/selfdeploy/lib.sh` の
//! `sd_schema_version_of_tree` がその file の `SCHEMA_VERSION` を読む）。
//! 新しい migration は `migrations/NNNN_*.sql` を足し、この表に 1 行足し、`SCHEMA_VERSION` を上げる。

/// 版数と SQL 本体（`include_str!`）の組。版数の昇順で、版数は重複しない。
pub(crate) const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_events_global_id.sql")),
    (3, include_str!("../migrations/0003_tasks_list_columns.sql")),
    (
        4,
        include_str!("../migrations/0004_tasks_objective_column.sql"),
    ),
    (5, include_str!("../migrations/0005_tasks_genre_column.sql")),
    (6, include_str!("../migrations/0006_organization.sql")),
    (
        7,
        include_str!("../migrations/0007_messages_task_id_and_reports_project.sql"),
    ),
    (8, include_str!("../migrations/0008_notifications.sql")),
    (
        9,
        include_str!("../migrations/0009_notifications_project_id.sql"),
    ),
    (
        10,
        include_str!("../migrations/0010_projects_workspace.sql"),
    ),
    (11, include_str!("../migrations/0011_daemon_instances.sql")),
    // ADR-0043 D1/D2（Phase 52）: `project_repos` と `tasks.repos_json`。
    // この版を当てた直後に、同じトランザクションで `backfill_project_repos` が写しを作る。
    (12, include_str!("../migrations/0012_project_repos.sql")),
    // ADR-0044 D2/D3（Phase 53）: `task_comments` と `tasks.labels_json` / `tasks.category`。
    (13, include_str!("../migrations/0013_task_comments.sql")),
    // ADR-0043 D5（Phase 54）: `task_integrations`（取り込みの記録）。
    (14, include_str!("../migrations/0014_task_integrations.sql")),
    // ADR-0044 D6（Phase 55）: 案件・途中目標の中止・一時停止・アーカイブ（`archived_at` / `paused_from`）。
    (15, include_str!("../migrations/0015_lifecycle.sql")),
    // ADR-0046 D1/D2/D4（Phase 59）: `org_nodes.profile_json` と `tasks.skills_json` / `tasks.mode`。
    (16, include_str!("../migrations/0016_org_profiles.sql")),
    (17, include_str!("../migrations/0017_console_actions.sql")),
    // ADR-0047 D4（Phase 62）: `knowledge_runs`（知識整理 run を高々 1 回だけ起こすための目印）。
    (18, include_str!("../migrations/0018_knowledge_runs.sql")),
    (19, include_str!("../migrations/0019_notification_scan.sql")),
    (20, include_str!("../migrations/0020_deliveries.sql")),
    // ADR-0052 D2 / D3（Phase 64）: `knowledge_runs.retried_at` と `knowledge_runs.via`。
    (
        21,
        include_str!("../migrations/0021_knowledge_run_retry.sql"),
    ),
    // ADR-0053 D1（Phase 65）: `llm_proxy_requests`（LLM source ローカルプロキシの要求記録）。
    (
        22,
        include_str!("../migrations/0022_llm_proxy_requests.sql"),
    ),
    // ADR-0054 D1（Phase 67）: `node_sessions`（ノードごとの継続セッションと resume）。
    (23, include_str!("../migrations/0023_node_sessions.sql")),
    // ADR-0056 D1 / D4（Phase 78）: `mcp_clients` / `mcp_calls`（MCP サーバーの認証とログ）。
    (24, include_str!("../migrations/0024_mcp.sql")),
    // ADR-0059 D6（Phase 99）: `cluster_settings`（クラスタの作業ディレクトリの DB 上書き）。
    (25, include_str!("../migrations/0025_cluster_settings.sql")),
    // ADR-0072 D5/D23（Phase E2）: `execution_plans` / `work_units` / `runs`（Task 下の内部実行層の
    // 派生索引。正本は events。`CREATE TABLE IF NOT EXISTS` だけで既存の表には触れない）。
    (26, include_str!("../migrations/0026_execution.sql")),
    // ADR-0074 D1/§5.2（Phase F2）: `work_units` に v2（並列実行）の phase/lease/branch/commit 列を足す。
    (
        27,
        include_str!("../migrations/0027_parallel_work_units.sql"),
    ),
    // ADR-0074 D3.2 / D3.8（Phase F4b）: `projects.auto_advance` と `milestones.plan_key`。
    (28, include_str!("../migrations/0028_project_plan_go.sql")),
    // ADR-0044 D7 追記 / ADR-0047 追記（Phase K-1）: `projects.slug`（知識ベースの `projects/<slug>/`）。
    (29, include_str!("../migrations/0029_project_slug.sql")),
    // ADR-0078 D5: `cluster_connection_log`（クラスタの ssh master の接続・切断の記録）。
    (
        30,
        include_str!("../migrations/0030_cluster_connection_log.sql"),
    ),
    // ADR-0079 D4 / D7 / D15（Phase R1a）: `tasks.root_id`・`work_units.child_task_id` /
    // `needs_decisions_json`・`decisions`（再帰的な task の木。既存の行は書き換えない）。
    (31, include_str!("../migrations/0031_task_tree.sql")),
    // ADR-0080 D4/D5: browser の人待ち（登録依頼・承認）と credential 台帳（秘密なし）。
    (32, include_str!("../migrations/0032_browser_waits.sql")),
    (
        33,
        include_str!("../migrations/0033_browser_task_policies.sql"),
    ),
    // ADR-0090 D2: `cluster_job_waits`（クラスタ job の durable wait。events が正本の派生の索引）。
    (34, include_str!("../migrations/0034_cluster_job_waits.sql")),
    // browser Phase 3: control lease・live proxy・identity の store（ブランチの 0034 を main の後へ振り直し）。
    (
        35,
        include_str!("../migrations/0035_browser_phase3_store.sql"),
    ),
    // browser Phase 4: trusted login（ブランチの 0035 を振り直し）。
    (
        36,
        include_str!("../migrations/0036_browser_trusted_login.sql"),
    ),
    // ADR-0121 付記: `idx_events_delivery_skipped`（受信箱の delivery_skipped 走査を絞る部分 index）。
    (
        37,
        include_str!("../migrations/0037_events_delivery_skipped_index.sql"),
    ),
    // ADR-0133 D3.2: `feed_notices` / `feed_sources` / `feed_cursor`（通知の既読と束ね）。
    (41, include_str!("../migrations/0041_feed_notices.sql")),
    // ADR-0118 D3: delivery の検査対象と merge candidate を記録する。
    // ブランチでは 0037 だったが、main の `0037_events_delivery_skipped_index`（本番適用済み）と重複したので
    // review sync で空き番号 42 へ振り直した。列を足すだけで 0038〜0041 とは依存しない。
    (
        42,
        include_str!("../migrations/0042_review_target_sync.sql"),
    ),
    // ADR-0140 D2: `node_sessions` に execute continuation の WU 単位 session の列を足す。
    // ブランチでは 0038 だったが、main の 0038〜0040 は未使用のまま空いていなかったため
    // （他の celeris/* ブランチが 0038〜0040 を使用中）、mig-renumber で 0043 へ振り直した。
    (
        43,
        include_str!("../migrations/0043_work_unit_sessions.sql"),
    ),
    // ブランチでは 0039 だったが、同じ理由で 0044 へ振り直した。
    (44, include_str!("../migrations/0044_write_sets.sql")),
    // ADR-0130 D4: task branch の target からの behind snapshot。
    // ブランチでは 0040 だったが、同じ理由で 0045 へ振り直した。
    (45, include_str!("../migrations/0045_behind_targets.sql")),
    // ADR-0131 D1: 定期実行（`cron_jobs`）と実行履歴（`cron_job_runs`）。
    // （ブランチの 0039 を main の 0041 と並列開発 task の 0042〜0045 の後へ振り直し。）
    (46, include_str!("../migrations/0046_cron_jobs.sql")),
    // ADR parallel integration D4: 受信箱の統合依頼 event を絞る index。
    (
        47,
        include_str!("../migrations/0047_events_integration_request_index.sql"),
    ),
    // ADR 2026-10-04-multi-objective-model-routing Phase 2: `llm_proxy_requests` に routing の相関欄
    // （decision / snapshot / run / task / source / model）と、決定の索引を足す（additive）。
    (
        48,
        include_str!("../migrations/0048_routing_log_correlation.sql"),
    ),
    // ADR 2026-10-04-multi-objective-model-routing Phase 4: 実行 shadow の UTC 日次上限の共有予約（additive）。
    (
        49,
        include_str!("../migrations/0049_routing_shadow_budget.sql"),
    ),
    // CoS chat D1: durable conversation, attachments, operations, and FTS.
    (50, include_str!("../migrations/0050_cos_chat.sql")),
    (
        51,
        include_str!("../migrations/0051_org_browser_events.sql"),
    ),
    // ADR 2026-10-06 D4: モデル catalog・上書き・発見記録（additive）。
    (52, include_str!("../migrations/0052_model_catalog.sql")),
    // ADR 2026-10-06 model-role-assignments D1: source × 役割 → model_id の割り当て表（additive）。
    (
        53,
        include_str!("../migrations/0053_model_role_assignments.sql"),
    ),
    (
        54,
        include_str!("../migrations/0054_model_role_memberships.sql"),
    ),
    // CoS D3: hash-only, expiring credentials scoped to a chat run.
    (
        55,
        include_str!("../migrations/0055_cos_run_credentials.sql"),
    ),
    // CoS D6: old notification provenance at route cutover.
    (56, include_str!("../migrations/0056_cos_triage.sql")),
    // ADR 2026-10-07-browser-trusted-devices D2: 信頼できる端末（hash のみ）。
    (
        57,
        include_str!("../migrations/0057_browser_trusted_devices.sql"),
    ),
    // ADR 2026-10-07-build-tmp-hygiene D4.4: ディスク使用率の監視の状態。
    (58, include_str!("../migrations/0058_disk_watch_state.sql")),
    // ADR 2026-10-07-cos-inbox-thread-conversation D1: 受信箱の件の題名。
    (
        59,
        include_str!("../migrations/0059_cos_inbox_item_summary.sql"),
    ),
    // ADR 2026-10-08-browser-prod-enablement D3: site policy の DB 正本と変更の監査。
    (
        60,
        include_str!("../migrations/0060_browser_site_policies.sql"),
    ),
    (61, include_str!("../migrations/0061_cos_chat_usage.sql")),
    // ADR 2026-10-05-cos-chat-home 付記: rollover の context 占有と累積の課金相当入力。
    (
        62,
        include_str!("../migrations/0062_cos_chat_context_tokens.sql"),
    ),
    // ADR 2026-10-05-cos-chat-home 付記: resume 時の差分配送の cursor。
    (
        63,
        include_str!("../migrations/0063_cos_chat_delivered_through.sql"),
    ),
    // ADR 2026-10-09 credential username / post-login: site policy の username selector と post_login。
    (
        64,
        include_str!("../migrations/0064_browser_site_policy_post_login.sql"),
    ),
    // ADR 2026-10-09 credential username / post-login 付記 2026-10-10b: 同意頁の固定ボタン。
    (
        65,
        include_str!("../migrations/0065_browser_site_policy_consent.sql"),
    ),
    // ADR 2026-10-08-account-concurrency-limits: アカウントごとの同時実行上限（正本設定）。
    (66, include_str!("../migrations/0066_account_limits.sql")),
    (
        67,
        include_str!("../migrations/0067_task_repo_integration_states.sql"),
    ),
    (
        70,
        include_str!("../migrations/0070_task_integration_not_applicable.sql"),
    ),
    (71, include_str!("../migrations/0071_llm_route_store.sql")),
    (
        72,
        include_str!("../migrations/0072_drop_model_role_assignments.sql"),
    ),
    (73, include_str!("../migrations/0073_route_records.sql")),
    (
        74,
        include_str!("../migrations/0074_routing_evidence_stage.sql"),
    ),
    (
        75,
        include_str!("../migrations/0075_drop_routing_shadow_reservations.sql"),
    ),
    // 付記 2026-10-12（eval-dispatch）: 評価 run の試行。
    (76, include_str!("../migrations/0076_eval_trials.sql")),
    (
        77,
        include_str!("../migrations/0077_routing_evidence_eval_columns.sql"),
    ),
    // ADR 2026-10-14-eval-verify-isolation D3.1: 評価試行の持ち主の instance。
    (
        78,
        include_str!("../migrations/0078_eval_trials_instance_id.sql"),
    ),
    // ADR 2026-10-14-local-machine-settings D2: `local_settings`・`task_workspace_roots`・`local_settings_events`。
    // ADR 2026-10-14-local-machine-settings D2: ローカルマシン設定と task ごとの root 固定。
    (79, include_str!("../migrations/0079_local_settings.sql")),
    (
        80,
        include_str!("../migrations/0080_root_done_summaries.sql"),
    ),
    // ADR 2026-10-15-mlsys-min-decision-assisted D4: credential 待ち（`credential_waits`・追記専用の `credential_wait_events`）。
    (81, include_str!("../migrations/0081_credential_waits.sql")),
    // ADR 2026-10-15-decision-log-export: 判断・実行・成果・消費の最小一貫ログ。
    (82, include_str!("../migrations/0082_decision_log.sql")),
    // ADR 2026-10-16-project-auto-integrate D1: `projects.auto_integrate`。
    (
        83,
        include_str!("../migrations/0083_project_auto_integrate.sql"),
    ),
    // ADR 2026-10-16-mlsys-measure-task-routing D2: task 単位の routing 固定（`task_routing_overrides`）。
    (
        84,
        include_str!("../migrations/0084_task_routing_overrides.sql"),
    ),
    // ADR 2026-10-16-campaign-identity-propagation: `task_campaigns` と決定ログ run 行の識別列。
    (85, include_str!("../migrations/0085_task_campaigns.sql")),
    // ADR 2026-10-16-metering-attribution: 消費行の帰属規則・共有オーバーヘッド。
    (86, include_str!("../migrations/0086_metering.sql")),
    // 分類段・lane 各段の記録（classifier_status / call_id / fixed・jev・constrained lane / basis）。
    (
        87,
        include_str!("../migrations/0087_decision_log_lane_stages.sql"),
    ),
    // campaign 識別の variant 参照と構成 pin（`task_campaigns`・決定ログ run 行）。
    (
        88,
        include_str!("../migrations/0088_task_campaign_variant.sql"),
    ),
    // ADR 2026-10-20-replan-dep-change-wu-rebase D2: `work_units.rebase_pending`。
    (
        89,
        include_str!("../migrations/0089_work_unit_rebase_pending.sql"),
    ),
    // ADR 2026-10-22-harness-balance-and-lane-first-routing: 完了単位の根拠表・harness 切替の guardrail。
    (
        90,
        include_str!("../migrations/0090_completion_outcomes.sql"),
    ),
    // ADR 2026-10-22-dynamic-config D4: `config_events`（config.toml の欄の変更履歴）。
    (91, include_str!("../migrations/0091_config_events.sql")),
    // ADR 2026-10-22-cos-fixed-threads: 統括 CoS 1 本と案件ごとの CoS 1 本に固定し、既読の水位を足す。
    (92, include_str!("../migrations/0092_cos_fixed_threads.sql")),
    // ADR 2026-10-23-mcp-read-only-db-and-api D6: `mcp_calls.request`（読み取りの道具の要求の監査）。
    (93, include_str!("../migrations/0093_mcp_call_request.sql")),
    (
        94,
        include_str!("../migrations/0094_task_idempotency_keys.sql"),
    ),
    // ADR 2026-10-27-project-brief D1/D2: `project_briefs`（案件の状態の要約）を足し、`projects.secretary_summary` を消す。
    (95, include_str!("../migrations/0095_project_briefs.sql")),
    // ADR 2026-10-27-unblocks-reopen-on-promotion D1: `task_unblocks`（修正 task と待っている task の結び）。
    (96, include_str!("../migrations/0096_task_unblocks.sql")),
    // ADR 2026-10-27-event-principals D3: `task_comments.author_kind` に cos・mcp・operator を足し、誤記録の行を author から直す。
    (
        97,
        include_str!("../migrations/0097_comment_author_kinds.sql"),
    ),
    // ADR 2026-10-27-decision-log-intervention-actors D1: `decision_log_interventions` に主体（`actor_kind`・`actor_id`）。
    (
        98,
        include_str!("../migrations/0098_decision_log_intervention_actors.sql"),
    ),
    // ADR 2026-10-27-web-terminal D4: `terminal_session_events`（web 端末の開始・終了の監査。追記専用）。
    (
        99,
        include_str!("../migrations/0099_terminal_session_events.sql"),
    ),
    // ADR 2026-10-28-cos-session-policy-key D1: `node_sessions.policy_key`（CoS session の照合 key の方針の部分）。
    (
        100,
        include_str!("../migrations/0100_cos_session_policy_key.sql"),
    ),
    // ADR 2026-10-28-cos-inbox-chat-summary D2: `cos_inbox_items.detail`（取り込んだ revision の本文の写し）。
    (
        101,
        include_str!("../migrations/0101_cos_inbox_item_detail.sql"),
    ),
    // ADR 2026-10-28-cos-review-back-thread D1/D7: `chat_threads.kind` に `review`（統括 CoS の裏 thread、1 件まで）。
    (
        102,
        include_str!("../migrations/0102_cos_review_thread.sql"),
    ),
    // ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4: `script_generations`・`script_domain_modes`・
    // `script_domain_events`（TS 制御層の領域の mode と世代、mode 変更の追記専用の記録）。
    (103, include_str!("../migrations/0103_script_domains.sql")),
    // ADR 2026-11-02-ts-control-layer-boundary 付記 S4-4・S4-5: `script_domain_modes` に `cos_digest` の行、
    // thread・run に属する compare の追記専用の記録 `script_compare_log`。
    (
        104,
        include_str!("../migrations/0104_script_cos_digest.sql"),
    ),
    // ADR 2026-11-02-ts-control-layer-boundary 付記 S5-1・S5-3・S5-7: generation の状態の拡張、`canary_bps`、
    // 追記専用の `script_generation_events`、停止基準の計数 `script_guard_state`。
    (
        105,
        include_str!("../migrations/0105_script_generations_s5.sql"),
    ),
    // ADR 2026-11-02-ts-control-layer-boundary 付記 S5-7: 受信箱 `script_control` の項目と記録の表ごとの cursor。
    (106, include_str!("../migrations/0106_script_inbox.sql")),
    // ADR 2026-11-03-cos-digest-persist: the outbox of the `cos_digest` compare shadow.
    (
        107,
        include_str!("../migrations/0107_cos_digest_shadow_outbox.sql"),
    ),
    // ADR 2026-11-03-ts-intended-behavior-change D4: 承認した方針改善の仕様と追記専用の記録。
    (
        108,
        include_str!("../migrations/0108_script_behavior_changes.sql"),
    ),
    (
        109,
        include_str!("../migrations/0109_script_execution_gate.sql"),
    ),
    // ADR 2026-11-04-lane-rules-exhaustive-evidence D4: lane_rules の網羅の証拠 `script_exhaustive_runs`。
    (
        110,
        include_str!("../migrations/0110_script_exhaustive_runs.sql"),
    ),
    // ADR 2026-11-04-project-controller D7: 領域 `project_controller` の mode の行。
    (
        111,
        include_str!("../migrations/0111_script_project_controller.sql"),
    ),
    // ADR 2026-11-04-project-controller D12・A1: 案件の controller の状態・wake・判断の記録。
    (
        112,
        include_str!("../migrations/0112_project_controller.sql"),
    ),
    // ADR 2026-11-04-project-controller 付記 A5: 案件の Roadmap（版・Outcome の状態・root との対応の履歴）。
    (113, include_str!("../migrations/0113_project_roadmap.sql")),
    // ADR 2026-11-04-project-controller A7: 人の許可範囲（authority ledger）。
    (114, include_str!("../migrations/0114_authority_ledger.sql")),
    // 第 4 フェーズ P4-B: wake の種類 `evidence_changed`（達成の根拠の失効で controller を起こす）。
    (
        115,
        include_str!("../migrations/0115_project_wake_evidence_changed.sql"),
    ),
    // 第 4 フェーズ P4-D（ADR 2026-11-05-phase4-p4d-retention）: wake・判断・compare の記録の保持と GC
    // （許可の行・GC の追記専用の記録・削除を拒む trigger の置き換え）。
    (
        117,
        include_str!("../migrations/0117_project_retention.sql"),
    ),
    (
        118,
        include_str!("../migrations/0118_evidence_integrity.sql"),
    ),
    (
        119,
        include_str!("../migrations/0119_chat_queued_index.sql"),
    ),
    (
        120,
        include_str!("../migrations/0120_chat_run_launch_boundary.sql"),
    ),
    (
        121,
        include_str!("../migrations/0121_health_inventory_runs.sql"),
    ),
];

/// 他の celeris/* ブランチが使っていて、このブランチにはまだ無い版数（0038 work_unit_sessions、
/// 0039 write_sets、0040 behind_targets、0067/0068 の並行 migration）。`migrate` は飛ばし、
/// `schema_migrations` にも記録しない。統合で本物の migration が入ったら、ここから外して
/// [`MIGRATIONS`] に足す（記録が無いので後から当たる）。
pub(crate) const RESERVED_VERSIONS: &[u32] = &[38, 39, 40, 68, 69, 116];

/// 版数 `version` の SQL 本体。表に無い版数は `None`。
pub(crate) fn migration_sql(version: u32) -> Option<&'static str> {
    MIGRATIONS
        .iter()
        .find(|(v, _)| *v == version)
        .map(|(_, sql)| *sql)
}
