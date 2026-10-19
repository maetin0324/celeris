//! P1-4: (a) A の実行ファイルは preflight が解決した物をそのまま起動する（PATH 上の別の claude に流れない）。
//! (b) 実 process の A（stub）を `watch` で追跡し、watch の停止・再起動後も同じ試行へ再接続して二重投入せず、
//! 上限到達で停止を確認できる。無課金（stub のみ。外部 LLM・network なし）。時間は gate file と出来事待ちで進める。
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use celeris_campaign::{
    executor::{ExecStatus, Executor, TrialRequest},
    harness_exec::{BudgetGuard, ClaudeCodeBuilder, HarnessExecutor},
    ledger::Ledger,
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, TokenBasis, manifest_hash,
    },
    preflight::{resolve_executable, resolve_program_in},
    runner::RunOptions,
    watch::{Clock, WatchEnd, WatchOptions, watch},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

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

fn cfg(id: ConfigId) -> Config {
    Config {
        id,
        harness: "claude-code".into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p".into(),
        routing: None,
    }
}

// --- (a) 実行ファイルの固定 ---------------------------------------------------------------------

/// `--version` に `<tag> 1.2.3` を返し、それ以外は自分が走った印を `$MARKS/<tag>` に残す stub。
fn stub_script(tag: &str, marks: &Path) -> String {
    format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{tag} 1.2.3'; exit 0; fi\necho ran >> '{}/{tag}'\necho '{{\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}'\n",
        marks.display()
    )
}

struct Pin {
    _d: tempfile::TempDir,
    marks: PathBuf,
    ws: PathBuf,
    state: PathBuf,
    decoy_dir: PathBuf,
    stub: PathBuf,
}

fn pin() -> Pin {
    let d = tempfile::tempdir().expect("dir");
    let r = d.path();
    let (marks, ws, state) = (r.join("marks"), r.join("ws"), r.join("state"));
    let (decoy_dir, stub_dir) = (r.join("path-bin"), r.join("stub-bin"));
    for p in [&marks, &ws, &state, &decoy_dir, &stub_dir] {
        fs::create_dir_all(p).expect("mkdir");
    }
    // PATH 上の別の claude（走ったら DECOY の印が残る）と、--a-program で指す stub。
    write_executable(&decoy_dir.join("claude"), &stub_script("DECOY", &marks));
    let stub = stub_dir.join("claude-stub");
    write_executable(&stub, &stub_script("STUB", &marks));
    Pin {
        marks,
        ws,
        state,
        decoy_dir,
        stub,
        _d: d,
    }
}

fn run_once(p: &Pin, builder: ClaudeCodeBuilder, key: &str) -> Value {
    let c = cfg(ConfigId::A);
    let req = TrialRequest {
        campaign_id: "c",
        manifest_sha256: "m",
        config: &c,
        problem_id: "p0",
        prompt: "SECRET-LOOKING PROMPT TEXT",
        trial_no: 1,
        rep: 1,
        workspace: &p.ws,
        kb: &p.ws,
        timeout_secs: 60,
        budget: Cap {
            max_tokens: 1_000_000,
            max_wall_secs: u64::MAX,
        },
    };
    let x = HarnessExecutor::new(p.state.clone(), Box::new(builder));
    let t = x.submit(&req, key).expect("submit");
    for _ in 0..400 {
        if matches!(x.poll(&t).expect("poll"), ExecStatus::Done(_)) {
            let b = fs::read(p.state.join(key).join("launch.json")).expect("launch.json");
            return serde_json::from_slice(&b).expect("json");
        }
        sleep(Duration::from_millis(25));
    }
    panic!("stub did not finish");
}

