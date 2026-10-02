use super::*;
use task_core::{
    Budget, Check, Criterion, SqliteStore, Task, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn base_task(status: Status, acceptance: Vec<Criterion>) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: format!("task {id}"),
        objective: "do it".into(),
        acceptance,
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: id.to_string().into(),
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
        labels: Vec::new(),
        category: Default::default(),
        conversation: None,
    }
}

#[test]
fn lints_draft_and_ready_tasks_but_not_other_statuses() {
    let store = SqliteStore::open_in_memory().expect("open");
    let bad_draft = base_task(
        Status::Draft,
        vec![Criterion {
            text: "a human looks".into(),
            check: Check::Human,
        }],
    );
    let bad_ready = base_task(
        Status::Ready,
        vec![Criterion {
            text: "a human looks".into(),
            check: Check::Human,
        }],
    );
    let ok_task = base_task(
        Status::Draft,
        vec![
            Criterion {
                text: "a human looks".into(),
                check: Check::Human,
            },
            Criterion {
                text: "artifact exists".into(),
                check: Check::ArtifactExists {
                    name: "result.md".into(),
                },
            },
        ],
    );
    // done でも同じ違反を持つが、対象外（draft/ready だけ）。
    let done_bad = base_task(
        Status::Done,
        vec![Criterion {
            text: "a human looks".into(),
            check: Check::Human,
        }],
    );
    for t in [&bad_draft, &bad_ready, &ok_task, &done_bad] {
        store.insert(t).expect("insert");
    }

    let violations = lint(&store).expect("lint");
    let ids: Vec<TaskId> = violations.iter().map(|v| v.task_id).collect();
    assert!(ids.contains(&bad_draft.id));
    assert!(ids.contains(&bad_ready.id));
    assert!(!ids.contains(&ok_task.id));
    assert!(!ids.contains(&done_bad.id));
    assert_eq!(violations.len(), 2);
    for v in &violations {
        assert!(v.reason.contains("人が確認する成果物"));
    }
}

#[test]
fn no_violations_when_every_human_check_has_a_deliverable() {
    let store = SqliteStore::open_in_memory().expect("open");
    let ok_task = base_task(
        Status::Ready,
        vec![
            Criterion {
                text: "a human looks".into(),
                check: Check::Human,
            },
            Criterion {
                text: "kb page".into(),
                check: Check::KnowledgePage {
                    path: "projects/x/decision.md".into(),
                },
            },
        ],
    );
    store.insert(&ok_task).expect("insert");
    assert!(lint(&store).expect("lint").is_empty());
}
