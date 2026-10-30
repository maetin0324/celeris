//! `ops-r1-assemble` と既存段（stage-grader-inputs・grade-out・check・export・final-accounting）の ops-r1 での通し
//! （docs/ops/ops-r1-pipeline.md §4・§6）。A・B は stub 記録で、LLM も daemon も使わない。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::{Value, json};

const A_COND: &str = "mlsys-main-4-ops-r1-k4p1-A-direct-claude-code";
const B_COND: &str = "mlsys-main-4-ops-r1-k4p1-B-celeris-ops";
const A_TRIAL: &str = "mlsys-main-4-ops-r1-k4p1-A-t001";
const B_TRIAL: &str = "mlsys-main-4-ops-r1-k4p1-B-t001";
const K4: &str = "k4-cmp4-validity-analysis";
const SONNET: &str = "claude-sonnet-5-5";
const OPUS: &str = "claude-opus-5-5";
const TGZ: &str = "main-v3-n2-43706.tgz";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// repo の根（crates/campaign の 2 つ上）。
fn repo() -> PathBuf {
    root().join("..").join("..")
}

fn bin() -> Command {
    Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
}

fn copy(src: &Path, dst: &Path) {
    celeris_campaign::snapshot::copy_tree(src, dst).expect("copy");
}

fn ok(o: &Output, what: &str) -> String {
    assert!(
        o.status.success(),
        "{what}: exit {:?}\nstdout {}\nstderr {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// stub 記録の可変部（終了の仕方と CoS の model）。
#[derive(Clone)]
struct Stub {
    a_stop: &'static str,
    a_exit: Option<i32>,
    b_to: Option<&'static str>,
    cos_model: &'static str,
}

impl Default for Stub {
    fn default() -> Self {
        Self {
            a_stop: "exited",
            a_exit: Some(0),
            b_to: Some("done"),
            cos_model: OPUS,
        }
    }
}

struct Fx {
    _dir: tempfile::TempDir,
    d: PathBuf,
    manifest: PathBuf,
    input: PathBuf,
    cases: PathBuf,
    grade: PathBuf,
    ins: PathBuf,
}

impl Fx {
    fn lock_args(&self) -> Vec<String> {
        [
            ("--cases-root", &self.cases),
            ("--grader-root", &root().join("fixtures/grader-private")),
            ("--grade-case-root", &self.grade),
        ]
        .iter()
        .flat_map(|(k, v)| [k.to_string(), v.display().to_string()])
        .chain(["--measurement-id".into(), "mlsys-main-4-ops-r1-k4p1".into()])
        .collect()
    }

    fn manifest_args(&self) -> Vec<String> {
        vec![
            "--manifest".into(),
            self.manifest.display().to_string(),
            "--input-manifest".into(),
            self.input.display().to_string(),
        ]
    }
}

/// ops-r1 の manifest（docs/ops/ops-r1-manifest.template.json と同じ形。hash の値は試験の資産に合わせる）。
fn manifest_json(success: Option<&str>) -> Value {
    let text = fs::read_to_string(repo().join("docs/ops/ops-r1-manifest.template.json"))
        .expect("template");
    let mut m: Value = serde_json::from_str(&text).expect("json");
    let k4: Value = serde_json::from_slice(
        &fs::read(root().join("fixtures/frozen-manifests/main-4-v4-pilot-k4.json")).expect("k4"),
    )
    .expect("k4 json");
    let input: Value = serde_json::from_slice(
        &fs::read(
            repo().join("agent-docs/progress/2026-10-16-mlsys-final/final-input-manifest.json"),
        )
        .expect("input manifest"),
    )
    .expect("input json");
    m["input"]["sha256"] = input["sha256"].clone();
    m["snapshot"] = k4["snapshot"].clone();
    let sha = "a".repeat(64);
    for arm in ["A", "B"] {
        m["contract"]["pinned_versions"][arm]["claude_code_sha256"] = json!(sha);
    }
    m["contract"]["pinned_versions"]["B"]["pricing_version"] = json!("p1");
    m["experiment_sets"]["ops-r1"]["fixed"]["initial_info"] =
        k4["snapshot"]["materials_sha256"].clone();
    match success {
        Some(s) => m["experiment_sets"]["ops-r1"]["fixed"]["success"] = json!(s),
        None => {
            if let Some(f) = m["experiment_sets"]["ops-r1"]["fixed"].as_object_mut() {
                f.remove("success");
            }
        }
    }
    m
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().expect("dir");
    let d = dir.path().to_path_buf();
    let (cases, grade, ins) = (d.join("cases"), d.join("grade-cases"), d.join("inputs-src"));
    copy(&root().join("cases"), &cases);
    copy(&root().join("fixtures/grade-cases"), &grade);
    fs::create_dir_all(&ins).expect("ins");
    celeris_campaign::cells_validity::synth::healthy()
        .install_k4_fixture(&grade, &cases, &ins)
        .expect("k4 fixture");
    let input = d.join("final-input-manifest.json");
    fs::copy(
        repo().join("agent-docs/progress/2026-10-16-mlsys-final/final-input-manifest.json"),
        &input,
    )
    .expect("input manifest");
    let manifest = d.join("ops-r1.json");
    fs::write(&manifest, manifest_json(None).to_string()).expect("write");
    let fx = Fx {
        _dir: dir,
        d,
        manifest,
        input,
        cases,
        grade,
        ins,
    };
    // success（grader の固定 hash）は lock から取って書き直す。
    let m = celeris_campaign::manifest::CampaignManifest::load(&fx.manifest).expect("manifest");
    let lock = celeris_campaign::lock_exec::expected_lock(
        &celeris_campaign::lock_exec::LockSpec {
            cases_root: &fx.cases,
            grader_root: &root().join("fixtures/grader-private"),
            grade_cases_root: &fx.grade,
            measurement_id: None,
        },
        &m,
    )
    .expect("lock");
    fs::write(
        &fx.manifest,
        manifest_json(Some(&lock.grader_set_sha256())).to_string(),
    )
    .expect("write");
    fx
}

/// 試行の初期 workspace（公開入力だけ）と、その成果物を除いた hash。
fn initial_ws(fx: &Fx, at: &Path) -> String {
    fs::create_dir_all(at.join("inputs")).expect("ws");
    fs::copy(fx.ins.join(TGZ), at.join("inputs").join(TGZ)).expect("tgz");
    celeris_campaign::inputs::artifact_hash(at, K4).expect("hash")
}

/// A・B の stub 記録を書き、assemble の引数を返す。
fn stubs(fx: &Fx, s: &Stub, tag: &str) -> Vec<String> {
    let d = fx.d.join(format!("stub-{tag}"));
    let (aw, bw) = (d.join("a-artifact"), d.join("b-artifact"));
    let init = initial_ws(fx, &aw);
    initial_ws(fx, &bw);
    fs::write(aw.join("report.md"), "# A report\n").expect("a");
    fs::write(bw.join("report.md"), "# B report\n").expect("b");
    let sha = "a".repeat(64);
    let a_raw = json!({
        "schema": "ops-r1-a-raw/1", "trial_id": A_TRIAL, "condition_id": A_COND,
        "started_at": "2026-10-30T01:00:00Z", "ended_at": "2026-10-30T01:20:00Z",
        "stop": s.a_stop, "exit_code": s.a_exit,
        "claude_code": {"version": "2.1.303", "sha256": sha},
        "cost_usd": 1.25, "workspace_sha256_initial": init, "session_id": "sess-a",
        "models": [SONNET],
        "token_usage": {"input": 1000, "output": 200, "cache_read": 3000, "cache_creation": 400},
    });
    let intake = json!({
        "schema": "ops-r1-intake/1", "trial_id": B_TRIAL, "condition_id": B_COND,
        "thread_id": "th-1", "project_id": "p-1", "message_id": "msg-1", "message_seq": 1,
        "accepted_at": "2026-10-30T02:00:00Z", "request_sha256": sha, "request_bytes": 10,
        "intake_mode": "operator_post", "client_message_id": "ops-r1-intake:B-t001",
        "actor": {"kind": "human", "id": "human:rmaeda"},
    });
    let mut stages = vec![
        json!({"stage": "intake", "at": "2026-10-30T02:00:00Z", "ref": {}, "reason": null}),
        json!({"stage": "created", "at": "2026-10-30T02:01:00Z", "ref": {
            "root_task_id": "T-root", "extra_root_task_ids": []}, "reason": null}),
        json!({"stage": "executed", "at": "2026-10-30T02:02:00Z", "ref": {
            "task_ids": ["T-child", "T-root"],
            "runs": [{"run_id": "R-plan", "task_id": "T-root", "role": "planner"},
                     {"run_id": "R-work", "task_id": "T-child", "role": "worker"},
                     {"run_id": "R-rev", "task_id": "T-child", "role": "reviewer"}]}, "reason": null}),
    ];
    match s.b_to {
        Some(to) => stages.push(json!({"stage": "outcome", "at": "2026-10-30T02:20:00Z",
            "ref": {"task_id": "T-root", "to": to}, "reason": null})),
        None => stages.push(json!({"stage": "outcome", "at": null, "ref": null,
            "reason": "not_terminal_by_cutoff"})),
    }
    let reported = s.b_to == Some("done");
    stages.push(if reported {
        json!({"stage": "report", "at": "2026-10-30T02:25:00Z", "ref": {}, "reason": null})
    } else {
        json!({"stage": "report", "at": null, "ref": null, "reason": "root_not_done"})
    });
    stages.push(json!({"stage": "cutoff", "at": "2026-10-30T02:30:00Z",
        "ref": {"source": "argument", "report_before_cutoff": reported.then_some(true)}, "reason": null}));
    let trace = json!({"schema": "ops-r1-trace/1", "trial_id": B_TRIAL, "thread_id": "th-1",
        "project_id": "p-1", "cutoff": "2026-10-30T02:30:00Z", "stages": stages});
    let row = |src: &str, run: &str, task: Option<&str>, role: &str, model: &str, n: u64| {
        json!({"source": src, "run_id": run, "task_id": task, "thread_id": null,
            "trial_id": B_TRIAL, "condition_id": B_COND, "role": role, "role_raw": role,
            "model": model, "harness": "claude-code",
            "input_tokens": n, "output_tokens": n / 10, "cache_read_tokens": n * 2,
            "cache_write_tokens": n / 5, "wall_ms": 1000, "cost_usd": 0.5,
            "cost_basis": "pricing_estimate", "usage_reason": null, "time_reason": null, "reason": null})
    };
    let attr = json!({"schema": "ops-r1-attribution/2", "trial_id": B_TRIAL, "condition_id": B_COND,
    "thread_id": "th-1", "project_id": "p-1", "cutoff": "2026-10-30T02:30:00Z",
    "rows": [
        row("chat_runs", "C-1", None, "cos_project", s.cos_model, 500),
        row("runs", "R-plan", Some("T-root"), "planner", SONNET, 1000),
        row("runs", "R-work", Some("T-child"), "worker", SONNET, 5000),
        row("runs", "R-rev", Some("T-child"), "reviewer", SONNET, 800),
    ]});
    let versions = json!({"release": "94f86928dc13",
        "claude_code": {"path": "", "sha256": sha, "version": "2.1.303 (Claude Code)"},
        "pricing_version": "p1", "cos_policy_version": "cos-v6"});
    let mut paths = BTreeMap::new();
    for (k, v) in [
        ("a-raw", &a_raw),
        ("b-intake", &intake),
        ("b-trace", &trace),
        ("b-attribution", &attr),
        ("b-observed-versions", &versions),
    ] {
        let p = d.join(format!("{k}.json"));
        fs::write(&p, v.to_string()).expect("stub");
        paths.insert(k, p);
    }
    let mut args: Vec<String> = vec!["ops-r1-assemble".into()];
    args.extend(fx.manifest_args());
    args.extend(fx.lock_args());
    for (k, p) in &paths {
        args.extend([format!("--{k}"), p.display().to_string()]);
    }
    args.extend([
        "--a-artifact".into(),
        aw.display().to_string(),
        "--b-artifact".into(),
        bw.display().to_string(),
        "--b-workspace-sha256-initial".into(),
        init,
    ]);
    args
}

fn assemble(fx: &Fx, s: &Stub, tag: &str) -> (Output, PathBuf, PathBuf) {
    let out = fx.d.join(format!("out-{tag}"));
    let rec = fx.d.join(format!("records-{tag}.jsonl"));
    let mut args = stubs(fx, s, tag);
    args.extend([
        "--out".into(),
        out.display().to_string(),
        "--records-out".into(),
        rec.display().to_string(),
    ]);
    (bin().args(&args).output().expect("spawn"), out, rec)
}

fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        for e in fs::read_dir(&p).expect("read_dir") {
            let e = e.expect("entry").path();
            if e.is_dir() {
                stack.push(e);
            } else {
                let rel = e.strip_prefix(dir).expect("rel").display().to_string();
                m.insert(rel, fs::read(&e).expect("read"));
            }
        }
    }
    m
}

