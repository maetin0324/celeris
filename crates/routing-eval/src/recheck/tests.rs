use super::*;
use serde_json::json;
use tempfile::{TempDir, tempdir};

struct Repo {
    _dir: TempDir,
    path: PathBuf,
    base: String,
    head: String,
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args).expect("git is available");
    assert!(out.ok, "git {args:?}: {}", out.stderr);
    out.stdout.trim().to_owned()
}

/// base（`notes/flag.txt` が無い）と head（ある）の 2 commit を持つ一時リポジトリ。
fn repo_fixture() -> Repo {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let hooks = dir.path().join("hooks");
    fs::create_dir_all(&path).expect("mkdir repo");
    fs::create_dir_all(&hooks).expect("mkdir hooks");
    git_ok(&path, &["init", "-q", "-b", "main"]);
    git_ok(&path, &["config", "user.email", "eval@example.com"]);
    git_ok(&path, &["config", "user.name", "eval"]);
    git_ok(&path, &["config", "commit.gpgsign", "false"]);
    git_ok(
        &path,
        &["config", "core.hooksPath", hooks.to_str().expect("utf-8")],
    );
    fs::create_dir_all(path.join("notes")).expect("mkdir notes");
    fs::write(path.join("notes/readme.md"), b"base\n").expect("write");
    git_ok(&path, &["add", "-A"]);
    git_ok(&path, &["commit", "-q", "-m", "base"]);
    let base = git_ok(&path, &["rev-parse", "HEAD"]);
    fs::write(path.join("notes/flag.txt"), b"head\n").expect("write");
    git_ok(&path, &["add", "-A"]);
    git_ok(&path, &["commit", "-q", "-m", "head"]);
    let head = git_ok(&path, &["rev-parse", "HEAD"]);
    Repo {
        _dir: dir,
        path,
        base,
        head,
    }
}

fn problem(
    id: &str,
    command: &str,
    base: Option<&str>,
    head: Option<&str>,
) -> crate::manifest::Problem {
    crate::manifest::Problem {
        id: id.into(),
        task_id: format!("task-{id}"),
        work_unit_id: id.into(),
        created_at: "2026-09-01T00:00:00Z".into(),
        genre: "coding".into(),
        kind: "implement".into(),
        command: command.into(),
        prompt: "Fixture problem description".into(),
        split: "dev".into(),
        stratum: "coding:implement".into(),
        subset_12: false,
        head_commit: head.map(str::to_owned),
        base_commit: base.map(str::to_owned),
        base_commit_from_parent: false,
        features_sha256: "features-sha".into(),
        features: json!({"schema": "celeris.task-features/1"}),
        sha256: format!("sha-{id}"),
    }
}

fn write_manifest(dir: &Path, problems: Vec<crate::manifest::Problem>) -> PathBuf {
    let manifest = Manifest {
        schema: crate::MANIFEST_SCHEMA.into(),
        version: "v1".into(),
        cutoff: "2026-10-05".into(),
        seed: 7,
        problems,
        refill_rejected: Vec::new(),
        sha256: "manifest-sha".into(),
    };
    let path = dir.join("manifest.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).expect("serialize"),
    )
    .expect("write manifest");
    path
}

fn options(manifest: &Path, repo: &Path, root: &Path) -> Options {
    Options {
        manifest: manifest.to_path_buf(),
        repo: repo.to_path_buf(),
        out: Some(root.join("recheck.json")),
        timeout: Duration::from_secs(120),
        worktree_root: Some(root.join("wt-root")),
        checked_at: Some("2026-10-12T00:00:00Z".into()),
        limit: None,
    }
}

fn reason_of(record: &Recheck, id: &str) -> Option<String> {
    record
        .problems
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| p.reason.clone())
}

