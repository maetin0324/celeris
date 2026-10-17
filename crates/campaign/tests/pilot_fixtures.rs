//! pilot manifest・代表 fixture・制御 mock 台本・誤り入りレビュー fixture の試験。
//! fixture の生成物（input-manifest.json・pilot.json・review/report-good.md）は
//! `CAMPAIGN_UPDATE_FIXTURES=1 cargo test -p celeris-campaign --test pilot_fixtures` で作り直す。
//! 通常は「置いてある fixture が現行の生成結果と一致する」ことだけを確かめる。
mod common;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use celeris_campaign::{
    checker::{Recomputed, check},
    executor::{MockExecutor, MockStep},
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    report::{check_report, compare},
    runner::{RunOptions, run},
    snapshot::hash_tree,
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn bin() -> Command {
    Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
}

fn build_input() -> Input {
    let src: Vec<Value> =
        serde_json::from_slice(&fs::read(fx().join("problems.json")).expect("problems.json"))
            .expect("problems json");
    let problems: Vec<Value> = src
        .iter()
        .map(|p| {
            json!({"id":p["id"],"task_id":"fixture","work_unit_id":p["id"],"created_at":"2026-10-15T00:00:00Z",
                "genre":p["genre"],"kind":"leaf","command":p["command"],"prompt":p["prompt"],
                "split":"dev","stratum":p["stratum"],"subset_12":false,"head_commit":null,"base_commit":null,
                "features_sha256":"","features":{},"sha256":""})
        })
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,
        "version":"pilot-fixture-1","cutoff":"2026-10-15","seed":1,"problems":problems,"sha256":""}))
    .expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn build_pilot(input: &Input) -> CampaignManifest {
    let cfg = |id, h: &str, hv: &str, pol: &str| Config {
        id,
        harness: h.into(),
        harness_version: hv.into(),
        model: "mock-model".into(),
        model_version: "mock-1".into(),
        policy_version: pol.into(),
        routing: None,
    };
    let mut m = CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "pilot-dev-1".into(),
        version: "1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input-manifest.json".into(),
            sha256: input.sha256.clone(),
        },
        problems: input.problems.iter().map(|p| p.id.clone()).collect(),
        configs: vec![
            cfg(ConfigId::A, "claude-code", "baseline-1", "ops-baseline"),
            cfg(ConfigId::B, "celeris", "1", "fixed"),
            cfg(ConfigId::C, "celeris", "1", "jev-1"),
        ],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec![
                "impl-benchfs-chunk-index".into(),
                "review-numeric-consistency".into(),
            ],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 1_000_000,
            max_wall_secs: 36_000,
            max_disk_bytes: 1 << 37,
            max_parallel: 2,
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
            repo_commit: "fixture-repo-1".into(),
            materials_sha256: hash_tree(&fx().join("materials")).expect("materials hash"),
            kb_snapshot: "kb-fixture-1".into(),
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        dependencies: Default::default(),
        sha256: String::new(),
    };
    m.sha256 = m.compute_sha256().expect("sha");
    m
}

fn pretty(v: &impl serde::Serialize) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("ser");
    s.push('\n');
    s
}

fn run_opts(out: &Path) -> RunOptions {
    RunOptions {
        out: out.to_path_buf(),
        repo_src: Some(fx().join("repo")),
        materials_src: Some(fx().join("materials")),
        kb_src: Some(fx().join("kb")),
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
    }
}

/// seeded mock で pilot を回し、checker の再計算から報告本文を作る。
fn good_report(m: &CampaignManifest, rc: &Recomputed) -> String {
    let mut s = String::from(
        "# pilot（dev・mock 実行器）結果\n\n実 LLM の測定ではない。各行は生記録 trials.jsonl の再計算値と一致する。\n\n",
    );
    for (c, t) in &rc.totals {
        s += &format!("result {c:?} passed={} n={} split=dev\n", t.passed, t.n);
    }
    s += &format!("\ncampaign_sha256: {}\n", m.compute_sha256().expect("sha"));
    s
}

fn pilot_run(out: &Path) -> (CampaignManifest, Input, Recomputed) {
    let input = build_input();
    let m = build_pilot(&input);
    run(&m, &input, &MockExecutor::seeded(), &run_opts(out)).expect("run");
    common::write_grades(&m, out, |_| true);
    let rep = check(&m, out);
    assert!(rep.ok, "{:?}", rep.violations);
    (m, input, rep.recomputed.expect("recomputed"))
}

#[test]
fn fixtures_are_current_or_regenerated() {
    let input = build_input();
    let pilot = build_pilot(&input);
    let dir = tempfile::tempdir().expect("dir");
    let (m, _, rc) = pilot_run(dir.path());
    let want = [
        ("input-manifest.json", pretty(&input)),
        ("pilot.json", pretty(&pilot)),
        ("review/report-good.md", good_report(&m, &rc)),
    ];
    for (name, content) in want {
        let p = fx().join(name);
        if std::env::var("CAMPAIGN_UPDATE_FIXTURES").as_deref() == Ok("1") {
            fs::write(&p, content).expect("write fixture");
        } else {
            assert_eq!(
                fs::read_to_string(&p).expect("fixture"),
                content,
                "{name} is stale: CAMPAIGN_UPDATE_FIXTURES=1 で作り直す"
            );
        }
    }
}

