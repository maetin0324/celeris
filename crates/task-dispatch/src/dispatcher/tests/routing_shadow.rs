//! multi-objective routing Phase 4（dispatch-shadow）: decision shadow は primary（legacy）を変えず、
//! 追加の呼出し（HTTP・health probe・worker 起動）をせず、候補 policy の比較だけを
//! `routing_shadow_recorded`（kind decision）に 1 件残す。mode=legacy / enforce では何も記録しない。
//! 時計は `set_now_unix_fn`、local の health probe は数える偽物で、ネットワークには出ない。
use super::*;
use crate::dispatcher::{
    DECISION_SHADOW_COMPARISON_VERSION, DECISION_SHADOW_POLICY_VERSION, DecisionShadowComparison,
    DispatchRoutingSettings,
};
use task_core::model_router::{
    policy::RoutingMode,
    shadow::{ShadowKind, ShadowRecord, ShadowStatus},
};
use task_core::model_routing::ModelBinding;

const QWEN_URL: &str = "http://qwen.invalid/v1";

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

struct Observed {
    status: Status,
    /// event の type 名（順序どおり）。
    types: Vec<String>,
    started: Vec<(Option<String>, String, Option<String>)>,
    record: Option<task_core::RoutingRecord>,
    shadows: Vec<ShadowRecord>,
    spawned: usize,
    probes: usize,
    usage_file: String,
}

fn observe(store: &Arc<dyn TaskStore>, task: TaskId) -> Observed {
    let events = store.events_for(task).unwrap();
    Observed {
        status: store.get(task).unwrap().unwrap().status,
        types: events
            .iter()
            .map(|(_, e)| {
                serde_json::to_value(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect(),
        started: events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::WorkerStarted {
                    provider,
                    model,
                    account,
                    ..
                } => Some((provider.clone(), model.clone(), account.clone())),
                _ => None,
            })
            .collect(),
        record: routing_record(&events),
        shadows: events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::RoutingShadowRecorded { record } => Some((**record).clone()),
                _ => None,
            })
            .collect(),
        spawned: 0,
        probes: 0,
        usage_file: String::new(),
    }
}

/// p1 = pool（アカウント a、5h/7d とも `utilization`）、p2 = 非 pool（frontier も提供）。frontier の task。
async fn pool_case(mode: RoutingMode, utilization: f64) -> Observed {
    let accounts = accounts_fixture();
    let usage_path = accounts.path().join(".celeris-usage.json");
    let mut book = AccountBook::load(&usage_path);
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
    let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
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
            captured: captured.clone(),
            spawn_failure: false,
        }),
        "",
        Some("a"),
    );
    let other = tiered(
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
    );
    let mut d = pool_dispatcher(
        store.clone(),
        pool,
        Some(("p2", other)),
        accounts.path().to_path_buf(),
        2,
        2,
    );
    d.set_now_unix_fn(Arc::new(|| 10_000));
    d.set_dispatch_routing(DispatchRoutingSettings {
        mode,
        ..Default::default()
    });
    d.tick().unwrap();
    // event 列と帳簿は tick 直後（worker の進行より前）の写し。
    let mut out = observe(&store, task.id);
    out.usage_file = std::fs::read_to_string(&usage_path).unwrap_or_default();
    // spawn された worker を出来事待ちで数える（長い保険つき。ADR-0125）。shadow が run を足していないこと。
    let deadline = Instant::now() + Duration::from_secs(60);
    while captured.lock().unwrap().len() < out.started.len() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    out.spawned = captured.lock().unwrap().len();
    out
}

