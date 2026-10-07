//! ADR 2026-10-06 model-role-assignments D2/D3/D6: DB の割り当ては config の `tier_models` に勝ち、
//! `Excluded` の lane は provider を候補から外して次の provider へ落とす。reader が無ければ従来どおり。
//! 外部ネットワークには出ない。
use super::*;
use crate::dispatcher::{DecisionShadowComparison, DispatchRoutingSettings, StaticModelProfiles};
use task_core::Tier;
use task_core::model_catalog::CatalogSource;
use task_core::model_catalog::assignments::{
    AssignmentState, AssignmentView, EffectiveAssignment, StaticAssignments,
};
use task_core::model_router::policy::RoutingMode;
use task_core::model_router::profiles::{
    Capabilities, ContextLimits, ModelProfile, QualityIndex, Support,
};
use task_core::model_routing::{ModelBinding, ProviderCandidateOutcome};

fn tiered(
    base: Arc<dyn WorkerAdapter>,
    prefix: &str,
    account: Option<&str>,
) -> Arc<dyn WorkerAdapter> {
    Arc::new(task_worker::tiered::TieredAdapter {
        base,
        models: config_models(prefix),
        account_id: account.map(str::to_owned),
        credential_error: None,
    })
}

fn config_models(prefix: &str) -> task_core::model_routing::TierModels {
    [
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
    .collect()
}

fn hint(tier: Tier, adapter: Option<&str>) -> task_core::WorkerHint {
    task_core::WorkerHint {
        tier,
        adapter: adapter.map(str::to_string),
    }
}

fn item(source: &str, tier: Tier, model: &str, state: AssignmentState) -> EffectiveAssignment {
    EffectiveAssignment {
        priority: 0,
        source: CatalogSource::new(source),
        tier,
        model_id: model.into(),
        state,
        note: None,
        updated_at: 1,
        updated_by: "admin".into(),
    }
}

fn view(items: Vec<EffectiveAssignment>) -> AssignmentView {
    AssignmentView {
        items,
        managed: Vec::new(),
    }
}

fn excluded() -> AssignmentState {
    AssignmentState::Excluded {
        reason: "catalog:unavailable",
    }
}

fn done_pool_adapter(captured: CapturedEnvs) -> Arc<dyn WorkerAdapter> {
    Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: vec![],
        captured,
        spawn_failure: false,
    })
}

fn live(
    id: &str,
    adapter: &str,
    source: &str,
    account_pool: bool,
    tier_models: serde_json::Value,
) -> task_ops::daemon::ProviderLive {
    serde_json::from_value(serde_json::json!({
        "id": id, "adapter": adapter, "tiers": ["frontier", "standard", "cheap"],
        "concurrency": 2, "model": "m", "in_use": 0, "account_pool": account_pool,
        "llm_source": {"source": source, "origin": "explicit"},
        "tier_models": tier_models,
    }))
    .unwrap()
}

fn config_tier_models_json(prefix: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (tier, id) in [
        ("frontier", "frontier-id"),
        ("standard", "standard-id"),
        ("cheap", "cheap-id"),
    ] {
        map.insert(
            tier.into(),
            serde_json::json!({"name": format!("{prefix}{id}"), "model_id": format!("{prefix}{id}")}),
        );
    }
    serde_json::Value::Object(map)
}

fn publish(d: &mut Dispatcher, providers: Vec<task_ops::daemon::ProviderLive>) {
    let (tx, _) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(crate::dispatcher::SnapshotPublisher {
        tx,
        instance_id: "test".into(),
        hostname: "test".into(),
        started_at: String::new(),
        tick_ms: 1000,
        providers,
        provider_checks: Default::default(),
    });
}

fn insert_task(store: &Arc<dyn TaskStore>, ws: &std::path::Path, tier: Tier) -> TaskId {
    let mut task = new_task(
        ws,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.worker_hint.tier = tier;
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Human,
        ..Default::default()
    });
    store.insert(&task).unwrap();
    task.id
}

