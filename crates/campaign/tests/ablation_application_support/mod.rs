//! `ablation_application` の所有 fixture（無 LLM）。
//!
//! 1 つの [`Fixture`] が、一時 dir（DB・API token・org の種・workspace・HOME・スタブの状態）、loopback の port、
//! 実 `celeris` binary の daemon process（実 dispatcher の tick・実 `fake` worker adapter）と、daemon が起こした
//! スタブ process を所有し、`Drop` で daemon を止めて、記録したスタブの process group まで kill・wait してから dir を消す。
//!
//! - スタブはこの試験 binary 自身（`--ablation-stub <dir>` で起動されたときだけ、`main` より前に [`stub_main`] が
//!   動いて終わる）。非 LLM で、`spec.json` に従い決定的に計画出力（`execution-plan.json`）を書き、指定の title で
//!   指定回数だけ決定的に失敗（`retryable`）する。起動ごとに PID・親 PID・`CELERIS_RUN_ID`・`CELERIS_TASK_ID` を
//!   `launches.jsonl` に追記する。
//! - LLM の実行先はスタブだけ: config の source / harness は全部 `kind = "fake"`。daemon の PATH の先頭には
//!   `claude`・`codex`・`opencode`・`gemini`・`aider` の罠を置き、起動されたら `unexpected.log` に残す
//!   （[`Fixture::assert_only_stub_launches`] が失敗にする）。
//! - 同期は出来事の到着待ち＋期限。起動は daemon の stderr の「listening」行、event は SSE `GET /stream` の
//!   frame（到着ごとに `GET /events` を読み直して判定）、停止は daemon の `wait` を、どれも channel の
//!   `recv_timeout` で受ける（[`Fixture::wait_events`]）。固定時間の sleep・空回りの poll で順序を作らない。
//! - 環境不備（binary が無い・daemon が上がらない・curl 等）は skip せず panic で失敗する。userns・本番権限は要らない。

#![allow(dead_code)]

