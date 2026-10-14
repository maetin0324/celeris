//! ADR 2026-10-14-local-machine-settings D2/D5: `local_settings`（ローカルマシンの上書き）・
//! `local_settings_events`（追記専用の監査）・`task_workspace_roots`（task ごとの root の固定）。
//!
//! `local_settings_events` と `task_workspace_roots` は UPDATE・DELETE を持たない（INSERT だけ）。
//! 解決（CLI > store > config）は `crate::local_machine` にある。

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;

use crate::model::TaskId;

use super::{
    LocalSettings, LocalSettingsEvent, LocalSettingsOverrides, SqliteStore, StoreError,
    format_rfc3339, parse_rfc3339,
};

/// `max_concurrency` の許す範囲（D5 の 4）。API が先に 422 で弾くが、store も範囲外を書かない。
pub(crate) const MAX_CONCURRENCY_RANGE: std::ops::RangeInclusive<u32> = 1..=256;

impl SqliteStore {
    pub(super) fn local_settings_get_impl(&self) -> Result<Option<LocalSettings>, StoreError> {
        self.with_read_conn(Self::local_settings_read)
    }

    fn local_settings_read(conn: &Connection) -> Result<Option<LocalSettings>, StoreError> {
        conn.query_row(
            "SELECT workspace_root, max_concurrency, updated_at FROM local_settings WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(workspace_root, max_concurrency, updated_at)| {
            let max_concurrency = max_concurrency
                .map(|v| {
                    u32::try_from(v).map_err(|_| {
                        StoreError::Invalid(format!(
                            "local_settings.max_concurrency out of range: {v}"
                        ))
                    })
                })
                .transpose()?;
            Ok(LocalSettings {
                workspace_root,
                max_concurrency,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })
        .transpose()
    }

    pub(super) fn local_settings_set_impl(
        &self,
        overrides: &LocalSettingsOverrides,
        actor: &str,
        now: OffsetDateTime,
    ) -> Result<Option<LocalSettings>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let after = Self::local_settings_set_tx(&tx, overrides, actor, now)?;
        tx.commit()?;
        Ok(after)
    }

