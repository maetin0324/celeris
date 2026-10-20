//! B1/C1（同一測定系列）の累計予算: 測定 ID ごとに固定した 1 か所の系列台帳へ消費を追記・そこから合算する。
//! manifest hash・出力先（許可 root の中）を変えても既消費は合算され、超過・許可 root 外の出力・台帳の欠損/破損は
//! 副作用の前に拒否する。主比較（side でない manifest）の計上は不変。
use std::{collections::BTreeMap, path::Path};

use celeris_campaign::{
    SIDE_CAMPAIGN_SCHEMA,
    executor::MockAsyncExecutor,
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary,
        SnapshotRef, Split, Termination, TokenBasis, manifest_hash,
    },
    runner::{RunOptions, run, run_with_series},
    series::{self, SeriesPin},
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
        variant: None,
        schema: SIDE_CAMPAIGN_SCHEMA.into(),
        campaign_id: "jev-b1c1-series".into(),
        version: version.into(),
        split: Split::Dev,
        fault_scenarios: Default::default(),
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
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

const SERIES: &str = "jev-b1c1-series";
const MID: &str = "jev-b1c1-m1";

fn pin(root: &Path) -> SeriesPin {
    SeriesPin::new(root, MID).expect("pin")
}

fn init(root: &Path, i: &Input) -> SeriesPin {
    let p = pin(root);
    series::init(&p, SERIES, &[], &side(i, "v1", 10_000_000).budget).expect("series-init");
    p
}

fn opts(out: &Path) -> RunOptions {
    RunOptions {
        out: out.to_path_buf(),
        ..RunOptions::default()
    }
}

/// 副比較の run（系列台帳の固定位置を明示する）。
fn go(
    m: &CampaignManifest,
    i: &Input,
    x: &MockAsyncExecutor,
    out: &Path,
    p: &SeriesPin,
) -> Result<celeris_campaign::runner::RunReport, String> {
    run_with_series(m, i, x, &opts(out), Some(p))
}

fn out_in(p: &SeriesPin, name: &str) -> std::path::PathBuf {
    Path::new(&p.out_root).join(name)
}

struct Fx {
    d: tempfile::TempDir,
    i: Input,
    p: SeriesPin,
    /// v1 を最後まで実行して確定した tokens。
    tokens: u64,
}

/// v1（out-v1）を最後まで実行して消費を作る。
fn consumed_v1() -> Fx {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let p = init(d.path(), &i);
    let m1 = side(&i, "v1", 10_000_000);
    let x = MockAsyncExecutor::new(0);
    for _ in 0..4 {
        go(&m1, &i, &x, &out_in(&p, "out-v1"), &p).expect("v1");
    }
    let text =
        std::fs::read_to_string(Path::new(&p.dir).join(series::LEDGER_FILE)).expect("ledger");
    let last: serde_json::Value =
        serde_json::from_str(text.lines().last().expect("line")).expect("json");
    let tokens = last["tokens"].as_u64().expect("tokens");
    assert!(tokens > 0, "v1 consumption is in the pinned series ledger");
    Fx { d, i, p, tokens }
}

#[test]
fn hash_change_still_counts_prior_spend_of_the_same_series() {
    let f = consumed_v1();
    // 承認総枠 = 既消費ちょうど: 新 hash で何も使えないので、out も作らず拒否する。
    let m2 = side(&f.i, "v2", f.tokens);
    assert_ne!(
        m2.compute_sha256().unwrap(),
        side(&f.i, "v1", 1).compute_sha256().unwrap()
    );
    let out2 = out_in(&f.p, "out-v2");
    let x = MockAsyncExecutor::new(0);
    let err = go(&m2, &f.i, &x, &out2, &f.p).expect_err("over the approved total");
    assert!(err.contains("approved total"), "{err}");
    assert_eq!(x.submit_calls(), 0);
    assert!(!out2.exists(), "refused before creating the out dir");
}

