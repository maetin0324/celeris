//! ADR 2026-10-14-local-machine-settings D3・D4 の結線: 実効の `max_concurrency`（store > 設定ファイル、reload でも
//! store が勝つ）が `PUT /clusters/local/settings` → admin チャネル → dispatcher に届くこと、CLI の
//! `--workspace-root` が store と task ごとの固定を無視すること、`[[clusters]] id = "local"` が読み込みエラーで
//! あることを、daemon と同じ組み立て（`Config::load` → `build_dispatcher` / `api_settings` → loopback の HTTP）で
//! 確かめる。check が `cargo test -p celeris --bins -- --exact <名前>` で引くので、試験の入口は `main.rs` の
//! crate 直下に置き、本体はこの module に置く。外部ネットワーク・userns・本番の `~/.local/celeris` には触れない。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use celeris::{ClusterMasters, Config, Overrides};
use serde_json::{Value, json};
use task_core::local_machine::{SettingSource, WorkspaceRoots};
use task_core::{
    DaemonMode, InstanceRole, LocalSettingsOverrides, SharedRole, SqliteStore, TaskId, TaskStore,
};
use time::OffsetDateTime;

const TOKEN: &str = "local-settings-wire-token";

fn config_toml(max_concurrency: usize, extra: &str) -> String {
    format!(
        r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200
max_concurrency = {max_concurrency}

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

[route_defaults.fake]
candidates = [{{ source = "local", model = "fake" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "fake", required_capabilities = [] }}]
{extra}
"#
    )
}

