use super::*;

fn args(root: &Path) -> RootArgs {
    RootArgs {
        root: Some(root.to_path_buf()),
        // 実ホームの設定を読ませない（テストは `~/.local/share/celeris/knowledge` に触らない）。
        config: Some(PathBuf::from("/nonexistent/celeris.toml")),
    }
}

/// `init` → `record` → `search` → `get` が tempdir の KB だけで完結する（DB もネットワークも使わない）。
#[test]
fn the_cli_works_on_a_temporary_knowledge_base() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    assert_eq!(run_init(&args(&root)).expect("init"), ExitCode::SUCCESS);
    assert!(root.join("user/profile.md").exists());
    // 2 回目も成功する（冪等）。
    assert_eq!(run_init(&args(&root)).expect("again"), ExitCode::SUCCESS);

    // `search` は KB が無ければエラー（黙って空にしない）。
    let missing = dir.path().join("nope");
    assert!(
        run_search(&SearchArgs {
            query: vec!["x".into()],
            scope: None,
            limit: 10,
            json: false,
            root: args(&missing),
        })
        .is_err()
    );

    assert_eq!(
        run_search(&SearchArgs {
            query: vec!["pegasus".into()],
            scope: Some("environment".into()),
            limit: 5,
            json: true,
            root: args(&root),
        })
        .expect("search"),
        ExitCode::SUCCESS
    );
    assert_eq!(
        run_get(&GetArgs {
            path: "user/profile.md".into(),
            json: true,
            root: args(&root),
        })
        .expect("get"),
        ExitCode::SUCCESS
    );
    // 根の外は読めない。
    assert!(
        run_get(&GetArgs {
            path: "../../etc/passwd".into(),
            json: false,
            root: args(&root),
        })
        .is_err()
    );
}

/// ADR-0052 D3: `knowledge rerun <task_id>` は `knowledge_runs` の 1 行を
/// 「まだやり直していない失敗」に戻す（run を作るのはデーモンの次の tick）。
#[test]
fn rerun_resets_the_knowledge_run_row_and_rejects_unknown_ids() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task_id = TaskId::new();
    let run_task_id = TaskId::new();
    let now = time::OffsetDateTime::now_utc();
    store
        .knowledge_run_create(task_id, run_task_id, now)
        .expect("create");
    store
        .knowledge_run_finish(
            task_id,
            task_core::KnowledgeRunState::Done,
            now,
            Some(&task_core::KnowledgeRunSummary::default()),
            Some(task_core::VIA_LANGMEM),
        )
        .expect("finish");
    store
        .knowledge_run_retry(task_id, TaskId::new(), now)
        .expect("retry");

    assert_eq!(
        run_rerun(
            &store,
            &RerunArgs {
                task_id: task_id.to_string()
            }
        )
        .expect("rerun"),
        ExitCode::SUCCESS
    );
    let run = store
        .knowledge_run_get(task_id)
        .expect("get")
        .expect("some");
    assert_eq!(run.state, task_core::KnowledgeRunState::Failed);
    assert!(run.retried_at.is_none(), "もう 1 回だけ自動でやり直せる");
    assert!(run.via.is_none());

    // 知識整理 run が無いタスク・id の綴り間違いはエラー（黙って成功しない）。
    assert!(
        run_rerun(
            &store,
            &RerunArgs {
                task_id: TaskId::new().to_string()
            }
        )
        .is_err()
    );
    assert!(
        run_rerun(
            &store,
            &RerunArgs {
                task_id: "not-an-ulid".into()
            }
        )
        .is_err()
    );
    // `needs_db` は rerun だけ真。
    assert!(
        KnowledgeCommand::Rerun(RerunArgs {
            task_id: task_id.to_string()
        })
        .needs_db()
    );
    assert!(!KnowledgeCommand::Reindex(RootArgs::default()).needs_db());
}

/// `--root` が最優先。設定が読めなければ既定（`~/.local/share/celeris/knowledge`）に落ちる。
#[test]
fn the_root_flag_wins_over_an_unreadable_config() {
    let explicit = PathBuf::from("/tmp/celerisctl-kb-test");
    assert_eq!(
        root_of(&RootArgs {
            root: Some(explicit.clone()),
            config: Some(PathBuf::from("/nonexistent/celeris.toml")),
        }),
        explicit
    );
    let fallback = root_of(&RootArgs {
        root: None,
        config: Some(PathBuf::from("/nonexistent/celeris.toml")),
    });
    assert!(fallback.ends_with("knowledge"), "{}", fallback.display());
}