/// cheap lane: pool の p1（claude-code）とローカルの qwen（health probe は数える偽物）。
async fn local_case(mode: RoutingMode, reach: Reachability) -> Observed {
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
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    d.set_local_providers(vec![crate::dispatcher::LocalProviderSpec {
        provider: "qwen".into(),
        health: vec![crate::dispatcher::LocalHealthTarget {
            base_url: QWEN_URL.into(),
            bearer_token: None,
        }],
    }]);
    d.set_local_provider_probe(Arc::new(move |url, _| {
        assert_eq!(url, QWEN_URL);
        counter.fetch_add(1, Ordering::SeqCst);
        reach.clone()
    }));
    d.set_now_unix_fn(Arc::new(|| 10_000));
    d.set_dispatch_routing(DispatchRoutingSettings {
        mode,
        ..Default::default()
    });
    d.tick().unwrap();
    let mut out = observe(&store, task.id);
    out.probes = calls.load(Ordering::SeqCst);
    out
}

fn comparison(record: &ShadowRecord) -> DecisionShadowComparison {
    serde_json::from_str(record.detail.as_deref().expect("comparison in detail")).unwrap()
}

/// legacy と shadow で primary・event 列・副作用が同じで、shadow には記録 1 件だけが増える。
fn assert_same_primary(legacy: &Observed, shadow: &Observed) {
    assert!(legacy.shadows.is_empty(), "legacy records no shadow");
    assert_eq!(shadow.started, legacy.started, "primary is the legacy one");
    assert_eq!(shadow.status, legacy.status);
    assert_eq!(shadow.spawned, legacy.spawned, "no extra worker");
    assert_eq!(shadow.probes, legacy.probes, "no extra health probe / HTTP");
    assert_eq!(
        shadow.usage_file, legacy.usage_file,
        "account book untouched"
    );
    let mut without: Vec<String> = shadow.types.clone();
    let at = without
        .iter()
        .position(|t| t == "routing_shadow_recorded")
        .expect("shadow event");
    without.remove(at);
    assert_eq!(without, legacy.types, "only the shadow event is added");
    assert_eq!(
        shadow.types[at - 1],
        "routing_decided",
        "{:?}",
        shadow.types
    );
    let (l, s) = (
        legacy.record.as_ref().expect("legacy record"),
        shadow.record.as_ref().expect("shadow-mode record"),
    );
    assert_eq!(s.resolution, l.resolution);
    assert_eq!(s.quota_reason, l.quota_reason);
    assert_eq!(
        s.optimizer.as_ref().map(|t| t.mode),
        Some(RoutingMode::Legacy)
    );
    assert_eq!(shadow.shadows.len(), 1);
    let rec = &shadow.shadows[0];
    rec.validate().unwrap();
    assert_eq!(rec.kind, ShadowKind::Decision);
    assert_eq!(rec.status, ShadowStatus::Completed);
    assert_eq!(rec.reason, None);
    assert_eq!(rec.policy_version, DECISION_SHADOW_POLICY_VERSION);
    assert_eq!(
        Some(rec.primary_decision_id.as_str()),
        s.optimizer.as_ref().map(|t| t.decision_id.as_str())
    );
    // 候補比較だけ: tokens・費用・出力 hash・予約は持たない。
    assert_eq!(
        (
            rec.input_tokens,
            rec.output_tokens,
            &rec.output_sha256,
            rec.cash_usd,
            rec.effective_usd,
            rec.latency_ms,
            &rec.reservation_id
        ),
        (None, None, &None, None, None, None, &None)
    );
}

