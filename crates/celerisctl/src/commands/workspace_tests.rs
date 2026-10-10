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
        requirements: Default::default(),
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

// ---- ADR-0067 付記 2026-10-07 D3-d: `celerisctl workspace backfill-artifacts` ----

fn insert_remote_task(store: &SqliteStore) -> TaskId {
    let id = insert_done_task(store, OffsetDateTime::now_utc());
    let mut task = store.get(id).unwrap().unwrap();
    task.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/work/NBB/cmp4"),
        mode: None,
    };
    store
        .update_task(&task, Event::worker_progress("x", "remote"))
        .unwrap();
    id
}

fn produced_paths(store: &SqliteStore, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, ev)| match ev {
            Event::ArtifactProduced { artifact, .. } => Some(artifact.path),
            _ => None,
        })
        .collect()
}

/// `--dry-run` は登録せず、本番の実行は remote の task の写し `<workspace_root>/<task_id>/artifacts/` から
/// 未登録の成果物を `declared: false` で登録する。2 回目は増えない。`--task` で 1 件に絞れる。
#[test]
fn backfill_artifacts_registers_remote_mirror_artifacts_once() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let remote_id = insert_remote_task(&store);
    // local（worktree ではない）task の作業場所はその `path` そのもの（`<workspace_root>/<task_id>` ではない）。
    let local_id = insert_done_task(&store, OffsetDateTime::now_utc());
    let local_ws = dir.path().join("local-ws");
    let mut local = store.get(local_id).unwrap().unwrap();
    local.workspace = WorkspaceSpec::Local {
        path: local_ws.clone(),
        mode: None,
    };
    store
        .update_task(&local, Event::worker_progress("x", "local"))
        .unwrap();
    for ws in [workspace_root.join(remote_id.to_string()), local_ws] {
        let final_dir = ws.join("artifacts").join("cmp4").join("final");
        std::fs::create_dir_all(&final_dir).unwrap();
        std::fs::write(final_dir.join("report.md"), "# report\n").unwrap();
        std::fs::write(final_dir.join("F1-bandwidth.png"), "png").unwrap();
    }
    let config_path = write_config(dir.path(), &workspace_root);

    let result = run_backfill_artifacts(
        &store,
        BackfillArtifactsArgs {
            config: config_path.clone(),
            task: None,
            dry_run: true,
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);
    assert!(
        produced_paths(&store, remote_id).is_empty(),
        "dry-run must not register"
    );

    let result = run_backfill_artifacts(
        &store,
        BackfillArtifactsArgs {
            config: config_path.clone(),
            task: None,
            dry_run: false,
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);
    assert_eq!(
        produced_paths(&store, remote_id),
        vec![
            "artifacts/cmp4/final/report.md".to_string(),
            "artifacts/cmp4/final/F1-bandwidth.png".to_string(),
        ]
    );
    // `--task` 無しの対象は remote だけ（local の worktree task は run ごとに拾われている）。
    assert!(produced_paths(&store, local_id).is_empty());

    // 2 回目は増えない。
    run_backfill_artifacts(
        &store,
        BackfillArtifactsArgs {
            config: config_path.clone(),
            task: None,
            dry_run: false,
        },
    )
    .unwrap();
    assert_eq!(produced_paths(&store, remote_id).len(), 2);

    // `--task` で local の task も補完できる。
    run_backfill_artifacts(
        &store,
        BackfillArtifactsArgs {
            config: config_path,
            task: Some(local_id.to_string()),
            dry_run: false,
        },
    )
    .unwrap();
    assert_eq!(produced_paths(&store, local_id).len(), 2);
}

/// Parentless retries share the original workspace but must never backfill its files.
#[test]
fn backfill_artifacts_isolates_local_retry_and_original_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let original_id = insert_done_task(&store, OffsetDateTime::now_utc());
    let retry_id = insert_done_task(&store, OffsetDateTime::now_utc());
    let sibling_id = insert_done_task(&store, OffsetDateTime::now_utc());
    let workspace = dir.path().join(original_id.to_string());
    for id in [original_id, retry_id, sibling_id] {
        let mut task = store.get(id).unwrap().unwrap();
        assert!(task.parent_id.is_none());
        task.workspace = WorkspaceSpec::Local {
            path: workspace.clone(),
            mode: None,
        };
        store
            .update_task(&task, Event::worker_progress("x", "shared local"))
            .unwrap();
    }
    let retry_rel = format!(".taskd/artifacts/{retry_id}");
    let retry_dir = workspace.join(&retry_rel);
    std::fs::create_dir_all(retry_dir.join("reports")).unwrap();
    std::fs::create_dir_all(workspace.join("artifacts")).unwrap();
    std::fs::write(workspace.join("artifacts/original.md"), "# original\n").unwrap();
    let sibling_dir = workspace.join(format!(".taskd/artifacts/{sibling_id}"));
    std::fs::create_dir_all(&sibling_dir).unwrap();
    std::fs::write(sibling_dir.join("sibling.md"), "# sibling\n").unwrap();
    // The legacy manaba inventory: 28 reports plus assignments and summary.
    let mut expected = vec![
        format!("{retry_rel}/assignments.md"),
        format!("{retry_rel}/summary.md"),
    ];
    for name in ["assignments.md", "summary.md"] {
        std::fs::write(retry_dir.join(name), "# retry\n").unwrap();
    }
    for i in 0..28 {
        let name = format!("reports/{i:02}.md");
        std::fs::write(retry_dir.join(&name), "# retry report\n").unwrap();
        expected.push(format!("{retry_rel}/{name}"));
    }
    std::fs::write(retry_dir.join("result.json"), "{}").unwrap();
    let config = write_config(dir.path(), dir.path());
    for dry_run in [true, false, false] {
        assert_eq!(
            run_backfill_artifacts(
                &store,
                BackfillArtifactsArgs {
                    config: config.clone(),
                    task: Some(retry_id.to_string()),
                    dry_run,
                }
            )
            .unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            produced_paths(&store, retry_id),
            if dry_run { vec![] } else { expected.clone() }
        );
        assert!(produced_paths(&store, original_id).is_empty());
        assert!(produced_paths(&store, sibling_id).is_empty());
    }
    run_backfill_artifacts(
        &store,
        BackfillArtifactsArgs {
            config,
            task: Some(original_id.to_string()),
            dry_run: false,
        },
    )
    .unwrap();
    assert_eq!(
        produced_paths(&store, original_id),
        vec!["artifacts/original.md"]
    );
}

#[test]
fn backfill_artifacts_with_an_unknown_task_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let config_path = write_config(dir.path(), &workspace_root);
    let result = run_backfill_artifacts(
        &store,
        BackfillArtifactsArgs {
            config: config_path,
            task: Some(TaskId::new().to_string()),
            dry_run: true,
        },
    );
    assert!(matches!(result, Err(CliError::Message(_))), "{result:?}");
}
