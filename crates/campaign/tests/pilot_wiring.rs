//! MLSys 少量 pilot（2026-10-16）で見つかった配線の欠けの回帰試験（ADR 2026-10-16-mlsys-pilot-wiring-fixes）。
//! 実 LLM・本番 API は使わない（mock executor・stub 実行ファイル・temp dir）。
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    thread::sleep,
    time::Duration,
};

use celeris_campaign::{
    aggregate::Outcome,
    celeris_exec::check_template,
    checker::check,
    executor::{
        ExecError, ExecResult, ExecStatus, Executor, MockAsyncExecutor, MockExecutor, TaskRef,
        TokenUsage, TrialRequest,
    },
    export::export,
    grade_out::{GRADES_FILE, grade_out},
    grade_runner::ProcessRunner,
    grader::{CommandOutput, CommandRunner, GradeCase, GradeOptions, grade},
    harness_exec::{
        ClaudeCodeBuilder, CommandBuilder, HarnessCommand, HarnessExecutor, parse_claude_json_usage,
    },
    ledger::Ledger,
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, TokenBasis, manifest_hash,
    },
    record::{REASON_EXECUTOR_ERROR, REASON_NO_ARTIFACT},
    runner::{RunOptions, RunReport, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Map, Value, json};

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn campaign(i: &Input, n: usize, max_parallel: u32, basis: TokenBasis) -> CampaignManifest {
    let cfg = |id, h: &str| Config {
        id,
        harness: h.into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: None,
    };
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "pilot-wiring".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: (0..n).map(|k| format!("p{k}")).collect(),
        configs: vec![
            cfg(ConfigId::A, "claude-code"),
            cfg(ConfigId::B, "celeris"),
            cfg(ConfigId::C, "celeris"),
        ],
        fault_scenarios: Default::default(),
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: basis,
            max_tokens: 100_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel,
            per_case: Cap {
                max_tokens: 1_000_000,
                max_wall_secs: 3_600,
            },
            per_arm: Cap {
                max_tokens: 10_000_000,
                max_wall_secs: 100_000,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: true,
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        snapshot: SnapshotRef {
            repo_commit: "r".into(),
            materials_sha256: "m".into(),
            kb_snapshot: "k".into(),
        },
        dependencies: BTreeMap::new(),
        variant: None,
        sha256: String::new(),
    }
}

fn opts(out: &Path) -> RunOptions {
    RunOptions {
        global_ledger: None,
        out: out.to_path_buf(),
        ..RunOptions::default()
    }
}

fn records(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join("trials.jsonl"))
        .expect("trials")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect()
}

/// checker の違反のうち routing 以外（mock/stub の executor は Celeris の routing 値を持たない）。
fn non_routing_violations(m: &CampaignManifest, out: &Path) -> Vec<String> {
    check(m, out)
        .violations
        .into_iter()
        .filter(|v| !v.contains("routing"))
        .collect()
}

fn json_file(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).expect("read")).expect("json")
}

// --- 3. max_parallel -------------------------------------------------------------------------

#[test]
fn max_parallel_caps_in_flight_trials_and_later_runs_submit_the_rest() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, 2, 1, TokenBasis::InputOutput);
    let ex = MockAsyncExecutor::new(1); // 各 task は 1 回目の poll で Wait、2 回目で Done
    let o = opts(&d.path().join("out"));
    let planned = celeris_campaign::runner::plan(&m).len() as u64; // A,B x2 + C x1 = 5

    let first = run(&m, &i, &ex, &o).expect("run 1");
    // 1 回の run で全試行を投入しない: in-flight は max_parallel（1）まで。
    assert_eq!(ex.remote_task_count(), 1);
    assert_eq!(first.parallel_deferred, planned - 1);
    assert_eq!(first.waiting, planned);

    let mut runs = 1;
    let mut last: RunReport = first;
    while last.waiting > 0 {
        last = run(&m, &i, &ex, &o).expect("run");
        runs += 1;
        let in_flight = Ledger::open(&o.out).expect("ledger").unfinished().len();
        assert!(in_flight <= 1, "in-flight {in_flight} > max_parallel");
        assert!(runs <= planned + 2, "did not converge");
    }
    assert_eq!(ex.remote_task_count() as u64, planned, "one task per trial");
    assert_eq!(records(&o.out).len() as u64, planned);
    let summary = json_file(&o.out.join("summary.json"));
    assert_eq!(summary["budget"]["max_parallel"], 1);
    assert_eq!(non_routing_violations(&m, &o.out), Vec::<String>::new());
}

