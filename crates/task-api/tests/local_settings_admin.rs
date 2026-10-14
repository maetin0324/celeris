//! ADR 2026-10-14-local-machine-settings D4/D5: `GET /clusters` の `local`（ローカルマシンの行）と
//! `PUT /clusters/local/settings`（store の上書きの全置換）。
//!
//! host の path（`$HOME`・DB・設定ファイル・releases）は試験ごとの tempdir（`Host`）に作り、保護 path は
//! `ProtectedPaths::for_daemon` で daemon と同じ規則で組む。実 `$HOME` と本番の path には触らない。

mod common;

use std::path::{Path, PathBuf};

use common::cos_ops::{db, run_domain};
use common::*;
use serde_json::{Value, json};
use task_core::{LocalSettingsOverrides, TaskStore};

const LOCAL: &str = "/api/v1/clusters/local/settings";

/// 試験用の host の配置（`$HOME`・DB・設定ファイル・releases）。
struct Host {
    dir: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("host tempdir");
        for sub in ["home/.config/celeris", "state", "releases", "data"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("host dir");
        }
        std::fs::write(dir.path().join("state/celeris.db"), b"").expect("db file");
        Self { dir }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn home(&self) -> PathBuf {
        self.path("home")
    }

    fn settings(&self) -> task_api::LocalMachineSettings {
        task_api::LocalMachineSettings {
            scratch_root: Some(self.path("scratch")),
            home: Some(self.home()),
            protected: task_core::local_machine::ProtectedPaths::for_daemon(
                Some(&self.home()),
                &self.path("state/celeris.db"),
                Some(&self.path("home/.config/celeris/config.toml")),
                Some(&self.path("releases")),
            ),
        }
    }
}

fn env_for(host: &Host) -> (TestEnv, tokio::sync::mpsc::Receiver<task_api::AdminRequest>) {
    let (admin_tx, admin_rx) = tokio::sync::mpsc::channel(8);
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        admin_tx: Some(admin_tx),
        local_machine: Some(host.settings()),
        ..EnvOptions::default()
    });
    (env, admin_rx)
}