#[test]
fn changing_the_out_inside_the_allowed_root_still_sums_even_with_the_same_hash() {
    let f = consumed_v1();
    // 同じ manifest（同じ hash）を別の out へ: 旧 out の消費は台帳から合算される（兄弟走査に頼らない）。
    let m1 = side(&f.i, "v1", f.tokens);
    let x = MockAsyncExecutor::new(0);
    let other = out_in(&f.p, "nested").join("out-again");
    let err = go(&m1, &f.i, &x, &other, &f.p).expect_err("over the approved total");
    assert!(err.contains("approved total"), "{err}");
    assert_eq!(x.submit_calls(), 0);
}

#[test]
fn remaining_total_below_one_reservation_submits_nothing() {
    let f = consumed_v1();
    // 残り < 1 試行の予約: 既消費を合算しなければ（新 hash だけを見れば）投入されてしまう量。
    let m2 = side(&f.i, "v2", f.tokens + CASE / 2);
    let x = MockAsyncExecutor::new(0);
    let rep = go(&m2, &f.i, &x, &out_in(&f.p, "out-v2"), &f.p).expect("run");
    assert!(rep.budget_stopped);
    assert_eq!(x.submit_calls(), 0);
}

#[test]
fn resume_and_repeat_do_not_double_count() {
    let f = consumed_v1();
    let m1 = side(&f.i, "v1", 10_000_000);
    let x = MockAsyncExecutor::new(0);
    for _ in 0..3 {
        go(&m1, &f.i, &x, &out_in(&f.p, "out-v1"), &f.p).expect("resume");
    }
    // 総枠 = 既消費 + 1 試行分: v1 を数え直して 2 倍にしていれば、v2 は拒否される。
    let m2 = side(&f.i, "v2", f.tokens + CASE * 4);
    let rep = go(&m2, &f.i, &x, &out_in(&f.p, "out-v2"), &f.p).expect("v2");
    assert!(
        rep.executed > 0,
        "prior counted once, so a reservation still fits"
    );
}

#[test]
fn out_outside_the_allowed_root_is_refused_before_any_effect() {
    let f = consumed_v1();
    let m2 = side(&f.i, "v2", 10_000_000);
    let x = MockAsyncExecutor::new(0);
    let outside = f.d.path().join("elsewhere").join("out-v2");
    let escape = out_in(&f.p, "..").join("escaped");
    let link_dir = f.d.path().join("target-of-link");
    std::fs::create_dir_all(&link_dir).expect("dir");
    let link = out_in(&f.p, "link");
    std::os::unix::fs::symlink(&link_dir, &link).expect("symlink");
    let via_link = link.join("out-v2");
    let root_itself = std::path::PathBuf::from(&f.p.out_root);
    for out in [
        outside.clone(),
        escape.clone(),
        via_link.clone(),
        root_itself,
    ] {
        let err = go(&m2, &f.i, &x, &out, &f.p).expect_err("outside the allowed root");
        assert!(
            err.contains("outside the allowed output root") || err.contains("'..'"),
            "{out:?}: {err}"
        );
    }
    assert_eq!(x.submit_calls(), 0);
    assert!(!outside.exists() && !escape.exists());
    assert!(
        !link_dir.join("out-v2").exists(),
        "nothing written through the symlink"
    );
}

#[test]
fn missing_or_broken_series_ledger_is_refused_not_zero() {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let m = side(&i, "v1", 10_000_000);
    let x = MockAsyncExecutor::new(0);
    // 未設定。
    let e = run(
        &m,
        &i,
        &x,
        &RunOptions {
            out: d.path().join("o"),
            ..RunOptions::default()
        },
    )
    .expect_err("no series pin");
    assert!(e.contains("pinned series ledger"), "{e}");
    // 台帳が無い（series-init していない）。
    let p = pin(d.path());
    std::fs::create_dir_all(&p.out_root).expect("dir");
    let e = go(&m, &i, &x, &out_in(&p, "o"), &p).expect_err("no ledger");
    assert!(e.contains("missing"), "{e}");
    // 初期化後に壊す: 行の破損・台帳 file の欠損・series.json の別測定 ID。
    series::init(&p, SERIES, &[], &m.budget).expect("init");
    let ledger = Path::new(&p.dir).join(series::LEDGER_FILE);
    std::fs::write(&ledger, "not json\n").expect("corrupt");
    assert!(go(&m, &i, &x, &out_in(&p, "o"), &p).is_err());
    std::fs::write(&ledger, "").expect("reset");
    std::fs::remove_file(&ledger).expect("rm");
    assert!(go(&m, &i, &x, &out_in(&p, "o"), &p).is_err());
    std::fs::write(&ledger, "").expect("restore");
    let other = SeriesPin::new(d.path(), "another-id").expect("pin");
    let meta = std::fs::read_to_string(Path::new(&p.dir).join(series::SERIES_FILE)).expect("meta");
    std::fs::create_dir_all(&other.out_root).expect("dir");
    std::fs::write(Path::new(&other.dir).join(series::SERIES_FILE), meta).expect("copy");
    assert!(go(&m, &i, &x, &out_in(&other, "o"), &other).is_err());
    assert_eq!(x.submit_calls(), 0);
    assert!(!out_in(&p, "o").exists());
}

