use std::collections::HashMap;

use super::*;
use crate::add::{NewTaskSpec, create_task};
use task_core::model_router::feedback::{RequestSourceAttempt, RoutingRequestRecord};
use task_core::model_router::policy::RoutingMode;
use task_core::model_routing::LaneResolution;
use task_core::{LaneCeiling, RoutingRecord, Task, Tier};

fn new_task(store: &SqliteStore) -> Task {
    let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "t", "objective": "o",
        "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]
    }))
    .unwrap();
    create_task(store, spec, time::OffsetDateTime::now_utc()).unwrap()
}

fn trace(
    stage: &str,
    decision_id: &str,
    run_id: Option<&str>,
    request_id: Option<&str>,
) -> RoutingTraceV1 {
    RoutingTraceV1 {
        decision_id: decision_id.into(),
        parent_decision_id: None,
        task_id: None,
        work_unit_id: None,
        run_id: run_id.map(str::to_string),
        request_id: request_id.map(str::to_string),
        stage: stage.into(),
        mode: RoutingMode::Shadow,
        policy_version: "p2".into(),
        catalog_version: "c1".into(),
        feature_version: "1".into(),
        estimator_version: "heuristic-1".into(),
        snapshot_id: "snap-1".into(),
        observed_at: None,
        requested_lane: Tier::Cheap,
        selected_lane: Some(Tier::Cheap),
        candidates: vec![],
        selected: Some("qwen".into()),
        fallback_order: vec!["qwen".into()],
        reasons: vec![],
        source_id: Some("openai_compatible:qwen".into()),
        model: Some("qwen3".into()),
        account_id: None,
    }
}

fn decided(task: &Task, run_id: &str, optimizer: Option<RoutingTraceV1>) -> Event {
    let decision = task_core::decide_for_task(task, &LaneCeiling::default()).unwrap();
    Event::RoutingDecided {
        run_id: run_id.into(),
        record: Box::new(RoutingRecord {
            org_node: None,
            harness: None,
            decision,
            resolution: LaneResolution::default(),
            quota_reason: None,
            work_unit_id: None,
            escalation: None,
            optimizer,
        }),
    }
}

#[derive(Default)]
struct FakeLog(HashMap<String, RoutingCorrelation>);

impl RequestLog for FakeLog {
    fn correlation(&self, request_id: &str) -> Result<Option<RoutingCorrelation>, OpsError> {
        Ok(self.0.get(request_id).cloned())
    }
}

#[test]
fn reads_the_audit_from_the_store_and_rejects_unknown_tasks() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(&store);
    store
        .append_event(
            task.id,
            &Event::WorkerFinished {
                run_id: "r1".into(),
                outcome: "done: ok".into(),
                usage: None,
                role: None,
                metrics: Some(task_core::RunMetrics {
                    wall_ms: 5,
                    retries: 0,
                    peak_context_tokens: None,
                    turns: None,
                }),
                end: None,
            },
        )
        .unwrap();
    let audit = task_routing_audit(&store, task.id).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].wall_ms, Some(5));
    assert!(matches!(
        task_routing_audit(&store, TaskId::new()),
        Err(OpsError::NotFound(_))
    ));
    // 旧 run（Phase 2 の trace 無し）の新欄は None。
    let full = task_routing_audit_with_requests(&store, &store, task.id).unwrap();
    assert_eq!(full.runs.len(), 1);
    assert_eq!(full.runs[0].requests, None);
    assert_eq!(full.runs[0].audit_incomplete, None);
    assert!(full.unbound_requests.is_empty());
    assert!(matches!(
        task_routing_audit_with_requests(&store, &store, TaskId::new()),
        Err(OpsError::NotFound(_))
    ));
}

