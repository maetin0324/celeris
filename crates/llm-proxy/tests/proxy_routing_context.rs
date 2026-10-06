//! ADR 2026-10-04 §10 Phase 3: `x-celeris-routing-context` の参照解決・偽装拒否・header 除去。
//!
//! 偽 adapter（この試験の reqwest client）が header 付きで proxy を叩き、proxy は偽上流
//! （`127.0.0.1:0` の openai-compatible relay）へ送る。偽上流は受けた header と本文を全部記録し、
//! header・ref・内部 run ID がどこにも出ていないことを確かめる。外部ネットワークには出ない。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use llm_proxy::config::{LlmProxyConfig, OpenAiCompatibleConfig};
use llm_proxy::routing_context::{ProxyEventSink, ProxyRoutingEvent, ROUTING_CONTEXT_HEADER};
use llm_proxy::{ProxyState, router};
use serde_json::{Value, json};
use task_core::SharedRole;
use task_core::model_router::context::RoutingContext;
use task_core::model_router::context_registry::{
    InMemoryRoutingContextRegistry, RoutingContextRegistry,
};
use task_core::model_router::feedback::FeatureStage;
use tokio::net::TcpListener;

const RUN_ID: &str = "run-internal-7f3a";
const TASK_ID: &str = "task-ctx-1";
const PARENT_DECISION: &str = "dec-run-1";

async fn spawn(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// 偽上流が受けた要求 1 件（header の名前と値、本文）。
type SeenRequest = (Vec<(String, String)>, Value);

/// 偽上流が受けたもの。
#[derive(Default)]
struct Seen {
    requests: StdMutex<Vec<SeenRequest>>,
}

async fn relay_models() -> axum::response::Response {
    Json(json!({"object": "list", "data": [{"id": "qwen3.8-27b", "object": "model"}]}))
        .into_response()
}

async fn relay_chat(
    AxumState(seen): AxumState<Arc<Seen>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let headers = headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    seen.requests.lock().expect("lock").push((headers, body));
    Json(json!({
        "id": "1", "object": "chat.completion", "model": "qwen3.8-27b",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6}
    }))
    .into_response()
}

#[derive(Default)]
struct RecordingSink {
    events: StdMutex<Vec<ProxyRoutingEvent>>,
}

impl ProxyEventSink for RecordingSink {
    fn parent_decision(&self, run_id: &str) -> Option<String> {
        (run_id == RUN_ID).then(|| PARENT_DECISION.to_string())
    }
    fn record(&self, event: ProxyRoutingEvent) {
        self.events.lock().expect("lock").push(event);
    }
}

fn make_db(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("celeris.sqlite3");
    let conn = rusqlite::Connection::open(&path).expect("open db");
    conn.execute_batch(include_str!(
        "../../task-core/migrations/0022_llm_proxy_requests.sql"
    ))
    .expect("0022");
    // 0048 は events の部分索引も作るので、最小の events を先に用意する。
    conn.execute_batch("CREATE TABLE events (seq INTEGER, task_id TEXT, json TEXT);")
        .expect("events");
    conn.execute_batch(include_str!(
        "../../task-core/migrations/0048_routing_log_correlation.sql"
    ))
    .expect("0048");
    path
}

struct Harness {
    proxy: SocketAddr,
    seen: Arc<Seen>,
    sink: Arc<RecordingSink>,
    registry: Arc<InMemoryRoutingContextRegistry>,
    state: Arc<ProxyState>,
    db: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

async fn harness(with_registry: bool) -> Harness {
    let seen = Arc::new(Seen::default());
    let upstream = spawn(
        Router::new()
            .route("/v1/models", get(relay_models))
            .route("/v1/chat/completions", post(relay_chat))
            .with_state(seen.clone()),
    )
    .await;
    let mut config = LlmProxyConfig {
        prefer_free: false,
        ..LlmProxyConfig::default()
    };
    config
        .sources
        .openai_compatible
        .push(OpenAiCompatibleConfig {
            id: "qwen".into(),
            base_url: format!("http://{upstream}/v1"),
            api_key: None,
            enabled: true,
        });
    let tmp = tempfile::tempdir().expect("tmp");
    let db = make_db(tmp.path());
    let registry = Arc::new(InMemoryRoutingContextRegistry::new());
    let sink = Arc::new(RecordingSink::default());
    let state = ProxyState::new(
        config,
        reqwest::Client::new(),
        None,
        None,
        None,
        SharedRole::default(),
        Some(db.clone()),
        Duration::from_secs(5),
    )
    .with_routing_context(
        with_registry.then(|| registry.clone() as Arc<dyn RoutingContextRegistry>),
        Some(sink.clone() as Arc<dyn ProxyEventSink>),
    );
    let proxy = spawn(router(state.clone())).await;
    Harness {
        proxy,
        seen,
        sink,
        registry,
        state,
        db,
        _tmp: tmp,
    }
}

/// daemon が登録する context（組織・優先度は daemon の出自。要求能力と context は厳しめ）。
fn registered_context() -> RoutingContext {
    RoutingContext {
        version: "1".into(),
        origin: "task".into(),
        task_id: Some(TASK_ID.into()),
        run_id: Some(RUN_ID.into()),
        org_node: Some("software-engineering".into()),
        role: Some("worker".into()),
        priority: Some(1),
        required_tools: true,
        input_tokens: Some(120_000),
        provenance: "dispatcher:task-metadata".into(),
        ..RoutingContext::default()
    }
}

/// 本文で組織・優先度・privacy を自己申告し、制約を緩めようとする要求。
fn spoofing_body() -> Value {
    json!({
        "model": "qwen/cheap",
        "messages": [{"role": "user", "content": "hello"}],
        "stream": false,
        "org": "cos",
        "org_node": "cos",
        "priority": 999,
        "privacy": "external_ok",
        "run_id": "run-spoofed",
        "metadata": {"celeris": {"org": "cos", "priority": 999, "privacy": "external_ok"}},
    })
}

async fn send_chat(h: &Harness, reference: Option<&str>) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(format!("http://{}/v1/chat/completions", h.proxy))
        .json(&spoofing_body());
    if let Some(r) = reference {
        req = req.header(ROUTING_CONTEXT_HEADER, r);
    }
    req.send().await.expect("send")
}