struct Started {
    provider: Option<String>,
    model: String,
    account: Option<String>,
}

fn started(store: &Arc<dyn TaskStore>, id: TaskId) -> Option<Started> {
    store
        .events_for(id)
        .unwrap()
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted {
                provider,
                model,
                account,
                ..
            } => Some(Started {
                provider: provider.clone(),
                model: model.clone(),
                account: account.clone(),
            }),
            _ => None,
        })
}

/// p1 = claude-code のプール（claude_oauth）、p2 = 非プール（openai_compatible:p2）。reader は `assignments`。
fn claude_dispatcher(
    accounts: &tempfile::TempDir,
    store: Arc<dyn TaskStore>,
    reader: Option<AssignmentView>,
) -> Dispatcher {
    let pool = tiered(done_pool_adapter(Default::default()), "", Some("a"));
    let other = tiered(done_pool_adapter(Default::default()), "p2-", None);
    let mut d = pool_dispatcher(
        store,
        pool,
        Some(("p2", other)),
        accounts.path().to_path_buf(),
        2,
        2,
    );
    d.set_now_unix_fn(Arc::new(|| 10_000));
    publish(
        &mut d,
        vec![
            live(
                "p1",
                "claude-code",
                "claude_oauth",
                true,
                config_tier_models_json(""),
            ),
            live(
                "p2",
                "instant",
                "openai_compatible:p2",
                false,
                config_tier_models_json("p2-"),
            ),
        ],
    );
    if let Some(view) = reader {
        d.set_role_assignment_reader(Arc::new(StaticAssignments(view)));
    }
    d
}

#[tokio::test]
async fn assignment_beats_config_tier_models() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let id = insert_task(&store, ws.path(), Tier::Frontier);
    let mut d = claude_dispatcher(
        &accounts,
        store.clone(),
        Some(view(vec![item(
            "claude-oauth",
            Tier::Frontier,
            "claude-assigned",
            AssignmentState::Assigned,
        )])),
    );
    d.tick().unwrap();
    let s = started(&store, id).expect("worker started");
    assert_eq!(s.provider.as_deref(), Some("p1"));
    assert_eq!(s.model, "claude-assigned");
    assert_eq!(s.account.as_deref(), Some("a"));
    // 候補の model identity も実効 bindings（割り当て）に従う。
    let profiles = d.legacy_provider_profiles(&hint(Tier::Frontier, None));
    let p1 = profiles.iter().find(|p| p.provider_id == "p1").unwrap();
    assert_eq!(p1.model.id, "claude-assigned");
    assert_eq!(p1.model.provenance, "model_role_assignments");
    assert_eq!(p1.deployment.id, "p1/model:claude-assigned");
    let p2 = profiles.iter().find(|p| p.provider_id == "p2").unwrap();
    assert_eq!(p2.deployment.id, "p2");
    assert_eq!(p2.model.id, "p2-frontier-id");
    assert_eq!(p2.model.provenance, "providers.tier_models");
    let trace = d
        .legacy_optimizer_trace(&hint(Tier::Frontier, None), "run", "p1")
        .unwrap();
    assert_eq!(
        trace.catalog_version,
        "model_role_assignments+providers.tier_models"
    );
}

#[tokio::test]
async fn lanes_without_an_assignment_keep_config_models() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let id = insert_task(&store, ws.path(), Tier::Standard);
    let mut d = claude_dispatcher(
        &accounts,
        store.clone(),
        Some(view(vec![item(
            "claude-oauth",
            Tier::Frontier,
            "claude-assigned",
            AssignmentState::Assigned,
        )])),
    );
    d.tick().unwrap();
    let s = started(&store, id).expect("worker started");
    assert_eq!(s.provider.as_deref(), Some("p1"));
    assert_eq!(s.model, "standard-id");
}