// --- 4. A の起動と fail-closed ----------------------------------------------------------------

/// 実行可能 file を別 process で書く（ETXTBSY 回避）。
fn write_executable(path: &Path, contents: &str) {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn writer");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(contents.as_bytes())
        .expect("write");
    assert!(child.wait().expect("wait").success());
}

struct Stub(PathBuf);
impl CommandBuilder for Stub {
    fn build(&self, _req: &TrialRequest) -> HarnessCommand {
        HarnessCommand {
            program: self.0.clone(),
            args: vec![],
            env: vec![],
        }
    }
}

/// stub の A を runner で完走させ、A の記録を返す。
fn run_a_with(script: &str) -> (Value, PathBuf, CampaignManifest, tempfile::TempDir) {
    let d = tempfile::tempdir().expect("dir");
    let stub = d.path().join("stub.sh");
    write_executable(&stub, script);
    let i = input(1);
    let m = campaign(&i, 1, 8, TokenBasis::InputOutput);
    let out = d.path().join("out");
    let ex = HarnessExecutor::new(out.join("harness-state"), Box::new(Stub(stub)));
    let o = opts(&out);
    for _ in 0..200 {
        if run(&m, &i, &ex, &o).expect("run").waiting == 0 {
            break;
        }
        sleep(Duration::from_millis(50));
    }
    let a = records(&out)
        .into_iter()
        .find(|r| r["config"] == "A")
        .expect("A record");
    (a, out, m, d)
}

#[test]
fn a_exit_zero_without_any_artifact_is_failed_not_passed() {
    // pilot: permission mode 無しで書き込みが全て拒否され、workspace 無変更のまま exit 0 → passed と記録された。
    let (a, out, m, _d) = run_a_with(
        "#!/bin/sh\necho '{\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}'\nexit 0\n",
    );
    assert_eq!(a["outcome"], "failed", "{a}");
    assert_eq!(a["passed"], false);
    assert_eq!(a["reason"], REASON_NO_ARTIFACT);
    assert_eq!(a["workspace_sha256_final"], a["workspace_sha256_initial"]);
    assert_eq!(non_routing_violations(&m, &out), Vec::<String>::new());
    let s = json_file(&out.join("summary.json"));
    // B/C（mock は成果物を書かない）も同じ規則で数える。summary は記録の件数と一致する。
    let n = records(&out)
        .iter()
        .filter(|r| r["reason"] == REASON_NO_ARTIFACT)
        .count();
    assert_eq!(s["budget"]["no_artifact"], n);
    assert!(n >= 1);
}

#[test]
fn a_that_writes_its_artifact_passes() {
    let (a, out, m, _d) = run_a_with(
        "#!/bin/sh\necho done > result.txt\necho '{\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}'\n",
    );
    assert_eq!(a["outcome"], "passed", "{a}");
    assert!(a["reason"].is_null());
    assert_eq!(non_routing_violations(&m, &out), Vec::<String>::new());
}

