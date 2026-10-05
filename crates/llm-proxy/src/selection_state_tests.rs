use std::sync::Barrier;
use std::sync::atomic::{AtomicU32, Ordering};

use task_core::Tier;
use task_core::model_router::estimator::HeuristicEstimator;
use task_core::model_router::profiles::*;
use time::macros::datetime;

use super::*;
use crate::openai::{ChatMessage, MessageContent};
use crate::reservation::{CapacityLimits, ReserveError, reserve_with_reselect};

const NOW: OffsetDateTime = datetime!(2026-10-05 12:00 UTC);

fn model(id: &str, quality: f64, tools: Support) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        revision: "r1".into(),
        family: "test".into(),
        capabilities: Capabilities {
            tools,
            structured_output: Support::Unknown,
            vision: Support::Unsupported,
            streaming: Support::Supported,
            reasoning_efforts: vec![],
        },
        context_limits: ContextLimits {
            input: Some(1000),
            output: Some(1000),
            total: Some(2000),
        },
        quality: vec![QualityIndex {
            domain: "general".into(),
            index: quality,
            evaluation_version: "test".into(),
            samples: None,
            provenance: "test".into(),
        }],
        pricing: None,
        provenance: "test".into(),
    }
}

fn deployment(id: &str, model_id: &str, order: usize, group: Option<&str>) -> DeploymentProfile {
    DeploymentProfile {
        id: id.into(),
        source_ref: format!("src-{id}"),
        model_profile_id: model_id.into(),
        upstream_model: format!("{id}-wire"),
        adapter_constraints: vec![],
        billing: Billing::SelfHosted,
        host: None,
        region: None,
        trust_zone: None,
        external_network: false,
        retains_data: Some(false),
        allowed_lanes: vec![Tier::Cheap],
        resource_group_id: group.map(String::from),
        concurrency_limit: None,
        rpm_limit: None,
        tpm_limit: None,
        price_override: None,
        config_order: order,
    }
}

fn state(id: &str) -> SourceState {
    SourceState {
        reachability: Reachability::Up,
        latency_ms: Some(100.0),
        ..SourceState::unobserved(id)
    }
}

