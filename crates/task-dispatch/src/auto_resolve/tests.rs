use super::*;
use std::fs;
use std::path::Path;

fn command(repo: &Path, args: &[&str]) -> String {
    git(repo, args).unwrap().trim().to_string()
}

fn write(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn commit(repo: &Path, subject: &str) -> String {
    command(repo, &["add", "."]);
    command(repo, &["commit", "-m", subject]);
    command(repo, &["rev-parse", "HEAD"])
}

fn new_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    command(repo, &["init", "-q", "-b", "target"]);
    command(repo, &["config", "user.email", "test@example.invalid"]);
    command(repo, &["config", "user.name", "Test"]);
    tmp
}

#[test]
fn code_conflict_requests_human_with_both_subjects_and_diffstats() {
    let tmp = new_repo();
    let repo = tmp.path();
    write(repo, "src/main.rs", "fn value() -> u8 { 0 }\n");
    let base = commit(repo, "base");
    command(repo, &["branch", "source"]);
    write(repo, "src/main.rs", "fn value() -> u8 { 1 }\n");
    let target_sha = commit(repo, "target changes value");
    command(repo, &["checkout", "-q", "source"]);
    write(repo, "src/main.rs", "fn value() -> u8 { 2 }\n");
    let source_sha = commit(repo, "source changes value");
    command(repo, &["checkout", "-q", "target"]);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "--no-commit", "source"])
        .status()
        .unwrap();
    assert!(!status.success());
    let ctx = ResolveContext {
        target_branch: "target".into(),
        target_sha,
        source_branch: "source".into(),
        source_sha,
        merge_base: Some(base),
        generated_command: None,
    };
    let Resolution::NeedsHuman { request } = resolve(repo, &ctx).unwrap() else {
        panic!("code conflict must need human");
    };
    assert_eq!(request.conflict_files, ["src/main.rs"]);
    assert_eq!(
        request.intent[0].target.commits[0].subject,
        "target changes value"
    );
    assert_eq!(
        request.intent[0].source.commits[0].subject,
        "source changes value"
    );
    assert_eq!(request.intent[0].target.diffstat.as_ref().unwrap().added, 1);
    let markdown = request.to_markdown();
    assert!(markdown.contains("target changes value"));
    assert!(markdown.contains("source changes value"));
    assert!(
        serde_json::to_string(&request)
            .unwrap()
            .contains("src/main.rs")
    );
    assert!(
        repo.join(".git/MERGE_HEAD").exists(),
        "resolver must not abort"
    );
}

#[test]
fn path_classes_and_number_duplicates_are_distinct() {
    use ConflictKind::*;
    assert_eq!(classify::kind("docs/PROGRESS.md"), Record);
    assert_eq!(classify::kind("docs/progress/task.md"), Record);
    assert_eq!(classify::kind("agent-docs/progress/task.md"), Record);
    assert_eq!(
        classify::kind("crates/task-core/migrations/0039_jobs.sql"),
        Migration
    );
    assert_eq!(classify::kind("docs/adr/0137-parallel.md"), Adr);
    assert_eq!(classify::kind("docs/protocol/task.schema.json"), Generated);
    assert_eq!(classify::kind("docs/api/v1/task.schema.json"), Generated);
    assert_eq!(classify::kind("src/main.rs"), Code);

    let tmp = new_repo();
    let repo = tmp.path();
    let target_sha = commit_empty_target(repo);
    write(
        repo,
        "crates/task-core/migrations/0039_jobs.sql",
        "select 1;\n",
    );
    write(
        repo,
        "crates/task-core/migrations/0039_write_sets.sql",
        "select 2;\n",
    );
    write(repo, "docs/adr/0137-first.md", "# First\n");
    write(repo, "docs/adr/0137-second.md", "# Second\n");
    commit(repo, "numbered additions");
    let items = classify::classify(repo, &target_sha).unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(
        items.iter().filter(|item| item.kind == Migration).count(),
        2
    );
    assert_eq!(items.iter().filter(|item| item.kind == Adr).count(), 2);
    assert!(items.iter().all(|item| item.duplicate_with.len() == 1));
}

#[test]
fn active_merge_classifies_each_supported_conflict() {
    use ConflictKind::*;
    let tmp = new_repo();
    let repo = tmp.path();
    let paths = [
        "docs/PROGRESS.md",
        "crates/task-core/migrations/0039_jobs.sql",
        "docs/adr/0137-first.md",
        "docs/protocol/task.schema.json",
    ];
    for path in paths {
        write(repo, path, "base\n");
    }
    commit(repo, "base");
    command(repo, &["branch", "source"]);
    for path in paths {
        write(repo, path, "target\n");
    }
    let target_sha = commit(repo, "target edits");
    command(repo, &["checkout", "-q", "source"]);
    for path in paths {
        write(repo, path, "source\n");
    }
    commit(repo, "source edits");
    command(repo, &["checkout", "-q", "target"]);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "--no-commit", "source"])
        .output()
        .unwrap();
    assert!(!status.status.success());
    let kinds = classify::classify(repo, &target_sha)
        .unwrap()
        .into_iter()
        .map(|item| item.kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds.len(), 4);
    assert!(kinds.contains(&Record));
    assert!(kinds.contains(&Migration));
    assert!(kinds.contains(&Adr));
    assert!(kinds.contains(&Generated));
}

fn commit_empty_target(repo: &Path) -> String {
    command(
        repo,
        &["commit", "--allow-empty", "-m", "target before additions"],
    );
    command(repo, &["rev-parse", "HEAD"])
}