#[test]
fn claude_code_a_is_started_with_a_permission_mode_and_cache_usage_is_parsed() {
    let i = input(1);
    let m = campaign(&i, 1, 1, TokenBasis::All);
    let d = tempfile::tempdir().expect("dir");
    let req = TrialRequest {
        campaign_id: "c1",
        manifest_sha256: "m1",
        config: &m.configs[0],
        problem_id: "p0",
        prompt: "do it",
        trial_no: 1,
        rep: 1,
        workspace: d.path(),
        kb: d.path(),
        timeout_secs: 10,
        budget: m.budget.per_case,
        variant: None,
    };
    let c = ClaudeCodeBuilder {
        program: "claude".into(),
        version: None,
    }
    .build(&req);
    let pos = c
        .args
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("--permission-mode");
    assert_eq!(c.args[pos + 1], "bypassPermissions");
    assert!(
        !c.args.iter().any(|a| a == "--max-turns"),
        "no added limits"
    );

    let (u, cost) = parse_claude_json_usage(
        r#"{"usage":{"input_tokens":12,"output_tokens":3,"cache_read_input_tokens":200000,"cache_creation_input_tokens":900},"total_cost_usd":0.5}"#,
    );
    assert_eq!(u, Some(TokenUsage::full(12, 3, 200_000, 900)));
    assert_eq!(cost, Some(500_000));
    let (u, _) = parse_claude_json_usage(r#"{"usage":{"input_tokens":12,"output_tokens":3}}"#);
    let u = u.expect("usage");
    assert_eq!(
        (u.cache_read, u.cache_creation),
        (None, None),
        "missing ≠ 0"
    );
}

// --- 7. tokens の内訳と基準 ----------------------------------------------------------------------

/// 全試行に同じ内訳を返し、workspace に成果物を残す sync executor。
struct Spend(TokenUsage);
impl Executor for Spend {
    fn name(&self) -> &'static str {
        "spend"
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        fs::write(req.workspace.join("out.txt"), "x").map_err(|e| e.to_string())?;
        Ok(ExecStatus::Done(ExecResult {
            outcome: Outcome::Passed,
            tokens: Some(self.0),
            cost_micro_usd: None,
            wall_secs: 1,
        }))
    }
}

#[test]
fn all_token_kinds_are_kept_in_records_summary_and_export_and_basis_decides_the_count() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let u = TokenUsage::full(100, 50, 20_000, 3_000);
    for (basis, want) in [
        (TokenBasis::InputOutput, 150),
        (TokenBasis::InputOutputCacheRead, 20_150),
        (TokenBasis::All, 23_150),
    ] {
        let m = campaign(&i, 1, 1, basis);
        let out = d.path().join(format!("{basis:?}"));
        let rep = run(&m, &i, &Spend(u), &opts(&out)).expect("run");
        assert_eq!(rep.tokens_total, want * 3, "{basis:?}");
        for r in records(&out) {
            assert_eq!(r["tokens"], want, "{basis:?}");
            assert_eq!(
                r["token_usage"],
                json!({"input":100,"output":50,"cache_read":20000,"cache_creation":3000})
            );
        }
        let s = json_file(&out.join("summary.json"));
        assert_eq!(
            s["budget"]["token_basis"],
            serde_json::to_value(basis).unwrap()
        );
        assert_eq!(
            s["cells"]["B"]["tokens"],
            json!({"input":100,"output":50,"cache_read":20000,"cache_creation":3000,
                   "usage_missing_trials":0,"cache_missing_trials":0})
        );
        let t = export(&m, &out, &out.join("export")).expect("export");
        let row = &t["configs"][0];
        assert_eq!(row["cache_read_tokens_sum"], 20_000);
        assert_eq!(row["cache_creation_tokens_sum"], 3_000);
        assert_eq!(row["input_tokens_sum"], 100);
        assert_eq!(row["tokens_sum"], want);
        let csv = fs::read_to_string(out.join("export/table.incomplete.csv")).expect("csv");
        assert!(
            csv.lines()
                .next()
                .unwrap()
                .contains("cache_read_tokens_sum")
        );
        assert!(check(&m, &out).ok);
    }

    // 基準に要る cache が取れない試行は欠測（0 にしない）。
    let m = campaign(&i, 1, 1, TokenBasis::All);
    let out = d.path().join("no-cache");
    let nc = TokenUsage {
        cache_read: None,
        cache_creation: None,
        ..u
    };
    let rep = run(&m, &i, &Spend(nc), &opts(&out)).expect("run");
    assert_eq!(rep.tokens_missing, 3);
    assert_eq!(rep.tokens_total, 0);
    let s = json_file(&out.join("summary.json"));
    assert_eq!(s["cells"]["A"]["tokens"]["cache_missing_trials"], 1);
    assert!(check(&m, &out).ok);
}

