//! 依存待ち: 前提 problem が全 config で passed になるまで依存先を実行しない。
//! 前提が失敗側なら blocked を分母に残し、外部待ちなら waiting で止めて再開で完走する。
mod common;
use std::{collections::BTreeMap, fs, path::Path};

use celeris_campaign::{
    checker::check,
    executor::{MockExecutor, MockStep},
    export::export,
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, RunReport, run},
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

/// p0..p2。p1 は p0 に依存する。C は subset {p0, p1} だけ。
fn campaign(i: &Input) -> CampaignManifest {
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
        campaign_id: "dep".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: vec!["p0".into(), "p1".into(), "p2".into()],
        configs: vec![
            cfg(ConfigId::A, "claude-code"),
            cfg(ConfigId::B, "celeris"),
            cfg(ConfigId::C, "celeris"),
        ],
        fault_scenarios: Default::default(),
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into(), "p1".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 1,
            per_case: celeris_campaign::manifest::Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_000_000,
            },
            per_arm: celeris_campaign::manifest::Cap {
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
        dependencies: BTreeMap::from([("p1".to_string(), vec!["p0".to_string()])]),
        variant: None,
        sha256: String::new(),
    }
}

struct Env {
    _d: tempfile::TempDir,
    i: Input,
    m: CampaignManifest,
    opts: RunOptions,
}

fn env() -> Env {
    let d = tempfile::tempdir().expect("dir");
    let i = input(3);
    let m = campaign(&i);
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    Env { _d: d, i, m, opts }
}

fn step(c: ConfigId, p: &str, s: MockStep) -> MockExecutor {
    MockExecutor::scripted(BTreeMap::from([((c, p.to_string()), s)]))
}

fn lines(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join(TRIALS_FILE))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect()
}

fn write_lines(out: &Path, v: &[Value]) {
    let s: String = v.iter().map(|x| format!("{x}\n")).collect();
    fs::write(out.join(TRIALS_FILE), s).expect("write");
}

fn find<'a>(v: &'a mut [Value], cfg: &str, p: &str) -> &'a mut Value {
    v.iter_mut()
        .find(|r| r["config"] == cfg && r["problem_id"] == p)
        .expect("record")
}

#[test]
fn prerequisite_success_runs_dependents() {
    let e = env();
    let rep = run(
        &e.m,
        &e.i,
        &MockExecutor::scripted(BTreeMap::new()),
        &e.opts,
    )
    .expect("run");
    assert_eq!((rep.executed, rep.blocked, rep.waiting), (8, 0, 0));
    let c = check(&e.m, &e.opts.out);
    assert!(c.ok, "{:?}", c.violations);
    let t = &c.recomputed.expect("rc").totals;
    assert_eq!((t[&ConfigId::A].passed, t[&ConfigId::C].passed), (3, 2));
    // 前提 p0 の記録は依存先 p1 より先に書かれている。
    let l = lines(&e.opts.out);
    let pos = |p: &str| {
        l.iter()
            .position(|r| r["problem_id"] == p && r["config"] == "A")
    };
    assert!(pos("p0") < pos("p1"));
}

#[test]
fn prerequisite_failure_blocks_dependents_and_keeps_denominator() {
    let e = env();
    // B だけ p0 に失敗しても、前提は「全 config が passed」でなければ成立しない。
    let rep = run(
        &e.m,
        &e.i,
        &step(ConfigId::B, "p0", MockStep::Fail),
        &e.opts,
    )
    .expect("run");
    assert_eq!((rep.executed, rep.blocked, rep.waiting), (5, 3, 0));
    let c = check(&e.m, &e.opts.out);
    assert!(c.ok, "{:?}", c.violations);
    let t = &c.recomputed.expect("rc").totals;
    for cfg in [ConfigId::A, ConfigId::B, ConfigId::C] {
        assert_eq!(t[&cfg].blocked, 1, "{cfg:?}");
    }
    assert_eq!((t[&ConfigId::A].n, t[&ConfigId::A].passed), (3, 2)); // p0, p2
    assert_eq!(
        (
            t[&ConfigId::B].n,
            t[&ConfigId::B].passed,
            t[&ConfigId::B].failed
        ),
        (3, 1, 1)
    );
    assert_eq!((t[&ConfigId::C].n, t[&ConfigId::C].passed), (2, 1));
    let blocked: Vec<Value> = lines(&e.opts.out)
        .into_iter()
        .filter(|r| r["outcome"] == "blocked")
        .collect();
    assert_eq!(blocked.len(), 3);
    for r in &blocked {
        assert_eq!(r["reason"], REASON_BLOCKED);
        assert_eq!(r["passed"], false);
        assert!(r["tokens"].is_null() && r["workspace"] == "");
    }
    // runner の summary と export にも blocked が分母込みで現れる。
    common::write_grades(&e.m, &e.opts.out, |_| true);
    let tab = export(&e.m, &e.opts.out, &e.opts.out.join("ex")).expect("export");
    let row = &tab["configs"][0];
    assert_eq!(
        (
            row["n"].as_u64(),
            row["blocked"].as_u64(),
            row["failures"].as_u64()
        ),
        (Some(3), Some(1), Some(1))
    );
    let csv = fs::read_to_string(e.opts.out.join("ex/table.csv")).expect("csv");
    assert!(csv.lines().next().is_some_and(|h| h.contains(",blocked,")));
}