#[tokio::test]
async fn excluded_lane_skips_the_provider_and_falls_back_to_the_next() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let id = insert_task(&store, ws.path(), Tier::Frontier);
    let mut d = claude_dispatcher(
        &accounts,
        store.clone(),
        Some(view(vec![item(
            "claude-oauth",
            Tier::Frontier,
            "gone",
            excluded(),
        )])),
    );
    d.tick().unwrap();
    let s = started(&store, id).expect("worker started");
    assert_eq!(s.provider.as_deref(), Some("p2"));
    assert_eq!(s.model, "p2-frontier-id");
    let record = routing_record(&store.events_for(id).unwrap()).expect("routing record");
    let selection = record.resolution.selection.expect("selection recorded");
    let p1 = selection
        .candidates
        .iter()
        .find(|c| c.provider == "p1")
        .expect("p1 candidate recorded");
    assert_eq!(p1.outcome, ProviderCandidateOutcome::Unsupported);
    assert_eq!(
        p1.detail.as_deref(),
        Some("assignment_excluded: assignment:gone catalog:unavailable")
    );
}

#[tokio::test]
async fn excluded_lane_with_no_other_provider_is_unroutable_not_started() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let id = insert_task(&store, ws.path(), Tier::Frontier);
    let pool = tiered(done_pool_adapter(Default::default()), "", Some("a"));
    let mut d = pool_dispatcher(
        store.clone(),
        pool,
        None,
        accounts.path().to_path_buf(),
        2,
        2,
    );
    d.set_now_unix_fn(Arc::new(|| 10_000));
    publish(
        &mut d,
        vec![live(
            "p1",
            "claude-code",
            "claude_oauth",
            true,
            config_tier_models_json(""),
        )],
    );
    d.set_role_assignment_reader(Arc::new(StaticAssignments(view(vec![item(
        "claude-oauth",
        Tier::Frontier,
        "gone",
        excluded(),
    )]))));
    d.tick().unwrap();
    assert!(started(&store, id).is_none());
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Ready);
}

#[tokio::test]
async fn without_a_reader_or_with_an_empty_view_nothing_changes() {
    for reader in [None, Some(AssignmentView::empty())] {
        let accounts = accounts_fixture();
        let ws = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let id = insert_task(&store, ws.path(), Tier::Frontier);
        let mut d = claude_dispatcher(&accounts, store.clone(), reader);
        d.tick().unwrap();
        let s = started(&store, id).expect("worker started");
        assert_eq!(s.provider.as_deref(), Some("p1"));
        assert_eq!(s.model, "frontier-id");
        let profiles = d.legacy_provider_profiles(&hint(Tier::Frontier, None));
        assert!(
            profiles
                .iter()
                .all(|p| p.model.provenance == "providers.tier_models")
        );
        let trace = d
            .legacy_optimizer_trace(&hint(Tier::Frontier, None), "run", "p1")
            .unwrap();
        assert_eq!(trace.catalog_version, "providers.tier_models");
    }
}