#[test]
fn explicit_a_program_is_the_one_checked_and_launched_even_with_another_claude_on_path() {
    let p = pin();
    let path_var = std::env::join_paths([&p.decoy_dir]).expect("PATH");
    // preflight と同じ解決: --a-program の stub が選ばれ、版も stub のもの。
    let resolved = resolve_program_in(&p.stub, Some(&path_var)).expect("resolve");
    assert_eq!(resolved.path, fs::canonicalize(&p.stub).expect("canon"));
    assert_eq!(resolved.version, "STUB 1.2.3");
    // 実起動は、その解決結果から作った builder（PATH を引き直さない）。
    let launch = run_once(
        &p,
        ClaudeCodeBuilder::from_resolved(&resolved),
        "campaign-pin1",
    );
    assert_eq!(launch["program"], json!(resolved.path));
    assert_eq!(launch["version"], "STUB 1.2.3");
    let argv = launch["argv"].as_array().expect("argv");
    assert!(argv.contains(&json!("--permission-mode")), "{argv:?}");
    assert!(
        argv.contains(&json!("<prompt 26 bytes>")) && !launch.to_string().contains("SECRET"),
        "prompt is recorded by length only: {launch}"
    );
    assert!(p.marks.join("STUB").exists(), "the pinned stub ran");
    assert!(!p.marks.join("DECOY").exists(), "the PATH claude never ran");
}

#[test]
fn default_a_program_resolves_the_same_executable_for_check_and_launch() {
    let p = pin();
    // 未指定の既定は `claude` を PATH で引く: 検査（resolve）と実起動は同じ path・版になる。
    let path_var = std::env::join_paths([&p.decoy_dir]).expect("PATH");
    let resolved = resolve_program_in(Path::new("claude"), Some(&path_var)).expect("resolve");
    assert_eq!(
        resolved.path,
        fs::canonicalize(p.decoy_dir.join("claude")).expect("canon")
    );
    assert_eq!(resolved.version, "DECOY 1.2.3");
    let launch = run_once(
        &p,
        ClaudeCodeBuilder::from_resolved(&resolved),
        "campaign-pin2",
    );
    assert_eq!(launch["program"], json!(resolved.path));
    assert_eq!(launch["version"], "DECOY 1.2.3");
    assert!(p.marks.join("DECOY").exists() && !p.marks.join("STUB").exists());
}

#[test]
fn unresolvable_or_versionless_program_is_refused() {
    let p = pin();
    let path_var = std::env::join_paths([&p.decoy_dir]).expect("PATH");
    assert!(resolve_executable(Path::new("nope"), Some(&path_var)).is_err());
    assert!(resolve_executable(Path::new("claude"), None).is_err());
    let silent = p.decoy_dir.join("silent");
    write_executable(&silent, "#!/bin/sh\nexit 0\n");
    assert!(
        resolve_program_in(&silent, None).is_err(),
        "no version output"
    );
}

// --- (b) watch の停止・再起動 ---------------------------------------------------------------------

/// 注入した時計。間隔 `d` を実際には待たず、短い譲り（1ms）だけで進める。回数でなく出来事（外部 process の
/// 停止確認）で終わるよう、反復上限は大きく取り、保険の壁時計上限（panic）だけを置く。
struct StepClock {
    started: std::time::Instant,
    limit: Duration,
}
impl StepClock {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            limit: Duration::from_secs(300),
        }
    }
}
impl Clock for StepClock {
    fn sleep(&self, _d: Duration) {
        assert!(self.started.elapsed() < self.limit, "watch did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn campaign(i: &Input, per_case: u64) -> CampaignManifest {
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "pin-watch".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: vec!["p0".into()],
        configs: vec![cfg(ConfigId::A), cfg(ConfigId::B), cfg(ConfigId::C)],
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: TokenBasis::All,
            max_tokens: 100_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 8,
            per_case: Cap {
                max_tokens: per_case,
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
        sha256: String::new(),
    }
}

fn pid_gone(pid: &str) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |s| {
        s[s.rfind(')').unwrap_or(0) + 1..]
            .trim_start()
            .starts_with('Z')
    })
}

