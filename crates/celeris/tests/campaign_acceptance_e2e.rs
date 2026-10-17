//! 監査修正 6b: 通常経路の通し受け入れ試験。
//!
//! 通常の `POST /tasks`（実 HTTP・一時 daemon の API・temp DB）→ 子 task・run 追跡 → 成果物（試行 workspace）
//! → 独立 grader（`grade_out`）→ 最終 export（`checker`/`export`）を、campaign の実行器（`CelerisExecutor`）が
//! 実 HTTP で駆動する。識別・routing 固定・冪等キーは API が受け取った値で、`task_campaigns` などへの直挿入は無い。
//!
//! mock にしてよい「実行内部」だけを test が担う: dispatcher 相当の task の状態遷移（`apply_transition`）、
//! worker の run イベントと成果物の書き込み、A arm の harness、grader の評価 command。
//! 時計は注入（`Clock::sleep` が mock の実行内部を 1 歩進める）で、実時間を待たず、CPU 負荷も掛けない。
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use celeris::Config;
use celeris_campaign::{
    aggregate::Outcome,
    celeris_exec::{CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport},
    checker::check,
    executor::{
        ExecError, ExecResult, ExecStatus, Executor, ObservedRun, TaskRef, TokenUsage,
        TrialRequest, Usage,
    },
    export::{export, export_with},
    final_accounting::{FinalInputs, compute, recheck},
    grade_out::grade_out,
    grader::{CommandOutput, CommandRunner},
    ledger::{Ledger, LedgerState},
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config as ArmConfig, ConfigId, InputRef,
        Secondary, SnapshotRef, Split, Termination, TokenBasis, manifest_hash,
    },
    runner::RunOptions,
    watch::{Clock, WatchEnd, WatchOptions, WatchReport, watch},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use task_core::{
    DaemonMode, Event, InstanceRole, RunEnd, RunMetrics, RunRole, SharedRole, SqliteStore, Status,
    TaskId, TaskStore, Trigger, Usage as RunUsage,
    decision_log::{
        CampaignVerdict, CandidateRef, DecisionLogOutcome, DecisionLogProposal, DecisionLogUsage,
        UsageRole,
    },
    decision_log_export::{ExportParams, verify_connected},
};
use time::OffsetDateTime;

const TOKEN: &str = "campaign-acceptance-token";

// ---------------------------------------------------------------- 一時 daemon（実 HTTP の task-api）

struct Server {
    port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
}

