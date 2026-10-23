use std::collections::BTreeSet;

use time::OffsetDateTime;

use super::*;
use crate::execution_plan::{WorkUnitContext, WorkUnitSpec, newly_ready, newly_ready_with};
use crate::message::MessageId;
use crate::store::tests::sample_task;
use crate::store::{SqliteStore, TaskStore};

fn unit(
    key: &str,
    phase: Option<&str>,
    seq: u32,
    status: WorkUnitStatus,
    deps: &[&str],
) -> WorkUnitRow {
    let spec = WorkUnitSpec {
        key: key.to_string(),
        kind: if key.starts_with("integrate-") {
            WorkUnitKind::Integrate
        } else {
            WorkUnitKind::Implement
        },
        title: format!("title {key}"),
        objective: format!("objective {key}"),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: phase.map(str::to_string),
    };
    WorkUnitRow::new(
        format!("wu-{key}"),
        "task".to_string(),
        "plan".to_string(),
        seq,
        spec,
        status,
        "2026-10-11T00:00:00Z".to_string(),
    )
}

fn no_halts() -> HaltedSet {
    HaltedSet::new()
}

#[test]
fn task_wait_depends_on_lists_unfinished_and_conversation_accepts_terminal() {
    let task = {
        let mut t = sample_task(Status::Ready);
        t.depends_on = vec![TaskId::new(), TaskId::new(), TaskId::new()];
        t
    };
    let deps = vec![
        (task.depends_on[0], Some(Status::Done)),
        (task.depends_on[1], Some(Status::Failed)),
        (task.depends_on[2], None),
    ];
    let halts = no_halts();
    let facts = WaitFacts {
        task: &task,
        ancestors: &[],
        deps: &deps,
        parent: None,
        halted_projects: &halts,
        halted_milestones: &halts,
    };
    let mut expected = vec![task.depends_on[1], task.depends_on[2]];
    expected.sort();
    assert_eq!(
        db_waits(&facts),
        vec![TaskWait::DependsOn { task_ids: expected }]
    );

    // P-78: 対話は前の対話が終端（failed）なら進めてよい。無い行は待つ。
    let mut conv = task.clone();
    conv.conversation = Some(MessageId::new());
    let facts = WaitFacts {
        task: &conv,
        ..facts
    };
    assert_eq!(
        db_waits(&facts),
        vec![TaskWait::DependsOn {
            task_ids: vec![task.depends_on[2]]
        }]
    );
}

#[test]
fn task_wait_paused_self_ancestor_and_halted_project() {
    let root = {
        let mut t = sample_task(Status::Running);
        t.paused_at = Some(OffsetDateTime::now_utc());
        t
    };
    let mid = sample_task(Status::Running);
    let mut leaf = sample_task(Status::Ready);
    leaf.parent_id = Some(mid.id);
    let halts = no_halts();
    let ancestors = vec![mid.clone(), root.clone()];
    assert_eq!(
        halt_waits(&leaf, &ancestors, &halts, &halts),
        vec![TaskWait::Paused { by: root.id }]
    );

    // 自分の一時停止は祖先より先。
    let mut paused_leaf = leaf.clone();
    paused_leaf.paused_at = Some(OffsetDateTime::now_utc());
    assert_eq!(
        halt_waits(&paused_leaf, &ancestors, &halts, &halts),
        vec![TaskWait::Paused { by: paused_leaf.id }]
    );

    // 祖先が止まった案件に属す・自分の案件と途中目標が止まっている。
    let project = ProjectId::new();
    let milestone = MilestoneId::new();
    let mut in_project = mid.clone();
    in_project.project_id = Some(project);
    let mut projects = HaltedSet::new();
    projects.insert(project.to_string(), HaltState::Archived);
    assert_eq!(
        halt_waits(&leaf, std::slice::from_ref(&in_project), &projects, &halts),
        vec![TaskWait::AncestorProjectHalted {
            task_id: in_project.id,
            project_id: project
        }]
    );
    let mut own = sample_task(Status::Ready);
    own.project_id = Some(project);
    own.milestone_id = Some(milestone);
    let mut milestones = HaltedSet::new();
    milestones.insert(milestone.to_string(), HaltState::Paused);
    assert_eq!(
        halt_waits(&own, &[], &projects, &milestones),
        vec![
            TaskWait::ProjectHalted {
                project_id: project,
                state: HaltState::Archived
            },
            TaskWait::MilestoneHalted {
                milestone_id: milestone,
                state: HaltState::Paused
            },
        ]
    );

    // 対話は止めない（ADR-0044 D6）。
    own.conversation = Some(MessageId::new());
    assert!(halt_waits(&own, &ancestors, &projects, &milestones).is_empty());
}

