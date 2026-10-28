//! ADR 2026-10-28-cos-integrate-task: a delivery conflict that auto-resolve cannot settle becomes an
//! integration request; CoS answers it with `integrate_task` from the inbox triage, which files an
//! ordinary root task linked to the request; once the integrate task is done (reviewer pass) and the
//! delivery has fast-forwarded main, the daemon's settle closes the original request. A failed
//! integrate task puts the request back in front of a person, and CoS can no longer answer it.
//! Everything is deterministic: real temporary git repositories, no LLM, no worker run.
mod common;

use std::path::Path;
use std::process::Command;

use common::cos_ops::with_basis;
use common::*;
use serde_json::{Value, json};
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{ChatPostMessageRequest, ChatSendMode};
use task_core::integration_request::{DELIVERY_ORIGIN, IntegrationRequest};
use task_core::{
    Event, Project, ProjectId, ProjectStatus, RepoId, RepoKind, RepoRef, RepoRun, Status, Task,
    TaskKind, TaskStore, Trigger, WorkspaceSpec,
};
use time::{Duration, OffsetDateTime};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo_dir: std::path::PathBuf,
    original: Task,
    request: IntegrationRequest,
    request_id: String,
}

/// A project repo whose `main` and the original task's branch changed the same line, the done
/// original task, and the delivery's integration request for that pair of heads.
fn fixture(env: &TestEnv) -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("repo");
    std::fs::create_dir(&dir).expect("dir");
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("lib.rs"), "fn a() {}\n").expect("write");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "base"]);
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        auto_integrate: true,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    env.store.project_create(&project).expect("project");
    let repo = task_core::ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "app".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::Local {
            path: dir.clone(),
            mode: None,
        },
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::default(),
        is_primary: true,
        created_at: now,
    };
    env.store.repo_create(&repo).expect("repo");
    let mut original = new_task(TaskKind::Execute, Status::Done);
    original.title = "機能 X".into();
    original.project_id = Some(project.id);
    original.repos = vec![RepoRef::of(&repo)];
    env.seed(&original);
    let branch = format!("celeris/{}", original.id);
    git(&dir, &["checkout", "-qb", &branch]);
    std::fs::write(dir.join("lib.rs"), "fn a() { feature() }\n").expect("write");
    git(&dir, &["commit", "-qam", "feature"]);
    let source = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("lib.rs"), "fn a() { main() }\n").expect("write");
    git(&dir, &["commit", "-qam", "main"]);
    let target = git(&dir, &["rev-parse", "HEAD"]);
    let request = IntegrationRequest {
        target_branch: "main".into(),
        target_sha: target,
        source_branch: branch,
        source_sha: source,
        merge_base: None,
        conflict_files: vec!["lib.rs".into()],
        intent: vec![],
        reason: "人の判断が必要: code conflict".into(),
        recommendation: "両側の意図を保って解く".into(),
        actions: vec![],
        candidate_sha: None,
    };
    let request_id = request.id_for(original.id);
    assert!(
        env.store
            .integration_request_record(original.id, &request, DELIVERY_ORIGIN)
            .expect("record")
    );
    Fixture {
        _tmp: tmp,
        repo_dir: dir,
        original,
        request,
        request_id,
    }
}

/// The worker's part, done by hand: merge the original head into a branch cut from main, resolve,
/// commit; then the delivery's fast-forward of main.
fn resolve_and_deliver(fx: &Fixture) {
    let dir = &fx.repo_dir;
    git(dir, &["checkout", "-qb", "celeris/integrate"]);
    let merge = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["merge", "--no-ff", "-q", &fx.request.source_sha])
        .output()
        .expect("git merge");
    assert!(!merge.status.success(), "the fixture must conflict");
    std::fs::write(dir.join("lib.rs"), "fn a() { main(); feature() }\n").expect("write");
    git(dir, &["add", "lib.rs"]);
    git(dir, &["commit", "-qm", "resolve"]);
    // The integrate task's own acceptance check holds in its worktree.
    git(
        dir,
        &[
            "merge-base",
            "--is-ancestor",
            &fx.request.source_sha,
            "HEAD",
        ],
    );
    git(dir, &["checkout", "-q", "main"]);
    git(dir, &["merge", "--ff-only", "-q", "celeris/integrate"]);
}

fn cos_bearer(env: &TestEnv, key: &str) -> (String, String) {
    let store = &env.store;
    let now = OffsetDateTime::now_utc();
    let thread = common::project_cos_thread(store, &ProjectId::new().to_string());
    store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "input".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    let run = format!("run-{key}");
    store
        .chat_run_claim_next(&thread.id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(store, &thread.id, &run, Duration::hours(1))
        .expect("issue");
    (thread.id, format!("Bearer {bearer}"))
}

fn ingest(env: &TestEnv, key: &str, revision: &str) -> String {
    let source = CosTriageSource {
        source_kind: "inbox".into(),
        source_key: key.into(),
        source_revision: revision.into(),
        source_event_id: None,
        operation_id: None,
        summary: format!("inbox {key}"),
        policy_version: "1".into(),
        task_id: None,
    };
    let at = OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time");
    env.store
        .cos_triage_ingest_batch("inbox", revision, &[source], at)
        .expect("ingest");
    rusqlite::Connection::open(&env.db_path)
        .expect("db")
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1 AND source_revision=?2",
            [key, revision],
            |r| r.get(0),
        )
        .expect("ingested row")
}

