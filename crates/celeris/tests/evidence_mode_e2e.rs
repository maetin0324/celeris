//! evidence_mode（none / pinned / latest）の通し試験: 通常の `POST /tasks`（HTTP）で起票し、子・孫・retry へ
//! override が引き継がれ、実 dispatcher の route 判断が途中で最新根拠表を差し替えても `none` では変わらず
//! 根拠表 lookup が 0 回であること、同条件の pinned / latest との差が判断記録（`routing_decided` event）と
//! DTO（GET /tasks/{id}）で判別できること、矛盾指定が起票時に拒否されることを確かめる。
//! temp DB・loopback・fake harness のみ（実 LLM・本番 DB なし）。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use celeris::Config;
use serde_json::{Value, json};
use task_core::route_select::{RoutingStage, RoutingStageChange};
use task_core::routing_evidence::{EvidenceKey, EvidenceRow};
use task_core::{DaemonMode, InstanceRole, SharedRole, SqliteStore, TaskId, TaskStore};
use task_dispatch::Dispatcher;

const TOKEN: &str = "evidence-mode-token";
const GENRE: &str = "light";

struct World {
    _dir: tempfile::TempDir,
    config: Config,
    sqlite: Arc<SqliteStore>,
    base: String,
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
}

impl World {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        std::fs::create_dir_all(home.join("ws")).expect("ws");
        std::fs::write(home.join("api.token"), format!("{TOKEN}\n")).expect("token");
        std::fs::write(
            home.join("config.toml"),
            r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 20
max_concurrency = 2

[api]
listen = "127.0.0.1:0"

[[api.tokens]]
principal = "operator:test"
token_file = "api.token"

[selfdeploy]
releases_dir = "releases"
repo = "repo"
detach = "inline"

[adapters.fake]
command = ["sh", "-c", "cat >/dev/null; echo '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'"]

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-a", "model-b"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.fake]
candidates = [{ source = "local", model = "model-a" }, { source = "local", model = "model-b" }]

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "fake", required_capabilities = [] }]
"#,
        )
        .expect("config");
        let config = Config::load(&home.join("config.toml")).expect("load");
        let sqlite = Arc::new(SqliteStore::open(&config.db.path).expect("open"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        let settings = celeris::api_settings(
            &config,
            addr,
            vec![task_api::ApiToken::new(
                task_api::Principal::Human {
                    name: "test".into(),
                },
                TOKEN,
            )],
            "01J00000000000000000000000".to_string(),
            "2026-10-17T00:00:00Z".to_string(),
            None,
            "dev".to_string(),
            DaemonMode::Normal,
            SharedRole::new(InstanceRole::Active),
            None,
        );
        let (daemon_tx, daemon_rx) = tokio::sync::watch::channel(None);
        let state = task_api::ApiState::new(settings, daemon_rx).expect("state");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
            let _ = stopped.await;
        }));
        Self {
            _dir: dir,
            config,
            sqlite,
            base: format!("http://{addr}"),
            _daemon_tx: daemon_tx,
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    /// 本番の組み立て（`build_dispatcher`）と同じ部品を、数えられる store（`self.sqlite`）で組む。
    fn dispatcher(&self) -> Dispatcher {
        let store: Arc<dyn TaskStore> = self.sqlite.clone();
        let mut d = Dispatcher::new(store, self.config.dispatch_config());
        d.set_llm_route_registry(self.config.llm_route_registry().expect("registry"));
        d.set_harness_adapters(celeris::build_harness_adapters(&self.config).expect("adapters"));
        d.set_routing_mode(self.config.routing.mode);
        d
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = reqwest::Client::new()
            .request(method, format!("{}{path}", self.base))
            .header("authorization", format!("Bearer {TOKEN}"));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.expect("request");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }

    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.call(reqwest::Method::GET, path, None).await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn create(
        &self,
        parent: Option<TaskId>,
        routing_override: Option<Value>,
    ) -> (u16, Value) {
        let mut body = json!({
            "title": "trial", "objective": "o", "genre": GENRE,
            "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
        });
        if let Some(parent) = parent {
            body["parent"] = json!(parent.to_string());
        }
        if let Some(ov) = routing_override {
            body["routing_override"] = ov;
        }
        self.post("/api/v1/tasks", body).await
    }

    async fn create_ok(&self, parent: Option<TaskId>, ov: Option<Value>) -> TaskId {
        let (s, v) = self.create(parent, ov).await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().expect("id").parse().expect("task id")
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(Duration::from_secs(60), handle)
                .await
                .expect("server stopped")
                .expect("server task")
                .expect("server result");
        }
    }
}

