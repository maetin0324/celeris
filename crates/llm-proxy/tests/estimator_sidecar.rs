//! ADR 2026-10-04 Phase 5: estimator sidecar client の試験。偽 sidecar は 127.0.0.1 の in-process server。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use llm_proxy::estimator_sidecar::{
    SidecarClientConfig, SidecarEstimatorClient, SidecarUnavailable, TokioClock,
};
use task_core::model_router::estimator::sidecar::{EstimateRequestV1, EstimatorDescriptor};
use tokio::net::TcpListener;
use tokio::sync::Notify;

fn fixture(name: &str) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../task-core/tests/fixtures/estimator_sidecar_v1/"
    );
    std::fs::read_to_string(format!("{path}{name}.json")).expect("fixture is checked in")
}

fn descriptor() -> EstimatorDescriptor {
    serde_json::from_str(&fixture("descriptor_valid")).expect("descriptor")
}

fn request() -> EstimateRequestV1 {
    serde_json::from_str(&fixture("request_valid")).expect("request")
}

#[derive(Clone)]
enum Mode {
    Body(String),
    Status(u16),
    Hang,
}

struct Fake {
    mode: Mutex<Mode>,
    hits: AtomicUsize,
    bodies: Mutex<Vec<serde_json::Value>>,
    arrived: Notify,
}

