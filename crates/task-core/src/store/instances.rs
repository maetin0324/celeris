use rusqlite::{TransactionBehavior, params};
use time::OffsetDateTime;

use crate::instance::{DaemonInstance, InstanceRole, SELECT_INSTANCE, row_to_instance};

use super::{SqliteStore, StoreError, format_rfc3339};

impl SqliteStore {
    pub(super) fn instance_register_impl(
        &self,
        instance: &DaemonInstance,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO daemon_instances \
             (instance_id, \"release\", pid, role, started_at, heartbeat_at, handoff_requested_at, drained_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                instance.instance_id,
                instance.release,
                i64::from(instance.pid),
                instance.role.as_str(),
                format_rfc3339(instance.started_at)?,
                format_rfc3339(instance.heartbeat_at)?,
                instance.handoff_requested_at.map(format_rfc3339).transpose()?,
                instance.drained_at.map(format_rfc3339).transpose()?,
            ],
        )?;
        Ok(())
    }

    pub(super) fn instance_heartbeat_impl(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET heartbeat_at = ?1 WHERE instance_id = ?2",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn instance_request_handoff_impl(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET handoff_requested_at = ?1 \
             WHERE instance_id = ?2 AND handoff_requested_at IS NULL",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn instance_set_role_impl(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET role = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![role.as_str(), format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn instance_mark_drained_impl(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let ts = format_rfc3339(at)?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET drained_at = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![ts.clone(), ts, instance_id],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn instance_list_impl(&self) -> Result<Vec<DaemonInstance>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{SELECT_INSTANCE} ORDER BY started_at ASC, instance_id ASC"
            ))?;
            let rows = stmt.query_map([], row_to_instance)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    pub(super) fn instance_delete_impl(&self, instance_id: &str) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM daemon_instances WHERE instance_id = ?1",
            params![instance_id],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn instance_delete_stale_impl(
        &self,
        keep: &str,
        heartbeat_before: OffsetDateTime,
    ) -> Result<Vec<String>, StoreError> {
        let mut conn = self.lock()?;
        let before = format_rfc3339(heartbeat_before)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT instance_id FROM daemon_instances \
                 WHERE instance_id <> ?1 AND (drained_at IS NOT NULL OR heartbeat_at < ?2)",
            )?;
            let rows = stmt.query_map(params![keep, before], |row| row.get::<_, String>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            ids
        };
        removed.sort();
        for id in &removed {
            tx.execute(
                "DELETE FROM daemon_instances WHERE instance_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }
}
