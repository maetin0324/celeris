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

/// CoS が task を作らなかった B の形に stub を書き換える（B-t001・B-t002）: trace は created 以降が欠測、
/// attribution は試行に帰属しない CoS の行だけ（`finished_at` は受付の CoS run の終了）。`--out` は足さない。
fn taskless_b(fx: &Fx, tag: &str, cos_finished_at: &str) -> Vec<String> {
    let args = stubs(fx, &Stub::default(), tag);
    let path_of = |args: &[String], k: &str| {
        let i = args.iter().position(|a| a == k).expect("arg");
        PathBuf::from(&args[i + 1])
    };
    let trace = json!({"schema": "ops-r1-trace/1", "trial_id": B_TRIAL, "thread_id": "th-1",
        "project_id": "p-1", "cutoff": "2026-10-30T02:30:00Z", "stages": [
        {"stage": "intake", "at": "2026-10-30T02:00:00Z",
         "ref": {"author": "human:rmaeda", "cos_run_id": "C-1", "message_id": "msg-1"}, "reason": null},
        {"stage": "created", "at": null, "ref": null, "reason": "no_task_create_operation"},
        {"stage": "executed", "at": null, "ref": null, "reason": "upstream_missing:created"},
        {"stage": "outcome", "at": null, "ref": null, "reason": "upstream_missing:created"},
        {"stage": "report", "at": null, "ref": null, "reason": "upstream_missing:outcome"},
        {"stage": "cutoff", "at": "2026-10-30T02:30:00Z",
         "ref": {"report_before_cutoff": null, "source": "argument"}, "reason": null}]});
    fs::write(path_of(&args, "--b-trace"), trace.to_string()).expect("trace");
    let attr = json!({"schema": "ops-r1-attribution/2", "trial_id": B_TRIAL, "condition_id": B_COND,
        "thread_id": "th-1", "project_id": "p-1", "cutoff": "2026-10-30T02:30:00Z", "root_task_ids": [],
        "rows": [{"source": "chat_runs", "run_id": "C-1", "task_id": null, "thread_id": "th-1",
            "trial_id": null, "condition_id": null, "role": "cos_project", "role_raw": "project",
            "model": OPUS, "harness": "claude_code", "input_tokens": 30, "output_tokens": 12785,
            "cache_read_tokens": 485960, "cache_write_tokens": 51909, "wall_ms": 165976,
            "cost_usd": 0.61, "cost_basis": "pricing_estimate",
            "started_at": "2026-10-30T02:00:02Z", "finished_at": cos_finished_at}]});
    fs::write(path_of(&args, "--b-attribution"), attr.to_string()).expect("attr");
    args
}

fn with_out(fx: &Fx, tag: &str, mut args: Vec<String>) -> Vec<String> {
    let out = fx.d.join(format!("out-{tag}"));
    args.extend([
        "--out".into(),
        out.display().to_string(),
        "--records-out".into(),
        fx.d.join(format!("records-{tag}.jsonl"))
            .display()
            .to_string(),
    ]);
    args
}

/// B を停止記録（`ops-r1-stop/1`）で止めた形に stub を書き換え、`--b-stop` を足す（§4.6）。
fn stopped_b(fx: &Fx, tag: &str, stop_trial: &str) -> Vec<String> {
    let mut args = taskless_b(fx, tag, "2026-10-30T02:02:48Z");
    let stop = json!({"schema": "ops-r1-stop/1", "measurement_id": "mlsys-main-4-ops-r1-k4p1",
        "stopped_at": "2026-10-30T02:03:49Z", "stop_id": "condition_drift", "trial_id": stop_trial,
        "started_trials": [A_TRIAL, B_TRIAL], "not_started": [], "evidence": [],
        "what": "the CoS ran the analysis itself without inputs/",
        "decided_by": "external_operator:claude-code-x",
        "decided_by_note": "applied runbook 4.2/8; no human has reviewed this stop yet",
        "next": "人の判断待ち"});
    let p = fx.d.join(format!("stub-{tag}")).join("stop.json");
    fs::write(&p, stop.to_string()).expect("stop");
    args.extend(["--b-stop".into(), p.display().to_string()]);
    with_out(fx, tag, args)
}