fn context() -> RoutingContext {
    let req = ChatCompletionRequest {
        model: "celeris/cheap".into(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: Some(MessageContent::Text("x".repeat(40))),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        temperature: None,
        top_p: None,
        max_tokens: Some(50),
        stop: None,
        stream: false,
        tools: None,
        tool_choice: None,
    };
    request_context(&req)
}

fn input<'a>(policy: &'a RoutingPolicy, ctx: &'a RoutingContext) -> StateSelectInput<'a> {
    StateSelectInput {
        policy,
        context: ctx,
        estimator: &HeuristicEstimator,
        usage: RequestUsage::default(),
        self_host: SelfHostRates::default(),
        freshness: FreshnessPolicy::default(),
    }
}

#[test]
fn request_context_takes_the_floor_from_request_fields() {
    let ctx = context();
    assert_eq!(ctx.input_tokens, Some(10));
    assert_eq!(ctx.output_reserve, Some(50));
    assert!(!ctx.required_tools);
}

#[test]
fn constraints_exclude_before_score_and_reasons_are_traced() {
    let m_ok = model("m-ok", 0.9, Support::Supported);
    let m_notools = model("m-notools", 0.9, Support::Unsupported);
    let m_low = model("m-low", 0.1, Support::Supported);
    let d_ok = deployment("ok", "m-ok", 0, None);
    let d_notools = deployment("notools", "m-notools", 1, None);
    let d_cool = deployment("cool", "m-ok", 2, None);
    let mut d_ext = deployment("ext", "m-ok", 3, None);
    d_ext.external_network = true;
    let d_low = deployment("low", "m-low", 4, None);
    let s_ok = state("ok");
    let s_notools = state("notools");
    let s_cool = SourceState {
        cooldown_until: Some("2026-10-05T12:10:00Z".into()),
        ..state("cool")
    };
    let s_ext = state("ext");
    let s_low = state("low");
    let cands = [
        (&m_ok, &d_ok, &s_ok),
        (&m_notools, &d_notools, &s_notools),
        (&m_ok, &d_cool, &s_cool),
        (&m_ok, &d_ext, &s_ext),
        (&m_low, &d_low, &s_low),
    ]
    .map(|(model, deployment, state)| StateCandidate {
        model,
        deployment,
        state,
        account_id: None,
    });
    let mut policy = shadow_policy(Tier::Cheap);
    policy.constraints.external_network_allowed = Some(false);
    let mut ctx = context();
    ctx.required_tools = true;
    let inp = input(&policy, &ctx);
    let sel = select_state(&inp, &cands, NOW, None).unwrap();
    assert_eq!(sel.outcome, "ranked");
    assert_eq!(
        sel.ranked
            .iter()
            .map(|p| p.deployment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["ok"]
    );
    let reason = |id: &str| {
        let t = sel.traces.iter().find(|t| t.deployment_id == id).unwrap();
        // 落ちた候補は score を持たない（制約は score より前）。
        if !t.excluded_reasons.is_empty() {
            assert!(t.score.is_none() && t.score_breakdown.is_none(), "{id}");
        }
        t.excluded_reasons.clone()
    };
    assert_eq!(reason("notools"), vec!["capability"]);
    assert_eq!(reason("cool"), vec!["cooldown"]);
    assert_eq!(reason("ext"), vec!["privacy"]);
    assert_eq!(reason("low"), vec!["quality_below_min"]);
    assert!(reason("ok").is_empty());

    // 同じ snapshot と時刻からは同じ決定と trace。
    let again = select_state(&inp, &cands, NOW, None).unwrap();
    assert_eq!(sel, again);
    let ids = TraceIds {
        decision_id: "dec-1".into(),
        snapshot_id: snapshot_id(&cands),
        ..TraceIds::default()
    };
    let t1 = routing_trace(&inp, &sel, sel.ranked.first(), &ids, "cat-1", NOW);
    let t2 = routing_trace(&inp, &again, again.ranked.first(), &ids, "cat-1", NOW);
    assert_eq!(
        serde_json::to_string(&t1).unwrap(),
        serde_json::to_string(&t2).unwrap()
    );
    assert_eq!(t1.selected.as_deref(), Some("ok"));
    assert_eq!(t1.source_id.as_deref(), Some("src-ok"));
    let corr = correlation(&t1);
    assert_eq!(corr.decision_id.as_deref(), Some("dec-1"));
    assert_eq!(corr.snapshot_id, Some(ids.snapshot_id.clone()));
}

#[test]
fn legacy_mode_does_not_use_state_selection() {
    let policy = RoutingPolicy::defaults(Tier::Cheap, RoutingMode::Legacy);
    let ctx = context();
    assert!(select_state(&input(&policy, &ctx), &[], NOW, None).is_err());
}

#[test]
fn routing_reservation_conflict_reselects_once() {
    let m_a = model("m-a", 0.9, Support::Supported);
    let m_b = model("m-b", 0.5, Support::Supported);
    let d_a = deployment("a", "m-a", 0, Some("gpu0"));
    let d_b = deployment("b", "m-b", 1, Some("gpu1"));
    let s_a = state("a");
    let s_b = state("b");
    let cands = [
        StateCandidate {
            model: &m_a,
            deployment: &d_a,
            state: &s_a,
            account_id: None,
        },
        StateCandidate {
            model: &m_b,
            deployment: &d_b,
            state: &s_b,
            account_id: None,
        },
    ];
    let policy = shadow_policy(Tier::Cheap);
    let ctx = context();
    let table = ReservationTable::new(CapacityLimits {
        per_account: None,
        resource_groups: [("gpu0".to_string(), 1), ("gpu1".to_string(), 1)].into(),
    });
    let barrier = Barrier::new(2);
    let calls = AtomicU32::new(0);
    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    let inp = input(&policy, &ctx);
                    reserve_with_reselect(&table, |attempt, t| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let sel = select_state(&inp, &cands, NOW, Some(t)).ok()?;
                        let pick = sel.ranked.into_iter().next();
                        if attempt == 0 {
                            // 両方が空の表を見て同じ候補を選んでから、予約で取り合う。
                            barrier.wait();
                        }
                        pick
                    })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut got: Vec<(String, u32)> = results
        .iter()
        .map(|r| {
            let r = r.as_ref().unwrap();
            (r.pick.deployment_id.clone(), r.reselects)
        })
        .collect();
    got.sort();
    // 1 つは a を初回で取り、もう 1 つは競合で 1 回だけ選び直して b を取る。
    assert_eq!(got, vec![("a".into(), 0), ("b".into(), 1)]);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(table.peak(&SlotKey::group("gpu0")), 1);
    assert_eq!(table.peak(&SlotKey::group("gpu1")), 1);
    drop(results);

    // 候補が a だけなら、負けた側は選び直しで候補が無く、それ以上は選び直さない。
    let only_a = &cands[..1];
    let barrier = Barrier::new(2);
    let calls = AtomicU32::new(0);
    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    let inp = input(&policy, &ctx);
                    reserve_with_reselect(&table, |attempt, t| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let sel = select_state(&inp, only_a, NOW, Some(t)).ok()?;
                        if attempt == 0 {
                            barrier.wait();
                        } else {
                            // 選び直しの trace には埋まった枠の理由が残る。
                            let t = &sel.traces[0];
                            assert_eq!(t.excluded_reasons, vec!["concurrency"]);
                            assert_eq!(sel.outcome, "defer");
                        }
                        sel.ranked.into_iter().next()
                    })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let lost: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert_eq!(ok, 1);
    assert_eq!(lost, vec![&ReserveError::NoCandidate { attempt: 1 }]);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(table.peak(&SlotKey::group("gpu0")), 1);
}