#[test]
fn checker_rejects_tokens_that_do_not_follow_the_basis() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i, 1, 1, TokenBasis::All);
    let out = d.path().join("out");
    run(&m, &i, &Spend(TokenUsage::full(1, 1, 100, 0)), &opts(&out)).expect("run");
    // input+output だけを数えた（cache を落とした）記録に書き換える。
    let text = fs::read_to_string(out.join("trials.jsonl")).expect("r");
    fs::write(
        out.join("trials.jsonl"),
        text.replacen("\"tokens\":102", "\"tokens\":2", 1),
    )
    .expect("w");
    let rep = check(&m, &out);
    assert!(!rep.ok);
    assert!(
        rep.violations.iter().any(|v| v.contains("token_usage")),
        "{:?}",
        rep.violations
    );
}

// --- 2. executor の失敗本文 ---------------------------------------------------------------------

/// 照会（投入前、workspace がまだ無い時点）で HTTP エラーを返す async executor。
struct Http400;
impl Executor for Http400 {
    fn name(&self) -> &'static str {
        "http400"
    }
    fn execute(&self, _req: &TrialRequest) -> Result<ExecStatus, String> {
        Err("unused".into())
    }
    fn is_async(&self) -> bool {
        true
    }
    fn lookup(&self, _key: &str) -> Result<Option<TaskRef>, ExecError> {
        Err(ExecError::refused(
            r#"POST /api/v1/tasks: HTTP 400 {"error":"acceptance[0]: missing field `type`"}"#,
        ))
    }
}

#[test]
fn executor_error_body_is_recorded_and_a_missing_workspace_does_not_hide_it() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i, 1, 8, TokenBasis::InputOutput);
    let out = d.path().join("out");
    // 以前は workspace 不在の hash エラーで run ごと失敗し、本当の原因（400 の本文）が消えていた。
    let rep = run(&m, &i, &Http400, &opts(&out)).expect("run continues");
    assert_eq!(rep.executor_errors, 3);
    for r in records(&out) {
        assert_eq!(r["outcome"], "error");
        assert_eq!(r["reason"], REASON_EXECUTOR_ERROR);
        let e = r["error"].as_str().expect("error body");
        assert!(
            e.contains("HTTP 400") && e.contains("missing field `type`"),
            "{e}"
        );
    }
    assert!(check(&m, &out).ok);
}

// --- 2. template・1. --api-url -----------------------------------------------------------------

#[test]
fn task_template_accepts_only_the_api_acceptance_shape() {
    let t = |v: Value| -> Map<String, Value> { v.as_object().cloned().expect("object") };
    let nested = t(json!({"acceptance":[{"text":"x","check":{"type":"reviewer"}}]}));
    assert!(check_template(&nested).unwrap_err().contains("nested"));
    assert!(check_template(&t(json!({"acceptance":[{"text":"x"}]}))).is_err());
    assert!(check_template(&t(json!({"tier":"standard"}))).is_err());
    assert!(check_template(&t(json!({"objective":"x"}))).is_err());
    let ok = t(json!({"project_id":"p","acceptance":[{"type":"reviewer","text":"x"}]}));
    assert!(check_template(&ok).is_ok());
    // repos は実行器が空配列で埋める（ADR 2026-10-20-mlsys-main2-fixes D2）。
    assert!(check_template(&t(json!({"repos":["r"]}))).is_err());
}

fn bin() -> Command {
    Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
}

