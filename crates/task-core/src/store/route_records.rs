//! Append-only storage for versioned routing training records (ADR 2026-10-11 D7).
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{SqliteStore, StoreError};
use crate::route_record::{RouteOptions, RouteOutcome, TaskFeatures};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StoredRouteDecision {
    pub decision_id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub run_id: Option<String>,
    pub features_hash: String,
    pub options_hash: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub judge: serde_json::Value,
    pub stage: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StoredRouteJudgment {
    pub judgment_id: String,
    pub features_hash: String,
    pub options_hash: String,
    pub input_sha256: String,
    pub judge: serde_json::Value,
    pub probs: serde_json::Value,
    pub source: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StoredRouteOutcome {
    pub decision_id: String,
    pub outcome: RouteOutcome,
    pub recorded_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RouteDecisionRow {
    pub decision: StoredRouteDecision,
    pub features: TaskFeatures,
    pub options_hash: String,
    pub options: RouteOptions,
    pub outcome: Option<StoredRouteOutcome>,
}

impl SqliteStore {
    /// Content-addressed features are immutable; inserting an existing hash is a no-op.
    pub fn insert_route_features(
        &self,
        hash: &str,
        subject_kind: &str,
        subject_id: &str,
        features: &TaskFeatures,
        created_at: &str,
    ) -> Result<(), StoreError> {
        if hash != features.hash {
            return Err(StoreError::Invalid(
                "route feature hash does not match the feature record".into(),
            ));
        }
        let conn = self.lock()?;
        conn.execute("INSERT OR IGNORE INTO route_features(hash,schema,template,subject_kind,subject_id,brief,aux_json,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![hash, features.schema, features.template, subject_kind, subject_id, features.brief, serde_json::to_string(&features.aux)?, created_at])?;
        Ok(())
    }

    /// Content-addressed options are immutable; inserting an existing hash is a no-op.
    pub fn insert_route_options(
        &self,
        hash: &str,
        options: &RouteOptions,
        created_at: &str,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute("INSERT OR IGNORE INTO route_options(hash,schema,builder,question,options_json,created_at) VALUES (?1,?2,?3,?4,?5,?6)", params![hash, options.schema, options.builder, options.question, serde_json::to_string(&options.options)?, created_at])?;
        Ok(())
    }

    pub fn insert_route_decision(&self, d: &StoredRouteDecision) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute("INSERT INTO route_decisions(decision_id,subject_kind,subject_id,run_id,features_hash,options_hash,input_json,output_json,judge_json,stage,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![d.decision_id,d.subject_kind,d.subject_id,d.run_id,d.features_hash,d.options_hash,serde_json::to_string(&d.input)?,serde_json::to_string(&d.output)?,serde_json::to_string(&d.judge)?,d.stage,d.created_at])?;
        Ok(())
    }

    /// Write the one `celeris.route-decision/1` row of a routing decision (ADR 2026-10-11
    /// task-routing-features D6/D7): its `celeris.route-options/1` options (content-addressed, so a
    /// repeat is a no-op) and the decision row. The features snapshot must already exist
    /// (`route_decisions.features_hash` references `route_features`), so a decision taken without
    /// one is refused rather than recorded half-way.
    pub fn record_route_decision_impl(
        &self,
        decision: &crate::route_select::RouteDecision,
    ) -> Result<(), StoreError> {
        let Some(features_hash) = decision.features_hash.clone() else {
            return Err(StoreError::Invalid(
                "route decision without a task features snapshot".into(),
            ));
        };
        let Some(options_hash) = decision.options_hash.clone() else {
            return Err(StoreError::Invalid("route decision without options".into()));
        };
        if decision.decision_id.is_empty() {
            return Err(StoreError::Invalid(
                "route decision without a decision_id".into(),
            ));
        }
        let created_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| StoreError::Invalid(format!("format route decision timestamp: {e}")))?;
        self.insert_route_options(&options_hash, &decision.route_options(), &created_at)?;
        self.insert_route_decision(&StoredRouteDecision {
            decision_id: decision.decision_id.clone(),
            subject_kind: decision.subject_kind.clone(),
            subject_id: decision.subject_id.clone(),
            run_id: decision.run_id.clone(),
            features_hash,
            options_hash,
            input: decision.input_json(),
            output: decision.output_json(),
            judge: decision.judge_json(),
            stage: decision.stage.as_str().to_owned(),
            created_at,
        })
    }

    /// A decision has at most one outcome; a second insert returns the SQLite constraint error.
    pub fn insert_route_outcome(&self, outcome: &StoredRouteOutcome) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute("INSERT INTO route_outcomes(decision_id,schema,labels_json,recorded_at) VALUES (?1,?2,?3,?4)", params![outcome.decision_id,outcome.outcome.schema,serde_json::to_string(&outcome.outcome.labels)?,outcome.recorded_at])?;
        Ok(())
    }

    pub fn insert_route_judgment(&self, j: &StoredRouteJudgment) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute("INSERT INTO route_judgments(judgment_id,features_hash,options_hash,input_sha256,judge_json,probs_json,source,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![j.judgment_id,j.features_hash,j.options_hash,j.input_sha256,serde_json::to_string(&j.judge)?,serde_json::to_string(&j.probs)?,j.source,j.created_at])?;
        Ok(())
    }

    pub fn route_features_get(&self, hash: &str) -> Result<Option<TaskFeatures>, StoreError> {
        self.with_read_conn(|c| {
            c.query_row(
                "SELECT schema,template,brief,aux_json,hash FROM route_features WHERE hash=?1",
                [hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .map(|(schema, template, brief, aux, hash)| {
                Ok(TaskFeatures {
                    schema,
                    template,
                    brief,
                    aux: serde_json::from_str(&aux)?,
                    hash,
                })
            })
            .transpose()
        })
    }

    pub fn route_options_get(&self, hash: &str) -> Result<Option<RouteOptions>, StoreError> {
        self.with_read_conn(|c| {
            c.query_row(
                "SELECT schema,builder,question,options_json FROM route_options WHERE hash=?1",
                [hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .map(|(schema, builder, question, json)| {
                Ok(RouteOptions {
                    schema,
                    builder,
                    question,
                    options: serde_json::from_str(&json)?,
                })
            })
            .transpose()
        })
    }

    pub fn route_decision_get(&self, id: &str) -> Result<Option<StoredRouteDecision>, StoreError> {
        self.with_read_conn(|c| Ok(c.query_row("SELECT decision_id,subject_kind,subject_id,run_id,features_hash,options_hash,input_json,output_json,judge_json,stage,created_at FROM route_decisions WHERE decision_id=?1", [id], decision_row).optional()?))
    }

    pub fn route_outcome_get(&self, id: &str) -> Result<Option<StoredRouteOutcome>, StoreError> {
        self.with_read_conn(|c| c.query_row("SELECT decision_id,schema,labels_json,recorded_at FROM route_outcomes WHERE decision_id=?1", [id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?.map(|(decision_id,schema,labels,recorded_at)| Ok(StoredRouteOutcome { decision_id, outcome:RouteOutcome { schema,labels:serde_json::from_str(&labels)? }, recorded_at })).transpose())
    }

    pub fn route_judgment_get(&self, id: &str) -> Result<Option<StoredRouteJudgment>, StoreError> {
        self.with_read_conn(|c| c.query_row("SELECT judgment_id,features_hash,options_hash,input_sha256,judge_json,probs_json,source,created_at FROM route_judgments WHERE judgment_id=?1", [id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?))).optional()?.map(|(judgment_id,features_hash,options_hash,input_sha256,judge,probs,source,created_at)| Ok(StoredRouteJudgment { judgment_id,features_hash,options_hash,input_sha256,judge:serde_json::from_str(&judge)?,probs:serde_json::from_str(&probs)?,source,created_at })).transpose())
    }

    /// Judgments are reusable only for the exact feature/options content pair.
    pub fn route_judgments_for_hashes(
        &self,
        features_hash: &str,
        options_hash: &str,
    ) -> Result<Vec<StoredRouteJudgment>, StoreError> {
        self.with_read_conn(|c| {
            let mut stmt=c.prepare("SELECT judgment_id,features_hash,options_hash,input_sha256,judge_json,probs_json,source,created_at FROM route_judgments WHERE features_hash=?1 AND options_hash=?2 ORDER BY judgment_id")?;
            let rows=stmt.query_map(params![features_hash,options_hash], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?)))?;
            rows.map(|row| { let (judgment_id,features_hash,options_hash,input_sha256,judge,probs,source,created_at)=row?; Ok(StoredRouteJudgment { judgment_id,features_hash,options_hash,input_sha256,judge:serde_json::from_str(&judge)?,probs:serde_json::from_str(&probs)?,source,created_at }) }).collect()
        })
    }

    /// Decisions are returned in stable decision_id order for an inclusive time window.
    pub fn route_decisions_export(
        &self,
        since: Option<&str>,
        until: Option<&str>,
        stage: Option<&str>,
    ) -> Result<Vec<RouteDecisionRow>, StoreError> {
        self.with_read_conn(|c| {
            let mut stmt=c.prepare("SELECT decision_id,subject_kind,subject_id,run_id,features_hash,options_hash,input_json,output_json,judge_json,stage,created_at FROM route_decisions WHERE (?1 IS NULL OR created_at>=?1) AND (?2 IS NULL OR created_at<=?2) AND (?3 IS NULL OR stage=?3) ORDER BY decision_id")?;
            let decisions=stmt.query_map(params![since,until,stage],decision_row)?.collect::<Result<Vec<_>,_>>()?;
            decisions.into_iter().map(|d| {
                let feature_row=c.query_row("SELECT schema,template,brief,aux_json,hash FROM route_features WHERE hash=?1", [&d.features_hash], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?;
                let options_row=c.query_row("SELECT schema,builder,question,options_json FROM route_options WHERE hash=?1", [&d.options_hash], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?;
                let outcome=c.query_row("SELECT decision_id,schema,labels_json,recorded_at FROM route_outcomes WHERE decision_id=?1", [&d.decision_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?;
                let options_hash=d.options_hash.clone();
                let stored_outcome=match outcome { Some((decision_id,schema,labels,recorded_at))=>Some(StoredRouteOutcome { decision_id,outcome:RouteOutcome { schema,labels:serde_json::from_str(&labels)? },recorded_at }),None=>None };
                Ok(RouteDecisionRow { decision:d, features:TaskFeatures { schema:feature_row.0,template:feature_row.1,brief:feature_row.2,aux:serde_json::from_str(&feature_row.3)?,hash:feature_row.4 }, options_hash, options:RouteOptions { schema:options_row.0,builder:options_row.1,question:options_row.2,options:serde_json::from_str(&options_row.3)? }, outcome:stored_outcome })
            }).collect()
        })
    }
}

fn decision_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRouteDecision> {
    Ok(StoredRouteDecision {
        decision_id: r.get(0)?,
        subject_kind: r.get(1)?,
        subject_id: r.get(2)?,
        run_id: r.get(3)?,
        features_hash: r.get(4)?,
        options_hash: r.get(5)?,
        input: serde_json::from_str(&r.get::<_, String>(6)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?,
        output: serde_json::from_str(&r.get::<_, String>(7)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
        })?,
        judge: serde_json::from_str(&r.get::<_, String>(8)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?,
        stage: r.get(9)?,
        created_at: r.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route_record::{
        Candidate, FeaturesInput, ROUTE_OUTCOME_SCHEMA, build_task_features, candidates_one_shot,
    };
    use crate::store::{SCHEMA_VERSION, SqliteStore};

    fn fixtures() -> (TaskFeatures, RouteOptions) {
        (
            build_task_features(&FeaturesInput {
                title: Some("t".into()),
                objective: Some("o".into()),
                ..Default::default()
            }),
            candidates_one_shot(
                "choose",
                &[Candidate {
                    source_id: "s".into(),
                    model_id: "m".into(),
                    harness_id: "h".into(),
                    label: "Model m".into(),
                }],
            ),
        )
    }
    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().unwrap()
    }

    #[test]
    fn route_store_schema_is_pinned_and_has_all_tables() {
        let s = store();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 88);
        let c = s.lock().unwrap();
        for name in [
            "route_features",
            "route_options",
            "route_decisions",
            "route_outcomes",
            "route_judgments",
        ] {
            let found: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{name}");
        }
    }

    #[test]
    fn route_store_content_records_are_idempotent_and_readable() {
        let s = store();
        let (f, o) = fixtures();
        s.insert_route_features(&f.hash, "work_unit", "w1", &f, "2026-01-01T00:00:00Z")
            .unwrap();
        s.insert_route_features(&f.hash, "work_unit", "w1", &f, "2026-01-01T00:00:00Z")
            .unwrap();
        s.insert_route_options("oh", &o, "2026-01-01T00:00:00Z")
            .unwrap();
        s.insert_route_options("oh", &o, "2026-01-01T00:00:00Z")
            .unwrap();
        assert_eq!(s.route_features_get(&f.hash).unwrap().unwrap(), f);
        assert_eq!(s.route_options_get("oh").unwrap().unwrap(), o);
        let count: i64 = s
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM route_features", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    fn decision(id: &str, at: &str, stage: &str, features_hash: &str) -> StoredRouteDecision {
        StoredRouteDecision {
            decision_id: id.into(),
            subject_kind: "work_unit".into(),
            subject_id: "w1".into(),
            run_id: None,
            features_hash: features_hash.into(),
            options_hash: "oh".into(),
            input: serde_json::json!({}),
            output: serde_json::json!({}),
            judge: serde_json::json!({}),
            stage: stage.into(),
            created_at: at.into(),
        }
    }
    #[test]
    fn route_store_rejects_duplicate_decision_and_outcome_and_exports_in_id_order() {
        let s = store();
        let (f, o) = fixtures();
        s.insert_route_features(&f.hash, "work_unit", "w1", &f, "t")
            .unwrap();
        s.insert_route_options("oh", &o, "t").unwrap();
        s.insert_route_decision(&decision("z", "2026-01-02T00:00:00Z", "shadow", &f.hash))
            .unwrap();
        s.insert_route_decision(&decision("a", "2026-01-01T00:00:00Z", "eval", &f.hash))
            .unwrap();
        assert!(
            s.insert_route_decision(&decision("a", "2026-01-01T00:00:00Z", "eval", &f.hash))
                .is_err()
        );
        let outcome = StoredRouteOutcome {
            decision_id: "a".into(),
            outcome: RouteOutcome {
                schema: ROUTE_OUTCOME_SCHEMA.into(),
                labels: serde_json::json!({"run_status":"completed"}),
            },
            recorded_at: "2026-01-03T00:00:00Z".into(),
        };
        s.insert_route_outcome(&outcome).unwrap();
        assert!(s.insert_route_outcome(&outcome).is_err());
        let rows = s.route_decisions_export(None, None, Some("eval")).unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r.decision.decision_id.as_str())
                .collect::<Vec<_>>(),
            ["a"]
        );
        assert!(rows[0].outcome.is_some());
        assert_eq!(
            s.route_decisions_export(None, None, None)
                .unwrap()
                .iter()
                .map(|r| r.decision.decision_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
    }
}
