use super::*;
use crate::org::{OrgKind, OrgNode};
use crate::store::TaskStore;

fn node(id: &str, parent: Option<&str>, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        name: id.to_string(),
        kind,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn store_with_org() -> SqliteStore {
    let store = SqliteStore::open_in_memory().expect("open");
    for n in [
        node("secretary", None, OrgKind::Secretary),
        node("coding", Some("secretary"), OrgKind::Department),
        node("coding-poc", Some("coding"), OrgKind::Section),
    ] {
        TaskStore::org_upsert(&store, &n).expect("seed org");
    }
    store
}

fn sample(
    node_id: &str,
    project: Option<ProjectId>,
    question: &str,
    at: OffsetDateTime,
) -> Approval {
    Approval {
        id: ApprovalId::new(),
        project_id: project,
        node_id: node_id.to_string(),
        task_id: Some(TaskId::new()),
        question: question.to_string(),
        decision: None,
        answer: None,
        created_at: at,
        decided_at: None,
    }
}

#[test]
fn decision_has_fixed_spellings() {
    assert_eq!(Decision::Once.as_str(), "once");
    assert_eq!(Decision::parse("standing"), Some(Decision::Standing));
    assert_eq!(Decision::parse("bogus"), None);
}

#[test]
fn append_and_get_round_trip_including_a_pending_row() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let project = ProjectId::new();
    let a = sample("coding-poc", Some(project), "どのクラスタを使いますか", now);
    store.approval_append(&a).expect("append");
    let back = store.approval_get(a.id).expect("get").expect("some");
    assert_eq!(back, a);
    assert!(back.is_pending());
    assert_eq!(store.approval_get(ApprovalId::new()).expect("get"), None);
}

#[test]
fn list_filters_by_pending_project_and_node_oldest_first() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let project = ProjectId::new();
    let other = ProjectId::new();
    let a = sample(
        "coding-poc",
        Some(project),
        "a",
        now - time::Duration::minutes(2),
    );
    let b = sample(
        "coding-poc",
        Some(project),
        "b",
        now - time::Duration::minutes(1),
    );
    let c = sample("secretary", Some(other), "c", now);
    store.approval_append(&a).expect("append");
    store.approval_append(&b).expect("append");
    store.approval_append(&c).expect("append");

    let all = store.approval_list(None, None, None).expect("list");
    assert_eq!(
        all.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![a.id, b.id, c.id],
        "oldest first"
    );

    let by_project = store
        .approval_list(None, Some(project), None)
        .expect("list");
    assert_eq!(by_project.len(), 2);
    let by_node = store
        .approval_list(None, None, Some("secretary"))
        .expect("list");
    assert_eq!(by_node.iter().map(|x| x.id).collect::<Vec<_>>(), vec![c.id]);

    store
        .approval_decide(a.id, Decision::Once, Some("pegasus".into()), now)
        .expect("decide")
        .expect("some");
    let pending = store.approval_list(Some(true), None, None).expect("list");
    assert_eq!(
        pending.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![b.id, c.id]
    );
    // R5（Phase 27）: `Some(false)` は**決定済みだけ**（以前は絞り込み無しと同じだった）。
    let decided = store.approval_list(Some(false), None, None).expect("list");
    assert_eq!(decided.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a.id]);
    assert!(decided.iter().all(|x| x.decision.is_some()));
    // 絞り込みは他の条件と AND で効く。
    assert!(
        store
            .approval_list(Some(false), None, Some("secretary"))
            .expect("list")
            .is_empty()
    );
    assert_eq!(
        store.approval_list(None, None, None).expect("list").len(),
        3
    );
}

