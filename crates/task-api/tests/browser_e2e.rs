//! ADR-0084 D6: 旧 Phase 2 の durable wait を再現し、API 登録・承認後も
//! 未適合 backend を worker が拒否して lease 発行と承認消費を防ぐ結合試験。
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
    ArtifactRef, BrowserAction, BrowserCapability, BrowserDomainMode, BrowserRun,
    BrowserTaskPolicy, EffectiveProfile, ProgressFields, SqliteStore, Status, Task, TaskId,
    TaskKind, TaskStore,
};
use task_worker::browser_credential::{CredentialSupervisor, UnixLeaseBroker};
use task_worker::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};
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
    auth_calls: Mutex<Vec<(bool, usize, bool, Option<String>)>>,
    live: Mutex<Vec<String>>,
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
            site_policies: Vec::new(),
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
        auth_calls: Mutex::default(),
        live: Mutex::default(),
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
    /// ADR-0080 H3 の記録: (active, その時点の worker 出力件数, 書いた後の control 状態の
    /// auth_section, 区間中に takeover を試した結果)。
    auth_calls: Mutex<Vec<(bool, usize, bool, Option<String>)>>,
    live: Mutex<Vec<String>>,
}
impl EventSink for StoreSink {
    fn browser_auth_section(
        &self,
        run_id: &str,
        session_id: &str,
        active: bool,
    ) -> Result<(), String> {
        let task_id = self.task_id.to_string();
        let key = || task_core::browser_store::BrowserSessionKey {
            task_id: &task_id,
            run_id,
            session_id,
        };
        let state = self
            .store
            .browser_control_auth_section(key(), active)
            .map_err(|e| e.to_string())?;
        // 区間中に人が takeover しようとした: store の control 遷移（API と同じ）で拒否される。
        let takeover = active.then(|| {
            let mut probe = state.clone();
            if probe.phase() == task_core::browser_control::ControlPhase::AgentRunning {
                let _ = probe.apply(
                    &task_core::browser_control::ControlRequest {
                        command: task_core::browser_control::ControlCommand::Pause,
                        expected_version: probe.version(),
                        idempotency_key: "probe-pause".into(),
                    },
                    0,
                );
            }
            match probe.apply(
                &task_core::browser_control::ControlRequest {
                    command: task_core::browser_control::ControlCommand::Takeover {
                        holder: "owner".into(),
                        ttl_secs: None,
                    },
                    expected_version: probe.version(),
                    idempotency_key: "probe-takeover".into(),
                },
                0,
            ) {
                Ok(_) => "accepted".to_string(),
                Err(e) => e.to_string(),
            }
        });
        self.auth_calls.lock().unwrap().push((
            active,
            self.output.lock().unwrap().len(),
            state.auth_section_active(),
            takeover,
        ));
        Ok(())
    }
    fn browser_live(
        &self,
        _run_id: &str,
        _session_id: &str,
        event: &task_core::browser_live::ScrubbedLiveEvent,
    ) {
        self.live
            .lock()
            .unwrap()
            .push(serde_json::to_string(event.as_persisted()).unwrap());
    }
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

/// Routing refusal must happen before the harness runs.
struct Harness;
#[async_trait::async_trait]
impl WorkerAdapter for Harness {
    fn id(&self) -> &str {
        "acp"
    }
    async fn run(
        &self,
        _: RunRequest,
        _: &str,
        _: RunLimits,
        _: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        panic!("uncertified credential backend must never reach the harness");
    }
}

impl World {
    /// dispatcher の一回分: lease を取り（Ready → Running）、browser supervisor を通して harness を走らせる。
    async fn dispatch(&self, run_id: &str) -> (Result<RunOutcome, AdapterError>, Vec<BrowserRun>) {
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
            auth_calls: Mutex::default(),
            live: Mutex::default(),
        };
        let record = self.env.dir.path().join("empty-browser-conformance.json");
        std::fs::write(
            &record,
            br#"{"schema":1,"source":"celeris-browser-conformance","results":[]}"#,
        )
        .unwrap();
        let outcome = task_worker::browser::run_with_executable_record(
            Arc::new(Harness),
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
            &record,
        )
        .await;
        self.worker_output
            .lock()
            .unwrap()
            .push(format!("{outcome:?}"));
        *self.auth_calls.lock().unwrap() = sink.auth_calls.into_inner().unwrap();
        *self.live.lock().unwrap() = sink.live.into_inner().unwrap();
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

