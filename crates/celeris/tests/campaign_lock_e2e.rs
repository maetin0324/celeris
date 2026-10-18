//! P1-5 最終接続: campaign lock を、正式な task-api（実 HTTP・一時 DB・router/DTO そのまま）の通常経路で受け入れる。
//!
//! 正しい入力 → 期待 lock（`preflight::expected_lock`、CLI の run/watch と同じ関数）→ 通常の `POST /tasks`
//! （`CelerisExecutor` が実 HTTP で投入）→ task 追跡 → 完了 artifact の hash → `grade_out_expected` → bind（`check_expected`）
//! → 最終 `export_expected`。誤初期 repo・別 submodule・資料・KB・外部入力は POST と外部実行 0 で拒否され、
//! 同一 case id/split の grader・公開/隠しテスト差替えは grade・bind・export で拒否、lock・入力の変更は既存測定の再開を拒否する。
//!
//! 境界の fake JSON も campaign 情報の DB 直挿入も無い。mock にしてよい「実行内部」だけを test が担う:
//! dispatcher 相当の状態遷移、worker の run イベントと成果物、A arm の harness、grader の評価 command。
//! 時計は注入（`Clock::sleep` が実行内部を 1 歩進める）。外部 LLM・有料 job・本番変更・userns は使わない。
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};

use celeris::Config;
use celeris_campaign::{
    aggregate::Outcome,
    cases::CaseSet,
    celeris_exec::{CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport},
    checker::check_expected,
    executor::{
        ExecError, ExecResult, ExecStatus, Executor, ObservedRun, TaskRef, TokenUsage,
        TrialRequest, Usage,
    },
    export::export_expected,
    grade_out::grade_out_expected,
    grader::{CommandOutput, CommandRunner},
    grades::GRADES_FILE,
    inputs,
    lock::CampaignLock,
    lock_exec::LOCK_FILE,
    lock_tree,
    manifest::{CampaignManifest, ConfigId},
    preflight::{self, Host, Inputs, ResolvedProgram},
    runner::RunOptions,
    watch::{Clock, WatchEnd, WatchOptions, WatchReport, watch},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::{
    DaemonMode, Event, InstanceRole, RunEnd, RunMetrics, RunRole, SharedRole, SqliteStore, Status,
    TaskId, TaskStore, Trigger, Usage as RunUsage,
};

#[path = "../../campaign/tests/preflight_setup/mod.rs"]
mod preflight_setup;

const TOKEN: &str = "campaign-lock-token";
const K2: &str = "k2-bf-xattr-restart-campaign";
const K4: &str = "k4-cmp4-validity-analysis";
const K4_INPUT: &str = "main-v3-n2-43706.tgz";
const K2_TEST: &str = "lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks";

fn campaign_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../campaign")
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for e in fs::read_dir(from).expect("read_dir") {
        let p = e.expect("entry").path();
        let t = to.join(p.file_name().expect("name"));
        if p.is_dir() {
            copy(&p, &t);
        } else {
            fs::copy(&p, &t).expect("copy");
        }
    }
}

// ---------------------------------------------------------------- 一時 daemon（実 HTTP の task-api）

struct Server {
    port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
}

impl Server {
    async fn start(home: &Path) -> Self {
        fs::create_dir_all(home.join("ws")).expect("ws");
        fs::write(home.join("api.token"), format!("{TOKEN}\n")).expect("token");
        fs::write(
            home.join("config.toml"),
            r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200

[api]
listen = "127.0.0.1:0"
token_file = "api.token"

[selfdeploy]
releases_dir = "releases"
repo = "repo"
detach = "inline"

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.fake]
candidates = [{ source = "local", model = "fake" }]

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "fake", required_capabilities = [] }]
"#,
        )
        .expect("config");
        let config = Config::load(&home.join("config.toml")).expect("load");
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let settings = celeris::api_settings(
            &config,
            addr,
            Some(TOKEN.to_string()),
            "01J00000000000000000000000".to_string(),
            "2026-10-17T00:00:00Z".to_string(),
            None,
            "dev".to_string(),
            DaemonMode::Normal,
            SharedRole::new(InstanceRole::Active),
            None,
        );
        let (daemon_tx, daemon_rx) = tokio::sync::watch::channel(None);
        let state = task_api::ApiState::new(settings, daemon_rx).expect("state");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
            let _ = stopped.await;
        }));
        Self {
            port: addr.port(),
            stop: Some(stop),
            handle: Some(handle),
            _daemon_tx: daemon_tx,
        }
    }

    async fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(Duration::from_secs(60), handle)
                .await
                .expect("server stopped")
                .expect("server task")
                .expect("server result");
        }
    }
}

// ---------------------------------------------------------------- 実 HTTP の Transport

