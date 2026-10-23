use super::*;
#[test]
fn paired_comparisons_never_cross_harnesses() {
    let problem = Problem {
        id: "p".into(),
        task_id: "t".into(),
        work_unit_id: "w".into(),
        created_at: "".into(),
        genre: "coding".into(),
        kind: "task".into(),
        command: "check".into(),
        prompt: "Fixture problem description".into(),
        split: "dev".into(),
        stratum: "coding:task".into(),
        subset_12: true,
        head_commit: None,
        base_commit: None,
        base_commit_from_parent: false,
        features_sha256: "".into(),
        features: json!({}),
        sha256: "".into(),
    };
    let manifest = Manifest {
        schema: "x".into(),
        version: "1".into(),
        cutoff: "".into(),
        seed: 0,
        problems: vec![problem],
        refill_rejected: Vec::new(),
        sha256: "x".into(),
    };
    let make_trial = |harness: &str, source: &str| Trial {
        decision_id: source.into(),
        problem_id: "p".into(),
        trial_no: 1,
        key: EvidenceKey {
            lane: String::new(),
            genre: "coding".into(),
            harness_id: harness.into(),
            harness_version: "1".into(),
            source_id: source.into(),
            model_id: "m".into(),
        },
        passed: true,
        timed_out: false,
        wall_secs: None,
        usd_list: None,
        quota: BTreeMap::new(),
        status: "passed".into(),
        eval_status: None,
    };
    let comparisons = paired_comparisons(
        &manifest,
        &[
            make_trial("claude_code", "claude_oauth"),
            make_trial("codex", "codex_oauth"),
        ],
    );
    assert!(comparisons.is_empty());
}

#[test]
fn aggregate_emits_intervals_metrics_pareto_and_k1_low_confidence() {
    let p = Problem {
        id: "p1".into(),
        task_id: "t".into(),
        work_unit_id: "w".into(),
        created_at: "".into(),
        genre: "coding".into(),
        kind: "task".into(),
        command: "check".into(),
        prompt: "Fixture problem description".into(),
        split: "dev".into(),
        stratum: "coding:task".into(),
        subset_12: true,
        head_commit: None,
        base_commit: None,
        base_commit_from_parent: false,
        features_sha256: "".into(),
        features: json!({}),
        sha256: "".into(),
    };
    let m = Manifest {
        schema: "manifest".into(),
        version: "1".into(),
        cutoff: "".into(),
        seed: 0,
        problems: vec![p],
        refill_rejected: Vec::new(),
        sha256: "manifest-hash".into(),
    };
    let trial = Trial {
        decision_id: "d".into(),
        problem_id: "p1".into(),
        trial_no: 1,
        key: EvidenceKey {
            lane: String::new(),
            genre: "coding".into(),
            harness_id: "claude_code".into(),
            harness_version: "1".into(),
            source_id: "claude_oauth".into(),
            model_id: "m".into(),
        },
        passed: true,
        timed_out: false,
        wall_secs: Some(3.0),
        usd_list: Some(0.4),
        quota: BTreeMap::from([("7d".into(), 0.02)]),
        status: "passed".into(),
        eval_status: None,
    };
    let (agg, detail) = build(&m, &[trial]);
    assert_eq!(agg.rows.len(), 1);
    let row = &agg.rows[0];
    assert_eq!(row.n, 1);
    assert_eq!(row.pass, 1);
    assert!(row.pass_lower < 1.0 && row.pass_upper == 1.0);
    assert_eq!((row.pass_boot_lower, row.pass_boot_upper), (1.0, 1.0));
    assert!(row.low_confidence);
    assert_eq!(row.usd_per_pass, Some(0.4));
    assert_eq!(row.quota_per_run["7d"], 0.02);
    assert_eq!(detail.len(), 1);
    assert_eq!(agg.pareto.len(), 1);
}

