use super::*;

pub(crate) fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("mkdir");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
    ] {
        let out = git(dir, &args).expect("git");
        assert!(out.ok, "git {args:?}: {}", out.stderr);
    }
    std::fs::write(dir.join("README.md"), b"hello\n").expect("write");
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "first"]] {
        let out = git(dir, &args).expect("git");
        assert!(out.ok, "git {args:?}: {}", out.stderr);
    }
}

pub(crate) fn commit(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), name.as_bytes()).expect("write");
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", name]] {
        let out = git(dir, &args).expect("git");
        assert!(out.ok, "git {args:?}: {}", out.stderr);
    }
    rev_parse(dir, "HEAD").expect("head")
}

#[test]
fn a_plain_directory_is_not_a_git_repository() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(!is_git_repo(dir.path()));
    init_repo(dir.path());
    assert!(is_git_repo(dir.path()));
}

/// base の既定は `main`（ADR-0041 D1）。
#[test]
fn the_base_is_main_when_there_is_no_current_release() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let base = resolve_base(dir.path(), None).expect("base");
    assert_eq!(base.kind, BaseKind::Main);
    assert_eq!(
        base.sha,
        rev_parse(dir.path(), "refs/heads/main").expect("main")
    );
}

/// `main` が無いリポジトリでは `HEAD`。
#[test]
fn the_base_is_head_when_there_is_no_main() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let out = git(dir.path(), &["checkout", "-q", "-b", "trunk"]).expect("git");
    assert!(out.ok, "{}", out.stderr);
    let out = git(dir.path(), &["branch", "-q", "-D", "main"]).expect("git");
    assert!(out.ok, "{}", out.stderr);
    let base = resolve_base(dir.path(), None).expect("base");
    assert_eq!(base.kind, BaseKind::Head);
}

/// 本番の `current` が `main` の子孫なら、そちらを base にする（本番より古いコードから分岐させない）。
#[test]
fn the_base_is_the_current_release_when_it_is_a_descendant_of_main() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let main_sha = rev_parse(dir.path(), "refs/heads/main").expect("main");
    let out = git(dir.path(), &["checkout", "-q", "-b", "ahead"]).expect("git");
    assert!(out.ok, "{}", out.stderr);
    let ahead = commit(dir.path(), "ahead.txt");
    let base = resolve_base(dir.path(), Some(&ahead)).expect("base");
    assert_eq!(base.kind, BaseKind::Current);
    assert_eq!(base.sha, ahead);
    assert_ne!(base.sha, main_sha);
}

/// `current` が `main` の祖先（本番が古い）なら `main` のまま。
#[test]
fn the_base_stays_main_when_the_current_release_is_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let first = rev_parse(dir.path(), "HEAD").expect("head");
    let second = commit(dir.path(), "second.txt");
    let base = resolve_base(dir.path(), Some(&first)).expect("base");
    assert_eq!(base.kind, BaseKind::Main);
    assert_eq!(base.sha, second);
}

fn worktree_for(repo: &Path, root: &Path, id: &str) -> LocalWorktree {
    let task_dir = root.join(id);
    LocalWorktree {
        dir: task_dir.join(WORKTREE_DIR_NAME),
        task_dir,
        repo: repo.to_path_buf(),
        branch: format!("{DEFAULT_BRANCH_PREFIX}{id}"),
        base: resolve_base(repo, None).expect("base"),
    }
}

/// ADR-0041 D1: やり直しの run は worktree を**作り直さない**（未コミットの作業を消さない）。
#[tokio::test]
async fn a_retry_reuses_the_existing_worktree() {
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let root = tempfile::tempdir().expect("tempdir");
    let wt = worktree_for(repo.path(), root.path(), "01TASK");
    wt.ensure().await.expect("first");
    std::fs::write(wt.dir.join("work-in-progress"), b"x").expect("write");
    wt.ensure().await.expect("retry");
    assert!(
        wt.dir.join("work-in-progress").is_file(),
        "やり直しで作業を消さない"
    );
    assert_eq!(
        git(repo.path(), &["worktree", "list"])
            .expect("git")
            .stdout
            .lines()
            .count(),
        2
    );
}

/// ブランチだけ残っている（前の run の後で worktree を消した）ときは、そのブランチで作り直す。
#[tokio::test]
async fn a_removed_worktree_is_recreated_on_the_same_branch() {
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let root = tempfile::tempdir().expect("tempdir");
    let wt = worktree_for(repo.path(), root.path(), "01TASK");
    wt.ensure().await.expect("first");
    let committed = commit(&wt.dir, "done.txt");
    assert_eq!(wt.remove_if_clean(), CleanupOutcome::Removed);
    assert!(!wt.dir.exists());
    wt.ensure().await.expect("again");
    // ブランチの先端（前の run のコミット）から再開する。base には戻らない。
    assert_eq!(rev_parse(&wt.dir, "HEAD").expect("head"), committed);
    assert!(wt.dir.join("done.txt").is_file());
}

/// 終端の後片付け: クリーンなら消す、汚れていれば残す。ブランチは消さない。
#[tokio::test]
async fn cleanup_removes_a_clean_worktree_and_keeps_a_dirty_one() {
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let root = tempfile::tempdir().expect("tempdir");
    let dirty = worktree_for(repo.path(), root.path(), "01DIRTY");
    dirty.ensure().await.expect("ensure");
    std::fs::write(dirty.dir.join("untracked"), b"x").expect("write");
    assert_eq!(dirty.remove_if_clean(), CleanupOutcome::Dirty);
    assert!(dirty.dir.join("untracked").is_file());

    let clean = worktree_for(repo.path(), root.path(), "01CLEAN");
    clean.ensure().await.expect("ensure");
    assert_eq!(clean.remove_if_clean(), CleanupOutcome::Removed);
    assert!(!clean.dir.exists());
    assert!(
        branch_exists(repo.path(), &clean.branch),
        "ブランチは消さない"
    );
    assert_eq!(clean.remove_if_clean(), CleanupOutcome::AlreadyGone);
}

/// `current/manifest.json` の `sha` を読む（`releases_dir` の**親**にある。ADR-0040 D6）。
#[test]
fn the_current_release_sha_comes_from_the_manifest_next_to_releases() {
    let home = tempfile::tempdir().expect("tempdir");
    let releases = home.path().join("releases");
    std::fs::create_dir_all(&releases).expect("mkdir");
    assert_eq!(current_release_sha(&releases), None);
    let current = home.path().join("current");
    std::fs::create_dir_all(&current).expect("mkdir");
    std::fs::write(
        current.join("manifest.json"),
        br#"{"sha":"abc123def456789","sha12":"abc123def456"}"#,
    )
    .expect("write");
    assert_eq!(
        current_release_sha(&releases).as_deref(),
        Some("abc123def456789")
    );
    std::fs::write(current.join("manifest.json"), b"not json").expect("write");
    assert_eq!(current_release_sha(&releases), None);
}
