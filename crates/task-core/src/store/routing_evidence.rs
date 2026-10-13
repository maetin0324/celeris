use rusqlite::params;

use crate::model::{Event, TaskId};
use crate::route_select::{EvidenceKey, EvidenceRow, RoutingStage, RoutingStageChange};

use super::{SqliteStore, StoreError};

impl SqliteStore {
    pub(super) fn replace_routing_evidence_impl(
        &self,
        version: &str,
        rows: &[EvidenceRow],
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM routing_evidence WHERE evidence_version = ?1",
            params![version],
        )?;
        for item in rows {
            tx.execute("INSERT INTO routing_evidence (genre,harness_id,harness_version,source_id,model_id,evidence_version,n,pass,pass_lower,pass_upper,wall_median_s,usd_list_median,quota_per_run_json,timeout_rate) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)", params![item.key.genre,item.key.harness_id,item.key.harness_version,item.key.source_id,item.key.model_id,version,item.n,item.pass,item.pass_lower,item.pass_upper,item.wall_median_s,item.usd_list_median,serde_json::to_string(&item.quota_per_run)?,item.timeout_rate])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn latest_routing_evidence_impl(
        &self,
        genre: &str,
        harness_id: &str,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        let conn = self.lock()?;
        let version: Option<String> = conn.query_row(
            "SELECT MAX(evidence_version) FROM routing_evidence WHERE genre=?1 AND harness_id=?2",
            params![genre, harness_id],
            |r| r.get(0),
        )?;
        let Some(version) = version else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare("SELECT harness_version,source_id,model_id,n,pass,pass_lower,pass_upper,wall_median_s,usd_list_median,quota_per_run_json,timeout_rate FROM routing_evidence WHERE genre=?1 AND harness_id=?2 AND evidence_version=?3 ORDER BY source_id,model_id")?;
        let rows = stmt.query_map(params![genre, harness_id, version], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, u64>(4)?,
                r.get::<_, f64>(5)?,
                r.get::<_, f64>(6)?,
                r.get::<_, f64>(7)?,
                r.get::<_, f64>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, f64>(10)?,
            ))
        })?;
        rows.map(|row| {
            let (
                harness_version,
                source_id,
                model_id,
                n,
                pass,
                pass_lower,
                pass_upper,
                wall_median_s,
                usd_list_median,
                quota,
                timeout_rate,
            ) = row?;
            Ok(EvidenceRow {
                key: EvidenceKey {
                    genre: genre.into(),
                    harness_id: harness_id.into(),
                    harness_version,
                    source_id,
                    model_id,
                },
                evidence_version: version.clone(),
                n,
                pass,
                pass_lower,
                pass_upper,
                wall_median_s,
                usd_list_median,
                quota_per_run: serde_json::from_str(&quota)?,
                timeout_rate,
            })
        })
        .collect()
    }

    pub(super) fn routing_stage_impl(
        &self,
        genre: &str,
        harness_id: &str,
    ) -> Result<RoutingStage, StoreError> {
        let conn = self.lock()?;
        let stage: Option<String> = conn
            .query_row(
                "SELECT stage FROM routing_stage WHERE genre=?1 AND harness_id=?2",
                params![genre, harness_id],
                |r| r.get(0),
            )
            .optional()?;
        match stage.as_deref() {
            Some("enforce") => Ok(RoutingStage::Enforce),
            _ => Ok(RoutingStage::Shadow),
        }
    }

    pub(super) fn set_routing_stage_impl(
        &self,
        task_id: TaskId,
        change: &RoutingStageChange,
    ) -> Result<(), StoreError> {
        let RoutingStageChange {
            genre,
            harness_id,
            stage,
            since,
            evidence_version,
            reason,
            window,
            actor,
        } = change;
        if !matches!(actor.as_str(), "human" | "daemon") {
            return Err(StoreError::Invalid(
                "routing stage actor must be human or daemon".into(),
            ));
        }
        let to = match stage {
            RoutingStage::Shadow => "shadow",
            RoutingStage::Enforce => "enforce",
        };
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let from: Option<String> = tx
            .query_row(
                "SELECT stage FROM routing_stage WHERE genre=?1 AND harness_id=?2",
                params![genre, harness_id],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute("INSERT INTO routing_stage (genre,harness_id,stage,since,evidence_version) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(genre,harness_id) DO UPDATE SET stage=excluded.stage,since=excluded.since,evidence_version=excluded.evidence_version", params![genre,harness_id,to,since,evidence_version])?;
        let event = Event::RoutingStageChanged {
            genre: genre.into(),
            harness_id: harness_id.into(),
            from,
            to: to.into(),
            reason: reason.clone(),
            window: window.clone(),
            actor: actor.clone(),
        };
        Self::append_event_tx(&tx, task_id, &event)?;
        tx.commit()?;
        Ok(())
    }
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SCHEMA_VERSION, TaskStore};

    #[test]
    fn evidence_versions_replace_and_stage_is_shadow_by_default_then_event_is_appended() {
        let store = SqliteStore::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 74);
        assert_eq!(
            store.routing_stage("coding", "codex").unwrap(),
            RoutingStage::Shadow
        );
        let row = |version: &str, pass| EvidenceRow {
            key: EvidenceKey {
                genre: "coding".into(),
                harness_id: "codex".into(),
                harness_version: "1.2.0".into(),
                source_id: "s".into(),
                model_id: "m".into(),
            },
            evidence_version: version.into(),
            n: 10,
            pass,
            pass_lower: 0.5,
            pass_upper: 0.9,
            wall_median_s: 12.0,
            usd_list_median: 0.1,
            quota_per_run: vec![("7d".into(), 0.01)],
            timeout_rate: 0.0,
        };
        store
            .replace_routing_evidence("v1", &[row("v1", 8)])
            .unwrap();
        store
            .replace_routing_evidence("v1", &[row("v1", 9)])
            .unwrap();
        store
            .replace_routing_evidence("v2", &[row("v2", 10)])
            .unwrap();
        let latest = store.latest_routing_evidence("coding", "codex").unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].evidence_version, "v2");
        assert_eq!(latest[0].pass, 10);
        let task = crate::store::tests::sample_task(crate::model::Status::Draft);
        store.insert(&task).unwrap();
        store
            .set_routing_stage(
                task.id,
                &RoutingStageChange {
                    genre: "coding".into(),
                    harness_id: "codex".into(),
                    stage: RoutingStage::Enforce,
                    since: "now".into(),
                    evidence_version: Some("v2".into()),
                    reason: "human approved".into(),
                    window: serde_json::json!({"runs": 50}),
                    actor: "human".into(),
                },
            )
            .unwrap();
        assert_eq!(
            store.routing_stage("coding", "codex").unwrap(),
            RoutingStage::Enforce
        );
        let events = store.events_for(task.id).unwrap();
        assert!(
            matches!(&events[0].1, Event::RoutingStageChanged { from: None, to, actor, .. } if to == "enforce" && actor == "human")
        );
    }
}
