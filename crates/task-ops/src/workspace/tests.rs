use super::*;

fn task(path: &str, mode: Option<WorkspaceMode>) -> Task {
    use task_core::*;
    let now = time::OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: task_core::TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from(path),
            mode,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 0,
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

/// 目印が無ければ従来どおり `path`（`mode` の既定が `worktree` でも変わらない）。
#[test]
fn without_the_marker_the_local_dir_is_the_path() {
    let root = tempfile::tempdir().expect("tempdir");
    let t = task("/srv/repo", None);
    assert_eq!(local_dir(&t, root.path()), PathBuf::from("/srv/repo"));
}

/// 目印があれば `<workspace_root>/<task_id>`（worktree を消した後も同じ）。
#[test]
fn with_the_marker_the_local_dir_is_the_per_task_directory() {
    let root = tempfile::tempdir().expect("tempdir");
    let t = task("/srv/repo", None);
    let per_task = root.path().join(t.id.to_string());
    std::fs::create_dir_all(&per_task).expect("mkdir");
    std::fs::write(per_task.join(WORKTREE_MARKER), "{}").expect("write");
    assert_eq!(local_dir(&t, root.path()), per_task);
}

/// ADR-0043 D2: 複数リポジトリのタスクは、`mode = shared` のリポジトリでも足回りが per-task にある
/// （目印の `repos` が空でないのが印）。
#[test]
fn a_multi_repo_marker_wins_over_the_shared_mode() {
    let root = tempfile::tempdir().expect("tempdir");
    let t = task("/srv/repo", Some(WorkspaceMode::Shared));
    let per_task = root.path().join(t.id.to_string());
    std::fs::create_dir_all(&per_task).expect("mkdir");
    let marker = WorktreeMarker {
        repo: "/srv/repo".into(),
        dir: per_task.join("repos/code").to_string_lossy().into_owned(),
        branch: "celeris/x".into(),
        base: "abc".into(),
        base_kind: "main".into(),
        repos: vec![WorktreeMarkerRepo {
            name: "code".into(),
            kind: "git".into(),
            source: "/srv/repo".into(),
            dir: per_task.join("repos/code").to_string_lossy().into_owned(),
            branch: Some("celeris/x".into()),
            base: Some("abc".into()),
            base_kind: Some("main".into()),
        }],
    };
    write_marker(&per_task, &marker).expect("write");
    assert_eq!(local_dir(&t, root.path()), per_task);
    // Phase 49 までの目印（`repos` 無し）も読める。
    let old = read_marker(&per_task).expect("marker");
    assert_eq!(old.repos.len(), 1);
    std::fs::write(
        per_task.join(WORKTREE_MARKER),
        br#"{"repo":"/srv/repo","dir":"/x/tree","branch":"b","base":"s","base_kind":"main"}"#,
    )
    .expect("write");
    assert_eq!(read_marker(&per_task).expect("marker").repos, Vec::new());
}

/// `mode = shared` は目印があっても従来どおり（worktree を切らないので目印も付かないが、念のため）。
#[test]
fn shared_mode_ignores_the_marker() {
    let root = tempfile::tempdir().expect("tempdir");
    let t = task("/srv/repo", Some(WorkspaceMode::Shared));
    let per_task = root.path().join(t.id.to_string());
    std::fs::create_dir_all(&per_task).expect("mkdir");
    std::fs::write(per_task.join(WORKTREE_MARKER), "{}").expect("write");
    assert_eq!(local_dir(&t, root.path()), PathBuf::from("/srv/repo"));
}
