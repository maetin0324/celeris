use super::*;

fn spec(id: &str, adapter: &str, tiers: &[Tier], concurrency: usize) -> ProviderSpec {
    ProviderSpec {
        id: id.to_string(),
        adapter: adapter.to_string(),
        tiers: tiers.to_vec(),
        concurrency,
        model: format!("{id}-model"),
    }
}

fn hint(tier: Tier, adapter: Option<&str>) -> WorkerHint {
    WorkerHint {
        tier,
        adapter: adapter.map(str::to_string),
    }
}

#[test]
fn unpinned_agent_work_never_uses_specialized_harnesses() {
    let policy = StaticPolicy::new(
        vec![
            spec("research", "paperqa", &[Tier::Standard], 2),
            spec("web", "local-deep-research", &[Tier::Standard], 2),
            spec("memory", "langmem", &[Tier::Standard], 2),
            spec("agent", "codex", &[Tier::Standard], 2),
        ],
        Duration::from_secs(5),
    );
    assert_eq!(
        policy
            .pick(&hint(Tier::Standard, None), Instant::now())
            .unwrap()
            .1,
        "agent"
    );
    assert_eq!(
        policy
            .pick(&hint(Tier::Standard, Some("paperqa")), Instant::now())
            .unwrap()
            .1,
        "research"
    );
    assert_eq!(
        policy.select(
            &hint(Tier::Standard, None),
            Instant::now(),
            &["agent".into()].into()
        ),
        Selection::Busy
    );
}

#[test]
fn picks_first_matching_by_priority() {
    let policy = StaticPolicy::new(
        vec![
            spec("p1", "claude-code", &[Tier::Frontier], 2),
            spec("p2", "codex", &[Tier::Frontier], 2),
        ],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, None), now),
        Some(("claude-code".to_string(), "p1".to_string()))
    );
}

#[test]
fn filters_by_adapter_hint() {
    let policy = StaticPolicy::new(
        vec![
            spec("p1", "claude-code", &[Tier::Frontier], 2),
            spec("p2", "codex", &[Tier::Frontier], 2),
        ],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, Some("codex")), now),
        Some(("codex".to_string(), "p2".to_string()))
    );
    assert_eq!(policy.pick(&hint(Tier::Frontier, Some("dsh")), now), None);
}

#[test]
fn filters_by_tier() {
    let policy = StaticPolicy::new(
        vec![
            spec("p1", "claude-code", &[Tier::Standard], 2),
            spec("p2", "codex", &[Tier::Frontier], 2),
        ],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, None), now),
        Some(("codex".to_string(), "p2".to_string()))
    );
}

#[test]
fn throttled_provider_is_skipped_until_retry_after() {
    let mut policy = StaticPolicy::new(
        vec![
            spec("p1", "claude-code", &[Tier::Frontier], 2),
            spec("p2", "codex", &[Tier::Frontier], 2),
        ],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    policy.report(
        "p1".to_string(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(10),
        },
    );
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, None), now),
        Some(("codex".to_string(), "p2".to_string()))
    );
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, None), now + Duration::from_secs(11)),
        Some(("claude-code".to_string(), "p1".to_string()))
    );
}

#[test]
fn throttled_only_provider_yields_none() {
    let mut policy = StaticPolicy::new(
        vec![spec("p1", "claude-code", &[Tier::Frontier], 2)],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    policy.report(
        "p1".to_string(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(10),
        },
    );
    assert_eq!(policy.pick(&hint(Tier::Frontier, None), now), None);
}

#[test]
fn auth_failed_uses_error_cooldown() {
    let mut policy = StaticPolicy::new(
        vec![
            spec("p1", "claude-code", &[Tier::Frontier], 2),
            spec("p2", "codex", &[Tier::Frontier], 2),
        ],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    policy.report("p1".to_string(), &ProviderOutcome::AuthFailed);
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, None), now),
        Some(("codex".to_string(), "p2".to_string()))
    );
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, None), now + Duration::from_secs(6)),
        Some(("claude-code".to_string(), "p1".to_string()))
    );
}

