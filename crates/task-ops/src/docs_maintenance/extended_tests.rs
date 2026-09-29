use super::*;
#[test]
fn declared_generator_drift_and_all_classifications() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    crate::docs::init_docs_repo(&repo, "Fixture").unwrap();
    for (p, b) in [
        ("docs/architecture.md", "# Architecture\n"),
        ("docs/reference.md", "# Reference\n"),
        ("docs/report.md", "# Experiment\n"),
        ("docs/generated.md", "# Generated\ndo not edit\n"),
        ("docs/plan.md", "# Work\nstatus: active\n"),
        ("generate.rs", "// generator\n"),
    ] {
        std::fs::write(repo.join(p), b).unwrap();
    }
    run(&repo, &["add", "."]).unwrap();
    run(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@local",
            "commit",
            "-m",
            "fixture",
        ],
    )
    .unwrap();
    std::fs::write(repo.join("generate.rs"), "// changed generator\n").unwrap();
    run(&repo, &["add", "."]).unwrap();
    run(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@local",
            "commit",
            "-m",
            "generator changed",
        ],
    )
    .unwrap();
    let mut policy = Policy::default();
    policy
        .categories
        .insert("docs/README.md".into(), Category::Canonical);
    policy
        .generated_sources
        .insert("docs/generated.md".into(), "generate.rs".into());
    let audit = audit_with_policy(&repo, "main", &policy).unwrap();
    for category in [
        Category::Canonical,
        Category::Architecture,
        Category::Reference,
        Category::Residue,
        Category::Generated,
        Category::ActivePlan,
    ] {
        assert!(
            audit.documents.iter().any(|d| d.category == category),
            "{category:?}"
        );
    }
    assert!(audit.documents.iter().any(|d| {
        d.findings
            .iter()
            .any(|f| f.starts_with("generated_drift_candidate"))
    }));
}
#[test]
fn approved_merge_move_and_index_leave_default_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    crate::docs::init_docs_repo(&repo, "Fixture").unwrap();
    let before = audit(&repo, "main").unwrap();
    let mut plan = proposal(&before);
    plan.actions.push(Action::Move {
        path: "docs/README.md".into(),
        destination: "docs/guide.md".into(),
    });
    plan.actions.push(Action::Index {
        path: "docs/index.md".into(),
        body: "# Index\n[Guide](guide.md)\n".into(),
    });
    let state = dir.path().join("state");
    approve_plan(&state, "r", &plan).unwrap();
    let wt = dir.path().join("wt");
    apply_plan(&repo, "main", &wt, &plan, &state, "r").unwrap();
    assert!(wt.join("docs/guide.md").exists());
    assert!(wt.join("docs/index.md").exists());
    assert!(repo.join("docs/README.md").exists());
    assert!(!repo.join("docs/index.md").exists());
}
#[test]
fn traversal_and_symlink_destinations_are_rejected_before_writes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    crate::docs::init_docs_repo(&repo, "Fixture").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path(), repo.join("escape")).unwrap();
        run(&repo, &["add", "escape"]).unwrap();
        run(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@local",
                "commit",
                "-m",
                "symlink fixture",
            ],
        )
        .unwrap();
    }
    let state = dir.path().join("state");
    let wt = dir.path().join("wt");
    for path in ["../outside.md", "escape/outside.md"] {
        let mut plan = proposal(&audit(&repo, "main").unwrap());
        plan.actions.push(Action::Index {
            path: path.into(),
            body: "# no".into(),
        });
        approve_plan(&state, "r", &plan).unwrap();
        assert!(apply_plan(&repo, "main", &wt, &plan, &state, "r").is_err());
        assert!(!wt.exists());
        assert!(!dir.path().join("outside.md").exists());
    }
    let audit = audit(&repo, "main").unwrap();
    assert!(bounded_review(&audit, 10, 100).is_empty());
}
#[test]
fn oversized_document_is_inventoried_without_loading_or_blocking_other_pages() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    crate::docs::init_docs_repo(&repo, "Fixture").unwrap();
    let large = "x".repeat(crate::docs::MAX_PAGE_BYTES + 1);
    std::fs::write(repo.join("docs/huge.md"), &large).unwrap();
    run(&repo, &["add", "."]).unwrap();
    run(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@local",
            "commit",
            "-m",
            "large fixture",
        ],
    )
    .unwrap();
    std::fs::write(repo.join("untracked.md"), "human draft").unwrap();
    let before = run(&repo, &["status", "--porcelain"]).unwrap();
    let first = audit(&repo, "main").unwrap();
    let second = audit(&repo, "main").unwrap();
    let doc = first
        .documents
        .iter()
        .find(|doc| doc.path == "docs/huge.md")
        .unwrap();
    assert!(
        doc.findings
            .iter()
            .any(|finding| finding == "large_document")
    );
    assert_eq!(doc.category, Category::Unknown);
    assert!(doc.excerpt.is_empty());
    assert!(doc.hash.starts_with("git-blob:"));
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    assert!(
        first
            .documents
            .iter()
            .any(|doc| doc.path == "docs/README.md")
    );
    assert_eq!(before, run(&repo, &["status", "--porcelain"]).unwrap());
    assert_eq!(
        std::fs::read_to_string(repo.join("docs/huge.md")).unwrap(),
        large
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("untracked.md")).unwrap(),
        "human draft"
    );
}
