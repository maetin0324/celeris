//! CLI 引数（`--cases-root/--grader-root/--grade-case-root/--measurement-id/--repo-src` 等）が lock の期待値へ配線されていることを、
//! 実 binary `celeris-campaign` で確かめる（P1-5）。外部 LLM・ネットワーク・有料実行なし。
//! locked な out は通常経路 `run_gated`（CLI の run と同じ関数）で作り、check / grade-out / export / run を binary で叩く。
#![allow(dead_code)]
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

use celeris_campaign::{
    cases::CaseSet,
    executor::{ExecStatus, Executor, MockExecutor, TrialRequest},
    inputs,
    lock::CampaignLock,
    lock_exec::{self, LOCK_FILE},
    lock_tree,
    manifest::CampaignManifest,
    preflight::{self, Host, Inputs, ResolvedProgram, run_gated},
    runner::{self, RunOptions},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod daemon_identity;
mod preflight_setup;

const K2: &str = "k2-bf-xattr-restart-campaign";
const K4: &str = "k4-cmp4-validity-analysis";
const K4_INPUT: &str = "main-v3-n2-43706.tgz";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
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

/// 呼び出し回数を数える無課金 executor（実行 = 有料投入に相当する点）。
struct Counting {
    calls: AtomicUsize,
    inner: MockExecutor,
}

impl Counting {
    fn new() -> Self {
        Counting {
            calls: AtomicUsize::new(0),
            inner: MockExecutor::seeded(),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Executor for Counting {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn simulated(&self) -> bool {
        true
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.execute(req)
    }
}

struct OkHost;
impl Host for OkHost {
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

    fn env_nonempty(&self, _: &str) -> bool {
        true
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        Ok(ResolvedProgram {
            path: p.to_path_buf(),
            version: "fake 1.0".into(),
        })
    }
    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        Ok(match path {
            "/api/v1/config" => daemon_identity::matching_daemon_body(),
            "/api/v1/projects/01ABC" => {
                json!({"project":{"id":"01ABC","auto_integrate":false},"repos":[]})
            }
            "/api/v1/projects/01ABC/repos" => {
                json!({"items":[{"name":"scratch","is_primary":true,"location":{"kind":"local","path":"/x"}}]})
            }
            _ => {
                json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":account_dir(),"logged_in":true}]})
            }
        })
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
}

/// 空の測定専用アカウント dir（process 内で共有）。
fn account_dir() -> &'static Path {
    static D: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    D.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

struct Fx {
    dir: tempfile::TempDir,
    inp: Inputs,
}

fn git(repo: &Path, args: &[&str]) {
    let o = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{args:?}");
}

fn resolved_manifest() -> Value {
    let p = root()
        .join("..")
        .join("..")
        .join("agent-docs/progress/2026-10-16-mlsys-final/final.json");
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

/// K2 の初期木（試験用の小さな git repo。本番資産には触れない）と K4 の外部入力を持つ final 一式。
fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let pd = root()
        .join("..")
        .join("..")
        .join("agent-docs/progress/2026-10-16-mlsys-final");
    fs::copy(
        pd.join("final-input-manifest.json"),
        d.join("final-input-manifest.json"),
    )
    .unwrap();
    fs::write(d.join("final.json"), resolved_manifest().to_string()).unwrap();
    let (cases, grade, ins) = (d.join("cases"), d.join("grade-cases"), d.join("inputs-src"));
    copy(&root().join("cases"), &cases);
    copy(&root().join("fixtures/grade-cases"), &grade);
    fs::create_dir_all(&ins).unwrap();
    fs::write(ins.join(K4_INPUT), b"fake archive for the test").unwrap();
    let repo = d.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    fs::write(repo.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "c"]);
    let li = lock_tree::derive_initial(&repo, "HEAD", &|_| None).unwrap();
    let k2 = cases.join("final").join(K2);
    fs::write(
        k2.join("initial.lock.json"),
        serde_json::to_string(&li).unwrap(),
    )
    .unwrap();
    let edit = |p: PathBuf, f: &dyn Fn(&mut Value)| {
        let mut c: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        f(&mut c);
        fs::write(p, c.to_string()).unwrap();
    };
    edit(k2.join("case.json"), &|c| {
        c["initial"]["commit"] = json!(li.commit);
        c["initial"]["tree"] = json!(li.tree);
        c["initial"]["submodules"] = json!([]);
        c["snapshot"]["repo_commit"] = json!(li.commit);
    });
    edit(cases.join("final").join(K4).join("case.json"), &|c| {
        c["inputs"][0]["sha256"] = json!(format!(
            "{:x}",
            Sha256::digest(b"fake archive for the test")
        ));
    });
    let (m, k) = (d.join("materials"), d.join("kb"));
    for c in [K2, K4] {
        copy(
            &root().join("cases/final").join(c).join("materials"),
            &m.join(c),
        );
    }
    copy(&root().join("cases/final").join(K2).join("kb"), &k);
    preflight_setup::pin_trial_kb(&d.join("final.json"), &cases, &k);
    let measure = preflight_setup::write_configs(
        d,
        &preflight_setup::measure_config(d, "measure"),
        &preflight_setup::normal_config(d),
    );
    fs::write(
        d.join("task-template.json"),
        json!({"project_id":"01ABC","max_turns": 100, "acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
    )
    .unwrap();
    let inp = Inputs {
        manifest_path: d.join("final.json"),
        input_override: None,
        cases_root: cases,
        grader_root: root().join("fixtures/grader-private"),
        grade_cases_root: grade,
        a_program: "claude".into(),
        api_url: Some("http://127.0.0.1:7701".into()),
        token_env: Some("CELERIS_CAMPAIGN_TOKEN".into()),
        task_template: Some(d.join("task-template.json")),
        repo_src: Some(repo),
        materials_src: Some(m),
        kb_src: Some(k),
        inputs_src: Some(ins),
        a_account: Some("lab".into()),
        measure: Some(measure),
        measurement_id: None,
        cap_env: Some(preflight_setup::write_cap_env(d)),
        variant_b0_sha256: None,
        worker_programs: Vec::new(),
    };
    Fx { dir, inp }
}

impl Fx {
    fn manifest(&self) -> CampaignManifest {
        CampaignManifest::load(&self.inp.manifest_path).unwrap()
    }
    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }
    fn lock(&self) -> CampaignLock {
        preflight::expected_lock(&self.inp, &self.manifest()).expect("expected lock")
    }
    /// 通常経路の RunOptions（case_inputs は case.json の宣言から。lock は run_gated が付ける）。
    fn opts(&self) -> RunOptions {
        let set = CaseSet::load(&self.inp.cases_root, &self.inp.grader_root).unwrap();
        RunOptions {
            public_cases: Default::default(),
            global_ledger: None,
            out: self.out(),
            repo_src: self.inp.repo_src.clone(),
            materials_src: self.inp.materials_src.clone(),
            kb_src: self.inp.kb_src.clone(),
            inputs_src: self.inp.inputs_src.clone(),
            case_inputs: inputs::for_problems(&set, &self.manifest().problems),
            lock: None,
        }
    }
    fn gated(&self, e: &Counting, o: &RunOptions) -> Result<runner::RunReport, String> {
        let m = self.manifest();
        let input = m.load_input(&self.inp.manifest_path, None).unwrap();
        run_gated(&self.inp, &OkHost, &m, &input, e, o)
    }
    fn direct(&self, e: &Counting, o: &RunOptions) -> Result<runner::RunReport, String> {
        let m = self.manifest();
        let input = m.load_input(&self.inp.manifest_path, None).unwrap();
        runner::run(&m, &input, e, o)
    }
}

fn bin(sub: &str, f: &Fx, extra: &[&str]) -> Output {
    let i = &f.inp;
    let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
    c.arg(sub)
        .arg("--manifest")
        .arg(&i.manifest_path)
        .arg("--out")
        .arg(f.out())
        .arg("--cases-root")
        .arg(&i.cases_root)
        .arg("--grader-root")
        .arg(&i.grader_root)
        .arg("--grade-case-root")
        .arg(&i.grade_cases_root)
        .arg("--repo-src")
        .arg(i.repo_src.as_ref().unwrap())
        .arg("--materials-src")
        .arg(i.materials_src.as_ref().unwrap())
        .arg("--kb-src")
        .arg(i.kb_src.as_ref().unwrap())
        .arg("--inputs-src")
        .arg(i.inputs_src.as_ref().unwrap())
        .args(extra);
    c.output().expect("run binary")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// 通常経路で locked な out を作る（無課金 mock executor）。
fn locked_out() -> Fx {
    let f = fx();
    f.gated(&Counting::new(), &f.opts()).expect("locked run");
    assert!(f.out().join(LOCK_FILE).exists());
    f
}

/// check / export は、CLI 引数から作り直した期待 lock と保存 lock を突き合わせる。引数が違えば拒否する。
#[test]
fn cli_check_and_export_rebuild_the_expected_lock_from_their_arguments() {
    let f = locked_out();
    let ok = bin("check", &f, &[]);
    assert!(ok.status.success(), "{}", text(&ok));
    // 測定 ID の引数が違う = 同一測定ではない。
    let bad = bin("check", &f, &["--measurement-id", "another-measurement"]);
    assert!(!bad.status.success(), "{}", text(&bad));
    assert!(text(&bad).contains("lock"), "{}", text(&bad));
    let dest = f.dir.path().join("exp");
    let bad = bin(
        "export",
        &f,
        &[
            "--measurement-id",
            "another-measurement",
            "--dest",
            dest.to_str().unwrap(),
        ],
    );
    assert!(!bad.status.success(), "{}", text(&bad));
    assert!(!dest.join("table.json").exists());
    // 正しい引数なら export は lock 検証済みと表に記録する。
    let good = bin("export", &f, &["--dest", dest.to_str().unwrap()]);
    assert!(good.status.success(), "{}", text(&good));
    let t: Value = serde_json::from_slice(&good.stdout).expect("table json");
    assert_eq!(t["lock_verified"], true, "{t}");
    // 保存 lock の丸ごと差替え（再封印）は、CLI が引数から作る期待 lock と食い違い拒否される。
    let path = f.out().join(LOCK_FILE);
    let mut other: CampaignLock = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    other.cases[0].grader.grade_case_sha256 = "22".repeat(32);
    fs::write(&path, serde_json::to_vec(&other.seal().unwrap()).unwrap()).unwrap();
    assert!(!bin("check", &f, &[]).status.success());
}

/// grade-out は `--grade-case-root`（期待 lock）と `--cases`（採点に使う定義）の両方を lock に照合する。
/// 差替えは評価 command（cargo test）を走らせる前に拒否され、grade を書かない。
#[test]
fn cli_grade_out_rejects_swapped_cases_before_any_grader_command() {
    let f = locked_out();
    let alt = f.dir.path().join("alt-cases");
    copy(&f.inp.grade_cases_root, &alt);
    fs::write(
        alt.join("final").join(K2).join("hidden/.grader/run.sh"),
        "exit 0\n",
    )
    .unwrap();
    let o = bin("grade-out", &f, &["--cases", alt.to_str().unwrap()]);
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains(K2), "{}", text(&o));
    let grades = f.out().join("grades.jsonl");
    let graded = fs::read_to_string(&grades).unwrap_or_default();
    let k2_graded = graded
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|g| g["case_id"] == K2 && g["graded"] == true)
        .count();
    assert_eq!(k2_graded, 0, "別 grader の K2 採点を書かない");
    // --grade-case-root 側を差し替えると、期待 lock の作成段で拒否される。
    let o = bin(
        "grade-out",
        &f,
        &[
            "--cases",
            f.inp.grade_cases_root.to_str().unwrap(),
            "--measurement-id",
            "other",
        ],
    );
    assert!(!o.status.success(), "{}", text(&o));
}

/// run は preflight・期待 lock を通ってから始まる。誤った --repo-src では out も task も作らない。
#[test]
fn cli_run_with_a_wrong_repo_src_creates_nothing() {
    let f = fx();
    fs::write(f.inp.repo_src.as_ref().unwrap().join("README"), "tampered").unwrap();
    let o = bin(
        "run",
        &f,
        &[
            "--executor",
            "celeris",
            "--api-url",
            "http://127.0.0.1:9",
            "--token-env",
            "CELERIS_TEST_TOKEN",
        ],
    );
    assert!(!o.status.success(), "{}", text(&o));
    assert!(!f.out().exists(), "out を作らない: {}", text(&o));
}

// ---- offline-lock: 無試行の lock 入口（通常の expected_lock / begin / save / env_lock::establish だけを使う） ----

struct Off {
    f: Fx,
    a: PathBuf,
    worker: PathBuf,
    evidence: PathBuf,
}

fn stub(path: &Path, body: &str) {
    use std::io::Write;
    let mut c = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
    assert!(c.wait().unwrap().success());
}

fn models_body() -> Value {
    json!({"items": [
        {"source": "claude_oauth", "model_id": "m-big", "override": {"tier": "frontier"}},
        {"source": "claude_oauth", "model_id": "m-small"},
        {"source": "claude_oauth", "model_id": "m-off", "override": {"disabled": true}},
    ]})
}

fn off() -> Off {
    let f = fx();
    let d = f.dir.path();
    let (a, worker, evidence) = (d.join("claude"), d.join("codex"), d.join("models.json"));
    stub(&a, "#!/bin/sh\necho 'claude-code@2.1.0'\n");
    stub(&worker, "#!/bin/sh\necho 'codex-stub 0.9'\n");
    fs::write(&evidence, models_body().to_string()).unwrap();
    Off {
        f,
        a,
        worker,
        evidence,
    }
}

impl Off {
    fn sha(&self) -> String {
        format!("{:x}", Sha256::digest(fs::read(&self.evidence).unwrap()))
    }
    fn run(&self, extra: &[&str]) -> Output {
        let w = format!("codex={}", self.worker.display());
        let (a, ev, sha) = (
            self.a.display().to_string(),
            self.evidence.display().to_string(),
            self.sha(),
        );
        let mut args = vec![
            "--a-program",
            &a,
            "--worker-program",
            &w,
            "--models-evidence",
            &ev,
            "--models-evidence-sha256",
            &sha,
        ];
        args.extend_from_slice(extra);
        bin("offline-lock", &self.f, &args)
    }
    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.f.out())
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
}

#[test]
fn offline_lock_creates_and_reverifies_without_any_trial_path() {
    let o = off();
    let r = o.run(&["--verify"]);
    assert!(!r.status.success(), "verify without a lock must fail");
    assert!(o.files().is_empty(), "verify must not create out");
    let r = o.run(&[]);
    assert!(r.status.success(), "{}", text(&r));
    let v: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(v["paid_calls_made"], 0);
    assert_eq!(v["env_lock"], "created");
    assert_eq!(v["campaign_lock"], "created");
    // 試行開始経路（runner・台帳・meta・harness 状態）が作る物は無い。
    assert_eq!(o.files(), ["env-lock.json", "lock.json"]);
    let env: Value =
        serde_json::from_slice(&fs::read(o.f.out().join("env-lock.json")).unwrap()).unwrap();
    assert_eq!(env["schema"], "celeris-env-lock/3");
    let cat = &env["bc"]["arms"][0]["catalog"];
    assert_eq!(cat.as_array().unwrap().len(), 2, "disabled row is excluded");
    assert_eq!(cat[0]["tier"], "frontier");
    // 保存 lock は通常経路の期待 lock と同一（封印込み）。
    let saved = lock_exec::load_saved(&o.f.manifest(), &o.f.out(), Some(&o.f.lock()))
        .unwrap()
        .unwrap();
    assert_eq!(saved.sha256, v["lock_sha256"]);
    let r = o.run(&[]);
    assert!(r.status.success(), "{}", text(&r));
    assert!(text(&r).contains("\"verified\""));
    assert!(o.run(&["--verify"]).status.success());
    assert_eq!(o.files(), ["env-lock.json", "lock.json"]);
}

#[test]
fn offline_lock_rejects_forged_or_missing_evidence_and_writes_nothing() {
    let o = off();
    // 実証跡の差替え（hash 不一致）。
    // 本文を書き換える（承認時の hash のまま）。
    fs::write(
        &o.evidence,
        json!({"items":[{"source":"claude_oauth","model_id":"made-up"}]}).to_string(),
    )
    .unwrap();
    let sha_before = o.sha();
    let ev = o.evidence.display().to_string();
    let w = format!("codex={}", o.worker.display());
    let a = o.a.display().to_string();
    let r = bin(
        "offline-lock",
        &o.f,
        &[
            "--a-program",
            &a,
            "--worker-program",
            &w,
            "--models-evidence",
            &ev,
            "--models-evidence-sha256",
            &"1".repeat(64),
        ],
    );
    assert!(!r.status.success());
    assert!(
        text(&r).contains("differs from the approved"),
        "{}",
        text(&r)
    );
    assert!(sha_before.len() == 64 && o.files().is_empty());
    // 実証跡の欠落・hash 未指定・空 catalog。
    let r = bin("offline-lock", &o.f, &["--a-program", &a]);
    assert!(
        text(&r).contains("--models-evidence is required"),
        "{}",
        text(&r)
    );
    fs::write(&o.evidence, json!({"items": []}).to_string()).unwrap();
    let r = o.run(&[]);
    assert!(!r.status.success());
    assert!(text(&r).contains("no enabled candidate"), "{}", text(&r));
    assert!(o.files().is_empty());
}

#[test]
fn offline_lock_rejects_tampering_and_path_mismatch() {
    let mut o = off();
    assert!(o.run(&[]).status.success());
    let (env_before, lock_before) = (
        fs::read(o.f.out().join("env-lock.json")).unwrap(),
        fs::read(o.f.out().join(LOCK_FILE)).unwrap(),
    );
    // 実証跡の差替え（hash も合わせて作り直した偽の catalog）は env-lock と食い違う。
    let good = fs::read(&o.evidence).unwrap();
    fs::write(
        &o.evidence,
        json!({"items":[{"source":"claude_oauth","model_id":"made-up"}]}).to_string(),
    )
    .unwrap();
    let r = o.run(&[]);
    assert!(!r.status.success());
    assert!(text(&r).contains("catalog"), "{}", text(&r));
    fs::write(&o.evidence, good).unwrap();
    // A の実体差替え。
    stub(&o.a, "#!/bin/sh\n# tampered\necho 'claude-code@2.1.0'\n");
    let r = o.run(&[]);
    assert!(!r.status.success());
    assert!(text(&r).contains("a.program.sha256"), "{}", text(&r));
    stub(&o.a, "#!/bin/sh\necho 'claude-code@2.1.0'\n");
    assert!(o.run(&[]).status.success());
    // 取り込み元の path 不一致（別の repo / cases root）。
    let other = o.f.dir.path().join("other-repo");
    copy(o.f.inp.repo_src.as_ref().unwrap(), &other);
    fs::write(other.join("README"), "different").unwrap();
    git(&other, &["commit", "-q", "-am", "diverged"]);
    let (repo, cases) = (
        o.f.inp.repo_src.clone().unwrap(),
        o.f.inp.cases_root.clone(),
    );
    o.f.inp.repo_src = Some(other);
    let r = o.run(&[]);
    assert!(!r.status.success(), "{}", text(&r));
    assert!(text(&r).contains("--repo-src"), "{}", text(&r));
    o.f.inp.repo_src = Some(repo);
    o.f.inp.cases_root = o.f.dir.path().join("nope");
    let r = o.run(&[]);
    assert!(!r.status.success(), "{}", text(&r));
    o.f.inp.cases_root = cases;
    assert!(o.run(&["--verify"]).status.success());
    // 保存 lock の変造（封印が合わない）は再照合で拒否。
    let mut lock: Value = serde_json::from_slice(&lock_before).unwrap();
    lock["kb_snapshot"] = json!("forged");
    fs::write(o.f.out().join(LOCK_FILE), lock.to_string()).unwrap();
    let r = o.run(&["--verify"]);
    assert!(!r.status.success(), "{}", text(&r));
    fs::write(o.f.out().join(LOCK_FILE), &lock_before).unwrap();
    // 保存 env-lock の変造。
    let mut env: Value = serde_json::from_slice(&env_before).unwrap();
    env["bc"]["release"] = json!("ffffffffffff");
    fs::write(o.f.out().join("env-lock.json"), env.to_string()).unwrap();
    let r = o.run(&["--verify"]);
    assert!(!r.status.success());
    assert!(text(&r).contains("bc.release"), "{}", text(&r));
}

#[test]
fn offline_lock_refuses_an_out_that_already_has_trial_data() {
    let o = off();
    fs::create_dir_all(o.f.out()).unwrap();
    fs::write(o.f.out().join("records.jsonl"), "{}\n").unwrap();
    let r = o.run(&[]);
    assert!(!r.status.success());
    assert!(
        text(&r).contains("has data but no lock.json"),
        "{}",
        text(&r)
    );
    assert_eq!(o.files(), ["records.jsonl"]);
}

#[test]
fn lock_refusal_names_existing_entries() {
    let o = off();
    fs::create_dir_all(o.f.out()).unwrap();
    fs::write(o.f.out().join("final-run-1.json"), "").unwrap();
    let r = o.run(&[]);
    assert!(!r.status.success());
    assert!(text(&r).contains("final-run-1.json"), "{}", text(&r));
}

/// runbook どおり、stdout の redirect 先は OUT の外（`$RUNS`）にあり、OUT 直下へ向けた redirect が無い。
#[test]
fn runbook_does_not_redirect_into_out() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docs")
        .join("ops");
    let mut seen = 0;
    for e in fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
        let n = e.file_name().to_string_lossy().into_owned();
        if !(n.starts_with("measure-run-") && n.ends_with(".md")) {
            continue;
        }
        seen += 1;
        let t = fs::read_to_string(e.path()).unwrap();
        for pat in ["> $OUT/", ">$OUT/", "> \"$OUT/", ">\"$OUT/", "> ${OUT}/"] {
            assert!(!t.contains(pat), "{n} redirects into OUT: {pat}");
        }
    }
    assert!(seen > 0);
}