#[derive(Default)]
struct Http {
    port: u16,
    posts: AtomicU32,
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).trim(), 16).unwrap_or(0);
        if n == 0 || b.len() < i + 2 + n {
            break;
        }
        out.extend_from_slice(&b[i + 2..i + 2 + n]);
        b = &b[(i + 2 + n + 2).min(b.len())..];
    }
    out
}

impl Http {
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<HttpResponse, String> {
        let payload = body.map(Value::to_string).unwrap_or_default();
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(60))).ok();
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\n\
             Connection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        )
        .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        if method == "POST" {
            self.posts.fetch_add(1, Ordering::SeqCst);
        }
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or("malformed HTTP response")?;
        let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .ok_or("no status")?;
        let rest = &raw[split + 4..];
        let bytes = if head.contains("transfer-encoding: chunked") {
            dechunk(rest)
        } else {
            rest.to_vec()
        };
        Ok(HttpResponse {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        })
    }
}

#[derive(Clone)]
struct Shared(Arc<Http>);

impl Transport for Shared {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        self.0.call(method, path, body)
    }
}

// ---------------------------------------------------------------- A arm（harness）の mock

#[derive(Default)]
struct ArmA {
    seen: Mutex<BTreeMap<String, TaskRef>>,
}

impl ArmA {
    fn submitted(&self) -> usize {
        self.seen.lock().expect("lock").len()
    }
}

impl Executor for ArmA {
    fn name(&self) -> &'static str {
        "arm-a-mock"
    }
    fn execute(&self, _: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only".into())
    }
    fn is_async(&self) -> bool {
        true
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        Ok(self.seen.lock().expect("lock").get(key).cloned())
    }
    fn submit(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        fs::write(r.workspace.join("result.txt"), "correct").map_err(|e| e.to_string())?;
        let t = TaskRef {
            task_id: format!("A-{key}"),
            ..TaskRef::default()
        };
        self.seen
            .lock()
            .expect("lock")
            .insert(key.to_string(), t.clone());
        Ok(t)
    }
    fn poll(&self, _: &TaskRef) -> Result<ExecStatus, ExecError> {
        Ok(ExecStatus::Done(ExecResult {
            outcome: Outcome::Passed,
            tokens: Some(TokenUsage::full(10, 5, 0, 0)),
            cost_micro_usd: Some(1),
            wall_secs: 1,
        }))
    }
}

/// A は mock、B/C は実 HTTP の CelerisExecutor（CLI の `--executor` 配線と同じ分け方）。
struct ByArm<'a> {
    a: &'a ArmA,
    bc: &'a CelerisExecutor<Shared>,
}

impl ByArm<'_> {
    fn by_task(&self, t: &TaskRef) -> &dyn Executor {
        if t.task_id.starts_with("A-") {
            self.a
        } else {
            self.bc
        }
    }
    fn by_req(&self, r: &TrialRequest) -> &dyn Executor {
        if r.config.id == ConfigId::A {
            self.a
        } else {
            self.bc
        }
    }
}

impl Executor for ByArm<'_> {
    fn name(&self) -> &'static str {
        "split"
    }
    fn execute(&self, _: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only".into())
    }
    fn is_async(&self) -> bool {
        true
    }
    fn routing(&self, r: &TrialRequest) -> Result<Value, String> {
        self.by_req(r).routing(r)
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        Ok(match self.bc.lookup(key)? {
            Some(t) => Some(t),
            None => self.a.lookup(key)?,
        })
    }
    fn submit(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        self.by_req(r).submit(r, key)
    }
    fn track(&self, t: &TaskRef) -> Result<TaskRef, ExecError> {
        self.by_task(t).track(t)
    }
    fn usage(&self, t: &TaskRef) -> Result<Option<Usage>, ExecError> {
        self.by_task(t).usage(t)
    }
    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        self.by_task(t).poll(t)
    }
    fn observed(&self, t: &TaskRef) -> Result<Vec<ObservedRun>, String> {
        self.by_task(t).observed(t)
    }
    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), ExecError> {
        self.by_task(t).cancel(t, reason)
    }
    fn stopped(&self, t: &TaskRef) -> Result<bool, ExecError> {
        self.by_task(t).stopped(t)
    }
}

// ---------------------------------------------------------------- mock の実行内部（dispatcher / worker）

struct Interior {
    store: SqliteStore,
    http: Arc<Http>,
    children: Mutex<BTreeMap<TaskId, TaskId>>,
    runs: AtomicU32,
}

fn ev_started(run: &str, role: Option<RunRole>) -> Event {
    Event::WorkerStarted {
        run_id: run.into(),
        adapter: "claude-code".into(),
        model: "m-exec".into(),
        provider: Some("src".into()),
        account: None,
        role,
        task_role: None,
    }
}