impl Fake {
    fn set(&self, mode: Mode) {
        *self.mode.lock().expect("mode") = mode;
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

async fn handle(State(fake): State<Arc<Fake>>, body: Bytes) -> Response {
    let mode = fake.mode.lock().expect("mode").clone();
    if let Ok(value) = serde_json::from_slice(&body) {
        fake.bodies.lock().expect("bodies").push(value);
    }
    fake.hits.fetch_add(1, Ordering::SeqCst);
    fake.arrived.notify_one();
    match mode {
        Mode::Body(body) => ([("content-type", "application/json")], body).into_response(),
        Mode::Status(code) => StatusCode::from_u16(code).expect("status").into_response(),
        Mode::Hang => std::future::pending().await,
    }
}

async fn fake_sidecar(mode: Mode) -> (Arc<Fake>, reqwest::Url) {
    let fake = Arc::new(Fake {
        mode: Mutex::new(mode),
        hits: AtomicUsize::new(0),
        bodies: Mutex::new(Vec::new()),
        arrived: Notify::new(),
    });
    let app = Router::new()
        .route("/estimate", post(handle))
        .with_state(fake.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    assert!(addr.ip().is_loopback());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = reqwest::Url::parse(&format!("http://{addr}/")).expect("url");
    (fake, url)
}

fn client(config: SidecarClientConfig) -> SidecarEstimatorClient {
    SidecarEstimatorClient::new(config, Arc::new(TokioClock)).expect("client")
}

fn config(url: &reqwest::Url) -> SidecarClientConfig {
    let mut config = SidecarClientConfig::new(url.clone(), descriptor());
    config.circuit_failure_threshold = 100;
    config
}

async fn reason_for(body: &str) -> &'static str {
    let (_fake, url) = fake_sidecar(Mode::Body(body.to_owned())).await;
    client(config(&url))
        .estimate(request())
        .await
        .expect_err("rejected")
        .reason()
}

#[tokio::test(flavor = "current_thread")]
async fn routing_sidecar_protocol_validates_identity_range_and_size() {
    // 正常: 検証済み snapshot を返し、cache に残す。
    let (fake, url) = fake_sidecar(Mode::Body(fixture("response_valid"))).await;
    let ok = client(config(&url));
    let snapshot = ok.estimate(request()).await.expect("valid estimate");
    assert_eq!(fake.hits(), 1);
    let cached = ok.cached_snapshot().expect("cached");
    assert!(Arc::ptr_eq(&snapshot, &cached));
    use task_core::model_router::estimator::QualityEstimator;
    let kernel = snapshot.descriptor();
    assert_eq!(
        (kernel.id.as_str(), kernel.version.as_str()),
        ("route-test", "1")
    );

    // 応答の検証失敗はすべて理由付きの評価不能。
    for (name, reason) in [
        ("response_out_of_range", "invalid_estimate"),
        ("response_unknown_id", "invalid_estimate"),
        ("response_missing_id", "invalid_estimate"),
        ("response_duplicate_id", "invalid_estimate"),
        ("response_wrong_request", "invalid_estimate"),
        ("response_wrong_version", "version_mismatch"),
    ] {
        assert_eq!(reason_for(&fixture(name)).await, reason, "{name}");
    }
    let deps =
        fixture("response_valid").replace(r#""needs_network":false"#, r#""needs_network":true"#);
    assert_eq!(reason_for(&deps).await, "dependency_mismatch");
    assert_eq!(reason_for("not json").await, "decode");

    // 期待 descriptor の protocol 版が違えば送らない。
    let (fake, url) = fake_sidecar(Mode::Body(fixture("response_valid"))).await;
    let mut wrong = config(&url);
    wrong.descriptor.protocol_version = 2;
    let err = client(wrong)
        .estimate(request())
        .await
        .expect_err("version");
    assert_eq!(err.reason(), "version_mismatch");
    assert_eq!(fake.hits(), 0);

    // 応答の大きさ上限。
    let (_fake, url) = fake_sidecar(Mode::Body(format!(
        "{}{}",
        fixture("response_valid"),
        " ".repeat(4096)
    )))
    .await;
    let mut small = config(&url);
    small.max_payload_bytes = 1024;
    let err = client(small).estimate(request()).await.expect_err("large");
    assert_eq!(err, SidecarUnavailable::PayloadTooLarge(1024));

    // request の大きさ上限: 送らない。
    let (fake, url) = fake_sidecar(Mode::Body(fixture("response_valid"))).await;
    let mut tiny = config(&url);
    tiny.max_payload_bytes = 16;
    let err = client(tiny).estimate(request()).await.expect_err("request");
    assert_eq!(err.reason(), "payload_too_large");
    assert_eq!(fake.hits(), 0);

    // HTTP の失敗。
    let (_fake, url) = fake_sidecar(Mode::Status(500)).await;
    let err = client(config(&url))
        .estimate(request())
        .await
        .expect_err("500");
    assert_eq!(err, SidecarUnavailable::Status(500));

    // timeout と max_inflight: 1 件目が sidecar に届いて止まっている間、2 件目は送らない。
    let (fake, url) = fake_sidecar(Mode::Hang).await;
    let mut hang = config(&url);
    hang.max_inflight = 1;
    hang.timeout = Duration::from_millis(300);
    let hang = Arc::new(client(hang));
    let arrived = fake.arrived.notified();
    let first = tokio::spawn({
        let hang = hang.clone();
        async move { hang.estimate(request()).await }
    });
    arrived.await;
    let err = hang.estimate(request()).await.expect_err("inflight");
    assert_eq!(err.reason(), "max_inflight");
    let err = first.await.expect("join").expect_err("timeout");
    assert_eq!(err, SidecarUnavailable::Timeout);
    assert_eq!(fake.hits(), 1);

    // 連続失敗で circuit を開き、開いている間は送らない。時計は tokio::time::pause で進める。
    let (fake, url) = fake_sidecar(Mode::Status(503)).await;
    let mut breaker = config(&url);
    breaker.circuit_failure_threshold = 2;
    breaker.circuit_open_for = Duration::from_secs(30);
    let breaker = client(breaker);
    for _ in 0..2 {
        let err = breaker.estimate(request()).await.expect_err("503");
        assert_eq!(err.reason(), "http_status");
    }
    assert_eq!(fake.hits(), 2);
    tokio::time::pause();
    assert!(breaker.circuit_open());
    let err = breaker.estimate(request()).await.expect_err("open");
    assert_eq!(err, SidecarUnavailable::CircuitOpen);
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(breaker.circuit_open());
    assert_eq!(fake.hits(), 2);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(!breaker.circuit_open());
    tokio::time::resume();
    fake.set(Mode::Body(fixture("response_valid")));
    breaker
        .estimate(request())
        .await
        .expect("half-open trial succeeds");
    assert_eq!(fake.hits(), 3);
    assert!(!breaker.circuit_open());
}

#[tokio::test(flavor = "current_thread")]
async fn routing_sidecar_privacy_and_dependencies_gate_prompt() {
    let mut with_prompt = request();
    with_prompt.optional_prompt = Some("secret prompt text".into());

    // send_prompt=false: prompt は送らない（body に現れない）。
    let (fake, url) = fake_sidecar(Mode::Body(fixture("response_valid"))).await;
    client(config(&url))
        .estimate(with_prompt.clone())
        .await
        .expect("estimate without prompt");
    let sent = fake.bodies.lock().expect("bodies").clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].get("optional_prompt").is_none());
    assert!(!sent[0].to_string().contains("secret"));

    // prompt 必須の estimator は send_prompt=false なら評価不能・送信 0。
    let (fake, url) = fake_sidecar(Mode::Body(fixture("response_valid"))).await;
    let mut needs = config(&url);
    needs.descriptor.needs_prompt = true;
    let err = client(needs.clone())
        .estimate(with_prompt.clone())
        .await
        .expect_err("prompt required");
    assert_eq!(err, SidecarUnavailable::PromptRequired);
    // send_prompt=true でも prompt が無ければ送らない。
    needs.send_prompt = true;
    let err = client(needs.clone())
        .estimate(request())
        .await
        .expect_err("no prompt");
    assert_eq!(err.reason(), "prompt_required");

    // network / 外部 embeddings 依存の申告は許可が無ければ送らない。
    for (network, embeddings) in [(true, false), (false, true)] {
        let mut deps = config(&url);
        deps.descriptor.dependencies.needs_network = network;
        deps.descriptor.dependencies.external_embeddings = embeddings;
        let err = client(deps).estimate(request()).await.expect_err("deps");
        assert_eq!(err.reason(), "dependencies_not_allowed");
    }
    assert_eq!(fake.hits(), 0, "gated requests must not reach the sidecar");

    // 許可された prompt は送る。
    client(needs)
        .estimate(with_prompt)
        .await
        .expect("prompt allowed");
    assert_eq!(fake.hits(), 1);
    let sent = fake.bodies.lock().expect("bodies").clone();
    assert_eq!(sent[0]["optional_prompt"], "secret prompt text");
}
