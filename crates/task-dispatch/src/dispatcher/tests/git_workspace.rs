use super::*;

/// ADR-0059 D3: `mode` 省略・`repos` 無し（コードを触らない仕事）で worktree 準備が exit 65 になったら、
/// 失敗にせず `shared`（`SyncMode::None`）として続行し、`Event::WorkspaceModeDowngraded` を残し、
/// `task.workspace.mode` を `Some(Shared)` に書き戻す。
#[tokio::test]
async fn exit_65_with_no_mode_and_no_repos_downgrades_to_shared_and_continues() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let stub = write_stub_ssh(tmp.path(), 65);
    let task = remote_task(tmp.path(), None, Vec::new());
    store.insert(&task).unwrap();

    let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("/work/proj"));
    settings.sync = SyncMode::Worktree;
    settings.task_id = task.id.to_string();
    settings.ssh_command = vec![stub.to_string_lossy().into_owned()];

    let outcome =
        run_worker_for_test(store.clone(), task.id, tmp.path().join("mirror"), settings).await;
    assert!(outcome.is_ok(), "{:?}", outcome.err());

    let events = store.events_for(task.id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkspaceModeDowngraded { cluster, .. } if cluster == "pegasus")),
        "{events:?}"
    );
    let stored = store.get(task.id).unwrap().expect("task exists");
    assert_eq!(
        stored.workspace,
        WorkspaceSpec::Remote {
            cluster: "pegasus".into(),
            path: PathBuf::from("/work/proj"),
            mode: Some(WorkspaceMode::Shared),
        }
    );
}

/// ADR-0059 D3: `repos` があるタスク（コードを触る想定）は、`mode` 省略でも exit 65 で格下げせず
/// 従来どおり失敗する。
#[tokio::test]
async fn exit_65_with_repos_present_does_not_downgrade() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let stub = write_stub_ssh(tmp.path(), 65);
    let repo = task_core::RepoRef {
        repo_id: task_core::RepoId::new(),
        name: "agent-platform".into(),
    };
    let task = remote_task(tmp.path(), None, vec![repo]);
    store.insert(&task).unwrap();

    let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("/work/proj"));
    settings.sync = SyncMode::Worktree;
    settings.task_id = task.id.to_string();
    settings.ssh_command = vec![stub.to_string_lossy().into_owned()];

    let outcome =
        run_worker_for_test(store.clone(), task.id, tmp.path().join("mirror"), settings).await;
    assert!(outcome.is_err());
    let events = store.events_for(task.id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkspaceModeDowngraded { .. })),
        "{events:?}"
    );
    let stored = store.get(task.id).unwrap().expect("task exists");
    assert_eq!(stored.workspace.remote_mode(), WorkspaceMode::Worktree);
}

/// ADR-0059 D3: 明示的に `mode = "worktree"` を選んだタスクは、`repos` が無くても exit 65 で
/// 格下げしない（利用者の意図を尊重する）。
#[tokio::test]
async fn exit_65_with_explicit_worktree_mode_does_not_downgrade() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let stub = write_stub_ssh(tmp.path(), 65);
    let task = remote_task(tmp.path(), Some(WorkspaceMode::Worktree), Vec::new());
    store.insert(&task).unwrap();

    let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("/work/proj"));
    settings.sync = SyncMode::Worktree;
    settings.task_id = task.id.to_string();
    settings.ssh_command = vec![stub.to_string_lossy().into_owned()];

    let outcome =
        run_worker_for_test(store.clone(), task.id, tmp.path().join("mirror"), settings).await;
    assert!(outcome.is_err());
    let events = store.events_for(task.id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkspaceModeDowngraded { .. })),
        "{events:?}"
    );
}

/// ADR-0041 D1: ローカルの git リポジトリは、タスクごとの worktree
/// （`<workspace_root>/<task_id>/tree`、ブランチ `celeris/<task_id>`、base は `main`）で動く。
/// 成果物・`runs/` は作業ツリーの**外**（`<workspace_root>/<task_id>/`）。
#[tokio::test]
async fn a_local_git_workspace_runs_in_a_per_task_worktree_on_its_own_branch() {
    let repo_dir = tempfile::tempdir().unwrap();
    let main_sha = init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 判定コマンドも worktree の中で走る（ADR-0019 D1 6.）: アダプタが cwd に置いたファイルが見える。
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "test -f in-tree".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec!["in-tree".into()],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    run_until_idle(&mut d, 60).await;

    let task_dir = root.path().join(task.id.to_string());
    let tree = task_dir.join("tree");
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert!(
        tree.join(".git").exists(),
        "worktree at <workspace_root>/<task_id>/tree"
    );
    assert!(
        tree.join("README.md").is_file(),
        "追跡ファイルが checkout されている"
    );
    // ワーカーの cwd は worktree、`workspace`（= `runs/` の親）と成果物はその外。
    let runs = seen.lock().unwrap().clone();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].0.canonicalize().unwrap(),
        tree.canonicalize().unwrap(),
        "cwd は worktree"
    );
    assert_eq!(
        runs[0].1.canonicalize().unwrap(),
        task_dir.canonicalize().unwrap(),
        "workspace は worktree の親"
    );
    assert_eq!(
        runs[0].2,
        task_dir.canonicalize().unwrap().join("artifacts"),
        "成果物は作業ツリーの外"
    );
    assert!(task_dir.join("artifacts").is_dir());
    assert!(task_dir.join("runs").is_dir());
    assert!(
        !tree.join("runs").exists(),
        "`runs/` を作業ツリーに作らない（git status を汚さない）"
    );
    // ブランチは `celeris/<task_id>` で、base は `main`。celeris はコミットしない。
    let branch = format!("celeris/{}", task.id);
    assert!(git_ok(
        repo_dir.path(),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}")
        ]
    ));
    assert_eq!(
        git_out(&tree, &["rev-parse", "HEAD"]),
        main_sha,
        "base は main"
    );
    assert_eq!(
        git_out(&tree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        branch
    );
    // 目印（API / CLI が「run のログは作業ツリーの外」と判断するのに使う）。
    let marker = task_ops::workspace::read_marker(&task_dir).expect("worktree.json");
    assert_eq!(marker.branch, branch);
    assert_eq!(marker.base, main_sha);
    assert_eq!(marker.base_kind, "main");
    assert_eq!(
        task_ops::workspace::local_dir(&store.get(task.id).unwrap().unwrap(), root.path()),
        task_dir
    );
}