fn ev_finished(run: &str, role: Option<RunRole>, tokens: u64) -> Event {
    Event::WorkerFinished {
        run_id: run.into(),
        outcome: "x".into(),
        usage: Some(RunUsage {
            input_tokens: Some(tokens),
            output_tokens: Some(1),
            cache_read_tokens: Some(0),
            cache_creation_tokens: Some(0),
            cost_usd: Some(0.5),
            duplicate_reads: None,
            session_resumed: None,
            context_tokens: None,
        }),
        role,
        metrics: Some(RunMetrics {
            wall_ms: 100,
            retries: 0,
            peak_context_tokens: None,
            turns: None,
        }),
        end: Some(RunEnd::Completed),
    }
}

impl Interior {
    fn run_id(&self, what: &str) -> String {
        format!("{what}-{}", self.runs.fetch_add(1, Ordering::SeqCst))
    }

    /// 実行内部を 1 歩進める。Ready → Running → （planner が子 task を通常の POST で作成・子の完了）→ root 完了。
    fn step(&self) {
        let mut tasks = self.store.list(None).expect("list");
        tasks.sort_by_key(|t| t.id);
        for t in tasks {
            match t.status {
                Status::Ready => {
                    self.store
                        .apply_transition(t.id, Trigger::Dispatch, None)
                        .expect("dispatch");
                }
                Status::Running if t.parent_id.is_some() => {
                    let r = self.run_id("r-child");
                    self.store
                        .apply_transition_with_events(
                            t.id,
                            Trigger::WorkerDone,
                            vec![ev_started(&r, None), ev_finished(&r, None, 300)],
                        )
                        .expect("child done");
                    self.store
                        .apply_transition(t.id, Trigger::ReviewPass, None)
                        .expect("child pass");
                }
                Status::Running => self.step_root(&t),
                _ => {}
            }
        }
    }

    fn step_root(&self, t: &task_core::Task) {
        let child = self.children.lock().expect("lock").get(&t.id).copied();
        let Some(child) = child else {
            let r = self.run_id("r-plan");
            self.store
                .append_event(t.id, &ev_started(&r, Some(RunRole::Planner)))
                .expect("event");
            self.store
                .append_event(t.id, &ev_finished(&r, Some(RunRole::Planner), 50))
                .expect("event");
            let resp = self
                .http
                .call(
                    "POST",
                    "/api/v1/tasks",
                    Some(&json!({"title": "child", "objective": "child",
                        "parent": t.id.to_string(),
                        "acceptance": [{"type": "artifact_exists", "name": "r"}]})),
                )
                .expect("child post");
            assert_eq!(resp.status, 201, "{}", resp.body);
            let c: TaskId = resp.body["id"]
                .as_str()
                .expect("id")
                .parse()
                .expect("task id");
            self.children.lock().expect("lock").insert(t.id, c);
            return;
        };
        let child_done = self
            .store
            .get(child)
            .expect("get")
            .is_some_and(|c| matches!(c.status, Status::Done));
        if !child_done {
            return;
        }
        let task_core::WorkspaceSpec::Local { path, .. } = &t.workspace else {
            panic!("unexpected workspace {:?}", t.workspace);
        };
        fs::write(path.join("result.txt"), "correct").expect("artifact");
        let (w, rv) = (self.run_id("r-worker"), self.run_id("r-review"));
        self.store
            .apply_transition_with_events(
                t.id,
                Trigger::WorkerDone,
                vec![
                    ev_started(&w, None),
                    ev_finished(&w, None, 200),
                    ev_started(&rv, Some(RunRole::Reviewer)),
                    ev_finished(&rv, Some(RunRole::Reviewer), 20),
                ],
            )
            .expect("worker done");
        self.store
            .apply_transition(t.id, Trigger::ReviewPass, None)
            .expect("review pass");
    }
}

struct StepClock<'a>(&'a Interior);

impl Clock for StepClock<'_> {
    fn sleep(&self, _d: Duration) {
        self.0.step();
    }
}

// ---------------------------------------------------------------- preflight の host（api_get だけ stub）

struct OkHost;
impl Host for OkHost {
    fn probe_runner(&self) -> Box<dyn celeris_campaign::cap_probe::ProbeRunner> {
        Box::new(preflight_setup::OkRunner)
    }
    fn worker_daemon_env(
        &self,
        _: &celeris_campaign::harness_exec::Toolchain,
    ) -> celeris_campaign::cap_probe::WorkerDaemonEnv {
        preflight_setup::fake_worker_daemon()
    }

