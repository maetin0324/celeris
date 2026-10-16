//! 冪等な試行台帳と非同期実行器: 投入前の永続化、応答喪失・再起動後の重複投入なし、
//! Wait 中の workspace 保持、打ち切り時の cancel（子 task・job を含む）。
use std::{collections::BTreeMap, fs};

use celeris_campaign::{
    executor::MockAsyncExecutor,
    ledger::{LEDGER_FILE, Ledger, LedgerState, TrialKey},
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::Value;
use serde_json::json;

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
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
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

fn env(max_tokens: u64) -> Env {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let mut m = campaign(&i);
    m.budget.max_tokens = max_tokens;
    let opts = RunOptions {
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
    }
}

fn ledger_lines(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.opts.out.join(LEDGER_FILE))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect()
}

#[test]
fn lost_submit_response_is_recorded_as_error_and_the_orphan_task_cancelled() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(0);
    x.lose_next_submit_responses(1);
    // submit の Err はその試行（A）だけを error にし、B/C は続行する（ADR 2026-10-16-campaign-arm-order-isolation D3）。
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("run continues");
    assert_eq!((rep.executed, rep.executor_errors), (3, 1));
    // 外部に出来た task は冪等キーで照会して cancel され、台帳は閉じる。再投入は起きない。
    assert_eq!(x.remote_task_count(), 3);
    assert_eq!(x.submit_calls(), 3);
    assert_eq!(x.cancelled().len(), 1, "the orphaned A task is cancelled");
    // 投入『前』に planned された行（routing 固定記録つき）が残っている。
    assert!(
        ledger_lines(&e)
            .iter()
            .any(|l| l["routing"]["mode"] == "mock")
    );
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    assert_eq!(
        lg.get(&key(&e, ConfigId::A)).expect("row").state,
        LedgerState::Cancelled
    );
    for c in [ConfigId::B, ConfigId::C] {
        assert_eq!(lg.get(&key(&e, c)).expect("row").state, LedgerState::Done);
    }
    // 再実行は何も足さない。
    let rep3 = run(&e.m, &e.i, &x, &e.opts).expect("again");
    assert_eq!((rep3.executed, rep3.resumed_skipped), (0, 3));
    assert_eq!(x.submit_calls(), 3);
}

#[test]
fn restart_with_planned_row_and_no_task_id() {
    let e = env(10_000_000);
    fs::create_dir_all(&e.opts.out).expect("out");
    // 投入前に落ちた状況: planned 行だけがあり、外部には何も無い。
    let mut lg = Ledger::open(&e.opts.out).expect("ledger");
    lg.plan(
        &key(&e, ConfigId::A),
        "trials/A/p0/1/workspace",
        json!({"mode":"x"}),
    )
    .expect("plan");
    drop(lg);
    let x = MockAsyncExecutor::new(0);
    run(&e.m, &e.i, &x, &e.opts).expect("run");
    assert_eq!(x.submit_calls(), 3);
    assert_eq!(x.remote_task_count(), 3);
    // 先に plan された routing 固定記録は上書きされない。
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    assert_eq!(
        lg.get(&key(&e, ConfigId::A)).expect("row").routing["mode"],
        "x"
    );
}

#[test]
fn restart_after_submit_before_recording_task_id_does_not_resubmit() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(0);
    run(&e.m, &e.i, &x, &e.opts).expect("run");
    // task_id 追記と done が落ちた状況を、planned 行だけの台帳 + 記録なしで再現する。
    fs::remove_file(e.opts.out.join(LEDGER_FILE)).expect("rm");
    fs::remove_file(e.opts.out.join(TRIALS_FILE)).expect("rm");
    run(&e.m, &e.i, &x, &e.opts).expect("rerun");
    assert_eq!(x.remote_task_count(), 3);
    assert_eq!(
        x.submit_calls(),
        3,
        "lookup by idempotency key, no new submit"
    );
}

#[test]
fn wait_keeps_workspace_and_resume_follows_same_task() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(1);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("run");
    assert_eq!((rep.executed, rep.waiting), (0, 3));
    assert!(
        fs::read_to_string(e.opts.out.join(TRIALS_FILE))
            .unwrap_or_default()
            .is_empty()
    );
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        let r = lg.get(&key(&e, c)).expect("row");
        assert_eq!(r.state, LedgerState::Waiting);
        assert!(lg.is_referenced(&r.workspace));
        assert!(
            e.opts.out.join(&r.workspace).is_dir(),
            "waiting workspace must stay: {}",
            r.workspace
        );
        assert_eq!(r.child_task_ids.len(), 1);
        assert_eq!(r.cluster_job_ids.len(), 1);
    }
    assert_eq!(lg.referenced_workspaces().len(), 3);
    let before: Vec<_> = lg.rows().map(|r| r.task_id.clone()).collect();
    let rep2 = run(&e.m, &e.i, &x, &e.opts).expect("resume");
    assert_eq!(rep2.executed, 3);
    assert_eq!(x.submit_calls(), 3);
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let after: Vec<_> = lg.rows().map(|r| r.task_id.clone()).collect();
    assert_eq!(before, after, "same tasks tracked");
    assert!(lg.referenced_workspaces().is_empty());
}

#[test]
fn budget_stop_cancels_unfinished_with_children_and_jobs() {
    let e = env(10_000);
    let x = MockAsyncExecutor::new(1);
    run(&e.m, &e.i, &x, &e.opts).expect("first");
    assert!(x.cancelled().is_empty());
    // 2 回目: A が 10000 token で完了して予算に達し、B/C は未完了のまま打ち切られる。
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("second");
    assert!(rep.budget_stopped);
    assert_eq!((rep.executed, rep.budget_cancelled), (1, 2));
    let c = x.cancelled();
    assert_eq!(c.len(), 2);
    for (t, reason) in &c {
        assert_eq!(reason, "budget_stop");
        assert_eq!(t.child_task_ids.len(), 1, "children are cancel targets");
        assert_eq!(
            t.cluster_job_ids.len(),
            1,
            "cluster jobs are cancel targets"
        );
    }
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    assert_eq!(
        lg.get(&key(&e, ConfigId::A)).expect("A").state,
        LedgerState::Done
    );
    for c in [ConfigId::B, ConfigId::C] {
        assert_eq!(
            lg.get(&key(&e, c)).expect("row").state,
            LedgerState::Cancelled
        );
    }
    assert!(lg.unfinished().is_empty());
    let tl = fs::read_to_string(e.opts.out.join(TRIALS_FILE)).expect("trials");
    assert_eq!(tl.matches("\"outcome\":\"cancelled\"").count(), 2);
}

#[test]
fn torn_ledger_tail_is_dropped_and_keys_are_stable() {
    let e = env(10_000_000);
    fs::create_dir_all(&e.opts.out).expect("out");
    let mut lg = Ledger::open(&e.opts.out).expect("ledger");
    lg.plan(&key(&e, ConfigId::B), "w", json!({}))
        .expect("plan");
    drop(lg);
    let p = e.opts.out.join(LEDGER_FILE);
    let mut s = fs::read_to_string(&p).expect("read");
    s.push_str("{\"schema\":\"tor");
    fs::write(&p, s).expect("write");
    let lg = Ledger::open(&e.opts.out).expect("reopen");
    assert_eq!(lg.rows().count(), 1);
    assert_eq!(
        key(&e, ConfigId::A).idempotency_key(),
        key(&e, ConfigId::A).idempotency_key()
    );
    assert_ne!(
        key(&e, ConfigId::A).idempotency_key(),
        key(&e, ConfigId::B).idempotency_key()
    );
}