/// ADR-0074 §6 F1 (j): git worktree の Task で run 後に `artifacts/` を走査し、`report.md` を
/// `ArtifactProduced{declared:false}` として登録する（`GET /tasks/{id}/artifacts` の材料）。
#[tokio::test]
async fn git_worktree_task_registers_report_md_as_an_artifact() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let mut d = worktree_dispatcher(
        store.clone(),
        Arc::new(WritesReportAdapter),
        root.path(),
        None,
    );
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);

    let events = store.events_for(task_id).unwrap();
    let produced: Vec<&task_core::ArtifactRef> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ArtifactProduced { artifact, .. } => Some(artifact),
            _ => None,
        })
        .collect();
    let report_artifact = produced
        .iter()
        .find(|a| a.path.ends_with("report.md"))
        .unwrap_or_else(|| panic!("no report.md artifact: {produced:?}"));
    assert!(!report_artifact.declared, "{report_artifact:?}");
    assert_eq!(report_artifact.kind, "md");
}

/// ADR-0041 D1: 前置きに作業ツリー・ブランチ・base と「このブランチにコミットせよ」が出る。
#[tokio::test]
async fn the_preamble_note_names_the_worktree_the_branch_and_the_base() {
    let repo_dir = tempfile::tempdir().unwrap();
    let main_sha = init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let d = worktree_dispatcher(
        store.clone(),
        Arc::new(RecordingAdapter {
            seen: Arc::new(StdMutex::new(Vec::new())),
            files: vec![],
        }),
        root.path(),
        None,
    );
    let worktree = d.task_workspaces_for(&task).expect("worktree plan");
    let extras = d
        .run_extras(&task, Some(&worktree), None, "claude-code")
        .unwrap();
    let note = extras.workspace_note.expect("workspace_note");
    let tree = root.path().join(task.id.to_string()).join("tree");
    assert!(note.contains(&format!("→ `{}`", tree.display())), "{note}");
    assert!(
        note.contains(&format!("ブランチ `celeris/{}`", task.id)),
        "{note}"
    );
    assert!(
        note.contains(&format!("base `{}`（main）", &main_sha[..12])),
        "{note}"
    );
    assert!(
        note.contains(&format!("カレントディレクトリは `{}`", tree.display())),
        "{note}"
    );
    assert!(note.contains("ブランチにコミットせよ"), "{note}");
    assert!(note.contains("`main` に直接コミットするな"), "{note}");
    assert!(
        note.contains("`git checkout` でブランチを変えるな"),
        "{note}"
    );
    // ADR-0043 D8: 成果物と文書の置き場。
    assert!(
        note.contains(&format!("は `{}/docs` の下に置け", tree.display())),
        "{note}"
    );
    assert!(note.contains("`artifacts/` は run の中間物"), "{note}");
}

/// ADR-0041 D1: 本番の `current` が `main` の子孫なら、その sha から分岐する（本番より古いコードから始めない）。
#[tokio::test]
async fn the_worktree_branches_from_the_current_release_when_it_is_ahead_of_main() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    // `main` の先に 1 コミット（= 本番のリリースが main に未反映の状態）。
    let _ = git_out(repo_dir.path(), &["checkout", "-q", "-b", "released"]);
    std::fs::write(repo_dir.path().join("shipped.txt"), b"x").unwrap();
    let _ = git_out(repo_dir.path(), &["add", "-A"]);
    let _ = git_out(repo_dir.path(), &["commit", "-q", "-m", "shipped"]);
    let shipped = git_out(repo_dir.path(), &["rev-parse", "HEAD"]);
    let _ = git_out(repo_dir.path(), &["checkout", "-q", "main"]);
    // 偽の `current` リリース（`releases_dir` の親にある。ADR-0040 D6）。
    let home = tempfile::tempdir().unwrap();
    let releases = home.path().join("releases");
    std::fs::create_dir_all(&releases).unwrap();
    let current = home.path().join("current");
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(
        current.join("manifest.json"),
        format!("{{\"sha\":\"{shipped}\",\"sha12\":\"{}\"}}", &shipped[..12]),
    )
    .unwrap();

    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen,
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), Some(releases));
    let worktree = d.local_worktree_for(&task).expect("worktree plan");
    assert_eq!(worktree.base.kind.as_str(), "current");
    assert_eq!(worktree.base.sha, shipped);
    run_until_idle(&mut d, 60).await;
    // クリーンなので worktree は消えているが、ブランチは残り、その先端は `current` の sha。
    let branch = format!("celeris/{}", task.id);
    assert_eq!(git_out(repo_dir.path(), &["rev-parse", &branch]), shipped);
}

