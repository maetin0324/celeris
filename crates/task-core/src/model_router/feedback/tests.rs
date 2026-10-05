use super::*;
use crate::model::{RunMetrics, Status, TaskRouting, Tier, TierSource, Usage};
use crate::model_policy::{LaneCeiling, RoutingRecord, decide_for_task};
use crate::model_routing::LaneResolution;

fn routing_decided(run_id: &str) -> Event {
    let mut t = crate::model_policy::tests::task("crates/x.rs の typo を直す", vec![]);
    t.routing = Some(TaskRouting {
        tier_source: TierSource::Default,
        ..TaskRouting::default()
    });
    let decision = decide_for_task(&t, &LaneCeiling::default()).unwrap();
    Event::RoutingDecided {
        run_id: run_id.into(),
        record: Box::new(RoutingRecord {
            org_node: Some("software-engineering".into()),
            harness: Some("coding".into()),
            decision,
            resolution: LaneResolution {
                lane: Some(Tier::Cheap),
                adapter: "claude-code".into(),
                provider: Some("cc-1".into()),
                account: None,
                model_id: "model-cheap".into(),
                reasoning_effort: None,
                selection: None,
            },
            quota_reason: None,
            work_unit_id: None,
            optimizer: None,
        }),
    }
}

fn started(run_id: &str) -> Event {
    Event::WorkerStarted {
        run_id: run_id.into(),
        adapter: "claude-code".into(),
        model: "model-cheap".into(),
        provider: Some("cc-1".into()),
        account: None,
        role: None,
        task_role: None,
    }
}

fn finished(run_id: &str, cost: Option<f64>, end: RunEnd) -> Event {
    Event::WorkerFinished {
        run_id: run_id.into(),
        outcome: "done: ok".into(),
        usage: Some(Usage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cost_usd: cost,
            ..Usage::default()
        }),
        role: None,
        metrics: Some(RunMetrics {
            wall_ms: 1_800_000,
            retries: 1,
            peak_context_tokens: None,
            turns: None,
        }),
        end: Some(end),
    }
}

fn request(request_id: &str) -> Event {
    Event::RoutingRequestDecided {
        record: Box::new(RoutingRequestRecord {
            request_id: request_id.into(),
            decision_id: format!("proxy:{request_id}"),
            parent_decision_id: Some("run:r1".into()),
            run_id: Some("r1".into()),
            trace: None,
            attempts: vec![RequestSourceAttempt {
                source_id: "qwen-local".into(),
                model: Some("qwen".into()),
                account_id: None,
                fallback_reason: None,
            }],
            fallback_reason: None,
        }),
    }
}

fn review(pass: bool) -> Vec<Event> {
    let mut out = Vec::new();
    if !pass {
        out.push(Event::ReviewVerdict {
            run_id: "rev".into(),
            criterion_idx: 1,
            pass: false,
            reason: "missing".into(),
        });
    }
    out.push(Event::Transitioned {
        from: Status::Reviewing,
        to: if pass { Status::Done } else { Status::Ready },
        reason: if pass { "review_pass" } else { "review_fail" }.into(),
    });
    out
}

fn recorded(o: &RoutingOutcome) -> Event {
    Event::RoutingOutcomeRecorded {
        outcome: Box::new(o.clone()),
    }
}