fn row(lane: &str, model: &str, pass_lower: f64) -> EvidenceRow {
    EvidenceRow {
        quota_cost_per_pass: None,
        key: EvidenceKey {
            lane: lane.into(),
            genre: GENRE.into(),
            harness_id: "fake".into(),
            harness_version: "1.0.0".into(),
            source_id: "local".into(),
            model_id: model.into(),
        },
        evidence_version: String::new(),
        n: 30,
        pass: 30,
        pass_lower,
        pass_upper: pass_lower + 0.01,
        pass_boot_lower: pass_lower,
        pass_boot_upper: 0.99,
        wall_p90_s: Some(10.0),
        usd_per_pass: Some(0.1),
        quota_per_run: BTreeMap::new(),
        timeout_rate: 0.0,
        low_confidence: false,
    }
}

/// 版 `version` の根拠表を入れる。`best` が最良（enforce で選ばれる側）。
fn publish_evidence(store: &SqliteStore, version: &str, best: &str, other: &str) {
    // 根拠表は lane ごと（ADR 2026-10-22 D5）。lane を持たない直接の選択（`decide`）と、tick が付ける
    // standard lane の dispatch の両方に同じ値を入れる。
    let rows: Vec<_> = ["", "standard"]
        .into_iter()
        .flat_map(|lane| [row(lane, best, 0.9), row(lane, other, 0.5)])
        .collect();
    store
        .replace_routing_evidence(version, &rows)
        .expect("evidence");
    store
        .set_routing_stage(
            TaskId::new(),
            &RoutingStageChange {
                genre: GENRE.into(),
                harness_id: "fake".into(),
                stage: RoutingStage::Enforce,
                since: "2026-10-17".into(),
                evidence_version: Some(version.into()),
                reason: "test".into(),
                window: json!({}),
                actor: "human".into(),
            },
        )
        .expect("stage");
}

