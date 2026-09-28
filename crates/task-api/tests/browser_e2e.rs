//! ADR-0080 Phase 2 の端から端: worker → WAITING_FOR_AUTH → API 手動登録 → 再開 →
//! WAITING_FOR_APPROVAL → API 承認/拒否 → broker lease → plugin bridge → 成功/失敗。
//!
//! broker（celeris-credentiald の IPC と `bridge` 実行ファイル）・store・API は本物。
//! ブラウザだけが fake substrate（`tests/fixtures/fake-agent-browser.py`）と localhost fixture
//! （`site.json`）で、**実 agent-browser での成功を意味しない**。

mod common;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use task_api::browser::{BrowserApiConfig, UnixCredentialBrokerControl};
use task_core::browser_wait::{BrowserWait, BrowserWaitStore, ConsumedBrowserApproval};
use task_core::{
    ArtifactRef, BrowserAction, BrowserCapability, BrowserDomainMode, BrowserRun, BrowserRunState,
    BrowserTaskPolicy, EffectiveProfile, ProgressFields, SqliteStore, Status, Task, TaskId,
    TaskKind, TaskStore,
};
use task_worker::browser_credential::{CredentialSupervisor, UnixLeaseBroker};
use task_worker::{
    AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, Terminal, WorkerAdapter,
};
use time::OffsetDateTime;

const SENTINEL_USER: &str = "SENTINEL-e2e-user-51f0";
const SENTINEL_PASS: &str = "SENTINEL-e2e-pass-a93c";
const ORIGIN: &str = "https://login.example.com";

struct World {
    env: TestEnv,
    app: axum::Router,
    key: ring::signature::Ed25519KeyPair,
    supervisor: CredentialSupervisor,
    broker_root: PathBuf,
    substrate: PathBuf,
    task: Task,
    /// 全 run の worker 出力（進捗・browser lifecycle・harness が見た shim の出力・最終結果）。
    worker_output: Arc<Mutex<Vec<String>>>,
}

fn bridge_binary() -> PathBuf {
    // `cargo test --workspace` builds the broker binary next to the test's deps directory.
    let exe = std::env::current_exe().expect("test exe");
    let bin = exe
        .parent()
        .and_then(Path::parent)
        .expect("target dir")
        .join("celeris-credentiald");
    if !bin.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-q",
                "-p",
                "celeris-credentiald",
                "--bin",
                "celeris-credentiald",
            ])
            .status()
            .expect("cargo build");
        assert!(status.success());
    }
    assert!(bin.exists(), "missing {}", bin.display());
    bin
}

fn keypair() -> ring::signature::Ed25519KeyPair {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
    ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("keypair")
}

