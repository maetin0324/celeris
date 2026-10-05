use super::{
    context::RoutingContext,
    estimator::HeuristicEstimator,
    optimizer::{Candidate, optimize},
    policy::{RoutingMode, RoutingPolicy},
    profiles::*,
};
use crate::{Event, Tier};

fn model(id: &str, quality: Option<f64>) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        revision: "r1".into(),
        family: "test".into(),
        capabilities: Capabilities {
            tools: Support::Supported,
            structured_output: Support::Unknown,
            vision: Support::Unsupported,
            streaming: Support::Supported,
            reasoning_efforts: vec![],
        },
        context_limits: ContextLimits {
            input: Some(100),
            output: Some(20),
            total: Some(120),
        },
        quality: quality
            .map(|index| QualityIndex {
                domain: "general".into(),
                index,
                evaluation_version: "test".into(),
                samples: None,
                provenance: "test".into(),
            })
            .into_iter()
            .collect(),
        pricing: None,
        provenance: "test".into(),
    }
}
fn deployment(id: &str, model_id: &str, order: usize) -> DeploymentProfile {
    DeploymentProfile {
        id: id.into(),
        source_ref: id.into(),
        model_profile_id: model_id.into(),
        upstream_model: format!("{id}-wire"),
        adapter_constraints: vec![],
        billing: Billing::MeteredApi,
        host: None,
        region: None,
        trust_zone: None,
        external_network: true,
        retains_data: Some(false),
        allowed_lanes: vec![Tier::Standard],
        resource_group_id: None,
        concurrency_limit: None,
        rpm_limit: None,
        tpm_limit: None,
        price_override: None,
        config_order: order,
    }
}
fn context() -> RoutingContext {
    RoutingContext {
        version: "1".into(),
        origin: "test".into(),
        task_id: None,
        work_unit_id: None,
        run_id: None,
        role: None,
        harness: None,
        task_kind: None,
        required_tools: true,
        required_structured_output: false,
        required_vision: false,
        required_streaming: false,
        input_tokens: Some(40),
        output_reserve: Some(10),
        safety_margin: 5,
        provenance: "test".into(),
    }
}
fn candidate<'a>(m: &'a ModelProfile, d: &'a DeploymentProfile) -> Candidate<'a> {
    Candidate {
        model: m,
        deployment: d,
        state: None,
        eligible_provider_ids: vec![d.id.clone()],
        cost_usd: Some(0.1),
        latency_ms: Some(100.0),
        pressure: None,
    }
}

#[test]
fn routing_profile_shared_across_deployments() {
    let m = model("stable-id", Some(0.8));
    let a = deployment("a", &m.id, 0);
    let mut b = deployment("b", &m.id, 1);
    b.price_override = Some(TokenPricing {
        input_usd_per_million: Some(2.0),
        cached_input_usd_per_million: None,
        output_usd_per_million: None,
        as_of: None,
        provenance: "test".into(),
    });
    assert_eq!(a.model_profile_id, b.model_profile_id);
    assert_ne!(a.upstream_model, b.upstream_model);
    assert_ne!(a.price_override, b.price_override);
    let alias = model("other-id", Some(0.8));
    assert_ne!(alias.id, m.id);
    let bad = deployment("alias", &alias.id, 2);
    let policy = RoutingPolicy::defaults(Tier::Standard, RoutingMode::Enforce);
    let result = optimize(
        &policy,
        &context(),
        &[candidate(&m, &bad)],
        &HeuristicEstimator,
    )
    .unwrap();
    assert_eq!(result.outcome, "unroutable");
    assert!(
        result.traces[0]
            .excluded_reasons
            .contains(&"model_identity".into())
    );
}

#[test]
fn routing_kernel_constraints_before_score() {
    let good = model("good", Some(0.8));
    let mut no_tools = model("no-tools", Some(1.0));
    no_tools.capabilities.tools = Support::Unknown;
    let low = model("low", Some(0.4));
    let short = ModelProfile {
        context_limits: ContextLimits {
            input: Some(1),
            output: Some(20),
            total: Some(120),
        },
        ..model("short", Some(1.0))
    };
    let a = deployment("a", &good.id, 0);
    let b = deployment("b", &no_tools.id, 1);
    let c = deployment("c", &low.id, 2);
    let d = deployment("d", &short.id, 3);
    let mut private = deployment("private", &good.id, 4);
    private.retains_data = Some(true);
    let mut policy = RoutingPolicy::defaults(Tier::Standard, RoutingMode::Enforce);
    policy.constraints.data_retention_allowed = Some(false);
    let result = optimize(
        &policy,
        &context(),
        &[
            candidate(&good, &a),
            candidate(&no_tools, &b),
            candidate(&low, &c),
            candidate(&short, &d),
            candidate(&good, &private),
        ],
        &HeuristicEstimator,
    )
    .unwrap();
    assert_eq!(result.allowlist, vec!["a"]);
    assert!(
        result.traces[1]
            .excluded_reasons
            .contains(&"capability".into())
    );
    assert!(
        result.traces[2]
            .excluded_reasons
            .contains(&"quality_below_min".into())
    );
    assert!(
        result.traces[3]
            .excluded_reasons
            .contains(&"context".into())
    );
    assert!(
        result.traces[4]
            .excluded_reasons
            .contains(&"privacy".into())
    );
}

