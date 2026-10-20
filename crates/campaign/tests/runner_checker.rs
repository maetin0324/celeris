//! mock 実行器で A/B/C を回し、checker の再計算・export・改ざん検出・隔離・再開・予算停止を確かめる。
mod common;
use std::{collections::BTreeMap, fs, path::Path};

use celeris_campaign::{
    checker::check,
    executor::{MockExecutor, MockStep},
    export::export,
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

fn input(n: usize, split: &str) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":split,"stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn campaign(i: &Input, split: Split, n: usize, max_tokens: u64, repeats: u32) -> CampaignManifest {
    let cfg = |id, h: &str| Config {
        id,
        harness: h.into(),
        harness_version: if h == "celeris" {
            "0123456789ab".into()
        } else {
            "1".into()
        },
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        // final では B/C の routing 固定が要る（C は根拠表の版も）。
        routing: (id != ConfigId::A).then(|| celeris_campaign::manifest::ArmRouting {
            lower_policy: Some("lp".into()),
            evidence_table_id: Some("ev".into()),
            online_update: Some(false),
            ..Default::default()
        }),
    };
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "c1".into(),
        version: "v1".into(),
        split,
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
            subset: vec!["p0".into(), "p1".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens,
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
            trial_timeout_secs: 300,
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

fn script() -> MockExecutor {
    let mut s = BTreeMap::new();
    s.insert((ConfigId::A, "p1".to_string()), MockStep::Fail);
    s.insert((ConfigId::A, "p2".to_string()), MockStep::Timeout);
    s.insert((ConfigId::B, "p2".to_string()), MockStep::Error);
    s.insert((ConfigId::A, "p3".to_string()), MockStep::PassUnmetered);
    MockExecutor::scripted(s)
}

fn seeded_src(dir: &Path) -> RunOptions {
    let (r, k) = (dir.join("src-repo"), dir.join("src-kb"));
    fs::create_dir_all(&r).expect("mkdir");
    fs::create_dir_all(&k).expect("mkdir");
    fs::write(r.join("a.rs"), "fn main(){}").expect("w");
    fs::write(k.join("kb.md"), "seed").expect("w");
    RunOptions {
        global_ledger: None,
        out: dir.join("out"),
        repo_src: Some(r),
        materials_src: None,
        kb_src: Some(k),
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
    }
}

struct Env {
    _d: tempfile::TempDir,
    m: CampaignManifest,
    opts: RunOptions,
}

fn ran(max_tokens: u64, repeats: u32) -> Env {
    let d = tempfile::tempdir().expect("dir");
    let i = input(4, "dev");
    let m = campaign(&i, Split::Dev, 4, max_tokens, repeats);
    let opts = seeded_src(d.path());
    run(&m, &i, &script(), &opts).expect("run");
    Env { _d: d, m, opts }
}

fn lines(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join(TRIALS_FILE))
        .expect("read")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect()
}

fn write_lines(out: &Path, v: &[Value]) {
    let s: String = v.iter().map(|x| format!("{x}\n")).collect();
    fs::write(out.join(TRIALS_FILE), s).expect("write");
}

#[test]
fn abc_run_is_rechecked_and_exported_consistently() {
    let e = ran(10_000_000, 1);
    let out = &e.opts.out;
    let rep = check(&e.m, out);
    assert!(rep.ok, "{:?}", rep.violations);
    let rc = rep.recomputed.expect("recomputed");
    let a = &rc.totals[&ConfigId::A];
    // A: p0 pass, p1 fail, p2 timeout, p3 pass(unmetered). 失敗・timeout は分母 4 に残る。
    assert_eq!(
        (a.n, a.passed, a.failed, a.timeout, a.error, a.missing),
        (4, 2, 1, 1, 0, 0)
    );
    let b = &rc.totals[&ConfigId::B];
    assert_eq!((b.n, b.passed, b.error), (4, 3, 1));
    let c = &rc.totals[&ConfigId::C];
    assert_eq!(
        (c.n, c.passed),
        (2, 2),
        "C runs only the pre-registered subset"
    );
    assert_eq!(a.tokens_missing_trials, 1);

    common::write_grades(&e.m, out, |_| true);
    let t = export(&e.m, out, &out.join("export")).expect("export");
    // export は runner の summary.json と一致する。
    let summary: Value =
        serde_json::from_slice(&fs::read(out.join(SUMMARY_FILE)).expect("r")).expect("j");
    for row in t["configs"].as_array().expect("rows") {
        let cell = &summary["cells"][row["config"].as_str().expect("c")];
        for k in [
            "n",
            "passed",
            "failed",
            "error",
            "timeout",
            "cancelled",
            "missing",
        ] {
            assert_eq!(row[k], cell[k], "{} {k}", row["config"]);
        }
    }
    let rows = t["configs"].as_array().expect("rows");
    assert_eq!(rows[0]["tokens_status"], "partial");
    assert_eq!(rows[0]["timeout"], 1);
    assert_eq!(t["comparisons"][0]["name"], "primary");
    let csv = fs::read_to_string(out.join("export/table.csv")).expect("csv");
    assert_eq!(csv.lines().count(), 4);
    assert!(csv.lines().next().expect("h").starts_with("config,harness"));
}

#[test]
fn missing_usage_is_labelled_not_zero() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2, "dev");
    let mut m = campaign(&i, Split::Dev, 2, 10_000_000, 1);
    m.secondary.subset = vec!["p0".into()];
    let mut s = BTreeMap::new();
    for p in ["p0", "p1"] {
        s.insert((ConfigId::A, p.to_string()), MockStep::PassUnmetered);
    }
    let opts = seeded_src(d.path());
    run(&m, &i, &MockExecutor::scripted(s), &opts).expect("run");
    let t = export(&m, &opts.out, &d.path().join("x")).expect("export");
    assert_eq!(t["configs"][0]["tokens_status"], "missing");
    assert!(t["configs"][0]["tokens_sum"].is_null());
    let csv = fs::read_to_string(d.path().join("x/table.incomplete.csv")).expect("csv");
    assert!(csv.lines().nth(1).expect("row").contains("欠測"));
}

