//! shadow の統合テスト（ADR 2026-10-04-multi-objective-model-routing §7.1・§10 Phase 4）。
//! 偽の上流（`127.0.0.1:0`）を立て、`llm-proxy` を実際に HTTP で叩く。偽上流は受けた要求を全て
//! 数える（chat と probe の `/v1/models` の両方）。待ちは出来事（Notify）で行い、sleep しない。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use axum::extract::State as AxumState;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use llm_proxy::config::{LlmProxyConfig, OpenAiCompatibleConfig};
use llm_proxy::reservation::SystemClock;
use llm_proxy::shadow::{
    AllowAllBudget, DecisionPolicy, ProxyShadow, RelayShadowExecutor, ShadowCandidate, ShadowEvent,
    ShadowQueue, ShadowSink,
};
use llm_proxy::{ProxyState, router};
use serde_json::{Value, json};
use task_core::SharedRole;
use task_core::model_router::shadow::{
    SHADOW_ALLOW_ANY, ShadowAllowlist, ShadowKind, ShadowPolicy, ShadowReason, ShadowRecord,
    ShadowStatus, output_sha256,
};
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

const SHADOW_BODY: &str = r#"{"id":"s","object":"chat.completion","model":"qwen-b","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"write_file","arguments":"{}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}"#;

/// 偽の openai-compatible 上流。`block` なら chat は `release` まで止まる。
struct Fake {
    chat_hits: AtomicU32,
    model_hits: AtomicU32,
    bodies: StdMutex<Vec<Value>>,
    block: bool,
    tool_calls: bool,
    arrived: Notify,
    release: Notify,
}

impl Fake {
    fn hits(&self) -> (u32, u32) {
        (
            self.chat_hits.load(Ordering::SeqCst),
            self.model_hits.load(Ordering::SeqCst),
        )
    }
}