#[test]
fn routing_audit_marks_missing_request_as_incomplete() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(&store);
    let events = vec![
        decided(
            &task,
            "r1",
            Some(trace("dispatcher", "r1", Some("r1"), None)),
        ),
        // log と結べる要求。
        decided(
            &task,
            "r1",
            Some(trace(PROXY_STAGE, "dec-a", Some("r1"), Some("req-a"))),
        ),
        decided(
            &task,
            "r2",
            Some(trace("dispatcher", "r2", Some("r2"), None)),
        ),
        // log に行が無い要求。
        decided(
            &task,
            "r2",
            Some(trace(PROXY_STAGE, "dec-b", Some("r2"), Some("req-b"))),
        ),
        // log の decision が食い違う要求（推定で結ばない）。
        decided(
            &task,
            "r2",
            Some(trace(PROXY_STAGE, "dec-c", Some("r2"), Some("req-c"))),
        ),
        // 知らない run の要求。
        decided(
            &task,
            "r9",
            Some(trace(PROXY_STAGE, "dec-d", None, Some("req-d"))),
        ),
    ];
    let mut log = FakeLog::default();
    log.0.insert(
        "req-a".into(),
        RoutingCorrelation {
            decision_id: Some("dec-a".into()),
            snapshot_id: Some("snap-1".into()),
            run_id: Some("r1".into()),
            task_id: None,
            source_id: Some("openai_compatible:qwen".into()),
            model: Some("qwen3".into()),
            account: None,
        },
    );
    log.0.insert(
        "req-c".into(),
        RoutingCorrelation {
            decision_id: Some("other".into()),
            ..RoutingCorrelation::default()
        },
    );
    // log に行はあるが run が分からない要求。
    log.0.insert(
        "req-d".into(),
        RoutingCorrelation {
            decision_id: Some("dec-d".into()),
            ..RoutingCorrelation::default()
        },
    );
    let audit = routing_audit_with_requests(&task, &events, &log).unwrap();
    assert_eq!(audit.runs.len(), 2);

    // r1: dispatch の trace は proxy の決定で上書きされず、子は log と結べた。
    let r1 = &audit.runs[0];
    assert_eq!(r1.audit.run_id, "r1");
    assert_eq!(r1.audit.optimizer.as_ref().unwrap().stage, "dispatcher");
    assert_eq!(r1.audit_incomplete, Some(false));
    let children = r1.requests.as_ref().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].request_id.as_deref(), Some("req-a"));
    assert_eq!(
        children[0].log.as_ref().unwrap().source_id.as_deref(),
        Some("openai_compatible:qwen")
    );
    assert_eq!(children[0].incomplete_reason, None);

    // r2: 行の無い要求と decision の食い違う要求は結ばず、audit_incomplete。
    let r2 = &audit.runs[1];
    assert_eq!(r2.audit_incomplete, Some(true));
    assert_eq!(
        r2.incomplete_reasons,
        vec![
            INCOMPLETE_REQUEST_LOG_MISMATCH.to_string(),
            INCOMPLETE_REQUEST_LOG_MISSING.to_string()
        ]
    );
    assert!(
        r2.requests
            .as_ref()
            .unwrap()
            .iter()
            .all(|c| c.log.is_none())
    );

    // 知らない run の要求は run に結ばない。
    assert_eq!(audit.unbound_requests.len(), 1);
    assert_eq!(
        audit.unbound_requests[0].incomplete_reason.as_deref(),
        Some(INCOMPLETE_RUN_UNKNOWN)
    );

    // JSON: 旧欄は flatten で同じ位置、新欄は別名で区別できる。
    let json = serde_json::to_value(&audit.runs[1]).unwrap();
    assert_eq!(json["run_id"], "r2");
    assert_eq!(json["audit_incomplete"], true);
    // 同じ入力から同じ監査。
    assert_eq!(
        audit,
        routing_audit_with_requests(&task, &events, &log).unwrap()
    );
}

#[test]
fn dispatch_decision_pointing_at_a_request_without_child_is_incomplete() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(&store);
    let events = vec![decided(
        &task,
        "r1",
        Some(trace("dispatcher", "r1", Some("r1"), Some("req-x"))),
    )];
    let audit = routing_audit_with_requests(&task, &events, &FakeLog::default()).unwrap();
    assert_eq!(audit.runs[0].audit_incomplete, Some(true));
    assert_eq!(
        audit.runs[0].incomplete_reasons,
        vec![INCOMPLETE_REQUEST_LOG_MISSING.to_string()]
    );
}

fn request_decided(
    request_id: &str,
    decision_id: &str,
    parent: Option<&str>,
    run_id: Option<&str>,
    attempts: &[(&str, &str, Option<&str>)],
) -> Event {
    let mut t = trace(PROXY_STAGE, decision_id, run_id, Some(request_id));
    t.parent_decision_id = parent.map(str::to_string);
    Event::RoutingRequestDecided {
        record: Box::new(RoutingRequestRecord {
            request_id: request_id.into(),
            decision_id: decision_id.into(),
            parent_decision_id: parent.map(str::to_string),
            run_id: run_id.map(str::to_string),
            trace: Some(Box::new(t)),
            attempts: attempts
                .iter()
                .map(|(source, model, fallback)| RequestSourceAttempt {
                    source_id: (*source).into(),
                    model: Some((*model).into()),
                    account_id: None,
                    fallback_reason: fallback.map(str::to_string),
                })
                .collect(),
            fallback_reason: attempts.iter().find_map(|a| a.2.map(str::to_string)),
        }),
    }
}