    fn env_nonempty(&self, _: &str) -> bool {
        true
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        Ok(ResolvedProgram {
            path: p.to_path_buf(),
            version: "fake 1.0".into(),
        })
    }
    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        Ok(match path {
            // 測定 config で起動した daemon の応答: daemon 自身の `config_view`（実コード）を測定 config から作る。
            "/api/v1/config" => {
                let cfg_path = preflight_setup::current_config();
                let cfg = Config::load(&cfg_path).map_err(|e| e.to_string())?;
                let listen = cfg.api.listen.ok_or("no [api] listen")?;
                let mut view = celeris::config_view(
                    &cfg,
                    listen,
                    "01MEASUREDAEMON00000000000",
                    preflight_setup::MEASURE_RELEASE,
                    "2026-10-18T00:00:00Z",
                );
                // launcher 所有の daemon の pid として答える。
                view.identity.pid = preflight_setup::DAEMON_PID;
                serde_json::to_value(view).map_err(|e| e.to_string())?
            }
            "/api/v1/projects/01ABC" => {
                json!({"project":{"id":"01ABC","auto_integrate":false},"repos":[]})
            }
            "/api/v1/projects/01ABC/repos" => {
                json!({"items":[{"name":"scratch","is_primary":true,"location":{"kind":"local","path":"/x"}}]})
            }
            _ => {
                json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":account_dir(),"logged_in":true}]})
            }
        })
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
}

/// 空の測定専用アカウント dir（process 内で共有）。
fn account_dir() -> &'static Path {
    static D: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    D.get_or_init(|| tempfile::tempdir().expect("account dir"))
        .path()
}

// ---------------------------------------------------------------- grader command の stub（評価 command 内部だけ）

struct CountRunner(AtomicUsize);
impl CommandRunner for CountRunner {
    fn run(&self, _argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(CommandOutput {
            exit: Some(0),
            stdout: format!("test {K2_TEST} ... ok\n"),
            stderr: String::new(),
        })
    }
}