#[test]
fn recheck_adopts_a_base_failure_and_head_success_and_excludes_the_rest_with_reasons() {
    let repo = repo_fixture();
    let dir = tempdir().expect("tempdir");
    let unknown = "0123456789abcdef0123456789abcdef01234567";
    let manifest = write_manifest(
        dir.path(),
        vec![
            problem(
                "p-adopted",
                "test -f notes/flag.txt",
                Some(&repo.base),
                Some(&repo.head),
            ),
            problem("p-noop", "true", Some(&repo.base), Some(&repo.head)),
            problem(
                "p-head-fails",
                "test -f notes/never.txt",
                Some(&repo.base),
                Some(&repo.head),
            ),
            problem("p-no-command", "", Some(&repo.base), Some(&repo.head)),
            problem(
                "p-no-head",
                "test -f notes/flag.txt",
                Some(&repo.base),
                None,
            ),
            problem(
                "p-no-base",
                "test -f notes/flag.txt",
                None,
                Some(&repo.head),
            ),
            problem(
                "p-unknown-commit",
                "test -f notes/flag.txt",
                Some(&repo.base),
                Some(unknown),
            ),
        ],
    );
    let record = recheck(&options(&manifest, &repo.path, dir.path())).expect("recheck");

    assert_eq!(record.adopted, 1, "{record:?}");
    assert_eq!(record.excluded, 6);
    assert_eq!(record.manifest_sha256, "manifest-sha");
    assert_eq!(record.schema, RECHECK_SCHEMA);
    let adopted = record
        .problems
        .iter()
        .find(|p| p.id == "p-adopted")
        .expect("adopted row");
    assert!(adopted.adopted);
    assert_eq!(adopted.reason, None);
    assert_eq!(adopted.base_exit, Some(1), "base must fail");
    assert_eq!(adopted.head_exit, Some(0), "head must pass");
    assert_eq!(
        reason_of(&record, "p-noop").as_deref(),
        Some(REASON_NOOP_BASE_PASSES)
    );
    assert_eq!(
        reason_of(&record, "p-head-fails").as_deref(),
        Some(REASON_HEAD_FAILS)
    );
    assert_eq!(
        reason_of(&record, "p-no-command").as_deref(),
        Some(REASON_NO_COMMAND)
    );
    assert_eq!(
        reason_of(&record, "p-no-head").as_deref(),
        Some(REASON_NO_HEAD_COMMIT)
    );
    assert_eq!(
        reason_of(&record, "p-no-base").as_deref(),
        Some(REASON_NO_BASE_COMMIT)
    );
    assert_eq!(
        reason_of(&record, "p-unknown-commit").as_deref(),
        Some(REASON_COMMIT_NOT_IN_REPO)
    );
    assert_eq!(
        record.excluded_by_reason.get(REASON_NOOP_BASE_PASSES),
        Some(&1)
    );

    // worktree は残さない（$TMPDIR の下も消す）。
    assert!(
        !dir.path().join("wt-root").exists(),
        "worktree root is gone"
    );
    assert_eq!(
        git_ok(&repo.path, &["worktree", "list"]).lines().count(),
        1,
        "no leftover worktree registration"
    );
    // 記録は manifest の `recheck` 欄に入り、問題集合と sha256 は変わらない。
    let raw: Value =
        serde_json::from_slice(&fs::read(&manifest).expect("read")).expect("parse manifest");
    assert_eq!(raw["sha256"], "manifest-sha");
    assert_eq!(raw["recheck"]["adopted"], 1);
    let again: Manifest = serde_json::from_value(raw).expect("manifest still parses");
    assert_eq!(again.problems.len(), 7);
    // `--out` の写しも同じ内容。
    let copy: Recheck =
        serde_json::from_slice(&fs::read(dir.path().join("recheck.json")).expect("read copy"))
            .expect("parse copy");
    assert_eq!(copy, record);
}

#[test]
fn recheck_excludes_a_check_that_does_not_finish_in_time() {
    let repo = repo_fixture();
    let dir = tempdir().expect("tempdir");
    let manifest = write_manifest(
        dir.path(),
        vec![problem(
            "p-slow",
            "sleep 30",
            Some(&repo.base),
            Some(&repo.head),
        )],
    );
    let mut opts = options(&manifest, &repo.path, dir.path());
    opts.timeout = Duration::from_secs(1);
    let record = recheck(&opts).expect("recheck");
    assert_eq!(record.adopted, 0);
    assert_eq!(
        reason_of(&record, "p-slow").as_deref(),
        Some(REASON_TIMED_OUT)
    );
    assert!(!dir.path().join("wt-root").exists());
}

#[test]
fn recheck_does_not_let_the_base_run_pollute_the_head_run() {
    let repo = repo_fixture();
    let dir = tempdir().expect("tempdir");
    // base の実行が file を作っても、head は別の worktree なので影響を受けない。
    let manifest = write_manifest(
        dir.path(),
        vec![problem(
            "p-dirty",
            "touch notes/made-by-check && test -f notes/never.txt",
            Some(&repo.base),
            Some(&repo.head),
        )],
    );
    let record = recheck(&options(&manifest, &repo.path, dir.path())).expect("recheck");
    assert_eq!(
        reason_of(&record, "p-dirty").as_deref(),
        Some(REASON_HEAD_FAILS)
    );
    assert_eq!(
        git_ok(&repo.path, &["status", "--porcelain"]),
        "",
        "the source repository is untouched"
    );
}

