//! campaign lock の実行・再開・監視への接続（P1-5 run-resume）。外部 LLM・ネットワーク・有料実行なし。
//! 通常経路は `preflight::expected_lock`（CLI の run/watch と同じ関数）→ `run_gated` / `runner::run` / `watch::watch`。
//! 誤った実体では executor 呼び出し 0・out 未作成で止まる。
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use celeris_campaign::{
    cases::CaseSet,
    executor::{ExecStatus, Executor, MockExecutor, TrialRequest},
    inputs,
    lock::{CampaignLock, LockedInitial},
    lock_exec::{self, LOCK_FILE},
    lock_tree,
    manifest::CampaignManifest,
    preflight::{self, Host, Inputs, ResolvedProgram, run_gated},
    runner::{self, RunOptions},
    snapshot::hash_tree,
    watch::{self, Clock, WatchOptions},
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

struct NoSleep;
impl Clock for NoSleep {
    fn sleep(&self, _: Duration) {}
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

fn with_lock(o: &RunOptions, l: &CampaignLock) -> RunOptions {
    RunOptions {
        global_ledger: None,
        lock: Some(l.clone()),
        ..o.clone()
    }
}

/// 正しい初回・同一条件の resume / watch は通り、初期木は宣言した case にだけ配られる。
#[test]
fn first_run_resume_and_watch_pass_with_the_same_lock() {
    let f = fx();
    let e = Counting::new();
    let r1 = f.gated(&e, &f.opts()).expect("first run");
    assert!(r1.executed > 0);
    let n = e.calls();
    let saved = fs::read(f.out().join(LOCK_FILE)).expect("lock saved");
    let stored: CampaignLock = serde_json::from_slice(&saved).unwrap();
    assert_eq!(stored, f.lock(), "保存 lock は期待 lock と同じ");
    stored
        .bind_to_manifest(&f.manifest())
        .expect("campaign に結合");
    // 初期木を宣言する K2 だけが repo を持ち、K4 の workspace に K2 の repo は配られない。
    let ws = |p: &str| {
        let mut hit = None;
        for e in walk(&f.out().join("trials"), p) {
            hit = Some(e);
        }
        hit.expect(p)
    };
    assert!(ws(K2).join("README").is_file());
    assert!(
        !ws(K4).join("README").exists(),
        "K4 に K2 の初期木を配らない"
    );
    assert!(ws(K4).join("inputs").join(K4_INPUT).is_file());
    // 同一条件の resume（再実行）と watch。
    let r2 = f.gated(&e, &f.opts()).expect("resume");
    assert_eq!(r2.executed, 0);
    assert_eq!(e.calls(), n);
    let m = f.manifest();
    let input = m.load_input(&f.inp.manifest_path, None).unwrap();
    let o = with_lock(&f.opts(), &f.lock());
    let w = watch::watch(
        &m,
        &input,
        &e,
        &o,
        WatchOptions {
            interval_secs: 1,
            max_iterations: 2,
        },
        &NoSleep,
    )
    .expect("watch");
    assert_eq!(w.last.executed, 0);
    assert_eq!(e.calls(), n);
    assert_eq!(
        fs::read(f.out().join(LOCK_FILE)).unwrap(),
        saved,
        "lock は書き換わらない"
    );
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

fn refused(f: &Fx, e: &Counting, o: &RunOptions) -> String {
    let err = f.gated(e, o).expect_err("must be refused");
    assert_eq!(e.calls(), 0, "executor 投入 0: {err}");
    assert!(!f.out().exists(), "out・台帳・task を作らない: {err}");
    err
}

/// 誤った repo / submodule / 資料 / KB / 外部入力は、executor 投入 0・out 未作成で止まる。
#[test]
fn wrong_repo_materials_kb_or_inputs_stop_before_any_side_effect() {
    let f = fx();
    let l = f.lock();
    let e = Counting::new();
    let repo = f.inp.repo_src.clone().unwrap();
    // 修正版 repo（1 byte 違い）・余分な file・欠落。
    fs::write(repo.join("README"), "k2 initial tree!").unwrap();
    let err = refused(&f, &e, &f.opts());
    assert!(err.contains("repo-src") || err.contains("content"), "{err}");
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    fs::write(repo.join("extra.rs"), "x").unwrap();
    refused(&f, &e, &f.opts());
    fs::remove_file(repo.join("extra.rs")).unwrap();
    // 違う submodule（lock が持たない path に展開されたもの）も余分な内容として拒否される。
    fs::create_dir_all(repo.join("lib/pluvio")).unwrap();
    fs::write(repo.join("lib/pluvio/x.rs"), "fn x() {}").unwrap();
    refused(&f, &e, &f.opts());
    fs::remove_dir_all(repo.join("lib")).unwrap();
    // 違う資料・KB・外部入力。
    let m = f.inp.materials_src.clone().unwrap();
    fs::write(m.join(K2).join("added.txt"), "x").unwrap();
    let err = refused(&f, &e, &f.opts());
    assert!(err.contains("material"), "{err}");
    fs::remove_file(m.join(K2).join("added.txt")).unwrap();
    let k = f.inp.kb_src.clone().unwrap();
    fs::write(k.join("added.txt"), "x").unwrap();
    refused(&f, &e, &f.opts());
    fs::remove_file(k.join("added.txt")).unwrap();
    let ins = f.inp.inputs_src.clone().unwrap();
    let pinned_tgz = f.inp.grade_cases_root.join("final").join(K4).join(K4_INPUT);
    fs::write(ins.join(K4_INPUT), b"another archive").unwrap();
    refused(&f, &e, &f.opts());
    fs::copy(&pinned_tgz, ins.join(K4_INPUT)).unwrap();
    // 直接 runner::run（preflight を通さない）でも、lock があれば同じ。
    fs::write(repo.join("README"), "swapped").unwrap();
    let err = f.direct(&e, &with_lock(&f.opts(), &l)).expect_err("lock");
    assert!(err.contains("repo-src"), "{err}");
    assert_eq!((e.calls(), f.out().exists()), (0, false));
    // 正しい実体なら通る（上の拒否が実体差だけによることの対照）。
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    f.gated(&e, &f.opts()).expect("correct inputs run");
}

/// 異なる初期木の case を共通 --repo-src で配らない。初期木を宣言しない case には repo を渡さない。
#[test]
fn different_initial_trees_and_repo_free_cases_are_not_mixed() {
    let f = fx();
    let l = f.lock();
    assert_eq!(
        lock_exec::repo_free_cases(&l)
            .into_iter()
            .collect::<Vec<_>>(),
        vec![K4.to_string()]
    );
    let other = LockedInitial {
        commit: "1".repeat(40),
        tree: "2".repeat(40),
        content_sha256: "3".repeat(64),
        submodules: vec![],
    };
    let mut two = l.clone();
    two.cases.iter_mut().find(|c| c.id == K4).unwrap().initial = Some(other);
    let e = lock_exec::shared_initial(&two).expect_err("two initial trees");
    assert!(e.contains("different initial tree"), "{e}");
    // 初期木を持つ case が無いのに --repo-src を渡すと拒否。
    let mut none = l.clone();
    for c in &mut none.cases {
        c.initial = None;
    }
    let err = lock_exec::verify_sources(
        &none,
        lock_exec::Sources {
            repo: f.inp.repo_src.as_deref(),
            ..Default::default()
        },
        false,
    )
    .expect_err("repo without initial");
    assert!(err.contains("no case pins an initial tree"), "{err}");
}

/// lock の欠落・変更、grader/test・入力・構成の差替え後は同一測定の resume を拒否し、lock を書き換えない。
#[test]
fn missing_or_changed_lock_and_swapped_definitions_refuse_resume() {
    let f = fx();
    let e = Counting::new();
    f.gated(&e, &f.opts()).expect("first run");
    let n = e.calls();
    let path = f.out().join(LOCK_FILE);
    let good = fs::read(&path).unwrap();
    let refuse = |why: &str| {
        let err = f.gated(&e, &f.opts()).expect_err(why);
        assert_eq!(e.calls(), n, "{why}: executor 投入 0 ({err})");
        err
    };
    // (a) 保存 lock の改変（封印と合わない）／再封印した別内容。
    let mut v: Value = serde_json::from_slice(&good).unwrap();
    v["cases"][0]["grader"]["private_sha256"] = json!("f".repeat(64));
    fs::write(&path, v.to_string()).unwrap();
    let tampered = fs::read(&path).unwrap();
    refuse("tampered lock");
    let mut resealed: CampaignLock = serde_json::from_slice(&tampered).unwrap();
    resealed = resealed.seal().unwrap();
    fs::write(&path, serde_json::to_vec_pretty(&resealed).unwrap()).unwrap();
    let again = fs::read(&path).unwrap();
    let err = refuse("resealed different lock");
    assert!(err.contains("saved lock differs"), "{err}");
    assert_eq!(
        fs::read(&path).unwrap(),
        again,
        "拒否しても lock を書き換えない"
    );
    // (b) lock の欠落: 既存データを新しい lock に暗黙移行しない。
    fs::remove_file(&path).unwrap();
    let err = refuse("lock missing");
    assert!(err.contains("not adopted"), "{err}");
    assert!(!path.exists(), "新しい lock を書かない");
    fs::write(&path, &good).unwrap();
    f.gated(&e, &f.opts()).expect("元の lock なら再開できる");
    // (c) 公開/隠しテストの実体差替え → 期待 lock を作る段で拒否。
    let hidden = f.inp.grade_cases_root.join(format!(
        "final/{K2}/hidden/.grader/k2_metadata_ops_hidden.rs"
    ));
    let orig = fs::read(&hidden).unwrap();
    fs::write(&hidden, b"// swapped\n").unwrap();
    let err = refuse("hidden test swapped");
    assert!(err.contains("differs"), "{err}");
    fs::write(&hidden, orig).unwrap();
    // (d) grade-case 定義の差替え（case.json の pin ごと整合させても、保存 lock と食い違う）。
    let gc = f.inp.grade_cases_root.join(format!("final/{K2}/{K2}.json"));
    let orig_gc = fs::read(&gc).unwrap();
    let mut g: Value = serde_json::from_slice(&orig_gc).unwrap();
    g["implementation"]["must_pass"] = json!([
        "lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks",
        "extra"
    ]);
    fs::write(&gc, g.to_string()).unwrap();
    let cj = f.inp.cases_root.join(format!("final/{K2}/case.json"));
    let orig_cj = fs::read(&cj).unwrap();
    let mut c: Value = serde_json::from_slice(&orig_cj).unwrap();
    c["grade_case_ref"]["sha256"] =
        json!(format!("{:x}", Sha256::digest(g.to_string().as_bytes())));
    fs::write(&cj, c.to_string()).unwrap();
    let err = refuse("grade-case swapped with matching pin");
    assert!(err.contains("saved lock differs"), "{err}");
    fs::write(&gc, orig_gc).unwrap();
    fs::write(&cj, orig_cj).unwrap();
    // (e) 現在指定の入力差替え（repo / 外部入力）。
    let repo = f.inp.repo_src.clone().unwrap();
    fs::write(repo.join("README"), "changed").unwrap();
    refuse("repo swapped on resume");
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    // (f) 測定 ID の変更は同一測定ではない。
    let mut other = f.inp.clone();
    other.measurement_id = Some("new-measurement".into());
    let m = f.manifest();
    let input = m.load_input(&f.inp.manifest_path, None).unwrap();
    let err = run_gated(&other, &OkHost, &m, &input, &e, &f.opts()).expect_err("id");
    assert!(err.contains("saved lock differs"), "{err}");
    assert_eq!(e.calls(), n);
    assert_eq!(fs::read(&path).unwrap(), good, "最後まで lock は元のまま");
    f.gated(&e, &f.opts()).expect("元の定義で再開できる");
}

/// lock 無しで作られた既存の out（旧データ）は、新しい lock に暗黙移行されない。
#[test]
fn existing_data_without_a_lock_is_not_adopted() {
    let f = fx();
    let e = Counting::new();
    let l = f.lock();
    // lock 無し（無課金の制御用 None）で作った out。
    f.direct(&e, &f.opts()).expect("unlocked run");
    let n = e.calls();
    let err = f.direct(&e, &with_lock(&f.opts(), &l)).expect_err("adopt");
    assert!(err.contains("not adopted"), "{err}");
    assert!(!f.out().join(LOCK_FILE).exists());
    assert_eq!(e.calls(), n);
}

/// 保存 snapshot が（snapshot.json の hash まで整合させて）差替えられても、lock の期待 hash で拒否する。
/// コピー後の実体検査 `verify_snapshot` は取り込み直後にも同じ期待値を当てる。
#[test]
fn saved_snapshot_is_rechecked_against_the_lock_after_copy() {
    let f = fx();
    let e = Counting::new();
    f.gated(&e, &f.opts()).expect("first run");
    let n = e.calls();
    let l = f.lock();
    let snap = f.out().join("snapshot");
    lock_exec::verify_snapshot(&l, &snap).expect("copied snapshot matches the lock");
    // repo と seed を同じように書き換え、snapshot.json の hash も合わせる（旧来の自己整合検査は通る）。
    for p in [snap.join("repo/README"), snap.join("workspace-seed/README")] {
        fs::write(p, "swapped after the check").unwrap();
    }
    let meta = snap.join("snapshot.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
    v["repo_sha256"] = json!(hash_tree(&snap.join("repo")).unwrap());
    v["workspace_seed_sha256"] = json!(hash_tree(&snap.join("workspace-seed")).unwrap());
    fs::write(&meta, v.to_string()).unwrap();
    let err = lock_exec::verify_snapshot(&l, &snap).expect_err("verify");
    assert!(err.contains("snapshot/repo"), "{err}");
    let err = f.gated(&e, &f.opts()).expect_err("resume");
    assert!(err.contains("snapshot/repo"), "{err}");
    assert_eq!(e.calls(), n, "executor 投入 0");
}

/// 新測定 ID 規則: 既定は `<campaign_id>-<campaign hash 先頭 12 桁>`。内容を変えた測定は別 ID・別 out。
#[test]
fn measurement_id_rule_and_successor() {
    let f = fx();
    let l = f.lock();
    let m = f.manifest();
    let h = m.compute_sha256().unwrap();
    assert_eq!(l.measurement_id, format!("{}-{}", m.campaign_id, &h[..12]));
    let mut inp = f.inp.clone();
    inp.measurement_id = Some("final-2".into());
    let l2 = preflight::expected_lock(&inp, &m).unwrap();
    assert_eq!(l2.measurement_id, "final-2");
    assert!(celeris_campaign::lock::verify_lock(&l, &l2).is_err());
    celeris_campaign::lock::check_successor(&l, &l2).expect("別 ID なら後継として別 out に作れる");
}