#[test]
fn task_wait_ready_tasks_excludes_rows_with_db_waits() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut paused_root = sample_task(Status::Running);
    paused_root.paused_at = Some(OffsetDateTime::now_utc());
    store.insert(&paused_root).expect("insert root");
    let mut under_paused = sample_task(Status::Ready);
    under_paused.parent_id = Some(paused_root.id);
    store.insert(&under_paused).expect("insert child");

    let pred = sample_task(Status::Running);
    store.insert(&pred).expect("insert pred");
    let mut waits_on_pred = sample_task(Status::Ready);
    waits_on_pred.depends_on = vec![pred.id];
    store.insert(&waits_on_pred).expect("insert dependent");

    let free = sample_task(Status::Ready);
    store.insert(&free).expect("insert free");

    let ready: Vec<TaskId> = store
        .ready_tasks(10)
        .expect("ready")
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ready, vec![free.id]);

    // 一覧の API 用の読み方も同じ関数（ready_tasks から外れた行は待ちが空でない）。
    let waits = store
        .db_waits_for(&[under_paused.clone(), waits_on_pred.clone(), free.clone()])
        .expect("waits");
    assert_eq!(
        waits[&under_paused.id],
        vec![TaskWait::Paused { by: paused_root.id }]
    );
    assert_eq!(
        waits[&waits_on_pred.id],
        vec![TaskWait::DependsOn {
            task_ids: vec![pred.id]
        }]
    );
    assert!(waits[&free.id].is_empty());
}

#[test]
fn task_wait_unit_waits_agree_with_newly_ready_on_depends_on_and_stage() {
    // 段 edges → remove（直列）。edges の dispatch-prune は cos-route を待つ。
    let units = vec![
        unit("cos-route", Some("edges"), 0, WorkUnitStatus::Running, &[]),
        unit(
            "dispatch-prune",
            Some("edges"),
            1,
            WorkUnitStatus::Pending,
            &["cos-route"],
        ),
        unit("free", Some("edges"), 2, WorkUnitStatus::Pending, &[]),
        unit(
            "integrate-edges",
            Some("edges"),
            3,
            WorkUnitStatus::Pending,
            &["cos-route", "dispatch-prune", "free"],
        ),
        unit("rm", Some("remove"), 4, WorkUnitStatus::Pending, &[]),
    ];
    let waits = unit_waits(&units, &BTreeSet::new());
    let by_key = |k: &str| waits.iter().find(|w| w.key == k).cloned().expect(k);
    assert_eq!(by_key("dispatch-prune").waiting_on, vec!["cos-route"]);
    assert_eq!(
        by_key("integrate-edges").waiting_on,
        vec!["cos-route", "dispatch-prune", "free"]
    );
    assert_eq!(by_key("rm").waiting_stage.as_deref(), Some("edges"));
    assert!(by_key("free").is_clear());

    // 「待ちが空 ⇔ newly_ready に入る」（統合 WU は newly_ready に入れない）。
    let clear: Vec<String> = units
        .iter()
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| waits.iter().any(|w| w.key == u.key && w.is_clear()))
        .map(|u| u.id.clone())
        .collect();
    assert_eq!(newly_ready(&units), clear);
    assert_eq!(newly_ready(&units), vec!["wu-free".to_string()]);

    // 走っている unit があるので task の WorkUnits 待ちは出さない。
    assert!(
        !plan_waits(&units, &[], &[])
            .iter()
            .any(|w| matches!(w, TaskWait::WorkUnits { .. }))
    );
}

