//! 予算の予約と確定消費の整合（global / arm / case / in-flight）。時計は使わず、mock の台本で決定的に確かめる
//! （ADR 2026-10-16-campaign-budget-reservation）。
use std::{collections::BTreeMap, fs};

use celeris_campaign::{
    budget::{self, Gate},
    executor::{MockAsyncExecutor, TokenUsage},
    ledger::{Ledger, LedgerState, TrialKey},
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

const CASE: u64 = 100_000;

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

/// p0 だけ（A/B/C の 3 試行）。
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
        problems: vec!["p0".into()],
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
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 64,
            per_case: celeris_campaign::manifest::Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_800,
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
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    }
}

struct Env {
    _d: tempfile::TempDir,
    i: Input,
    m: CampaignManifest,
    opts: RunOptions,
}

fn env(global: u64, parallel: u32) -> Env {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let mut m = campaign(&i);
    m.budget.max_tokens = global;
    m.budget.max_parallel = parallel;
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    Env { _d: d, i, m, opts }
}

fn key(e: &Env, arm: ConfigId) -> TrialKey {
    TrialKey {
        manifest_sha256: e.m.compute_sha256().expect("hash"),
        problem_id: "p0".into(),
        arm,
        trial_no: 1,
        rep: 1,
    }
}

fn records(e: &Env) -> Vec<TrialRecord> {
    fs::read_to_string(e.opts.out.join(TRIALS_FILE))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("record"))
        .collect()
}

fn spend(e: &Env) -> budget::Spend {
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let h = e.m.compute_sha256().expect("hash");
    budget::spend(&records(e), &lg, &e.m.budget, &h, None)
}

#[test]
fn new_submission_is_deferred_when_remaining_is_below_the_reservation() {
    // global 250k、予約は 1 試行 100k。A・B は予約 200k で投入、C は残り 50k < 100k なので投入しない。
    let e = env(250_000, 64);
    let x = MockAsyncExecutor::new(1);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("first");
    assert_eq!(x.remote_task_count(), 2, "C is not submitted");
    assert_eq!((rep.reserve_deferred, rep.waiting), (1, 3));
    let s = spend(&e);
    assert_eq!(
        (s.held_tokens, s.tokens, s.in_flight),
        (2 * CASE, 2 * CASE, 2)
    );
    // 残予算 = 上限 − 確定消費(0) − 予約(200k)。
    assert_eq!(250_000 - s.tokens, 50_000);
    // 予約は解けないので再 run でも（先行が Done になるまで）C は出ない → 先行が Done（各 10k）になった後に出す。
    let rep2 = run(&e.m, &e.i, &x, &e.opts).expect("second");
    assert_eq!(rep2.executed, 2);
    assert_eq!(
        x.remote_task_count(),
        3,
        "C is submitted once the holds are settled"
    );
    let s = spend(&e);
    assert_eq!((s.tokens - s.held_tokens, s.held_tokens), (20_000, CASE));
}

#[test]
fn nothing_in_flight_and_remaining_below_reservation_stops_without_paid_runs() {
    let e = env(50_000, 64);
    let x = MockAsyncExecutor::new(0);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("run");
    assert!(rep.budget_stopped);
    assert_eq!((x.submit_calls(), rep.budget_cancelled), (0, 3));
}

#[test]
fn parallel_one_holds_a_single_reservation_and_serializes_trials() {
    let e = env(1_000_000, 1);
    let x = MockAsyncExecutor::new(1);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("first");
    assert_eq!((x.remote_task_count(), rep.parallel_deferred), (1, 2));
    let s = spend(&e);
    assert_eq!((s.held_tokens, s.in_flight), (CASE, 1));
    // 並列 1 の間、arm A の枠も予約で減る（global も 1 件分だけ）。
    assert_eq!(s.arm.get(&ConfigId::A).copied().unwrap_or_default().0, CASE);
    for _ in 0..4 {
        run(&e.m, &e.i, &x, &e.opts).expect("next");
        assert!(spend(&e).in_flight <= 1, "never more than one reservation");
    }
    assert_eq!(x.remote_task_count(), 3);
    assert_eq!(spend(&e).held_tokens, 0, "all settled");
}

#[test]
fn missing_usage_is_never_zero_while_waiting_and_survives_restart() {
    let e = env(1_000_000, 64);
    let x = MockAsyncExecutor::new(5);
    // usage が欠測（tokens None）・更新待ちの間も、予約は消費 0 に落ちない。
    x.set_usage(None, 7);
    run(&e.m, &e.i, &x, &e.opts).expect("first");
    let before = spend(&e);
    assert_eq!(
        before.held_tokens,
        3 * CASE,
        "missing tokens hold the reservation"
    );
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let a = lg.get(&key(&e, ConfigId::A)).expect("row");
    assert!(a.observed_usage.is_some_and(|o| o.tokens.is_none()));
    // usage 取得が通信断になっても予約は変わらない。
    x.fail_next_usages(3);
    run(&e.m, &e.i, &x, &e.opts).expect("usage failure");
    assert_eq!(spend(&e), before);
    // 観測が予約より小さければ予約のまま、大きければ観測を採る（max）。
    x.set_usage(Some(TokenUsage::full(40_000, 0, 0, 0)), 9);
    run(&e.m, &e.i, &x, &e.opts).expect("small observed");
    assert_eq!(spend(&e).held_tokens, 3 * CASE);
    // 再起動（台帳の読み直し）後も同じ値で、同じ task を追う。
    assert_eq!(spend(&e).in_flight, 3);
    assert_eq!(x.remote_task_count(), 3);
}

#[test]
fn reservation_is_kept_until_cancel_is_confirmed_and_not_double_counted() {
    let e = env(10_000_000, 64);
    let x = MockAsyncExecutor::new(100);
    run(&e.m, &e.i, &x, &e.opts).expect("first");
    // 上限超過（200k > per_case）を観測 → cancel の通信断で停止未確認。
    x.set_usage(Some(TokenUsage::full(200_000, 0, 0, 0)), 5);
    x.fail_next_cancels(1);
    run(&e.m, &e.i, &x, &e.opts).expect("second");
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    assert_eq!(
        lg.get(&key(&e, ConfigId::A)).expect("row").state,
        LedgerState::CancelPending
    );
    let s = spend(&e);
    // 記録済みでも A は停止未確認なので予約（観測 200k）で 1 回だけ数える。B・C は停止確認済みで確定 200k ずつ。
    assert_eq!(s.held_tokens, 200_000);
    assert_eq!(s.tokens, 200_000 + 2 * 200_000);
    // 停止が確認されたら予約が確定消費へ移る（総量は変わらない）。
    run(&e.m, &e.i, &x, &e.opts).expect("third");
    let s = spend(&e);
    assert_eq!((s.held_tokens, s.tokens), (0, 600_000));
}

#[test]
fn admit_is_a_pure_function_of_limits_and_spend() {
    let e = env(250_000, 64);
    let mut s = budget::Spend::default();
    assert!(
        matches!(budget::admit(&e.m.budget, &s, ConfigId::A), Gate::Admit(c) if c.max_tokens == CASE)
    );
    s.tokens = 200_000;
    s.in_flight = 2;
    assert_eq!(budget::admit(&e.m.budget, &s, ConfigId::A), Gate::Defer);
    s.in_flight = 0;
    assert_eq!(budget::admit(&e.m.budget, &s, ConfigId::A), Gate::Exhausted);
}
