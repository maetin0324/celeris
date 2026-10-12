use rusqlite::params;

use super::{SqliteStore, StoreError};
use crate::routing_evidence::EvidenceRow;

impl SqliteStore {
    /// Replace all rows for an evidence version atomically, retaining other versions.
    pub fn insert_routing_evidence(&self, rows: &[EvidenceRow]) -> Result<(), StoreError> {
        let Some(version) = rows.first().map(|row| row.evidence_version.as_str()) else {
            return Ok(());
        };
        if rows.iter().any(|row| row.evidence_version != version) {
            return Err(StoreError::Invalid(
                "routing evidence batch must use one evidence_version".into(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM routing_evidence WHERE evidence_version = ?1",
            [version],
        )?;
        {
            let mut stmt = tx.prepare("INSERT INTO routing_evidence(genre,harness_id,harness_version,source_id,model_id,evidence_version,n,pass,pass_lower,pass_upper,wall_median_s,usd_list_median,quota_per_run_json,timeout_rate,low_confidence) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)")?;
            for row in rows {
                stmt.execute(params![
                    row.key.genre,
                    row.key.harness_id,
                    row.key.harness_version,
                    row.key.source_id,
                    row.key.model_id,
                    row.evidence_version,
                    row.n,
                    row.pass,
                    row.pass_lower,
                    row.pass_upper,
                    row.wall_median_s,
                    row.usd_list_median,
                    serde_json::to_string(&row.quota_per_run)?,
                    row.timeout_rate,
                    row.low_confidence
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// List evidence, optionally restricted to one immutable evaluation version.
    pub fn list_routing_evidence(
        &self,
        version: Option<&str>,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT genre,harness_id,harness_version,source_id,model_id,evidence_version,n,pass,pass_lower,pass_upper,wall_median_s,usd_list_median,quota_per_run_json,timeout_rate,low_confidence FROM routing_evidence WHERE (?1 IS NULL OR evidence_version = ?1) ORDER BY genre,harness_id,harness_version,source_id,model_id,evidence_version")?;
        let rows = stmt.query_map([version], |row| {
            let quota: String = row.get(12)?;
            Ok(EvidenceRow {
                key: crate::routing_evidence::EvidenceKey {
                    genre: row.get(0)?,
                    harness_id: row.get(1)?,
                    harness_version: row.get(2)?,
                    source_id: row.get(3)?,
                    model_id: row.get(4)?,
                },
                evidence_version: row.get(5)?,
                n: row.get(6)?,
                pass: row.get(7)?,
                pass_lower: row.get(8)?,
                pass_upper: row.get(9)?,
                wall_median_s: row.get(10)?,
                usd_list_median: row.get(11)?,
                quota_per_run: serde_json::from_str(&quota).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        12,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                timeout_rate: row.get(13)?,
                low_confidence: row.get(14)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing_evidence::EvidenceKey;
    use std::collections::BTreeMap;

    fn row(version: &str, model: &str, pass: u32) -> EvidenceRow {
        EvidenceRow {
            key: EvidenceKey {
                genre: "coding".into(),
                harness_id: "codex".into(),
                harness_version: "1".into(),
                source_id: "codex-oauth".into(),
                model_id: model.into(),
            },
            evidence_version: version.into(),
            n: 2,
            pass,
            pass_lower: 0.1,
            pass_upper: 0.9,
            wall_median_s: Some(10.0),
            usd_list_median: None,
            quota_per_run: BTreeMap::new(),
            timeout_rate: 0.0,
            low_confidence: false,
        }
    }

    #[test]
    fn routing_evidence_replaces_same_version_and_appends_another() {
        let store = SqliteStore::open_in_memory().expect("open store");
        store
            .insert_routing_evidence(&[row("v1", "m1", 1)])
            .expect("insert v1");
        store
            .insert_routing_evidence(&[row("v1", "m2", 2)])
            .expect("replace v1");
        store
            .insert_routing_evidence(&[row("v2", "m1", 0)])
            .expect("append v2");
        let v1 = store.list_routing_evidence(Some("v1")).expect("list v1");
        assert_eq!(v1.len(), 1);
        assert_eq!(v1[0].key.model_id, "m2");
        assert_eq!(
            store.list_routing_evidence(None).expect("list all").len(),
            2
        );
    }
}
