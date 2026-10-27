//! ADR 2026-10-14-local-machine-settings D5: 足回り（`<root>/<task_id>`）の root は `WorkspaceRoots` 1 本で解き、
//! store の `workspace_root` を変えても既存 task・走行中 run の場所は動かず、新しい task だけが store の root に
//! 作られることを、daemon の config から配線した `ApiState`（loopback の HTTP）と tempdir の実 file で確かめる。
//!
//! 流れ: `<home>/config.toml`（`workspace_root = "ws"`）を読む → store と同じ DB を開く → task の足回り
//! （`worktree.json`・`runs/<run_id>/stdout.jsonl`・`artifacts/`）を tempdir に作る → `local_settings` の
//! `workspace_root` を書き換える → `GET /tasks/{id}`・`/runs/{run_id}/stdout`・`/artifacts` で同じ場所から
//! 読めること、`task_ops::workspace::{local_dir, prepare_task_dir}`（dispatcher が足回りを用意する経路）が
//! 返す場所を検査する。外部ネットワーク・userns・本番の `~/.local/celeris` には触れない。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use celeris::Config;
use serde_json::Value;
use task_core::local_machine::WorkspaceRoots;
use task_core::{
    ArtifactRef, Check, Criterion, DaemonMode, Event, InstanceRole, Lease, LocalSettingsOverrides,
    SharedRole, SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, WorkspaceSpec,
};
use task_ops::workspace::{WORKTREE_MARKER, WorktreeMarker, local_dir, prepare_task_dir};
use time::OffsetDateTime;

const TOKEN: &str = "workspace-roots-token";
/// `runs/<run_id>/` の名前（API は ULID でない run id を 403 にする）。
const RUN_ID: &str = "01M4X4AKT8HWQ9J50TK6SBVB4J";

struct Daemon {
    dir: tempfile::TempDir,
    store: SqliteStore,
    roots: WorkspaceRoots,
    base: String,
    /// 落とすと `watch` が閉じるので、テストの間は持ち続ける。
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
}