#[test]
fn external_wait_stops_waiting_and_resume_completes() {
    let e = env();
    let rep = run(
        &e.m,
        &e.i,
        &step(ConfigId::B, "p0", MockStep::Wait),
        &e.opts,
    )
    .expect("run");
    // B/p0 が待ち、p1 の 3 試行は前提未完のため実行も記録もしない。
    assert_eq!((rep.executed, rep.blocked, rep.waiting), (4, 0, 4));
    let l = lines(&e.opts.out);
    assert_eq!(l.len(), 4);
    assert!(l.iter().all(|r| r["problem_id"] != "p1"));
    assert!(
        !e.opts.out.join("trials/B/p0/rep1").exists(),
        "no leftovers of the waiting trial"
    );
    let mid = check(&e.m, &e.opts.out);
    assert!(mid.ok, "{:?}", mid.violations);
    assert_eq!(mid.recomputed.expect("rc").totals[&ConfigId::B].missing, 2);
    assert!(export(&e.m, &e.opts.out, &e.opts.out.join("ex0")).is_ok());

    // 再開: 前提が済み、依存先が実行される。
    let rep2: RunReport = run(
        &e.m,
        &e.i,
        &MockExecutor::scripted(BTreeMap::new()),
        &e.opts,
    )
    .expect("resume");
    assert_eq!(
        (
            rep2.executed,
            rep2.resumed_skipped,
            rep2.waiting,
            rep2.blocked
        ),
        (4, 4, 0, 0)
    );
    let c = check(&e.m, &e.opts.out);
    assert!(c.ok, "{:?}", c.violations);
    let t = &c.recomputed.expect("rc").totals;
    assert_eq!(
        (
            t[&ConfigId::A].passed,
            t[&ConfigId::B].passed,
            t[&ConfigId::C].passed
        ),
        (3, 3, 2)
    );
    assert_eq!(lines(&e.opts.out).len(), 8);
}

#[test]
fn bad_dependencies_are_rejected_by_validate() {
    let i = input(3);
    let ok = campaign(&i);
    assert!(ok.validate().is_ok());
    let with = |d: &[(&str, &[&str])]| {
        let mut m = ok.clone();
        m.dependencies = d
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect();
        m.validate()
    };
    assert!(with(&[("p1", &["zz"])]).unwrap_err().contains("unknown"));
    assert!(with(&[("zz", &["p0"])]).unwrap_err().contains("unknown"));
    assert!(with(&[("p1", &["p1"])]).unwrap_err().contains("itself"));
    assert!(
        with(&[("p0", &["p1"]), ("p1", &["p0"])])
            .unwrap_err()
            .contains("cycle")
    );
    assert!(
        with(&[("p0", &["p1"]), ("p1", &["p2"]), ("p2", &["p0"])])
            .unwrap_err()
            .contains("cycle")
    );
    // run も拒否する。
    let e = env();
    let mut bad = e.m.clone();
    bad.dependencies = BTreeMap::from([
        ("p0".into(), vec!["p1".into()]),
        ("p1".into(), vec!["p0".into()]),
    ]);
    assert!(
        run(
            &bad,
            &e.i,
            &MockExecutor::scripted(BTreeMap::new()),
            &e.opts
        )
        .is_err()
    );
}

