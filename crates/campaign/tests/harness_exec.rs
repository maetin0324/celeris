//! A arm の HarnessExecutor: stub 実行ファイルで完走・Wait・timeout cancel・再起動後の重複起動なし。
use std::{collections::BTreeMap, fs};

use celeris_campaign::{
    executor::{ExecStatus, Executor, TaskRef, TrialRequest},
    harness_exec::{CommandBuilder, HarnessCommand, HarnessExecutor},
    ledger::{Ledger, LedgerState, TrialKey},
    manifest::{
        Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, manifest_hash,
    },
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::Value;
use serde_json::json;

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

/// p0 だけ（A/B/C の 3 試行）。
fn campaign(i: &Input) -> CampaignManifest {
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
        campaign_id: "dep".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: vec!["p0".into()],
        configs: vec![
            cfg(ConfigId::A, "claude-code"),
            cfg(ConfigId::B, "celeris"),
            cfg(ConfigId::C, "celeris"),
        ],
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 10_000_000,
            // 全体の wall 上限は 1 試行の上限より大きくする。同値だと実時間が 1 秒進んだだけで残りが
            // 予約額を割り、後続 arm が budget_stop で打ち切られる（時計依存の不安定）。
            max_wall_secs: 10_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 1,
            per_case: celeris_campaign::manifest::Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_000_000,
            },
            per_arm: celeris_campaign::manifest::Cap {
                max_tokens: 10_000_000_000,
                max_wall_secs: 1_000_000,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: true,
        },
        snapshot: SnapshotRef {
            repo_commit: "abc".into(),
            materials_sha256: "d".into(),
            kb_snapshot: "kb@1".into(),
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    }
}

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

/// 実行可能 file を別 process で書く（ETXTBSY 回避。task-worker の test_support::write_executable と同流儀）。
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

/// workspace に started を 1 行足し、`go` が現れるまで待ち、usage 付き JSON を出して終わる。
const STUB: &str = r#"#!/bin/sh
echo started >> started.log
sleep 300 & echo $! > child.pid
while [ ! -e go ]; do sleep 0.05; done
kill $! 2>/dev/null
echo '{"usage":{"input_tokens":100,"output_tokens":23},"total_cost_usd":0.0123}'
"#;

struct H {
    _d: tempfile::TempDir,
    state: PathBuf,
    ws: PathBuf,
    kb: PathBuf,
    stub: PathBuf,
    cfg: Config,
}

fn h(script: &str) -> H {
    let d = tempfile::tempdir().expect("dir");
    let (ws, kb, state) = (
        d.path().join("ws"),
        d.path().join("kb"),
        d.path().join("st"),
    );
    for p in [&ws, &kb, &state] {
        fs::create_dir_all(p).expect("mkdir");
    }
    let stub = d.path().join("stub.sh");
    write_executable(&stub, script);
    let cfg = campaign(&input(1)).configs[0].clone();
    H {
        _d: d,
        state,
        ws,
        kb,
        stub,
        cfg,
    }
}

impl H {
    fn exec(&self) -> HarnessExecutor {
        HarnessExecutor::new(self.state.clone(), Box::new(Stub(self.stub.clone())))
    }
    fn req(&self, timeout_secs: u64) -> TrialRequest<'_> {
        TrialRequest {
            campaign_id: "c1",
            manifest_sha256: "m1",
            config: &self.cfg,
            problem_id: "p0",
            prompt: "fix it",
            trial_no: 1,
            rep: 1,
            workspace: &self.ws,
            kb: &self.kb,
            timeout_secs,
            budget: celeris_campaign::manifest::Cap {
                max_tokens: u64::MAX,
                max_wall_secs: u64::MAX,
            },
        }
    }
    fn started_lines(&self) -> usize {
        fs::read_to_string(self.ws.join("started.log")).map_or(0, |s| s.lines().count())
    }
}

