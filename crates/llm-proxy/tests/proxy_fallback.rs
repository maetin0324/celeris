//! 同一要求内 fallback の統合テスト（ADR 2026-10-04-multi-objective-model-routing §5、Phase 2）。
//! 偽の上流（`127.0.0.1:0`）に 401/429/5xx・stream の途中失敗を返させ、`llm-proxy` を実際に HTTP で叩く。
//! 時刻は注入した手動の時計で進める（sleep しない）。half_open の試し打ちは偽上流の中で止めて
//! 「届いた」出来事を待ってから次の要求を送る。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use axum::body::Body;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use llm_proxy::config::{ClaudeOauthConfig, LlmProxyConfig, OpenAiCompatibleConfig};
use llm_proxy::fallback::{BreakerSettings, BreakerState, FallbackSettings, RetryLimits};
use llm_proxy::reservation::Clock;
use llm_proxy::{ProxyState, router};
use serde_json::{Value, json};
use task_core::SharedRole;
use task_core::Tier;
use task_dispatch::accounts::AccountBook;
use time::OffsetDateTime;
use time::macros::datetime;
use tokio::net::TcpListener;
use tokio::sync::Notify;

const T0: OffsetDateTime = datetime!(2026-10-05 00:00:00 UTC);

struct ManualClock(StdMutex<OffsetDateTime>);

impl ManualClock {
    fn set(&self, at: OffsetDateTime) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

async fn spawn(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

// ---------------------------------------------------------------------------
// 偽の relay（openai-compatible）
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Mode {
    Ok,
    Status(u16),
    /// 200 と header を返した後、最初の byte の前に本文を壊す。
    StreamErrBeforeFirst,
    /// 最初の event を返してから本文を壊す。
    StreamErrAfterFirst,
    /// `arrived` を知らせ、`release` まで応答を止める（その後 Ok）。
    Block,
}

struct FakeRelay {
    mode: StdMutex<Mode>,
    hits: AtomicU32,
    arrived: Notify,
    release: Notify,
}

impl FakeRelay {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode: StdMutex::new(mode),
            hits: AtomicU32::new(0),
            arrived: Notify::new(),
            release: Notify::new(),
        })
    }
    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    }
    fn hits(&self) -> u32 {
        self.hits.load(Ordering::SeqCst)
    }
}

const CHUNK: &str = "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"model\":\"qwen3.8-27b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n";

fn ok_response(stream: bool) -> axum::response::Response {
    if stream {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            format!("{CHUNK}data: [DONE]\n\n"),
        )
            .into_response();
    }
    Json(json!({
        "id": "1", "object": "chat.completion", "model": "qwen3.8-27b",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "relayed"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
    .into_response()
}

fn broken_stream(first: Option<&'static str>) -> axum::response::Response {
    use futures_util::StreamExt;
    let head = futures_util::stream::iter(first.map(|c| Ok(bytes::Bytes::from(c))));
    // 一度 Pending を返して header を先に flush させてから本文を壊す。
    let fail = futures_util::stream::once(async {
        tokio::task::yield_now().await;
        Err::<bytes::Bytes, _>(std::io::Error::other("broken"))
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(head.chain(fail)),
    )
        .into_response()
}

async fn relay_chat(
    AxumState(fake): AxumState<Arc<FakeRelay>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    fake.hits.fetch_add(1, Ordering::SeqCst);
    let stream = body["stream"].as_bool().unwrap_or(false);
    let mode = *fake.mode.lock().unwrap_or_else(|e| e.into_inner());
    match mode {
        Mode::Ok => ok_response(stream),
        Mode::Status(429) => (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "7")],
            "slow down",
        )
            .into_response(),
        Mode::Status(code) => (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            "failed",
        )
            .into_response(),
        Mode::StreamErrBeforeFirst => broken_stream(None),
        Mode::StreamErrAfterFirst => broken_stream(Some(CHUNK)),
        Mode::Block => {
            let released = fake.release.notified();
            fake.arrived.notify_one();
            released.await;
            ok_response(stream)
        }
    }
}

async fn relay_models() -> axum::response::Response {
    Json(json!({"object": "list", "data": [{"id": "qwen3.8-27b", "object": "model"}]}))
        .into_response()
}

async fn spawn_relay(mode: Mode) -> (Arc<FakeRelay>, SocketAddr) {
    let fake = FakeRelay::new(mode);
    let app = Router::new()
        .route("/v1/models", get(relay_models))
        .route("/v1/chat/completions", post(relay_chat))
        .with_state(fake.clone());
    (fake, spawn(app).await)
}

/// 制約の外（`qwen/…` に対する Claude）に倒れていないことを数えるだけの偽 Anthropic。
async fn spawn_counting_claude(hits: Arc<AtomicU32>) -> SocketAddr {
    let app = Router::new()
        .route(
            "/v1/messages",
            post(|AxumState(hits): AxumState<Arc<AtomicU32>>| async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (StatusCode::INTERNAL_SERVER_ERROR, "should not be called").into_response()
            }),
        )
        .with_state(hits);
    spawn(app).await
}

