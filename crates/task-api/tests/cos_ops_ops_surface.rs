//! ADR 2026-10-09-cos-operations-all-mutations D3 (WU ops-surface): the surface routes (chat,
//! console, org messages, artifact promotion, browser operations) run through `/cos/operations`.
//! Each is refused when called directly with the CoS credential (422 + rejected audit row) and
//! applied through the envelope with its audit record. Chat writes are one transaction (A); file,
//! run-start, docs and browser operations are two-stage external operations (C). A CoS chat
//! message never queues a CoS run, and browser control keeps the owner session's signed assertion,
//! origin and lease checks. Temporary DB, local git and in-process fakes only (no userns).
mod common;

use common::cos_ops::{OPS, cos_bearer, db, op_body, run_domain, run_external};
use common::*;
use ring::signature::KeyPair;
use serde_json::{Value, json};

/// The project repository's document directory (a fixture repo, not this one).
const DOCS: &str = "docs";
use task_api::browser::BrowserApiConfig;
use task_core::browser_wait::BrowserWaitStore;
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::{
    ArtifactRef, Event, GenreSpec, RoleSpec, RunIndexRole, RunIndexStatus, RunRow, Status,
    TaskKind, TaskStore, Tier,
};
use time::OffsetDateTime;

fn surface_env() -> TestEnv {
    let mut env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        roles: vec![RoleSpec {
            id: "secretary".into(),
            tier: Some(Tier::Standard),
            adapter: Some("claude-code".into()),
            ..RoleSpec::default()
        }],
        genres: vec![GenreSpec {
            id: "secretary".into(),
            description: "人と話す".into(),
            default_role: Some("secretary".into()),
            roles: vec!["secretary".into()],
            ..GenreSpec::default()
        }],
        ..EnvOptions::default()
    });
    env.state = env.state.clone().with_chat_attachments(
        env.dir.path().to_path_buf(),
        ChatAttachmentLimits::default(),
    );
    env
}

fn human_thread(env: &TestEnv, key: &str) -> Value {
    let thread = env
        .store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: format!("human {key}"),
                project_id: None,
                client_thread_id: key.into(),
            },
            OffsetDateTime::now_utc(),
        )
        .expect("thread")
        .thread;
    serde_json::to_value(thread).expect("thread json")
}