use std::{
    fs,
    io::{BufRead, Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        OnceLock,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use celeris_campaign::celeris_exec::{HttpResponse, Transport};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// スタブとして起動されたことを示す argv[1]。
pub const STUB_FLAG: &str = "--ablation-stub";
/// LLM CLI の罠（daemon の PATH 先頭）。どれかが起動されたら試験失敗。
pub const TRAPPED_CLIS: &[&str] = &["claude", "codex", "opencode", "gemini", "aider"];
/// スタブが artifacts と作業 dir に残す痕跡の file 名。
pub const TRIAL_NOTES: &str = "trial-notes.md";
/// 待ちの既定期限。
pub const DEADLINE: Duration = Duration::from_secs(120);
/// daemon の停止（TERM）を待つ期限。過ぎたら KILL。
const STOP_DEADLINE: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------------------------
// スタブ process
// ---------------------------------------------------------------------------------------------

/// スタブの振る舞い（fixture が `spec.json` に書く。全て title で決定的に選ぶ）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StubSpec {
    /// planner run（`context.execution_planner` あり）で、task の title に対して書く `execution-plan.json` の中身。
    #[serde(default)]
    pub plans: Vec<StubPlan>,
    /// task の title ごとに、最初の `times` 回の起動を `retryable` の error で終える。
    #[serde(default)]
    pub fail: Vec<StubFail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubPlan {
    pub title: String,
    pub plan: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubFail {
    pub title: String,
    pub times: u32,
}

/// スタブ 1 起動の記録（`launches.jsonl` の `start` 行と `end` 行を合わせたもの）。
#[derive(Debug, Clone)]
pub struct Launch {
    pub pid: u32,
    pub ppid: u32,
    pub run_id: String,
    pub task_id: String,
    pub request_task_id: String,
    pub title: String,
    pub planner: bool,
    /// スタブが stdin で受け取った `RunRequest.context.campaign`（variant 付き run の pin。無ければ null）。
    pub campaign: Value,
    /// `RunRequest.artifacts_dir`（run ごとに別の dir）。
    pub artifacts_dir: String,
    /// `end` 行の結果（`done` / `error`）。終わっていなければ `None`。
    pub outcome: Option<String>,
}

#[used]
#[unsafe(link_section = ".init_array")]
static STUB_ENTRY: extern "C" fn() = stub_entry;

/// libtest の `main` より前に走る。`--ablation-stub` で起動されたときだけスタブとして振る舞って終わる
/// （libtest の出力を worker protocol の stdout に混ぜないため）。
extern "C" fn stub_entry() {
    let Ok(cmdline) = fs::read("/proc/self/cmdline") else {
        return;
    };
    let args: Vec<String> = cmdline
        .split(|b| *b == 0)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    if args.get(1).map(String::as_str) != Some(STUB_FLAG) {
        return;
    }
    let code = match args.get(2) {
        Some(dir) => match stub_main(Path::new(dir)) {
            Ok(()) => 0,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "ablation stub: {e}");
                3
            }
        },
        None => 2,
    };
    std::process::exit(code);
}

fn append_line(path: &Path, v: &Value) -> Result<(), String> {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(format!("{v}\n").as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn ppid_of_self() -> u32 {
    fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|s| {
            let rest = s.rsplit_once(')')?.1.to_string();
            rest.split_whitespace().nth(1)?.parse().ok()
        })
        .unwrap_or(0)
}

/// スタブ本体。stdin の `RunRequest`（1 行）を読み、`spec.json` に従って決定的に振る舞う。
fn stub_main(dir: &Path) -> Result<(), String> {
    let spec: StubSpec = serde_json::from_str(
        &fs::read_to_string(dir.join("spec.json")).map_err(|e| format!("spec.json: {e}"))?,
    )
    .map_err(|e| format!("spec.json: {e}"))?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("stdin: {e}"))?;
    let req: Value = serde_json::from_str(&line).map_err(|e| format!("run request: {e}"))?;
    let title = req["task"]["title"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let planner = !req["context"]["execution_planner"].is_null();
    let artifacts = PathBuf::from(
        req["artifacts_dir"]
            .as_str()
            .ok_or("run request has no artifacts_dir")?,
    );
    let pid = std::process::id();
    let run_id = std::env::var("CELERIS_RUN_ID").unwrap_or_default();
    let log = dir.join("launches.jsonl");
    // 受け取った RunRequest の全文（canary の流入検査が読む）。
    append_line(&dir.join("requests.jsonl"), &req)?;
    append_line(
        &log,
        &json!({"phase": "start", "pid": pid, "ppid": ppid_of_self(), "run_id": run_id,
                "task_id": std::env::var("CELERIS_TASK_ID").unwrap_or_default(),
                "request_task_id": req["task"]["id"], "title": title, "planner": planner,
                "campaign": req["context"]["campaign"], "artifacts_dir": artifacts}),
    )?;
    let end = |outcome: &str| {
        append_line(
            &log,
            &json!({"phase": "end", "pid": pid, "run_id": run_id, "outcome": outcome}),
        )
    };

    if let Some(rule) = spec.fail.iter().find(|f| f.title == title) {
        let counter = dir.join(format!("fail-{}", hex(&title)));
        let n: u32 = fs::read_to_string(&counter)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if n < rule.times {
            fs::write(&counter, (n + 1).to_string()).map_err(|e| e.to_string())?;
            end("error")?;
            println!(
                "{}",
                json!({"type": "error", "message": format!("ablation stub: deterministic failure {} of {}", n + 1, rule.times), "retryable": true})
            );
            return Ok(());
        }
    }
    fs::create_dir_all(&artifacts).map_err(|e| format!("{}: {e}", artifacts.display()))?;
    if planner && let Some(p) = spec.plans.iter().find(|p| p.title == title) {
        fs::write(artifacts.join("execution-plan.json"), p.plan.to_string())
            .map_err(|e| format!("execution-plan.json: {e}"))?;
    }
    // 試行が残す痕跡（objective の写し）を artifacts と作業 dir に置く。隣接 trial の canary 検査で、
    // 先の trial が残した値が後の trial の入力・作業場所に流れ込まないことを見る材料になる。
    let notes = format!(
        "trial notes\n{}\n",
        req["task"]["objective"].as_str().unwrap_or_default()
    );
    fs::write(artifacts.join(TRIAL_NOTES), &notes).map_err(|e| format!("{TRIAL_NOTES}: {e}"))?;
    if let Some(w) = req["work_dir"]
        .as_str()
        .or(req["workspace"].as_str())
        .filter(|w| Path::new(w).is_dir())
    {
        fs::write(Path::new(w).join(TRIAL_NOTES), &notes)
            .map_err(|e| format!("{w}/{TRIAL_NOTES}: {e}"))?;
    }
    fs::write(artifacts.join("result.md"), result_md_body(&run_id, pid))
        .map_err(|e| format!("result.md: {e}"))?;
    end("done")?;
    println!(
        "{}",
        json!({"type": "done", "summary": format!("ablation stub {title}"), "evidence": []})
    );
    Ok(())
}

/// スタブが書く `result.md` の中身。PID を含むので、daemon の `artifact_produced{run_id, sha256}` から
/// run id と PID の対応が引ける（planner run には `CELERIS_RUN_ID` が渡らないため、こちらを正とする）。
pub fn result_md_body(env_run_id: &str, pid: u32) -> String {
    format!("ablation stub pid {pid} env-run {env_run_id}\n")
}

pub fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(b))
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// 実 celeris binary
// ---------------------------------------------------------------------------------------------

/// 実 `celeris` binary。`celeris-campaign` と同じ出力 dir に無ければ `cargo build -p celeris --bin celeris`。
/// 作れなければ失敗（skip しない）。`celeris` crate には依存しない。
pub fn celeris_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let campaign = std::env::var("CARGO_BIN_EXE_celeris-campaign")
            .expect("CARGO_BIN_EXE_celeris-campaign (run through cargo test / nextest)");
        let p = Path::new(&campaign)
            .parent()
            .expect("bin dir")
            .join("celeris");
        if !p.exists() {
            let st = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
                .args(["build", "-p", "celeris", "--bin", "celeris"])
                .status()
                .expect("cargo build celeris");
            assert!(st.success(), "cargo build -p celeris --bin celeris failed");
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
        .expect("spawn sh");
    c.stdin
        .take()
        .expect("stdin")
        .write_all(body.as_bytes())
        .expect("write executable");
    assert!(c.wait().expect("wait").success(), "{}", path.display());
}

// ---------------------------------------------------------------------------------------------
// HTTP（loopback・Bearer。試験 process の環境変数は使わない）
// ---------------------------------------------------------------------------------------------

/// loopback の daemon への最小の HTTP/1.1 client（`Connection: close`）。`Transport` を実装するので
/// 既存の `CelerisExecutor` もこの daemon へ向けられる。
#[derive(Debug, Clone)]
pub struct Http {
    pub addr: SocketAddr,
    token: String,
}

impl Http {
    pub fn new(addr: SocketAddr, token: &str) -> Self {
        assert!(addr.ip().is_loopback(), "fixture daemon must be loopback");
        Self {
            addr,
            token: token.into(),
        }
    }

    pub fn get(&self, path: &str) -> HttpResponse {
        self.request("GET", path, None)
            .unwrap_or_else(|e| panic!("GET {path}: {e}"))
    }

    pub fn post(&self, path: &str, body: &Value) -> HttpResponse {
        self.request("POST", path, Some(body))
            .unwrap_or_else(|e| panic!("POST {path}: {e}"))
    }
}

impl Transport for Http {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let mut s = TcpStream::connect_timeout(&self.addr, Duration::from_secs(10))
            .map_err(|e| format!("connect {}: {e}", self.addr))?;
        s.set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| e.to_string())?;
        let payload = body.map(Value::to_string).unwrap_or_default();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\nAccept: application/json\r\n",
            self.addr, self.token
        );
        if body.is_some() {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                payload.len()
            ));
        }
        req.push_str("\r\n");
        req.push_str(&payload);
        s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or("no header terminator")?;
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let mut rest = raw[split + 4..].to_vec();
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| format!("bad status line: {head}"))?;
        if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            rest = dechunk(&rest)?;
        }
        let text = String::from_utf8_lossy(&rest);
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.into_owned()))
        };
        Ok(HttpResponse { status, body })
    }
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let nl = b.windows(2).position(|w| w == b"\r\n").ok_or("bad chunk")?;
        let size_str = String::from_utf8_lossy(&b[..nl]);
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|e| format!("chunk size {size_str:?}: {e}"))?;
        b = &b[nl + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size + 2 {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

// ---------------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------------

/// fixture の入力。spec-off / spec-on は別々の `FixtureSpec` から別々の [`Fixture`] を作る。
#[derive(Debug, Clone)]
pub struct FixtureSpec {
    /// 識別用（ログと token の値に入る）。
    pub label: String,
    /// `org_include` の中身（`[[org]]`）。空 DB にだけ蒔かれる種（通常の起動経路）。
    pub org_toml: String,
    /// config の末尾に足す TOML（`[[genres]]` 等。kind は `fake` のみ許す）。
    pub extra_config: String,
    pub stub: StubSpec,
}

impl FixtureSpec {
    /// 根（secretary）と coding 課 1 つだけの最小 org。
    pub fn minimal(label: &str) -> Self {
        Self {
            label: label.into(),
            org_toml: r#"
[[org]]
id = "secretary"
name = "秘書"
kind = "secretary"

[[org]]
id = "coding"
name = "開発課"
kind = "section"
parent_id = "secretary"
genre = "coding"

[org.profile.harnesses]
allowed = ["coding"]
"#
            .into(),
            extra_config: String::new(),
            stub: StubSpec::default(),
        }
    }
}

/// 回収の確認用に、`Drop` の後でも見られる値。
#[derive(Debug, Clone)]
pub struct Owned {
    pub dir: PathBuf,
    pub addr: SocketAddr,
    pub daemon_pid: u32,
    pub stub_pids: Vec<u32>,
}

/// daemon 由来の出来事（fixture の待ちはこれだけを受ける）。
enum Signal {
    /// daemon の stdout/stderr の 1 行（daemon.log にも書いた）。
    Log(String),
    /// SSE `GET /stream` の frame が 1 つ届いた（`task.event`・`heartbeat` など）。
    Stream,
    /// SSE の接続が閉じた。
    StreamClosed,
    /// daemon process が終わった（`wait` の結果）。
    Exited(Option<ExitStatus>),
    /// daemon の stdout か stderr が EOF になった（1 本につき 1 回）。
    LogClosed,
}

pub struct Fixture {
    /// daemon の PID（`Child` は wait する thread が持つ）。
    pid: Option<u32>,
    exited: Option<Option<ExitStatus>>,
    signals: Receiver<Signal>,
    signal_tx: Sender<Signal>,
    dir: Option<tempfile::TempDir>,
    pub http: Http,
    pub label: String,
    events: Vec<Value>,
    last_event_id: u64,
}

/// port の衝突で起動し直す回数の上限。
const BIND_ATTEMPTS: usize = 5;

/// daemon が API の port を他に取られて終わったときの行。
fn is_bind_conflict(line: &str) -> bool {
    line.contains("cannot bind the API listener") && line.contains("Address already in use")
}

fn free_loopback_addr() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind loopback")
        .local_addr()
        .expect("local addr")
}

/// 試験の config（全 source / harness が `fake`。worker は `[adapters.fake] command` = スタブ）。
fn config_toml(addr: SocketAddr, stub_exe: &Path, stub_dir: &Path, extra: &str) -> String {
    format!(
        r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 100
org_include = "org.toml"

[api]
listen = "{addr}"

[[api.tokens]]
principal = "operator:test"
token_file = "api.token"

[adapters.fake]
command = ["{exe}", "{flag}", "{stub_dir}"]

[llm_sources.stub]
kind = "fake"
accounts = ["stub"]
models = ["stub"]

[harnesses.stub]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.stub]
candidates = [{{ source = "stub", model = "stub" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "stub", required_capabilities = [] }}]

[[roles]]
id = "implementer"

[[genres]]
id = "coding"
description = "コードを書く"
default_role = "implementer"
roles = ["implementer"]
{extra}
"#,
        exe = stub_exe.display(),
        flag = STUB_FLAG,
        stub_dir = stub_dir.display(),
    )
}

