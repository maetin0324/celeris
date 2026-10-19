//! 独立反復（rep）と分母（ADR 2026-10-16-campaign-repeats-denominator）。
use std::{fs, path::Path, sync::Mutex};

use celeris_campaign::{
    aggregate::{Outcome, TrialResult, aggregate},
    checker::check,
    executor::{ExecResult, ExecStatus, Executor, TrialRequest},
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, run},
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

fn campaign(i: &Input, n: usize, repeats: u32) -> CampaignManifest {
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
                max_tokens: 10_000_000,
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
        repeats,
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

/// (arm, problem, rep) で結果を決める sync executor。使った workspace を覚える。
struct ByRep {
    seen: Mutex<Vec<(ConfigId, String, u32, String)>>,
}

impl Executor for ByRep {
    fn name(&self) -> &'static str {
        "by-rep"
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        self.seen.lock().map_err(|e| e.to_string())?.push((
            req.config.id,
            req.problem_id.to_string(),
            req.rep,
            req.workspace.display().to_string(),
        ));
        // 成果物を残す（A は workspace が変わらないと no_artifact で failed になる）。
        std::fs::write(req.workspace.join("out.txt"), "x").map_err(|e| e.to_string())?;
        let outcome = match (req.config.id, req.rep) {
            (ConfigId::A, 2) => Outcome::Failed,
            (ConfigId::B, 3) => Outcome::Timeout,
            (ConfigId::C, 1) => return Err("boom".into()),
            _ => Outcome::Passed,
        };
        let wall_secs = if outcome == Outcome::Timeout {
            req.timeout_secs
        } else {
            1
        };
        Ok(ExecStatus::Done(ExecResult {
            outcome,
            tokens: Some(celeris_campaign::manifest::TokenUsage::full(10, 0, 0, 0)),
            cost_micro_usd: None,
            wall_secs,
        }))
    }
}

fn manifest_with(f: impl FnOnce(&mut CampaignManifest)) -> CampaignManifest {
    let i = input(2);
    let mut m = campaign(&i, 2, 1);
    f(&mut m);
    m
}

#[test]
fn attempts_other_than_one_and_zero_repeats_are_rejected() {
    for a in [0, 2, 3] {
        let m = manifest_with(|m| m.termination.max_attempts_per_trial = a);
        assert!(m.validate().is_err(), "attempts {a}");
    }
    assert!(manifest_with(|m| m.repeats = 0).validate().is_err());
    assert!(manifest_with(|m| m.repeats = 3).validate().is_ok());
}

#[test]
fn repeats_run_under_separate_ids_and_outs_and_the_denominator_is_the_plan() {
    let tmp = tempfile::tempdir().expect("tmp");
    let i = input(2);
    let m = campaign(&i, 2, 3);
    m.validate().expect("valid");
    let ex = ByRep {
        seen: Mutex::default(),
    };
    let o = opts(tmp.path());
    let out = o.out.clone();
    let rep = run(&m, &i, &ex, &o).expect("run");
    // A/B: 2 problems × 3 reps、C: subset(p0) × 3 reps。
    assert_eq!(rep.executed, 6 + 6 + 3);

    let recs = records(&out);
    assert_eq!(recs.len(), 15);
    let wss: std::collections::BTreeSet<&str> = recs
        .iter()
        .map(|r| r["workspace"].as_str().expect("ws"))
        .collect();
    assert_eq!(wss.len(), 15, "each repetition has its own out");
    for k in 1..=3 {
        assert!(out.join(format!("trials/A/p0/rep{k}/workspace")).is_dir());
        let r = recs
            .iter()
            .find(|r| r["config"] == "A" && r["problem_id"] == "p0" && r["rep"] == k)
            .expect("rec");
        assert_eq!(r["trial_no"], 1, "repetition is not an attempt");
    }
    let keys: std::collections::BTreeSet<String> = (1..=3)
        .map(|rep| {
            celeris_campaign::ledger::TrialKey {
                manifest_sha256: "h".into(),
                problem_id: "p0".into(),
                arm: ConfigId::A,
                trial_no: 1,
                rep,
            }
            .idempotency_key()
        })
        .collect();
    assert_eq!(keys.len(), 3);

    let s = summary(&out);
    for (arm, n) in [("A", 6), ("B", 6), ("C", 3)] {
        assert_eq!(s["cells"][arm]["n"], n, "{arm}");
    }
    assert_eq!(s["cells"]["A"]["failed"], 2);
    assert_eq!(s["cells"]["A"]["passed"], 4);
    assert_eq!(s["cells"]["B"]["timeout"], 2);
    assert_eq!(s["cells"]["C"]["error"], 1);
    let rep = check(&m, &out);
    assert!(rep.ok, "{:?}", rep.violations);
    let t = &rep.recomputed.expect("recomputed").totals;
    assert_eq!((t[&ConfigId::A].n, t[&ConfigId::A].passed), (6, 4));
}

#[test]
fn cancelled_and_missing_stay_in_the_planned_denominator() {
    let i = input(2);
    let m = campaign(&i, 2, 3);
    let hash = m.compute_sha256().expect("hash");
    let r = |cfg, p: &str, rep, outcome| TrialResult {
        campaign_sha256: hash.clone(),
        split: Split::Dev,
        config: cfg,
        problem_id: p.into(),
        trial_no: 1,
        rep,
        outcome,
    };
    let rs = vec![
        r(ConfigId::A, "p0", 1, Outcome::Passed),
        r(ConfigId::A, "p0", 2, Outcome::Cancelled),
        r(ConfigId::A, "p0", 3, Outcome::Failed),
        r(ConfigId::A, "p1", 1, Outcome::Timeout),
        r(ConfigId::A, "p1", 2, Outcome::Error),
        // p1 rep3 は未完了（missing）
        r(ConfigId::B, "p0", 1, Outcome::Blocked),
    ];
    let cells = aggregate(&m, &rs).expect("aggregate");
    let a = &cells[&ConfigId::A];
    assert_eq!(a.n, 6);
    assert_eq!(
        (
            a.passed,
            a.failed,
            a.timeout,
            a.error,
            a.cancelled,
            a.missing
        ),
        (1, 1, 1, 1, 1, 1)
    );
    // 率は計画数割り（除外した 1/(6-…) ではない）。
    assert_eq!(
        a.passed + a.failed + a.timeout + a.error + a.cancelled + a.missing,
        a.n
    );
    assert!((a.passed as f64 / a.n as f64 - 1.0 / 6.0).abs() < 1e-12);
    let b = &cells[&ConfigId::B];
    assert_eq!((b.n, b.blocked, b.missing), (6, 1, 5));

    // 範囲外の rep は拒否する。
    assert!(aggregate(&m, &[r(ConfigId::A, "p0", 4, Outcome::Passed)]).is_err());
    assert!(aggregate(&m, &[r(ConfigId::A, "p0", 0, Outcome::Passed)]).is_err());
}

fn records(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join(TRIALS_FILE))
        .expect("read")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect()
}

fn summary(out: &Path) -> Value {
    serde_json::from_slice(&fs::read(out.join(SUMMARY_FILE)).expect("summary")).expect("json")
}
