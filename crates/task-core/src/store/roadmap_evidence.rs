//! N5: 起票時の条件と、検証から記録までの CAS。
use super::{RootEventMark, SqliteStore, StoreError};
use crate::roadmap::RoadmapOutcomeSpec;
use rusqlite::Connection;
use std::collections::BTreeMap;

impl SqliteStore {
    pub fn task_roadmap_specs(&self, project: &str) -> Result<BTreeMap<String, i64>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT task_id, version FROM task_roadmap_specs WHERE project_id = ?1")?;
            let rows = stmt.query_map([project], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            rows.map(|row| row.map_err(StoreError::from)).collect()
        })
    }

    pub fn roadmap_evidence_token(
        &self,
        project: &str,
    ) -> Result<(i64, RootEventMark), StoreError> {
        self.with_read_conn(|conn| Self::roadmap_evidence_token_tx(conn, project))
    }

    pub fn roadmap_evidence_token_tx(
        conn: &Connection,
        project: &str,
    ) -> Result<(i64, RootEventMark), StoreError> {
        let version = conn.query_row("SELECT COALESCE((SELECT version FROM project_roadmap_versions WHERE project_id = ?1 AND status = 'approved'), 0)", [project], |r| r.get(0))?;
        Ok((version, Self::root_event_mark_tx(conn, project)?))
    }

    pub fn task_roadmap_versions(
        &self,
        project: &str,
    ) -> Result<BTreeMap<i64, Vec<RoadmapOutcomeSpec>>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare("SELECT v.version, v.outcomes_json FROM project_roadmap_versions v WHERE v.project_id = ?1 AND EXISTS (SELECT 1 FROM task_roadmap_specs s WHERE s.project_id = v.project_id AND s.version = v.version)")?;
            let rows = stmt.query_map([project], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            rows.map(|row| {
                let (version, json) = row?;
                Ok((version, serde_json::from_str(&json)?))
            }).collect()
        })
    }
}
