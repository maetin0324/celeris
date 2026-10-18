//! B/C の能力 probe は、測定専用 daemon の通常の worker 起動経路で走る（CLI の環境で代用しない）。
//!
//! 実 `celeris` binary を、`env_clear` で環境を絞った一時 daemon（一時 DB・loopback・偽 harness だけ）として起動し、
//! probe は通常の `POST /tasks`（`run_in_daemon`・実 HTTP）で検査 task を投入する。daemon の dispatcher が通常どおり
//! worker（`celeris-campaign cap-worker`。`[adapters.fake] command`）を起こし、その環境（daemon の環境 + adapter env +
//! run ごとの env）の中で検査が走る。検査先 identity は daemon の `GET /config` が答えたもの。LLM・有料 job・本番は使わない。
//!
//! この試験 process（= CLI 側）には、daemon とは別の toolchain dir と変数を持たせる。daemon にだけある toolchain は通り、
//! CLI にあって daemon に無い toolchain・変数・socket は該当 arm の `env_failure` になる。

use std::{
    fs,
    io::Write,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

use celeris_campaign::{
    cap_probe::{self, CapEnv, DaemonRef, ProbeReport},
    cases::CaseSet,
    celeris_exec::Transport,
    curl_transport::CurlTransport,
    manifest::ConfigId,
};
use serde_json::{Value, json};

const TOKEN_ENV: &str = "CAP_PROBE_REAL_DAEMON_TOKEN";
const SECRET: &str = "super-secret-value-must-not-leak";

fn campaign_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../campaign")
}

fn campaign_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let p = Path::new(env!("CARGO_BIN_EXE_celeris"))
            .parent()
            .expect("bin dir")
            .join("celeris-campaign");
        if !p.exists() {
            let st = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
                .args([
                    "build",
                    "-p",
                    "celeris-campaign",
                    "--bin",
                    "celeris-campaign",
                ])
                .status()
                .expect("cargo build");
            assert!(st.success(), "cargo build celeris-campaign");
        }
        assert!(p.exists(), "{} was not built", p.display());
        p
    })
    .clone()
}

/// 実行可能 file は別 process で書く（ETXTBSY 回避）。
fn write_executable(path: &Path, body: &str) {
    let mut c = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn");
    c.stdin
        .take()
        .expect("stdin")
        .write_all(body.as_bytes())
        .expect("write");
    assert!(c.wait().expect("wait").success());
}

/// 道具の dir（`python3` の stub。`Python 3 mark=<CAP_MARK>` を出す）。
fn tools(mark_note: &str) -> PathBuf {
    let d = tempfile::tempdir().expect("tools").keep();
    write_executable(
        &d.join("python3"),
        &format!("#!/bin/sh\necho \"Python 3 mark=$CAP_MARK {mark_note}\"\n"),
    );
    d
}

/// この試験 process = CLI 側の環境。daemon とは別の道具 dir と変数を持たせる（daemon の環境の代用にしないことの対照）。
fn cli_tools() -> &'static Path {
    static T: OnceLock<PathBuf> = OnceLock::new();
    T.get_or_init(|| {
        let d = tools("cli");
        let path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: この試験 file の全試験が最初に呼ぶ（OnceLock の中で 1 回だけ）。他は環境を書き換えない。
        unsafe {
            std::env::set_var("PATH", format!("{}:{path}", d.display()));
            std::env::set_var("CAP_MARK", "cli-mark");
            std::env::set_var(TOKEN_ENV, "real-daemon-token");
        }
        d
    })
}