fn queue_message(env: &TestEnv, thread: &str, key: &str) -> String {
    env.store
        .chat_message_post(
            thread,
            &ChatPostMessageRequest {
                client_message_id: key.into(),
                text: "人の入力".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("message")
        .response
        .message
        .id
}

fn queued_inputs(env: &TestEnv, thread: &str) -> i64 {
    db(env)
        .query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE thread_id=?1 AND role='user' AND state='queued'",
            [thread],
            |row| row.get(0),
        )
        .expect("count")
}

fn operation_state(env: &TestEnv, thread: &str, key: &str) -> (String, String) {
    db(env)
        .query_row(
            "SELECT state, payload_json FROM cos_operations WHERE thread_id=?1 AND idempotency_key=?2",
            [thread, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("operation row")
}

/// Send one envelope on a fresh CoS run and expect it refused with `status`/`code` and recorded.
async fn expect_refused(
    env: &TestEnv,
    app: &axum::Router,
    key: &str,
    request: (&str, &str, Value),
    status: u16,
    code: &str,
) -> String {
    let (thread, _, bearer) = cos_bearer(env, key);
    let headers = [("authorization", bearer.as_str())];
    let (method, path, body) = request;
    let resp = send(
        app,
        post_json_with(OPS, &op_body(key, method, path, body), &headers),
    )
    .await;
    assert_problem(&resp, status, code);
    let (state, payload) = operation_state(env, &thread, key);
    assert_eq!(state, "rejected", "{key}");
    payload
}

#[tokio::test]
async fn cos_ops_ops_surface_chat_writes_are_audited_in_one_transaction() {
    let env = surface_env();

    let op = run_domain(
        &env,
        "thread-create",
        "POST",
        "/api/v1/chat/threads",
        json!({"title": "CoS が開いた相談", "client_thread_id": "cos-opened-1"}),
        "chat.thread_create",
    )
    .await;
    assert_eq!(op["result"]["created"], true, "{op}");
    let opened = op["result"]["thread"]["id"].as_str().expect("id");
    assert!(env.store.chat_thread_get(opened).expect("get").is_some());

    let thread = human_thread(&env, "rename-me");
    let id = thread["id"].as_str().expect("id").to_string();
    run_domain(
        &env,
        "thread-update",
        "PATCH",
        &format!("/api/v1/chat/threads/{id}"),
        json!({"title": "改名した", "expected_revision": thread["revision"]}),
        "chat.thread_update",
    )
    .await;
    let renamed = env
        .store
        .chat_thread_get(&id)
        .expect("get")
        .expect("thread");
    assert_eq!(renamed.title, "改名した");

    // Cancelling a queued human input.
    let thread = human_thread(&env, "cancel-me");
    let id = thread["id"].as_str().expect("id").to_string();
    let message = queue_message(&env, &id, "to-cancel");
    let op = run_domain(
        &env,
        "message-cancel",
        "DELETE",
        &format!("/api/v1/chat/threads/{id}/messages/{message}"),
        json!(null),
        "chat.message_cancel",
    )
    .await;
    assert_eq!(op["result"]["message"]["state"], "cancelled", "{op}");
    assert_eq!(queued_inputs(&env, &id), 0);

    // Stopping a running chat run pauses the queue; resuming it clears the pause.
    let thread = human_thread(&env, "stop-me");
    let id = thread["id"].as_str().expect("id").to_string();
    queue_message(&env, &id, "first");
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_run_claim_next(&id, "run-stop-me", &json!({}), now)
        .expect("claim")
        .expect("run");
    let op = run_domain(
        &env,
        "run-stop",
        "POST",
        &format!("/api/v1/chat/threads/{id}/stop"),
        json!({"run_id": "run-stop-me"}),
        "chat.run_stop",
    )
    .await;
    assert_eq!(op["result"]["accepted"], true, "{op}");
    let paused = env
        .store
        .chat_thread_get(&id)
        .expect("get")
        .expect("thread");
    assert!(paused.queue_paused);
    run_domain(
        &env,
        "queue-resume",
        "POST",
        &format!("/api/v1/chat/threads/{id}/resume-queue"),
        json!({"expected_revision": paused.revision}),
        "chat.queue_resume",
    )
    .await;
    assert!(
        !env.store
            .chat_thread_get(&id)
            .expect("get")
            .expect("thread")
            .queue_paused
    );

    let op = run_domain(
        &env,
        "new-conversation",
        "POST",
        "/api/v1/console/new-conversation",
        json!({"scope": "all"}),
        "console.new_conversation",
    )
    .await;
    let legacy = op["result"]["thread_id"].as_str().expect("thread id");
    assert!(env.store.chat_thread_get(legacy).expect("get").is_some());

    // A refused chat write leaves no domain change and a rejected record (same status as the route).
    let thread = human_thread(&env, "inbox-like");
    let id = thread["id"].as_str().expect("id").to_string();
    let app = env.router();
    expect_refused(
        &env,
        &app,
        "stale-revision",
        (
            "PATCH",
            &format!("/api/v1/chat/threads/{id}"),
            json!({"title": "古い", "expected_revision": 999}),
        ),
        409,
        "chat_conflict",
    )
    .await;
    assert_eq!(
        env.store
            .chat_thread_get(&id)
            .expect("get")
            .expect("thread")
            .title,
        "human inbox-like"
    );
}

/// A CoS chat message is a completed `assistant` notice: it does not enter the run queue, so it
/// cannot start a CoS run, in another thread or in the CoS run's own thread.
#[tokio::test]
async fn cos_ops_ops_surface_cos_message_does_not_chain_a_cos_run() {
    let env = surface_env();
    let thread = human_thread(&env, "notify-human");
    let id = thread["id"].as_str().expect("id").to_string();
    let op = run_domain(
        &env,
        "message-post",
        "POST",
        &format!("/api/v1/chat/threads/{id}/messages"),
        json!({"client_message_id": "cos-note-1", "text": "調査が終わりました",
               "attachment_ids": [], "reply_to_id": null, "mode": "queue", "resume_queue": false}),
        "chat.message_post",
    )
    .await;
    let message = &op["result"]["message"];
    assert_eq!(message["role"], "assistant", "{op}");
    assert_eq!(message["state"], "completed", "{op}");
    assert_eq!(queued_inputs(&env, &id), 0);
    let now = OffsetDateTime::now_utc();
    assert!(
        env.store
            .chat_run_claim_next(&id, "would-chain", &json!({}), now)
            .expect("claim")
            .is_none(),
        "a CoS message must not start a CoS run"
    );

    // The CoS run's own thread: after its run finishes, nothing is left to claim.
    let app = env.router();
    let (own, run, bearer) = cos_bearer(&env, "own-thread");
    let headers = [("authorization", bearer.as_str())];
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "own-note",
                "POST",
                &format!("/api/v1/chat/threads/{own}/messages"),
                json!({"client_message_id": "own-1", "text": "途中経過", "attachment_ids": [],
                       "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["operation"]["state"], "applied");
    assert_eq!(queued_inputs(&env, &own), 0);
    env.store
        .chat_run_finish(
            &run,
            task_core::chat::ChatRunState::Completed,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .expect("finish");
    assert!(
        env.store
            .chat_run_claim_next(
                &own,
                "would-chain-own",
                &json!({}),
                OffsetDateTime::now_utc()
            )
            .expect("claim")
            .is_none(),
        "the CoS run's own message must not start the next CoS run"
    );

    // Queue-affecting forms are refused and recorded.
    expect_refused(
        &env,
        &app,
        "interrupt",
        (
            "POST",
            &format!("/api/v1/chat/threads/{id}/messages"),
            json!({"client_message_id": "cos-2", "text": "割り込み", "attachment_ids": [],
                   "reply_to_id": null, "mode": "interrupt", "resume_queue": false}),
        ),
        422,
        "validation",
    )
    .await;

    // Talking to the CoS node would queue another CoS run: refused (cos_self_chain).
    let tasks_before = env.store.list(None).expect("tasks").len();
    expect_refused(
        &env,
        &app,
        "self-chain",
        (
            "POST",
            "/api/v1/org/cos/messages",
            json!({"text": "自分に話しかける"}),
        ),
        422,
        "cos_self_chain",
    )
    .await;
    assert_eq!(env.store.list(None).expect("tasks").len(), tasks_before);
}

#[tokio::test]
async fn cos_ops_ops_surface_files_runs_and_docs_are_external() {
    let env = surface_env();
    let app = env.router();

    // Upload (C, file): the bytes stay out of the audit record.
    let thread = human_thread(&env, "files");
    let id = thread["id"].as_str().expect("id").to_string();
    let op = run_external(
        &env,
        "upload",
        "POST",
        &format!("/api/v1/chat/threads/{id}/attachments"),
        json!({"client_upload_id": "up-1", "name": "note.txt",
               "content_base64": "U0VDUkVULWJ5dGVzLTQyCg=="}),
        "chat.attachment_upload",
    )
    .await;
    let attachment = op["result"]["attachment"]["id"]
        .as_str()
        .expect("attachment id")
        .to_string();
    assert_eq!(op["result"]["attachment"]["size_bytes"], 16, "{op}");
    let payloads: Vec<String> = {
        let conn = db(&env);
        let mut stmt = conn
            .prepare("SELECT payload_json FROM cos_operations")
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect()
    };
    assert!(
        payloads.iter().all(|p| !p.contains("U0VDUkVU")),
        "upload bytes must not be recorded"
    );
    run_external(
        &env,
        "attachment-delete",
        "DELETE",
        &format!("/api/v1/chat/attachments/{attachment}"),
        json!(null),
        "chat.attachment_delete",
    )
    .await;
    let gone = send(
        &app,
        get_admin(&format!("/api/v1/chat/attachments/{attachment}")),
    )
    .await;
    assert_eq!(gone.status.as_u16(), 404, "{}", gone.text());

    // org.message (C, run start): one conversation task assigned to the node.
    for body in [
        json!({"id": "secretary", "name": "秘書", "kind": "secretary", "genre": "secretary"}),
        json!({"id": "research", "name": "研究部", "kind": "department", "parent_id": "secretary"}),
    ] {
        let resp = send(&app, post_admin("/api/v1/org", &body)).await;
        assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    }
    let op = run_external(
        &env,
        "org-message",
        "POST",
        "/api/v1/org/research/messages",
        json!({"text": "先週の続きを見てほしい"}),
        "org.message",
    )
    .await;
    let task_id: task_core::TaskId = op["result"]["task_id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("ulid");
    let task = env.store.get(task_id).expect("get").expect("task");
    assert_eq!(task.assignee.as_deref(), Some("research"));
    let conversations = env
        .store
        .list(None)
        .expect("tasks")
        .into_iter()
        .filter(|t| t.assignee.as_deref() == Some("research"))
        .count();
    assert_eq!(
        conversations, 1,
        "the resent operation must not start a second run"
    );

    // artifact.promote (C, docs git) into a project's primary repository.
    let project = send(
        &app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "Pluvio PoC", "request": "書く"}),
        ),
    )
    .await;
    assert_eq!(project.status.as_u16(), 201, "{}", project.text());
    let project = project.json()["id"].as_str().expect("id").to_string();
    let repo = env.dir.path().join("primary");
    std::fs::create_dir_all(repo.join("docs")).expect("mkdir");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@example.com"],
        &["config", "user.name", "t"],
    ] {
        git(&repo, args);
    }
    std::fs::write(repo.join("docs/README.md"), "# 案件\n").expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    let created = send(
        &app,
        post_admin(
            &format!("/api/v1/projects/{project}/repos"),
            &json!({"location": {"kind": "local", "path": repo.to_string_lossy()}}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let mut task = new_task(TaskKind::Execute, Status::Done);
    task.project_id = Some(project.parse().expect("project id"));
    env.seed(&task);
    let dir = env.workspace(&task).join("artifacts");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("answer.md"), "# 調査の答え\n\n本文\n").expect("write");
    env.store
        .append_event(
            task.id,
            &Event::ArtifactProduced {
                run_id: ulid::Ulid::new().to_string(),
                artifact: ArtifactRef {
                    name: "answer.md".into(),
                    path: "artifacts/answer.md".into(),
                    sha256: "0".repeat(64),
                    kind: "file".into(),
                    declared: true,
                },
            },
        )
        .expect("artifact");
    let op = run_external(
        &env,
        "promote",
        "POST",
        &format!("/api/v1/tasks/{}/artifacts/promote", task.id),
        json!({"name": "answer.md", "path": format!("{DOCS}/research/answer.md")}),
        "artifact.promote",
    )
    .await;
    assert_eq!(op["target_kind"], "docs", "{op}");
    let shown = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["show", format!("main:{DOCS}/research/answer.md").as_str()])
        .output()
        .expect("git show");
    assert!(shown.status.success());
    assert!(String::from_utf8_lossy(&shown.stdout).contains("調査の答え"));
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn keypair() -> ring::signature::Ed25519KeyPair {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
    ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("key")
}

/// The Live View assertion the web signs for the owner session (`owner`, `origin_ok`).
fn sign(
    key: &ring::signature::Ed25519KeyPair,
    path: (&str, &str, &str),
    owner: &str,
    is_owner: bool,
    origin_ok: bool,
) -> Value {
    let payload = json!({"task_id": path.0, "run_id": path.1, "browser_session_id": path.2,
        "owner_session_id": owner, "owner_session": is_owner, "origin_ok": origin_ok,
        "expires_at": OffsetDateTime::now_utc().unix_timestamp() + 30})
    .to_string();
    let signature: String = key
        .sign(payload.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({"payload": payload, "signature": signature})
}

fn browser_env() -> (TestEnv, ring::signature::Ed25519KeyPair) {
    let mut env = surface_env();
    let key = keypair();
    env.state = env.state.clone().with_browser(BrowserApiConfig {
        attestation_public_key: Some(key.public_key().as_ref().to_vec()),
        broker: None,
    });
    (env, key)
}

#[tokio::test]
async fn cos_ops_ops_surface_browser_policies_and_requests_are_external() {
    let (env, _) = browser_env();
    run_external(
        &env,
        "site-put",
        "PUT",
        "/api/v1/browser/site-policies/pol-example",
        json!({"exact_origin": "https://login.example.com",
               "login_url": "https://login.example.com/login",
               "password_selector": "#password"}),
        "browser.site_policy_put",
    )
    .await;
    let policies = env.store.browser_site_policy_list().expect("list");
    assert_eq!(policies.len(), 1);
    run_external(
        &env,
        "site-delete",
        "DELETE",
        "/api/v1/browser/site-policies/pol-example",
        json!(null),
        "browser.site_policy_delete",
    )
    .await;
    assert!(
        env.store
            .browser_site_policy_list()
            .expect("list")
            .is_empty()
    );

    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    run_external(
        &env,
        "task-policy",
        "PUT",
        &format!("/api/v1/tasks/{}/browser/policy", task.id),
        json!({"policy_id": "read-only", "revision": 1, "domain_mode": "common_hosts",
               "network_domains": ["example.com"], "allowed_actions": ["navigate", "snapshot"],
               "approval_actions": [], "credential_policy_ids": []}),
        "browser.task_policy_put",
    )
    .await;
    assert!(
        env.store
            .browser_task_policy_get(task.id)
            .expect("policy")
            .is_some()
    );

    assert!(
        env.store
            .acquire_lease(task.id, "run-1", std::time::Duration::from_secs(60))
            .expect("lease")
    );
    let op = run_external(
        &env,
        "request-open",
        "POST",
        &format!("/api/v1/tasks/{}/browser/requests", task.id),
        json!({"run_id": "run-1", "session_id": "sess-1", "reason": "waiting_for_approval",
               "origin": "https://example.com", "purpose": "Download the report",
               "operation": {"intent_id": "intent-1", "action": "download", "args_digest": "sha256-args"},
               "policy_revision": 1, "policy_hash": "sha256-policy", "owner_id": "owner",
               "resume_key": "rk-cos-1"}),
        "browser.request_open",
    )
    .await;
    assert_eq!(op["result"]["created"], true, "{op}");

    // A wait naming a credential is the excluded credential/attestation series.
    let app = env.router();
    expect_refused(
        &env,
        &app,
        "credential-wait",
        (
            "POST",
            &format!("/api/v1/tasks/{}/browser/requests", task.id),
            json!({"run_id": "run-1", "session_id": "sess-1", "reason": "waiting_for_auth",
                   "origin": "https://login.example.com", "purpose": "Sign in",
                   "credential_policy_id": "pol-example", "policy_revision": 2,
                   "policy_hash": "sha256-policy", "owner_id": "owner", "resume_key": "rk-cos-2"}),
        ),
        422,
        "browser_credential_attestation",
    )
    .await;
}

fn start_run(env: &TestEnv, task: &str, run: &str) {
    env.store
        .run_index_start(RunRow {
            run_id: run.into(),
            task_id: task.into(),
            work_unit_id: None,
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Running,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: "2026-10-09T00:00:00Z".into(),
            finished_at: None,
        })
        .expect("run index");
}

/// Browser control through CoS keeps the route's checks: only the owner session's signed
/// assertion, the assertion's origin for resume, and the lease holder; the assertion is not
/// recorded. The worker routes (agent begin/end, auth section, live events) keep daemon auth.
#[tokio::test]
async fn cos_ops_ops_surface_browser_control_keeps_owner_origin_and_lease_checks() {
    let (env, key) = browser_env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    assert!(
        env.store
            .acquire_lease(task.id, "r1", std::time::Duration::from_secs(60))
            .expect("lease")
    );
    let id = task.id.to_string();
    start_run(&env, &id, "r1");
    let session = (id.as_str(), "r1", "browser-1");
    let base = format!("/api/v1/tasks/{id}/browser/control/r1/browser-1");

    let op = run_external(
        &env,
        "agent-begin",
        "POST",
        &format!("{base}/agent/begin"),
        json!(null),
        "browser.agent_begin",
    )
    .await;
    assert_eq!(op["result"]["in_flight"], 1, "{op}");
    run_external(
        &env,
        "agent-end",
        "POST",
        &format!("{base}/agent/end"),
        json!(null),
        "browser.agent_end",
    )
    .await;

    // Not the owner session: refused before any transition.
    let payload = expect_refused(
        &env,
        &app,
        "not-owner",
        (
            "POST",
            &base,
            json!({"assertion": sign(&key, session, "viewer", false, true),
                   "command": {"kind": "pause"}, "expected_version": 0,
                   "idempotency_key": "c-0"}),
        ),
        403,
        "not_owner_session",
    )
    .await;
    assert!(!payload.contains("signature"), "{payload}");

    let op = run_external(
        &env,
        "pause",
        "POST",
        &base,
        json!({"assertion": sign(&key, session, "owner", true, true),
               "command": {"kind": "pause"}, "expected_version": 0, "idempotency_key": "c-1"}),
        "browser.control",
    )
    .await;
    assert_eq!(op["result"]["phase"], "paused", "{op}");
    let version = op["result"]["version"].as_u64().expect("version");
    let op = run_external(
        &env,
        "takeover",
        "POST",
        &base,
        json!({"assertion": sign(&key, session, "owner", true, true),
               "command": {"kind": "takeover", "ttl_secs": 60}, "expected_version": version,
               "idempotency_key": "c-2"}),
        "browser.control",
    )
    .await;
    let version = op["result"]["version"].as_u64().expect("version");
    let recorded: String = db(&env)
        .query_row(
            "SELECT payload_json FROM cos_operations WHERE id=?1",
            [op["id"].as_str().expect("id")],
            |row| row.get(0),
        )
        .expect("payload");
    assert!(recorded.contains("redacted"), "{recorded}");
    assert!(!recorded.contains("signature"), "{recorded}");
    let holder: Option<String> = db(&env)
        .query_row(
            "SELECT lease_holder FROM browser_control_state WHERE task_id=?1",
            [&id],
            |row| row.get(0),
        )
        .expect("state");
    assert_eq!(
        holder.as_deref(),
        Some("owner"),
        "the owner session holds the lease, not CoS"
    );

    // Another owner session cannot renew the lease it does not hold.
    expect_refused(
        &env,
        &app,
        "other-renew",
        (
            "POST",
            &base,
            json!({"assertion": sign(&key, session, "someone-else", true, true),
                   "command": {"kind": "renew", "ttl_secs": 60}, "expected_version": version,
                   "idempotency_key": "c-3"}),
        ),
        403,
        "not_lease_holder",
    )
    .await;
    // Resume needs the assertion's origin check as well.
    expect_refused(
        &env,
        &app,
        "origin-mismatch",
        (
            "POST",
            &base,
            json!({"assertion": sign(&key, session, "owner", true, false),
                   "command": {"kind": "resume", "fresh_snapshot": true, "policy_origin_ok": true},
                   "expected_version": version, "idempotency_key": "c-4"}),
        ),
        422,
        "resume_not_verified",
    )
    .await;
    // An assertion for another browser session does not verify.
    expect_refused(
        &env,
        &app,
        "other-session",
        (
            "POST",
            &base,
            json!({"assertion": sign(&key, (id.as_str(), "r1", "browser-2"), "owner", true, true),
                   "command": {"kind": "stop"}, "expected_version": version,
                   "idempotency_key": "c-5"}),
        ),
        403,
        "not_owner_session",
    )
    .await;

    run_external(
        &env,
        "disconnect",
        "POST",
        &format!("{base}/disconnect"),
        json!({"assertion": sign(&key, session, "owner", true, true)}),
        "browser.control_disconnect",
    )
    .await;
    let op = run_external(
        &env,
        "auth-section",
        "POST",
        &format!("{base}/auth-section"),
        json!({"active": true}),
        "browser.auth_section",
    )
    .await;
    assert_eq!(op["result"]["auth_section"], true, "{op}");

    let live = format!("/api/v1/tasks/{id}/browser/live/r1/browser-1/events");
    let op = run_external(
        &env,
        "live-event",
        "POST",
        &live,
        json!({"kind": "url", "url": "https://example.com/report"}),
        "browser.live_event",
    )
    .await;
    assert!(op["result"]["seq"].as_u64().is_some(), "{op}");
}