fn write_claude_credentials(dir: &std::path::Path, account_id: &str) {
    let account_dir = dir.join(account_id);
    std::fs::create_dir_all(&account_dir).expect("mkdir");
    let expires = (OffsetDateTime::now_utc().unix_timestamp() + 3600) * 1000;
    let value = json!({"claudeAiOauth": {
        "accessToken": format!("fake-access-{account_id}"), "refreshToken": "fake-refresh", "expiresAt": expires,
        "scopes": ["user:inference"], "subscriptionType": "max",
    }});
    std::fs::write(
        account_dir.join(".credentials.json"),
        serde_json::to_vec_pretty(&value).expect("json"),
    )
    .expect("write");
}

struct Proxy {
    addr: SocketAddr,
    state: Arc<ProxyState>,
}

async fn spawn_proxy(
    relays: &[(&str, SocketAddr)],
    claude: Option<(SocketAddr, &std::path::Path)>,
    settings: FallbackSettings,
    clock: Arc<ManualClock>,
) -> Proxy {
    spawn_proxy_with(relays, claude, settings, clock, None).await
}

async fn spawn_proxy_with(
    relays: &[(&str, SocketAddr)],
    claude: Option<(SocketAddr, &std::path::Path)>,
    settings: FallbackSettings,
    clock: Arc<ManualClock>,
    assignments: Option<task_core::model_catalog::assignments::AssignmentView>,
) -> Proxy {
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
    if let Some((addr, dir)) = claude {
        config.sources.claude_oauth = Some(ClaudeOauthConfig {
            accounts_dir: dir.to_path_buf(),
            base_url: format!("http://{addr}"),
            token_url: format!("http://{addr}/oauth/token"),
            client_id: "test-client".to_string(),
            enabled: true,
        });
    }
    let state = ProxyState::new(
        config,
        reqwest::Client::new(),
        Some(Arc::new(StdMutex::new(AccountBook::new_in_memory()))),
        None,
        None,
        SharedRole::default(),
        None,
        std::time::Duration::from_secs(5),
    )
    .with_fallback(settings, clock)
    .with_role_assignments(assignments.map(|view| {
        Arc::new(task_core::model_catalog::assignments::StaticAssignments(
            view,
        )) as Arc<dyn task_core::model_catalog::assignments::RoleAssignmentReader>
    }));
    let addr = spawn(router(state.clone())).await;
    Proxy { addr, state }
}

async fn post_chat(addr: SocketAddr, model: &str, stream: bool) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&json!({
            "model": model, "stream": stream,
            "messages": [{"role": "user", "content": "hi"}],
        }))
        .send()
        .await
        .expect("send")
}

