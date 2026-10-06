use super::*;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, SqliteStore, Task, Tier, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn sample_task(kind: TaskKind, status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind,
        title: "do something".to_string(),
        objective: "make it work".to_string(),
        acceptance: vec![Criterion {
            text: "tests pass".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![ArtifactRef {
            name: "spec".to_string(),
            path: "spec.md".to_string(),
            sha256: "abc".to_string(),
            kind: "doc".to_string(),
            declared: true,
        }],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "/tmp/workspace".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

#[test]
fn approve_moves_draft_to_ready() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&task).expect("insert");

    let result = approve(&store, task.id, None, None).expect("approve");
    assert_eq!(result.from, Status::Draft);
    assert_eq!(result.to, Status::Ready);

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Ready);
}

#[test]
fn approve_on_approval_ready_moves_to_done_and_records_event() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Approval, Status::Ready);
    store.insert(&task).expect("insert");

    approve(&store, task.id, Some("looks good".to_string()), None).expect("approve");

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Done);

    let events = store.events_for(task.id).expect("events_for");
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::ApprovalDecided { approved: true, .. }))
    );
}

#[test]
fn approve_with_matching_expected_succeeds() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&task).expect("insert");

    let result = approve(&store, task.id, None, Some(Status::Draft)).expect("approve");
    assert_eq!(result.to, Status::Ready);
}

#[test]
fn approve_with_mismatched_expected_returns_conflict_without_transitioning() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&task).expect("insert");

    let result = approve(&store, task.id, None, Some(Status::Ready));
    assert!(matches!(
        result,
        Err(OpsError::Conflict {
            expected: Status::Ready,
            actual: Status::Draft
        })
    ));

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(
        fetched.status,
        Status::Draft,
        "no transition should have happened"
    );
}

#[test]
fn reject_on_approval_ready_moves_to_failed_and_records_event() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Approval, Status::Ready);
    store.insert(&task).expect("insert");

    reject(&store, task.id, Some("not good enough".to_string()), None).expect("reject");

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Failed);

    let events = store.events_for(task.id).expect("events_for");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::ApprovalDecided {
            approved: false,
            ..
        }
    )));
}

#[test]
fn reject_on_draft_is_an_error_and_does_not_change_status() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&task).expect("insert");

    let result = reject(&store, task.id, None, None);
    assert!(result.is_err());

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Draft);
}

#[test]
fn reject_with_mismatched_expected_returns_conflict() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Approval, Status::Ready);
    store.insert(&task).expect("insert");

    let result = reject(&store, task.id, None, Some(Status::Draft));
    assert!(matches!(
        result,
        Err(OpsError::Conflict {
            expected: Status::Draft,
            actual: Status::Ready
        })
    ));
}

#[test]
fn answer_on_blocked_moves_to_ready() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&task).expect("insert");

    let result = answer(&store, task.id, "the answer".to_string(), None).expect("answer");
    assert_eq!(result.to, Status::Ready);

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Ready);
}

/// GUI 監査 H2（Phase 29）: `answer` は、そのタスクの未決の `approvals` を `once` + 同じ答えで
/// 決定済みにする（`GET /approvals?pending=true` からそのタスクの行が消える）。
/// 他のタスクの未決の approvals は触らない。approvals が無ければ何もしない（エラーにならない）。
#[test]
fn answer_settles_the_tasks_pending_approvals_as_once_with_the_same_answer() {
    use task_core::approval::{Approval, ApprovalId, ApprovalStore};

    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&task).expect("insert");
    let other = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&other).expect("insert other");

    let now = OffsetDateTime::now_utc();
    let mine = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "secretary".into(),
        task_id: Some(task.id),
        question: "どのクラスタを使いますか".into(),
        decision: None,
        answer: None,
        created_at: now,
        decided_at: None,
    };
    let unrelated = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "secretary".into(),
        task_id: Some(other.id),
        question: "別の質問".into(),
        decision: None,
        answer: None,
        created_at: now,
        decided_at: None,
    };
    store.approval_append(&mine).expect("append mine");
    store.approval_append(&unrelated).expect("append unrelated");

    answer(&store, task.id, "pegasus".to_string(), None).expect("answer");

    let decided = store.approval_get(mine.id).expect("get").expect("some");
    assert_eq!(decided.decision, Some(Decision::Once));
    assert_eq!(decided.answer.as_deref(), Some("pegasus"));
    assert!(decided.decided_at.is_some());

    // 他のタスクの approval は手つかず。
    let untouched = store
        .approval_get(unrelated.id)
        .expect("get")
        .expect("some");
    assert!(untouched.is_pending());

    // approvals が無いタスクへの answer はエラーにならない。
    let plain = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&plain).expect("insert plain");
    assert!(answer(&store, plain.id, "x".to_string(), None).is_ok());
}

