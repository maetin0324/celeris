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
    preflight::{Host, Inputs, ResolvedProgram, check, run_gated},
    runner::RunOptions,
};
use serde_json::{Value, json};

mod preflight_setup;
use preflight_setup::*;
const SECRET: &str = "s3cret-token-value-xyz";

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

    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        if self.programs.iter().any(|x| Path::new(x) == p) {
            Ok(ResolvedProgram {
                path: p.to_path_buf(),
                version: "fake 1.0".into(),
            })
        } else {
            Err("not available".into())
        }
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
            json!({"project":{"id":"01ABC","auto_integrate":false},"repos":[]}),
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

/// 空の測定専用アカウント dir（process 内で共有）。
fn account_dir() -> &'static Path {
    static D: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    D.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

#[test]
fn preflight_rejects_an_account_dir_with_past_transcripts_or_claude_md() {
    let t = tempfile::tempdir().unwrap();
    assert!(celeris_campaign::preflight::account_dir_residue(t.path()).is_empty());
    fs::create_dir_all(t.path().join("projects/-x")).unwrap();
    assert_eq!(
        celeris_campaign::preflight::account_dir_residue(t.path()).len(),
        1
    );
    fs::write(t.path().join("CLAUDE.md"), "memo").unwrap();
    assert_eq!(
        celeris_campaign::preflight::account_dir_residue(t.path()).len(),
        2
    );
    // preflight 全体でも measure-env で落ちる
    let s = setup();
    let h = FakeHost {
        api: api(t.path()),
        ..host()
    };
    let r = check(&s.inp, &h);
    assert!(
        r.missing
            .iter()
            .any(|m| m.check == "measure-env" && m.detail.contains("CLAUDE.md"))
    );
}

fn host() -> FakeHost {
    FakeHost {
        env: vec!["CELERIS_CAMPAIGN_TOKEN"],
        programs: vec!["claude"],
        api: api(account_dir()),
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

fn measure_misses(inp: &Inputs) -> Vec<String> {
    check(inp, &host())
        .missing
        .into_iter()
        .filter(|m| m.check == "measure-env")
        .map(|m| m.detail)
        .collect()
}

#[test]
fn preflight_accepts_the_isolated_measurement_environment() {
    let s = setup();
    assert!(
        measure_misses(&s.inp).is_empty(),
        "{:?}",
        measure_misses(&s.inp)
    );
}

#[test]
fn preflight_rejects_a_measurement_config_that_reaches_normal_operation() {
    let n = |s: &Setup| s.dir.path().join("normal");
    // 通常 KB root を共有 / 既定 root に落ちる / mount が memory・dir / 記憶が通常と同じ / DB 共有 / 作業場所共有
    type Edit = Box<dyn Fn(&Path, &str) -> String>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "overlaps",
            Box::new(|d, c| {
                c.replacen(
                    &format!("{}/measure/kb", d.display()),
                    &format!("{}/normal/kb", d.display()),
                    1,
                )
            }),
        ),
        (
            "explicitly",
            Box::new(|_, c| c.replacen("\nroot = ", "\n#root = ", 1)),
        ),
        (
            "kb:<scope>",
            Box::new(|_, c| c.replacen("[\"kb:trial\"]", "[\"kb:trial\", \"memory\"]", 1)),
        ),
        (
            "default_mounts explicitly",
            Box::new(|_, c| c.replacen("default_mounts =", "#default_mounts =", 1)),
        ),
        (
            "[memory] dir",
            Box::new(|d, c| format!("{c}[memory]\ndir = \"{}/normal/memory\"\n", d.display())),
        ),
        (
            "normal operation's db",
            Box::new(|d, c| {
                c.replacen(
                    &format!("{}/measure/celeris.db", d.display()),
                    &format!("{}/normal/celeris.db", d.display()),
                    1,
                )
            }),
        ),
        (
            "workspace_root",
            Box::new(|d, c| {
                c.replacen(
                    &format!("{}/measure/ws", d.display()),
                    &format!("{}/normal/ws", d.display()),
                    1,
                )
            }),
        ),
        (
            "langmem",
            Box::new(|_, c| format!("{c}[knowledge.langmem]\nenabled = true\n")),
        ),
    ];
    for (needle, edit) in cases {
        let s = setup();
        let d = s.dir.path();
        let bad = edit(d, &measure_config(d, "measure"));
        fs::write(d.join("measure.toml"), bad).unwrap();
        let m = measure_misses(&s.inp);
        assert!(m.iter().any(|x| x.contains(needle)), "{needle}: {m:?}");
        let _ = n(&s);
    }
}

