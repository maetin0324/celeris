//! 実行中の予算上限（per_case / per_arm）。実時間は使わず、使用量を台本で返す注入 executor で決定的に確かめる
//! （ADR 2026-10-16-campaign-budget-cap）。
use std::{collections::BTreeMap, fs, path::Path, sync::Mutex};

use celeris_campaign::{
    aggregate::Outcome,
    checker::check,
    executor::{
        ExecError, ExecResult, ExecStatus, Executor, TaskRef, TokenUsage, TrialRequest, Usage,
    },
    ledger::Ledger,
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    record::*,
    runner::{OVERSHOOT_GRANULARITY, RunOptions, RunReport, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

const CASE_TOKENS: u64 = 1_000;

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn campaign(i: &Input, n: usize, arm_tokens: u64) -> CampaignManifest {
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
        contract: None,
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
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            // in-flight 上限はこの試験の対象ではない（max_parallel は parallel_cap.rs）。
            max_parallel: 64,
            per_case: Cap {
                max_tokens: CASE_TOKENS,
                max_wall_secs: 600,
            },
            per_arm: Cap {
                max_tokens: arm_tokens,
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

/// 1 試行の台本。`running` は poll ごと（async）に見える累積使用量、`done` は最終結果。
#[derive(Clone)]
struct Script {
    running: Vec<Usage>,
    done: ExecResult,
}

fn ok(tokens: Option<u64>, wall: u64) -> ExecResult {
    ExecResult {
        outcome: Outcome::Passed,
        tokens: tokens.map(|t| TokenUsage::full(t, 0, 0, 0)),
        cost_micro_usd: None,
        wall_secs: wall,
    }
}

struct Scripted {
    is_async: bool,
    scripts: BTreeMap<(ConfigId, String), Script>,
    default_done: ExecResult,
    polls: Mutex<BTreeMap<String, usize>>,
    cancelled: Mutex<Vec<(TaskRef, String)>>,
    seen_caps: Mutex<Vec<(ConfigId, String, Cap)>>,
    keys: Mutex<BTreeMap<String, (ConfigId, String)>>,
}

impl Scripted {
    fn new(is_async: bool, scripts: Vec<((ConfigId, &str), Script)>) -> Self {
        Self {
            is_async,
            scripts: scripts
                .into_iter()
                .map(|((c, p), s)| ((c, p.to_string()), s))
                .collect(),
            default_done: ok(Some(100), 10),
            polls: Mutex::default(),
            cancelled: Mutex::default(),
            seen_caps: Mutex::default(),
            keys: Mutex::default(),
        }
    }
    fn script(&self, c: ConfigId, p: &str) -> Script {
        self.scripts
            .get(&(c, p.to_string()))
            .cloned()
            .unwrap_or(Script {
                running: vec![],
                done: self.default_done.clone(),
            })
    }
}

impl Executor for Scripted {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn is_async(&self) -> bool {
        self.is_async
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        self.seen_caps.lock().map_err(|e| e.to_string())?.push((
            req.config.id,
            req.problem_id.to_string(),
            req.budget,
        ));
        Ok(ExecStatus::Done(
            self.script(req.config.id, req.problem_id).done,
        ))
    }
    fn submit(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        self.seen_caps.lock().map_err(|e| e.to_string())?.push((
            req.config.id,
            req.problem_id.to_string(),
            req.budget,
        ));
        let mut k = self.keys.lock().map_err(|e| e.to_string())?;
        k.insert(key.to_string(), (req.config.id, req.problem_id.to_string()));
        Ok(TaskRef {
            task_id: format!("t-{key}"),
            run_id: None,
            child_task_ids: vec![format!("c-{key}")],
            cluster_job_ids: vec![format!("j-{key}")],
        })
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        let known = self
            .keys
            .lock()
            .map_err(|e| e.to_string())?
            .contains_key(key);
        Ok(known.then(|| TaskRef {
            task_id: format!("t-{key}"),
            run_id: None,
            child_task_ids: vec![format!("c-{key}")],
            cluster_job_ids: vec![format!("j-{key}")],
        }))
    }
    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        let key = t.task_id.trim_start_matches("t-").to_string();
        let (c, p) = self
            .keys
            .lock()
            .map_err(|e| e.to_string())?
            .get(&key)
            .cloned()
            .ok_or("unknown task")?;
        let s = self.script(c, &p);
        let mut polls = self.polls.lock().map_err(|e| e.to_string())?;
        let n = polls.entry(key).or_default();
        *n += 1;
        if *n <= s.running.len() {
            Ok(ExecStatus::Wait)
        } else {
            Ok(ExecStatus::Done(s.done))
        }
    }
    fn usage(&self, t: &TaskRef) -> Result<Option<Usage>, ExecError> {
        let key = t.task_id.trim_start_matches("t-").to_string();
        let (c, p) = self
            .keys
            .lock()
            .map_err(|e| e.to_string())?
            .get(&key)
            .cloned()
            .ok_or("unknown task")?;
        let n = *self
            .polls
            .lock()
            .map_err(|e| e.to_string())?
            .get(&key)
            .unwrap_or(&0);
        Ok(self.script(c, &p).running.get(n.saturating_sub(1)).copied())
    }
    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), ExecError> {
        self.cancelled
            .lock()
            .map_err(|e| e.to_string())?
            .push((t.clone(), reason.into()));
        Ok(())
    }
}

fn records(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join(TRIALS_FILE))
        .expect("read")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect()
}

fn rec<'a>(v: &'a [Value], cfg: &str, p: &str) -> &'a Value {
    v.iter()
        .find(|r| r["config"] == cfg && r["problem_id"] == p)
        .expect("record")
}