#[test]
fn deciding_writes_the_decision_answer_and_decided_at_and_can_be_redone() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let a = sample("coding-poc", None, "a", now);
    store.approval_append(&a).expect("append");

    let decided = store
        .approval_decide(a.id, Decision::Denied, Some("だめです".into()), now)
        .expect("decide")
        .expect("some");
    assert_eq!(decided.decision, Some(Decision::Denied));
    assert_eq!(decided.answer.as_deref(), Some("だめです"));
    assert_eq!(decided.decided_at, Some(now));
    assert!(!decided.is_pending());

    // 無い id は None（何も書かない）。
    assert_eq!(
        store
            .approval_decide(ApprovalId::new(), Decision::Once, None, now)
            .expect("decide"),
        None
    );

    // 答え直せる（上書き）。
    let redone = store
        .approval_decide(a.id, Decision::Once, Some("やっぱりいいです".into()), now)
        .expect("decide")
        .expect("some");
    assert_eq!(redone.decision, Some(Decision::Once));
    assert_eq!(redone.answer.as_deref(), Some("やっぱりいいです"));
}

// ---- Phase F7: 認可元のタスクの終端で認可の要求を閉じる ----

fn f7_task(status: Status, kind: crate::model::TaskKind) -> crate::model::Task {
    use crate::model::{Budget, Tier, WorkerHint, WorkspaceSpec};
    let now = OffsetDateTime::now_utc();
    crate::model::Task {
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
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
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
        assignee: Some("coding-poc".into()),
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn pending_for(store: &SqliteStore, task_id: TaskId, question: &str) -> Approval {
    let mut a = sample("coding-poc", None, question, OffsetDateTime::now_utc());
    a.task_id = Some(task_id);
    store.approval_append(&a).expect("append");
    a
}

fn withdrawn_events(
    store: &SqliteStore,
    task_id: TaskId,
) -> Vec<(Vec<ApprovalId>, Status, String)> {
    store
        .events_for(task_id)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ApprovalsWithdrawn {
                approval_ids,
                task_status,
                reason,
            } => Some((approval_ids, task_status, reason)),
            _ => None,
        })
        .collect()
}

#[test]
fn withdrawn_has_a_fixed_spelling() {
    assert_eq!(Decision::Withdrawn.as_str(), "withdrawn");
    assert_eq!(Decision::parse("withdrawn"), Some(Decision::Withdrawn));
    assert_eq!(
        serde_json::to_string(&Decision::Withdrawn).expect("json"),
        "\"withdrawn\""
    );
}

#[test]
fn cancelling_a_task_withdraws_its_pending_approvals_in_the_same_transition() {
    use crate::transition::Trigger;
    let store = store_with_org();
    let task = f7_task(Status::Blocked, crate::model::TaskKind::Execute);
    store.insert(&task).expect("insert");
    let a = pending_for(&store, task.id, "どのクラスタを使いますか");
    let b = pending_for(&store, task.id, "予算を超えてよいですか");
    let decided = pending_for(&store, task.id, "前の質問");
    store
        .approval_decide(
            decided.id,
            Decision::Once,
            Some("はい".into()),
            OffsetDateTime::now_utc(),
        )
        .expect("decide");
    let other = f7_task(Status::Blocked, crate::model::TaskKind::Execute);
    store.insert(&other).expect("insert");
    let untouched = pending_for(&store, other.id, "別のタスクの質問");

    store
        .apply_transition(task.id, Trigger::Cancel, None)
        .expect("cancel");

    let pending = store.approval_list(Some(true), None, None).expect("list");
    assert_eq!(
        pending.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![untouched.id],
        "only the other task's approval stays pending"
    );
    for id in [a.id, b.id] {
        let back = store.approval_get(id).expect("get").expect("some");
        assert_eq!(back.decision, Some(Decision::Withdrawn));
        assert!(
            back.answer
                .as_deref()
                .is_some_and(|s| s.starts_with("task cancelled")),
            "answer: {:?}",
            back.answer
        );
        assert!(back.decided_at.is_some());
    }
    // 人が決めたものは書き換えない。
    let kept = store.approval_get(decided.id).expect("get").expect("some");
    assert_eq!(kept.decision, Some(Decision::Once));
    assert_eq!(kept.answer.as_deref(), Some("はい"));
    assert_eq!(
        withdrawn_events(&store, task.id),
        vec![(
            vec![a.id, b.id],
            Status::Cancelled,
            WITHDRAWN_BY_TRANSITION.to_string()
        )]
    );
    assert!(withdrawn_events(&store, other.id).is_empty());
}