#[test]
fn exhausted_uses_error_cooldown() {
    let mut policy = StaticPolicy::new(
        vec![spec("p1", "claude-code", &[Tier::Frontier], 2)],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    policy.report("p1".to_string(), &ProviderOutcome::Exhausted);
    assert_eq!(policy.pick(&hint(Tier::Frontier, None), now), None);
    assert_eq!(
        policy.pick(&hint(Tier::Frontier, None), now + Duration::from_secs(6)),
        Some(("claude-code".to_string(), "p1".to_string()))
    );
}

#[test]
fn ok_does_not_clear_cooldown() {
    let mut policy = StaticPolicy::new(
        vec![spec("p1", "claude-code", &[Tier::Frontier], 2)],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    policy.report(
        "p1".to_string(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(10),
        },
    );
    policy.report("p1".to_string(), &ProviderOutcome::Ok);
    assert_eq!(policy.pick(&hint(Tier::Frontier, None), now), None);
}

#[test]
fn report_unknown_provider_does_not_panic() {
    let mut policy = StaticPolicy::new(Vec::new(), Duration::from_secs(5));
    policy.report("unknown".to_string(), &ProviderOutcome::AuthFailed);
    policy.report(
        "unknown".to_string(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(1),
        },
    );
}

#[test]
fn concurrency_limit_returns_spec_value_or_zero() {
    let policy = StaticPolicy::new(
        vec![spec("p1", "claude-code", &[Tier::Frontier], 3)],
        Duration::from_secs(5),
    );
    assert_eq!(policy.concurrency_limit("p1".to_string()), 3);
    assert_eq!(policy.concurrency_limit("unknown".to_string()), 0);
}

/// ADR-0012 D2（P-20）: 除外されたプロバイダ（並列度の上限）と cooldown 中のプロバイダを飛ばして次の行へフォールバックする。
#[test]
fn select_falls_back_past_excluded_and_cooling_providers() {
    let mut policy = StaticPolicy::new(
        vec![
            spec("acct-a", "claude-code", &[Tier::Standard], 1),
            spec("acct-b", "claude-code", &[Tier::Standard], 1),
            spec("acct-c", "claude-code", &[Tier::Standard], 1),
        ],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    let h = hint(Tier::Standard, Some("claude-code"));
    let picked = |p: &str| Selection::Picked {
        adapter: "claude-code".into(),
        provider: p.into(),
    };
    assert_eq!(policy.select(&h, now, &HashSet::new()), picked("acct-a"));
    let excluded: HashSet<ProviderId> = ["acct-a".to_string()].into();
    assert_eq!(policy.select(&h, now, &excluded), picked("acct-b"));
    policy.report(
        "acct-b".into(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(60),
        },
    );
    assert_eq!(policy.select(&h, now, &excluded), picked("acct-c"));
    let all: HashSet<ProviderId> = ["acct-a".to_string(), "acct-c".to_string()].into();
    assert_eq!(policy.select(&h, now, &all), Selection::Busy);
}

/// ADR-0012 D2（P-33）: 設定に合う行が無い場合は `NoMatchingProvider`、合う行が cooldown 中なら `Busy`。
#[test]
fn select_distinguishes_no_matching_provider_from_busy() {
    let mut policy = StaticPolicy::new(
        vec![spec("p1", "codex", &[Tier::Frontier], 1)],
        Duration::from_secs(5),
    );
    let now = Instant::now();
    assert_eq!(
        policy.select(&hint(Tier::Standard, None), now, &HashSet::new()),
        Selection::NoMatchingProvider
    );
    assert_eq!(
        policy.select(
            &hint(Tier::Frontier, Some("claude-code")),
            now,
            &HashSet::new()
        ),
        Selection::NoMatchingProvider
    );
    policy.report("p1".into(), &ProviderOutcome::Exhausted);
    assert_eq!(
        policy.select(&hint(Tier::Frontier, None), now, &HashSet::new()),
        Selection::Busy
    );
}

/// ADR-0013 D4: `cooldowns` は期限内のものだけを理由つきで返し、期限が過ぎたものは返さない。
#[test]
fn cooldowns_lists_active_cooldowns_with_reasons() {
    let mut policy = StaticPolicy::new(
        vec![
            spec("a", "claude-code", &[Tier::Standard], 1),
            spec("b", "claude-code", &[Tier::Standard], 1),
            spec("c", "codex", &[Tier::Standard], 1),
        ],
        Duration::from_secs(300),
    );
    let now = Instant::now();
    assert!(policy.cooldowns(now).is_empty());
    policy.report(
        "b".into(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(60),
        },
    );
    policy.report("a".into(), &ProviderOutcome::AuthFailed);
    policy.report("c".into(), &ProviderOutcome::Exhausted);
    let active = policy.cooldowns(Instant::now());
    assert_eq!(
        active
            .iter()
            .map(|c| (c.provider.as_str(), c.reason))
            .collect::<Vec<_>>(),
        vec![
            ("a", CooldownReason::AuthFailed),
            ("b", CooldownReason::Throttled),
            ("c", CooldownReason::Exhausted)
        ]
    );
    // throttled（60 秒）だけが先に切れる。
    let later = Instant::now() + Duration::from_secs(120);
    assert_eq!(
        policy
            .cooldowns(later)
            .iter()
            .map(|c| c.provider.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "c"]
    );
}

/// `select` を実装しない既存のポリシー（供給層）は、`pick` からの既定実装で従来どおり動く。
#[test]
fn default_select_is_derived_from_pick() {
    struct PickOnly;
    impl ProviderPolicy for PickOnly {
        fn pick(&self, _hint: &WorkerHint, _now: Instant) -> Option<(AdapterId, ProviderId)> {
            Some(("fake".into(), "only".into()))
        }
        fn report(&mut self, _provider: ProviderId, _outcome: &ProviderOutcome) {}
        fn concurrency_limit(&self, _provider: ProviderId) -> usize {
            1
        }
    }
    let now = Instant::now();
    let h = hint(Tier::Standard, None);
    assert_eq!(
        PickOnly.select(&h, now, &HashSet::new()),
        Selection::Picked {
            adapter: "fake".into(),
            provider: "only".into()
        }
    );
    assert_eq!(
        PickOnly.select(&h, now, &["only".to_string()].into()),
        Selection::Busy
    );
}

/// ADR-0132 付記 L2 (a): `offers` は hint（tier・adapter の固定・専用アダプタの除外）だけを見て、
/// cooldown は見ない。`adapter_of` は設定行のアダプタ名を返す。
#[test]
fn static_policy_offers_matches_hint_without_cooldown() {
    let mut policy = StaticPolicy::new(
        vec![
            spec("qwen", "acp", &[Tier::Cheap], 1),
            spec("research", "paperqa", &[Tier::Cheap], 1),
        ],
        Duration::from_secs(60),
    );
    assert!(policy.offers("qwen", &hint(Tier::Cheap, None)));
    assert!(policy.offers("qwen", &hint(Tier::Cheap, Some("acp"))));
    assert!(!policy.offers("qwen", &hint(Tier::Standard, None)));
    assert!(!policy.offers("qwen", &hint(Tier::Cheap, Some("claude-code"))));
    assert!(!policy.offers("research", &hint(Tier::Cheap, None)));
    assert!(!policy.offers("missing", &hint(Tier::Cheap, None)));
    policy.report("qwen".into(), &ProviderOutcome::AuthFailed);
    assert!(policy.offers("qwen", &hint(Tier::Cheap, None)));
    assert_eq!(policy.adapter_of("qwen").as_deref(), Some("acp"));
    assert_eq!(policy.adapter_of("missing"), None);
}
