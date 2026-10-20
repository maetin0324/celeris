//! 冪等な試行台帳と非同期実行器: 投入前の永続化、応答喪失・再起動後の重複投入なし、
//! Wait 中の workspace 保持、打ち切り時の cancel（子 task・job を含む）。
use std::{collections::BTreeMap, fs};

use celeris_campaign::{
    executor::{ExecError, ExecStatus, Executor, MockAsyncExecutor, TaskRef, TrialRequest},
    ledger::{LEDGER_FILE, Ledger, LedgerState, TrialKey},
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, TokenUsage, manifest_hash,
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
        fault_scenarios: Default::default(),
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

fn key(e: &Env, arm: ConfigId) -> TrialKey {
    TrialKey {
        manifest_sha256: e.m.compute_sha256().expect("hash"),
        problem_id: "p0".into(),
        arm,
        trial_no: 1,
        rep: 1,
    }
}

#[test]
fn lost_submit_response_is_unknown_not_error_and_never_resubmitted() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(0);
    x.lose_next_submit_responses(1);
    // POST の応答喪失は『投入結果不明』: error にも cancel にもせず、A は次回の照会に回る。B/C は続行する。
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("run continues");
    assert_eq!((rep.executed, rep.executor_errors, rep.waiting), (2, 0, 1));
    assert_eq!((x.submit_calls(), x.remote_task_count()), (3, 3));
    assert!(x.cancelled().is_empty(), "nothing is cancelled on unknown");
    // 投入『前』に planned された行（routing 固定記録・開始時刻・予約つき）が残り、state は submit_unknown。
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let a = lg.get(&key(&e, ConfigId::A)).expect("row");
    assert_eq!(a.state, LedgerState::SubmitUnknown);
    assert!(a.task_id.is_none() && a.started_at.is_some() && a.reserved.is_some());
    assert_eq!(a.comm_failures, 1);
    assert!(a.routing["mode"] == "mock");
    assert!(lg.is_referenced(&a.workspace), "workspace is kept");
    // 再実行（台帳の再読込後）: 冪等キーの照会で見つかった task を追う。再 submit はしない。
    let rep2 = run(&e.m, &e.i, &x, &e.opts).expect("resume");
    assert_eq!((rep2.executed, rep2.waiting), (1, 0));
    assert_eq!((x.submit_calls(), x.remote_task_count()), (3, 3));
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        assert_eq!(lg.get(&key(&e, c)).expect("row").state, LedgerState::Done);
    }
    let rep3 = run(&e.m, &e.i, &x, &e.opts).expect("again");
    assert_eq!((rep3.executed, rep3.resumed_skipped), (0, 3));
    assert_eq!(x.submit_calls(), 3);
}

#[test]
fn unknown_submit_not_found_by_lookup_is_never_resubmitted() {
    let e = env(10_000_000);
    fs::create_dir_all(&e.opts.out).expect("out");
    // 応答喪失の後、再起動した状況: submit_unknown の行があり、外部の照会ではまだ見えない。
    let mut lg = Ledger::open(&e.opts.out).expect("ledger");
    lg.plan(
        &key(&e, ConfigId::A),
        "trials/A/p0/rep1/workspace",
        json!({}),
    )
    .expect("plan");
    lg.mark_submit_unknown(&key(&e, ConfigId::A), "submit: lost")
        .expect("unknown");
    drop(lg);
    let x = MockAsyncExecutor::new(0);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("run");
    assert_eq!((rep.executed, rep.waiting), (2, 1));
    assert_eq!(x.submit_calls(), 2, "A is not resubmitted while unknown");
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let a = lg.get(&key(&e, ConfigId::A)).expect("row");
    assert_eq!(a.state, LedgerState::SubmitUnknown);
    assert!(a.comm_failures >= 2);
}

/// 全試行が最初に `Wait` を返し、2 回目以降の poll で終わる状況を作る。
fn waiting(e: &Env, x: &MockAsyncExecutor) {
    let rep = run(&e.m, &e.i, x, &e.opts).expect("first");
    assert_eq!((rep.executed, rep.waiting), (0, 3));
}

