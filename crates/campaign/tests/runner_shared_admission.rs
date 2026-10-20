//! runner の全投入経路の共有予約（全体台帳）。毎投入の最新全体読取・最小上限・予約が executor より前に
//! fsync され、再開・通信断・cancel 未確認で予約を失わず、二重投入・二重課金しないこと（契約は
//! global_ledger.rs の module doc）。合成値の mock だけを使い、sleep・負荷で交差を待たない。
use std::{collections::BTreeMap, sync::Mutex};

use celeris_campaign::{
    executor::{ExecStatus, Executor, MockAsyncExecutor, MockExecutor, TokenUsage, TrialRequest},
    global_ledger::{
        Admission, AttemptState, Caps, Consumed, GlobalLedger, Published, TrialAdmission,
    },
    ledger::TrialKey,
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, TokenBasis, manifest_hash,
    },
    record::*,
    runner::{RunOptions, RunReport, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::json;

const MID: &str = "shared";
const CASE: u64 = 100_000;

fn input() -> Input {
    let p = json!({"id":"p0","task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""});
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":[p],"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

/// p0 の A/B/C の 3 試行。
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
        variant: None,
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: MID.into(),
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
            token_basis: TokenBasis::InputOutput,
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 64,
            per_case: Cap {
                max_tokens: CASE,
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

struct Env {
    d: tempfile::TempDir,
    i: Input,
    m: CampaignManifest,
}

impl Env {
    fn new() -> Self {
        let i = input();
        let m = campaign(&i);
        Self {
            d: tempfile::tempdir().expect("dir"),
            i,
            m,
        }
    }
    fn gdir(&self) -> std::path::PathBuf {
        self.d.path().join("global")
    }
    fn g(&self) -> GlobalLedger {
        GlobalLedger::open(&self.gdir()).expect("global")
    }
    fn run(&self, exec: &dyn Executor) -> RunReport {
        let opts = RunOptions {
            out: self.d.path().join("out"),
            global_ledger: Some(self.gdir()),
            ..RunOptions::default()
        };
        run(&self.m, &self.i, exec, &opts).expect("run")
    }
    fn records(&self) -> Vec<TrialRecord> {
        std::fs::read_to_string(self.d.path().join("out").join(TRIALS_FILE))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("record"))
            .collect()
    }
    fn key(&self, arm: ConfigId) -> TrialKey {
        TrialKey {
            manifest_sha256: self.m.compute_sha256().expect("hash"),
            problem_id: "p0".into(),
            arm,
            trial_no: 1,
            rep: 1,
        }
    }
    fn consumed(&self) -> Consumed {
        self.g().current().expect("current")[MID].consumed
    }
    fn outstanding(&self) -> usize {
        self.g().outstanding(MID).expect("outstanding").len()
    }
}

fn publish_other(g: &GlobalLedger, tokens: u64, caps: Caps) {
    g.record(&Published {
        measurement_id: "other".into(),
        manifest_sha256: "sha-other".into(),
        consumed: Consumed {
            tokens,
            wall_secs: 0,
            trials: 1,
        },
        caps,
    })
    .expect("publish other");
}

/// 同期 executor の spy。execute のたびに要求の上限を記録し、最初の呼び出しの中で別測定 ID の消費を増やす
/// （開始後に他 ID が増えた状況を出来事で作る）。
struct Spy {
    inner: MockExecutor,
    budgets: Mutex<Vec<u64>>,
    grow_other: Option<(std::path::PathBuf, u64)>,
}

impl Executor for Spy {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn simulated(&self) -> bool {
        true
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        let mut b = self.budgets.lock().expect("lock");
        if b.is_empty()
            && let Some((dir, tokens)) = &self.grow_other
        {
            publish_other(&GlobalLedger::open(dir)?, *tokens, Caps::UNLIMITED);
        }
        b.push(req.budget.max_tokens);
        drop(b);
        self.inner.execute(req)
    }
}

fn spy(grow_other: Option<(std::path::PathBuf, u64)>) -> Spy {
    Spy {
        inner: MockExecutor::scripted(BTreeMap::new()),
        budgets: Mutex::new(Vec::new()),
        grow_other,
    }
}

#[test]
fn sync_runs_without_stop_on_budget_still_see_other_ids_growth_before_each_submit() {
    let mut e = Env::new();
    e.m.budget.max_tokens = 250_000;
    e.m.termination.stop_on_budget = false;
    // 1 試行目の実行中に別測定 ID が 150k を消費する。2 試行目: 150k + 自分 10k + 予約 100k > 250k。
    let s = spy(Some((e.gdir(), 150_000)));
    let rep = e.run(&s);
    assert_eq!(
        s.budgets.lock().expect("lock").len(),
        1,
        "no submit after the shared cap"
    );
    assert!(rep.budget_stopped);
    let stopped = e
        .records()
        .iter()
        .filter(|r| r.reason.as_deref() == Some(REASON_BUDGET_STOP))
        .count();
    assert_eq!(stopped, 2);
    assert_eq!(e.outstanding(), 0);
    assert_eq!(e.consumed().tokens, 10_000);
}

#[test]
fn persisted_low_cap_binds_the_real_submission_even_with_a_larger_manifest() {
    let e = Env::new();
    // 古い承認の最小 cap（50k）が台帳に残る。manifest は 10M・per_case 100k: 1 試行も投入しない。
    publish_other(
        &e.g(),
        0,
        Caps {
            tokens: 50_000,
            trials: 100,
            provider_slots: 64,
        },
    );
    let s = spy(None);
    let rep = e.run(&s);
    assert!(s.budgets.lock().expect("lock").is_empty(), "0 submits");
    assert!(rep.budget_stopped);
    assert_eq!(e.outstanding(), 0);

    // cap が per_case 以上なら投入し、要求の上限は共有台帳で押さえた額（per_case）を超えない。
    let e = Env::new();
    publish_other(
        &e.g(),
        0,
        Caps {
            tokens: 150_000,
            trials: 100,
            provider_slots: 64,
        },
    );
    let s = spy(None);
    e.run(&s);
    let b = s.budgets.lock().expect("lock").clone();
    // 1 試行目 100k 予約 → 10k で精算、2 試行目 10k + 100k ≤ 150k、3 試行目 20k + 100k ≤ 150k。
    assert_eq!(b, vec![CASE, CASE, CASE]);
    assert_eq!(e.consumed().tokens, 30_000);
}

#[test]
fn shared_provider_slots_defer_async_submission_until_another_id_settles() {
    let mut e = Env::new();
    e.m.budget.max_parallel = 1;
    let g = e.g();
    let other = match g
        .admit_trial(&TrialAdmission {
            measurement_id: "other".into(),
            trial: "t0".into(),
            manifest_sha256: "sha-other".into(),
            caps: Caps::UNLIMITED,
            tokens: 1,
            provider_slots: 1,
            adopt: false,
        })
        .expect("other")
    {
        Admission::Granted { id, .. } => id,
        Admission::Rejected(r) => panic!("{}", r.msg),
    };
    let exec = MockAsyncExecutor::new(0);
    let rep = e.run(&exec);
    assert_eq!(exec.submit_calls(), 0, "slot held by another id");
    assert_eq!(rep.reserve_deferred, 3);
    assert!(e.records().is_empty());

    g.settle(
        &other,
        celeris_campaign::global_ledger::Terminal::NotSubmitted,
    )
    .expect("settle other");
    e.run(&exec);
    assert_eq!(exec.submit_calls(), 3);
    assert_eq!(e.records().len(), 3);
    assert_eq!(e.outstanding(), 0);
}

/// 全試行の共有予約が精算されるまで run を繰り返す（上限付き）。
fn drain(e: &Env, exec: &dyn Executor) {
    for _ in 0..6 {
        e.run(exec);
        if e.records().len() == 3 && e.outstanding() == 0 {
            return;
        }
    }
    panic!("did not settle");
}

#[test]
fn async_resume_and_lost_submit_response_keep_one_reservation_per_trial() {
    let e = Env::new();
    let exec = MockAsyncExecutor::new(1);
    exec.lose_next_submit_responses(1);
    let rep = e.run(&exec);
    assert_eq!(rep.waiting, 3);
    // 投入済み（応答喪失を含む）の試行は予約を持ち続ける。
    let st = e.g().state().expect("state");
    assert_eq!(
        st.attempts
            .values()
            .filter(|a| a.state.outstanding())
            .count(),
        3
    );
    assert!(
        st.attempts
            .values()
            .any(|a| a.state == AttemptState::Reserved),
        "submit outcome unknown stays reserved, not released"
    );
    drain(&e, &exec);
    assert_eq!(exec.submit_calls(), 3, "no duplicate submit");
    let st = e.g().state().expect("state");
    assert!(
        st.attempts.keys().all(|a| a.attempt == 1),
        "same idempotency key"
    );
    let c = e.consumed();
    assert_eq!((c.tokens, c.trials), (30_000, 3), "charged once");
}

#[test]
fn crash_between_shared_reserve_and_local_plan_is_released_only_after_lookup() {
    let e = Env::new();
    let g = e.g();
    // 共有予約の fsync 後・ローカル台帳の plan 前に止まった状態（合成）。
    let a = TrialAdmission {
        measurement_id: MID.into(),
        trial: e.key(ConfigId::B).idempotency_key(),
        manifest_sha256: e.m.compute_sha256().expect("hash"),
        caps: Caps::UNLIMITED,
        tokens: CASE,
        provider_slots: 1,
        adopt: false,
    };
    assert!(matches!(
        g.admit_trial(&a).expect("reserve"),
        Admission::Granted { .. }
    ));
    let exec = MockAsyncExecutor::new(0);
    drain(&e, &exec);
    assert_eq!(exec.submit_calls(), 3);
    let st = g.state().expect("state");
    let b: Vec<_> = st
        .attempts
        .values()
        .filter(|r| r.id.trial == a.trial)
        .collect();
    assert_eq!(b.len(), 2);
    assert_eq!(b[0].usage.tokens, 0, "lookup proved not submitted");
    assert!(!b[0].usage.ran);
    let c = e.consumed();
    assert_eq!((c.tokens, c.trials), (30_000, 3));
}

#[test]
fn sync_crash_mid_execute_is_charged_conservatively_and_retry_keeps_it() {
    let e = Env::new();
    let g = e.g();
    let a = TrialAdmission {
        measurement_id: MID.into(),
        trial: e.key(ConfigId::A).idempotency_key(),
        manifest_sha256: e.m.compute_sha256().expect("hash"),
        caps: Caps::UNLIMITED,
        tokens: CASE,
        provider_slots: 1,
        adopt: false,
    };
    assert!(matches!(
        g.admit_trial(&a).expect("reserve"),
        Admission::Granted { .. }
    ));
    let s = spy(None);
    e.run(&s);
    assert_eq!(s.budgets.lock().expect("lock").len(), 3);
    assert_eq!(e.outstanding(), 0);
    // 実行中に止まった attempt 1 は予約額を消費として残し、retry（attempt 2）の 10k を足す。
    let c = e.consumed();
    assert_eq!(c.tokens, CASE + 30_000);
    assert_eq!(c.trials, 3, "the retried trial counts once");
}

#[test]
fn unconfirmed_cancel_keeps_the_shared_reservation_until_stop_is_confirmed() {
    let e = Env::new();
    let exec = MockAsyncExecutor::new(5);
    exec.set_usage(Some(TokenUsage::full(200_000, 0, 0, 0)), 10);
    exec.unconfirmed_stops(1);
    e.run(&exec);
    assert_eq!(e.records().len(), 3);
    assert_eq!(e.outstanding(), 1, "cancel not confirmed: still held");
    e.run(&exec);
    assert_eq!(e.outstanding(), 0);
    let st = e.g().state().expect("state");
    assert!(st.attempts.keys().all(|a| a.attempt == 1));
    assert_eq!(
        e.consumed().tokens,
        3 * 200_000,
        "observed usage, once each"
    );
    assert_eq!(exec.submit_calls(), 3);
}
