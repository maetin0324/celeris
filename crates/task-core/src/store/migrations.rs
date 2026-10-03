use rusqlite::{Connection, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};

pub(crate) const MIGRATION_0001: &str = include_str!("../../migrations/0001_init.sql");
pub(crate) const MIGRATION_0002: &str = include_str!("../../migrations/0002_events_global_id.sql");
pub(crate) const MIGRATION_0003: &str =
    include_str!("../../migrations/0003_tasks_list_columns.sql");
pub(crate) const MIGRATION_0004: &str =
    include_str!("../../migrations/0004_tasks_objective_column.sql");
pub(crate) const MIGRATION_0005: &str =
    include_str!("../../migrations/0005_tasks_genre_column.sql");
pub(crate) const MIGRATION_0006: &str = include_str!("../../migrations/0006_organization.sql");
pub(crate) const MIGRATION_0007: &str =
    include_str!("../../migrations/0007_messages_task_id_and_reports_project.sql");
pub(crate) const MIGRATION_0008: &str = include_str!("../../migrations/0008_notifications.sql");
pub(crate) const MIGRATION_0009: &str =
    include_str!("../../migrations/0009_notifications_project_id.sql");
pub(crate) const MIGRATION_0010: &str =
    include_str!("../../migrations/0010_projects_workspace.sql");
pub(crate) const MIGRATION_0011: &str = include_str!("../../migrations/0011_daemon_instances.sql");
/// ADR-0043 D1/D2（Phase 52）: `project_repos` と `tasks.repos_json`。
/// この版を当てた直後に、同じトランザクションで `backfill_project_repos` が写しを作る。
pub(crate) const MIGRATION_0012: &str = include_str!("../../migrations/0012_project_repos.sql");
/// ADR-0044 D2/D3（Phase 53）: `task_comments` と `tasks.labels_json` / `tasks.category`。
pub(crate) const MIGRATION_0013: &str = include_str!("../../migrations/0013_task_comments.sql");
/// ADR-0043 D5（Phase 54）: `task_integrations`（取り込みの記録）。
pub(crate) const MIGRATION_0014: &str = include_str!("../../migrations/0014_task_integrations.sql");
/// ADR-0044 D6（Phase 55）: 案件・途中目標の中止・一時停止・アーカイブ（`archived_at` / `paused_from`）。
pub(crate) const MIGRATION_0015: &str = include_str!("../../migrations/0015_lifecycle.sql");
/// ADR-0046 D1/D2/D4（Phase 59）: `org_nodes.profile_json` と `tasks.skills_json` / `tasks.mode`。
pub(crate) const MIGRATION_0016: &str = include_str!("../../migrations/0016_org_profiles.sql");
pub(crate) const MIGRATION_0017: &str = include_str!("../../migrations/0017_console_actions.sql");
/// ADR-0047 D4（Phase 62）: `knowledge_runs`（知識整理 run を高々 1 回だけ起こすための目印）。
pub(crate) const MIGRATION_0020: &str = include_str!("../../migrations/0020_deliveries.sql");
pub(crate) const MIGRATION_0019: &str = include_str!("../../migrations/0019_notification_scan.sql");
pub(crate) const MIGRATION_0018: &str = include_str!("../../migrations/0018_knowledge_runs.sql");
/// ADR-0052 D2 / D3（Phase 64）: `knowledge_runs.retried_at` と `knowledge_runs.via`。
pub(crate) const MIGRATION_0021: &str =
    include_str!("../../migrations/0021_knowledge_run_retry.sql");
/// ADR-0053 D1（Phase 65）: `llm_proxy_requests`（LLM source ローカルプロキシの要求記録）。
pub(crate) const MIGRATION_0022: &str =
    include_str!("../../migrations/0022_llm_proxy_requests.sql");
