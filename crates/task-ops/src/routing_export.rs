//! Deterministic, read-only export of routing decisions as ADR D8 training rows.
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

const TRAINING_SCHEMA: &str = "celeris.route-training/1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportText {
    #[default]
    Brief,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions<'a> {
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
    pub stage: Option<&'a str>,
    pub text: ExportText,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("failed to open routing database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("failed to serialize routing record: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to write routing export: {0}")]
    Io(#[from] std::io::Error),
}

/// Export decisions from the explicitly supplied database. The database is opened read-only.
/// Returns the number of JSONL rows written.
pub fn export(
    db_path: impl AsRef<Path>,
    options: &ExportOptions<'_>,
    out: &mut impl Write,
) -> Result<usize, ExportError> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let mut stmt = conn.prepare(
        "SELECT d.decision_id,d.subject_kind,d.subject_id,d.features_hash,d.options_hash,\
                d.input_json,d.output_json,d.judge_json,d.created_at,\
                f.schema,f.template,f.brief,f.aux_json,\
                o.builder,o.question,o.options_json,\
                r.schema,r.labels_json \
         FROM route_decisions d \
         LEFT JOIN route_features f ON f.hash=d.features_hash \
         JOIN route_options o ON o.hash=d.options_hash \
         LEFT JOIN route_outcomes r ON r.decision_id=d.decision_id \
         WHERE (?1 IS NULL OR d.created_at>=?1) \
           AND (?2 IS NULL OR d.created_at<=?2) \
           AND (?3 IS NULL OR d.stage=?3) \
         ORDER BY d.decision_id",
    )?;
    let mut rows = stmt.query(params![options.since, options.until, options.stage])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let decision_id: String = row.get(0)?;
        let subject_kind: String = row.get(1)?;
        let subject_id: String = row.get(2)?;
        let features_hash: String = row.get(3)?;
        let _options_hash: String = row.get(4)?;
        let input: Value = serde_json::from_str(&row.get::<_, String>(5)?)?;
        let output: Value = serde_json::from_str(&row.get::<_, String>(6)?)?;
        let judge: Value = serde_json::from_str(&row.get::<_, String>(7)?)?;
        let _created_at: String = row.get(8)?;
        let f_schema: Option<String> = row.get(9)?;
        let f_template: Option<String> = row.get(10)?;
        let f_brief: Option<String> = row.get(11)?;
        let f_aux: Option<String> = row.get(12)?;
        let o_builder: String = row.get(13)?;
        let o_question: String = row.get(14)?;
        let o_options: String = row.get(15)?;
        let outcome_schema: Option<String> = row.get(16)?;
        let outcome_labels: Option<String> = row.get(17)?;

        let mut features = serde_json::Map::new();
        features.insert("hash".into(), json!(features_hash));
        if let (Some(schema), Some(template)) = (f_schema, f_template) {
            features.insert("schema".into(), json!(schema));
            features.insert("template".into(), json!(template));
            if options.text == ExportText::Brief
                && let Some(brief) = f_brief
            {
                features.insert("brief".into(), json!(brief));
            }
            if let Some(aux) = f_aux {
                features.insert("aux".into(), serde_json::from_str(&aux)?);
            }
        }
        let outcome = match (outcome_schema, outcome_labels) {
            (Some(schema), Some(labels)) => {
                json!({"schema":schema,"labels":serde_json::from_str::<Value>(&labels)?})
            }
            _ => Value::Null,
        };
        let evidence_version = input
            .get("evidence_version")
            .or_else(|| input.get("context").and_then(|v| v.get("evidence_version")))
            .cloned()
            .unwrap_or(Value::Null);
        let record = TrainingRow {
            schema: TRAINING_SCHEMA,
            decision_id,
            subject: json!({"kind":subject_kind,"id":subject_id}),
            features: Value::Object(features),
            options: json!({"builder":o_builder,"question":o_question,"options":serde_json::from_str::<Value>(&o_options)?}),
            input,
            output,
            judge,
            outcome,
            evidence_version,
        };
        serde_json::to_writer(&mut *out, &record)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    Ok(count)
}

