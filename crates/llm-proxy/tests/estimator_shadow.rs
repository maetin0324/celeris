//! estimator shadow の統合テスト（ADR 2026-10-04 §7.3・§10 Phase 5）。偽の上流と偽 sidecar は
//! どちらも `127.0.0.1:0` の in-process server。待ちは出来事（Notify）で行い、sleep しない。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use axum::extract::State as AxumState;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use llm_proxy::config::{LlmProxyConfig, OpenAiCompatibleConfig};
use llm_proxy::estimator_shadow::{
    EstimatorShadow, EstimatorShadowConfig, EstimatorShadowError, kernel_choice,
};
use llm_proxy::estimator_sidecar::{SidecarClientConfig, SidecarEstimatorClient, TokioClock};
use llm_proxy::legacy_catalog::{LegacyCatalog, normalize_legacy_config};
use llm_proxy::reservation::{CapacityLimits, ReservationTable, SlotKey, SystemClock};
use llm_proxy::shadow::{AllowAllBudget, ShadowCandidate, ShadowEvent, ShadowSink};
use llm_proxy::{ProxyState, router};
use serde_json::{Value, json};
use task_core::model_router::context::RoutingContext;
use task_core::model_router::profiles::ContextLimits;
use task_core::model_router::estimator::sidecar::{
    EstimateRequestV1, EstimateResponseV1, EstimatorDescriptor, SidecarEstimateSnapshot,
};
use task_core::model_router::shadow::{
    SHADOW_ALLOW_ANY, ShadowAllowlist, ShadowKind, ShadowPolicy, ShadowReason, ShadowRecord,
    ShadowSettlement, ShadowStatus,
};
use task_core::{SharedRole, Tier};
use tokio::net::TcpListener;
use tokio::sync::Notify;

