//! ADR-0133 D5: the two feeds have disjoint jobs and retain the domain endpoints.
mod common;
use common::*;
use serde_json::json;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::integration_request::IntegrationRequest;
use task_core::report::{Report, ReportId, ReportKind, ReportStore};
use task_core::{Event, NoticeEvent, NoticeKind, NoticeStore, Status, TaskKind, TaskStore};
use time::OffsetDateTime;

#[tokio::test]
async fn judgment_is_only_in_inbox_and_answer_removes_it() {
    let env = admin_env();
    let task = new_task(TaskKind::Execute, Status::Blocked);
    env.seed_with(
        &task,
        vec![Event::WorkerFinished {
            run_id: ulid::Ulid::new().to_string(),
            outcome: "question: which database?".into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        }],
    );
    let app = env.router();
    let inbox = send(&app, get_admin("/api/v1/inbox/items")).await;
    assert_eq!(inbox.status, 200, "{}", inbox.text());
    let item = inbox.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["kind"] == "question")
        .cloned()
        .expect("question in inbox");
    let id = item["id"].as_str().unwrap();
    task_ops::notify_feed::sync_notifications(&env.store, OffsetDateTime::now_utc()).unwrap();
    let notices = send(&app, get_admin("/api/v1/notifications")).await;
    assert_eq!(notices.status, 200);
    assert_eq!(notices.json()["items"].as_array().unwrap().len(), 0);
    let answer = send(
        &app,
        post_admin(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option":"answer","note":"SQLite"}),
        ),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(answer.json()["removed"], true);
    let after = send(&app, get_admin("/api/v1/inbox/items")).await;
    assert!(
        after.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["id"] != id)
    );
    let old = send(&app, get_admin("/api/v1/inbox")).await;
    assert_eq!(old.status, 200);
    assert_eq!(old.header("deprecation"), Some("true"));
    assert!(old.json()["questions"].is_array());
}

#[tokio::test]
async fn integration_request_answer_appends_event_and_removes_only_its_inbox_item() {
    let env = admin_env();
    let task = new_task(TaskKind::Execute, Status::Running);
    env.seed(&task);
    let request = IntegrationRequest {
        target_branch: "main".into(),
        target_sha: "target-head".into(),
        source_branch: "feature".into(),
        source_sha: "source-head".into(),
        merge_base: None,
        conflict_files: vec!["src/lib.rs".into()],
        intent: vec![],
        reason: "conflict".into(),
        recommendation: "resolve both sides".into(),
        actions: vec![],
        candidate_sha: None,
    };
    let request_id = request.id_for(task.id);
    assert!(
        env.store
            .integration_request_record(task.id, &request, "test")
            .unwrap()
    );
    let app = env.router();
    let inbox = send(
        &app,
        get_admin("/api/v1/inbox/items?kind=integration_request"),
    )
    .await;
    assert_eq!(inbox.status, 200, "{}", inbox.text());
    let items = inbox.json()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1, "{items:?}");
    let id = items[0]["id"].as_str().unwrap();
    task_ops::notify_feed::sync_notifications(&env.store, OffsetDateTime::now_utc()).unwrap();
    let notices = send(&app, get_admin("/api/v1/notifications")).await;
    assert_eq!(notices.status, 200, "{}", notices.text());
    assert!(
        notices.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["target"]["kind"] != "integration_request")
    );

    let answer = send(
        &app,
        post_admin(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option":"integrated","note":"merged locally"}),
        ),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(answer.json()["removed"], true);
    assert_eq!(answer.json()["result"]["request_id"], request_id);
    assert!(env.store.events_for(task.id).unwrap().iter().any(|row| matches!(&row.1,
        Event::IntegrationAnswered { request_id: id, answer, note }
            if id.as_str() == request_id.as_str() && answer == "integrated" && note.as_deref() == Some("merged locally"))));
    let after = send(
        &app,
        get_admin("/api/v1/inbox/items?kind=integration_request"),
    )
    .await;
    assert_eq!(after.json()["counts"]["total"], 0);
    assert_eq!(
        send(
            &app,
            post_admin(
                &format!("/api/v1/inbox/items/{id}/answer"),
                &json!({"option":"retry"})
            )
        )
        .await
        .status,
        404
    );
    assert_eq!(
        send(
            &app,
            post_admin(
                "/api/v1/inbox/items/integration_request-unknown/answer",
                &json!({"option":"retry"})
            )
        )
        .await
        .status,
        404
    );
    for option in ["declined", "retry"] {
        let mut next = request.clone();
        next.source_sha = format!("source-{option}");
        let next_id = next.id_for(task.id);
        assert!(
            env.store
                .integration_request_record(task.id, &next, "test")
                .unwrap()
        );
        let inbox = send(
            &app,
            get_admin("/api/v1/inbox/items?kind=integration_request"),
        )
        .await;
        let item_id = inbox.json()["items"][0]["id"].as_str().unwrap().to_owned();
        let answer = send(
            &app,
            post_admin(
                &format!("/api/v1/inbox/items/{item_id}/answer"),
                &json!({"option":option}),
            ),
        )
        .await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        assert_eq!(answer.json()["removed"], true);
        assert!(
            env.store
                .events_for(task.id)
                .unwrap()
                .iter()
                .any(|row| matches!(&row.1,
            Event::IntegrationAnswered { request_id, answer, note }
                if request_id == &next_id && answer == option && note.is_none()))
        );
    }
}