impl CountRunner {
    fn new() -> Self {
        CountRunner(AtomicUsize::new(0))
    }
    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------- 世界（daemon + 実 case 一式 + 評価）

fn git(repo: &Path, args: &[&str]) {
    let o = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(o.status.success(), "git {args:?}");
}

fn resolved_manifest() -> Value {
    let p = campaign_root()
        .join("..")
        .join("..")
        .join("agent-docs/progress/2026-10-16-mlsys-final/final.json");
    let mut v: Value = serde_json::from_slice(&fs::read(p).expect("final.json")).expect("json");
    for c in v["configs"].as_array_mut().expect("configs") {
        match c["id"].as_str().expect("id") {
            "A" => c["harness_version"] = json!("claude-code@2.1.0"),
            _ => c["harness_version"] = json!("0123456789ab"),
        }
        if c["routing"]["evidence_table_id"].is_string() {
            c["routing"]["evidence_table_id"] = json!("evidence-v7");
        }
    }
    v
}

struct World {
    dir: tempfile::TempDir,
    home: tempfile::TempDir,
    server: Server,
    http: Arc<Http>,
    interior: Interior,
    a: ArmA,
    inp: Inputs,
}

impl World {
    /// 実 final の case 定義・grade-case・grader 一式と、K2 の初期木（試験用の小さな git repo。本番資産には触れない）。
    async fn new() -> Self {
        let home = tempfile::tempdir().expect("home");
        let server = Server::start(home.path()).await;
        let http = Arc::new(Http {
            port: server.port,
            ..Http::default()
        });
        let config = Config::load(&home.path().join("config.toml")).expect("load");
        let store = SqliteStore::open(&config.db.path).expect("open");

        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path();
        let c = campaign_root();
        fs::copy(
            c.join("..")
                .join("..")
                .join("agent-docs/progress/2026-10-16-mlsys-final/final-input-manifest.json"),
            d.join("final-input-manifest.json"),
        )
        .expect("input manifest");
        fs::write(d.join("final.json"), resolved_manifest().to_string()).expect("manifest");
        let (cases, grade, ins) = (d.join("cases"), d.join("grade-cases"), d.join("inputs-src"));
        copy(&c.join("cases"), &cases);
        copy(&c.join("fixtures/grade-cases"), &grade);
        fs::create_dir_all(&ins).expect("ins");
        fs::write(ins.join(K4_INPUT), b"fake archive for the test").expect("input");
        let repo = d.join("repo");
        fs::create_dir_all(repo.join("src")).expect("src");
        fs::write(repo.join("README"), "k2 initial tree").expect("readme");
        fs::write(repo.join("src/lib.rs"), "pub fn a() {}\n").expect("lib");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "c"]);
        let li = lock_tree::derive_initial(&repo, "HEAD", &|_| None).expect("initial");
        let k2 = cases.join("final").join(K2);
        fs::write(
            k2.join("initial.lock.json"),
            serde_json::to_string(&li).expect("json"),
        )
        .expect("initial.lock");
        let edit = |p: PathBuf, f: &dyn Fn(&mut Value)| {
            let mut v: Value = serde_json::from_slice(&fs::read(&p).expect("read")).expect("json");
            f(&mut v);
            fs::write(p, v.to_string()).expect("write");
        };
        edit(k2.join("case.json"), &|v| {
            v["initial"]["commit"] = json!(li.commit);
            v["initial"]["tree"] = json!(li.tree);
            v["initial"]["submodules"] = json!([]);
            v["snapshot"]["repo_commit"] = json!(li.commit);
        });
        edit(cases.join("final").join(K4).join("case.json"), &|v| {
            v["inputs"][0]["sha256"] = json!(format!(
                "{:x}",
                Sha256::digest(b"fake archive for the test")
            ));
        });
        let (m, k) = (d.join("materials"), d.join("kb"));
        for case in [K2, K4] {
            copy(
                &c.join("cases/final").join(case).join("materials"),
                &m.join(case),
            );
        }
        copy(&c.join("cases/final").join(K2).join("kb"), &k);
        preflight_setup::pin_trial_kb(&d.join("final.json"), &cases, &k);
        // 測定 daemon の `[api] token_file`（`Config::load` が読めること。値は試験用）。
        fs::create_dir_all(d.join("measure")).expect("measure dir");
        fs::write(d.join("measure/token"), "t\n").expect("token");
        let measure = preflight_setup::write_configs(
            d,
            &preflight_setup::measure_config(d, "measure")
                .replace("127.0.0.1:7701", &format!("127.0.0.1:{}", server.port)),
            &preflight_setup::normal_config(d),
        );
        fs::write(
            d.join("task-template.json"),
            json!({"project_id":"01ABC","acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
        )
        .expect("template");
        let inp = Inputs {
            manifest_path: d.join("final.json"),
            input_override: None,
            cases_root: cases,
            grader_root: c.join("fixtures/grader-private"),
            grade_cases_root: grade,
            a_program: "claude".into(),
            api_url: Some(format!("http://127.0.0.1:{}", server.port)),
            token_env: Some("CELERIS_CAMPAIGN_TOKEN".into()),
            task_template: Some(d.join("task-template.json")),
            repo_src: Some(repo),
            materials_src: Some(m),
            kb_src: Some(k),
            inputs_src: Some(ins),
            a_account: Some("lab".into()),
            measure: Some(measure),
            measurement_id: None,
            cap_env: Some(preflight_setup::write_cap_env(d)),
        };
        Self {
            interior: Interior {
                store,
                http: http.clone(),
                children: Mutex::default(),
                runs: AtomicU32::new(0),
            },
            dir,
            home,
            server,
            http,
            a: ArmA::default(),
            inp,
        }
    }

    fn manifest(&self) -> CampaignManifest {
        CampaignManifest::load(&self.inp.manifest_path).expect("manifest")
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    fn repo(&self) -> PathBuf {
        self.inp.repo_src.clone().expect("repo")
    }

    /// CLI の `run|watch --executor celeris` と同じ順序: gate → 期待 lock → RunOptions → watch。
    /// 実行器は実 HTTP の `CelerisExecutor`（B/C）と mock の A harness。
    fn drive(&self, inp: &Inputs, max_iterations: u32) -> Result<WatchReport, String> {
        let m = self.manifest();
        preflight::gate(inp, &OkHost)?;
        let input = m.load_input(&inp.manifest_path, None)?;
        let set = CaseSet::load(&inp.cases_root, &inp.grader_root)?;
        let opts = RunOptions {
            out: self.out(),
            repo_src: inp.repo_src.clone(),
            materials_src: inp.materials_src.clone(),
            kb_src: inp.kb_src.clone(),
            inputs_src: inp.inputs_src.clone(),
            case_inputs: inputs::for_problems(&set, &m.problems),
            lock: Some(preflight::expected_lock(inp, &m)?),
        };
        let template = json!({"acceptance": [{"type": "artifact_exists", "name": "result.txt"}]})
            .as_object()
            .cloned()
            .unwrap_or_default();
        let bc = CelerisExecutor::new(
            Shared(self.http.clone()),
            template,
            1800,
            self.out().join(CLUSTER_CANCEL_FILE),
        );
        let ex = ByArm {
            a: &self.a,
            bc: &bc,
        };
        tokio::task::block_in_place(|| {
            watch(
                &m,
                &input,
                &ex,
                &opts,
                WatchOptions {
                    interval_secs: 1,
                    max_iterations,
                },
                &StepClock(&self.interior),
            )
        })
    }

    fn roots(&self) -> Vec<task_core::Task> {
        self.interior
            .store
            .list(None)
            .expect("list")
            .into_iter()
            .filter(|t| t.parent_id.is_none())
            .collect()
    }

    fn all_tasks(&self) -> usize {
        self.interior.store.list(None).expect("list").len()
    }

    /// 外部実行（POST・A harness の投入）と task の合計。拒否例はこれが変わらない。
    fn side_effects(&self) -> (u32, usize, usize) {
        (
            self.http.posts.load(Ordering::SeqCst),
            self.a.submitted(),
            self.all_tasks(),
        )
    }

    fn expected(&self) -> CampaignLock {
        preflight::expected_lock(&self.inp, &self.manifest()).expect("expected lock")
    }

    fn grade(&self, cases: &Path) -> (Value, usize) {
        let r = CountRunner::new();
        let v = grade_out_expected(
            &self.manifest(),
            &self.out(),
            cases,
            &r,
            Some(&self.expected()),
        )
        .expect("grade-out");
        (v, r.calls())
    }

    fn violations(&self) -> Vec<String> {
        check_expected(&self.manifest(), &self.out(), Some(&self.expected())).violations
    }

    async fn finish(mut self) {
        self.server.shutdown().await;
        drop(self.home);
    }
}

fn grade_lines(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join(GRADES_FILE))
        .expect("grades")
        .lines()
        .map(|l| serde_json::from_str(l).expect("line"))
        .collect()
}

fn write_lines(out: &Path, v: &[Value]) {
    let s: String = v.iter().map(|x| format!("{x}\n")).collect();
    fs::write(out.join(GRADES_FILE), s).expect("grades");
}

// ---------------------------------------------------------------- 場面

/// 正例: 正しい入力 → lock → 通常 POST/task 追跡 → 完了 artifact hash → grade-out → bind → 最終 export。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locked_measurement_runs_through_task_api_to_the_final_export() {
    let w = World::new().await;
    let expected = w.expected();
    let r = w.drive(&w.inp, 200).expect("locked watch");
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    assert!(r.iterations > 1, "外部 task の完了を待つ");

