use super::*;

fn task(path: &str, mode: Option<WorkspaceMode>) -> Task {
    use task_core::*;
    let now = time::OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
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
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from(path),
            mode,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
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
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// 固定の無い（この ADR 以前の）task として、設定ファイルの root `root` で解く。
fn dir_of(t: &Task, root: &std::path::Path) -> PathBuf {
    let store = task_core::SqliteStore::open_in_memory().expect("store");
    local_dir(&store, &WorkspaceRoots::new(root.to_path_buf(), None), t).expect("local_dir")
}

/// 目印が無ければ従来どおり `path`（`mode` の既定が `worktree` でも変わらない）。
#[test]
fn without_the_marker_the_local_dir_is_the_path() {
    let root = tempfile::tempdir().expect("tempdir");
    let t = task("/srv/repo", None);
    assert_eq!(dir_of(&t, root.path()), PathBuf::from("/srv/repo"));
}

/// 目印があれば `<root>/<task_id>`（worktree を消した後も同じ）。
#[test]
fn with_the_marker_the_local_dir_is_the_per_task_directory() {
    let root = tempfile::tempdir().expect("tempdir");
    let t = task("/srv/repo", None);
    let per_task = root.path().join(t.id.to_string());
    std::fs::create_dir_all(&per_task).expect("mkdir");
    std::fs::write(per_task.join(WORKTREE_MARKER), "{}").expect("write");
    assert_eq!(dir_of(&t, root.path()), per_task);
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
    assert_eq!(dir_of(&t, root.path()), per_task);
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
    assert_eq!(dir_of(&t, root.path()), PathBuf::from("/srv/repo"));
}

/// ADR 2026-10-14-local-machine-settings D5: 足回りを初めて用意するときに root が固定され、以後の
/// `local_dir` は store の上書きが変わっても固定の root で解く。子は親の固定を継ぐ。
#[test]
fn prepare_task_dir_pins_the_root_and_local_dir_follows_the_pin() {
    use task_core::store::LocalSettingsOverrides;
    let store = task_core::SqliteStore::open_in_memory().expect("store");
    let cfg = tempfile::tempdir().expect("tempdir");
    let pinned = tempfile::tempdir().expect("tempdir");
    let later = tempfile::tempdir().expect("tempdir");
    let roots = WorkspaceRoots::new(cfg.path().to_path_buf(), None);
    let now = time::OffsetDateTime::now_utc();
    let set = |root: &std::path::Path| {
        store
            .local_settings_set(
                &LocalSettingsOverrides {
                    workspace_root: Some(root.to_string_lossy().into_owned()),
                    max_concurrency: None,
                },
                "admin",
                now,
            )
            .expect("set");
    };
    set(pinned.path());
    let parent = task("rel", None);
    let mut child = task("rel", None);
    child.parent_id = Some(parent.id);
    let parent_dir = prepare_task_dir(&store, &roots, &parent, now).expect("prepare");
    assert_eq!(parent_dir, pinned.path().join(parent.id.to_string()));
    write_marker(&parent_dir, &WorktreeMarker::default()).expect("write");

    set(later.path());
    assert_eq!(local_dir(&store, &roots, &parent).expect("dir"), parent_dir);
    assert_eq!(
        prepare_task_dir(&store, &roots, &child, now).expect("prepare"),
        pinned.path().join(child.id.to_string())
    );
    // 目印の無い相対 path も固定の root 基準。
    assert_eq!(
        local_dir(&store, &roots, &child).expect("dir"),
        pinned.path().join("rel")
    );
}
