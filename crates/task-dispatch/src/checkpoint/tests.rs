use super::*;

#[test]
fn missing_cwd_yields_empty_repo_facts() {
    let (repo_state, files) = gather_repo_facts(None, "celeris/x");
    assert!(repo_state.is_none());
    assert!(files.is_empty());
}

#[test]
fn tests_and_activity_pairs_test_commands_with_the_following_result() {
    let activity = vec![
        ToolActivity::Use {
            tool: Some("Bash".into()),
            summary: Some("cargo test -p task-core".into()),
        },
        ToolActivity::Result { error: false },
        ToolActivity::Use {
            tool: Some("Edit".into()),
            summary: Some("crates/task-core/src/execution.rs".into()),
        },
        ToolActivity::Result { error: false },
        ToolActivity::Use {
            tool: Some("Bash".into()),
            summary: Some("cargo clippy --workspace".into()),
        },
        ToolActivity::Result { error: true },
    ];
    let (tests_run, recent_activity) = tests_and_activity(&activity);
    assert_eq!(tests_run.len(), 2);
    assert_eq!(tests_run[0].command, "cargo test -p task-core");
    assert_eq!(tests_run[0].exit, Some(0));
    assert_eq!(tests_run[1].command, "cargo clippy --workspace");
    assert_eq!(tests_run[1].exit, Some(1));
    assert_eq!(recent_activity.len(), 3);
    assert_eq!(recent_activity[0], "Bash: cargo test -p task-core");
    assert_eq!(
        recent_activity[1],
        "Edit: crates/task-core/src/execution.rs"
    );
}

#[test]
fn recent_activity_keeps_only_the_most_recent_items() {
    let activity: Vec<ToolActivity> = (0..30)
        .map(|i| ToolActivity::Use {
            tool: Some("Bash".into()),
            summary: Some(format!("step {i}")),
        })
        .collect();
    let (_, recent_activity) = tests_and_activity(&activity);
    assert_eq!(recent_activity.len(), MAX_RECENT_ACTIVITY);
    assert_eq!(recent_activity[0], "Bash: step 10");
    assert_eq!(recent_activity.last().unwrap(), "Bash: step 29");
}

#[test]
fn read_worker_checkpoint_returns_none_when_the_file_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_worker_checkpoint(dir.path()).is_none());
}

#[test]
fn read_worker_checkpoint_reads_a_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("checkpoint.json"),
        r#"{"completed": ["a"], "next_action": "b"}"#,
    )
    .unwrap();
    let cp = read_worker_checkpoint(dir.path()).unwrap();
    assert_eq!(cp.completed, vec!["a".to_string()]);
    assert_eq!(cp.next_action.as_deref(), Some("b"));
}