/// og = ACP + opencode go のプール（`tier_models` なし）、p2 = 非プールの別 provider。
fn opencode_dispatcher(
    store: Arc<dyn TaskStore>,
    og_accounts: &tempfile::TempDir,
    view: AssignmentView,
) -> Dispatcher {
    let claude = accounts_fixture();
    let og = tiered_empty(done_pool_adapter(Default::default()), Some("a"));
    let other = tiered(done_pool_adapter(Default::default()), "p2-", None);
    let mut d = pool_dispatcher(
        store,
        og.clone(),
        Some(("p2", other)),
        claude.path().to_path_buf(),
        2,
        2,
    );
    // pool_dispatcher の p1 を opencode go の acp 行に作り替える。
    d.policy = Box::new(StaticPolicy::new(
        vec![
            ProviderSpec {
                id: "og".into(),
                adapter: "acp".into(),
                tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
                concurrency: 2,
                model: String::new(),
            },
            ProviderSpec {
                id: "p2".into(),
                adapter: "instant".into(),
                tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
                concurrency: 2,
                model: "m".into(),
            },
        ],
        Duration::from_secs(1),
    ));
    d.adapters.insert("og".into(), og);
    d.account_pool_providers = ["og".to_string()].into();
    d.account_pool_adapters = HashMap::from([("og".to_string(), AccountAdapter::OpencodeGo)]);
    let accounts = d.config.accounts.as_mut().unwrap();
    accounts
        .roots
        .insert(AccountAdapter::OpencodeGo, og_accounts.path().to_path_buf());
    d.account_books.insert(
        AccountAdapter::OpencodeGo,
        Arc::new(StdMutex::new(AccountBook::load(
            &og_accounts.path().join(".celeris-usage.json"),
        ))),
    );
    d.set_now_unix_fn(Arc::new(|| 10_000));
    publish(
        &mut d,
        vec![
            {
                let mut og = live("og", "acp", "opencode_go", true, serde_json::json!({}));
                og.model = None;
                og
            },
            live(
                "p2",
                "instant",
                "openai_compatible:p2",
                false,
                config_tier_models_json("p2-"),
            ),
        ],
    );
    d.set_role_assignment_reader(Arc::new(StaticAssignments(view)));
    // keep the claude fixture alive until here: the dispatcher only reads its path lazily.
    drop(claude);
    d
}

fn tiered_empty(base: Arc<dyn WorkerAdapter>, account: Option<&str>) -> Arc<dyn WorkerAdapter> {
    Arc::new(task_worker::tiered::TieredAdapter {
        base,
        models: Default::default(),
        account_id: account.map(str::to_owned),
        credential_error: None,
    })
}

fn opencode_accounts() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let acct = dir.path().join("a");
    std::fs::create_dir_all(acct.join("opencode")).unwrap();
    std::fs::write(acct.join("opencode/auth.json"), "{}").unwrap();
    dir
}