#[test]
fn watch_stop_and_restart_reattaches_without_resubmit_and_stops_at_the_cap() {
    let d = tempfile::tempdir().expect("dir");
    let r = d.path();
    let (gate, launches, pids, printed) = (
        r.join("GATE"),
        r.join("launches"),
        r.join("pids"),
        r.join("printed"),
    );
    let line = |id: &str, n: u64| {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","usage":{{"input_tokens":{n},"output_tokens":5}}}}}}"#
        )
    };
    // 起動 → 少し消費（100）→ gate が開くまで待つ → 上限超過の消費（5000）→ 長い子を残して待つ。
    let stub = r.join("stub.sh");
    write_executable(
        &stub,
        &format!(
            "#!/bin/sh\necho $$ >> '{pids}'\necho x >> '{launches}'\necho '{l0}'\necho x >> '{printed}'\nwhile [ ! -e '{gate}' ]; do sleep 0.05; done\necho '{l1}'\nsleep 300 &\nwait\n",
            pids = pids.display(),
            launches = launches.display(),
            printed = printed.display(),
            gate = gate.display(),
            l0 = line("m0", 100),
            l1 = line("m1", 5000),
        ),
    );
    let i = input(1);
    let m = campaign(&i, 1_000);
    let out = r.join("out");
    let opts = RunOptions {
        global_ledger: None,
        out: out.clone(),
        ..RunOptions::default()
    };
    let builder = || ClaudeCodeBuilder {
        program: stub.clone(),
        version: Some("stub 1".into()),
    };
    let w = |max_iterations| WatchOptions {
        interval_secs: 1,
        max_iterations,
    };
    let count = |p: &Path| fs::read_to_string(p).map_or(0, |s| s.lines().count());

    // 1 回目の watch（監視 thread なし = CLI が落ちた後と同じ）: 3 試行を投入し、上限回数で戻る。
    let x1 = HarnessExecutor::new(out.join("harness-state"), Box::new(builder()));
    let r1 = watch(&m, &i, &x1, &opts, w(2), &StepClock::new()).expect("watch 1");
    assert_eq!(r1.end, WatchEnd::MaxIterations);
    assert_eq!(count(&launches), 3, "one process per trial");
    // 高負荷では起動が遅れ m0 の出力がまだ観測されていないことがある。
    // 3 つとも m0 を出力するまで待ち（出来事待ち）、もう一度観測してから止める。
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while count(&printed) < 3 {
        assert!(std::time::Instant::now() < deadline, "m0 not printed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let r1b = watch(&m, &i, &x1, &opts, w(2), &StepClock::new()).expect("watch 1b");
    assert_eq!(r1b.end, WatchEnd::MaxIterations);
    let rows_before: Vec<_> = Ledger::open(&out)
        .expect("ledger")
        .rows()
        .cloned()
        .collect();
    assert_eq!(rows_before.len(), 3);
    assert!(rows_before.iter().all(|r| !r.state.is_terminal()));
    drop(x1);

    // 上限を超える消費が起きる（監視が止まっている間）。process はまだ生きている。
    fs::write(&gate, b"1").expect("gate");

    // 再起動: 新しい executor（予算 guard 付き）が同じ state dir から同じ process を追う。
    let x2 = HarnessExecutor::new(out.join("harness-state"), Box::new(builder()))
        .with_budget_guard(BudgetGuard {
            basis: TokenBasis::All,
            interval: Duration::from_millis(20),
        });
    let clock = StepClock::new();
    let r2 = watch(
        &m,
        &i,
        &x2,
        &opts,
        WatchOptions {
            interval_secs: 1,
            max_iterations: u32::MAX,
        },
        &clock,
    )
    .expect("watch 2");
    assert_eq!(r2.end, WatchEnd::Settled, "{r2:?}");
    assert_eq!(count(&launches), 3, "restart never resubmits");

    // 台帳は全て終端で、観測した消費は 0 に戻っていない（上限 1000 を超えた分も記録される）。
    let lg = Ledger::open(&out).expect("ledger");
    assert!(lg.rows().all(|r| r.state.is_terminal()));
    let recs: Vec<Value> = fs::read_to_string(out.join("trials.jsonl"))
        .expect("trials")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert_eq!(recs.len(), 3);
    for rec in &recs {
        assert_eq!(rec["outcome"], "cancelled", "{rec}");
        // 再起動前の消費（m0: 105）も、再起動後の消費（m1: 5005）も合算して残る。
        assert_eq!(rec["tokens"], 5_110, "observed usage kept: {rec}");
    }
    // 停止の確認: 3 つの process は全て消えている（process group ごと kill）。
    let alive: Vec<String> = fs::read_to_string(&pids)
        .expect("pids")
        .lines()
        .filter(|p| !pid_gone(p))
        .map(String::from)
        .collect();
    assert!(alive.is_empty(), "still alive: {alive:?}");
}
