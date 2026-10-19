//! mock 実行器で A/B/C を通す一続きの統合試験（外部 network・LLM は使わない）。
//! 実行 → checker → export（mock の印・空/欠測は成功にしない）→ 予算停止 → 再開（重複投入なし）→ dev/final 混在拒否。
mod common;
use std::{collections::BTreeMap, fs, path::Path};

use celeris_campaign::{
    aggregate::{Outcome, TrialResult, aggregate},
    checker::check,
    executor::{MockAsyncExecutor, MockExecutor, MockStep},
    export::export,
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    record::*,
    report::check_report,
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

fn campaign(i: &Input, n: usize, split: Split, max_tokens: u64) -> CampaignManifest {
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
        split,
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
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            // in-flight 上限はこの試験の対象ではない（max_parallel は parallel_cap.rs）。
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
        variant: None,
        sha256: String::new(),
    }
}

fn opts(d: &Path) -> RunOptions {
    RunOptions {
        global_ledger: None,
        out: d.join("out"),
        ..RunOptions::default()
    }
}

fn json_file(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).expect("read")).expect("json")
}

#[test]
fn mock_run_flows_through_check_and_export_with_a_mock_mark() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, 2, Split::Dev, 10_000_000);
    let o = opts(d.path());
    // A は p1 で失敗、B は p0 で executor error、C は p0 に token を多く使う。A の失敗は B/C を止めない。
    let mut s = BTreeMap::new();
    s.insert((ConfigId::A, "p1".to_string()), MockStep::Fail);
    s.insert((ConfigId::B, "p0".to_string()), MockStep::Err);
    s.insert((ConfigId::C, "p0".to_string()), MockStep::Spend(50_000));
    let rep = run(&m, &i, &MockExecutor::scripted(s), &o).expect("run");
    // 計画 = A 2 + B 2 + C 1 = 5 試行すべてが実行・記録された。
    assert_eq!((rep.executed, rep.executor_errors), (5, 1));
    let c = check(&m, &o.out);
    assert!(c.ok, "{:?}", c.violations);
    let t = export(&m, &o.out, &d.path().join("ex")).expect("export");
    // mock 由来の印: run-meta・summary・table.json・CSV のすべてに残り、実性能として扱わない。
    assert_eq!(json_file(&o.out.join(META_FILE))["evidence_kind"], "mock");
    assert_eq!(json_file(&o.out.join(META_FILE))["real_performance"], false);
    let sm = json_file(&o.out.join(SUMMARY_FILE));
    assert_eq!(
        (&sm["evidence_kind"], &sm["real_performance"]),
        (&json!("mock"), &json!(false))
    );
    // 未採点（採点が無い）の集計は最終版にしない。
    assert_eq!(t["complete"], false);
    assert!(!d.path().join("ex/table.json").exists());
    assert_eq!(t["evidence_kind"], "mock");
    assert_eq!(t["real_performance"], false);
    assert!(
        t["notice"]
            .as_str()
            .is_some_and(|n| n.contains("not real performance"))
    );
    let csv = fs::read_to_string(d.path().join("ex/table.incomplete.csv")).expect("csv");
    assert!(
        csv.lines()
            .next()
            .is_some_and(|h| h.ends_with(",evidence_kind,complete"))
    );
    assert!(csv.lines().skip(1).all(|l| l.ends_with(",mock,false")));
    // 失敗・error は分母に残る（A: 2 試行中 1 通過）。
    let a = &t["configs"][0];
    assert_eq!(
        (a["config"].as_str(), a["n"].as_u64(), a["passed"].as_u64()),
        (Some("A"), Some(2), Some(1))
    );
    assert_eq!(t["configs"][1]["error"], 1);
    // 報告本文の数値は生記録からの再計算と一致しなければ通らない。
    let ok = "result A passed=1 n=2 split=dev\nresult B passed=1 n=2 split=dev\nresult C passed=1 n=1 split=dev\n";
    // 未採点の間は、数値が合っていても最終報告にならない。
    assert!(!check_report(&m, &o.out, ok).is_empty());
    common::write_grades(&m, &o.out, |_| true);
    // 採点が揃っても、実行されなかった試行（B/p0 は executor Err）がある間は最終報告にならない。
    let r = check_report(&m, &o.out, ok);
    assert!(r.iter().any(|x| x.contains("never executed")), "{r:?}");
    // 同じ台本で B/p0 が実行されて失敗に終わった campaign は、数値が合えば最終報告になる。
    let d2 = tempfile::tempdir().expect("dir");
    let o2 = opts(d2.path());
    let mut s = BTreeMap::new();
    s.insert((ConfigId::A, "p1".to_string()), MockStep::Fail);
    s.insert((ConfigId::B, "p0".to_string()), MockStep::Fail);
    s.insert((ConfigId::C, "p0".to_string()), MockStep::Spend(50_000));
    run(&m, &i, &MockExecutor::scripted(s), &o2).expect("run");
    common::write_grades(&m, &o2.out, |_| true);
    assert!(check_report(&m, &o2.out, ok).is_empty());
    let forged = ok.replace("A passed=1", "A passed=2");
    assert!(!check_report(&m, &o2.out, &forged).is_empty());
}