#[tokio::test]
async fn information_is_only_a_notice_and_read_reduces_count() {
    let env = admin_env();
    let now = OffsetDateTime::now_utc();
    let id = env
        .store
        .notice_record(&NoticeEvent {
            source_key: "task:done-1".into(),
            kind: NoticeKind::TaskDone,
            group_key: "task_done:project:test".into(),
            title: "finished".into(),
            summary: "finished".into(),
            project_id: Some("test".into()),
            task_id: None,
            target: None,
            links: vec![],
            at: now,
        })
        .unwrap()
        .notice_id();
    let app = env.router();
    let inbox = send(&app, get_admin("/api/v1/inbox/items")).await;
    assert_eq!(inbox.status, 200);
    assert_eq!(inbox.json()["counts"]["total"], 0);
    let notices = send(&app, get_admin("/api/v1/notifications?unread=true")).await;
    assert_eq!(notices.status, 200, "{}", notices.text());
    assert_eq!(notices.json()["items"][0]["id"], id.to_string());
    let count = send(&app, get_admin("/api/v1/notifications/unread-count")).await;
    assert_eq!(count.json()["unread"], 1);
    assert_eq!(count.json()["events"], 1);
    let read = send(
        &app,
        post_admin(&format!("/api/v1/notifications/{id}/read"), &json!({})),
    )
    .await;
    assert_eq!(read.status, 200, "{}", read.text());
    let count = send(&app, get_admin("/api/v1/notifications/unread-count")).await;
    assert_eq!(count.json()["unread"], 0);
    assert_eq!(count.json()["events"], 0);
    let again = send(
        &app,
        post_admin(&format!("/api/v1/notifications/{id}/read"), &json!({})),
    )
    .await;
    assert_eq!(again.status, 200);
    let legacy = send(&app, get_admin("/api/v1/reports")).await;
    assert_eq!(legacy.status, 200);
    assert!(legacy.json()["items"].is_array());
}