/// ADR-0010 D3（P-10）: `answer` は `Event::Answered{question, answer}` を
/// `Trigger::Answer` の `Event::Transitioned` と同一トランザクションで、その直後に
/// 追記する。`question` は直近の `WorkerFinished{outcome:"question: ..."}` から取る。
#[test]
fn answer_persists_answered_event_with_question_from_worker_finished() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&task).expect("insert");
    store
        .append_event(
            task.id,
            &Event::WorkerFinished {
                run_id: "run-1".to_string(),
                outcome: "question: which version?".to_string(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("append worker finished");

    answer(&store, task.id, "use v2".to_string(), None).expect("answer");

    let events = store.events_for(task.id).expect("events_for");
    let tail = &events[events.len() - 2..];
    assert!(matches!(
        &tail[0].1,
        Event::Transitioned { reason, .. } if reason == "answer"
    ));
    assert_eq!(
        tail[1].1,
        Event::Answered {
            question: "which version?".to_string(),
            answer: "use v2".to_string(),
        }
    );
}

/// `WorkerFinished` が無ければ `question` は空文字列になる。
#[test]
fn answer_without_worker_finished_uses_empty_question() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&task).expect("insert");

    answer(&store, task.id, "the answer".to_string(), None).expect("answer");

    let events = store.events_for(task.id).expect("events_for");
    let last = &events.last().expect("some event").1;
    assert_eq!(
        *last,
        Event::Answered {
            question: String::new(),
            answer: "the answer".to_string(),
        }
    );
}

#[test]
fn answer_on_ready_task_is_an_error() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).expect("insert");

    let result = answer(&store, task.id, "irrelevant".to_string(), None);
    assert!(result.is_err());
}

#[test]
fn cancels_draft_task() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&task).expect("insert");

    let result = cancel(&store, task.id, None).expect("cancel");
    assert_eq!(result.to, Status::Cancelled);

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Cancelled);
}

#[test]
fn cancels_ready_task() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).expect("insert");

    cancel(&store, task.id, None).expect("cancel");

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Cancelled);
}

#[test]
fn cancels_running_task_acquired_via_lease() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).expect("insert");
    let acquired = store
        .acquire_lease(task.id, "run-1", std::time::Duration::from_secs(60))
        .expect("acquire_lease");
    assert!(acquired);

    cancel(&store, task.id, None).expect("cancel");

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Cancelled);
    assert!(fetched.lease.is_none());
}

#[test]
fn cancel_on_done_or_cancelled_task_errors_and_leaves_status_unchanged() {
    let store = SqliteStore::open_in_memory().expect("open store");
    for status in [Status::Done, Status::Cancelled] {
        let task = sample_task(TaskKind::Execute, status);
        store.insert(&task).expect("insert");

        let result = cancel(&store, task.id, None);
        assert!(result.is_err(), "cancel of {status:?} should fail");

        let fetched = store.get(task.id).expect("get").expect("some");
        assert_eq!(fetched.status, status);
    }
}

