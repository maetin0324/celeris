//! ADR-0132 付記 L2〜L8: cheap lane のローカル優先（provider 選択の前段と `RoutingDecided.selection`）。
//! probe は偽物に差し替え、ネットワークには出ない。

use super::*;
use task_core::model_routing::{
    ProviderCandidateKind, ProviderCandidateOutcome, ProviderSelection, ProviderSelectionReason,
};

const QWEN_URL: &str = "http://qwen.invalid/v1";

fn qwen_spec() -> crate::dispatcher::LocalProviderSpec {
    crate::dispatcher::LocalProviderSpec {
        provider: "qwen".into(),
        health: vec![crate::dispatcher::LocalHealthTarget {
            base_url: QWEN_URL.into(),
            bearer_token: None,
        }],
    }
}

/// 呼ばれた回数を数え、毎回 `outcome` を返す偽の probe。
fn counting_probe(
    outcome: Reachability,
) -> (crate::dispatcher::LocalProviderProbe, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let probe: crate::dispatcher::LocalProviderProbe = Arc::new(move |url, _| {
        assert_eq!(url, QWEN_URL);
        counter.fetch_add(1, Ordering::SeqCst);
        outcome.clone()
    });
    (probe, calls)
}

/// プールの `p1`（claude-code、cheap + standard）とローカルの `qwen`（acp、`qwen_tiers`、concurrency 1）。
fn local_first_dispatcher(
    claude: &tempfile::TempDir,
    qwen_tiers: Vec<Tier>,
    probe: crate::dispatcher::LocalProviderProbe,
) -> Dispatcher {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = pool_dispatcher(store, adapter, None, claude.path().into(), 2, 2);
    d.now_unix_fn = Arc::new(|| 10_000);
    d.policy = Box::new(StaticPolicy::new(
        vec![
            ProviderSpec {
                id: "p1".into(),
                adapter: "claude-code".into(),
                tiers: vec![Tier::Cheap, Tier::Standard],
                concurrency: 2,
                model: String::new(),
            },
            ProviderSpec {
                id: "qwen".into(),
                adapter: "acp".into(),
                tiers: qwen_tiers,
                concurrency: 1,
                model: String::new(),
            },
        ],
        Duration::from_secs(5),
    ));
    for id in ["a", "b"] {
        d.record_account_check(
            AccountAdapter::ClaudeCode,
            id,
            "ok",
            None,
            Some(usage_window(0.2, 3600)),
        );
    }
    d.set_local_providers(vec![qwen_spec()]);
    d.set_local_provider_probe(probe);
    d
}

fn hint(tier: Tier, adapter: Option<&str>) -> WorkerHint {
    WorkerHint {
        tier,
        adapter: adapter.map(str::to_string),
    }
}

fn local_outcome(selection: &ProviderSelection) -> Option<ProviderCandidateOutcome> {
    selection
        .candidates
        .iter()
        .find(|c| c.provider == "qwen" && c.kind == ProviderCandidateKind::Local)
        .map(|c| c.outcome)
}