fn stage(fx: &Fx, cmd: &str, out: &Path, extra: &[String]) -> Output {
    let mut args = vec![cmd.to_string()];
    args.extend(fx.manifest_args());
    args.extend(fx.lock_args());
    args.extend(["--out".into(), out.display().to_string()]);
    args.extend(extra.iter().cloned());
    bin().args(&args).output().expect("spawn")
}

fn check(fx: &Fx, out: &Path) -> Value {
    let o = stage(fx, "check", out, &[]);
    let text = if o.status.success() {
        String::from_utf8_lossy(&o.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&o.stderr)
            .trim_start_matches("error: ")
            .to_string()
    };
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("check output {e}: {text}"))
}

fn trials(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join("trials.jsonl"))
        .expect("trials")
        .lines()
        .map(|l| serde_json::from_str(l).expect("line"))
        .collect()
}

#[test]
fn ops_r1_pipeline_assemble_is_byte_identical_and_runner_shaped() {
    let fx = fixture();
    let (o1, out1, rec1) = assemble(&fx, &Stub::default(), "1");
    let s1 = ok(&o1, "assemble 1");
    let (o2, out2, rec2) = assemble(&fx, &Stub::default(), "2");
    let s2 = ok(&o2, "assemble 2");
    assert_eq!(s1, s2, "stdout differs");
    let (t1, t2) = (tree(&out1), tree(&out2));
    assert_eq!(t1.keys().collect::<Vec<_>>(), t2.keys().collect::<Vec<_>>());
    for (k, v) in &t1 {
        assert_eq!(Some(v), t2.get(k), "{k} differs between two assemblies");
    }
    assert_eq!(fs::read(&rec1).expect("r1"), fs::read(&rec2).expect("r2"));
    for f in [
        "campaign.json",
        "input-manifest.json",
        "run-meta.json",
        "trials.jsonl",
        "ledger.jsonl",
        "summary.json",
        "lock.json",
        "snapshot/snapshot.json",
        "workspaces/A/k4-cmp4-validity-analysis/rep1/report.md",
        "workspaces/B/k4-cmp4-validity-analysis/rep1/report.md",
    ] {
        assert!(t1.contains_key(f), "{f} missing");
    }
    // 時刻・host・実行 path を出さない。
    let tmp = fx.d.display().to_string();
    for (k, v) in &t1 {
        let text = String::from_utf8_lossy(v);
        assert!(!text.contains(&tmp), "{k} carries the run path");
    }
    let meta: Value = serde_json::from_slice(&t1["run-meta.json"]).expect("meta");
    assert_eq!(meta["executor"], "ops-r1-assemble");
    let tr = trials(&out1);
    assert_eq!(tr.len(), 2);
    assert_eq!(
        (tr[0]["config"].as_str(), tr[0]["outcome"].as_str()),
        (Some("A"), Some("passed"))
    );
    assert_eq!(
        (tr[1]["config"].as_str(), tr[1]["outcome"].as_str()),
        (Some("B"), Some("passed"))
    );
    assert!(tr[1].get("routing").is_none());
    // B の消費は task の run の行の和（4 欄）。CoS は observed にも試行の値にも入れない（records の共有行）。
    assert_eq!(tr[1]["token_usage"]["input"], 6800);
    let roles: Vec<&str> = tr[1]["observed"]
        .as_array()
        .expect("observed")
        .iter()
        .filter_map(|o| o["role"].as_str())
        .collect();
    assert_eq!(roles, ["planner", "worker", "reviewer"]);
    assert_eq!(tr[1]["wall_secs"], 1500);
    let rec = fs::read_to_string(&rec1).expect("records");
    assert_eq!(rec.lines().count(), 7);
    let cos: Value = serde_json::from_str(rec.lines().next().expect("cos")).expect("cos json");
    assert_eq!(
        (cos["role"].as_str(), cos["shared_overhead"].as_bool()),
        (Some("cos"), Some(true))
    );
    assert!(
        rec.lines()
            .all(|l| l.contains("\"schema\":\"celeris.decision-log/1\""))
    );
    let ledger = fs::read_to_string(out1.join("ledger.jsonl")).expect("ledger");
    let b: Value = serde_json::from_str(ledger.lines().nth(1).expect("B")).expect("row");
    assert_eq!(
        (b["task_id"].as_str(), b["run_id"].as_str()),
        (Some("T-root"), Some("R-work"))
    );
    assert_eq!(b["child_task_ids"], json!(["T-child"]));
}