/// 直近の log 行の (run_id, task_id, decision_id, status)。
type LogRow = (Option<String>, Option<String>, Option<String>, String);

fn log_row(db: &std::path::Path) -> LogRow {
    let conn = rusqlite::Connection::open(db).expect("open");
    conn.query_row(
        "SELECT run_id, task_id, decision_id, status FROM llm_proxy_requests ORDER BY ts DESC, rowid DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .expect("row")
}

#[tokio::test]
async fn routing_context_ref_propagates_and_rejects_spoofing() {
    let h = harness(true).await;
    let reference = h.registry.register(
        RUN_ID,
        registered_context(),
        Duration::from_secs(600),
        Instant::now(),
    );

    // 1) 有効な ref: daemon 登録の context を使い、header・ref・run ID は上流に出ない。
    let resp = send_chat(&h, Some(&reference)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    {
        let seen = h.seen.requests.lock().expect("lock");
        assert_eq!(seen.len(), 1);
        let (headers, body) = &seen[0];
        for (name, value) in headers {
            assert!(
                !name.starts_with("x-celeris"),
                "celeris header leaked: {name}"
            );
            assert!(!value.contains(&reference), "ref leaked in {name}");
            assert!(!value.contains(RUN_ID), "run id leaked in {name}");
        }
        let text = body.to_string();
        assert!(!text.contains(&reference), "ref leaked into the body");
        assert!(!text.contains(RUN_ID), "run id leaked into the body");
        for spoofed in [
            "org", "org_node", "priority", "privacy", "run_id", "metadata",
        ] {
            assert!(body.get(spoofed).is_none(), "{spoofed} forwarded upstream");
        }
    }
    {
        let events = h.sink.events.lock().expect("lock");
        assert_eq!(events.len(), 1, "one task-correlated request");
        let ev = &events[0];
        assert_eq!(ev.task_id, TASK_ID);
        let f = &ev.features.features;
        // 本文の自己申告（cos / 999 / run-spoofed）は採らない。
        assert_eq!(f["org_node"], "software-engineering");
        assert_eq!(f["priority"], 1);
        assert_eq!(f["run_id"], RUN_ID);
        // 本文で要求能力・context を緩められない（tools 無し・短い本文でも登録値のまま）。
        assert_eq!(f["required_tools"], true);
        assert_eq!(f["input_tokens"], 120_000);
        assert_eq!(ev.features.stage, Some(FeatureStage::Proxy));
        assert_eq!(ev.request.run_id.as_deref(), Some(RUN_ID));
        assert_eq!(
            ev.request.parent_decision_id.as_deref(),
            Some(PARENT_DECISION)
        );
        assert_eq!(ev.request.decision_id, ev.features.decision_id);
        assert_eq!(
            ev.features.request_id.as_deref(),
            Some(ev.request.request_id.as_str())
        );
        assert_eq!(ev.request.attempts.len(), 1);
        assert_eq!(ev.request.attempts[0].source_id, "openai-compatible:qwen");
    }
    // run → request の相関が proxy log（0048 の欄）に残る。
    let (run, task, decision, status) = log_row(&h.db);
    assert_eq!(run.as_deref(), Some(RUN_ID));
    assert_eq!(task.as_deref(), Some(TASK_ID));
    assert_eq!(decision.as_deref(), Some(PARENT_DECISION));
    assert_eq!(status, "ok");
    // 要求が終われば in-flight の数は戻る。
    assert_eq!(h.state.in_flight_contexts().count(RUN_ID), 0);

    // 2) 登録の無い ref（偽造）は 400。上流へ送らず、sink にも渡さない。
    let resp = send_chat(&h, Some("ctx_forged")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["type"], "invalid_routing_context");

    // 3) 期限切れの ref も 400（既定の context に倒さない）。
    let expired = h
        .registry
        .register(RUN_ID, registered_context(), Duration::ZERO, Instant::now());
    let resp = send_chat(&h, Some(&expired)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["type"], "expired_routing_context");

    // release 済みの ref は無効。
    h.registry.release(RUN_ID);
    let resp = send_chat(&h, Some(&reference)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(h.seen.requests.lock().expect("lock").len(), 1);
    assert_eq!(h.sink.events.lock().expect("lock").len(), 1);

    // 4) header 無しは standalone: 動くが task には結び付けず（sink に渡さない）、本文の run_id も採らない。
    let resp = send_chat(&h, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(h.seen.requests.lock().expect("lock").len(), 2);
    assert_eq!(h.sink.events.lock().expect("lock").len(), 1);
    let (run, task, decision, status) = log_row(&h.db);
    assert_eq!((run, task, decision), (None, None, None));
    assert_eq!(status, "ok");
}

#[tokio::test]
async fn routing_context_header_without_a_registry_is_rejected() {
    // registry を持たない proxy は header を信頼できない（fail-closed）。
    let h = harness(false).await;
    let resp = send_chat(&h, Some("ctx_anything")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(h.seen.requests.lock().expect("lock").is_empty());
    // header 無しなら従来どおり動く。
    let resp = send_chat(&h, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
}