#[test]
fn a_second_concurrent_run_of_the_series_is_refused() {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let p = init(d.path(), &i);
    let held = series::open(&p, SERIES).expect("first holder");
    let x = MockAsyncExecutor::new(0);
    let e = go(&side(&i, "v1", 10_000_000), &i, &x, &out_in(&p, "o"), &p).expect_err("busy");
    assert!(e.contains("another run"), "{e}");
    assert_eq!(x.submit_calls(), 0);
    drop(held);
    go(&side(&i, "v1", 10_000_000), &i, &x, &out_in(&p, "o"), &p).expect("free again");
}

#[test]
fn carry_over_from_an_existing_out_is_explicit() {
    let f = consumed_v1();
    // 既存の out（旧い配置の消費）を新しい固定台帳へ引き継ぐ: init の import で明示し、以後は合算される。
    let root2 = f.d.path().join("root2");
    let p2 = SeriesPin::new(&root2, MID).expect("pin");
    let m2 = side(&f.i, "v2", f.tokens);
    series::init(&p2, SERIES, &[out_in(&f.p, "out-v1")], &m2.budget).expect("import");
    let x = MockAsyncExecutor::new(0);
    let e = go(&m2, &f.i, &x, &out_in(&p2, "out-v2"), &p2).expect_err("carried over");
    assert!(e.contains("approved total"), "{e}");
}

#[test]
fn primary_manifest_ignores_the_series() {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let mut p = side(&i, "v2", 10_000_000);
    p.schema = celeris_campaign::CAMPAIGN_SCHEMA.into();
    let x = MockAsyncExecutor::new(0);
    // 主比較は系列台帳なしで、どこへ出してもよい（既存の計上は不変）。
    let rep = run(
        &p,
        &i,
        &x,
        &RunOptions {
            out: d.path().join("any/out"),
            ..RunOptions::default()
        },
    );
    assert!(rep.is_ok() || !rep.expect_err("err").contains("series"));
}

/// 通常 CLI（`celeris-campaign run`、launcher が呼ぶのと同じ引数）で、許可 root 内の別 out への合算、
/// root 外・未設定・台帳欠損の副作用前拒否を確かめる。
mod cli {
    use super::*;
    use std::process::{Command, Output};

    struct W {
        d: tempfile::TempDir,
    }

    impl W {
        fn manifest(&self, version: &str, max_tokens: u64) -> std::path::PathBuf {
            let i = input();
            let m = side(&i, version, max_tokens);
            let p = self.d.path().join(format!("manifest-{version}.json"));
            std::fs::write(
                self.d.path().join("input.json"),
                serde_json::to_vec(&i).unwrap(),
            )
            .unwrap();
            std::fs::write(&p, serde_json::to_vec(&m).unwrap()).unwrap();
            p
        }

        fn root(&self) -> std::path::PathBuf {
            self.d.path().join("series")
        }

        fn out_root(&self) -> std::path::PathBuf {
            self.root().join(MID).join(series::RUNS_DIR)
        }

        fn cmd(&self, sub: &str, manifest: &Path, extra: &[&str]) -> Output {
            Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"))
                .args([sub, "--manifest"])
                .arg(manifest)
                .args(extra)
                .output()
                .expect("run")
        }

