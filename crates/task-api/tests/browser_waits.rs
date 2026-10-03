//! ADR-0080 D4/D5: browser の登録依頼・承認の API（`/tasks/{id}/browser/*`、`/browser/waits`）。
//!
//! 人の操作は bearer + GUI 専用鍵で署名した human attestation を要する。手動登録の秘密は broker にだけ
//! 渡り、DB（本体・WAL）・event・応答に残らないことを sentinel で確かめる。

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use task_api::browser::{
    BrokerFailure, BrokerReceipt, BrowserApiConfig, CredentialBrokerControl, ManualRegistration,
};
use task_core::browser_wait::BrowserWaitStore;
use task_core::{Status, TaskId, TaskKind, TaskStore};
use time::OffsetDateTime;

const SENTINEL_USER: &str = "SENTINEL-user-7c1e";
const SENTINEL_PASS: &str = "SENTINEL-pass-4d9b";

#[tokio::test]
async fn task_browser_policy_is_persisted_and_running_tasks_cannot_expand_it() {
    let f = fixture();
    let task = new_task(TaskKind::Execute, Status::Ready);
    f.env.seed(&task);
    let path = format!("/api/v1/tasks/{}/browser/policy", task.id);
    let policy = json!({
        "policy_id":"read-only", "revision":1, "domain_mode":"common_hosts",
        "network_domains":["example.com"], "allowed_actions":["navigate","snapshot"],
        "approval_actions":[], "credential_policy_ids":[]
    });
    let put = |value: &Value| {
        axum::http::Request::put(&path)
            .header("host", HOST)
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(value.to_string()))
            .unwrap()
    };
    let response = send(&f.app, put(&policy)).await;
    assert_eq!(response.status, 200, "{}", response.text());
    let fetched = send(&f.app, get_admin(&path)).await;
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.json()["policy"]["policy_id"], "read-only");
    assert!(
        f.env
            .store
            .browser_task_policy_get(task.id)
            .unwrap()
            .is_some()
    );
    assert!(
        f.env
            .store
            .acquire_lease(task.id, "run-policy", std::time::Duration::from_secs(60))
            .unwrap()
    );
    let response = send(&f.app, put(&policy)).await;
    assert_problem(&response, 409, "browser_policy_state");
}