#[test]
fn routing_kernel_stable_ties_and_unknowns() {
    let m = model("m", Some(0.8));
    let unknown = model("unknown", None);
    let nan = model("nan", Some(f64::NAN));
    let a = deployment("a", &m.id, 1);
    let b = deployment("b", &m.id, 0);
    let c = deployment("c", &unknown.id, 2);
    let d = deployment("d", &nan.id, 3);
    let policy = RoutingPolicy::defaults(Tier::Standard, RoutingMode::Enforce);
    let x = optimize(
        &policy,
        &context(),
        &[
            candidate(&m, &a),
            candidate(&m, &b),
            candidate(&unknown, &c),
            candidate(&nan, &d),
        ],
        &HeuristicEstimator,
    )
    .unwrap();
    let y = optimize(
        &policy,
        &context(),
        &[
            candidate(&nan, &d),
            candidate(&unknown, &c),
            candidate(&m, &b),
            candidate(&m, &a),
        ],
        &HeuristicEstimator,
    )
    .unwrap();
    assert_eq!(x.allowlist, vec!["b", "a"]);
    assert_eq!(x.allowlist, y.allowlist);
    assert!(
        x.traces[2]
            .excluded_reasons
            .contains(&"quality_unknown".into())
    );
    assert!(
        x.traces[3]
            .excluded_reasons
            .contains(&"quality_unknown".into())
    );
    let mut bad = policy;
    bad.weights.cost = f64::NAN;
    assert!(bad.validate().is_err());
}

#[test]
fn routing_old_events_deserialize_without_optimizer() {
    let task = crate::model_policy::tests::task("test", vec![]);
    let decision = crate::model_policy::decide_for_task(&task, &Default::default()).unwrap();
    let mut record = crate::model_policy::RoutingRecord {
        org_node: None,
        harness: None,
        decision,
        resolution: Default::default(),
        quota_reason: None,
        work_unit_id: None,
        optimizer: None,
    };
    let old_event = Event::RoutingDecided {
        run_id: "r".into(),
        record: Box::new(record.clone()),
    };
    let json = serde_json::to_value(&old_event).unwrap();
    let old: Event = serde_json::from_value(json.clone()).unwrap();
    if let Event::RoutingDecided { record, .. } = &old {
        assert!(record.optimizer.is_none());
    } else {
        panic!("wrong event");
    }
    record.optimizer = Some(super::trace::RoutingTraceV1 {
        decision_id: "decision-1".into(),
        parent_decision_id: None,
        task_id: None,
        work_unit_id: None,
        run_id: Some("r".into()),
        request_id: None,
        stage: "dispatch".into(),
        mode: RoutingMode::Shadow,
        policy_version: "phase1-v1".into(),
        catalog_version: "catalog-1".into(),
        feature_version: "1".into(),
        estimator_version: "heuristic-1".into(),
        snapshot_id: "snapshot-1".into(),
        observed_at: None,
        requested_lane: Tier::Standard,
        selected_lane: Some(Tier::Standard),
        candidates: vec![],
        selected: None,
        fallback_order: vec![],
        reasons: vec![],
    });
    let new = Event::RoutingDecided {
        run_id: "r".into(),
        record: Box::new(record),
    };
    let new: Event = serde_json::from_value(serde_json::to_value(new).unwrap()).unwrap();
    let old_audit = crate::routing_audit::routing_audit(&task, &[old]);
    let new_audit = crate::routing_audit::routing_audit(&task, &[new]);
    // optimizer trace は ADR §9 のとおり task routing の optional trace。旧 event は None、
    // 新 event は Some(trace) を投影する。それ以外の欄は旧新で等しいことを確かめる。
    assert!(old_audit.iter().all(|a| a.optimizer.is_none()));
    assert!(new_audit.iter().all(|a| a.optimizer.is_some()));
    let strip = |mut audits: Vec<crate::routing_audit::RoutingAudit>| {
        for a in &mut audits {
            a.optimizer = None;
        }
        audits
    };
    assert_eq!(strip(old_audit), strip(new_audit));
    assert!(json.to_string().contains("routing_decided"));
}