#[test]
fn representative_problems_cover_the_five_stages_dev_only() {
    let input: Input =
        serde_json::from_slice(&fs::read(fx().join("input-manifest.json")).expect("read"))
            .expect("parse");
    assert!((3..=5).contains(&input.problems.len()));
    let strata: Vec<&str> = input.problems.iter().map(|p| p.stratum.as_str()).collect();
    for s in [
        "implementation",
        "short-experiment",
        "analysis",
        "report",
        "review",
    ] {
        assert!(strata.contains(&s), "stage {s} missing");
    }
    assert!(input.problems.iter().all(|p| p.split == "dev"));
    let pilot = CampaignManifest::load(&fx().join("pilot.json")).expect("pilot");
    pilot.check_against_input(&input).expect("hash and split");
    assert_eq!(pilot.secondary.subset.len(), 2);
    assert_eq!(
        pilot.snapshot.materials_sha256,
        hash_tree(&fx().join("materials")).expect("hash")
    );
}

#[test]
fn pilot_dry_run_exits_zero_with_estimate() {
    let out = bin()
        .args(["dry-run", "--manifest"])
        .arg(fx().join("pilot.json"))
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["llm_calls_made"], 0);
    assert_eq!(v["resources"]["total_trials"], 5 + 5 + 2);
    assert_eq!(v["acceptance"]["within_budget"], true);
    assert_eq!(v["tokens"]["label"], "estimated");
    assert!(v["tokens"]["total"].as_u64().expect("total") > 0);
}

