use super::*;
use task_core::{Status, TaskKind};

#[test]
fn unknown_and_repeated_keys_are_rejected() {
    assert!(QueryParams::parse(Some("limit=1&bogus=2"), &["limit"]).is_err());
    let q =
        QueryParams::parse(Some("limit=1&limit=2"), &["limit"]).unwrap_or_else(|_| panic!("parse"));
    assert!(q.single("limit").is_err());
}

#[test]
fn lists_accept_repeated_and_comma_separated_values() {
    let q = QueryParams::parse(Some("status=ready,running&status=done"), &["status"])
        .unwrap_or_else(|_| panic!("parse"));
    assert_eq!(q.list("status"), vec!["ready", "running", "done"]);
    let statuses: Vec<Status> = q
        .list("status")
        .into_iter()
        .map(|s| parse_snake("status", s).unwrap_or_else(|_| panic!("status")))
        .collect();
    assert_eq!(statuses, vec![Status::Ready, Status::Running, Status::Done]);
    assert!(parse_snake::<TaskKind>("kind", "bogus").is_err());
}

#[test]
fn limit_defaults_rejects_zero_and_clamps() {
    let parse =
        |raw: &str| QueryParams::parse(Some(raw), &["limit"]).unwrap_or_else(|_| panic!("parse"));
    assert_eq!(parse("").limit("limit", 100, 500).ok(), Some(100));
    assert!(parse("limit=0").limit("limit", 100, 500).is_err());
    assert!(parse("limit=-1").limit("limit", 100, 500).is_err());
    assert_eq!(parse("limit=9999").limit("limit", 100, 500).ok(), Some(500));
}

#[test]
fn event_types_are_validated_against_the_event_vocabulary() {
    let q = QueryParams::parse(Some("types=transitioned,worker_finished"), &["types"])
        .unwrap_or_else(|_| panic!("parse"));
    let set = q.event_types().ok().flatten().unwrap_or_default();
    assert!(set.contains("transitioned") && set.contains("worker_finished") && set.len() == 2);
    let f5 = QueryParams::parse(
        Some(
            "types=project_plan_proposed,project_plan_decided,phase_reported,pause_points_resolved",
        ),
        &["types"],
    )
    .unwrap_or_else(|_| panic!("parse"));
    let f5_types = f5.event_types().ok().flatten().unwrap_or_default();
    assert_eq!(f5_types.len(), 4);
    for event_type in [
        "project_plan_proposed",
        "project_plan_decided",
        "phase_reported",
        "pause_points_resolved",
    ] {
        assert!(f5_types.contains(event_type), "{event_type}");
    }
    let bad =
        QueryParams::parse(Some("types=nope"), &["types"]).unwrap_or_else(|_| panic!("parse"));
    assert!(bad.event_types().is_err());
}

/// ADR-0079（Phase R1a）: 木の Event の `type` 名が serde の名前・`event_type_name`・`EVENT_TYPES` で一致する。
#[test]
fn tree_event_types_match_their_serde_names() {
    let child = TaskId::new();
    let events = vec![
        Event::ChildTaskCreated {
            plan_id: "p".into(),
            unit_key: "u".into(),
            child_task_id: child,
            depth: 2,
        },
        Event::ChildAdopted {
            plan_id: "p".into(),
            unit_key: "u".into(),
            stage: "s".into(),
            child_task_id: child,
        },
        Event::UnitGateOverridden {
            plan_id: "p".into(),
            unit_key: "u".into(),
            declared: task_core::UnitDeclared::Leaf,
            gate: task_core::ExecutionMode::Compound,
            action: task_core::UnitGateAction::Promoted,
            depth: 2,
            threshold: 7,
            score: 8,
            reason: "compound/score".into(),
        },
        Event::DecisionAnswered {
            id: "d".into(),
            option: "a".into(),
            note: None,
            by: "human".into(),
        },
        Event::DecisionWithdrawn {
            id: "d".into(),
            reason: "r".into(),
        },
        Event::PlanApprovalRequested {
            plan_id: "p".into(),
            reasons: vec!["has_decisions".into()],
        },
        Event::WorkUnitSpecOverridden {
            work_unit_id: "w".into(),
            key: "baseline".into(),
            plan_id: "p".into(),
            plan_version: 2,
            changed_fields: vec!["checks".into()],
        },
        Event::WorkUnitChecksFailed {
            run_id: "r".into(),
            work_unit_id: "w".into(),
            key: "verify".into(),
            cwd: "/ws/t".into(),
            failed: vec![task_core::FailedWorkUnitCheck {
                cmd: "false".into(),
                expect_exit: 0,
                detail: "cmd=\"false\" exit=Some(1) expected=0".into(),
            }],
        },
        Event::StallDetected {
            task_id: child,
            detail: "x".into(),
            reason: String::new(),
            since: String::new(),
            path: Vec::new(),
        },
    ];
    for e in &events {
        let v = serde_json::to_value(e).unwrap_or_default();
        let serde_name = v.get("type").and_then(|t| t.as_str()).unwrap_or_default();
        assert_eq!(serde_name, event_type_name(e));
        assert!(EVENT_TYPES.contains(&serde_name), "{serde_name}");
    }
    assert!(EVENT_TYPES.contains(&"decision_requested"));
}

/// ADR-0090 D3: クラスタ job の wait の Event の `type` 名が serde の名前・`event_type_name`・`EVENT_TYPES` で一致し、
/// `EVENT_TYPES` は重複なく 48 語（browser Phase 2 の 45 語 + 3）。
#[test]
fn cluster_job_wait_event_types_match_their_serde_names() {
    use task_core::cluster_job::{
        ClusterJobState, ClusterJobStatus, ClusterJobWait, ClusterJobWaitState, ClusterScheduler,
    };
    let task_id = TaskId::new();
    let status = ClusterJobStatus {
        job_id: "42634".into(),
        state: ClusterJobState::Running,
        exit_status: None,
        raw_state: Some("R".into()),
    };
    let events = vec![
        Event::ClusterJobWaitStarted {
            wait: Box::new(ClusterJobWait {
                wait_id: "w".into(),
                task_id,
                work_unit_id: None,
                run_id: "r".into(),
                cluster: "sirius".into(),
                scheduler: ClusterScheduler::Pbs,
                jobs: vec!["42634".into()],
                poll_secs: 300,
                timeout_secs: 3600,
                summary: String::new(),
                checkpoint: None,
                created_at: "2026-09-30T00:00:00Z".into(),
                deadline: "2026-09-30T01:00:00Z".into(),
                last_polled_at: None,
                finished_at: None,
                state: ClusterJobWaitState::Waiting,
                last_status: Vec::new(),
            }),
        },
        Event::ClusterJobWaitPolled {
            wait_id: "w".into(),
            jobs: vec![status.clone()],
        },
        Event::ClusterJobWaitFinished {
            wait_id: "w".into(),
            state: ClusterJobWaitState::Satisfied,
            jobs: vec![status],
            detail: String::new(),
        },
    ];
    for e in &events {
        let v = serde_json::to_value(e).unwrap_or_default();
        let serde_name = v.get("type").and_then(|t| t.as_str()).unwrap_or_default();
        assert_eq!(serde_name, event_type_name(e));
        assert!(EVENT_TYPES.contains(&serde_name), "{serde_name}");
    }
    let unique: std::collections::BTreeSet<&str> = EVENT_TYPES.iter().copied().collect();
    assert_eq!(unique.len(), EVENT_TYPES.len());
    assert_eq!(EVENT_TYPES.len(), 50);
}
