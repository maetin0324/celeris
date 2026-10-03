use super::*;
use task_ops::knowledge_curation::content_hash;

const MINIMAL: &str = r#"{"version":1,"kb":[],"inbox":[],"human_decisions":[]}"#;

/// `<root>/artifacts/curation-plan.json` と `<root>/inputs/kb/README.md` を持つ作業場所を作る。
fn workspace(plan: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(root.join("artifacts")).unwrap();
    std::fs::create_dir_all(root.join("inputs/kb/projects/demo")).unwrap();
    std::fs::write(root.join("inputs/kb/README.md"), "# KB\n").unwrap();
    std::fs::write(root.join("inputs/kb/projects/demo/a.md"), "# A\nold\n").unwrap();
    std::fs::write(root.join("artifacts/curation-plan.json"), plan).unwrap();
    std::fs::write(
        root.join("inputs/inbox.json"),
        r#"{"attention":[],"candidates":[{"task_id":"A"}]}"#,
    )
    .unwrap();
    prepare_snapshot(&root);
    (dir, root)
}

fn prepare_snapshot(root: &Path) {
    let snapshot =
        curation::InputSnapshot::capture(&root.join("inputs/kb"), &root.join("inputs/inbox.json"))
            .unwrap();
    std::fs::write(
        root.join("inputs").join(curation::SNAPSHOT_FILE),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
}

fn explicit(root: &Path) -> ValidateArgs {
    ValidateArgs {
        plan: Some(root.join("artifacts/curation-plan.json")),
        kb: Some(root.join("inputs/kb")),
        ..Default::default()
    }
}

fn resolved(root: &Path) -> Resolved {
    resolve(&explicit(root), root).unwrap()
}

fn run_validate(args: ValidateArgs) -> String {
    let code = run(CurationCommand::Validate(args)).unwrap();
    format!("{code:?}")
}

#[test]
fn worker_shape_of_2026_10_03_is_rejected_with_reason() {
    let (_dir, root) = workspace(include_str!(
        "../../../task-ops/src/knowledge_curation/fixtures/worker-plan-2026-10-03.json"
    ));
    let err = validate_paths(&resolved(&root)).unwrap_err();
    assert!(err.contains("curation-plan.json の形が違う"), "{err}");
    assert!(err.contains("task_id"), "{err}");
    assert_eq!(
        run_validate(explicit(&root)),
        format!("{:?}", ExitCode::from(1))
    );
}

#[test]
fn minimal_plan_passes() {
    let (_dir, root) = workspace(MINIMAL);
    let summary = validate_paths(&resolved(&root)).unwrap();
    assert_eq!(
        (
            summary.merge,
            summary.new,
            summary.delete,
            summary.keep,
            summary.fix,
            summary.inbox,
            summary.human_decisions,
            summary.inbox_candidates
        ),
        (0, 0, 0, 0, 0, 0, 0, 0)
    );
    assert_eq!(
        run_validate(explicit(&root)),
        format!("{:?}", ExitCode::SUCCESS)
    );
    let json = ValidateArgs {
        json: true,
        ..explicit(&root)
    };
    assert_eq!(run_validate(json), format!("{:?}", ExitCode::SUCCESS));
}

#[test]
fn discovers_workspace_root_from_repo_worktree_cwd() {
    let (_dir, root) = workspace(MINIMAL);
    let cwd = root.join("repos/demo");
    std::fs::create_dir_all(&cwd).unwrap();
    let args = ValidateArgs::default();
    let found = resolve(&args, &cwd).unwrap();
    assert_eq!(found.plan, root.join("artifacts/curation-plan.json"));
    assert_eq!(found.kb, root.join("inputs/kb"));
    assert_eq!(found.diff, None);
    assert_eq!(found.inbox, root.join("inputs/inbox.json"));
    assert_eq!(
        found.snapshot,
        root.join("inputs").join(curation::SNAPSHOT_FILE)
    );

    std::fs::write(root.join("artifacts/curation.diff"), "").unwrap();
    std::fs::write(root.join("inputs/inbox.json"), "{}").unwrap();
    let found = resolve(&args, &cwd).unwrap();
    assert_eq!(found.diff, Some(root.join("artifacts/curation.diff")));
    assert_eq!(found.inbox, root.join("inputs/inbox.json"));
    let no_diff = ValidateArgs {
        no_diff: true,
        ..Default::default()
    };
    assert_eq!(resolve(&no_diff, &cwd).unwrap().diff, None);

    // 作業場所の外から探すと、理由つきで失敗する。
    let outside = tempfile::tempdir().unwrap();
    let err = resolve(&args, outside.path()).unwrap_err();
    assert!(err.contains("curation-plan.json が見つからない"), "{err}");
}

#[test]
fn inbox_proposal_for_unknown_task_is_rejected() {
    let plan = r#"{"version":1,"kb":[],"inbox":[{"task_id":"B","proposal":"閉じる","reason":"古い"}],"human_decisions":[]}"#;
    let (_dir, root) = workspace(plan);
    std::fs::write(
        root.join("inputs/inbox.json"),
        r#"{"attention":[],"candidates":[{"task_id":"A"}]}"#,
    )
    .unwrap();
    let r = resolve(&ValidateArgs::default(), &root).unwrap();
    assert!(r.inbox.is_file());
    let err = validate_paths(&r).unwrap_err();
    assert!(err.contains("不正または重複した inbox 提案"), "{err}");
}

#[test]
fn diff_touching_unplanned_path_is_rejected() {
    let hash = content_hash("# A\nold\n");
    let plan = json!({
        "version": 1,
        "kb": [{
            "path": "projects/demo/a.md",
            "action": "fix",
            "target": null,
            "reason": "直した",
            "content": "# A\nnew\n",
            "expected_hash": hash,
            "target_hash": null,
        }],
        "inbox": [],
        "human_decisions": [],
    })
    .to_string();
    let (_dir, root) = workspace(&plan);
    let diff_path = root.join("artifacts/curation.diff");
    std::fs::write(
        &diff_path,
        "--- a/inputs/kb/projects/demo/a.md\n+++ b/inputs/kb/projects/demo/a.md\n@@ -1 +1 @@\n",
    )
    .unwrap();
    let summary = validate_paths(&resolve(&ValidateArgs::default(), &root).unwrap()).unwrap();
    assert_eq!(summary.fix, 1);

    std::fs::write(
        &diff_path,
        "--- a/inputs/kb/projects/demo/other.md\n+++ b/inputs/kb/projects/demo/other.md\n@@ -1 +1 @@\n",
    )
    .unwrap();
    let err = validate_paths(&resolve(&ValidateArgs::default(), &root).unwrap()).unwrap_err();
    assert!(
        err.contains("curation.diff の path が計画と一致しません"),
        "{err}"
    );
    // `--no-diff` なら照合しない。
    let no_diff = ValidateArgs {
        no_diff: true,
        ..Default::default()
    };
    assert!(validate_paths(&resolve(&no_diff, &root).unwrap()).is_ok());
}

#[test]
fn more_than_max_inbox_candidates_is_rejected() {
    let (_dir, root) = workspace(MINIMAL);
    let kb = root.join("inputs/kb");
    std::fs::create_dir_all(kb.join("_inbox")).unwrap();
    let items: Vec<serde_json::Value> = (0..=MAX_INBOX_CANDIDATES_PER_RUN)
        .map(|i| {
            let path = format!("_inbox/c{i:03}.md");
            let body = format!("# C{i}\n");
            std::fs::write(kb.join(&path), &body).unwrap();
            json!({
                "path": path,
                "action": "delete",
                "target": null,
                "reason": "取り込み済み",
                "content": null,
                "expected_hash": content_hash(&body),
                "target_hash": null,
            })
        })
        .collect();
    prepare_snapshot(&root);
    assert_eq!(items.len(), 41);
    let plan = json!({"version": 1, "kb": items, "inbox": [], "human_decisions": []});
    std::fs::write(root.join("artifacts/curation-plan.json"), plan.to_string()).unwrap();
    let err = validate_paths(&resolved(&root)).unwrap_err();
    assert!(err.contains("上限 40"), "{err}");
}

#[test]
fn edited_kb_with_rehashed_plan_is_rejected_but_content_only_edit_passes() {
    let (_dir, root) = workspace(MINIMAL);
    let mut plan = json!({"version":1,"kb":[{
        "path":"projects/demo/a.md", "action":"fix", "reason":"更新",
        "content":"# A\nnew\n", "expected_hash":content_hash("# A\nold\n")
    }], "inbox":[], "human_decisions":[]});
    let path = root.join("artifacts/curation-plan.json");
    std::fs::write(&path, plan.to_string()).unwrap();
    assert_eq!(validate_paths(&resolved(&root)).unwrap().fix, 1);
    std::fs::write(root.join("inputs/kb/projects/demo/a.md"), "# A\nnew\n").unwrap();
    plan["kb"][0]["expected_hash"] = json!(content_hash("# A\nnew\n"));
    std::fs::write(&path, plan.to_string()).unwrap();
    let err = validate_paths(&resolved(&root)).unwrap_err();
    assert!(
        err.contains("inputs/kb が prepare 時の snapshot と一致しません"),
        "{err}"
    );
    assert_eq!(
        run_validate(explicit(&root)),
        format!("{:?}", ExitCode::from(1))
    );
}

#[test]
fn input_mutation_or_missing_snapshot_cannot_pass_even_for_minimal_plan() {
    for change in [
        "add",
        "delete",
        "inbox",
        "missing_inbox",
        "snapshot",
        "missing_snapshot",
        "symlink",
    ] {
        let (_dir, root) = workspace(MINIMAL);
        match change {
            "add" => std::fs::write(root.join("inputs/kb/projects/demo/new.md"), "new").unwrap(),
            "delete" => std::fs::remove_file(root.join("inputs/kb/projects/demo/a.md")).unwrap(),
            "inbox" => std::fs::write(
                root.join("inputs/inbox.json"),
                r#"{"candidates":[{"task_id":"B"}]}"#,
            )
            .unwrap(),
            "missing_inbox" => std::fs::remove_file(root.join("inputs/inbox.json")).unwrap(),
            "snapshot" => {
                std::fs::write(root.join("inputs").join(curation::SNAPSHOT_FILE), "{}").unwrap()
            }
            "missing_snapshot" => {
                std::fs::remove_file(root.join("inputs").join(curation::SNAPSHOT_FILE)).unwrap()
            }
            "symlink" => {
                let page = root.join("inputs/kb/projects/demo/a.md");
                let copy = root.join("a.md");
                std::fs::rename(&page, &copy).unwrap();
                std::os::unix::fs::symlink(copy, page).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(validate_paths(&resolved(&root)).is_err(), "{change}");
        assert_eq!(
            run_validate(explicit(&root)),
            format!("{:?}", ExitCode::from(1)),
            "{change}"
        );
    }
}

#[test]
fn adding_inbox_id_does_not_authorize_a_proposal() {
    let (_dir, root) = workspace(
        r#"{"version":1,"kb":[],"inbox":[{"task_id":"B","proposal":"閉じる","reason":"古い"}],"human_decisions":[]}"#,
    );
    std::fs::write(
        root.join("inputs/inbox.json"),
        r#"{"candidates":[{"task_id":"A"},{"task_id":"B"}]}"#,
    )
    .unwrap();
    let err = validate_paths(&resolved(&root)).unwrap_err();
    assert!(
        err.contains("inputs/inbox.json が prepare 時の snapshot と一致しません"),
        "{err}"
    );
}