/// ADR-0043 D2（ADR-0041 D1 の改定）: **終端では worktree を消さない**（`done` で未取り込みの
/// 差分を見るために残す）。ブランチも残る。
#[tokio::test]
async fn a_worktree_survives_the_terminal_state_together_with_its_branch() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen,
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    assert_eq!(d.tick().unwrap().dispatched, 1);
    finish_worker_and_review(&mut d, task.id).await;
    // 終端に達した次の tick で「片付け」が回っても消えない。
    d.tick().unwrap();

    let task_dir = root.path().join(task.id.to_string());
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert!(
        task_dir.join("tree").is_dir(),
        "ADR-0043 D2: 終端では消さない"
    );
    assert!(task_dir.join("artifacts").is_dir(), "成果物は残る");
    assert!(
        git_ok(
            repo_dir.path(),
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/celeris/{}", task.id)
            ]
        ),
        "ブランチも残る"
    );
    let events = store.events_for(task.id).unwrap();
    assert!(
        !events.iter().any(
            |(_, e)| matches!(e, Event::WorkerProgress { msg, .. } if msg.starts_with("未コミット"))
        ),
        "クリーンなら「未コミットの変更」は出さない"
    );
}

/// ADR-0043 D2: **中止**（cancel）されたタスクの worktree とブランチは消える。
#[tokio::test]
async fn cancelling_a_task_removes_its_worktree_and_branch() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    // 質問で止まる run（`blocked`）にして、終端になる前に人が中止できるようにする。
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Question {
            text: "どちらで進めますか".into(),
        },
        delay: Duration::ZERO,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    run_until_idle(&mut d, 60).await;
    let task_dir = root.path().join(task.id.to_string());
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    assert!(task_dir.join("tree").is_dir());
    // 未コミットの変更があっても cancel は消す（人の指示なので）。
    std::fs::write(task_dir.join("tree/wip.txt"), b"x").unwrap();

    // 人が GUI / CLI から中止する（`task-ops::gate::cancel` と同じ遷移）。
    store
        .apply_transition(task.id, task_core::Trigger::Cancel, None)
        .unwrap();
    d.tick().unwrap();

    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert!(
        !task_dir.join("tree").exists(),
        "中止したら worktree は消える"
    );
    assert!(
        task_dir.join("artifacts").is_dir(),
        "run の記録と成果物は残る"
    );
    assert!(
        !git_ok(
            repo_dir.path(),
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/celeris/{}", task.id)
            ]
        ),
        "中止したらブランチも消える"
    );
}

/// ADR-0041 D1 / ADR-0043 D2: 未コミットの変更が残っていれば `WorkerProgress` を 1 行積む（worktree は残す）。
#[tokio::test]
async fn a_dirty_worktree_is_kept_and_a_progress_line_is_appended() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "test -f left-behind".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen,
        files: vec!["left-behind".into()],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    run_until_idle(&mut d, 60).await;
    d.tick().unwrap();

    let tree = root.path().join(task.id.to_string()).join("tree");
    assert!(
        tree.join("left-behind").is_file(),
        "未コミットの変更ごと残す"
    );
    let events = store.events_for(task.id).unwrap();
    let progress: Vec<&String> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } if msg.starts_with("未コミット") => Some(msg),
            _ => None,
        })
        .collect();
    assert_eq!(progress.len(), 1, "1 行だけ");
    assert!(
        progress[0].contains(&tree.display().to_string()),
        "{}",
        progress[0]
    );
    // 2 回目の tick で重ねて積まない（記録は片付けたら落とす）。
    d.tick().unwrap();
    let again = store
        .events_for(task.id)
        .unwrap()
        .iter()
        .filter(|(_, e)| matches!(e, Event::WorkerProgress { msg, .. } if msg.starts_with("未コミット")))
        .count();
    assert_eq!(again, 1);
}