struct Daemon {
    child: Child,
    _dir: tempfile::TempDir,
    url: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 実 `celeris` を `env_clear` + `env` だけの環境で起動する。worker（検査 process）は dispatcher がこの daemon の環境から起こす。
fn start_daemon(env: &[(&str, &str)]) -> Daemon {
    start_daemon_with(env, &[])
}

/// `claude_env` は `[adapters.claude_code].env`（実 B/C harness の adapter 環境）。codex は adapter env なし。
fn start_daemon_with(env: &[(&str, &str)], claude_env: &[(&str, &str)]) -> Daemon {
    cli_tools();
    let dir = tempfile::tempdir().expect("daemon dir");
    let h = dir.path();
    fs::create_dir_all(h.join("ws")).expect("ws");
    fs::write(h.join("api.token"), "real-daemon-token\n").expect("token");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("port")
        .local_addr()
        .expect("addr")
        .port();
    fs::write(
        h.join("config.toml"),
        format!(
            r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 100

[api]
listen = "127.0.0.1:{port}"
token_file = "api.token"

[adapters.fake]
command = ["{bin}", "cap-worker"]

[adapters.claude_code.env]
{claude_env}

[llm_sources.cap-probe]
kind = "fake"
accounts = ["probe"]
models = ["fake"]

# 実 B/C harness（起動はしない。adapter env の持ち主）。
[harnesses.claude-code]
kind = "claude_code"
api_formats = ["fake"]
source_kinds = ["fake"]

[harnesses.codex]
kind = "codex"
api_formats = ["fake"]
source_kinds = ["fake"]

# 検査 harness: 無 LLM の cap-worker を、対応する実 harness の adapter 環境で起動する。
[harnesses.cap-probe-claude-code]
kind = "fake"
env_of = "claude-code"
api_formats = ["fake"]
source_kinds = ["fake"]

[harnesses.cap-probe-codex]
kind = "fake"
env_of = "codex"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.cap-probe-claude-code]
candidates = [{{ source = "cap-probe", model = "fake" }}]

[route_defaults.cap-probe-codex]
candidates = [{{ source = "cap-probe", model = "fake" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [
  {{ genre = "cap-probe-claude-code", harness = "cap-probe-claude-code", required_capabilities = [] }},
  {{ genre = "cap-probe-codex", harness = "cap-probe-codex", required_capabilities = [] }},
]
"#,
            bin = campaign_bin().display(),
            claude_env = claude_env
                .iter()
                .map(|(k, v)| format!("{k} = \"{v}\""))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .expect("config");
    let log = fs::File::create(h.join("daemon.log")).expect("log");
    let mut c = Command::new(env!("CARGO_BIN_EXE_celeris"));
    c.args(["--config"])
        .arg(h.join("config.toml"))
        .args(["--log-format", "text"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().expect("log")))
        .stderr(Stdio::from(log));
    for (k, v) in env {
        c.env(k, v);
    }
    let d = Daemon {
        child: c.spawn().expect("spawn celeris"),
        url: format!("http://127.0.0.1:{port}"),
        _dir: dir,
    };
    let t = CurlTransport::new(&d.url, TOKEN_ENV).expect("transport");
    let start = Instant::now();
    loop {
        if t.request("GET", "/api/v1/config", None)
            .is_ok_and(|r| r.status == 200)
        {
            return d;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "daemon did not come up: {}",
            fs::read_to_string(d._dir.path().join("daemon.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn k4() -> celeris_campaign::cases::Case {
    CaseSet::load(
        &campaign_root().join("cases"),
        &campaign_root().join("fixtures/grader-private"),
    )
    .expect("cases")
    .get("k4-cmp4-validity-analysis")
    .expect("k4")
    .case
    .clone()
}

fn cap_env(v: Value) -> CapEnv {
    serde_json::from_value(json!({"schema":"celeris.campaign-cap-env/1","a":v.clone(),"worker":v}))
        .expect("cap env")
}

/// 検査先 identity。daemon の `GET /config`（preflight の identity 照合と同じ答え）から取る。
fn identity(d: &Daemon) -> DaemonRef {
    let t = CurlTransport::new(&d.url, TOKEN_ENV).expect("transport");
    let r = t
        .request("GET", "/api/v1/config", None)
        .expect("GET /config");
    let id = &r.body["identity"];
    DaemonRef {
        pid: id["pid"].as_u64().expect("identity.pid") as u32,
        instance_id: id["instance_id"].as_str().map(String::from),
        release: id["release"].as_str().map(String::from),
        probe_task_id: None,
    }
}

fn probe(d: &Daemon, env: &CapEnv, arm: ConfigId) -> ProbeReport {
    probe_harnesses(d, env, arm, &["claude-code"])
}

fn probe_harnesses(d: &Daemon, env: &CapEnv, arm: ConfigId, hs: &[&str]) -> ProbeReport {
    let hs: Vec<String> = hs.iter().map(|h| h.to_string()).collect();
    let t = CurlTransport::new(&d.url, TOKEN_ENV).expect("transport");
    cap_probe::probe_worker(env, &identity(d), arm, &k4(), &hs, &|spec| {
        cap_probe::run_in_daemon(
            &t,
            spec,
            Duration::from_secs(90),
            Duration::from_millis(100),
        )
    })
}

fn failing(rep: &ProbeReport) -> Vec<String> {
    rep.results
        .iter()
        .filter(|r| !r.ok)
        .map(|r| r.id.clone())
        .collect()
}

fn base_path(tools: &Path) -> String {
    format!("{}:/usr/bin:/bin", tools.display())
}

/// daemon にだけある toolchain と変数（CLI の PATH・変数とは別）で通る。検査先 identity と検査 task が報告に残る。
#[test]
fn daemon_only_toolchain_and_variable_pass_through_the_real_worker_path() {
    let t = tools("daemon");
    let env =
        cap_env(json!({"env": [["CAP_MARK", "daemon-mark"], ["PATH", t.display().to_string()]]}));
    assert!(
        !std::env::var("PATH")
            .unwrap_or_default()
            .contains(&t.display().to_string())
    );
    let d = start_daemon(&[
        ("PATH", &base_path(&t)),
        ("CAP_MARK", "daemon-mark"),
        ("HOME", "/home/daemon-home"),
        ("API_TOKEN", SECRET),
    ]);
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = probe(&d, &env, arm);
        assert!(rep.ok, "{:?}", rep.results);
        assert_eq!(rep.paid_calls_made, 0);
        let id = rep.daemon.as_ref().expect("target identity");
        assert_eq!(
            id.pid,
            d.child.id(),
            "identity answers for the spawned daemon"
        );
        assert!(id.instance_id.as_deref().is_some_and(|s| !s.is_empty()));
        assert!(id.probe_task_id.as_deref().is_some_and(|s| !s.is_empty()));
        // 能力結果と識別だけ。秘密・ホーム・環境変数の値は報告にも task の event にも出ない。
        let json = serde_json::to_string(&rep).expect("json");
        assert!(
            !json.contains(SECRET)
                && !json.contains("daemon-home")
                && !json.contains("daemon-mark"),
            "{json}"
        );
        let task = id.probe_task_id.as_deref().expect("task");
        let ev = CurlTransport::new(&d.url, TOKEN_ENV)
            .expect("t")
            .request("GET", &format!("/api/v1/tasks/{task}/events"), None)
            .expect("events");
        let ev = ev.body.to_string();
        assert!(!ev.contains(SECRET) && !ev.contains("daemon-home"), "{ev}");
    }
}

/// CLI の PATH・変数は期待どおりでも、daemon の worker に toolchain・変数が無ければ該当 arm の env_failure。
#[test]
fn daemon_missing_toolchain_or_variable_is_env_failure_even_if_cli_has_it() {
    let cli = cli_tools().to_path_buf();
    let env =
        cap_env(json!({"env": [["CAP_MARK", "cli-mark"], ["PATH", cli.display().to_string()]]}));
    // CLI（この process）は期待の PATH 要素も CAP_MARK も持つ。daemon の worker は持たない。
    assert!(
        std::env::var("PATH")
            .unwrap_or_default()
            .contains(&cli.display().to_string())
    );
    assert_eq!(std::env::var("CAP_MARK").as_deref(), Ok("cli-mark"));
    let d = start_daemon(&[("PATH", "/usr/bin:/bin")]);
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = probe(&d, &env, arm);
        assert!(!rep.ok);
        assert_eq!(rep.failure_class, Some("env_failure"));
        assert_eq!(rep.arm, arm);
        assert_eq!(rep.paid_calls_made, 0);
        let f = failing(&rep);
        assert!(f.contains(&"claude-code/worker-env".to_string()), "{f:?}");
        // 期待 PATH 要素と CAP_MARK の 2 件（worker の python3 は host の /usr/bin のものが見えるので、道具の有無でなく
        // 環境の食い違いで落ちる）。値は報告に出ない。
        assert_eq!(
            f,
            vec!["claude-code/worker-env", "claude-code/worker-env"],
            "{f:?}"
        );
        let json = serde_json::to_string(&rep).expect("json");
        assert!(!json.contains("cli-mark"), "{json}");
        assert!(
            rep.daemon
                .as_ref()
                .expect("identity")
                .probe_task_id
                .is_some()
        );
    }
}

/// 承認済み socket と mount: daemon の worker から見えれば通り、無ければ該当 arm の env_failure。
#[test]
fn daemon_socket_and_mount_are_checked_from_the_worker() {
    let t = tools("daemon");
    let work = tempfile::tempdir().expect("work");
    // sockaddr_un の長さ上限があるので、socket は短い path に置く。
    let sock_dir = tempfile::Builder::new()
        .prefix("cp")
        .tempdir_in("/dev/shm")
        .expect("sock dir");
    let sock_dir = sock_dir.path().to_path_buf();
    let sock = sock_dir.join("a.sock");
    let _l = UnixListener::bind(&sock).expect("bind");
    let mount = work.path().join("prefix");
    fs::create_dir_all(&mount).expect("mount");
    let d = start_daemon(&[("PATH", &base_path(&t))]);
    let ok = cap_env(
        json!({"sockets": [sock], "ro_mounts": [mount], "env": [["PATH", t.display().to_string()]]}),
    );
    assert!(probe(&d, &ok, ConfigId::B).ok);
    let absent = sock_dir.join("absent.sock");
    let gone = work.path().join("absent-prefix");
    let bad = cap_env(
        json!({"sockets": [absent], "ro_mounts": [gone], "env": [["PATH", t.display().to_string()]]}),
    );
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = probe(&d, &bad, arm);
        assert_eq!(rep.failure_class, Some("env_failure"));
        assert_eq!(
            failing(&rep),
            vec![
                format!("claude-code/toolchain:{}", gone.display()),
                format!("claude-code/socket:{}", absent.display())
            ]
        );
    }
}

/// 検査 task が daemon で完了しない（接続できない・検査 harness が無い等）なら、検査は通ったことにならず env_failure
/// （LLM 呼出し前）。
#[test]
fn unreachable_daemon_is_env_failure_not_a_pass() {
    cli_tools();
    let env = cap_env(json!({}));
    let t = CurlTransport::new("http://127.0.0.1:9", TOKEN_ENV).expect("transport");
    let target = DaemonRef {
        pid: 1,
        ..DaemonRef::default()
    };
    let rep = cap_probe::probe_worker(
        &env,
        &target,
        ConfigId::B,
        &k4(),
        &["claude-code".into()],
        &|spec| {
            cap_probe::run_in_daemon(&t, spec, Duration::from_secs(2), Duration::from_millis(50))
        },
    );
    assert_eq!(rep.failure_class, Some("env_failure"));
    assert!(
        rep.results
            .iter()
            .any(|r| r.id == "claude-code/worker-run" && !r.ok),
        "{:?}",
        rep.results
    );
}

/// 検査は arm の実 harness の adapter 環境で走る。daemon の process 環境が同じでも、harness の adapter env が toolchain を
/// 足す（その harness だけ通る）・壊す（その harness だけ env_failure）。fake harness の環境では代用できない。
#[test]
fn probe_runs_under_each_real_harness_adapter_env() {
    let daemon_tools = tools("daemon");
    let adapter_tools = tools("adapter");
    let env = cap_env(json!({"env": [["PATH", adapter_tools.display().to_string()]]}));
    // 足す: process の PATH には無く、claude-code の adapter env だけが道具 dir を足す。
    let d = start_daemon_with(
        &[("PATH", "/usr/bin:/bin")],
        &[(
            "PATH",
            &format!("{}:/usr/bin:/bin", adapter_tools.display()),
        )],
    );
    let ok = probe_harnesses(&d, &env, ConfigId::B, &["claude-code"]);
    assert!(ok.ok, "{:?}", ok.results);
    let rep = probe_harnesses(&d, &env, ConfigId::B, &["claude-code", "codex"]);
    assert_eq!(rep.failure_class, Some("env_failure"));
    let f = failing(&rep);
    assert!(
        !f.is_empty() && f.iter().all(|id| id.starts_with("codex/")),
        "only codex (no adapter env) fails: {f:?}"
    );
    let id = rep.daemon.expect("identity");
    assert_eq!(id.pid, d.child.id());
    assert_eq!(
        id.probe_task_id.as_deref().map(|s| s.split(',').count()),
        Some(2)
    );
    // 壊す: process の PATH は正しいが、claude-code の adapter env が道具 dir を落とす。
    let env = cap_env(json!({"env": [["PATH", daemon_tools.display().to_string()]]}));
    let d = start_daemon_with(
        &[("PATH", &base_path(&daemon_tools))],
        &[("PATH", "/usr/bin:/bin")],
    );
    let bad = probe_harnesses(&d, &env, ConfigId::C, &["claude-code"]);
    assert_eq!(bad.failure_class, Some("env_failure"));
    assert_eq!(failing(&bad), vec!["claude-code/worker-env"]);
    assert_eq!(bad.paid_calls_made, 0);
}
