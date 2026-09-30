use super::*;
use crate::execution_plan::{WorkUnitBlockedReason, WorkUnitKind, WorkUnitStatus};
use crate::model::Status;

fn facts() -> PlanApprovalFacts {
    PlanApprovalFacts {
        stages: 2,
        max_units_in_stage: 2,
        child_task_units: 1,
        estimated_leaves: 6,
        estimated_runs: 8,
        ..Default::default()
    }
}

/// D8: 決定も `review: human` も無く 0.8 未満なら承認は要らない。
#[test]
fn small_plan_needs_no_approval() {
    let a = plan_approval(&facts(), &TreeLimits::default());
    assert!(!a.required);
    assert!(a.reasons.is_empty());
}

/// D8: 3 つの条件はそれぞれ単独で承認を要する（理由の文字列は決定的）。
#[test]
fn each_trigger_requires_approval() {
    // R6-2: 既定の木の上限は leaf 120 / run 400。この表は R3b のときの 40 / 120 で書いてある。
    let limits = TreeLimits {
        max_tree_leaves: 40,
        max_tree_runs: 120,
        ..TreeLimits::default()
    };
    let mut f = facts();
    f.open_decisions = vec!["h1".into(), "h2".into()];
    assert_eq!(
        plan_approval(&f, &limits).reasons,
        vec!["decisions:h1,h2".to_string()]
    );
    let mut f = facts();
    f.review_human_stages = vec!["phase-2".into()];
    assert_eq!(
        plan_approval(&f, &limits).reasons,
        vec!["review_human:phase-2".to_string()]
    );
    // 5 段階の上限の 0.8 = 4 段階から。
    let mut f = facts();
    f.stages = 4;
    assert_eq!(
        plan_approval(&f, &limits).reasons,
        vec!["near_limit:max_stages:4/5".to_string()]
    );
    f.stages = 3;
    assert!(!plan_approval(&f, &limits).required);
    // leaf の見込み 32/40、run の見込み 96/120。
    let mut f = facts();
    f.estimated_leaves = 32;
    f.estimated_runs = 96;
    assert_eq!(
        plan_approval(&f, &limits).reasons,
        vec![
            "near_limit:max_tree_leaves:32/40".to_string(),
            "near_limit:max_tree_runs:96/120".to_string()
        ]
    );
    assert!(near_limit(5, 6, 800));
    assert!(!near_limit(4, 6, 800));
    assert!(!near_limit(3, 0, 800));
}

fn node(units: Vec<LivenessUnitFacts>) -> NodeLivenessFacts {
    NodeLivenessFacts {
        task_id: TaskId::new(),
        status: Status::Ready,
        last_reason: Some("planned".into()),
        leased: false,
        eligible: true,
        has_plan: true,
        planner_pending: false,
        open_self_decision: false,
        tree_limit_decision_open: false,
        replans_left: true,
        plan_reviewed: false,
        units,
    }
}

fn leaf(key: &str, status: WorkUnitStatus) -> LivenessUnitFacts {
    LivenessUnitFacts {
        key: key.into(),
        kind: WorkUnitKind::Implement,
        status,
        blocked_reason: None,
        child: None,
        waits_on_open_decision: false,
        waits_on_child_dep: false,
    }
}

fn class(n: NodeLivenessFacts) -> (LivenessClass, String) {
    let out = liveness(&TreeSnapshot { nodes: vec![n] });
    (out[0].class, out[0].reason.clone())
}

/// D10: 名指しの待ち（決定・承認・途中確認・基盤・子）は理由なしにしない。
#[test]
fn named_waits_are_not_stalls() {
    let mut held = leaf("a", WorkUnitStatus::Blocked);
    held.blocked_reason = Some(WorkUnitBlockedReason::Decision);
    held.waits_on_open_decision = true;
    let pending = leaf("b", WorkUnitStatus::Pending);
    assert_eq!(
        class(node(vec![held.clone(), pending.clone()])),
        (LivenessClass::Waiting, "decision".to_string())
    );
    let mut infra = leaf("c", WorkUnitStatus::Blocked);
    infra.kind = WorkUnitKind::Task;
    infra.blocked_reason = Some(WorkUnitBlockedReason::Infra);
    assert_eq!(
        class(node(vec![infra])),
        (LivenessClass::Waiting, "infra".to_string())
    );
    let child = TaskId::new();
    let mut waiting_child = leaf("d", WorkUnitStatus::Running);
    waiting_child.kind = WorkUnitKind::Task;
    waiting_child.child = Some((child, Some(Status::Running)));
    assert_eq!(
        class(node(vec![waiting_child, pending.clone()])),
        (LivenessClass::Waiting, "children".to_string())
    );
    let mut approval = node(vec![pending.clone()]);
    approval.status = Status::Blocked;
    approval.last_reason = Some("awaiting_plan_approval".into());
    assert_eq!(
        class(approval),
        (LivenessClass::Waiting, "plan_approval".to_string())
    );
    let mut pause = node(vec![pending.clone()]);
    pause.status = Status::Blocked;
    pause.last_reason = Some("awaiting_human".into());
    assert_eq!(
        class(pause),
        (LivenessClass::Waiting, "pause_point".to_string())
    );
    let mut self_held = node(vec![pending.clone()]);
    self_held.open_self_decision = true;
    assert_eq!(class(self_held).0, LivenessClass::Waiting);
    let mut deps = node(vec![pending]);
    deps.eligible = false;
    assert_eq!(
        class(deps),
        (LivenessClass::Waiting, "dependencies".to_string())
    );
}