fn resolve_body(key: &str, revision: &str, outcome: &str, extra: Value) -> Value {
    let mut body = json!({
        "idempotency_key": key,
        "expected_revision": revision,
        "outcome": outcome,
        "reason": "統合の衝突を integrate task に解かせる",
        "policy_version": "1",
        "confidence": 0.9,
    });
    if let (Some(object), Some(more)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            object.insert(k.clone(), v.clone());
        }
    }
    body
}

async fn integration_items(app: &axum::Router) -> Vec<Value> {
    let resp = send(
        app,
        get_admin("/api/v1/inbox/items?kind=integration_request"),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    resp.json()["items"].as_array().expect("items").clone()
}

fn linked_task(env: &TestEnv, fx: &Fixture) -> (task_core::TaskId, String) {
    env.store
        .events_for(fx.original.id)
        .expect("events")
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::IntegrateTaskRequested {
                request_id,
                integrate_task_id,
                by,
            } if request_id == fx.request_id => Some((integrate_task_id, by)),
            _ => None,
        })
        .expect("link event")
}

fn open_requests(env: &TestEnv) -> usize {
    env.store.open_integration_requests().expect("open").len()
}

#[tokio::test]
async fn cos_integrate_task_resolves_the_conflict_and_closes_the_original_request() {
    let env = admin_env();
    let app = env.router();
    let fx = fixture(&env);

    // The conflict is in the inbox with the integrate_task option first and a native path.
    let items = integration_items(&app).await;
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert_eq!(item["options"][0]["key"], "integrate_task");
    assert_eq!(item["recommended"], "integrate_task");
    let native = format!(
        "/api/v1/tasks/{}/integration-requests/{}/integrate-task",
        fx.original.id, fx.request_id
    );
    assert_eq!(item["answer"]["native"]["path"], native.as_str());

    // CoS answers the triage item with integrate_task: the same operation layer files the task.
    let triage = ingest(&env, item["id"].as_str().expect("id"), "r1");
    let (thread, bearer) = cos_bearer(&env, "it");
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/cos/inbox/{triage}/resolve"),
            &with_basis(
                resolve_body(
                    "k-it",
                    "r1",
                    "answer",
                    json!({"answer": {"option": "integrate_task", "note": "両方の呼び出しを残す"}}),
                ),
                json!({"category": "documented_default",
                       "refs": [{"kind": "adr", "id": "agent-docs/adr/2026-10-28-cos-integrate-task.md"}]}),
            ),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(
        resp.json()["operation"]["action"],
        "integration.integrate_task"
    );
    assert_eq!(resp.json()["operation"]["state"], "applied");

    // The link names the CoS principal; the new root task carries the request and its checks.
    let (integrate_id, by) = linked_task(&env, &fx);
    assert_eq!(by, format!("cos:{thread}"));
    let integrate = env.store.get(integrate_id).expect("get").expect("task");
    assert_eq!(integrate.parent_id, None);
    assert_eq!(integrate.project_id, fx.original.project_id);
    assert_eq!(
        integrate
            .repos
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["app"]
    );
    assert!(integrate.labels.contains(&"integrate-task".to_string()));
    assert!(
        integrate
            .objective
            .contains(&format!("git merge --no-ff {}", fx.request.source_sha))
    );
    assert!(integrate.objective.contains("両方の呼び出しを残す"));
    assert!(integrate.acceptance.iter().any(|c| matches!(&c.check,
        task_core::Check::Command { cmd, .. } if cmd.contains(&format!("--is-ancestor {} HEAD", fx.request.source_sha)))));
    assert!(
        integrate
            .acceptance
            .iter()
            .any(|c| c.check == task_core::Check::Reviewer)
    );
    let created_by = env
        .store
        .events_for(integrate_id)
        .expect("events")
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::Created { created_by, .. } => Some(created_by),
            _ => None,
        })
        .expect("created");
    assert_eq!(
        created_by.as_deref(),
        Some(format!("cos:{thread}").as_str())
    );

    // While the integrate task works, the request is not shown, and a second one is refused.
    assert!(integration_items(&app).await.is_empty());
    let again = send(&app, post_admin(&native, &json!({}))).await;
    assert_problem(&again, 409, "integrate_task_exists");

    // Not done yet: settle changes nothing.
    assert!(
        task_ops::integrate_task::settle(&env.store)
            .expect("settle")
            .is_empty()
    );
    // The worker resolves, the reviewer passes, the delivery fast-forwards main.
    assert_eq!(integrate.status, Status::Ready);
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        env.store
            .apply_transition(integrate_id, trigger, None)
            .expect("transition");
    }
    assert_eq!(env.status_of(integrate_id), Status::Done);
    resolve_and_deliver(&fx);

    let settled = task_ops::integrate_task::settle(&env.store).expect("settle");
    assert_eq!(settled.len(), 1, "{settled:?}");
    assert_eq!(open_requests(&env), 0);
    let events = env.store.events_for(fx.original.id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(e,
        Event::IntegrationAnswered { request_id, answer, .. }
            if *request_id == fx.request_id && answer == "integrated")));
    assert!(events.iter().any(|(_, e)| matches!(e,
        Event::IntegrateTaskSettled { outcome, integrate_task_id, .. }
            if outcome == "integrated" && *integrate_task_id == integrate_id)));
    assert!(integration_items(&app).await.is_empty());
}