#[test]
fn poll_connection_loss_keeps_the_task_and_resumes_after_restart() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(1);
    waiting(&e, &x);
    let ids: Vec<_> = Ledger::open(&e.opts.out)
        .expect("ledger")
        .rows()
        .map(|r| r.task_id.clone())
        .collect();
    // 2 回目: A の poll が通信断。error・消費 0・cancel・done のいずれにもならず、行は task を保つ。
    x.set_usage(Some(TokenUsage::full(30_000, 0, 0, 0)), 5);
    x.fail_next_polls(1);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("second");
    assert_eq!((rep.executed, rep.executor_errors, rep.waiting), (2, 0, 1));
    assert!(x.cancelled().is_empty());
    let lg = Ledger::open(&e.opts.out).expect("reopen");
    let a = lg.get(&key(&e, ConfigId::A)).expect("row");
    assert_eq!(a.state, LedgerState::Waiting);
    assert_eq!(a.comm_failures, 1);
    assert!(a.last_error.as_deref().unwrap_or("").contains("poll"));
    assert!(a.started_at.is_some() && a.reserved.is_some());
    assert!(lg.is_referenced(&a.workspace));
    assert_eq!(
        lg.rows().map(|r| r.task_id.clone()).collect::<Vec<_>>(),
        ids,
        "same tasks"
    );
    // 3 回目（再起動後の別 run）: 同じ task へ再接続して最終結果を回収する。再投入は無い。
    let rep3 = run(&e.m, &e.i, &x, &e.opts).expect("third");
    assert_eq!((rep3.executed, rep3.waiting), (1, 0));
    assert_eq!((x.submit_calls(), x.remote_task_count()), (3, 3));
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let a = lg.get(&key(&e, ConfigId::A)).expect("row");
    assert_eq!((a.state, a.comm_failures), (LedgerState::Done, 0));
    assert!(check_ok(&e));
}

fn check_ok(e: &Env) -> bool {
    let t = fs::read_to_string(e.opts.out.join(TRIALS_FILE)).expect("trials");
    t.lines().count() == 3 && !t.contains("executor_error")
}

#[test]
fn observed_usage_survives_usage_and_poll_failures_and_restart() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(100);
    x.set_usage(Some(TokenUsage::full(40_000, 0, 0, 0)), 7);
    waiting(&e, &x);
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let seen = lg.get(&key(&e, ConfigId::B)).expect("row").observed_usage;
    assert_eq!(seen.map(|u| u.wall_secs), Some(7));
    // usage の取得が通信断: 既観測 usage は消えず、試行は error にも消費 0 の完了にもならない。
    x.fail_next_usages(3);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("second");
    assert_eq!((rep.executed, rep.executor_errors, rep.waiting), (0, 0, 3));
    let lg = Ledger::open(&e.opts.out).expect("reopen");
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        let r = lg.get(&key(&e, c)).expect("row");
        assert_eq!(r.observed_usage, seen, "observed usage is kept");
        assert_eq!(r.comm_failures, 1);
        assert_eq!(r.state, LedgerState::Waiting);
    }
}

#[test]
fn cancel_failure_stays_cancel_pending_until_the_external_task_is_stopped() {
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(100);
    waiting(&e, &x);
    // 上限超過（per_case 100k）を観測 → cancel するが、A の cancel が通信断。
    x.set_usage(Some(TokenUsage::full(200_000, 0, 0, 0)), 5);
    x.fail_next_cancels(1);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("second");
    assert_eq!(rep.case_budget_cancelled, 3);
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let a = lg.get(&key(&e, ConfigId::A)).expect("row");
    assert_eq!(a.state, LedgerState::CancelPending);
    assert!(!a.state.is_terminal(), "cancel failure is not terminal");
    assert!(a.last_error.as_deref().unwrap_or("").contains("cancel"));
    assert!(
        lg.is_referenced(&a.workspace),
        "reservation is not released"
    );
    assert_eq!(x.cancelled().len(), 2, "only B and C were cancelled");
    // 次の run: 新しい仕事より先に cancel を再試行する。停止の確認が取れない間は pending のまま。
    x.unconfirmed_stops(1);
    run(&e.m, &e.i, &x, &e.opts).expect("third");
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    assert_eq!(
        lg.get(&key(&e, ConfigId::A)).expect("row").state,
        LedgerState::CancelPending,
        "cancel accepted but the external task is not yet terminal"
    );
    run(&e.m, &e.i, &x, &e.opts).expect("fourth");
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        assert_eq!(
            lg.get(&key(&e, c)).expect("row").state,
            LedgerState::Cancelled
        );
    }
    assert!(lg.unfinished().is_empty());
    let stopped: std::collections::BTreeSet<_> =
        x.cancelled().into_iter().map(|(t, _)| t.task_id).collect();
    assert_eq!(stopped.len(), 3, "A's task was finally cancelled too");
    // 記録は 3 試行ぶんで、cancel の再試行が記録を増やさない。
    let tl = fs::read_to_string(e.opts.out.join(TRIALS_FILE)).expect("trials");
    assert_eq!(tl.lines().count(), 3);
}