impl Server {
    /// `port` が 0 なら空き port、非 0 なら同じ port で起動し直す（再起動の再現）。
    async fn start(home: &Path, port: u16) -> Self {
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
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let settings = celeris::api_settings(
            &config,
            addr,
            Some(TOKEN.to_string()),
            "01J00000000000000000000000".to_string(),
            "2026-10-16T00:00:00Z".to_string(),
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

// ---------------------------------------------------------------- 実 HTTP の Transport（障害注入つき）

#[derive(Default)]
struct Http {
    port: u16,
    /// 次の n 回の GET は接続できない（通信断。要求は届かない）。
    fail_gets: AtomicU32,
    /// 次の n 回の POST は処理されるが応答だけ失う（応答喪失。要求は届いている）。
    lose_post_responses: AtomicU32,
    posts: AtomicU32,
}

fn take(c: &AtomicU32) -> bool {
    c.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
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
        if method == "GET" && take(&self.fail_gets) {
            return Err("connection refused (injected)".into());
        }
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
            if take(&self.lose_post_responses) {
                return Err("connection reset (response lost, injected)".into());
            }
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

/// A は mock、B/C は実 HTTP の CelerisExecutor へ振り分ける（CLI の `--executor` 配線と同じ分け方）。
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

#[derive(Clone, Copy, Debug)]
enum Behave {
    /// 正しい成果物を書いて完了。
    Correct,
    /// 実行は完了（executor は passed）だが、成果物は誤り。
    Wrong,
    /// 完了せず、大量の token を使い続ける（budget 停止の再現）。
    Stall(u64),
}

type Plan = Box<dyn Fn(&str, &str) -> Behave>;

struct Interior {
    store: SqliteStore,
    http: Arc<Http>,
    plan: Plan,
    children: Mutex<BTreeMap<TaskId, TaskId>>,
    stalled: Mutex<Vec<TaskId>>,
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

    /// 実行内部を 1 歩進める。Ready → Running → （子の作成・完了）→ 完了。1 回で終わらせない。
    fn step(&self) {
        let now = OffsetDateTime::now_utc();
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
                Status::Running => self.step_root(&t, now),
                _ => {}
            }
        }
    }

    fn step_root(&self, t: &task_core::Task, _now: OffsetDateTime) {
        let Some(ident) = self
            .store
            .task_campaign_resolve(&t.id.to_string())
            .expect("resolve")
        else {
            return;
        };
        let (arm, case) = (
            ident.arm.clone().unwrap_or_default(),
            ident.case_id.clone().unwrap_or_default(),
        );
        let behave = (self.plan)(&arm, &case);
        let child = self.children.lock().expect("lock").get(&t.id).copied();
        let Some(child) = child else {
            // planner run → 子 task（通常の POST /tasks。parent 指定）。
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
        if let Behave::Stall(tokens) = behave {
            let mut st = self.stalled.lock().expect("lock");
            if !st.contains(&t.id) {
                // 完了しない run が大量に消費する（終端していない root の usage に出る）。
                let r = self.run_id("r-stall");
                self.store
                    .append_event(t.id, &ev_started(&r, None))
                    .expect("event");
                self.store
                    .append_event(t.id, &ev_finished(&r, None, tokens))
                    .expect("event");
                st.push(t.id);
            }
            return;
        }
        let child_done = self
            .store
            .get(child)
            .expect("get")
            .is_some_and(|c| matches!(c.status, Status::Done));
        if !child_done {
            return;
        }
        let WorkspacePath(path) = workspace_path(t);
        fs::write(
            path.join("result.txt"),
            if matches!(behave, Behave::Correct) {
                "correct"
            } else {
                "wrong"
            },
        )
        .expect("artifact");
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

struct WorkspacePath(PathBuf);

fn workspace_path(t: &task_core::Task) -> WorkspacePath {
    match &t.workspace {
        task_core::WorkspaceSpec::Local { path, .. } => WorkspacePath(path.clone()),
        other => panic!("unexpected workspace {other:?}"),
    }
}

struct StepClock<'a>(&'a Interior);

impl Clock for StepClock<'_> {
    fn sleep(&self, _d: Duration) {
        self.0.step();
    }
}

// ---------------------------------------------------------------- 世界（daemon + campaign + 評価）

struct World {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    server: Server,
    http: Arc<Http>,
    interior: Interior,
    m: CampaignManifest,
    input: Input,
    out: PathBuf,
    cases: PathBuf,
    a: ArmA,
}

const PRISTINE_TEST: &str = "#[test]\nfn t_ok() {}\n";

fn input(problems: &[&str]) -> Input {
    let ps: Vec<Value> = problems
        .iter()
        .map(|id| json!({"id":id,"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":ps,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn arm_cfg(id: ConfigId, routing: Option<ArmRouting>) -> ArmConfig {
    ArmConfig {
        id,
        harness: if id == ConfigId::A {
            "claude-code"
        } else {
            "celeris"
        }
        .into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing,
    }
}

struct Limits {
    max_tokens: u64,
    per_case: u64,
}

fn campaign(i: &Input, problems: &[&str], lim: &Limits) -> CampaignManifest {
    let routing = |evidence: bool| ArmRouting {
        lower_policy: Some("mlsys-min-1".into()),
        evidence_table_id: evidence.then(|| "ev-2026-10-15".into()),
        online_update: Some(false),
        ..ArmRouting::default()
    };
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "acceptance".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: problems.iter().map(|p| p.to_string()).collect(),
        configs: vec![
            arm_cfg(ConfigId::A, None),
            arm_cfg(ConfigId::B, Some(routing(false))),
            arm_cfg(ConfigId::C, Some(routing(true))),
        ],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: problems.iter().map(|p| p.to_string()).collect(),
        },
        budget: Budget {
            token_basis: TokenBasis::All,
            max_tokens: lim.max_tokens,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 64,
            per_case: Cap {
                max_tokens: lim.per_case,
                max_wall_secs: 1_800,
            },
            per_arm: Cap {
                max_tokens: lim.max_tokens,
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
            materials_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .into(),
            kb_snapshot: "kb@1".into(),
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    }
}

fn sha(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

fn write(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, s).expect("write");
}

/// 評価側の grade-case（成果物とは別の場所）。`graded` 以外の問題には case を置かない（未採点）。
fn write_grade_case(cases: &Path, id: &str) {
    write(&cases.join("pristine/tests/t.rs"), PRISTINE_TEST);
    write(
        &cases.join(format!("{id}.json")),
        &json!({
            "schema": "celeris.grade-case/1", "id": id, "split": "dev",
            "implementation": {"pristine_dir": "pristine", "repo": ".",
                "tests": [{"path": "tests/t.rs", "sha256": sha(PRISTINE_TEST)}],
                "command": ["cargo", "test"], "must_pass": ["t_ok"]}
        })
        .to_string(),
    );
}

/// 作業 dir の `result.txt` が正解のときだけ全試験 ok を返す疑似 cargo。成果物を書いた者の申告は見ない。
struct Judge;
impl CommandRunner for Judge {
    fn run(&self, _argv: &[String], cwd: &Path) -> Result<CommandOutput, String> {
        let ok = fs::read_to_string(cwd.join("result.txt")).is_ok_and(|t| t == "correct");
        Ok(CommandOutput {
            exit: Some(i32::from(!ok)),
            stdout: if ok {
                "test t_ok ... ok\n".into()
            } else {
                String::new()
            },
            stderr: String::new(),
        })
    }
}

impl World {
    async fn new(problems: &[&str], lim: Limits, plan: Plan) -> Self {
        let home = tempfile::tempdir().expect("home");
        let work = tempfile::tempdir().expect("work");
        let server = Server::start(home.path(), 0).await;
        let http = Arc::new(Http {
            port: server.port,
            ..Http::default()
        });
        let config = Config::load(&home.path().join("config.toml")).expect("load");
        let store = SqliteStore::open(&config.db.path).expect("open");
        let i = input(problems);
        let m = campaign(&i, problems, &lim);
        m.validate().expect("valid manifest");
        let repo = work.path().join("repo-src");
        write(&repo.join("tests/t.rs"), PRISTINE_TEST);
        write(&repo.join("src/lib.rs"), "pub fn f() {}\n");
        write(&repo.join("result.txt"), "unsolved");
        fs::create_dir_all(work.path().join("mat")).expect("mat");
        fs::create_dir_all(work.path().join("kb")).expect("kb");
        let cases = work.path().join("cases");
        let out = work.path().join("out");
        Self {
            interior: Interior {
                store,
                http: http.clone(),
                plan,
                children: Mutex::default(),
                stalled: Mutex::default(),
                runs: AtomicU32::new(0),
            },
            home,
            work,
            server,
            http,
            m,
            input: i,
            out,
            cases,
            a: ArmA::default(),
        }
    }

    fn opts(&self) -> RunOptions {
        RunOptions {
            out: self.out.clone(),
            repo_src: Some(self.work.path().join("repo-src")),
            materials_src: Some(self.work.path().join("mat")),
            kb_src: Some(self.work.path().join("kb")),
            inputs_src: None,
            case_inputs: Default::default(),
            // 制御 fixture（実 case を持たない）。実 case の lock 経路は campaign の lock_execution 試験。
            lock: None,
        }
    }

    fn exec(&self) -> CelerisExecutor<Shared> {
        let template = json!({"acceptance": [{"type": "artifact_exists", "name": "result.txt"}]})
            .as_object()
            .cloned()
            .unwrap_or_else(Map::new);
        CelerisExecutor::new(
            Shared(self.http.clone()),
            template,
            1800,
            self.out.join(CLUSTER_CANCEL_FILE),
        )
    }

    /// 実 HTTP の実行器で `watch`。待ちの間に mock の実行内部を進める。
    fn watch(&self, max_iterations: u32) -> WatchReport {
        let bc = self.exec();
        let ex = ByArm {
            a: &self.a,
            bc: &bc,
        };
        tokio::task::block_in_place(|| {
            watch(
                &self.m,
                &self.input,
                &ex,
                &self.opts(),
                WatchOptions {
                    interval_secs: 1,
                    max_iterations,
                },
                &StepClock(&self.interior),
            )
            .expect("watch")
        })
    }

    fn grade(&self, graded: &[&str]) -> Value {
        for g in graded {
            write_grade_case(&self.cases, g);
        }
        grade_out(&self.m, &self.out, &self.cases, &Judge).expect("grade-out")
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

    fn arm_of(&self, t: &task_core::Task) -> String {
        self.interior
            .store
            .task_campaign_resolve(&t.id.to_string())
            .expect("resolve")
            .and_then(|i| i.arm)
            .expect("arm")
    }

    async fn finish(mut self) {
        self.server.shutdown().await;
        drop(self.home);
    }
}

fn cell<'a>(table: &'a Value, arm: &str) -> &'a Value {
    table["configs"]
        .as_array()
        .expect("configs")
        .iter()
        .find(|r| r["config"] == arm)
        .unwrap_or_else(|| panic!("no row {arm}: {table}"))
}

fn all_correct() -> Plan {
    Box::new(|_, _| Behave::Correct)
}

fn ledger_states(w: &World) -> Vec<LedgerState> {
    Ledger::open(&w.out)
        .expect("ledger")
        .rows()
        .map(|r| r.state)
        .collect()
}

// ---------------------------------------------------------------- 場面

/// routing 側・採点側の writer が書く値（試験用）を積み、dispatcher と同じ投影経路で決定ログへ書く。識別は書かない。
/// 同じ記録の再投影は冪等（`dedup_key` の置換）。
fn project_decision_log(store: &SqliteStore, now: OffsetDateTime) {
    let all: Vec<TaskId> = store
        .list(None)
        .expect("list")
        .into_iter()
        .map(|t| t.id)
        .collect();
    for t in &all {
        for (_, e) in store.events_for(*t).expect("events") {
            if let Event::WorkerStarted { run_id, .. } = e {
                store
                    .decision_log_record_proposal(
                        &DecisionLogProposal {
                            run_id: run_id.clone(),
                            task_id: Some(t.to_string()),
                            proposed_first: Some(CandidateRef {
                                source_id: "src".into(),
                                model_id: "m-exec".into(),
                                harness_id: "claude-code".into(),
                            }),
                            ..Default::default()
                        },
                        now,
                    )
                    .expect("proposal");
                store
                    .decision_log_record_outcome(
                        &DecisionLogOutcome {
                            run_id,
                            campaign_verdict: Some(CampaignVerdict::Pass),
                            ..Default::default()
                        },
                        now,
                    )
                    .expect("outcome");
            }
        }
    }
    for t in &all {
        task_dispatch::record_task_decision_log(store, *t, now + time::Duration::seconds(5))
            .expect("project");
    }
}

/// 正常: POST /tasks（実 HTTP）→ 子 task・run → 成果物 → 独立 grader → 最終 export。接続は decision log 側でも確かめる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normal_path_post_tasks_to_independent_grade_to_final_export() {
    let w = World::new(
        &["p0"],
        Limits {
            max_tokens: 3_000_000,
            per_case: 300_000,
        },
        all_correct(),
    )
    .await;
    let r = w.watch(30);
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    assert!(r.iterations > 1, "at least one wait for the external tasks");
    assert!(ledger_states(&w).iter().all(|s| *s == LedgerState::Done));

    // 識別・routing 固定・冪等キーは通常の作成 API が受けた値（直挿入ではない）。
    let roots = w.roots();
    assert_eq!(roots.len(), 2, "B and C only; A is a harness trial");
    let store = &w.interior.store;
    for t in &roots {
        let arm = w.arm_of(t);
        let id = store
            .task_campaign_resolve(&t.id.to_string())
            .expect("resolve")
            .expect("identity");
        assert_eq!(id.campaign_id, "acceptance");
        assert_eq!(id.case_id.as_deref(), Some("p0"));
        let pin = store
            .task_routing_override(t.id)
            .expect("override")
            .expect("pinned at POST");
        let want = if arm == "B" {
            "fixed"
        } else {
            "decision_assisted"
        };
        assert_eq!(serde_json::to_value(pin.mode).expect("mode"), want);
        // 子・孫は祖先から継ぐ（修正 3。全体 config が逆でも変わらない）。
        let child = store.children(t.id).expect("children");
        assert_eq!(child.len(), 1, "planner's child task exists via the API");
        let resp = w
            .http
            .call(
                "POST",
                "/api/v1/tasks",
                Some(
                    &json!({"title": "g", "objective": "g", "parent": child[0].id.to_string(),
                    "acceptance": [{"type": "artifact_exists", "name": "r"}]}),
                ),
            )
            .expect("grand");
        let g: TaskId = resp.body["id"].as_str().expect("id").parse().expect("id");
        for d in [child[0].id, g] {
            let inh = store
                .task_routing_override(d)
                .expect("override")
                .expect("inherited from the root pin");
            assert_eq!(inh.mode, pin.mode);
            assert_eq!(inh.evidence_table_id, pin.evidence_table_id);
            assert!(!inh.online_update);
        }
    }

    // 成果物 → 独立 grader（実行器の申告は読まない）→ 最終 export。
    let g = w.grade(&["p0"]);
    assert_eq!(
        (
            g["ok"].clone(),
            g["graded"].clone(),
            g["final_pass"].clone()
        ),
        (json!(true), json!(3), json!(3))
    );
    let c = check(&w.m, &w.out);
    assert!(
        c.ok && c.complete,
        "{:?} {:?}",
        c.violations,
        c.incomplete_reasons
    );
    let t = export(&w.m, &w.out, &w.out.join("ex")).expect("export");
    assert_eq!(t["complete"], true);
    for arm in ["A", "B", "C"] {
        let row = cell(&t, arm);
        assert_eq!(
            (&row["n"], &row["final_passed"]),
            (&json!(1), &json!(1)),
            "{row}"
        );
        assert_eq!(row["pass_rate"].as_f64(), Some(1.0));
    }
    assert!(w.out.join("ex/table.json").exists());

    // 木の全 run（planner・子・worker・reviewer）の token は executor の集計と決定ログで一致し、campaign に結合する。
    let ledger = Ledger::open(&w.out).expect("ledger");
    let b_task = ledger
        .rows()
        .find(|r| r.key.arm == ConfigId::B)
        .and_then(|r| r.task_id.clone())
        .expect("B task id");
    assert!(!b_task.starts_with("A-"));
    let now = OffsetDateTime::now_utc();
    project_decision_log(store, now);
    let usage = store
        .decision_log_usage_list(Some("acceptance"))
        .expect("usage");
    let worker_rows: Vec<_> = usage
        .iter()
        .filter(|u| u.role != UsageRole::Cos && u.run_id.is_some())
        .collect();
    // B と C それぞれ planner 50 + 子 300 + worker 200 + reviewer 20（input）。
    assert_eq!(
        worker_rows
            .iter()
            .filter_map(|u| u.input_tokens)
            .sum::<u64>(),
        2 * (50 + 300 + 200 + 20)
    );
    let export = store
        .decision_log_export_campaign(&ExportParams {
            campaign_id: "acceptance".into(),
            manifest_hash: Some(w.m.compute_sha256().expect("hash")),
            export_cursor: None,
        })
        .expect("decision-log export");
    let problems = verify_connected(&export);
    assert!(problems.is_empty(), "{problems:?}");
    w.finish().await;
}

/// 誤成果物: 実行は完了（executor passed）でも、独立 grader が不合格なら成功にならない。分母は残る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_artifact_completed_run_is_not_a_success() {
    let w = World::new(
        &["p0"],
        Limits {
            max_tokens: 3_000_000,
            per_case: 300_000,
        },
        Box::new(|arm, _| {
            if arm == "B" {
                Behave::Wrong
            } else {
                Behave::Correct
            }
        }),
    )
    .await;
    assert_eq!(w.watch(30).end, WatchEnd::Settled);
    let g = w.grade(&["p0"]);
    assert_eq!(
        (g["graded"].clone(), g["final_pass"].clone()),
        (json!(3), json!(2))
    );
    let t = export(&w.m, &w.out, &w.out.join("ex")).expect("export");
    assert_eq!(t["complete"], true, "{t}");
    let b = cell(&t, "B");
    assert_eq!(b["passed"], 1, "executor says passed: {b}");
    assert_eq!(
        (&b["final_passed"], &b["grader_failed"], &b["n"]),
        (&json!(0), &json!(1), &json!(1))
    );
    assert_eq!(b["pass_rate"].as_f64(), Some(0.0));
    for arm in ["A", "C"] {
        assert_eq!(cell(&t, arm)["final_passed"], 1);
    }
    w.finish().await;
}

/// 未採点: grade-case の無い問題は成功にも分母除外にもならず、最終 export を作らない。揃えると最終版が出る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ungraded_trials_block_the_final_export_until_graded() {
    let w = World::new(
        &["p0", "p1"],
        Limits {
            max_tokens: 3_000_000,
            per_case: 300_000,
        },
        all_correct(),
    )
    .await;
    assert_eq!(w.watch(40).end, WatchEnd::Settled);
    let g = w.grade(&["p0"]);
    assert_eq!(g["ok"], false, "p1 has no grade-case: {g}");
    assert_eq!(g["missing_cases"], json!(["p1"]));
    let c = check(&w.m, &w.out);
    assert!(c.ok && !c.complete, "{:?}", c.violations);
    let t = export(&w.m, &w.out, &w.out.join("ex")).expect("export");
    assert_eq!(t["complete"], false);
    assert!(w.out.join("ex/table.incomplete.json").exists());
    assert!(!w.out.join("ex/table.json").exists());
    for arm in ["A", "B", "C"] {
        let row = cell(&t, arm);
        assert_eq!(
            (&row["n"], &row["final_passed"], &row["ungraded"]),
            (&json!(2), &json!(1), &json!(1)),
            "{row}"
        );
    }
    let g = w.grade(&["p0", "p1"]);
    assert_eq!(g["ok"], true, "{g}");
    let t = export(&w.m, &w.out, &w.out.join("ex")).expect("export");
    assert_eq!(t["complete"], true, "{t}");
    assert!(
        w.out.join("ex/table.json").exists() && !w.out.join("ex/table.incomplete.json").exists()
    );
    assert_eq!(cell(&t, "B")["final_passed"], 2);
    w.finish().await;
}

/// 通信障害: POST の応答喪失・GET の断が続いても、同じ task を追い続け、二重投入せず、最終 export まで進む。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn communication_failures_keep_tracking_and_never_double_submit() {
    let w = World::new(
        &["p0"],
        Limits {
            max_tokens: 3_000_000,
            per_case: 300_000,
        },
        all_correct(),
    )
    .await;
    // 最初の POST（B）の応答を失う。その後、GET が何度か断になる。
    w.http.lose_post_responses.store(1, Ordering::SeqCst);
    let first = w.watch(1);
    assert_eq!(first.end, WatchEnd::MaxIterations);
    w.http.fail_gets.store(6, Ordering::SeqCst);
    let r = w.watch(60);
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    assert!(ledger_states(&w).iter().all(|s| *s == LedgerState::Done));
    // 応答を失った投入も含めて、B・C とも root task は 1 つだけ（label の冪等キーで寄せる）。
    let roots = w.roots();
    assert_eq!(roots.len(), 2, "no duplicate submissions");
    let mut arms: Vec<String> = roots.iter().map(|t| w.arm_of(t)).collect();
    arms.sort();
    assert_eq!(arms, ["B", "C"]);
    assert_eq!(
        roots
            .iter()
            .filter(|t| t.status == Status::Cancelled)
            .count(),
        0,
        "a communication failure is not a cancel"
    );
    w.grade(&["p0"]);
    let t = export(&w.m, &w.out, &w.out.join("ex")).expect("export");
    assert_eq!(t["complete"], true, "{t}");
    assert_eq!(cell(&t, "B")["final_passed"], 1);
    w.finish().await;
}

/// daemon 再起動: 同じ port で API を起動し直しても、campaign は台帳から同じ task を追い、再投入しない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_resumes_the_same_tasks_without_resubmitting() {
    let mut w = World::new(
        &["p0"],
        Limits {
            max_tokens: 3_000_000,
            per_case: 300_000,
        },
        all_correct(),
    )
    .await;
    let first = w.watch(2);
    assert_eq!(first.end, WatchEnd::MaxIterations, "{first:?}");
    let before: Vec<TaskId> = w.roots().iter().map(|t| t.id).collect();
    assert_eq!(before.len(), 2);
    let posts_before = w.http.posts.load(Ordering::SeqCst);
    // daemon（API）を止めて、同じ DB・同じ port で起動し直す。campaign 側の process も作り直す（新しい executor）。
    let port = w.server.port;
    w.server.shutdown().await;
    w.server = Server::start(w.home.path(), port).await;
    assert_eq!(w.server.port, port);
    let r = w.watch(60);
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    let after: Vec<TaskId> = w.roots().iter().map(|t| t.id).collect();
    assert_eq!(before, after, "same tasks, none added");
    let root_posts = w.http.posts.load(Ordering::SeqCst) - posts_before;
    // 再起動後に増えた POST は、planner の子 task の作成（mock の実行内部）だけ。root の再投入は無い。
    assert_eq!(root_posts, 2, "only the interior's two child POSTs");
    w.grade(&["p0"]);
    let t = export(&w.m, &w.out, &w.out.join("ex")).expect("export");
    assert_eq!(t["complete"], true, "{t}");
    for arm in ["A", "B", "C"] {
        assert_eq!(cell(&t, arm)["final_passed"], 1);
    }
    w.finish().await;
}

/// budget 停止: 上限を超えて消費する試行は実 API の cancel で閉じ、停止を確認するまで台帳を閉じない。
/// 停止後は新しい試行を投入しない。消費は欠測を 0 にせず、分母は残る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_stop_cancels_through_the_api_and_stops_further_submissions() {
    let w = World::new(
        &["p0", "p1"],
        Limits {
            max_tokens: 100_000,
            per_case: 50_000,
        },
        Box::new(|arm, case| {
            if arm == "B" && case == "p0" {
                Behave::Stall(80_000)
            } else {
                Behave::Correct
            }
        }),
    )
    .await;
    let r = w.watch(60);
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    assert!(
        r.last.case_budget_cancelled + r.last.arm_budget_cancelled + r.last.budget_cancelled > 0
            || ledger_states(&w).contains(&LedgerState::Cancelled),
        "{r:?}"
    );
    // 停止は実 API の cancel（task 状態が Cancelled、子孫も閉じる）。
    let cancelled: Vec<_> = w
        .roots()
        .into_iter()
        .filter(|t| t.status == Status::Cancelled)
        .collect();
    assert!(
        !cancelled.is_empty(),
        "B/p0 was cancelled via POST /tasks/{{id}}/cancel"
    );
    for t in &cancelled {
        for c in w.interior.store.children(t.id).expect("children") {
            assert!(
                matches!(c.status, Status::Cancelled | Status::Done),
                "descendant closed: {:?}",
                c.status
            );
        }
    }
    assert!(
        Ledger::open(&w.out)
            .expect("ledger")
            .rows()
            .all(|r| r.state.is_terminal())
    );
    // 予算を使い切った後の試行は投入されない（root は計画 4 本（B/C × p0/p1）に満たない）。
    assert!(
        w.roots().len() < 4,
        "no submissions after the budget stop: {}",
        w.roots().len()
    );
    // 打ち切りも分母に残り、最終版は出ない（実行器 passed でない cell が有る）か、出ても成功に数えない。
    w.grade(&["p0", "p1"]);
    let t = export(&w.m, &w.out, &w.out.join("ex")).expect("export");
    let b = cell(&t, "B");
    assert_eq!(b["n"], 2);
    assert!(b["final_passed"].as_u64().unwrap_or(0) < 2, "{b}");
    w.finish().await;
}

