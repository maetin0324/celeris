use super::super::{Resolution, ResolveContext, classify, git, resolve as resolve_all};
use super::*;
use std::fs;
use std::path::Path;

const MIG: &str = "crates/task-core/migrations";

fn command(repo: &Path, args: &[&str]) -> String {
    git(repo, args).unwrap().trim().to_string()
}

fn write(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn read(repo: &Path, path: &str) -> String {
    fs::read_to_string(repo.join(path)).unwrap()
}

fn commit(repo: &Path, subject: &str) -> String {
    command(repo, &["add", "-A"]);
    command(repo, &["commit", "-q", "-m", subject]);
    command(repo, &["rev-parse", "HEAD"])
}

/// base を作り、`target` と `source` の両 branch を `edit` で進め、`source` を `--no-commit` で merge する。
struct Fixture {
    tmp: tempfile::TempDir,
    ctx: ResolveContext,
}

fn fixture(
    base: &[(&str, &str)],
    target: &[(&str, &str)],
    source: &[(&str, &str)],
    expect_conflict: bool,
) -> Fixture {
    fixture_with(base, target, source, &[], expect_conflict)
}

/// `others` は base から分けた別 branch（名前, 追加 file）。merge より前に作る。
fn fixture_with(
    base: &[(&str, &str)],
    target: &[(&str, &str)],
    source: &[(&str, &str)],
    others: &[(&str, &[(&str, &str)])],
    expect_conflict: bool,
) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    command(repo, &["init", "-q", "-b", "target"]);
    command(repo, &["config", "user.email", "test@example.invalid"]);
    command(repo, &["config", "user.name", "Test"]);
    write(repo, "README.md", "base\n");
    for (path, body) in base {
        write(repo, path, body);
    }
    let base_sha = commit(repo, "base");
    command(repo, &["branch", "source"]);
    for (path, body) in target {
        write(repo, path, body);
    }
    let target_sha = commit(repo, "target");
    command(repo, &["checkout", "-q", "source"]);
    for (path, body) in source {
        write(repo, path, body);
    }
    let source_sha = commit(repo, "source");
    for (branch, files) in others {
        command(repo, &["checkout", "-q", "-b", branch, &base_sha]);
        for (path, body) in *files {
            write(repo, path, body);
        }
        commit(repo, branch);
    }
    command(repo, &["checkout", "-q", "target"]);
    let merged = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "--no-commit", "--no-ff", "source"])
        .output()
        .unwrap();
    assert_eq!(!merged.status.success(), expect_conflict, "{merged:?}");
    Fixture {
        tmp,
        ctx: ResolveContext {
            target_branch: "target".into(),
            target_sha,
            source_branch: "source".into(),
            source_sha,
            merge_base: Some(base_sha),
            generated_command: None,
        },
    }
}

fn resolved(fx: &Fixture) -> Vec<ResolutionAction> {
    match resolve_all(fx.tmp.path(), &fx.ctx).unwrap() {
        Resolution::Resolved { actions } => actions,
        Resolution::NeedsHuman { request } => panic!("needs human: {}", request.reason),
    }
}

fn unmerged(repo: &Path) -> String {
    command(repo, &["diff", "--name-only", "--diff-filter=U"])
}

#[test]
fn migration_same_number_on_two_branches_moves_only_source_file() {
    let fx = fixture(
        &[(&format!("{MIG}/0038_prev.sql"), "select 0;\n")],
        &[(&format!("{MIG}/0039_cron_jobs.sql"), "create table cron;\n")],
        &[
            (&format!("{MIG}/0039_write_sets.sql"), "create table ws;\n"),
            (
                "crates/task-core/src/store/migrations.rs",
                "const WS: &str = include_str!(\"../../migrations/0039_write_sets.sql\");\n",
            ),
        ],
        false,
    );
    let repo = fx.tmp.path();
    let actions = resolved(&fx);
    assert_eq!(
        read(repo, &format!("{MIG}/0039_cron_jobs.sql")),
        "create table cron;\n"
    );
    assert!(!repo.join(format!("{MIG}/0039_write_sets.sql")).exists());
    assert_eq!(
        read(repo, &format!("{MIG}/0040_write_sets.sql")),
        "create table ws;\n"
    );
    assert!(
        read(repo, "crates/task-core/src/store/migrations.rs")
            .contains("migrations/0040_write_sets.sql")
    );
    assert!(actions.iter().any(|a| a.detail.contains("git mv")));
    assert!(
        actions
            .iter()
            .any(|a| a.path == "crates/task-core/src/store/migrations.rs")
    );
    // 振り直し後は番号重複が残らない。
    assert!(
        classify::classify(repo, &fx.ctx.target_sha)
            .unwrap()
            .is_empty()
    );
    // 同じ入力で再実行しても二重に動かさない。
    assert!(resolved(&fx).is_empty());
    assert!(repo.join(".git/MERGE_HEAD").exists());
}

