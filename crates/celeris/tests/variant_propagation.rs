//! 通常の daemon 接続境界での variant 伝播（ADR 2026-10-16-campaign-identity-propagation と variant-contract）。
//!
//! `campaign_wiring_e2e` と同じ一時 DB・loopback 専用 daemon・fake harness を使い、実 LLM は起動しない
//! （config の harness / source はすべて `fake`。下の試験で assert する）。userns 不要。
//! spec-on / spec-off は別の所有環境（別の一時 DB）で、org の準備はその一時 DB だけに行う。
//! 通常の `POST /tasks` → `GET` → 冪等再送 → DB 再 open → 子孫 → dispatcher（本番と同じ
//! `build_daemon_dispatcher` の tick）で、同じ hash / org pin / Assigned / 決定ログ / run 証拠を確かめる。
//! 非対象全体の封印照合は campaign-proof の fake 試験が担い、この試験は daemon 接続境界だけを見る。

use std::net::SocketAddr;
use std::time::Duration;

use celeris::{ClusterMasters, Config, InstanceIdentity, build_daemon_dispatcher};
use serde_json::{Value, json};
use task_core::org::{OrgKind, OrgNode};
use task_core::variant_wire::effective_profile_sha256;
use task_core::{
    DaemonMode, Event, InstanceRole, SharedRole, SqliteStore, Status, TaskId, TaskStore, profile,
};

const H1: &str = "b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44";
const H2: &str = "c7243b55c7243b55c7243b55c7243b55c7243b55c7243b55c7243b55c7243b55";

const TOKEN: &str = "variant-propagation-token";

struct Daemon {
    _dir: tempfile::TempDir,
    store: SqliteStore,
    config: Config,
    /// 実 daemon（`celeris::run`）と同じく、API の instance id と dispatcher の観測値に同じ値を使う。
    identity: InstanceIdentity,
    /// スタブ worker process が受け取った `RunRequest` を 1 run 1 file で書く dir。
    requests: std::path::PathBuf,
    base: String,
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
}