#[test]
fn routing_shared_gpu_capacity_not_double_counted() {
    let qwen_a = deployment("qwen-a", "m", 0, Some("gpu0"));
    let qwen_b = deployment("qwen-b", "m", 1, Some("gpu0"));
    let mut claude = deployment("claude", "m", 2, None);
    claude.source_ref = "claude-oauth".into();
    claude.billing = Billing::Subscription;
    // CLI/proxy の account を持ち、共有 GPU にも載る deployment（両方の枠を 1 つずつ取る）。
    let mut relay = deployment("relay", "m", 3, Some("gpu0"));
    relay.source_ref = "proxy-relay".into();
    let table = ReservationTable::new(CapacityLimits {
        per_account: Some(1),
        resource_groups: [("gpu0".to_string(), 3)].into(),
    });
    let gpu0 = SlotKey::group("gpu0");
    let acct_a = SlotKey::account("claude-oauth", "acct-a");

    let r1 = table.try_reserve(&slots_for(&qwen_a, None)).unwrap();
    let r2 = table
        .try_reserve(&slots_for(&claude, Some("acct-a")))
        .unwrap();
    // account 枠は GPU 枠に数えない。
    assert_eq!((table.held(&gpu0), table.held(&acct_a)), (1, 1));
    // 同じ group の別 deployment は同じ枠を共有する。
    let r3 = table.try_reserve(&slots_for(&qwen_b, None)).unwrap();
    assert_eq!(table.held(&gpu0), 2);
    // account と GPU の両方に載る要求は、それぞれの枠を 1 つずつ（GPU を二重に取らない）。
    let mut both = slots_for(&relay, Some("acct-x"));
    both.extend(slots_for(&relay, None));
    assert_eq!(both.len(), 3, "gpu0 appears twice before dedup");
    let r4 = table.try_reserve(&both).unwrap();
    assert_eq!(r4.slots().len(), 2);
    assert_eq!(table.held(&gpu0), 3);
    assert_eq!(table.held(&SlotKey::account("proxy-relay", "acct-x")), 1);
    // GPU が満杯でも、GPU に載らない account は取れる。account の満杯は GPU を止めない。
    let err = table.try_reserve(&slots_for(&qwen_a, None)).unwrap_err();
    assert_eq!(err.full, vec![gpu0.clone()]);
    let r5 = table
        .try_reserve(&slots_for(&claude, Some("acct-b")))
        .unwrap();
    let err = table
        .try_reserve(&slots_for(&claude, Some("acct-a")))
        .unwrap_err();
    assert_eq!(err.full, vec![acct_a.clone()]);
    assert_eq!(table.peak(&gpu0), 3);
    assert_eq!(table.peak(&acct_a), 1);
    drop((r1, r3));
    assert_eq!(table.held(&gpu0), 1);
    drop((r2, r4, r5));
    assert_eq!((table.held(&gpu0), table.held(&acct_a)), (0, 0));
}