async fn local_row(env: &TestEnv) -> Value {
    let resp = send(&env.router(), get_admin("/api/v1/clusters")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert!(body["items"].is_array(), "{body}");
    body["local"].clone()
}

async fn put_local(env: &TestEnv, body: Value) -> common::Resp {
    send(&env.router(), put_json_with(LOCAL, &body, &admin_headers())).await
}

fn path_str(path: &Path) -> String {
    path.display().to_string()
}

fn changed_count(rx: &mut tokio::sync::mpsc::Receiver<task_api::AdminRequest>) -> usize {
    let mut n = 0;
    while let Ok(request) = rx.try_recv() {
        assert!(
            matches!(request, task_api::AdminRequest::LocalSettingsChanged),
            "unexpected admin request"
        );
        n += 1;
    }
    n
}

#[tokio::test]
async fn local_settings_get_shows_local_row() {
    let host = Host::new();
    let (env, _rx) = env_for(&host);
    let config_root = path_str(&env.workspace_root);

    // スナップショットが無い間: in_use は null、設定ファイルの値がそのまま出る。
    let local = local_row(&env).await;
    assert_eq!(local["id"], "local", "{local}");
    assert!(local["in_use"].is_null(), "{local}");
    assert_eq!(
        local["workspace_root"],
        json!({"value": config_root, "source": "config", "config_value": config_root, "apply": "new_tasks"}),
    );
    assert_eq!(
        local["max_concurrency"],
        json!({"value": 4, "source": "config", "config_value": 4, "apply": "next_tick"}),
    );
    assert_eq!(local["scratch_root"], path_str(&host.path("scratch")));
    assert!(local["updated_at"].is_null(), "{local}");

    // スナップショットが来たら hostname と走行中の数はそこから。
    let snap = snapshot(3);
    let running = snap.in_flight.len();
    env.daemon_tx.send(Some(snap)).expect("snapshot");
    let local = local_row(&env).await;
    assert_eq!(local["hostname"], "lab-01", "{local}");
    assert_eq!(local["in_use"], json!(running), "{local}");
    // リモートの行（items）には混ぜない。
    let resp = send(&env.router(), get_admin("/api/v1/clusters")).await;
    let body = resp.json();
    assert!(
        body["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|c| c["id"] != "local"),
        "{body}"
    );
}

#[tokio::test]
async fn local_settings_put_validation_errors() {
    let host = Host::new();
    let (env, mut rx) = env_for(&host);
    let link = host.path("data/link-to-state");
    std::os::unix::fs::symlink(host.path("state"), &link).expect("symlink");

    // 管理系: token 無しは 401。
    let resp = send(
        &env.router(),
        put_json_with(LOCAL, &json!({"max_concurrency": 2}), &[]),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401, "{}", resp.text());

    let bad_roots = [
        ("relative", "relative/path".to_string(), "absolute"),
        ("empty", "  ".to_string(), "empty"),
        ("dotdot", path_str(&host.path("data/../data/ws")), ".."),
        ("root", "/".to_string(), "protected"),
        ("home", "~".to_string(), "protected"),
        ("home-abs", path_str(&host.home()), "protected"),
        ("db-dir", path_str(&host.path("state/ws")), "protected"),
        (
            "db-file",
            path_str(&host.path("state/celeris.db")),
            "protected",
        ),
        ("releases", path_str(&host.path("releases/ws")), "protected"),
        (
            "config-dir",
            "~/.config/celeris/ws".to_string(),
            "protected",
        ),
        ("contains-protected", path_str(host.dir.path()), "protected"),
        ("symlink", path_str(&link.join("ws")), "protected"),
        (
            "not-a-dir",
            path_str(&host.path("state/celeris.db/ws")),
            "protected",
        ),
    ];
    for (case, root, needle) in bad_roots {
        let resp = put_local(&env, json!({"workspace_root": root})).await;
        let problem = assert_problem(&resp, 422, "validation");
        let errors = problem["errors"].as_array().expect("errors");
        assert_eq!(errors.len(), 1, "{case}: {problem}");
        assert_eq!(errors[0]["field"], "workspace_root", "{case}: {problem}");
        let message = errors[0]["message"].as_str().expect("message");
        assert!(message.contains(needle), "{case}: {message}");
    }
    // 保護 path の中には dir を作らない（検証は作る前）。
    assert!(!host.path("state/ws").exists());
    assert!(!host.path("home/.config/celeris/ws").exists());

    // 書き込めない場所（作れない）は 422。root で走っている環境では権限で拒めないので確かめない。
    let ro = host.path("data/ro");
    std::fs::create_dir_all(&ro).expect("ro dir");
    std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o555))
        .expect("chmod");
    if std::fs::write(ro.join("probe"), b"").is_err() {
        let resp = put_local(&env, json!({"workspace_root": path_str(&ro.join("ws"))})).await;
        let problem = assert_problem(&resp, 422, "validation");
        assert_eq!(problem["errors"][0]["field"], "workspace_root", "{problem}");
    }

    for n in [0, 257, -1] {
        let resp = put_local(&env, json!({"max_concurrency": n})).await;
        let problem = assert_problem(&resp, 422, "validation");
        assert_eq!(
            problem["errors"][0]["field"], "max_concurrency",
            "{n}: {problem}"
        );
    }

    // 両方の欄が悪ければ 2 件まとめて返す。
    let resp = put_local(&env, json!({"workspace_root": "rel", "max_concurrency": 0})).await;
    let problem = assert_problem(&resp, 422, "validation");
    let fields: Vec<&str> = problem["errors"]
        .as_array()
        .expect("errors")
        .iter()
        .filter_map(|e| e["field"].as_str())
        .collect();
    assert_eq!(fields, ["max_concurrency", "workspace_root"], "{problem}");

    // 知らない欄は受けない（全置換の本文を取り違えない）。
    let resp = put_local(&env, json!({"scratch_root": "/x"})).await;
    assert!(resp.status.is_client_error(), "{}", resp.text());

    // 拒否はどれも store にも監査にも書かず、dispatcher にも知らせない。
    assert!(env.store.local_settings_get().expect("get").is_none());
    assert!(
        env.store
            .local_settings_events()
            .expect("events")
            .is_empty()
    );
    assert_eq!(changed_count(&mut rx), 0);
}

