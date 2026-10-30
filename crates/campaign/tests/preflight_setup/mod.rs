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
    preflight_isolation::{IsolationInputs, ScriptSandbox},
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
        isolation: isolation(d),
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
         [api]\nlisten = \"127.0.0.1:7701\"\n[[api.tokens]]\nprincipal = \"operator:measure\"\ntoken_file = \"{n}/token\"\n\
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

/// 試験用の sandbox script（ops-r2-sandbox.sh の代わり）。probe-read には全 path 拒否の JSON を返す。
/// 実 script と実 bwrap の中の probe は `runtime_probe.rs` の opt-in 試験が確かめる。
pub const SANDBOX_STUB: &str = r#"#!/bin/sh
for a; do [ "$a" = --print-args ] && { printf '%s\n' --ro-bind /usr /usr; exit 0; }; done
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
printf '{"schema":"celeris.runtime-probe/1","principal":"%s","uid":0,"paths":[],"mounts_outside_allow":[],"ok":true}\n' "$5"
"#;

/// bwrap を stub にした CLI 試験の probe（`--probe-program`）。全 path 拒否の JSON を返す。
pub const PROBE_STUB: &str = r#"#!/bin/sh
printf '{"schema":"celeris.runtime-probe/1","principal":"%s","uid":0,"paths":[],"mounts_outside_allow":[],"ok":true}\n' "$3"
"#;

/// launcher の env file に書く隔離検査の設定（`isolation` と同じ値。probe は `PROBE_STUB`）。
pub fn isolation_env(i: &IsolationInputs) -> String {
    let args = isolation_args(i);
    let mut out = String::new();
    for kv in args.chunks(2) {
        let key = kv[0]
            .trim_start_matches("--")
            .replace('-', "_")
            .to_uppercase();
        out.push_str(&format!("{key}={}\n", kv[1]));
    }
    let probe = i.deny_list.as_ref().unwrap().with_file_name("probe-stub");
    out.push_str(&format!("PROBE_PROGRAM={}\n", probe.display()));
    out
}

/// この host で別の uid が持つ dir（L1 の EVAL_ROOT の代わり）。L1 は実 lstat と実 uid で持ち主を確かめ、
/// 自分の uid が持つ tempdir では通らない（同じ uid の chmod 000 で通さない）。試験は別の uid を作れないので、
/// `/var/lib` などから L1 の判定（`l1_owner::owner_findings`）を通る dir を探す。中には何も作らない
/// （deny path は `<dir>/celeris-preflight-absent/...` で、run の uid は覗くことも作ることもできない）。
pub fn foreign_eval_root() -> PathBuf {
    use celeris_campaign::l1_owner::{RunIds, lstat, owner_findings};
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let run = RunIds::current().unwrap();
        let parents = ["/var/lib", "/var/cache", "/var/spool", "/var/log", "/var", "/run", "/opt", "/srv", "/home"];
        for parent in parents {
            let Ok(rd) = fs::read_dir(parent) else { continue };
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            dirs.sort();
            if let Some(d) = dirs.into_iter().find(|d| {
                owner_findings(&run, d, &[d.join("celeris-preflight-absent/x")], &lstat).is_empty()
            }) {
                return d;
            }
        }
        panic!("no directory owned by another uid under {parents:?}: the L1 owner check of preflight needs one");
    })
    .clone()
}

/// 正式試行の隔離検査の入力（deny list・評価器 root・B と operator の許可表）を `d/iso` に作る。
/// 評価器 root は `foreign_eval_root`（別の uid が持つ dir）の下。
pub fn isolation(d: &Path) -> IsolationInputs {
    let iso = d.join("iso");
    let eval = foreign_eval_root();
    let sb = |p: &str| {
        let s = ScriptSandbox {
            state: Some(iso.join(format!("{p}-state"))),
            release: Some(iso.join("release")),
            account: Some(iso.join(format!("{p}-account"))),
            ro: Vec::new(),
        };
        for x in [&s.state, &s.release, &s.account].into_iter().flatten() {
            fs::create_dir_all(x).unwrap();
        }
        s
    };
    let (b, operator) = (sb("b"), sb("operator"));
    fs::write(
        iso.join("deny.txt"),
        format!(
            "# 承認票と同じ deny list\n{}\n{}\n",
            eval.join("celeris-preflight-absent/k4/private.json")
                .display(),
            iso.join("old-out/answer.json").display()
        ),
    )
    .unwrap();
    let script = iso.join("sandbox-stub.sh");
    fs::write(&script, SANDBOX_STUB).unwrap();
    let probe = iso.join("probe-stub");
    fs::write(&probe, PROBE_STUB).unwrap();
    fs::set_permissions(&probe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    IsolationInputs {
        deny_list: Some(iso.join("deny.txt")),
        eval_root: Some(eval),
        sandbox_script: Some(script),
        b,
        operator,
        probe_program: None,
    }
}

/// `isolation` の入力を CLI 引数にする。
pub fn isolation_args(i: &IsolationInputs) -> Vec<String> {
    let mut v = Vec::new();
    let mut add = |k: &str, p: &Option<PathBuf>| {
        if let Some(p) = p {
            v.push(k.to_string());
            v.push(p.to_string_lossy().into_owned());
        }
    };
    add("--deny-list", &i.deny_list);
    add("--eval-root", &i.eval_root);
    add("--sandbox-script", &i.sandbox_script);
    for (n, s) in [("b", &i.b), ("operator", &i.operator)] {
        add(&format!("--{n}-state"), &s.state);
        add(&format!("--{n}-release"), &s.release);
        add(&format!("--{n}-account"), &s.account);
    }
    v
}
