//! 継続監視/再開 driver（`watch`）の試験。時計は注入（実時間を使わない）、外部は MockAsyncExecutor。
use std::{cell::RefCell, collections::BTreeMap, time::Duration};

use celeris_campaign::{
    executor::MockAsyncExecutor,
    ledger::{Ledger, LedgerState},
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, TokenUsage, manifest_hash,
    },
    runner::RunOptions,
    watch::{Clock, WatchEnd, WatchOptions, watch},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::Value;
use serde_json::json;

/// sleep を記録するだけの時計（実際には待たない）。
#[derive(Default)]
struct FakeClock {
    sleeps: RefCell<Vec<Duration>>,
}

impl Clock for FakeClock {
    fn sleep(&self, d: Duration) {
        self.sleeps.borrow_mut().push(d);
    }
}

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
            // in-flight 上限はこの試験の対象ではない（max_parallel は parallel_cap.rs）。
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

fn env(max_tokens: u64) -> Env {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let mut m = campaign(&i);
    m.budget.max_tokens = max_tokens;
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    Env { _d: d, i, m, opts }
}

fn w(interval_secs: u64, max_iterations: u32) -> WatchOptions {
    WatchOptions {
        interval_secs,
        max_iterations,
    }
}

fn all_done(e: &Env) -> bool {
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    lg.rows().count() == 3 && lg.rows().all(|r| r.state == LedgerState::Done)
}

#[test]
fn rescans_with_injected_clock_until_settled() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(2);
    let clock = FakeClock::default();
    let r = watch(&e.m, &e.i, &x, &e.opts, w(7, 20), &clock).expect("watch");
    assert_eq!(r.end, WatchEnd::Settled);
    assert_eq!(r.unfinished, 0);
    assert!(r.iterations >= 2, "waited at least one rescan");
    // 走査の間だけ注入した時計で待つ（最後の走査の後は待たない）。
    let sleeps = clock.sleeps.borrow();
    assert_eq!(sleeps.len() as u32, r.iterations - 1);
    assert!(sleeps.iter().all(|d| *d == Duration::from_secs(7)));
    assert!(all_done(&e));
    assert_eq!((x.submit_calls(), x.remote_task_count()), (3, 3));
}

#[test]
fn restart_after_max_iterations_resumes_same_tasks_without_resubmit() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(6);
    let c1 = FakeClock::default();
    let r1 = watch(&e.m, &e.i, &x, &e.opts, w(5, 2), &c1).expect("watch 1");
    assert_eq!((r1.end, r1.iterations), (WatchEnd::MaxIterations, 2));
    assert!(r1.unfinished > 0 && !all_done(&e));
    let submits = x.submit_calls();
    // driver の再起動（新しい watch 呼び出し）: 台帳から同じ task を追って終端まで進む。
    let c2 = FakeClock::default();
    let r2 = watch(&e.m, &e.i, &x, &e.opts, w(5, 50), &c2).expect("watch 2");
    assert_eq!(r2.end, WatchEnd::Settled);
    assert!(all_done(&e));
    assert_eq!(x.submit_calls(), submits, "restart never resubmits");
    assert_eq!(x.remote_task_count(), 3);
}

#[test]
fn comm_failures_during_watch_keep_tracking_and_do_not_duplicate() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(1);
    x.fail_next_polls(2);
    x.fail_next_usages(2);
    let r = watch(&e.m, &e.i, &x, &e.opts, w(1, 30), &FakeClock::default()).expect("watch");
    assert_eq!(r.end, WatchEnd::Settled);
    assert!(all_done(&e));
    assert_eq!((x.submit_calls(), x.remote_task_count()), (3, 3));
    assert!(x.cancelled().is_empty(), "comm failure is not a cancel");
}

#[test]
fn unconfirmed_stop_keeps_row_open_until_confirmed() {
    // 予算超過で cancel しても、外部の停止を確認するまで台帳を閉じず、watch は終わらない。
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(50);
    x.set_usage(Some(TokenUsage::full(1_000_000, 0, 0, 0)), 10);
    x.unconfirmed_stops(3);
    let r = watch(&e.m, &e.i, &x, &e.opts, w(1, 40), &FakeClock::default()).expect("watch");
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    assert!(lg.rows().all(|r| r.state.is_terminal()));
    assert!(!x.cancelled().is_empty());
    assert!(x.remote_task_count() <= 3, "no investment beyond the plan");
}

#[test]
fn zero_interval_or_iterations_is_rejected() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(0);
    let c = FakeClock::default();
    assert!(watch(&e.m, &e.i, &x, &e.opts, w(0, 5), &c).is_err());
    assert!(watch(&e.m, &e.i, &x, &e.opts, w(5, 0), &c).is_err());
    assert_eq!(x.submit_calls(), 0);
}
