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
use sha2::{Digest, Sha256};

const K4_INPUT: &str = "main-v3-n2-43706.tgz";
const SECRET: &str = "s3cret-token-value-xyz";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

struct FakeHost {
    env: Vec<&'static str>,
    programs: Vec<&'static str>,
    /// path → GET の本文（task-api の偽）。
    api: Vec<(&'static str, Value)>,
    sandbox: bool,
}

impl Host for FakeHost {
    fn env_nonempty(&self, name: &str) -> bool {
        self.env.contains(&name)
    }

    fn program_available(&self, p: &Path) -> bool {
        self.programs.iter().any(|x| Path::new(x) == p)
    }

    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        self.api
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| format!("GET {path}: HTTP 404"))
    }

    fn sandbox_usable(&self) -> Result<(), String> {
        if self.sandbox {
            Ok(())
        } else {
            Err("bwrap unavailable".into())
        }
    }
}

/// 専用の campaign 案件（auto_integrate off・ローカル repo）と A のアカウントを返す偽 API。
fn api(acct_dir: &Path) -> Vec<(&'static str, Value)> {
    vec![
        (
            "/api/v1/projects/01ABC",
            json!({"id":"01ABC","auto_integrate":false}),
        ),
        (
            "/api/v1/projects/01ABC/repos",
            json!({"items":[{"name":"scratch","is_primary":true,"location":{"kind":"local","path":"/x"}}]}),
        ),
        (
            "/api/v1/llm/accounts",
            json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":acct_dir,"logged_in":true}]}),
        ),
    ]
}

