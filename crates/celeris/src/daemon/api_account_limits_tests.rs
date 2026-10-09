//! ADR 2026-10-08 account-concurrency-limits §(d)(e): `PATCH /api/v1/accounts/{adapter}/{id}` と `GET /accounts` を、
//! 本物の API（`api_settings` → `ApiState`）・本物の管理要求の処理（`handle_admin_request`）・本物の `Dispatcher` で通す。

use std::path::Path;

use serde_json::{Value, json};
use task_core::{InstanceRole, SqliteStore, TaskStore};

use super::*;
use crate::daemon::admin::handle_admin_request;
use crate::{accounts_admin, build_dispatcher};

const TOKEN: &str = "account-limits-token";

fn write_config(dir: &Path, rows: &str) -> PathBuf {
    let path = dir.join("config.toml");
    let db = dir.join("celeris.db");
    let ws = dir.join("ws");
    let accounts = dir.join("claude-accounts");
    std::fs::create_dir_all(dir.join("providers.d")).expect("providers.d");
    std::fs::write(
        &path,
        format!(
            "db = {db:?}\nworkspace_root = {ws:?}\nproviders_include = \"providers.d/*.toml\"\n\
             [accounts]\nclaude_dir = {accounts:?}\nmax_runs_per_account = 2\n\
             [[providers]]\nid = \"pool\"\nadapter = \"claude-code\"\naccount_pool = true\nconcurrency = 1\n\
             [[providers]]\nid = \"qwen\"\nadapter = \"fake\"\nconcurrency = 1\n{rows}"
        ),
    )
    .expect("write config");
    path
}

/// 管理要求を本物の処理へ渡し、そのたびに tick してスナップショット（暗黙アカウントの一覧）を更新する。
async fn admin_loop(
    dispatcher: &mut Dispatcher,
    config: &mut Config,
    mut rx: tokio::sync::mpsc::Receiver<task_api::AdminRequest>,
) {
    let (check_tx, _check_rx) = tokio::sync::mpsc::channel(16);
    let (account_tx, _account_rx) = tokio::sync::mpsc::channel(16);
    let (cluster_tx, _cluster_rx) = tokio::sync::mpsc::channel(16);
    let login_sessions = accounts_admin::new_sessions();
    let codex_login_sessions = accounts_admin::new_codex_sessions();
    let masters: super::super::clusters::ClusterMasters = Default::default();
    while let Some(req) = rx.recv().await {
        handle_admin_request(
            dispatcher,
            config,
            req,
            check_tx.clone(),
            login_sessions.clone(),
            codex_login_sessions.clone(),
            account_tx.clone(),
            Default::default(),
            Arc::clone(&masters),
            cluster_tx.clone(),
        )
        .await;
        dispatcher.tick().expect("tick");
    }
}

fn item<'a>(list: &'a Value, adapter: &str, id: &str) -> &'a Value {
    list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|v| v["adapter"] == adapter && v["id"] == id)
        .unwrap_or_else(|| panic!("{adapter}/{id} not in {list}"))
}

