//! estimator sidecar を shadow 評価へ繋ぐ end-to-end 試験（ADR 2026-10-04 §7.3・§10 Phase 5）。
//!
//! 一時 DB の上に、in-process の proxy（127.0.0.1:0）・偽の上流 2 本・偽 sidecar を立て、
//! `RoutingDecided` を持つ run ごとに chat を投げる。estimator shadow の記録を DB の
//! `routing_shadow_recorded` event として追記し、`task_ops::routing_replay` の export → evaluate
//! （`celerisctl routing export` / `evaluate --policy estimator` と同じ関数）で比較表を確かめる。
//!
//! RoutingDecided は dispatch 側の decision id で run の前に残し、pin は正直な descriptor
//! （`route-test` / `1`）で行う（回避策なし）。proxy の decision（`pdec_…`）と dispatch の decision の
//! 対応は proxy が記録する `routing_request_decided` の `parent_decision_id` から export が引く。
//!
//! daemon 内部の配線（`EstimatorSidecarControl`・`shadow_settings`・`SidecarDailyBudget`・
//! `task_shadow_sink` / `append_shadow_event`・`TaskProxyEventSink`）は `pub(crate)` で外から呼べないため、この file では
//! 同じ手順を鏡写しにしている。daemon 内部の試験は `crates/celeris/src/daemon/routing_sidecar_tests.rs`。
//! 待ちは記録の到着（Notify）で行い、60 秒の保険の timeout だけを置く（固定 sleep は使わない）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use celeris::Config;
use llm_proxy::estimator_shadow::{EstimatorShadow, EstimatorShadowConfig, EstimatorShadowSlot};
use llm_proxy::estimator_sidecar::{SidecarClientConfig, SidecarEstimatorClient, TokioClock};
use llm_proxy::legacy_catalog::{LegacyCatalog, normalize_legacy_config};
use llm_proxy::reservation::FixedClock;
use llm_proxy::routing_context::{ProxyEventSink, ProxyRoutingEvent};
use llm_proxy::shadow::{ShadowBudget, ShadowEvent, ShadowSink};
use serde_json::{Value, json};
use task_core::model_policy::RoutingRecord;
use task_core::model_router::context::RoutingContext;
use task_core::model_router::context_registry::{
    InMemoryRoutingContextRegistry, RoutingContextRegistry,
};
use task_core::model_router::estimator::sidecar::{EstimatorDependencies, EstimatorDescriptor};
use task_core::model_router::policy::RoutingMode;
use task_core::model_router::shadow::{
    ShadowDailyCaps, ShadowKind, ShadowPolicy, ShadowReason, ShadowReservation,
    ShadowReservationRequest, ShadowSettlement, ShadowStatus,
};
use task_core::model_router::trace::{CandidateTrace, RoutingTraceV1};
use task_core::model_routing::LaneResolution;
use task_core::{Event, LaneCeiling, SqliteStore, Task, TaskId, TaskStore, Tier};
use task_ops::add::{NewTaskSpec, create_task};
use task_ops::routing_replay::{EstimatorComparisonInputV1, ExportOptions};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

/// ra の model（legacy 正規化が付ける id）と、rb の deployment を上書きして付ける別 model。
const MODEL_A: &str = "legacy:qwen:qwen3.8-27b";
const MODEL_B: &str = "model-b";
const DAILY_CAP: u64 = 3;
const NOMINAL_CALL_USD: f64 = 0.000_001;
const ANY: &str = "task_kinds = [\"*\"]\nroles = [\"*\"]\nlanes = [\"*\"]\nsources = [\"*\"]\n";

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