fn source_of(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get("x-celeris-source")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn manual_clock() -> Arc<ManualClock> {
    Arc::new(ManualClock(StdMutex::new(T0)))
}

fn no_breaker() -> FallbackSettings {
    FallbackSettings {
        breaker: BreakerSettings {
            failure_threshold: 0,
            open_secs: 30,
        },
        ..FallbackSettings::default()
    }
}

#[tokio::test]
async fn routing_proxy_fallback_preserves_constraints_and_stream_boundary() {
    let claude_hits = Arc::new(AtomicU32::new(0));
    let claude_addr = spawn_counting_claude(claude_hits.clone()).await;
    let accounts = tempfile::tempdir().expect("tmp");
    write_claude_credentials(accounts.path(), "acct-a");
    let claude = Some((claude_addr, accounts.path()));

    // 1. 401 → 429 → 503 の順に適格候補（qwen の relay）へ倒れ、4 番目で成功する。
    //    `qwen/…` の制約は引き継がれ、Claude には一度も送らない。
    let (r1, a1) = spawn_relay(Mode::Status(401)).await;
    let (r2, a2) = spawn_relay(Mode::Status(429)).await;
    let (r3, a3) = spawn_relay(Mode::Status(503)).await;
    let (r4, a4) = spawn_relay(Mode::Ok).await;
    let relays = [("r1", a1), ("r2", a2), ("r3", a3), ("r4", a4)];
    let proxy = spawn_proxy(&relays, claude, no_breaker(), manual_clock()).await;
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(source_of(&resp).as_deref(), Some("openai-compatible:r4"));
    assert_eq!([r1.hits(), r2.hits(), r3.hits(), r4.hits()], [1, 1, 1, 1]);

    // 2. 適格候補が全部 5xx: 分類別上限（server=2）で 3 件目の後に止まり、4 件目へは送らない。
    //    制約の外の Claude へは倒さない。
    r4.set(Mode::Status(500));
    r1.set(Mode::Status(502));
    r2.set(Mode::Status(503));
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        [r1.hits(), r2.hits(), r3.hits(), r4.hits()],
        [2, 2, 2, 1],
        "server-class retry limit must stop before the 4th candidate"
    );
    assert_eq!(claude_hits.load(Ordering::SeqCst), 0, "constraint leaked");

    // 3. 総上限: total_attempts=2 なら 401・429 の後は 3 件目が健全でも送らず、最後の 429 と
    //    Retry-After を caller に返す。
    let (t1, b1) = spawn_relay(Mode::Status(401)).await;
    let (t2, b2) = spawn_relay(Mode::Status(429)).await;
    let (t3, b3) = spawn_relay(Mode::Ok).await;
    let limited = FallbackSettings {
        limits: RetryLimits {
            total_attempts: 2,
            ..RetryLimits::default()
        },
        ..no_breaker()
    };
    let proxy = spawn_proxy(
        &[("t1", b1), ("t2", b2), ("t3", b3)],
        None,
        limited,
        manual_clock(),
    )
    .await;
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp.headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
        Some("7")
    );
    assert_eq!([t1.hits(), t2.hits(), t3.hits()], [1, 1, 0]);

    // 4. deadline（注入時計）: 0 秒なら最初の失敗の時点で過ぎており、次の候補へ倒さない。
    let (d1, c1) = spawn_relay(Mode::Status(503)).await;
    let (d2, c2) = spawn_relay(Mode::Ok).await;
    let no_deadline = FallbackSettings {
        deadline_secs: 0,
        ..no_breaker()
    };
    let proxy = spawn_proxy(&[("d1", c1), ("d2", c2)], None, no_deadline, manual_clock()).await;
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!([d1.hits(), d2.hits()], [1, 0]);

    // 5. stream: 最初の byte の前に壊れたら次の候補へ送る。
    let (s1, e1) = spawn_relay(Mode::StreamErrBeforeFirst).await;
    let (s2, e2) = spawn_relay(Mode::Ok).await;
    let proxy = spawn_proxy(
        &[("s1", e1), ("s2", e2)],
        None,
        no_breaker(),
        manual_clock(),
    )
    .await;
    let resp = post_chat(proxy.addr, "qwen/cheap", true).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(source_of(&resp).as_deref(), Some("openai-compatible:s2"));
    let text = resp.text().await.expect("text");
    assert!(text.contains("\"content\":\"hi\""), "{text}");
    assert!(text.ends_with("data: [DONE]\n\n"), "{text}");
    assert_eq!([s1.hits(), s2.hits()], [1, 1]);

    //    最初の byte の後に壊れたら caller に返す（再送 0 回）。
    s1.set(Mode::StreamErrAfterFirst);
    let resp = post_chat(proxy.addr, "qwen/cheap", true).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(source_of(&resp).as_deref(), Some("openai-compatible:s1"));
    let text = resp.text().await.unwrap_or_default();
    assert!(text.contains("\"content\":\"hi\""), "{text}");
    assert!(!text.contains("[DONE]"), "{text}");
    assert_eq!(
        [s1.hits(), s2.hits()],
        [2, 1],
        "no resend after the first byte"
    );

    // 6. breaker: 1 回の 5xx で open、open の間は送らず次へ。deadline 後は half_open の試し打ちが
    //    同時 1 件だけ。遷移は注入時計で進める。
    let clock = manual_clock();
    let (h1, f1) = spawn_relay(Mode::Status(503)).await;
    let (h2, f2) = spawn_relay(Mode::Ok).await;
    let breaker = FallbackSettings {
        breaker: BreakerSettings {
            failure_threshold: 1,
            open_secs: 30,
        },
        ..FallbackSettings::default()
    };
    let proxy = spawn_proxy(
        &[("h1", f1), ("h2", f2)],
        None,
        breaker.clone(),
        clock.clone(),
    )
    .await;
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(source_of(&resp).as_deref(), Some("openai-compatible:h2"));
    let h1_deployment = "openai-compatible:h1/qwen3.8-27b";
    let h1_state = proxy.state.breakers().state(h1_deployment);
    assert_eq!(
        h1_state,
        BreakerState::Open {
            until: T0 + time::Duration::seconds(30)
        }
    );
    clock.set(T0 + time::Duration::seconds(29));
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(source_of(&resp).as_deref(), Some("openai-compatible:h2"));
    assert_eq!(h1.hits(), 1, "open breaker must not send");

    clock.set(T0 + time::Duration::seconds(30));
    h1.set(Mode::Block);
    let arrived = h1.arrived.notified();
    let addr = proxy.addr;
    let probe = tokio::spawn(async move { post_chat(addr, "qwen/cheap", false).await });
    arrived.await;
    assert_eq!(
        proxy.state.breakers().state(h1_deployment),
        BreakerState::HalfOpen {
            probe_in_flight: true
        }
    );
    // 試し打ちが飛んでいる間の要求は h1 を飛ばして h2 へ。
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(source_of(&resp).as_deref(), Some("openai-compatible:h2"));
    assert_eq!(h1.hits(), 2, "only one half-open probe at a time");
    assert_eq!(h2.hits(), 3);
    h1.release.notify_one();
    let resp = probe.await.expect("join");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(source_of(&resp).as_deref(), Some("openai-compatible:h1"));
    assert_eq!(
        proxy.state.breakers().state(h1_deployment),
        BreakerState::Closed {
            consecutive_failures: 0
        }
    );

    // 7. 候補が全部 open: 送らずに 503、Retry-After は最も早く開く時刻まで（注入時計）。
    let clock = manual_clock();
    let (o1, g1) = spawn_relay(Mode::Status(500)).await;
    let proxy = spawn_proxy(&[("o1", g1)], None, breaker, clock.clone()).await;
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    clock.set(T0 + time::Duration::seconds(10));
    let resp = post_chat(proxy.addr, "qwen/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        resp.headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
        Some("20")
    );
    assert_eq!(o1.hits(), 1);
}

