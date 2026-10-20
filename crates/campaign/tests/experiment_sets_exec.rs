//! 実験セット（A/B のみ・B/C のみ）を runner・checker・export・dry-run・report・全体台帳の通常経路で通す統合試験。
//! 外部 network・LLM は使わない（mock 実行器）。ADR 2026-10-19-experiment-sets D7。
mod common;
use std::{collections::BTreeMap, fs, path::Path};

use celeris_campaign::{
    checker::check,
    dryrun,
    executor::{MockExecutor, MockStep},
    export::export,
    global_ledger::GlobalLedger,
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, ExperimentSet, InputRef, Secondary,
        SnapshotRef, Split, Termination, manifest_hash,
    },
    record::*,
    report::check_report,
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

fn set(arms: &[ConfigId], compare: (ConfigId, ConfigId)) -> ExperimentSet {
    ExperimentSet {
        arms: arms.to_vec(),
        compare: vec![compare],
        fixed: Default::default(),
    }
}

/// `which` = "ab"（A/B のみ）か "bc"（B/C のみ）。測定 ID は which。
fn campaign(i: &Input, which: &str, n: usize, max_tokens: u64) -> CampaignManifest {
    let cfg = |id, h: &str, model: &str| Config {
        id,
        harness: h.into(),
        harness_version: "1".into(),
        model: model.into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: None,
    };
    let (arms, pair, problems): (Vec<ConfigId>, _, Vec<String>) = if which == "ab" {
        (
            vec![ConfigId::A, ConfigId::B],
            (ConfigId::A, ConfigId::B),
            (0..n).map(|k| format!("p{k}")).collect(),
        )
    } else {
        (
            vec![ConfigId::B, ConfigId::C],
            (ConfigId::B, ConfigId::C),
            vec!["p0".to_string(), "p1".to_string()],
        )
    };
    let configs = arms
        .iter()
        .map(|a| match a {
            ConfigId::A => cfg(ConfigId::A, "claude-code", "sonnet"),
            ConfigId::B => cfg(ConfigId::B, "celeris", "sonnet"),
            ConfigId::C => cfg(ConfigId::C, "celeris", "sonnet"),
        })
        .collect();
    CampaignManifest {
        variant: None,
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: which.into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: problems.clone(),
        configs,
        experiment_sets: BTreeMap::from([(which.to_string(), set(&arms, pair))]),
        selected_sets: vec![which.to_string()],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: if which == "bc" { problems } else { Vec::new() },
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 4,
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
        sha256: String::new(),
    }
}

fn opts(out: &Path, global: Option<&Path>) -> RunOptions {
    RunOptions {
        out: out.to_path_buf(),
        global_ledger: global.map(Path::to_path_buf),
        ..RunOptions::default()
    }
}

fn arms_of(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|c| c["config"].as_str().expect("config").to_string())
        .collect()
}

#[test]
fn ab_only_never_plans_runs_counts_or_reports_c() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(3);
    let m = campaign(&i, "ab", 3, 10_000_000);
    m.validate().expect("valid");
    assert!(plan(&m).iter().all(|(c, _, _)| *c != ConfigId::C));
    assert_eq!(plan(&m).len(), 6);
    let out = d.path().join("out");
    let rep = run(
        &m,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &opts(&out, None),
    )
    .expect("run");
    assert_eq!(rep.executed, 6);
    let c = check(&m, &out);
    assert!(c.ok, "{:?}", c.violations);
    // summary・export・dry-run は有効 arm だけ。C は分母にも表にも出ない。
    let sm: Value =
        serde_json::from_slice(&fs::read(out.join(SUMMARY_FILE)).expect("s")).expect("j");
    let cells: Vec<&String> = sm["cells"].as_object().expect("cells").keys().collect();
    assert_eq!(cells, ["A", "B"]);
    common::write_grades(&m, &out, |_| true);
    let t = export(&m, &out, &d.path().join("ex")).expect("export");
    assert_eq!(arms_of(&t["configs"]), ["A", "B"]);
    assert_eq!(t["comparisons"].as_array().expect("cmp").len(), 1);
    assert_eq!(t["comparisons"][0]["name"], "primary");
    let dr = dryrun::estimate(&m, &i).expect("dryrun");
    assert_eq!(arms_of(&dr["tokens"]["per_config"]), ["A", "B"]);
    assert_eq!(dr["resources"]["total_trials"], 6);
    // C の result 行は有効計画に無いので報告検査が拒否する。
    let rep_txt = "result A passed=3 n=3 split=dev\nresult B passed=3 n=3 split=dev\nresult C passed=1 n=1 split=dev\n";
    assert!(
        check_report(&m, &out, rep_txt)
            .iter()
            .any(|v| v.contains("not in the effective plan"))
    );
    let ok_txt = "result A passed=3 n=3 split=dev\nresult B passed=3 n=3 split=dev\n";
    assert!(check_report(&m, &out, ok_txt).is_empty());
}