async fn fake_chat(
    AxumState(f): AxumState<Arc<Fake>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    f.chat_hits.fetch_add(1, Ordering::SeqCst);
    let stream = body["stream"].as_bool().unwrap_or(false);
    f.bodies
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(body);
    if f.block {
        let released = f.release.notified();
        f.arrived.notify_one();
        released.await;
    }
    if f.tool_calls {
        return (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            SHADOW_BODY,
        )
            .into_response();
    }
    if stream {
        return (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n",
        )
            .into_response();
    }
    Json(json!({
        "id": "1", "object": "chat.completion", "model": "m",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "primary"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
    .into_response()
}

async fn fake_models(AxumState(f): AxumState<Arc<Fake>>) -> axum::response::Response {
    f.model_hits.fetch_add(1, Ordering::SeqCst);
    Json(json!({"object": "list", "data": [{"id": "m", "object": "model"}]})).into_response()
}

async fn spawn_fake(block: bool, tool_calls: bool) -> (Arc<Fake>, SocketAddr) {
    let fake = Arc::new(Fake {
        chat_hits: AtomicU32::new(0),
        model_hits: AtomicU32::new(0),
        bodies: StdMutex::new(Vec::new()),
        block,
        tool_calls,
        arrived: Notify::new(),
        release: Notify::new(),
    });
    let app = Router::new()
        .route("/v1/models", get(fake_models))
        .route("/v1/chat/completions", post(fake_chat))
        .with_state(fake.clone());
    (fake, spawn(app).await)
}

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

/// 候補列の最後を選ぶ比較 policy（primary と違う判断を作る）。
struct PickLast;

impl DecisionPolicy for PickLast {
    fn version(&self) -> String {
        "test:pick-last/v1".to_string()
    }
    fn choose(&self, candidates: &[ShadowCandidate]) -> Option<usize> {
        candidates.len().checked_sub(1)
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

async fn spawn_proxy(config: LlmProxyConfig, shadow: Option<ProxyShadow>) -> SocketAddr {
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
    .with_shadow(shadow);
    spawn(router(state)).await
}

async fn post_chat(addr: SocketAddr, stream: bool) -> (Option<String>, String) {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&json!({
            "model": "qwen/cheap", "stream": stream, "max_tokens": 32,
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

fn execute_policy() -> ShadowPolicy {
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

#[tokio::test]
async fn routing_decision_shadow_never_calls_upstream_or_changes_primary() {
    // 同じ構成の proxy を shadow 無し（legacy）と decision shadow 有りで立て、同じ要求を送る。
    let (legacy_a, la) = spawn_fake(false, false).await;
    let (legacy_b, lb) = spawn_fake(false, false).await;
    let legacy = spawn_proxy(relay_config(&[("ra", la), ("rb", lb)]), None).await;

    let (shadow_a, sa) = spawn_fake(false, false).await;
    let (shadow_b, sb) = spawn_fake(false, false).await;
    let sink = Arc::new(Sink::default());
    let budget = Arc::new(AllowAllBudget::default());
    // execute = false（既定）の policy では実行 queue は作られない。
    let execution = ShadowQueue::new(
        ShadowPolicy::default(),
        "instance-a",
        Arc::new(RelayShadowExecutor::new(reqwest::Client::new(), Vec::new())),
        budget,
        sink.clone(),
        Arc::new(SystemClock),
    );
    assert!(execution.is_none());
    let shadowed = spawn_proxy(
        relay_config(&[("ra", sa), ("rb", sb)]),
        Some(ProxyShadow {
            decision: Some(Arc::new(PickLast)),
            execution,
            sink: sink.clone(),
            cost: Some(Arc::new(|_: &str, _: &str, _: u64| Some(0.0))),
            default_output_reserve: 1024,
        }),
    )
    .await;

    for stream in [false, true] {
        let expected = post_chat(legacy, stream).await;
        let got = post_chat(shadowed, stream).await;
        // primary の選択と応答は legacy と同じ。
        assert_eq!(got, expected, "stream={stream}");
        assert_eq!(got.0.as_deref(), Some("openai-compatible:ra"));
    }
    // 追加 HTTP 0: 上流が受けた要求（chat と probe）は legacy と同数で、shadow の候補 rb の chat は 0。
    assert_eq!(shadow_a.hits(), legacy_a.hits());
    assert_eq!(shadow_b.hits(), legacy_b.hits());
    assert_eq!(shadow_b.hits().0, 0);
    assert_eq!(shadow_a.hits().0, 2);

    // 候補比較だけが記録される（tokens・費用・出力 hash・予約は無い）。
    let records = sink.records();
    assert_eq!(records.len(), 2);
    for r in &records {
        r.validate().expect("valid");
        assert_eq!(r.kind, ShadowKind::Decision);
        assert_eq!(r.status, ShadowStatus::Completed);
        assert_eq!(r.policy_version, "test:pick-last/v1");
        assert_eq!(r.candidate_source.as_deref(), Some("openai-compatible:rb"));
        assert_eq!(r.detail.as_deref(), Some("differs_from_primary"));
        assert!(r.primary_decision_id.starts_with("pdec_"));
        assert!(r.input_tokens.is_none() && r.output_sha256.is_none());
        assert!(r.reservation_id.is_none());
    }
}

#[tokio::test]
async fn routing_execution_shadow_copies_request_without_delaying_primary() {
    // primary（ra）は即答し、shadow の候補（rb）は release まで止まる。rb は tool call を返す。
    let (primary, pa) = spawn_fake(false, false).await;
    let (cand, ca) = spawn_fake(true, true).await;
    let config = relay_config(&[("ra", pa), ("rb", ca)]);
    let sink = Arc::new(Sink::default());
    let budget = Arc::new(AllowAllBudget::default());
    let queue = ShadowQueue::new(
        execute_policy(),
        "instance-a",
        Arc::new(RelayShadowExecutor::new(
            reqwest::Client::new(),
            config.sources.openai_compatible.clone(),
        )),
        budget.clone(),
        sink.clone(),
        Arc::new(SystemClock),
    )
    .expect("valid execute policy");
    let proxy = spawn_proxy(
        config,
        Some(ProxyShadow {
            decision: Some(Arc::new(PickLast)),
            execution: Some(queue),
            sink: sink.clone(),
            cost: Some(Arc::new(|_: &str, _: &str, _: u64| Some(0.0))),
            default_output_reserve: 1024,
        }),
    )
    .await;

    let arrived = cand.arrived.notified();
    // shadow の上流が止まったままでも primary は返る。
    let (source, body) = post_chat(proxy, true).await;
    assert_eq!(source.as_deref(), Some("openai-compatible:ra"));
    assert!(body.contains("[DONE]"));
    arrived.await;
    assert_eq!(primary.hits().0, 1);
    // decision の記録だけが先にあり、実行 shadow はまだ終わっていない。
    assert_eq!(sink.records().len(), 1);

    cand.release.notify_one();
    sink.wait_len(2).await;
    let exec = sink
        .records()
        .into_iter()
        .find(|r| r.kind == ShadowKind::Execution)
        .expect("execution record");
    exec.validate().expect("valid");
    assert_eq!((exec.status, exec.reason), (ShadowStatus::Completed, None));
    assert_eq!(
        exec.candidate_source.as_deref(),
        Some("openai-compatible:rb")
    );
    assert_eq!(
        exec.output_sha256.as_deref(),
        Some(output_sha256(SHADOW_BODY.as_bytes()).as_str())
    );
    assert_eq!((exec.input_tokens, exec.output_tokens), (Some(5), Some(3)));
    // tool call は実行せず、続きの turn も送らない（rb の chat は 1 回だけ）。要求はコピーで stream=false。
    assert_eq!(cand.hits().0, 1);
    let sent = cand.bodies.lock().unwrap_or_else(|e| e.into_inner())[0].clone();
    assert_eq!(sent["stream"], false);
    assert_eq!(sent["messages"][0]["content"], "hi");
    assert_eq!(sent["max_tokens"], 32);
    assert_eq!(budget.settled().len(), 1);
}

/// 差し替えられる偽時計（UTC 日界を跨ぐ）。
struct MutClock(StdMutex<time::OffsetDateTime>);

impl MutClock {
    fn new(at: time::OffsetDateTime) -> Arc<Self> {
        Arc::new(Self(StdMutex::new(at)))
    }
    fn set(&self, at: time::OffsetDateTime) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }
    fn now_utc(&self) -> time::OffsetDateTime {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl llm_proxy::reservation::Clock for MutClock {
    fn now(&self) -> time::OffsetDateTime {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 1 instance（proxy + 共有 DB の予約）を立てる。`store` を開き直せば再起動と同じ。
async fn spawn_capped_proxy(
    config: &LlmProxyConfig,
    policy: ShadowPolicy,
    owner: &str,
    store: Arc<task_core::store::SqliteStore>,
    clock: Arc<MutClock>,
    sink: Arc<Sink>,
    cost_usd: Option<f64>,
) -> SocketAddr {
    let budget = llm_proxy::shadow_budget::StoreShadowBudget::new(store, &policy, clock.clone());
    let execution = budget.and_then(|budget| {
        ShadowQueue::new(
            policy,
            owner,
            Arc::new(RelayShadowExecutor::new(
                reqwest::Client::new(),
                config.sources.openai_compatible.clone(),
            )),
            Arc::new(budget),
            sink.clone(),
            clock,
        )
    });
    spawn_proxy(
        config.clone(),
        Some(ProxyShadow {
            decision: None,
            execution,
            sink,
            cost: Some(Arc::new(move |_: &str, _: &str, _: u64| cost_usd)),
            default_output_reserve: 1024,
        }),
    )
    .await
}

fn count(records: &[ShadowRecord], status: ShadowStatus, reason: Option<ShadowReason>) -> usize {
    records
        .iter()
        .filter(|r| r.status == status && r.reason == reason)
        .count()
}

fn rows(path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("open db");
    conn.query_row(
        "SELECT COUNT(*) FROM routing_shadow_reservations",
        [],
        |r| r.get(0),
    )
    .expect("count")
}

fn assert_within(
    store: &task_core::store::SqliteStore,
    policy: &ShadowPolicy,
    now: time::OffsetDateTime,
) -> task_core::model_router::shadow::ShadowDailyUsage {
    let caps = policy.daily_caps().expect("caps");
    let usage = store.routing_shadow_usage(now).expect("usage");
    assert!(usage.requests <= caps.max_requests, "{usage:?}");
    assert!(usage.tokens <= caps.max_tokens, "{usage:?}");
    assert!(
        usage.effective_usd <= caps.max_effective_usd + 1e-9,
        "{usage:?}"
    );
    assert_eq!(usage.open_reservations, 0, "{usage:?}");
    usage
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn routing_shadow_opt_in_caps_survive_restart_and_handoff() {
    use time::macros::datetime;

    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("celeris.db");
    let open = || Arc::new(task_core::store::SqliteStore::open(&db).expect("open temp db"));
    // primary は ra、shadow の候補は rb（rb の chat は shadow だけが叩く）。
    let (_primary, pa) = spawn_fake(false, false).await;
    let (cand, ca) = spawn_fake(false, false).await;
    let config = relay_config(&[("ra", pa), ("rb", ca)]);
    let clock = MutClock::new(datetime!(2026-10-05 23:58:00 UTC));
    let sink = Arc::new(Sink::default());
    // 1 件の最悪消費: input "hi"（1 token）+ max_tokens 32 = 33 tokens、費用は 0.1 USD。
    let capped = |requests: u64, tokens: u64, usd: f64| ShadowPolicy {
        daily_max_requests: Some(requests),
        daily_max_tokens: Some(tokens),
        daily_max_effective_usd: Some(usd),
        max_concurrency: Some(8),
        max_queue_depth: Some(8),
        ..execute_policy()
    };

    // A. off（既定）と allowlist 外は偽上流への送信 0・予約 0・記録 0。
    let store = open();
    let off = spawn_capped_proxy(
        &config,
        ShadowPolicy::default(),
        "a",
        store.clone(),
        clock.clone(),
        sink.clone(),
        Some(0.1),
    )
    .await;
    let mut other_source = capped(10, 10_000, 10.0);
    other_source.allowlist.sources = vec!["openai-compatible:zz".to_string()];
    let outside = spawn_capped_proxy(
        &config,
        other_source,
        "a",
        store.clone(),
        clock.clone(),
        sink.clone(),
        Some(0.1),
    )
    .await;
    for addr in [off, outside] {
        let (source, _) = post_chat(addr, false).await;
        assert_eq!(source.as_deref(), Some("openai-compatible:ra"));
    }
    assert_eq!(cand.hits().0, 0);
    assert_eq!(rows(&db), 0);
    assert!(sink.records().is_empty());

    // B. 未知の費用は送らず dropped(unknown_cost)、予約 0。
    let unknown = spawn_capped_proxy(
        &config,
        capped(10, 10_000, 10.0),
        "a",
        store.clone(),
        clock.clone(),
        sink.clone(),
        None,
    )
    .await;
    post_chat(unknown, false).await;
    sink.wait_len(1).await;
    assert_eq!(
        count(
            &sink.records(),
            ShadowStatus::Dropped,
            Some(ShadowReason::UnknownCost)
        ),
        1
    );
    assert_eq!(cand.hits().0, 0);
    assert_eq!(rows(&db), 0);

    // C. request 上限 2: 3 件目は dropped(cap_exceeded)。再起動（store と proxy を作り直す）しても
    //    同じ UTC 日なら共有 DB の予約を数えて送らない。
    let day1 = capped(2, 10_000, 10.0);
    let a = spawn_capped_proxy(
        &config,
        day1.clone(),
        "a",
        store.clone(),
        clock.clone(),
        sink.clone(),
        Some(0.1),
    )
    .await;
    for n in 0..3 {
        post_chat(a, false).await;
        sink.wait_len(2 + n).await;
    }
    let recs = sink.records();
    assert_eq!(count(&recs, ShadowStatus::Completed, None), 2);
    assert_eq!(
        count(
            &recs,
            ShadowStatus::Dropped,
            Some(ShadowReason::CapExceeded)
        ),
        1
    );
    assert_eq!(cand.hits().0, 2);
    drop(store);
    let store = open();
    clock.set(datetime!(2026-10-05 23:59:59 UTC));
    let restarted = spawn_capped_proxy(
        &config,
        day1.clone(),
        "a2",
        store.clone(),
        clock.clone(),
        sink.clone(),
        Some(0.1),
    )
    .await;
    post_chat(restarted, false).await;
    sink.wait_len(5).await;
    assert_eq!(
        count(
            &sink.records(),
            ShadowStatus::Dropped,
            Some(ShadowReason::CapExceeded)
        ),
        2
    );
    assert_eq!(cand.hits().0, 2);
    let usage = assert_within(&store, &day1, clock.now_utc());
    assert_eq!(usage.requests, 2);

    // D. UTC 日界を跨ぐと新しい日の上限で数える（+09:00 の 2026-10-06 08:59 はまだ 10-05）。
    clock.set(datetime!(2026-10-06 08:59:00 +09:00));
    post_chat(restarted, false).await;
    sink.wait_len(6).await;
    assert_eq!(cand.hits().0, 2);
    clock.set(datetime!(2026-10-06 00:00:01 UTC));
    post_chat(restarted, false).await;
    sink.wait_len(7).await;
    assert_eq!(cand.hits().0, 3);
    assert_eq!(count(&sink.records(), ShadowStatus::Completed, None), 3);

    // E. token 上限 70: 予約は最悪 33 tokens で数える。完了で実測（2 tokens）に確定するので
    //    33 → 2+33 → 4+33 → 6+33 = 39 … と入り、和 + 33 が 70 を越える所で止まる。
    clock.set(datetime!(2026-10-07 12:00:00 UTC));
    let day3 = capped(100, 70, 100.0);
    let tok = spawn_capped_proxy(
        &config,
        day3.clone(),
        "a3",
        store.clone(),
        clock.clone(),
        sink.clone(),
        Some(0.1),
    )
    .await;
    let before = sink.records().len();
    let hits_before = cand.hits().0;
    for n in 0..20 {
        post_chat(tok, false).await;
        sink.wait_len(before + n + 1).await;
    }
    let recs: Vec<ShadowRecord> = sink.records().split_off(before);
    let done = count(&recs, ShadowStatus::Completed, None);
    // 和 2k + 33 <= 70 を満たす k = 0..=18 → 19 件、20 件目で止まる。
    assert_eq!(done, 19);
    assert_eq!(
        count(
            &recs,
            ShadowStatus::Dropped,
            Some(ShadowReason::CapExceeded)
        ),
        1
    );
    assert_eq!(cand.hits().0 - hits_before, 19);
    assert_within(&store, &day3, clock.now_utc());

    // F. handoff: 同じ DB を別々に開いた 2 instance が同時に要求を受ける。effective 上限 0.35 USD
    //    （1 件 0.1）なので、どう競合しても 2 instance 合わせて 3 件だけが送られる。
    clock.set(datetime!(2026-10-08 06:00:00 UTC));
    let day4 = capped(100, 100_000, 0.35);
    let store_b = open();
    let a = spawn_capped_proxy(
        &config,
        day4.clone(),
        "old-daemon",
        store.clone(),
        clock.clone(),
        sink.clone(),
        Some(0.1),
    )
    .await;
    let b = spawn_capped_proxy(
        &config,
        day4.clone(),
        "new-daemon",
        store_b.clone(),
        clock.clone(),
        sink.clone(),
        Some(0.1),
    )
    .await;
    let before = sink.records().len();
    let hits_before = cand.hits().0;
    let sends: Vec<_> = (0..8)
        .map(|i| tokio::spawn(post_chat(if i % 2 == 0 { a } else { b }, false)))
        .collect();
    for s in sends {
        s.await.expect("join");
    }
    sink.wait_len(before + 8).await;
    let recs: Vec<ShadowRecord> = sink.records().split_off(before);
    assert_eq!(count(&recs, ShadowStatus::Completed, None), 3);
    assert_eq!(
        count(
            &recs,
            ShadowStatus::Dropped,
            Some(ShadowReason::CapExceeded)
        ),
        5
    );
    assert_eq!(cand.hits().0 - hits_before, 3);
    let usage = assert_within(&store_b, &day4, clock.now_utc());
    assert_eq!(usage.requests, 3);
    // 予約の行は送った数と同じ（拒否・対象外は行を書かない）。
    assert_eq!(rows(&db), i64::from(cand.hits().0));
    for r in sink.records() {
        r.validate().expect("valid");
    }
}
