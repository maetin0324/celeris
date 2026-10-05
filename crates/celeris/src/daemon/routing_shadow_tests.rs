//! daemon の shadow 配線の試験（ADR 2026-10-04 §7.1・§10 Phase 4）。一時 DB と 127.0.0.1 の偽上流だけを
//! 使う。時刻は固定時計（同じ UTC 日）、実行の完了は queue が空になる出来事を待つ（長い保険つき）。

use std::sync::atomic::AtomicU32;

use llm_proxy::config::OpenAiCompatibleConfig;
use llm_proxy::openai::{ChatCompletionRequest, ChatMessage, MessageContent};
use llm_proxy::reservation::FixedClock;
use llm_proxy::shadow::{RelayShadowExecutor, ShadowJob, SubmitOutcome};
use task_core::model_router::shadow::{ShadowReason, ShadowTarget};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::Config;
use crate::daemon::admin::reload_providers;
use crate::daemon::bootstrap::build_dispatcher;

const BODY: &str = r#"{"id":"s","object":"chat.completion","model":"qwen-b","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}"#;

/// 偽の openai-compatible 上流（HTTP/1.1 を 1 要求ごとに閉じる）。chat の要求数を数える。
async fn fake_upstream() -> (String, Arc<AtomicU32>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf);
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
                            break;
                        }
                    }
                }
                if String::from_utf8_lossy(&buf).contains("/chat/completions") {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{BODY}",
                    BODY.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1"), hits)
}

fn base(dir: &std::path::Path) -> String {
    format!(
        "db = {:?}\nworkspace_root = {:?}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        dir.join("db.sqlite3"),
        dir.join("ws"),
    )
}

/// mode = shadow、execute = true、日次 request 上限 `max_requests`（他の上限は十分大きい）。
fn capped(max_requests: u64) -> String {
    format!(
        "[model_routing]\nmode = \"shadow\"\n[model_routing.shadow]\nexecute = true\nsample_rate = 1.0\n\
         daily_max_requests = {max_requests}\ndaily_max_tokens = 100000\ndaily_max_effective_usd = 10.0\n\
         max_concurrency = 1\nmax_queue_depth = 4\ntimeout_ms = 30000\n\
         [model_routing.shadow.allowlist]\ntask_kinds = [\"*\"]\nroles = [\"*\"]\nlanes = [\"*\"]\nsources = [\"*\"]\n"
    )
}

fn deps(config: &Config, dispatcher: &Dispatcher, base_url: &str, owner: &str) -> ShadowDeps {
    ShadowDeps {
        db_path: config.db.path.clone(),
        busy_timeout: config.db.busy_timeout(),
        task_store: dispatcher.store(),
        executor: Arc::new(RelayShadowExecutor::new(
            reqwest::Client::new(),
            vec![OpenAiCompatibleConfig {
                id: "qwen".into(),
                base_url: base_url.into(),
                api_key: None,
                enabled: true,
            }],
        )),
        clock: Arc::new(FixedClock(
            time::OffsetDateTime::from_unix_timestamp(1_791_201_600).unwrap(),
        )),
        owner: owner.into(),
        cost: Some(Arc::new(|_: &str, _: &str, _: u64| Some(0.01))),
    }
}