fn write_home(home: &Path, max_concurrency: usize) -> PathBuf {
    std::fs::create_dir_all(home.join("ws")).unwrap_or_else(|e| panic!("ws: {e}"));
    std::fs::write(home.join("api.token"), format!("{TOKEN}\n"))
        .unwrap_or_else(|e| panic!("token: {e}"));
    let path = home.join("config.toml");
    std::fs::write(&path, config_toml(max_concurrency, ""))
        .unwrap_or_else(|e| panic!("config: {e}"));
    path
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// `celeris::api_settings` から作った `ApiState` を loopback に立てる（`admin_tx` は呼び出し元が持つ）。
struct Api {
    base: String,
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
}

impl Api {
    async fn start(
        config: &Config,
        admin_tx: Option<tokio::sync::mpsc::Sender<task_api::AdminRequest>>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("bind: {e}"));
        let addr: SocketAddr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("addr: {e}"));
        let settings = celeris::api_settings(
            config,
            addr,
            vec![task_api::ApiToken::new(
                task_api::Principal::Human {
                    name: "test".into(),
                },
                TOKEN,
            )],
            "01J00000000000000000000000".to_string(),
            "2026-10-14T00:00:00Z".to_string(),
            admin_tx,
            "dev".to_string(),
            DaemonMode::Normal,
            SharedRole::new(InstanceRole::Active),
            None,
        );
        let (daemon_tx, daemon_rx) = tokio::sync::watch::channel(None);
        let state =
            task_api::ApiState::new(settings, daemon_rx).unwrap_or_else(|e| panic!("state: {e}"));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
            let _ = stopped.await;
        }));
        Self {
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
            .unwrap_or_else(|e| panic!("get {path}: {e}"));
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn put_local(&self, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .put(format!("{}/api/v1/clusters/local/settings", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("put: {e}"));
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

/// admin チャネルに届いた要求が `LocalSettingsChanged` であることを確かめ、daemon の admin 処理と同じ
/// `celeris::local_settings_changed` で dispatcher に渡す。
async fn deliver_local_settings_changed(
    rx: &mut tokio::sync::mpsc::Receiver<task_api::AdminRequest>,
    dispatcher: &mut task_dispatch::Dispatcher,
) {
    let request = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("no admin request after PUT"))
        .unwrap_or_else(|| panic!("admin channel closed"));
    assert!(
        matches!(request, task_api::AdminRequest::LocalSettingsChanged),
        "unexpected admin request"
    );
    celeris::local_settings_changed(dispatcher);
}

/// D3・D4: 実効の `max_concurrency` は store > 設定ファイル。PUT の admin 通知で dispatcher が読み直し、
/// reload で設定ファイルの値が変わっても store の値が勝つ。上書きを消すと（reload 後の）設定ファイルの値に戻る。
/// store を直接書いた場合も次の tick の頭で読み直す。
pub(crate) async fn effective_max_concurrency_store_beats_config() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let config_path = write_home(dir.path(), 2);
    let mut config = Config::load(&config_path).unwrap_or_else(|e| panic!("load: {e}"));
    let mut dispatcher = celeris::build_dispatcher(&config, ClusterMasters::default())
        .unwrap_or_else(|e| panic!("build_dispatcher: {e}"));
    assert_eq!(dispatcher.max_concurrency(), 2, "no store override: config");

    let (admin_tx, mut admin_rx) = tokio::sync::mpsc::channel(8);
    let api = Api::start(&config, Some(admin_tx)).await;

    // PUT（管理 API）→ admin チャネル → dispatcher。
    let (status, body) = api.put_local(json!({"max_concurrency": 5})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["max_concurrency"]["value"], json!(5), "{body}");
    assert_eq!(
        body["max_concurrency"]["source"],
        json!("settings"),
        "{body}"
    );
    assert_eq!(body["max_concurrency"]["config_value"], json!(2), "{body}");
    assert_eq!(
        body["max_concurrency"]["apply"],
        json!("next_tick"),
        "{body}"
    );
    assert_eq!(dispatcher.max_concurrency(), 2, "not yet delivered");
    deliver_local_settings_changed(&mut admin_rx, &mut dispatcher).await;
    assert_eq!(dispatcher.max_concurrency(), 5, "store beats config");

    // reload: 設定ファイルの値を 3 に変えても store の 5 が勝つ。
    std::fs::write(&config_path, config_toml(3, "")).unwrap_or_else(|e| panic!("rewrite: {e}"));
    celeris::reload_providers(&mut dispatcher, &mut config)
        .unwrap_or_else(|e| panic!("reload: {e}"));
    assert_eq!(config.max_concurrency, 3, "reload reads the config file");
    assert_eq!(
        dispatcher.max_concurrency(),
        5,
        "store still beats config after reload"
    );
    dispatcher.tick().unwrap_or_else(|e| panic!("tick: {e}"));
    assert_eq!(
        dispatcher.max_concurrency(),
        5,
        "tick keeps the store value"
    );

    // 上書きを消す（全置換で null）→ reload 後の設定ファイルの値。
    let (status, body) = api.put_local(json!({})).await;
    assert_eq!(status, 200, "{body}");
    deliver_local_settings_changed(&mut admin_rx, &mut dispatcher).await;
    assert_eq!(
        dispatcher.max_concurrency(),
        3,
        "override cleared: config file value"
    );

    // store を直接書いても（admin 通知が無くても）次の tick の頭で読み直す。
    let store = SqliteStore::open(&config.db.path).unwrap_or_else(|e| panic!("open: {e}"));
    store
        .local_settings_set(
            &LocalSettingsOverrides {
                workspace_root: None,
                max_concurrency: Some(7),
            },
            "admin",
            now(),
        )
        .unwrap_or_else(|e| panic!("local_settings_set: {e}"));
    assert_eq!(dispatcher.max_concurrency(), 3);
    dispatcher.tick().unwrap_or_else(|e| panic!("tick: {e}"));
    assert_eq!(dispatcher.max_concurrency(), 7, "next tick reads the store");

    // 新しい dispatcher（再起動）も store の値から始まる。
    let restarted = celeris::build_dispatcher(&config, ClusterMasters::default())
        .unwrap_or_else(|e| panic!("build_dispatcher: {e}"));
    assert_eq!(
        restarted.max_concurrency(),
        7,
        "restart starts from the store value"
    );
    api.stop().await;
}

/// D3 の 1: CLI の `--workspace-root` があるときは store の `workspace_root` と task ごとの固定をどちらも無視し、
/// 全 task を CLI の root で解く。新しい task にも固定を書かない（staging が本番の作業場所に書かない）。
pub(crate) async fn cli_workspace_root_ignores_store_and_pins() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let home = dir.path();
    let config_path = write_home(home, 2);
    let cli_root = home.join("cli-ws");
    let store_root = home.join("store-ws");
    let pinned_root = home.join("pinned-ws");
    for d in [&cli_root, &store_root, &pinned_root] {
        std::fs::create_dir_all(d).unwrap_or_else(|e| panic!("mkdir: {e}"));
    }
    let mut config = Config::load(&config_path).unwrap_or_else(|e| panic!("load: {e}"));
    config.apply_overrides(&Overrides {
        workspace_root: Some(cli_root.clone()),
        ..Overrides::default()
    });
    let store = SqliteStore::open(&config.db.path).unwrap_or_else(|e| panic!("open: {e}"));
    store
        .local_settings_set(
            &LocalSettingsOverrides {
                workspace_root: Some(store_root.to_string_lossy().into_owned()),
                max_concurrency: None,
            },
            "admin",
            now(),
        )
        .unwrap_or_else(|e| panic!("local_settings_set: {e}"));
    let pinned_task = TaskId::new();
    store
        .task_workspace_root_pin(&pinned_task, &pinned_root.to_string_lossy(), now())
        .unwrap_or_else(|e| panic!("pin: {e}"));

    // resolver（daemon の dispatcher・API が共有する `Config::workspace_roots`）。
    let roots: WorkspaceRoots = config.workspace_roots();
    let effective = roots
        .effective_root_in(&store)
        .unwrap_or_else(|e| panic!("effective: {e}"));
    assert_eq!(effective.value, cli_root);
    assert_eq!(effective.source, SettingSource::Cli);
    assert_eq!(
        roots
            .task_root(&store, &pinned_task)
            .unwrap_or_else(|e| panic!("task_root: {e}")),
        cli_root,
        "the pin is ignored under the CLI override"
    );
    let new_task = TaskId::new();
    let pinned = roots
        .pin_task_root(&store, &new_task, None, now())
        .unwrap_or_else(|e| panic!("pin_task_root: {e}"));
    assert_eq!(pinned, cli_root);
    assert_eq!(
        store
            .task_workspace_root_get(&new_task)
            .unwrap_or_else(|e| panic!("pin get: {e}")),
        None,
        "no pin is written under the CLI override"
    );
    assert_eq!(
        store
            .task_workspace_root_get(&pinned_task)
            .unwrap_or_else(|e| panic!("pin get: {e}")),
        Some(pinned_root.to_string_lossy().into_owned()),
        "the existing pin row is kept untouched"
    );

    // API の応答も CLI の root と source "cli"（store の値ではない）。
    let api = Api::start(&config, None).await;
    let (status, body) = api.get("/api/v1/clusters").await;
    assert_eq!(status, 200, "{body}");
    let local = &body["local"];
    assert_eq!(
        local["workspace_root"]["value"],
        json!(cli_root.to_string_lossy()),
        "{body}"
    );
    assert_eq!(local["workspace_root"]["source"], json!("cli"), "{body}");
    api.stop().await;
}

