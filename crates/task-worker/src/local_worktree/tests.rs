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

/// Phase R6-3: submodule を持つリポジトリのタスクの worktree は、submodule まで初期化される
/// （再利用でも落ちない。`.gitmodules` が無ければ何もしない）。
#[tokio::test]
async fn the_worktree_initialises_submodules_and_reuse_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sub = tmp.path().join("sub");
    init_repo(&sub);
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let url = sub.to_string_lossy().into_owned();
    let out = git(
        &repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &url,
            "lib/sub",
        ],
    )
    .expect("git");
    assert!(out.ok, "{}", out.stderr);
    let out = git(&repo, &["commit", "-q", "-m", "submodule"]).expect("git");
    assert!(out.ok, "{}", out.stderr);

    let root = tmp.path().join("ws");
    let wt = worktree_for(&repo, &root, "01R63LOCAL");
    // 既定の git はローカルパス（file）の submodule の clone を拒む（git >= 2.38.1）。R7-4 から、その失敗は
    // 準備を止めない（警告）。worktree はでき、submodule は空のまま。
    wt.ensure()
        .await
        .expect("a failed submodule is a warning, not a prepare error");
    assert!(wt.dir.join("README.md").is_file());
    assert!(!wt.dir.join("lib/sub/README.md").exists());
    let report = init_submodules(&wt.dir)
        .expect("warning only")
        .expect("still uninitialised");
    assert_eq!(report.initialised, 0);
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].0, "lib/sub");
    assert!(!report.failed[0].1.is_empty());
    // file を許すと初期化される（本番の submodule は ssh / https なのでこの前置きは要らない）。
    let count = init_submodules_with(&wt.dir, &["-c", "protocol.file.allow=always"]).expect("init");
    assert_eq!(
        count,
        Some(SubmoduleInit {
            initialised: 1,
            failed: Vec::new()
        })
    );
    assert!(
        wt.dir.join("lib/sub/README.md").is_file(),
        "submodule populated"
    );
    // 再利用: 初期化済みなので何もしない（落ちない、ネットワークにも出ない）。
    assert_eq!(
        init_submodules(&wt.dir).expect("again"),
        None,
        "already initialised"
    );
    wt.ensure().await.expect("reuse");
    assert!(wt.dir.join("lib/sub/README.md").is_file());

    let plain = tempfile::tempdir().expect("tempdir");
    init_repo(plain.path());
    assert_eq!(init_submodules(plain.path()).expect("no gitmodules"), None);
}

/// Phase R7-4（本番 2026-09-30、sirius の BenchFS）: 固定した commit が remote に無い submodule
/// （`ior_integration/ior` の push していない commit）が 1 つあっても、ほかの submodule は初期化され、
/// 準備は通る。失敗した submodule は path と `fatal:` の行で報告される。
#[tokio::test]
async fn one_unfetchable_submodule_does_not_stop_the_others() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let good = tmp.path().join("good");
    let bad = tmp.path().join("bad");
    init_repo(&good);
    init_repo(&bad);
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    for (url, path) in [(&bad, "ior"), (&good, "lib/good")] {
        let url = url.to_string_lossy().into_owned();
        let out = git(
            &repo,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &url,
                path,
            ],
        )
        .expect("git");
        assert!(out.ok, "{}", out.stderr);
    }
    // `ior` の中で remote に無い commit を作り、上位はそれを固定する。
    let ior = repo.join("ior");
    for args in [
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
    ] {
        assert!(git(&ior, &args).expect("git").ok);
    }
    commit(&ior, "local-only");
    let out = git(&repo, &["add", "-A"]).expect("git");
    assert!(out.ok, "{}", out.stderr);
    let out = git(&repo, &["commit", "-q", "-m", "submodules"]).expect("git");
    assert!(out.ok, "{}", out.stderr);

    let root = tmp.path().join("ws");
    let wt = worktree_for(&repo, &root, "01R74LOCAL");
    wt.ensure_tree_blocking().expect("worktree");
    let report = init_submodules_with(&wt.dir, &["-c", "protocol.file.allow=always"])
        .expect("best-effort: not an error")
        .expect("something to do");
    assert_eq!(report.initialised, 1, "{report:?}");
    assert!(
        wt.dir.join("lib/good/README.md").is_file(),
        "good one populated"
    );
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].0, "ior");
    assert!(report.failed[0].1.starts_with("fatal:"), "{report:?}");
    // 再実行しても落ちない。失敗した ior は clone までは済んで行頭が `-` でなくなるので、試し直さない（None）。
    assert_eq!(
        init_submodules_with(&wt.dir, &["-c", "protocol.file.allow=always"]).expect("again"),
        None
    );
    wt.ensure().await.expect("ensure is not a prepare error");
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