fn approx(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

/// ADR §6・§10 Phase 3: review 未到着は reward=None、到着後 1 件、再投影は同じ、訂正は旧 outcome を
/// supersede。run の合否を各 request に複写しない。中断・欠測は null。
#[test]
fn routing_reward_waits_for_review_and_supersedes_idempotently() {
    let norm = RewardNormalization::default();
    let mut events = vec![
        started("r1"),
        routing_decided("r1"),
        request("req-1"),
        request("req-2"),
        finished("r1", Some(0.5), RunEnd::Completed),
    ];

    // review 未到着: run 1 件の outcome。review_passed・reward は null（false や 0 ではない）。
    let before = project_run_outcomes(&events, &norm);
    assert_eq!(before.len(), 1, "{before:?}");
    let pending = &before[0];
    assert_eq!(pending.decision_id, "run:r1");
    assert_eq!(pending.run_id.as_deref(), Some("r1"));
    assert_eq!(pending.review_passed, None);
    assert_eq!(pending.acceptance_passed, None);
    assert_eq!(pending.reward, None);
    assert_eq!(pending.supersedes, None);
    assert_eq!(pending.failure_class, None);
    assert_eq!(pending.tokens, Some(120));
    assert_eq!(pending.evaluation_version, ROUTING_OUTCOME_EVALUATION_VERSION);
    // run の合否を個々の request に複写しない。
    assert!(before.iter().all(|o| o.request_id.is_none()));
    // 再投影は同じ outcome_id。
    assert_eq!(project_run_outcomes(&events, &norm), before);
    events.push(recorded(pending));
    assert!(pending_run_outcomes(&events, &norm).is_empty());
    assert_eq!(project_run_outcomes(&events, &norm), before);

    // review 到着: 1 件だけ。reward = 1 - 0.2*0.5 - 0.1*0.5 - 0.1*0.25 = 0.825。旧 outcome を supersede。
    events.extend(review(true));
    let after = project_run_outcomes(&events, &norm);
    assert_eq!(after.len(), 1, "{after:?}");
    let passed = after[0].clone();
    assert_eq!(passed.review_passed, Some(true));
    assert!(approx(passed.reward, 0.825), "{:?}", passed.reward);
    assert_eq!(passed.supersedes.as_deref(), Some(pending.outcome_id.as_str()));
    assert_ne!(passed.outcome_id, pending.outcome_id);
    assert_eq!(pending_run_outcomes(&events, &norm), vec![passed.clone()]);
    // 再投影は同じ outcome_id（入力の順序・回数に依らない）。
    assert_eq!(project_run_outcomes(&events, &norm), after);
    events.push(recorded(&passed));
    assert!(pending_run_outcomes(&events, &norm).is_empty());
    assert_eq!(project_run_outcomes(&events, &norm), after);
    // 同じ event の二重追記でも二重に数えない（最新の記録と同じ結果ならそのまま）。
    events.push(recorded(&passed));
    assert_eq!(project_run_outcomes(&events, &norm), after);

    // 後の訂正（review の差し戻し）: 新しい outcome_id + supersedes。上書きしない。
    events.extend(review(false));
    let corrected = project_run_outcomes(&events, &norm);
    assert_eq!(corrected.len(), 1);
    let failed = &corrected[0];
    assert_eq!(failed.review_passed, Some(false));
    assert_eq!(failed.failure_class.as_deref(), Some("review_failed"));
    assert_eq!(failed.failed_criterion_ids, vec!["acceptance:1".to_string()]);
    assert!(approx(failed.reward, -0.175), "{:?}", failed.reward);
    assert_eq!(failed.supersedes.as_deref(), Some(passed.outcome_id.as_str()));
    assert!(failed.outcome_id != passed.outcome_id && failed.outcome_id != pending.outcome_id);
    events.push(recorded(failed));
    assert!(pending_run_outcomes(&events, &norm).is_empty());

    // 中断（cancel）は review_passed = None・reward = None。failure_class は中断の code。
    let cancelled = vec![
        started("r2"),
        routing_decided("r2"),
        finished("r2", Some(0.1), RunEnd::Cancelled),
    ];
    let c = project_run_outcomes(&cancelled, &norm);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].review_passed, None);
    assert_eq!(c[0].reward, None);
    assert_eq!(c[0].failure_class.as_deref(), Some("cancelled"));

    // 必要な実測（cash）が欠ければ review があっても reward = None。
    let mut missing = vec![
        started("r3"),
        routing_decided("r3"),
        finished("r3", None, RunEnd::Completed),
    ];
    missing.extend(review(true));
    let m = project_run_outcomes(&missing, &norm);
    assert_eq!(m[0].review_passed, Some(true));
    assert_eq!(m[0].reward, None);

    // RoutingDecided の無い導入前の run・未終了の run は outcome を作らない。
    let legacy = vec![started("r4"), finished("r4", Some(0.1), RunEnd::Completed)];
    assert!(project_run_outcomes(&legacy, &norm).is_empty());
    let running = vec![started("r5"), routing_decided("r5")];
    assert!(project_run_outcomes(&running, &norm).is_empty());
}