    /// Seed a durable wait left by Phase 2; this does not claim a browser ran.
    fn seed_wait(&self, approval: bool) {
        use task_core::browser_wait::{BrowserWaitReason, NewBrowserWait, OperationIntent};
        let req = run_request(self);
        let policy = task_worker::browser_policy::prepare(
            req.context
                .profile
                .as_ref()
                .unwrap()
                .browser
                .as_ref()
                .unwrap(),
            req.context.browser_policy.as_ref(),
            task_worker::browser::SUPPORTED_VERSION,
        )
        .unwrap();
        let credential = self
            .env
            .store
            .browser_waits_for_task(self.task.id)
            .unwrap()
            .last()
            .and_then(|w| w.credential.clone());
        let run_id = if approval { "run-approval" } else { "run-auth" };
        assert!(
            self.env
                .store
                .acquire_lease(self.task.id, run_id, Duration::from_secs(60))
                .unwrap()
        );
        self.env
            .store
            .browser_wait_open(
                self.task.id,
                &NewBrowserWait {
                    work_unit_id: None,
                    run_id: run_id.into(),
                    session_id: "legacy-session".into(),
                    reason: if approval {
                        BrowserWaitReason::WaitingForApproval
                    } else {
                        BrowserWaitReason::WaitingForAuth
                    },
                    origin: ORIGIN.into(),
                    purpose: "Sign in to read the build dashboard".into(),
                    credential_policy_id: Some("pol-login".into()),
                    credential,
                    operation: approval.then(|| OperationIntent {
                        intent_id: "legacy-intent".into(),
                        action: "credential_use".into(),
                        args_digest: None,
                    }),
                    trusted_login: None,
                    policy_revision: policy.binding.revision,
                    policy_hash: policy.binding.hash,
                    owner_id: Some("owner".into()),
                    ttl_secs: None,
                    resume_key: run_id.into(),
                },
                OffsetDateTime::now_utc(),
            )
            .unwrap();
    }

    async fn until_registered(&self) {
        self.seed_wait(false);
        let auth = self.pending().await;
        self.register(&auth).await;
        assert_eq!(self.env.status_of(self.task.id), Status::Ready);
    }

    async fn until_approval(&self) -> Value {
        self.until_registered().await;
        self.seed_wait(true);
        let approval = self.pending().await;
        assert_eq!(approval["reason"], "waiting_for_approval");
        approval
    }

    fn assert_no_credential_execution(&self, run_id: &str) {
        assert!(self.commands(run_id).is_empty());
        assert!(
            !self
                .env
                .workspace(&self.task)
                .join("runs")
                .join(run_id)
                .exists()
        );
        assert!(
            self.journal()
                .iter()
                .all(|r| !["grant", "use"].contains(&r["action"].as_str().unwrap()))
        );
        assert!(self.auth_calls.lock().unwrap().is_empty());
        assert!(self.live.lock().unwrap().is_empty());
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
        // Routing denial must not create run directories; any existing files are still scanned.
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
async fn approved_credential_requires_conformance_and_does_not_consume_approval() {
    let w = world(site());
    let approval = w.until_approval().await;
    w.decide(&approval, "approve_once").await;
    let (outcome, browsers) = w.dispatch("run-login").await;
    assert!(
        outcome
            .unwrap_err()
            .to_string()
            .contains("browser backend lacks required conformance")
    );
    assert!(browsers.is_empty());
    assert_eq!(
        w.env
            .store
            .browser_waits_for_task(w.task.id)
            .unwrap()
            .last()
            .unwrap()
            .state
            .as_str(),
        "approved"
    );
    let approvals = w
        .env
        .store
        .browser_approvals_for_wait(approval["wait_id"].as_str().unwrap())
        .unwrap();
    assert!(!approvals.is_empty());
    assert!(approvals.iter().all(|a| a.consumed_at.is_none()));
    w.assert_no_credential_execution("run-login");
    w.assert_sentinel_absent_everywhere();
}

#[tokio::test]
async fn registered_credential_does_not_open_approval_for_uncertified_backend() {
    let w = world(site());
    w.until_registered().await;
    let before = w.env.store.browser_waits_for_task(w.task.id).unwrap();
    let (outcome, browsers) = w.dispatch("run-unqualified").await;
    assert!(
        outcome
            .unwrap_err()
            .to_string()
            .contains("browser backend lacks required conformance")
    );
    assert!(browsers.is_empty());
    let after = w.env.store.browser_waits_for_task(w.task.id).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.last().unwrap().state.as_str(), "registered");
    w.assert_no_credential_execution("run-unqualified");
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

/// Store-side H3 contract remains tested while actual injection is unavailable.
/// Worker event suppression is separately exercised by its auth_section tests.
#[test]
fn auth_section_store_denies_takeover_until_closed() {
    let w = world(site());
    let sink = StoreSink {
        store: SqliteStore::open(&w.env.db_path).unwrap(),
        task_id: w.task.id,
        output: w.worker_output.clone(),
        browsers: Mutex::default(),
        auth_calls: Mutex::default(),
        live: Mutex::default(),
    };
    sink.browser_auth_section("run-auth", "session-auth", true)
        .unwrap();
    sink.browser_auth_section("run-auth", "session-auth", false)
        .unwrap();
    let calls = sink.auth_calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].0 && calls[0].2);
    assert_eq!(calls[0].3.as_deref(), Some("auth_section_active"));
    assert!(!calls[1].0 && !calls[1].2);
    assert_eq!(calls[0].1, calls[1].1);
}
