//! A 腕の隔離（bubblewrap）と token 予算の実行中強制（ADR 2026-10-17-mlsys-pilot2-fixes D2/D3）。
//! 隔離の構成は純粋関数で、実 bwrap の確認は userns が作れる host でだけ走らせる（作れない sandbox では skip。
//! `CELERIS_USERNS_TESTS=require` なら失敗にする。ADR-0126 の流儀）。
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use celeris_campaign::{
    aggregate::Outcome,
    executor::{ExecResult, ExecStatus, Executor, TaskRef, TrialRequest},
    harness_exec::{
        BudgetGuard, CommandBuilder, HarnessCommand, HarnessExecutor, Sandbox, TRIAL_MARKER,
        parse_claude_stream_usage,
    },
    manifest::{Cap, Config, ConfigId, TokenBasis},
};

fn write_executable(path: &Path, contents: &str) {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn");
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
    fn build(&self, _: &TrialRequest) -> HarnessCommand {
        HarnessCommand {
            program: self.0.clone(),
            args: vec![],
            env: vec![],
        }
    }
}

struct H {
    _d: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
    ws: PathBuf,
    kb: PathBuf,
    stub: PathBuf,
    cfg: Config,
}

fn h(script: &str) -> H {
    let d = tempfile::tempdir().expect("dir");
    let root = d.path().to_path_buf();
    let (ws, kb, state) = (root.join("ws"), root.join("kb"), root.join("st"));
    for p in [&ws, &kb, &state] {
        fs::create_dir_all(p).expect("mkdir");
    }
    let stub = root.join("stub.sh");
    write_executable(&stub, script);
    let cfg = Config {
        id: ConfigId::A,
        harness: "claude-code".into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p".into(),
        routing: None,
    };
    H {
        _d: d,
        root,
        state,
        ws,
        kb,
        stub,
        cfg,
    }
}

impl H {
    fn req(&self, max_tokens: u64) -> TrialRequest<'_> {
        TrialRequest {
            campaign_id: "c",
            manifest_sha256: "m",
            config: &self.cfg,
            problem_id: "p0",
            prompt: "x",
            trial_no: 1,
            rep: 1,
            workspace: &self.ws,
            kb: &self.kb,
            timeout_secs: 600,
            budget: Cap {
                max_tokens,
                max_wall_secs: u64::MAX,
            },
            variant: None,
        }
    }
    fn exec(&self) -> HarnessExecutor {
        HarnessExecutor::new(self.state.clone(), Box::new(Stub(self.stub.clone())))
    }
}

fn wait_done(x: &HarnessExecutor, t: &TaskRef) -> ExecResult {
    for _ in 0..400 {
        if let ExecStatus::Done(r) = x.poll(t).expect("poll") {
            return r;
        }
        sleep(Duration::from_millis(25));
    }
    panic!("harness did not finish");
}

const LINE: &str = r#"{"type":"assistant","message":{"id":"m1","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0}}}"#;

#[test]
fn live_usage_counts_each_message_once() {
    let two = format!("{LINE}\n{LINE}\n");
    let u = parse_claude_stream_usage(&two).expect("usage");
    assert_eq!((u.input, u.output, u.cache_read), (10, 5, Some(1000)));
    assert_eq!(u.count(TokenBasis::All), Some(1015));
    let other = LINE.replace("\"m1\"", "\"m2\"");
    let u = parse_claude_stream_usage(&format!("{LINE}\n{other}\n")).expect("usage");
    assert_eq!(u.count(TokenBasis::All), Some(2030));
    assert!(parse_claude_stream_usage("not json\n").is_none());
}