/// WU の受け入れ検査の不合格は review を待たずに pass=0 で確定する。合格だけでは未判定のまま。
#[test]
fn routing_outcome_reads_work_unit_checks_by_run() {
    let norm = RewardNormalization::default();
    let check = |pass: bool, index: u32| Event::WorkUnitCheckFinished {
        work_unit_id: "wu".into(),
        key: "k".into(),
        run_id: "r1".into(),
        index,
        total: 2,
        cmd: "true".into(),
        pass,
        exit: Some(if pass { 0 } else { 1 }),
        timed_out: false,
        duration_ms: 1,
    };
    let base = vec![
        started("r1"),
        routing_decided("r1"),
        finished("r1", Some(0.5), RunEnd::Completed),
        check(true, 0),
    ];
    let ok = project_run_outcomes(&base, &norm);
    assert_eq!(ok[0].acceptance_passed, Some(true));
    assert_eq!(ok[0].reward, None);
    let mut failed = base.clone();
    failed.push(check(false, 1));
    let f = project_run_outcomes(&failed, &norm);
    assert_eq!(f[0].acceptance_passed, Some(false));
    assert_eq!(f[0].failure_class.as_deref(), Some("acceptance_failed"));
    assert_eq!(f[0].failed_criterion_ids, vec!["check:1".to_string()]);
    assert!(approx(f[0].reward, -0.175), "{:?}", f[0].reward);
}

/// wire 名は ADR §6 の欄名が event に平たく並ぶ形。旧い（欄の欠けた）JSON・未知の欄も decode できる。
#[test]
fn routing_feedback_events_use_adr_wire_names_and_tolerate_missing_fields() {
    let outcome: Event = serde_json::from_value(serde_json::json!({
        "type": "routing_outcome_recorded",
        "outcome_id": "o1",
        "decision_id": "d1",
        "evaluation_version": "routing-outcome/1",
        "future_field": 1
    }))
    .unwrap();
    let Event::RoutingOutcomeRecorded { outcome } = &outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(outcome.reward, None);
    assert_eq!(outcome.review_passed, None);
    let json = serde_json::to_value(Event::RoutingOutcomeRecorded {
        outcome: outcome.clone(),
    })
    .unwrap();
    // 未判定は null のまま（欄が残る）。
    assert!(json["reward"].is_null() && json.get("reward").is_some());
    assert!(json.get("review_passed").is_some());

    let features: Event = serde_json::from_value(serde_json::json!({
        "type": "routing_features_recorded",
        "decision_id": "d1",
        "context_version": ROUTING_CONTEXT_VERSION,
        "features": {"role": "worker"},
        "provenance": {"role": "task.assignee"},
        "missing_fields": ["estimated_tokens"],
        "stage": "dispatch"
    }))
    .unwrap();
    let Event::RoutingFeaturesRecorded { record } = &features else {
        panic!("{features:?}")
    };
    assert_eq!(record.missing_fields, vec!["estimated_tokens".to_string()]);
    assert_eq!(record.stage, Some(FeatureStage::Dispatch));
    let round: Event = serde_json::from_value(serde_json::to_value(&features).unwrap()).unwrap();
    assert_eq!(round, features);

    let req: Event = serde_json::from_value(serde_json::json!({
        "type": "routing_request_decided",
        "request_id": "req-1",
        "decision_id": "proxy:req-1"
    }))
    .unwrap();
    let Event::RoutingRequestDecided { record } = &req else {
        panic!("{req:?}")
    };
    assert_eq!(record.parent_decision_id, None);
    assert!(record.attempts.is_empty());
}