#[derive(Serialize)]
struct TrainingRow {
    schema: &'static str,
    decision_id: String,
    subject: Value,
    features: Value,
    options: Value,
    input: Value,
    output: Value,
    judge: Value,
    outcome: Value,
    evidence_version: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{
        route_record::{
            FeaturesInput, ROUTE_OPTIONS_SCHEMA, ROUTE_OUTCOME_SCHEMA, RouteOption,
            RouteOptionBinds, RouteOptions, RouteOutcome, build_task_features,
        },
        store::{SqliteStore, StoredRouteDecision, StoredRouteOutcome},
    };

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing.sqlite3");
        let store = SqliteStore::open(&path).unwrap();
        let features = build_task_features(&FeaturesInput {
            title: Some("task".into()),
            objective: Some("objective".into()),
            ..Default::default()
        });
        let options = RouteOptions {
            schema: ROUTE_OPTIONS_SCHEMA.into(),
            builder: "builder/1".into(),
            question: "choose".into(),
            options: vec![RouteOption {
                option_id: "a".into(),
                text: "Option A".into(),
                binds: Some(RouteOptionBinds {
                    source_id: "s".into(),
                    model_id: "m".into(),
                    harness_id: "h".into(),
                }),
            }],
        };
        store
            .insert_route_features(
                &features.hash,
                "task",
                "subject",
                &features,
                "2026-01-01T00:00:00Z",
            )
            .unwrap();
        store
            .insert_route_options("options-hash", &options, "2026-01-01T00:00:00Z")
            .unwrap();
        for id in ["z", "a"] {
            store.insert_route_decision(&StoredRouteDecision { decision_id:id.into(), subject_kind:"task".into(), subject_id:"subject".into(), run_id:None, features_hash:features.hash.clone(), options_hash:"options-hash".into(), input:json!({"context":{"stage":"eval","evidence_version":"evidence/1"},"constraints":{},"candidates":[]}), output:json!({"selected":"a","action":"execute","scores":[]}), judge:json!({"kind":"rule"}), stage:"eval".into(), created_at: if id == "a" {"2026-01-01T00:00:00Z"} else {"2026-01-03T00:00:00Z"}.into() }).unwrap();
        }
        store
            .insert_route_outcome(&StoredRouteOutcome {
                decision_id: "a".into(),
                outcome: RouteOutcome {
                    schema: ROUTE_OUTCOME_SCHEMA.into(),
                    labels: json!({"run_status":"completed"}),
                },
                recorded_at: "2026-01-04T00:00:00Z".into(),
            })
            .unwrap();
        drop(store);
        (dir, path)
    }

    #[test]
    fn routing_export_is_byte_stable_ordered_and_joins_outcome() {
        let (_dir, path) = fixture();
        let mut first = Vec::new();
        let mut second = Vec::new();
        let n = export(&path, &ExportOptions::default(), &mut first).unwrap();
        export(&path, &ExportOptions::default(), &mut second).unwrap();
        assert_eq!(n, 2);
        assert_eq!(first, second);
        let rows = String::from_utf8(first)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str::<Value>(s).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows[0]["decision_id"], "a");
        assert_eq!(rows[1]["decision_id"], "z");
        assert_eq!(rows[0]["schema"], TRAINING_SCHEMA);
        assert_eq!(rows[0]["outcome"]["schema"], ROUTE_OUTCOME_SCHEMA);
        assert!(rows[1]["outcome"].is_null());
    }

    #[test]
    fn routing_export_text_filter_and_window_are_applied() {
        let (_dir, path) = fixture();
        let mut bytes = Vec::new();
        let n = export(
            &path,
            &ExportOptions {
                since: Some("2026-01-02T00:00:00Z"),
                until: None,
                stage: Some("eval"),
                text: ExportText::None,
            },
            &mut bytes,
        )
        .unwrap();
        assert_eq!(n, 1);
        let row: Value = serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap()).unwrap();
        assert!(row["features"].get("brief").is_none());
        assert!(row["features"]["hash"].as_str().is_some());
    }

    #[test]
    fn routing_export_missing_feature_record_keeps_decision_hash() {
        let (_dir, path) = fixture();
        let conn = Connection::open(&path).unwrap();
        conn.execute("PRAGMA foreign_keys=OFF", []).unwrap();
        conn.execute("DELETE FROM route_features", []).unwrap();
        drop(conn);
        let mut bytes = Vec::new();
        assert_eq!(
            export(&path, &ExportOptions::default(), &mut bytes).unwrap(),
            2
        );
        let row: Value =
            serde_json::from_slice(bytes.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert!(row["features"].get("hash").is_some());
        assert!(row["features"].get("brief").is_none());
        assert!(row["features"].get("schema").is_none());
    }
}