/// 実 LLM を起動できない構成であることの検査（kind は全部 `fake`、LLM CLI・API 名が無い）。
pub fn assert_config_is_stub_only(text: &str) {
    for line in text.lines().filter(|l| l.trim_start().starts_with("kind")) {
        let v = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
        assert!(
            v == "\"fake\"" || v == "\"secretary\"" || v == "\"section\"" || v == "\"department\"",
            "non-stub kind in fixture config: {line}"
        );
    }
    for word in [
        "claude",
        "codex",
        "opencode",
        "anthropic",
        "openai",
        "gemini",
    ] {
        assert!(
            !text.to_ascii_lowercase().contains(word),
            "fixture config mentions {word}"
        );
    }
}

impl Fixture {
    /// 一時 dir・token・org の種・workspace・HOME・スタブを用意し、実 `celeris` を `env_clear` で起動して
    /// `GET /health` が答えるまで待つ（期限付き。上がらなければ daemon.log 付きで失敗）。
    ///
    /// port は空きを確かめてから daemon に渡すので、その間に host 上の他の試験が同じ port を取ることがある。
    /// daemon が listen の前に「Address already in use」で終わったときだけ、新しい port で起動し直す。
    pub fn start(spec: FixtureSpec) -> Self {
        for _ in 0..BIND_ATTEMPTS {
            if let Some(fx) = Self::launch(spec.clone()) {
                return fx;
            }
        }
        panic!("daemon could not bind a free loopback port in {BIND_ATTEMPTS} attempts");
    }