/// 偽の openai-compatible 上流。`wire` という 1 モデルを持ち、chat には `content` を返す。
async fn fake_upstream(wire: &'static str, content: &'static str) -> SocketAddr {
    fake_http(Arc::new(move |head: &str, _: &str| {
        if head.contains("/models") {
            json!({"object": "list", "data": [{"id": wire, "object": "model"}]}).to_string()
        } else {
            json!({
                "id": "1", "object": "chat.completion", "model": wire,
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
            .to_string()
        }
    }))
    .await
}

/// 次の呼び出しで sidecar が何をするか。
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// `MODEL_B` を最高にする（heuristic の primary は `MODEL_A` なので食い違う）。
    PreferSecond,
    /// `MODEL_A` を最高にする（heuristic と一致）。
    PreferFirst,
    /// `request_id` が一致しない不正な応答を返す（評価不能）。
    WrongRequestId,
}

struct Sidecar {
    hits: AtomicU32,
    mode: Mutex<Mode>,
    candidates_seen: Mutex<Vec<String>>,
}

async fn fake_sidecar() -> (Arc<Sidecar>, SocketAddr) {
    let sidecar = Arc::new(Sidecar {
        hits: AtomicU32::new(0),
        mode: Mutex::new(Mode::PreferSecond),
        candidates_seen: Mutex::new(Vec::new()),
    });
    let s = Arc::clone(&sidecar);
    let addr = fake_http(Arc::new(move |_: &str, body: &str| {
        s.hits.fetch_add(1, Ordering::SeqCst);
        let body: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let mode = *s.mode.lock().unwrap();
        let candidates = body["candidates"].as_array().cloned().unwrap_or_default();
        *s.candidates_seen.lock().unwrap() = candidates
            .iter()
            .filter_map(|c| c["model_profile_id"].as_str().map(str::to_owned))
            .collect();
        let best = match mode {
            Mode::PreferFirst => MODEL_A,
            _ => MODEL_B,
        };
        let estimates: Vec<Value> = candidates
            .iter()
            .map(|c| {
                json!({"model_profile_id": c["model_profile_id"],
                       "index": if c["model_profile_id"] == best { 0.9 } else { 0.2 }, "confidence": 0.8,
                       "reasons": ["classifier_score"]})
            })
            .collect();
        let request_id = if mode == Mode::WrongRequestId {
            json!("not-the-request")
        } else {
            body["request_id"].clone()
        };
        json!({
            "request_id": request_id, "estimator_id": "route-test", "version": "1",
            "estimates": estimates,
            "dependencies": {"needs_network": false, "external_embeddings": false},
        })
        .to_string()
    }))
    .await;
    (sidecar, addr)
}

/// `SidecarDailyBudget` の鏡写し: 予約した時点で 1 回と数え、UTC 日が変わるまで戻さない。
struct CountingBudget {
    caps: ShadowDailyCaps,
    used: Mutex<u64>,
}

impl ShadowBudget for CountingBudget {
    fn reserve(
        &self,
        request: &ShadowReservationRequest,
        now: OffsetDateTime,
    ) -> ShadowReservation {
        if request
            .worst_effective_usd
            .is_none_or(|usd| !usd.is_finite() || usd < 0.0)
        {
            return ShadowReservation::Denied(ShadowReason::UnknownCost);
        }
        let mut used = self.used.lock().unwrap();
        if used.saturating_add(1) > self.caps.max_requests {
            return ShadowReservation::Denied(ShadowReason::CapExceeded);
        }
        *used += 1;
        let day = now.to_offset(time::UtcOffset::UTC).date().to_string();
        ShadowReservation::Reserved {
            reservation_id: format!("sidecar-{day}-{used}"),
            day,
        }
    }

    fn settle(&self, _reservation_id: &str, _settlement: ShadowSettlement) {}
}

/// `task_shadow_sink` / `append_shadow_event` の鏡写し。追記を試みた件数を数えて Notify で知らせる。
struct DbSink {
    store: Arc<SqliteStore>,
    attempts: Mutex<usize>,
    appended: Mutex<usize>,
    changed: Notify,
}

impl DbSink {
    fn append(&self, event: ShadowEvent) -> Result<(), String> {
        let task = event.task_id.ok_or("shadow record has no task")?;
        let task_id: TaskId = task.parse().map_err(|e| format!("invalid task id: {e}"))?;
        let run_id = event.record.run_id.as_deref().ok_or("no run id")?;
        if !self
            .store
            .run_index_get(run_id)
            .map_err(|e| e.to_string())?
            .is_some_and(|r| r.task_id == task)
        {
            return Err("run does not belong to task".into());
        }
        event.record.validate().map_err(|e| e.to_string())?;
        self.store
            .append_event(
                task_id,
                &Event::RoutingShadowRecorded {
                    record: Box::new(event.record),
                },
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn wait_appended(&self, n: usize) {
        let wait = async {
            loop {
                let changed = self.changed.notified();
                if *self.appended.lock().unwrap() >= n {
                    return;
                }
                changed.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(60), wait)
            .await
            .expect("estimator shadow event did not arrive");
    }
}

impl ShadowSink for DbSink {
    fn record(&self, event: ShadowEvent) {
        let result = self.append(event);
        *self.attempts.lock().unwrap() += 1;
        if result.is_ok() {
            *self.appended.lock().unwrap() += 1;
        }
        self.changed.notify_waiters();
    }
}

/// `TaskProxyEventSink` の鏡写し: run 側 decision を DB の `RoutingDecided` から引き、要求ごとの
/// `RoutingRequestDecided`（proxy decision → run 側 decision の対応）を追記する。
struct RequestSink {
    store: Arc<SqliteStore>,
    appended: Mutex<usize>,
    changed: Notify,
}

impl RequestSink {
    async fn wait_appended(&self, n: usize) {
        let wait = async {
            loop {
                let changed = self.changed.notified();
                if *self.appended.lock().unwrap() >= n {
                    return;
                }
                changed.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(60), wait)
            .await
            .expect("proxy request event did not arrive");
    }
}

impl ProxyEventSink for RequestSink {
    fn parent_decision(&self, run_id: &str) -> Option<String> {
        let run = self.store.run_index_get(run_id).ok()??;
        let task_id: TaskId = run.task_id.parse().ok()?;
        self.store
            .events_for(task_id)
            .ok()?
            .into_iter()
            .find_map(|(_, event)| match event {
                Event::RoutingDecided { run_id: id, record } if id == run_id => {
                    record.optimizer.map(|trace| trace.decision_id)
                }
                _ => None,
            })
    }

    fn record(&self, event: ProxyRoutingEvent) {
        let task_id: TaskId = event.task_id.parse().unwrap();
        self.store
            .append_event(
                task_id,
                &Event::RoutingRequestDecided {
                    record: Box::new(event.request),
                },
            )
            .unwrap();
        *self.appended.lock().unwrap() += 1;
        self.changed.notify_waiters();
    }
}

fn config_text(dir: &std::path::Path, ups: [SocketAddr; 2], sidecar: SocketAddr) -> String {
    format!(
        "db = {:?}\nworkspace_root = {:?}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n\
         [llm_proxy]\nenabled = true\nlisten = \"127.0.0.1:0\"\n\
         [[llm_proxy.sources.openai_compatible]]\nid = \"ra\"\nbase_url = \"http://{}/v1\"\n\
         [[llm_proxy.sources.openai_compatible]]\nid = \"rb\"\nbase_url = \"http://{}/v1\"\n\
         [model_routing.estimator.sidecar]\nenabled = true\nshadow_only = true\n\
         endpoint = \"http://{sidecar}/\"\nestimator_id = \"route-test\"\nestimator_version = \"1\"\n\
         timeout_ms = 30000\ndaily_max_requests = {DAILY_CAP}\n\
         [model_routing.estimator.sidecar.allowlist]\n{ANY}",
        dir.join("db.sqlite3"),
        dir.join("ws"),
        ups[0],
        ups[1],
    )
}

/// `shadow_settings` の鏡写し（検証済み設定 → estimator shadow と client の設定）。
fn shadow_settings(config: &Config) -> (EstimatorShadowConfig, SidecarClientConfig) {
    let entry = &config.model_routing.estimator.sidecar;
    assert!(entry.enabled && entry.shadow_only);
    let endpoint = reqwest::Url::parse(entry.endpoint.as_deref().unwrap()).unwrap();
    let max_requests = entry.daily_max_requests.unwrap();
    let max_inflight = entry.max_inflight.max(1);
    let descriptor = EstimatorDescriptor {
        estimator_id: entry.estimator_id.clone().unwrap(),
        version: entry.estimator_version.clone().unwrap(),
        protocol_version: entry.protocol_version,
        needs_prompt: entry.send_prompt,
        dependencies: EstimatorDependencies {
            needs_network: false,
            external_embeddings: false,
        },
    };
    let mut client = SidecarClientConfig::new(endpoint, descriptor);
    client.timeout = Duration::from_millis(entry.timeout_ms);
    client.max_inflight = usize::try_from(max_inflight).unwrap_or(usize::MAX);
    client.max_payload_bytes = entry.max_payload_bytes;
    client.send_prompt = entry.send_prompt;
    client.allow_external_dependencies = false;
    let policy = ShadowPolicy {
        execute: true,
        allowlist: entry.allowlist.clone(),
        sample_rate: 1.0,
        daily_max_requests: Some(max_requests),
        daily_max_tokens: Some(max_requests),
        daily_max_effective_usd: Some(max_requests as f64 * NOMINAL_CALL_USD),
        max_concurrency: Some(max_inflight),
        max_queue_depth: Some(max_inflight),
        timeout_ms: Some(entry.timeout_ms),
    };
    let shadow = EstimatorShadowConfig {
        shadow_only: true,
        policy,
        worst_call_effective_usd: Some(NOMINAL_CALL_USD),
        worst_call_tokens: 1,
        resource_group: None,
        prompt_allowlist: Default::default(),
    };
    (shadow, client)
}

/// proxy の catalog。daemon の `proxy_catalog` と同じく legacy 設定から導くが、legacy 正規化は
/// relay を全て同じ model にするため、rb だけ別 model（`MODEL_B`）に付け替えて比較の余地を作る
/// （config の検証は legacy 同一性の上書きを拒むので、設定ではなく catalog を直接組む）。
fn two_model_catalog(config: &Config) -> LegacyCatalog {
    let mut catalog = normalize_legacy_config(&config.llm_proxy);
    let mut model_b = catalog
        .models
        .iter()
        .find(|m| m.id == MODEL_A)
        .expect("legacy qwen model")
        .clone();
    model_b.id = MODEL_B.into();
    catalog.models.push(model_b);
    for model in &mut catalog.models {
        // context 上限が未知だと kernel が候補を除外するので宣言する。
        model.context_limits.input = Some(100_000);
        model.context_limits.output = Some(8_192);
        model.context_limits.total = Some(108_192);
    }
    for d in &mut catalog.deployments {
        if d.source_ref == "openai-compatible:rb" {
            d.model_profile_id = MODEL_B.into();
        }
    }
    catalog
}

fn new_task(store: &SqliteStore) -> Task {
    let spec: NewTaskSpec = serde_json::from_value(json!({
        "title": "t", "objective": "o",
        "acceptance": [{"type": "command", "cmd": "true"}]
    }))
    .unwrap();
    create_task(store, spec, OffsetDateTime::now_utc()).unwrap()
}

/// heuristic の決定: 1 番目の候補（`primary_model`）を高 score にして primary にする。
fn decided(task: &Task, run: &str, decision_id: &str, models: &[String]) -> Event {
    let decision = task_core::decide_for_task(task, &LaneCeiling::default()).unwrap();
    let candidates = models
        .iter()
        .enumerate()
        .map(|(i, m)| CandidateTrace {
            model_profile_id: m.clone(),
            deployment_id: format!("legacy:{}:Cheap", if i == 0 { "ra" } else { "rb" }),
            score: Some(if i == 0 { 0.9 } else { 0.3 }),
            cash_usd: Some(0.1),
            effective_usd: Some(0.1),
            ..CandidateTrace::default()
        })
        .collect();
    Event::RoutingDecided {
        run_id: run.into(),
        record: Box::new(RoutingRecord {
            org_node: None,
            harness: None,
            decision,
            resolution: LaneResolution {
                model_id: models[0].clone(),
                ..LaneResolution::default()
            },
            quota_reason: None,
            work_unit_id: None,
            escalation: None,
            optimizer: Some(RoutingTraceV1 {
                decision_id: decision_id.to_string(),
                parent_decision_id: None,
                task_id: Some(task.id.to_string()),
                work_unit_id: None,
                run_id: Some(run.into()),
                request_id: None,
                stage: "dispatch".into(),
                mode: RoutingMode::Shadow,
                policy_version: "p1".into(),
                catalog_version: "c1".into(),
                feature_version: "1".into(),
                estimator_version: "e1".into(),
                snapshot_id: "s1".into(),
                observed_at: None,
                requested_lane: Tier::Cheap,
                selected_lane: Some(Tier::Cheap),
                candidates,
                selected: Some("ra".into()),
                fallback_order: vec![],
                reasons: vec![],
                source_id: Some("ra".into()),
                model: Some(models[0].clone()),
                account_id: None,
            }),
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routing_estimator_sidecar_plugs_into_shadow_eval() {
    let up_a = fake_upstream("wire-a", "primary-a").await;
    let up_b = fake_upstream("wire-b", "primary-b").await;
    let (sidecar, sidecar_addr) = fake_sidecar().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config_text(dir.path(), [up_a, up_b], sidecar_addr)).unwrap();
    let config = Config::load(&path).unwrap();
    let db_path = dir.path().join("db.sqlite3");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());

    // daemon と同じ組み立て: sink → budget → client → EstimatorShadow → slot → proxy。
    let sink = Arc::new(DbSink {
        store: Arc::clone(&store),
        attempts: Mutex::new(0),
        appended: Mutex::new(0),
        changed: Notify::new(),
    });
    let (shadow_config, client_config) = shadow_settings(&config);
    let caps = shadow_config.policy.daily_caps().expect("daily caps");
    let budget = Arc::new(CountingBudget {
        caps,
        used: Mutex::new(0),
    });
    let catalog = Arc::new(two_model_catalog(&config));
    let client = SidecarEstimatorClient::new(client_config, Arc::new(TokioClock)).unwrap();
    let clock = Arc::new(FixedClock(
        OffsetDateTime::from_unix_timestamp(1_791_201_600).unwrap(),
    ));
    let estimator = EstimatorShadow::new(
        shadow_config,
        Arc::new(client),
        catalog,
        "instance-test",
        Arc::clone(&budget) as Arc<dyn ShadowBudget>,
        Arc::clone(&sink) as Arc<dyn ShadowSink>,
        clock,
    )
    .unwrap();
    let slot = Arc::new(EstimatorShadowSlot::default());
    slot.set(Some(estimator));
    let registry = Arc::new(InMemoryRoutingContextRegistry::new());
    let requests = Arc::new(RequestSink {
        store: Arc::clone(&store),
        appended: Mutex::new(0),
        changed: Notify::new(),
    });
    let state = llm_proxy::ProxyState::new(
        config.llm_proxy.clone(),
        reqwest::Client::new(),
        None,
        None,
        None,
        task_core::SharedRole::default(),
        None,
        Duration::from_secs(5),
    )
    .with_estimator_shadow_slot(slot)
    .with_routing_context(
        Some(Arc::clone(&registry) as Arc<dyn RoutingContextRegistry>),
        Some(Arc::clone(&requests) as Arc<dyn ProxyEventSink>),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    tokio::spawn(llm_proxy::serve(
        listener,
        state,
        std::future::pending::<()>(),
    ));

    // 4 run: 食い違い完了 / 不正応答で failed / heuristic と一致で完了 / 日次上限（3）超過で dropped。
    let scenario = [
        Mode::PreferSecond,
        Mode::WrongRequestId,
        Mode::PreferFirst,
        Mode::PreferFirst,
    ];
    let models: Vec<String> = vec![MODEL_A.into(), MODEL_B.into()];
    let mut runs: Vec<(Task, String)> = Vec::new();
    for (i, mode) in scenario.iter().enumerate() {
        let task = new_task(&store);
        let run_id = format!("run-{i}");
        store
            .run_index_start(task_core::RunRow {
                run_id: run_id.clone(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Worker,
                seq: 1,
                status: task_core::RunIndexStatus::Running,
                adapter: None,
                model: None,
                account: None,
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: "2026-10-05T00:00:00Z".into(),
                finished_at: None,
            })
            .unwrap();
        // dispatch の決定は run の前に、dispatch 側の decision id で残す（daemon と同じ順）。
        store
            .append_event(
                task.id,
                &decided(&task, &run_id, &format!("dec-{run_id}"), &models),
            )
            .unwrap();
        *sidecar.mode.lock().unwrap() = *mode;
        let reference = registry.register(
            &run_id,
            RoutingContext {
                version: "1".into(),
                origin: "task".into(),
                task_id: Some(task.id.to_string()),
                run_id: Some(run_id.clone()),
                role: Some("worker".into()),
                task_kind: Some("code".into()),
                ..RoutingContext::default()
            },
            Duration::from_secs(600),
            Instant::now(),
        );
        let resp = reqwest::Client::new()
            .post(format!("http://{proxy}/v1/chat/completions"))
            .header("x-celeris-routing-context", reference)
            .json(&json!({
                "model": "qwen/cheap", "stream": false, "max_tokens": 32,
                "messages": [{"role": "user", "content": "hello"}],
            }))
            .send()
            .await
            .unwrap();
        // primary の判断は変わらない: 設定順で先頭の ra の応答が返る。
        assert_eq!(resp.status(), 200);
        assert!(resp.text().await.unwrap().contains("primary-a"));
        sink.wait_appended(i + 1).await;
        if i == 0 {
            let mut seen = sidecar.candidates_seen.lock().unwrap().clone();
            seen.sort();
            let mut want = models.clone();
            want.sort();
            assert_eq!(seen, want, "the sidecar sees both relay candidates");
        }
        requests.wait_appended(i + 1).await;
        // estimator の記録は proxy 側の decision id（`pdec_…`）を持ち、proxy の要求記録が
        // それを dispatch の decision id に結ぶ。
        let events = store.events_for(task.id).unwrap();
        let pdec = events
            .iter()
            .find_map(|(_, e)| match e {
                Event::RoutingShadowRecorded { record } => Some(record.primary_decision_id.clone()),
                _ => None,
            })
            .expect("shadow event for the run");
        assert!(pdec.starts_with("pdec_"), "{pdec}");
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, Event::RoutingRequestDecided { record }
                if record.decision_id == pdec
                    && record.parent_decision_id.as_deref() == Some(&*format!("dec-{run_id}")))),
            "the proxy request maps {pdec} to the dispatch decision"
        );
        runs.push((task, run_id));
    }
    assert_eq!(*sink.attempts.lock().unwrap(), 4);

    // sidecar への送信は日次上限以内（4 回目は送らない）。
    assert_eq!(sidecar.hits.load(Ordering::SeqCst) as u64, DAILY_CAP);
    assert_eq!(*budget.used.lock().unwrap(), DAILY_CAP);

    // DB の shadow event。
    let mut shadows = Vec::new();
    for (task, _) in &runs {
        for (_, event) in store.events_for(task.id).unwrap() {
            if let Event::RoutingShadowRecorded { record } = event {
                shadows.push(*record);
            }
        }
    }
    assert_eq!(shadows.len(), 4, "{shadows:?}");
    assert!(shadows.iter().all(|r| r.kind == ShadowKind::Estimator));
    assert!(
        shadows
            .iter()
            .all(|r| r.policy_version == "estimator:route-test/1")
    );
    let by_run = |i: usize| {
        shadows
            .iter()
            .find(|r| r.run_id.as_deref() == Some(&format!("run-{i}")))
            .unwrap_or_else(|| panic!("no shadow for run-{i}"))
    };
    assert_eq!(by_run(0).status, ShadowStatus::Completed);
    assert_eq!(
        by_run(0).candidate_source.as_deref(),
        Some("openai-compatible:rb")
    );
    assert_eq!(by_run(1).status, ShadowStatus::Failed);
    assert!(by_run(1).reason.is_some(), "{:?}", by_run(1));
    assert_eq!(by_run(2).status, ShadowStatus::Completed);
    assert_eq!(
        by_run(2).candidate_source.as_deref(),
        Some("openai-compatible:ra")
    );
    assert_eq!(by_run(3).status, ShadowStatus::Dropped);
    assert_eq!(by_run(3).reason, Some(ShadowReason::CapExceeded));

    // export → evaluate（celerisctl routing export / evaluate --policy estimator と同じ関数）。
    let dataset = task_ops::routing_replay::export(
        &db_path,
        &ExportOptions {
            policy_hash: "policy-hash".into(),
            catalog_hash: "catalog-hash".into(),
            estimator_hash: "estimator-hash".into(),
            from_utc: None,
            until_utc: None,
            seed: 7,
        },
    )
    .unwrap();
    assert_eq!(dataset.rows.len(), 4);
    for row in &dataset.rows {
        // shadow は dispatch の decision の行に付く。
        assert_eq!(row.decision_id, format!("dec-{}", row.run_id));
        // primary の判断は dataset でも RoutingDecided のまま（1 番目の候補）。
        assert_eq!(row.primary_model.as_deref(), Some(&*models[0]));
        assert_eq!(row.shadows.len(), 1);
    }
    // file 境界（dataset.jsonl）を往復しても同じ。
    let jsonl = dataset.jsonl().unwrap();
    assert_eq!(jsonl.lines().count(), 4);
    let descriptor = EstimatorDescriptor {
        estimator_id: "route-test".into(),
        version: "1".into(),
        protocol_version: 1,
        needs_prompt: false,
        dependencies: EstimatorDependencies {
            needs_network: false,
            external_embeddings: false,
        },
    };
    let evaluate = |descriptor: EstimatorDescriptor| {
        task_ops::routing_replay::evaluate_with_estimator(
            &dataset,
            None,
            None,
            Some(&EstimatorComparisonInputV1 {
                descriptor,
                pair: None,
            }),
        )
        .unwrap()
    };
    // 正直な pin（daemon が記録する `estimator:route-test/1` と同じ id・version）で一致する。
    let report = evaluate(descriptor);
    let cmp = report
        .estimator_comparison
        .as_ref()
        .expect("estimator comparison");
    eprintln!("{cmp:#?}");
    // 硬い制約違反は 0（estimator は primary を変えない）。空の policies で空振りしない。
    assert!(
        report.policies.contains_key("legacy"),
        "{:?}",
        report.policies.keys()
    );
    assert!(
        report
            .policies
            .values()
            .all(|p| p.constraint_violations == 0)
    );
    assert_eq!(cmp.target_decisions, 4);
    assert_eq!(cmp.completed, 2);
    assert_eq!(cmp.failed, 1);
    assert_eq!(cmp.dropped, 1);
    assert_eq!(cmp.evaluated, 2);
    assert!(
        cmp.coverage > 0.0 && cmp.coverage <= 1.0,
        "{}",
        cmp.coverage
    );
    assert_eq!(cmp.observed_estimator_versions, ["estimator:route-test/1"]);
    assert_eq!(
        cmp.incomparable_reasons.get("estimator_version_mismatch"),
        None
    );
    assert_eq!(
        cmp.incomparable_reasons.get("estimator_model_unknown"),
        None
    );
    // 記録の model は profile id で残るので、dataset の候補・primary と同じ名前空間で比べられる。
    assert_eq!(by_run(0).candidate_model.as_deref(), Some(MODEL_B));
    assert_eq!(by_run(2).candidate_model.as_deref(), Some(MODEL_A));
    // run 0 は heuristic（score の高い MODEL_A）と食い違い、run 2 は一致して primary とも同じ。
    assert_eq!(cmp.differs_from_heuristic, 1);
    assert_eq!(cmp.same_as_heuristic, 1);
    assert_eq!(cmp.same_as_primary, 1);
    assert_eq!(cmp.heuristic_unavailable, 0);
    assert_eq!(
        cmp.incomparable_reasons
            .get("unselected_model_outcome_unknown"),
        Some(&1)
    );
    assert_eq!(cmp.incomparable_reasons.get("upstream_error"), Some(&1));
    assert_eq!(cmp.incomparable_reasons.get("cap_exceeded"), Some(&1));
    assert!(!cmp.unknown_reason.is_empty());
    assert!(!cmp.incomparable_reasons.is_empty(), "{cmp:?}");
}