#[test]
fn collect_reads_eval_decision_outcome_and_run_from_fixture_db() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("eval.sqlite");
    let conn = Connection::open(&db).expect("db");
    conn.execute_batch("CREATE TABLE route_decisions(decision_id TEXT,subject_id TEXT,subject_kind TEXT,run_id TEXT,input_json TEXT,output_json TEXT,stage TEXT); CREATE TABLE route_outcomes(decision_id TEXT,labels_json TEXT); CREATE TABLE runs(run_id TEXT,status TEXT,started_at TEXT,finished_at TEXT,usage_json TEXT,metrics_json TEXT); CREATE TABLE eval_trials(decision_id TEXT,status TEXT);")
        .expect("fixture schema");
    conn.execute("INSERT INTO route_decisions VALUES('d','p1','eval_problem','r1',?1,?2,'eval')", rusqlite::params![r#"{"manifest_sha":"mh","trial_no":2}"#,r#"{"selected":{"source_id":"s","model_id":"m","harness_id":"codex","harness_version":"v2.3.1"}}"#]).expect("decision");
    conn.execute(
        "INSERT INTO route_outcomes VALUES('d',?1)",
        [r#"{"passed":true,"timed_out":false,"wall_secs":4.0,"usd_list":0.3,"quota":{"7d":0.01}}"#],
    )
    .expect("outcome");
    conn.execute("INSERT INTO runs VALUES('r1','passed','2026-10-01T00:00:00Z','2026-10-01T00:00:05Z',NULL,NULL)", []).expect("run");
    drop(conn);
    let problem = Problem {
        id: "p1".into(),
        task_id: "t".into(),
        work_unit_id: "w".into(),
        created_at: "".into(),
        genre: "coding".into(),
        kind: "task".into(),
        command: "check".into(),
        prompt: "Fixture problem description".into(),
        split: "dev".into(),
        stratum: "coding:task".into(),
        subset_12: false,
        head_commit: None,
        base_commit: None,
        base_commit_from_parent: false,
        features_sha256: "".into(),
        features: json!({}),
        sha256: "".into(),
    };
    let manifest = Manifest {
        schema: "manifest".into(),
        version: "1".into(),
        cutoff: "".into(),
        seed: 0,
        problems: vec![problem],
        refill_rejected: Vec::new(),
        sha256: "mh".into(),
    };
    let rows = collect(db.to_str().expect("path"), &manifest).expect("collect");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].trial_no, 2);
    assert_eq!(rows[0].key.harness_version, "2");
    assert!(rows[0].passed);
    assert_eq!(rows[0].wall_secs, Some(4.0));
    assert_eq!(rows[0].quota["7d"], 0.01);
}