        fn run(&self, manifest: &Path, out: &Path, series_root: Option<&Path>) -> Output {
            let mut a: Vec<String> = ["--executor", "mock", "--measurement-id", MID, "--out"]
                .map(String::from)
                .to_vec();
            a.push(out.to_string_lossy().into_owned());
            if let Some(r) = series_root {
                a.push("--series-root".into());
                a.push(r.to_string_lossy().into_owned());
            }
            let a: Vec<&str> = a.iter().map(String::as_str).collect();
            self.cmd("run", manifest, &a)
        }

        fn init(&self, manifest: &Path) -> Output {
            self.cmd(
                "series-init",
                manifest,
                &[
                    "--measurement-id",
                    MID,
                    "--series-root",
                    self.root().to_str().unwrap(),
                ],
            )
        }
    }

    fn stderr(o: &Output) -> String {
        String::from_utf8_lossy(&o.stderr).into_owned()
    }

    #[test]
    fn cli_sums_across_outs_and_hashes_and_refuses_outside_before_any_effect() {
        let w = W {
            d: tempfile::tempdir().unwrap(),
        };
        let m1 = w.manifest("v1", 10_000_000);
        let root = w.root();
        // 未設定・台帳なしは、out を作らず拒否する。
        let early = w.out_root().join("early");
        let o = w.run(&m1, &early, None);
        assert!(
            !o.status.success() && stderr(&o).contains("--series-root"),
            "{}",
            stderr(&o)
        );
        let o = w.run(&m1, &early, Some(&root));
        assert!(
            !o.status.success() && stderr(&o).contains("series-init"),
            "{}",
            stderr(&o)
        );
        assert!(!early.exists());
        let o = w.init(&m1);
        assert!(o.status.success(), "{}", stderr(&o));
        let o = w.run(&m1, &w.out_root().join("out-v1"), Some(&root));
        assert!(o.status.success(), "{}", stderr(&o));
        let ledger = std::fs::read_to_string(root.join(MID).join(series::LEDGER_FILE)).unwrap();
        let last: serde_json::Value = serde_json::from_str(ledger.lines().last().unwrap()).unwrap();
        let used = last["tokens"].as_u64().unwrap();
        assert!(used > 0);
        // hash を変え、出力先も変える（許可 root の中）: 既消費を合算するので総枠 = 既消費で拒否される。
        let m2 = w.manifest("v2", used);
        let out2 = w.out_root().join("deeper/out-v2");
        let o = w.run(&m2, &out2, Some(&root));
        assert!(
            !o.status.success() && stderr(&o).contains("approved total"),
            "{}",
            stderr(&o)
        );
        assert!(!out2.exists());
        // 許可 root の外（別親・`..`・symlink）は、投入前に拒否される。
        let elsewhere = w.d.path().join("elsewhere/out-v2");
        let escape = w.out_root().join("../escaped");
        let target = w.d.path().join("link-target");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, w.out_root().join("lnk")).unwrap();
        let m3 = w.manifest("v3", 10_000_000);
        for out in [
            elsewhere.clone(),
            escape.clone(),
            w.out_root().join("lnk/out"),
        ] {
            let o = w.run(&m3, &out, Some(&root));
            assert!(!o.status.success(), "{out:?}");
            let e = stderr(&o);
            assert!(
                e.contains("outside the allowed output root") || e.contains("'..'"),
                "{e}"
            );
        }
        assert!(!elsewhere.exists() && !w.d.path().join("escaped").exists());
        assert!(std::fs::read_dir(&target).unwrap().next().is_none());
        // 台帳の破損は既消費 0 ではなく拒否。
        std::fs::write(root.join(MID).join(series::LEDGER_FILE), "{broken\n").unwrap();
        let o = w.run(&m3, &w.out_root().join("out-v3"), Some(&root));
        assert!(
            !o.status.success() && stderr(&o).contains("series ledger"),
            "{}",
            stderr(&o)
        );
        assert!(!w.out_root().join("out-v3").exists());
    }
}