fn job(id: &str) -> ShadowJob {
    ShadowJob {
        shadow_id: id.into(),
        primary_decision_id: format!("pdec_{id}"),
        task_id: None,
        run_id: None,
        request_id: Some(format!("req_{id}")),
        target: ShadowTarget {
            task_kind: "coding".into(),
            role: "coder".into(),
            lane: "cheap".into(),
            source: "openai-compatible:qwen".into(),
        },
        candidate_model: "qwen-b".into(),
        primary_source: "claude-oauth".into(),
        primary_resource_group: None,
        candidate_resource_group: None,
        request: ChatCompletionRequest {
            model: "celeris/cheap".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some(MessageContent::Text("hi".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: Some(16),
            stop: None,
            stream: false,
            tools: None,
            tool_choice: None,
        },
        worst_tokens: 32,
        worst_effective_usd: Some(0.01),
    }
}

/// queue が空（待ち 0・実行中 0）になるまで待つ。出来事待ちで、保険は 60 秒。
async fn drained(queue: &Arc<ShadowQueue>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while queue.depth() != (0, 0) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "shadow queue did not drain"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn reload_with(path: &std::path::Path, text: String, d: &mut Dispatcher, config: &mut Config) {
    std::fs::write(path, text).unwrap();
    reload_providers(d, config).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routing_shadow_wiring_defaults_off_and_reloads() {
    let (url, hits) = fake_upstream().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let base = base(dir.path());

    // 1) 既定（設定なし）: proxy に shadow を差し込まず、queue・予約 store を作らない。
    std::fs::write(&path, &base).unwrap();
    let mut config = Config::load(&path).unwrap();
    let mut dispatcher = build_dispatcher(&config, Default::default()).unwrap();
    assert!(!dispatcher.routing_shadow_policy().execute);
    let dispatcher_deps = deps(&config, &dispatcher, &url, "instance-off");
    let wiring = install_proxy_shadow(&mut dispatcher, dispatcher_deps).unwrap();
    assert!(wiring.shadow.is_none());
    let off = Arc::clone(&wiring.control);
    assert!(!off.installed() && off.queue().is_none() && !off.decision_on());
    // reload で有効にしても dispatcher は新しい policy を持ち、proxy は再起動まで送らない。
    reload_with(
        &path,
        format!("{base}{}", capped(2)),
        &mut dispatcher,
        &mut config,
    );
    assert!(dispatcher.routing_shadow_policy().execute);
    assert_eq!(
        dispatcher.dispatch_routing().mode,
        task_core::model_router::policy::RoutingMode::Shadow
    );
    assert!(off.decision_on() && off.queue().is_none());
    assert_eq!(hits.load(Ordering::SeqCst), 0, "default must not send");

    // 2) instance A: execute = true（上限 2）で起動すると queue ができ、共有 DB に予約して送る。
    let path_a = dir.path().join("a.toml");
    std::fs::write(&path_a, format!("{base}{}", capped(2))).unwrap();
    let mut config_a = Config::load(&path_a).unwrap();
    let mut dispatcher_a = build_dispatcher(&config_a, Default::default()).unwrap();
    let dispatcher_a_deps = deps(&config_a, &dispatcher_a, &url, "instance-a");
    let wiring_a = install_proxy_shadow(&mut dispatcher_a, dispatcher_a_deps).unwrap();
    let shadow_a = wiring_a.shadow.expect("execute policy installs shadow");
    assert!(shadow_a.decision.is_some());
    let queue_a = Arc::clone(shadow_a.execution.as_ref().expect("queue"));
    assert_eq!(queue_a.submit(job("a1")), SubmitOutcome::Started);
    drained(&queue_a).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // reload で off（mode legacy・execute なし）: 同じ queue が新しい policy を見て受け付けない。
    reload_with(&path_a, base.clone(), &mut dispatcher_a, &mut config_a);
    assert!(!wiring_a.control.decision_on());
    assert!(!queue_a.policy().execute);
    assert_eq!(
        queue_a.submit(job("a2")),
        SubmitOutcome::NotAdmitted(ShadowReason::Off)
    );
    // reload で上限 1 に締める: 上限・予約先が丸ごと差し替わり、今日の 1 件（DB の予約）で満杯。
    reload_with(
        &path_a,
        format!("{base}{}", capped(1)),
        &mut dispatcher_a,
        &mut config_a,
    );
    assert_eq!(queue_a.policy().daily_max_requests, Some(1));
    assert!(wiring_a.control.decision_on());
    queue_a.submit(job("a3"));
    drained(&queue_a).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1, "cap 1 already used today");

    // 3) handoff: 同じ DB を開いた新 instance B（上限 2）は A の予約を読み継ぎ、合計 2 件で止まる。
    let path_b = dir.path().join("b.toml");
    std::fs::write(&path_b, format!("{base}{}", capped(2))).unwrap();
    let config_b = Config::load(&path_b).unwrap();
    let mut dispatcher_b = build_dispatcher(&config_b, Default::default()).unwrap();
    let dispatcher_b_deps = deps(&config_b, &dispatcher_b, &url, "instance-b");
    let wiring_b = install_proxy_shadow(&mut dispatcher_b, dispatcher_b_deps).unwrap();
    let queue_b = Arc::clone(wiring_b.control.queue().expect("queue"));
    for id in ["b1", "b2"] {
        queue_b.submit(job(id));
        drained(&queue_b).await;
    }
    assert_eq!(hits.load(Ordering::SeqCst), 2, "daily cap is shared via DB");
}