#[test]
fn task_wait_plan_waits_reports_units_when_nothing_runnable() {
    let units = vec![
        unit("a", Some("s1"), 0, WorkUnitStatus::Done, &[]),
        unit("b", Some("s1"), 1, WorkUnitStatus::Pending, &["c"]),
        unit("c", Some("s1"), 2, WorkUnitStatus::Pending, &["b"]),
    ];
    assert_eq!(
        plan_waits(&units, &[], &[]),
        vec![
            TaskWait::PlanStalled {
                stage: Some("s1".into()),
                unit_keys: vec!["b".into(), "c".into()],
                detail: "計画の都合で起こせる unit が無い（現在の段 s1、止まっている unit: b, c）"
                    .into(),
            },
            TaskWait::WorkUnits {
                units: vec![
                    UnitWait {
                        key: "b".into(),
                        stage: Some("s1".into()),
                        waiting_on: vec!["c".into()],
                        waiting_stage: None,
                    },
                    UnitWait {
                        key: "c".into(),
                        stage: Some("s1".into()),
                        waiting_on: vec!["b".into()],
                        waiting_stage: None,
                    },
                ]
            }
        ]
    );
}

#[test]
fn task_wait_children_parent_waits_on_unfinished_child() {
    // 子待ちの親: kind task の unit（子の代理）と `child:<key>` 依存の pending unit。
    let child = TaskId::new();
    let other = TaskId::new();
    let mut proxy = unit("survey", None, 0, WorkUnitStatus::Running, &[]);
    proxy.kind = WorkUnitKind::Task;
    proxy.child_task_id = Some(child.to_string());
    let units = vec![
        proxy,
        unit("merge", None, 1, WorkUnitStatus::Pending, &["child:report"]),
    ];
    let children = vec![
        ChildFacts {
            task_id: child,
            status: Status::Running,
            labels: vec!["child-survey".into()],
        },
        ChildFacts {
            task_id: other,
            status: Status::Ready,
            labels: vec!["child-report".into()],
        },
    ];
    let res = resolve_children(&units, &children);
    assert_eq!(res.waiting_units, vec!["merge".to_string()]);
    let mut ids = vec![child, other];
    ids.sort();
    assert_eq!(
        plan_waits(&units, &children, &[])[0],
        TaskWait::Children {
            task_ids: ids,
            unit_keys: vec!["survey".into(), "merge".into()]
        }
    );

    // 子が done になれば依存は解け、newly_ready_with に入る。
    let done_children = vec![ChildFacts {
        task_id: other,
        status: Status::Done,
        labels: vec!["child-report".into()],
    }];
    let res = resolve_children(&units, &done_children);
    assert!(res.waiting_units.is_empty());
    assert_eq!(
        newly_ready_with(&units, &res.done),
        vec!["wu-merge".to_string()]
    );
}

#[test]
fn task_wait_stage_halted_decision_and_blocked_units() {
    let mut failed = unit("x", Some("s1"), 0, WorkUnitStatus::Failed, &[]);
    failed.blocked_reason = None;
    let mut decision = unit("y", Some("s1"), 1, WorkUnitStatus::Blocked, &[]);
    decision.blocked_reason = Some(WorkUnitBlockedReason::Decision);
    decision.needs_decisions = vec!["pick".into()];
    let mut infra = unit("z", Some("s1"), 2, WorkUnitStatus::Blocked, &[]);
    infra.blocked_reason = Some(WorkUnitBlockedReason::Infra);
    let units = vec![failed, decision, infra];
    assert_eq!(
        plan_waits(&units, &[], &["self-key".to_string()]),
        vec![
            TaskWait::StageHalted {
                stage: "s1".into(),
                unit_keys: vec!["x".into()]
            },
            TaskWait::HumanDecision {
                decision_keys: vec!["self-key".into(), "pick".into()]
            },
            TaskWait::UnitsBlocked {
                reason: WorkUnitBlockedReason::Infra,
                unit_keys: vec!["z".into()]
            },
        ]
    );
}

#[test]
fn task_wait_serializes_with_kind_tag() {
    let w = TaskWait::Capacity {
        scope: CapacityScope::Account,
        id: Some("claude-code/a".into()),
    };
    let v = serde_json::to_value(&w).expect("json");
    assert_eq!(v["kind"], "capacity");
    assert_eq!(v["scope"], "account");
    assert_eq!(
        serde_json::to_value(TaskWait::ReviewAwaitingHuman).expect("json")["kind"],
        "review_awaiting_human"
    );
}

