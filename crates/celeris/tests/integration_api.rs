//! ADR 2026-10-11（stranded done branches）: 取り残しの読み出しと人の処分 route を、daemon の config から
//! 配線した `ApiState` で HTTP に通して確かめる。
//!
//! 流れ: store と一時 git repo（`main` と未取り込みの `celeris/<task>`）を作る → 定期 sweep と同じ
//! 経路（`celeris::sweep_stranded_branches`）→ 一覧・task 詳細・受信箱に `unintegrated` と
//! `promotion_pending` が出る → hold → reopen → discard → reopen → API merge で `integrated`
//! （main が task head を含む）→ 冪等 merge → release の昇格で `promoted`。各段で task の統合 event が
//! 追記されることを、順番ごと検査する。
//!
//! 時計は sweep に注入する（固定の unix 時刻）。HTTP の handler は実時計を使う。外部ネットワーク・
//! userns・本番の `~/.local/celeris` には触れない（loopback と tempdir だけ）。

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use celeris::Config;
use serde_json::{Value, json};
use task_core::{
    Check, Criterion, DaemonMode, Event, InstanceRole, Project, ProjectId, ProjectRepo,
    ProjectStatus, RepoId, RepoKind, RepoRef, RepoRun, SharedRole, SqliteStore, Status, Task,
    TaskId, TaskKind, TaskStore, WorkspaceSpec,
};
use task_ops::workspace::{WorktreeMarker, WorktreeMarkerRepo, write_marker};
use time::OffsetDateTime;

const TOKEN: &str = "integration-api-token";
const REPO_NAME: &str = "agent-platform";

struct Daemon {
    dir: tempfile::TempDir,
    store: SqliteStore,
    base: String,
    /// 落とすと `watch` が閉じるので、テストの間は持ち続ける。
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
}