#[tokio::test]
async fn approved_docs_reconciliation_reuses_exact_worktree_branch_and_commit() {
    use task_ops::{
        docs_maintenance as docs,
        workspace::{WorktreeMarker, WorktreeMarkerRepo},
    };
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let base = init_test_repo(&source);
    let workspace = root.path().join("workspaces");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, repos) = project_with_repos(
        &store,
        &[("code", source.as_path(), task_core::RepoKind::Git)],
    );
    let mut task = new_task(
        &source,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.project_id = Some(project_id);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    store.insert(&task).unwrap();
    let task_dir = workspace.join(task.id.to_string());
    let attached = task_dir.join("repos/code");
    let state = root.path().join("state");
    let mut plan = docs::proposal(&docs::audit(&source, "main").unwrap());
    plan.actions.push(docs::Action::Rewrite {
        path: "README.md".into(),
        body: "# Approved canonical guide\n".into(),
    });
    docs::approve_plan(&state, "repo", &plan).unwrap();
    let approved_sha = docs::apply_plan(&source, "main", &attached, &plan, &state, "repo").unwrap();
    let branch = git_out(&attached, &["branch", "--show-current"]);
    let marker = WorktreeMarker {
        repo: source.display().to_string(),
        dir: attached.display().to_string(),
        branch: branch.clone(),
        base: base.clone(),
        base_kind: "main".into(),
        repos: vec![WorktreeMarkerRepo {
            name: "code".into(),
            kind: "git".into(),
            source: source.display().to_string(),
            dir: attached.display().to_string(),
            branch: Some(branch.clone()),
            base: Some(base.clone()),
            base_kind: Some("main".into()),
        }],
    };
    task_ops::workspace::write_marker(&task_dir, &marker).unwrap();
    std::fs::create_dir_all(task_dir.join("artifacts")).unwrap();
    std::fs::write(
        task_dir.join("artifacts/reconciliation-plan.json"),
        serde_json::to_vec(&plan).unwrap(),
    )
    .unwrap();
    let d = worktree_dispatcher(
        store,
        Arc::new(RecordingAdapter {
            seen: Arc::new(StdMutex::new(Vec::new())),
            files: vec![],
        }),
        &workspace,
        None,
    );
    let planned = d.task_workspaces_for(&task).unwrap();
    assert_eq!(planned.repos[0].branch(), Some(branch.as_str()));
    assert_eq!(planned.repos[0].worktree.as_ref().unwrap().base.sha, base);
    planned.ensure().await.unwrap();
    assert_eq!(git_out(&attached, &["rev-parse", "HEAD"]), approved_sha);
    assert_eq!(
        std::fs::read_to_string(attached.join("README.md")).unwrap(),
        "# Approved canonical guide\n"
    );
    assert_eq!(git_out(&source, &["rev-parse", "main"]), base);
    std::fs::remove_file(task_dir.join("artifacts/reconciliation-plan.json")).unwrap();
    assert_ne!(
        d.task_workspaces_for(&task).unwrap().repos[0].branch(),
        Some(branch.as_str())
    );
    std::fs::write(
        task_dir.join("artifacts/reconciliation-plan.json"),
        serde_json::to_vec(&plan).unwrap(),
    )
    .unwrap();
    let mut mismatched = marker;
    mismatched.repos[0].source = root.path().join("other").display().to_string();
    task_ops::workspace::write_marker(&task_dir, &mismatched).unwrap();
    assert_ne!(
        d.task_workspaces_for(&task).unwrap().repos[0].branch(),
        Some(branch.as_str())
    );
}

