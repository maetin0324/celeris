//! preflight: 何が欠けても有料実行・task 作成・台帳作成が 0 件で止まる。全部揃えば通る。キー値は出力に出ない。
//! 外部には出ない（偽 Host・偽 transport・実 process は preflight サブコマンドの起動だけ）。
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use celeris_campaign::{
    celeris_exec::{CelerisExecutor, HttpResponse, Transport},
    executor::MockExecutor,
    manifest::{CampaignManifest, ConfigId},
    preflight::{Host, Inputs, check, run_gated},
    runner::RunOptions,
};
use serde_json::{Value, json};

const SECRET: &str = "s3cret-token-value-xyz";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

struct FakeHost {
    env: Vec<&'static str>,
    programs: Vec<&'static str>,
}

impl Host for FakeHost {
    fn env_nonempty(&self, name: &str) -> bool {
        self.env.contains(&name)
    }

    fn program_available(&self, p: &Path) -> bool {
        self.programs.iter().any(|x| Path::new(x) == p)
    }
}

fn host() -> FakeHost {
    FakeHost {
        env: vec!["CELERIS_CAMPAIGN_TOKEN"],
        programs: vec!["claude"],
    }
}

struct CountingTransport(AtomicUsize);

impl Transport for &CountingTransport {
    fn request(&self, _: &str, _: &str, _: Option<&Value>) -> Result<HttpResponse, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("must not be called".into())
    }
}

/// 解決済みの final 一式（manifest・入力・材料・template）を一時 dir に作る。
struct Setup {
    dir: tempfile::TempDir,
    inp: Inputs,
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let t = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy(&p, &t);
        } else {
            fs::copy(&p, &t).unwrap();
        }
    }
}