#[test]
fn ops_r1_pipeline_existing_stages_pass_on_the_assembled_out() {
    let fx = fixture();
    // stage-grader-inputs: grade-case の隣の公開入力 tgz を消し、ops-r1 manifest で写し戻す。
    let staged = fx.grade.join("final").join(K4).join(TGZ);
    fs::remove_file(&staged).expect("rm");
    let o = bin()
        .args(["stage-grader-inputs", "--manifest"])
        .arg(&fx.manifest)
        .arg("--grade-case-root")
        .arg(&fx.grade)
        .arg("--inputs-src")
        .arg(&fx.ins)
        .output()
        .expect("spawn");
    ok(&o, "stage-grader-inputs");
    assert_eq!(
        fs::read(&staged).expect("staged"),
        fs::read(fx.ins.join(TGZ)).expect("src")
    );

    let (o, out, rec) = assemble(&fx, &Stub::default(), "p");
    ok(&o, "assemble");
    let cases = ["--cases".to_string(), fx.grade.display().to_string()];
    let g: Value = serde_json::from_str(&ok(&stage(&fx, "grade-out", &out, &cases), "grade-out"))
        .expect("grade-out json");
    assert_eq!(g["ok"], true, "{g}");
    let rep = check(&fx, &out);
    assert_eq!(rep["ok"], true, "check violations: {}", rep["violations"]);
    let records = ["--records".to_string(), rec.display().to_string()];
    let fa: Value = serde_json::from_str(&ok(
        &stage(&fx, "final-accounting", &out, &records),
        "final-accounting",
    ))
    .expect("fa json");
    assert_eq!(fa["schema"], "celeris.campaign-final-accounting/1");
    let cos = &fa["shared_unattributable"]["cos"];
    assert_eq!(cos["state"], "observed", "{cos}");
    let dest = fx.d.join("export");
    let mut ex = records.to_vec();
    ex.extend(["--dest".into(), dest.display().to_string()]);
    ok(&stage(&fx, "export", &out, &ex), "export");
    assert!(dest.is_dir());
}