#[test]
fn empty_or_missing_exports_are_not_success() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, 2, Split::Dev, 10_000_000);
    let o = opts(d.path());
    run(&m, &i, &MockExecutor::scripted(BTreeMap::new()), &o).expect("run");
    // trials.jsonl を空にした out は export できない（空の表を成功にしない）。
    let trials = o.out.join(TRIALS_FILE);
    let full = fs::read_to_string(&trials).expect("trials");
    fs::write(&trials, "").expect("truncate");
    assert!(export(&m, &o.out, &d.path().join("empty")).is_err());
    assert!(!d.path().join("empty/table.json").exists());
    // 1 行だけ欠けても拒否する。
    let cut: String = full.lines().skip(1).map(|l| format!("{l}\n")).collect();
    fs::write(&trials, cut).expect("cut");
    assert!(export(&m, &o.out, &d.path().join("cut")).is_err());
    // 欠測の token は 0 ではなく欠測として出る。
    let d2 = tempfile::tempdir().expect("dir");
    let o2 = opts(d2.path());
    let mut s = BTreeMap::new();
    for p in ["p0", "p1"] {
        s.insert((ConfigId::A, p.to_string()), MockStep::PassUnmetered);
    }
    run(&m, &i, &MockExecutor::scripted(s), &o2).expect("run");
    let t = export(&m, &o2.out, &d2.path().join("ex")).expect("export");
    assert_eq!(t["configs"][0]["tokens_status"], "missing");
    assert!(t["configs"][0]["tokens_sum"].is_null());
}

#[test]
fn budget_stop_keeps_the_denominator_and_the_rest_is_marked() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    // 全体上限 60,000 tokens。最初の試行が 50,000 使い、続く試行は止まる。
    let m = campaign(&i, 2, Split::Dev, 60_000);
    let o = opts(d.path());
    let mut s = BTreeMap::new();
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        for p in ["p0", "p1"] {
            s.insert((c, p.to_string()), MockStep::Spend(50_000));
        }
    }
    let rep = run(&m, &i, &MockExecutor::scripted(s), &o).expect("run");
    assert!(rep.budget_stopped);
    assert!(rep.budget_cancelled > 0 && rep.executed < 5);
    let c = check(&m, &o.out);
    assert!(c.ok, "{:?}", c.violations);
    let t = export(&m, &o.out, &d.path().join("ex")).expect("export");
    // 停止後も分母は計画どおり（A/B 2、C 1）で、止めた試行は cancelled として残る。
    let ns: Vec<u64> = t["configs"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["n"].as_u64().expect("n"))
        .collect();
    assert_eq!(ns, vec![2, 2, 1]);
    let cancelled: u64 = t["configs"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["cancelled"].as_u64().expect("c"))
        .sum();
    assert_eq!(cancelled, rep.budget_cancelled);
    assert_eq!(t["evidence_kind"], "mock");
}

#[test]
fn resume_after_lost_response_and_restart_never_duplicates_tasks() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, 2, Split::Dev, 10_000_000);
    let o = opts(d.path());
    let x = MockAsyncExecutor::new(1);
    x.lose_next_submit_responses(1);
    // 1 回目: 応答喪失は『投入結果不明』で error にならず、A の 1 試行は冪等キーの照会に回る。他は Wait。
    let first = run(&m, &i, &x, &o).expect("first run");
    assert_eq!(first.executor_errors, 0);
    assert!(first.waiting > 0);
    // 2 回目以降（同じ out・同じ外部）: 同じ試行を再投入しない。
    run(&m, &i, &x, &o).expect("resume");
    let last = run(&m, &i, &x, &o).expect("resume again");
    assert_eq!(last.waiting, 0);
    assert_eq!(
        x.remote_task_count(),
        5,
        "one external task per planned trial"
    );
    assert_eq!(x.submit_calls(), 5, "no resubmission across restarts");
    // 記録は計画どおり 5 試行（mock-async の routing は manifest に固定されないので checker/export は通さない）。
    let sm = json_file(&o.out.join(SUMMARY_FILE));
    assert_eq!(sm["records"], 5);
    assert_eq!(sm["evidence_kind"], "mock");
    assert_eq!(sm["real_performance"], false);
}

#[test]
fn dev_and_final_inputs_are_never_mixed() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let o = opts(d.path());
    // final の campaign に dev の input は渡せない（run も dry-run の検証も拒否）。
    let fin = campaign(&i, 2, Split::Final, 10_000_000);
    assert!(run(&fin, &i, &MockExecutor::scripted(BTreeMap::new()), &o).is_err());
    assert!(celeris_campaign::dryrun::estimate(&fin, &i).is_err());
    // dev の out dir に別 campaign（hash が違う）の実行を重ねない。
    let dev = campaign(&i, 2, Split::Dev, 10_000_000);
    run(&dev, &i, &MockExecutor::scripted(BTreeMap::new()), &o).expect("run");
    let mut other = dev.clone();
    other.version = "v2".into();
    assert!(run(&other, &i, &MockExecutor::scripted(BTreeMap::new()), &o).is_err());
    // 集計は split・hash の違う結果を拒否する。
    let mk = |split, hash: &str| TrialResult {
        campaign_sha256: hash.into(),
        split,
        config: ConfigId::A,
        problem_id: "p0".into(),
        trial_no: 1,
        rep: 1,
        outcome: Outcome::Passed,
    };
    let h = dev.compute_sha256().expect("hash");
    assert!(aggregate(&dev, &[mk(Split::Dev, &h)]).is_ok());
    assert!(aggregate(&dev, &[mk(Split::Final, &h)]).is_err());
    assert!(aggregate(&dev, &[mk(Split::Dev, "other")]).is_err());
    // 報告本文に final の行が混ざっても通らない。
    let text = "result A passed=2 n=2 split=final\nresult B passed=2 n=2 split=dev\nresult C passed=1 n=1 split=dev\n";
    assert!(
        check_report(&dev, &o.out, text)
            .iter()
            .any(|v| v.contains("split"))
    );
}