#[test]
fn workspaces_and_kbs_are_isolated_and_snapshot_is_saved() {
    let e = ran(10_000_000, 2);
    let out = &e.opts.out;
    for f in [
        CAMPAIGN_FILE,
        INPUT_FILE,
        META_FILE,
        SNAPSHOT_FILE,
        SUMMARY_FILE,
    ] {
        assert!(out.join(f).exists(), "{f}");
    }
    assert!(out.join("snapshot/repo/a.rs").exists());
    assert!(out.join("snapshot/kb/kb.md").exists());
    let snap: Value =
        serde_json::from_slice(&fs::read(out.join(SNAPSHOT_FILE)).expect("r")).expect("j");
    let recs = lines(out);
    assert_eq!(
        recs.len(),
        10 * 2,
        "A,B x4 + C x2 problems, 2 repetitions each"
    );
    let mut dirs = std::collections::BTreeSet::new();
    for r in &recs {
        // 試行の初期木は repo + materials/（workspace-seed）。
        assert_eq!(r["workspace_sha256_initial"], snap["workspace_seed_sha256"]);
        assert_eq!(r["kb_sha256_initial"], snap["kb_sha256"]);
        assert_ne!(
            r["workspace_sha256_final"], snap["repo_sha256"],
            "mock edits its own copy"
        );
        assert!(dirs.insert(r["workspace"].as_str().expect("w").to_string()));
        assert!(
            out.join(r["workspace"].as_str().expect("w"))
                .join("mock-result.txt")
                .exists()
        );
    }
    // 保存済み snapshot は試行の書き込みで変わっていない。
    assert!(!out.join("snapshot/repo/mock-result.txt").exists());
    assert!(!out.join("snapshot/kb/mock-note.md").exists());
    assert_eq!(snap["campaign_sha256"], Value::Null);
    let meta: Value =
        serde_json::from_slice(&fs::read(out.join(META_FILE)).expect("r")).expect("j");
    assert_eq!(meta["campaign_sha256"], e.m.compute_sha256().expect("h"));
    assert_eq!(meta["executor"], "mock");
    assert!(
        meta["budget"]["max_tokens"].is_u64() && meta["termination"]["trial_timeout_secs"].is_u64()
    );
    assert_eq!(meta["configs"].as_array().expect("c").len(), 3);
}

#[test]
fn budget_stop_keeps_unrun_trials_in_denominator() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(4, "dev");
    // p0 の A が 600 token 使うと予算 500 を超える。
    let m = campaign(&i, Split::Dev, 4, 500, 1);
    let mut s = BTreeMap::new();
    s.insert((ConfigId::A, "p0".to_string()), MockStep::Spend(600));
    let opts = seeded_src(d.path());
    let rep = run(&m, &i, &MockExecutor::scripted(s), &opts).expect("run");
    assert!(rep.budget_stopped);
    assert_eq!(rep.executed, 1);
    assert_eq!(rep.budget_cancelled, 9);
    let c = check(&m, &opts.out);
    assert!(c.ok, "{:?}", c.violations);
    let rc = c.recomputed.expect("rc");
    assert_eq!(rc.totals[&ConfigId::B].cancelled, 4);
    assert_eq!(rc.totals[&ConfigId::B].n, 4);
    assert_eq!(rc.totals[&ConfigId::A].cancelled, 3);
}