#[tokio::test]
async fn real_broker_registration_keeps_sentinel_out_of_db_events_artifacts_and_worker_output() {
    use celeris_credentiald::{Broker, ManualProvider, ipc};
    use std::os::unix::fs::DirBuilderExt;
    let env = admin_env();
    let root = env.dir.path();
    let runtime = root.join("runtime");
    let config = root.join("broker-config");
    let data = root.join("broker-data");
    for dir in [&runtime, &config, &data] {
        std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
    }
    let manual = ManualProvider::open(config.join("keys"), data.join("vault")).unwrap();
    manual.initialize_key().unwrap();
    let broker = Arc::new(Broker::new(manual, data.join("audit")).unwrap());
    let socket = runtime.join("celeris-credentiald/control.sock");
    std::thread::spawn(move || ipc::serve(broker, &runtime, vec![std::process::id()]).unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if socket.exists() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for fixture readiness"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(socket.exists());
    let key = keypair();
    let app = task_api::router(env.state.clone().with_browser(BrowserApiConfig {
        attestation_public_key: Some(key.public_key().as_ref().to_vec()),
        broker: Some(Arc::new(task_api::browser::UnixCredentialBrokerControl {
            socket: socket.clone(),
            site_policies: Vec::new(),
        })),
    }));
    let task_id = running_task(&env);
    let response = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/browser/requests"),
            &auth_request("rk-real"),
        ),
    )
    .await;
    assert_eq!(response.status, 201, "{}", response.text());
    let wait_id = response.json()["wait"]["wait_id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/browser/waits/{wait_id}/credential"),
            &json!({"expected_version":1,"username":SENTINEL_USER,"password":SENTINEL_PASS,
            "attestation":attest(&key, &claims(task_id, &wait_id, "register", "nonce-real"))}),
        ),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(env.status_of(task_id), Status::Ready);
    let credential_id = response.json()["wait"]["credential"]["credential_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        env.store
            .acquire_lease(task_id, "approval-run", std::time::Duration::from_secs(60))
            .unwrap()
    );
    let mut approval_body = approval_request("rk-real-approval");
    approval_body["run_id"] = json!("approval-run");
    approval_body["session_id"] = json!("approval-session");
    approval_body["credential"]["credential_id"] = json!(credential_id);
    let response = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/browser/requests"),
            &approval_body,
        ),
    )
    .await;
    assert_eq!(response.status, 201, "{}", response.text());
    let approval = response.json()["wait"].clone();
    let approval_wait_id = approval["wait_id"].as_str().unwrap();
    let response = send(&app, post_admin(
        &format!("/api/v1/tasks/{task_id}/browser/waits/{approval_wait_id}/decision"),
        &json!({"decision":"approve_once", "expected_version":1, "idempotency_key":"real-approval",
            "attestation":attest(&key, &claims(task_id, approval_wait_id, "approve_once", "nonce-real-approval"))}),
    )).await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(env.status_of(task_id), Status::Ready);
    let approved = response.json()["wait"].clone();
    let consumed = env
        .store
        .browser_wait_consume(
            task_id,
            approval_wait_id,
            "rk-real-approval",
            "approval-run",
            "approval-session",
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    assert_eq!(consumed.state.as_str(), "resumed");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let binding = json!({"op":"bind","binding":{
        "token":"", "task_id":task_id.to_string(),"run_id":"approval-run",
        "session_id":"approval-session","exact_origin":"https://login.example.com",
        "policy_hash":"sha256-policy","expires_at":now+60}});
    let bound = ipc::call(&socket, &serde_json::to_vec(&binding).unwrap()).unwrap();
    assert!(bound.success);
    let binding_token = bound.binding_token.unwrap();
    let grant = json!({"op":"grant","request":{
        "reference":{"credential_id":credential_id,"provider":"manual","policy_id":"pol-example"},
        "policy":{"policy_id":"pol-example","revision":1,"exact_origin":"https://login.example.com",
            "task_id":task_id.to_string(),"max_ttl_seconds":60,"require_approval":true,"allow_persistence":false},
        "credential_revision":1,"task_id":task_id.to_string(),"run_id":"approval-run",
        "session_id":"approval-session","approval_id":approved["approval_id"],
        "approved_by":"owner","policy_hash":"sha256-policy","idempotency_key":"real-lease",
        "ttl_seconds":60,"approval_expires_at":now+60,"session_expires_at":now+60}});
    let granted = ipc::call(&socket, &serde_json::to_vec(&grant).unwrap()).unwrap();
    assert!(granted.success);
    let lease_id = granted.lease_id.unwrap();
    let plugin = |url: &str| {
        json!({"protocol":"agent-browser.plugin.v1","type":"credential.resolve",
        "capability":"credential.read","request":{"profileName":"default","itemRef":lease_id,"url":url}})
    };
    let resolve = socket.with_file_name("resolve.sock");
    let wrong = ipc::bridge_request(
        &serde_json::to_vec(&plugin("https://other.example.com/login")).unwrap(),
        &binding_token,
        &resolve,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&wrong).unwrap()["success"],
        false
    );
    let raw = ipc::bridge_request(
        &serde_json::to_vec(&plugin("https://login.example.com/login")).unwrap(),
        &binding_token,
        &resolve,
    );
    let private = serde_json::from_slice::<Value>(&raw).unwrap();
    assert_eq!(private["success"], false);
    assert!(private.get("credential").is_none());
    // Approval does not make the old worker-facing secret-return protocol trusted.
    let direct = ipc::call(
        &resolve,
        &serde_json::to_vec(&json!({
            "op":"resolve", "binding_token":binding_token, "lease_id":lease_id,
            "observed_origin":"https://login.example.com"
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(!direct.success);
    assert_eq!(direct.code.as_deref(), Some("trusted_injection_required"));
    assert!(direct.credential.is_none());
    let replay = ipc::bridge_request(
        &serde_json::to_vec(&plugin("https://login.example.com/login")).unwrap(),
        &binding_token,
        &resolve,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&replay).unwrap()["success"],
        false
    );
    // The legacy protocol returns only a fixed refusal, never a credential.
    let public_result = json!({"success": private["success"]});
    let artifacts = root.join("artifacts");
    std::fs::create_dir(&artifacts).unwrap();
    std::fs::write(artifacts.join("result.json"), public_result.to_string()).unwrap();
    let worker = root.join("worker-output.json");
    std::fs::write(&worker, public_result.to_string()).unwrap();
    let mut inspected = Vec::new();
    fn walk(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    walk(root, &mut inspected);
    assert!(inspected.iter().any(|p| p.ends_with("celeris.db")));
    assert!(
        inspected
            .iter()
            .any(|p| p.ends_with("vault".to_string() + "/cred-" + &wait_id + ".json"))
    );
    assert!(inspected.iter().any(|p| p == &worker));
    assert!(inspected.iter().any(|p| p.ends_with("result.json")));
    for path in inspected {
        let bytes = std::fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(SENTINEL_USER) && !text.contains(SENTINEL_PASS),
            "secret leaked to {}",
            path.display()
        );
    }
    let events = env.store.events_for(task_id).unwrap();
    let text = serde_json::to_string(&events).unwrap();
    assert!(!text.contains(SENTINEL_USER) && !text.contains(SENTINEL_PASS));
}

/// broker の偽物: 受け取った秘密を記録し（転送の確認用）、receipt だけを返す。
#[derive(Default)]
struct FakeBroker {
    received: Mutex<Vec<(String, String, String)>>,
}

impl CredentialBrokerControl for FakeBroker {
    fn register(&self, r: ManualRegistration) -> Result<BrokerReceipt, BrokerFailure> {
        self.received.lock().expect("lock").push((
            r.origin.clone(),
            r.username.expose().to_string(),
            r.password.expose().to_string(),
        ));
        Ok(BrokerReceipt {
            credential_id: "cred-manual-1".into(),
            provider: "manual".into(),
            policy_id: r.policy_id,
            credential_revision: 1,
            origin: r.origin,
            receipt_id: "rcpt-1".into(),
        })
    }

    fn verify_receipt(&self, _: &str, receipt: &BrokerReceipt) -> Result<bool, BrokerFailure> {
        Ok(receipt.receipt_id == "rcpt-1")
    }
}

struct Fixture {
    env: TestEnv,
    app: axum::Router,
    key: ring::signature::Ed25519KeyPair,
    broker: Arc<FakeBroker>,
}

fn keypair() -> ring::signature::Ed25519KeyPair {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
    ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("keypair")
}

fn fixture() -> Fixture {
    let env = admin_env();
    let key = keypair();
    let broker = Arc::new(FakeBroker::default());
    let state = env.state.clone().with_browser(BrowserApiConfig {
        attestation_public_key: Some(key.public_key().as_ref().to_vec()),
        broker: Some(broker.clone()),
    });
    let app = task_api::router(state);
    Fixture {
        env,
        app,
        key,
        broker,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn running_task(env: &TestEnv) -> TaskId {
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    assert!(
        env.store
            .acquire_lease(task.id, "run-1", std::time::Duration::from_secs(60))
            .expect("lease")
    );
    task.id
}

fn auth_request(key: &str) -> Value {
    json!({
        "run_id": "run-1",
        "session_id": "sess-1",
        "reason": "waiting_for_auth",
        "origin": "https://login.example.com",
        "purpose": "Sign in to read the build dashboard",
        "credential_policy_id": "pol-example",
        "policy_revision": 2,
        "policy_hash": "sha256-policy",
        "owner_id": "owner",
        "resume_key": key,
    })
}

fn approval_request(key: &str) -> Value {
    json!({
        "run_id": "run-1",
        "session_id": "sess-1",
        "reason": "waiting_for_approval",
        "origin": "https://login.example.com",
        "purpose": "Submit the login form",
        "credential": {"credential_id": "cred-manual-1", "provider": "manual", "policy_id": "pol-example"},
        "operation": {"intent_id": "intent-1", "action": "credential_use", "args_digest": "sha256-args"},
        "policy_revision": 2,
        "policy_hash": "sha256-policy",
        "owner_id": "owner",
        "resume_key": key,
    })
}

struct Claims<'a> {
    task_id: TaskId,
    wait_id: &'a str,
    version: u64,
    decision: &'a str,
    nonce: &'a str,
    expires_in: i64,
    actor: &'a str,
}

fn attest(key: &ring::signature::Ed25519KeyPair, c: &Claims<'_>) -> Value {
    let payload = json!({
        "actor_id": c.actor,
        "owner_session_hash": "owner-session-hash",
        "task_id": c.task_id.to_string(),
        "wait_id": c.wait_id,
        "version": c.version,
        "decision": c.decision,
        "policy_hash": "sha256-policy",
        "nonce": c.nonce,
        "expires_at": OffsetDateTime::now_utc().unix_timestamp() + c.expires_in,
    })
    .to_string();
    let signature = hex(key.sign(payload.as_bytes()).as_ref());
    json!({"payload": payload, "signature": signature})
}

fn claims<'a>(task_id: TaskId, wait_id: &'a str, decision: &'a str, nonce: &'a str) -> Claims<'a> {
    Claims {
        task_id,
        wait_id,
        version: 1,
        decision,
        nonce,
        expires_in: 20,
        actor: "owner",
    }
}

async fn open(f: &Fixture, task_id: TaskId, body: &Value) -> Value {
    let resp = send(
        &f.app,
        post_admin(&format!("/api/v1/tasks/{task_id}/browser/requests"), body),
    )
    .await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    assert_eq!(resp.header("cache-control"), Some("no-store"));
    resp.json()["wait"].clone()
}

/// DB の本体・WAL・shm を含む tempdir の全ファイルに sentinel が無い。
fn assert_no_secret_on_disk(env: &TestEnv) {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(env.dir.path(), &mut files);
    assert!(files.iter().any(|p| p.ends_with("celeris.db")));
    for path in files {
        let bytes = std::fs::read(&path).expect("read");
        let hay = String::from_utf8_lossy(&bytes);
        for s in [SENTINEL_USER, SENTINEL_PASS] {
            assert!(!hay.contains(s), "secret leaked into {}", path.display());
        }
    }
}

#[tokio::test]
async fn registration_request_is_listed_and_manual_registration_resumes_without_storing_secret() {
    let f = fixture();
    let task_id = running_task(&f.env);
    let wait = open(&f, task_id, &auth_request("rk-auth")).await;
    let wait_id = wait["wait_id"].as_str().expect("wait_id").to_string();
    assert_eq!(wait["reason"], "waiting_for_auth");
    assert_eq!(wait["origin"], "https://login.example.com");
    assert_eq!(f.env.status_of(task_id), Status::Blocked);
    assert!(
        f.env
            .store
            .get(task_id)
            .expect("get")
            .expect("task")
            .lease
            .is_none(),
        "the worker slot must be released while waiting"
    );

    // resume_key の再送は同じ wait（200、created=false）。
    let again = send(
        &f.app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/browser/requests"),
            &auth_request("rk-auth"),
        ),
    )
    .await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(again.json()["created"], false);
    assert_eq!(again.json()["wait"]["wait_id"], wait_id.as_str());

    // 秘密の欄を持つ要求は固定コードで拒否し、値を反射しない。
    let mut with_secret = auth_request("rk-secret");
    with_secret["password"] = json!(SENTINEL_PASS);
    let resp = send(
        &f.app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/browser/requests"),
            &with_secret,
        ),
    )
    .await;
    assert_problem(&resp, 422, "browser_body_invalid");
    assert!(!resp.text().contains(SENTINEL_PASS));

    // 一覧（承認一覧）と inbox に出る。質問としては出ない。
    let pending = send(&f.app, get_admin("/api/v1/browser/waits")).await;
    assert_eq!(pending.status, 200, "{}", pending.text());
    let items = pending.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["run_state"], "WAITING_FOR_AUTH");
    assert_eq!(items[0]["task"]["id"], task_id.to_string());
    let inbox = send(&f.app, get_admin("/api/v1/inbox")).await;
    assert_eq!(inbox.status, 200, "{}", inbox.text());
    let inbox = inbox.json();
    assert_eq!(inbox["counts"]["browser_waits"], 1);
    assert_eq!(
        inbox["browser_waits"][0]["wait"]["wait_id"],
        wait_id.as_str()
    );
    assert!(
        inbox["questions"]
            .as_array()
            .expect("questions")
            .iter()
            .all(|q| q["task"]["id"] != task_id.to_string())
    );
    let listed = send(
        &f.app,
        get_admin(&format!("/api/v1/tasks/{task_id}/browser/waits")),
    )
    .await;
    assert_eq!(listed.status, 200);
    assert_eq!(listed.json()["items"][0]["state"], "pending");

    // 一般の回答では再開できない。
    let answer = send(
        &f.app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/answer"),
            &json!({"answer": "go"}),
        ),
    )
    .await;
    assert_eq!(answer.status, 409, "{}", answer.text());
    assert_eq!(f.env.status_of(task_id), Status::Blocked);

    let path = format!("/api/v1/tasks/{task_id}/browser/waits/{wait_id}/credential");
    let body = |attestation: Value| {
        json!({
            "expected_version": 1,
            "username": SENTINEL_USER,
            "password": SENTINEL_PASS,
            "attestation": attestation,
        })
    };
    // bearer が無い → 401。
    let good = attest(&f.key, &claims(task_id, &wait_id, "register", "n-reg"));
    let resp = send(&f.app, post_json(&path, &body(good.clone()))).await;
    assert_problem(&resp, 401, "unauthorized");
    // 別の鍵・違う決定・期限切れ・本人以外の attestation → 403、値は反射しない。
    let other_key = keypair();
    for bad in [
        attest(&other_key, &claims(task_id, &wait_id, "register", "n-1")),
        attest(&f.key, &claims(task_id, &wait_id, "approve_once", "n-2")),
        attest(
            &f.key,
            &Claims {
                expires_in: -5,
                ..claims(task_id, &wait_id, "register", "n-3")
            },
        ),
        attest(
            &f.key,
            &Claims {
                expires_in: 600,
                ..claims(task_id, &wait_id, "register", "n-4")
            },
        ),
        attest(
            &f.key,
            &Claims {
                actor: "someone-else",
                ..claims(task_id, &wait_id, "register", "n-5")
            },
        ),
    ] {
        let resp = send(&f.app, post_admin(&path, &body(bad))).await;
        assert_problem(&resp, 403, "attestation_invalid");
        assert!(!resp.text().contains(SENTINEL_PASS));
    }
    assert!(f.broker.received.lock().expect("lock").is_empty());

    let resp = send(&f.app, post_admin(&path, &body(good))).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.header("cache-control"), Some("no-store"));
    let text = resp.text();
    assert!(!text.contains(SENTINEL_USER) && !text.contains(SENTINEL_PASS));
    let out = resp.json();
    assert_eq!(out["task_status"], "ready");
    assert_eq!(out["wait"]["state"], "registered");
    assert_eq!(out["wait"]["credential"]["credential_id"], "cred-manual-1");
    // broker には届いている（origin は保存済みの wait のもの）。
    assert_eq!(
        f.broker.received.lock().expect("lock").as_slice(),
        &[(
            "https://login.example.com".to_string(),
            SENTINEL_USER.to_string(),
            SENTINEL_PASS.to_string()
        )]
    );
    assert_eq!(f.env.status_of(task_id), Status::Ready);

    // event にも DB（WAL を含む）にも秘密は無い。
    let events = send(
        &f.app,
        get_admin(&format!("/api/v1/tasks/{task_id}/events")),
    )
    .await;
    assert_eq!(events.status, 200);
    let events_text = events.text();
    assert!(events_text.contains("browser_wait_opened"));
    assert!(events_text.contains("browser_wait_resolved"));
    assert!(!events_text.contains(SENTINEL_USER) && !events_text.contains(SENTINEL_PASS));
    for (_, event) in f.env.store.events_for(task_id).expect("events") {
        let raw = serde_json::to_string(&event).expect("json");
        assert!(!raw.contains(SENTINEL_USER) && !raw.contains(SENTINEL_PASS));
    }
    assert_no_secret_on_disk(&f.env);
}

