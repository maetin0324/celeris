//! ADR 2026-10-08 account-concurrency-limits: authoritative settings, not replayed.
use super::{SqliteStore, StoreError};
use rusqlite::params;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLimit {
    pub adapter: String,
    pub account_id: String,
    pub max_runs: usize,
}

impl SqliteStore {
    pub(super) fn account_limits_impl(&self) -> Result<Vec<AccountLimit>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT adapter, account_id, max_runs FROM account_limits ORDER BY adapter, account_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AccountLimit {
                adapter: row.get(0)?,
                account_id: row.get(1)?,
                max_runs: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub(super) fn set_account_limit_impl(
        &self,
        adapter: &str,
        id: &str,
        max_runs: Option<usize>,
    ) -> Result<(), StoreError> {
        if max_runs.is_some_and(|n| !(1..=32).contains(&n)) {
            return Err(StoreError::Invalid("max_runs must be in 1..=32".into()));
        }
        let conn = self.lock()?;
        if let Some(max_runs) = max_runs {
            let now = OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|e| StoreError::Invalid(e.to_string()))?;
            conn.execute("INSERT INTO account_limits (adapter, account_id, max_runs, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(adapter, account_id) DO UPDATE SET max_runs=excluded.max_runs, updated_at=excluded.updated_at",
                params![adapter, id, max_runs, now])?;
        } else {
            conn.execute(
                "DELETE FROM account_limits WHERE adapter=?1 AND account_id=?2",
                params![adapter, id],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskStore;
    #[test]
    fn account_limits_persist_clear_and_keep_adapters_separate() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("limits.db");
        {
            let store = SqliteStore::open(&db).unwrap();
            store
                .set_account_limit("claude-code", "a", Some(3))
                .unwrap();
            store.set_account_limit("codex", "a", Some(1)).unwrap();
            store
                .set_account_limit("implicit", "qwen", Some(1))
                .unwrap();
            store
                .set_account_limit("claude-code", "a", Some(4))
                .unwrap();
            for value in [0, 33, usize::MAX] {
                assert!(
                    store
                        .set_account_limit("claude-code", "a", Some(value))
                        .is_err()
                );
            }
        }
        let store = SqliteStore::open(&db).unwrap();
        let limits = store.account_limits().unwrap();
        assert_eq!(limits.len(), 3);
        assert_eq!(limits[0].max_runs, 4);
        store.set_account_limit("claude-code", "a", None).unwrap();
        store.set_account_limit("claude-code", "a", None).unwrap();
        drop(store);
        let store = SqliteStore::open(&db).unwrap();
        assert_eq!(store.account_limits().unwrap().len(), 2);
    }
}
