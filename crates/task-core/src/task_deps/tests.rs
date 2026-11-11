//! ADR 2026-11-11-unit-task-dependencies の純粋関数の試験。既存の試験は `task:` の依存を持たないので、ここで
//! 解決（D2・D3・D6）・検証（D1・D4）・計画の詰まりと生存確認の扱い（D3・D5）を固定する。

use super::*;
use crate::execution_plan::{
    ExecutionLimits, WorkUnitContext, WorkUnitKind, WorkUnitSpec, newly_ready_with, validate,
};
use crate::tree::{LivenessClass, LivenessUnitFacts, NodeLivenessFacts, TreeSnapshot, liveness};

fn row(key: &str, seq: u32, status: WorkUnitStatus, depends_on: &[String]) -> WorkUnitRow {
    WorkUnitRow::new(
        format!("wu-{key}"),
        "task".to_string(),
        "plan".to_string(),
        seq,
        WorkUnitSpec {
            key: key.to_string(),
            kind: WorkUnitKind::Implement,
            title: format!("title {key}"),
            objective: format!("objective for {key}"),
            depends_on: depends_on.to_vec(),
            done_when: vec![],
            checks: vec![],
            context: WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: Some("s".into()),
        },
        status,
        "2026-11-11T00:00:00Z".to_string(),
    )
}

fn fact(id: TaskId, status: Status) -> TaskDependencyFact {
    TaskDependencyFact {
        task_id: id,
        status,
        integration: None,
    }
}

/// (a) の判断: done で取り込みの理由が無ければ満たされ、`newly_ready_with` が unit を上げる。done でない・取り込み待ちは
/// 待ちで、依存先と cause が出る。
#[test]
fn task_dependency_waits_until_done_and_integrated() {
    let dep = TaskId::new();
    let units = vec![row(
        "final",
        1,
        WorkUnitStatus::Pending,
        &[task_dependency_key(dep)],
    )];

    let running = resolve_task_dependencies(&units, &[fact(dep, Status::Running)]);
    assert!(running.met.is_empty());
    assert_eq!(running.waits.len(), 1);
    assert_eq!(running.waits[0].task_id, dep);
    assert_eq!(running.waits[0].cause, TaskDependencyCause::Unfinished);
    assert_eq!(running.waits[0].unit_keys, vec!["final".to_string()]);
    assert!(newly_ready_with(&units, &running.met).is_empty());

    let mut unmerged = fact(dep, Status::Done);
    unmerged.integration = Some((IntegrationWaitCause::Unmerged, "未取り込み".into()));
    let r = resolve_task_dependencies(&units, &[unmerged]);
    assert_eq!(r.waits[0].cause, TaskDependencyCause::Unmerged);
    assert_eq!(r.waits[0].detail, "未取り込み");
    assert!(newly_ready_with(&units, &r.met).is_empty());

    let r = resolve_task_dependencies(&units, &[fact(dep, Status::Done)]);
    assert!(r.waits.is_empty());
    assert!(r.met.contains(&task_dependency_key(dep)));
    assert_eq!(
        newly_ready_with(&units, &r.met),
        vec!["wu-final".to_string()]
    );
}

/// (c) の判断: failed・cancelled は終わった cause（D3）。discarded も同じ。保留・見送りは終わっていない。
#[test]
fn task_dependency_failed_or_cancelled_is_ended() {
    let (a, b) = (TaskId::new(), TaskId::new());
    let units = vec![
        row("x", 1, WorkUnitStatus::Pending, &[task_dependency_key(a)]),
        row("y", 2, WorkUnitStatus::Pending, &[task_dependency_key(b)]),
    ];
    let r = resolve_task_dependencies(
        &units,
        &[fact(a, Status::Failed), fact(b, Status::Cancelled)],
    );
    assert_eq!(r.cause_of("x"), Some(TaskDependencyCause::Failed));
    assert_eq!(r.cause_of("y"), Some(TaskDependencyCause::Cancelled));
    assert!(r.waits.iter().all(|w| w.cause.is_ended()));
    assert!(TaskDependencyCause::Discarded.is_ended());
    assert!(!TaskDependencyCause::Held.is_ended());
    assert!(!TaskDependencyCause::DeliverySkipped.is_ended());
}

/// D6: 始まった（ready 以降の）unit の依存は見ない。依存先が戻っても止めない。
#[test]
fn started_units_are_not_judged_again() {
    let dep = TaskId::new();
    let units = vec![
        row(
            "run",
            1,
            WorkUnitStatus::Running,
            &[task_dependency_key(dep)],
        ),
        row("ok", 2, WorkUnitStatus::Ready, &[task_dependency_key(dep)]),
    ];
    let r = resolve_task_dependencies(&units, &[fact(dep, Status::Reviewing)]);
    assert!(r.waits.is_empty() && r.met.is_empty());
}