/// task の判断（実 dispatcher が読む override → `select_route_with`）。(model, 記録された evidence_version)。
fn decide(d: &mut Dispatcher, store: &SqliteStore, task: TaskId) -> (String, Option<String>) {
    let ov = store.task_routing_override(task).expect("override");
    // RouteContext は dispatcher の公開面に無いので、型は select_route_with の引数から推論する
    // （f を先に書くと C が確定し、set の欄代入が型検査を通る）。
    fn with_ctx<C: Default, R>(f: impl FnOnce(&C) -> R, set: impl FnOnce(&mut C)) -> R {
        let mut c = C::default();
        set(&mut c);
        f(&c)
    }
    let (out, trace) = with_ctx(
        |ctx| {
            d.select_route_with(
                &task_dispatch::RouteInput {
                    task_id: task.to_string(),
                    harness: task_core::llm_route::HarnessRequest {
                        genre: Some(GENRE.into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ctx,
            )
        },
        |ctx| {
            ctx.evidence_mode = ov.as_ref().map(|o| o.evidence_mode).unwrap_or_default();
            ctx.evidence_version = ov.as_ref().and_then(|o| o.evidence_table_id.clone());
        },
    );
    let pick = out.expect("route");
    let version = trace.decision.and_then(|d| d.evidence_version);
    (pick.candidate.model.to_string(), version)
}

fn locks(store: &SqliteStore) -> u64 {
    let c = store.lock_counts();
    c.writer + c.reader
}

fn mode_of(v: &Value) -> Option<String> {
    v["routing_override"]["evidence_mode"]
        .as_str()
        .map(String::from)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn none_survives_post_child_grandchild_retry_and_ignores_table_updates() {
    let w = World::start().await;
    publish_evidence(&w.sqlite, "v1", "model-b", "model-a");

    // 通常 API で none / latest / pinned(v1) を同条件で起票する。
    let ov = |mode: &str, table: Option<&str>| {
        let mut o = json!({"mode": "fixed", "evidence_mode": mode, "online_update": false});
        if let Some(t) = table {
            o["evidence_table_id"] = json!(t);
        }
        Some(o)
    };
    let none_root = w.create_ok(None, ov("none", None)).await;
    let latest_root = w.create_ok(None, ov("latest", None)).await;
    let pinned_root = w.create_ok(None, ov("pinned", Some("v1"))).await;
    // none の子・孫（親の override を継ぐ。本文には書かない）。
    let none_child = w.create_ok(Some(none_root), None).await;
    let none_grand = w.create_ok(Some(none_child), None).await;
    // none の root を取り消して retry（複製の新しい root）。
    let (s, v) = w
        .post(&format!("/api/v1/tasks/{none_root}/cancel"), json!({}))
        .await;
    assert!((200..300).contains(&s), "{v}");
    let (s, v) = w
        .post(
            &format!("/api/v1/tasks/{none_root}/retry"),
            json!({"accept": false}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let none_retry: TaskId = v["task_id"].as_str().expect("task_id").parse().expect("id");

    // DTO（GET /tasks/{id}）で mode が判別できる。
    for (id, want) in [
        (none_root, "none"),
        (none_child, "none"),
        (none_grand, "none"),
        (none_retry, "none"),
        (latest_root, "latest"),
        (pinned_root, "pinned"),
    ] {
        let got = w.get(&format!("/api/v1/tasks/{id}")).await;
        assert_eq!(mode_of(&got).as_deref(), Some(want), "{id}");
    }
    let got = w.get(&format!("/api/v1/tasks/{pinned_root}")).await;
    assert_eq!(got["routing_override"]["evidence_table_id"], "v1");

    // 実 dispatcher の判断。none は固定の候補順（model-a）と table 版なし、latest/pinned は v1 の最良（model-b）。
    let mut d = w.dispatcher();
    let none_ids = [none_root, none_child, none_grand, none_retry];
    let before: Vec<_> = none_ids
        .iter()
        .map(|t| decide(&mut d, &w.sqlite, *t))
        .collect();
    for b in &before {
        assert_eq!(b, &("model-a".to_string(), None), "none は固定順・版なし");
    }
    assert_eq!(
        decide(&mut d, &w.sqlite, latest_root),
        ("model-b".to_string(), Some("v1".to_string()))
    );
    assert_eq!(
        decide(&mut d, &w.sqlite, pinned_root),
        ("model-b".to_string(), Some("v1".to_string()))
    );
    // lookup 回数: 同じ判断 1 回あたりの store 接続数。none は latest より根拠表 lookup の分だけ少ない。
    let cost = |d: &mut Dispatcher, t: TaskId| {
        let b = locks(&w.sqlite);
        decide(d, &w.sqlite, t);
        locks(&w.sqlite) - b
    };
    let none_cost_before = cost(&mut d, none_grand);
    let latest_cost = cost(&mut d, latest_root);
    assert!(
        latest_cost > none_cost_before,
        "latest={latest_cost} none={none_cost_before}"
    );

    // 途中で最新根拠表を差し替える（v2 は model-a が最良）。
    publish_evidence(&w.sqlite, "v2", "model-a", "model-b");
    for (t, b) in none_ids.iter().zip(&before) {
        assert_eq!(&decide(&mut d, &w.sqlite, *t), b, "none の判断は不変");
    }
    assert_eq!(
        cost(&mut d, none_grand),
        none_cost_before,
        "none は更新後も根拠表 lookup 0 回（接続数が lookup 無しの基準と同じ）"
    );
    assert_eq!(
        decide(&mut d, &w.sqlite, latest_root),
        ("model-a".to_string(), Some("v2".to_string())),
        "latest は更新に追従する"
    );
    assert_eq!(
        decide(&mut d, &w.sqlite, pinned_root),
        ("model-b".to_string(), Some("v1".to_string())),
        "pinned は指定版のまま"
    );
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contradictory_evidence_mode_is_rejected_before_any_task_exists() {
    let w = World::start().await;
    let count = || w.sqlite.list(None).expect("list").len();
    let base = count();
    for bad in [
        json!({"mode": "fixed", "evidence_mode": "none", "evidence_table_id": "v1"}),
        json!({"mode": "fixed", "evidence_mode": "pinned"}),
        json!({"mode": "fixed", "evidence_mode": "latest", "evidence_table_id": "v1"}),
        json!({"mode": "fixed", "evidence_mode": "bogus"}),
    ] {
        let (s, v) = w.create(None, Some(bad.clone())).await;
        assert!((400..500).contains(&s), "{bad}: {s} {v}");
    }
    assert_eq!(count(), base, "拒否した起票は task を作らない");
    w.shutdown().await;
}

/// task の `routing_decided` event を JSON で返す（実 dispatcher の tick が追記したもの）。
fn routing_decided(store: &SqliteStore, task: TaskId) -> Option<Value> {
    store
        .events_for(task)
        .expect("events")
        .into_iter()
        .map(|(_, e)| serde_json::to_value(&e).expect("event json"))
        .find(|v| v["type"] == "routing_decided")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ticked_runs_record_the_evidence_mode_and_the_table_version_actually_used() {
    let w = World::start().await;
    publish_evidence(&w.sqlite, "v1", "model-b", "model-a");
    let ov = |mode: &str, table: Option<&str>| {
        let mut o = json!({"mode": "fixed", "evidence_mode": mode, "online_update": false});
        if let Some(t) = table {
            o["evidence_table_id"] = json!(t);
        }
        Some(o)
    };
    let none = w.create_ok(None, ov("none", None)).await;
    let latest = w.create_ok(None, ov("latest", None)).await;
    let pinned = w.create_ok(None, ov("pinned", Some("v1"))).await;
    // 起票後・dispatch 前に最新根拠表を v2 へ差し替える。
    publish_evidence(&w.sqlite, "v2", "model-a", "model-b");

    let mut d = w.dispatcher();
    tokio::time::timeout(Duration::from_secs(60), async {
        while [none, latest, pinned]
            .iter()
            .any(|t| routing_decided(&w.sqlite, *t).is_none())
        {
            d.tick().expect("tick");
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("every task has a routing_decided event");

    let rec = |t| routing_decided(&w.sqlite, t).expect("event");
    let dec = |t| rec(t)["record"]["features"]["decision"].clone();
    let (n, l, p) = (dec(none), dec(latest), dec(pinned));
    // 記録で mode と実際に使った根拠表が判別できる（差し替え後の v2 は none/pinned に入らない）。
    assert_eq!(n["routing_resolved"]["evidence_mode"], "none");
    assert_eq!(l["routing_resolved"]["evidence_mode"], "latest");
    assert_eq!(p["routing_resolved"]["evidence_mode"], "pinned");
    assert_eq!(n["selected"]["model"], "model-a", "none は固定の候補順");
    assert!(n["evidence_version"].is_null(), "none は table 版なし: {n}");
    assert_eq!(l["evidence_version"], "v2", "latest は dispatch 時点の最新");
    assert_eq!(l["selected"]["model"], "model-a");
    assert_eq!(p["evidence_version"], "v1", "pinned は指定版");
    assert_eq!(p["selected"]["model"], "model-b");
    w.shutdown().await;
}
