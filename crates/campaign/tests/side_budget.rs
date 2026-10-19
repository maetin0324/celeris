//! B1/C1（同一測定系列）の累計予算: manifest hash を変えた再 manifest でも既消費が承認総枠へ合算され、
//! 超過なら実行前に拒否する。主比較（side でない manifest）の計上は不変。
use std::{collections::BTreeMap, path::Path};

use celeris_campaign::{
    SIDE_CAMPAIGN_SCHEMA, budget,
    executor::MockAsyncExecutor,
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary,
        SnapshotRef, Split, Termination, TokenBasis, manifest_hash,
    },
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::json;

const CASE: u64 = 100_000;

fn input() -> Input {
    let p = json!({"id":"p0","task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""});
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":[p],"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn cfg(id: ConfigId, policy: &str) -> Config {
    Config {
        id,
        harness: "celeris".into(),
        harness_version: "0123456789ab".into(),
        model: "claude-sonnet-5-5".into(),
        model_version: "claude-sonnet-5-5".into(),
        policy_version: policy.into(),
        routing: Some(ArmRouting {
            lower_policy: Some("mlsys-min-1".into()),
            online_update: Some(false),
            models: Some(vec!["claude-sonnet-5-5".into(), "claude-opus-5-5".into()]),
            run_harnesses: Some(vec!["claude-code".into()]),
            no_evidence_table: (id == ConfigId::C).then_some(true),
            evidence_table_id: None,
        }),
    }
}

/// B1/C1 の別測定 manifest。`version` を変えると hash だけが変わる（同一系列 = 同じ campaign_id）。
fn side(i: &Input, version: &str, max_tokens: u64) -> CampaignManifest {
    let mut m = CampaignManifest {
        schema: SIDE_CAMPAIGN_SCHEMA.into(),
        campaign_id: "jev-b1c1-series".into(),
        version: version.into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: vec!["p0".into()],
        configs: vec![cfg(ConfigId::B, "fixed"), cfg(ConfigId::C, "jev-1")],
        primary: (ConfigId::B, ConfigId::C),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: TokenBasis::InputOutput,
            max_tokens,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 8,
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
        order: Default::default(),
        seed: 0,
        repeats: 1,
        snapshot: SnapshotRef {
            repo_commit: "abc".into(),
            materials_sha256: "d".into(),
            kb_snapshot: "kb@1".into(),
        },
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    };
    m.sha256 = m.compute_sha256().expect("hash");
    m
}

fn opts(out: &Path) -> RunOptions {
    RunOptions {
        out: out.to_path_buf(),
        ..RunOptions::default()
    }
}

/// v1 を最後まで実行して消費を作る。戻りは (root, 入力, v1 が確定した tokens)。
fn consumed_v1() -> (tempfile::TempDir, Input, u64) {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let m1 = side(&i, "v1", 10_000_000);
    let x = MockAsyncExecutor::new(0);
    let out1 = d.path().join("out-v1");
    for _ in 0..4 {
        run(&m1, &i, &x, &opts(&out1)).expect("v1");
    }
    let prior = budget::series_prior_spend(&side(&i, "v2", 10_000_000), "other", &out1)
        .expect("spend of v1 itself is excluded");
    assert_eq!(prior.tokens, 0, "the out itself is never its own prior");
    let m2 = side(&i, "v2", 10_000_000);
    let h2 = m2.compute_sha256().expect("hash");
    let p = budget::series_prior_spend(&m2, &h2, &d.path().join("out-v2")).expect("prior");
    (d, i, p.tokens)
}

#[test]
fn hash_change_still_counts_prior_spend_of_the_same_series() {
    let (d, i, tokens) = consumed_v1();
    assert!(
        tokens > 0,
        "v1 consumption is visible under a different hash"
    );
    let m2 = side(&i, "v2", 10_000_000);
    let m1 = side(&i, "v1", 10_000_000);
    assert_ne!(m1.compute_sha256().unwrap(), m2.compute_sha256().unwrap());
    // 別系列（campaign_id が違う）の消費は合算しない。
    let mut other = side(&i, "v2", 10_000_000);
    other.campaign_id = "another-series".into();
    other.sha256 = other.compute_sha256().expect("hash");
    let h = other.compute_sha256().unwrap();
    let none = budget::series_prior_spend(&other, &h, &d.path().join("out-x")).expect("prior");
    assert_eq!(none.tokens, 0);
}

#[test]
fn run_is_refused_before_any_effect_when_prior_spend_reaches_the_approved_total() {
    let (d, i, tokens) = consumed_v1();
    // 承認総枠 = 既消費ちょうど: 新 hash で何も使えないので、out も作らず拒否する。
    let m2 = side(&i, "v2", tokens);
    let out2 = d.path().join("out-v2");
    let x = MockAsyncExecutor::new(0);
    let err = run(&m2, &i, &x, &opts(&out2)).expect_err("over the approved total");
    assert!(err.contains("approved total"), "{err}");
    assert_eq!(x.submit_calls(), 0);
    assert!(!out2.exists(), "refused before creating the out dir");
}

#[test]
fn remaining_total_below_one_reservation_submits_nothing() {
    let (d, i, tokens) = consumed_v1();
    // 残り < 1 試行の予約: 既消費を合算しなければ（新 hash だけを見れば）投入されてしまう量。
    let m2 = side(&i, "v2", tokens + CASE / 2);
    let x = MockAsyncExecutor::new(0);
    let rep = run(&m2, &i, &x, &opts(&d.path().join("out-v2"))).expect("run");
    assert!(rep.budget_stopped);
    assert_eq!(x.submit_calls(), 0);
}

#[test]
fn primary_manifest_never_counts_sibling_outs() {
    let (d, i, _) = consumed_v1();
    let mut p = side(&i, "v2", 10_000_000);
    p.schema = celeris_campaign::CAMPAIGN_SCHEMA.into();
    let h = p.compute_sha256().unwrap();
    let s = budget::series_prior_spend(&p, &h, &d.path().join("out-v2")).expect("prior");
    assert_eq!((s.tokens, s.in_flight), (0, 0));
}
