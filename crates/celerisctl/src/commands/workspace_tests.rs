use super::*;
use task_core::{
    Budget, Check, Criterion, SqliteStore, Task, TaskId, TaskKind, Trigger, WorkerHint,
    WorkspaceSpec,
};

fn write_config(dir: &std::path::Path, workspace_root: &std::path::Path) -> PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(
            &path,
            format!(
                "db = \"t.sqlite3\"\nworkspace_root = \"{}\"\n[workspace]\nprune_after_secs = 1\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
                workspace_root.display()
            ),
        )
        .unwrap();
    path
}

fn insert_done_task(store: &SqliteStore, updated_at: OffsetDateTime) -> TaskId {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    let task = Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: task_core::Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: task_core::Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("/tmp/ws"),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 60,
            max_retries: 1,
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
        labels: vec![],
        category: Default::default(),
        conversation: None,
    };
    store.insert(&task).unwrap();
    store
        .apply_transition_with_events(id, Trigger::Dispatch, vec![])
        .unwrap();
    store
        .apply_transition_with_events(id, Trigger::WorkerDone, vec![])
        .unwrap();
    store
        .apply_transition_with_events(id, Trigger::ReviewPass, vec![])
        .unwrap();
    let mut current = store.get(id).unwrap().unwrap();
    current.updated_at = updated_at;
    store
        .update_task(&current, Event::worker_progress("x", "backdated"))
        .unwrap();
    id
}

#[test]
fn dry_run_lists_candidates_without_deleting() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let id = insert_done_task(&store, OffsetDateTime::now_utc() - time::Duration::days(2));
    let target = workspace_root
        .join(id.to_string())
        .join("repos")
        .join("r")
        .join("target");
    std::fs::create_dir_all(&target).unwrap();

    let config_path = write_config(dir.path(), &workspace_root);
    let result = run_prune(
        &store,
        PruneArgs {
            config: config_path,
            dry_run: true,
            older_than: None,
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);
    assert!(target.exists(), "dry-run must not delete anything");
    assert!(
        store
            .events_for(id)
            .unwrap()
            .iter()
            .all(|(_, e)| !matches!(e, Event::WorkspacePruned { .. }))
    );
}

#[test]
fn prune_deletes_and_records_the_event() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let id = insert_done_task(&store, OffsetDateTime::now_utc() - time::Duration::days(2));
    let target = workspace_root
        .join(id.to_string())
        .join("repos")
        .join("r")
        .join("target");
    std::fs::create_dir_all(&target).unwrap();

    let config_path = write_config(dir.path(), &workspace_root);
    let result = run_prune(
        &store,
        PruneArgs {
            config: config_path,
            dry_run: false,
            older_than: None,
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);
    assert!(!target.exists());
    assert!(store.events_for(id).unwrap().iter().any(|(_, e)| matches!(
        e,
        Event::WorkspacePruned { removed } if removed == &vec!["repos/r/target".to_string()]
    )));
}

#[test]
fn older_than_overrides_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    // 5 秒前に終端になった（設定の 1 秒は超えるが、`--older-than 3600` なら超えない）。
    let id = insert_done_task(
        &store,
        OffsetDateTime::now_utc() - time::Duration::seconds(5),
    );
    let target = workspace_root
        .join(id.to_string())
        .join("repos")
        .join("r")
        .join("target");
    std::fs::create_dir_all(&target).unwrap();

    let config_path = write_config(dir.path(), &workspace_root);
    run_prune(
        &store,
        PruneArgs {
            config: config_path,
            dry_run: false,
            older_than: Some(3600),
        },
    )
    .unwrap();
    assert!(target.exists(), "3600 秒はまだ経っていない");
}