#[tokio::test]
async fn a_decision_answer_uses_the_domain_endpoint_and_stale_id_is_gone() {
    let env = admin_env();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    let decision = DecisionRequest {
        id: "decision-api-test".into(),
        key: "api-test".into(),
        kind: DecisionKind::Choice,
        question: "Which path?".into(),
        options: vec![DecisionOption {
            key: "first".into(),
            label: "First".into(),
            consequence: None,
        }],
        recommended: "first".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["self".into()],
        path: vec![DecisionPathEntry {
            task_id: task.id,
            title: task.title.clone(),
            stage: None,
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id: task.id,
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    env.store
        .append_event(
            task.id,
            &Event::DecisionRequested {
                decision: Box::new(decision),
            },
        )
        .unwrap();
    let app = env.router();
    let list = send(&app, get_admin("/api/v1/inbox/items")).await;
    let id = list.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["kind"] == "decision")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = send(
        &app,
        post_admin(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option":"first"}),
        ),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.json()["removed"], true);
    let old = send(&app, get_admin("/api/v1/decisions?open=true")).await;
    assert_eq!(old.status, 200);
    assert_eq!(old.json()["items"].as_array().unwrap().len(), 0);
    let stale = send(
        &app,
        post_admin(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option":"first"}),
        ),
    )
    .await;
    assert_problem(&stale, 404, "inbox-item-gone");
}

#[tokio::test]
async fn read_all_obeys_kind_and_project_filters() {
    let env = admin_env();
    let now = OffsetDateTime::now_utc();
    for (source, kind, project) in [
        ("one", NoticeKind::TaskDone, "a"),
        ("two", NoticeKind::TaskDone, "b"),
        ("three", NoticeKind::Report, "a"),
    ] {
        env.store
            .notice_record(&NoticeEvent {
                source_key: source.into(),
                kind,
                group_key: format!("{}:{project}", kind.as_str()),
                title: source.into(),
                summary: source.into(),
                project_id: Some(project.into()),
                task_id: None,
                target: None,
                links: vec![],
                at: now,
            })
            .unwrap();
    }
    let app = env.router();
    let response = send(
        &app,
        post_admin(
            "/api/v1/notifications/read-all",
            &json!({"kind":"task_done","project":"a"}),
        ),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.json()["marked"], 1);
    let count = send(&app, get_admin("/api/v1/notifications/unread-count")).await;
    assert_eq!(count.json()["unread"], 2);
}

#[tokio::test]
async fn legacy_report_read_also_marks_its_notice_read() {
    let env = admin_env();
    let at = OffsetDateTime::now_utc();
    let report = Report {
        id: ReportId::new(),
        project_id: None,
        node_id: "secretary".into(),
        task_id: None,
        kind: ReportKind::Result,
        level: 0,
        headline: "result".into(),
        body: "body".into(),
        sources: vec![],
        read_at: None,
        created_at: at,
    };
    env.store.report_append(&report).unwrap();
    env.store
        .notice_record(&NoticeEvent {
            source_key: format!("report:{}", report.id),
            kind: NoticeKind::Report,
            group_key: "report:secretary".into(),
            title: "result".into(),
            summary: "result".into(),
            project_id: None,
            task_id: None,
            target: Some(task_core::NoticeTarget {
                kind: "report".into(),
                id: report.id.to_string(),
            }),
            links: vec![],
            at,
        })
        .unwrap();
    let app = env.router();
    let old = send(
        &app,
        post_admin(
            "/api/v1/reports/read",
            &json!({"ids":[report.id.to_string()]}),
        ),
    )
    .await;
    assert_eq!(old.status, 200, "{}", old.text());
    assert_eq!(old.header("deprecation"), Some("true"));
    assert_eq!(old.json()["updated"], 1);
    let count = send(&app, get_admin("/api/v1/notifications/unread-count")).await;
    assert_eq!(count.json()["unread"], 0);
}

fn tree_child(parent: &task_core::Task, status: Status, unit_key: &str) -> task_core::Task {
    let mut child = new_task(TaskKind::Execute, status);
    child.parent_id = Some(parent.id);
    child.tree = Some(task_core::TreeInfo {
        root_id: parent.id,
        depth: 2,
        parent_unit: Some(task_core::ParentUnit {
            task_id: parent.id,
            plan_id: "plan".into(),
            unit_key: unit_key.into(),
            stage: "stage".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    child
}

/// ADR-0133 D4: items that lost their meaning are closed by the ADR-0131 inbox-rules
/// (`task_ops::inbox::attention_suppression`) and counted per rule in `suppressed`;
/// a failed task that still needs a human decision stays.
#[tokio::test]
async fn auto_close_drops_meaningless_items_and_keeps_failed_needing_a_decision() {
    let env = admin_env();
    // R1: the parent finished through another child, so this failed child was replaced.
    let done_parent = new_task(TaskKind::Execute, Status::Done);
    let replaced = tree_child(&done_parent, Status::Failed, "replaced");
    // R2: the ancestor was cancelled.
    let cancelled_parent = new_task(TaskKind::Execute, Status::Cancelled);
    let under_cancelled = tree_child(&cancelled_parent, Status::Failed, "withdrawn");
    // R3: a terminal task with a stale delivery_skipped attention.
    let skipped = Event::DeliverySkipped {
        reason: task_core::DeliverySkipReason::DepartmentUnresolved,
        detail: "needs attention".into(),
        head: Some("abc123".into()),
    };
    let mut terminal_done = new_task(TaskKind::Execute, Status::Done);
    terminal_done.project_id = Some(task_core::ProjectId::new());
    // Still needs a human: the parent has not finished.
    let open_parent = new_task(TaskKind::Execute, Status::Running);
    let needs_decision = tree_child(&open_parent, Status::Failed, "open");
    for t in [
        &done_parent,
        &replaced,
        &cancelled_parent,
        &under_cancelled,
        &open_parent,
        &needs_decision,
    ] {
        env.seed(t);
    }
    env.seed_with(&terminal_done, vec![skipped]);

    let app = env.router();
    let inbox = send(&app, get_admin("/api/v1/inbox/items")).await;
    assert_eq!(inbox.status, 200, "{}", inbox.text());
    let body = inbox.json();
    let task_ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["task"]["id"].as_str().map(str::to_owned))
        .collect();
    for gone in [&replaced, &under_cancelled, &terminal_done] {
        assert!(
            !task_ids.contains(&gone.id.to_string()),
            "{} should be auto-closed: {body}",
            gone.id
        );
    }
    let kept = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["task"]["id"] == needs_decision.id.to_string())
        .unwrap_or_else(|| panic!("failed child of an open parent stays: {body}"));
    assert_eq!(kept["kind"], "failed");
    assert_eq!(
        body["suppressed"],
        json!({
            "r1_parent_done": 1,
            "r2_ancestor_cancelled": 1,
            "r3_terminal_task": 1,
        }),
        "{body}"
    );
}