impl Daemon {
    async fn start(nodes: &[OrgNode]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        std::fs::create_dir_all(home.join("ws")).expect("ws");
        std::fs::write(home.join("api.token"), format!("{TOKEN}\n")).expect("token");
        let requests = home.join("requests");
        std::fs::create_dir_all(&requests).expect("requests");
        std::fs::write(
            home.join("config.toml"),
            format!(
                r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200

[api]
listen = "127.0.0.1:0"
token_file = "api.token"

[selfdeploy]
releases_dir = "releases"
repo = "repo"
detach = "inline"

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]
command = "sh"
args = ["-c", "cat > \"$0/run-$$.json\"; echo '{{\"type\":\"done\",\"summary\":\"stub\",\"evidence\":[]}}'", "{requests}"]

[route_defaults.fake]
candidates = [{{ source = "local", model = "fake" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "fake", required_capabilities = [] }}]
"#,
                requests = requests.display()
            ),
        )
        .expect("config");
        let config = Config::load(&home.join("config.toml")).expect("load");
        let store = SqliteStore::open(&config.db.path).expect("open");
        store.replace_org_nodes(nodes).expect("org");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        let identity = InstanceIdentity::new(None);
        let settings = celeris::api_settings(
            &config,
            addr,
            Some(TOKEN.to_string()),
            identity.instance_id.clone(),
            "2026-10-16T00:00:00Z".to_string(),
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
            store,
            config,
            identity,
            requests,
            base: format!("http://{addr}"),
            _daemon_tx: daemon_tx,
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .get(format!("{}{path}", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .send()
            .await
            .expect("get");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(&body)
            .send()
            .await
            .expect("post");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
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

fn node(id: &str, parent: Option<&str>, genre: Option<&str>, skills: &[&str]) -> OrgNode {
    OrgNode {
        id: id.into(),
        parent_id: parent.map(Into::into),
        name: id.into(),
        kind: if parent.is_none() {
            OrgKind::Secretary
        } else {
            OrgKind::Section
        },
        genre: genre.map(Into::into),
        brief: id.into(),
        profile: {
            let mut p = profile::Profile {
                skills: skills.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            };
            p.harnesses.allowed = vec!["coding".into()];
            p
        },
        position: 0,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

/// spec-off: 根 + 単一の coding 課。spec-on: 根 + coding 課 + 専門課（別環境の org）。
fn org(spec_on: bool, skill: &str) -> Vec<OrgNode> {
    let mut v = vec![
        node("secretary", None, None, &[]),
        node("coding", Some("secretary"), Some("coding"), &[skill]),
    ];
    if spec_on {
        v.push(node(
            "coding-spec",
            Some("secretary"),
            Some("coding"),
            &[skill],
        ));
    }
    v
}

fn pin(nodes: &[OrgNode]) -> Value {
    json!({
        "nodes": nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
        "coding_node": "coding",
        "effective_profile_sha256":
            effective_profile_sha256(&profile::resolve(nodes, "coding")).expect("hash"),
    })
}

fn identity(content: &str, pin: Value) -> Value {
    json!({"campaign_id": "vp", "case_id": "case-a", "arm": "B", "repetition": 1,
           "manifest_hash": "m1", "routing_setting": "fixed",
           "variant": {"comparison_id": "M1", "id": "v1",
                       "content_sha256": content, "selection_sha256": H1},
           "variant_org": pin})
}

fn root_body(campaign: Value) -> Value {
    json!({"title": "trial", "objective": "o", "genre": "coding",
           "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
           "campaign": campaign, "campaign_idempotency_key": "campaign-vp-1"})
}

fn tid(v: &Value) -> TaskId {
    v["id"].as_str().expect("id").parse().expect("task id")
}

async fn child(d: &Daemon, parent: TaskId, campaign: Option<Value>) -> (u16, Value) {
    let mut b = json!({"title": "c", "objective": "child", "parent": parent.to_string(),
        "acceptance": [{"type": "artifact_exists", "name": "r"}]});
    if let Some(c) = campaign {
        b["campaign"] = c;
        b["campaign_idempotency_key"] = json!(format!("child-key-{}", b["title"]));
    }
    d.post("/api/v1/tasks", b).await
}

fn all_fake(config: &Config) {
    // 実 LLM を起動できない構成: 全 source / harness が fake。
    let text = std::fs::read_to_string(config.db.path.parent().expect("dir").join("config.toml"))
        .expect("config");
    for line in text.lines().filter(|l| l.trim_start().starts_with("kind")) {
        assert!(line.contains("\"fake\""), "non-fake kind: {line}");
    }
    assert!(!text.contains("claude") && !text.contains("codex") && !text.contains("anthropic"));
}

/// 通常経路: POST → GET → 冪等 → 再 open → 子孫 → 矛盾拒否。spec-on / spec-off は別環境。
async fn normal_path(spec_on: bool) {
    let nodes = org(spec_on, "x");
    let d = Daemon::start(&nodes).await;
    all_fake(&d.config);
    let good = root_body(identity(H1, pin(&nodes)));

    let (s, a) = d.post("/api/v1/tasks", good.clone()).await;
    assert_eq!(s, 201, "{a}");
    let root = tid(&a);
    let (s, again) = d.post("/api/v1/tasks", good.clone()).await;
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["id"], a["id"]);

    // GET は同じ hash と org pin を返す。
    let (s, detail) = d.get(&format!("/api/v1/tasks/{root}")).await;
    assert_eq!(s, 200, "{detail}");
    assert_eq!(detail["campaign"]["variant"]["content_sha256"], json!(H1));
    assert_eq!(
        detail["campaign"]["variant_org"],
        good["campaign"]["variant_org"]
    );

    // 同じ campaign で content だけ違う再送、構成 pin の脱落は拒否（409）。
    let (s, v) = d
        .post("/api/v1/tasks", root_body(identity(H2, pin(&nodes))))
        .await;
    assert_eq!(s, 409, "{v}");
    let mut stripped = identity(H1, pin(&nodes));
    stripped.as_object_mut().expect("obj").remove("variant_org");
    let (s, v) = d.post("/api/v1/tasks", root_body(stripped)).await;
    assert!(s == 400 || s == 409, "{s} {v}");
    assert_eq!(d.store.list(None).expect("list").len(), 1);

    // 子・孫は祖先の識別を継ぐ。祖先と矛盾する識別を持つ子は保存されない。
    let (s, c) = child(&d, root, None).await;
    assert_eq!(s, 201, "{c}");
    let c_id = tid(&c);
    let (s, g) = child(&d, c_id, None).await;
    assert_eq!(s, 201, "{g}");
    let g_id = tid(&g);
    let (s, gd) = d.get(&format!("/api/v1/tasks/{g_id}")).await;
    assert_eq!(s, 200, "{gd}");
    assert_eq!(gd["campaign"]["variant"]["content_sha256"], json!(H1));
    assert_eq!(
        gd["campaign"]["variant_org"],
        good["campaign"]["variant_org"]
    );
    let before = d.store.list(None).expect("list").len();
    let (s, v) = child(&d, root, Some(identity(H2, pin(&nodes)))).await;
    assert!(
        s == 400 || s == 409,
        "contradicting child accepted: {s} {v}"
    );
    assert_eq!(d.store.list(None).expect("list").len(), before);

    // DB 再 open でも同じ解決（hash・pin・祖先継承）。
    let reopened = SqliteStore::open(&d.config.db.path).expect("reopen");
    let own = reopened
        .task_campaign_resolve(&g_id.to_string())
        .expect("resolve")
        .expect("campaign");
    assert_eq!(own.variant.as_ref().expect("variant").content_sha256, H1);
    assert_eq!(
        own.variant_org.as_ref().expect("pin").nodes.len(),
        nodes.len()
    );
    drop(reopened);

    // dispatcher（本番と同じ組み立て）: 既存 matching で Assigned、決定は共有 org を書き換えない。
    let org_before = d.store.org_list().expect("org");
    let mut dispatcher = build_daemon_dispatcher(&d.config, ClusterMasters::default(), &d.identity)
        .expect("build_daemon_dispatcher");
    dispatcher.tick().expect("tick");
    let t = d.store.get(root).expect("get").expect("task");
    assert_ne!(
        t.status,
        Status::Blocked,
        "pinned org must not block: {:?}",
        d.store.events_for(root).expect("events")
    );
    if let Some(a) = t.assignee.as_deref() {
        let allowed: Vec<&str> = if spec_on {
            vec!["coding", "coding-spec"]
        } else {
            vec!["coding"]
        };
        assert!(allowed.contains(&a), "assignee {a} outside the pinned org");
    }
    assert_eq!(d.store.org_list().expect("org"), org_before);
    // Assigned の証拠: 既存 matching の決定（spec-off は単一の coding 課）が pin の範囲に入る。
    let assigned: Vec<String> = d
        .store
        .events_for(root)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::Assigned { node, .. } => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(assigned, vec!["coding".to_string()]);
    d.shutdown().await;
}

#[tokio::test]
async fn spec_off_normal_path_keeps_hash_and_org() {
    normal_path(false).await;
}

#[tokio::test]
async fn spec_on_normal_path_keeps_hash_and_org() {
    normal_path(true).await;
}

/// 負: 割当後（または起動前）の org drift は run を起動せず blocked＋理由 event。
#[tokio::test]
async fn org_drift_after_post_blocks_before_any_run() {
    let nodes = org(false, "x");
    let d = Daemon::start(&nodes).await;
    let (s, a) = d
        .post("/api/v1/tasks", root_body(identity(H1, pin(&nodes))))
        .await;
    assert_eq!(s, 201, "{a}");
    let root = tid(&a);
    // 専用環境の org が POST 後に変わった（別 skill）。
    d.store.replace_org_nodes(&org(false, "y")).expect("org");
    let mut dispatcher = build_daemon_dispatcher(&d.config, ClusterMasters::default(), &d.identity)
        .expect("build_daemon_dispatcher");
    dispatcher.tick().expect("tick");
    let t = d.store.get(root).expect("get").expect("task");
    let events = d.store.events_for(root).expect("events");
    let started = events
        .iter()
        .any(|(_, e)| matches!(e, Event::WorkerStarted { .. }));
    assert!(!started, "run started under drift: {events:?}");
    assert_eq!(t.status, Status::Blocked, "{events:?}");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::QuestionRaised { text, .. } if text.contains("variant pin check failed")
    )));
    assert!(t.assignee.is_none(), "assigned under drift: {t:?}");
    d.shutdown().await;
}

/// 負: 別 assignee（pin 外の課）を持つ task は割り当てない。
#[tokio::test]
async fn foreign_assignee_is_not_accepted() {
    let nodes = org(false, "x");
    let d = Daemon::start(&nodes).await;
    let (s, a) = d
        .post("/api/v1/tasks", root_body(identity(H1, pin(&nodes))))
        .await;
    assert_eq!(s, 201, "{a}");
    let root = tid(&a);
    // pin 外の課（org に後から足されたのではなく、assignee だけが別）を直接付ける。
    let mut t = d.store.get(root).expect("get").expect("task");
    t.assignee = Some("elsewhere".into());
    d.store
        .update_task(
            &t,
            Event::Answered {
                question: "q".into(),
                answer: "a".into(),
            },
        )
        .expect("update");
    let mut dispatcher = build_daemon_dispatcher(&d.config, ClusterMasters::default(), &d.identity)
        .expect("build_daemon_dispatcher");
    dispatcher.tick().expect("tick");
    let events = d.store.events_for(root).expect("events");
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { .. })),
        "run started with a foreign assignee: {events:?}"
    );
    d.shutdown().await;
}