/// D4: `local` はローカルマシンの行の予約 id。`[[clusters]] id = "local"` は設定の読み込みエラー。
pub(crate) fn cluster_id_local_is_reserved_config_error() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let home = dir.path();
    write_home(home, 2);
    let cluster = |id: &str| format!("\n[[clusters]]\nid = \"{id}\"\nhost = \"h\"\n");

    let reserved = home.join("reserved.toml");
    std::fs::write(&reserved, config_toml(2, &cluster("local")))
        .unwrap_or_else(|e| panic!("write: {e}"));
    match Config::load(&reserved) {
        Ok(_) => panic!("[[clusters]] id = \"local\" must be a config error"),
        Err(e) => {
            let message = e.to_string();
            assert!(message.contains("reserved"), "{message}");
            assert!(message.contains("\"local\""), "{message}");
        }
    }

    // 対照: 予約でない id は読める。
    let other = home.join("other.toml");
    std::fs::write(&other, config_toml(2, &cluster("local2")))
        .unwrap_or_else(|e| panic!("write: {e}"));
    let config = Config::load(&other).unwrap_or_else(|e| panic!("load: {e}"));
    assert_eq!(config.clusters.len(), 1);
    assert_eq!(config.clusters[0].id, "local2");
}

/// daemon-identity-api: 実 `Config::load`（＋CLI 上書き）→ `api_settings` → 実 router の `GET /config` が
/// `identity` に config ファイルの sha256・適用した上書き・実効値を返し、token の値は応答のどこにも出ない。
pub(crate) async fn config_identity_reports_effective_values_without_secrets() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let config_path = write_home(dir.path(), 2);
    let mut config = Config::load(&config_path).unwrap_or_else(|e| panic!("load: {e}"));
    let other_db = dir.path().join("other.sqlite3");
    config.apply_overrides(&Overrides {
        db: Some(other_db.clone()),
        ..Default::default()
    });
    let api = Api::start(&config, None).await;
    let (status, body) = api.get("/api/v1/config").await;
    assert_eq!(status, 200, "{body}");
    let text = body.to_string();
    assert!(!text.contains(TOKEN), "token value leaked: {text}");

    let id = &body["identity"];
    let bytes = std::fs::read(&config_path).unwrap_or_else(|e| panic!("read: {e}"));
    let expected: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(id["config_sha256"], json!(expected), "{body}");
    assert_eq!(id["overrides"], json!(["db"]), "{body}");
    assert_eq!(id["instance_id"], json!("01J00000000000000000000000"));
    assert_eq!(id["release"], json!("dev"));
    assert_eq!(id["pid"], json!(std::process::id()));
    let eff = &id["effective"];
    assert_eq!(eff["db"], json!(other_db.display().to_string()), "{body}");
    assert_eq!(eff["selfdeploy_push"], json!(true), "{body}");
    assert!(
        !text.contains("api.token"),
        "token_file location leaked: {text}"
    );
    assert_eq!(eff["llm_proxy_listen"], Value::Null, "{body}");
    assert_eq!(eff["memory_dir"], Value::Null, "{body}");
    api.stop().await;
}
