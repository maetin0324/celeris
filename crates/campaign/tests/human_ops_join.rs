//! `human-op join`: 生記録を Celeris 側の記録（trials/ledger/export records）と同じ identity で結合する。
//! mock 実行（pilot の mock_e2e と同じ組み立て）の A/B 試行を使う。外部 network・LLM は使わない。
mod common;
use std::{collections::BTreeMap, fs, path::Path, process::Command};

use celeris_campaign::{
    executor::{MockExecutor, MockStep},
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn campaign(i: &Input, n: usize) -> CampaignManifest {
    let cfg = |id, h: &str| Config {
        id,
        harness: h.into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: None,
    };
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "e2e".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: (0..n).map(|k| format!("p{k}")).collect(),
        configs: vec![
            cfg(ConfigId::A, "claude-code"),
            cfg(ConfigId::B, "celeris"),
            cfg(ConfigId::C, "celeris"),
        ],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 64,
            per_case: Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_800,
            },
            per_arm: Cap {
                max_tokens: 10_000_000_000,
                max_wall_secs: 1_000_000,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: true,
        },
        snapshot: SnapshotRef {
            repo_commit: "abc".into(),
            materials_sha256: "d".into(),
            kb_snapshot: "kb@1".into(),
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    }
}

fn cli(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
        .arg("human-op")
        .args(args)
        .env("CELERIS_HUMAN_OPS_NOW", "2026-10-19T01:00:00Z")
        .env("CELERIS_HUMAN_OPS_HOST", "eval-host")
        .output()
        .expect("spawn");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// 評価者記録: 同じ command 列を arm だけ変えて書く。p0 の op は終了時刻が不明（op_end 無し）。
fn record_ops(file: &Path, hash: &str, arm: &str, case: &str, task: Option<&str>) {
    let f = file.to_str().expect("path");
    let mut base = vec![
        "--file",
        f,
        "--campaign",
        "e2e",
        "--manifest-sha256",
        hash,
        "--case",
        case,
        "--arm",
        arm,
        "--rep",
        "1",
    ];
    if let Some(t) = task {
        base.extend(["--task-id", t]);
    }
    let mut known = vec!["record"];
    known.extend(&base);
    known.extend([
        "--op-kind",
        "research_direction",
        "--actor",
        "eval1",
        "--mode",
        "manual",
        "--intent",
        "research",
        "--logical-op",
        "o1",
        "--started-at",
        "2026-10-19T00:10:00Z",
        "--ended-at",
        "2026-10-19T00:12:00Z",
    ]);
    let (c, _, e) = cli(&known);
    assert_eq!(c, 0, "{e}");
    let mut unknown = vec!["record"];
    unknown.extend(&base);
    unknown.extend([
        "--op-kind",
        "status_check",
        "--actor",
        "eval1",
        "--mode",
        "manual",
        "--intent",
        "monitoring",
        "--logical-op",
        "o2",
        "--started-at",
        "2026-10-19T00:20:00Z",
    ]);
    let (c, _, e) = cli(&unknown);
    assert_eq!(c, 0, "{e}");
}

fn join_rows(file: &Path, events: &Path) -> Vec<Value> {
    let (c, out, e) = cli(&[
        "join",
        "--file",
        file.to_str().expect("p"),
        "--events",
        events.to_str().expect("p"),
    ]);
    assert_eq!(c, 0, "{e}");
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("jsonl"))
        .collect()
}

fn row<'a>(rows: &'a [Value], arm: &str, case: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["arm"] == arm && r["case_id"] == case && r["row_type"] == "trial")
        .unwrap_or_else(|| panic!("row {arm}/{case} in {rows:?}"))
}

fn src<'a>(r: &'a Value, f: &str) -> &'a str {
    r[f]["source"]
        .as_str()
        .unwrap_or_else(|| panic!("{f} in {r}"))
}

/// export の records.jsonl に相当する、B 腕の Celeris event 由来の行（重複・再 export を含む）。
fn write_records(dir: &Path, hash: &str, dup: bool) {
    let run_row = json!({"record_type":"run","dedup_key":"run:r1","campaign_id":"e2e","arm":"B",
        "case_id":"p0","repetition":1,"manifest_hash":hash,"task_id":"t-b-p0","run_id":"r1"});
    let outcome = json!({"record_type":"outcome","dedup_key":"outcome:r1","campaign_id":"e2e",
        "run_id":"r1","wall_ms":90_000});
    let iv = json!({"record_type":"intervention","dedup_key":"intervention:i1","campaign_id":"e2e",
        "intervention_id":"i1","kind":"research_judgment","task_id":"t-b-p0","at":"2026-10-19T00:11:00Z",
        "wall_ms":null});
    let stray = json!({"record_type":"intervention","dedup_key":"intervention:i9","campaign_id":"e2e",
        "intervention_id":"i9","kind":"recovery","task_id":"t-unknown","at":"2026-10-19T00:11:00Z","wall_ms":null});
    let mut rows = vec![run_row, outcome, iv, stray];
    if dup {
        rows.extend(rows.clone());
    }
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    fs::write(dir.join("records.jsonl"), text).expect("records");
}

