use super::*;

#[test]
fn is_milestones_plan_task_needs_the_kind_and_the_label() {
    use crate::model::{
        Budget, Check, Criterion, Status, Task, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
    };
    let now = time::OffsetDateTime::now_utc();
    let base = Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Plan,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: "ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 0,
            max_tokens: None,
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
    };
    assert!(!is_milestones_plan_task(&base), "no label yet");

    let mut labeled = base.clone();
    labeled.labels = vec![MILESTONES_PLAN_LABEL.to_string()];
    assert!(is_milestones_plan_task(&labeled));

    let mut wrong_kind = labeled.clone();
    wrong_kind.kind = TaskKind::Execute;
    assert!(!is_milestones_plan_task(&wrong_kind));
}

/// ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成。
#[test]
fn committed_schema_matches_generated() {
    crate::assert_committed_schema("protocol/project-plan.schema.json", &schema_value());
}

#[test]
fn committed_delta_schema_matches_generated() {
    crate::assert_committed_schema(
        "protocol/project-plan-delta.schema.json",
        &delta_schema_value(),
    );
}
