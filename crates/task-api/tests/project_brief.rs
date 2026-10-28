//! ADR 2026-10-27-project-brief: 案件の状態の要約（`PUT /projects/{id}/status`・CoS op
//! `project.status_put`）と、`GET /projects` の要約・件数・最終活動時刻・人待ち。
mod common;

use common::cos_ops::{OPS, db, op_body};
use common::*;
use serde_json::{Value, json};
use task_core::chat::{ChatOpenThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::{ProjectId, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};

fn brief_body(state: &str) -> Value {
    json!({
        "objective": "論文初版の計測を終える",
        "state": state,
        "open_issues": ["E3 の再計測"],
        "next_actions": ["Sirius の保守明けに投入"],
        "waiting_on": [{"kind": "external", "target": "sirius", "note": "保守"}],
        "evidence_refs": ["run:e3-measure"],
    })
}

/// A live CoS run of `thread_id`: `"Bearer …"`.
fn bearer_for(env: &TestEnv, thread_id: &str, key: &str) -> String {
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_message_post(
            thread_id,
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
    env.store
        .chat_run_claim_next(thread_id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, thread_id, &run, Duration::hours(1))
        .expect("issue");
    env.store
        .chat_run_record_policy(&run, "1", "test")
        .expect("policy");
    format!("Bearer {bearer}")
}

fn project_of(thread: &task_core::chat::ChatThread) -> ProjectId {
    thread
        .project_id
        .as_deref()
        .expect("project thread")
        .parse()
        .expect("project id")
}

#[tokio::test]
async fn project_brief_admin_put_replaces_and_shows_in_list_and_detail() {
    let env = admin_env();
    let app = env.router();
    let thread = project_cos_thread(&env.store, "brief");
    let project = project_of(&thread);
    let path = format!("/api/v1/projects/{project}/status");

    // No token: 401 (管理系).
    let anon = send(&app, put_json_with(&path, &brief_body("一"), &[])).await;
    assert_eq!(anon.status.as_u16(), 401, "{}", anon.text());

    let first = send(
        &app,
        put_json_with(&path, &brief_body("一"), &admin_headers()),
    )
    .await;
    assert_eq!(first.status.as_u16(), 200, "{}", first.text());
    assert_eq!(first.json()["updated_by"], TEST_ACTOR);
    assert_eq!(first.json()["project_id"], project.to_string());

    let mut second = brief_body("二");
    second["open_issues"] = json!([]);
    let resp = send(&app, put_json_with(&path, &second, &admin_headers())).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());

    let detail = send(&app, get_admin(&format!("/api/v1/projects/{project}"))).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    let brief = &detail.json()["brief"];
    assert_eq!(brief["state"], "二");
    assert_eq!(
        brief["open_issues"],
        json!([]),
        "PUT replaces the whole brief"
    );
    assert_eq!(brief["waiting_on"][0]["kind"], "external");

    let list = send(&app, get_admin("/api/v1/projects")).await;
    let item = list.json()["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|p| p["id"] == project.to_string())
        .cloned()
        .expect("listed");
    assert_eq!(item["title"], "brief", "project fields stay flat");
    assert_eq!(item["brief"]["state"], "二");

    // Validation: blank state, foreign task, unknown field; missing project.
    let mut bad = brief_body(" ");
    bad["waiting_on"] =
        json!([{"kind": "task", "target": task_core::TaskId::new().to_string(), "note": "x"}]);
    let resp = send(&app, put_json_with(&path, &bad, &admin_headers())).await;
    let problem = assert_problem(&resp, 422, "validation");
    let fields: Vec<_> = problem["errors"]
        .as_array()
        .expect("errors")
        .iter()
        .map(|e| e["field"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(fields, ["state", "waiting_on[0].target"], "{problem}");
    let mut unknown = brief_body("x");
    unknown["secretary_summary"] = json!("old");
    let resp = send(&app, put_json_with(&path, &unknown, &admin_headers())).await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    let resp = send(
        &app,
        put_json_with(
            &format!("/api/v1/projects/{}/status", ProjectId::new()),
            &brief_body("x"),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());
}

#[tokio::test]
async fn project_list_reports_task_counts_last_activity_and_human_waits() {
    let env = admin_env();
    let app = env.router();
    let thread = project_cos_thread(&env.store, "activity");
    let project = project_of(&thread);
    let quiet = project_of(&project_cos_thread(&env.store, "quiet"));
    let base = OffsetDateTime::now_utc() + Duration::days(1);
    for (i, status) in [Status::Ready, Status::Ready, Status::Failed, Status::Done]
        .into_iter()
        .enumerate()
    {
        let mut t = new_task(TaskKind::Execute, status);
        t.project_id = Some(project);
        t.updated_at = base + Duration::minutes(i as i64);
        env.store.insert(&t).expect("insert");
    }

    let list = send(&app, get_admin("/api/v1/projects")).await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let items = list.json()["items"].as_array().expect("items").clone();
    let item = items
        .iter()
        .find(|p| p["id"] == project.to_string())
        .expect("listed");
    assert_eq!(
        item["task_counts"],
        json!({"ready": 2, "failed": 1, "done": 1})
    );
    let last = OffsetDateTime::parse(
        item["last_activity_at"].as_str().expect("last activity"),
        &time::format_description::well_known::Rfc3339,
    )
    .expect("rfc3339");
    assert_eq!(
        last.unix_timestamp(),
        (base + Duration::minutes(3)).unix_timestamp()
    );
    assert!(item.get("brief").is_none(), "no brief yet: {item}");

    // Human waits are counted exactly as `GET /inbox/items?project=` counts them.
    let inbox = send(
        &app,
        get_admin(&format!("/api/v1/inbox/items?project={project}")),
    )
    .await;
    let inbox_items = inbox.json()["items"].as_array().expect("inbox").clone();
    assert!(!inbox_items.is_empty(), "the failed task waits on a person");
    assert_eq!(item["human_waits"], inbox_items.len());
    let oldest = inbox_items
        .iter()
        .map(|i| i["created_at"].as_str().expect("created_at").to_string())
        .map(|at| {
            OffsetDateTime::parse(&at, &time::format_description::well_known::Rfc3339)
                .expect("rfc3339")
        })
        .min()
        .expect("oldest");
    let reported = OffsetDateTime::parse(
        item["oldest_human_wait_at"].as_str().expect("oldest wait"),
        &time::format_description::well_known::Rfc3339,
    )
    .expect("rfc3339");
    assert_eq!(reported, oldest);

    let quiet_item = items
        .iter()
        .find(|p| p["id"] == quiet.to_string())
        .expect("quiet listed");
    assert_eq!(quiet_item["task_counts"], json!({}));
    assert_eq!(quiet_item["human_waits"], 0);
    assert!(quiet_item.get("oldest_human_wait_at").is_none());
}

#[tokio::test]
async fn project_status_put_is_written_by_the_project_cos_only() {
    let env = admin_env();
    let app = env.router();
    let thread = project_cos_thread(&env.store, "own");
    let project = project_of(&thread);
    let path = format!("/api/v1/projects/{project}/status");

    // The project's own CoS: applied, audited, `updated_by = cos:<thread>`.
    let bearer = bearer_for(&env, &thread.id, "own");
    let headers = [("authorization", bearer.as_str())];
    let direct = send(&app, put_json_with(&path, &brief_body("一"), &headers)).await;
    assert_problem(&direct, 422, "cos_audit_context_required");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body("brief-own", "PUT", &path, brief_body("案件 CoS が書いた")),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "project.status_put");
    assert_eq!(op["target_id"], project.to_string());
    let brief = env
        .store
        .project_brief_get(project)
        .expect("get")
        .expect("brief");
    assert_eq!(brief.state, "案件 CoS が書いた");
    assert_eq!(brief.updated_by, format!("cos:{}", thread.id));

    // Another project's CoS and the general CoS are refused and recorded as rejected.
    let other = project_cos_thread(&env.store, "other");
    let general = env
        .store
        .cos_thread_open(
            &ChatOpenThreadRequest { project_id: None },
            OffsetDateTime::now_utc(),
        )
        .expect("general")
        .thread;
    for (key, thread_id) in [("other", other.id.clone()), ("general", general.id.clone())] {
        let bearer = bearer_for(&env, &thread_id, key);
        let headers = [("authorization", bearer.as_str())];
        let resp = send(
            &app,
            post_json_with(
                OPS,
                &op_body(&format!("brief-{key}"), "PUT", &path, brief_body(key)),
                &headers,
            ),
        )
        .await;
        assert_problem(&resp, 403, "project_brief_forbidden");
        let rejected: i64 = db(&env)
            .query_row(
                "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='rejected' \
                 AND target_kind='project'",
                [&thread_id],
                |row| row.get(0),
            )
            .expect("rejected row");
        assert_eq!(rejected, 1, "{key}");
    }
    assert_eq!(
        env.store
            .project_brief_get(project)
            .expect("get")
            .expect("brief")
            .state,
        "案件 CoS が書いた",
        "refused writes leave the brief alone"
    );
}