#[test]
fn inbox_cleanup_cancel_failed_is_available_to_human() {
    let store = SqliteStore::open_in_memory().expect("store");
    let parent = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&parent).expect("parent");
    let mut task = sample_task(TaskKind::Execute, Status::Failed);
    task.parent_id = Some(parent.id);
    assert!(crate::view::actions(&task).contains(&crate::view::Action::Cancel));
    store.insert(&task).expect("insert");
    let result = cancel(&store, task.id, Some(Status::Failed)).expect("cancel failed");
    assert_eq!(result.to, Status::Cancelled);
    assert_eq!(result.reason, "cancel_failed");
    assert_eq!(
        store
            .get(parent.id)
            .expect("get parent")
            .expect("parent")
            .status,
        Status::Blocked
    );
}

#[test]
fn cancel_on_missing_task_errors() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let missing_id = TaskId::new();

    let result = cancel(&store, missing_id, None);
    assert!(matches!(result, Err(OpsError::NotFound(id)) if id == missing_id));
}

#[test]
fn cancel_with_mismatched_expected_returns_conflict_without_transitioning() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).expect("insert");

    let result = cancel(&store, task.id, Some(Status::Running));
    assert!(matches!(
        result,
        Err(OpsError::Conflict {
            expected: Status::Running,
            actual: Status::Ready
        })
    ));

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(
        fetched.status,
        Status::Ready,
        "no transition should have happened"
    );
}

// ---- cascaded (docs/api/v1/gui-api.md §5.7) ----

#[test]
fn reject_approval_cascades_cancel_to_its_own_children() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let approval = sample_task(TaskKind::Approval, Status::Ready);
    store.insert(&approval).expect("insert approval");

    let mut child = sample_task(TaskKind::Execute, Status::Draft);
    child.parent_id = Some(approval.id);
    store.insert(&child).expect("insert child");

    let result = reject(&store, approval.id, None, None).expect("reject");
    assert_eq!(result.to, Status::Failed);
    assert_eq!(result.cascaded.len(), 1);
    assert_eq!(result.cascaded[0].id, child.id);
    assert_eq!(result.cascaded[0].status, Status::Cancelled);

    let fetched_child = store.get(child.id).expect("get").expect("some");
    assert_eq!(fetched_child.status, Status::Cancelled);
}

#[test]
fn cancel_cascades_dependency_failed_to_dependents() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let upstream = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&upstream).expect("insert upstream");

    let mut downstream = sample_task(TaskKind::Execute, Status::Draft);
    downstream.depends_on = vec![upstream.id];
    store.insert(&downstream).expect("insert downstream");

    let result = cancel(&store, upstream.id, None).expect("cancel");
    assert_eq!(result.to, Status::Cancelled);
    assert_eq!(result.cascaded.len(), 1);
    assert_eq!(result.cascaded[0].id, downstream.id);

    let fetched_downstream = store.get(downstream.id).expect("get").expect("some");
    assert_eq!(fetched_downstream.status, Status::Cancelled);
}

#[test]
fn cancel_without_children_or_dependents_has_empty_cascaded() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).expect("insert");

    let result = cancel(&store, task.id, None).expect("cancel");
    assert!(result.cascaded.is_empty());
}

#[test]
fn approve_without_cascading_effects_has_empty_cascaded() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&task).expect("insert");

    let result = approve(&store, task.id, None, None).expect("approve");
    assert!(result.cascaded.is_empty());
}

/// ADR-0079 D13（Phase R5a）: 案件計画の提案の一括決定（`POST /projects/{id}/project-plan/{version}/decide`）は
/// 410 になったので、未決の提案（途中目標が `proposed`）に属する draft も個別に `accept` できる（取り残さない）。
#[test]
fn a_draft_of_a_frozen_project_plan_proposal_can_be_accepted_individually() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let now = OffsetDateTime::now_utc();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        id: task_core::ProjectId::new(),
        title: "t".into(),
        request: "r".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("create project");
    let milestone = store
        .milestone_create(
            project.id,
            "survey",
            "",
            task_core::MilestoneStatus::Proposed,
        )
        .expect("create milestone");

    let mut task = sample_task(TaskKind::Execute, Status::Draft);
    task.project_id = Some(project.id);
    task.milestone_id = Some(milestone.id);
    store.insert(&task).expect("insert");

    let result = accept(&store, task.id, None).expect("accept");
    assert_eq!(result.to, Status::Ready);
}