#[test]
fn resume_after_crash_does_not_rerun_and_drops_partial_line() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(4, "dev");
    let m = campaign(&i, Split::Dev, 4, 10_000_000, 1);
    let opts = seeded_src(d.path());
    run(&m, &i, &script(), &opts).expect("run");
    let full = fs::read_to_string(opts.out.join(TRIALS_FILE)).expect("r");
    let keep: Vec<&str> = full.lines().take(5).collect();
    let crashed = format!(
        "{}\n{}",
        keep.join("\n"),
        &full.lines().nth(5).expect("l")[..20]
    );
    fs::write(opts.out.join(TRIALS_FILE), crashed).expect("w");
    let rep = run(&m, &i, &script(), &opts).expect("resume");
    assert_eq!(rep.resumed_skipped, 5);
    assert_eq!(rep.executed, 5);
    let c = check(&m, &opts.out);
    assert!(c.ok, "{:?}", c.violations);
    assert_eq!(
        fs::read_to_string(opts.out.join(TRIALS_FILE)).expect("r"),
        full
    );
}

#[test]
fn resume_with_another_campaign_is_refused() {
    let e = ran(10_000_000, 1);
    let i = input(4, "dev");
    let mut other = campaign(&i, Split::Dev, 4, 10_000_000, 1);
    other.version = "v2".into();
    let err = run(&other, &i, &script(), &e.opts).unwrap_err();
    assert!(err.contains("different campaign"), "{err}");
}

fn tampered(mutate: impl FnOnce(&mut Vec<Value>)) -> Vec<String> {
    let e = ran(10_000_000, 1);
    let mut v = lines(&e.opts.out);
    mutate(&mut v);
    write_lines(&e.opts.out, &v);
    let rep = check(&e.m, &e.opts.out);
    assert!(!rep.ok, "tampering must be detected");
    assert!(
        export(&e.m, &e.opts.out, &e.opts.out.join("ex")).is_err(),
        "export must refuse"
    );
    rep.violations
}

#[test]
fn detects_flipped_failure() {
    let v = tampered(|v| {
        let r = v
            .iter_mut()
            .find(|r| r["outcome"] == "failed")
            .expect("failed");
        r["outcome"] = json!("passed");
        r["passed"] = json!(true);
    });
    assert!(v.iter().any(|s| s.contains("summary.json")), "{v:?}");
}

#[test]
fn detects_outcome_flag_mismatch() {
    let v = tampered(|v| v[0]["passed"] = json!(false));
    assert!(v.iter().any(|s| s.contains("passed flag")), "{v:?}");
}

#[test]
fn detects_dropped_failure_record() {
    let v = tampered(|v| {
        let i = v
            .iter()
            .position(|r| r["outcome"] == "timeout")
            .expect("timeout");
        v.remove(i);
    });
    assert!(v.iter().any(|s| s.contains("records")), "{v:?}");
}

#[test]
fn detects_split_mixing_and_foreign_hash() {
    let v = tampered(|v| v[3]["split"] = json!("final"));
    assert!(v.iter().any(|s| s.contains("mixed")), "{v:?}");
    let v = tampered(|v| v[3]["campaign_sha256"] = json!("deadbeef"));
    assert!(v.iter().any(|s| s.contains("campaign hash")), "{v:?}");
    let v = tampered(|v| v[3]["input_sha256"] = json!("deadbeef"));
    assert!(v.iter().any(|s| s.contains("input manifest hash")), "{v:?}");
}

#[test]
fn detects_shared_workspace_and_edited_snapshot_hash() {
    let v = tampered(|v| {
        let w = v[0]["workspace"].clone();
        v[1]["workspace"] = w;
    });
    assert!(v.iter().any(|s| s.contains("shared")), "{v:?}");
    let v = tampered(|v| v[2]["workspace_sha256_initial"] = json!("0"));
    assert!(v.iter().any(|s| s.contains("snapshot")), "{v:?}");
}

#[test]
fn detects_duplicate_and_unexpected_trials() {
    let v = tampered(|v| {
        let d = v[0].clone();
        v.push(d);
    });
    assert!(v.iter().any(|s| s.contains("duplicate")), "{v:?}");
    let v = tampered(|v| {
        let i = v.iter().position(|r| r["config"] == "C").expect("c");
        v[i]["problem_id"] = json!("p3");
    });
    assert!(v.iter().any(|s| s.contains("not an expected")), "{v:?}");
}

