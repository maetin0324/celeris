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
    assert_eq!(classify::kind("agent-docs/progress/x/leaf.md"), Record);
    assert_eq!(classify::kind("agent-docs/PROGRESS.md"), Record);
    assert_eq!(classify::kind("agent-docs/progress/x/leaf.json"), Code);
    assert_eq!(classify::kind("docs/adr/0137-parallel.md"), Adr);
    assert_eq!(classify::kind("agent-docs/adr/0137-parallel.md"), Adr);
    assert_eq!(classify::kind("agent-docs/adr/2026-10-02-parallel.md"), Adr);
    assert_eq!(classify::kind("agent-docs/adr/README.md"), Code);
    assert_eq!(classify::kind("agent-docs/adr/sub/0137-x.md"), Code);
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
    write(repo, "agent-docs/adr/0137-second.md", "# Second\n");
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

/// 付記（union の範囲）: task ごとの進捗ファイルは front matter（`status:` 等）が書き換わるので
/// `.gitattributes` の `merge=union` を使わない。実リポジトリの `.gitattributes` を一時 repo に
/// 写し、両側が同じ `status:` 行を別の値に変えても `git merge` が衝突して止まり（黙って片方の値が
/// もう片方を消さない）、records resolver 経由で `Resolution::NeedsHuman` に回ることを確かめる。
#[test]
fn front_matter_status_conflict_requests_human_and_is_not_silently_merged() {
    let attrs_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.gitattributes");
    let attrs = fs::read_to_string(&attrs_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", attrs_path.display()));
    assert!(
        !attrs.contains("agent-docs/progress/"),
        ".gitattributes must not set merge=union under agent-docs/progress/ \
         (front matter is rewritten there, not append-only): {attrs}"
    );

    let tmp = new_repo();
    let repo = tmp.path();
    fs::write(repo.join(".gitattributes"), &attrs).unwrap();
    let path = "agent-docs/progress/2026-10-03-example/leaf.md";
    let body = "---\ntitle: 例\ntasks: [example]\nstatus: running\nupdated: 2026-10-03\n---\n\n# 例\n\n本文\n";
    write(repo, path, body);
    let base = commit(repo, "base (front matter)");
    command(repo, &["branch", "source"]);
    write(repo, path, &body.replace("status: running", "status: done"));
    let target_sha = commit(repo, "target: status done");
    command(repo, &["checkout", "-q", "source"]);
    write(
        repo,
        path,
        &body.replace("status: running", "status: blocked"),
    );
    let source_sha = commit(repo, "source: status blocked");
    command(repo, &["checkout", "-q", "target"]);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "--no-commit", "source"])
        .output()
        .unwrap();
    assert!(
        !status.status.success(),
        "git merge must conflict on the shared status: line instead of silently merging"
    );
    assert!(
        fs::read_to_string(repo.join(path))
            .unwrap()
            .contains("<<<<<<<"),
        "expected conflict markers left in the working tree"
    );

    let ctx = ResolveContext {
        target_branch: "target".into(),
        target_sha,
        source_branch: "source".into(),
        source_sha,
        merge_base: Some(base),
        generated_command: None,
    };
    let Resolution::NeedsHuman { request } = resolve(repo, &ctx).unwrap() else {
        panic!("front matter changed on both sides must need human");
    };
    assert_eq!(request.conflict_files, [path]);
    assert!(request.reason.contains("記録の既存行が両側で変わった"));
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

