//! ADR 2026-10-22-dynamic-config D4: `config_events`（config.toml の欄の変更の追記専用の履歴）。
//! INSERT だけで UPDATE・DELETE を持たない。値は呼び出し側（daemon）が secret を伏せてから渡す。

use rusqlite::params;
use time::OffsetDateTime;

use super::{
    ConfigEventRow, NewConfigEvent, SqliteStore, StoreError, format_rfc3339, parse_rfc3339,
};

impl SqliteStore {
    pub(super) fn config_events_append_impl(
        &self,
        events: &[NewConfigEvent],
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let ts = format_rfc3339(now)?;
        for e in events {
            tx.execute(
                "INSERT INTO config_events (ts, actor, key, class, apply_unit, before_json, after_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    ts,
                    e.actor,
                    e.key,
                    e.class,
                    e.apply_unit,
                    serde_json::to_string(&e.before)?,
                    serde_json::to_string(&e.after)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 新しい順に最大 `limit` 件。
    pub(super) fn config_events_recent_impl(
        &self,
        limit: usize,
    ) -> Result<Vec<ConfigEventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, actor, key, class, apply_unit, before_json, after_json \
                 FROM config_events ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, ts, actor, key, class, apply_unit, before, after) = row?;
                out.push(ConfigEventRow {
                    id,
                    ts: parse_rfc3339(&ts)?,
                    actor,
                    key,
                    class,
                    apply_unit,
                    before: serde_json::from_str(&before)?,
                    after: serde_json::from_str(&after)?,
                });
            }
            Ok(out)
        })
    }
}
