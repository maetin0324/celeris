use super::*;
use task_core::{
    Budget, Check, Criterion, Event, SqliteStore, Task, TaskKind, Trigger, WorkerHint,
    WorkspaceSpec,
};

fn store_with_task(status: Status, updated_at: OffsetDateTime) -> (SqliteStore, TaskId) {
    let store = SqliteStore::open_in_memory().expect("open store");
    let id = insert_task(&store, status, updated_at);
    (store, id)
}

/// `store` に 1 件タスクを足し、`Trigger` を積み重ねて `status` まで進めてから、`updated_at` だけを
/// 望みの時刻に書き戻す（`update_task` は `status`/`attempts`/`lease` を DB の現在値から取るので、
/// 状態機械を通さずに `status` を直接書くことはできない。ADR-0002 の保護）。
pub(crate) fn insert_task(
    store: &SqliteStore,
    status: Status,
    updated_at: OffsetDateTime,
) -> TaskId {
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
        status: Status::Ready,
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
    store.insert(&task).expect("insert");
    match status {
        Status::Cancelled => {
            store
                .apply_transition_with_events(id, Trigger::Cancel, vec![])
                .expect("cancel");
        }
        Status::Done => {
            store
                .apply_transition_with_events(id, Trigger::Dispatch, vec![])
                .expect("dispatch");
            store
                .apply_transition_with_events(id, Trigger::WorkerDone, vec![])
                .expect("worker_done");
            store
                .apply_transition_with_events(id, Trigger::ReviewPass, vec![])
                .expect("review_pass");
        }
        Status::Failed => {
            store
                .apply_transition_with_events(id, Trigger::Dispatch, vec![])
                .expect("dispatch");
            store
                .apply_transition_with_events(id, Trigger::WorkerError { retryable: false }, vec![])
                .expect("worker_error");
        }
        Status::Ready => {}
        Status::Running => {
            store
                .apply_transition_with_events(id, Trigger::Dispatch, vec![])
                .expect("dispatch");
        }
        other => panic!("unsupported status for this test helper: {other:?}"),
    }
    let mut current = store.get(id).expect("get").expect("task exists");
    current.updated_at = updated_at;
    store
        .update_task(&current, Event::worker_progress("test", "backdated"))
        .expect("backdate updated_at");
    id
}

#[test]
fn prunable_paths_finds_target_under_repos_and_ignores_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path();
    let repo_dir = task_dir.join("repos").join("benchfs");
    std::fs::create_dir_all(repo_dir.join("target")).unwrap();
    std::fs::create_dir_all(repo_dir.join("gui").join("node_modules")).unwrap();
    std::fs::create_dir_all(repo_dir.join("src")).unwrap();
    // シンボリックリンクのリポジトリ（`dir` 種別）は対象外。
    let real = dir.path().join("real-elsewhere");
    std::fs::create_dir_all(real.join("target")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, task_dir.join("repos").join("data")).unwrap();

    let mut found = prunable_paths(task_dir);
    found.sort();
    let mut expected = vec![
        repo_dir.join("target"),
        repo_dir.join("gui").join("node_modules"),
    ];
    expected.sort();
    assert_eq!(found, expected);
}

/// ADR 2026-10-07-build-tmp-hygiene D2.2: 終端タスクに残った `runs/<run_id>/tmp` は刈る対象（ログは残す）。
#[test]
fn run_tmpdir_leftovers_of_a_terminal_task_are_prunable() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path();
    let run_dir = task_dir.join("runs").join("run-a");
    std::fs::create_dir_all(run_dir.join("tmp").join("rw")).unwrap();
    std::fs::write(run_dir.join("result.json"), "{}").unwrap();
    std::fs::create_dir_all(task_dir.join("runs").join("run-b")).unwrap();
    let found = prunable_paths(task_dir);
    assert_eq!(found, vec![run_dir.join("tmp")]);
}

#[test]
fn prunable_paths_falls_back_to_the_legacy_tree_dir() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path();
    std::fs::create_dir_all(task_dir.join("tree").join("target")).unwrap();
    let found = prunable_paths(task_dir);
    assert_eq!(found, vec![task_dir.join("tree").join("target")]);
}

#[test]
fn find_prune_candidates_only_returns_terminal_tasks_old_enough_with_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path();
    let now = OffsetDateTime::now_utc();

    let (store, done_id) = store_with_task(Status::Done, now - time::Duration::days(2));
    std::fs::create_dir_all(
        workspace_root
            .join(done_id.to_string())
            .join("repos")
            .join("r")
            .join("target"),
    )
    .unwrap();

    // 終端だが新しすぎる（`after_secs` 未満）ので候補に入らない。
    let fresh_id = insert_task(&store, Status::Failed, now);
    std::fs::create_dir_all(
        workspace_root
            .join(fresh_id.to_string())
            .join("repos")
            .join("r")
            .join("target"),
    )
    .unwrap();

    // 終端でも生成物が残っていないので候補に入らない。
    let clean_id = insert_task(&store, Status::Cancelled, now - time::Duration::days(2));

    // 終端ではない（`ready`）ので候補に入らない。
    let ready_id = TaskId::new();

    let candidates = find_prune_candidates(&store, workspace_root, now, 86400).unwrap();
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].task_id, done_id);
    assert_eq!(candidates[0].paths.len(), 1);
    let _ = (fresh_id, clean_id, ready_id);
}

#[test]
fn after_secs_zero_disables_pruning() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path();
    let now = OffsetDateTime::now_utc();
    let (store, done_id) = store_with_task(Status::Done, now - time::Duration::days(10));
    std::fs::create_dir_all(
        workspace_root
            .join(done_id.to_string())
            .join("repos")
            .join("r")
            .join("target"),
    )
    .unwrap();
    let candidates = find_prune_candidates(&store, workspace_root, now, 0).unwrap();
    assert!(candidates.is_empty());
}

#[test]
fn prune_removes_the_candidate_paths_and_keeps_the_source_tree() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path().join("01TASK");
    let repo_dir = task_dir.join("repos").join("benchfs");
    std::fs::create_dir_all(repo_dir.join("target").join("debug")).unwrap();
    std::fs::create_dir_all(repo_dir.join("src")).unwrap();
    std::fs::write(repo_dir.join("src").join("main.rs"), "fn main() {}").unwrap();
    let candidate = PruneCandidate {
        task_id: TaskId::new(),
        task_dir: task_dir.clone(),
        paths: vec![repo_dir.join("target")],
    };
    let removed = prune(&candidate);
    assert_eq!(removed, vec![repo_dir.join("target")]);
    assert!(!repo_dir.join("target").exists());
    assert!(repo_dir.join("src").join("main.rs").exists());
    let rel = relative_removed(&task_dir, &removed);
    assert_eq!(rel, vec!["repos/benchfs/target".to_string()]);
}