/// 日付名 ADR（ADR-0128 D5）は番号を持たない。両側がそれぞれ別の日付名 ADR を足しただけの、衝突しない
/// merge で統合の依頼を出してはならない（2026-10-04 本番の誤検出: `2026-…` の先頭 4 桁を番号 2026 と
/// 誤認して 4 本の日付 ADR を「日付名 ADR の衝突」にした）。
#[test]
fn dated_adrs_added_on_both_sides_without_conflict_request_nothing() {
    let tmp = new_repo();
    let repo = tmp.path();
    write(repo, "README.md", "base\n");
    commit(repo, "base");
    write(
        repo,
        "agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md",
        "# ADR 2026-10-02-parallel-integration-auto-resolve\n",
    );
    write(
        repo,
        "agent-docs/adr/2026-10-03-ownerless-running-runs.md",
        "# ADR 2026-10-03-ownerless-running-runs\n",
    );
    let target_sha = commit(repo, "target: two dated adrs");
    command(repo, &["checkout", "-q", "-b", "source", "HEAD~1"]);
    write(
        repo,
        "agent-docs/adr/2026-10-04-release-notes.md",
        "# ADR 2026-10-04-release-notes\n",
    );
    let source_sha = commit(repo, "source: another dated adr");
    command(repo, &["checkout", "-q", "target"]);
    let merge_base = command(repo, &["merge-base", "target", "source"]);
    command(repo, &["merge", "--no-ff", "--no-edit", "source"]);
    let ctx = ResolveContext {
        target_branch: "target".into(),
        target_sha,
        source_branch: "source".into(),
        source_sha,
        merge_base: Some(merge_base),
        generated_command: None,
    };
    let resolution = resolve(repo, &ctx).unwrap();
    assert_eq!(
        resolution,
        Resolution::Resolved {
            actions: Vec::new()
        },
        "衝突の無い日付名 ADR の取り込みで依頼を出した"
    );
    assert!(classify::adr_number("agent-docs/adr/2026-10-04-release-notes.md").is_none());
    assert!(classify::adr_number("docs/adr/2026-10-04-release-notes.md").is_none());
    assert_eq!(
        classify::adr_number("agent-docs/adr/0137-parallel.md").as_deref(),
        Some("0137")
    );
}

/// merge base が target 先端（source が target を含む、fast-forward できる取り込み）: 取り込み側が
/// 日付名 ADR を足し、target にも日付名 ADR がある場合（本番 01M420EMSFS1VP5RWF2FGCV6XR の integrate-close
/// の形）でも、依頼を出さず `Resolved` になる。
#[test]
fn fast_forward_source_with_dated_adrs_requests_nothing() {
    let tmp = new_repo();
    let repo = tmp.path();
    write(repo, "README.md", "base\n");
    write(
        repo,
        "agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md",
        "# ADR 2026-10-02-parallel-integration-auto-resolve\n",
    );
    write(
        repo,
        "agent-docs/adr/2026-10-03-ownerless-running-runs.md",
        "# ADR 2026-10-03-ownerless-running-runs\n",
    );
    write(
        repo,
        "agent-docs/adr/2026-10-03-write-set-no-starvation.md",
        "# ADR 2026-10-03-write-set-no-starvation\n",
    );
    let target_sha = commit(repo, "target: dated adrs from main");
    command(repo, &["checkout", "-q", "-b", "source"]);
    write(
        repo,
        "agent-docs/adr/2026-10-04-release-notes.md",
        "# ADR 2026-10-04-release-notes\n",
    );
    write(repo, "crates/celeris/src/release_notes.rs", "// notes\n");
    let source_sha = commit(repo, "source: release notes");
    command(repo, &["checkout", "-q", "target"]);
    let merge_base = command(repo, &["merge-base", "target", "source"]);
    assert_eq!(merge_base, target_sha, "merge base は target 先端");
    command(repo, &["merge", "--no-ff", "--no-edit", "source"]);
    let ctx = ResolveContext {
        target_branch: "target".into(),
        target_sha,
        source_branch: "source".into(),
        source_sha,
        merge_base: Some(merge_base),
        generated_command: None,
    };
    let resolution = resolve(repo, &ctx).unwrap();
    assert_eq!(
        resolution,
        Resolution::Resolved {
            actions: Vec::new()
        },
        "merge base が target 先端の取り込みで依頼を出した"
    );
}