    // lock は実行前に保存され、期待 lock と同一。campaign に結合する。
    let saved: CampaignLock =
        serde_json::from_slice(&fs::read(w.out().join(LOCK_FILE)).expect("lock.json"))
            .expect("lock");
    assert_eq!(saved, expected);
    saved.bind_to_manifest(&w.manifest()).expect("bind");

    // B/C × 2 case の root task が通常の POST /tasks で作られている（A は harness の試行）。
    let roots = w.roots();
    assert_eq!(roots.len(), 4, "B,C × (k2,k4)");
    for t in &roots {
        let id = w
            .interior
            .store
            .task_campaign_resolve(&t.id.to_string())
            .expect("resolve")
            .expect("API が受けた識別");
        assert_eq!(id.campaign_id, "mlsys-final-1");
        assert_eq!(t.status, Status::Done);
    }
    assert_eq!(w.a.submitted(), 2, "A は case ごとに 1 試行");

    // 同一条件の再開では何も投入しない。
    let before = w.side_effects();
    let again = w.drive(&w.inp, 5).expect("resume");
    assert_eq!(again.end, WatchEnd::Settled);
    assert_eq!(w.side_effects(), before, "resume は再投入 0");
    assert_eq!(
        serde_json::from_slice::<CampaignLock>(&fs::read(w.out().join(LOCK_FILE)).expect("lock"))
            .expect("lock"),
        expected,
        "lock は書き換わらない"
    );

    // 完了 artifact を独立 grader（stub command）で採点し、bind・最終 export。
    let (v, calls) = w.grade(&w.inp.grade_cases_root);
    assert!(calls > 0, "正しい定義なら grader command が走る");
    assert_eq!(v["rejected"], json!([]), "{v}");
    assert!(v["graded"].as_u64().expect("graded") > 0);
    for g in grade_lines(&w.out()).iter().filter(|g| g["graded"] == true) {
        let c = expected
            .case(g["case_id"].as_str().expect("case"))
            .expect("case");
        assert_eq!(g["lock_sha256"], expected.sha256.as_str());
        assert_eq!(g["grader_sha256"], c.grader.grade_case_sha256.as_str());
        assert_eq!(g["tests_sha256"], c.tests_sha256().as_str());
    }
    assert_eq!(w.violations(), Vec::<String>::new());
    let dest = w.dir.path().join("export");
    let t = export_expected(&w.manifest(), &w.out(), &dest, Some(&expected)).expect("export");
    assert_eq!(t["lock_verified"], true, "{t}");
    assert!(
        dest.join("table.json").exists() || dest.join("table.incomplete.json").exists(),
        "export の表が書かれる"
    );
    w.finish().await;
}