/// ADR-0054 D1（Phase 67）: `node_sessions`（ノードごとの継続セッションと resume）。
pub(crate) const MIGRATION_0023: &str = include_str!("../../migrations/0023_node_sessions.sql");
/// ADR-0056 D1 / D4（Phase 78）: `mcp_clients` / `mcp_calls`（MCP サーバーの認証とログ）。
pub(crate) const MIGRATION_0024: &str = include_str!("../../migrations/0024_mcp.sql");
/// ADR-0059 D6（Phase 99）: `cluster_settings`（クラスタの作業ディレクトリの DB 上書き）。
pub(crate) const MIGRATION_0025: &str = include_str!("../../migrations/0025_cluster_settings.sql");
/// ADR-0072 D5/D23（Phase E2）: `execution_plans` / `work_units` / `runs`（Task 下の内部実行層の
/// 派生索引。正本は events。`CREATE TABLE IF NOT EXISTS` だけで既存の表には触れない）。
pub(crate) const MIGRATION_0026: &str = include_str!("../../migrations/0026_execution.sql");
/// ADR-0074 D1/§5.2（Phase F2）: `work_units` に v2（並列実行）の phase/lease/branch/commit 列を足す。
pub(crate) const MIGRATION_0027: &str =
    include_str!("../../migrations/0027_parallel_work_units.sql");
/// ADR-0074 D3.2 / D3.8（Phase F4b）: `projects.auto_advance` と `milestones.plan_key`。
pub(crate) const MIGRATION_0028: &str = include_str!("../../migrations/0028_project_plan_go.sql");
/// ADR-0044 D7 追記 / ADR-0047 追記（Phase K-1）: `projects.slug`（知識ベースの `projects/<slug>/`）。
pub(crate) const MIGRATION_0029: &str = include_str!("../../migrations/0029_project_slug.sql");
/// ADR-0078 D5: `cluster_connection_log`（クラスタの ssh master の接続・切断の記録）。
pub(crate) const MIGRATION_0030: &str =
    include_str!("../../migrations/0030_cluster_connection_log.sql");
/// ADR-0079 D4 / D7 / D15（Phase R1a）: `tasks.root_id`・`work_units.child_task_id` /
/// `needs_decisions_json`・`decisions`（再帰的な task の木。既存の行は書き換えない）。
pub(crate) const MIGRATION_0031: &str = include_str!("../../migrations/0031_task_tree.sql");
/// ADR-0080 D4/D5: browser の人待ち（登録依頼・承認）と credential 台帳（秘密なし）。
pub(crate) const MIGRATION_0032: &str = include_str!("../../migrations/0032_browser_waits.sql");
pub(crate) const MIGRATION_0033: &str =
    include_str!("../../migrations/0033_browser_task_policies.sql");
/// ADR-0090 D2: `cluster_job_waits`（クラスタ job の durable wait。events が正本の派生の索引）。
pub(crate) const MIGRATION_0034: &str = include_str!("../../migrations/0034_cluster_job_waits.sql");
/// browser Phase 3: control lease・live proxy・identity の store（ブランチの 0034 を main の後へ振り直し）。
pub(crate) const MIGRATION_0035: &str =
    include_str!("../../migrations/0035_browser_phase3_store.sql");
/// browser Phase 4: trusted login（ブランチの 0035 を振り直し）。
pub(crate) const MIGRATION_0036: &str =
    include_str!("../../migrations/0036_browser_trusted_login.sql");
/// ADR-0121 付記: `idx_events_delivery_skipped`（受信箱の delivery_skipped 走査を絞る部分 index）。
pub(crate) const MIGRATION_0037: &str =
    include_str!("../../migrations/0037_events_delivery_skipped_index.sql");
/// ADR-0133 D3.2: `feed_notices` / `feed_sources` / `feed_cursor`（通知の既読と束ね）。
pub(crate) const MIGRATION_0041: &str = include_str!("../../migrations/0041_feed_notices.sql");
/// ADR-0118 D3: delivery の検査対象と merge candidate を記録する。
/// ブランチでは 0037 だったが、main の `0037_events_delivery_skipped_index`（本番適用済み）と重複したので
/// review sync で空き番号 42 へ振り直した。列を足すだけで 0038〜0041 とは依存しない。
pub(crate) const MIGRATION_0042: &str =
    include_str!("../../migrations/0042_review_target_sync.sql");
/// ADR-0124 D2: `node_sessions` に execute continuation の WU 単位 session の列を足す。
/// ブランチでは 0038 だったが、main の 0038〜0040 は未使用のまま空いていなかったため
/// （他の celeris/* ブランチが 0038〜0040 を使用中）、mig-renumber で 0043 へ振り直した。
pub(crate) const MIGRATION_0043: &str =
    include_str!("../../migrations/0043_work_unit_sessions.sql");
