use rusqlite::{Connection, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};

/// このバイナリが知っている最新のスキーマ版数（ADR-0013 D5）。DB の版数がこれより大きければ
/// `SqliteStore::open`/`open_with` は `StoreError::SchemaTooNew` で失敗する。
pub const SCHEMA_VERSION: u32 = 125;

impl SqliteStore {
    /// migration 0105 の `script_domain_modes.canary_bps`。CHECK は NULL で通るので `IS NOT NULL` を明示する。
    fn add_script_canary_bps(tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
        let exists: bool = tx.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('script_domain_modes') WHERE name = 'canary_bps'",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            tx.execute_batch(
                "ALTER TABLE script_domain_modes ADD COLUMN canary_bps INTEGER CHECK ( \
                     CASE WHEN mode = 'canary' \
                         THEN canary_bps IS NOT NULL AND canary_bps BETWEEN 1 AND 9999 \
                         ELSE canary_bps IS NULL \
                     END);",
            )?;
        }
        Ok(())
    }

    pub(crate) fn apply_migration_version(
        conn: &mut Connection,
        version: u32,
    ) -> Result<(), StoreError> {
        let sql = crate::migration_registry::migration_sql(version)
            .ok_or_else(|| StoreError::Invalid(format!("unknown migration version: {version}")))?;
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
        // 付記 S5-3: `canary_bps`（ADD COLUMN は当て直せないので列が無いときだけ足す）。
        if version == 105 {
            Self::add_script_canary_bps(&tx)?;
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