#[tokio::test]
async fn routing_decision_shadow_never_calls_upstream_or_changes_primary() {
    // (1) 残量 20% の pool p1: legacy は frontier → standard に下げて p1。候補 policy（enforce）は lane を
    //     保って p2 を選ぶ。shadow mode でも primary は legacy のまま、差だけが記録に残る。
    let legacy = pool_case(RoutingMode::Legacy, 0.8).await;
    let shadow = pool_case(RoutingMode::Shadow, 0.8).await;
    assert_eq!(
        legacy.started,
        vec![(Some("p1".into()), "standard-id".into(), Some("a".into()))]
    );
    assert_same_primary(&legacy, &shadow);
    assert_eq!(shadow.spawned, 1, "exactly the primary worker");
    let rec = &shadow.shadows[0];
    assert_eq!(rec.candidate_source.as_deref(), Some("p2"));
    assert_eq!(rec.candidate_model.as_deref(), Some("p2-frontier-id"));
    let cmp = comparison(rec);
    assert_eq!(cmp.version, DECISION_SHADOW_COMPARISON_VERSION);
    assert_eq!(cmp.primary_mode, RoutingMode::Legacy);
    assert_eq!(
        (cmp.primary_source.as_str(), cmp.primary_model.as_str()),
        ("p1", "standard-id")
    );
    assert_eq!(cmp.primary_lane, Tier::Standard);
    assert_eq!(cmp.requested_lane, Tier::Frontier);
    assert_eq!(cmp.candidate_lane, Some(Tier::Frontier));
    assert!(!cmp.agrees);
    assert_eq!(cmp.differences, ["source", "model", "lane"]);
    let p1 = cmp
        .candidates
        .iter()
        .find(|c| c.deployment_id == "p1")
        .unwrap();
    assert!(p1.primary && !p1.selected);
    assert!(
        p1.excluded_reasons
            .iter()
            .any(|r| r == "quota_low_for_lane"),
        "{:?}",
        p1.excluded_reasons
    );
    let p2 = cmp
        .candidates
        .iter()
        .find(|c| c.deployment_id == "p2")
        .unwrap();
    assert!(p2.selected && !p2.primary && p2.excluded_reasons.is_empty());
    // 同じ入力（時計・帳簿）なら同じ比較（shadow_id は記録ごとの id なので除く）。
    let again = pool_case(RoutingMode::Shadow, 0.8).await;
    assert_eq!(comparison(&again.shadows[0]), cmp);

    // (2) 残量が十分: 両者とも p1 frontier で一致。
    let legacy = pool_case(RoutingMode::Legacy, 0.1).await;
    let shadow = pool_case(RoutingMode::Shadow, 0.1).await;
    assert_same_primary(&legacy, &shadow);
    let cmp = comparison(&shadow.shadows[0]);
    assert!(cmp.agrees, "{cmp:?}");
    assert!(cmp.differences.is_empty());
    assert_eq!(cmp.candidate_source.as_deref(), Some("p1"));

    // (3) enforce は primary 自体が kernel なので shadow を記録しない。
    let enforce = pool_case(RoutingMode::Enforce, 0.1).await;
    assert!(enforce.shadows.is_empty());
    assert!(!enforce.types.iter().any(|t| t == "routing_shadow_recorded"));

    // (4) cheap のローカル優先: health probe は primary の 1 回だけ（shadow は probe しない）。
    for reach in [
        Reachability::Ok,
        Reachability::Unreachable {
            reason: "timed out".into(),
        },
    ] {
        let legacy = local_case(RoutingMode::Legacy, reach.clone()).await;
        let shadow = local_case(RoutingMode::Shadow, reach.clone()).await;
        assert_eq!(legacy.probes, 1);
        assert_same_primary(&legacy, &shadow);
        let cmp = comparison(&shadow.shadows[0]);
        let qwen = cmp
            .candidates
            .iter()
            .find(|c| c.deployment_id == "qwen")
            .unwrap();
        match reach {
            Reachability::Ok => {
                assert_eq!(cmp.primary_source, "qwen");
                assert_eq!(cmp.candidate_source.as_deref(), Some("qwen"));
                assert!(cmp.agrees, "{cmp:?}");
            }
            _ => {
                assert_eq!(cmp.primary_source, "p1");
                // 不通は primary が見た結果を写す（probe し直さない）。
                assert_eq!(qwen.excluded_reasons, ["unreachable"]);
                assert_eq!(cmp.candidate_source.as_deref(), Some("p1"));
            }
        }
    }
}