/// 誤初期 repo・別 submodule・資料・KB・外部入力は、POST も A harness の投入も task 作成も 0、out 未作成で止まる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_initial_repo_submodule_materials_kb_or_inputs_cause_no_post_or_external_run() {
    let w = World::new().await;
    let baseline = w.side_effects();
    assert_eq!(baseline, (0, 0, 0));
    let refused = |what: &str| {
        let err = w.drive(&w.inp, 5).expect_err(what);
        assert_eq!(
            w.side_effects(),
            baseline,
            "{what}: POST/外部実行/task 0 ({err})"
        );
        assert!(!w.out().exists(), "{what}: out・台帳を作らない ({err})");
        err
    };
    let repo = w.repo();
    // 誤初期 repo: 1 byte 違い・余分な file・別の初期木。
    fs::write(repo.join("README"), "k2 initial tree!").expect("w");
    refused("modified repo");
    fs::write(repo.join("README"), "k2 initial tree").expect("w");
    fs::write(repo.join("extra.rs"), "x").expect("w");
    refused("extra file in repo");
    fs::remove_file(repo.join("extra.rs")).expect("rm");
    // 別 submodule（lock が持たない path への展開）。
    fs::create_dir_all(repo.join("lib/pluvio")).expect("mkdir");
    fs::write(repo.join("lib/pluvio/x.rs"), "fn x() {}").expect("w");
    refused("foreign submodule");
    fs::remove_dir_all(repo.join("lib")).expect("rm");
    // 別の repo 全体。
    let other = w.dir.path().join("other-repo");
    fs::create_dir_all(&other).expect("mkdir");
    fs::write(other.join("README"), "unrelated").expect("w");
    git(&other, &["init", "-q"]);
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "o"]);
    let mut wrong = w.inp.clone();
    wrong.repo_src = Some(other);
    let err = w.drive(&wrong, 5).expect_err("another repo");
    assert_eq!(w.side_effects(), baseline, "{err}");
    assert!(!w.out().exists());
    // 資料・KB・外部入力。
    let m = w.inp.materials_src.clone().expect("m");
    fs::write(m.join(K2).join("added.txt"), "x").expect("w");
    let err = refused("materials");
    assert!(err.contains("material"), "{err}");
    fs::remove_file(m.join(K2).join("added.txt")).expect("rm");
    let k = w.inp.kb_src.clone().expect("kb");
    fs::write(k.join("added.txt"), "x").expect("w");
    refused("kb");
    fs::remove_file(k.join("added.txt")).expect("rm");
    let ins = w.inp.inputs_src.clone().expect("ins");
    fs::write(ins.join(K4_INPUT), b"another archive").expect("w");
    refused("external input");
    fs::write(ins.join(K4_INPUT), b"fake archive for the test").expect("w");
    // 対照: 元の入力なら通る（上の拒否が実体差だけによる）。
    let r = w.drive(&w.inp, 200).expect("correct inputs");
    assert_eq!(r.end, WatchEnd::Settled);
    assert_eq!(w.roots().len(), 4);
    w.finish().await;
}

/// 同一 case id/split で grader・公開/隠しテストを差し替えると、grade では command 0 回で拒否、bind・export でも拒否。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn swapped_grader_or_tests_are_rejected_at_grade_bind_and_export() {
    let w = World::new().await;
    assert_eq!(w.drive(&w.inp, 200).expect("watch").end, WatchEnd::Settled);
    let k2 = |root: &Path| root.join("final").join(K2);
    for (name, edit) in [
        ("grader definition", 0),
        ("hidden test body", 1),
        ("same id other split", 2),
        ("removed hidden test", 3),
    ] {
        let alt = w.dir.path().join(format!("alt-{edit}"));
        copy(&w.inp.grade_cases_root, &alt);
        let case = k2(&alt).join(format!("{K2}.json"));
        let mut j: Value = serde_json::from_slice(&fs::read(&case).expect("r")).expect("json");
        match edit {
            0 => j["implementation"]["must_pass"] = json!(["something_else"]),
            1 => fs::write(k2(&alt).join("hidden/.grader/run.sh"), "exit 0\n").expect("w"),
            2 => j["split"] = json!("dev"),
            _ => {
                j["implementation"]["hidden_tests"]
                    .as_array_mut()
                    .expect("hidden")
                    .pop();
            }
        }
        fs::write(&case, j.to_string()).expect("w");
        // 通常入口の期待 lock 作成（CLI は --grade-case-root から作る）も、同一 id の別定義を拒否する。
        let mut alt_inp = w.inp.clone();
        alt_inp.grade_cases_root = alt.clone();
        assert!(
            w.drive(&alt_inp, 3).is_err() || edit == 1,
            "{name}: 別 grader の期待 lock は resume できない"
        );
        // grade-out は（元の期待 lock に対し）差替え済み cases を grader command 実行前に拒否。
        let (v, calls) = w.grade(&alt);
        assert_eq!(calls, 0, "{name}: grader command を走らせない");
        assert_eq!(v["ok"], false, "{name}: {v}");
        assert!(
            v["rejected"]
                .as_array()
                .expect("rejected")
                .iter()
                .any(|r| r.as_str().is_some_and(|s| s.contains(K2))),
            "{name}: {v}"
        );
    }
    assert!(
        !w.out().join(GRADES_FILE).exists()
            || grade_lines(&w.out())
                .iter()
                .all(|g| g["case_id"] != K2 || g["graded"] != true),
        "別 grader の採点を書かない"
    );

    // 正しい採点の後、grade 行の grader/test/lock/artifact を差し替えても bind が拒否し、export は旧最終版を残さない。
    let (_, calls) = w.grade(&w.inp.grade_cases_root);
    assert!(calls > 0);
    let good = grade_lines(&w.out());
    let idx = good
        .iter()
        .position(|g| g["graded"] == true && g["case_id"] == K2)
        .expect("k2 graded");
    let forged: [(&str, &str, &str); 4] = [
        ("grader_sha256", "ab", "locked grader"),
        ("tests_sha256", "cd", "locked tests"),
        ("lock_sha256", "ef", "campaign lock"),
        ("artifact_sha256", "00", "artifact hash"),
    ];
    for (field, byte, needle) in forged {
        let mut v = good.clone();
        v[idx][field] = json!(byte.repeat(32));
        write_lines(&w.out(), &v);
        let viol = w.violations();
        assert!(viol.iter().any(|x| x.contains(needle)), "{field}: {viol:?}");
        let dest = w.dir.path().join(format!("export-{field}"));
        fs::create_dir_all(&dest).expect("mkdir");
        fs::write(dest.join("table.json"), "{}").expect("w");
        assert!(
            export_expected(&w.manifest(), &w.out(), &dest, Some(&w.expected())).is_err(),
            "{field}: export 拒否"
        );
        assert!(
            !dest.join("table.json").exists(),
            "{field}: 旧最終版を残さない"
        );
    }
    write_lines(&w.out(), &good);
    assert!(w.violations().is_empty(), "対照: 元の grade は結合できる");
    w.finish().await;
}

