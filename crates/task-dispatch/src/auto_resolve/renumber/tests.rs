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

/// 取り込み側で file を足した commit の日付（fixture の source commit）。
fn source_date(fx: &Fixture) -> String {
    command(
        fx.tmp.path(),
        &["log", "-1", "--format=%cs", &fx.ctx.source_sha],
    )
}

#[test]
fn numbered_adr_duplicate_moves_source_file_to_dated_name() {
    // main は agent-docs/adr に、取り込み側は旧 docs/adr に同じ番号を足した（同じ名前空間）。
    let fx = fixture(
        &[],
        &[("agent-docs/adr/0140-first.md", "# ADR-0140: first\n")],
        &[
            ("docs/adr/0140-second.md", "# ADR-0140: second\n\n本文\n"),
            (
                "agent-docs/progress/2026-10-03-x.md",
                "- [second](../adr/0140-second.md)\n- 0140-second の補足\n",
            ),
        ],
        false,
    );
    let repo = fx.tmp.path();
    let date = source_date(&fx);
    let moved = format!("agent-docs/adr/{date}-second.md");
    let actions = resolved(&fx);
    assert_eq!(
        read(repo, "agent-docs/adr/0140-first.md"),
        "# ADR-0140: first\n"
    );
    assert!(!repo.join("docs/adr/0140-second.md").exists());
    assert_eq!(
        read(repo, &moved),
        format!("# ADR {date}-second: second\n\n本文\n")
    );
    assert_eq!(
        read(repo, "agent-docs/progress/2026-10-03-x.md"),
        format!("- [second](../adr/{date}-second.md)\n- {date}-second の補足\n")
    );
    assert!(actions.iter().any(|a| a.detail.contains("git mv")));
    // 番号は振り直さない: 0141 は作られない。
    assert!(!repo.join("agent-docs/adr/0141-second.md").exists());
    assert!(
        classify::classify(repo, &fx.ctx.target_sha)
            .unwrap()
            .is_empty()
    );
    assert!(resolved(&fx).is_empty());
}

#[test]
fn numbered_adr_follow_only_touches_source_side_files() {
    let fx = fixture(
        &[("docs/index.md", "- index\n")],
        &[
            ("agent-docs/adr/0140-first.md", "# ADR-0140: first\n"),
            ("docs/main-only.md", "0140-second は main 側の語\n"),
        ],
        &[("agent-docs/adr/0140-second.md", "# ADR-0140: second\n")],
        false,
    );
    let repo = fx.tmp.path();
    resolved(&fx);
    assert_eq!(
        read(repo, "docs/main-only.md"),
        "0140-second は main 側の語\n"
    );
}

#[test]
fn adr_add_add_conflict_keeps_target_version_and_adds_source_under_dated_name() {
    let fx = fixture(
        &[],
        &[("agent-docs/adr/0140-plan.md", "# ADR-0140: target plan\n")],
        &[("agent-docs/adr/0140-plan.md", "# ADR-0140: source plan\n")],
        true,
    );
    let repo = fx.tmp.path();
    let date = source_date(&fx);
    resolved(&fx);
    assert_eq!(unmerged(repo), "");
    assert_eq!(
        read(repo, "agent-docs/adr/0140-plan.md"),
        "# ADR-0140: target plan\n"
    );
    assert_eq!(
        read(repo, &format!("agent-docs/adr/{date}-plan.md")),
        format!("# ADR {date}-plan: source plan\n")
    );
}

#[test]
fn dated_adr_conflict_goes_to_human() {
    let fx = fixture(
        &[],
        &[("agent-docs/adr/2026-10-03-plan.md", "# target\n")],
        &[("agent-docs/adr/2026-10-03-plan.md", "# source\n")],
        true,
    );
    let Resolution::NeedsHuman { request } = resolve_all(fx.tmp.path(), &fx.ctx).unwrap() else {
        panic!("dated ADR conflict must need human");
    };
    assert!(request.reason.contains("日付名"), "{}", request.reason);
    assert_eq!(
        request.conflict_files,
        vec!["agent-docs/adr/2026-10-03-plan.md".to_string()]
    );
}

#[test]
fn main_migration_numbers_never_move() {
    // main に入った 0039・0040 は本番 DB に適用済み。取り込み側の 0039 だけが動く。
    let cron = format!("{MIG}/0039_cron_jobs.sql");
    let feed = format!("{MIG}/0040_feed.sql");
    let ws = format!("{MIG}/0039_write_sets.sql");
    let fx = fixture(
        &[(&format!("{MIG}/0038_prev.sql"), "select 0;\n")],
        &[
            (&cron, "create table cron;\n"),
            (&feed, "create table feed;\n"),
        ],
        &[(&ws, "create table ws;\n")],
        false,
    );
    let repo = fx.tmp.path();
    resolved(&fx);
    // main 側の migration は名前も内容も target のまま（追加以外の差分が無い）。
    let changed = command(
        repo,
        &[
            "diff",
            "--cached",
            "--name-status",
            "--no-renames",
            &fx.ctx.target_sha,
            "--",
            MIG,
        ],
    );
    assert_eq!(changed, format!("A\t{MIG}/0041_write_sets.sql"));
    for (path, body) in [
        (&cron, "create table cron;\n"),
        (&feed, "create table feed;\n"),
    ] {
        assert_eq!(read(repo, path), body);
    }
    // main 側 file を直接渡しても動かさない。
    for path in [&cron, &feed] {
        let item = ClassifiedPath {
            path: path.clone(),
            kind: ConflictKind::Migration,
            duplicate_with: vec![],
        };
        assert_eq!(
            resolve(repo, &fx.ctx, &item).unwrap(),
            ResolveAttempt::Handled { actions: vec![] }
        );
    }
    assert!(repo.join(&cron).exists() && repo.join(&feed).exists());
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