#[tokio::test]
async fn opencode_go_row_becomes_a_candidate_for_an_assigned_lane_and_falls_back_when_exhausted() {
    let og_accounts = opencode_accounts();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let assigned = view(vec![item(
        "opencode-go",
        Tier::Cheap,
        "glm-5",
        AssignmentState::Assigned,
    )]);
    let mut d = opencode_dispatcher(store.clone(), &og_accounts, assigned);

    // 割り当て済みの lane: og が候補に入り、`opencode-go/<id>` で起動する。
    let cheap = insert_task(&store, ws.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    let s = started(&store, cheap).expect("worker started");
    assert_eq!(s.provider.as_deref(), Some("og"));
    assert_eq!(s.model, "opencode-go/glm-5");
    assert_eq!(s.account.as_deref(), Some("a"));

    // 割り当ての無い lane（standard）は binding が無いので og へ routing しない（次の provider へ）。
    let ws2 = tempfile::tempdir().unwrap();
    let standard = insert_task(&store, ws2.path(), Tier::Standard);
    assert!(run_until_idle(&mut d, 200).await.idle);
    let s = started(&store, standard).expect("worker started");
    assert_eq!(s.provider.as_deref(), Some("p2"));
    assert_eq!(s.model, "p2-standard-id");

    // pool の全 account が Exhausted なら、割り当て済みの lane でも次の provider へ落ちる。
    d.record_account_failure(
        AccountAdapter::OpencodeGo,
        "a",
        "exhausted",
        &ProviderOutcome::Exhausted,
    );
    let ws3 = tempfile::tempdir().unwrap();
    let fallback = insert_task(&store, ws3.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    let s = started(&store, fallback).expect("worker started");
    assert_eq!(s.provider.as_deref(), Some("p2"));
    assert_eq!(s.model, "p2-cheap-id");
}

/// ADR D3: model も `tier_models` も無い行は割り当てだけで routing する。割り当てが無ければどの lane の候補にもならず、
/// lane を 1 つ割り当てるとその lane だけ候補になる。
#[tokio::test]
async fn modelless_opencode_go_row_routes_only_through_assignments() {
    let og_accounts = opencode_accounts();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = opencode_dispatcher(store.clone(), &og_accounts, AssignmentView::empty());

    // 割り当てなし: cheap でも og は候補にならず、次の provider へ。
    let cheap = insert_task(&store, ws.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    let s = started(&store, cheap).expect("worker started");
    assert_eq!(s.provider.as_deref(), Some("p2"));
    assert!(d.effective_lane_model("og", Tier::Cheap).is_err());

    // og だけなら経路なし（走らず Ready のまま）。
    let ws2 = tempfile::tempdir().unwrap();
    let only = insert_task(&store, ws2.path(), Tier::Standard);
    d.policy = Box::new(StaticPolicy::new(
        vec![ProviderSpec {
            id: "og".into(),
            adapter: "acp".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: 2,
            model: String::new(),
        }],
        Duration::from_secs(1),
    ));
    d.tick().unwrap();
    assert!(started(&store, only).is_none());
    assert_eq!(store.get(only).unwrap().unwrap().status, Status::Ready);

    // cheap を割り当てると、その lane だけ og が候補になる。
    d.set_role_assignment_reader(Arc::new(StaticAssignments(view(vec![item(
        "opencode-go",
        Tier::Cheap,
        "glm-5",
        AssignmentState::Assigned,
    )]))));
    assert_eq!(
        d.effective_lane_model("og", Tier::Cheap),
        Ok(Some("opencode-go/glm-5".into()))
    );
    assert!(d.effective_lane_model("og", Tier::Standard).is_err());
    let ws3 = tempfile::tempdir().unwrap();
    let cheap2 = insert_task(&store, ws3.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    assert_eq!(
        started(&store, cheap2).and_then(|s| s.provider).as_deref(),
        Some("og")
    );
    // standard はまだ og に出ない（og だけの policy なので Ready のまま）。
    assert!(started(&store, only).is_none());
}

#[tokio::test]
async fn role_members_priority_all_candidates_and_opencode_execution() {
    let accounts = opencode_accounts();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut preferred = item(
        "opencode-go",
        Tier::Cheap,
        "preferred",
        AssignmentState::Assigned,
    );
    preferred.priority = 1;
    let mut second = item(
        "opencode-go",
        Tier::Cheap,
        "second",
        AssignmentState::Assigned,
    );
    second.priority = 2;
    let mut removed = item("opencode-go", Tier::Cheap, "removed", excluded());
    removed.priority = 0;
    let mut fallback_member = item(
        "openai-compatible:p2",
        Tier::Cheap,
        "p2-cheap-id",
        AssignmentState::Assigned,
    );
    fallback_member.priority = 20;
    let mut d = opencode_dispatcher(
        store.clone(),
        &accounts,
        view(vec![second.clone(), removed, preferred, fallback_member]),
    );
    let profiles = d.legacy_provider_profiles(&hint(Tier::Cheap, None));
    let candidates: Vec<_> = profiles
        .iter()
        .filter(|p| p.provider_id == "og")
        .map(|p| p.model.id.as_str())
        .collect();
    assert_eq!(candidates, ["opencode-go/preferred", "opencode-go/second"]);
    // 候補の identity はモデルごと、容量の identity は provider。
    assert_eq!(
        profiles
            .iter()
            .filter(|p| p.provider_id == "og")
            .map(|p| p.deployment.id.as_str())
            .collect::<Vec<_>>(),
        ["og/model:preferred", "og/model:second"]
    );
    let round = d.enforce_round(&hint(Tier::Cheap, None));
    assert_eq!(
        round
            .candidates
            .iter()
            .filter(|c| c.eligible_provider_ids == ["og"])
            .count(),
        2
    );
    let cheap = insert_task(&store, ws.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    assert_eq!(
        started(&store, cheap).unwrap().model,
        "opencode-go/preferred"
    );
    // The pool quota belongs to the source, so exhaustion skips both models together.
    d.record_account_failure(
        AccountAdapter::OpencodeGo,
        "a",
        "exhausted",
        &ProviderOutcome::Exhausted,
    );
    let ws2 = tempfile::tempdir().unwrap();
    let fallback = insert_task(&store, ws2.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    assert_eq!(
        started(&store, fallback).unwrap().provider.as_deref(),
        Some("p2")
    );
    // Excluding the preferred model selects the next model without removing its role.
    d.set_role_assignment_reader(Arc::new(StaticAssignments(view(vec![
        item("opencode-go", Tier::Cheap, "preferred", excluded()),
        second,
    ]))));
    assert_eq!(
        d.effective_lane_model("og", Tier::Cheap)
            .unwrap()
            .as_deref(),
        Some("opencode-go/second")
    );
}

fn model_profile(id: &str, quality: f64) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        revision: "r".into(),
        family: "f".into(),
        capabilities: Capabilities {
            tools: Support::Unknown,
            structured_output: Support::Unknown,
            vision: Support::Unknown,
            streaming: Support::Unknown,
            reasoning_efforts: vec![],
        },
        context_limits: ContextLimits {
            input: None,
            output: None,
            total: None,
        },
        quality: vec![QualityIndex {
            domain: "general".into(),
            index: quality,
            evaluation_version: "v1".into(),
            samples: None,
            provenance: "test".into(),
        }],
        pricing: None,
        provenance: "test".into(),
    }
}

fn cheap_members() -> AssignmentView {
    let mut preferred = item(
        "opencode-go",
        Tier::Cheap,
        "preferred",
        AssignmentState::Assigned,
    );
    preferred.priority = 1;
    let mut second = item(
        "opencode-go",
        Tier::Cheap,
        "second",
        AssignmentState::Assigned,
    );
    second.priority = 2;
    let mut other = item(
        "openai-compatible:p2",
        Tier::Cheap,
        "p2-cheap-id",
        AssignmentState::Assigned,
    );
    other.priority = 20;
    view(vec![second, preferred, other])
}

/// 付記「モデルごとの複数役割」: enforce は役割の全メンバーを候補にし、kernel（品質推定）の選択を実行モデルに
/// する。品質が unknown なら priority 順。account が枯れたら次の source へ（同じ account の別モデルは試さない）。
#[tokio::test]
async fn enforce_executes_the_kernel_choice_among_role_members_and_falls_back_by_priority() {
    let accounts = opencode_accounts();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = opencode_dispatcher(store.clone(), &accounts, cheap_members());
    d.set_dispatch_routing(DispatchRoutingSettings {
        mode: RoutingMode::Enforce,
        ..Default::default()
    });

    // 1) 品質 unknown: priority 順の先頭（preferred）。trace は 2 メンバーを別候補として残す。
    let ws = tempfile::tempdir().unwrap();
    let t1 = insert_task(&store, ws.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    assert_eq!(started(&store, t1).unwrap().model, "opencode-go/preferred");
    let trace = routing_record(&store.events_for(t1).unwrap())
        .and_then(|r| r.optimizer)
        .expect("enforce trace");
    assert_eq!(trace.mode, RoutingMode::Enforce);
    assert_eq!(trace.model.as_deref(), Some("opencode-go/preferred"));
    assert_eq!(trace.estimator_version, "heuristic");
    let og: Vec<_> = trace
        .candidates
        .iter()
        .filter(|c| c.eligible_provider_ids == ["og"])
        .map(|c| c.deployment_id.as_str())
        .collect();
    assert_eq!(og, ["og/model:preferred", "og/model:second"]);
    assert_eq!(
        &trace.fallback_order[..2],
        ["og/model:preferred", "og/model:second"]
    );
    assert!(
        trace
            .reasons
            .iter()
            .any(|r| r.contains("ranked by priority"))
    );

    // 2) catalog の品質で second が上回る: kernel の選択（second）で run が起きる。
    d.set_routing_model_profiles(Arc::new(StaticModelProfiles(vec![
        model_profile("opencode-go/second", 0.9),
        model_profile("opencode-go/preferred", 0.5),
    ])));
    let ws2 = tempfile::tempdir().unwrap();
    let t2 = insert_task(&store, ws2.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    assert_eq!(started(&store, t2).unwrap().model, "opencode-go/second");
    let trace = routing_record(&store.events_for(t2).unwrap())
        .and_then(|r| r.optimizer)
        .expect("enforce trace");
    assert_eq!(trace.model.as_deref(), Some("opencode-go/second"));
    assert_eq!(trace.estimator_version, "heuristic-1");
    assert_eq!(
        &trace.fallback_order[..2],
        ["og/model:second", "og/model:preferred"]
    );
    let second = trace
        .candidates
        .iter()
        .find(|c| c.deployment_id == "og/model:second")
        .unwrap();
    let preferred = trace
        .candidates
        .iter()
        .find(|c| c.deployment_id == "og/model:preferred")
        .unwrap();
    assert!(second.score.unwrap() > preferred.score.unwrap());
    assert!(second.excluded_reasons.is_empty() && preferred.excluded_reasons.is_empty());

    // 3) account が枯れたら同じ account の別モデルは試さず、次の source（p2）へ。
    d.record_account_failure(
        AccountAdapter::OpencodeGo,
        "a",
        "exhausted",
        &ProviderOutcome::Exhausted,
    );
    let ws3 = tempfile::tempdir().unwrap();
    let t3 = insert_task(&store, ws3.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    let s = started(&store, t3).unwrap();
    assert_eq!(s.provider.as_deref(), Some("p2"));
    assert_eq!(s.model, "p2-cheap-id");
}

/// 付記「モデルごとの複数役割」: shadow は primary（legacy = priority 順）を変えず、役割の全メンバーを候補に
/// kernel の選択を記録する（モデルの差が `differences` に出る）。
#[tokio::test]
async fn shadow_records_the_kernel_choice_among_role_members() {
    let accounts = opencode_accounts();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = opencode_dispatcher(store.clone(), &accounts, cheap_members());
    d.set_dispatch_routing(DispatchRoutingSettings {
        mode: RoutingMode::Shadow,
        ..Default::default()
    });
    d.set_routing_model_profiles(Arc::new(StaticModelProfiles(vec![
        model_profile("opencode-go/second", 0.9),
        model_profile("opencode-go/preferred", 0.5),
    ])));
    let ws = tempfile::tempdir().unwrap();
    let t = insert_task(&store, ws.path(), Tier::Cheap);
    assert!(run_until_idle(&mut d, 200).await.idle);
    // primary は legacy のまま（priority 順の先頭）。
    assert_eq!(started(&store, t).unwrap().model, "opencode-go/preferred");
    let shadow = store
        .events_for(t)
        .unwrap()
        .iter()
        .find_map(|(_, e)| match e {
            Event::RoutingShadowRecorded { record } => Some((**record).clone()),
            _ => None,
        })
        .expect("decision shadow recorded");
    let cmp: DecisionShadowComparison =
        serde_json::from_str(shadow.detail.as_deref().unwrap()).unwrap();
    assert_eq!(cmp.primary_source, "og");
    assert_eq!(cmp.primary_model, "opencode-go/preferred");
    assert_eq!(cmp.candidate_source.as_deref(), Some("og"));
    assert_eq!(cmp.candidate_model.as_deref(), Some("opencode-go/second"));
    assert_eq!(cmp.differences, ["model"]);
    let og: Vec<_> = cmp
        .candidates
        .iter()
        .filter(|c| c.provider_id == "og")
        .collect();
    assert_eq!(og.len(), 2);
    assert!(
        og.iter()
            .any(|c| c.deployment_id == "og/model:second" && c.selected)
    );
    assert!(
        og.iter()
            .any(|c| c.deployment_id == "og/model:preferred" && c.primary)
    );
    assert_eq!(
        shadow.candidate_model.as_deref(),
        Some("opencode-go/second")
    );
}