/// D5: 解けていない `task:` 依存を持つ pending の unit は名指しの待ちで、計画の詰まりにしない。生存確認は終わった cause
/// だけを理由なし（`task_dependency_ended`）に、それ以外を名指しの待ち（`task_dependency`）に分ける。
#[test]
fn task_dependency_is_a_named_wait_and_ended_ones_are_unexplained() {
    let dep = TaskId::new();
    let units = vec![row(
        "final",
        1,
        WorkUnitStatus::Pending,
        &[task_dependency_key(dep)],
    )];
    assert_eq!(crate::wait::plan_stall(&units, &BTreeSet::new(), &[]), None);

    let node = |cause| NodeLivenessFacts {
        task_id: dep,
        status: Status::Ready,
        last_reason: None,
        leased: false,
        eligible: true,
        has_plan: true,
        planner_pending: false,
        open_self_decision: false,
        tree_limit_decision_open: false,
        replans_left: true,
        plan_reviewed: false,
        plan_stalled: None,
        units: vec![LivenessUnitFacts {
            key: "final".into(),
            kind: WorkUnitKind::Implement,
            status: WorkUnitStatus::Pending,
            blocked_reason: None,
            child: None,
            waits_on_open_decision: false,
            waits_on_child_dep: false,
            task_dependency: Some(cause),
        }],
    };
    let v = liveness(&TreeSnapshot {
        nodes: vec![node(TaskDependencyCause::Unmerged)],
    });
    assert_eq!(
        (v[0].class, v[0].reason.as_str()),
        (LivenessClass::Waiting, "task_dependency")
    );
    let v = liveness(&TreeSnapshot {
        nodes: vec![node(TaskDependencyCause::Cancelled)],
    });
    assert_eq!(
        (v[0].class, v[0].reason.as_str()),
        (LivenessClass::Unexplained, "task_dependency_ended")
    );
}

fn tree_on() -> ExecutionLimits {
    ExecutionLimits {
        tree: crate::tree::TreeLimits {
            enabled: true,
            ..crate::tree::TreeLimits::default()
        },
        ..ExecutionLimits::default()
    }
}

fn v3_with_dep(dep: &str) -> ExecutionPlanSpec {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/execution-plan/v3-browser.json"
    ))
    .unwrap();
    let mut spec: ExecutionPlanSpec = serde_json::from_str(&text).unwrap();
    let p3 = spec.units.iter_mut().find(|u| u.key == "p3").unwrap();
    p3.depends_on.push(dep.to_string());
    spec
}

/// D1: /3 は `task:<ULID>` を受け、段の規則に数えない。id の形が違えば拒む。/1・/2 では未知の依存。
#[test]
fn v3_accepts_task_dependency_and_rejects_a_malformed_id() {
    let id = TaskId::new();
    let spec = v3_with_dep(&task_dependency_key(id));
    assert!(validate(&spec, tree_on(), &[]).is_ok());
    assert_eq!(plan_task_dependencies(&spec), vec![("p3".to_string(), id)]);

    let bad = v3_with_dep("task:not-a-ulid");
    let errors = validate(&bad, tree_on(), &[]).expect_err("malformed id");
    assert!(
        errors.contains(&PlanValidationError::InvalidTaskDependency {
            key: "p3".into(),
            depends_on: "task:not-a-ulid".into()
        }),
        "{errors:?}"
    );
}

/// (d): 存在しない task・自分の木の task・別の案件・循環を拒み、木の外の同じ案件の task は通す。
#[test]
fn task_dependency_errors_reject_missing_own_tree_other_project_and_cycle() {
    let project = ProjectId::new();
    let (me, root, sibling, other, waits_back, missing) = (
        TaskId::new(),
        TaskId::new(),
        TaskId::new(),
        TaskId::new(),
        TaskId::new(),
        TaskId::new(),
    );
    let ctx = TaskDependencyContext {
        project: Some(project),
        own_tree: BTreeSet::from([root, me]),
        targets: BTreeMap::from([
            (sibling, Some(project)),
            (root, Some(project)),
            (other, Some(ProjectId::new())),
            (waits_back, Some(project)),
        ]),
        waits_on: BTreeMap::from([(waits_back, vec![sibling]), (sibling, vec![me])]),
    };
    let errors_of =
        |id: TaskId| task_dependency_errors(&v3_with_dep(&task_dependency_key(id)), &ctx);
    let key = || "p3".to_string();
    assert_eq!(
        errors_of(missing),
        vec![PlanValidationError::TaskDependencyNotFound {
            key: key(),
            task_id: missing
        }]
    );
    assert_eq!(
        errors_of(root),
        vec![PlanValidationError::TaskDependencyInOwnTree {
            key: key(),
            task_id: root
        }]
    );
    assert_eq!(
        errors_of(other),
        vec![PlanValidationError::TaskDependencyOtherProject {
            key: key(),
            task_id: other
        }]
    );
    assert_eq!(
        errors_of(waits_back),
        vec![PlanValidationError::TaskDependencyCycle {
            key: key(),
            task_id: waits_back
        }]
    );
    let free = TaskId::new();
    let ok = TaskDependencyContext {
        targets: BTreeMap::from([(free, Some(project))]),
        ..ctx.clone()
    };
    assert!(task_dependency_errors(&v3_with_dep(&task_dependency_key(free)), &ok).is_empty());
    let no_project = TaskDependencyContext {
        project: None,
        ..ok
    };
    assert_eq!(
        task_dependency_errors(&v3_with_dep(&task_dependency_key(free)), &no_project),
        vec![PlanValidationError::TaskDependencyOtherProject {
            key: key(),
            task_id: free
        }]
    );
}