/// 上限を超えたら、poll の間隔を待たずに process group ごと止め、cancelled・観測した tokens を返す。
#[test]
fn a_arm_is_stopped_while_running_when_it_passes_the_token_cap() {
    let t = h(&format!(
        "#!/bin/sh\necho '{LINE}'\necho '{LINE}'\nsleep 300 &\necho $! > child.pid\nwait\n"
    ));
    let x = t.exec().with_budget_guard(BudgetGuard {
        basis: TokenBasis::All,
        interval: Duration::from_millis(20),
    });
    let r = x.submit(&t.req(500), "campaign-cap1").expect("submit");
    // 監視 thread が止める。ここでは poll を回すだけ（kill は poll か thread のどちらでも起きる）。
    let res = wait_done(&x, &r);
    assert_eq!(res.outcome, Outcome::Cancelled);
    assert_eq!(
        res.tokens.and_then(|u| u.count(TokenBasis::All)),
        Some(1015),
        "重複行を数えない、観測した累積"
    );
    // 子 process も残らない。
    let pid = fs::read_to_string(t.ws.join("child.pid"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    for _ in 0..200 {
        let gone = pid.is_empty()
            || fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |s| {
                s[s.rfind(')').unwrap_or(0) + 1..]
                    .trim_start()
                    .starts_with('Z')
            });
        if gone {
            return;
        }
        sleep(Duration::from_millis(25));
    }
    panic!("child survived the budget kill");
}

#[test]
fn a_arm_under_the_cap_runs_to_the_end() {
    let t = h(&format!(
        "#!/bin/sh\necho '{LINE}'\necho '{{\"type\":\"result\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_input_tokens\":1000,\"cache_creation_input_tokens\":0}},\"total_cost_usd\":0.01}}'\n"
    ));
    let x = t.exec().with_budget_guard(BudgetGuard {
        basis: TokenBasis::All,
        interval: Duration::from_millis(20),
    });
    let r = x.submit(&t.req(5_000), "campaign-cap2").expect("submit");
    let res = wait_done(&x, &r);
    assert_eq!(res.outcome, Outcome::Passed);
    assert_eq!(
        res.tokens.and_then(|u| u.count(TokenBasis::All)),
        Some(1015)
    );
    // 実行中の使用量（runner が poll ごとに見る）も返る。
    assert!(x.usage(&r).expect("usage").is_some());
}

fn sb_for(root: &str) -> Sandbox {
    Sandbox {
        account_id: "lab".into(),
        auth_dir: "/acct/lab".into(),
        state_root: format!("{root}/trial-state").into(),
        toolchain: Default::default(),
    }
}

/// 隔離の構成: 見える path は固定で、運用者の file・他試行・環境変数は入らない。
#[test]
fn sandbox_exposes_only_the_workspace_trial_state_and_the_binary() {
    let sb = sb_for("/out");
    let ws = Path::new("/out/trials/A/k3/rep1/workspace");
    let st = Path::new("/out/trial-state/c/A/p0/rep1");
    let real = Path::new("/home/op/.local/share/claude/versions/2.1");
    let a = sb.args(ws, st, real, &[]);
    let mut binds = BTreeSet::new();
    let mut envs = BTreeSet::new();
    for w in a.windows(3) {
        if w[0] == "--bind" || w[0] == "--ro-bind" {
            binds.insert(w[1].clone());
            if w[1] != "/acct/lab/.credentials.json" {
                assert_eq!(w[1], w[2], "same path inside");
            }
        }
        if w[0] == "--setenv" {
            envs.insert(w[1].clone());
        }
    }
    let allowed_system = ["/usr", "/etc", "/bin", "/sbin", "/lib", "/lib32", "/lib64"];
    for b in &binds {
        assert!(
            allowed_system.contains(&b.as_str())
                || b == "/out/trials/A/k3/rep1/workspace"
                || b == "/out/trial-state/c/A/p0/rep1"
                || b == "/acct/lab/.credentials.json"
                || b == "/home/op/.local/share/claude/versions/2.1",
            "unexpected mount {b}"
        );
    }
    // アカウント dir 全体は bind しない。認証材料は read-only で state dir の中に重ねる。
    assert!(!binds.contains("/acct/lab"));
    assert!(a.windows(3).any(|w| w
        == [
            "--ro-bind",
            "/acct/lab/.credentials.json",
            "/out/trial-state/c/A/p0/rep1/.credentials.json"
        ]));
    assert!(
        a.windows(2)
            .any(|w| w == ["CLAUDE_CONFIG_DIR", "/out/trial-state/c/A/p0/rep1"])
    );
    assert!(a.contains(&"--clearenv".to_string()));
    assert!(a.windows(2).any(|w| w == ["--tmpfs", "/tmp"]));
    assert_eq!(
        envs,
        ["CLAUDE_CONFIG_DIR", "HOME", "LANG", "PATH", "TERM"]
            .map(String::from)
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    for bad in [
        "/",
        "/home",
        "/local",
        "/work",
        "/out",
        "/out/trials",
        "/out/trial-state",
        "/tmp",
    ] {
        assert!(!binds.contains(bad), "{bad} must not be mounted");
    }
}

/// state dir は campaign / arm / case / repetition で決まり、同一試行では同じ path、別試行では互いに素。
#[test]
fn trial_state_dirs_are_stable_per_trial_and_disjoint_across_trials() {
    let t = h("#!/bin/sh\n");
    let sb = sb_for("/out");
    let base = t.req(1);
    let same = t.req(1);
    assert_eq!(sb.state_dir(&base), sb.state_dir(&same));
    let mut dirs = BTreeSet::new();
    dirs.insert(sb.state_dir(&base));
    let mut rep2 = t.req(1);
    rep2.rep = 2;
    dirs.insert(sb.state_dir(&rep2));
    let mut case2 = t.req(1);
    case2.problem_id = "p1";
    dirs.insert(sb.state_dir(&case2));
    let mut tricky = t.req(1);
    tricky.problem_id = "../p0";
    dirs.insert(sb.state_dir(&tricky));
    let mut b_cfg = t.cfg.clone();
    b_cfg.id = ConfigId::B;
    let mut arm_b = t.req(1);
    arm_b.config = &b_cfg;
    dirs.insert(sb.state_dir(&arm_b));
    assert_eq!(dirs.len(), 5);
    for d in &dirs {
        assert!(d.starts_with("/out/trial-state"), "{d:?}");
        assert!(!d.components().any(|c| c.as_os_str() == ".."), "{d:?}");
        // 別試行の bwrap 引数に、この試行以外の state dir は現れない。
        let a = sb.args(Path::new("/w"), d, Path::new("/bin/claude"), &[]);
        for other in dirs.iter().filter(|o| *o != d) {
            let o = other.to_string_lossy();
            assert!(
                !a.iter().any(|x| x.starts_with(o.as_ref())),
                "{o} visible in {d:?}"
            );
        }
    }
}

/// prepare は marker を書き、同一試行の再開では dir を保ち、他試行の marker・出所不明の状態は拒否する。
#[test]
fn prepare_writes_identity_marker_and_refuses_foreign_state() {
    let t = h("#!/bin/sh\n");
    let acct = t.root.join("acct");
    fs::create_dir_all(&acct).expect("acct");
    fs::write(acct.join(".credentials.json"), "{}").expect("cred");
    fs::write(acct.join("history.jsonl"), "old-session-id").expect("hist");
    let sb = Sandbox {
        account_id: "lab".into(),
        auth_dir: acct.clone(),
        state_root: t.root.join("trial-state"),
        toolchain: Default::default(),
    };
    let req = t.req(1);
    let d = sb.prepare(&req).expect("first prepare");
    let m: serde_json::Value =
        serde_json::from_slice(&fs::read(d.join(TRIAL_MARKER)).expect("marker")).expect("json");
    assert_eq!(m["case"], "p0");
    assert_eq!(m["rep"], 1);
    assert_eq!(m["arm"], "A");
    // 再開: 試行が書いた状態は保たれ、同じ dir が返る。
    fs::write(d.join("projects.txt"), "my-session").expect("write");
    assert_eq!(sb.prepare(&req).expect("resume"), d);
    assert_eq!(
        fs::read_to_string(d.join("projects.txt")).expect("kept"),
        "my-session"
    );
    // アカウント dir の履歴は state dir に現れない。
    assert!(!d.join("history.jsonl").exists());
    // 他試行の marker が居る dir は拒否する。
    let mut other = t.req(1);
    other.campaign_id = "other";
    let od = sb.state_dir(&other);
    fs::create_dir_all(&od).expect("od");
    fs::copy(d.join(TRIAL_MARKER), od.join(TRIAL_MARKER)).expect("plant");
    assert!(
        sb.prepare(&other)
            .expect_err("foreign")
            .contains("another trial")
    );
    // marker の無い既存状態（混入）も引き継がない。
    let mut dirty = t.req(1);
    dirty.rep = 9;
    let dd = sb.state_dir(&dirty);
    fs::create_dir_all(&dd).expect("dd");
    fs::write(dd.join("leftover"), "x").expect("leftover");
    assert!(
        sb.prepare(&dirty)
            .expect_err("dirty")
            .contains("no identity marker")
    );
}

fn bwrap_usable() -> bool {
    let ok = Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--unshare-pid",
            "--",
            "true",
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        assert_ne!(
            std::env::var("CELERIS_USERNS_TESTS").as_deref(),
            Ok("require"),
            "bubblewrap (user namespace) is required for this test"
        );
        eprintln!("skip: bwrap cannot create a namespace here");
    }
    ok
}

/// 実 bwrap: workspace の外の file は読めず、/tmp への書き込みは host に出ず、運用者の token 環境変数は見えない。
#[test]
fn real_bwrap_hides_everything_outside_the_workspace() {
    if !bwrap_usable() {
        return;
    }
    let t = h("#!/bin/sh
if cat \"$SECRET_FILE\" >/dev/null 2>&1; then echo read-ok > probe.txt; else echo read-denied > probe.txt; fi
echo \"token=${CELERIS_CAMPAIGN_TOKEN:-unset}\" >> probe.txt
echo leak > /tmp/leaked-by-a
ls -A \"$(dirname \"$PWD\")\" >> probe.txt
test -e \"$GRADER_DIR\" && echo grader-visible >> probe.txt
echo done >> probe.txt
");
    let secret = t.root.join("secret.txt");
    fs::write(&secret, "grader-private").expect("secret");
    let grader = t.root.join("grader-private");
    fs::create_dir_all(&grader).expect("grader");
    let acct = t.root.join("acct");
    fs::create_dir_all(&acct).expect("acct");
    // 環境変数は実行器の環境から渡す（A に漏れないことの確認）。
    unsafe {
        std::env::set_var("CELERIS_CAMPAIGN_TOKEN", "operator-secret");
        std::env::set_var("SECRET_FILE", &secret);
        std::env::set_var("GRADER_DIR", &grader);
    }
    fs::write(acct.join(".credentials.json"), "{}").expect("cred");
    let x = t.exec().with_sandbox(Sandbox {
        account_id: "lab".into(),
        auth_dir: acct,
        state_root: t.root.join("trial-state"),
        toolchain: Default::default(),
    });
    let r = x.submit(&t.req(u64::MAX), "campaign-sbx1").expect("submit");
    let res = wait_done(&x, &r);
    assert_eq!(res.outcome, Outcome::Passed);
    let probe = fs::read_to_string(t.ws.join("probe.txt")).expect("probe");
    assert!(probe.contains("read-denied"), "{probe}");
    assert!(probe.contains("token=unset"), "{probe}");
    assert!(!probe.contains("grader-visible"), "{probe}");
    // workspace の隣（他試行・grader-private・secret・実行器の状態 dir）は見えない。
    for hidden in ["grader-private", "secret.txt", "st", "kb"] {
        assert!(
            !probe.lines().any(|l| l == hidden),
            "{hidden} is visible next to the workspace: {probe}"
        );
    }
    assert!(probe.contains("done"), "{probe}");
    assert!(
        !Path::new("/tmp/leaked-by-a").exists(),
        "write to /tmp must not reach the host"
    );
    // 観測記録には A のアカウントが残る。
    let seen = x.observed(&r).expect("observed");
    assert_eq!(seen[0].source.as_deref(), Some("claude_oauth:lab"));
}
