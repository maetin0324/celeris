//! ADR 2026-10-27-unblocks-reopen-on-promotion: `POST /api/v1/tasks`（と CoS の `task.create`）の
//! `unblocks` は修正 task と待っている task を作成と同じ transaction で結ぶ。修正 task を含む release の
//! 昇格（`promoted.json` の `included.tasks`）で、待っていた `failed` の task が 1 度だけ reopen される。
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::{ChatPostMessageRequest, ChatSendMode};
use task_core::transition::Trigger;
use task_core::unblocks::UnblockAction;
use task_core::{Event, Status, TaskId, TaskStore};
use time::{Duration, OffsetDateTime};

const OPS: &str = "/api/v1/cos/operations";

fn body(title: &str, unblocks: &[String]) -> Value {
    json!({
        "title": title,
        "objective": "o",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
        "unblocks": unblocks,
    })
}

async fn create(app: &axum::Router, title: &str, unblocks: &[String]) -> Resp {
    send(app, post_admin("/api/v1/tasks", &body(title, unblocks))).await
}

fn task_id(resp: &Resp) -> TaskId {
    resp.json()["id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("ulid")
}

fn count(env: &TestEnv, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(&env.db_path).expect("db");
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}

fn write_release(dir: &std::path::Path, sha12: &str, promoted_at: &str, fix: TaskId) {
    let release = dir.join(sha12);
    std::fs::create_dir_all(&release).expect("release dir");
    std::fs::write(
        release.join("promoted.json"),
        json!({
            "promoted_at": promoted_at,
            "mode": "live",
            "from": null,
            "included": {
                "releases": [sha12],
                "tasks": [{"task_id": fix.to_string(), "title": "fix"}],
                "complete": true,
            },
        })
        .to_string(),
    )
    .expect("promoted.json");
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339")
}

fn reopens(env: &TestEnv, id: TaskId) -> usize {
    env.store
        .events_for(id)
        .expect("events")
        .iter()
        .filter(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "reopen"))
        .count()
}

#[tokio::test]
async fn unblocks_api_links_and_promotion_reopens_the_failed_task_once() {
    let env = admin_env();
    let app = env.router();
    // 待っている task: 作ってから失敗させる（Ready → Failed）。
    let waiting = task_id(&create(&app, "waits for the fix", &[]).await);
    env.store
        .apply_transition(waiting, Trigger::TokenBudgetExhausted, None)
        .expect("fail the waiting task");

    let fix = create(&app, "the fix", &[waiting.to_string()]).await;
    assert_eq!(fix.status.as_u16(), 201, "{}", fix.text());
    let fix = task_id(&fix);
    let links = env.store.unblocks_of(fix).expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].waiting_task_id, waiting);
    assert_eq!(links[0].fired_at, None);

    let releases = tempfile::tempdir().expect("releases dir");
    // 昇格は結びの作成より後（ADR 付記 A1: 作成前の昇格は見ない）。
    let now = OffsetDateTime::now_utc();
    write_release(
        releases.path(),
        "0123456789ab",
        &rfc3339(now + Duration::hours(1)),
        fix,
    );
    let fired = task_ops::unblocks::sweep(
        &env.store,
        &task_ops::unblocks::read_promoted_releases(releases.path()),
        now,
    )
    .expect("sweep");
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].action, UnblockAction::Reopened);
    assert_eq!(fired[0].release, "0123456789ab");
    assert_eq!(
        env.store.get(waiting).unwrap().unwrap().status,
        Status::Ready
    );
    assert_eq!(reopens(&env, waiting), 1);

    // 二度目の昇格（修正を含む後の release）では重ならない。
    env.store
        .apply_transition(waiting, Trigger::TokenBudgetExhausted, None)
        .expect("the reopened task fails again");
    write_release(
        releases.path(),
        "ba9876543210",
        &rfc3339(now + Duration::hours(3)),
        fix,
    );
    let fired = task_ops::unblocks::sweep(
        &env.store,
        &task_ops::unblocks::read_promoted_releases(releases.path()),
        now,
    )
    .expect("second sweep");
    assert!(fired.is_empty(), "{fired:?}");
    assert_eq!(reopens(&env, waiting), 1);
    assert_eq!(
        env.store.get(waiting).unwrap().unwrap().status,
        Status::Failed
    );
}

#[tokio::test]
async fn unblocks_api_rejects_unknown_and_duplicate_ids_and_creates_nothing() {
    let env = admin_env();
    let app = env.router();
    let waiting = task_id(&create(&app, "waiting", &[]).await).to_string();
    let unknown = create(&app, "fix", &[TaskId::new().to_string()]).await;
    assert_problem(&unknown, 422, "validation");
    assert_eq!(unknown.json()["errors"][0]["field"], "unblocks");
    let twice = create(&app, "fix", &[waiting.clone(), waiting.clone()]).await;
    assert_problem(&twice, 422, "validation");
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 1);
    assert_eq!(count(&env, "SELECT COUNT(*) FROM task_unblocks"), 0);
}

#[tokio::test]
async fn unblocks_cos_task_create_writes_the_link_in_the_audited_transaction() {
    let env = admin_env();
    let app = env.router();
    let waiting = task_id(&create(&app, "waiting", &[]).await);
    let thread = common::project_cos_thread(&env.store, "unblocks-cos").id;
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "m-1".into(),
                text: "修正を起票して".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    env.store
        .chat_run_claim_next(&thread, "run-unblocks", &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = format!(
        "Bearer {}",
        task_api::cos::issue_run_bearer(&env.store, &thread, "run-unblocks", Duration::hours(1))
            .expect("issue")
    );
    env.store
        .chat_run_record_policy("run-unblocks", "1", "test")
        .expect("policy");
    let created = send(
        &app,
        post_json_with(
            OPS,
            &json!({
                "idempotency_key": "create-fix",
                "expected_revision": null,
                "reason": "待っている task を直す修正を起票する",
                "policy_version": "1",
                "request": {
                    "method": "POST",
                    "path": "/api/v1/tasks",
                    "body": body("the fix", &[waiting.to_string()]),
                },
            }),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 200, "{}", created.text());
    let operation = &created.json()["operation"];
    assert_eq!(operation["state"], "applied");
    let fix: TaskId = operation["result"]["task_id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("ulid");
    let links = env.store.unblocks_of(fix).expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].waiting_task_id, waiting);
}