/// 全体 config を逆 mode にしても arm の routing は維持される。dispatcher の解決（`routing_for`）は
/// `root_routing_pin_is_inherited_by_child_grandchild_and_retry_whatever_the_config`（task-dispatch
/// lane_assist.rs）が両方向（config=B/C の逆）で固定し、実測の照合（別 model・別 harness・config への fallback を
/// 違反にする）は `routing_pin.rs` / `harness_exec.rs`（campaign）が固定する。ここでは実 API 経由の投入が
/// arm の固定を task 欄と台帳に載せること（全体 config に依らない形）を通しで確かめる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_pin_is_set_on_the_task_through_the_api_not_by_the_global_config() {
    let w = World::new(
        &["p0"],
        Limits {
            max_tokens: 3_000_000,
            per_case: 300_000,
        },
        all_correct(),
    )
    .await;
    assert_eq!(w.watch(30).end, WatchEnd::Settled);
    for t in w.roots() {
        let o = w
            .interior
            .store
            .task_routing_override(t.id)
            .expect("override")
            .expect("pinned");
        let want = match w.arm_of(&t).as_str() {
            "B" => "fixed",
            _ => "decision_assisted",
        };
        assert_eq!(serde_json::to_value(o.mode).expect("mode"), want);
        assert!(!o.online_update);
    }
    // 台帳に残る試行の routing 固定は manifest の宣言どおり（B=fixed, C=decision_assisted + 根拠表）。
    let ledger = Ledger::open(&w.out).expect("ledger");
    for r in ledger.rows().filter(|r| r.key.arm != ConfigId::A) {
        let want = if r.key.arm == ConfigId::B {
            "fixed"
        } else {
            "decision_assisted"
        };
        assert_eq!(r.routing["mode"], want);
        assert_eq!(r.routing["online_update"], false);
    }
    w.finish().await;
}

