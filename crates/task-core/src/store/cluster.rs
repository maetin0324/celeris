use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;

use super::{
    ClusterConnectionRecord, ClusterSettings, SqliteStore, StoreError, format_rfc3339,
    parse_rfc3339,
};

impl SqliteStore {
    pub(super) fn cluster_settings_get_impl(
        &self,
        cluster_id: &str,
    ) -> Result<Option<ClusterSettings>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT cluster_id, work_dir, updated_at FROM cluster_settings WHERE cluster_id = ?1",
                params![cluster_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(cluster_id, work_dir, updated_at)| {
                Ok(ClusterSettings {
                    cluster_id,
                    work_dir,
                    updated_at: parse_rfc3339(&updated_at)?,
                })
            })
            .transpose()
        })
    }

    pub(super) fn cluster_settings_list_impl(&self) -> Result<Vec<ClusterSettings>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cluster_id, work_dir, updated_at FROM cluster_settings ORDER BY cluster_id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (cluster_id, work_dir, updated_at) = row?;
                out.push(ClusterSettings {
                    cluster_id,
                    work_dir,
                    updated_at: parse_rfc3339(&updated_at)?,
                });
            }
            Ok(out)
        })
    }

    pub(super) fn cluster_settings_set_impl(
        &self,
        cluster_id: &str,
        work_dir: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::cluster_settings_set_tx(&conn, cluster_id, work_dir, now)
    }

    /// `cluster_settings_set` in the caller's transaction (CoS `cluster.settings_put`).
    pub fn cluster_settings_set_tx(
        conn: &Connection,
        cluster_id: &str,
        work_dir: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        match work_dir {
            Some(work_dir) => {
                conn.execute(
                    "INSERT INTO cluster_settings (cluster_id, work_dir, updated_at) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(cluster_id) DO UPDATE SET work_dir = excluded.work_dir, \
                     updated_at = excluded.updated_at",
                    params![cluster_id, work_dir, format_rfc3339(now)?],
                )?;
            }
            None => {
                conn.execute(
                    "DELETE FROM cluster_settings WHERE cluster_id = ?1",
                    params![cluster_id],
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn cluster_connection_record_impl(
        &self,
        record: &ClusterConnectionRecord,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let uptime = record
            .uptime_secs
            .map(|v| i64::try_from(v).unwrap_or(i64::MAX));
        conn.execute(
            "INSERT INTO cluster_connection_log (cluster_id, kind, method, cause, uptime_secs, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.cluster_id,
                record.kind,
                record.method,
                record.cause,
                uptime,
                format_rfc3339(record.at)?
            ],
        )?;
        Ok(())
    }

    pub(super) fn cluster_connection_list_since_impl(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<ClusterConnectionRecord>, StoreError> {
        // RFC 3339 の文字列は小数秒の桁数で順序が崩れうるので、比較は読んでから時刻で行う
        // （行は接続・切断ごとに 1 行で少ない）。
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cluster_id, kind, method, cause, uptime_secs, at FROM cluster_connection_log \
                 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (cluster_id, kind, method, cause, uptime, at) = row?;
                let at = parse_rfc3339(&at)?;
                if at < since {
                    continue;
                }
                out.push(ClusterConnectionRecord {
                    cluster_id,
                    kind,
                    method,
                    cause,
                    uptime_secs: uptime.and_then(|v| u64::try_from(v).ok()),
                    at,
                });
            }
            Ok(out)
        })
    }
}