/// ADR-0043 D2: git 2 つ + `dir` 1 つのタスクは、`repos/<name>/` に worktree 2 つと
/// シンボリックリンク 1 つを持ち、cwd は**先頭のリポジトリ**になる。前置きには全部が並ぶ。
#[tokio::test]
async fn a_task_with_several_repos_gets_one_worktree_per_git_repo_and_a_link_for_the_rest() {
    let root = tempfile::tempdir().unwrap();
    let code = root.path().join("benchfs");
    let paper = root.path().join("benchfs-paper");
    init_test_repo(&code);
    init_test_repo(&paper);
    // The recording worker writes this same content. Keep the worktree clean so
    // pre-review target sync can inspect a committed HEAD.
    std::fs::write(code.join("in-tree"), b"x").unwrap();
    git_out(&code, &["add", "in-tree"]);
    git_out(&code, &["commit", "-q", "-m", "review fixture"]);
    let data = root.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("one.csv"), b"1\n").unwrap();

    let ws_root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, repos) = project_with_repos(
        &store,
        &[
            ("benchfs", code.as_path(), task_core::RepoKind::Git),
            ("benchfs-paper", paper.as_path(), task_core::RepoKind::Git),
            ("data", data.as_path(), task_core::RepoKind::Dir),
        ],
    );
    // 先頭が cwd になる（`repos[0]`）。
    let mut task = new_task(
        &code,
        Check::Command {
            cmd: "test -f in-tree".into(),
            expect_exit: 0,
        },
        0,
    );
    task.project_id = Some(project_id);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    store.insert(&task).unwrap();

    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec!["in-tree".into()],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws_root.path(), None);
    // 前置き（`run_extras`）は dispatch の前に組める。
    let workspaces = d.task_workspaces_for(&task).expect("workspaces");
    assert_eq!(workspaces.repos.len(), 3);
    let note = d
        .run_extras(&task, Some(&workspaces), None, "claude-code")
        .unwrap()
        .workspace_note
        .expect("note");
    for name in ["benchfs", "benchfs-paper", "data"] {
        assert!(note.contains(&format!("- `{name}` →")), "{note}");
    }
    assert!(
        note.contains("ディレクトリ。読み書き可。git ではない"),
        "{note}"
    );
    assert!(
        note.contains(&format!("ブランチ `celeris/{}`", task.id)),
        "{note}"
    );

    assert_eq!(d.tick().unwrap().dispatched, 1);
    finish_worker_and_review(&mut d, task.id).await;

    let task_dir = ws_root.path().join(task.id.to_string());
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    // git のリポジトリは worktree。
    for name in ["benchfs", "benchfs-paper"] {
        let dir = task_dir.join("repos").join(name);
        assert!(dir.join(".git").exists(), "{name} は worktree");
        assert!(dir.join("README.md").is_file());
    }
    // `dir` はシンボリックリンク（コピーしない）。
    let link = task_dir.join("repos").join("data");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(link.join("one.csv").is_file());
    // cwd は先頭のリポジトリ。`runs/` と `artifacts/` は作業ツリーの外。
    let (cwd, workspace, artifacts) = seen.lock().unwrap()[0].clone();
    assert_eq!(cwd, task_dir.join("repos/benchfs").canonicalize().unwrap());
    assert_eq!(workspace, task_dir.canonicalize().unwrap());
    assert_eq!(
        artifacts,
        task_dir.canonicalize().unwrap().join("artifacts")
    );
    assert!(task_dir.join("runs").is_dir());
    // 判定コマンドも先頭の worktree で走った（`test -f in-tree` が通っている）。
    assert!(task_dir.join("repos/benchfs/in-tree").is_file());

    // 目印（`worktree.json`）に全部が並ぶ（ファイル閲覧 API がこれを見る）。
    let marker = task_ops::workspace::read_marker(&task_dir).expect("marker");
    assert_eq!(marker.repos.len(), 3);
    assert_eq!(marker.repos[0].name, "benchfs");
    assert_eq!(marker.repos[0].kind, "git");
    assert_eq!(marker.repos[2].kind, "dir");
    assert_eq!(
        marker.dir, marker.repos[0].dir,
        "先頭の写しが Phase 49 の目印になる"
    );
    assert_eq!(
        task_ops::workspace::local_dir(&store.get(task.id).unwrap().unwrap(), ws_root.path()),
        task_dir
    );
}

/// ADR-0006 Phase 115 D3（本番障害 01M3915FARENW8M0JM11XVF6W0）: 報告のまとめ（`report-compressor`）
/// のような diff を作らない内部タスクは、その案件にリポジトリが付いていても worktree を作らない
/// （`work_dir = workspace` のまま走る）。`task_ops::add::create_support_task` を実際に通して、
/// `resolve_repos` の primary 継承の抑止（`workspace_mode: Shared`）と `task_workspaces_for` の
/// 両方が効くことを確かめる（D4(c)）。
#[tokio::test]
async fn a_compaction_task_does_not_get_a_worktree_even_when_its_project_has_a_primary_repo() {
    let root = tempfile::tempdir().unwrap();
    let code = root.path().join("agent-platform");
    init_test_repo(&code);

    let ws_root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, _repos) = project_with_repos(
        &store,
        &[("agent-platform", code.as_path(), task_core::RepoKind::Git)],
    );
    store
        .org_upsert(&org_node_of("secretary", None, OrgKind::Secretary, None))
        .unwrap();
    store
        .org_upsert(&org_node_of(
            "engineering",
            Some("secretary"),
            OrgKind::Department,
            None,
        ))
        .unwrap();

    let spec = task_ops::add::NewTaskSpec {
        requirements: Default::default(),
        title: "報告のまとめ: engineering 課".into(),
        objective: "まとめてください".into(),
        acceptance: Vec::new(),
        kind: TaskKind::Execute,
        tier: None,
        priority: Some(task_ops::add::PriorityInput::Number(0)),
        parent: None,
        depends_on: Vec::new(),
        max_turns: Some(8),
        max_wall_secs: Some(600),
        max_retries: 1,
        role: Some(task_core::report::COMPACTION_ROLE.to_string()),
        genre: None,
        aggregate: false,
        project_id: Some(project_id),
        milestone_id: None,
        assignee: Some("engineering".to_string()),
        workspace: None,
        cluster: None,
        workspace_mode: Some(task_core::WorkspaceMode::Shared),
        adapter: None,
        repos: Vec::new(),
        labels: Vec::new(),
        skills: Vec::new(),
        mode: None,
        category: None,
        features: None,
        execution: None,
        pause_after: None,
        stages_hint: Vec::new(),
        provenance: Default::default(),
        status: None,
    };
    let task = task_ops::add::create_support_task(
        store.as_ref(),
        spec,
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    // ADR-0006 Phase 115 D3: 案件の primary（`agent-platform`）を暗黙に継がない。
    assert!(task.repos.is_empty(), "{:?}", task.repos);

    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: Vec::new(),
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws_root.path(), None);
    // `task_workspaces_for` が `None`（worktree を用意しない）ことを直接確かめる。
    assert!(
        d.task_workspaces_for(&task).is_none(),
        "compaction task should not get any worktree"
    );

    run_until_idle(&mut d, 60).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);

    let task_dir = ws_root.path().join(task.id.to_string());
    // work_dir が無い＝ cwd はそのまま workspace（`task_dir`）。
    let (cwd, workspace, _artifacts) = seen.lock().unwrap()[0].clone();
    assert_eq!(
        cwd, workspace,
        "cwd should just be the workspace, no worktree cwd"
    );
    assert_eq!(workspace, task_dir);
    assert!(!task_dir.join("tree").exists(), "no single-repo worktree");
    assert!(!task_dir.join("repos").exists(), "no per-repo worktree dir");
}

