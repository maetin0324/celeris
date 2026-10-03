use super::*;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("kb");
    std::fs::create_dir_all(root.join("projects/demo")).unwrap();
    std::fs::create_dir_all(root.join("user")).unwrap();
    std::fs::write(root.join("README.md"), "# KB\n").unwrap();
    std::fs::write(root.join("projects/demo/a.md"), "# A\nold\n").unwrap();
    std::fs::write(root.join("projects/demo/b.md"), "# B\nold\n").unwrap();
    std::fs::write(root.join("projects/demo/c.md"), "# C\nold\n").unwrap();
    std::fs::write(root.join("projects/demo/d.md"), "# D\nold\n").unwrap();
    std::fs::write(root.join("user/profile.md"), "# Human\nkeep this\n").unwrap();
    (dir, root)
}

fn item(
    root: &Path,
    path: &str,
    action: Action,
    target: Option<&str>,
    content: Option<&str>,
) -> KbAction {
    KbAction {
        path: path.into(),
        action,
        target: target.map(str::to_owned),
        reason: "整理した".into(),
        content: content.map(str::to_owned),
        expected_hash: std::fs::read_to_string(root.join(path))
            .ok()
            .map(|s| content_hash(&s)),
        target_hash: target
            .and_then(|t| std::fs::read_to_string(root.join(t)).ok())
            .map(|s| content_hash(&s)),
    }
}

fn plan(kb: Vec<KbAction>) -> CurationPlan {
    CurationPlan {
        version: 1,
        kb,
        inbox: vec![],
        human_decisions: vec![],
    }
}

#[test]
fn curation_diff_match_accepts_normalized_plan_paths() {
    let (_dir, root) = fixture();
    let validated = validate(
        &root,
        &plan(vec![
            item(
                &root,
                "projects/demo/a.md",
                Action::Merge,
                Some("projects/demo/b.md"),
                Some("# B\nmerged\n"),
            ),
            item(
                &root,
                "projects/demo/new.md",
                Action::New,
                None,
                Some("# New\n"),
            ),
        ]),
    )
    .unwrap();
    let diff = "--- a/inputs/kb/projects/demo/a.md\n+++ /dev/null\n@@ -1 +0,0 @@\n--- a/inputs/kb/projects/demo/b.md\n+++ b/inputs/kb/projects/demo/b.md\n@@ -1 +1 @@\n--- /dev/null\n+++ b/inputs/kb/projects/demo/new.md\n@@ -0,0 +1 @@\n";
    assert_eq!(check_diff_matches(&validated, diff), Ok(()));
}

#[test]
fn curation_diff_match_reports_extra_paths() {
    let (_dir, root) = fixture();
    let validated = validate(
        &root,
        &plan(vec![item(
            &root,
            "projects/demo/a.md",
            Action::Fix,
            None,
            Some("# A\nupdated\n"),
        )]),
    )
    .unwrap();
    let err = check_diff_matches(
        &validated,
        "--- a/projects/demo/a.md\n+++ b/projects/demo/a.md\n--- /dev/null\n+++ b/projects/demo/extra.md\n",
    )
    .unwrap_err();
    assert!(err.contains("余分: [projects/demo/extra.md]"));
}

#[test]
fn curation_diff_match_reports_missing_paths() {
    let (_dir, root) = fixture();
    let validated = validate(
        &root,
        &plan(vec![
            item(
                &root,
                "projects/demo/a.md",
                Action::Merge,
                Some("projects/demo/b.md"),
                Some("# B\nmerged\n"),
            ),
            item(&root, "projects/demo/c.md", Action::Delete, None, None),
        ]),
    )
    .unwrap();
    let err = check_diff_matches(
        &validated,
        "--- a/projects/demo/a.md\n+++ /dev/null\n--- a/projects/demo/b.md\n+++ b/projects/demo/b.md\n",
    )
    .unwrap_err();
    assert!(err.contains("不足: [projects/demo/c.md]"));
}

#[test]
fn curation_diff_match_ignores_keep_actions() {
    let (_dir, root) = fixture();
    let validated = validate(
        &root,
        &plan(vec![
            item(&root, "user/profile.md", Action::Keep, None, None),
            item(
                &root,
                "projects/demo/a.md",
                Action::Merge,
                Some("projects/demo/b.md"),
                Some("# B\nmerged\n"),
            ),
            item(&root, "projects/demo/c.md", Action::Delete, None, None),
        ]),
    )
    .unwrap();
    assert!(validated.kb.iter().any(|item| item.action == Action::Keep));

    let matching_diff = "--- a/projects/demo/a.md\n+++ /dev/null\n--- a/projects/demo/b.md\n+++ b/projects/demo/b.md\n--- a/projects/demo/c.md\n+++ /dev/null\n";
    assert_eq!(check_diff_matches(&validated, matching_diff), Ok(()));

    let keep_only_diff = "--- a/user/profile.md\n+++ b/user/profile.md\n";
    let err = check_diff_matches(&validated, keep_only_diff).unwrap_err();
    assert!(err.contains("不足: [projects/demo/a.md, projects/demo/b.md, projects/demo/c.md]"));
    assert!(err.contains("余分: [user/profile.md]"));
}