#[test]
fn cli_rejects_api_url_with_path_and_nested_template_before_any_request() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i, 1, 1, TokenBasis::InputOutput);
    fs::write(d.path().join("input.json"), serde_json::to_vec(&i).unwrap()).unwrap();
    fs::write(d.path().join("c.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    fs::write(
        d.path().join("tpl.json"),
        r#"{"acceptance":[{"text":"x","check":{"type":"reviewer"}}]}"#,
    )
    .unwrap();
    let go = |url: &str, tpl: Option<&str>| {
        let mut c = bin();
        c.current_dir(d.path())
            .env("CAMPAIGN_TEST_TOKEN", "t")
            .args(["run", "--executor", "celeris", "--manifest", "c.json"])
            .args(["--out", "rel-out", "--api-url", url])
            .args(["--token-env", "CAMPAIGN_TEST_TOKEN"]);
        if let Some(t) = tpl {
            c.args(["--task-template", t]);
        }
        let o = c.output().expect("spawn");
        assert_eq!(o.status.code(), Some(1));
        String::from_utf8_lossy(&o.stderr).into_owned()
    };
    let e = go("http://127.0.0.1:9/api/v1", None);
    assert!(e.contains("without a path"), "{e}");
    let e = go("http://127.0.0.1:9", Some("tpl.json"));
    assert!(e.contains("nested"), "{e}");
}

// --- 5. materials の配布 --------------------------------------------------------------------------

#[test]
fn materials_are_distributed_into_every_trial_workspace() {
    let d = tempfile::tempdir().expect("dir");
    let (repo, mat) = (d.path().join("repo"), d.path().join("mat"));
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/lib.rs"), "").unwrap();
    fs::create_dir_all(&mat).unwrap();
    fs::write(mat.join("raw.json"), "{}").unwrap();
    let i = input(2);
    let mut m = campaign(&i, 2, 1, TokenBasis::InputOutput);
    m.snapshot.materials_sha256 = celeris_campaign::snapshot::hash_tree(&mat).unwrap();
    let out = d.path().join("out");
    let o = RunOptions {
        public_cases: Default::default(),
        global_ledger: None,
        out: out.clone(),
        repo_src: Some(repo.clone()),
        materials_src: Some(mat.clone()),
        kb_src: None,
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
    };
    run(&m, &i, &MockExecutor::scripted(BTreeMap::new()), &o).expect("run");
    let recs = records(&out);
    assert_eq!(recs.len(), 5);
    for r in &recs {
        let ws = out.join(r["workspace"].as_str().unwrap());
        assert_eq!(
            fs::read_to_string(ws.join("materials/raw.json")).unwrap(),
            "{}"
        );
        assert!(ws.join("src/lib.rs").exists());
    }
    assert!(check(&m, &out).ok);

    // repo 自身に materials/ があると衝突として拒否する。
    fs::create_dir_all(repo.join("materials")).unwrap();
    let o2 = RunOptions {
        global_ledger: None,
        out: d.path().join("out2"),
        ..o
    };
    let e = run(&m, &i, &MockExecutor::scripted(BTreeMap::new()), &o2).unwrap_err();
    assert!(e.contains("collide"), "{e}");
}

// --- 6・9. pilot fixture の自己完結と grade-case ---------------------------------------------------

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            fs::copy(e.path(), &to).unwrap();
        }
    }
}

#[test]
fn pilot_fixture_cases_are_self_contained() {
    let m = CampaignManifest::load(&fx().join("pilot.json")).expect("pilot");
    let i = m.load_input(&fx().join("pilot.json"), None).expect("input");
    m.check_against_input(&i).expect("self-contained");
    let review = i
        .problems
        .iter()
        .find(|p| p.id == "review-numeric-consistency")
        .expect("review case");
    assert!(review.prompt.contains("materials/pluvio-report-draft.md"));
    assert!(fx().join("materials/pluvio-report-draft.md").is_file());
}

