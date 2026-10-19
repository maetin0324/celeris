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

mod daemon_identity;
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
    fn probe_runner(&self) -> Box<dyn celeris_campaign::cap_probe::ProbeRunner> {
        Box::new(preflight_setup::OkRunner)
    }
    fn probe_daemon_worker(
        &self,
        _: &str,
        _: &str,
        spec: &celeris_campaign::cap_probe::WorkerProbeSpec,
    ) -> Result<celeris_campaign::cap_probe::WorkerProbeRun, String> {
        Ok(preflight_setup::ok_worker_run(spec))
    }

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
        if path == "/api/v1/config" && !self.api.iter().any(|(p, _)| *p == path) {
            return Ok(daemon_identity::matching_daemon_body());
        }
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

/// 試行 state（A の CLAUDE_CONFIG_DIR）の検査に使う部品。
struct Trial {
    out: PathBuf,
    sha: String,
    cid: String,
}

fn trial_env(s: &Setup) -> Trial {
    let m = CampaignManifest::load(&s.inp.manifest_path).unwrap();
    let out = s.inp.measure.as_ref().unwrap().out.clone().unwrap();
    fs::create_dir_all(&out).unwrap();
    Trial {
        out,
        sha: m.compute_sha256().unwrap(),
        cid: m.campaign_id.clone(),
    }
}

impl Trial {
    fn dir(&self, case: &str, rep: u32) -> PathBuf {
        celeris_campaign::harness_exec::trial_state_dir(
            &self.out.join("trial-state"),
            &self.cid,
            "A",
            case,
            rep,
        )
    }

    fn marker(&self, case: &str, rep: u32) -> celeris_campaign::harness_exec::TrialMarker {
        celeris_campaign::harness_exec::TrialMarker {
            campaign_id: self.cid.clone(),
            manifest_sha256: self.sha.clone(),
            arm: "A".into(),
            case: case.into(),
            rep,
            account_id: "lab".into(),
        }
    }

    fn write_marker(&self, dir: &Path, mk: &celeris_campaign::harness_exec::TrialMarker) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(celeris_campaign::harness_exec::TRIAL_MARKER),
            serde_json::to_vec(mk).unwrap(),
        )
        .unwrap();
    }

    /// 台帳に A の試行を 1 行足す（開始済みにする。正式の Ledger API）。
    fn start(&self, case: &str, rep: u32) {
        let key = celeris_campaign::ledger::TrialKey {
            manifest_sha256: self.sha.clone(),
            problem_id: case.into(),
            arm: ConfigId::A,
            trial_no: 1,
            rep,
        };
        celeris_campaign::ledger::Ledger::open(&self.out)
            .unwrap()
            .plan(&key, "ws", json!({}))
            .unwrap();
    }
}

fn state_misses(s: &Setup) -> Vec<String> {
    let h = FakeHost {
        api: api(account_dir()),
        ..host()
    };
    check(&s.inp, &h)
        .missing
        .into_iter()
        .filter(|m| m.check == "trial-state")
        .map(|m| m.detail)
        .collect()
}

#[test]
fn fresh_trial_accepts_missing_or_empty_state_and_ignores_account_dir_history() {
    let s = setup();
    let t = trial_env(&s);
    assert!(state_misses(&s).is_empty());
    // 空の state dir（認証 mount 先の空 file と自分の marker だけ）は可。
    let d = t.dir("k1", 1);
    t.write_marker(&d, &t.marker("k1", 1));
    fs::write(d.join(".credentials.json"), "").unwrap();
    assert!(state_misses(&s).is_empty());
    // アカウント dir の過去の transcript や CLAUDE.md は試行に渡らないので拒否理由にしない（旧一律拒否の廃止）。
    let acct = tempfile::tempdir().unwrap();
    fs::create_dir_all(acct.path().join("projects/-x")).unwrap();
    fs::write(acct.path().join("CLAUDE.md"), "memo").unwrap();
    let h = FakeHost {
        api: api(acct.path()),
        ..host()
    };
    assert!(
        check(&s.inp, &h)
            .missing
            .iter()
            .all(|m| m.check != "measure-env" || !m.detail.contains("CLAUDE.md"))
    );
}

#[test]
fn fresh_trial_rejects_contaminated_state_before_start() {
    let s = setup();
    let t = trial_env(&s);
    // CLAUDE.md
    let d = t.dir("k1", 1);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("CLAUDE.md"), "memo").unwrap();
    let m = state_misses(&s);
    assert!(
        m.iter()
            .any(|e| e.contains("not empty") && e.contains("CLAUDE.md")),
        "{m:?}"
    );
    fs::remove_file(d.join("CLAUDE.md")).unwrap();
    // projects 非空（前試行の transcript）
    fs::create_dir_all(d.join("projects/-w")).unwrap();
    assert!(state_misses(&s).iter().any(|e| e.contains("projects")));
    fs::remove_dir_all(d.join("projects")).unwrap();
    // 他試行の marker
    t.write_marker(&d, &t.marker("k2", 3));
    assert!(
        state_misses(&s)
            .iter()
            .any(|e| e.contains("another trial's identity marker"))
    );
}

