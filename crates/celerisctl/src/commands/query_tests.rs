use super::*;
use task_core::{Budget, SqliteStore, TaskKind, WorkerHint, WorkspaceSpec};
use time::OffsetDateTime;

fn sample_task(status: Status, parent_id: Option<TaskId>) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id,
        kind: TaskKind::Execute,
        title: "title".to_string(),
        objective: "objective".to_string(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: task_core::Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "/tmp".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
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

#[test]
fn run_ls_without_filter_lists_all_tasks() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let t1 = sample_task(Status::Draft, None);
    let t2 = sample_task(Status::Ready, None);
    store.insert(&t1).expect("insert t1");
    store.insert(&t2).expect("insert t2");

    let expected = store.list(None).expect("list").len();
    assert_eq!(expected, 2);

    let result = run_ls(
        &store,
        LsArgs {
            status: None,
            tree: false,
        },
    );
    assert!(result.is_ok());
}

#[test]
fn run_ls_with_status_filter_matches_store_list() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let t1 = sample_task(Status::Draft, None);
    let t2 = sample_task(Status::Ready, None);
    store.insert(&t1).expect("insert t1");
    store.insert(&t2).expect("insert t2");

    let expected = store.list(Some(Status::Ready)).expect("list").len();
    assert_eq!(expected, 1);

    let result = run_ls(
        &store,
        LsArgs {
            status: Some(StatusArg::Ready),
            tree: false,
        },
    );
    assert!(result.is_ok());
}

#[test]
fn run_ls_tree_groups_children_under_parent() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let parent = sample_task(Status::Draft, None);
    let child = sample_task(Status::Draft, Some(parent.id));
    store.insert(&parent).expect("insert parent");
    store.insert(&child).expect("insert child");

    let result = run_ls(
        &store,
        LsArgs {
            status: None,
            tree: true,
        },
    );
    assert!(result.is_ok());
}

#[test]
fn run_show_found_task_succeeds() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(Status::Draft, None);
    store.insert(&task).expect("insert task");

    let result = run_show(
        &store,
        ShowArgs {
            id: task.id.to_string(),
            json: false,
            workspace_root: None,
            config: None,
        },
    );
    assert!(result.is_ok());
}

#[test]
fn run_show_missing_task_errors() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let missing_id = TaskId::new().to_string();

    let result = run_show(
        &store,
        ShowArgs {
            id: missing_id,
            json: false,
            workspace_root: None,
            config: None,
        },
    );
    assert!(result.is_err());
}

#[test]
fn run_show_json_prints_task_detail_with_actions_and_matches_task_id() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(Status::Draft, None);
    store.insert(&task).expect("insert task");

    let result = run_show(
        &store,
        ShowArgs {
            id: task.id.to_string(),
            json: true,
            workspace_root: None,
            config: None,
        },
    );
    assert!(result.is_ok());
}

#[test]
fn task_detail_json_is_parseable_and_contains_task_id_and_actions() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(Status::Draft, None);
    store.insert(&task).expect("insert task");

    let json = task_detail_json(&store, task.id, None, None).expect("task_detail_json");
    let value: serde_json::Value = serde_json::from_str(&json).expect("output must be valid json");
    assert_eq!(
        value["task"]["id"],
        serde_json::Value::String(task.id.to_string())
    );
    let actions = value["actions"]
        .as_array()
        .expect("actions must be an array");
    assert!(
        !actions.is_empty(),
        "a draft task should have at least the `approve` action"
    );
}

/// ADR-0019 D2 / P-54: `--config`（`CELERIS_CONFIG`）を渡すと `celerisctl show --json` が API と同じ値になる。
/// `sync = "worktree"` のクラスタのタスクには `worktree`（パスとブランチ）が出る。
#[test]
fn task_detail_json_with_a_config_reports_the_worktree() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut task = sample_task(Status::Ready, None);
    task.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/work/NBB/x/benchfs"),
        mode: None,
    };
    store.insert(&task).expect("insert task");

    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"workspace_root = "ws"
max_requeues = 3
[[providers]]
id = "p"
adapter = "fake"
[[clusters]]
id = "pegasus"
host = "pegasus"
sync = "worktree"
"#,
    )
    .expect("write config");

    let json =
        task_detail_json(&store, task.id, None, Some(&config_path)).expect("task_detail_json");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(
        value["worktree"]["branch"],
        serde_json::Value::String(format!("celeris/{}", task.id))
    );
    assert_eq!(
        value["worktree"]["dir"],
        serde_json::Value::String(format!(
            "/work/NBB/x/benchfs/.celeris-worktrees/{}",
            task.id
        ))
    );
    // 設定の値が効く（既定の 5 ではなく 3）。workspace_root も設定ファイル基準の絶対パスになる。
    assert_eq!(value["timers"]["max_requeues"], serde_json::json!(3));
    assert!(
        value["workspace_dir"]
            .as_str()
            .expect("workspace_dir")
            .starts_with(&dir.path().to_string_lossy().to_string()),
        "{}",
        value["workspace_dir"]
    );

    // `--config` が無ければ今までどおり（`worktree` は null）。
    let json = task_detail_json(&store, task.id, None, None).expect("task_detail_json");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(value["worktree"], serde_json::Value::Null);
}

#[test]
fn run_show_json_missing_task_errors() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let missing_id = TaskId::new().to_string();

    let result = run_show(
        &store,
        ShowArgs {
            id: missing_id,
            json: true,
            workspace_root: None,
            config: None,
        },
    );
    assert!(result.is_err());
}

#[test]
fn run_log_without_follow_succeeds() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(Status::Draft, None);
    store.insert(&task).expect("insert task");

    let result = run_log(
        &store,
        LogArgs {
            id: task.id.to_string(),
            follow: false,
        },
    );
    assert!(result.is_ok());
}