async fn spawn(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

// --- 偽の openai-compatible 上流 -------------------------------------------------

#[derive(Default)]
struct Upstream {
    chat_hits: AtomicU32,
}

async fn upstream_chat(AxumState(u): AxumState<Arc<Upstream>>) -> axum::response::Response {
    u.chat_hits.fetch_add(1, Ordering::SeqCst);
    Json(json!({
        "id": "1", "object": "chat.completion", "model": "m",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "primary"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
    .into_response()
}

async fn upstream_models() -> axum::response::Response {
    Json(json!({"object": "list", "data": [{"id": "m", "object": "model"}]})).into_response()
}

async fn spawn_upstream() -> (Arc<Upstream>, SocketAddr) {
    let u = Arc::new(Upstream::default());
    let app = Router::new()
        .route("/v1/models", get(upstream_models))
        .route("/v1/chat/completions", post(upstream_chat))
        .with_state(u.clone());
    (u, spawn(app).await)
}

// --- 偽 sidecar ----------------------------------------------------------------

/// 偽 sidecar: 受けた候補ごとに固定の index を返す。`block` なら `release` まで応答を止める。
struct Sidecar {
    hits: AtomicU32,
    requests: StdMutex<Vec<Value>>,
    block: bool,
    arrived: Notify,
    release: Notify,
}

fn score(model: &str) -> f64 {
    match model {
        // 除外される model を最高評価にする。
        "model-x" => 1.0,
        "model-b" => 0.9,
        _ => 0.5,
    }
}

async fn sidecar_estimate(
    AxumState(s): AxumState<Arc<Sidecar>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    s.hits.fetch_add(1, Ordering::SeqCst);
    s.requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(body.clone());
    if s.block {
        let released = s.release.notified();
        s.arrived.notify_one();
        released.await;
    }
    let estimates: Vec<Value> = body["candidates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let id = c["model_profile_id"].as_str().unwrap_or_default();
            json!({"model_profile_id": id, "index": score(id), "confidence": 0.8, "reasons": ["classifier_score"]})
        })
        .collect();
    Json(json!({
        "request_id": body["request_id"], "estimator_id": "route-test", "version": "1",
        "estimates": estimates,
        "dependencies": {"needs_network": false, "external_embeddings": false},
    }))
    .into_response()
}

async fn spawn_sidecar(block: bool) -> (Arc<Sidecar>, SocketAddr) {
    let s = Arc::new(Sidecar {
        hits: AtomicU32::new(0),
        requests: StdMutex::new(Vec::new()),
        block,
        arrived: Notify::new(),
        release: Notify::new(),
    });
    let app = Router::new()
        .route("/estimate", post(sidecar_estimate))
        .with_state(s.clone());
    (s, spawn(app).await)
}

fn descriptor() -> EstimatorDescriptor {
    serde_json::from_value(json!({
        "estimator_id": "route-test", "version": "1", "protocol_version": 1, "needs_prompt": false,
        "dependencies": {"needs_network": false, "external_embeddings": false},
    }))
    .expect("descriptor")
}

fn client(addr: SocketAddr) -> Arc<SidecarEstimatorClient> {
    let mut config = SidecarClientConfig::new(
        reqwest::Url::parse(&format!("http://{addr}/")).expect("url"),
        descriptor(),
    );
    config.timeout = std::time::Duration::from_secs(60);
    Arc::new(SidecarEstimatorClient::new(config, Arc::new(TokioClock)).expect("client"))
}

// --- sink・設定 --------------------------------------------------------------------

#[derive(Default)]
struct Sink {
    records: StdMutex<Vec<ShadowRecord>>,
    changed: Notify,
}

impl Sink {
    fn records(&self) -> Vec<ShadowRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    async fn wait_len(&self, n: usize) {
        loop {
            let changed = self.changed.notified();
            if self.records().len() >= n {
                return;
            }
            changed.await;
        }
    }
}

impl ShadowSink for Sink {
    fn record(&self, event: ShadowEvent) {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.record);
        self.changed.notify_one();
    }
}

fn relay_config(relays: &[(&str, SocketAddr)]) -> LlmProxyConfig {
    let mut config = LlmProxyConfig {
        prefer_free: true,
        ..LlmProxyConfig::default()
    };
    for (id, addr) in relays {
        config
            .sources
            .openai_compatible
            .push(OpenAiCompatibleConfig {
                id: (*id).to_string(),
                base_url: format!("http://{addr}/v1"),
                api_key: None,
                enabled: true,
            });
    }
    config
}

/// relay ごとに別の model profile を持つ catalog。`rc` は外部 network を使う deployment で、cheap の
/// policy は外部 network を禁じる（privacy の hard constraint で除外される）。
fn catalog(config: &LlmProxyConfig) -> LegacyCatalog {
    let mut catalog = normalize_legacy_config(config);
    let template = catalog.models[0].clone();
    catalog.models.clear();
    for d in &mut catalog.deployments {
        let suffix = match d.source_ref.as_str() {
            "openai-compatible:ra" => "a",
            "openai-compatible:rb" => "b",
            _ => "x",
        };
        d.model_profile_id = format!("model-{suffix}");
        d.external_network = suffix == "x";
        d.retains_data = Some(false);
        let mut model = template.clone();
        model.id = d.model_profile_id.clone();
        model.context_limits = ContextLimits {
            input: Some(100_000),
            output: Some(8_192),
            total: Some(108_192),
        };
        catalog.models.push(model);
    }
    for p in &mut catalog.policies {
        p.constraints.external_network_allowed = Some(false);
    }
    catalog
}

fn shadow_policy() -> ShadowPolicy {
    let any = || vec![SHADOW_ALLOW_ANY.to_string()];
    ShadowPolicy {
        execute: true,
        allowlist: ShadowAllowlist {
            task_kinds: any(),
            roles: any(),
            lanes: any(),
            sources: any(),
        },
        sample_rate: 1.0,
        daily_max_requests: Some(10),
        daily_max_tokens: Some(10_000),
        daily_max_effective_usd: Some(1.0),
        max_concurrency: Some(1),
        max_queue_depth: Some(1),
        timeout_ms: Some(60_000),
    }
}

fn shadow_config(worst_usd: Option<f64>) -> EstimatorShadowConfig {
    EstimatorShadowConfig {
        shadow_only: true,
        policy: shadow_policy(),
        worst_call_effective_usd: worst_usd,
        worst_call_tokens: 16,
        resource_group: Some("sidecar-cpu".into()),
        prompt_allowlist: ShadowAllowlist::default(),
    }
}

async fn spawn_proxy(
    config: LlmProxyConfig,
    estimator: Option<Arc<EstimatorShadow>>,
) -> SocketAddr {
    let state = ProxyState::new(
        config,
        reqwest::Client::new(),
        None,
        None,
        None,
        SharedRole::default(),
        None,
        std::time::Duration::from_secs(5),
    )
    .with_estimator_shadow(estimator);
    spawn(router(state)).await
}

async fn post_chat(addr: SocketAddr) -> (Option<String>, String) {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&json!({
            "model": "qwen/cheap", "stream": false, "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}],
        }))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
    let source = resp
        .headers()
        .get("x-celeris-source")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (source, resp.text().await.expect("body"))
}

fn candidate(id: &str, config: &LlmProxyConfig) -> ShadowCandidate {
    ShadowCandidate {
        source: format!("openai-compatible:{id}"),
        model: config
            .models
            .qwen
            .get(&Tier::Cheap)
            .cloned()
            .expect("cheap qwen wire model"),
    }
}

#[tokio::test]
async fn routing_sidecar_cannot_override_constraints_or_primary() {
    // 1) shadow_only = false は作れない（sidecar に決定権を渡さない）。
    let (_, idle_sidecar) = spawn_sidecar(false).await;
    let base = relay_config(&[("ra", idle_sidecar)]);
    let refused = EstimatorShadow::new(
        EstimatorShadowConfig {
            shadow_only: false,
            ..shadow_config(Some(0.001))
        },
        client(idle_sidecar),
        Arc::new(catalog(&base)),
        "instance-a",
        Arc::new(AllowAllBudget::default()),
        Arc::new(Sink::default()),
        Arc::new(SystemClock),
    );
    assert!(matches!(
        refused,
        Err(EstimatorShadowError::ShadowOnlyRequired)
    ));

    // 2) kernel: privacy で除外した model-x を sidecar が 1.0 と評価しても選ばない。
    let (a, b, x) = (
        spawn_upstream().await,
        spawn_upstream().await,
        spawn_upstream().await,
    );
    let config = relay_config(&[("ra", a.1), ("rb", b.1), ("rc", x.1)]);
    let cat = catalog(&config);
    let candidates = vec![
        candidate("ra", &config),
        candidate("rb", &config),
        candidate("rc", &config),
    ];
    let request: EstimateRequestV1 = serde_json::from_value(json!({
        "request_id": "req-1", "context_features": {},
        "candidates": [{"model_profile_id": "model-a"}, {"model_profile_id": "model-b"}, {"model_profile_id": "model-x"}],
    }))
    .expect("request");
    let response: EstimateResponseV1 = serde_json::from_value(json!({
        "request_id": "req-1", "estimator_id": "route-test", "version": "1",
        "estimates": [
            {"model_profile_id": "model-a", "index": 0.5, "confidence": 0.9, "reasons": ["r"]},
            {"model_profile_id": "model-b", "index": 0.9, "confidence": 0.9, "reasons": ["r"]},
            {"model_profile_id": "model-x", "index": 1.0, "confidence": 1.0, "reasons": ["r"]},
        ],
        "dependencies": {"needs_network": false, "external_embeddings": false},
    }))
    .expect("response");
    let snapshot =
        SidecarEstimateSnapshot::from_response(&request, &response, &descriptor(), 65_536)
            .expect("valid snapshot");
    let policy = cat
        .policies
        .iter()
        .find(|p| p.lane == Tier::Cheap)
        .expect("cheap policy")
        .clone();
    let choice = kernel_choice(
        &cat,
        &policy,
        &RoutingContext::default(),
        &candidates,
        &snapshot,
    );
    assert_eq!(choice.eligible_model_ids, vec!["model-a", "model-b"]);
    assert_eq!(choice.chosen.as_ref(), Some(&candidates[1]));

    // 3) proxy: estimator 無しと有りで primary が一致し、primary は sidecar の完了を待たない。
    let legacy = spawn_proxy(config.clone(), None).await;
    let (sidecar, sidecar_addr) = spawn_sidecar(true).await;
    let sink = Arc::new(Sink::default());
    let budget = Arc::new(AllowAllBudget::default());
    let capacity = ReservationTable::new(CapacityLimits::default());
    let estimator = EstimatorShadow::new(
        shadow_config(Some(0.002)),
        client(sidecar_addr),
        Arc::new(cat.clone()),
        "instance-a",
        budget.clone(),
        sink.clone(),
        Arc::new(SystemClock),
    )
    .expect("shadow_only estimator")
    .with_capacity(capacity.clone());
    let shadowed = spawn_proxy(config.clone(), Some(estimator)).await;

    let expected = post_chat(legacy).await;
    assert_eq!(expected.0.as_deref(), Some("openai-compatible:ra"));
    let arrived = sidecar.arrived.notified();
    // sidecar は応答を止めている: それでも primary は返り、legacy と同じ。
    let before = post_chat(shadowed).await;
    assert_eq!(before, expected);
    arrived.await;
    assert!(sink.records().is_empty(), "sidecar has not answered yet");
    // sidecar の計算資源の枠を往復の間だけ持つ（resource pressure に数える）。
    assert_eq!(capacity.held(&SlotKey::group("sidecar-cpu")), 1);
    sidecar.release.notify_one();
    sink.wait_len(1).await;
    // sidecar 完了後も primary の決定は同じ。
    let arrived = sidecar.arrived.notified();
    let after = post_chat(shadowed).await;
    assert_eq!(after, expected);
    arrived.await;
    sidecar.release.notify_one();
    sink.wait_len(2).await;
    assert_eq!(capacity.held(&SlotKey::group("sidecar-cpu")), 0);

    // 除外候補（model-x）は sidecar に送られず、上流 rc / rb は呼ばれない（primary は ra だけ）。
    for req in sidecar
        .requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        let ids: Vec<&str> = req["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .filter_map(|c| c["model_profile_id"].as_str())
            .collect();
        assert_eq!(ids, vec!["model-a", "model-b"]);
        assert!(req.get("optional_prompt").is_none(), "prompt is not sent");
    }
    assert_eq!(b.0.chat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(x.0.chat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(sidecar.hits.load(Ordering::SeqCst), 2);

    // 記録: kind=estimator、estimator id/version、completed、比較、overhead、予約。
    let records = sink.records();
    assert_eq!(records.len(), 2);
    for r in &records {
        r.validate().expect("valid record");
        assert_eq!(r.kind, ShadowKind::Estimator);
        assert_eq!(r.status, ShadowStatus::Completed);
        assert_eq!(r.policy_version, "estimator:route-test/1");
        assert_eq!(r.candidate_source.as_deref(), Some("openai-compatible:rb"));
        assert_eq!(
            r.detail.as_deref(),
            Some("differs_from_primary;heuristic:differs_from_primary")
        );
        assert!(r.latency_ms.is_some());
        assert!(r.reservation_id.is_some());
        assert!(r.effective_usd.is_none() && r.input_tokens.is_none());
    }
    // 未計測の推論費用は予約した最悪値で確定する（ゼロにしない）。
    let settled = budget.settled();
    assert_eq!(settled.len(), 2);
    for (_, s) in settled {
        assert_eq!(
            s,
            ShadowSettlement::Completed {
                tokens: 16,
                effective_usd: 0.002
            }
        );
    }
}

#[tokio::test]
async fn routing_estimator_shadow_unknown_cost_and_full_capacity_drop_without_sidecar_call() {
    let (a, b) = (spawn_upstream().await, spawn_upstream().await);
    let config = relay_config(&[("ra", a.1), ("rb", b.1)]);
    let (sidecar, sidecar_addr) = spawn_sidecar(false).await;

    // 最悪費用が分からない: unknown_cost で dropped、sidecar は呼ばない。
    let sink = Arc::new(Sink::default());
    let estimator = EstimatorShadow::new(
        shadow_config(None),
        client(sidecar_addr),
        Arc::new(catalog(&config)),
        "instance-a",
        Arc::new(AllowAllBudget::default()),
        sink.clone(),
        Arc::new(SystemClock),
    )
    .expect("estimator");
    let proxy = spawn_proxy(config.clone(), Some(estimator)).await;
    assert_eq!(
        post_chat(proxy).await.0.as_deref(),
        Some("openai-compatible:ra")
    );
    sink.wait_len(1).await;
    let r = &sink.records()[0];
    assert_eq!(
        (r.kind, r.status, r.reason),
        (
            ShadowKind::Estimator,
            ShadowStatus::Dropped,
            Some(ShadowReason::UnknownCost)
        )
    );

    // sidecar の資源 group が primary 側で埋まっている: concurrency_limit で dropped、予約もしない。
    let sink = Arc::new(Sink::default());
    let budget = Arc::new(AllowAllBudget::default());
    let mut limits = CapacityLimits::default();
    limits.resource_groups.insert("sidecar-cpu".into(), 1);
    let capacity = ReservationTable::new(limits);
    let _held = capacity
        .try_reserve(&[SlotKey::group("sidecar-cpu")])
        .expect("primary holds the slot");
    let estimator = EstimatorShadow::new(
        shadow_config(Some(0.001)),
        client(sidecar_addr),
        Arc::new(catalog(&config)),
        "instance-a",
        budget.clone(),
        sink.clone(),
        Arc::new(SystemClock),
    )
    .expect("estimator")
    .with_capacity(capacity);
    let proxy = spawn_proxy(config, Some(estimator)).await;
    assert_eq!(
        post_chat(proxy).await.0.as_deref(),
        Some("openai-compatible:ra")
    );
    sink.wait_len(1).await;
    let r = &sink.records()[0];
    assert_eq!(
        (r.status, r.reason),
        (ShadowStatus::Dropped, Some(ShadowReason::ConcurrencyLimit))
    );
    assert!(budget.settled().is_empty());
    assert_eq!(sidecar.hits.load(Ordering::SeqCst), 0);
}
