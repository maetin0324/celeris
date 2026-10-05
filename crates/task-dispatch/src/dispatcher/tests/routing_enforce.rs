//! multi-objective routing Phase 2（p2-dispatch-enforce）: enforce は quota で lane を下げず、同じ lane の
//! 別 source を先に試し、無ければ defer する。legacy は従来の `select_tier` の結果。制約を proxy に渡せ
//! ない経路は除外理由を残して候補から外す。時計は `set_now_unix_fn` で注入する。
use super::*;
use crate::dispatcher::DispatchRoutingSettings;
use task_core::model_router::{
    policy::{Constraints, RoutingMode},
    trace::ExcludedReason as TraceExcludedReason,
};
use task_core::model_routing::ModelBinding;

fn tiered(
    base: Arc<dyn WorkerAdapter>,
    prefix: &str,
    account: Option<&str>,
) -> Arc<dyn WorkerAdapter> {
    Arc::new(task_worker::tiered::TieredAdapter {
        base,
        models: [
            (Tier::Frontier, "frontier-id"),
            (Tier::Standard, "standard-id"),
            (Tier::Cheap, "cheap-id"),
        ]
        .into_iter()
        .map(|(tier, id)| {
            let id = format!("{prefix}{id}");
            (
                tier,
                ModelBinding {
                    name: id.clone(),
                    model_id: Some(id),
                    unavailable_reason: None,
                    reasoning_effort: None,
                },
            )
        })
        .collect(),
        account_id: account.map(str::to_owned),
        credential_error: None,
    })
}

struct Outcome {
    status: Status,
    started: Option<(Option<String>, String)>,
    record: Option<task_core::RoutingRecord>,
}

/// p1 = pool（アカウント a、5h/7d とも `utilization`）、`second` なら p2（非 pool・frontier も提供）。
async fn run_case(
    mode: RoutingMode,
    utilization: f64,
    second: bool,
    constraints: Constraints,
    p2_llm_source: bool,
) -> Outcome {
    let accounts = accounts_fixture();
    let mut book = AccountBook::load(&accounts.path().join(".celeris-usage.json"));
    let mut obs = usage_window(utilization, 90_000);
    obs.seven_day = obs.five_hour;
    book.record_observation("a", obs, ObservationSource::Run);
    book.save().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        ws.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.worker_hint.tier = Tier::Frontier;
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Human,
        ..Default::default()
    });
    store.insert(&task).unwrap();
    let pool = tiered(
        Arc::new(PoolAdapter {
            terminal_or_throttled: Ok(Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            }),
            delay: Duration::ZERO,
            observation: None,
            env: vec![],
            captured: Arc::new(StdMutex::new(Vec::new())),
            spawn_failure: false,
        }),
        "",
        Some("a"),
    );
    let other = second.then(|| {
        tiered(
            Arc::new(InstantAdapter {
                terminal: Terminal::Done {
                    summary: "ok".into(),
                    evidence: vec![],
                    usage: None,
                },
                delay: Duration::ZERO,
            }),
            "p2-",
            None,
        )
    });
    let mut d = pool_dispatcher(
        store.clone(),
        pool,
        other.map(|a| ("p2", a)),
        accounts.path().to_path_buf(),
        2,
        2,
    );
    d.set_now_unix_fn(Arc::new(|| 10_000));
    if p2_llm_source {
        let (tx, _) = tokio::sync::watch::channel(None);
        d.set_snapshot_publisher(crate::dispatcher::SnapshotPublisher {
            tx,
            instance_id: "test".into(),
            hostname: "test".into(),
            started_at: String::new(),
            tick_ms: 1000,
            providers: vec![
                serde_json::from_value(serde_json::json!({
                    "id": "p2", "adapter": "instant", "tiers": ["frontier", "standard", "cheap"],
                    "concurrency": 2, "model": "m", "in_use": 0,
                    "llm_source": {"source": "openai_compatible:p2", "origin": "explicit"}
                }))
                .unwrap(),
            ],
            provider_checks: Default::default(),
        });
    }
    d.set_dispatch_routing(DispatchRoutingSettings {
        mode,
        constraints,
        ..Default::default()
    });
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    let started = events.iter().find_map(|(_, e)| match e {
        Event::WorkerStarted {
            provider, model, ..
        } => Some((provider.clone(), model.clone())),
        _ => None,
    });
    Outcome {
        status: store.get(task.id).unwrap().unwrap().status,
        started,
        record: routing_record(&events),
    }
}

