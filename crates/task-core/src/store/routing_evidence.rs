use rusqlite::params;

use crate::model::{Event, TaskId};
use crate::route_select::{RoutingStage, RoutingStageChange};
use crate::routing_evidence::{EvidenceKey, EvidenceRow};

use super::{SqliteStore, StoreError};

/// 根拠表 `routing_evidence` の列（読み書きで同じ並び）。
const EVIDENCE_COLUMNS: &str = "genre,harness_id,harness_version,lane,source_id,model_id,evidence_version,n,pass,pass_lower,pass_upper,pass_boot_lower,pass_boot_upper,wall_p90_s,usd_per_pass,quota_cost_per_pass,quota_per_run_json,timeout_rate,low_confidence";

fn evidence_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRow> {
    let quota: String = r.get(16)?;
    Ok(EvidenceRow {
        key: EvidenceKey {
            genre: r.get(0)?,
            harness_id: r.get(1)?,
            harness_version: r.get(2)?,
            lane: r.get(3)?,
            source_id: r.get(4)?,
            model_id: r.get(5)?,
        },
        evidence_version: r.get(6)?,
        n: r.get(7)?,
        pass: r.get(8)?,
        pass_lower: r.get(9)?,
        pass_upper: r.get(10)?,
        pass_boot_lower: r.get(11)?,
        pass_boot_upper: r.get(12)?,
        wall_p90_s: r.get(13)?,
        usd_per_pass: r.get(14)?,
        quota_cost_per_pass: r.get(15)?,
        quota_per_run: serde_json::from_str(&quota).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                16,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        timeout_rate: r.get(17)?,
        low_confidence: r.get(18)?,
    })
}

fn evidence_rows_at(
    conn: &rusqlite::Connection,
    genre: &str,
    harness_id: &str,
    version: &str,
) -> Result<Vec<EvidenceRow>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {EVIDENCE_COLUMNS} FROM routing_evidence WHERE genre=?1 AND harness_id=?2 AND evidence_version=?3 ORDER BY source_id,model_id"
    ))?;
    let rows = stmt.query_map(params![genre, harness_id, version], evidence_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
}

impl SqliteStore {
    /// 同じ版は丸ごと置き換え、別の版は残す（daemon が完了単位の本番結果から書く（`task_core::completion_outcome::refresh_routing_evidence`））。
    /// 行の版は引数 `version` で決まる（行の `evidence_version` は読まない）。
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
        {
            let mut stmt = tx.prepare(&format!(
                "INSERT INTO routing_evidence ({EVIDENCE_COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)"
            ))?;
            for item in rows {
                stmt.execute(params![
                    item.key.genre,
                    item.key.harness_id,
                    item.key.harness_version,
                    item.key.lane,
                    item.key.source_id,
                    item.key.model_id,
                    version,
                    item.n,
                    item.pass,
                    item.pass_lower,
                    item.pass_upper,
                    item.pass_boot_lower,
                    item.pass_boot_upper,
                    item.wall_p90_s,
                    item.usd_per_pass,
                    item.quota_cost_per_pass,
                    serde_json::to_string(&item.quota_per_run)?,
                    item.timeout_rate,
                    item.low_confidence
                ])?;
            }
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
        evidence_rows_at(&conn, genre, harness_id, &version)
    }

    pub(super) fn routing_evidence_at_impl(
        &self,
        genre: &str,
        harness_id: &str,
        version: &str,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        let conn = self.lock()?;
        evidence_rows_at(&conn, genre, harness_id, version)
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
        if !(matches!(actor.as_str(), "human" | "daemon") || crate::is_principal_actor(actor)) {
            return Err(StoreError::Invalid(
                "routing stage actor must be human, daemon or an API principal".into(),
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
        assert_eq!(SCHEMA_VERSION, 99);
        assert_eq!(
            store.routing_stage("coding", "codex").unwrap(),
            RoutingStage::Shadow
        );
        let row = |version: &str, pass| EvidenceRow {
            quota_cost_per_pass: None,
            key: EvidenceKey {
                lane: String::new(),
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
            pass_boot_lower: 0.55,
            pass_boot_upper: 0.85,
            wall_p90_s: Some(12.0),
            usd_per_pass: None,
            quota_per_run: [("7d".to_string(), 0.01)].into(),
            timeout_rate: 0.0,
            low_confidence: version == "v2",
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
        // 評価 runner の欄（bootstrap 区間・低確度・値の無い定価）も同じ行で往復する（migration 0077）。
        assert_eq!(
            (latest[0].pass_boot_lower, latest[0].pass_boot_upper),
            (0.55, 0.85)
        );
        assert!(latest[0].low_confidence);
        assert_eq!(latest[0].usd_per_pass, None);
        assert_eq!(latest[0].quota_per_run.get("7d"), Some(&0.01));
        // task 欄 `evidence_table_id` の固定読み: 新しい版があっても指定版だけ（無い版は空）。
        let pinned = store.routing_evidence_at("coding", "codex", "v1").unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(
            (pinned[0].evidence_version.as_str(), pinned[0].pass),
            ("v1", 9)
        );
        assert!(
            store
                .routing_evidence_at("coding", "codex", "v9")
                .unwrap()
                .is_empty()
        );
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