#[test]
fn resume_accepts_a_started_trial_whose_marker_matches() {
    let s = setup();
    let t = trial_env(&s);
    t.start("k1", 1);
    let d = t.dir("k1", 1);
    t.write_marker(&d, &t.marker("k1", 1));
    // 再開側では transcript・memory が残っていてよい（消さない）。
    fs::create_dir_all(d.join("projects/-w")).unwrap();
    fs::write(d.join("projects/-w/t.jsonl"), "x").unwrap();
    fs::write(d.join("CLAUDE.md"), "memory").unwrap();
    assert!(state_misses(&s).is_empty(), "{:?}", state_misses(&s));
}

#[test]
fn resume_rejects_mismatched_foreign_or_missing_marker() {
    let s = setup();
    let t = trial_env(&s);
    t.start("k1", 1);
    // state 不在は引き継ぐ物が無いので可。内容があるのに marker が無ければ拒否。
    assert!(state_misses(&s).is_empty());
    let d = t.dir("k1", 1);
    fs::create_dir_all(d.join("projects/-w")).unwrap();
    assert!(
        state_misses(&s)
            .iter()
            .any(|e| e.contains("without an identity marker"))
    );
    fs::remove_dir_all(d.join("projects")).unwrap();
    for mutate in [
        (|m: &mut celeris_campaign::harness_exec::TrialMarker| m.rep = 2) as fn(&mut _),
        |m| m.case = "k2".into(),
        |m| m.arm = "B".into(),
        |m| m.campaign_id = "other".into(),
        |m| m.manifest_sha256 = "0".repeat(64),
        |m| m.account_id = "other".into(),
    ] {
        let mut mk = t.marker("k1", 1);
        mutate(&mut mk);
        t.write_marker(&d, &mk);
        assert!(
            state_misses(&s)
                .iter()
                .any(|e| e.contains("belongs to another trial")),
            "{mk:?}"
        );
    }
    // 保存 lock が別 campaign
    t.write_marker(&d, &t.marker("k1", 1));
    assert!(state_misses(&s).is_empty());
    let m0 = CampaignManifest::load(&s.inp.manifest_path).unwrap();
    let mut lock = celeris_campaign::preflight::expected_lock(&s.inp, &m0).unwrap();
    lock.campaign_sha256 = "f".repeat(64);
    fs::write(t.out.join("lock.json"), serde_json::to_vec(&lock).unwrap()).unwrap();
    let m = state_misses(&s);
    assert!(
        m.iter()
            .any(|e| e.contains("saved lock belongs to campaign")),
        "{m:?}"
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
fn fake_api_on(l: std::net::TcpListener, routes: Vec<(&'static str, Value)>) -> String {
    use std::io::{Read, Write};
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
    // 偽 API は測定 config の `[api] listen` と同じ origin で答える（api-url は config から導いた値と一致必須）。
    // 先に port を取り、測定 config の listen をそれに書き換えてから、その config で起動した daemon の応答を返す。
    let mut routes = api(&s.dir.path().join("inputs-src"));
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let cfg = s.dir.path().join("measure.toml");
    let text = fs::read_to_string(&cfg).unwrap();
    fs::write(
        &cfg,
        text.replace("127.0.0.1:7701", &format!("127.0.0.1:{port}")),
    )
    .unwrap();
    routes.push((
        "/api/v1/config",
        daemon_identity::daemon_config_body(&cfg, MEASURE_RELEASE),
    ));
    let api_url = fake_api_on(l, routes);
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
        .arg(api_url)
        .arg("--token-env")
        .arg("CELERIS_PREFLIGHT_TEST_TOKEN")
        .arg("--task-template")
        .arg(s.dir.path().join("task-template.json"))
        .args(["--daemon-pid", &preflight_setup::DAEMON_PID.to_string()])
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
    // 実 host の能力（ucx・sirius 接続 socket 等）は host 依存。不足は `capability` の env_failure として別に数え、
    // それ以外の不足がこの host 条件だけであることを確かめる。
    let v: Value = if ok.status.success() {
        serde_json::from_slice(&ok.stdout).unwrap()
    } else {
        serde_json::from_slice(&ok.stderr_json()).unwrap()
    };
    let others: Vec<&Value> = v["missing"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["check"] != "capability")
        .collect();
    assert_eq!(v["paid_calls_made"], json!(0));
    if userns {
        assert!(others.is_empty(), "{v}");
    } else {
        assert!(
            others.len() == 1 && others[0]["detail"].as_str().unwrap().contains("bwrap"),
            "{v}"
        );
    }
    let cap_missing = v["missing"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["check"] == "capability");
    assert_eq!(v["failure_class"] == json!("env_failure"), cap_missing);
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

#[test]
fn gate_refuses_a_contaminated_new_trial_before_any_ledger_or_executor_work() {
    let s = setup();
    let t = trial_env(&s);
    let d = t.dir("k1", 1);
    fs::create_dir_all(d.join("projects/-prev")).unwrap();
    let h = FakeHost {
        api: api(account_dir()),
        ..host()
    };
    let e = celeris_campaign::preflight::gate(&s.inp, &h).unwrap_err();
    assert!(e.contains("trial-state") && e.contains("projects"), "{e}");
    assert!(!t.out.join("ledger.jsonl").exists());
    // 検査は何も消さない。
    assert!(d.join("projects/-prev").is_dir());
}

/// 実験セット宣言つきの manifest にする。未使用 arm の config は置かない（有効 arm ちょうど）。
fn declare_set(s: &Setup, arms: &[&str], compare: (&str, &str)) {
    let mut v: Value = serde_json::from_slice(&fs::read(&s.inp.manifest_path).unwrap()).unwrap();
    v["configs"]
        .as_array_mut()
        .unwrap()
        .retain(|c| arms.contains(&c["id"].as_str().unwrap()));
    // 固定構成の宣言: A/B は B0（全役割 Sonnet）、B/C は B1/C1（共通候補・根拠表なし）。
    let fixed = if compare == ("A", "B") {
        let roles = ["planner", "reviewer", "cos", "worker"];
        json!({"role_models": roles.iter().map(|r| (r.to_string(), json!("claude-sonnet-5-5"))).collect::<serde_json::Map<_, _>>()})
    } else {
        for c in v["configs"].as_array_mut().unwrap() {
            let is_c = c["id"] == "C";
            let r = &mut c["routing"];
            r["models"] = json!(["claude-sonnet-5-5", "claude-opus-5-5"]);
            r.as_object_mut().unwrap().remove("evidence_table_id");
            if is_c {
                r["no_evidence_table"] = json!(true);
            }
        }
        json!({})
    };
    v["experiment_sets"] =
        json!({"s": {"arms": arms, "compare": [[compare.0, compare.1]], "fixed": fixed}});
    if !arms.contains(&"C") {
        v["secondary"]["subset"] = json!([]);
    }
    if !arms.contains(&"A") {
        v["problems"] = v["secondary"]["subset"].clone();
    }
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
}

/// A/B のみ: C の credential・probe・環境は要らず、C への呼出しは 0。A/B は C の config 無しで通る。
#[test]
fn a_b_only_set_passes_without_c_and_probes_no_c() {
    let s = setup();
    declare_set(&s, &["A", "B"], ("A", "B"));
    let r = check(&s.inp, &host());
    assert!(r.ok, "{:?}", r.missing);
    assert_eq!(r.paid_calls_made, 0);
    assert!(r.probe_targets.iter().all(|t| t.arm != ConfigId::C));
    assert!(r.probe_targets.iter().any(|t| t.arm == ConfigId::B));
    // A/B の A 側の要求（実行ファイル）は残る。
    let h = FakeHost {
        programs: vec![],
        ..host()
    };
    assert!(missing_checks(&s.inp, &h).contains(&"auth-env".into()));
}

/// B/C のみ: A の実行ファイル・bwrap・account・A の能力 probe は要らず、A への呼出しは 0。
#[test]
fn b_c_only_set_passes_without_a_program_sandbox_or_account() {
    let s = setup();
    declare_set(&s, &["B", "C"], ("B", "C"));
    let mut inp = s.inp.clone();
    inp.a_account = None;
    let h = FakeHost {
        programs: vec![],
        // A の account は GET /llm/accounts にも無い（A を使わないので引かれない）。
        api: api(account_dir())
            .into_iter()
            .filter(|(p, _)| *p != "/api/v1/llm/accounts")
            .collect(),
        sandbox: false,
        ..host()
    };
    let r = check(&inp, &h);
    assert!(r.ok, "{:?}", r.missing);
    assert!(r.a_program.is_none());
    assert!(r.probe_targets.iter().all(|t| t.arm != ConfigId::A));
    assert!(r.probe_targets.iter().any(|t| t.arm == ConfigId::C));
}

/// 有効 arm の欠落は失敗のまま（B/C のみで B の config が無い）。
#[test]
fn active_arm_gap_still_fails_in_a_declared_set() {
    let s = setup();
    declare_set(&s, &["B", "C"], ("B", "C"));
    let mut v: Value = serde_json::from_slice(&fs::read(&s.inp.manifest_path).unwrap()).unwrap();
    v["configs"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["id"] != "B");
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
    assert!(!check(&s.inp, &host()).ok);
}