/// 付記「モデルごとの複数役割」: 役割に同じ source のモデルが複数あるとき（claude-oauth cheap = m1, m2）、
/// 候補は priority 順（m1 を全 account、次に m2）。同じ要求の中で 429 を受けた account は残りのモデル候補からも
/// 外し、次の account へ進む。全 account が 429 なら m2 は一度も送らずに 429 を返す。
#[tokio::test]
async fn routing_proxy_role_members_skip_an_account_rejected_in_the_same_request() {
    use task_core::model_catalog::CatalogSource;
    use task_core::model_catalog::assignments::{
        AssignmentState, AssignmentView, EffectiveAssignment,
    };
    type Seen = Arc<StdMutex<Vec<(String, String)>>>;
    #[derive(Clone)]
    struct Fake {
        seen: Seen,
        reject_all: Arc<std::sync::atomic::AtomicBool>,
    }
    async fn claude_by_account(
        AxumState(fake): AxumState<Fake>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let model = body["model"].as_str().unwrap_or_default().to_string();
        fake.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((auth.clone(), model));
        if fake.reject_all.load(Ordering::SeqCst) || auth.ends_with("acct-a") {
            (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "30")]).into_response()
        } else {
            Json(json!({
                "id": "msg_ok",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }))
            .into_response()
        }
    }
    let fake = Fake {
        seen: Arc::new(StdMutex::new(Vec::new())),
        reject_all: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let app = Router::new()
        .route("/v1/messages", post(claude_by_account))
        .with_state(fake.clone());
    let claude_addr = spawn(app).await;
    let accounts = tempfile::tempdir().expect("tmp");
    write_claude_credentials(accounts.path(), "acct-a");
    write_claude_credentials(accounts.path(), "acct-b");
    let members = AssignmentView {
        managed: Vec::new(),
        items: ["m1", "m2"]
            .iter()
            .enumerate()
            .map(|(i, m)| EffectiveAssignment {
                priority: i as u32,
                source: CatalogSource::new("claude-oauth"),
                tier: Tier::Cheap,
                model_id: (*m).to_string(),
                state: AssignmentState::Assigned,
                note: None,
                updated_at: 1,
                updated_by: "admin".into(),
            })
            .collect(),
    };
    let proxy = spawn_proxy_with(
        &[],
        Some((claude_addr, accounts.path())),
        no_breaker(),
        manual_clock(),
        Some(members),
    )
    .await;

    // 1) acct-a が 429: acct-a の m2 は試さず acct-b の m1 で成功する。
    let resp = post_chat(proxy.addr, "claude/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(source_of(&resp).as_deref(), Some("claude-oauth"));
    let seen = fake.seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(
        seen,
        [
            ("Bearer fake-access-acct-a".to_string(), "m1".to_string()),
            ("Bearer fake-access-acct-b".to_string(), "m1".to_string()),
        ]
    );

    // 2) 全 account が 429: m1 を両 account で試した後、m2 は一度も送らずに 429 を返す。
    fake.seen.lock().unwrap_or_else(|e| e.into_inner()).clear();
    fake.reject_all.store(true, Ordering::SeqCst);
    // acct-a は 1) で cooldown に入っているので、この要求は acct-b から始まる。
    let resp = post_chat(proxy.addr, "claude/cheap", false).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let seen = fake.seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert!(
        seen.iter().all(|(_, model)| model == "m1"),
        "m2 must not be sent to an account that rejected m1: {seen:?}"
    );
    assert!(!seen.is_empty());
}