    /// 1 回の起動。listen の前に port の衝突で終わったら `None`（他の失敗は daemon.log 付きで panic）。
    fn launch(spec: FixtureSpec) -> Option<Self> {
        let dir = tempfile::Builder::new()
            .prefix(&format!("ablation-app-{}-", spec.label))
            .tempdir()
            .expect("fixture dir");
        let h = dir.path();
        for d in ["ws", "home", "stub", "trap"] {
            fs::create_dir_all(h.join(d)).expect("fixture subdir");
        }
        let token = format!("ablation-{}-{}-{}", spec.label, std::process::id(), nonce());
        fs::write(h.join("api.token"), format!("{token}\n")).expect("token");
        fs::write(h.join("org.toml"), &spec.org_toml).expect("org seed");
        fs::write(
            h.join("stub/spec.json"),
            serde_json::to_string_pretty(&spec.stub).expect("stub spec"),
        )
        .expect("stub spec");
        for cli in TRAPPED_CLIS {
            write_executable(
                &h.join("trap").join(cli),
                &format!(
                    "#!/bin/sh\necho \"{cli} $*\" >> \"{}\"\nexit 97\n",
                    h.join("unexpected.log").display()
                ),
            );
        }
        let exe = std::env::current_exe().expect("current test exe");
        let addr = free_loopback_addr();
        let config = config_toml(addr, &exe, &h.join("stub"), &spec.extra_config);
        assert_config_is_stub_only(&config);
        fs::write(h.join("config.toml"), &config).expect("config");

        let mut c = Command::new(celeris_bin());
        c.arg("--config")
            .arg(h.join("config.toml"))
            .args(["--log-format", "text"])
            .current_dir(h)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", h.join("trap").display()),
            )
            .env("HOME", h.join("home"))
            .env("TMPDIR", h.join("home"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().expect("spawn celeris daemon");
        let pid = child.id();
        let (tx, rx) = mpsc::channel();
        let log = fs::File::create(h.join("daemon.log")).expect("daemon.log");
        let stdout = child.stdout.take().expect("daemon stdout");
        let stderr = child.stderr.take().expect("daemon stderr");
        tee_lines(stdout, log.try_clone().expect("log"), tx.clone());
        tee_lines(stderr, log, tx.clone());
        let exit_tx = tx.clone();
        thread::spawn(move || {
            let st = child.wait().ok();
            let _ = exit_tx.send(Signal::Exited(st));
        });
        let mut fx = Self {
            pid: Some(pid),
            exited: None,
            signals: rx,
            signal_tx: tx,
            dir: Some(dir),
            http: Http::new(addr, &token),
            label: spec.label,
            events: Vec::new(),
            last_event_id: 0,
        };
        let up_by = Instant::now() + Duration::from_secs(60);
        let mut bind_conflict = false;
        let mut logs_open = 2;
        loop {
            let left = up_by.saturating_duration_since(Instant::now());
            match fx.signals.recv_timeout(left) {
                Ok(Signal::Log(line)) if line.contains("celeris API listening") => break,
                Ok(Signal::Log(line)) => bind_conflict |= is_bind_conflict(&line),
                Ok(Signal::LogClosed) => logs_open -= 1,
                Ok(Signal::Exited(st)) => {
                    fx.exited = Some(st);
                    // 終了の知らせは stderr の最後の行より先に届くことがある。listen 前は worker が
                    // pipe を持たないので、stdout・stderr の EOF まで読み切ってから判定する。
                    while logs_open > 0 {
                        let left = up_by.saturating_duration_since(Instant::now());
                        match fx.signals.recv_timeout(left) {
                            Ok(Signal::Log(line)) => bind_conflict |= is_bind_conflict(&line),
                            Ok(Signal::LogClosed) => logs_open -= 1,
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                    if bind_conflict {
                        return None;
                    }
                    fx.next_signal(up_by, "the API listening line");
                }
                Ok(_) => {}
                Err(_) => {
                    fx.next_signal(up_by, "the API listening line");
                }
            }
        }
        let health = fx.http.get("/api/v1/health");
        assert_eq!(
            health.status,
            200,
            "health after listening: {}\n{}",
            health.body,
            fx.daemon_log()
        );
        fx.open_stream();
        Some(fx)
    }

    /// SSE `GET /stream` を開き、frame の到着を [`Signal::Stream`] で知らせる thread を起こす
    /// （daemon の停止で接続が閉じると thread も終わる）。
    fn open_stream(&self) {
        let mut s = TcpStream::connect_timeout(&self.http.addr, Duration::from_secs(10))
            .unwrap_or_else(|e| panic!("connect stream: {e}"));
        let req = format!(
            "GET /api/v1/stream HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: text/event-stream\r\n\r\n",
            self.http.addr, self.http.token
        );
        s.write_all(req.as_bytes()).expect("stream request");
        let mut r = std::io::BufReader::new(s);
        let mut status = String::new();
        r.read_line(&mut status).expect("stream status line");
        assert!(
            status.split_whitespace().nth(1) == Some("200"),
            "GET /stream: {status}"
        );
        let tx = self.signal_tx.clone();
        thread::spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.starts_with("event:") => {
                        if tx.send(Signal::Stream).is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                }
            }
            let _ = tx.send(Signal::StreamClosed);
        });
    }

