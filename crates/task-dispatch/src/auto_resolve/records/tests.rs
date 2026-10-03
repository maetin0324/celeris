use super::*;

fn command(repo: &Path, args: &[&str]) -> String {
    git(repo, args).unwrap().trim().to_string()
}

fn commit(repo: &Path, body: &str, subject: &str) -> String {
    fs::create_dir_all(repo.join("docs/progress")).unwrap();
    fs::write(repo.join("docs/progress/task.md"), body).unwrap();
    command(repo, &["add", "."]);
    command(repo, &["commit", "-m", subject]);
    command(repo, &["rev-parse", "HEAD"])
}

fn conflicted_repo(base: &str, ours: &str, theirs: &str) -> (tempfile::TempDir, ResolveContext) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    command(repo, &["init", "-q", "-b", "target"]);
    command(repo, &["config", "user.email", "test@example.invalid"]);
    command(repo, &["config", "user.name", "Test"]);
    let base_sha = commit(repo, base, "base");
    command(repo, &["branch", "source"]);
    let target_sha = commit(repo, ours, "target record");
    command(repo, &["checkout", "-q", "source"]);
    let source_sha = commit(repo, theirs, "source record");
    command(repo, &["checkout", "-q", "target"]);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "--no-commit", "source"])
        .output()
        .unwrap();
    assert!(!status.status.success(), "test setup needs a conflict");
    (
        tmp,
        ResolveContext {
            target_branch: "target".into(),
            target_sha,
            source_branch: "source".into(),
            source_sha,
            merge_base: Some(base_sha),
            generated_command: None,
        },
    )
}

fn resolve_record(repo: &Path, ctx: &ResolveContext) -> ResolveAttempt {
    resolve(
        repo,
        ctx,
        &ClassifiedPath {
            path: "docs/progress/task.md".into(),
            kind: ConflictKind::Record,
            duplicate_with: Vec::new(),
        },
    )
    .unwrap()
}

#[test]
fn appends_both_tails_and_stages_result() {
    let (tmp, ctx) = conflicted_repo("# Record\n", "# Record\nours\n", "# Record\ntheirs\n");
    let repo = tmp.path();
    assert!(matches!(
        resolve_record(repo, &ctx),
        ResolveAttempt::Handled { .. }
    ));
    assert_eq!(
        fs::read_to_string(repo.join("docs/progress/task.md")).unwrap(),
        "# Record\nours\ntheirs\n"
    );
    assert_eq!(
        command(repo, &["show", ":0:docs/progress/task.md"]),
        "# Record\nours\ntheirs"
    );
    assert!(command(repo, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
}

#[test]
fn inserts_at_distinct_positions() {
    assert_eq!(
        merge(
            "# Record\nfirst\nsecond\nend\n",
            "# Record\nfirst\nours\nsecond\nend\n",
            "# Record\nfirst\nsecond\ntheirs\nend\n",
        ),
        Some("# Record\nfirst\nours\nsecond\ntheirs\nend\n".into())
    );
}

#[test]
fn changed_existing_line_requests_human() {
    let (tmp, ctx) = conflicted_repo(
        "# Record\nold\n",
        "# Record\nchanged\n",
        "# Record\nold\nadded\n",
    );
    let repo = tmp.path();
    assert_eq!(
        resolve_record(repo, &ctx),
        ResolveAttempt::NotHandled {
            reason: "記録の既存行が両側で変わった".into()
        }
    );
    let super::super::Resolution::NeedsHuman { request } =
        super::super::resolve(repo, &ctx).unwrap()
    else {
        panic!("changed record must request human integration");
    };
    assert!(request.reason.contains("記録の既存行が両側で変わった"));
}

#[test]
fn same_section_is_added_once() {
    let section = "## Shared\nentry\n";
    assert_eq!(
        merge(
            "# Record\n",
            &format!("# Record\n{section}"),
            &format!("# Record\n{section}")
        ),
        Some(format!("# Record\n{section}"))
    );
}
