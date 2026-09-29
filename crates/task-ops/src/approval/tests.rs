use super::*;
use task_core::approval::ApprovalStore;
use task_core::org::{OrgKind, OrgNode};
use task_core::{
    Budget, Check, Criterion, SqliteStore, Status, Task, TaskId, TaskKind, Tier, WorkerHint,
    WorkspaceSpec,
};

fn node(id: &str, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: None,
        name: id.into(),
        kind,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

/// `Blocked` のタスクと、それを指す `Approval` を作る（`gate::answer` が再開できる状態）。
fn blocked_task_with_approval(store: &SqliteStore, node_id: &str) -> (Task, Approval) {
    let now = OffsetDateTime::now_utc();
    let id = TaskId::new();
    let task = Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Blocked,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "ws".into(),
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
        project_id: None,
        milestone_id: None,
        assignee: Some(node_id.to_string()),
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    };
    store.create_task(&task, vec![]).expect("create");
    let approval = Approval {
        id: task_core::approval::ApprovalId::new(),
        project_id: None,
        node_id: node_id.to_string(),
        task_id: Some(id),
        question: "どのクラスタを使いますか".into(),
        decision: None,
        answer: None,
        created_at: now,
        decided_at: None,
    };
    store.approval_append(&approval).expect("append");
    (store.get(id).expect("get").expect("task"), approval)
}

#[test]
fn once_answers_the_task_and_leaves_no_standing_rule() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("coding-poc", OrgKind::Secretary))
        .expect("seed");
    let (task, approval) = blocked_task_with_approval(&store, "coding-poc");

    let outcome = decide(
        &store,
        approval.clone(),
        Decision::Once,
        "pegasus".into(),
        Scope::Node,
        OffsetDateTime::now_utc(),
    )
    .expect("decide");
    assert_eq!(outcome.approval.decision, Some(Decision::Once));
    assert_eq!(outcome.approval.answer.as_deref(), Some("pegasus"));
    assert!(outcome.standing_rule.is_none());
    let transition = outcome.transition.expect("transition");
    assert_eq!(
        transition.to,
        Status::Ready,
        "既存の答える経路（blocked → ready）で再開する"
    );
    assert_eq!(
        store.get(task.id).expect("get").expect("task").status,
        Status::Ready
    );
}

#[test]
fn standing_records_a_rule_scoped_to_the_node_by_default() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("coding-poc", OrgKind::Secretary))
        .expect("seed");
    let (_, approval) = blocked_task_with_approval(&store, "coding-poc");

    let outcome = decide(
        &store,
        approval,
        Decision::Standing,
        "pegasus のジョブは 1 ノードで始めてよい".into(),
        Scope::Node,
        OffsetDateTime::now_utc(),
    )
    .expect("decide");
    let rule = outcome.standing_rule.expect("rule");
    assert_eq!(rule.node_id.as_deref(), Some("coding-poc"));
    assert_eq!(rule.rule, "pegasus のジョブは 1 ノードで始めてよい");
    assert_eq!(
        store.standing_rule_list(Some("coding-poc")).expect("list"),
        vec![rule]
    );
}

#[test]
fn standing_with_scope_all_applies_to_everyone() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("coding-poc", OrgKind::Secretary))
        .expect("seed");
    let (_, approval) = blocked_task_with_approval(&store, "coding-poc");

    let outcome = decide(
        &store,
        approval,
        Decision::Standing,
        "深夜は連絡しない".into(),
        Scope::All,
        OffsetDateTime::now_utc(),
    )
    .expect("decide");
    assert_eq!(outcome.standing_rule.expect("rule").node_id, None);
    // 全員向けなので他ノードの一覧にも出る。
    assert_eq!(
        store
            .standing_rule_list(Some("someone-else"))
            .expect("list")
            .len(),
        1
    );
}

#[test]
fn denied_prefixes_the_answer_so_the_worker_can_tell() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("coding-poc", OrgKind::Secretary))
        .expect("seed");
    let (task, approval) = blocked_task_with_approval(&store, "coding-poc");

    let outcome = decide(
        &store,
        approval,
        Decision::Denied,
        "予算超過".into(),
        Scope::Node,
        OffsetDateTime::now_utc(),
    )
    .expect("decide");
    assert_eq!(outcome.approval.decision, Some(Decision::Denied));
    assert!(outcome.standing_rule.is_none());
    let events = store.events_for(task.id).expect("events");
    let answered = events.iter().rev().find_map(|(_, e)| match e {
        task_core::Event::Answered { answer, .. } => Some(answer.clone()),
        _ => None,
    });
    assert_eq!(answered.as_deref(), Some("認めない: 予算超過"));
}

#[test]
fn a_blank_answer_is_rejected_without_writing_anything() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("coding-poc", OrgKind::Secretary))
        .expect("seed");
    let (_, approval) = blocked_task_with_approval(&store, "coding-poc");
    assert!(matches!(
        decide(
            &store,
            approval.clone(),
            Decision::Once,
            "   ".into(),
            Scope::Node,
            OffsetDateTime::now_utc()
        ),
        Err(OpsError::Validation(_))
    ));
    assert!(
        store
            .approval_get(approval.id)
            .expect("get")
            .expect("some")
            .is_pending()
    );
}

#[test]
fn an_approval_without_a_task_id_only_records_the_decision() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("secretary", OrgKind::Secretary))
        .expect("seed");
    let approval = Approval {
        id: task_core::approval::ApprovalId::new(),
        project_id: None,
        node_id: "secretary".into(),
        task_id: None,
        question: "クラスタの障害、続報を待ちますか".into(),
        decision: None,
        answer: None,
        created_at: OffsetDateTime::now_utc(),
        decided_at: None,
    };
    store.approval_append(&approval).expect("append");
    let outcome = decide(
        &store,
        approval,
        Decision::Once,
        "待つ".into(),
        Scope::Node,
        OffsetDateTime::now_utc(),
    )
    .expect("decide");
    assert!(outcome.transition.is_none());
    assert_eq!(outcome.approval.answer.as_deref(), Some("待つ"));
}