    /// 次の出来事を `by` まで待つ。daemon が終わっていたら・期限を過ぎたら daemon.log 付きで失敗。
    fn next_signal(&mut self, by: Instant, what: &str) -> Signal {
        if let Some(st) = self.exited {
            panic!(
                "daemon exited ({st:?}) while waiting for {what}:\n{}",
                self.daemon_log()
            );
        }
        let left = by.saturating_duration_since(Instant::now());
        match self.signals.recv_timeout(left) {
            Ok(Signal::Exited(st)) => {
                self.exited = Some(st);
                panic!(
                    "daemon exited ({st:?}) while waiting for {what}:\n{}",
                    self.daemon_log()
                );
            }
            Ok(sig) => sig,
            Err(RecvTimeoutError::Timeout) => panic!(
                "timed out waiting for {what}; events:\n{}\ndaemon.log:\n{}",
                self.events
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
                self.daemon_log()
            ),
            Err(RecvTimeoutError::Disconnected) => unreachable!("fixture holds a sender"),
        }
    }

    pub fn dir(&self) -> &Path {
        self.dir.as_ref().expect("fixture dir").path()
    }

    pub fn daemon_pid(&self) -> u32 {
        self.pid.expect("daemon")
    }

    pub fn daemon_log(&self) -> String {
        fs::read_to_string(self.dir().join("daemon.log")).unwrap_or_default()
    }