#[test]
fn joins_on_same_identity_without_duplicates_and_a_arm_keeps_missing() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, 2);
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::scripted({
            let mut s = BTreeMap::new();
            s.insert((ConfigId::A, "p1".to_string()), MockStep::Fail);
            s
        }),
        &RunOptions {
            out: out.clone(),
            ..RunOptions::default()
        },
    )
    .expect("run");
    let hash = m.compute_sha256().expect("hash");
    let ops = d.path().join("human-ops.jsonl");
    record_ops(&ops, &hash, "A0", "p0", None);
    record_ops(&ops, &hash, "B0", "p0", Some("t-b-p0"));
    write_records(&out, &hash, false);

    let rows = join_rows(&ops, &out);
    let (a, b) = (row(&rows, "A", "p0"), row(&rows, "B", "p0"));
    // 同じ identity で 1 行ずつ。A/B の p1・C の p0 は評価者記録が無いが event 側の行として残る。
    assert_eq!(rows.iter().filter(|r| r["row_type"] == "trial").count(), 5);

    // B: TrialRecord.wall_secs と export の run/outcome/intervention は event 由来、人の操作は評価者記録。
    assert_eq!(src(b, "wall_secs"), "event");
    assert_eq!(src(b, "run_ids"), "event");
    assert_eq!(b["run_ids"]["value"], json!(["r1"]));
    assert_eq!(b["run_wall_ms"]["value"]["observed_sum"], 90_000);
    assert_eq!(
        b["event_interventions"]["value"],
        json!({"research_judgment": 1})
    );
    assert_eq!(src(b, "human_ops"), "evaluator");
    assert_eq!(
        b["human_ops"]["value"],
        json!({"research_direction": 1, "status_check": 1})
    );
    // op_end が無い操作の時間は 0 にならず、欠測数に出る。
    assert_eq!(
        b["human_op_secs"]["value"],
        json!({"observed_sum": 120, "observed": 1, "missing": 1})
    );
    assert_eq!(b["arm_labels"], json!(["B0"]));
    // task_id が試行に無い介入は試行へ結ばず、別行に出所つきで残す。
    let unj: Vec<&Value> = rows
        .iter()
        .filter(|r| r["row_type"] == "unjoined_event")
        .collect();
    assert_eq!(unj.len(), 1);
    assert_eq!(unj[0]["intervention_id"], "i9");

    // A: Celeris event が無い。評価者記録のみで、欠測は 0 にならない。
    assert_eq!(a["arm_labels"], json!(["A0"]));
    assert_eq!(src(a, "human_ops"), "evaluator");
    for f in [
        "event_interventions",
        "run_wall_ms",
        "human_wait",
        "job_observations",
    ] {
        assert_eq!(src(a, f), "missing", "{f}");
        assert_eq!(a[f]["value"], Value::Null, "{f}");
    }
    assert!(a["missing_count"].as_u64().expect("n") >= 4, "{a}");
    assert!(
        a["missing_fields"]
            .as_array()
            .expect("a")
            .contains(&json!("event_interventions"))
    );
    assert_eq!(a["human_op_secs"]["value"]["missing"], 1);

    // 再開・重複: events の行を二重にし、評価者の command 列を再実行しても、結合結果は byte 一致。
    let first = fs::read_to_string({
        let p = d.path().join("first.jsonl");
        let (_, o, _) = cli(&[
            "join",
            "--file",
            ops.to_str().expect("p"),
            "--events",
            out.to_str().expect("p"),
        ]);
        fs::write(&p, o).expect("w");
        p
    })
    .expect("r");
    write_records(&out, &hash, true);
    record_ops(&ops, &hash, "A0", "p0", None);
    record_ops(&ops, &hash, "B0", "p0", Some("t-b-p0"));
    let (_, again, _) = cli(&[
        "join",
        "--file",
        ops.to_str().expect("p"),
        "--events",
        out.to_str().expect("p"),
    ]);
    assert_eq!(again, first);
}

#[test]
fn rejects_missing_events_input() {
    let d = tempfile::tempdir().expect("dir");
    let ops = d.path().join("h.jsonl");
    fs::write(&ops, "").expect("w");
    let (c, _, e) = cli(&[
        "join",
        "--file",
        ops.to_str().expect("p"),
        "--events",
        d.path().to_str().expect("p"),
    ]);
    assert_ne!(c, 0);
    assert!(e.contains("records.jsonl"), "{e}");
}

fn join_multi(file: &Path, events: &[&Path]) -> (i32, String, String) {
    let mut a = vec!["join", "--file", file.to_str().expect("p")];
    for e in events {
        a.extend(["--events", e.to_str().expect("p")]);
    }
    cli(&a)
}

