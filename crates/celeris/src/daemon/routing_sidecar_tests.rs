//! daemon の estimator sidecar 配線の試験（ADR 2026-10-04 §7.3・§10 Phase 5）。偽の上流と偽 sidecar は
//! どちらも 127.0.0.1:0 の in-process server（外部 network に出ない）。待ちは記録の到着（Notify）で行い、
//! 長い保険だけを置く。日次上限は固定時計（同じ UTC 日）で数える。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};

use llm_proxy::estimator_sidecar::TokioClock;
use llm_proxy::reservation::FixedClock;
use llm_proxy::shadow::ShadowEvent;
use serde_json::{Value, json};
use task_core::SharedRole;
use task_core::model_router::shadow::{ShadowKind, ShadowReason, ShadowRecord, ShadowStatus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

use super::*;
use crate::Config;
use crate::daemon::admin::reload_providers;
use crate::daemon::bootstrap::build_dispatcher;

type Handler = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// 1 要求ごとに閉じる HTTP/1.1 の偽 server。`handler(request line, body)` が JSON 本文を返す。
async fn fake_http(handler: Handler) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body) = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            let head = text[..end].lines().next().unwrap_or("").to_string();
                            break (head, text[end + 4..].to_string());
                        }
                    }
                };
                let out = handler(&head, &body);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                    out.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    addr
}