/// ADR 2026-10-23-stage-order-and-plan-stall D4: 本番 01M5HDSN3A の止まった形（修正前の段の順位で作られた状態に、
/// 依存先の行が無い pending を足したもの）。前の段 gap-c の pending が行の無い key を待ち、後の段 close の parity-final は
/// ready だが現在の段でないので起こせない。waits に `PlanStalled` が出る（以前は ready の unit があるので [] だった）。
#[test]
fn plan_stall_is_reported_when_nothing_can_be_dispatched() {
    let units = vec![
        unit("gap-b1", Some("gap-b"), 0, WorkUnitStatus::Done, &[]),
        unit(
            "integrate-gap-b",
            Some("gap-b"),
            1,
            WorkUnitStatus::Done,
            &["gap-b1"],
        ),
        unit(
            "chat-port",
            Some("gap-c"),
            2,
            WorkUnitStatus::Pending,
            &["gone"],
        ),
        unit(
            "integrate-gap-c",
            Some("gap-c"),
            3,
            WorkUnitStatus::Pending,
            &["chat-port"],
        ),
        unit("parity-final", Some("close"), 4, WorkUnitStatus::Ready, &[]),
        unit(
            "integrate-close",
            Some("close"),
            5,
            WorkUnitStatus::Pending,
            &["parity-final"],
        ),
    ];
    let waits = plan_waits(&units, &[], &[]);
    assert_eq!(
        waits,
        vec![TaskWait::PlanStalled {
            stage: Some("gap-c".into()),
            unit_keys: vec!["chat-port".into(), "parity-final".into()],
            detail: "計画の都合で起こせる unit が無い（現在の段 gap-c、止まっている unit: chat-port, parity-final）".into(),
        }]
    );
    let json = serde_json::to_value(&waits[0]).unwrap();
    assert_eq!(json["kind"], "plan_stalled");
}

/// 動いている・名指しの待ちがある計画には `PlanStalled` を出さない。
#[test]
fn plan_stall_is_none_while_the_plan_can_move_or_waits_on_a_named_reason() {
    let ext = BTreeSet::new();
    // 起こせる unit がある。
    let runnable = vec![unit("a", Some("s1"), 0, WorkUnitStatus::Ready, &[])];
    assert_eq!(plan_stall(&runnable, &ext, &[]), None);
    // 上げられる unit がある（dispatcher が次の tick で上げる）。
    let promotable = vec![
        unit("a", Some("s1"), 0, WorkUnitStatus::Done, &[]),
        unit("b", Some("s1"), 1, WorkUnitStatus::Pending, &["a"]),
    ];
    assert_eq!(plan_stall(&promotable, &ext, &[]), None);
    // 段の unit がすべて done で統合が走れる。
    let integrate = vec![
        unit("a", Some("s1"), 0, WorkUnitStatus::Done, &[]),
        unit(
            "integrate-s1",
            Some("s1"),
            1,
            WorkUnitStatus::Pending,
            &["a"],
        ),
        unit("b", Some("s2"), 2, WorkUnitStatus::Pending, &[]),
    ];
    assert_eq!(plan_stall(&integrate, &ext, &[]), None);
    // 走っている・blocked・failed の unit、未回答の決定、子待ち。
    for status in [
        WorkUnitStatus::Running,
        WorkUnitStatus::Blocked,
        WorkUnitStatus::Failed,
    ] {
        let units = vec![
            unit("a", Some("s1"), 0, status, &[]),
            unit("b", Some("s1"), 1, WorkUnitStatus::Pending, &["a"]),
        ];
        assert_eq!(plan_stall(&units, &ext, &[]), None, "{status:?}");
    }
    let stuck = vec![unit("b", Some("s1"), 0, WorkUnitStatus::Pending, &["gone"])];
    assert!(plan_stall(&stuck, &ext, &[]).is_some());
    assert_eq!(plan_stall(&stuck, &ext, &["d".to_string()]), None);
    let child = vec![unit(
        "b",
        Some("s1"),
        0,
        WorkUnitStatus::Pending,
        &["child:k"],
    )];
    assert_eq!(plan_stall(&child, &ext, &[]), None);
    // 全部 done（計画の完了は別の経路）。
    let done = vec![unit("a", Some("s1"), 0, WorkUnitStatus::Done, &[])];
    assert_eq!(plan_stall(&done, &ext, &[]), None);
}