/// 通常経路: runner（MockExecutor）→ `export_full`（table/final-accounting だけを書く）→ 一時 store の decision-log export
/// （`celerisctl campaign export` と同じ `decision_log_export_campaign`）→ join。records.jsonl は手作りしない。
#[test]
fn joins_runner_out_and_decision_log_export_from_normal_paths() {
    use task_core::{
        SqliteStore,
        decision_log::{
            DecisionLogIntervention, DecisionLogOutcome, DecisionLogProposal, InterventionKind,
        },
        decision_log_export::ExportParams,
    };
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, 2);
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::seeded(),
        &RunOptions {
            out: out.clone(),
            ..RunOptions::default()
        },
    )
    .expect("run");
    let hash = m.compute_sha256().expect("hash");

    // 通常 export は table / final-accounting だけで、join の入力 3 file は出ない。
    let dest = d.path().join("export");
    celeris_campaign::export::export_full(&m, &out, &dest, None, None).expect("export");
    for n in ["records.jsonl", "trials.jsonl", "ledger.jsonl"] {
        assert!(!dest.join(n).exists(), "{n}");
    }
    let ops = d.path().join("human-ops.jsonl");
    record_ops(&ops, &hash, "A0", "p0", None);
    record_ops(&ops, &hash, "B0", "p0", Some("t-b-p0"));
    let (c, _, e) = join_multi(&ops, &[&dest]);
    assert_ne!(c, 0);
    assert!(e.contains("records.jsonl"), "{e}");

    // decision-log: 隔離した一時 store に通常の記録 API で書き、通常の export 関数で取り出す。
    let store = SqliteStore::open(&d.path().join("fixture.db")).expect("store");
    let now = time::OffsetDateTime::UNIX_EPOCH;
    store
        .decision_log_record_proposal(
            &DecisionLogProposal {
                run_id: "r1".into(),
                campaign_id: Some("e2e".into()),
                arm: Some("B".into()),
                case_id: Some("p0".into()),
                repetition: Some(1),
                manifest_hash: Some(hash.clone()),
                task_id: Some("t-b-p0".into()),
                ..Default::default()
            },
            now,
        )
        .expect("proposal");
    store
        .decision_log_record_outcome(
            &DecisionLogOutcome {
                run_id: "r1".into(),
                wall_ms: Some(90_000),
                ..Default::default()
            },
            now,
        )
        .expect("outcome");
    store
        .decision_log_record_intervention(&DecisionLogIntervention {
            intervention_id: "i1".into(),
            kind: InterventionKind::ResearchJudgment,
            task_id: Some("t-b-p0".into()),
            campaign_id: Some("e2e".into()),
            at: "2026-10-19T00:11:00Z".into(),
            wall_ms: None,
            source_event: None,
        })
        .expect("intervention");
    let exp = store
        .decision_log_export_campaign(&ExportParams {
            campaign_id: "e2e".into(),
            manifest_hash: Some(hash.clone()),
            export_cursor: None,
        })
        .expect("decision-log export");
    let dl = d.path().join("decision-log");
    fs::create_dir_all(&dl).expect("mkdir");
    fs::write(dl.join("records.jsonl"), exp.records_jsonl()).expect("write");

    let (c, first, e) = join_multi(&ops, &[&out, &dl]);
    assert_eq!(c, 0, "{e}");
    let rows: Vec<Value> = first
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("jsonl"))
        .collect();
    let (a, b) = (row(&rows, "A", "p0"), row(&rows, "B", "p0"));
    assert_eq!(rows.iter().filter(|r| r["row_type"] == "trial").count(), 5);
    // runner の trials（wall_secs）と decision-log（run/outcome/intervention）が同じ identity の 1 行に出る。
    assert_eq!(src(b, "wall_secs"), "event");
    assert_eq!(b["run_ids"]["value"], json!(["r1"]));
    assert_eq!(b["run_wall_ms"]["value"]["observed_sum"], 90_000);
    assert_eq!(
        b["event_interventions"]["value"],
        json!({"research_judgment": 1})
    );
    assert_eq!(src(b, "human_ops"), "evaluator");
    assert_eq!(
        b["human_op_secs"]["value"],
        json!({"observed_sum": 120, "observed": 1, "missing": 1})
    );
    // A 腕は decision-log が無い: 欠測は null のまま。
    assert_eq!(src(a, "run_wall_ms"), "missing");
    assert_eq!(a["run_wall_ms"]["value"], Value::Null);
    assert!(
        a["missing_fields"]
            .as_array()
            .expect("a")
            .contains(&json!("event_interventions"))
    );
    // 出所の分かる入力の重複（同じ入力を二度渡す）・順序で結果は変わらない。
    let (c, again, e) = join_multi(&ops, &[&dl, &out, &dl, &out]);
    assert_eq!(c, 0, "{e}");
    assert_eq!(again, first);
}