/// ADR-0090 D4: クラスタ job の wait は名指しの待ち（unit の `blocked(cluster_jobs)` も、wait で止めた atomic の
/// 節点〈`blocked`、直前の理由 `waiting_for_cluster_jobs`〉も StallDetected にしない）。
#[test]
fn cluster_job_waits_are_named_waits() {
    let mut waiting = leaf("a", WorkUnitStatus::Blocked);
    waiting.blocked_reason = Some(WorkUnitBlockedReason::ClusterJobs);
    let done = leaf("b", WorkUnitStatus::Done);
    assert_eq!(
        class(node(vec![waiting, done])),
        (LivenessClass::Waiting, "cluster_jobs".to_string())
    );
    let mut atomic = node(vec![]);
    atomic.has_plan = false;
    atomic.status = Status::Blocked;
    atomic.last_reason = Some(crate::cluster_job::REASON_WAITING.into());
    assert_eq!(
        class(atomic),
        (LivenessClass::Waiting, "cluster_jobs".to_string())
    );
}

/// D10: 走っている・走れる節点は理由なしにしない。
#[test]
fn running_and_runnable_nodes_are_live() {
    assert_eq!(
        class(node(vec![leaf("a", WorkUnitStatus::Ready)])).0,
        LivenessClass::Runnable
    );
    assert_eq!(
        class(node(vec![leaf("a", WorkUnitStatus::Running)])).0,
        LivenessClass::Running
    );
    assert_eq!(
        class(node(vec![leaf("a", WorkUnitStatus::Done)])),
        (LivenessClass::Runnable, "completion".to_string())
    );
    // ADR-0074「F5-fix8 実装時の明確化」: unit の無い計画も同じ（次の tick で最終レビュー）。
    assert_eq!(
        class(node(Vec::new())),
        (LivenessClass::Runnable, "completion".to_string())
    );
    // 審査の後（不合格で ready に戻った）なら replan、余地が無ければ理由なし。
    let mut reviewed = node(vec![leaf("a", WorkUnitStatus::Done)]);
    reviewed.plan_reviewed = true;
    assert_eq!(
        class(reviewed.clone()),
        (LivenessClass::Runnable, "replan".to_string())
    );
    reviewed.replans_left = false;
    assert_eq!(
        class(reviewed),
        (LivenessClass::Unexplained, "replans_exhausted".to_string())
    );
    let mut atomic = node(Vec::new());
    atomic.has_plan = false;
    assert_eq!(class(atomic).0, LivenessClass::Runnable);
    let mut failed = node(vec![leaf("a", WorkUnitStatus::Failed)]);
    assert_eq!(
        class(failed.clone()),
        (LivenessClass::Runnable, "replan".to_string())
    );
    failed.replans_left = false;
    assert_eq!(class(failed).0, LivenessClass::Unexplained);
    let mut running = node(Vec::new());
    running.status = Status::Running;
    assert_eq!(class(running).0, LivenessClass::Running);
    let mut done = node(Vec::new());
    done.status = Status::Done;
    assert!(liveness(&TreeSnapshot { nodes: vec![done] }).is_empty());
}

/// D10 の網: 子の消えた親・pending だけの節点・R3a 付記 15. の「人の replan の 1 回目が不正で 2 回目が
/// 起きない」（blocked(decision) の unit を止める決定は replan で回答済み、次の planner も無い）。
#[test]
fn unexplained_stalls_are_flagged() {
    let mut orphan = leaf("k", WorkUnitStatus::Running);
    orphan.kind = WorkUnitKind::Task;
    orphan.child = Some((TaskId::new(), None));
    assert_eq!(
        class(node(vec![orphan])),
        (LivenessClass::Unexplained, "child_missing".to_string())
    );
    assert_eq!(
        class(node(vec![leaf("p", WorkUnitStatus::Pending)])),
        (LivenessClass::Unexplained, "nothing_runnable".to_string())
    );
    let mut released = leaf("a", WorkUnitStatus::Blocked);
    released.blocked_reason = Some(WorkUnitBlockedReason::Decision);
    let lost = node(vec![released]);
    assert_eq!(
        class(lost.clone()),
        (LivenessClass::Unexplained, "decision_released".to_string())
    );
    // 再試行が待っていると分かれば（R3b の修正）走れる。
    let mut fixed = lost;
    fixed.planner_pending = true;
    assert_eq!(
        class(fixed),
        (LivenessClass::Runnable, "planner".to_string())
    );
}