/// 偽の openai-compatible 上流（`/models` と `/chat/completions`）。
async fn fake_upstream() -> SocketAddr {
    fake_http(Arc::new(|head: &str, _: &str| {
        if head.contains("/models") {
            json!({"object": "list", "data": [{"id": "m", "object": "model"}]}).to_string()
        } else {
            json!({
                "id": "1", "object": "chat.completion", "model": "m",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "primary"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
            .to_string()
        }
    }))
    .await
}

/// 偽 sidecar。受けた本文を残し、候補ごとに固定の index を返す。`needs_network` を申告させられる。
struct Sidecar {
    hits: AtomicU32,
    bodies: std::sync::Mutex<Vec<Value>>,
    needs_network: bool,
}

async fn fake_sidecar(needs_network: bool) -> (Arc<Sidecar>, SocketAddr) {
    let sidecar = Arc::new(Sidecar {
        hits: AtomicU32::new(0),
        bodies: std::sync::Mutex::new(Vec::new()),
        needs_network,
    });
    let s = Arc::clone(&sidecar);
    let addr = fake_http(Arc::new(move |_: &str, body: &str| {
        s.hits.fetch_add(1, Ordering::SeqCst);
        let body: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        s.bodies.lock().unwrap().push(body.clone());
        let estimates: Vec<Value> = body["candidates"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| {
                json!({"model_profile_id": c["model_profile_id"], "index": 0.7, "confidence": 0.8,
                       "reasons": ["classifier_score"]})
            })
            .collect();
        json!({
            "request_id": body["request_id"], "estimator_id": "route-test", "version": "1",
            "estimates": estimates,
            "dependencies": {"needs_network": s.needs_network, "external_embeddings": false},
        })
        .to_string()
    }))
    .await;
    (sidecar, addr)
}

/// 記録を貯める sink（到着を Notify で知らせる）。
#[derive(Default)]
struct Sink {
    records: std::sync::Mutex<Vec<ShadowRecord>>,
    changed: Notify,
}

impl Sink {
    fn records(&self) -> Vec<ShadowRecord> {
        self.records.lock().unwrap().clone()
    }

    async fn wait_len(&self, n: usize) -> Vec<ShadowRecord> {
        let wait = async {
            loop {
                let changed = self.changed.notified();
                if self.records().len() >= n {
                    return;
                }
                changed.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(60), wait)
            .await
            .expect("estimator shadow record did not arrive");
        self.records()
    }
}

impl ShadowSink for Sink {
    fn record(&self, event: ShadowEvent) {
        self.records.lock().unwrap().push(event.record);
        self.changed.notify_waiters();
    }
}

/// relay `ra` 1 本の proxy。kernel が context 長で除外しないよう model の context 上限を宣言する。
fn base(dir: &std::path::Path, upstream: SocketAddr) -> String {
    format!(
        "db = {:?}\nworkspace_root = {:?}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n\
         [llm_proxy]\nenabled = true\nlisten = \"127.0.0.1:0\"\n\
         [[llm_proxy.sources.openai_compatible]]\nid = \"ra\"\nbase_url = \"http://{upstream}/v1\"\n\
         [[model_routing.models]]\nid = \"legacy:qwen:qwen3.8-27b\"\n\
         [model_routing.models.context_limits]\ninput = 100000\noutput = 8192\ntotal = 108192\n",
        dir.join("db.sqlite3"),
        dir.join("ws"),
    )
}

const ANY: &str = "task_kinds = [\"*\"]\nroles = [\"*\"]\nlanes = [\"*\"]\nsources = [\"*\"]\n";

/// 有効な sidecar 設定。`extra` は `[model_routing.estimator.sidecar]` に足す行、`allowlist` は対象。
fn sidecar(addr: SocketAddr, daily: u64, extra: &str, allowlist: &str) -> String {
    format!(
        "[model_routing.estimator.sidecar]\nenabled = true\nshadow_only = true\n\
         endpoint = \"http://{addr}/\"\nestimator_id = \"route-test\"\nestimator_version = \"1\"\n\
         timeout_ms = 30000\ndaily_max_requests = {daily}\n{extra}\
         [model_routing.estimator.sidecar.allowlist]\n{allowlist}"
    )
}

struct Harness {
    path: std::path::PathBuf,
    config: Config,
    dispatcher: task_dispatch::Dispatcher,
    control: Arc<EstimatorSidecarControl>,
    sink: Arc<Sink>,
    proxy: SocketAddr,
}

impl Harness {
    /// daemon と同じ順で組む: config → dispatcher → control（差し替え口）→ proxy。sink だけ試験用。
    async fn start(dir: &std::path::Path, text: String) -> Self {
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        let mut config = Config::load(&path).unwrap();
        let dispatcher = build_dispatcher(&config, Default::default()).unwrap();
        let sink = Arc::new(Sink::default());
        let control = Arc::new(EstimatorSidecarControl::new(SidecarDeps {
            sink: Arc::clone(&sink) as Arc<dyn ShadowSink>,
            clock: Arc::new(FixedClock(
                time::OffsetDateTime::from_unix_timestamp(1_791_201_600).unwrap(),
            )),
            monotonic: Arc::new(TokioClock),
            owner: "instance-test".into(),
        }));
        control.apply(
            &config.model_routing.estimator.sidecar,
            proxy_catalog(&config),
        );
        config.model_routing.estimator_sidecar_control = Some(Arc::clone(&control));
        let state = llm_proxy::ProxyState::new(
            config.llm_proxy.clone(),
            reqwest::Client::new(),
            None,
            None,
            None,
            SharedRole::default(),
            None,
            Duration::from_secs(5),
        )
        .with_estimator_shadow_slot(control.slot());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = listener.local_addr().unwrap();
        tokio::spawn(llm_proxy::serve(
            listener,
            state,
            std::future::pending::<()>(),
        ));
        Self {
            path,
            config,
            dispatcher,
            control,
            sink,
            proxy,
        }
    }

    /// 管理 API の reload と同じ関数で設定を差し替える。
    fn reload(&mut self, text: String) {
        std::fs::write(&self.path, text).unwrap();
        reload_providers(&mut self.dispatcher, &mut self.config).unwrap();
    }

    async fn chat(&self, prompt: &str) {
        let resp = reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", self.proxy))
            .json(&json!({
                "model": "qwen/cheap", "stream": false, "max_tokens": 32,
                "messages": [{"role": "user", "content": prompt}],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "primary must succeed");
        assert!(resp.text().await.unwrap().contains("primary"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routing_sidecar_wiring_defaults_off_and_reloads() {
    let upstream = fake_upstream().await;
    let (sc, sc_addr) = fake_sidecar(false).await;
    let dir = tempfile::tempdir().unwrap();
    let base = base(dir.path(), upstream);

    // 0) daemon の proxy 組み立て（build_llm_proxy_state）も既定では client を作らない。
    {
        let path = dir.path().join("daemon.toml");
        std::fs::write(&path, &base).unwrap();
        let mut config = Config::load(&path).unwrap();
        assert!(!config.model_routing.estimator.sidecar.enabled);
        let mut dispatcher = build_dispatcher(&config, Default::default()).unwrap();
        let registry: Arc<dyn task_core::model_router::context_registry::RoutingContextRegistry> =
            Arc::new(
                task_core::model_router::context_registry::InMemoryRoutingContextRegistry::new(),
            );
        let state = crate::daemon::services::build_llm_proxy_state(
            &mut config,
            &mut dispatcher,
            SharedRole::default(),
            registry,
        )
        .unwrap();
        assert!(state.is_some());
        let control = config
            .model_routing
            .estimator_sidecar_control
            .as_ref()
            .expect("wired");
        assert!(!control.active(), "default off must not build a client");
    }

    // 1) 既定（設定なし）: client 無し、要求しても sidecar への送信 0・記録 0。
    let mut h = Harness::start(dir.path(), base.clone()).await;
    assert!(!h.control.active());
    h.chat("hello").await;
    assert_eq!(sc.hits.load(Ordering::SeqCst), 0, "default must not send");
    assert!(h.sink.records().is_empty());

    // 2) reload で有効にすると（shadow_only・上限 2）評価が始まり、primary は変わらない。
    h.reload(format!("{base}{}", sidecar(sc_addr, 2, "", ANY)));
    assert!(h.control.active());
    h.chat("hello").await;
    let records = h.sink.wait_len(1).await;
    assert_eq!(sc.hits.load(Ordering::SeqCst), 1, "{records:?}");
    let r = &records[0];
    assert_eq!(r.kind, ShadowKind::Estimator);
    assert_eq!(r.status, ShadowStatus::Completed, "{r:?}");
    assert_eq!(r.policy_version, "estimator:route-test/1");

    // 3) 同じ設定の reload は作り直さない（数え済みの上限も保つ）。上限を 1 に締めると送らない。
    h.reload(format!("{base}{}", sidecar(sc_addr, 2, "", ANY)));
    assert!(h.control.active());
    h.reload(format!("{base}{}", sidecar(sc_addr, 1, "", ANY)));
    h.chat("hello").await;
    let records = h.sink.wait_len(2).await;
    assert_eq!(records[1].status, ShadowStatus::Dropped);
    assert_eq!(records[1].reason, Some(ShadowReason::CapExceeded));
    assert_eq!(
        sc.hits.load(Ordering::SeqCst),
        1,
        "tightened cap must not send"
    );
    assert_eq!(h.control.budget.used(), 1);

    // 4) reload で off へ戻すと client を外し、送信も記録も増えない。
    h.reload(base.clone());
    assert!(!h.control.active());
    h.chat("hello").await;
    assert_eq!(sc.hits.load(Ordering::SeqCst), 1);
    assert_eq!(h.sink.records().len(), 2);

    // 5) dispatcher の tick は sidecar を呼ばない（評価は proxy の要求の後だけ）。
    h.reload(format!("{base}{}", sidecar(sc_addr, 5, "", ANY)));
    assert!(h.control.active());
    tokio::task::block_in_place(|| h.dispatcher.tick()).unwrap();
    assert_eq!(
        sc.hits.load(Ordering::SeqCst),
        1,
        "tick must not call the sidecar"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routing_sidecar_privacy_and_dependencies_gate_prompt() {
    let upstream = fake_upstream().await;
    let (sc, sc_addr) = fake_sidecar(false).await;
    let dir = tempfile::tempdir().unwrap();
    let base = base(dir.path(), upstream);

    // 1) send_prompt = false（既定）: 評価はするが prompt は送らない。
    let mut h = Harness::start(
        dir.path(),
        format!("{base}{}", sidecar(sc_addr, 10, "", ANY)),
    )
    .await;
    h.chat("secret-prompt-text").await;
    h.sink.wait_len(1).await;
    {
        let bodies = sc.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].get("optional_prompt").is_none(), "{}", bodies[0]);
        assert!(!bodies[0].to_string().contains("secret-prompt-text"));
    }

    // 2) allowlist 外（lane が frontier だけ）: 入口で落ち、送信 0・記録 0。
    let frontier_only =
        "task_kinds = [\"*\"]\nroles = [\"*\"]\nlanes = [\"frontier\"]\nsources = [\"*\"]\n";
    h.reload(format!("{base}{}", sidecar(sc_addr, 10, "", frontier_only)));
    h.chat("secret-prompt-text").await;
    assert_eq!(
        sc.hits.load(Ordering::SeqCst),
        1,
        "outside allowlist must not send"
    );
    assert_eq!(h.sink.records().len(), 1);

    // 3) send_prompt = true でも prompt_allowlist 外の要求は送る前に止まる（privacy で dropped、送信 0）。
    let prompt_research_only = "send_prompt = true\n\
         [model_routing.estimator.sidecar.prompt_allowlist]\n\
         task_kinds = [\"research\"]\nroles = [\"*\"]\nlanes = [\"*\"]\nsources = [\"*\"]\n";
    h.reload(format!(
        "{base}{}",
        sidecar(sc_addr, 10, prompt_research_only, ANY)
    ));
    h.chat("secret-prompt-text").await;
    let records = h.sink.wait_len(2).await;
    assert_eq!(records[1].status, ShadowStatus::Dropped, "{:?}", records[1]);
    assert_eq!(records[1].reason, Some(ShadowReason::Privacy));
    assert_eq!(records[1].detail.as_deref(), Some("prompt_required"));
    assert_eq!(
        sc.hits.load(Ordering::SeqCst),
        1,
        "prompt gate must not send"
    );

    // 4) prompt_allowlist の対象なら prompt を渡す。
    let prompt_any =
        format!("send_prompt = true\n[model_routing.estimator.sidecar.prompt_allowlist]\n{ANY}");
    h.reload(format!("{base}{}", sidecar(sc_addr, 10, &prompt_any, ANY)));
    h.chat("allowed-prompt").await;
    let records = h.sink.wait_len(3).await;
    assert_eq!(
        records[2].status,
        ShadowStatus::Completed,
        "{:?}",
        records[2]
    );
    assert_eq!(
        sc.bodies.lock().unwrap()[1]["optional_prompt"],
        json!("allowed-prompt")
    );

    // 5) 宣言に無い外部依存を申告する sidecar の応答は評価不能（比較に使わない）。primary は成功のまま。
    let (net, net_addr) = fake_sidecar(true).await;
    h.reload(format!("{base}{}", sidecar(net_addr, 10, "", ANY)));
    h.chat("hello").await;
    let records = h.sink.wait_len(4).await;
    assert_eq!(net.hits.load(Ordering::SeqCst), 1);
    assert_eq!(records[3].status, ShadowStatus::Failed, "{:?}", records[3]);
    assert_eq!(records[3].detail.as_deref(), Some("dependency_mismatch"));
    assert!(records[3].candidate_model.is_none());
}