impl Daemon {
    /// `<home>/config.toml` を読み、その設定から `ApiState` を作って loopback に立てる
    /// （`integration_api.rs` の fixture と同じ組み立て）。
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let home = dir.path();
        std::fs::create_dir_all(home.join("ws")).unwrap_or_else(|e| panic!("ws: {e}"));
        std::fs::write(home.join("api.token"), format!("{TOKEN}\n"))
            .unwrap_or_else(|e| panic!("token: {e}"));
        std::fs::write(
            home.join("config.toml"),
            r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200

[api]
listen = "127.0.0.1:0"

[[api.tokens]]
principal = "operator:test"
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
candidates = [{ source = "local", model = "fake" }]

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "fake", required_capabilities = [] }]
"#,
        )
        .unwrap_or_else(|e| panic!("config: {e}"));
        let config =
            Config::load(&home.join("config.toml")).unwrap_or_else(|e| panic!("load: {e}"));
        let store = SqliteStore::open(&config.db.path).unwrap_or_else(|e| panic!("open: {e}"));
        let roots = config.workspace_roots();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("bind: {e}"));
        let addr: SocketAddr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("addr: {e}"));
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
            "2026-10-14T00:00:00Z".to_string(),
            None,
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
            dir,
            store,
            roots,
            base: format!("http://{addr}"),
            _daemon_tx: daemon_tx,
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    /// 設定ファイルの root（`<home>/ws`）。
    fn config_root(&self) -> PathBuf {
        self.roots.config_root().to_path_buf()
    }

    /// 管理 API（`PUT /clusters/local/settings`）と同じ store の上書きを書く。
    fn set_store_root(&self, root: &Path) {
        self.store
            .local_settings_set(
                &LocalSettingsOverrides {
                    workspace_root: Some(root.to_string_lossy().into_owned()),
                    max_concurrency: None,
                },
                "admin",
                now(),
            )
            .unwrap_or_else(|e| panic!("local_settings_set: {e}"));
    }

    fn pinned(&self, id: TaskId) -> Option<String> {
        self.store
            .task_workspace_root_get(&id)
            .unwrap_or_else(|e| panic!("pin get: {e}"))
    }

    async fn get_json(&self, path: &str) -> (u16, Value) {
        let (status, text) = self.get_text(path).await;
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn get_text(&self, path: &str) -> (u16, String) {
        let resp = reqwest::Client::new()
            .get(format!("{}{path}", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("get {path}: {e}"));
        let status = resp.status().as_u16();
        (status, resp.text().await.unwrap_or_default())
    }

    /// API から見た足回り（`TaskDetail.workspace_dir` と `worktree.dir`）。
    async fn detail_dirs(&self, id: TaskId) -> (String, String) {
        let (status, body) = self.get_json(&format!("/api/v1/tasks/{id}")).await;
        assert_eq!(status, 200, "detail {id}: {body}");
        (
            body["workspace_dir"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            body["worktree"]["dir"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    }

    async fn stdout(&self, id: TaskId) -> String {
        let (status, text) = self
            .get_text(&format!("/api/v1/tasks/{id}/runs/{RUN_ID}/stdout"))
            .await;
        assert_eq!(status, 200, "stdout {id}: {text}");
        text
    }

    async fn artifact(&self, id: TaskId) -> String {
        let (status, list) = self
            .get_json(&format!("/api/v1/tasks/{id}/artifacts"))
            .await;
        assert_eq!(status, 200, "artifacts {id}: {list}");
        assert_eq!(list["items"][0]["exists"], Value::Bool(true), "{list}");
        let (status, body) = self
            .get_text(&format!("/api/v1/tasks/{id}/artifacts/0"))
            .await;
        assert_eq!(status, 200, "artifact body {id}: {body}");
        body
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(Duration::from_secs(60), handle)
                .await
                .expect("fixture server stopped")
                .expect("fixture server task")
                .expect("fixture server result");
        }
    }
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock")
}

/// worktree を切った Local task（`worktree.json` があれば足回りは `<root>/<task_id>`）。
fn local_task(id: TaskId, parent: Option<TaskId>, status: Status) -> Task {
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: parent,
        kind: TaskKind::Execute,
        title: "workspace roots fixture".into(),
        objective: "exercise workspace root resolution".into(),
        acceptance: vec![Criterion {
            text: "fixture exists".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: Vec::new(),
        depends_on: Vec::new(),
        status,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: "repo".into(),
            mode: None,
        },
        budget: task_core::Budget {
            max_turns: 1,
            max_wall_secs: 60,
            max_retries: 0,
            max_tokens: None,
        },
        attempts: 0,
        lease: (status == Status::Running).then(|| Lease {
            worker_run_id: RUN_ID.into(),
            expires_at: now() + time::Duration::hours(1),
        }),
        created_at: now(),
        updated_at: now(),
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// 足回り `<task_dir>` に `worktree.json`・`runs/<RUN_ID>/stdout.jsonl`・`artifacts/report.md` を作り、
/// 成果物の event を追記する（worker の run が残すものと同じ配置）。
fn materialize(store: &SqliteStore, task: &Task, task_dir: &Path, label: &str) {
    let tree = task_dir.join("tree");
    std::fs::create_dir_all(&tree).expect("tree");
    std::fs::create_dir_all(task_dir.join("runs").join(RUN_ID)).expect("runs");
    std::fs::create_dir_all(task_dir.join("artifacts")).expect("artifacts");
    task_ops::workspace::write_marker(
        task_dir,
        &WorktreeMarker {
            repo: "/srv/repo".into(),
            dir: tree.to_string_lossy().into_owned(),
            branch: format!("celeris/{}", task.id),
            base: "0".repeat(40),
            base_kind: "main".into(),
            repos: Vec::new(),
        },
    )
    .expect("marker");
    std::fs::write(
        task_dir.join("runs").join(RUN_ID).join("stdout.jsonl"),
        format!("{{\"line\":\"{label}-1\"}}\n"),
    )
    .expect("stdout");
    std::fs::write(task_dir.join("artifacts").join("report.md"), label).expect("artifact");
    store
        .append_event(
            task.id,
            &Event::ArtifactProduced {
                run_id: RUN_ID.into(),
                artifact: ArtifactRef {
                    name: "report.md".into(),
                    path: "artifacts/report.md".into(),
                    sha256: String::new(),
                    kind: "file".into(),
                    declared: true,
                },
            },
        )
        .expect("artifact event");
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 既存 task（固定済み・固定なし）の `artifacts`・`worktree.json`・`runs/` は、store の root を変えた後も
/// 同じ場所から読める。固定の無い task は設定ファイルの root のまま（store の root に移らない）。
#[tokio::test]
async fn existing_workspace_unchanged_after_root_change() {
    let daemon = Daemon::start().await;
    let (first, moved) = (daemon.home().join("first"), daemon.home().join("moved"));
    let config_root = daemon.config_root();

    // この ADR 以前の task: 固定なし、足回りは設定ファイルの root。
    let legacy = local_task(TaskId::new(), None, Status::Done);
    daemon.store.insert(&legacy).expect("insert legacy");
    let legacy_dir = config_root.join(legacy.id.to_string());
    materialize(&daemon.store, &legacy, &legacy_dir, "legacy");

    // store の root が first のときに足回りを用意した task（first に固定）。
    daemon.set_store_root(&first);
    let pinned = local_task(TaskId::new(), None, Status::Done);
    daemon.store.insert(&pinned).expect("insert pinned");
    let pinned_dir =
        prepare_task_dir(&daemon.store, &daemon.roots, &pinned, now()).expect("prepare pinned");
    assert_eq!(pinned_dir, first.join(pinned.id.to_string()));
    materialize(&daemon.store, &pinned, &pinned_dir, "pinned");

    let mut before = Vec::new();
    for id in [legacy.id, pinned.id] {
        before.push((
            daemon.detail_dirs(id).await,
            daemon.stdout(id).await,
            daemon.artifact(id).await,
        ));
    }

    // root を変える。
    daemon.set_store_root(&moved);

    for ((id, dir, label), before) in [
        (legacy.id, &legacy_dir, "legacy"),
        (pinned.id, &pinned_dir, "pinned"),
    ]
    .into_iter()
    .zip(before)
    {
        let after = (
            daemon.detail_dirs(id).await,
            daemon.stdout(id).await,
            daemon.artifact(id).await,
        );
        assert_eq!(after, before, "{label}: API view moved after root change");
        assert_eq!(after.0.0, s(dir), "{label}: workspace_dir");
        assert_eq!(after.0.1, s(&dir.join("tree")), "{label}: worktree.json");
        assert_eq!(after.1, format!("{{\"line\":\"{label}-1\"}}\n"));
        assert_eq!(after.2, label);
        assert!(dir.join(WORKTREE_MARKER).is_file());
        assert!(
            !moved.join(id.to_string()).exists(),
            "{label}: copied to new root"
        );
    }
    // 固定なしの task は固定されず、設定ファイルの root から解かれる。
    assert_eq!(daemon.pinned(legacy.id), None);
    assert_eq!(
        local_dir(&daemon.store, &daemon.roots, &legacy).expect("legacy local_dir"),
        legacy_dir
    );
    assert_eq!(daemon.pinned(pinned.id), Some(s(&first)));
    daemon.shutdown().await;
}

/// 走行中 run の作業場所（cwd・`runs/<run_id>/`）は store の root を変えても動かない。再 dispatch で足回りを
/// 用意し直しても固定された root が返り、run が続けて書いた出力も同じ場所から読める。
#[tokio::test]
async fn running_run_workspace_stable_after_root_change() {
    let daemon = Daemon::start().await;
    let (first, moved) = (daemon.home().join("first"), daemon.home().join("moved"));
    let config_root = daemon.config_root();

    // dispatch で first に固定され、run が走っている task。
    daemon.set_store_root(&first);
    let running = local_task(TaskId::new(), None, Status::Running);
    daemon.store.insert(&running).expect("insert running");
    let run_dir =
        prepare_task_dir(&daemon.store, &daemon.roots, &running, now()).expect("prepare running");
    materialize(&daemon.store, &running, &run_dir, "running");
    // この ADR 以前に走り始めた run（固定なし、設定ファイルの root）。
    let legacy = local_task(TaskId::new(), None, Status::Running);
    daemon.store.insert(&legacy).expect("insert legacy");
    let legacy_dir = config_root.join(legacy.id.to_string());
    materialize(&daemon.store, &legacy, &legacy_dir, "legacy");

    daemon.set_store_root(&moved);

    for (task, dir, label) in [
        (&running, &run_dir, "running"),
        (&legacy, &legacy_dir, "legacy"),
    ] {
        // run は渡された作業場所に書き続ける。
        let stdout = dir.join("runs").join(RUN_ID).join("stdout.jsonl");
        let mut text = std::fs::read_to_string(&stdout).expect("read stdout");
        text.push_str(&format!("{{\"line\":\"{label}-2\"}}\n"));
        std::fs::write(&stdout, text).expect("append stdout");

        assert_eq!(
            local_dir(&daemon.store, &daemon.roots, task).expect("local_dir"),
            *dir,
            "{label}: local_dir moved"
        );
        assert_eq!(
            daemon.stdout(task.id).await,
            format!("{{\"line\":\"{label}-1\"}}\n{{\"line\":\"{label}-2\"}}\n"),
            "{label}: run output"
        );
        assert_eq!(daemon.detail_dirs(task.id).await.0, s(dir));
        assert!(!moved.join(task.id.to_string()).exists());
    }

    // 固定済みの task は再 dispatch で用意し直しても同じ root（固定は 1 回だけ）。
    assert_eq!(
        prepare_task_dir(&daemon.store, &daemon.roots, &running, now()).expect("re-prepare"),
        run_dir
    );
    assert_eq!(daemon.pinned(running.id), Some(s(&first)));
    assert_eq!(daemon.pinned(legacy.id), None);
    daemon.shutdown().await;
}

/// 新しい task は store の root に作られて `task_workspace_roots` に固定され、子 task は（その後 store の
/// root が変わっても）親の固定を継ぐ。
#[tokio::test]
async fn new_task_uses_store_workspace_root() {
    let daemon = Daemon::start().await;
    let (store_root, later) = (
        daemon.home().join("store-root"),
        daemon.home().join("later"),
    );
    let config_root = daemon.config_root();

    daemon.set_store_root(&store_root);
    let parent = local_task(TaskId::new(), None, Status::Running);
    daemon.store.insert(&parent).expect("insert parent");
    let parent_dir =
        prepare_task_dir(&daemon.store, &daemon.roots, &parent, now()).expect("prepare parent");
    assert_eq!(parent_dir, store_root.join(parent.id.to_string()));
    assert_eq!(daemon.pinned(parent.id), Some(s(&store_root)));
    materialize(&daemon.store, &parent, &parent_dir, "parent");
    assert_eq!(daemon.detail_dirs(parent.id).await.0, s(&parent_dir));
    assert_eq!(daemon.stdout(parent.id).await, "{\"line\":\"parent-1\"}\n");
    assert!(!config_root.join(parent.id.to_string()).exists());

    // store の root が変わった後に作られた子は、今の store の root ではなく親の固定を継ぐ。
    daemon.set_store_root(&later);
    let child = local_task(TaskId::new(), Some(parent.id), Status::Running);
    daemon.store.insert(&child).expect("insert child");
    let child_dir =
        prepare_task_dir(&daemon.store, &daemon.roots, &child, now()).expect("prepare child");
    assert_eq!(child_dir, store_root.join(child.id.to_string()));
    assert_eq!(daemon.pinned(child.id), Some(s(&store_root)));
    materialize(&daemon.store, &child, &child_dir, "child");
    assert_eq!(daemon.detail_dirs(child.id).await.0, s(&child_dir));
    assert_eq!(daemon.artifact(child.id).await, "child");

    // 子を持たない新しい root task は、今の store の root（later）に固定される。
    let fresh = local_task(TaskId::new(), None, Status::Running);
    daemon.store.insert(&fresh).expect("insert fresh");
    assert_eq!(
        prepare_task_dir(&daemon.store, &daemon.roots, &fresh, now()).expect("prepare fresh"),
        later.join(fresh.id.to_string())
    );
    assert_eq!(daemon.pinned(fresh.id), Some(s(&later)));
    daemon.shutdown().await;
}