fn world(site: Value) -> World {
    use celeris_credentiald::{Broker, ManualProvider, ipc};
    let env = admin_env();
    let root = env.dir.path().to_path_buf();
    let broker_root = root.join("credentiald");
    let runtime = broker_root.join("runtime");
    let config = broker_root.join("config");
    let data = broker_root.join("data");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&broker_root)
        .unwrap();
    for dir in [&runtime, &config, &data] {
        std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
    }
    let manual = ManualProvider::open(config.join("keys"), data.join("vault")).unwrap();
    manual.initialize_key().unwrap();
    let broker = Arc::new(Broker::new(manual, data.join("audit")).unwrap());
    let control = runtime.join("celeris-credentiald/control.sock");
    let serve_runtime = runtime.clone();
    std::thread::spawn(move || ipc::serve(broker, &serve_runtime, vec![std::process::id()]));
    for _ in 0..200 {
        if control.exists() && control.with_file_name("resolve.sock").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(control.exists());
    let key = keypair();
    let app = task_api::router(env.state.clone().with_browser(BrowserApiConfig {
        attestation_public_key: Some(key.public_key().as_ref().to_vec()),
        broker: Some(Arc::new(UnixCredentialBrokerControl {
            socket: control.clone(),
        })),
    }));
    let fixture = root.join("browser-fixture");
    std::fs::create_dir(&fixture).unwrap();
    let substrate = fixture.join("fake-agent-browser");
    std::fs::write(&substrate, include_str!("fixtures/fake-agent-browser.py")).unwrap();
    std::fs::set_permissions(&substrate, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(fixture.join("site.json"), site.to_string()).unwrap();
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    env.seed(&task);
    env.store
        .browser_task_policy_set(task.id, &task_policy())
        .unwrap();
    World {
        supervisor: CredentialSupervisor {
            broker: Arc::new(UnixLeaseBroker {
                control_socket: control,
            }),
            bridge: bridge_binary(),
            runtime_dir: Some(runtime),
        },
        env,
        app,
        key,
        broker_root,
        substrate,
        task,
        worker_output: Arc::default(),
    }
}

fn site() -> Value {
    json!({"pages": {
        "https://login.example.com/": "Sign in",
        "https://login.example.com/dashboard": "Build dashboard",
    }})
}

fn task_policy() -> BrowserTaskPolicy {
    BrowserTaskPolicy {
        policy_id: "login-read".into(),
        revision: 1,
        domain_mode: BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec!["login.example.com".into(), "sso.example.com".into()],
        allowed_actions: vec![
            BrowserAction::Navigate,
            BrowserAction::Snapshot,
            BrowserAction::CredentialUse,
        ],
        approval_actions: vec![],
        credential_policy_ids: vec!["pol-login".into()],
        artifact_policy_id: None,
    }
}

fn run_request(w: &World) -> RunRequest {
    let workspace = w.env.workspace(&w.task);
    RunRequest {
        protocol: task_worker::protocol::PROTOCOL_VERSION,
        task: w.env.store.get(w.task.id).unwrap().unwrap(),
        workspace: workspace.clone(),
        work_dir: None,
        artifacts_dir: workspace.join("artifacts"),
        context: task_worker::protocol::RunContext {
            profile: Some(EffectiveProfile {
                browser: Some(BrowserCapability {
                    allowed_domains: vec!["*.example.com".into()],
                    allowed_actions: Some(vec![
                        BrowserAction::Navigate,
                        BrowserAction::Snapshot,
                        BrowserAction::CredentialUse,
                    ]),
                    credential_policy_ids: vec!["pol-login".into()],
                    live_view_url: Some("https://browser.example.com/live".into()),
                }),
                ..Default::default()
            }),
            browser_policy: w.env.store.browser_task_policy_get(w.task.id).unwrap(),
            ..Default::default()
        },
        cargo_target_dir: None,
    }
}

/// dispatcher の `StoreSink` と同じ store 操作。worker 出力は全部 `output` に写す。
struct StoreSink {
    store: SqliteStore,
    task_id: TaskId,
    output: Arc<Mutex<Vec<String>>>,
    browsers: Mutex<Vec<BrowserRun>>,
}
impl EventSink for StoreSink {
    fn browser_wait_open(
        &self,
        request: &task_core::browser_wait::NewBrowserWait,
    ) -> Result<(), String> {
        self.store
            .browser_wait_open(self.task_id, request, OffsetDateTime::now_utc())
            .map(|_| ())
            .map_err(|e| e.code().into())
    }
    fn browser_waits(&self) -> Result<Vec<BrowserWait>, String> {
        self.store
            .browser_waits_for_task(self.task_id)
            .map_err(|e| e.to_string())
    }
    fn browser_approval_consume(
        &self,
        wait: &BrowserWait,
    ) -> Result<ConsumedBrowserApproval, String> {
        task_core::browser_wait::consume_credential_approval(
            &self.store,
            self.task_id,
            wait,
            OffsetDateTime::now_utc(),
        )
        .map_err(String::from)
    }
    fn browser_updated(&self, browser: &BrowserRun) {
        self.output
            .lock()
            .unwrap()
            .push(serde_json::to_string(browser).unwrap());
        self.browsers.lock().unwrap().push(browser.clone());
    }
    fn progress(&self, msg: &str) {
        self.output.lock().unwrap().push(msg.into());
    }
    fn progress_with(&self, msg: &str, fields: &ProgressFields) {
        self.output
            .lock()
            .unwrap()
            .push(format!("{msg} {}", serde_json::to_string(fields).unwrap()));
    }
    fn artifact(&self, artifact: &ArtifactRef) {
        self.output.lock().unwrap().push(artifact.path.clone());
    }
}

/// モデルの代わり。credential が要ると登録依頼を出して止まり、ログイン後は認証済みページを開く。
struct Harness {
    output: Arc<Mutex<Vec<String>>>,
}
#[async_trait::async_trait]
impl WorkerAdapter for Harness {
    fn id(&self) -> &str {
        "acp"
    }
    async fn run(
        &self,
        req: RunRequest,
        _: &str,
        _: RunLimits,
        _: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let browser = req.context.browser.as_ref().expect("browser context");
        let shim = |args: &[&str]| {
            let out = std::process::Command::new("python3")
                .arg(&browser.cli)
                .args(args)
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            self.output.lock().unwrap().push(text.clone());
            (out.status.success(), text)
        };
        self.output
            .lock()
            .unwrap()
            .push(task_worker::browser::prompt(browser));
        let summary = if browser.credential_used {
            assert!(task_worker::browser::prompt(browser).contains("result: success"));
            let (ok, _) = shim(&["open", "https://login.example.com/dashboard"]);
            assert!(ok, "navigation after login");
            // 認証後の観測（snapshot）は session の終わりまで止まる（ADR-0080 D3）。
            let (ok, text) = shim(&["snapshot"]);
            assert!(!ok, "snapshot must be blocked after credential use: {text}");
            "dashboard reached"
        } else {
            let (ok, text) = shim(&[
                "request-credential",
                "pol-login",
                ORIGIN,
                "Sign in to read the build dashboard",
            ]);
            assert!(ok, "{text}");
            "stopped for credential"
        };
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: summary.into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

impl World {
    /// dispatcher の一回分: lease を取り（Ready → Running）、browser supervisor を通して harness を走らせる。
    async fn dispatch(&self, run_id: &str) -> (RunOutcome, Vec<BrowserRun>) {
        assert!(
            self.env
                .store
                .acquire_lease(self.task.id, run_id, Duration::from_secs(60))
                .unwrap()
        );
        let sink = StoreSink {
            store: SqliteStore::open(&self.env.db_path).unwrap(),
            task_id: self.task.id,
            output: self.worker_output.clone(),
            browsers: Mutex::default(),
        };
        let outcome = task_worker::browser::run_with_executable(
            Arc::new(Harness {
                output: self.worker_output.clone(),
            }),
            run_request(self),
            run_id,
            RunLimits {
                wall_clock: Duration::from_secs(60),
                idle_timeout: Duration::from_secs(30),
                kill_grace: Duration::from_millis(100),
            },
            &sink,
            &self.substrate,
            Some(&self.supervisor),
        )
        .await
        .unwrap();
        self.worker_output
            .lock()
            .unwrap()
            .push(format!("{:?}", outcome.terminal));
        let browsers = sink.browsers.into_inner().unwrap();
        (outcome, browsers)
    }

    fn attest(&self, wait: &Value, decision: &str, nonce: &str) -> Value {
        let payload = json!({
            "actor_id": "owner",
            "owner_session_hash": "owner-session-hash",
            "task_id": self.task.id.to_string(),
            "wait_id": wait["wait_id"],
            "version": wait["version"],
            "decision": decision,
            "policy_hash": wait["policy_hash"],
            "nonce": nonce,
            "expires_at": OffsetDateTime::now_utc().unix_timestamp() + 30,
        })
        .to_string();
        let signature: String = self
            .key
            .sign(payload.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        json!({"payload": payload, "signature": signature})
    }

    /// 人の inbox（`GET /api/v1/browser/waits`）に出ている、この task の wait。
    async fn pending(&self) -> Value {
        let resp = send(&self.app, get_admin("/api/v1/browser/waits")).await;
        assert_eq!(resp.status, 200, "{}", resp.text());
        let body = resp.json();
        let items = body["items"].as_array().expect("items").clone();
        let item = items
            .into_iter()
            .find(|i| i["wait"]["task_id"] == self.task.id.to_string())
            .expect("pending wait for the task");
        assert_eq!(
            item["run_state"],
            item["wait"]["reason"].as_str().unwrap().to_uppercase()
        );
        item["wait"].clone()
    }

    async fn register(&self, wait: &Value) {
        let path = format!(
            "/api/v1/tasks/{}/browser/waits/{}/credential",
            self.task.id,
            wait["wait_id"].as_str().unwrap()
        );
        let body = json!({"expected_version": wait["version"], "username": SENTINEL_USER,
            "password": SENTINEL_PASS, "attestation": self.attest(wait, "register", "nonce-register")});
        let resp = send(&self.app, post_admin(&path, &body)).await;
        assert_eq!(resp.status, 200, "{}", resp.text());
        assert!(!resp.text().contains(SENTINEL_PASS) && !resp.text().contains(SENTINEL_USER));
    }

    async fn decide(&self, wait: &Value, decision: &str) {
        let path = format!(
            "/api/v1/tasks/{}/browser/waits/{}/decision",
            self.task.id,
            wait["wait_id"].as_str().unwrap()
        );
        let body = json!({"decision": decision, "expected_version": wait["version"],
            "idempotency_key": format!("decide-{decision}"),
            "attestation": self.attest(wait, decision, &format!("nonce-{decision}"))});
        let resp = send(&self.app, post_admin(&path, &body)).await;
        assert_eq!(resp.status, 200, "{}", resp.text());
    }

    /// 未登録 → WAITING_FOR_AUTH → 登録 → WAITING_FOR_APPROVAL まで進め、承認待ちの wait を返す。
    async fn until_approval(&self) -> Value {
        let (outcome, browsers) = self.dispatch("run-auth").await;
        assert!(matches!(outcome.terminal, Terminal::Question { .. }));
        assert_eq!(
            browsers.last().unwrap().state,
            BrowserRunState::WaitingForAuth
        );
        assert_eq!(self.env.status_of(self.task.id), Status::Blocked);
        let auth = self.pending().await;
        assert_eq!(auth["reason"], "waiting_for_auth");
        assert_eq!(auth["origin"], ORIGIN);
        assert_eq!(auth["purpose"], "Sign in to read the build dashboard");
        self.register(&auth).await;
        assert_eq!(self.env.status_of(self.task.id), Status::Ready);

        let (outcome, browsers) = self.dispatch("run-approval").await;
        assert!(matches!(outcome.terminal, Terminal::Question { .. }));
        assert_eq!(
            browsers.last().unwrap().state,
            BrowserRunState::WaitingForApproval
        );
        assert_eq!(self.env.status_of(self.task.id), Status::Blocked);
        let approval = self.pending().await;
        assert_eq!(approval["reason"], "waiting_for_approval");
        assert_eq!(approval["operation"]["action"], "credential_use");
        approval
    }

    fn journal(&self) -> Vec<Value> {
        let path = self.broker_root.join("data/audit/journal.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn commands(&self, run_id: &str) -> Vec<Vec<String>> {
        let path = self
            .env
            .workspace(&self.task)
            .join("runs")
            .join(run_id)
            .join("browser/commands.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// tempdir の全ファイル（DB 本体・WAL・shm、workspace・runs・artifacts、broker の vault/journal、
    /// fake substrate の状態）・task の全 event・worker 出力に sentinel が無い。
    fn assert_sentinel_absent_everywhere(&self) {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let ft = entry.file_type().unwrap();
                if ft.is_dir() {
                    walk(&path, out);
                } else if ft.is_file() {
                    out.push(path);
                }
            }
        }
        let root = self.env.dir.path();
        let output = root.join("worker-output.txt");
        std::fs::write(&output, self.worker_output.lock().unwrap().join("\n")).unwrap();
        let mut files = Vec::new();
        walk(root, &mut files);
        for must in ["celeris.db", "worker-output.txt"] {
            assert!(
                files.iter().any(|p| p.ends_with(must)),
                "scan must cover {must}"
            );
        }
        assert!(
            files
                .iter()
                .any(|p| p.parent().is_some_and(|d| d.ends_with("vault"))),
            "scan must cover the vault"
        );
        assert!(
            files.iter().any(|p| p.to_string_lossy().contains("/runs/")),
            "scan must cover run directories"
        );
        for path in &files {
            let bytes = std::fs::read(path).unwrap();
            for needle in [SENTINEL_USER, SENTINEL_PASS] {
                assert!(
                    !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                    "sentinel leaked to {}",
                    path.display()
                );
            }
        }
        let events =
            serde_json::to_string(&self.env.store.events_for(self.task.id).unwrap()).unwrap();
        assert!(!events.contains(SENTINEL_USER) && !events.contains(SENTINEL_PASS));
        eprintln!(
            "sentinel scan: {} files + events + worker output",
            files.len()
        );
    }
}

#[tokio::test]
async fn unregistered_credential_waits_registers_resumes_and_succeeds() {
    let w = world(site());
    let approval = w.until_approval().await;
    w.decide(&approval, "approve_once").await;
    assert_eq!(w.env.status_of(w.task.id), Status::Ready);

    let (outcome, browsers) = w.dispatch("run-login").await;
    match &outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "dashboard reached"),
        other => panic!("expected success, got {other:?}"),
    }
    let last = browsers.last().unwrap();
    assert_eq!(last.state, BrowserRunState::Completed);
    // The approved session continues; Live View is off once a credential is used.
    assert_eq!(last.session_id, approval["session_id"].as_str().unwrap());
    assert!(browsers.iter().all(|b| b.live_view_url.is_none()));
    let waits = w.env.store.browser_waits_for_task(w.task.id).unwrap();
    assert_eq!(waits.last().unwrap().state.as_str(), "resumed");
    // Supervisor-built segment: exact origin checked before and after `auth login`.
    let commands = w.commands("run-login");
    let verbs: Vec<&str> = commands.iter().map(|c| c[0].as_str()).collect();
    assert_eq!(&verbs[..4], ["open", "get", "auth", "get"], "{commands:?}");
    let state = std::fs::read_to_string(
        w.env
            .workspace(&w.task)
            .join("runs/run-login/browser")
            .join(format!("state-{}.json", last.session_id)),
    )
    .unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["plugin"], "success");
    let digest = {
        use sha2::Digest;
        format!(
            "{:x}",
            sha2::Sha256::digest(format!("{SENTINEL_USER}\0{SENTINEL_PASS}"))
        )
    };
    assert_eq!(
        state["login_digest"], digest,
        "the page received the registered secret"
    );
    let actions: Vec<String> = w
        .journal()
        .iter()
        .map(|r| r["action"].as_str().unwrap().to_string())
        .collect();
    assert!(
        actions.contains(&"grant".into()) && actions.contains(&"use".into()),
        "{actions:?}"
    );
    // The one-time approval is spent: another dispatch cannot reuse it.
    assert!(
        w.env
            .store
            .browser_approvals_for_wait(approval["wait_id"].as_str().unwrap())
            .unwrap()
            .iter()
            .all(|a| a.consumed_at.is_some())
    );
    w.assert_sentinel_absent_everywhere();
}

#[tokio::test]
async fn denied_approval_fails_the_task_without_a_lease() {
    let w = world(site());
    let approval = w.until_approval().await;
    w.decide(&approval, "deny").await;
    assert_eq!(w.env.status_of(w.task.id), Status::Failed);
    let waits = w.env.store.browser_waits_for_task(w.task.id).unwrap();
    assert_eq!(waits.last().unwrap().state.as_str(), "denied");
    assert!(
        w.journal()
            .iter()
            .all(|r| !["grant", "use"].contains(&r["action"].as_str().unwrap())),
        "no lease may be issued after a denial"
    );
    assert!(
        w.commands("run-approval").is_empty(),
        "approval run never starts the browser"
    );
    w.assert_sentinel_absent_everywhere();
}

#[tokio::test]
async fn login_page_on_another_origin_is_denied_before_the_plugin_runs() {
    let mut s = site();
    // The trusted login URL redirects to a different (still allowed) origin.
    s["redirects"] = json!({"https://login.example.com/": "https://sso.example.com/login"});
    let w = world(s);
    let approval = w.until_approval().await;
    w.decide(&approval, "approve_once").await;
    let (outcome, browsers) = w.dispatch("run-login").await;
    match &outcome.terminal {
        Terminal::Error { message, retryable } => {
            assert!(message.contains("origin_mismatch"), "{message}");
            assert!(!retryable);
        }
        other => panic!("expected denial, got {other:?}"),
    }
    assert_eq!(browsers.last().unwrap().state, BrowserRunState::Failed);
    let commands = w.commands("run-login");
    assert!(
        commands.iter().all(|c| c[0] != "auth"),
        "auth login must not run: {commands:?}"
    );
    let actions: Vec<String> = w
        .journal()
        .iter()
        .map(|r| r["action"].as_str().unwrap().to_string())
        .collect();
    assert!(actions.contains(&"revoke".into()), "{actions:?}");
    assert!(!actions.contains(&"use".into()), "{actions:?}");
    let progress = w.worker_output.lock().unwrap().join("\n");
    assert!(progress.contains("browser.credential_use: failure"));
    w.assert_sentinel_absent_everywhere();
}