/// ブランチでは 0039 だったが、同じ理由で 0044 へ振り直した。
pub(crate) const MIGRATION_0044: &str = include_str!("../../migrations/0044_write_sets.sql");
/// ADR-0130 D4: task branch の target からの behind snapshot。
/// ブランチでは 0040 だったが、同じ理由で 0045 へ振り直した。
pub(crate) const MIGRATION_0045: &str = include_str!("../../migrations/0045_behind_targets.sql");

/// 他の celeris/* ブランチが使っていて、このブランチにはまだ無い版数。`migrate` は飛ばし、
/// `schema_migrations` にも記録しない。統合で本物の migration が入ったら、ここから外して
/// `migration_sql` に足す（記録が無いので後から当たる）。0038〜0040 は mig-renumber で
/// 0043〜0045 へ振り直したので、このブランチでは空いたまま（他ブランチの別内容がまだ使用中）。
pub(crate) const RESERVED_VERSIONS: &[u32] = &[38, 39, 40];

/// このバイナリが知っている最新のスキーマ版数（ADR-0013 D5）。DB の版数がこれより大きければ
/// `SqliteStore::open`/`open_with` は `StoreError::SchemaTooNew` で失敗する。
pub const SCHEMA_VERSION: u32 = 45;

impl SqliteStore {
    fn migration_sql(version: u32) -> Result<&'static str, StoreError> {
        match version {
            1 => Ok(MIGRATION_0001),
            2 => Ok(MIGRATION_0002),
            3 => Ok(MIGRATION_0003),
            4 => Ok(MIGRATION_0004),
            5 => Ok(MIGRATION_0005),
            6 => Ok(MIGRATION_0006),
            7 => Ok(MIGRATION_0007),
            8 => Ok(MIGRATION_0008),
            9 => Ok(MIGRATION_0009),
            10 => Ok(MIGRATION_0010),
            11 => Ok(MIGRATION_0011),
            12 => Ok(MIGRATION_0012),
            13 => Ok(MIGRATION_0013),
            14 => Ok(MIGRATION_0014),
            15 => Ok(MIGRATION_0015),
            16 => Ok(MIGRATION_0016),
            17 => Ok(MIGRATION_0017),
            18 => Ok(MIGRATION_0018),
            19 => Ok(MIGRATION_0019),
            20 => Ok(MIGRATION_0020),
            21 => Ok(MIGRATION_0021),
            22 => Ok(MIGRATION_0022),
            23 => Ok(MIGRATION_0023),
            24 => Ok(MIGRATION_0024),
            25 => Ok(MIGRATION_0025),
            26 => Ok(MIGRATION_0026),
            27 => Ok(MIGRATION_0027),
            28 => Ok(MIGRATION_0028),
            29 => Ok(MIGRATION_0029),
            30 => Ok(MIGRATION_0030),
            31 => Ok(MIGRATION_0031),
            32 => Ok(MIGRATION_0032),
            33 => Ok(MIGRATION_0033),
            34 => Ok(MIGRATION_0034),
            35 => Ok(MIGRATION_0035),
            36 => Ok(MIGRATION_0036),
            37 => Ok(MIGRATION_0037),
            41 => Ok(MIGRATION_0041),
            42 => Ok(MIGRATION_0042),
            43 => Ok(MIGRATION_0043),
            44 => Ok(MIGRATION_0044),
            45 => Ok(MIGRATION_0045),
            other => Err(StoreError::Invalid(format!(
                "unknown migration version: {other}"
            ))),
        }
    }

    pub(crate) fn apply_migration_version(
        conn: &mut Connection,
        version: u32,
    ) -> Result<(), StoreError> {
        let sql = Self::migration_sql(version)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        // ADR-0043 D1（Phase 52）: 既存の `projects.workspace` を `is_primary = 1` のリポジトリ 1 件に
        // 写す。id が ULID で、`kind` の判定にファイルシステムを見る必要があるので SQL では書けない
        // （migration 0012 のコメント参照）。同じトランザクションの中で 1 度だけ走る。
        if version == 12 {
            Self::backfill_project_repos(&tx)?;
        }
        // Phase K-1: 既存の案件に `slug` を付ける（作り方は Rust にしか無い。migration 0029 のコメント参照）。
        if version == 29 {
            Self::backfill_project_slugs(&tx)?;
        }
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, ts],
        )?;
        tx.commit()?;
        Ok(())
    }
}