#[test]
fn bc_only_runs_no_a_and_inactive_records_fail_the_checker() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(4);
    let m = campaign(&i, "bc", 4, 10_000_000);
    m.validate().expect("valid");
    assert!(plan(&m).iter().all(|(c, _, _)| *c != ConfigId::A));
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &opts(&out, None),
    )
    .expect("run");
    assert!(check(&m, &out).ok);
    common::write_grades(&m, &out, |_| true);
    let t = export(&m, &out, &d.path().join("ex")).expect("export");
    assert_eq!(arms_of(&t["configs"]), ["B", "C"]);
    assert_eq!(t["comparisons"][0]["name"], "secondary");
    // 未使用 arm A の記録を混ぜたら checker が失敗する。
    let trials = out.join(TRIALS_FILE);
    let text = fs::read_to_string(&trials).expect("t");
    let first = text
        .lines()
        .next()
        .expect("line")
        .replace("\"config\":\"B\"", "\"config\":\"A\"");
    fs::write(&trials, format!("{text}{first}\n")).expect("w");
    let c = check(&m, &out);
    assert!(
        !c.ok
            && c.violations
                .iter()
                .any(|v| v.contains("not an expected trial")),
        "{:?}",
        c.violations
    );
}

#[test]
fn resume_with_changed_arms_under_the_same_measurement_id_is_rejected() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, "ab", 2, 10_000_000);
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &opts(&out, None),
    )
    .expect("run");
    // 同じ測定 ID（ab）で有効 arm を B/C へ変える。
    let mut other = campaign(&i, "bc", 2, 10_000_000);
    other.campaign_id = "ab".into();
    let e = run(
        &other,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &opts(&out, None),
    )
    .expect_err("rejected");
    assert!(
        e.contains("resume rejected") || e.contains("different campaign"),
        "{e}"
    );
}

#[test]
fn b0_observed_non_sonnet_run_fails_the_checker() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, "ab", 2, 10_000_000);
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &opts(&out, None),
    )
    .expect("run");
    assert!(check(&m, &out).ok);
    let trials = out.join(TRIALS_FILE);
    let text = fs::read_to_string(&trials).expect("t");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let idx = lines
        .iter()
        .position(|l| l.contains("\"config\":\"B\""))
        .expect("b");
    let mut v: Value = serde_json::from_str(&lines[idx]).expect("j");
    v["observed"] = json!([{"run_id":"r1","role":"reviewer","model":"claude-opus-5-5"}]);
    lines[idx] = v.to_string();
    fs::write(&trials, lines.join("\n") + "\n").expect("w");
    let c = check(&m, &out);
    assert!(
        c.violations
            .iter()
            .any(|x| x.contains("B0") && x.contains("non-Sonnet")),
        "{:?}",
        c.violations
    );
}

#[test]
fn global_ledger_carries_consumption_across_measurement_ids_without_double_count() {
    let d = tempfile::tempdir().expect("dir");
    let g = d.path().join("global");
    let i = input(2);
    let spend = |arm| BTreeMap::from([((arm, "p0".to_string()), MockStep::Spend(50_000))]);
    // 1) A/B のセット（測定 ID ab）。
    let ab = campaign(&i, "ab", 2, 400_000);
    run(
        &ab,
        &i,
        &MockExecutor::scripted(spend(ConfigId::A)),
        &opts(&d.path().join("ab"), Some(&g)),
    )
    .expect("ab");
    let gl = GlobalLedger::open(&g).expect("open");
    let row_ab = gl.current().expect("cur")["ab"].clone();
    assert!(row_ab.tokens >= 50_000 && row_ab.trials == 4, "{row_ab:?}");
    // 2) 再実行（resume/retry の再集計）は加算しない。
    run(
        &ab,
        &i,
        &MockExecutor::scripted(spend(ConfigId::A)),
        &opts(&d.path().join("ab"), Some(&g)),
    )
    .expect("ab again");
    assert_eq!(gl.current().expect("cur")["ab"], row_ab);
    // 3) B/C のセット（別測定 ID）。ab の既消費を保持したまま動き、自分の行を足す。
    let bc = campaign(&i, "bc", 2, 400_000);
    run(
        &bc,
        &i,
        &MockExecutor::scripted(spend(ConfigId::C)),
        &opts(&d.path().join("bc"), Some(&g)),
    )
    .expect("bc");
    let cur = gl.current().expect("cur");
    assert_eq!(cur["ab"], row_ab);
    // provider 枠は未終了予約の合計（宣言 max_parallel の max 集計ではない）。終わった測定は枠を持たない。
    assert!(cur["bc"].tokens >= 50_000 && cur["bc"].provider_slots == 0);
    assert_eq!(gl.others("bc").expect("o").tokens, row_ab.tokens);
    // 4) 横断上限を超える測定は、out の作成も投入もする前に拒否する。
    let tight = campaign(&i, "bc", 2, row_ab.tokens);
    let mut tight = tight;
    tight.campaign_id = "bc2".into();
    let out = d.path().join("bc2");
    let e = run(
        &tight,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &opts(&out, Some(&g)),
    )
    .expect_err("over cap");
    assert!(e.contains("global ledger rejects"), "{e}");
    assert!(!out.exists());
}
