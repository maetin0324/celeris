//! preflight 試験の共有部品: 解決済みの final 一式を一時 dir に作る（`preflight.rs` と `preflight_contract.rs`）。
#![allow(dead_code)]
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use celeris_campaign::{
    lock_tree,
    preflight::{Inputs, MeasureEnv},
    snapshot::hash_tree,
};
use serde_json::{Value, json};

/// K4 の合成 raw tgz の名前（`install_k4_fixture` が case の `inputs[0].name` もこれに替える）。
/// 実 case の公開入力 v2 は `main-v3-n2-43706-public2.tgz`（ADR 2026-10-25-k4-public-input-v2）。
pub const K4_INPUT: &str = "main-v3-n2-43706.tgz";

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// 解決済みの final 一式（manifest・入力・材料・template）を一時 dir に作る。
pub struct Setup {
    pub dir: tempfile::TempDir,
    pub inp: Inputs,
}

pub fn copy(from: &Path, to: &Path) {
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

/// 凍結済みの final 入力 manifest（過去の証跡。書き換えない）の prompt を、公開 case 定義の配送先
/// `materials/<case>/case.json` を参照する形（ADR 2026-10-20 mlsys-main2-fixes D2-4）に直して `to` へ書き、
/// 作り直した manifest の sha256 を返す。試験が使うのは写しだけ。
pub fn write_materials_prompt_manifest(from: &Path, to: &Path) -> String {
    let mut m: routing_eval::manifest::Manifest =
        serde_json::from_slice(&fs::read(from).unwrap()).unwrap();
    for p in &mut m.problems {
        let repo_ref = format!("crates/campaign/cases/final/{}/case.json", p.id);
        let want = celeris_campaign::inputs::public_case_path(&p.id);
        assert!(p.prompt.contains(&repo_ref), "frozen prompt shape changed");
        p.prompt = p.prompt.replace(&repo_ref, &want);
    }
    m.sha256 = celeris_campaign::manifest::manifest_hash(&m).unwrap();
    fs::write(to, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    m.sha256
}

pub fn resolved_manifest() -> Value {
    let p = root()
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final/final.json");
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

/// 本番資産の無い host でも通るよう、試験用の小さな git repo を K2 の初期木として case と initial.lock.json に固定する。
pub fn pin_synthetic_initial(cases: &Path, repo: &Path) {
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c"]);
    let li = lock_tree::derive_initial(repo, "HEAD", &|_| None).unwrap();
    let dir = cases.join("final/k2-bf-xattr-restart-campaign");
    fs::write(
        dir.join("initial.lock.json"),
        serde_json::to_string(&li).unwrap(),
    )
    .unwrap();
    let cj = dir.join("case.json");
    let mut c: Value = serde_json::from_slice(&fs::read(&cj).unwrap()).unwrap();
    c["initial"]["commit"] = json!(li.commit);
    c["initial"]["tree"] = json!(li.tree);
    c["initial"]["submodules"] = json!([]);
    c["snapshot"]["repo_commit"] = json!(li.commit);
    fs::write(&cj, c.to_string()).unwrap();
}

/// 試行用 KB の目印（required canary）は固定 KB の一部: `kb` と各 final case の `kb/` に置き、
/// manifest と case.json の `kb_snapshot` をその内容の hash に固定し直す（campaign lock が `--kb-src` を照合する）。
pub fn pin_trial_kb(manifest: &Path, cases: &Path, kb: &Path) {
    let canary = format!("trial kb {CANARY_TRIAL}\n");
    fs::write(kb.join("CANARY.md"), &canary).unwrap();
    let h = hash_tree(kb).unwrap();
    for e in fs::read_dir(cases.join("final")).unwrap() {
        let dir = e.unwrap().path();
        fs::write(dir.join("kb/CANARY.md"), &canary).unwrap();
        let cj = dir.join("case.json");
        let mut c: Value = serde_json::from_slice(&fs::read(&cj).unwrap()).unwrap();
        c["snapshot"]["kb_snapshot"] = json!(h);
        fs::write(&cj, c.to_string()).unwrap();
    }
    let mut v: Value = serde_json::from_slice(&fs::read(manifest).unwrap()).unwrap();
    v["snapshot"]["kb_snapshot"] = json!(h);
    fs::write(manifest, v.to_string()).unwrap();
}

/// 全能力が通る stub（userns の要る実走は worker sandbox で出来ない。実 sandbox 実走は cap-runbook）。
pub struct OkRunner;

impl celeris_campaign::cap_probe::ProbeRunner for OkRunner {
    fn run(&self, _: &str, _: &[String]) -> celeris_campaign::cap_probe::Ran {
        celeris_campaign::cap_probe::Ran::Exit {
            code: 0,
            stdout: "cmake version git version Python 3 rustc-min ok".into(),
        }
    }
}

pub fn write_cap_env(d: &Path) -> PathBuf {
    let p = d.join("cap-env.json");
    fs::write(
        &p,
        json!({"schema":"celeris.campaign-cap-env/1","a":{},"worker":{}}).to_string(),
    )
    .unwrap();
    p
}

pub fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let pd = root()
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final");
    let input_sha = write_materials_prompt_manifest(
        &pd.join("final-input-manifest.json"),
        &d.join("final-input-manifest.json"),
    );
    let mut fin = resolved_manifest();
    fin["input"]["sha256"] = json!(input_sha);
    fs::write(d.join("final.json"), fin.to_string()).unwrap();
    let (m, k) = (d.join("materials"), d.join("kb"));
    // 外部入力: 実物（本番の asset）は無いので、case を写して k4 の宣言 sha256 を試験用の中身に合わせる。
    let (cases, ins) = (d.join("cases"), d.join("inputs-src"));
    copy(&root().join("cases"), &cases);
    fs::create_dir_all(&ins).unwrap();
    let repo = d.join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    pin_synthetic_initial(&cases, &repo);
    // K4 の固定入力 tgz（公開 inputs と grader の隣）は合成 raw で代用し、grade-case と case.json を再固定する。
    let grade = d.join("grade-cases");
    copy(&root().join("fixtures/grade-cases"), &grade);
    celeris_campaign::cells_validity::synth::healthy()
        .install_k4_fixture(&grade, &cases, &ins)
        .unwrap();
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
    pin_trial_kb(&d.join("final.json"), &cases, &k);
    let measure = write_configs(d, &measure_config(d, "measure"), &normal_config(d));
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
        api_url: Some(MEASURE_API_URL.into()),
        token_env: Some("CELERIS_CAMPAIGN_TOKEN".into()),
        task_template: Some(d.join("task-template.json")),
        repo_src: Some(repo),
        materials_src: Some(m),
        kb_src: Some(k),
        inputs_src: Some(ins),
        a_account: Some("lab".into()),
        measure: Some(measure),
        measurement_id: None,
        cap_env: Some(write_cap_env(d)),
        variant_b0_sha256: None,
        worker_programs: Vec::new(),
    };
    Setup { dir, inp }
}

pub const CANARY_TRIAL: &str = "CANARY-TRIAL-KB-7f3a";
pub const CANARY_NORMAL: &str = "CANARY-NORMAL-KB-91c2";
pub const CANARY_GRADER: &str = "CANARY-GRADER-PRIVATE-c0de";

pub fn normal_config(d: &Path) -> String {
    format!(
        "db = \"{n}/celeris.db\"\nworkspace_root = \"{n}/ws\"\n[knowledge]\nroot = \"{n}/kb\"\n[memory]\ndir = \"{n}/memory\"\n\
         [api]\nlisten = \"127.0.0.1:7700\"\ntoken_file = \"{n}/token\"\n\
         [[clusters]]\nid = \"pegasus\"\nhost = \"pegasus\"\n",
        n = d.join("normal").display()
    )
}

/// 正しい測定専用 config（`tag` の dir に KB・DB・作業場所を分け、`[memory]` 無し、mount は kb: のみ）。
pub fn measure_config(d: &Path, tag: &str) -> String {
    format!(
        "db = \"{n}/celeris.db\"\nworkspace_root = \"{n}/ws\"\n[knowledge]\nroot = \"{n}/kb\"\ndefault_mounts = [\"kb:trial\"]\n\
         [api]\nlisten = \"127.0.0.1:7701\"\ntoken_file = \"{n}/token\"\n\
         [[clusters]]\nid = \"pegasus\"\nhost = \"pegasus\"\n{probe}",
        n = d.join(tag).display(),
        probe = probe_declarations("claude-code"),
    )
}

/// 測定 config の検査 harness 宣言（cap-runbook §1）: 分野 `cap-probe-<h>` を kind=fake・env_of=<h> の harness へ結ぶ。
pub fn probe_declarations(h: &str) -> String {
    format!(
        "[harnesses.{h}]\nkind = \"fake\"\n[harnesses.cap-probe-{h}]\nkind = \"fake\"\nenv_of = \"{h}\"\n\
         [harness_selection]\nexecution_class_defaults = {{}}\nrules = [{{ genre = \"cap-probe-{h}\", harness = \"cap-probe-{h}\", required_capabilities = [] }}]\n"
    )
}

pub fn write_configs(d: &Path, measure: &str, normal: &str) -> MeasureEnv {
    CURRENT_CONFIG.with(|c| *c.borrow_mut() = Some(d.join("measure.toml")));
    fs::write(d.join("measure.toml"), measure).unwrap();
    fs::write(d.join("normal.toml"), normal).unwrap();
    fs::write(
        d.join("canaries.json"),
        json!({"forbidden":[CANARY_NORMAL, CANARY_GRADER, "CANARY-OTHER-TRIAL-55aa"],"required":[CANARY_TRIAL]}).to_string(),
    )
    .unwrap();
    MeasureEnv {
        config: d.join("measure.toml"),
        normal_config: d.join("normal.toml"),
        canaries: d.join("canaries.json"),
        out: Some(d.join("out")),
        daemon_pid: Some(DAEMON_PID),
    }
}

/// `measure_config` の `[api] listen` から導かれる origin。
pub const MEASURE_API_URL: &str = "http://127.0.0.1:7701";
/// 試験の daemon DTO（`api_common::config_view`）の `identity.pid`。
pub const DAEMON_PID: u32 = 1234;
pub const MEASURE_RELEASE: &str = "0123456789ab";

thread_local! {
    /// この試験 thread が最後に書いた測定 config。偽 Host の `GET /config` はその config で起動した daemon の応答を返す。
    static CURRENT_CONFIG: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub fn current_config() -> PathBuf {
    CURRENT_CONFIG
        .with(|c| c.borrow().clone())
        .expect("write_configs first")
}

/// 測定専用 daemon の worker が全検査に通った結果（他の関心の試験が使う固定の返答。daemon の経路そのものは
/// `tests/cap_probe_daemon.rs`・`crates/celeris/tests/cap_probe_real_daemon.rs` が実 process で確かめる）。
pub fn ok_worker_run(
    spec: &celeris_campaign::cap_probe::WorkerProbeSpec,
) -> celeris_campaign::cap_probe::WorkerProbeRun {
    use celeris_campaign::cap_probe::{WorkerCheckResult, WorkerProbeRun};
    let ok = |id: &str| WorkerCheckResult {
        id: id.to_string(),
        ok: true,
        detail: "ok".into(),
    };
    let mut results = vec![ok("worker-env")];
    results.extend(spec.checks.iter().map(|c| ok(&c.id)));
    WorkerProbeRun {
        task_id: "01TESTPROBETASK".into(),
        results,
    }
}