/// ADR-0043 D3（Phase 56）: `[run] mode = "container"` のリポジトリを使うタスクは、コンテナ
/// runtime が使えないと **run を始めず** `blocked` になり、人に質問が積まれる。
///
/// runtime の検出には**偽の podman / docker**（`info` が失敗する sh スクリプト）を使う。
/// 本物の podman / docker にもネットワークにも触らない。
#[tokio::test]
async fn a_container_task_is_blocked_with_a_question_when_no_runtime_works() {
    let root = tempfile::tempdir().unwrap();
    let code = root.path().join("benchfs");
    init_test_repo(&code);
    std::fs::create_dir_all(code.join(".config/celeris")).unwrap();
    std::fs::write(
        code.join(".config/celeris/workspace.toml"),
        b"[run]\nmode = \"container\"\n",
    )
    .unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "container"]] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&code)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // 偽の runtime: `info` が必ず落ちる（podman は rootless、docker はデーモン不在を模す）。
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for (name, message) in [
        ("podman", "newuidmap: Operation not permitted"),
        ("docker", "Cannot connect to the Docker daemon"),
    ] {
        let path = bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\necho '{message}' 1>&2\nexit 1\n")).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    let ws_root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, repos) = project_with_repos(
        &store,
        &[("benchfs", code.as_path(), task_core::RepoKind::Git)],
    );
    let mut task = new_task(
        &code,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.project_id = Some(project_id);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    store.insert(&task).unwrap();

    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws_root.path(), None);
    // 実物の検出（`detect_with` + `probe_program`）を偽の実行ファイルに向ける。
    let probe = task_worker::container::detect_with(task_worker::RuntimePreference::Auto, |rt| {
        task_worker::container::probe_program(
            &bin.join(rt.as_str()).display().to_string(),
            Duration::from_secs(10),
        )
    });
    assert!(!probe.is_available(), "{probe:?}");
    d.set_container_probe(probe);
    run_until_idle(&mut d, 60).await;

    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    assert!(seen.lock().unwrap().is_empty(), "ワーカーは起こさない");
    let events = store.events_for(task.id).unwrap();
    let question = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished { outcome, .. } if outcome.starts_with("question:") => {
                Some(outcome.clone())
            }
            _ => None,
        })
        .expect("question outcome");
    assert!(
        question.contains("コンテナ runtime が使えません"),
        "{question}"
    );
    assert!(question.contains("benchfs"), "{question}");
    assert!(question.contains("newuidmap"), "{question}");
    assert!(question.contains("Cannot connect"), "{question}");
    // `setup` も走らない（実行環境が決まらないので run の手前で止まる）。
    assert!(
        !ws_root
            .path()
            .join(task.id.to_string())
            .join("runs/setup.log")
            .exists()
    );
}

/// ADR-0043 D3（Phase 56）: `[run] mode` を書いていない（= `host`）リポジトリのタスクは、
/// runtime が使えなくても従来どおりホストで走る（コンテナの工事は既存の運用を変えない）。
#[tokio::test]
async fn a_host_task_still_runs_when_no_container_runtime_is_available() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    // 既定（`RuntimeProbe::default()` = 何も使えない）のまま走らせる。
    assert!(!d.container_probe().is_available());
    run_until_idle(&mut d, 60).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "ホストのタスクはそのまま走る"
    );
}