#[test]
fn a_task_without_pending_approvals_gets_no_withdrawn_event() {
    use crate::transition::Trigger;
    let store = store_with_org();
    let task = f7_task(Status::Ready, crate::model::TaskKind::Execute);
    store.insert(&task).expect("insert");
    store
        .apply_transition(task.id, Trigger::Cancel, None)
        .expect("cancel");
    assert!(withdrawn_events(&store, task.id).is_empty());
}

#[test]
fn cascade_cancel_withdraws_the_approvals_of_dependents_and_approval_children() {
    use crate::model::TaskKind;
    use crate::transition::Trigger;
    let store = store_with_org();
    let parent = f7_task(Status::Ready, TaskKind::Execute);
    store.insert(&parent).expect("insert");
    // 後続（depends_on）: 親の取り消しで dependency_failed → cancelled。
    let mut dependent = f7_task(Status::Blocked, TaskKind::Execute);
    dependent.depends_on = vec![parent.id];
    store.insert(&dependent).expect("insert");
    // `kind = approval` の子: 親の終端で cancel（P-37）。
    let mut child = f7_task(Status::Ready, TaskKind::Approval);
    child.parent_id = Some(parent.id);
    store.insert(&child).expect("insert");
    let dep_a = pending_for(&store, dependent.id, "後続の質問");
    let child_a = pending_for(&store, child.id, "子の質問");

    store
        .apply_transition(parent.id, Trigger::Cancel, None)
        .expect("cancel");

    assert_eq!(
        store.get(dependent.id).expect("get").expect("some").status,
        Status::Cancelled
    );
    assert_eq!(
        store.get(child.id).expect("get").expect("some").status,
        Status::Cancelled
    );
    assert!(
        store
            .approval_list(Some(true), None, None)
            .expect("list")
            .is_empty()
    );
    assert_eq!(
        withdrawn_events(&store, dependent.id),
        vec![(
            vec![dep_a.id],
            Status::Cancelled,
            WITHDRAWN_BY_TRANSITION.to_string()
        )]
    );
    assert_eq!(
        withdrawn_events(&store, child.id),
        vec![(
            vec![child_a.id],
            Status::Cancelled,
            WITHDRAWN_BY_TRANSITION.to_string()
        )]
    );
}

#[test]
fn failing_withdraws_but_a_non_terminal_transition_does_not() {
    use crate::transition::Trigger;
    let store = store_with_org();
    let task = f7_task(Status::Blocked, crate::model::TaskKind::Execute);
    store.insert(&task).expect("insert");
    let a = pending_for(&store, task.id, "質問");
    // blocked → ready（答え）は終端ではないので、ストアは閉じない（`gate::answer` が `once` で閉じる）。
    store
        .apply_transition(task.id, Trigger::Answer, None)
        .expect("answer");
    assert!(
        store
            .approval_get(a.id)
            .expect("get")
            .expect("some")
            .is_pending()
    );
    assert!(withdrawn_events(&store, task.id).is_empty());

    // running → failed（やり直せない失敗）でも閉じる。
    let running = f7_task(Status::Running, crate::model::TaskKind::Execute);
    store.insert(&running).expect("insert");
    let b = pending_for(&store, running.id, "質問");
    store
        .apply_transition(running.id, Trigger::WorkerError { retryable: false }, None)
        .expect("fail");
    assert_eq!(
        store.get(running.id).expect("get").expect("some").status,
        Status::Failed
    );
    let back = store.approval_get(b.id).expect("get").expect("some");
    assert_eq!(back.decision, Some(Decision::Withdrawn));
    assert!(
        back.answer
            .as_deref()
            .is_some_and(|s| s.starts_with("task failed"))
    );
    assert_eq!(
        withdrawn_events(&store, running.id),
        vec![(
            vec![b.id],
            Status::Failed,
            WITHDRAWN_BY_TRANSITION.to_string()
        )]
    );
}