fn resolved_manifest() -> Value {
    let p = root()
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final/final.json");
    let mut v: Value = serde_json::from_slice(&fs::read(p).unwrap()).unwrap();
    for c in v["configs"].as_array_mut().unwrap() {
        match c["id"].as_str().unwrap() {
            "A" => c["harness_version"] = json!("claude-code@2.1.0"),
            _ => c["harness_version"] = json!("0123456789ab"),
        }
        if c["routing"]["evidence_table_id"].is_string() {
            c["routing"]["evidence_table_id"] = json!("evidence-v7");
        }
    }
    v
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let pd = root()
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final");
    fs::copy(
        pd.join("final-input-manifest.json"),
        d.join("final-input-manifest.json"),
    )
    .unwrap();
    fs::write(d.join("final.json"), resolved_manifest().to_string()).unwrap();
    let (m, k) = (d.join("materials"), d.join("kb"));
    for c in ["k2-bf-xattr-restart-campaign", "k4-cmp4-validity-analysis"] {
        copy(
            &root().join("cases/final").join(c).join("materials"),
            &m.join(c),
        );
    }
    copy(
        &root().join("cases/final/k2-bf-xattr-restart-campaign/kb"),
        &k,
    );
    fs::write(
        d.join("task-template.json"),
        json!({"project_id":"01ABC","acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
    )
    .unwrap();
    let inp = Inputs {
        manifest_path: d.join("final.json"),
        input_override: None,
        cases_root: root().join("cases"),
        grader_root: root().join("fixtures/grader-private"),
        grade_cases_root: root().join("fixtures/grade-cases"),
        a_program: "claude".into(),
        api_url: Some("http://127.0.0.1:8080".into()),
        token_env: Some("CELERIS_CAMPAIGN_TOKEN".into()),
        task_template: Some(d.join("task-template.json")),
        repo_src: None,
        materials_src: Some(m),
        kb_src: Some(k),
    };
    Setup { dir, inp }
}

fn missing_checks(inp: &Inputs, h: &FakeHost) -> Vec<String> {
    let r = check(inp, h);
    assert!(!r.ok);
    r.missing.iter().map(|m| m.check.to_string()).collect()
}

#[test]
fn complete_setup_passes() {
    let s = setup();
    let r = check(&s.inp, &host());
    assert!(r.ok, "{:?}", r.missing);
    assert_eq!(r.paid_calls_made, 0);
    assert!(r.campaign_sha256.is_some());
}

/// 欠けた項目ごとに、(preflight が止める check 名) を確かめる。
#[test]
fn every_kind_of_gap_is_reported() {
    // 版: 未解決 placeholder / alias
    let s = setup();
    let mut v = resolved_manifest();
    v["configs"][0]["harness_version"] = json!("claude-code@<version at H1>");
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
    assert!(missing_checks(&s.inp, &host()).contains(&"versions".into()));
    let mut v = resolved_manifest();
    v["configs"][0]["harness_version"] = json!("claude-code@installed");
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
    assert!(missing_checks(&s.inp, &host()).contains(&"versions".into()));
    // C の routing 欠落
    let mut v = resolved_manifest();
    v["configs"][2].as_object_mut().unwrap().remove("routing");
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
    assert!(missing_checks(&s.inp, &host()).contains(&"versions".into()));

    // 予算: 上限超過・並列 2・token_basis
    for (path, val) in [
        ("max_tokens", json!(3_000_001)),
        ("max_parallel", json!(2)),
        ("token_basis", json!("input_output")),
    ] {
        let s = setup();
        let mut v = resolved_manifest();
        v["budget"][path] = val;
        fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
        assert!(
            missing_checks(&s.inp, &host()).contains(&"budget".into()),
            "{path}"
        );
    }
    // 時間不整合: trial timeout が per_case を超える
    let s = setup();
    let mut v = resolved_manifest();
    v["termination"]["trial_timeout_secs"] = json!(7200);
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
    assert!(missing_checks(&s.inp, &host()).contains(&"budget".into()));

    // 認証/環境: token 環境変数が無い・A の実行ファイルが無い・URL に path
    let s = setup();
    let h = FakeHost {
        env: vec![],
        programs: vec![],
    };
    let m = missing_checks(&s.inp, &h);
    assert_eq!(m.iter().filter(|c| *c == "auth-env").count(), 2, "{m:?}");
    let mut inp = s.inp.clone();
    inp.api_url = Some("http://127.0.0.1:8080/api/v1".into());
    assert!(missing_checks(&inp, &host()).contains(&"auth-env".into()));
    let mut inp = s.inp.clone();
    inp.api_url = None;
    assert!(missing_checks(&inp, &host()).contains(&"auth-env".into()));
    // task template の placeholder
    fs::write(
        s.dir.path().join("task-template.json"),
        json!({"project_id":"<案件 id>","acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
    )
    .unwrap();
    assert!(missing_checks(&s.inp, &host()).contains(&"versions".into()));

    // case 入力: 材料の hash 不一致・材料 dir 無し・未知 case
    let s = setup();
    fs::write(s.inp.materials_src.as_ref().unwrap().join("extra"), "x").unwrap();
    assert!(missing_checks(&s.inp, &host()).contains(&"case-inputs".into()));
    let s = setup();
    let mut inp = s.inp.clone();
    inp.materials_src = None;
    assert!(missing_checks(&inp, &host()).contains(&"case-inputs".into()));
}

fn grade_copy(s: &Setup) -> PathBuf {
    let g = s.dir.path().join("grade-cases");
    copy(&root().join("fixtures/grade-cases"), &g);
    g
}

#[test]
fn grader_gaps_are_reported() {
    // grade-case 本体が無い
    let s = setup();
    let g = grade_copy(&s);
    fs::remove_file(g.join("final/k4-cmp4-validity-analysis/k4-cmp4-validity-analysis.json"))
        .unwrap();
    let mut inp = s.inp.clone();
    inp.grade_cases_root = g;
    assert!(missing_checks(&inp, &host()).contains(&"graders".into()));
    // grade-case の改変（hash が固定とずれる）
    let s = setup();
    let g = grade_copy(&s);
    let p = g.join("final/k2-bf-xattr-restart-campaign/k2-bf-xattr-restart-campaign.json");
    fs::write(&p, fs::read_to_string(&p).unwrap() + "\n ").unwrap();
    let mut inp = s.inp.clone();
    inp.grade_cases_root = g;
    assert!(missing_checks(&inp, &host()).contains(&"graders".into()));
    // 隠しテストの grader 側の写しが無い
    let s = setup();
    let g = grade_copy(&s);
    fs::remove_dir_all(g.join("final/k2-bf-xattr-restart-campaign/hidden")).unwrap();
    let mut inp = s.inp.clone();
    inp.grade_cases_root = g;
    assert!(missing_checks(&inp, &host()).contains(&"graders".into()));
}

#[test]
fn one_gap_means_zero_ledger_and_transport_calls() {
    let s = setup();
    let mut v = resolved_manifest();
    v["configs"][1]["harness_version"] = json!("<release sha12 at H1>");
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
    let m = CampaignManifest::load(&s.inp.manifest_path).unwrap();
    let input = m.load_input(&s.inp.manifest_path, None).unwrap();
    let out = s.dir.path().join("out");
    let t = CountingTransport(AtomicUsize::new(0));
    let exec = CelerisExecutor::new(&t, serde_json::Map::new(), 1800, out.join("cancel.jsonl"));
    let e = run_gated(
        &s.inp,
        &host(),
        &m,
        &input,
        &exec,
        &RunOptions {
            out: out.clone(),
            ..Default::default()
        },
    )
    .expect_err("gate");
    assert!(e.contains("preflight failed"), "{e}");
    assert_eq!(t.0.load(Ordering::SeqCst), 0, "transport 呼び出し 0 回");
    assert!(!out.exists(), "ledger・out を作らない");
    assert!(m.configs.iter().any(|c| c.id == ConfigId::A));
}

#[test]
fn passing_preflight_lets_run_start() {
    let s = setup();
    let m = CampaignManifest::load(&s.inp.manifest_path).unwrap();
    let input = m.load_input(&s.inp.manifest_path, None).unwrap();
    let out = s.dir.path().join("out");
    let rep = run_gated(
        &s.inp,
        &host(),
        &m,
        &input,
        &MockExecutor::seeded(),
        &RunOptions {
            out: out.clone(),
            materials_src: s.inp.materials_src.clone(),
            kb_src: s.inp.kb_src.clone(),
            ..Default::default()
        },
    )
    .expect("run");
    assert!(rep.executed > 0);
}

fn bin(args: &[&str], s: &Setup, env: &[(&str, &str)]) -> std::process::Output {
    let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
    c.args(args)
        .arg("--manifest")
        .arg(&s.inp.manifest_path)
        .arg("--cases-root")
        .arg(&s.inp.cases_root)
        .arg("--grader-root")
        .arg(&s.inp.grader_root)
        .arg("--grade-case-root")
        .arg(&s.inp.grade_cases_root)
        .arg("--a-program")
        .arg("/bin/sh")
        .arg("--materials-src")
        .arg(s.inp.materials_src.as_ref().unwrap())
        .arg("--kb-src")
        .arg(s.inp.kb_src.as_ref().unwrap())
        .arg("--api-url")
        .arg("http://127.0.0.1:1")
        .arg("--token-env")
        .arg("CELERIS_PREFLIGHT_TEST_TOKEN")
        .arg("--task-template")
        .arg(s.dir.path().join("task-template.json"));
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("run")
}

#[test]
fn cli_preflight_exit_codes_and_secret_never_printed() {
    let s = setup();
    let ok = bin(
        &["preflight"],
        &s,
        &[("CELERIS_PREFLIGHT_TEST_TOKEN", SECRET)],
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let v: Value = serde_json::from_slice(&ok.stdout).unwrap();
    assert_eq!(
        (v["ok"].clone(), v["paid_calls_made"].clone()),
        (json!(true), json!(0))
    );
    // 環境変数が無い → 非 0。run も始まらない（out を作らない）。
    let out = s.dir.path().join("o");
    let o = bin(&["preflight"], &s, &[]);
    assert_eq!(o.status.code(), Some(1));
    let r = bin(
        &[
            "run",
            "--executor",
            "celeris",
            "--out",
            out.to_str().unwrap(),
        ],
        &s,
        &[],
    );
    assert_eq!(r.status.code(), Some(1));
    assert!(!out.exists());
    for o in [&ok, &o, &r] {
        for b in [&o.stdout, &o.stderr] {
            assert!(!String::from_utf8_lossy(b).contains(SECRET));
        }
    }
}
