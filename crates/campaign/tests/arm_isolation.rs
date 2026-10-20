//! arm 順の巡回/無作為化（再現可能・記録される）と、A の Err/待機/枯渇が B/C を止めないこと
//! （ADR 2026-10-16-campaign-arm-order-isolation）。実時間は使わない。
use std::{collections::BTreeMap, fs, path::Path};

use celeris_campaign::{
    aggregate::Outcome,
    checker::check,
    executor::{
        ExecError, ExecResult, ExecStatus, Executor, MockExecutor, MockStep, TaskRef, TrialRequest,
    },
    manifest::{
        ArmOrder, Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary,
        SnapshotRef, Split, Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, plan, run},
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

fn campaign(i: &Input, n: usize, order: ArmOrder, seed: u64) -> CampaignManifest {
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
        campaign_id: "c1".into(),
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
        fault_scenarios: Default::default(),
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: (0..n).map(|k| format!("p{k}")).collect(),
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 100_000_000,
            max_wall_secs: 100_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 1,
            per_case: Cap {
                max_tokens: 50_000,
                max_wall_secs: 600,
            },
            per_arm: Cap {
                max_tokens: 100_000_000,
                max_wall_secs: 100_000_000,
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
        order,
        seed,
        repeats: 1,
        dependencies: Default::default(),
        variant: None,
        sha256: String::new(),
    }
}

fn opts(dir: &Path) -> RunOptions {
    RunOptions {
        global_ledger: None,
        out: dir.join("out"),
        ..RunOptions::default()
    }
}

fn arms_by_problem(m: &CampaignManifest) -> BTreeMap<String, Vec<ConfigId>> {
    let mut v: BTreeMap<String, Vec<ConfigId>> = BTreeMap::new();
    for (c, p, _) in plan(m) {
        v.entry(p).or_default().push(c);
    }
    v
}

fn records(dir: &Path) -> Vec<TrialRecord> {
    fs::read_to_string(dir.join("out").join(TRIALS_FILE))
        .expect("trials")
        .lines()
        .map(|l| serde_json::from_str(l).expect("record"))
        .collect()
}

#[test]
fn abc_rotation_cycles_and_keeps_dependency_order() {
    let i = input(6);
    let mut m = campaign(&i, 6, ArmOrder::AbcRotation, 0);
    m.dependencies = BTreeMap::from([("p0".to_string(), vec!["p3".to_string()])]);
    let order = m.normalized().problem_order().expect("order");
    let by = arms_by_problem(&m);
    let seq: Vec<Vec<ConfigId>> = order.iter().map(|p| by[p].clone()).collect();
    let abc = vec![ConfigId::A, ConfigId::B, ConfigId::C];
    for (pos, s) in seq.iter().enumerate() {
        let mut want = abc.clone();
        want.rotate_left(pos % 3);
        assert_eq!(s, &want, "position {pos}");
    }
    // 前提 p3 は p0 より先に計画される。
    let planned: Vec<String> = plan(&m).into_iter().map(|(_, p, _)| p).collect();
    let first = |p: &str| planned.iter().position(|x| x == p).expect("planned");
    assert!(first("p3") < first("p0"));
    // 各 arm が各位置に同数ずつ来る（6 problem・3 位置）。
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        for rank in 0..3 {
            let n = seq.iter().filter(|s| s[rank] == c).count();
            assert_eq!(n, 2);
        }
    }
}

#[test]
fn seeded_shuffle_is_reproducible_and_seed_dependent() {
    let i = input(12);
    let m1 = campaign(&i, 12, ArmOrder::SeededShuffle, 7);
    let m1b = campaign(&i, 12, ArmOrder::SeededShuffle, 7);
    let m2 = campaign(&i, 12, ArmOrder::SeededShuffle, 8);
    assert_eq!(plan(&m1), plan(&m1b));
    assert_ne!(plan(&m1), plan(&m2));
    // 並べ替えでも各 problem に全 arm が 1 回ずつ出る。
    for arms in arms_by_problem(&m1).values() {
        let mut s = arms.clone();
        s.sort();
        assert_eq!(s, vec![ConfigId::A, ConfigId::B, ConfigId::C]);
    }
    // A が常に先頭ではない（固定順の交絡が無い）。
    assert!(arms_by_problem(&m1).values().any(|a| a[0] != ConfigId::A));
}

#[test]
fn order_seed_and_rank_are_recorded() {
    let dir = tempfile::tempdir().expect("tmp");
    let i = input(4);
    let m = campaign(&i, 4, ArmOrder::SeededShuffle, 42);
    run(&m, &i, &MockExecutor::default(), &opts(dir.path())).expect("run");
    let meta: Value =
        serde_json::from_slice(&fs::read(dir.path().join("out").join(META_FILE)).expect("meta"))
            .expect("json");
    assert_eq!(meta["order"], "seeded_shuffle");
    assert_eq!(meta["seed"], 42);
    let recs = records(dir.path());
    let by = arms_by_problem(&m);
    for p in by.keys() {
        let got: Vec<(u32, ConfigId)> = recs
            .iter()
            .filter(|r| &r.problem_id == p)
            .map(|r| (r.arm_rank, r.config))
            .collect();
        let want: Vec<(u32, ConfigId)> = by[p]
            .iter()
            .enumerate()
            .map(|(k, c)| (k as u32 + 1, *c))
            .collect();
        assert_eq!(got, want, "{p}");
    }
}