/// §4.5・§6: condition_drift で止めた B（task run 0 件）は分母に残り、非成功のまま check・grade-out・export・
/// final-accounting を通る。wall は受付〜停止の実測で、停止を決めた主体は records の停止の行に残る。
#[test]
fn ops_r1_pipeline_b_stop_condition_drift_passes_the_stages() {
    let fx = fixture();
    let args = stopped_b(&fx, "drift", B_TRIAL);
    let o = bin().args(&args).output().expect("spawn");
    ok(&o, "assemble --b-stop");
    let (out, rec) = (fx.d.join("out-drift"), fx.d.join("records-drift.jsonl"));
    let tr = trials(&out);
    assert_eq!(tr.len(), 2, "denominator");
    let b = &tr[1];
    assert_eq!(
        (
            b["config"].as_str(),
            b["outcome"].as_str(),
            b["reason"].as_str()
        ),
        (Some("B"), Some("cancelled"), Some("condition_drift"))
    );
    assert_eq!(b["passed"], false);
    assert_eq!(b["wall_secs"], 229, "accepted 02:00:00 → stopped 02:03:49");
    assert!(
        b["observed"].as_array().is_none_or(|o| o.is_empty()),
        "no task run is invented: {}",
        b["observed"]
    );
    let text = fs::read_to_string(&rec).expect("records");
    let stops: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("line"))
        .filter(|v| v["record_type"] == "trial_stop")
        .collect();
    assert_eq!(stops.len(), 1, "{text}");
    assert_eq!(stops[0]["decided_by"], "external_operator:claude-code-x");
    assert_eq!(
        stops[0]["decided_by_note"],
        "applied runbook 4.2/8; no human has reviewed this stop yet"
    );
    let cos: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("\"role\":\"cos\""))
        .collect();
    assert_eq!(
        cos.len(),
        1,
        "the CoS run named by the trace intake is a shared row"
    );
    assert_eq!(
        stops[0]["observed_cos_runs"],
        json!([{"run_id": "C-1", "role": "cos_project", "source": "trace.intake"}])
    );

    let cases = ["--cases".to_string(), fx.grade.display().to_string()];
    ok(&stage(&fx, "grade-out", &out, &cases), "grade-out");
    let rep = check(&fx, &out);
    assert_eq!(rep["ok"], true, "check violations: {}", rep["violations"]);
    let records = ["--records".to_string(), rec.display().to_string()];
    let fa: Value = serde_json::from_str(&ok(
        &stage(&fx, "final-accounting", &out, &records),
        "final-accounting",
    ))
    .expect("fa json");
    assert_eq!(fa["schema"], "celeris.campaign-final-accounting/1");
    let totals = &rep["recomputed"]["totals"];
    assert_eq!(
        totals["A"]["n"].as_u64().zip(totals["B"]["n"].as_u64()),
        Some((1, 1))
    );
    assert_eq!(totals["B"]["ran_trials"], 1);
    assert_eq!(totals["B"]["cancelled"], 1);
    assert_eq!(totals["B"]["final_passed"], 0);
    // CoS の消費は共有の CoS 行（試行の tokens には足さない）。
    assert_eq!(
        fa["shared_unattributable"]["cos"]["state"], "observed",
        "{fa}"
    );
    let dest = fx.d.join("export-drift");
    let mut ex = records.to_vec();
    ex.extend(["--dest".into(), dest.display().to_string()]);
    ok(&stage(&fx, "export", &out, &ex), "export");

    // human-op report と整合性 checker（停止の B も分母 2 のまま一致する）。
    let ho = fx.d.join("human-ops-drift.jsonl");
    for (cond, op, from, to) in [
        (
            A_COND,
            "a-start",
            "2026-10-30T00:58:00Z",
            "2026-10-30T01:00:00Z",
        ),
        (
            B_COND,
            "b-intake",
            "2026-10-30T01:59:00Z",
            "2026-10-30T02:00:10Z",
        ),
        (
            B_COND,
            "b-stop",
            "2026-10-30T02:03:00Z",
            "2026-10-30T02:03:49Z",
        ),
    ] {
        let o = bin()
            .args(["human-op", "record", "--file"])
            .arg(&ho)
            .args(["--campaign", "mlsys-main-4-ops-r1-k4p1", "--manifest"])
            .arg(&fx.manifest)
            .args(["--condition", cond, "--case", K4, "--rep", "1"])
            .args(["--op-kind", "protocol_step", "--actor-kind", "human"])
            .args([
                "--actor",
                "evaluator",
                "--mode",
                "manual",
                "--intent",
                "protocol",
            ])
            .args(["--logical-op", op, "--started-at", from, "--ended-at", to])
            .output()
            .expect("spawn");
        ok(&o, "human-op record");
    }
    let report = fx.d.join("human-ops-report-drift");
    let o = bin()
        .args(["human-op", "report", "--file"])
        .arg(&ho)
        .arg("--events")
        .arg(&out)
        .arg("--events")
        .arg(&rec)
        .arg("--out")
        .arg(&report)
        .output()
        .expect("spawn");
    ok(&o, "human-op report");
    let (check_p, fa_p) = (fx.d.join("check-drift.json"), fx.d.join("fa-drift.json"));
    fs::write(&check_p, rep.to_string()).expect("check json");
    fs::write(&fa_p, fa.to_string()).expect("fa json");
    let o = Command::new("python3")
        .arg(repo().join("scripts/campaign/check-out-consistency.py"))
        .arg("--out")
        .arg(&out)
        .arg("--check-json")
        .arg(&check_p)
        .arg("--export-dir")
        .arg(&dest)
        .arg("--human-ops-report")
        .arg(&report)
        .arg("--final-accounting")
        .arg(&fa_p)
        .arg("--records")
        .arg(&rec)
        .output()
        .expect("python3");
    ok(&o, "check-out-consistency.py");
}