    /// `local_settings_set` in the caller's transaction (CoS `local.settings_put`)。
    /// 上書きの全置換と監査 1 行の INSERT を同じ transaction で行う。
    pub fn local_settings_set_tx(
        conn: &Connection,
        overrides: &LocalSettingsOverrides,
        actor: &str,
        now: OffsetDateTime,
    ) -> Result<Option<LocalSettings>, StoreError> {
        if let Some(n) = overrides.max_concurrency
            && !MAX_CONCURRENCY_RANGE.contains(&n)
        {
            return Err(StoreError::Invalid(format!(
                "local_settings.max_concurrency must be within 1..=256: {n}"
            )));
        }
        let before = Self::local_settings_read(conn)?
            .map(|s| LocalSettingsOverrides {
                workspace_root: s.workspace_root,
                max_concurrency: s.max_concurrency,
            })
            .unwrap_or_default();
        let ts = format_rfc3339(now)?;
        if overrides.is_empty() {
            conn.execute("DELETE FROM local_settings WHERE id = 1", [])?;
        } else {
            conn.execute(
                "INSERT INTO local_settings (id, workspace_root, max_concurrency, updated_at) \
                 VALUES (1, ?1, ?2, ?3) \
                 ON CONFLICT(id) DO UPDATE SET workspace_root = excluded.workspace_root, \
                 max_concurrency = excluded.max_concurrency, updated_at = excluded.updated_at",
                params![
                    overrides.workspace_root,
                    overrides.max_concurrency.map(i64::from),
                    ts
                ],
            )?;
        }
        conn.execute(
            "INSERT INTO local_settings_events (ts, actor, before_json, after_json) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                ts,
                actor,
                serde_json::to_string(&before)?,
                serde_json::to_string(overrides)?
            ],
        )?;
        Self::local_settings_read(conn)
    }

    pub(super) fn local_settings_events_impl(&self) -> Result<Vec<LocalSettingsEvent>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, actor, before_json, after_json FROM local_settings_events \
                 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, ts, actor, before, after) = row?;
                out.push(LocalSettingsEvent {
                    id,
                    ts: parse_rfc3339(&ts)?,
                    actor,
                    before: serde_json::from_str(&before)?,
                    after: serde_json::from_str(&after)?,
                });
            }
            Ok(out)
        })
    }

    pub(super) fn task_workspace_root_pin_impl(
        &self,
        task_id: &TaskId,
        root: &str,
        now: OffsetDateTime,
    ) -> Result<String, StoreError> {
        let conn = self.lock()?;
        let id = task_id.to_string();
        conn.execute(
            "INSERT OR IGNORE INTO task_workspace_roots (task_id, root, pinned_at) \
             VALUES (?1, ?2, ?3)",
            params![id, root, format_rfc3339(now)?],
        )?;
        let pinned: String = conn.query_row(
            "SELECT root FROM task_workspace_roots WHERE task_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(pinned)
    }

    pub(super) fn task_workspace_root_get_impl(
        &self,
        task_id: &TaskId,
    ) -> Result<Option<String>, StoreError> {
        let id = task_id.to_string();
        self.with_read_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT root FROM task_workspace_roots WHERE task_id = ?1",
                    params![id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::TaskStore;

    fn at(hour: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_791_939_600 + hour * 3600).unwrap()
    }

    #[test]
    pub(crate) fn local_settings_store_roundtrip() {
        let store = SqliteStore::open_in_memory().unwrap();
        assert_eq!(store.local_settings_get().unwrap(), None);

        let t1 = at(1);
        let both = LocalSettingsOverrides {
            workspace_root: Some("/local/celeris/data/workspaces".into()),
            max_concurrency: Some(6),
        };
        let got = store.local_settings_set(&both, "admin", t1).unwrap();
        let expected = LocalSettings {
            workspace_root: Some("/local/celeris/data/workspaces".into()),
            max_concurrency: Some(6),
            updated_at: t1,
        };
        assert_eq!(got.as_ref(), Some(&expected));
        assert_eq!(store.local_settings_get().unwrap(), Some(expected));

        // PUT は全置換: 省略した欄は上書きが消える。
        let t2 = at(2);
        let only_conc = LocalSettingsOverrides {
            workspace_root: None,
            max_concurrency: Some(3),
        };
        store.local_settings_set(&only_conc, "admin", t2).unwrap();
        let got = store.local_settings_get().unwrap().unwrap();
        assert_eq!(got.workspace_root, None);
        assert_eq!(got.max_concurrency, Some(3));
        assert_eq!(got.updated_at, t2);

        // 全欄 None で行ごと消える。
        let cleared = store
            .local_settings_set(&LocalSettingsOverrides::default(), "admin", t2)
            .unwrap();
        assert_eq!(cleared, None);
        assert_eq!(store.local_settings_get().unwrap(), None);
        let rows: i64 = store
            .with_read_conn(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM local_settings", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(rows, 0);

        // 範囲外は書かない（監査も残らない）。
        let n_events = store.local_settings_events().unwrap().len();
        let bad = LocalSettingsOverrides {
            workspace_root: None,
            max_concurrency: Some(0),
        };
        assert!(matches!(
            store.local_settings_set(&bad, "admin", t2),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(store.local_settings_events().unwrap().len(), n_events);
    }

    #[test]
    pub(crate) fn local_settings_audit_event_appended() {
        let store = SqliteStore::open_in_memory().unwrap();
        let t1 = at(1);
        let t2 = at(2);
        let t3 = at(3);
        let a = LocalSettingsOverrides {
            workspace_root: Some("/srv/ws".into()),
            max_concurrency: None,
        };
        let b = LocalSettingsOverrides {
            workspace_root: Some("/srv/ws2".into()),
            max_concurrency: Some(8),
        };
        store.local_settings_set(&a, "admin", t1).unwrap();
        store.local_settings_set(&b, "cos:op-1", t2).unwrap();
        store
            .local_settings_set(&LocalSettingsOverrides::default(), "admin", t3)
            .unwrap();

        let events = store.local_settings_events().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].actor, "admin");
        assert_eq!(events[0].ts, t1);
        assert_eq!(events[0].before, LocalSettingsOverrides::default());
        assert_eq!(events[0].after, a);
        assert_eq!(events[1].actor, "cos:op-1");
        assert_eq!(events[1].before, a);
        assert_eq!(events[1].after, b);
        assert_eq!(events[2].before, b);
        assert_eq!(events[2].after, LocalSettingsOverrides::default());
        assert!(events[0].id < events[1].id && events[1].id < events[2].id);

        // 監査は呼び出し元の transaction と一緒に巻き戻る（同じ transaction で書く）。
        {
            let mut conn = store.lock().unwrap();
            let tx = conn.transaction().unwrap();
            SqliteStore::local_settings_set_tx(&tx, &a, "cos:op-2", t3).unwrap();
            tx.rollback().unwrap();
        }
        assert_eq!(store.local_settings_events().unwrap().len(), 3);
        assert_eq!(store.local_settings_get().unwrap(), None);
    }

    #[test]
    pub(crate) fn task_workspace_root_pin_once() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::new();
        assert_eq!(store.task_workspace_root_get(&task).unwrap(), None);

        let t1 = at(1);
        let pinned = store
            .task_workspace_root_pin(&task, "/old/root", t1)
            .unwrap();
        assert_eq!(pinned, "/old/root");
        // 2 回目は無視され、最初の固定が返る（更新しない）。
        let again = store
            .task_workspace_root_pin(&task, "/new/root", t1)
            .unwrap();
        assert_eq!(again, "/old/root");
        assert_eq!(
            store.task_workspace_root_get(&task).unwrap().as_deref(),
            Some("/old/root")
        );
        let other = TaskId::new();
        assert_eq!(store.task_workspace_root_get(&other).unwrap(), None);
    }
}
