//! Production browser.rs H3 path with real isolated Chromium and controller CDP.
//! The scripted LLM observes only the post-auth request. The only network
//! namespace connection is a loopback HTTPS fixture; no external network exists.

mod common;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use task_api::browser::{BrowserApiConfig, TrustedSitePolicy, UnixCredentialBrokerControl};
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
const ORIGIN: &str = "https://fixture.example.com";
type AuthCall = (bool, usize, bool, Option<String>);

struct World {
    env: TestEnv,
    app: axum::Router,
    key: ring::signature::Ed25519KeyPair,
    supervisor: CredentialSupervisor,
    substrate: PathBuf,
    task: Task,
    /// 全 run の worker 出力（進捗・browser lifecycle・harness が見た shim の出力・最終結果）。
    worker_output: Arc<Mutex<Vec<String>>>,
    auth_calls: Mutex<Vec<AuthCall>>,
    live: Mutex<Vec<String>>,
    llm_input: Arc<Mutex<Vec<Vec<u8>>>>,
    password: String,
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

fn world() -> World {
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
        .expect("test fixture");
    for dir in [&runtime, &config, &data] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .expect("test fixture");
    }
    let manual =
        ManualProvider::open(config.join("keys"), data.join("vault")).expect("test fixture");
    manual.initialize_key().expect("test fixture");
    let broker = Arc::new(Broker::new(manual, data.join("audit")).expect("test fixture"));
    let control = runtime.join("celeris-credentiald/control.sock");
    let serve_runtime = runtime.clone();
    std::thread::spawn(move || {
        ipc::serve_with(
            broker,
            &serve_runtime,
            vec![std::process::id()],
            celeris_credentiald::injection_ipc::Admission::SameUidHarness,
        )
    });
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
        // ADR-0091 D2: the administrator's site policy is daemon configuration handed to the
        // production control client; the HTTP registration cannot name a URL or selector.
        broker: Some(Arc::new(UnixCredentialBrokerControl {
            socket: control.clone(),
            site_policies: vec![TrustedSitePolicy {
                policy_id: "pol-login".into(),
                exact_origin: ORIGIN.into(),
                login_url: format!("{ORIGIN}/login"),
                password_selector: "#password".into(),
                submit_selector: Some("#submit".into()),
            }],
        })),
    }));
    let fixture = root.join("browser-fixture");
    std::fs::create_dir(&fixture).expect("test fixture");
    let substrate = fixture.join("fake-agent-browser");
    std::fs::write(
        &substrate,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo agent-browser 0.38.1; fi\nexit 0\n",
    )
    .expect("write CLI stub");
    std::fs::set_permissions(&substrate, std::fs::Permissions::from_mode(0o755))
        .expect("test fixture");
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    env.seed(&task);
    let password = format!("H3-sentinel-{}", task.id);
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
        substrate,
        task,
        worker_output: Arc::default(),
        auth_calls: Mutex::default(),
        live: Mutex::default(),
        llm_input: Arc::default(),
        password,
    }
}