#[tokio::test]
async fn cos_integrate_task_failure_escalates_the_request_to_a_person() {
    let env = admin_env();
    let app = env.router();
    let fx = fixture(&env);

    // A person can choose the same option in the inbox; it goes to the same route.
    let items = integration_items(&app).await;
    let id = items[0]["id"].as_str().expect("id").to_owned();
    let resp = send(
        &app,
        post_admin(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option": "integrate_task"}),
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let (integrate_id, by) = linked_task(&env, &fx);
    assert!(by.starts_with("human:"), "{by}");

    // The integrate task fails (e.g. its checks never pass).
    for trigger in [Trigger::Dispatch, Trigger::WorkerError { retryable: false }] {
        env.store
            .apply_transition(integrate_id, trigger, None)
            .expect("transition");
    }
    assert_eq!(env.status_of(integrate_id), Status::Failed);
    let settled = task_ops::integrate_task::settle(&env.store).expect("settle");
    assert_eq!(settled.len(), 1, "{settled:?}");

    // The request stays open and is back in the inbox for a person, without integrate_task.
    assert_eq!(open_requests(&env), 1);
    let items = integration_items(&app).await;
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert!(
        item["title"]
            .as_str()
            .expect("title")
            .contains("integrate task が失敗"),
        "{item}"
    );
    assert!(
        item["detail"]
            .as_str()
            .expect("detail")
            .contains(&integrate_id.to_string())
    );
    assert!(
        item["options"]
            .as_array()
            .expect("options")
            .iter()
            .all(|o| o["key"] != "integrate_task")
    );

    // CoS cannot answer it (human_required) and cannot file a second integrate task.
    let triage = ingest(&env, item["id"].as_str().expect("id"), "r2");
    let (_thread, bearer) = cos_bearer(&env, "esc");
    let headers = [("authorization", bearer.as_str())];
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/cos/inbox/{triage}/resolve"),
            &resolve_body(
                "k-esc",
                "r2",
                "answer",
                json!({"answer": {"option": "declined"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 403, "cos_human_required");
    let native = format!(
        "/api/v1/tasks/{}/integration-requests/{}/integrate-task",
        fx.original.id, fx.request_id
    );
    let resp = send(
        &app,
        post_json_with(
            "/api/v1/cos/operations",
            &json!({
                "idempotency_key": "k-esc-op",
                "expected_revision": null,
                "reason": "もう一度",
                "policy_version": "1",
                "request": {"method": "POST", "path": native, "body": {}},
            }),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 409, "integrate_task_escalated");
    // Escalating it to the person is what remains for CoS.
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/cos/inbox/{triage}/resolve"),
            &resolve_body(
                "k-esc-2",
                "r2",
                "escalate",
                json!({"escalation": {
                    "summary": "integrate task が失敗した統合の依頼",
                    "options": item["options"].as_array().expect("options").iter()
                        .map(|o| json!({"key": o["key"], "label": o["label"]})).collect::<Vec<_>>(),
                    "recommended": null,
                    "recommendation_reason": "衝突の解き方を人が決める",
                    "web_path": format!("/tasks/{}", fx.original.id),
                }}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json()["item"]["state"], "escalated");
}

#[tokio::test]
async fn cos_integrate_task_refuses_unknown_and_non_delivery_requests() {
    let env = admin_env();
    let app = env.router();
    let fx = fixture(&env);
    let gone = send(
        &app,
        post_admin(
            &format!(
                "/api/v1/tasks/{}/integration-requests/{}:x:y/integrate-task",
                fx.original.id, fx.original.id
            ),
            &json!({}),
        ),
    )
    .await;
    assert_problem(&gone, 410, "integration_request_gone");

    let mut phase = new_task(TaskKind::Execute, Status::Blocked);
    phase.project_id = fx.original.project_id;
    phase.repos = fx.original.repos.clone();
    env.seed(&phase);
    let request = IntegrationRequest {
        target_branch: "celeris/parent".into(),
        ..fx.request.clone()
    };
    env.store
        .integration_request_record(phase.id, &request, "phase:p1")
        .expect("record");
    let resp = send(
        &app,
        post_admin(
            &format!(
                "/api/v1/tasks/{}/integration-requests/{}/integrate-task",
                phase.id,
                request.id_for(phase.id)
            ),
            &json!({}),
        ),
    )
    .await;
    assert_problem(&resp, 422, "integrate_task_unsupported_origin");
    assert!(
        env.store
            .events_for(phase.id)
            .expect("events")
            .iter()
            .all(|(_, e)| !matches!(e, Event::IntegrateTaskRequested { .. }))
    );
}