#[test]
fn withdraw_stale_closes_only_approvals_of_terminal_tasks_and_is_idempotent() {
    use crate::model::TaskKind;
    let store = store_with_org();
    // F7 より前の残り: タスクは既に終端（insert で直接その状態にする）なのに未決の行がある。
    let done = f7_task(Status::Done, TaskKind::Execute);
    let failed = f7_task(Status::Failed, TaskKind::Execute);
    let cancelled = f7_task(Status::Cancelled, TaskKind::Execute);
    let live = f7_task(Status::Blocked, TaskKind::Execute);
    for t in [&done, &failed, &cancelled, &live] {
        store.insert(t).expect("insert");
    }
    let a_done = pending_for(&store, done.id, "q");
    let a_failed = pending_for(&store, failed.id, "q");
    let a_cancelled = pending_for(&store, cancelled.id, "q");
    let a_live = pending_for(&store, live.id, "q");
    // タスクの無い approval は対象外。
    let mut orphan = sample(
        "secretary",
        None,
        "タスクの無い質問",
        OffsetDateTime::now_utc(),
    );
    orphan.task_id = None;
    store.approval_append(&orphan).expect("append");

    let now = OffsetDateTime::now_utc();
    let mut out = store.approval_withdraw_stale(now).expect("withdraw");
    out.sort_by_key(|w| w.task_id);
    let mut expected = vec![
        WithdrawnApprovals {
            task_id: done.id,
            task_status: Status::Done,
            approval_ids: vec![a_done.id],
        },
        WithdrawnApprovals {
            task_id: failed.id,
            task_status: Status::Failed,
            approval_ids: vec![a_failed.id],
        },
        WithdrawnApprovals {
            task_id: cancelled.id,
            task_status: Status::Cancelled,
            approval_ids: vec![a_cancelled.id],
        },
    ];
    expected.sort_by_key(|w| w.task_id);
    assert_eq!(out, expected);

    let pending = store.approval_list(Some(true), None, None).expect("list");
    assert_eq!(
        pending.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![a_live.id, orphan.id]
    );
    let back = store.approval_get(a_failed.id).expect("get").expect("some");
    assert_eq!(back.decision, Some(Decision::Withdrawn));
    assert!(
        back.answer
            .as_deref()
            .is_some_and(|s| s.starts_with("task failed"))
    );
    assert_eq!(back.decided_at, Some(now));
    assert_eq!(
        withdrawn_events(&store, done.id),
        vec![(
            vec![a_done.id],
            Status::Done,
            WITHDRAWN_BY_RECONCILE.to_string()
        )]
    );
    // 2 回目は何もしない（イベントも増えない）。
    assert!(
        store
            .approval_withdraw_stale(now)
            .expect("again")
            .is_empty()
    );
    assert_eq!(withdrawn_events(&store, done.id).len(), 1);
}

#[test]
fn standing_rules_round_trip_and_list_combines_global_and_node_specific() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let global = StandingRule {
        id: StandingRuleId::new(),
        node_id: None,
        rule: "深夜は連絡しない".into(),
        created_at: now - time::Duration::minutes(2),
    };
    let for_coding = StandingRule {
        id: StandingRuleId::new(),
        node_id: Some("coding-poc".into()),
        rule: "pegasus は 1 ノードで始めてよい".into(),
        created_at: now - time::Duration::minutes(1),
    };
    let for_secretary = StandingRule {
        id: StandingRuleId::new(),
        node_id: Some("secretary".into()),
        rule: "秘書だけの規則".into(),
        created_at: now,
    };
    store.standing_rule_append(&global).expect("append");
    store.standing_rule_append(&for_coding).expect("append");
    store.standing_rule_append(&for_secretary).expect("append");

    // 絞り込み無し = 全件。
    let all = store.standing_rule_list(None).expect("list");
    assert_eq!(all.len(), 3);

    // そのノード向け: 全員向け + そのノードの分だけ（他ノードの分は出ない）。
    let for_coding_view = store.standing_rule_list(Some("coding-poc")).expect("list");
    assert_eq!(
        for_coding_view.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![global.id, for_coding.id],
        "oldest first"
    );

    assert!(store.standing_rule_delete(for_coding.id).expect("delete"));
    assert!(
        !store.standing_rule_delete(for_coding.id).expect("delete"),
        "already gone"
    );
    let after = store.standing_rule_list(Some("coding-poc")).expect("list");
    assert_eq!(
        after.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![global.id]
    );
}
