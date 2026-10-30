//! campaign lock の採点・結合・export への接続（P1-5 grade-export）。外部 LLM・ネットワーク・有料実行なし。
//! 通常経路: `expected_lock` → `run_gated` → `grade_out_expected` → `checker::check_expected` → `export_expected`（CLI と同じ関数）。
//! 同一 case id/split で別の grader・公開/隠しテストを渡すと、grader command を走らせる前に拒否される。
#![allow(dead_code, clippy::type_complexity)]
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use celeris_campaign::{
    cases::CaseSet,
    checker::check_expected,
    executor::{ExecStatus, Executor, MockExecutor, TrialRequest},
    export::export_expected,
    grade_out::grade_out_expected,
    grader::{CommandOutput, CommandRunner},
    grades::GRADES_FILE,
    inputs,
    lock::CampaignLock,
    lock_exec::LOCK_FILE,
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
    fn runtime_probe(
        &self,
        r: &celeris_campaign::preflight_isolation::ProbeRequest,
    ) -> Result<celeris_campaign::runtime_probe::RuntimeProbe, String> {
        Ok(r.all_denied())
    }
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
    // 公開 materials（K2・K4 の materials/）の内容 hash は repo の現物から計算する（task.md を変えると固定値がずれる）。
    {
        let t = tempfile::tempdir().unwrap();
        for c in ["k2-bf-xattr-restart-campaign", "k4-cmp4-validity-analysis"] {
            copy(
                &root().join("cases/final").join(c).join("materials"),
                &t.path().join(c),
            );
        }
        v["snapshot"]["materials_sha256"] =
            json!(celeris_campaign::snapshot::hash_tree(t.path()).unwrap());
    }
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
    // K4 の固定入力 tgz は合成 raw で代用（公開 inputs と grade-case の隣に同じ tgz。case.json を再固定）。
    celeris_campaign::cells_validity::synth::healthy()
        .install_k4_fixture(&grade, &cases, &ins)
        .unwrap();
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
        isolation: preflight_setup::isolation(d),
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

/// 評価 command を走らせた回数を数える stub（grader command の実行 = ここ）。
struct CountRunner(AtomicUsize);
impl CommandRunner for CountRunner {
    fn run(&self, _argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(CommandOutput {
            exit: Some(0),
            stdout: "test lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks ... ok\n"
                .into(),
            stderr: String::new(),
        })
    }
}

impl CountRunner {
    fn new() -> Self {
        CountRunner(AtomicUsize::new(0))
    }
    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// 実行済みの locked out（通常経路 run_gated）。
fn ran() -> (Fx, CampaignManifest, CampaignLock) {
    let f = fx();
    let e = Counting::new();
    f.gated(&e, &f.opts()).expect("locked run");
    let m = f.manifest();
    let l = f.lock();
    (f, m, l)
}

fn grade(f: &Fx, m: &CampaignManifest, l: &CampaignLock, cases: &Path) -> (Value, usize) {
    let r = CountRunner::new();
    let v = grade_out_expected(m, &f.out(), cases, &r, Some(l)).expect("grade-out");
    (v, r.calls())
}

fn grade_lines(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join(GRADES_FILE))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn write_lines(out: &Path, v: &[Value]) {
    let s: String = v.iter().map(|x| format!("{x}\n")).collect();
    fs::write(out.join(GRADES_FILE), s).unwrap();
}

fn bind_violations(f: &Fx, m: &CampaignManifest, l: &CampaignLock) -> Vec<String> {
    check_expected(m, &f.out(), Some(l)).violations
}

/// 正しい lock と完成成果物は採点・結合・export できる。未実行は未採点で、整合性違反ではない。
#[test]
fn correct_lock_grades_binds_and_exports() {
    let (f, m, l) = ran();
    let (v, calls) = grade(&f, &m, &l, &f.inp.grade_cases_root);
    assert!(calls > 0, "grader command は正しい定義なら走る");
    assert_eq!(v["rejected"], json!([]), "{v}");
    assert!(v["graded"].as_u64().unwrap() > 0);
    assert_eq!(bind_violations(&f, &m, &l), Vec::<String>::new());
    // 行は lock・grader・テスト digest を持つ。
    for g in grade_lines(&f.out()).iter().filter(|g| g["graded"] == true) {
        let c = l.case(g["case_id"].as_str().unwrap()).unwrap();
        assert_eq!(g["lock_sha256"], l.sha256.as_str());
        assert_eq!(g["grader_sha256"], c.grader.grade_case_sha256.as_str());
        assert_eq!(g["tests_sha256"], c.tests_sha256().as_str());
    }
    let dest = f.dir.path().join("export");
    let t = export_expected(&m, &f.out(), &dest, Some(&l)).expect("export");
    assert_eq!(t["lock_verified"], true);
    let rc = check_expected(&m, &f.out(), Some(&l)).recomputed.unwrap();
    assert!(rc.totals.values().any(|t| t.final_passed > 0 || t.n > 0));
}

/// 同一 case id/split の別 grader・別の公開/隠しテストは、grader command を走らせる前に拒否する。
#[test]
fn swapped_grader_or_tests_are_rejected_before_the_grader_command_runs() {
    let (f, m, l) = ran();
    let k2 = |root: &Path| root.join("final").join(K2);
    for (name, edit) in [
        ("grader definition", 0),
        ("hidden test body", 1),
        ("added public test", 2),
        ("removed hidden test", 3),
    ] {
        let alt = f.dir.path().join(format!("alt-{edit}"));
        copy(&f.inp.grade_cases_root, &alt);
        let case = k2(&alt).join(format!("{K2}.json"));
        let mut j: Value = serde_json::from_slice(&fs::read(&case).unwrap()).unwrap();
        match edit {
            0 => j["implementation"]["must_pass"] = json!(["something_else"]),
            1 => {
                // 定義 JSON は無傷のまま、隠しテストの実体だけを差し替える。
                fs::write(k2(&alt).join("hidden/.grader/run.sh"), "exit 0\n").unwrap();
            }
            2 => {
                let sha = format!("{:x}", Sha256::digest(b"pub t"));
                fs::create_dir_all(k2(&alt).join("pristine")).unwrap();
                fs::write(k2(&alt).join("pristine/t.rs"), b"pub t").unwrap();
                j["implementation"]["tests"] = json!([{"path": "t.rs", "sha256": sha}]);
            }
            _ => {
                j["implementation"]["hidden_tests"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
        }
        fs::write(&case, j.to_string()).unwrap();
        let (v, calls) = grade(&f, &m, &l, &alt);
        assert_eq!(calls, 0, "{name}: grader command を走らせない");
        assert_eq!(v["ok"], false, "{name}");
        assert!(
            v["rejected"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap().contains(K2)),
            "{name}: {v}"
        );
        assert!(
            grade_lines(&f.out())
                .iter()
                .all(|g| g["case_id"] != K2 || g["graded"] != true),
            "{name}: 別 grader の採点を書かない"
        );
    }
}

/// bind は grader/test hash を非空ではなく lock の期待値に照合する。旧 grade・別 lock・重複・改変成果物を拒否する。
#[test]
fn bind_rejects_foreign_grader_tests_lock_stale_duplicate_and_altered_artifacts() {
    let (f, m, l) = ran();
    grade(&f, &m, &l, &f.inp.grade_cases_root);
    let good = grade_lines(&f.out());
    let idx = good
        .iter()
        .position(|g| g["graded"] == true && g["case_id"] == K2)
        .unwrap();
    let cases: [(&str, Box<dyn Fn(&mut Vec<Value>)>, &str); 5] = [
        (
            "grader hash",
            Box::new(move |v| v[idx]["grader_sha256"] = json!("ab".repeat(32))),
            "locked grader",
        ),
        (
            "test hash",
            Box::new(move |v| v[idx]["tests_sha256"] = json!("cd".repeat(32))),
            "locked tests",
        ),
        (
            "old lock",
            Box::new(move |v| v[idx]["lock_sha256"] = json!("ef".repeat(32))),
            "campaign lock",
        ),
        (
            "duplicate",
            Box::new(move |v| {
                let d = v[idx].clone();
                v.push(d);
            }),
            "duplicate",
        ),
        (
            "stale artifact",
            Box::new(move |v| v[idx]["artifact_sha256"] = json!("00".repeat(32))),
            "artifact hash",
        ),
    ];
    for (name, edit, needle) in cases {
        let mut v = good.clone();
        edit(&mut v);
        write_lines(&f.out(), &v);
        let viol = bind_violations(&f, &m, &l);
        assert!(viol.iter().any(|x| x.contains(needle)), "{name}: {viol:?}");
        // 最終 export は拒否され、既存の最終版を有効なまま残さない。
        let dest = f
            .dir
            .path()
            .join(format!("export-{}", name.replace(' ', "-")));
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("table.json"), "{}").unwrap();
        fs::write(dest.join("table.csv"), "x").unwrap();
        assert!(
            export_expected(&m, &f.out(), &dest, Some(&l)).is_err(),
            "{name}"
        );
        assert!(
            !dest.join("table.json").exists(),
            "{name}: 古い最終版が残る"
        );
        assert!(!dest.join("table.csv").exists(), "{name}");
    }
    // 成果物そのものを採点後に改変しても、結合で拒否される。
    write_lines(&f.out(), &good);
    assert!(bind_violations(&f, &m, &l).is_empty());
    let ws = walk(&f.out().join("trials"), K2).pop().unwrap();
    fs::write(ws.join("tampered.txt"), "x").unwrap();
    let (v, _) = grade(&f, &m, &l, &f.inp.grade_cases_root);
    assert_eq!(v["ok"], false, "改変された成果物は採点しない: {v}");
}

/// lock の欠落・改変・差替えは採点・結合・export で拒否される。
#[test]
fn missing_tampered_or_replaced_lock_is_rejected_everywhere() {
    let (f, m, l) = ran();
    grade(&f, &m, &l, &f.inp.grade_cases_root);
    let path = f.out().join(LOCK_FILE);
    let saved = fs::read(&path).unwrap();
    let dest = f.dir.path().join("exp");
    // 改変（封印と合わない）。
    let mut j: Value = serde_json::from_slice(&saved).unwrap();
    j["cases"][0]["grader"]["grade_case_sha256"] = json!("11".repeat(32));
    fs::write(&path, j.to_string()).unwrap();
    let r = CountRunner::new();
    assert!(grade_out_expected(&m, &f.out(), &f.inp.grade_cases_root, &r, Some(&l)).is_err());
    assert_eq!(r.calls(), 0);
    assert!(!bind_violations(&f, &m, &l).is_empty());
    assert!(export_expected(&m, &f.out(), &dest, Some(&l)).is_err());
    // 封印し直した別 lock（期待 lock と違う grader）。
    let mut other = l.clone();
    other.cases[0].grader.grade_case_sha256 = "22".repeat(32);
    let other = other.seal().unwrap();
    fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    assert!(!bind_violations(&f, &m, &l).is_empty());
    assert!(export_expected(&m, &f.out(), &dest, Some(&l)).is_err());
    // 欠落。期待 lock を渡さなくても、locked と記録された out では拒否。
    fs::remove_file(&path).unwrap();
    assert!(grade_out_expected(&m, &f.out(), &f.inp.grade_cases_root, &r, None).is_err());
    assert!(!check_expected(&m, &f.out(), None).violations.is_empty());
    assert!(export_expected(&m, &f.out(), &dest, None).is_err());
    assert_eq!(r.calls(), 0);
    // 戻せば通る（上の拒否が lock の状態だけによることの対照）。
    fs::write(&path, &saved).unwrap();
    assert!(bind_violations(&f, &m, &l).is_empty());
}

/// legacy（lock 導入前・無課金 mock）の記録は従来の結合のみ。lock_verified=false で本測定の根拠にならない。
#[test]
fn legacy_lockless_records_are_never_real_performance() {
    let f = fx();
    let m = f.manifest();
    let input = m.load_input(&f.inp.manifest_path, None).unwrap();
    runner::run(&m, &input, &Counting::new(), &f.opts()).expect("lockless run");
    assert!(!f.out().join(LOCK_FILE).exists());
    let dest = f.dir.path().join("exp");
    let t = export_expected(&m, &f.out(), &dest, None).expect("legacy export");
    assert_eq!(t["lock_verified"], false);
    assert_eq!(t["real_performance"], false);
}

fn walk(dir: &Path, problem: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "workspace")
                    && p.parent()
                        .and_then(|q| q.parent())
                        .is_some_and(|q| q.file_name().is_some_and(|n| n == problem))
                {
                    v.push(p);
                } else {
                    stack.push(p);
                }
            }
        }
    }
    v
}