fn task_policy() -> BrowserTaskPolicy {
    BrowserTaskPolicy {
        policy_id: "login-read".into(),
        revision: 1,
        domain_mode: BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec!["fixture.example.com".into()],
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
        task: w
            .env
            .store
            .get(w.task.id)
            .expect("test fixture")
            .expect("test fixture"),
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
            browser_policy: w
                .env
                .store
                .browser_task_policy_get(w.task.id)
                .expect("test fixture"),
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
    auth_calls: Mutex<Vec<AuthCall>>,
    live: Mutex<Vec<String>>,
}
impl EventSink for StoreSink {
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<std::sync::Arc<dyn task_worker::browser_live::ControlGate>> {
        Some(std::sync::Arc::new(task_worker::browser_live::InMemoryGate::new()))
    }
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
        self.auth_calls.lock().expect("test fixture").push((
            active,
            self.output.lock().expect("test fixture").len(),
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
            .expect("test fixture")
            .push(serde_json::to_string(event.as_persisted()).expect("test fixture"));
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
            .expect("test fixture")
            .push(serde_json::to_string(browser).expect("test fixture"));
        self.browsers
            .lock()
            .expect("test fixture")
            .push(browser.clone());
    }
    fn progress(&self, msg: &str) {
        self.output.lock().expect("test fixture").push(msg.into());
    }
    fn progress_with(&self, msg: &str, fields: &ProgressFields) {
        self.output.lock().expect("test fixture").push(format!(
            "{msg} {}",
            serde_json::to_string(fields).expect("test fixture")
        ));
    }
    fn artifact(&self, artifact: &ArtifactRef) {
        self.output
            .lock()
            .expect("test fixture")
            .push(artifact.path.clone());
    }
}

/// Scripted LLM records every input the harness receives after H3.
struct Harness {
    llm: Arc<Mutex<Vec<Vec<u8>>>>,
}
#[async_trait::async_trait]
impl WorkerAdapter for Harness {
    fn id(&self) -> &str {
        "acp"
    }
    async fn run(
        &self,
        request: RunRequest,
        _: &str,
        _: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let input = serde_json::to_vec(&request).expect("serialize scripted LLM input");
        self.llm.lock().expect("LLM mutex").push(input);
        std::fs::create_dir_all(&request.artifacts_dir).expect("artifact directory");
        std::fs::write(
            request.artifacts_dir.join("scripted-llm.txt"),
            b"public fixture result",
        )
        .expect("scripted LLM artifact");
        sink.progress("scripted LLM completed");
        Ok(RunOutcome {
            terminal: task_worker::Terminal::Done {
                summary: "fixture checked".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

impl World {
    /// dispatcher の一回分: lease を取り（Ready → Running）、browser supervisor を通して harness を走らせる。
    async fn dispatch(&self, run_id: &str) -> (Result<RunOutcome, AdapterError>, Vec<BrowserRun>) {
        assert!(
            self.env
                .store
                .acquire_lease(self.task.id, run_id, Duration::from_secs(60))
                .expect("test fixture")
        );
        let sink = StoreSink {
            store: SqliteStore::open(&self.env.db_path).expect("test fixture"),
            task_id: self.task.id,
            output: self.worker_output.clone(),
            browsers: Mutex::default(),
            auth_calls: Mutex::default(),
            live: Mutex::default(),
        };
        let record = self.env.dir.path().join("empty-browser-conformance.json");
        // ADR-0093: the P4-B cases count only with per-test evidence in the ledger.
        let evidence: Vec<_> = [
            task_core::browser_backend::FixtureCase::InjectionAttackSuite,
            task_core::browser_backend::FixtureCase::AuthSectionObservationStop,
        ]
        .into_iter()
        .flat_map(|case| {
            task_core::browser_backend::required_evidence(case)
                .into_iter()
                .map(
                    move |test| task_core::browser_backend::ConformanceEvidence {
                        case,
                        test,
                        outcome: task_core::browser_backend::EvidenceOutcome::Passed,
                    },
                )
        })
        .collect();
        std::fs::write(
            &record,
            serde_json::to_vec(&serde_json::json!({
                "schema": 1, "source": "celeris-browser-conformance",
                "results": [{
                    "backend_id": "acp", "version": "0.38.1",
                    "passed": ["open_allowed_origin", "refuse_denied_origin", "resume_after_crash",
                        "snapshot_has_refs", "click_by_ref", "screenshot_artifact",
                        "download_to_artifacts", "isolation_suite", "egress_negative_suite",
                        "injection_attack_suite", "auth_section_observation_stop"],
                    "evidence": evidence,
                }],
            }))
            .expect("test fixture"),
        )
        .expect("test fixture");
        let outcome = task_worker::browser::run_with_executable_record(
            Arc::new(Harness {
                llm: self.llm_input.clone(),
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
            &record,
        )
        .await;
        self.worker_output
            .lock()
            .expect("test fixture")
            .push(format!("{outcome:?}"));
        *self.auth_calls.lock().expect("test fixture") =
            sink.auth_calls.into_inner().expect("test fixture");
        *self.live.lock().expect("test fixture") = sink.live.into_inner().expect("test fixture");
        let browsers = sink.browsers.into_inner().expect("test fixture");
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
            item["wait"]["reason"]
                .as_str()
                .expect("test fixture")
                .to_uppercase()
        );
        item["wait"].clone()
    }

    async fn register(&self, wait: &Value) {
        let path = format!(
            "/api/v1/tasks/{}/browser/waits/{}/credential",
            self.task.id,
            wait["wait_id"].as_str().expect("test fixture")
        );
        let body = json!({"expected_version": wait["version"], "username": SENTINEL_USER,
            "password": self.password, "attestation": self.attest(wait, "register", "nonce-register")});
        let resp = send(&self.app, post_admin(&path, &body)).await;
        assert_eq!(resp.status, 200, "{}", resp.text());
        assert!(!resp.text().contains(&self.password) && !resp.text().contains(SENTINEL_USER));
    }

    async fn decide(&self, wait: &Value, decision: &str) {
        let path = format!(
            "/api/v1/tasks/{}/browser/waits/{}/decision",
            self.task.id,
            wait["wait_id"].as_str().expect("test fixture")
        );
        let body = json!({"decision": decision, "expected_version": wait["version"],
            "idempotency_key": format!("decide-{decision}"),
            "attestation": self.attest(wait, decision, &format!("nonce-{decision}"))});
        let resp = send(&self.app, post_admin(&path, &body)).await;
        assert_eq!(resp.status, 200, "{}", resp.text());
    }

    async fn seed_wait(&self) {
        use task_core::browser_wait::{BrowserWaitReason, NewBrowserWait};
        let policy_response = send(
            &self.app,
            put_json_with(
                &format!("/api/v1/tasks/{}/browser/policy", self.task.id),
                &serde_json::to_value(task_policy()).expect("serialize policy"),
                &admin_headers(),
            ),
        )
        .await;
        assert_eq!(policy_response.status, 200, "{}", policy_response.text());
        let req = run_request(self);
        let policy = task_worker::browser_policy::prepare(
            req.context
                .profile
                .as_ref()
                .expect("profile")
                .browser
                .as_ref()
                .expect("browser"),
            req.context.browser_policy.as_ref(),
            task_worker::browser::SUPPORTED_VERSION,
        )
        .expect("prepared policy");
        assert!(
            self.env
                .store
                .acquire_lease(self.task.id, "run-auth", Duration::from_secs(60))
                .expect("lease")
        );
        let request = NewBrowserWait {
            work_unit_id: None,
            run_id: "run-auth".into(),
            session_id: "auth-session".into(),
            reason: BrowserWaitReason::WaitingForAuth,
            origin: ORIGIN.into(),
            purpose: "Sign in to read the build dashboard".into(),
            credential_policy_id: Some("pol-login".into()),
            credential: None,
            operation: None,
            trusted_login: None,
            policy_revision: policy.binding.revision,
            policy_hash: policy.binding.hash,
            owner_id: Some("owner".into()),
            ttl_secs: None,
            resume_key: "run-auth".into(),
        };
        let response = send(
            &self.app,
            post_admin(
                &format!("/api/v1/tasks/{}/browser/requests", self.task.id),
                &serde_json::to_value(request).expect("serialize wait"),
            ),
        )
        .await;
        assert_eq!(response.status, 201, "{}", response.text());
    }

    async fn until_registered(&self) {
        self.seed_wait().await;
        let auth = self.pending().await;
        self.register(&auth).await;
        assert_eq!(self.env.status_of(self.task.id), Status::Ready);
    }

    async fn until_approval(&self) -> Value {
        self.until_registered().await;
        let (outcome, _) = self.dispatch("run-approval").await;
        assert!(matches!(
            outcome.expect("approval run").terminal,
            task_worker::Terminal::Question { .. }
        ));
        let approval = self.pending().await;
        assert_eq!(approval["reason"], "waiting_for_approval");
        assert_eq!(approval["trusted_login"]["password_selector"], "#password");
        approval
    }
}

fn tool(name: &str) -> PathBuf {
    let path = PathBuf::from("/usr/bin").join(name);
    assert!(
        path.is_file(),
        "required test tool missing: {}",
        path.display()
    );
    path
}

fn worker_binary(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("test executable");
    let path = exe
        .parent()
        .and_then(Path::parent)
        .expect("target directory")
        .join(name);
    let status = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "-q",
            "-p",
            "task-worker",
            "--bins",
            "--features",
            "h3-e2e-insecure-cert",
        ])
        .status()
        .expect("build worker binaries");
    assert!(status.success(), "worker binary build failed");
    assert!(
        path.is_file(),
        "required worker binary missing: {}",
        path.display()
    );
    path
}

fn dns_server(listener: std::net::TcpListener) {
    use std::io::{Read, Write};
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut len = [0; 2];
        if stream.read_exact(&mut len).is_err() {
            continue;
        }
        let mut query = vec![0; usize::from(u16::from_be_bytes(len))];
        if stream.read_exact(&mut query).is_err() || query.len() < 17 {
            continue;
        }
        let mut cursor = 12;
        let mut labels = Vec::new();
        while cursor < query.len() && query[cursor] != 0 {
            let size = usize::from(query[cursor]);
            if cursor + 1 + size >= query.len() {
                break;
            }
            labels
                .push(String::from_utf8_lossy(&query[cursor + 1..cursor + 1 + size]).into_owned());
            cursor += size + 1;
        }
        if cursor + 4 >= query.len() {
            continue;
        }
        let query_type = u16::from_be_bytes([query[cursor + 1], query[cursor + 2]]);
        let answer = labels.join(".") == "fixture.example.com" && query_type == 1;
        let mut response = query[..cursor + 5].to_vec();
        response[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        response[6..8].copy_from_slice(&u16::from(answer).to_be_bytes());
        if answer {
            response.extend([0xc0, 0x0c]);
            response.extend(query_type.to_be_bytes());
            response.extend([0, 1, 0, 0, 0, 30, 0, 4, 93, 184, 216, 34]);
        }
        let _ = stream.write_all(&(response.len() as u16).to_be_bytes());
        let _ = stream.write_all(&response);
    }
}

fn start_fixture(root: &Path, password: &str) -> std::process::Child {
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    let cert = Command::new(tool("openssl"))
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
            "-days",
            "1",
            "-subj",
            "/CN=fixture.example.com",
            "-addext",
            "subjectAltName=DNS:fixture.example.com",
        ])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("fixture certificate");
    assert!(cert.success());
    let script = r#"import http.server, os, ssl, urllib.parse
from pathlib import Path
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        page = b'<html><body><form action="/accept" method="POST"><input id="password" name="password" type="password"><button id="submit" type="submit">Sign in</button></form></body></html>'
        self.send_response(200); self.end_headers(); self.wfile.write(page)
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        fields = urllib.parse.parse_qs(body.decode('utf-8'))
        if self.path == '/accept' and fields.get('password') == [os.environ['EXPECTED']]:
            Path('accepted').write_text('accepted')
            self.send_response(200)
        else: self.send_response(403)
        self.end_headers()
server = http.server.HTTPServer(('93.184.216.34', 443), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain('cert.pem', 'key.pem')
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
"#;
    std::fs::write(root.join("fixture.py"), script).expect("fixture server source");
    let child = Command::new("python3")
        .arg("fixture.py")
        .env("EXPECTED", password)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fixture server");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("93.184.216.34", 443)).is_err() {
        assert!(
            std::time::Instant::now() < deadline,
            "fixture did not listen"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    for entry in std::fs::read_dir(root).expect("scan directory") {
        let entry = entry.expect("directory entry");
        let kind = entry.file_type().expect("entry type");
        if kind.is_dir() {
            collect_files(&entry.path(), files);
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
}

fn check_surface(label: &str, bytes: &[u8], sentinel: &str) -> Result<(), String> {
    use base64::Engine as _;
    let variants = [
        sentinel.to_owned(),
        base64::engine::general_purpose::STANDARD.encode(sentinel),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sentinel),
        sentinel
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        sentinel
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect(),
        form_urlencoded::byte_serialize(sentinel.as_bytes()).collect(),
        sentinel
            .as_bytes()
            .iter()
            .map(|b| format!("%{b:02X}"))
            .collect(),
        sentinel
            .as_bytes()
            .iter()
            .map(|b| format!("%{b:02x}"))
            .collect(),
    ];
    for variant in variants {
        if !variant.is_empty()
            && bytes
                .windows(variant.len())
                .any(|window| window == variant.as_bytes())
        {
            return Err(format!("sentinel leaked into {label}"));
        }
    }
    Ok(())
}

fn scan_observations(world: &World) -> Result<(), String> {
    let events = serde_json::to_vec(&world.env.store.events_for(world.task.id).expect("events"))
        .expect("serialize events");
    let llm = world.llm_input.lock().expect("LLM inputs");
    let output = world
        .worker_output
        .lock()
        .expect("worker output")
        .join("\n");
    let live = world.live.lock().expect("live events").join("\n");
    let run_log = world
        .env
        .workspace(&world.task)
        .join("runs/run-login/run.log");
    std::fs::write(&run_log, &output).expect("run log");
    let mut paths = Vec::new();
    collect_files(&world.env.workspace(&world.task).join("runs"), &mut paths);
    collect_files(
        &world.env.workspace(&world.task).join("artifacts"),
        &mut paths,
    );
    let wal = PathBuf::from(format!("{}-wal", world.env.db_path.display()));
    assert!(world.env.db_path.is_file(), "SQLite database missing");
    assert!(wal.is_file(), "SQLite WAL missing");
    for secret in [&world.password, SENTINEL_USER] {
        check_surface("event", &events, secret)?;
        check_surface("live event", live.as_bytes(), secret)?;
        for input in llm.iter() {
            check_surface("LLM input", input, secret)?;
        }
        check_surface(
            "SQLite DB",
            &std::fs::read(&world.env.db_path).expect("DB bytes"),
            secret,
        )?;
        check_surface(
            "SQLite WAL",
            &std::fs::read(&wal).expect("WAL bytes"),
            secret,
        )?;
        check_surface("run log", output.as_bytes(), secret)?;
        for path in &paths {
            check_surface(
                &format!("artifact or run file {}", path.display()),
                &std::fs::read(path).expect("run file bytes"),
                secret,
            )?;
        }
    }
    assert!(!llm.is_empty(), "scripted LLM received no input");
    assert!(!paths.is_empty(), "run files not scanned");
    Ok(())
}

async fn inner_h3_test() {
    use std::net::TcpListener;
    use std::process::Command;
    let status = Command::new(tool("ip"))
        .args(["link", "set", "lo", "up"])
        .status()
        .expect("loopback setup");
    assert!(status.success());
    let status = Command::new(tool("ip"))
        .args(["addr", "add", "93.184.216.34/32", "dev", "lo"])
        .status()
        .expect("fixture address");
    assert!(status.success());
    let dns = TcpListener::bind("127.0.0.1:53").expect("DNS listener");
    std::thread::spawn(move || dns_server(dns));
    let world = world();
    let fixture_dir = world.env.dir.path().join("https-fixture");
    std::fs::create_dir(&fixture_dir).expect("fixture directory");
    let mut fixture = start_fixture(&fixture_dir, &world.password);
    let isolation = task_worker::browser::IsolatedBrowserConfig {
        resolver: Some("127.0.0.1".parse().expect("resolver")),
        record_dir: world.env.dir.path().join("isolation-records"),
        bwrap: tool("bwrap"),
        sandboxd: worker_binary("celeris-browser-sandboxd"),
        egress: worker_binary("celeris-browser-egress"),
        live_sessions: None,
    };
    task_worker::browser::configure_isolated_runtime(isolation);
    let approval = world.until_approval().await;
    world.decide(&approval, "approve_once").await;
    let (outcome, _) = world.dispatch("run-login").await;
    let outcome = outcome.expect("H3 run result");
    assert!(
        matches!(outcome.terminal, task_worker::Terminal::Done { .. }),
        "{outcome:?}"
    );
    assert!(
        fixture_dir.join("accepted").is_file(),
        "fixture did not accept the injected sentinel"
    );
    let calls = world.auth_calls.lock().expect("auth calls");
    assert_eq!(
        calls.iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![true, false]
    );
    drop(calls);
    scan_observations(&world).expect("six-surface sentinel scan");
    let _ = fixture.kill();
    let _ = fixture.wait();
}

#[tokio::test(flavor = "multi_thread")]
async fn production_h3_injects_once_without_exposure() {
    for name in ["unshare", "ip", "openssl", "bwrap"] {
        tool(name);
    }
    let result = std::process::Command::new(tool("unshare"))
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "inner_h3_injection",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CELERIS_H3_INNER", "1")
        .output()
        .expect("isolated test process");
    assert!(
        result.status.success(),
        "H3 test failed: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("H3 six-surface scan passed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn inner_h3_injection() {
    if std::env::var_os("CELERIS_H3_INNER").is_some() {
        inner_h3_test().await;
        eprintln!("H3 six-surface scan passed");
    }
}

#[test]
fn injected_leak_is_caught_by_the_same_scanner() {
    let secret = "H3-negative-control-sentinel";
    for surface in [
        "event",
        "artifact",
        "LLM input",
        "SQLite DB",
        "SQLite WAL",
        "run log",
    ] {
        assert!(
            check_surface(surface, secret.as_bytes(), secret).is_err(),
            "{surface} mutation escaped"
        );
    }
}
