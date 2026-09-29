use super::*;
#[test]
fn disabled_and_interval_boundaries() {
    let store = task_core::SqliteStore::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        tick(
            &store,
            &GardenerConfig::default(),
            tmp.path(),
            &[],
            &[],
            OffsetDateTime::now_utc()
        )
        .unwrap()
        .is_empty()
    );
    assert!(!due(100, 3699, 1));
    assert!(due(100, 3700, 1));
    assert!(!due(100, 0, 1));
}
#[test]
fn managed_scan_schedules_bounded_review_and_survives_state_loss() {
    use task_core::{Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun};
    let store = task_core::SqliteStore::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir_all(repo_dir.join("docs")).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_dir)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    std::fs::write(repo_dir.join("docs/a.md"), "# Same\n[broken](missing.md)\n").unwrap();
    std::fs::write(repo_dir.join("docs/b.md"), "# Same\ntext\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "fixture"]);
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        id: ProjectId::new(),
        title: "fixture".into(),
        request: "".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "repo".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::Local {
            path: repo_dir.clone(),
            mode: None,
        },
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&repo).unwrap();
    let policy = docs::Policy {
        mode: docs::Mode::Managed,
        ..Default::default()
    };
    let config = GardenerConfig {
        enabled: true,
        batch_pages: 1,
        max_context_chars: 80,
        ..Default::default()
    };
    let state = tmp.path().join("state");
    let ws = tmp.path().join("workspaces");
    let id = schedule_repo(
        &store,
        &config,
        &ws,
        &[],
        &[],
        now,
        &state,
        "fixture",
        &repo,
        &repo_dir,
        &policy,
    )
    .unwrap()
    .unwrap();
    let task = store.get(id).unwrap().unwrap();
    assert_eq!(task.role.as_deref(), Some("doc-gardener"));
    assert_eq!(task_core::report::support_kind(&task), Some("doc_gardener"));
    assert!(task.repos.is_empty());
    assert_eq!(
        task.workspace,
        WorkspaceSpec::Local {
            path: task.id.to_string().into(),
            mode: None
        }
    );
    assert!(
        ws.join(id.to_string())
            .join("artifacts/docs-audit.json")
            .is_file()
    );
    assert!(task.objective.contains("Do not edit repository files"));
    assert!(
        schedule_repo(
            &store,
            &config,
            &ws,
            &[],
            &[],
            now,
            &state,
            "fixture",
            &repo,
            &repo_dir,
            &policy
        )
        .unwrap()
        .is_none()
    );
    std::fs::remove_dir_all(&state).unwrap();
    assert!(
        schedule_repo(
            &store,
            &config,
            &ws,
            &[],
            &[],
            now + time::Duration::days(30),
            &state,
            "fixture",
            &repo,
            &repo_dir,
            &policy
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(store.list(None).unwrap().len(), 1);
}