/// 許可リストの正本（scripts/dev/check-adr-numbers.sh も同じ file を読む）を一時 repo に写す。
const ALLOWED_LIST: &str = "# 既存の重複\n0078-browser-execution-capability.md\n0078-ssh-master-persist-independent-of-daemon.md\n";

/// `base` → target（`target_files`）と source（`source_edit`）に分け、source を `--no-ff` で衝突なしに merge する。
fn merge_clean(
    base: &[(&str, &str)],
    target_files: &[(&str, &str)],
    source_edit: impl FnOnce(&Path),
) -> (tempfile::TempDir, ResolveContext) {
    let tmp = new_repo();
    let repo = tmp.path();
    write(repo, "README.md", "base\n");
    for (path, body) in base {
        write(repo, path, body);
    }
    commit(repo, "base");
    command(repo, &["branch", "source"]);
    for (path, body) in target_files {
        write(repo, path, body);
    }
    let target_sha = if target_files.is_empty() {
        commit_empty_target(repo)
    } else {
        commit(repo, "target")
    };
    command(repo, &["checkout", "-q", "source"]);
    source_edit(repo);
    command(repo, &["add", "-A"]);
    command(repo, &["commit", "-q", "-m", "source"]);
    let source_sha = command(repo, &["rev-parse", "HEAD"]);
    command(repo, &["checkout", "-q", "target"]);
    let merge_base = command(repo, &["merge-base", "target", "source"]);
    command(repo, &["merge", "--no-ff", "--no-edit", "source"]);
    let ctx = ResolveContext {
        target_branch: "target".into(),
        target_sha,
        source_branch: "source".into(),
        source_sha,
        merge_base: Some(merge_base),
        generated_command: None,
    };
    (tmp, ctx)
}

/// 2026-10-04 本番（release c1b24fb6、task 01M42XH8AAW5RQRRJT43FFP8YP）の形: target は古く 0078 を持たず、
/// 取り込み側が許可リストにある 0078 の 2 本（main で既に並んでいる）を持ち込む。merge は衝突なしで通る。
/// 許可済みの重複なので依頼を出さず、どちらも動かさない。
#[test]
fn allowed_adr_duplicate_brought_in_by_source_requests_nothing() {
    let (tmp, ctx) = merge_clean(&[], &[], |repo| {
        write(repo, "scripts/dev/adr-allowed-duplicates.txt", ALLOWED_LIST);
        write(
            repo,
            "agent-docs/adr/0078-browser-execution-capability.md",
            "# ADR-0078: browser\n",
        );
        write(
            repo,
            "agent-docs/adr/0078-ssh-master-persist-independent-of-daemon.md",
            "# ADR-0078: ssh master\n",
        );
    });
    let repo = tmp.path();
    assert!(
        classify::classify(repo, &ctx.target_sha)
            .unwrap()
            .is_empty(),
        "許可済みの重複を番号重複として拾った"
    );
    assert_eq!(
        resolve(repo, &ctx).unwrap(),
        Resolution::Resolved {
            actions: Vec::new()
        },
        "許可済みの ADR 番号重複で依頼を出した"
    );
    assert!(
        repo.join("agent-docs/adr/0078-browser-execution-capability.md")
            .exists()
    );
    assert!(
        repo.join("agent-docs/adr/0078-ssh-master-persist-independent-of-daemon.md")
            .exists()
    );
}