#[test]
fn recheck_refuses_a_foreign_schema_and_a_directory_that_is_not_a_repository() {
    let repo = repo_fixture();
    let dir = tempdir().expect("tempdir");
    let manifest = write_manifest(
        dir.path(),
        vec![problem(
            "p-adopted",
            "true",
            Some(&repo.base),
            Some(&repo.head),
        )],
    );
    let mut opts = options(&manifest, &repo.path, dir.path());
    opts.repo = dir.path().join("not-a-repo");
    fs::create_dir_all(&opts.repo).expect("mkdir");
    assert!(
        recheck(&opts)
            .unwrap_err()
            .contains("is not a git repository"),
        "a plain directory is rejected"
    );
    let mut raw: Value =
        serde_json::from_slice(&fs::read(&manifest).expect("read")).expect("parse");
    raw["schema"] = json!("celeris.something-else/1");
    fs::write(&manifest, serde_json::to_vec(&raw).expect("serialize")).expect("write");
    let opts = options(&manifest, &repo.path, dir.path());
    assert!(
        recheck(&opts).unwrap_err().contains("manifest schema"),
        "another schema is rejected"
    );
}

#[test]
fn the_command_line_needs_a_manifest_and_a_repository_and_takes_a_timeout() {
    assert!(parse(&[]).unwrap_err().contains("--manifest"));
    assert!(
        parse(&["--manifest".into(), "m.json".into()])
            .unwrap_err()
            .contains("--repo")
    );
    let opts = parse(&[
        "--manifest".into(),
        "m.json".into(),
        "--repo".into(),
        "/r".into(),
        "--timeout".into(),
        "42".into(),
        "--limit".into(),
        "3".into(),
    ])
    .expect("parse");
    assert_eq!(opts.timeout, Duration::from_secs(42));
    assert_eq!(opts.limit, Some(3));
    assert_eq!(opts.out, None);
    assert!(
        parse(&[
            "--manifest".into(),
            "m.json".into(),
            "--repo".into(),
            "/r".into(),
            "--timeout".into(),
            "soon".into(),
        ])
        .unwrap_err()
        .contains("--timeout")
    );
}

#[test]
fn the_tail_keeps_the_end_of_a_long_text() {
    assert_eq!(tail("  hello  ", 10), "hello");
    assert_eq!(tail("abcdefgh", 3), "fgh");
}

#[test]
fn the_usage_explains_the_discrimination_rule_and_the_exclusion_reasons() {
    run(&["--help".into()]).expect("usage exits cleanly");
    for needle in [
        "--manifest",
        "--repo",
        "$TMPDIR",
        "noop_check_passes_at_base",
        "check_fails_at_head_commit",
        "no_base_commit",
        "no_command_check",
    ] {
        assert!(USAGE.contains(needle), "{needle} is missing from the usage");
    }
}

/// CLI 経由（worktree は既定の `$TMPDIR` の下）で manifest の `recheck` 欄が書かれる。
#[test]
fn the_command_line_writes_the_record_into_the_manifest_and_reports_counts() {
    let repo = repo_fixture();
    let dir = tempdir().expect("tempdir");
    let manifest = write_manifest(
        dir.path(),
        vec![
            problem(
                "p-adopted",
                "test -f notes/flag.txt",
                Some(&repo.base),
                Some(&repo.head),
            ),
            problem("p-noop", "true", Some(&repo.base), Some(&repo.head)),
        ],
    );
    let args: Vec<String> = [
        "--manifest",
        manifest.to_str().expect("utf-8"),
        "--repo",
        repo.path.to_str().expect("utf-8"),
        "--timeout",
        "60",
        "--checked-at",
        "2026-10-12T00:00:00Z",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    run(&args).expect("cli recheck");

    let raw: Value = serde_json::from_slice(&fs::read(&manifest).expect("read")).expect("parse");
    assert_eq!(raw["recheck"]["schema"], RECHECK_SCHEMA);
    assert_eq!(raw["recheck"]["adopted"], 1);
    assert_eq!(raw["recheck"]["excluded"], 1);
    assert_eq!(raw["recheck"]["checked_at"], "2026-10-12T00:00:00Z");
    assert_eq!(
        raw["recheck"]["excluded_by_reason"][REASON_NOOP_BASE_PASSES],
        json!(1)
    );
    assert_eq!(
        git_ok(&repo.path, &["worktree", "list"]).lines().count(),
        1,
        "the $TMPDIR worktrees are removed"
    );
}