#[tokio::test]
async fn patch_account_limits_validate_persist_and_survive_reload_and_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = write_config(dir.path(), "");
    let accounts = dir.path().join("claude-accounts");
    for id in ["a", "check", "login"] {
        std::fs::create_dir_all(accounts.join(id)).expect("account dir");
    }
    let mut config = Config::load(&config_path).expect("config");
    let mut dispatcher = build_dispatcher(&config, Default::default()).expect("dispatcher");
    let (snap_tx, snap_rx) = tokio::sync::watch::channel(None);
    dispatcher.set_snapshot_publisher(SnapshotPublisher {
        tx: snap_tx,
        instance_id: "i".into(),
        hostname: "h".into(),
        started_at: "t".into(),
        tick_ms: config.tick_ms,
        providers: provider_lives(&config),
        provider_checks: std::collections::HashMap::new(),
    });
    dispatcher.tick().expect("first tick");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (admin_tx, admin_rx) = tokio::sync::mpsc::channel(8);
    let settings = api_settings(
        &config,
        addr,
        Some(TOKEN.into()),
        "i".into(),
        "t".into(),
        Some(admin_tx),
        "sha12sha12ab".into(),
        DaemonMode::Normal,
        SharedRole::new(InstanceRole::Active),
        None,
    );
    let state = tokio::task::spawn_blocking(move || ApiState::new(settings, snap_rx))
        .await
        .expect("join")
        .expect("state");
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
        let _ = stop_rx.await;
    }));
    let base = format!("http://{addr}/api/v1");
    let client = reqwest::Client::new();
    let qwen_dir = dir.path().to_path_buf();

    let requests = async {
        let get = || async {
            let resp = client
                .get(format!("{base}/accounts"))
                .bearer_auth(TOKEN)
                .send()
                .await
                .expect("get");
            assert_eq!(resp.status().as_u16(), 200);
            resp.json::<Value>().await.expect("json")
        };
        let patch = |path: String, body: Value| {
            let client = client.clone();
            let url = format!("{base}/accounts/{path}");
            async move {
                let resp = client
                    .patch(url)
                    .bearer_auth(TOKEN)
                    .json(&body)
                    .send()
                    .await
                    .expect("patch");
                let status = resp.status().as_u16();
                (status, resp.json::<Value>().await.expect("json"))
            }
        };

        // GET: 実アカウントの既定、暗黙アカウント（既定。旧 concurrency は使わない）。プール行には暗黙アカウントが無い。
        let list = get().await;
        assert_eq!(list["max_runs_per_account"], json!(2));
        let a = item(&list, "claude-code", "a");
        assert_eq!(a["max_runs"], json!(null));
        assert_eq!(a["effective_max_runs"], json!(2));
        assert_eq!(a["max_runs_source"], json!("default"));
        assert_eq!(a["implicit"], json!(false));
        let qwen = item(&list, "implicit", "qwen");
        assert_eq!(qwen["implicit"], json!(true));
        assert_eq!(qwen["provider"], json!("qwen"));
        assert_eq!(qwen["dir"], json!(null));
        assert_eq!(qwen["effective_max_runs"], json!(2));
        assert_eq!(qwen["max_runs_source"], json!("default"));
        assert!(
            !list["items"]
                .as_array()
                .expect("items")
                .iter()
                .any(|v| v["adapter"] == "implicit" && v["id"] == "pool"),
            "pool rows have no implicit account: {list}"
        );
        // 実アカウントが先、暗黙アカウントが後ろ。
        let adapters: Vec<&str> = list["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|v| v["adapter"].as_str().expect("adapter"))
            .collect();
        assert_eq!(adapters.last(), Some(&"implicit"));
        assert_eq!(adapters.first(), Some(&"claude-code"));

        // 設定。
        let (status, body) = patch("claude-code/a".into(), json!({"max_runs": 3})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["max_runs"], json!(3));
        assert_eq!(body["effective_max_runs"], json!(3));
        assert_eq!(body["max_runs_source"], json!("account"));
        let (status, body) = patch("implicit/qwen".into(), json!({"max_runs": 2})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["implicit"], json!(true));
        assert_eq!(body["effective_max_runs"], json!(2));
        assert_eq!(body["max_runs_source"], json!("account"));
        // 静的な段と同名の id（`check` / `login`）も編集できる。
        for id in ["check", "login"] {
            let (status, body) = patch(format!("claude-code/{id}"), json!({"max_runs": 4})).await;
            assert_eq!(status, 200, "{id}: {body}");
            assert_eq!(body["id"], json!(id));
            assert_eq!(body["max_runs"], json!(4));
        }

        // 検証エラー（400）。
        for bad in [
            json!({"max_runs": 0}),
            json!({"max_runs": 33}),
            json!({"max_runs": -1}),
            json!({"max_runs": 1.5}),
            json!({"max_runs": "3"}),
            json!({}),
            json!({"max_runs": 2, "extra": true}),
            json!([2]),
        ] {
            let (status, body) = patch("claude-code/a".into(), bad.clone()).await;
            assert_eq!(status, 400, "{bad}: {body}");
            assert_eq!(body["code"], json!("bad_request"), "{bad}");
        }
        let (status, body) = patch("bogus/a".into(), json!({"max_runs": 2})).await;
        assert_eq!((status, body["code"].clone()), (400, json!("bad_request")));
        let (status, body) = patch("claude-code/bad.id".into(), json!({"max_runs": 2})).await;
        assert_eq!((status, body["code"].clone()), (400, json!("bad_request")));

        // 存在しないアカウント（404）: 実アカウントのディレクトリが無い、非プール行が無い、プール行の id。
        for path in ["claude-code/missing", "implicit/nope", "implicit/pool"] {
            let (status, body) = patch(path.into(), json!({"max_runs": 2})).await;
            assert_eq!(status, 404, "{path}: {body}");
            assert_eq!(body["code"], json!("account_not_found"), "{path}");
        }
        // 根が設定されていないアダプタは 409。
        let (status, body) = patch("codex/a".into(), json!({"max_runs": 2})).await;
        assert_eq!(
            (status, body["code"].clone()),
            (409, json!("accounts_unavailable"))
        );
        // 管理系なので token が要る。
        let resp = client
            .patch(format!("{base}/accounts/claude-code/a"))
            .json(&json!({"max_runs": 2}))
            .send()
            .await
            .expect("patch");
        assert_eq!(resp.status().as_u16(), 401);

        // providers の concurrency は受け付けるが保存せず、暗黙アカウントの上限にも効かない。
        let resp = client
            .post(format!("{base}/providers"))
            .bearer_auth(TOKEN)
            .json(&json!({"id": "extra", "adapter": "fake", "concurrency": 5}))
            .send()
            .await
            .expect("create provider");
        assert_eq!(resp.status().as_u16(), 201);
        let file = std::fs::read_to_string(qwen_dir.join("providers.d/extra.toml"))
            .expect("providers.d file");
        assert!(!file.contains("concurrency"), "{file}");

        // reload を跨いで残る（DB が正本）。reload で足した非プール行は暗黙アカウントとして既定で出る。
        let resp = client
            .post(format!("{base}/reload"))
            .bearer_auth(TOKEN)
            .json(&json!({}))
            .send()
            .await
            .expect("reload");
        assert_eq!(resp.status().as_u16(), 200);
        let list = get().await;
        assert_eq!(
            item(&list, "claude-code", "a")["effective_max_runs"],
            json!(3)
        );
        assert_eq!(
            item(&list, "implicit", "qwen")["effective_max_runs"],
            json!(2)
        );
        let extra = item(&list, "implicit", "extra");
        assert_eq!(extra["effective_max_runs"], json!(2));
        assert_eq!(extra["max_runs_source"], json!("default"));

        // null で保存値を消して既定へ戻す。
        let (status, body) = patch("claude-code/a".into(), json!({"max_runs": null})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["max_runs"], json!(null));
        assert_eq!(body["effective_max_runs"], json!(2));
        assert_eq!(body["max_runs_source"], json!("default"));
        let (status, body) = patch("implicit/qwen".into(), json!({"max_runs": null})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["max_runs_source"], json!("default"));
        let (status, _) = patch("implicit/qwen".into(), json!({"max_runs": 3})).await;
        assert_eq!(status, 200);
    };
    tokio::select! {
        () = requests => {}
        () = admin_loop(&mut dispatcher, &mut config, admin_rx) => panic!("admin loop ended"),
    }
    let _ = stop.send(());
    server.await.expect("join").expect("serve");

    // ディスパッチャの表（次の tick から効く値）と DB が一致し、再起動（同じ DB で作り直す）でも残る。
    assert_eq!(dispatcher.account_max_runs("claude-code", "a"), None);
    assert_eq!(dispatcher.account_max_runs("claude-code", "check"), Some(4));
    assert_eq!(
        dispatcher.implicit_account_limit("qwen"),
        Some((3, task_dispatch::capacity::MaxRunsSource::Account))
    );
    assert_eq!(
        dispatcher.implicit_account_limit("extra"),
        Some((2, task_dispatch::capacity::MaxRunsSource::Default)),
        "the deprecated concurrency of POST /providers must not reach dispatch"
    );
    drop(dispatcher);
    let restarted = build_dispatcher(&config, Default::default()).expect("restart");
    assert_eq!(restarted.account_max_runs("implicit", "qwen"), Some(3));
    assert_eq!(restarted.account_max_runs("claude-code", "login"), Some(4));
    let store = SqliteStore::open(&config.db.path).expect("store");
    assert_eq!(store.account_limits().expect("limits").len(), 3);
}