/// §6: harness_exit・operator_stop・task_failed・task_cancelled・timeout を checker が違反にしない。
/// routing の無い B（set fixed に models を宣言）も reconcile_set_fixed を通る。
#[test]
fn ops_r1_pipeline_checker_accepts_ops_r1_reasons() {
    let fx = fixture();
    for (tag, s, want) in [
        (
            "exit",
            Stub {
                a_exit: Some(3),
                b_to: Some("failed"),
                ..Stub::default()
            },
            [
                ("failed", Some("harness_exit")),
                ("failed", Some("task_failed")),
            ],
        ),
        (
            "stop",
            Stub {
                a_stop: "operator_stop",
                a_exit: None,
                b_to: Some("cancelled"),
                ..Stub::default()
            },
            [
                ("cancelled", Some("operator_stop")),
                ("cancelled", Some("task_cancelled")),
            ],
        ),
        (
            "cut",
            Stub {
                a_stop: "cutoff",
                a_exit: None,
                b_to: None,
                ..Stub::default()
            },
            [("timeout", None), ("timeout", None)],
        ),
    ] {
        let (o, out, _) = assemble(&fx, &s, tag);
        ok(&o, tag);
        let tr = trials(&out);
        for (r, (outcome, reason)) in tr.iter().zip(want) {
            assert_eq!(r["outcome"], outcome, "{tag}");
            assert_eq!(r["reason"].as_str(), reason, "{tag}");
        }
        let rep = check(&fx, &out);
        assert_eq!(rep["ok"], true, "{tag}: {}", rep["violations"]);
    }
}