fn summary(out: &Path) -> Value {
    serde_json::from_slice(&fs::read(out.join(SUMMARY_FILE)).expect("summary")).expect("json")
}

fn usage(tokens: Option<u64>, wall_secs: u64) -> Usage {
    Usage {
        tokens: tokens.map(|t| TokenUsage::full(t, 0, 0, 0)),
        wall_secs,
    }
}

#[test]
fn async_running_trial_over_case_cap_is_cancelled_with_children_and_jobs() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i, 1, 1_000_000);
    let ex = Scripted::new(
        true,
        vec![(
            (ConfigId::A, "p0"),
            Script {
                running: vec![usage(Some(400), 30), usage(Some(1_500), 90)],
                done: ok(Some(9_999), 100),
            },
        )],
    );
    let o = opts(d.path());
    // 1 回目の poll は上限内: A は外部待ち（B/C は止まらずに完了する）。
    let r1 = run(&m, &i, &ex, &o).expect("run1");
    assert_eq!((r1.waiting, r1.case_budget_cancelled), (1, 0));
    assert!(ex.cancelled.lock().expect("lock").is_empty());
    // 2 回目の poll で累積 1500 > 1000: 実行中のまま cancel される。
    let r2: RunReport = run(&m, &i, &ex, &o).expect("run2");
    assert_eq!(r2.case_budget_cancelled, 1);
    assert_eq!(r2.max_overshoot_tokens, 500);

    let c = ex.cancelled.lock().expect("lock").clone();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].1, REASON_CASE_BUDGET);
    assert_eq!(c[0].0.child_task_ids.len(), 1);
    assert_eq!(c[0].0.cluster_job_ids.len(), 1);

    let v = records(&o.out);
    let a = rec(&v, "A", "p0");
    assert_eq!(a["outcome"], "cancelled");
    assert_eq!(a["reason"], REASON_CASE_BUDGET);
    assert_eq!(a["tokens"], 1_500);
    assert_ne!(rec(&v, "B", "p0")["outcome"], "cancelled");
    assert!(
        Ledger::open(&o.out)
            .expect("ledger")
            .unfinished()
            .is_empty()
    );
    let rep = check(&m, &o.out);
    assert!(rep.ok, "{:?}", rep.violations);
    // 取り消された試行も分母に残る。
    let t = &rep.recomputed.expect("rc").totals[&ConfigId::A];
    assert_eq!((t.n, t.passed, t.cancelled), (1, 0, 1));
    let s = summary(&o.out);
    assert_eq!(s["budget"]["max_overshoot_tokens"], 500);
    assert_eq!(s["budget"]["overshoot_granularity"], OVERSHOOT_GRANULARITY);
}

#[test]
fn sync_executor_receives_the_cap_and_overrun_is_cancelled_after_the_fact() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i, 1, 1_000_000);
    let mut ex = Scripted::new(false, vec![]);
    ex.scripts.insert(
        (ConfigId::A, "p0".into()),
        Script {
            running: vec![],
            done: ok(Some(5_000), 60),
        },
    );
    let o = opts(d.path());
    let rep = run(&m, &i, &ex, &o).expect("run");
    assert_eq!(rep.case_budget_cancelled, 1);
    assert_eq!(rep.max_overshoot_tokens, 4_000);
    let caps = ex.seen_caps.lock().expect("lock").clone();
    assert!(caps.iter().all(|(_, _, c)| c.max_tokens == CASE_TOKENS));
    let v = records(&o.out);
    let a = rec(&v, "A", "p0");
    assert_eq!(
        (a["outcome"].clone(), a["reason"].clone()),
        ("cancelled".into(), REASON_CASE_BUDGET.into())
    );
    assert_eq!(a["tokens"], 5_000, "the measured usage is kept");
    assert_ne!(rec(&v, "B", "p0")["outcome"], "cancelled");
    assert!(check(&m, &o.out).ok);
}

