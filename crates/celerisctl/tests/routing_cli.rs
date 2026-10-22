//! CLI coverage for the versioned routing training JSONL export.
mod common;

use rusqlite::Connection;
use serde_json::Value;
use std::process::{Command, Output};

fn ctl(args: &[&str]) -> Output {
    Command::new(common::celerisctl_path())
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

fn stage_fixture(path: &std::path::Path) -> String {
    use task_core::TaskStore;
    use task_core::routing_evidence::{EvidenceKey, EvidenceRow};
    let store = task_core::SqliteStore::open(path).unwrap();
    let spec: task_ops::add::NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "t", "objective": "o",
        "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]
    }))
    .unwrap();
    let task = task_ops::add::create_task(&store, spec, time::OffsetDateTime::now_utc()).unwrap();
    let row = |n, pass_lower| EvidenceRow {
        quota_cost_per_pass: None,
        key: EvidenceKey {
            lane: String::new(),
            genre: "coding".into(),
            harness_id: "codex".into(),
            harness_version: "1.4.0".into(),
            source_id: "openai".into(),
            model_id: "gpt-5".into(),
        },
        evidence_version: String::new(),
        n,
        pass: n,
        pass_lower,
        pass_upper: 0.9,
        pass_boot_lower: pass_lower,
        pass_boot_upper: 0.9,
        wall_p90_s: Some(30.0),
        usd_per_pass: Some(0.2),
        quota_per_run: Default::default(),
        timeout_rate: 0.0,
        low_confidence: false,
    };
    store
        .replace_routing_evidence("v1", &[row(10, 0.3)])
        .unwrap();
    task.id.to_string()
}

#[test]
fn routing_stage_set_refuses_enforce_without_evidence_then_lists_the_human_change() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("celeris.db");
    let task = stage_fixture(&db);
    let db = db.to_str().unwrap();
    let set = |stage: &str| {
        ctl(&[
            "routing",
            "stage",
            "set",
            "--db",
            db,
            "--task",
            &task,
            "--genre",
            "coding",
            "--harness",
            "codex",
            "--stage",
            stage,
            "--reason",
            "reviewed",
        ])
    };
    let refused = set("enforce");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("enforce requires qualifying evidence"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let shadow = set("shadow");
    assert!(
        shadow.status.success(),
        "{}",
        String::from_utf8_lossy(&shadow.stderr)
    );
    let list = ctl(&["routing", "stage", "list", "--db", db]);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let text = String::from_utf8_lossy(&list.stdout);
    assert!(text.starts_with("coding/codex: shadow since="), "{text}");
    assert!(
        text.contains("evidence=v1 latest=v1 last=-->shadow by human"),
        "{text}"
    );
    let json = ctl(&["routing", "stage", "list", "--db", db, "--json"]);
    let value: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["stages"][0]["last_change"]["task_id"], task);
    let evidence = ctl(&["routing", "evidence", "--db", db, "--genre", "coding"]);
    assert!(
        evidence.status.success(),
        "{}",
        String::from_utf8_lossy(&evidence.stderr)
    );
    let text = String::from_utf8_lossy(&evidence.stdout);
    assert!(text.contains("=== coding/codex (v1) ==="), "{text}");
    assert!(
        text.contains("openai/gpt-5 harness=1.4.0 n=10 pass=10 [0.300, 0.900]"),
        "{text}"
    );
    assert!(!text.contains("qualifies"), "{text}");
    let missing = ctl(&["routing", "stage", "list"]);
    assert!(!missing.status.success());
}