#[test]
fn dependencies_are_part_of_the_hash_and_absent_ones_keep_it() {
    let i = input(3);
    let with = campaign(&i);
    let mut none = with.clone();
    none.dependencies.clear();
    assert_ne!(
        with.compute_sha256().expect("h"),
        none.compute_sha256().expect("h")
    );
    // 欄が無い JSON と空の欄は同じ hash（既存 manifest の hash 互換）。
    let mut v = serde_json::to_value(&none).expect("v");
    assert!(v.get("dependencies").is_none());
    let parsed: CampaignManifest = serde_json::from_value(v.clone()).expect("parse");
    v["dependencies"] = json!({});
    let parsed2: CampaignManifest = serde_json::from_value(v).expect("parse");
    assert_eq!(parsed.compute_sha256(), none.compute_sha256());
    assert_eq!(parsed2.compute_sha256(), none.compute_sha256());
    // 並びは正規化される。
    let mut a = with.clone();
    a.dependencies = BTreeMap::from([("p2".into(), vec!["p1".into(), "p0".into(), "p0".into()])]);
    let mut b = with.clone();
    b.dependencies = BTreeMap::from([("p2".into(), vec!["p0".into(), "p1".into()])]);
    assert_eq!(a.compute_sha256(), b.compute_sha256());
}

#[test]
fn checker_detects_blocked_disguised_as_executed() {
    let e = env();
    run(
        &e.m,
        &e.i,
        &step(ConfigId::A, "p0", MockStep::Fail),
        &e.opts,
    )
    .expect("run");
    assert!(check(&e.m, &e.opts.out).ok);
    let mut v = lines(&e.opts.out);
    let r = find(&mut v, "A", "p1");
    assert_eq!(r["outcome"], "blocked");
    r["outcome"] = json!("passed");
    r["passed"] = json!(true);
    r["reason"] = Value::Null;
    r["tokens"] = json!(10_000);
    r["wall_secs"] = json!(60);
    write_lines(&e.opts.out, &v);
    let rep = check(&e.m, &e.opts.out);
    assert!(!rep.ok);
    assert!(
        rep.violations
            .iter()
            .any(|s| s.contains("executed although prerequisite p0 did not pass")),
        "{:?}",
        rep.violations
    );
    assert!(export(&e.m, &e.opts.out, &e.opts.out.join("ex")).is_err());
}

#[test]
fn checker_detects_blocked_without_a_failed_prerequisite_and_dropped_blocked() {
    // 前提が通っているのに blocked と偽る（失敗を隠す）。
    let e = env();
    run(
        &e.m,
        &e.i,
        &MockExecutor::scripted(BTreeMap::new()),
        &e.opts,
    )
    .expect("run");
    let mut v = lines(&e.opts.out);
    let r = find(&mut v, "B", "p1");
    r["outcome"] = json!("blocked");
    r["passed"] = json!(false);
    r["reason"] = json!(REASON_BLOCKED);
    r["tokens"] = Value::Null;
    r["cost_micro_usd"] = Value::Null;
    r["wall_secs"] = json!(0);
    r["workspace"] = json!("");
    write_lines(&e.opts.out, &v);
    let rep = check(&e.m, &e.opts.out);
    assert!(
        rep.violations
            .iter()
            .any(|s| s.contains("blocked although no prerequisite had failed")),
        "{:?}",
        rep.violations
    );

    // blocked の記録を落として分母から消す。
    let e = env();
    run(
        &e.m,
        &e.i,
        &step(ConfigId::A, "p0", MockStep::Error),
        &e.opts,
    )
    .expect("run");
    let mut v = lines(&e.opts.out);
    v.retain(|r| r["outcome"] != "blocked");
    write_lines(&e.opts.out, &v);
    let rep = check(&e.m, &e.opts.out);
    assert!(!rep.ok, "dropped blocked records must be visible");
    // blocked の outcome を残したまま reason を消す偽装。
    let e = env();
    run(
        &e.m,
        &e.i,
        &step(ConfigId::A, "p0", MockStep::Error),
        &e.opts,
    )
    .expect("run");
    let mut v = lines(&e.opts.out);
    find(&mut v, "A", "p1")["reason"] = Value::Null;
    write_lines(&e.opts.out, &v);
    assert!(!check(&e.m, &e.opts.out).ok);
}