/// ADR-0043 D4: リポジトリの `[commands] check` は、タスクが検査コマンドを書いていないときだけ
/// レビューの暗黙の条件になる（書いていればタスクの方が勝つ）。
#[tokio::test]
async fn the_repository_check_commands_are_the_reviewers_default() {
    let root = tempfile::tempdir().unwrap();
    let code = root.path().join("benchfs");
    init_test_repo(&code);
    std::fs::create_dir_all(code.join(".config/celeris")).unwrap();
    std::fs::write(
        code.join(".config/celeris/workspace.toml"),
        b"[commands]\ncheck = [\"test -f in-tree\"]\n",
    )
    .unwrap();
    // The recording worker writes this same content; the check fixture is a
    // committed snapshot for pre-review target sync.
    std::fs::write(code.join("in-tree"), b"x").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "check"]] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&code)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let ws_root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, repos) = project_with_repos(
        &store,
        &[("benchfs", code.as_path(), task_core::RepoKind::Git)],
    );
    // 受け入れ条件に `Check::Command` が無い → リポジトリの `check` が暗黙の条件として足される。
    let mut task = new_task(
        &code,
        Check::ArtifactExists {
            name: "missing.json".into(),
        },
        0,
    );
    task.project_id = Some(project_id);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    store.insert(&task).unwrap();
    // 自分で検査コマンドを書いたタスクには足さない（明示が勝つ）。
    let mut explicit = new_task(
        &code,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    explicit.project_id = Some(project_id);
    explicit.repos = task.repos.clone();
    store.insert(&explicit).unwrap();

    let seen = Arc::new(StdMutex::new(Vec::new()));
    // ワーカーが cwd に `in-tree` を置くので、リポジトリの `check` は通る。
    let adapter = Arc::new(RecordingAdapter {
        seen,
        files: vec!["in-tree".into()],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws_root.path(), None);
    assert_eq!(
        d.default_checks(&store.get(task.id).unwrap().unwrap()),
        vec!["test -f in-tree".to_string()]
    );
    run_until_idle(&mut d, 60).await;

    let verdicts = |id: TaskId| -> Vec<(usize, bool, String)> {
        store
            .events_for(id)
            .unwrap()
            .iter()
            .filter_map(|(_, e)| match e {
                Event::ReviewVerdict {
                    criterion_idx,
                    pass,
                    reason,
                    ..
                } => Some((*criterion_idx, *pass, reason.clone())),
                _ => None,
            })
            .collect()
    };
    // 条件 0（`artifact_exists`）は落ち、暗黙の条件 1（リポジトリの `check`）は通る。
    let mine = verdicts(task.id);
    assert_eq!(mine.len(), 2, "{mine:?}");
    assert_eq!(mine[0].0, 0);
    assert!(!mine[0].1, "{mine:?}");
    assert_eq!(mine[1].0, 1);
    assert!(mine[1].1, "{mine:?}");
    assert!(mine[1].2.contains("workspace.toml check"), "{mine:?}");

    // 自分で検査コマンドを書いたタスクには暗黙の条件は足されない。
    let theirs = verdicts(explicit.id);
    assert_eq!(theirs.len(), 1, "{theirs:?}");
    assert!(theirs[0].1, "{theirs:?}");
    assert_eq!(
        store.get(explicit.id).unwrap().unwrap().status,
        Status::Done
    );
}

/// ADR-0041 D1: `mode = "shared"` は従来どおり `path` をそのまま作業ディレクトリにする。
#[tokio::test]
async fn shared_mode_keeps_the_repository_itself_as_the_working_directory() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        Some(task_core::WorkspaceMode::Shared),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    assert!(d.local_worktree_for(&task).is_none());
    run_until_idle(&mut d, 60).await;

    let runs = seen.lock().unwrap().clone();
    assert_eq!(
        runs[0].0,
        repo_dir.path().canonicalize().unwrap(),
        "cwd はリポジトリそのもの"
    );
    assert_eq!(
        runs[0].2,
        repo_dir.path().canonicalize().unwrap().join("artifacts")
    );
    assert!(
        !root.path().join(task.id.to_string()).exists(),
        "タスクごとのディレクトリは作らない"
    );
}

/// ADR-0041 D1: git リポジトリでない `path` は `mode` の既定が `worktree` でも従来どおり。
#[tokio::test]
async fn a_local_path_that_is_not_a_git_repository_is_unchanged() {
    let plain = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        plain.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    assert!(d.local_worktree_for(&task).is_none());
    run_until_idle(&mut d, 60).await;

    let runs = seen.lock().unwrap().clone();
    assert_eq!(runs[0].0, plain.path().canonicalize().unwrap());
    assert_eq!(runs[0].1, plain.path().canonicalize().unwrap());
    assert!(!root.path().join(task.id.to_string()).exists());
}

/// ADR-0041 D1: 委譲の子は親の作業場所を継ぐので、**子ごとに別の worktree**になる。
/// 親の集約 run には子のブランチ名が渡る（親はそれを merge する）。
#[tokio::test]
async fn each_delegated_child_gets_its_own_worktree_and_the_parent_sees_the_branches() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut parent = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    parent.aggregate = true;
    store.insert(&parent).unwrap();
    let mut children = Vec::new();
    for title in ["a", "b"] {
        let mut child = git_task(
            repo_dir.path(),
            None,
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        );
        child.parent_id = Some(parent.id);
        child.title = title.into();
        child.status = Status::Done;
        store.insert(&child).unwrap();
        children.push(child);
    }
    store
        .append_event(
            parent.id,
            &Event::Transitioned {
                from: Status::Running,
                to: Status::Reviewing,
                reason: "aggregate".into(),
            },
        )
        .unwrap();
    let d = worktree_dispatcher(
        store.clone(),
        Arc::new(RecordingAdapter {
            seen: Arc::new(StdMutex::new(Vec::new())),
            files: vec![],
        }),
        root.path(),
        None,
    );
    let extras = d.run_extras(&parent, None, None, "claude-code").unwrap();
    let mut branches: Vec<String> = extras
        .children
        .iter()
        .filter_map(|c| c.branch.clone())
        .collect();
    branches.sort();
    let mut expected: Vec<String> = children
        .iter()
        .map(|c| format!("celeris/{}", c.id))
        .collect();
    expected.sort();
    assert_eq!(branches, expected, "子ごとに別のブランチ");
    // 子の worktree は互いに別のディレクトリ（親の作業ツリーも共有しない）。
    let dirs: Vec<PathBuf> = children
        .iter()
        .map(|c| d.local_worktree_for(c).expect("child worktree").dir)
        .collect();
    assert_ne!(dirs[0], dirs[1]);
    assert_ne!(
        dirs[0],
        d.local_worktree_for(&parent).expect("parent worktree").dir
    );
    // 子でも成果物はタスクごとのディレクトリの中（`.taskd/artifacts/<id>` ではない）。
    assert_eq!(
        extras.children[0].workspace.as_deref(),
        Some(root.path().join(children[0].id.to_string()).as_path())
    );
}