/// 負: daemon の観測値（instance id / config sha256）が無い dispatcher は variant 付き run を起動しない
/// （適用済みと扱わない）。起動経路の `build_daemon_dispatcher` も sha256 が無ければ観測値を渡さない。
#[tokio::test]
async fn missing_daemon_observation_blocks_the_run() {
    let nodes = org(false, "x");
    let d = Daemon::start(&nodes).await;
    let (s, a) = d
        .post("/api/v1/tasks", root_body(identity(H1, pin(&nodes))))
        .await;
    assert_eq!(s, 201, "{a}");
    let root = tid(&a);
    // 観測値の無い daemon（設定がファイル由来でなく config sha256 が無い）。起動経路の組み立てでも
    // 観測値は渡らない。
    let mut config = d.config.clone();
    config.source_sha256 = None;
    let mut dispatcher = build_daemon_dispatcher(&config, ClusterMasters::default(), &d.identity)
        .expect("build_daemon_dispatcher");
    dispatcher.tick().expect("tick");
    let events = d.store.events_for(root).expect("events");
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { .. })),
        "{events:?}"
    );
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::QuestionRaised { text, .. } if text.contains("not available")
    )));
    d.shutdown().await;
}

/// daemon 起動経路の観測値配線: `build_daemon_dispatcher` が渡す instance id と config sha256 が、
/// 実 process のスタブ worker が受け取った `RunRequest.context.campaign` に載り、`GET /health` の
/// instance id・`GET /config` の `identity.config_sha256`（= 設定ファイルの bytes の sha256）と一致する。
#[tokio::test(flavor = "multi_thread")]
async fn daemon_build_path_pins_observed_identity() {
    use sha2::{Digest, Sha256};

    let nodes = org(false, "x");
    let d = Daemon::start(&nodes).await;
    all_fake(&d.config);
    let (s, a) = d
        .post("/api/v1/tasks", root_body(identity(H1, pin(&nodes))))
        .await;
    assert_eq!(s, 201, "{a}");
    let root = tid(&a);

    let (s, health) = d.get("/api/v1/health").await;
    assert_eq!(s, 200, "{health}");
    let (s, cfg) = d.get("/api/v1/config").await;
    assert_eq!(s, 200, "{cfg}");
    let file = std::fs::read(d.config.db.path.parent().expect("dir").join("config.toml"))
        .expect("config bytes");
    let file_sha: String = Sha256::digest(&file)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(health["instance_id"], json!(d.identity.instance_id));
    assert_eq!(cfg["identity"]["config_sha256"], json!(file_sha), "{cfg}");

    let mut dispatcher = build_daemon_dispatcher(&d.config, ClusterMasters::default(), &d.identity)
        .expect("build_daemon_dispatcher");
    // 出来事待ち＋期限: スタブ process の終了（WorkerFinished）まで tick を回す。
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        dispatcher.tick().expect("tick");
        let events = d.store.events_for(root).expect("events");
        if events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { .. }))
        {
            break;
        }
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, Event::QuestionRaised { .. })),
            "run did not start: {events:?}"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "no WorkerFinished: {events:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let requests: Vec<Value> = std::fs::read_dir(&d.requests)
        .expect("requests")
        .map(|e| {
            let text = std::fs::read_to_string(e.expect("entry").path()).expect("request");
            serde_json::from_str(text.lines().next().expect("line")).expect("json")
        })
        .collect();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let pin = &requests[0]["context"]["campaign"];
    assert_eq!(requests[0]["task"]["id"], json!(root.to_string()));
    assert_eq!(pin["daemon_instance_id"], health["instance_id"], "{pin}");
    assert_eq!(pin["config_sha256"], json!(file_sha), "{pin}");
    assert_eq!(pin["identity"]["variant"]["content_sha256"], json!(H1));
    d.shutdown().await;
}