/// lock・入力・定義の変更は、通常入口（gate → 期待 lock → watch）で既存測定の再開を拒否する。再投入 0・lock 不変。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_lock_or_inputs_refuse_to_resume_an_existing_measurement() {
    let w = World::new().await;
    // 途中まで進める（root は投入済み、未完了）。
    let first = w.drive(&w.inp, 2).expect("partial");
    assert_eq!(first.end, WatchEnd::MaxIterations, "{first:?}");
    let before = w.side_effects();
    assert!(before.0 > 0 && before.2 > 0);
    let path = w.out().join(LOCK_FILE);
    let good = fs::read(&path).expect("lock");
    let refuse = |why: &str, inp: &Inputs| {
        let err = w.drive(inp, 3).expect_err(why);
        assert_eq!(w.side_effects(), before, "{why}: 再投入 0 ({err})");
        err
    };

    // lock の改変（封印と合わない）／再封印した別内容。
    let mut v: Value = serde_json::from_slice(&good).expect("json");
    v["cases"][0]["grader"]["private_sha256"] = json!("f".repeat(64));
    fs::write(&path, v.to_string()).expect("w");
    let tampered = fs::read(&path).expect("r");
    refuse("tampered lock", &w.inp);
    let resealed: CampaignLock = serde_json::from_slice::<CampaignLock>(&tampered)
        .expect("lock")
        .seal()
        .expect("seal");
    fs::write(&path, serde_json::to_vec_pretty(&resealed).expect("json")).expect("w");
    let again = fs::read(&path).expect("r");
    let err = refuse("resealed lock", &w.inp);
    assert!(err.contains("saved lock differs"), "{err}");
    assert_eq!(fs::read(&path).expect("r"), again, "拒否しても書き換えない");
    // lock の欠落: 暗黙に新しい lock へ移行しない。
    fs::remove_file(&path).expect("rm");
    let err = refuse("missing lock", &w.inp);
    assert!(err.contains("not adopted"), "{err}");
    assert!(!path.exists());
    fs::write(&path, &good).expect("w");

    // grader・隠しテストの差替え。
    let hidden = w
        .inp
        .grade_cases_root
        .join(format!("final/{K2}/hidden/.grader/run.sh"));
    let orig = fs::read(&hidden).expect("r");
    fs::write(&hidden, b"exit 0\n").expect("w");
    refuse("hidden test swapped", &w.inp);
    fs::write(&hidden, orig).expect("w");
    // 測定 ID の変更は同一測定ではない。
    let mut other = w.inp.clone();
    other.measurement_id = Some("new-measurement".into());
    let err = refuse("measurement id", &other);
    assert!(err.contains("saved lock differs"), "{err}");
    // 入力（repo）の変更。
    fs::write(w.repo().join("README"), "changed").expect("w");
    refuse("repo changed on resume", &w.inp);
    fs::write(w.repo().join("README"), "k2 initial tree").expect("w");

    assert_eq!(
        fs::read(&path).expect("r"),
        good,
        "最後まで lock は元のまま"
    );
    // 元の定義なら同じ task を追って完了する（新規 root を作らない）。
    let roots_before: Vec<TaskId> = w.roots().iter().map(|t| t.id).collect();
    let r = w.drive(&w.inp, 200).expect("resume");
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    let roots_after: Vec<TaskId> = w.roots().iter().map(|t| t.id).collect();
    assert_eq!(
        roots_after[..roots_before.len()],
        roots_before,
        "投入済みの task を追い続ける"
    );
    assert_eq!(roots_after.len(), 4, "B,C × (k2,k4) だけ。二重投入なし");
    w.finish().await;
}