/// ADR-0074 D1.2: WU の run は WU の worktree で走り、成果物は `<task_dir>/wu/<key>/artifacts`、
/// `runs/` は Task のものを共有する。
#[tokio::test]
async fn a_parallel_unit_runs_in_its_own_worktree_with_its_own_artifacts() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(&store, task.id, &["build"], vec![v2_wu("a", "build", &[])]);
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(10)));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    run_until_idle(&mut d, 300).await;
    let task_dir = root.path().join(task.id.to_string());
    let seen = adapter.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let wu_dir = task_dir.join("wu").join("a");
    let repo_name = task_worker::task_repos::repo_display_name(repo.path());
    let expected_tree = wu_dir.join("repos").join(&repo_name);
    assert_eq!(
        seen[0].1.canonicalize().unwrap_or(seen[0].1.clone()),
        expected_tree
            .canonicalize()
            .unwrap_or(expected_tree.clone()),
        "cwd は WU の worktree"
    );
    assert_eq!(seen[0].2, wu_dir.join("artifacts"), "成果物は WU ごと");
    assert!(task_dir.join("runs").is_dir(), "runs/ は Task のものを共有");
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

/// ADR-0074 D1.1: v1 の計画は並列にならず、WU の worktree も統合 WU も作らない（挙動不変）。
#[tokio::test]
async fn a_v1_plan_stays_serial_without_work_unit_worktrees() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "v1".into(),
        work_units: vec![wu_spec("a", &[]), wu_spec("b", &[]), wu_spec("c", &[])],
        phases: Vec::new(),
        children: Vec::new(),
    };
    task_ops::execution::adopt_plan(
        store.as_ref(),
        task.id,
        spec,
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(100)));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    run_until_idle(&mut d, 600).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(adapter.max_active(), 1, "v1 は直列");
    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 3, "統合 WU は足さない");
    assert!(
        units
            .iter()
            .all(|u| u.branch.is_none() && u.phase.is_none())
    );
    assert!(!root.path().join(task.id.to_string()).join("wu").exists());
}

/// Repair an already-run blocked child, then use the ordinary dispatcher and worker path.
#[tokio::test]
async fn repository_required_attached_blocked_child_runs_in_registered_worktree() {
    let root = tempfile::tempdir().unwrap();
    let code = root.path().join("source");
    init_test_repo(&code);
    let ws_root = root.path().join("workspaces");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, _) = project_with_repos(
        &store,
        &[("code", code.as_path(), task_core::RepoKind::Git)],
    );
    let mut parent = new_task(
        root.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    parent.status = Status::Draft;
    store.insert(&parent).unwrap();
    let mut task = new_task(
        root.path(),
        Check::Command {
            cmd: "test -f README.md".into(),
            expect_exit: 0,
        },
        0,
    );
    task.parent_id = Some(parent.id);
    task.workspace = WorkspaceSpec::local(task.id.to_string());
    task.status = Status::Blocked;
    task.attempts = 1;
    store.insert(&task).unwrap();
    let task_dir = ws_root.join(task.id.to_string());
    std::fs::create_dir_all(task_dir.join("artifacts")).unwrap();
    std::fs::write(task_dir.join("artifacts/old-run.txt"), "keep").unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, &ws_root, None);
    assert!(d.task_workspaces_for(&task).is_none());
    let edited = task_ops::edit::edit_task(
        store.as_ref(),
        task.id,
        task_ops::edit::TaskEdit {
            project_id: Some(project_id),
            repos: Some(vec!["code".into()]),
            ..Default::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(edited.task.status, Status::Blocked);
    store
        .apply_transition(
            task.id,
            Trigger::Answer,
            Some(Event::Answered {
                question: "リポジトリが無い".into(),
                answer: "リポジトリを付けた".into(),
            }),
        )
        .unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    finish_worker_and_review(&mut d, task.id).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        seen.lock().unwrap()[0].0,
        task_dir.join("repos/code").canonicalize().unwrap()
    );
    assert!(task_dir.join("repos/code/.git").is_file());
    assert!(task_dir.join("repos/code/README.md").is_file());
    assert_eq!(
        std::fs::read_to_string(task_dir.join("artifacts/old-run.txt")).unwrap(),
        "keep"
    );
    let marker = task_ops::workspace::read_marker(&task_dir).unwrap();
    assert_eq!(marker.repos[0].name, "code");
}