    /// `Drop` の後で回収を確かめるための値。
    pub fn owned(&self) -> Owned {
        Owned {
            dir: self.dir().to_path_buf(),
            addr: self.http.addr,
            daemon_pid: self.daemon_pid(),
            stub_pids: self.launches().iter().map(|l| l.pid).collect(),
        }
    }

    /// 通常の `POST /api/v1/tasks`。
    pub fn post_task(&self, body: &Value) -> HttpResponse {
        self.http.post("/api/v1/tasks", body)
    }

    /// 新しい event を読み足す（`GET /api/v1/events?after_id=`）。
    pub fn refresh_events(&mut self) -> &[Value] {
        loop {
            let r = self.http.get(&format!(
                "/api/v1/events?after_id={}&limit=5000",
                self.last_event_id
            ));
            assert_eq!(r.status, 200, "GET /events: {}", r.body);
            let rows = event_rows(&r.body);
            if rows.is_empty() {
                break;
            }
            for row in rows {
                if let Some(id) = row["id"].as_u64() {
                    self.last_event_id = self.last_event_id.max(id);
                }
                self.events.push(row);
            }
        }
        &self.events
    }

    pub fn events(&self) -> &[Value] {
        &self.events
    }

    /// 読んだ event の列が `pred` を満たすまで待つ。SSE の frame（または daemon の出来事）が届くたびに
    /// `GET /events` を読み直して判定する（期限付き。過ぎたら event・daemon.log 付きで失敗）。
    pub fn wait_events(&mut self, what: &str, deadline: Duration, pred: impl Fn(&[Value]) -> bool) {
        let by = Instant::now() + deadline;
        loop {
            if pred(self.refresh_events()) {
                return;
            }
            if let Signal::StreamClosed = self.next_signal(by, what) {
                panic!(
                    "event stream closed while waiting for {what}:\n{}",
                    self.daemon_log()
                );
            }
        }
    }