#[test]
fn pilot_mock_run_check_export_and_report_via_cli() {
    let dir = tempfile::tempdir().expect("dir");
    let out = dir.path().join("out");
    let pilot = fx().join("pilot.json");
    let ok = |args: Vec<String>| {
        let o = bin().args(&args).output().expect("run");
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let run_out = ok(vec![
        "run".into(),
        "--manifest".into(),
        s(&pilot),
        "--executor".into(),
        "mock".into(),
        "--out".into(),
        s(&out),
        "--repo-src".into(),
        s(&fx().join("repo")),
        "--materials-src".into(),
        s(&fx().join("materials")),
        "--kb-src".into(),
        s(&fx().join("kb")),
    ]);
    let r: Value = serde_json::from_str(&run_out).expect("run json");
    assert_eq!(r["executed"], 12);
    assert_eq!(r["budget_stopped"], false);
    common::write_grades(&CampaignManifest::load(&pilot).expect("m"), &out, |_| true);
    ok(vec![
        "check".into(),
        "--manifest".into(),
        s(&pilot),
        "--out".into(),
        s(&out),
    ]);
    ok(vec![
        "export".into(),
        "--manifest".into(),
        s(&pilot),
        "--out".into(),
        s(&out),
    ]);
    ok(vec![
        "check-report".into(),
        "--manifest".into(),
        s(&pilot),
        "--out".into(),
        s(&out),
        "--report".into(),
        s(&fx().join("review/report-good.md")),
    ]);
}

fn control_script() -> MockExecutor {
    let v: Value =
        serde_json::from_slice(&fs::read(fx().join("control/mock-script.json")).expect("read"))
            .expect("parse");
    let mut s = BTreeMap::new();
    for st in v["steps"].as_array().expect("steps") {
        let cfg = match st["config"].as_str().expect("config") {
            "A" => ConfigId::A,
            "B" => ConfigId::B,
            _ => ConfigId::C,
        };
        let step = match st["step"].as_str().expect("step") {
            "fail" => MockStep::Fail,
            "timeout" => MockStep::Timeout,
            "error" => MockStep::Error,
            "unmetered" => MockStep::PassUnmetered,
            o => panic!("unknown step {o}"),
        };
        s.insert(
            (cfg, st["problem"].as_str().expect("problem").to_string()),
            step,
        );
    }
    MockExecutor::scripted(s)
}

#[test]
fn control_script_keeps_failures_in_the_denominator() {
    let dir = tempfile::tempdir().expect("dir");
    let input = build_input();
    let m = build_pilot(&input);
    run(&m, &input, &control_script(), &run_opts(dir.path())).expect("run");
    let rep = check(&m, dir.path());
    assert!(rep.ok, "{:?}", rep.violations);
    let t = &rep.recomputed.expect("rc").totals;
    // A: fail 1 + timeout 1、B: error 1 + 欠測 pass 1、C(2 問): fail 1。分母は常に全問。
    assert_eq!((t[&ConfigId::A].n, t[&ConfigId::A].passed), (5, 3));
    assert_eq!((t[&ConfigId::A].failed, t[&ConfigId::A].timeout), (1, 1));
    assert_eq!((t[&ConfigId::B].n, t[&ConfigId::B].passed), (5, 4));
    assert_eq!(t[&ConfigId::B].error, 1);
    assert_eq!(t[&ConfigId::B].tokens_missing_trials, 1);
    assert_eq!((t[&ConfigId::C].n, t[&ConfigId::C].passed), (2, 1));
}

#[test]
fn budget_stop_on_pilot_cancels_rest_but_keeps_denominator() {
    let dir = tempfile::tempdir().expect("dir");
    let input = build_input();
    let mut m = build_pilot(&input);
    m.budget.max_tokens = 25_000; // mock は 1 試行 ≥10k token なので 3 試行目の前で止まる
    m.sha256 = m.compute_sha256().expect("sha");
    let rep = run(
        &m,
        &input,
        &MockExecutor::scripted(BTreeMap::new()),
        &run_opts(dir.path()),
    )
    .expect("run");
    assert!(rep.budget_stopped && rep.budget_cancelled > 0);
    let c = check(&m, dir.path());
    assert!(c.ok, "{:?}", c.violations);
    let t = &c.recomputed.expect("rc").totals;
    assert_eq!(t[&ConfigId::A].n, 5);
    assert!(t[&ConfigId::A].cancelled + t[&ConfigId::B].cancelled > 0);
}

fn mutate_trials(out: &Path, f: impl FnOnce(&mut Vec<Value>)) {
    let p = out.join("trials.jsonl");
    let mut recs: Vec<Value> = fs::read_to_string(&p)
        .expect("read")
        .lines()
        .map(|l| serde_json::from_str(l).expect("line"))
        .collect();
    f(&mut recs);
    let body: String = recs.iter().map(|r| format!("{r}\n")).collect();
    fs::write(p, body).expect("write");
}

/// 報告の config 行の `key=<n>` を書き換える。
fn edit_row(text: &str, cfg: &str, key: &str, f: impl Fn(u64) -> String) -> String {
    text.lines()
        .map(|l| {
            if l.starts_with(&format!("result {cfg} ")) {
                l.split(' ')
                    .map(|t| match t.strip_prefix(&format!("{key}=")) {
                        Some(x) => format!("{key}={}", f(x.parse().unwrap_or(0))),
                        None => t.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn checker_detects_every_known_error_in_the_review_fixtures() {
    let cases: Value =
        serde_json::from_slice(&fs::read(fx().join("review/cases.json")).expect("read"))
            .expect("parse");
    let cases = cases["cases"].as_array().expect("cases");
    assert!(cases.len() >= 6);
    let good = fs::read_to_string(fx().join("review/report-good.md")).expect("good report");
    for case in cases {
        let (name, kind, expect) = (
            case["name"].as_str().expect("name"),
            case["kind"].as_str().expect("kind"),
            case["expect"].as_str().expect("expect"),
        );
        let dir = tempfile::tempdir().expect("dir");
        let (m, _, rc) = pilot_run(dir.path());
        // 誤りを入れる前の組は合格する（偽陽性でないこと）。
        assert!(
            check_report(&m, dir.path(), &good).is_empty(),
            "{name}: control must pass"
        );
        let mut report = good.clone();
        match kind {
            "flip_passed_flag" => mutate_trials(dir.path(), |r| {
                let p = r[0]["passed"] == true;
                r[0]["passed"] = json!(!p);
            }),
            "drop_failed_record" => mutate_trials(dir.path(), |r| {
                let i = r
                    .iter()
                    .position(|x| x["outcome"] != "passed")
                    .expect("a non-pass record");
                r.remove(i);
            }),
            "mix_split_record" => mutate_trials(dir.path(), |r| r[0]["split"] = json!("final")),
            "report_inflate_passed" => {
                report = edit_row(&good, "A", "passed", |x| (x + 1).to_string())
            }
            "report_drop_denominator" => {
                report = edit_row(&good, "B", "n", |x| (x - 1).to_string())
            }
            "report_split_mixed" => report = edit_row(&good, "C", "split", |_| "final".into()),
            o => panic!("unknown case kind {o}"),
        }
        let v = check_report(&m, dir.path(), &report);
        assert!(
            v.iter().any(|x| x.contains(expect)),
            "{name}: expected a violation containing `{expect}`, got {v:?}"
        );
        // 報告の誤りは生記録側 checker が無傷でも compare 単独で検出できる（独立判定）。
        if kind.starts_with("report_") {
            assert!(check(&m, dir.path()).ok, "{name}: records are intact");
            assert!(!compare(&m, &rc, &report).is_empty());
        }
    }
}

#[test]
fn check_report_cli_fails_on_bad_report() {
    let dir = tempfile::tempdir().expect("dir");
    let (_m, _, _) = pilot_run(&dir.path().join("out"));
    let good = fs::read_to_string(fx().join("review/report-good.md")).expect("good");
    let bad = dir.path().join("bad.md");
    fs::write(
        &bad,
        edit_row(&good, "A", "passed", |x| (x + 1).to_string()),
    )
    .expect("write");
    let o = bin()
        .args(["check-report", "--manifest"])
        .arg(fx().join("pilot.json"))
        .arg("--out")
        .arg(dir.path().join("out"))
        .arg("--report")
        .arg(&bad)
        .output()
        .expect("run");
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("recomputed"));
}