#[test]
fn certain_not_sent_keeps_planned_and_retries_without_error_record() {
    let e = env(10_000_000);
    let x = NotSentOnce {
        inner: MockAsyncExecutor::new(0),
        left: std::sync::atomic::AtomicU32::new(1),
    };
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("run");
    assert_eq!((rep.executed, rep.executor_errors, rep.waiting), (2, 0, 1));
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    let a = lg.get(&key(&e, ConfigId::A)).expect("row");
    assert_eq!(a.state, LedgerState::Planned);
    assert!(a.task_id.is_none());
    assert_eq!(x.inner.remote_task_count(), 2, "nothing was posted for A");
    let rep2 = run(&e.m, &e.i, &x, &e.opts).expect("retry");
    assert_eq!((rep2.executed, rep2.waiting), (1, 0));
    assert_eq!(x.inner.remote_task_count(), 3);
}

/// 最初の submit だけ『確実に未投入』で失敗する executor。
struct NotSentOnce {
    inner: MockAsyncExecutor,
    left: std::sync::atomic::AtomicU32,
}

impl Executor for NotSentOnce {
    fn name(&self) -> &'static str {
        "not-sent-once"
    }
    fn is_async(&self) -> bool {
        true
    }
    fn execute(&self, _r: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only".into())
    }
    fn routing(&self, r: &TrialRequest) -> Result<Value, String> {
        self.inner.routing(r)
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        self.inner.lookup(key)
    }
    fn submit(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.left.load(SeqCst) > 0 {
            self.left.fetch_sub(1, SeqCst);
            return Err(ExecError::not_sent("connection refused"));
        }
        self.inner.submit(r, key)
    }
    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        self.inner.poll(t)
    }
}

#[test]
fn legacy_ledger_rows_without_the_new_fields_are_readable() {
    let e = env(10_000_000);
    fs::create_dir_all(&e.opts.out).expect("out");
    let k = key(&e, ConfigId::A);
    let line = json!({
        "schema": "celeris.campaign-ledger/1",
        "manifest_sha256": k.manifest_sha256, "problem_id": "p0", "arm": "A", "trial_no": 1,
        "state": "waiting", "idempotency_key": k.idempotency_key(), "task_id": "task-9",
        "child_task_ids": [], "cluster_job_ids": [], "routing": null,
        "workspace": "trials/A/p0/rep1/workspace", "submitted_at": 5,
    });
    fs::write(e.opts.out.join(LEDGER_FILE), format!("{line}\n")).expect("write");
    let lg = Ledger::open(&e.opts.out).expect("legacy row reads");
    let r = lg.get(&k).expect("row");
    assert_eq!(r.state, LedgerState::Waiting);
    assert!(r.started_at.is_none() && r.observed_usage.is_none() && r.reserved.is_none());
    assert_eq!(r.comm_failures, 0);
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
    // 予約（ADR 2026-10-16-campaign-budget-reservation）で global を超える投入はしないので、
    // in-flight の試行が打ち切られるのは実行中の使用量が上限を超えたとき。子 task・cluster job ごと cancel する。
    let e = env(10_000_000);
    let x = MockAsyncExecutor::new(100);
    run(&e.m, &e.i, &x, &e.opts).expect("first");
    assert!(x.cancelled().is_empty());
    x.set_usage(Some(TokenUsage::full(200_000, 0, 0, 0)), 5);
    let rep = run(&e.m, &e.i, &x, &e.opts).expect("second");
    assert_eq!(rep.case_budget_cancelled, 3);
    let c = x.cancelled();
    assert_eq!(c.len(), 3);
    for (t, reason) in &c {
        assert_eq!(reason, "case_budget");
        assert_eq!(t.child_task_ids.len(), 1, "children are cancel targets");
        assert_eq!(
            t.cluster_job_ids.len(),
            1,
            "cluster jobs are cancel targets"
        );
    }
    let lg = Ledger::open(&e.opts.out).expect("ledger");
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        assert_eq!(
            lg.get(&key(&e, c)).expect("row").state,
            LedgerState::Cancelled
        );
    }
    assert!(lg.unfinished().is_empty());
    let tl = fs::read_to_string(e.opts.out.join(TRIALS_FILE)).expect("trials");
    assert_eq!(tl.matches("\"outcome\":\"cancelled\"").count(), 3);
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