#[tokio::test]
async fn local_settings_put_audit_and_apply() {
    let host = Host::new();
    let (env, mut rx) = env_for(&host);
    let config_root = path_str(&env.workspace_root);
    let new_root = host.home().join("ws");

    // 1. 管理 API: `~` を展開して保存し、dir を作り、監査 1 行、dispatcher へ知らせる。
    let resp = put_local(
        &env,
        json!({"workspace_root": "~/ws", "max_concurrency": 8}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let view = resp.json();
    assert_eq!(
        view["workspace_root"],
        json!({"value": path_str(&new_root), "source": "settings", "config_value": config_root, "apply": "new_tasks"}),
    );
    assert_eq!(
        view["max_concurrency"],
        json!({"value": 8, "source": "settings", "config_value": 4, "apply": "next_tick"}),
    );
    assert!(view["updated_at"].is_string(), "{view}");
    assert!(new_root.is_dir());
    let stored = env.store.local_settings_get().expect("get").expect("row");
    assert_eq!(
        stored.workspace_root.as_deref(),
        Some(path_str(&new_root).as_str())
    );
    assert_eq!(stored.max_concurrency, Some(8));
    let events = env.store.local_settings_events().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor, "admin");
    assert_eq!(changed_count(&mut rx), 1);

    // 2. 全置換: 省略した欄は上書きを消す（設定ファイルの値に戻る）。
    let resp = put_local(&env, json!({"max_concurrency": 2})).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let view = resp.json();
    assert_eq!(view["workspace_root"]["source"], "config", "{view}");
    assert_eq!(view["workspace_root"]["value"], config_root, "{view}");
    assert_eq!(view["max_concurrency"]["value"], 2, "{view}");
    // null で全欄を消すと行ごと消える。
    let resp = put_local(
        &env,
        json!({"workspace_root": null, "max_concurrency": null}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let view = resp.json();
    assert_eq!(view["max_concurrency"]["source"], "config", "{view}");
    assert!(view["updated_at"].is_null(), "{view}");
    assert!(env.store.local_settings_get().expect("get").is_none());
    assert_eq!(env.store.local_settings_events().expect("events").len(), 3);
    assert_eq!(changed_count(&mut rx), 2);

    // 3. CoS: `cluster.settings_put` と同じ OperationAudit の経路（直の CoS 呼び出しは 422、
    // 封筒経由で applied・監査 event・chat card）。store の監査は `cos:<operation id>`。
    let op = run_domain(
        &env,
        "local-settings",
        "PUT",
        LOCAL,
        json!({"max_concurrency": 3}),
        "local.settings_put",
    )
    .await;
    let stored = env.store.local_settings_get().expect("get").expect("row");
    assert_eq!(stored.max_concurrency, Some(3));
    assert_eq!(stored.workspace_root, None);
    let events = env.store.local_settings_events().expect("events");
    assert_eq!(events.len(), 4);
    let op_id = op["id"].as_str().expect("operation id");
    assert_eq!(events[3].actor, format!("cos:{op_id}"));
    let target: (String, String) = db(&env)
        .query_row(
            "SELECT target_kind, target_id FROM cos_operations WHERE id = ?1",
            [op_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("cos operation row");
    assert_eq!(target, ("local".to_string(), "local".to_string()));
    assert_eq!(changed_count(&mut rx), 1);

    // CoS の検証失敗も 422（store は変わらない）。
    let resp = send(
        &env.router(),
        post_json_with(
            common::cos_ops::OPS,
            &common::cos_ops::op_body("local-bad", "PUT", LOCAL, json!({"max_concurrency": 999})),
            &[(
                "authorization",
                common::cos_ops::cos_bearer(&env, "local-bad").2.as_str(),
            )],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    assert_eq!(env.store.local_settings_events().expect("events").len(), 4);
}

#[tokio::test]
async fn local_settings_store_beats_config_in_response() {
    let host = Host::new();
    let (env, _rx) = env_for(&host);
    let config_root = path_str(&env.workspace_root);
    let now = time::OffsetDateTime::now_utc();

    // store の上書き（daemon・CoS など API 以外の書き手を含む）は設定ファイルに勝つ。欄ごとに解く。
    env.store
        .local_settings_set(
            &LocalSettingsOverrides {
                workspace_root: Some("/srv/store-root".into()),
                max_concurrency: None,
            },
            "admin",
            now,
        )
        .expect("set");
    let local = local_row(&env).await;
    assert_eq!(
        local["workspace_root"],
        json!({"value": "/srv/store-root", "source": "settings", "config_value": config_root, "apply": "new_tasks"}),
    );
    assert_eq!(local["max_concurrency"]["source"], "config", "{local}");
    assert_eq!(local["max_concurrency"]["value"], 4, "{local}");
    assert!(local["updated_at"].is_string(), "{local}");

    env.store
        .local_settings_set(
            &LocalSettingsOverrides {
                workspace_root: None,
                max_concurrency: Some(12),
            },
            "admin",
            now,
        )
        .expect("set");
    let local = local_row(&env).await;
    assert_eq!(local["workspace_root"]["source"], "config", "{local}");
    assert_eq!(local["workspace_root"]["value"], config_root, "{local}");
    assert_eq!(
        local["max_concurrency"],
        json!({"value": 12, "source": "settings", "config_value": 4, "apply": "next_tick"}),
    );
}