fn host() -> FakeHost {
    FakeHost {
        env: vec!["CELERIS_CAMPAIGN_TOKEN"],
        programs: vec!["claude"],
        api: api(Path::new("/tmp")),
        sandbox: true,
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
    // 外部入力: 実物（本番の asset）は無いので、case を写して k4 の宣言 sha256 を試験用の中身に合わせる。
    let (cases, ins) = (d.join("cases"), d.join("inputs-src"));
    copy(&root().join("cases"), &cases);
    fs::create_dir_all(&ins).unwrap();
    let repo = d.join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    fs::write(ins.join(K4_INPUT), b"fake archive for the test").unwrap();
    let cj = cases.join("final/k4-cmp4-validity-analysis/case.json");
    let mut c: Value = serde_json::from_slice(&fs::read(&cj).unwrap()).unwrap();
    c["inputs"][0]["sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(b"fake archive for the test")
    ));
    fs::write(&cj, c.to_string()).unwrap();
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
        cases_root: cases,
        grader_root: root().join("fixtures/grader-private"),
        grade_cases_root: root().join("fixtures/grade-cases"),
        a_program: "claude".into(),
        api_url: Some("http://127.0.0.1:8080".into()),
        token_env: Some("CELERIS_CAMPAIGN_TOKEN".into()),
        task_template: Some(d.join("task-template.json")),
        repo_src: Some(repo),
        materials_src: Some(m),
        kb_src: Some(k),
        inputs_src: Some(ins),
        a_account: Some("lab".into()),
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
        api: vec![],
        sandbox: true,
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

/// task-api の偽（GET だけ。path ごとの JSON を返す）。thread は process 終了で消える。
fn fake_api(routes: Vec<(&'static str, Value)>) -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for st in l.incoming() {
            let Ok(mut st) = st else { continue };
            let mut buf = [0u8; 4096];
            let n = st.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let (code, body) = match routes.iter().find(|(p, _)| *p == path) {
                Some((_, v)) => (200, v.to_string()),
                None => (404, "{}".to_string()),
            };
            let _ = write!(
                st,
                "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
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
        .arg("--inputs-src")
        .arg(s.inp.inputs_src.as_ref().unwrap())
        .arg("--repo-src")
        .arg(s.inp.repo_src.as_ref().unwrap())
        .arg("--a-account")
        .arg("lab")
        .arg("--api-url")
        .arg(fake_api(api(&s.dir.path().join("inputs-src"))))
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
    // 実 bwrap の可否は host 依存（userns の無い sandbox では使えない）。使えるときだけ全件 OK を求め、
    // 使えないときは不足が隔離の 1 件だけであることを確かめる。
    let userns = Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--unshare-pid",
            "--",
            "true",
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    if userns {
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
    } else {
        let v: Value = serde_json::from_slice(&ok.stderr_json()).unwrap();
        let miss = v["missing"].as_array().unwrap();
        assert!(
            miss.len() == 1 && miss[0]["detail"].as_str().unwrap().contains("bwrap"),
            "{v}"
        );
    }
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

fn with_api(extra: Vec<(&'static str, Value)>) -> FakeHost {
    let mut h = host();
    for (p, v) in extra {
        h.api.retain(|(q, _)| *q != p);
        h.api.push((p, v));
    }
    h
}

/// 外部入力: 取り込み元が無い・ファイルが欠ける・sha256 が違う、のどれも `inputs` で止まる。
#[test]
fn missing_or_wrong_external_inputs_are_detected() {
    let s = setup();
    let mut inp = s.inp.clone();
    inp.inputs_src = None;
    let r = check(&inp, &host());
    assert!(
        r.missing
            .iter()
            .any(|m| m.check == "inputs" && m.detail.contains(K4_INPUT)),
        "{:?}",
        r.missing
    );
    let src = s.inp.inputs_src.clone().unwrap();
    fs::write(src.join(K4_INPUT), b"tampered").unwrap();
    let r = check(&s.inp, &host());
    assert!(
        r.missing
            .iter()
            .any(|m| m.check == "inputs" && m.detail.contains("sha256")),
        "{:?}",
        r.missing
    );
    fs::remove_file(src.join(K4_INPUT)).unwrap();
    assert!(missing_checks(&s.inp, &host()).contains(&"inputs".into()));
}

/// k2 は初期 repo（`initial`）を持つ: `--repo-src` が無ければ止める。
#[test]
fn initial_repo_needs_repo_src() {
    let s = setup();
    let mut inp = s.inp.clone();
    inp.repo_src = None;
    let r = check(&inp, &host());
    assert!(
        r.missing
            .iter()
            .any(|m| m.check == "inputs" && m.detail.contains("--repo-src")),
        "{:?}",
        r.missing
    );
}

/// pilot-2 の 422: local workspace とリモート repo の組合せ（案件の primary がリモート）を投入前に拒否する。
#[test]
fn template_that_would_422_is_rejected() {
    let s = setup();
    let remote = with_api(vec![(
        "/api/v1/projects/01ABC/repos",
        json!({"items":[{"name":"benchfs","is_primary":true,"location":{"kind":"remote","cluster":"pegasus","path":"/w"}}]}),
    )]);
    let r = check(&s.inp, &remote);
    assert!(!r.ok);
    assert!(
        r.missing
            .iter()
            .any(|m| m.check == "task-template" && m.detail.contains("remote repo")),
        "{:?}",
        r.missing
    );
    // template が repos で明示したリモート repo も同じ。
    let mixed = with_api(vec![(
        "/api/v1/projects/01ABC/repos",
        json!({"items":[
            {"name":"scratch","is_primary":true,"location":{"kind":"local","path":"/x"}},
            {"name":"benchfs","is_primary":false,"location":{"kind":"remote","cluster":"pegasus","path":"/w"}}]}),
    )]);
    fs::write(
        s.dir.path().join("task-template.json"),
        json!({"project_id":"01ABC","repos":["benchfs"],"acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
    )
    .unwrap();
    assert!(
        check(&s.inp, &mixed)
            .missing
            .iter()
            .any(|m| m.check == "task-template")
    );
    // 明示した local repo は通る。
    fs::write(
        s.dir.path().join("task-template.json"),
        json!({"project_id":"01ABC","repos":["scratch"],"acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
    )
    .unwrap();
    let r = check(&s.inp, &mixed);
    assert!(r.ok, "{:?}", r.missing);
}

/// 自動取り込みが有効な案件は、試行 task が自己改善の main に入り得るので拒否する。
#[test]
fn auto_integrate_project_is_rejected() {
    let s = setup();
    let h = with_api(vec![(
        "/api/v1/projects/01ABC",
        json!({"id":"01ABC","auto_integrate":true}),
    )]);
    let r = check(&s.inp, &h);
    assert!(
        r.missing
            .iter()
            .any(|m| m.check == "task-template" && m.detail.contains("auto_integrate")),
        "{:?}",
        r.missing
    );
}

/// A は Celeris 管理のアカウントで動く: 指定が無い・未ログイン・未知なら止める。bwrap が使えない host も止める。
#[test]
fn a_arm_needs_a_managed_account_and_a_working_sandbox() {
    let s = setup();
    let mut inp = s.inp.clone();
    inp.a_account = None;
    assert!(missing_checks(&inp, &host()).contains(&"a-arm".into()));
    let mut inp = s.inp.clone();
    inp.a_account = Some("nobody".into());
    assert!(missing_checks(&inp, &host()).contains(&"a-arm".into()));
    let logged_out = with_api(vec![(
        "/api/v1/llm/accounts",
        json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":"/tmp","logged_in":false}]}),
    )]);
    assert!(missing_checks(&s.inp, &logged_out).contains(&"a-arm".into()));
    let mut no_sandbox = host();
    no_sandbox.sandbox = false;
    assert!(missing_checks(&s.inp, &no_sandbox).contains(&"a-arm".into()));
}

trait StderrJson {
    fn stderr_json(&self) -> Vec<u8>;
}

impl StderrJson for std::process::Output {
    /// stderr の `error: ` 以降（preflight が不足一覧の JSON を書く）。
    fn stderr_json(&self) -> Vec<u8> {
        let t = String::from_utf8_lossy(&self.stderr).into_owned();
        t.strip_prefix("error: ").unwrap_or(&t).as_bytes().to_vec()
    }
}