#[test]
fn adr_same_number_renames_source_and_follows_links_and_title() {
    let fx = fixture(
        &[],
        &[("docs/adr/0021-first.md", "# ADR-0021: first\n")],
        &[
            ("docs/adr/0021-second.md", "# ADR-0021: second\n\n本文\n"),
            (
                "docs/index.md",
                "- [second](adr/0021-second.md)\n- 0021-second の補足\n",
            ),
        ],
        false,
    );
    let repo = fx.tmp.path();
    resolved(&fx);
    assert_eq!(read(repo, "docs/adr/0021-first.md"), "# ADR-0021: first\n");
    assert_eq!(
        read(repo, "docs/adr/0022-second.md"),
        "# ADR-0022: second\n\n本文\n"
    );
    assert_eq!(
        read(repo, "docs/index.md"),
        "- [second](adr/0022-second.md)\n- 0022-second の補足\n"
    );
}

#[test]
fn adr_add_add_conflict_keeps_target_version_and_adds_source_under_new_number() {
    let fx = fixture(
        &[],
        &[("docs/adr/0021-plan.md", "# ADR-0021: target plan\n")],
        &[("docs/adr/0021-plan.md", "# ADR-0021: source plan\n")],
        true,
    );
    let repo = fx.tmp.path();
    resolved(&fx);
    assert_eq!(unmerged(repo), "");
    assert_eq!(
        read(repo, "docs/adr/0021-plan.md"),
        "# ADR-0021: target plan\n"
    );
    assert_eq!(
        read(repo, "docs/adr/0022-plan.md"),
        "# ADR-0022: source plan\n"
    );
}

#[test]
fn new_number_skips_numbers_used_on_other_celeris_branches() {
    let jobs = format!("{MIG}/0040_jobs.sql");
    let feed = format!("{MIG}/0041_feed.sql");
    let wu = format!("{MIG}/0050_wu_only.sql");
    // 他 task の branch が 0040・0041 を使っている。celeris-wu/* は走査対象外。
    let fx = fixture_with(
        &[],
        &[(&format!("{MIG}/0039_cron_jobs.sql"), "a;\n")],
        &[(&format!("{MIG}/0039_write_sets.sql"), "b;\n")],
        &[
            ("celeris/other", &[(&jobs, "c;\n"), (&feed, "d;\n")]),
            ("celeris-wu/x/y", &[(&wu, "e;\n")]),
        ],
        false,
    );
    let repo = fx.tmp.path();
    let refs = default_refs(repo, &fx.ctx).unwrap();
    assert!(refs.contains(&"refs/heads/celeris/other".to_string()));
    assert!(!refs.iter().any(|r| r.contains("celeris-wu")));
    resolved(&fx);
    assert!(repo.join(format!("{MIG}/0042_write_sets.sql")).exists());
    assert!(repo.join(format!("{MIG}/0039_cron_jobs.sql")).exists());
}

#[test]
fn target_side_file_is_never_moved() {
    let fx = fixture(
        &[],
        &[(&format!("{MIG}/0039_cron_jobs.sql"), "a;\n")],
        &[(&format!("{MIG}/0039_write_sets.sql"), "b;\n")],
        false,
    );
    let repo = fx.tmp.path();
    let item = ClassifiedPath {
        path: format!("{MIG}/0039_cron_jobs.sql"),
        kind: ConflictKind::Migration,
        duplicate_with: vec![format!("{MIG}/0039_write_sets.sql")],
    };
    let refs = vec![fx.ctx.target_sha.clone()];
    let attempt = resolve_with_refs(repo, &fx.ctx, &item, &refs).unwrap();
    assert_eq!(attempt, ResolveAttempt::Handled { actions: vec![] });
    assert!(repo.join(&item.path).exists());
    assert!(repo.join(format!("{MIG}/0039_write_sets.sql")).exists());
}

#[test]
fn bare_number_literal_goes_to_human_with_reason() {
    let fx = fixture(
        &[],
        &[(&format!("{MIG}/0039_cron_jobs.sql"), "a;\n")],
        &[
            (&format!("{MIG}/0039_write_sets.sql"), "b;\n"),
            (
                "crates/task-core/src/store/migrations.rs",
                "pub(crate) const RESERVED_VERSIONS: &[u32] = &[38, 39];\n",
            ),
        ],
        false,
    );
    let repo = fx.tmp.path();
    let Resolution::NeedsHuman { request } = resolve_all(repo, &fx.ctx).unwrap() else {
        panic!("bare number must need human");
    };
    assert!(
        request.reason.contains("番号だけの参照"),
        "{}",
        request.reason
    );
    assert!(
        request.reason.contains("RESERVED_VERSIONS"),
        "{}",
        request.reason
    );
    assert!(
        request.reason.contains("0040_write_sets.sql"),
        "{}",
        request.reason
    );
}
