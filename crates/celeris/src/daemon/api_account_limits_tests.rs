//! ADR 2026-10-08 account-concurrency-limits §(d)(e): `PATCH /api/v1/llm/sources/{source_id}/accounts/{account_id}` と
//! `GET /api/v1/llm/accounts` を、本物の API（`api_settings` → `ApiState`）・本物の管理要求の処理（`handle_admin_request`）・
//! 本物の `Dispatcher` で通す。

use std::path::Path;

use serde_json::{Value, json};
use task_core::{InstanceRole, SqliteStore, TaskStore};

use super::*;
use crate::daemon::admin::handle_admin_request;
use crate::{accounts_admin, build_dispatcher};

const TOKEN: &str = "account-limits-token";

/// account pool を持つ source（`claude_oauth`）と持たない source（`qwen`、fake）を宣言する。`sources` は
/// 追加の `[llm_sources.<id>]`。
fn write_config(dir: &Path, sources: &str) -> PathBuf {
    let path = dir.join("config.toml");
    let db = dir.join("celeris.db");
    let ws = dir.join("ws");
    let accounts = dir.join("claude-accounts");
    std::fs::write(
        &path,
        format!(
            "db = {db:?}\nworkspace_root = {ws:?}\n\
             [accounts]\nclaude_dir = {accounts:?}\nmax_runs_per_account = 2\n\
             [llm_sources.claude_oauth]\nkind = \"claude_oauth\"\napi_format = \"anthropic\"\nmodels = [\"claude-sonnet-5\"]\n\
             [llm_sources.qwen]\nkind = \"fake\"\nmodels = [\"fake\"]\n{sources}"
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

fn item<'a>(list: &'a Value, source: &str, account: &str) -> &'a Value {
    list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|v| v["source_id"] == source && v["account_id"] == account)
        .unwrap_or_else(|| panic!("{source}/{account} not in {list}"))
}