#[test]
fn cheap_local_first_picks_local_when_free() {
    let claude = accounts_fixture();
    let (probe, calls) = counting_probe(Reachability::Ok);
    let mut d = local_first_dispatcher(&claude, vec![Tier::Cheap], probe);
    let mut full = std::collections::HashSet::new();
    let (picked, selection) = d.select_provider_for(
        &hint(Tier::Cheap, None),
        Instant::now(),
        TaskId::new(),
        &mut full,
        None,
        false,
        true,
    );
    let (adapter, provider, account) = picked.unwrap();
    assert_eq!(
        (adapter.as_str(), provider.as_str()),
        ("acp", "qwen"),
        "{selection:?}"
    );
    assert!(account.is_none());
    assert_eq!(selection.reason, ProviderSelectionReason::LocalPreferred);
    assert_eq!(
        local_outcome(&selection),
        Some(ProviderCandidateOutcome::Selected)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(full.is_empty());
}

#[test]
fn cheap_local_first_falls_to_pool_when_local_full() {
    let claude = accounts_fixture();
    let (probe, calls) = counting_probe(Reachability::Ok);
    let mut d = local_first_dispatcher(&claude, vec![Tier::Cheap], probe);
    // この tick で qwen の枠（concurrency 1）は埋まっている。
    let mut full: std::collections::HashSet<ProviderId> = ["qwen".to_string()].into();
    let (picked, selection) = d.select_provider_for(
        &hint(Tier::Cheap, None),
        Instant::now(),
        TaskId::new(),
        &mut full,
        None,
        false,
        true,
    );
    let (_, provider, account) = picked.unwrap();
    assert_eq!(provider, "p1", "{selection:?}");
    assert!(account.is_some());
    assert_eq!(selection.reason, ProviderSelectionReason::LocalFull);
    assert_eq!(
        local_outcome(&selection),
        Some(ProviderCandidateOutcome::Full)
    );
    assert!(selection.candidates.iter().any(|c| c.provider == "p1"
        && c.kind == ProviderCandidateKind::Pool
        && c.outcome == ProviderCandidateOutcome::Selected));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a full local is not probed"
    );

    // concurrency 0（走っている run で埋まったのと同じ判定）でも満杯として扱う。
    let (probe, calls) = counting_probe(Reachability::Ok);
    let mut d = local_first_dispatcher(&claude, vec![Tier::Cheap], probe);
    d.policy = Box::new(StaticPolicy::new(
        vec![
            ProviderSpec {
                id: "p1".into(),
                adapter: "claude-code".into(),
                tiers: vec![Tier::Cheap],
                concurrency: 2,
                model: String::new(),
            },
            ProviderSpec {
                id: "qwen".into(),
                adapter: "acp".into(),
                tiers: vec![Tier::Cheap],
                concurrency: 0,
                model: String::new(),
            },
        ],
        Duration::from_secs(5),
    ));
    let mut full = std::collections::HashSet::new();
    let (picked, selection) = d.select_provider_for(
        &hint(Tier::Cheap, None),
        Instant::now(),
        TaskId::new(),
        &mut full,
        None,
        false,
        true,
    );
    assert_eq!(picked.unwrap().1, "p1");
    assert_eq!(selection.reason, ProviderSelectionReason::LocalFull);
    assert!(full.contains("qwen"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cheap_local_first_falls_to_pool_when_local_down() {
    let claude = accounts_fixture();
    let (probe, calls) = counting_probe(Reachability::Unreachable {
        reason: "connection refused".into(),
    });
    let mut d = local_first_dispatcher(&claude, vec![Tier::Cheap], probe);
    let now = Instant::now();
    let task = TaskId::new();
    let h = hint(Tier::Cheap, None);
    let mut full = std::collections::HashSet::new();
    let (picked, selection) = d.select_provider_for(&h, now, task, &mut full, None, false, true);
    assert_eq!(picked.unwrap().1, "p1", "{selection:?}");
    assert_eq!(selection.reason, ProviderSelectionReason::LocalDown);
    let local = selection
        .candidates
        .iter()
        .find(|c| c.provider == "qwen")
        .unwrap();
    assert_eq!(local.outcome, ProviderCandidateOutcome::Down);
    assert!(
        local
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("connection refused")),
        "{local:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(full.is_empty(), "a down local is not a full provider");

    // 60 秒以内はキャッシュを使う。
    let (picked, _) = d.select_provider_for(
        &h,
        now + Duration::from_secs(30),
        task,
        &mut full,
        None,
        false,
        true,
    );
    assert_eq!(picked.unwrap().1, "p1");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 期限を過ぎたら probe し直す。
    let later = now + Duration::from_secs(61);
    let (picked, selection) = d.select_provider_for(&h, later, task, &mut full, None, false, true);
    assert_eq!(picked.unwrap().1, "p1");
    assert_eq!(selection.reason, ProviderSelectionReason::LocalDown);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // プールも使えないときに、不通のローカルを fallback として返さない。
    let mut pool_full: std::collections::HashSet<ProviderId> = ["p1".to_string()].into();
    let (picked, selection) =
        d.select_provider_for(&h, later, task, &mut pool_full, None, false, true);
    assert!(picked.is_none(), "{selection:?}");
    assert_eq!(selection.reason, ProviderSelectionReason::LocalDown);
    assert!(!d.unroutable.contains(&task));
}

#[test]
fn cheap_local_first_standard_lane_is_unchanged() {
    let claude = accounts_fixture();
    let (probe, calls) = counting_probe(Reachability::Ok);
    let mut d = local_first_dispatcher(&claude, vec![Tier::Cheap, Tier::Standard], probe);
    let now = Instant::now();
    let task = TaskId::new();
    let mut full = std::collections::HashSet::new();
    let (picked, selection) = d.select_provider_for(
        &hint(Tier::Standard, None),
        now,
        task,
        &mut full,
        None,
        false,
        true,
    );
    let (_, provider, account) = picked.unwrap();
    assert_eq!(provider, "p1", "{selection:?}");
    assert!(account.is_some());
    assert_eq!(selection.reason, ProviderSelectionReason::Pool);
    // 順位付けで見たローカルの行は local として記録される（選び方は従来どおり）。
    assert_eq!(
        local_outcome(&selection),
        Some(ProviderCandidateOutcome::Available)
    );

    // reviewer の経路（`select_provider`）は cheap でもローカル優先を使わない。
    let picked = d
        .select_provider(&hint(Tier::Cheap, None), now, task, &mut full, None)
        .unwrap();
    assert_eq!(picked.1, "p1");
    // CoS の対話 run も同じ。
    let (picked, _) = d.select_provider_for(
        &hint(Tier::Cheap, None),
        now,
        task,
        &mut full,
        None,
        true,
        true,
    );
    assert_eq!(picked.unwrap().1, "p1");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cheap_local_first_unsupported_adapter_goes_to_pool() {
    let claude = accounts_fixture();
    let (probe, calls) = counting_probe(Reachability::Ok);
    let mut d = local_first_dispatcher(&claude, vec![Tier::Cheap], probe);
    let mut full = std::collections::HashSet::new();
    let (picked, selection) = d.select_provider_for(
        &hint(Tier::Cheap, Some("claude-code")),
        Instant::now(),
        TaskId::new(),
        &mut full,
        None,
        false,
        true,
    );
    assert_eq!(picked.unwrap().1, "p1", "{selection:?}");
    assert_eq!(selection.reason, ProviderSelectionReason::Pool);
    assert_eq!(
        local_outcome(&selection),
        Some(ProviderCandidateOutcome::Unsupported)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// A policy with the old trait surface is the control path: it has no kernel
/// candidate enumeration, but forwards every live decision to StaticPolicy.
struct LegacyControlPolicy(StaticPolicy);

impl crate::policy::ProviderPolicy for LegacyControlPolicy {
    fn pick(&self, hint: &WorkerHint, now: Instant) -> Option<(AdapterId, ProviderId)> {
        self.0.pick(hint, now)
    }
    fn report(&mut self, provider: ProviderId, outcome: &crate::policy::ProviderOutcome) {
        self.0.report(provider, outcome)
    }
    fn concurrency_limit(&self, provider: ProviderId) -> usize {
        self.0.concurrency_limit(provider)
    }
    fn select(
        &self,
        hint: &WorkerHint,
        now: Instant,
        excluded: &std::collections::HashSet<ProviderId>,
    ) -> crate::policy::Selection {
        self.0.select(hint, now, excluded)
    }
    fn cooldowns(&self, now: Instant) -> Vec<crate::policy::Cooldown> {
        self.0.cooldowns(now)
    }
    fn offers(&self, provider: &str, hint: &WorkerHint) -> bool {
        self.0.offers(provider, hint)
    }
    fn adapter_of(&self, provider: &str) -> Option<AdapterId> {
        self.0.adapter_of(provider)
    }
}

#[test]
fn routing_dispatch_legacy_equivalence_matrix() {
    let claude = accounts_fixture();
    let now = Instant::now();
    for lane in [Tier::Cheap, Tier::Standard, Tier::Frontier] {
        for (role, cos, prefer_local) in [
            ("worker", false, true),
            ("reviewer", false, false),
            ("cos", true, true),
            ("worker_no_local", false, false),
        ] {
            for sticky in [false, true] {
                for supply in ["free", "local_full", "local_down", "pool_full", "all_full"] {
                    let reachability = if supply == "local_down" {
                        Reachability::Unreachable {
                            reason: "offline".into(),
                        }
                    } else {
                        Reachability::Ok
                    };
                    let run = |control: bool| {
                        let (probe, _) = counting_probe(reachability.clone());
                        let mut d = local_first_dispatcher(
                            &claude,
                            vec![Tier::Cheap, Tier::Standard],
                            probe,
                        );
                        if control {
                            d.policy = Box::new(LegacyControlPolicy(StaticPolicy::new(
                                vec![
                                    ProviderSpec {
                                        id: "p1".into(),
                                        adapter: "claude-code".into(),
                                        tiers: vec![Tier::Cheap, Tier::Standard],
                                        concurrency: 2,
                                        model: String::new(),
                                    },
                                    ProviderSpec {
                                        id: "qwen".into(),
                                        adapter: "acp".into(),
                                        tiers: vec![Tier::Cheap, Tier::Standard],
                                        concurrency: 1,
                                        model: String::new(),
                                    },
                                ],
                                Duration::from_secs(5),
                            )));
                        }
                        let mut full: std::collections::HashSet<ProviderId> = match supply {
                            "local_full" => ["qwen".into()].into(),
                            "pool_full" => ["p1".into()].into(),
                            "all_full" => ["qwen".into(), "p1".into()].into(),
                            _ => Default::default(),
                        };
                        let session = sticky.then(|| {
                            NodeSession::new(
                                "cos",
                                SessionKind::Conversation,
                                None,
                                "claude-code",
                                Some("a".into()),
                                "550e8400-e29b-41d4-a716-446655440000",
                                OffsetDateTime::now_utc(),
                            )
                        });
                        let (pick, selection) = d.select_provider_for(
                            &hint(lane, None),
                            now,
                            TaskId::new(),
                            &mut full,
                            session.as_ref(),
                            cos,
                            prefer_local,
                        );
                        let result = pick.map(|(_, provider, account)| {
                            let model = d.models.get(&provider).cloned().unwrap_or_default();
                            (provider, account.map(|(_, id)| id), model)
                        });
                        (result, selection.reason)
                    };
                    assert_eq!(
                        run(false),
                        run(true),
                        "lane={lane:?} role={role} sticky={sticky} supply={supply}"
                    );
                }
            }
        }
    }
}

#[test]
fn routing_dispatch_legacy_tier_models_feed_model_identity_only() {
    let claude = accounts_fixture();
    let (probe, _) = counting_probe(Reachability::Ok);
    let mut d = local_first_dispatcher(&claude, vec![Tier::Cheap], probe);
    let (tx, _) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(crate::dispatcher::SnapshotPublisher {
        tx,
        instance_id: "test".into(),
        hostname: "test".into(),
        started_at: String::new(),
        tick_ms: 1000,
        providers: vec![
            serde_json::from_value(serde_json::json!({
                "id": "qwen", "adapter": "acp", "tiers": ["cheap"],
                "concurrency": 1, "model": "old-model", "in_use": 0,
                "llm_source": {"source": "openai_compatible:qwen", "origin": "explicit"},
                "tier_models": {"cheap": {"name": "qwen", "model_id": "qwen-from-binding"}}
            }))
            .unwrap(),
        ],
        provider_checks: Default::default(),
    });
    let trace = d
        .legacy_optimizer_trace(&hint(Tier::Cheap, None), "run", "qwen")
        .unwrap();
    assert_eq!(trace.candidates[1].model_profile_id, "qwen-from-binding");
    assert_eq!(trace.fallback_order, ["p1", "qwen"]);
    let (picked, selection) = d.select_provider_for(
        &hint(Tier::Cheap, None),
        Instant::now(),
        TaskId::new(),
        &mut Default::default(),
        None,
        false,
        true,
    );
    assert_eq!(picked.unwrap().1, "qwen");
    assert_eq!(selection.reason, ProviderSelectionReason::LocalPreferred);
}

/// tick を通して、worker run の `RoutingDecided` に選択の理由が残る（付記 L8）。
async fn routing_decided_with_probe(
    probe_outcome: Reachability,
) -> (task_core::RoutingRecord, Arc<AtomicUsize>) {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        ws.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    task.worker_hint.tier = Tier::Cheap;
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Human,
        ..Default::default()
    });
    store.insert(&task).unwrap();
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(InstantAdapter {
        terminal: Terminal::Question {
            text: "not needed".into(),
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    d.policy = Box::new(StaticPolicy::new(
        vec![
            ProviderSpec {
                id: "p1".into(),
                adapter: "instant".into(),
                tiers: vec![Tier::Cheap, Tier::Standard],
                concurrency: 2,
                model: "m".into(),
            },
            ProviderSpec {
                id: "qwen".into(),
                adapter: "acp".into(),
                tiers: vec![Tier::Cheap],
                concurrency: 1,
                model: "qwen3.8-27b".into(),
            },
        ],
        Duration::from_secs(1),
    ));
    d.adapters.insert("qwen".into(), adapter);
    let (probe, calls) = counting_probe(probe_outcome);
    d.set_local_providers(vec![qwen_spec()]);
    d.set_local_provider_probe(probe);
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    let record = routing_record(&events).expect("routing_decided");
    (record, calls)
}

#[tokio::test]
async fn cheap_local_first_routing_decided_records_reason() {
    let (record, calls) = routing_decided_with_probe(Reachability::Ok).await;
    let trace = record.optimizer.as_ref().expect("legacy optimizer trace");
    assert_eq!(
        trace.mode,
        task_core::model_router::policy::RoutingMode::Legacy
    );
    assert_eq!(trace.selected.as_deref(), Some("qwen"));
    assert_eq!(trace.fallback_order, ["p1", "qwen"]);
    assert_eq!(trace.candidates[1].model_profile_id, "qwen3.8-27b");
    assert_eq!(record.resolution.provider.as_deref(), Some("qwen"));
    assert_eq!(record.resolution.adapter, "acp");
    let selection = record.resolution.selection.expect("selection");
    assert_eq!(selection.reason, ProviderSelectionReason::LocalPreferred);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let (record, _) = routing_decided_with_probe(Reachability::Unreachable {
        reason: "timed out".into(),
    })
    .await;
    assert_eq!(record.resolution.provider.as_deref(), Some("p1"));
    let selection = record.resolution.selection.expect("selection");
    assert_eq!(selection.reason, ProviderSelectionReason::LocalDown);
    assert_eq!(
        local_outcome(&selection),
        Some(ProviderCandidateOutcome::Down)
    );
}