#[test]
fn ops_r1_pipeline_assemble_stops_on_cos_model_deviation() {
    let fx = fixture();
    let (o, out, _) = assemble(
        &fx,
        &Stub {
            cos_model: SONNET,
            ..Stub::default()
        },
        "dev",
    );
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("condition deviation: CoS run"), "{err}");
    assert!(!out.join("trials.jsonl").exists());
}

#[test]
fn ops_r1_pipeline_assemble_rejects_old_manifest_and_dirty_out() {
    let fx = fixture();
    // 旧系列の manifest（conditions なし）は受けない。
    let mut args = stubs(&fx, &Stub::default(), "old");
    let i = args.iter().position(|a| a == "--manifest").expect("arg");
    args[i + 1] = root()
        .join("fixtures/frozen-manifests/main-4-v4-pilot-k4.json")
        .display()
        .to_string();
    let out = fx.d.join("out-old");
    args.extend([
        "--out".into(),
        out.display().to_string(),
        "--records-out".into(),
        fx.d.join("r-old").display().to_string(),
    ]);
    let o = bin().args(&args).output().expect("spawn");
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("ops-r1 manifest"));
    // 空でない out は拒否。
    let (o, out, _) = assemble(&fx, &Stub::default(), "x");
    ok(&o, "first");
    let mut args = stubs(&fx, &Stub::default(), "x2");
    args.extend([
        "--out".into(),
        out.display().to_string(),
        "--records-out".into(),
        fx.d.join("r-x2").display().to_string(),
    ]);
    let o = bin().args(&args).output().expect("spawn");
    assert!(String::from_utf8_lossy(&o.stderr).contains("must be empty"));
}
