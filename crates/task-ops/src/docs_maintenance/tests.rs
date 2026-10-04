use super::*;
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    crate::docs::init_docs_repo(&repo, "Fixture").unwrap();
    for (p, b) in [
        ("docs/a.md", "# Repeated\n[missing](missing.md)\n"),
        ("docs/b.md", "# Repeated\n"),
        ("docs/unknown.md", "# Undecided\n"),
        ("agent-docs/adr/1.md", "# Decision\n"),
        ("docs/old.md", "# Old\nstatus: superseded\n"),
        ("AGENTS.md", "# Instructions\n"),
    ] {
        let p = repo.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
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
    (dir, repo)
}
#[test]
fn audit_is_read_only_and_authority_stays_unknown() {
    let (_dir, repo) = fixture();
    std::fs::write(repo.join("docs/unknown.md"), "human dirty content").unwrap();
    std::fs::write(repo.join("untracked.md"), "human draft").unwrap();
    let status = run(&repo, &["status", "--porcelain"]).unwrap();
    let first = audit(&repo, "main").unwrap();
    let second = audit(&repo, "main").unwrap();
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    assert_eq!(status, run(&repo, &["status", "--porcelain"]).unwrap());
    assert_eq!(
        std::fs::read_to_string(repo.join("docs/unknown.md")).unwrap(),
        "human dirty content"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("untracked.md")).unwrap(),
        "human draft"
    );
    assert_eq!(
        first
            .documents
            .iter()
            .find(|d| d.path == "docs/unknown.md")
            .unwrap()
            .category,
        Category::Unknown
    );
    assert!(
        first
            .documents
            .iter()
            .any(|d| d.category == Category::Duplicate && !d.evidence.is_empty())
    );
    assert!(
        first
            .documents
            .iter()
            .any(|d| d.category == Category::Historical)
    );
    assert!(
        first
            .documents
            .iter()
            .any(|d| d.findings.iter().any(|f| f.starts_with("broken_link")))
    );
    assert!(bounded_review(&first, 1, 50).chars().count() <= 50);
}
#[test]
fn exact_human_approval_and_isolated_worktree_are_required() {
    let (dir, repo) = fixture();
    let state = dir.path().join("state");
    let wt = dir.path().join("wt");
    let audit = audit(&repo, "main").unwrap();
    let mut plan = proposal(&audit);
    plan.actions.push(Action::Delete {
        path: "docs/b.md".into(),
    });
    assert!(
        apply_plan(&repo, "main", &wt, &plan, &state, "r")
            .unwrap_err()
            .contains("approval")
    );
    approve_plan(&state, "r", &plan).unwrap();
    assert!(apply_plan(&repo, "main", &repo, &plan, &state, "r").is_err());
    std::fs::write(repo.join("untracked.md"), "draft").unwrap();
    assert!(
        apply_plan(&repo, "main", &wt, &plan, &state, "r")
            .unwrap_err()
            .contains("uncommitted")
    );
    std::fs::remove_file(repo.join("untracked.md")).unwrap();
    let mut altered = plan.clone();
    altered.actions.push(Action::Delete {
        path: "docs/a.md".into(),
    });
    assert!(apply_plan(&repo, "main", &wt, &altered, &state, "r").is_err());
    let sha = apply_plan(&repo, "main", &wt, &plan, &state, "r").unwrap();
    assert_ne!(sha, audit.revision);
    assert!(repo.join("docs/b.md").exists());
    assert!(!wt.join("docs/b.md").exists());
    assert_eq!(run(&repo, &["rev-parse", "main"]).unwrap(), audit.revision);
}
#[test]
fn stale_approval_rejected_overlay_does_not_change_repo() {
    let (dir, repo) = fixture();
    let state = dir.path().join("state");
    let before = run(&repo, &["status", "--porcelain"]).unwrap();
    let p = Policy {
        mode: Mode::Managed,
        ..Policy::default()
    };
    save_policy(&state, "r", &p).unwrap();
    assert_eq!(load_policy(&state, "r").unwrap().mode, Mode::Managed);
    assert_eq!(before, run(&repo, &["status", "--porcelain"]).unwrap());
    let mut plan = proposal(&audit(&repo, "main").unwrap());
    plan.actions.push(Action::Delete {
        path: "docs/a.md".into(),
    });
    approve_plan(&state, "r", &plan).unwrap();
    std::fs::write(repo.join("docs/a.md"), "# Changed").unwrap();
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
            "change",
        ],
    )
    .unwrap();
    assert!(
        apply_plan(&repo, "main", &dir.path().join("wt"), &plan, &state, "r")
            .unwrap_err()
            .contains("stale")
    );
    assert!(!may_publish(Category::Residue, true, true));
    assert!(!may_publish(Category::Unknown, true, true));
    assert!(!may_publish(Category::Canonical, true, false));
    assert!(may_publish(Category::Canonical, true, true));
}