fn all_a(step: MockStep, n: usize) -> MockExecutor {
    MockExecutor::scripted(
        (0..n)
            .map(|k| ((ConfigId::A, format!("p{k}")), step.clone()))
            .collect(),
    )
}

#[test]
fn a_err_for_every_case_does_not_stop_b_and_c() {
    let dir = tempfile::tempdir().expect("tmp");
    let i = input(4);
    let m = campaign(&i, 4, ArmOrder::AbcRotation, 0);
    let rep = run(&m, &i, &all_a(MockStep::Err, 4), &opts(dir.path())).expect("run survives Err");
    let recs = records(dir.path());
    assert_eq!(recs.len(), 12);
    for r in &recs {
        match r.config {
            ConfigId::A => {
                assert_eq!(r.outcome, Outcome::Error);
                assert_eq!(r.reason.as_deref(), Some(REASON_EXECUTOR_ERROR));
                assert_eq!(r.tokens, None);
            }
            _ => assert_eq!(
                r.outcome,
                Outcome::Passed,
                "{:?}/{}",
                r.config,
                r.problem_id
            ),
        }
    }
    assert_eq!(rep.executor_errors, 4);
    // 観測の無い Err は課金されない（B/C の分だけ）。欠測扱いにもならない。
    assert_eq!(rep.tokens_charged_total, 8 * 10_000);
    assert_eq!(rep.tokens_missing, 0);
    assert!(!rep.budget_stopped);
    let v = check(&m, &dir.path().join("out"));
    assert!(v.violations.is_empty(), "{:?}", v.violations);
}

#[test]
fn a_wait_does_not_stop_b_and_c() {
    let dir = tempfile::tempdir().expect("tmp");
    let i = input(3);
    let m = campaign(&i, 3, ArmOrder::SeededShuffle, 3);
    let rep = run(&m, &i, &all_a(MockStep::Wait, 3), &opts(dir.path())).expect("run");
    assert_eq!(rep.waiting, 3);
    let recs = records(dir.path());
    assert_eq!(recs.len(), 6);
    assert!(
        recs.iter()
            .all(|r| r.config != ConfigId::A && r.outcome == Outcome::Passed)
    );
}

#[test]
fn a_arm_budget_exhaustion_does_not_stop_b_and_c() {
    let dir = tempfile::tempdir().expect("tmp");
    let i = input(4);
    let mut m = campaign(&i, 4, ArmOrder::AbcRotation, 0);
    m.budget.per_arm.max_tokens = 50_000; // A（20_000/件）は 3 件目で尽きる。B/C（3_000/件）は尽きない
    let script: BTreeMap<_, _> = (0..4)
        .map(|k| ((ConfigId::A, format!("p{k}")), MockStep::Spend(20_000)))
        .chain((0..4).flat_map(|k| {
            [ConfigId::B, ConfigId::C].map(|c| ((c, format!("p{k}")), MockStep::Spend(3_000)))
        }))
        .collect();
    let rep = run(&m, &i, &MockExecutor::scripted(script), &opts(dir.path())).expect("run");
    let recs = records(dir.path());
    assert_eq!(recs.len(), 12);
    for c in [ConfigId::B, ConfigId::C] {
        assert!(
            recs.iter()
                .filter(|r| r.config == c)
                .all(|r| r.outcome == Outcome::Passed)
        );
    }
    let a: Vec<&TrialRecord> = recs.iter().filter(|r| r.config == ConfigId::A).collect();
    assert_eq!(
        a.iter().filter(|r| r.outcome == Outcome::Cancelled).count(),
        2
    );
    assert_eq!(rep.arm_budget_cancelled, 2);
    assert!(
        !rep.budget_stopped,
        "arm exhaustion is not global exhaustion"
    );
}

/// async: A の submit が常に Err でも B/C は完走する。
struct AsyncAErr {
    inner: celeris_campaign::executor::MockAsyncExecutor,
}

impl Executor for AsyncAErr {
    fn name(&self) -> &'static str {
        "async-a-err"
    }
    fn is_async(&self) -> bool {
        true
    }
    fn execute(&self, _r: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only".into())
    }
    fn submit(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        if req.config.id == ConfigId::A {
            return Err(ExecError::refused("submit refused"));
        }
        self.inner.submit(req, key)
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        self.inner.lookup(key)
    }
    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        self.inner.poll(t)
    }
    fn cancel(&self, t: &TaskRef, why: &str) -> Result<(), ExecError> {
        self.inner.cancel(t, why)
    }
}

#[test]
fn async_a_submit_err_does_not_stop_b_and_c() {
    let dir = tempfile::tempdir().expect("tmp");
    let i = input(3);
    let m = campaign(&i, 3, ArmOrder::AbcRotation, 0);
    let exec = AsyncAErr {
        inner: celeris_campaign::executor::MockAsyncExecutor::new(0),
    };
    let rep = run(&m, &i, &exec, &opts(dir.path())).expect("run");
    assert_eq!(rep.executor_errors, 3);
    let recs = records(dir.path());
    assert_eq!(recs.len(), 9);
    for r in &recs {
        if r.config == ConfigId::A {
            assert_eq!(r.outcome, Outcome::Error);
        } else {
            assert_eq!(r.outcome, Outcome::Passed);
        }
    }
    let _ = ExecResult {
        outcome: Outcome::Passed,
        tokens: None,
        cost_micro_usd: None,
        wall_secs: 0,
    };
}
