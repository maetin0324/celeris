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
        ..RoutingContext::default()
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
        source_id: None,
        model: None,
        account_id: None,
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

fn trace_with(candidates: Vec<super::trace::CandidateTrace>) -> super::trace::RoutingTraceV1 {
    super::trace::RoutingTraceV1 {
        decision_id: "d".into(),
        parent_decision_id: None,
        task_id: Some("t".into()),
        work_unit_id: None,
        run_id: Some("r".into()),
        request_id: None,
        stage: "dispatch".into(),
        mode: RoutingMode::Shadow,
        policy_version: "phase2-v1".into(),
        catalog_version: "c".into(),
        feature_version: "1".into(),
        estimator_version: "heuristic-1".into(),
        snapshot_id: "s".into(),
        observed_at: None,
        requested_lane: Tier::Cheap,
        selected_lane: Some(Tier::Cheap),
        candidates,
        selected: None,
        fallback_order: vec![],
        reasons: vec![],
        source_id: None,
        model: None,
        account_id: None,
    }
}

#[test]
fn routing_trace_fields_are_additive() {
    use super::trace::{CandidateTrace, ExcludedReason, ScoreTrace};
    const NEW_KEYS: [&str; 10] = [
        "\"config_order\"",
        "\"excluded_reason\"",
        "\"score_breakdown\"",
        "\"cash_usd\"",
        "\"shadow_usd\"",
        "\"resource_usd\"",
        "\"effective_usd\"",
        "\"source_id\"",
        "\"model\"",
        "\"account_id\"",
    ];
    // Phase 1 の形の JSON（新欄なし）が読め、新欄は None。
    let old_json = r#"{"decision_id":"d","parent_decision_id":null,"task_id":null,
        "work_unit_id":null,"run_id":"r","request_id":null,"stage":"dispatcher","mode":"legacy",
        "policy_version":"p","catalog_version":"c","feature_version":"f","estimator_version":"e",
        "snapshot_id":"s","observed_at":null,"requested_lane":"cheap","selected_lane":"cheap",
        "candidates":[{"model_profile_id":"m","deployment_id":"dep","eligible_provider_ids":["p"],
        "excluded_reasons":["cooldown"],"quality":null,"cost_usd":null,"latency_ms":null,
        "pressure":null,"score":null}],"selected":"p","fallback_order":["p"],"reasons":[]}"#;
    let old: super::trace::RoutingTraceV1 = serde_json::from_str(old_json).unwrap();
    assert_eq!(old.source_id, None);
    assert_eq!(old.model, None);
    assert_eq!(old.account_id, None);
    let c = &old.candidates[0];
    assert_eq!(c.excluded_reason, None);
    assert_eq!(c.score_breakdown, None);
    assert_eq!(c.config_order, None);
    assert!(c.cash_usd.is_none() && c.shadow_usd.is_none());
    assert!(c.resource_usd.is_none() && c.effective_usd.is_none());
    // 省略時は新欄を出さない: 旧 JSON は再直列化で同じ値に戻る（旧 event の replay が変わらない）。
    let reserialized = serde_json::to_value(&old).unwrap();
    let original: serde_json::Value = serde_json::from_str(old_json).unwrap();
    assert_eq!(reserialized, original);
    let text = reserialized.to_string();
    for key in NEW_KEYS {
        assert!(!text.contains(key), "{key} must be omitted when None");
    }
    // 旧 RoutingDecided event（optimizer の旧形）も同じ JSON に戻り、audit も変わらない。
    let task = crate::model_policy::tests::task("test", vec![]);
    let decision = crate::model_policy::decide_for_task(&task, &Default::default()).unwrap();
    let record = crate::model_policy::RoutingRecord {
        org_node: None,
        harness: None,
        decision,
        resolution: Default::default(),
        quota_reason: None,
        work_unit_id: None,
        optimizer: None,
    };
    let mut event = serde_json::to_value(Event::RoutingDecided {
        run_id: "r".into(),
        record: Box::new(record),
    })
    .unwrap();
    event["record"]["optimizer"] = original.clone();
    let parsed: Event = serde_json::from_value(event.clone()).unwrap();
    assert_eq!(serde_json::to_value(&parsed).unwrap(), event);
    let audit = crate::routing_audit::routing_audit(&task, std::slice::from_ref(&parsed));
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].optimizer.as_ref(), Some(&old));

    // 新欄を全部埋めた JSON は往復する。
    let estimate = super::cost::CostEstimate {
        cash_usd: Some(0.0),
        subscription_shadow_usd: Some(0.25),
        self_host_resource_usd: Some(0.0),
        effective_usd: Some(0.25),
        pressure: Some(0.5),
        assumptions: vec![],
    };
    let breakdown = super::cost::ScoreBreakdown {
        quality: 0.8,
        cost_term: 0.25,
        latency_term: 1.0,
        pressure_term: 0.5,
        unknown: vec!["latency_unknown"],
        score: 0.1,
    };
    let weights = RoutingPolicy::defaults(Tier::Cheap, RoutingMode::Shadow).weights;
    let mut full = CandidateTrace {
        model_profile_id: "m".into(),
        deployment_id: "dep".into(),
        eligible_provider_ids: vec!["p".into()],
        excluded_reasons: vec!["quota_exhausted".into()],
        config_order: Some(3),
        excluded_reason: Some(ExcludedReason::QuotaExhausted),
        score_breakdown: Some(ScoreTrace::new(&breakdown, &weights)),
        ..Default::default()
    }
    .with_cost(&estimate);
    full.score = Some(0.1);
    let mut new = trace_with(vec![
        full.clone(),
        CandidateTrace {
            model_profile_id: "m2".into(),
            deployment_id: "dep2".into(),
            excluded_reason: Some(ExcludedReason::Constraint {
                name: "privacy".into(),
            }),
            ..Default::default()
        },
    ]);
    new.source_id = Some("celeris".into());
    new.model = Some("qwen".into());
    new.account_id = Some("acct-1".into());
    let json = serde_json::to_string(&new).unwrap();
    for key in NEW_KEYS {
        assert!(json.contains(key), "{key} must be serialized when set");
    }
    let back: super::trace::RoutingTraceV1 = serde_json::from_str(&json).unwrap();
    assert_eq!(back, new);
    assert_eq!(back.candidates[0].shadow_usd, Some(0.25));
    assert_eq!(
        back.candidates[0].score_breakdown.as_ref().unwrap().unknown,
        vec!["latency_unknown".to_string()]
    );
    assert!(json.contains(r#""excluded_reason":{"kind":"constraint","name":"privacy"}"#));
}

#[test]
fn routing_candidate_reasons_serialize_stably() {
    use super::trace::{CandidateTrace, ExcludedReason};
    let cand = |id: &str, order: Option<usize>, reasons: &[&str]| CandidateTrace {
        model_profile_id: format!("m-{id}"),
        deployment_id: id.into(),
        eligible_provider_ids: vec![format!("p-{id}")],
        excluded_reasons: reasons.iter().map(|r| r.to_string()).collect(),
        config_order: order,
        ..Default::default()
    };
    let a = vec![
        cand("a", Some(2), &["rate_limit", "cooldown", "privacy"]),
        cand("b", Some(0), &[]),
        cand("c", Some(1), &["quota_exhausted", "concurrency"]),
        cand("legacy", None, &["context_unknown", "cooldown", "cooldown"]),
        cand("d", Some(1), &["lane"]),
    ];
    let mut b = a.clone();
    b.reverse();
    for c in &mut b {
        c.excluded_reasons.reverse();
    }
    let mut c = a.clone();
    c.rotate_left(2);
    let mut outputs = Vec::new();
    for candidates in [a, b, c] {
        let mut t = trace_with(candidates);
        t.normalize();
        outputs.push(serde_json::to_string(&t).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
    let t: super::trace::RoutingTraceV1 = serde_json::from_str(&outputs[0]).unwrap();
    let ids: Vec<_> = t
        .candidates
        .iter()
        .map(|c| c.deployment_id.as_str())
        .collect();
    // 設定順 → ID。設定順の無い候補は最後。
    assert_eq!(ids, ["b", "c", "d", "a", "legacy"]);
    assert_eq!(t.candidates[0].excluded_reason, None);
    assert_eq!(
        t.candidates[1].excluded_reason,
        Some(ExcludedReason::QuotaExhausted)
    );
    assert_eq!(
        t.candidates[3].excluded_reason,
        Some(ExcludedReason::Constraint {
            name: "privacy".into()
        })
    );
    assert_eq!(
        t.candidates[3].excluded_reasons,
        ["cooldown", "privacy", "rate_limit"]
    );
    assert_eq!(
        t.candidates[4].excluded_reasons,
        ["context_unknown", "cooldown"]
    );
    // 既知の理由コードは型付きの理由に写り、未知のコードは Other で残る。
    for code in ["cooldown", "rate_limit", "concurrency", "quota_exhausted"] {
        assert!(!matches!(
            ExcludedReason::from_code(code),
            ExcludedReason::Other { .. }
        ));
    }
    assert_eq!(
        ExcludedReason::from_code("brand_new"),
        ExcludedReason::Other {
            code: "brand_new".into()
        }
    );
}

#[test]
fn routing_context_ignores_unknown_fields() {
    let json = serde_json::json!({
        "version": "1",
        "origin": "test",
        "task_id": "t1",
        "org_node": "software-engineering",
        "role": "worker",
        "harness": "claude-code",
        "task_kind": "coding",
        "phase": "implementation",
        "acceptance_criteria": ["ac-0", "ac-1"],
        "required_tools": true,
        "required_tool_ids": ["bash"],
        "required_structured_output": false,
        "required_vision": false,
        "required_streaming": false,
        "environment": {"locality": "local", "host": null, "external_network": false},
        "input_tokens": 40,
        "output_reserve": 10,
        "safety_margin": 5,
        "attempts": 1,
        "review_failures": 0,
        "check_failures": 0,
        "priority": 3,
        "provenance": "test",
        "field_provenance": {"role": "task-dispatch"},
        "missing_fields": [],
        // Phase 3 より後の未知の欄。deny_unknown_fields を付けていないので無視して decode できる。
        "future_quantum_lane": {"nested": true},
    });
    let ctx: RoutingContext = serde_json::from_value(json).unwrap();
    assert_eq!(ctx.task_id, Some("t1".into()));
    assert_eq!(
        ctx.phase,
        Some(super::context::RoutingPhase::Implementation)
    );
    assert_eq!(ctx.acceptance_criteria, vec!["ac-0", "ac-1"]);
    assert_eq!(ctx.environment.locality, Some("local".into()));
    assert_eq!(ctx.attempts, Some(1));
    assert_eq!(
        ctx.field_provenance.get("role").map(String::as_str),
        Some("task-dispatch")
    );

    // 知らない phase の値も decode を失敗させず Unknown に読む。
    let unknown_phase: RoutingContext = serde_json::from_value(serde_json::json!({
        "version": "1",
        "origin": "test",
        "phase": "some_future_phase",
        "provenance": "test",
    }))
    .unwrap();
    assert_eq!(
        unknown_phase.phase,
        Some(super::context::RoutingPhase::Unknown)
    );
}

#[test]
fn routing_context_decodes_phase2_json() {
    // Phase 2 時点の RoutingContext（この欄の集合しか知らない旧 event/設定）を、
    // Phase 3 で足した欄なしで decode できることを固定する。
    let phase2_json = serde_json::json!({
        "version": "1",
        "origin": "test",
        "task_id": null,
        "work_unit_id": null,
        "run_id": null,
        "role": null,
        "harness": null,
        "task_kind": null,
        "required_tools": true,
        "required_structured_output": false,
        "required_vision": false,
        "required_streaming": false,
        "input_tokens": 40,
        "output_reserve": 10,
        "safety_margin": 5,
        "provenance": "test",
    });
    let ctx: RoutingContext = serde_json::from_value(phase2_json).unwrap();
    assert_eq!(ctx.version, "1");
    assert!(ctx.required_tools);
    assert_eq!(ctx.safety_margin, 5);
    // Phase 3 で足した欄は欠けていたので既定値（None / 空）になる。0 や false で埋めない。
    assert_eq!(ctx.org_node, None);
    assert_eq!(ctx.phase, None);
    assert!(ctx.acceptance_criteria.is_empty());
    assert_eq!(
        ctx.environment,
        super::context::RoutingEnvironment::default()
    );
    assert_eq!(ctx.attempts, None);
    assert_eq!(ctx.review_failures, None);
    assert_eq!(ctx.check_failures, None);
    assert_eq!(ctx.priority, None);
    assert!(ctx.field_provenance.is_empty());
    assert!(ctx.missing_fields.is_empty());
}

#[test]
fn routing_context_registry_expires_and_rejects_unknown_refs() {
    use super::context_registry::{
        ContextRefError, InMemoryRoutingContextRegistry, RoutingContextRegistry,
    };
    use std::time::{Duration, Instant};

    let registry = InMemoryRoutingContextRegistry::new();
    let base = Instant::now();
    let ctx = context();
    let reference = registry.register("run-1", ctx.clone(), Duration::from_secs(10), base);

    // 未知の ref は登録の有無に関わらず Invalid。
    assert_eq!(
        registry.resolve("ctx_does_not_exist", base),
        Err(ContextRefError::Invalid)
    );

    // ttl 内は in-flight の間ずっと解決できる。
    assert_eq!(
        registry.resolve(&reference, base + Duration::from_secs(5)),
        Ok(ctx.clone())
    );
    assert_eq!(
        registry.resolve(&reference, base + Duration::from_secs(9)),
        Ok(ctx.clone())
    );

    // ttl を過ぎたら Expired（Invalid と区別する）。
    assert_eq!(
        registry.resolve(&reference, base + Duration::from_secs(10)),
        Err(ContextRefError::Expired)
    );
    assert_eq!(
        registry.resolve(&reference, base + Duration::from_secs(99)),
        Err(ContextRefError::Expired)
    );

    // 別の run を登録し、in-flight 終了（release）で ttl 前でも即時に Invalid へ落とす。
    let reference2 = registry.register("run-2", ctx.clone(), Duration::from_secs(10), base);
    registry.release("run-2");
    assert_eq!(
        registry.resolve(&reference2, base + Duration::from_secs(1)),
        Err(ContextRefError::Invalid)
    );

    // release は指定した run だけに効く。他の run の ref は影響を受けない。
    assert_eq!(
        registry.resolve(&reference, base + Duration::from_secs(1)),
        Ok(ctx.clone())
    );

    // prune は期限切れの entry だけ数えて取り除く。
    let pruned = registry.prune(base + Duration::from_secs(10));
    assert_eq!(pruned, 1);
    assert_eq!(
        registry.resolve(&reference, base + Duration::from_secs(10)),
        Err(ContextRefError::Invalid)
    );
}