#[test]
fn missing_tokens_are_never_free_and_exhaust_only_their_own_arm() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(4);
    // arm 上限 2500、1 試行の上限 1000。欠測は 1000 を消費したとみなす。
    let m = campaign(&i, 4, 2_500);
    let mut ex = Scripted::new(false, vec![]);
    for p in ["p0", "p1", "p2", "p3"] {
        ex.scripts.insert(
            (ConfigId::A, p.into()),
            Script {
                running: vec![],
                done: ok(None, 10),
            },
        );
    }
    let o = opts(d.path());
    let rep = run(&m, &i, &ex, &o).expect("run");
    // 3 件で 3000 >= 2500 となり、4 件目の A は実行されない。欠測は別に数える。
    assert_eq!(rep.tokens_missing, 3);
    assert_eq!(rep.arm_budget_cancelled, 1);
    assert_eq!(
        rep.tokens_total,
        100 * 5,
        "observed tokens only (B 4 + C 1)"
    );
    assert_eq!(rep.tokens_charged_total, 3_000 + 500);
    let v = records(&o.out);
    let a3 = rec(&v, "A", "p3");
    assert_eq!(a3["outcome"], "cancelled");
    assert_eq!(a3["reason"], REASON_ARM_BUDGET);
    assert_eq!(a3["workspace"], "");
    assert!(
        rec(&v, "A", "p0")["tokens"].is_null(),
        "missing stays null, not 0"
    );
    // 3 件目は arm の残り 500 が実効上限として渡された。
    let caps = ex.seen_caps.lock().expect("lock").clone();
    let third = caps
        .iter()
        .find(|(c, p, _)| *c == ConfigId::A && p == "p2")
        .expect("cap");
    assert_eq!(third.2.max_tokens, 500);
    // B は A の枯渇で止まらない。
    assert!((0..4).all(|k| rec(&v, "B", &format!("p{k}"))["outcome"] != "cancelled"));
    let s = summary(&o.out);
    assert_eq!(s["budget"]["tokens_missing"], 3);
    assert_eq!(s["cells"]["A"]["cancelled"], 1);
    assert_eq!(s["cells"]["A"]["n"], 4);
    let rep = check(&m, &o.out);
    assert!(rep.ok, "{:?}", rep.violations);
}

#[test]
fn in_flight_missing_tokens_do_not_trip_the_token_cap_but_wall_still_does() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i, 1, 1_000_000);
    let ex = Scripted::new(
        true,
        vec![(
            (ConfigId::A, "p0"),
            Script {
                running: vec![usage(None, 100), usage(None, 700)],
                done: ok(None, 800),
            },
        )],
    );
    let o = opts(d.path());
    let r1 = run(&m, &i, &ex, &o).expect("run1");
    assert_eq!((r1.waiting, r1.case_budget_cancelled), (1, 0));
    let r2 = run(&m, &i, &ex, &o).expect("run2");
    assert_eq!(r2.case_budget_cancelled, 1, "wall 700 > 600");
    assert_eq!(r2.max_overshoot_wall_secs, 100);
    let v = records(&o.out);
    let a = rec(&v, "A", "p0");
    assert_eq!(a["reason"], REASON_CASE_BUDGET);
    assert!(a["tokens"].is_null());
    assert_eq!(r2.tokens_missing, 1);
    assert_eq!(r2.tokens_charged_total, CASE_TOKENS + 200);
}

#[test]
fn checker_rejects_a_record_over_the_case_cap_that_was_not_cancelled() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i, 1, 1_000_000);
    let ex = Scripted::new(false, vec![]);
    let o = opts(d.path());
    run(&m, &i, &ex, &o).expect("run");
    assert!(check(&m, &o.out).ok);
    let mut v = records(&o.out);
    v[0]["tokens"] = json!(5_000);
    let s: String = v.iter().map(|x| format!("{x}\n")).collect();
    fs::write(o.out.join(TRIALS_FILE), s).expect("write");
    let rep = check(&m, &o.out);
    assert!(
        rep.violations.iter().any(|x| x.contains("per-case budget")),
        "{:?}",
        rep.violations
    );
}

#[test]
fn per_arm_below_per_case_is_rejected() {
    let i = input(1);
    let mut m = campaign(&i, 1, 1_000_000);
    m.budget.per_arm.max_tokens = CASE_TOKENS - 1;
    assert!(m.validate().unwrap_err().contains("per_arm"));
}