impl Daemon {
    /// `<home>/config.toml` を読み、その設定から `ApiState` を作って loopback に立てる。
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
token_file = "api.token"

[selfdeploy]
releases_dir = "releases"
repo = "repo"
# ADR-0095 付記 D-c: 実行環境の systemd bus には触れない。
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
        // 試験の store は daemon と同じ DB ファイルを開く（マイグレーションもここで流れる）。
        let store = SqliteStore::open(&config.db.path).unwrap_or_else(|e| panic!("open: {e}"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("bind: {e}"));
        let addr: SocketAddr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("addr: {e}"));
        let settings = celeris::api_settings(
            &config,
            addr,
            Some(TOKEN.to_string()),
            "01J00000000000000000000000".to_string(),
            "2026-10-11T00:00:00Z".to_string(),
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
            base: format!("http://{addr}"),
            _daemon_tx: daemon_tx,
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .get(format!("{}{path}", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("get {path}: {e}"));
        read(resp).await
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("post {path}: {e}"));
        read(resp).await
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

async fn read(resp: reqwest::Response) -> (u16, Value) {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// `main` に base、`celeris/<task>` に 1 commit を持つ repo を作る（`main` は base のまま）。
fn fixture_repo(path: &Path, task_branch: &str) -> String {
    std::fs::create_dir_all(path).expect("repo dir");
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.invalid"]);
    git(path, &["config", "user.name", "integration api"]);
    std::fs::write(path.join("base"), "base\n").expect("base file");
    git(path, &["add", "base"]);
    git(path, &["commit", "-m", "base"]);
    git(path, &["switch", "-c", task_branch]);
    std::fs::write(path.join("task.txt"), "task\n").expect("task file");
    git(path, &["add", "task.txt"]);
    git(path, &["commit", "-m", "task change"]);
    let head = git(path, &["rev-parse", "HEAD"]);
    git(path, &["switch", "main"]);
    head
}

fn done_task(id: TaskId, project: ProjectId, repo: RepoId) -> Task {
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: vec![RepoRef {
            repo_id: repo,
            name: REPO_NAME.into(),
        }],
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: "integration api fixture".into(),
        objective: "exercise integration routes".into(),
        acceptance: vec![Criterion {
            text: "fixture exists".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: Vec::new(),
        depends_on: Vec::new(),
        status: Status::Done,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: "workspace".into(),
            mode: None,
        },
        budget: task_core::Budget {
            max_turns: 1,
            max_wall_secs: 60,
            max_retries: 0,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: Some(project),
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// task の統合 event を順番どおりの名前で返す（`IntegrationRequested` は状態変化ではないので除く）。
fn integration_events(store: &SqliteStore, task: TaskId) -> Vec<&'static str> {
    store
        .events_for(task)
        .expect("event history")
        .into_iter()
        .filter_map(|(_, event)| match event {
            Event::IntegrationDetected { .. } => Some("detected"),
            Event::IntegrationHeld { .. } => Some("held"),
            Event::IntegrationReopened { .. } => Some("reopened"),
            Event::IntegrationDiscarded { .. } => Some("discarded"),
            Event::IntegrationMerged { .. } => Some("merged"),
            _ => None,
        })
        .collect()
}

/// 受信箱の応答の中から、この task の `unintegrated` item を探す（形は入れ子でもよい）。
fn find_unintegrated(value: &Value, task: TaskId) -> Option<&Value> {
    match value {
        Value::Object(map) => {
            if map.get("type") == Some(&json!("unintegrated"))
                && map.get("task").and_then(|t| t.get("id")) == Some(&json!(task.to_string()))
            {
                return Some(value);
            }
            map.values().find_map(|v| find_unintegrated(v, task))
        }
        Value::Array(items) => items.iter().find_map(|v| find_unintegrated(v, task)),
        _ => None,
    }
}

#[tokio::test]
async fn integration_api_stranded_branch_round_trip_through_routes_and_events() {
    let daemon = Daemon::start().await;
    let home = daemon.home().to_path_buf();
    let repo_path = home.join("repo");
    let task_id = TaskId::new();
    let branch = format!("celeris/{task_id}");
    fixture_repo(&repo_path, &branch);

    // 案件と repo（`agent-platform`、既定ブランチ main）。
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");
    let project = Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        id: ProjectId::new(),
        title: "agent-platform".into(),
        request: "stranded".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    daemon.store.project_create(&project).expect("project");
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: REPO_NAME.into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(repo_path.clone()),
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    daemon.store.repo_create(&repo).expect("repo");

    // 完了した root task。担当の worktree 目印（merge route が読む）も置く。
    let task = done_task(task_id, project.id, repo.id);
    daemon.store.insert(&task).expect("insert task");
    let task_dir = home.join("ws").join(task_id.to_string());
    write_marker(
        &task_dir,
        &WorktreeMarker {
            repo: repo_path.display().to_string(),
            dir: String::new(),
            branch: branch.clone(),
            base: String::new(),
            base_kind: String::new(),
            repos: vec![WorktreeMarkerRepo {
                name: REPO_NAME.into(),
                kind: "git".into(),
                source: repo_path.display().to_string(),
                dir: task_dir.join("repos").join(REPO_NAME).display().to_string(),
                branch: Some(branch.clone()),
                base: None,
                base_kind: None,
            }],
        },
    )
    .expect("worktree marker");

    // 1. 定期 sweep と同じ経路。取り残しは 1 件の変化として記録される。
    assert_eq!(
        celeris::sweep_stranded_branches(&daemon.store, now).expect("sweep"),
        1
    );
    assert_eq!(integration_events(&daemon.store, task_id), ["detected"]);
    let path_unintegrated = "/integrations?state=unintegrated";
    let (status, list) = daemon.get(&format!("/api/v1{path_unintegrated}")).await;
    assert_eq!(status, 200, "{list}");
    let items = list["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["state"], "unintegrated");
    assert_eq!(items[0]["branch"], branch.as_str());

    // 2. 詳細・一覧・受信箱に取り残しと promotion_pending が出る。
    let (status, detail) = daemon.get(&format!("/api/v1/tasks/{task_id}")).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(
        detail["integrations"][0]["state"], "unintegrated",
        "{detail}"
    );
    assert_eq!(
        detail["promotion"]["status"], "promotion_pending",
        "{detail}"
    );
    let (status, tasks) = daemon.get("/api/v1/tasks?with=integrations").await;
    assert_eq!(status, 200, "{tasks}");
    assert!(
        tasks["items"]
            .as_array()
            .expect("task items")
            .iter()
            .any(|item| {
                item["id"] == task_id.to_string()
                    && item["integrations"][0]["state"] == "unintegrated"
            }),
        "{tasks}"
    );
    let (status, inbox) = daemon.get("/api/v1/inbox").await;
    assert_eq!(status, 200, "{inbox}");
    let item = find_unintegrated(&inbox, task_id).unwrap_or_else(|| panic!("inbox: {inbox}"));
    assert!(
        item["operation_path"]
            .as_str()
            .expect("operation path")
            .ends_with(&format!(
                "/api/v1/tasks/{task_id}/integrations/{REPO_NAME}/"
            )),
        "{item}"
    );

    // 3. hold（理由必須）→ reopen。
    let base = format!("/api/v1/tasks/{task_id}/integrations/{REPO_NAME}");
    let (status, held) = daemon
        .post(
            &format!("{base}/hold"),
            json!({"reason": "waiting for a decision"}),
        )
        .await;
    assert_eq!(status, 200, "{held}");
    assert_eq!(held["state"]["state"], "held");
    assert_eq!(
        integration_events(&daemon.store, task_id),
        ["detected", "held"]
    );
    let (status, reopened) = daemon.post(&format!("{base}/reopen"), json!({})).await;
    assert_eq!(status, 200, "{reopened}");
    assert_eq!(reopened["state"]["state"], "unintegrated");
    assert_eq!(
        integration_events(&daemon.store, task_id),
        ["detected", "held", "reopened"]
    );

    // 4. discard（理由必須）→ reopen。
    let (status, discarded) = daemon
        .post(&format!("{base}/discard"), json!({"reason": "superseded"}))
        .await;
    assert_eq!(status, 200, "{discarded}");
    assert_eq!(discarded["state"]["state"], "discarded");
    assert_eq!(discarded["state"]["reason"], "superseded");
    assert_eq!(
        integration_events(&daemon.store, task_id),
        ["detected", "held", "reopened", "discarded"]
    );
    let (status, reopened) = daemon.post(&format!("{base}/reopen"), json!({})).await;
    assert_eq!(status, 200, "{reopened}");
    assert_eq!(reopened["state"]["state"], "unintegrated");
    assert_eq!(
        integration_events(&daemon.store, task_id),
        ["detected", "held", "reopened", "discarded", "reopened"]
    );

    // 5. API 自身が main に merge し、祖先確認後に integrated を返す。
    let task_head = git(&repo_path, &["rev-parse", &branch]);
    let (status, merged) = daemon.post(&format!("{base}/merge"), json!({})).await;
    assert_eq!(status, 200, "{merged}");
    assert_eq!(merged["integrated"], true);
    assert_eq!(merged["state"]["state"], "integrated");
    assert!(
        std::process::Command::new("git")
            .args(["-C"])
            .arg(&repo_path)
            .args(["merge-base", "--is-ancestor", &task_head, "main"])
            .status()
            .expect("ancestor check")
            .success()
    );
    assert_eq!(
        integration_events(&daemon.store, task_id),
        [
            "detected",
            "held",
            "reopened",
            "discarded",
            "reopened",
            "merged"
        ]
    );
    let (status, detail) = daemon.get(&format!("/api/v1/tasks/{task_id}")).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["integrations"][0]["state"], "integrated", "{detail}");
    // 統合されても昇格は別の欄で、まだ release が無いので pending のまま。
    assert_eq!(
        detail["promotion"]["status"], "promotion_pending",
        "{detail}"
    );

    let (status, tasks) = daemon.get("/api/v1/tasks?with=integrations").await;
    assert_eq!(status, 200, "{tasks}");
    assert!(
        tasks["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == task_id.to_string()
                && item["integrations"][0]["state"] == "integrated"),
        "{tasks}"
    );

    // 6. 同じ merge は冪等: event は増えない。
    let (status, again) = daemon.post(&format!("{base}/merge"), json!({})).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["integrated"], true);
    assert_eq!(again["state"]["state"], "integrated");
    assert_eq!(
        integration_events(&daemon.store, task_id).len(),
        6,
        "idempotent merge must not append an event"
    );

    // 7. release が main（取り込んだ head を含む）を昇格すると promoted になる。
    let main_head = git(&repo_path, &["rev-parse", "main"]);
    assert_eq!(main_head, git(&repo_path, &["rev-parse", "HEAD"]));
    let release = home.join("releases").join(&main_head[..12]);
    std::fs::create_dir_all(&release).expect("release dir");
    std::fs::write(
        release.join("manifest.json"),
        json!({"sha": main_head}).to_string(),
    )
    .expect("manifest");
    std::fs::write(
        release.join("promoted.json"),
        json!({"promoted_at": "2026-10-11T00:00:00Z"}).to_string(),
    )
    .expect("promoted marker");
    let (status, detail) = daemon.get(&format!("/api/v1/tasks/{task_id}")).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["promotion"]["status"], "promoted", "{detail}");
    assert_eq!(detail["promotion"]["release_sha"], main_head.as_str());

    // 取り残しが無くなったので受信箱にも出ない。
    let (status, inbox) = daemon.get("/api/v1/inbox").await;
    assert_eq!(status, 200, "{inbox}");
    assert!(find_unintegrated(&inbox, task_id).is_none(), "{inbox}");

    daemon.shutdown().await;
}

#[tokio::test]
async fn integration_api_merge_conflict_keeps_main_unintegrated() {
    let daemon = Daemon::start().await;
    let home = daemon.home().to_path_buf();
    let repo_path = home.join("repo");
    let task_id = TaskId::new();
    let branch = format!("celeris/{task_id}");
    fixture_repo(&repo_path, &branch);
    git(&repo_path, &["switch", &branch]);
    std::fs::write(repo_path.join("base"), "task version\n").expect("task edit");
    git(&repo_path, &["add", "base"]);
    git(&repo_path, &["commit", "-m", "task conflict edit"]);
    git(&repo_path, &["switch", "main"]);
    std::fs::write(repo_path.join("base"), "main version\n").expect("main edit");
    git(&repo_path, &["add", "base"]);
    git(&repo_path, &["commit", "-m", "main conflict edit"]);
    let main_before = git(&repo_path, &["rev-parse", "main"]);

    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");
    let project = Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        id: ProjectId::new(),
        title: "agent-platform".into(),
        request: "stranded conflict".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    daemon.store.project_create(&project).expect("project");
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: REPO_NAME.into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(repo_path.clone()),
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    daemon.store.repo_create(&repo).expect("repo");
    daemon
        .store
        .insert(&done_task(task_id, project.id, repo.id))
        .expect("insert task");
    let task_dir = home.join("ws").join(task_id.to_string());
    write_marker(
        &task_dir,
        &WorktreeMarker {
            repo: repo_path.display().to_string(),
            dir: String::new(),
            branch: branch.clone(),
            base: String::new(),
            base_kind: String::new(),
            repos: vec![WorktreeMarkerRepo {
                name: REPO_NAME.into(),
                kind: "git".into(),
                source: repo_path.display().to_string(),
                dir: task_dir.join("repos").join(REPO_NAME).display().to_string(),
                branch: Some(branch),
                base: None,
                base_kind: None,
            }],
        },
    )
    .expect("worktree marker");
    assert_eq!(
        celeris::sweep_stranded_branches(&daemon.store, now).expect("sweep"),
        1
    );

    let (status, result) = daemon
        .post(
            &format!("/api/v1/tasks/{task_id}/integrations/{REPO_NAME}/merge"),
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["integrated"], false, "{result}");
    assert!(
        result["reason"].as_str().is_some_and(|s| !s.is_empty()),
        "{result}"
    );
    assert_eq!(git(&repo_path, &["rev-parse", "main"]), main_before);
    let (status, detail) = daemon.get(&format!("/api/v1/tasks/{task_id}")).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(
        detail["integrations"][0]["state"], "unintegrated",
        "{detail}"
    );
    assert!(
        daemon
            .store
            .open_integration_requests()
            .expect("requests")
            .iter()
            .any(|row| { matches!(row.event, Event::IntegrationRequested { .. }) })
    );
    let worktrees = git(&repo_path, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        worktrees
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1,
        "{worktrees}"
    );
    daemon.shutdown().await;
}

/// ADR 2026-10-11 D3: 案件と Local の登録 repo（既定ブランチ main）だけを作る。task は案件に属さない。
fn register_repo(store: &SqliteStore, repo_path: &Path) -> ProjectRepo {
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");
    let project = Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        id: ProjectId::new(),
        title: "agent-platform".into(),
        request: "repoless".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: REPO_NAME.into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(repo_path.to_path_buf()),
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&repo).expect("repo");
    repo
}

/// 本番の 01M4D7RVKX・01M4F3V643 と同じ形: project_id なし・repos 空・workspace は task id。
fn repoless_done_task(id: TaskId) -> Task {
    let mut task = done_task(id, ProjectId::new(), RepoId::new());
    task.project_id = None;
    task.repos = Vec::new();
    task.workspace = WorkspaceSpec::Local {
        path: id.to_string().into(),
        mode: None,
    };
    task
}

#[tokio::test]
async fn integration_api_repoless_done_task_detected_in_inbox_and_detail_then_discarded() {
    let daemon = Daemon::start().await;
    let repo_path = daemon.home().join("repo");
    let task_id = TaskId::new();
    let branch = format!("celeris/{task_id}");
    let head = fixture_repo(&repo_path, &branch);
    let repo = register_repo(&daemon.store, &repo_path);
    daemon
        .store
        .insert(&repoless_done_task(task_id))
        .expect("insert task");
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");

    // (1) 登録 repo の celeris/<id> branch から解決され、main に無い commit は unintegrated。
    assert_eq!(
        celeris::sweep_stranded_branches(&daemon.store, now).expect("sweep"),
        1
    );
    let state = daemon
        .store
        .task_integration_get(task_id, repo.id)
        .expect("state")
        .expect("row");
    assert_eq!(
        state.state,
        task_core::TaskRepoIntegrationStatus::Unintegrated
    );
    assert_eq!(state.branch, branch);
    assert_eq!(state.head_sha, head);
    assert_eq!(integration_events(&daemon.store, task_id), ["detected"]);
    let (status, inbox) = daemon.get("/api/v1/inbox").await;
    assert_eq!(status, 200, "{inbox}");
    assert!(find_unintegrated(&inbox, task_id).is_some(), "{inbox}");
    let (status, detail) = daemon.get(&format!("/api/v1/tasks/{task_id}")).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(
        detail["integrations"][0]["state"], "unintegrated",
        "{detail}"
    );
    assert_eq!(detail["integrations"][0]["repo"], REPO_NAME, "{detail}");
    // 繰り返しの sweep は変化なし。
    assert_eq!(
        celeris::sweep_stranded_branches(&daemon.store, now).expect("repeat sweep"),
        0
    );

    // (2) discard（project なしでも repo 名で解決）→ discarded、受信箱から消え、sweep も戻さない。
    let (status, discarded) = daemon
        .post(
            &format!("/api/v1/tasks/{task_id}/integrations/{REPO_NAME}/discard"),
            json!({"reason": "superseded by a manual merge"}),
        )
        .await;
    assert_eq!(status, 200, "{discarded}");
    assert_eq!(discarded["state"]["state"], "discarded");
    let (status, inbox) = daemon.get("/api/v1/inbox").await;
    assert_eq!(status, 200, "{inbox}");
    assert!(find_unintegrated(&inbox, task_id).is_none(), "{inbox}");
    assert_eq!(
        celeris::sweep_stranded_branches(&daemon.store, now).expect("sweep after discard"),
        0
    );
    assert_eq!(
        integration_events(&daemon.store, task_id),
        ["detected", "discarded"]
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn integration_api_repoless_task_without_branch_records_nothing() {
    let daemon = Daemon::start().await;
    let repo_path = daemon.home().join("repo");
    // 登録 repo には別 task の branch だけがある。
    fixture_repo(&repo_path, &format!("celeris/{}", TaskId::new()));
    register_repo(&daemon.store, &repo_path);
    let task_id = TaskId::new();
    daemon
        .store
        .insert(&repoless_done_task(task_id))
        .expect("insert task");
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");

    // (3) branch が無い repos 空 task は repo を変えていない扱いで、何も記録しない。
    assert_eq!(
        celeris::sweep_stranded_branches(&daemon.store, now).expect("sweep"),
        0
    );
    assert!(
        daemon
            .store
            .task_integration_list(None)
            .expect("integration list")
            .is_empty()
    );
    assert!(integration_events(&daemon.store, task_id).is_empty());
    daemon.shutdown().await;
}

#[tokio::test]
async fn integration_api_repoless_branch_in_main_is_integrated() {
    let daemon = Daemon::start().await;
    let repo_path = daemon.home().join("repo");
    let task_id = TaskId::new();
    let branch = format!("celeris/{task_id}");
    let head = fixture_repo(&repo_path, &branch);
    git(&repo_path, &["merge", "--ff-only", &branch]);
    let repo = register_repo(&daemon.store, &repo_path);
    daemon
        .store
        .insert(&repoless_done_task(task_id))
        .expect("insert task");
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");

    // (4) branch head が main の祖先なら integrated。受信箱には出ない。
    assert_eq!(
        celeris::sweep_stranded_branches(&daemon.store, now).expect("sweep"),
        1
    );
    let state = daemon
        .store
        .task_integration_get(task_id, repo.id)
        .expect("state")
        .expect("row");
    assert_eq!(
        state.state,
        task_core::TaskRepoIntegrationStatus::Integrated
    );
    assert_eq!(state.head_sha, head);
    let (status, inbox) = daemon.get("/api/v1/inbox").await;
    assert_eq!(status, 200, "{inbox}");
    assert!(find_unintegrated(&inbox, task_id).is_none(), "{inbox}");
    daemon.shutdown().await;
}