#[tokio::test]
async fn registered_receipt_is_verified_with_the_broker() {
    let f = fixture();
    let task_id = running_task(&f.env);
    let wait = open(&f, task_id, &auth_request("rk-rcpt")).await;
    let wait_id = wait["wait_id"].as_str().expect("wait_id").to_string();
    let path = format!("/api/v1/tasks/{task_id}/browser/waits/{wait_id}/registered");
    let receipt = |id: &str| {
        json!({
            "credential_id": "cred-manual-1",
            "provider": "manual",
            "policy_id": "pol-example",
            "credential_revision": 1,
            "origin": "https://login.example.com",
            "receipt_id": id,
        })
    };
    let resp = send(
        &f.app,
        post_admin(
            &path,
            &json!({
                "expected_version": 1,
                "receipt": receipt("rcpt-forged"),
                "attestation": attest(&f.key, &claims(task_id, &wait_id, "register", "n-a")),
            }),
        ),
    )
    .await;
    assert_problem(&resp, 422, "credential_receipt_invalid");
    let resp = send(
        &f.app,
        post_admin(
            &path,
            &json!({
                "expected_version": 1,
                "receipt": receipt("rcpt-1"),
                "attestation": attest(&f.key, &claims(task_id, &wait_id, "register", "n-b")),
            }),
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json()["task_status"], "ready");
}

#[tokio::test]
async fn approval_needs_attestation_and_approve_or_deny_moves_the_task() {
    let f = fixture();
    let task_id = running_task(&f.env);
    let wait = open(&f, task_id, &approval_request("rk-appr")).await;
    let wait_id = wait["wait_id"].as_str().expect("wait_id").to_string();
    assert_eq!(wait["reason"], "waiting_for_approval");
    let path = format!("/api/v1/tasks/{task_id}/browser/waits/{wait_id}/decision");

    // bearer だけ（attestation なし）では承認できない。
    let resp = send(
        &f.app,
        post_admin(
            &path,
            &json!({"decision": "approve_once", "expected_version": 1, "idempotency_key": "k1"}),
        ),
    )
    .await;
    assert_problem(&resp, 422, "browser_body_invalid");
    // 登録用の attestation を承認に流用できない。
    let resp = send(
        &f.app,
        post_admin(
            &path,
            &json!({
                "decision": "approve_once", "expected_version": 1, "idempotency_key": "k1",
                "attestation": attest(&f.key, &claims(task_id, &wait_id, "register", "n-x")),
            }),
        ),
    )
    .await;
    assert_problem(&resp, 403, "attestation_invalid");
    // 古い version は 409。
    let resp = send(
        &f.app,
        post_admin(
            &path,
            &json!({
                "decision": "approve_once", "expected_version": 5, "idempotency_key": "k1",
                "attestation": attest(&f.key, &Claims { version: 5, ..claims(task_id, &wait_id, "approve_once", "n-v") }),
            }),
        ),
    )
    .await;
    assert_problem(&resp, 409, "browser_wait_version_conflict");

    let approve = json!({
        "decision": "approve_once", "expected_version": 1, "idempotency_key": "k1",
        "attestation": attest(&f.key, &claims(task_id, &wait_id, "approve_once", "n-ok")),
    });
    let resp = send(&f.app, post_admin(&path, &approve)).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json()["task_status"], "ready");
    assert_eq!(resp.json()["wait"]["state"], "approved");
    assert_eq!(f.env.status_of(task_id), Status::Ready);
    // 同じ要求の再送は冪等。
    let resp = send(&f.app, post_admin(&path, &approve)).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json()["replayed"], true);
    // 別の idempotency_key で同じ nonce（attestation の再利用）は 403。
    let mut reuse = approve.clone();
    reuse["idempotency_key"] = json!("k2");
    reuse["decision"] = json!("deny");
    let resp = send(&f.app, post_admin(&path, &reuse)).await;
    assert_problem(&resp, 403, "attestation_invalid");

    // 未消費の承認の失効: worker の消費は 410 相当で断られる。
    let revoke = send(
        &f.app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/browser/waits/{wait_id}/revoke"),
            &json!({
                "expected_version": 2, "idempotency_key": "k-revoke",
                "attestation": attest(&f.key, &Claims { version: 2, ..claims(task_id, &wait_id, "revoke", "n-r") }),
            }),
        ),
    )
    .await;
    assert_eq!(revoke.status, 200, "{}", revoke.text());
    assert_eq!(revoke.json()["wait"]["state"], "revoked");
    assert!(
        f.env
            .store
            .browser_wait_consume(
                task_id,
                &wait_id,
                "rk-appr",
                "run-1",
                "sess-1",
                OffsetDateTime::now_utc()
            )
            .is_err()
    );

    // 拒否: task は failed（自動 retry なし）。
    let task2 = running_task(&f.env);
    let w2 = open(&f, task2, &approval_request("rk-deny")).await;
    let w2_id = w2["wait_id"].as_str().expect("wait_id").to_string();
    let resp = send(
        &f.app,
        post_admin(
            &format!("/api/v1/tasks/{task2}/browser/waits/{w2_id}/decision"),
            &json!({
                "decision": "deny", "expected_version": 1, "idempotency_key": "k-deny",
                "attestation": attest(&f.key, &claims(task2, &w2_id, "deny", "n-deny")),
            }),
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json()["task_status"], "failed");
    assert_eq!(resp.json()["wait"]["resolution_code"], "approval_denied");
    assert_eq!(f.env.status_of(task2), Status::Failed);
}