#[test]
fn detects_fake_timeout_and_overlong_wall() {
    let v = tampered(|v| {
        let r = v.iter_mut().find(|r| r["outcome"] == "timeout").expect("t");
        r["wall_secs"] = json!(5);
    });
    assert!(v.iter().any(|s| s.contains("timeout recorded")), "{v:?}");
    let v = tampered(|v| v[0]["wall_secs"] = json!(99_999));
    assert!(
        v.iter().any(|s| s.contains("exceeds trial timeout")),
        "{v:?}"
    );
}

#[test]
fn detects_manifest_swap_and_version_drift() {
    let e = ran(10_000_000, 1);
    let mut other = e.m.clone();
    other.version = "v-other".into();
    assert!(!check(&other, &e.opts.out).ok);
    let mut v = lines(&e.opts.out);
    v[0]["model_version"] = json!("9");
    write_lines(&e.opts.out, &v);
    let rep = check(&e.m, &e.opts.out);
    assert!(rep.violations.iter().any(|s| s.contains("version differs")));
}

#[test]
fn run_refuses_split_mix_and_unknown_executor_path_has_no_network_code() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(4, "dev");
    let m = campaign(&i, Split::Final, 4, 1_000, 1);
    let err = run(&m, &i, &script(), &seeded_src(d.path())).unwrap_err();
    assert!(err.contains("must not be mixed"), "{err}");
    // 実 LLM・network・子 process を使う語が src に無い。
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let mut dagbench_files = Vec::new();
    for f in fs::read_dir(&src).expect("src") {
        let p = f.expect("e").path();
        if p.is_dir() {
            // 走査対象外にしてよいディレクトリは dagbench だけ（未知のディレクトリは失敗させる）。
            // dagbench は測定専用の所有 stub 子 process を起こす module で、実 LLM・外部 network は呼ばない
            // （network の語は下で同じく禁止し、子 process だけを許す）。
            assert_eq!(
                p.file_name().and_then(|n| n.to_str()),
                Some("dagbench"),
                "unknown directory under src: {}",
                p.display()
            );
            for g in fs::read_dir(&p).expect("dagbench") {
                dagbench_files.push(g.expect("e").path());
            }
        } else {
            files.push(p);
        }
    }
    files.extend(dagbench_files.iter().cloned());
    for p in files {
        let mut t = fs::read_to_string(&p).expect("r");
        // dagbench の ipc は所有 process 間の Unix domain socket で、閉じるのに std::net::Shutdown（enum）だけを使う。
        // この語だけを除き、socket を作る TcpListener・UdpSocket・ToSocketAddrs は下で禁止する。
        if dagbench_files.contains(&p) {
            t = t.replace("std::net::Shutdown", "");
        }
        // 子 process は A arm の harness_exec.rs、grade の評価 command の grade_runner.rs、
        // asset 組立の git/tar の asset_assembly.rs、評価側の read-only `ssh qstat -x -f` の ssh_qstat.rs だけが起動する
        // （command は注入・固定、実 LLM は呼ばない）。
        let spawns = dagbench_files.contains(&p)
            || p.file_name().is_some_and(|n| {
                n == "harness_exec.rs"
                    || n == "grade_runner.rs"
                    || n == "asset_assembly.rs"
                    || n == "ssh_qstat.rs"
            });
        for bad in [
            "reqwest",
            "std::net",
            "TcpStream",
            "TcpListener",
            "UdpSocket",
            "ToSocketAddrs",
            if spawns {
                "reqwest"
            } else {
                "process::Command"
            },
            "tokio::net",
            "api.anthropic",
            "api.openai",
        ] {
            assert!(!t.contains(bad), "{} uses {bad}", p.display());
        }
    }
}

#[test]
fn cli_run_check_export_end_to_end() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(4, "dev");
    let m = campaign(&i, Split::Dev, 4, 10_000_000, 1);
    fs::write(
        d.path().join("input.json"),
        serde_json::to_vec(&i).expect("s"),
    )
    .expect("w");
    fs::write(
        d.path().join("campaign.json"),
        serde_json::to_vec(&m).expect("s"),
    )
    .expect("w");
    let bin = std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path");
    let go = |args: &[&str]| {
        std::process::Command::new(&bin)
            .args(args)
            .args(["--manifest"])
            .arg(d.path().join("campaign.json"))
            .args(["--out"])
            .arg(d.path().join("o"))
            .output()
            .expect("spawn")
    };
    let r = go(&["run", "--executor", "mock"]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(go(&["check"]).status.success());
    let e = go(&["export"]);
    assert!(e.status.success());
    assert!(d.path().join("o/export/table.incomplete.csv").exists());
    assert!(!go(&["run", "--executor", "claude"]).status.success());
    let mut v = lines(&d.path().join("o"));
    v[0]["passed"] = json!(!v[0]["passed"].as_bool().expect("b"));
    write_lines(&d.path().join("o"), &v);
    assert!(!go(&["check"]).status.success());
}