#[test]
fn ops_r1_pipeline_b_stop_rejects_a_foreign_trial() {
    let fx = fixture();
    let args = stopped_b(&fx, "foreign", "mlsys-main-4-ops-r1-k4p1-B-t002");
    let o = bin().args(&args).output().expect("spawn");
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("--b-stop: trial_id"), "{err}");
    assert!(!fx.d.join("out-foreign").join("trials.jsonl").exists());
}

/// §4.7（人の判断 3）: 案件 CoS が起票せず自分で済ませた B は `--b-cos-self-completed` で受付の CoS run の終了に閉じる。
/// 分母に残り、outcome は passed・reason cos_self_completed、試行の tokens は task の run の和（0）、CoS は共有の行。
/// checker は task run 0 件を違反にせず、check・grade-out・final-accounting・export が通る。
#[test]
fn ops_r1_pipeline_b_cos_self_completed_passes_the_stages() {
    let fx = fixture();
    let mut args = taskless_b(&fx, "self", "2026-10-30T02:01:29.900Z");
    args.push("--b-cos-self-completed".into());
    let o = bin()
        .args(with_out(&fx, "self", args))
        .output()
        .expect("spawn");
    ok(&o, "assemble --b-cos-self-completed");
    let (out, rec) = (fx.d.join("out-self"), fx.d.join("records-self.jsonl"));
    let tr = trials(&out);
    assert_eq!(tr.len(), 2, "denominator");
    let b = &tr[1];
    assert_eq!(
        (
            b["config"].as_str(),
            b["outcome"].as_str(),
            b["reason"].as_str()
        ),
        (Some("B"), Some("passed"), Some("cos_self_completed"))
    );
    assert_eq!(
        b["wall_secs"], 89,
        "accepted 02:00:00 → CoS run finished 02:01:29.9"
    );
    assert_eq!(b["tokens"], 0, "the CoS run is not trial consumption");
    assert!(
        b["observed"].as_array().is_none_or(|o| o.is_empty()),
        "no task run is invented: {}",
        b["observed"]
    );
    let text = fs::read_to_string(&rec).expect("records");
    assert_eq!(
        text.lines()
            .filter(|l| l.contains("\"role\":\"cos\""))
            .count(),
        1,
        "the CoS run named by the trace intake is a shared row: {text}"
    );
    assert!(!text.contains("trial_stop"), "{text}");

    let cases = ["--cases".to_string(), fx.grade.display().to_string()];
    ok(&stage(&fx, "grade-out", &out, &cases), "grade-out");
    let rep = check(&fx, &out);
    assert_eq!(rep["ok"], true, "check violations: {}", rep["violations"]);
    assert_eq!(rep["recomputed"]["totals"]["B"]["n"], 1);
    assert_eq!(rep["recomputed"]["totals"]["B"]["ran_trials"], 1);
    let records = ["--records".to_string(), rec.display().to_string()];
    let fa: Value = serde_json::from_str(&ok(
        &stage(&fx, "final-accounting", &out, &records),
        "final-accounting",
    ))
    .expect("fa json");
    assert_eq!(
        fa["shared_unattributable"]["cos"]["state"], "observed",
        "{fa}"
    );
    let dest = fx.d.join("export-self");
    let mut ex = records.to_vec();
    ex.extend(["--dest".into(), dest.display().to_string()]);
    ok(&stage(&fx, "export", &out, &ex), "export");
}

/// CoS run が cutoff の後に終わったら timeout（wall は trial_timeout）。reason は cos_self_completed のまま。
#[test]
fn ops_r1_pipeline_b_cos_self_completed_after_cutoff_is_timeout() {
    let fx = fixture();
    let mut args = taskless_b(&fx, "late", "2026-10-30T02:31:00Z");
    args.push("--b-cos-self-completed".into());
    ok(
        &bin()
            .args(with_out(&fx, "late", args))
            .output()
            .expect("spawn"),
        "assemble",
    );
    let b = &trials(&fx.d.join("out-late"))[1];
    assert_eq!(
        (b["outcome"].as_str(), b["reason"].as_str()),
        (Some("timeout"), Some("cos_self_completed"))
    );
    assert_eq!(b["wall_secs"], 1800);
    assert_eq!(b["passed"], false);
}

/// trace に root task があるとき・`--b-stop` と同時のときは error で out を作らない。
#[test]
fn ops_r1_pipeline_b_cos_self_completed_rejects_task_trace_and_stop() {
    let fx = fixture();
    let mut args = stubs(&fx, &Stub::default(), "withtask");
    args.push("--b-cos-self-completed".into());
    let o = bin()
        .args(with_out(&fx, "withtask", args))
        .output()
        .expect("spawn");
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("not no_task_create_operation"), "{err}");
    assert!(!fx.d.join("out-withtask").join("trials.jsonl").exists());

    let mut args = stopped_b(&fx, "both", B_TRIAL);
    args.push("--b-cos-self-completed".into());
    let o = bin().args(&args).output().expect("spawn");
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("exclusive"), "{err}");
    assert!(!fx.d.join("out-both").join("trials.jsonl").exists());
}