    /// スタブの起動記録（`start` と `end` を pid で合わせる）。
    pub fn launches(&self) -> Vec<Launch> {
        read_launches(self.dir())
    }

    /// スタブの各起動を daemon の run id に対応付ける（daemon の記録だけから引く）。done の起動は、スタブが書いた
    /// `result.md`（PID 入り）の sha256 と一致する `artifact_produced` event の run id。error の起動（result.md を
    /// 書かない）は環境の `CELERIS_RUN_ID`。どちらも、その run の `worker_started` が起動の `RunRequest.task.id` の
    /// task にあることを確かめる。対応しない起動があれば失敗。
    pub fn launch_runs(&mut self) -> Vec<(Launch, String)> {
        self.refresh_events();
        let events = self.events.clone();
        let produced = events_of(&events, "artifact_produced");
        let started = events_of(&events, "worker_started");
        self.launches()
            .into_iter()
            .map(|l| {
                let run = match l.outcome.as_deref() {
                    Some("done") => {
                        let sha = sha256_hex(result_md_body(&l.run_id, l.pid).as_bytes());
                        let hits: Vec<&str> = produced
                            .iter()
                            .filter(|e| {
                                e["task_id"] == l.request_task_id.as_str()
                                    && e["event"]["artifact"]["name"] == "result.md"
                                    && e["event"]["artifact"]["sha256"] == sha.as_str()
                            })
                            .filter_map(|e| e["event"]["run_id"].as_str())
                            .collect();
                        assert_eq!(
                            hits.len(),
                            1,
                            "launch pid {} has no unique result.md artifact event: {hits:?}",
                            l.pid
                        );
                        hits[0].to_string()
                    }
                    _ => l.run_id.clone(),
                };
                assert!(!run.is_empty(), "launch without a run id: {l:?}");
                if !l.run_id.is_empty() {
                    assert_eq!(l.run_id, run, "env run id differs from the daemon record");
                }
                assert!(
                    started
                        .iter()
                        .any(|e| e["task_id"] == l.request_task_id.as_str()
                            && e["event"]["run_id"] == run.as_str()),
                    "run {run} of launch pid {} has no worker_started on task {}",
                    l.pid,
                    l.request_task_id
                );
                (l, run)
            })
            .collect()
    }