fn wait_done(x: &HarnessExecutor, t: &TaskRef) -> celeris_campaign::executor::ExecResult {
    for _ in 0..400 {
        if let ExecStatus::Done(r) = x.poll(t).expect("poll") {
            return r;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("harness did not finish");
}

fn gone(pid_file: &Path) -> bool {
    let pid = fs::read_to_string(pid_file)
        .expect("pid")
        .trim()
        .to_string();
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(s) => s[s.rfind(')').unwrap_or(0) + 1..]
            .trim_start()
            .starts_with('Z'),
    }
}

fn wait_gone(pid_file: &Path) {
    for _ in 0..200 {
        if gone(pid_file) {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("child process group member still alive");
}

#[test]
fn stub_runs_to_completion_with_usage_and_wait_keeps_workspace() {
    let t = h(STUB);
    let x = t.exec();
    let r = x.submit(&t.req(600), "campaign-k1").expect("submit");
    assert!(r.run_id.as_deref().is_some_and(|s| s.starts_with("pid:")));
    // 実行中は Wait で、workspace（stub が書いた file）は残る。
    assert_eq!(x.poll(&r).expect("poll"), ExecStatus::Wait);
    while t.started_lines() == 0 {
        sleep(Duration::from_millis(20));
    }
    assert!(t.ws.join("started.log").exists());
    fs::write(t.ws.join("go"), "").expect("go");
    let res = wait_done(&x, &r);
    assert_eq!(res.outcome, celeris_campaign::aggregate::Outcome::Passed);
    let u = res.tokens.expect("usage");
    assert_eq!(u.input + u.output, 123);
    assert_eq!(res.cost_micro_usd, Some(12_300));
}

#[test]
fn missing_usage_is_none() {
    let t = h("#!/bin/sh
echo hello
");
    let x = t.exec();
    let r = x.submit(&t.req(600), "campaign-k2").expect("submit");
    let res = wait_done(&x, &r);
    assert_eq!((res.tokens, res.cost_micro_usd), (None, None));
}

#[test]
fn nonzero_exit_is_failed() {
    let t = h("#!/bin/sh
exit 3
");
    let x = t.exec();
    let r = x.submit(&t.req(600), "campaign-k3").expect("submit");
    let res = wait_done(&x, &r);
    assert_eq!(res.outcome, celeris_campaign::aggregate::Outcome::Failed);
}

#[test]
fn restart_does_not_start_twice_and_follows_same_process() {
    let t = h(STUB);
    let key = "campaign-k4";
    let r1 = t.exec().submit(&t.req(600), key).expect("submit");
    while t.started_lines() == 0 {
        sleep(Duration::from_millis(20));
    }
    // アダプタ再起動 = 新しい executor。lookup が生きている process を返し、submit も再起動しない。
    let x2 = t.exec();
    assert_eq!(x2.lookup(key).expect("lookup"), Some(r1.clone()));
    let r2 = x2.resolve(&t.req(600), key).expect("resolve");
    assert_eq!(r2, r1, "same pid/pgid");
    assert_eq!(x2.submit(&t.req(600), key).expect("submit again"), r1);
    assert_eq!(x2.poll(&r2).expect("poll"), ExecStatus::Wait);
    fs::write(t.ws.join("go"), "").expect("go");
    wait_done(&x2, &r2);
    assert_eq!(t.started_lines(), 1, "harness was started exactly once");
}

#[test]
fn crash_during_spawn_marker_refuses_to_restart() {
    let t = h(STUB);
    let dir = t.state.join("campaign-k5");
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(dir.join("starting"), "1").expect("marker");
    let x = t.exec();
    assert!(x.lookup("campaign-k5").is_err());
    assert!(x.submit(&t.req(600), "campaign-k5").is_err());
    assert_eq!(t.started_lines(), 0);
}

#[test]
fn timeout_kills_whole_process_group() {
    let t = h(STUB);
    let x = t.exec();
    let r = x.submit(&t.req(60), "campaign-k6").expect("submit");
    while !t.ws.join("child.pid").exists() || t.started_lines() == 0 {
        sleep(Duration::from_millis(20));
    }
    sleep(Duration::from_millis(100));
    // 時計を差し替える代わりに起動時刻を過去へ書き換えて、timeout 超過を決定的に作る。
    let mp = t.state.join("campaign-k6/meta.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&mp).expect("meta")).expect("json");
    m["started_unix"] = json!(1);
    fs::write(&mp, serde_json::to_vec(&m).expect("ser")).expect("write");
    let ExecStatus::Done(res) = x.poll(&r).expect("poll") else {
        panic!("expected timeout")
    };
    assert_eq!(res.outcome, celeris_campaign::aggregate::Outcome::Timeout);
    assert_eq!(res.wall_secs, 60);
    wait_gone(&t.ws.join("child.pid"));
    // 確定済み: 再 poll も同じ結果で、再起動しない。
    assert!(matches!(x.poll(&r).expect("poll"), ExecStatus::Done(_)));
    assert_eq!(t.started_lines(), 1);
}

#[test]
fn cancel_kills_process_group_and_is_idempotent() {
    let t = h(STUB);
    let x = t.exec();
    let r = x.submit(&t.req(600), "campaign-k7").expect("submit");
    while !t.ws.join("child.pid").exists() || t.started_lines() == 0 {
        sleep(Duration::from_millis(20));
    }
    sleep(Duration::from_millis(100));
    x.cancel(&r, "budget_stop").expect("cancel");
    x.cancel(&r, "budget_stop").expect("cancel twice");
    wait_gone(&t.ws.join("child.pid"));
    let ExecStatus::Done(res) = x.poll(&r).expect("poll") else {
        panic!("expected cancelled")
    };
    assert_eq!(res.outcome, celeris_campaign::aggregate::Outcome::Cancelled);
}

#[test]
fn runner_drives_three_arms_with_stub_and_resumes_without_restart() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(1);
    let m = campaign(&i);
    let out = d.path().join("out");
    let stub = d.path().join("stub.sh");
    // 試行の workspace は out 配下。started.log は試行ごとの workspace に書かれる。
    write_executable(
        &stub,
        "#!/bin/sh
echo started >> started.log
echo '{\"usage\":{\"input_tokens\":5,\"output_tokens\":5}}'
",
    );
    let opts = RunOptions {
        global_ledger: None,
        out: out.clone(),
        ..RunOptions::default()
    };
    let mk = || HarnessExecutor::new(d.path().join("state"), Box::new(Stub(stub.clone())));
    // 1 周目は完了を待たずに戻りうる（Wait）。max_parallel=1 なので 1 回の run は 1 試行ずつ投入する。
    // 全試行が記録されるまで再実行する = resume（記録済みは resumed_skipped に数えられる）。
    let mut recorded = 0;
    for _ in 0..400 {
        let rep = run(&m, &i, &mk(), &opts).expect("run");
        recorded = rep.executed + rep.resumed_skipped;
        if recorded == 3 {
            break;
        }
        sleep(Duration::from_millis(50));
    }
    assert_eq!(recorded, 3);
    let lg = Ledger::open(&out).expect("ledger");
    let mh = m.compute_sha256().expect("hash");
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        let r = lg
            .get(&TrialKey {
                manifest_sha256: mh.clone(),
                problem_id: "p0".into(),
                arm: c,
                trial_no: 1,
                rep: 1,
            })
            .expect("row");
        assert_eq!(r.state, LedgerState::Done);
        assert!(r.run_id.as_deref().is_some_and(|s| s.contains("pgid:")));
        assert_eq!(r.routing["mode"], "none");
        let log = fs::read_to_string(out.join(&r.workspace).join("started.log")).expect("log");
        assert_eq!(log.lines().count(), 1, "started once");
    }
    // 完了後の再実行は何も起動しない。
    run(&m, &i, &mk(), &opts).expect("rerun");
    let started: usize = lg
        .rows()
        .map(|r| {
            fs::read_to_string(out.join(&r.workspace).join("started.log"))
                .map_or(0, |s| s.lines().count())
        })
        .sum();
    assert_eq!(started, 3);
}

/// 実測: 実際に渡した `--model` と実行ファイル名を残す（manifest の宣言名ではない）。
#[test]
fn observed_run_comes_from_the_launched_command_not_the_manifest() {
    struct WithModel(PathBuf);
    impl CommandBuilder for WithModel {
        fn build(&self, _req: &TrialRequest) -> HarnessCommand {
            HarnessCommand {
                program: self.0.clone(),
                args: vec!["--model".into(), "actually-run".into()],
                env: vec![],
            }
        }
    }
    let t = h("#!/bin/sh
echo hello
");
    let x = HarnessExecutor::new(t.state.clone(), Box::new(WithModel(t.stub.clone())));
    let r = x.submit(&t.req(600), "campaign-obs").expect("submit");
    let _ = wait_done(&x, &r);
    let seen = x.observed(&r).expect("observed");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].model.as_deref(), Some("actually-run"));
    assert_ne!(seen[0].model.as_deref(), Some(t.cfg.model.as_str()));
    assert_eq!(seen[0].harness.as_deref(), Some("stub.sh"));
}