#[tokio::test]
async fn expired_wait_is_gone_and_unconfigured_api_refuses_human_actions() {
    let f = fixture();
    let task_id = running_task(&f.env);
    let wait = open(&f, task_id, &approval_request("rk-exp")).await;
    let wait_id = wait["wait_id"].as_str().expect("wait_id").to_string();
    let expired = f
        .env
        .store
        .browser_waits_expire(OffsetDateTime::now_utc() + time::Duration::minutes(6))
        .expect("expire");
    assert_eq!(expired.len(), 1);
    assert_eq!(f.env.status_of(task_id), Status::Failed);
    let resp = send(
        &f.app,
        post_admin(
            &format!("/api/v1/tasks/{task_id}/browser/waits/{wait_id}/decision"),
            &json!({
                "decision": "approve_once", "expected_version": 1, "idempotency_key": "k",
                "attestation": attest(&f.key, &claims(task_id, &wait_id, "approve_once", "n")),
            }),
        ),
    )
    .await;
    assert_problem(&resp, 410, "browser_wait_expired");

    // 鍵も broker も無い既定の設定では、人の操作は 503。
    let env = admin_env();
    let app = env.router();
    let t = running_task(&env);
    let resp = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{t}/browser/requests"),
            &auth_request("rk-plain"),
        ),
    )
    .await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    let w = resp.json()["wait"]["wait_id"]
        .as_str()
        .expect("wait_id")
        .to_string();
    let resp = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{t}/browser/waits/{w}/decision"),
            &json!({
                "decision": "deny", "expected_version": 1, "idempotency_key": "k",
                "attestation": {"payload": "{}", "signature": "00"},
            }),
        ),
    )
    .await;
    assert_problem(&resp, 503, "browser_unavailable");
    // 知らない wait は 404、running でない task には開けない（409）。
    let resp = send(
        &app,
        get_admin(&format!("/api/v1/tasks/{}/browser/waits", TaskId::new())),
    )
    .await;
    assert_eq!(resp.status, 404);
    let resp = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{t}/browser/requests"),
            &auth_request("rk-plain-2"),
        ),
    )
    .await;
    assert_problem(&resp, 409, "task_not_running");
}