/// 本物の API（`api_settings` → `ApiState` → `serve_with_listener`）を起動し、`(base, client, stop, server)`
/// を返す。スナップショットは `dispatcher` から publish される（呼び出し側が tick する）。
async fn spawn_api(
    config: &Config,
    dispatcher: &mut Dispatcher,
    admin_tx: tokio::sync::mpsc::Sender<task_api::AdminRequest>,
) -> (
    String,
    reqwest::Client,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), task_api::ApiError>>,
) {
    let (snap_tx, snap_rx) = tokio::sync::watch::channel(None);
    dispatcher.set_snapshot_publisher(SnapshotPublisher {
        tx: snap_tx,
        instance_id: "i".into(),
        hostname: "h".into(),
        started_at: "t".into(),
        tick_ms: config.tick_ms,
    });
    dispatcher.tick().expect("first tick");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let settings = api_settings(
        config,
        addr,
        vec![task_api::ApiToken::new(
            task_api::Principal::Human {
                name: "test".into(),
            },
            TOKEN,
        )],
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
    (
        format!("http://{addr}/api/v1"),
        reqwest::Client::new(),
        stop,
        server,
    )
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
    let (admin_tx, admin_rx) = tokio::sync::mpsc::channel(8);
    let (base, client, stop, server) = spawn_api(&config, &mut dispatcher, admin_tx).await;

    let requests = async {
        let get = || async {
            let resp = client
                .get(format!("{base}/llm/accounts"))
                .bearer_auth(TOKEN)
                .send()
                .await
                .expect("get");
            assert_eq!(resp.status().as_u16(), 200);
            resp.json::<Value>().await.expect("json")
        };
        // `path` は `<source_id>/<account_id>`。
        let patch = |path: String, body: Value| {
            let client = client.clone();
            let (source, account) = path.split_once('/').expect("source/account");
            let url = format!("{base}/llm/sources/{source}/accounts/{account}");
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

        // GET: 実アカウントの既定、暗黙アカウント（既定）。account pool を持つ source には暗黙アカウントが無い。
        let list = get().await;
        assert_eq!(list["max_runs_per_account"], json!(2));
        let a = item(&list, "claude_oauth", "a");
        assert_eq!(a["max_runs"], json!(null));
        assert_eq!(a["effective_max_runs"], json!(2));
        assert_eq!(a["max_runs_source"], json!("default"));
        assert_eq!(a["implicit"], json!(false));
        let qwen = item(&list, "qwen", "qwen");
        assert_eq!(qwen["implicit"], json!(true));
        assert_eq!(qwen["dir"], json!(null));
        assert_eq!(qwen["effective_max_runs"], json!(2));
        assert_eq!(qwen["max_runs_source"], json!("default"));
        assert!(
            !list["items"]
                .as_array()
                .expect("items")
                .iter()
                .any(|v| v["implicit"] == true && v["source_id"] == "claude_oauth"),
            "pool sources have no implicit account: {list}"
        );
        // 実アカウントが先、暗黙アカウントが後ろ。
        let items = list["items"].as_array().expect("items");
        assert_eq!(
            items.first().expect("first")["source_id"],
            json!("claude_oauth")
        );
        assert_eq!(items.last().expect("last")["implicit"], json!(true));

        // 設定。
        let (status, body) = patch("claude_oauth/a".into(), json!({"max_runs": 3})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["account_id"], json!("a"));
        assert_eq!(body["max_runs"], json!(3));
        assert_eq!(body["effective_max_runs"], json!(3));
        assert_eq!(body["max_runs_source"], json!("account"));
        let (status, body) = patch("qwen/qwen".into(), json!({"max_runs": 2})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["implicit"], json!(true));
        assert_eq!(body["effective_max_runs"], json!(2));
        assert_eq!(body["max_runs_source"], json!("account"));
        // 静的な段と同名の id（`check` / `login`）も編集できる。
        for id in ["check", "login"] {
            let (status, body) = patch(format!("claude_oauth/{id}"), json!({"max_runs": 4})).await;
            assert_eq!(status, 200, "{id}: {body}");
            assert_eq!(body["account_id"], json!(id));
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
            let (status, body) = patch("claude_oauth/a".into(), bad.clone()).await;
            assert_eq!(status, 400, "{bad}: {body}");
            assert_eq!(body["code"], json!("bad_request"), "{bad}");
        }
        let (status, body) = patch("claude_oauth/bad.id".into(), json!({"max_runs": 2})).await;
        assert_eq!((status, body["code"].clone()), (400, json!("bad_request")));

        // 存在しないアカウント・source（404）: 実アカウントのディレクトリが無い、宣言の無い source、
        // pool の source に対応する暗黙アカウントは無い、暗黙アカウントで account_id が source_id と違う、
        // 根が設定されていない pool の source（`codex_oauth` は `[accounts]` に無い）。
        for path in [
            "claude_oauth/missing",
            "qwen/nope",
            "claude_oauth/claude_oauth",
            "bogus/a",
            "codex_oauth/a",
        ] {
            let (status, body) = patch(path.into(), json!({"max_runs": 2})).await;
            assert_eq!(status, 404, "{path}: {body}");
            assert_eq!(body["code"], json!("account_not_found"), "{path}");
        }
        // 管理系なので token が要る。
        let resp = client
            .patch(format!("{base}/llm/sources/claude_oauth/accounts/a"))
            .json(&json!({"max_runs": 2}))
            .send()
            .await
            .expect("patch");
        assert_eq!(resp.status().as_u16(), 401);

        // reload を跨いで残る（DB が正本）。
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
            item(&list, "claude_oauth", "a")["effective_max_runs"],
            json!(3)
        );
        let shared = item(&list, "qwen", "qwen");
        assert_eq!(shared["effective_max_runs"], json!(2));
        assert_eq!(shared["max_runs_source"], json!("account"));

        // null で保存値を消して既定へ戻す。
        let (status, body) = patch("claude_oauth/a".into(), json!({"max_runs": null})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["max_runs"], json!(null));
        assert_eq!(body["effective_max_runs"], json!(2));
        assert_eq!(body["max_runs_source"], json!("default"));
        let (status, body) = patch("qwen/qwen".into(), json!({"max_runs": null})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["max_runs_source"], json!("default"));
        let (status, _) = patch("qwen/qwen".into(), json!({"max_runs": 3})).await;
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
    drop(dispatcher);
    let restarted = build_dispatcher(&config, Default::default()).expect("restart");
    assert_eq!(restarted.account_max_runs("implicit", "qwen"), Some(3));
    assert_eq!(restarted.account_max_runs("claude-code", "login"), Some(4));
    let store = SqliteStore::open(&config.db.path).expect("store");
    assert_eq!(store.account_limits().expect("limits").len(), 3);
}

/// ADR 2026-10-08 account-concurrency-limits §(d)(e)（人の決定 implicit-account）:
/// `PATCH /api/v1/llm/sources/<llm source id>/accounts/<llm source id>` は source 単位の暗黙アカウントを保存し、
/// `POST /reload` を跨いで残る。account pool を持たない source ごとに 1 件で、source 同士は上限を共有しない。
#[tokio::test]
async fn implicit_source_account_limit_survives_reload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = write_config(
        dir.path(),
        "[llm_sources.qwen2]\nkind = \"fake\"\nmodels = [\"fake\"]\n",
    );
    let mut config = Config::load(&config_path).expect("config");
    let mut dispatcher = build_dispatcher(&config, Default::default()).expect("dispatcher");
    let (admin_tx, admin_rx) = tokio::sync::mpsc::channel(8);
    let (base, client, stop, server) = spawn_api(&config, &mut dispatcher, admin_tx).await;
    let db = config.db.path.clone();

    let requests = async {
        let resp = client
            .patch(format!("{base}/llm/sources/qwen/accounts/qwen"))
            .bearer_auth(TOKEN)
            .json(&json!({"max_runs": 1}))
            .send()
            .await
            .expect("patch");
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.json::<Value>().await.expect("json");
        assert_eq!(body["source_id"], json!("qwen"));
        assert_eq!(body["account_id"], json!("qwen"));
        assert_eq!(body["implicit"], json!(true));
        assert_eq!(body["max_runs"], json!(1));
        assert_eq!(body["effective_max_runs"], json!(1));
        assert_eq!(body["max_runs_source"], json!("account"));

        let resp = client
            .post(format!("{base}/reload"))
            .bearer_auth(TOKEN)
            .json(&json!({}))
            .send()
            .await
            .expect("reload");
        assert_eq!(resp.status().as_u16(), 200);

        let resp = client
            .get(format!("{base}/llm/accounts"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .expect("get");
        let list = resp.json::<Value>().await.expect("json");
        let after = item(&list, "qwen", "qwen");
        assert_eq!(
            after["max_runs"],
            json!(1),
            "reload must keep the source limit"
        );
        assert_eq!(after["effective_max_runs"], json!(1));
        assert_eq!(after["max_runs_source"], json!("account"));
        // 別の source（`qwen2`）は自分の暗黙アカウントを持ち、`qwen` の上限を共有しない。
        let other = item(&list, "qwen2", "qwen2");
        assert_eq!(other["effective_max_runs"], json!(2));
        assert_eq!(other["max_runs_source"], json!("default"));
    };
    tokio::select! {
        () = requests => {}
        () = admin_loop(&mut dispatcher, &mut config, admin_rx) => panic!("admin loop ended"),
    }
    let _ = stop.send(());
    server.await.expect("join").expect("serve");

    // reload 後のディスパッチャの表: `qwen` だけが保存値で止まる。
    assert_eq!(
        dispatcher.implicit_account_limit("qwen"),
        Some((1, task_dispatch::capacity::MaxRunsSource::Account))
    );
    assert_eq!(
        dispatcher.implicit_account_ids(),
        vec!["qwen".to_string(), "qwen2".to_string()]
    );
    drop(dispatcher);
    // 再起動（同じ DB で作り直す）でも残る。
    let restarted = build_dispatcher(&config, Default::default()).expect("restart");
    assert_eq!(restarted.account_max_runs("implicit", "qwen"), Some(1));
    let store = SqliteStore::open(&db).expect("store");
    let limits = store.account_limits().expect("limits");
    assert_eq!(limits.len(), 1, "{limits:?}");
    assert_eq!(limits[0].source_id, "implicit");
    assert_eq!(limits[0].account_id, "qwen");
}