    /// 罠の LLM CLI が 1 度も起動されていない・全 `worker_started` が `fake` adapter で、スタブの起動と 1 対 1。
    pub fn assert_only_stub_launches(&mut self) {
        let trap = fs::read_to_string(self.dir().join("unexpected.log")).unwrap_or_default();
        assert!(trap.is_empty(), "unexpected LLM CLI launches:\n{trap}");
        let runs = self.launch_runs();
        let started = events_of(&self.events, "worker_started");
        for e in &started {
            assert_eq!(
                e["event"]["adapter"], "fake",
                "worker started with a non-stub adapter: {e}"
            );
        }
        let mut ev_runs: Vec<String> = started
            .iter()
            .map(|e| {
                e["event"]["run_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut stub_runs: Vec<String> = runs.into_iter().map(|(_, r)| r).collect();
        ev_runs.sort();
        stub_runs.sort();
        assert_eq!(
            ev_runs, stub_runs,
            "worker_started runs and stub launches differ"
        );
    }

    /// daemon を止め（TERM → 期限で KILL）、記録したスタブの process group を KILL して wait する。
    fn shutdown(&mut self) {
        let stubs: Vec<u32> = if self.dir.is_some() {
            self.launches().iter().map(|l| l.pid).collect()
        } else {
            Vec::new()
        };
        if let Some(pid) = self.pid.take()
            && self.exited.is_none()
        {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
            let mut by = Instant::now() + STOP_DEADLINE;
            let mut killed = false;
            loop {
                let left = by.saturating_duration_since(Instant::now());
                match self.signals.recv_timeout(left) {
                    Ok(Signal::Exited(st)) => {
                        self.exited = Some(st);
                        break;
                    }
                    Ok(_) => {}
                    Err(_) if !killed => {
                        // 期限内に止まらなければ KILL し、wait の結果をもう一度（期限付きで）待つ。
                        let _ = Command::new("kill")
                            .args(["-KILL", &pid.to_string()])
                            .status();
                        killed = true;
                        by = Instant::now() + STOP_DEADLINE;
                    }
                    Err(_) => break,
                }
            }
        }
        for pid in stubs {
            if pid_alive(pid) && is_our_stub(pid) {
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{pid}")])
                    .status();
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown();
        // TempDir の drop が DB・token・org・workspace・HOME・スタブ状態をまとめて消す。
        self.dir.take();
    }
}

/// `src` を 1 行ずつ `log` に写し、各行を [`Signal::Log`] で知らせる thread を起こす。
fn tee_lines(src: impl Read + Send + 'static, mut log: fs::File, tx: Sender<Signal>) {
    thread::spawn(move || {
        let mut r = std::io::BufReader::new(src);
        let mut line = String::new();
        loop {
            line.clear();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Signal::LogClosed);
                    return;
                }
                Ok(_) => {
                    let _ = log.write_all(line.as_bytes());
                    let _ = tx.send(Signal::Log(line.clone()));
                }
            }
        }
    });
}

fn nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// `/proc/<pid>` があって zombie でない。
pub fn pid_alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit_once(')')
                .and_then(|(_, r)| r.split_whitespace().next().map(|st| st != "Z"))
        })
        .unwrap_or(false)
}

fn is_our_stub(pid: u32) -> bool {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|c| String::from_utf8_lossy(&c).contains(STUB_FLAG))
        .unwrap_or(false)
}

/// `GET /events` の応答から event の行を取り出す（`{"events": [...]}` か配列）。
pub fn event_rows(body: &Value) -> Vec<Value> {
    body.get("events")
        .or_else(|| body.get("items"))
        .unwrap_or(body)
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// event の種類（`event.type`）で絞る。
pub fn events_of<'a>(events: &'a [Value], ty: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["event"]["type"] == ty).collect()
}

/// fixture dir の `stub/launches.jsonl` を読む（`start` と `end` を pid で合わせる。待ちの述語からも使う）。
pub fn read_launches(dir: &Path) -> Vec<Launch> {
    let text = fs::read_to_string(dir.join("stub/launches.jsonl")).unwrap_or_default();
    let mut out: Vec<Launch> = Vec::new();
    // 書きかけの最後の行（改行で終わっていない）は読まない。
    let complete = text.rfind('\n').map_or("", |i| &text[..i]);
    for line in complete.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).expect("launch line");
        let pid = v["pid"].as_u64().expect("pid") as u32;
        match v["phase"].as_str() {
            Some("start") => out.push(Launch {
                pid,
                ppid: v["ppid"].as_u64().unwrap_or(0) as u32,
                run_id: v["run_id"].as_str().unwrap_or_default().into(),
                task_id: v["task_id"].as_str().unwrap_or_default().into(),
                request_task_id: v["request_task_id"].as_str().unwrap_or_default().into(),
                title: v["title"].as_str().unwrap_or_default().into(),
                planner: v["planner"].as_bool().unwrap_or(false),
                campaign: v["campaign"].clone(),
                artifacts_dir: v["artifacts_dir"].as_str().unwrap_or_default().into(),
                outcome: None,
            }),
            Some("end") => {
                if let Some(l) = out.iter_mut().rev().find(|l| l.pid == pid) {
                    l.outcome = v["outcome"].as_str().map(String::from);
                }
            }
            other => panic!("unknown launch phase {other:?}"),
        }
    }
    out
}

/// スタブが受け取った `RunRequest` の全文（起動順）。
pub fn read_requests(dir: &Path) -> Vec<Value> {
    fs::read_to_string(dir.join("stub/requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// `root` 以下の通常 file のうち、中身（lossy）に `needle` を含むものの path。
pub fn files_containing(root: &Path, needle: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file()
                && fs::read(&p).is_ok_and(|b| String::from_utf8_lossy(&b).contains(needle))
            {
                out.push(p);
            }
        }
    }
    out
}