/// Phase 3: `routing_request_decided` を parent decision か run_id で run に結び、Phase 2 の同じ要求の子へ
/// 足す。実際の source は proxy log → 試した source → trace の順。結べない要求は推定で結ばない。
#[test]
fn request_records_link_to_runs_with_actual_source() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(&store);
    let events = vec![
        decided(&task, "r1", Some(trace("dispatch", "d1", Some("r1"), None))),
        // parent decision で r1 に結ぶ。log が無いので incomplete、実 source は試した最後。
        request_decided(
            "req-x",
            "px",
            Some("d1"),
            None,
            &[
                ("openai_compatible:qwen", "qwen3", Some("rate_limit")),
                ("claude:cc-1", "sonnet", None),
            ],
        ),
        // run_id で結ぶ。log と一致するので実 source は log。
        request_decided("req-y", "py", None, Some("r1"), &[]),
        // Phase 2 の子と同じ要求は 1 件にまとめる。
        decided(
            &task,
            "r1",
            Some(trace(PROXY_STAGE, "pz", Some("r1"), Some("req-z"))),
        ),
        request_decided(
            "req-z",
            "pz",
            Some("d1"),
            Some("r1"),
            &[("s-z", "m-z", None)],
        ),
        // 知らない親の要求は run に結ばない。
        request_decided("req-u", "pu", Some("d-unknown"), None, &[]),
    ];
    let mut log = FakeLog::default();
    for (req, dec) in [("req-y", "py"), ("req-z", "pz")] {
        log.0.insert(
            req.into(),
            RoutingCorrelation {
                decision_id: Some(dec.into()),
                source_id: Some(format!("log-src-{req}")),
                model: Some(format!("log-model-{req}")),
                ..RoutingCorrelation::default()
            },
        );
    }
    let audit = routing_audit_with_requests(&task, &events, &log).unwrap();
    assert_eq!(audit.runs.len(), 1);
    let run = &audit.runs[0];
    assert_eq!(run.audit.decision_id.as_deref(), Some("d1"));
    let reqs = run.requests.as_ref().expect("requests");
    let ids: Vec<_> = reqs.iter().map(|c| c.request_id.as_deref()).collect();
    assert_eq!(ids, vec![Some("req-z"), Some("req-x"), Some("req-y")]);
    let x = &reqs[1];
    assert_eq!(x.parent_decision_id.as_deref(), Some("d1"));
    assert_eq!(x.attempts.len(), 2);
    assert_eq!(x.fallback_reason.as_deref(), Some("rate_limit"));
    assert_eq!(
        x.incomplete_reason.as_deref(),
        Some(INCOMPLETE_REQUEST_LOG_MISSING)
    );
    let xa = x.actual.as_ref().expect("actual");
    assert_eq!(
        (
            xa.source_id.as_deref(),
            xa.model.as_deref(),
            xa.from.as_str()
        ),
        (
            Some("claude:cc-1"),
            Some("sonnet"),
            ACTUAL_FROM_REQUEST_ATTEMPTS
        )
    );
    let ya = reqs[2].actual.as_ref().expect("actual");
    assert_eq!(
        (ya.model.as_deref(), ya.from.as_str()),
        (Some("log-model-req-y"), ACTUAL_FROM_PROXY_LOG)
    );
    let z = &reqs[0];
    assert_eq!(z.attempts.len(), 1);
    assert_eq!(z.parent_decision_id.as_deref(), Some("d1"));
    assert_eq!(
        z.actual.as_ref().map(|a| a.from.as_str()),
        Some(ACTUAL_FROM_PROXY_LOG)
    );
    assert_eq!(run.audit_incomplete, Some(true));
    assert_eq!(run.incomplete_reasons, vec![INCOMPLETE_REQUEST_LOG_MISSING]);
    assert_eq!(run.actual_sources.len(), 3);
    assert_eq!(audit.unbound_requests.len(), 1);
    assert_eq!(
        audit.unbound_requests[0].request_id.as_deref(),
        Some("req-u")
    );
}