#[tokio::test]
async fn routing_enforce_quota_defers_without_lane_downgrade() {
    // legacy: 残量 20% の frontier は ADR-0069 の select_tier で standard に下がる（従来結果のまま）。
    for second in [false, true] {
        let legacy = run_case(
            RoutingMode::Legacy,
            0.8,
            second,
            Constraints::default(),
            false,
        )
        .await;
        assert_eq!(
            legacy.started,
            Some((Some("p1".into()), "standard-id".into())),
            "legacy second={second}"
        );
        let record = legacy.record.expect("legacy routing record");
        assert_eq!(record.resolution.lane, Some(Tier::Standard));
        assert_eq!(
            record.optimizer.as_ref().map(|t| t.mode),
            Some(RoutingMode::Legacy)
        );
    }

    // enforce: 同じ残量でも lane は frontier のまま、同じ lane の別 source（p2）へ。
    let enforce = run_case(
        RoutingMode::Enforce,
        0.8,
        true,
        Constraints::default(),
        false,
    )
    .await;
    assert_eq!(
        enforce.started,
        Some((Some("p2".into()), "p2-frontier-id".into()))
    );
    let record = enforce.record.expect("enforce routing record");
    assert_eq!(record.resolution.lane, Some(Tier::Frontier));
    assert!(
        !record
            .decision
            .reasons
            .iter()
            .any(|r| r.contains("quota layer lowered lane")),
        "{:?}",
        record.decision.reasons
    );
    let trace = record.optimizer.expect("enforce trace");
    assert_eq!(trace.mode, RoutingMode::Enforce);
    assert_eq!(trace.requested_lane, Tier::Frontier);
    assert_eq!(trace.selected_lane, Some(Tier::Frontier));
    assert_eq!(trace.source_id.as_deref(), Some("p2"));
    assert_eq!(trace.model.as_deref(), Some("p2-frontier-id"));
    let p1 = trace
        .candidates
        .iter()
        .find(|c| c.deployment_id == "p1")
        .expect("p1 candidate");
    assert_eq!(
        p1.excluded_reason,
        Some(TraceExcludedReason::QuotaExhausted)
    );
    assert!(
        p1.excluded_reasons
            .iter()
            .any(|r| r == "quota_low_for_lane"),
        "{:?}",
        p1.excluded_reasons
    );
    assert_eq!(trace.fallback_order, ["p2"]);

    // enforce: 同じ lane に別 source が無ければ defer（lane を下げず、run も始めない）。
    let deferred = run_case(
        RoutingMode::Enforce,
        0.8,
        false,
        Constraints::default(),
        false,
    )
    .await;
    assert_eq!(deferred.started, None);
    assert_eq!(deferred.status, Status::Ready);

    // enforce: 残量が十分なら pool の p1 を frontier のまま使う（同じ時計・同じ snapshot で同じ決定）。
    for _ in 0..2 {
        let healthy = run_case(
            RoutingMode::Enforce,
            0.1,
            true,
            Constraints::default(),
            false,
        )
        .await;
        assert_eq!(
            healthy.started,
            Some((Some("p1".into()), "frontier-id".into()))
        );
        let trace = healthy.record.and_then(|r| r.optimizer).unwrap();
        assert!(trace.candidates.iter().all(|c| c.excluded_reason.is_none()));
        assert_eq!(trace.account_id.as_deref(), Some("a"));
        assert!(trace.observed_at.is_some());
    }
}

#[tokio::test]
async fn routing_enforce_keeps_exclusion_reasons_for_constrained_routes() {
    // task/組織固有の制約（source の allowlist）を proxy 経由の p2 には渡せない → 除外理由を残す。
    let constraints = Constraints {
        allowed_sources: Some(vec!["p1".into()]),
        ..Default::default()
    };
    let out = run_case(RoutingMode::Enforce, 0.1, true, constraints.clone(), true).await;
    assert_eq!(out.started, Some((Some("p1".into()), "frontier-id".into())));
    let trace = out.record.and_then(|r| r.optimizer).expect("enforce trace");
    let p2 = trace
        .candidates
        .iter()
        .find(|c| c.deployment_id == "p2")
        .expect("constrained candidate is kept in the trace, not dropped");
    assert_eq!(
        p2.excluded_reason,
        Some(TraceExcludedReason::Constraint {
            name: "context_transport".into()
        })
    );
    assert!(
        p2.excluded_reasons
            .iter()
            .any(|r| r == "context_transport_unsupported"),
        "{:?}",
        p2.excluded_reasons
    );
    assert!(p2.excluded_reasons.iter().any(|r| r == "source"));
    assert_eq!(trace.fallback_order, ["p1"]);

    // legacy は制約を見ない（従来どおり）。
    let legacy = run_case(RoutingMode::Legacy, 0.1, true, constraints, true).await;
    assert_eq!(
        legacy.started,
        Some((Some("p1".into()), "frontier-id".into()))
    );
}