#[test]
fn pilot_review_grade_case_detects_the_known_defects() {
    let case_path = fx().join("grade-cases/pilot/review-numeric-consistency.json");
    let case = GradeCase::load(&case_path).expect("case");
    let d = tempfile::tempdir().expect("dir");
    let art = d.path().join("ws");
    fs::create_dir_all(art.join("review")).unwrap();
    let f = "materials/pluvio-report-draft.md";
    fs::write(
        art.join("review/findings.json"),
        json!({"findings":[
            {"file":f,"line":4,"summary":"IO scheduling の平均は 1.95 で不一致","detail":"(1.8+2.1)/2"},
            {"file":f,"line":5,"summary":"分母は 3 件","detail":"全体平均の分母が 2 件になっている"},
            {"file":f,"line":6,"summary":"split 混在","detail":"held-out を混ぜている"}
        ]})
        .to_string(),
    )
    .unwrap();
    let opts = GradeOptions {
        case_dir: case_path.parent().unwrap().to_path_buf(),
        work_root: d.path().to_path_buf(),
        runner: &ProcessRunner::default(),
        observed_jobs: None,
        initial_tree: None,
    };
    let rep = grade(&case, &art, &opts);
    assert!(rep.final_pass, "{}", serde_json::to_string(&rep).unwrap());
    fs::write(art.join("review/findings.json"), r#"{"findings":[]}"#).unwrap();
    assert!(!grade(&case, &art, &opts).final_pass);
}

#[test]
fn pilot_impl_grade_case_really_runs_cargo_test_in_the_grade_work_dir() {
    let case_path = fx().join("grade-cases/pilot/impl-benchfs-chunk-index.json");
    let case = GradeCase::load(&case_path).expect("case");
    let d = tempfile::tempdir().expect("dir");
    let opts = GradeOptions {
        case_dir: case_path.parent().unwrap().to_path_buf(),
        work_root: d.path().to_path_buf(),
        runner: &ProcessRunner::default(),
        observed_jobs: None,
        initial_tree: None,
    };
    // 未実装（todo!）の初期 repo は不合格。
    let unsolved = d.path().join("unsolved");
    copy_dir(&fx().join("repo"), &unsolved);
    let rep = grade(&case, &unsolved, &opts);
    assert!(!rep.final_pass);
    assert_eq!(rep.checks.implementation.details["ran"], true);
    // 実装した成果物は、pristine の試験を重ねた cargo test が実際に通って合格。
    let solved = d.path().join("solved");
    copy_dir(&fx().join("repo"), &solved);
    fs::write(
        solved.join("src/chunk_index.rs"),
        "pub fn chunk_of(offset: u64, chunk_size: u64) -> u64 {\n    offset / chunk_size\n}\n",
    )
    .unwrap();
    let rep = grade(&case, &solved, &opts);
    assert!(
        rep.final_pass,
        "{}",
        serde_json::to_string(&rep.checks.implementation).unwrap()
    );
}

/// 全試験 ok を返す stub（grade-out の配線だけを見る）。
struct AllOk(Mutex<u32>);
impl CommandRunner for AllOk {
    fn run(&self, _argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
        *self.0.lock().unwrap() += 1;
        Ok(CommandOutput {
            exit: Some(0),
            stdout: "test chunk_zero ... ok\ntest chunk_last_in_first ... ok\ntest chunk_second ... ok\n"
                .into(),
            stderr: String::new(),
        })
    }
}

#[test]
fn grade_out_grades_every_run_trial_and_reports_missing_cases() {
    let d = tempfile::tempdir().expect("dir");
    let mp = fx().join("pilot.json");
    let m = CampaignManifest::load(&mp).expect("pilot");
    let i = m.load_input(&mp, None).expect("input");
    let out = d.path().join("out");
    let o = RunOptions {
        public_cases: Default::default(),
        global_ledger: None,
        out: out.clone(),
        repo_src: Some(fx().join("repo")),
        materials_src: Some(fx().join("materials")),
        kb_src: Some(fx().join("kb")),
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
    };
    run(&m, &i, &MockExecutor::seeded(), &o).expect("run");
    let runner = AllOk(Mutex::new(0));
    let v = grade_out(&m, &out, &fx().join("grade-cases/pilot"), &runner).expect("grade-out");
    // pilot の 2 case だけに grade-case がある。他の 3 problem は未採点として名前が出る。
    assert_eq!(v["ok"], false);
    let missing: Vec<&str> = v["missing_cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(missing.len(), 3, "{missing:?}");
    assert!(!missing.contains(&"impl-benchfs-chunk-index"));
    // impl case の 3 試行（A/B/C）は評価 command を走らせた。
    assert_eq!(*runner.0.lock().unwrap(), 3);
    let lines = fs::read_to_string(out.join(GRADES_FILE)).unwrap();
    assert_eq!(lines.lines().count(), records(&out).len());
}