/// 両側に既にある重複（許可リストに無い）: target にも source にも同じ 2 本があり、取り込み側は旧
/// `docs/adr/` から `agent-docs/adr/` へ移した（ADR-0128 の配置換え）・本文を変えただけ。新しく生じた
/// 重複ではないので依頼を出さない。
#[test]
fn duplicate_already_on_both_sides_requests_nothing() {
    let (tmp, ctx) = merge_clean(
        &[
            ("docs/adr/0090-alpha.md", "# ADR-0090: alpha\n"),
            ("docs/adr/0090-beta.md", "# ADR-0090: beta\n"),
            ("crates/task-core/migrations/0010_a.sql", "select 1;\n"),
            ("crates/task-core/migrations/0010_b.sql", "select 2;\n"),
        ],
        &[("src/lib.rs", "// target\n")],
        |repo| {
            fs::create_dir_all(repo.join("agent-docs")).unwrap();
            command(repo, &["mv", "docs/adr", "agent-docs/adr"]);
            write(
                repo,
                "agent-docs/adr/0090-alpha.md",
                "# ADR-0090: alpha\n\n追記\n",
            );
            write(
                repo,
                "crates/task-core/migrations/0010_b.sql",
                "select 22;\n",
            );
        },
    );
    let repo = tmp.path();
    assert!(
        classify::classify(repo, &ctx.target_sha)
            .unwrap()
            .is_empty(),
        "両側に既にある重複を拾った"
    );
    assert_eq!(
        resolve(repo, &ctx).unwrap(),
        Resolution::Resolved {
            actions: Vec::new()
        },
        "両側に既にある重複で依頼を出した"
    );
    assert!(repo.join("agent-docs/adr/0090-alpha.md").exists());
    assert!(repo.join("agent-docs/adr/0090-beta.md").exists());
}

/// 新しく生じた重複は従来どおり: 許可済みの組に取り込み側が 3 本目の 0078 を足したら日付名へ移し、
/// 取り込み側同士が許可リストに無い同じ番号を足したら人に回す。
#[test]
fn new_duplicates_are_still_renamed_or_requested() {
    let allowed = [
        ("scripts/dev/adr-allowed-duplicates.txt", ALLOWED_LIST),
        (
            "agent-docs/adr/0078-browser-execution-capability.md",
            "# ADR-0078: browser\n",
        ),
        (
            "agent-docs/adr/0078-ssh-master-persist-independent-of-daemon.md",
            "# ADR-0078: ssh master\n",
        ),
    ];
    let (tmp, ctx) = merge_clean(&allowed, &[("src/lib.rs", "// target\n")], |repo| {
        write(repo, "agent-docs/adr/0078-third.md", "# ADR-0078: third\n");
    });
    let repo = tmp.path();
    let Resolution::Resolved { actions } = resolve(repo, &ctx).unwrap() else {
        panic!("新しい 3 本目の 0078 を振り直さなかった");
    };
    assert!(actions.iter().any(|a| a.detail.contains("git mv")));
    assert!(!repo.join("agent-docs/adr/0078-third.md").exists());
    assert!(
        repo.join("agent-docs/adr/0078-browser-execution-capability.md")
            .exists()
    );
    assert!(
        repo.join("agent-docs/adr/0078-ssh-master-persist-independent-of-daemon.md")
            .exists()
    );

    let (tmp, ctx) = merge_clean(&allowed, &[("src/lib.rs", "// target\n")], |repo| {
        write(repo, "agent-docs/adr/0091-one.md", "# ADR-0091: one\n");
        write(repo, "agent-docs/adr/0091-two.md", "# ADR-0091: two\n");
    });
    let Resolution::NeedsHuman { request } = resolve(tmp.path(), &ctx).unwrap() else {
        panic!("取り込み側同士の新しい重複を依頼にしなかった");
    };
    assert!(request.reason.contains("0091"), "{}", request.reason);
}

/// resolver と check-adr-numbers.sh は同じ正本を読む: repo の許可リストに本番の 0078 の組が載っている。
#[test]
fn repository_allowlist_is_the_single_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let allowed = classify::allowed_adr_duplicates(&root);
    let pair = [
        "agent-docs/adr/0078-browser-execution-capability.md".to_string(),
        "docs/adr/0078-ssh-master-persist-independent-of-daemon.md".to_string(),
    ];
    assert!(classify::allowed_adr_group(&allowed, &pair), "{allowed:?}");
    let script = fs::read_to_string(root.join("scripts/dev/check-adr-numbers.sh")).unwrap();
    assert!(script.contains(classify::ALLOWED_ADR_DUPLICATES_FILE));
    assert!(!script.contains("0078-browser-execution-capability.md"));
}