#[test]
fn aggregate_excludes_error_trials() {
    let problem = |id: &str| Problem {
        id: id.into(),
        task_id: "t".into(),
        work_unit_id: "w".into(),
        created_at: "".into(),
        genre: "coding".into(),
        kind: "task".into(),
        command: "check".into(),
        prompt: "Fixture problem description".into(),
        split: "dev".into(),
        stratum: "coding:task".into(),
        subset_12: false,
        head_commit: None,
        base_commit: None,
        base_commit_from_parent: false,
        features_sha256: "".into(),
        features: json!({}),
        sha256: "".into(),
    };
    let manifest = Manifest {
        schema: "manifest".into(),
        version: "1".into(),
        cutoff: "".into(),
        seed: 0,
        problems: ["p1", "p2", "p3", "p4"].map(problem).to_vec(),
        refill_rejected: Vec::new(),
        sha256: "mh".into(),
    };
    // eval_trials を持つ実 DB の形で collect から通す: done（passed / failed）2 件、
    // error 2 件（route_outcomes なし・runs は error）、cancelled 1 件。
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("eval.sqlite");
    let conn = Connection::open(&db).expect("db");
    conn.execute_batch("CREATE TABLE route_decisions(decision_id TEXT,subject_id TEXT,subject_kind TEXT,run_id TEXT,input_json TEXT,output_json TEXT,stage TEXT); CREATE TABLE route_outcomes(decision_id TEXT,labels_json TEXT); CREATE TABLE runs(run_id TEXT,status TEXT,started_at TEXT,finished_at TEXT,usage_json TEXT,metrics_json TEXT); CREATE TABLE eval_trials(trial_id TEXT,decision_id TEXT,status TEXT,error TEXT);")
        .expect("fixture schema");
    let selected = r#"{"selected":{"source_id":"codex_oauth","model_id":"m","harness_id":"codex","harness_version":"1.0"}}"#;
    let trials = [
        ("d1", "p1", "passed", Some(true)),
        ("d2", "p2", "failed", Some(false)),
        ("d3", "p3", "error", None),
        ("d4", "p4", "error", None),
        ("d5", "p4", "cancelled", None),
    ];
    for (n, (decision, problem_id, status, passed)) in trials.iter().enumerate() {
        let run = format!("r{n}");
        conn.execute(
            "INSERT INTO route_decisions VALUES(?1,?2,'eval_problem',?3,?4,?5,'eval')",
            rusqlite::params![
                decision,
                problem_id,
                run,
                json!({"trial_no": n + 1}).to_string(),
                selected
            ],
        )
        .expect("decision");
        let run_status = if passed.is_some() {
            "completed"
        } else {
            "error"
        };
        conn.execute(
            "INSERT INTO runs VALUES(?1,?2,NULL,NULL,NULL,NULL)",
            rusqlite::params![run, run_status],
        )
        .expect("run");
        if let Some(passed) = passed {
            conn.execute(
                "INSERT INTO route_outcomes VALUES(?1,?2)",
                rusqlite::params![decision, json!({"passed": passed}).to_string()],
            )
            .expect("outcome");
        }
        conn.execute(
            "INSERT INTO eval_trials VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                format!("t{n}"),
                decision,
                status,
                (passed.is_none()).then_some("run ended as error")
            ],
        )
        .expect("trial");
    }
    drop(conn);

    let collected = collect(db.to_str().expect("path"), &manifest).expect("collect");
    // 古い error 行は消さずに読む（除外は集計の側で行う）。
    assert_eq!(collected.len(), 5);
    let (agg, detail) = build(&manifest, &collected);
    assert_eq!(agg.rows.len(), 1);
    let row = &agg.rows[0];
    // 母数と Wilson 区間は done の 2 件（1 合格・1 不合格）だけから出る。
    assert_eq!((row.n, row.pass), (2, 1));
    let done_only = stats::wilson_95(1, 2);
    assert_eq!(
        (row.pass_lower, row.pass_upper),
        (done_only.lower, done_only.upper)
    );
    let with_errors = stats::wilson_95(1, 4);
    assert_ne!(row.pass_lower, with_errors.lower);
    assert_eq!(
        agg.excluded_for(&row.key),
        ExcludedCount {
            error: 2,
            cancelled: 1
        }
    );
    assert_eq!(detail[0]["excluded"]["error"], 2);
    assert_eq!(detail[0]["excluded"]["cancelled"], 1);

    // 根拠表（evidence.md / evidence.json）に除外数が出る。
    let manifest_path = dir.path().join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest).expect("manifest json"),
    )
    .expect("manifest");
    let out = dir.path().join("out");
    run_args(vec![
        "--db".into(),
        db.display().to_string(),
        "--manifest".into(),
        manifest_path.display().to_string(),
        "--out".into(),
        out.display().to_string(),
    ])
    .expect("aggregate");
    let md = fs::read_to_string(out.join("evidence.md")).expect("evidence.md");
    assert!(md.contains("| Excluded (error/cancelled) |"), "{md}");
    assert!(
        md.contains("| 2 | 1 | 0.") && md.contains("| 2/1 |"),
        "{md}"
    );
    assert!(
        md.contains("Excluded trials (error/cancelled; not counted in n, Pass or intervals): 3"),
        "{md}"
    );
    assert!(
        md.lines()
            .any(|line| line == "| coding | codex | 1.0 | codex_oauth | m | 2 | 1 |"),
        "{md}"
    );
    let evidence: Value =
        serde_json::from_slice(&fs::read(out.join("evidence.json")).expect("evidence.json"))
            .expect("evidence json");
    assert_eq!(evidence["excluded"][0]["error"], 2);
    assert_eq!(evidence["excluded"][0]["cancelled"], 1);
    assert_eq!(evidence["rows"][0]["n"], 2);
}