#[test]
fn preflight_rejects_a_measurement_kb_root_with_unknown_content_and_accepts_the_fixed_kb() {
    let s = setup();
    let d = s.dir.path();
    let root = d.join("measure/kb");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("stray.md"), "left over").unwrap();
    assert!(
        measure_misses(&s.inp)
            .iter()
            .any(|x| x.contains("neither empty"))
    );
    // 固定初期 KB と同一内容なら通る。
    fs::remove_dir_all(&root).unwrap();
    copy(s.inp.kb_src.as_ref().unwrap(), &root);
    assert!(
        measure_misses(&s.inp).is_empty(),
        "{:?}",
        measure_misses(&s.inp)
    );
}

#[test]
fn preflight_keeps_cluster_and_api_connections() {
    let s = setup();
    let d = s.dir.path();
    let cfg = measure_config(d, "measure").replace("host = \"pegasus\"", "host = \"elsewhere\"");
    fs::write(d.join("measure.toml"), cfg).unwrap();
    assert!(
        measure_misses(&s.inp)
            .iter()
            .any(|x| x.contains("clusters"))
    );
    let cfg = measure_config(d, "measure").replace("token_file", "#token_file");
    fs::write(d.join("measure.toml"), cfg).unwrap();
    assert!(
        measure_misses(&s.inp)
            .iter()
            .any(|x| x.contains("token_file"))
    );
}

#[test]
fn preflight_rejects_canaries_in_the_trial_tree_and_a_trial_kb_without_its_canary() {
    // 通常 KB の目印が試行用 KB / materials に混ざる
    for sub in ["kb", "materials"] {
        let s = setup();
        let dir = if sub == "kb" {
            s.inp.kb_src.clone()
        } else {
            s.inp.materials_src.clone()
        }
        .unwrap();
        let f = dir.join("leak.txt");
        fs::write(&f, CANARY_NORMAL).unwrap();
        let m = measure_misses(&s.inp);
        if sub == "kb" {
            assert!(
                m.iter()
                    .any(|x| x.contains("forbidden canary") && x.contains("kb/leak.txt")),
                "{m:?}"
            );
        } else {
            // materials は hash pin に当たるので case-inputs でも落ちる。canary 側も報告される。
            assert!(m.iter().any(|x| x.contains("forbidden canary")), "{m:?}");
        }
    }
    // grader 私有の目印が repo に
    let s = setup();
    fs::write(
        s.inp.repo_src.as_ref().unwrap().join("g.txt"),
        CANARY_GRADER,
    )
    .unwrap();
    assert!(
        measure_misses(&s.inp)
            .iter()
            .any(|x| x.contains(CANARY_GRADER))
    );
    // 試行用 KB に自分の目印が無い（配られていない）
    let s = setup();
    fs::remove_file(s.inp.kb_src.as_ref().unwrap().join("CANARY.md")).unwrap();
    assert!(
        measure_misses(&s.inp)
            .iter()
            .any(|x| x.contains("required canary"))
    );
    // 再開: 既存 out の試行木に禁止 canary
    let s = setup();
    let out = s.dir.path().join("out/B/p/1/output");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("answer.md"), "see CANARY-OTHER-TRIAL-55aa").unwrap();
    assert!(
        measure_misses(&s.inp)
            .iter()
            .any(|x| x.contains("existing out dir"))
    );
}

#[test]
fn preflight_requires_the_measurement_environment_inputs() {
    let s = setup();
    let mut inp = s.inp.clone();
    inp.measure = None;
    assert!(missing_checks(&inp, &host()).contains(&"measure-env".into()));
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
            repo_src: s.inp.repo_src.clone(),
            inputs_src: s.inp.inputs_src.clone(),
            case_inputs: celeris_campaign::inputs::for_problems(
                &celeris_campaign::cases::CaseSet::load(&s.inp.cases_root, &s.inp.grader_root)
                    .unwrap(),
                &m.problems,
            ),
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
    // A の実行ファイルは `--version` に答える stub（/bin/sh は版を返さず、preflight が拒否する）。
    let stub = s.dir.path().join("a-stub");
    if !stub.exists() {
        let tmp = s.dir.path().join("a-stub.tmp");
        fs::write(&tmp, "#!/bin/sh\necho 'stub 1.0'\n").unwrap();
        let ok = Command::new("sh")
            .arg("-c")
            .arg(r#"chmod 755 "$1" && mv "$1" "$2""#)
            .arg("sh")
            .arg(&tmp)
            .arg(&stub)
            .status()
            .unwrap();
        assert!(ok.success());
    }
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
        .arg(&stub)
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
        .arg(s.dir.path().join("task-template.json"))
        .arg("--measure-config")
        .arg(s.dir.path().join("measure.toml"))
        .arg("--normal-config")
        .arg(s.dir.path().join("normal.toml"))
        .arg("--canaries")
        .arg(s.dir.path().join("canaries.json"));
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
        json!({"project":{"id":"01ABC","auto_integrate":true},"repos":[]}),
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
