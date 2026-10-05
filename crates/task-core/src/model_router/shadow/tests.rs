use time::OffsetDateTime;

use super::*;

fn at(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
}

fn target() -> ShadowTarget {
    ShadowTarget {
        task_kind: "coding".into(),
        role: "implementer".into(),
        lane: "cheap".into(),
        source: "qwen-local".into(),
    }
}

pub(crate) fn enabled_policy() -> ShadowPolicy {
    ShadowPolicy {
        execute: true,
        allowlist: ShadowAllowlist {
            task_kinds: vec!["coding".into()],
            roles: vec![SHADOW_ALLOW_ANY.into()],
            lanes: vec!["cheap".into()],
            sources: vec!["qwen-local".into()],
        },
        sample_rate: 1.0,
        daily_max_requests: Some(3),
        daily_max_tokens: Some(3_000),
        daily_max_effective_usd: Some(0.03),
        max_concurrency: Some(1),
        max_queue_depth: Some(4),
        timeout_ms: Some(10_000),
    }
}

#[test]
fn shadow_policy_default_is_off_and_valid() {
    let policy = ShadowPolicy::default();
    assert!(!policy.execute);
    assert_eq!(policy.validate(), Ok(()));
    assert_eq!(policy.daily_caps(), None);
    assert_eq!(policy.admit(&target(), "d1"), Err(ShadowReason::Off));
    // 空の TOML 相当の JSON からも既定 off。
    let parsed: ShadowPolicy = serde_json::from_str("{}").unwrap();
    assert_eq!(parsed, policy);
}

#[test]
fn shadow_policy_execute_requires_every_cap() {
    assert_eq!(enabled_policy().validate(), Ok(()));
    let mut p = enabled_policy();
    p.daily_max_tokens = None;
    assert_eq!(
        p.validate(),
        Err(ShadowPolicyError::Missing("daily_max_tokens"))
    );
    assert_eq!(p.daily_caps(), None);
    assert_eq!(p.admit(&target(), "d1"), Err(ShadowReason::Off));

    let mut p = enabled_policy();
    p.daily_max_effective_usd = Some(f64::NAN);
    assert!(matches!(
        p.validate(),
        Err(ShadowPolicyError::NotPositive("daily_max_effective_usd"))
    ));
    let mut p = enabled_policy();
    p.max_queue_depth = Some(0);
    assert_eq!(
        p.validate(),
        Err(ShadowPolicyError::NotPositive("max_queue_depth"))
    );
    let mut p = enabled_policy();
    p.sample_rate = 1.5;
    assert!(matches!(
        p.validate(),
        Err(ShadowPolicyError::SampleRate(_))
    ));
    let mut p = enabled_policy();
    p.allowlist = ShadowAllowlist::default();
    assert_eq!(p.validate(), Err(ShadowPolicyError::EmptyAllowlist));
}

#[test]
fn shadow_allowlist_empty_dimension_targets_nothing() {
    let mut p = enabled_policy();
    assert_eq!(p.admit(&target(), "d1"), Ok(()));
    p.allowlist.sources.clear();
    assert_eq!(p.validate(), Ok(()));
    assert_eq!(p.admit(&target(), "d1"), Err(ShadowReason::NotAllowlisted));
    let mut p = enabled_policy();
    let mut other = target();
    other.lane = "frontier".into();
    assert_eq!(p.admit(&other, "d1"), Err(ShadowReason::NotAllowlisted));
    p.allowlist.lanes.push(SHADOW_ALLOW_ANY.into());
    assert_eq!(p.admit(&other, "d1"), Ok(()));
}

#[test]
fn shadow_sampling_is_stable_and_bounded() {
    assert!(!sampled_in("x", 0.0));
    assert!(sampled_in("x", 1.0));
    assert!(!sampled_in("x", f64::NAN));
    let ids: Vec<String> = (0..2_000).map(|i| format!("decision-{i}")).collect();
    let first: Vec<bool> = ids.iter().map(|id| sampled_in(id, 0.25)).collect();
    let second: Vec<bool> = ids.iter().map(|id| sampled_in(id, 0.25)).collect();
    assert_eq!(first, second);
    let hits = first.iter().filter(|b| **b).count();
    assert!((400..600).contains(&hits), "hits = {hits}");
    // 率を上げると標本は単調に広がる（同じ id は外れない）。
    for id in &ids {
        if sampled_in(id, 0.25) {
            assert!(sampled_in(id, 0.5));
        }
    }
    let mut p = enabled_policy();
    p.sample_rate = 0.0;
    assert_eq!(p.admit(&target(), "d1"), Err(ShadowReason::SampledOut));
}

#[test]
fn shadow_record_wire_and_validation() {
    let record = ShadowRecord {
        shadow_id: "s1".into(),
        primary_decision_id: "d1".into(),
        kind: ShadowKind::Execution,
        status: ShadowStatus::Dropped,
        reason: Some(ShadowReason::ResourceGroupShared),
        detail: None,
        policy_version: "shadow-v1".into(),
        run_id: None,
        request_id: Some("r1".into()),
        candidate_model: Some("qwen3".into()),
        candidate_source: Some("qwen-local".into()),
        input_tokens: None,
        output_tokens: None,
        output_sha256: None,
        cash_usd: None,
        effective_usd: None,
        latency_ms: None,
        reservation_id: None,
    };
    assert_eq!(record.validate(), Ok(()));
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["reason"], "resource_group_shared");
    assert_eq!(json["status"], "dropped");
    assert_eq!(json["kind"], "execution");
    assert!(json.get("output_sha256").is_none());

    let mut done = record.clone();
    done.status = ShadowStatus::Completed;
    assert!(matches!(
        done.validate(),
        Err(ShadowRecordError::UnexpectedReason(_))
    ));
    done.reason = None;
    done.output_sha256 = Some(output_sha256(b"hello"));
    done.output_tokens = Some(5);
    assert_eq!(done.validate(), Ok(()));
    assert_eq!(
        done.output_sha256.as_deref(),
        Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
    );
    done.output_sha256 = Some("ABC".into());
    assert!(matches!(
        done.validate(),
        Err(ShadowRecordError::BadHash(_))
    ));

    let mut decision = record;
    decision.kind = ShadowKind::Decision;
    decision.status = ShadowStatus::Completed;
    decision.reason = None;
    assert_eq!(decision.validate(), Ok(()));
    decision.output_tokens = Some(1);
    assert!(matches!(
        decision.validate(),
        Err(ShadowRecordError::DecisionWithUsage(_))
    ));
}

#[test]
fn shadow_utc_day_and_micros() {
    assert_eq!(utc_day(at("2026-10-05T23:59:59Z")), "2026-10-05");
    // +09:00 の 10-06 08:00 は UTC で 10-05。
    assert_eq!(utc_day(at("2026-10-06T08:00:00+09:00")), "2026-10-05");
    assert_eq!(usd_to_micros_ceil(0.0100001), Some(10_001));
    assert_eq!(usd_to_micros_ceil(-1.0), None);
    assert_eq!(usd_to_micros_ceil(f64::INFINITY), None);
    assert_eq!(usd_cap_to_micros_floor(0.03), 30_000);
}