// ---------------------------------------------------------------- 最終集計（final-accounting）

/// 正式 export（`decision_log_export_campaign`）から最終集計を作る。CLI の `final-accounting` / `export --records` と
/// 同じ関数（`compute` → `recheck`）で、records.jsonl を経由する。
fn final_accounting(w: &World, cos_basis: Option<&str>) -> (Value, String) {
    let export = w
        .interior
        .store
        .decision_log_export_campaign(&ExportParams {
            campaign_id: "acceptance".into(),
            manifest_hash: Some(w.m.compute_sha256().expect("hash")),
            export_cursor: None,
        })
        .expect("decision-log export");
    let jsonl = export.records_jsonl();
    let a = compute(
        &w.m,
        &w.out,
        &FinalInputs {
            records_jsonl: &jsonl,
            cos_not_called_basis: cos_basis,
            quota: None,
        },
    )
    .expect("compute");
    let problems = recheck(&a);
    assert!(problems.is_empty(), "{problems:?}");
    (a, jsonl)
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("not a number: {v}"))
}

fn gap_codes(a: &Value) -> Vec<String> {
    a["gaps"]
        .as_array()
        .expect("gaps")
        .iter()
        .map(|g| {
            format!(
                "{}:{}",
                g["metric"].as_str().unwrap_or(""),
                g["code"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// C arm の root task（Jev で分類される側）。
fn c_root(w: &World) -> String {
    w.roots()
        .iter()
        .find(|t| w.arm_of(t) == "C")
        .expect("C root")
        .id
        .to_string()
}

/// 実 router で作った tree の run を既存 writer で決定ログへ投影し、Jev・CoS を欠測/観測で変えて最終集計を再生成する。
/// 実 dispatcher・実 LLM・実 CoS は通っていない（run イベントと Jev/CoS 行は試験が既存 writer API で積む）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_accounting_is_regenerated_from_the_router_tree_with_jev_cos_gaps() {
    let w = World::new(
        &["p0"],
        Limits {
            max_tokens: 3_000_000,
            per_case: 300_000,
        },
        all_correct(),
    )
    .await;
    let r = w.watch(30);
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    w.grade(&["p0"]);
    let store = &w.interior.store;
    let now = OffsetDateTime::now_utc();
    project_decision_log(store, now);

    // 外側 trial: A=15（harness 試行。run 内訳なし）、B=C=574（input 570 + output 4）。
    // B/C の木は planner 50 + 子 300 + worker 200 + reviewer 20（input）+ 各 run output 1 ×4。
    const OUTER: u64 = 15 + 574 + 574;

    // 1. Jev/CoS の行が一切無い: CoS は呼ばれなかった根拠が無く欠測、C の Jev も未取得 → 総費用削減は未確定。
    let (gap, _) = final_accounting(&w, None);
    assert_eq!(n(&gap["observed_sum"]["tokens"]), OUTER);
    assert_eq!(gap["total"]["tokens"], Value::Null);
    assert_eq!(gap["reduction_confirmable"]["tokens"], false);
    let codes = gap_codes(&gap);
    assert!(
        codes.contains(&"all:cos_not_retrieved".to_string()),
        "{codes:?}"
    );
    assert!(codes.contains(&"all:jev_unknown".to_string()), "{codes:?}");
    // 最終 cutoff と実行中の見え方の差: 台帳の最後の行は全て done なので予約は残らず、差は 0。
    assert_eq!(n(&gap["budget"]["runtime_view"]["charged_tokens"]), OUTER);
    assert_eq!(
        n(&gap["budget"]["final_cutoff"]["observed_trial_tokens"]),
        OUTER
    );
    assert_eq!(
        n(&gap["budget"]["final_cutoff"]["unreleased_reservations"]),
        0
    );
    assert_eq!(
        n(&gap["budget"]["difference"]["reservation_tokens_not_in_final"]),
        0
    );
    assert_eq!(
        gap["budget"]["difference"]["runtime_charged_minus_observed_tokens"],
        0
    );
    assert!(
        !codes.contains(&"all:ledger_unfinished".to_string()),
        "{codes:?}"
    );

    // 2. Jev の行はあるが tokens が取れていない（欠測）+ CoS 根拠あり: Jev の tokens 欠測が残り、削減は未確定。
    let cid = c_root(&w);
    let mut jev = DecisionLogUsage::jev_call(&cid, 1, &cid, Some("acceptance"), Some(0.002));
    store
        .decision_log_record_usage(&jev, now)
        .expect("jev usage (tokens missing)");
    let (jev_missing, _) =
        final_accounting(&w, Some("CoS session log for the campaign window is empty"));
    assert_eq!(jev_missing["total"]["tokens"], Value::Null);
    assert_eq!(jev_missing["reduction_confirmable"]["tokens"], false);
    assert_eq!(n(&jev_missing["observed_sum"]["tokens"]), OUTER);
    assert_eq!(
        jev_missing["shared_unattributable"]["cos"]["state"],
        "not_called"
    );
    assert!(
        gap_codes(&jev_missing).contains(&"tokens:metric_missing".to_string()),
        "{:?}",
        gap_codes(&jev_missing)
    );

    // 3. Jev の tokens（2+3）が観測され、CoS の根拠もある: tokens の総量は確定し、削減が確かめられる。
    jev.input_tokens = Some(2);
    jev.output_tokens = Some(3);
    jev.cache_read_tokens = Some(0);
    jev.cache_creation_tokens = Some(0);
    store
        .decision_log_record_usage(&jev, now)
        .expect("jev usage (observed)");
    let (done, jsonl) =
        final_accounting(&w, Some("CoS session log for the campaign window is empty"));
    assert_eq!(n(&done["total"]["tokens"]), OUTER + 5);
    assert_eq!(done["total"]["complete"]["tokens"], true);
    assert_eq!(done["reduction_confirmable"]["tokens"], true);
    // A は Celeris の外で走り run 内訳が無いので、費用の総額は確定しない（0 にしない）。
    assert_eq!(done["total"]["complete"]["list_cost_usd"], false);
    assert_eq!(done["total"]["list_cost_micro_usd"], Value::Null);
    // 外側と decision-log の run は照合だけで加算しない（B/C 一致、A は内訳なし）。
    assert_eq!(
        done["trials"]["reconcile"],
        json!({"match": 2, "no_breakdown": 1})
    );
    assert_eq!(
        n(&done["by_role"]["worker"]["basis_tokens"]["sum"]),
        2 * (200 + 300 + 2)
    );
    assert_eq!(
        n(&done["by_role"]["planner"]["basis_tokens"]["sum"]),
        2 * 51
    );
    assert_eq!(
        n(&done["by_role"]["reviewer"]["basis_tokens"]["sum"]),
        2 * 21
    );
    assert_eq!(done["by_retry"]["additive"], false);
    assert_eq!(n(&done["jev"]["rows"]["basis_tokens"]["sum"]), 5);

    // 4. 同じ記録の再投影・再 export・再集計で同じ合計（二重加算しない）。CLI の export（--records）も同じ値を表に写す。
    project_decision_log(store, now);
    let (again, jsonl2) =
        final_accounting(&w, Some("CoS session log for the campaign window is empty"));
    assert_eq!(again, done);
    assert_eq!(jsonl2, jsonl);
    let doubled = format!("{jsonl}{jsonl}");
    let dup = compute(
        &w.m,
        &w.out,
        &FinalInputs {
            records_jsonl: &doubled,
            cos_not_called_basis: Some("CoS session log for the campaign window is empty"),
            quota: None,
        },
    )
    .expect("compute doubled");
    assert_eq!(dup["total"], done["total"]);
    let t = export_with(&w.m, &w.out, &w.out.join("ex-final"), Some(&done)).expect("export");
    assert_eq!(t["complete"], true);
    assert!(
        w.out
            .join("ex-final")
            .join("final-accounting.json")
            .exists()
    );
    let t2 = export_with(&w.m, &w.out, &w.out.join("ex-final2"), Some(&again)).expect("export");
    assert_eq!(t["consumption"], t2["consumption"]);
    w.finish().await;
}

/// 予算停止: 実行中の予算の見え方（停止した試行を cap まで確保）と最終集計の対象（観測できた token だけ）の差が数値で出る。
/// 欠測の試行を cap で置き換えて総量にしない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_accounting_shows_budget_view_versus_final_cutoff_after_a_budget_stop() {
    let w = World::new(
        &["p0", "p1"],
        Limits {
            max_tokens: 100_000,
            per_case: 50_000,
        },
        Box::new(|arm, case| {
            if arm == "B" && case == "p0" {
                Behave::Stall(80_000)
            } else {
                Behave::Correct
            }
        }),
    )
    .await;
    let r = w.watch(60);
    assert_eq!(r.end, WatchEnd::Settled, "{r:?}");
    w.grade(&["p0", "p1"]);
    project_decision_log(&w.interior.store, OffsetDateTime::now_utc());
    let (a, _) = final_accounting(&w, None);
    let b = &a["budget"];
    let charged = n(&b["runtime_view"]["charged_tokens"]);
    let observed = n(&b["final_cutoff"]["observed_trial_tokens"]);
    // 停止した B/p0 は 80_000 を消費して観測され、cap（50_000）で置き換えない。残りの 3 試行は 67 token。
    assert_eq!((charged, observed), (80_067, 80_067));
    assert_eq!(n(&b["max_tokens"]), 100_000);
    assert_eq!(n(&b["runtime_view"]["snapshot_tokens"]), 80_067);
    assert_eq!(n(&b["runtime_view"]["reserved_in_flight_tokens"]), 0);
    assert_eq!(n(&b["final_cutoff"]["missing_trials"]), 0);
    assert_eq!(b["difference"]["runtime_charged_minus_observed_tokens"], 0);
    assert_eq!(n(&b["runtime_view"]["missing_substituted_tokens"]), 0);
    // Jev（C）と CoS は未取得なので、観測できた合計は総費用ではなく、削減も未確定。
    assert_eq!(n(&a["observed_sum"]["tokens"]), 80_067);
    assert_eq!(a["total"]["tokens"], Value::Null);
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    w.finish().await;
}