#[test]
fn dry_run_never_changes_the_kb_and_apply_handles_each_action() {
    let (_dir, root) = fixture();
    let plan = plan(vec![
        item(
            &root,
            "projects/demo/a.md",
            Action::Merge,
            Some("projects/demo/b.md"),
            Some("# B\nmerged\n"),
        ),
        item(
            &root,
            "projects/demo/new.md",
            Action::New,
            None,
            Some("# New\n"),
        ),
        item(&root, "projects/demo/c.md", Action::Delete, None, None),
        item(
            &root,
            "projects/demo/d.md",
            Action::Fix,
            None,
            Some("# D\nfixed\n"),
        ),
        item(&root, "user/profile.md", Action::Keep, None, None),
    ]);
    let before = std::fs::read_to_string(root.join("projects/demo/a.md")).unwrap();
    let diff = dry_run(&root, &plan).unwrap();
    assert!(diff.contains("+++ /dev/null"));
    assert!(diff.contains("+++ b/projects/demo/new.md"));
    assert_eq!(
        std::fs::read_to_string(root.join("projects/demo/a.md")).unwrap(),
        before
    );
    assert!(!root.join("projects/demo/new.md").exists());
    assert!(!root.join("_curation").exists());
    let outcome = apply(&root, &plan, "2026-10-03").unwrap();
    assert_eq!(
        outcome,
        ApplyOutcome {
            merged: 1,
            new: 1,
            deleted: 1,
            fixed: 1,
            kept: 1,
            skipped_human: 0
        }
    );
    assert!(!root.join("projects/demo/a.md").exists());
    assert!(!root.join("projects/demo/c.md").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("projects/demo/b.md")).unwrap(),
        "# B\nmerged\n"
    );
    assert!(
        std::fs::read_to_string(root.join("_curation/2026-10-03.md"))
            .unwrap()
            .contains("projects/demo/c.md")
    );
    assert!(
        knowledge::load_index(&root)
            .unwrap()
            .get("projects/demo/new.md")
            .is_some()
    );
    assert!(
        std::fs::read_to_string(root.join("README.md"))
            .unwrap()
            .contains("projects/demo/new.md")
    );
}

#[test]
fn human_pages_are_moved_to_decisions_and_not_changed() {
    let (_dir, root) = fixture();
    std::fs::write(
        root.join("projects/demo/a.md"),
        "---\nsource: human\n---\n# A\nimportant\n",
    )
    .unwrap();
    let plan = plan(vec![
        item(&root, "user/profile.md", Action::Delete, None, None),
        item(
            &root,
            "projects/demo/a.md",
            Action::Fix,
            None,
            Some("# Replaced\n"),
        ),
    ]);
    let validated = validate(&root, &plan).unwrap();
    assert!(validated.kb.is_empty());
    assert_eq!(validated.human_decisions.len(), 2);
    assert_eq!(dry_run(&root, &plan).unwrap(), "");
    assert_eq!(apply(&root, &plan, "2026-10-03").unwrap().skipped_human, 2);
    assert!(root.join("user/profile.md").exists());
    assert!(
        std::fs::read_to_string(root.join("projects/demo/a.md"))
            .unwrap()
            .contains("important")
    );
}

#[test]
fn rejects_unsafe_paths_missing_pages_hash_changes_and_duplicate_operations() {
    let (_dir, root) = fixture();
    for path in [
        "../escape.md",
        "/tmp/escape.md",
        "projects/../escape.md",
        "_curation/log.md",
    ] {
        let bad = plan(vec![item(&root, path, Action::New, None, Some("# New\n"))]);
        assert!(validate(&root, &bad).is_err(), "{path}");
    }
    let missing = plan(vec![item(
        &root,
        "projects/demo/missing.md",
        Action::Delete,
        None,
        None,
    )]);
    assert!(validate(&root, &missing).is_err());
    let duplicate = plan(vec![
        item(&root, "projects/demo/a.md", Action::Delete, None, None),
        item(&root, "projects/demo/a.md", Action::Keep, None, None),
    ]);
    assert!(validate(&root, &duplicate).is_err());
    let stale = plan(vec![item(
        &root,
        "projects/demo/a.md",
        Action::Fix,
        None,
        Some("# Updated\n"),
    )]);
    std::fs::write(root.join("projects/demo/a.md"), "# Changed\n").unwrap();
    assert!(dry_run(&root, &stale).is_err());
    assert!(apply(&root, &stale, "2026-10-03").is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("projects/demo/a.md")).unwrap(),
        "# Changed\n"
    );
}

#[test]
fn inbox_proposals_must_refer_to_the_supplied_input() {
    let (_dir, root) = fixture();
    let mut plan = plan(vec![]);
    plan.inbox.push(InboxProposal {
        task_id: "01ABC".into(),
        proposal: "既読".into(),
        reason: "古い報告".into(),
    });
    assert!(validate_with_inbox(&root, &plan, Some(&BTreeSet::new())).is_err());
    assert!(validate_with_inbox(&root, &plan, Some(&BTreeSet::from(["01ABC".into()]))).is_ok());
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escape() {
    let (_dir, root) = fixture();
    std::os::unix::fs::symlink("/tmp", root.join("projects/elsewhere")).unwrap();
    let bad = plan(vec![item(
        &root,
        "projects/elsewhere/escape.md",
        Action::New,
        None,
        Some("# Escape\n"),
    )]);
    assert!(validate(&root, &bad).is_err());
}
