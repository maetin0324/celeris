use super::*;
use std::path::PathBuf;
use task_core::approval::ApprovalStore;
use task_core::org::{OrgKind as OK, OrgNode};
use task_core::{
    Budget, ProjectId, SqliteStore, Status, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
};

fn node(id: &str, parent: Option<&str>, kind: OK) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: id.into(),
        kind,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn task(assignee: Option<&str>, project: Option<ProjectId>) -> Task {
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
        kind: TaskKind::Execute,
        title: "調べる".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Running,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Cheap,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("."),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 0,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: project,
        milestone_id: None,
        assignee: assignee.map(str::to_string),
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

#[test]
fn a_question_with_an_assignee_goes_straight_to_that_node() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("secretary", None, OrgKind::Secretary))
        .expect("seed");
    store
        .org_upsert(&node("coding-poc", Some("secretary"), OrgKind::Section))
        .expect("seed");
    let project = ProjectId::new();
    let t = task(Some("coding-poc"), Some(project));
    let now = OffsetDateTime::now_utc();

    let approval = record_question_approval(&store, &t, "どのクラスタを使いますか", now)
        .expect("record")
        .expect("some");
    assert_eq!(approval.node_id, "coding-poc");
    assert_eq!(approval.task_id, Some(t.id));
    assert_eq!(approval.project_id, Some(project));
    assert_eq!(approval.question, "どのクラスタを使いますか");
    assert!(approval.is_pending());

    let listed = store.approval_list(Some(true), None, None).expect("list");
    assert_eq!(listed, vec![approval]);
}

#[test]
fn a_question_without_an_assignee_falls_back_to_the_secretary() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("secretary", None, OrgKind::Secretary))
        .expect("seed");
    let t = task(None, None);
    let approval = record_question_approval(&store, &t, "続けますか", OffsetDateTime::now_utc())
        .expect("record")
        .expect("some");
    assert_eq!(approval.node_id, "secretary");
}

/// Phase F7: 質問の遷移と追記の間に人が取り消していたら、要求を作らない。
#[test]
fn a_question_of_a_task_that_is_already_terminal_is_not_recorded() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("secretary", None, OrgKind::Secretary))
        .expect("seed");
    let mut t = task(None, None);
    t.status = Status::Cancelled;
    store.insert(&t).expect("insert");
    assert_eq!(
        record_question_approval(&store, &t, "続けますか", OffsetDateTime::now_utc())
            .expect("record"),
        None
    );
    assert!(
        store
            .approval_list(None, None, None)
            .expect("list")
            .is_empty()
    );
}

/// Phase F7（tick の照合）: 終端のタスクに未決で残った要求を閉じ、生きているタスクの分は残す。
#[test]
fn withdraw_stale_approvals_closes_the_backlog_of_terminal_tasks() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("secretary", None, OrgKind::Secretary))
        .expect("seed");
    let mut live = task(None, None);
    live.status = Status::Blocked;
    store.insert(&live).expect("insert");
    // F7 より前の DB を再現する: タスクは既に終端なのに未決の行がある。
    let mut gone = task(None, None);
    gone.status = Status::Cancelled;
    store.insert(&gone).expect("insert");
    let now = OffsetDateTime::now_utc();
    let keep = record_question_approval(&store, &live, "a", now)
        .expect("record")
        .expect("some");
    for q in ["b", "c"] {
        store
            .approval_append(&Approval {
                id: ApprovalId::new(),
                project_id: None,
                node_id: "secretary".into(),
                task_id: Some(gone.id),
                question: q.into(),
                decision: None,
                answer: None,
                created_at: now,
                decided_at: None,
            })
            .expect("append");
    }

    assert_eq!(withdraw_stale_approvals(&store, now).expect("withdraw"), 2);
    let pending = store.approval_list(Some(true), None, None).expect("list");
    assert_eq!(
        pending.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![keep.id]
    );
    assert_eq!(withdraw_stale_approvals(&store, now).expect("again"), 0);
}

#[test]
fn without_an_organization_nothing_is_recorded() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task(None, None);
    assert_eq!(
        record_question_approval(&store, &t, "続けますか", OffsetDateTime::now_utc())
            .expect("record"),
        None
    );
    assert!(
        store
            .approval_list(None, None, None)
            .expect("list")
            .is_empty()
    );
}
