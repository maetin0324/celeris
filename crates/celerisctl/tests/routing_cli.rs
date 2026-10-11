//! CLI coverage for the versioned routing training JSONL export.
use rusqlite::Connection;
use serde_json::Value;
use std::process::{Command, Output};

fn ctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args(args)
        .env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        .env_remove("CELERIS_RUN_DB")
        .output()
        .expect("spawn celerisctl")
}

fn fixture(path: &std::path::Path) {
    let db = Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE route_features(hash TEXT PRIMARY KEY,schema TEXT,template TEXT,subject_kind TEXT,subject_id TEXT,brief TEXT,aux_json TEXT,created_at TEXT);
      CREATE TABLE route_options(hash TEXT PRIMARY KEY,builder TEXT,question TEXT,options_json TEXT,created_at TEXT);
      CREATE TABLE route_decisions(decision_id TEXT PRIMARY KEY,subject_kind TEXT,subject_id TEXT,run_id TEXT,features_hash TEXT,options_hash TEXT,input_json TEXT,output_json TEXT,judge_json TEXT,stage TEXT,created_at TEXT);
      CREATE TABLE route_outcomes(decision_id TEXT PRIMARY KEY,schema TEXT,labels_json TEXT,recorded_at TEXT);
      INSERT INTO route_features VALUES('sha256:feature','celeris.task-features/1','task-brief/2','task','t1','safe brief','{}','2026-10-11T00:00:00Z');
      INSERT INTO route_options VALUES('sha256:options','candidates-one-shot/1','Choose','[{\"option_id\":\"o1\",\"text\":\"candidate\",\"binds\":null}]','2026-10-11T00:00:00Z');
      INSERT INTO route_decisions VALUES('d1','task','t1',NULL,'sha256:feature','sha256:options','{\"context\":{},\"constraints\":{},\"candidates\":[]}','{\"selected\":{},\"action\":\"execute\",\"scores\":[],\"rule\":\"evidence\",\"propensity\":1}','{\"kind\":\"rule\"}','shadow','2026-10-11T00:00:00Z');").unwrap();
}

#[test]
fn routing_export_emits_one_training_row_and_supports_text_omission() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fixture.db");
    fixture(&db);
    let out = dir.path().join("training.jsonl");
    let args = [
        "routing",
        "export",
        "--db",
        db.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ];
    let result = ctl(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = std::fs::read_to_string(&out).unwrap();
    assert_eq!(text.lines().count(), 1);
    let row: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(row["schema"], "celeris.route-training/1");
    for key in [
        "decision_id",
        "subject",
        "features",
        "options",
        "input",
        "output",
        "judge",
        "outcome",
        "evidence_version",
    ] {
        assert!(row.get(key).is_some(), "missing {key}");
    }
    assert_eq!(row["decision_id"], "d1");
    let no_text = dir.path().join("no-text.jsonl");
    let omitted = ctl(&[
        "routing",
        "export",
        "--db",
        db.to_str().unwrap(),
        "--out",
        no_text.to_str().unwrap(),
        "--text",
        "none",
    ]);
    assert!(
        omitted.status.success(),
        "{}",
        String::from_utf8_lossy(&omitted.stderr)
    );
    let row: Value =
        serde_json::from_str(std::fs::read_to_string(no_text).unwrap().trim()).unwrap();
    assert!(row["features"].get("brief").is_none());
    assert_eq!(row["features"]["hash"], "sha256:feature");
    let no_db = ctl(&[
        "routing",
        "export",
        "--out",
        dir.path().join("missing.jsonl").to_str().unwrap(),
    ]);
    assert!(!no_db.status.success());
    assert!(String::from_utf8_lossy(&no_db.stderr).contains("--db"));
}
