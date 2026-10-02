use super::*;

fn git_must(dir: &Path, args: &[&str]) {
    let out = git(dir, args, GIT_TIMEOUT).unwrap_or_else(|| panic!("git {args:?} did not start"));
    assert!(out.ok, "git {args:?}: {}", out.stderr);
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
    git_must(dir, &["init", "-q", "-b", "main"]);
    git_must(dir, &["config", "user.email", "t@example.com"]);
    git_must(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("README.md"), b"hello\n").unwrap_or_else(|e| panic!("write: {e}"));
    git_must(dir, &["add", "-A"]);
    git_must(dir, &["commit", "-q", "-m", "first"]);
}

fn add_worktree(repo: &Path, dir: &Path, branch: &str) {
    let path = dir.to_string_lossy().into_owned();
    git_must(repo, &["worktree", "add", "-b", branch, &path, "main"]);
    git_must(dir, &["config", "user.email", "t@example.com"]);
    git_must(dir, &["config", "user.name", "t"]);
}

/// ADR-0043 D5: コミットとコミットしていない変更、追跡外のファイルが 1 つの一覧になる。
#[test]
fn the_changes_of_a_worktree_cover_commits_working_tree_and_untracked_files() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");

    // 1 コミット + 未コミットの変更 + 追跡外のファイル。
    std::fs::write(tree.join("src.txt"), b"a\nb\nc\n").unwrap_or_else(|e| panic!("{e}"));
    git_must(&tree, &["add", "-A"]);
    git_must(&tree, &["commit", "-q", "-m", "add src"]);
    std::fs::write(tree.join("README.md"), b"hello\nmore\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(tree.join("new.txt"), b"x\ny\n").unwrap_or_else(|e| panic!("{e}"));

    let c = changes(&repo, Some(&tree), "celeris/01TASK", "main", None);
    assert!(!c.missing);
    assert_eq!(c.ahead, 1, "コミットは 1 つ");
    assert!(c.dirty, "未コミットの変更がある");
    let paths: Vec<&str> = c.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["README.md", "new.txt", "src.txt"],
        "{:?}",
        c.files
    );
    let src = c
        .files
        .iter()
        .find(|f| f.path == "src.txt")
        .unwrap_or_else(|| panic!("src"));
    assert_eq!(
        (src.status.as_str(), src.additions, src.deletions),
        ("A", 3, 0)
    );
    let untracked = c
        .files
        .iter()
        .find(|f| f.path == "new.txt")
        .unwrap_or_else(|| panic!("new"));
    assert_eq!((untracked.status.as_str(), untracked.additions), ("?", 2));
    assert_eq!(
        c.stat,
        DiffStat {
            files: 3,
            additions: 6,
            deletions: 0
        }
    );
    assert_eq!(c.base.len(), 40, "base は完全な sha");

    // 1 ファイルの diff。
    let diff = file_diff(
        &repo,
        Some(&tree),
        "celeris/01TASK",
        "main",
        None,
        "src.txt",
    )
    .unwrap_or_else(|| panic!("diff"));
    assert!(diff.diff.contains("+a"), "{}", diff.diff);
    assert!(!diff.truncated);
    // 追跡外のファイルも `--no-index` で出る。
    let untracked_diff = file_diff(
        &repo,
        Some(&tree),
        "celeris/01TASK",
        "main",
        None,
        "new.txt",
    )
    .unwrap_or_else(|| panic!("diff"));
    assert!(
        untracked_diff.diff.contains("+x"),
        "{}",
        untracked_diff.diff
    );
}

/// worktree を消してもブランチが残っていれば、元のリポジトリから差分が引ける。無ければ `missing`。
#[test]
fn changes_fall_back_to_the_branch_and_then_report_missing() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    std::fs::write(tree.join("src.txt"), b"a\n").unwrap_or_else(|e| panic!("{e}"));
    git_must(&tree, &["add", "-A"]);
    git_must(&tree, &["commit", "-q", "-m", "add src"]);

    let path = tree.to_string_lossy().into_owned();
    git_must(&repo, &["worktree", "remove", "--force", &path]);
    let c = changes(&repo, Some(&tree), "celeris/01TASK", "main", None);
    assert!(!c.missing, "ブランチは残っている");
    assert!(!c.dirty, "作業ツリーが無いので汚れようがない");
    assert_eq!(c.ahead, 1);
    assert_eq!(c.files.len(), 1);
    assert_eq!(c.files[0].path, "src.txt");
    assert!(
        file_diff(
            &repo,
            Some(&tree),
            "celeris/01TASK",
            "main",
            None,
            "src.txt"
        )
        .is_some_and(|d| d.diff.contains("+a"))
    );

    git_must(&repo, &["branch", "-D", "celeris/01TASK"]);
    let gone = changes(&repo, Some(&tree), "celeris/01TASK", "main", None);
    assert_eq!(
        gone,
        RepoChanges {
            missing: true,
            ..Default::default()
        }
    );
    assert_eq!(gone.ahead, 0);
    assert!(
        file_diff(
            &repo,
            Some(&tree),
            "celeris/01TASK",
            "main",
            None,
            "src.txt"
        )
        .is_none()
    );
}

/// コミットが 1 つも無いタスク（調査など）は `ahead = 0` で、ファイルも出ない。
#[test]
fn a_task_without_commits_is_zero_ahead() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    let c = changes(&repo, Some(&tree), "celeris/01TASK", "main", None);
    assert_eq!(c.ahead, 0);
    assert!(c.files.is_empty());
    assert!(!c.dirty);
    assert!(!c.missing);
}

/// 200 KiB で切る（切ったら印が立つ）。
#[test]
fn a_huge_diff_is_truncated_at_200_kib() {
    let small = truncate_diff("@@\n+a\n", "a.txt");
    assert!(!small.truncated);
    let huge = "x".repeat(MAX_DIFF_BYTES + 10);
    let cut = truncate_diff(&huge, "a.txt");
    assert!(cut.truncated);
    assert_eq!(cut.diff.len(), MAX_DIFF_BYTES);
    // 文字の境界で切る（多バイト文字を壊さない）。
    let multibyte = "あ".repeat(MAX_DIFF_BYTES);
    let cut = truncate_diff(&multibyte, "a.txt");
    assert!(cut.truncated);
    assert!(cut.diff.len() <= MAX_DIFF_BYTES);
}

#[test]
fn numstat_and_name_status_are_parsed() {
    assert_eq!(
        parse_numstat("3\t1\tsrc/a.rs\n-\t-\tlogo.png\nbroken\n"),
        vec![
            ("src/a.rs".to_string(), 3, 1, false),
            ("logo.png".to_string(), 0, 0, true)
        ]
    );
    assert_eq!(
        parse_name_status("M\tsrc/a.rs\nA\tnew.rs\nD\told.rs\n\n"),
        vec![
            ("src/a.rs".to_string(), "M".to_string()),
            ("new.rs".to_string(), "A".to_string()),
            ("old.rs".to_string(), "D".to_string()),
        ]
    );
}

#[test]
fn the_pr_url_and_view_json_are_parsed() {
    assert_eq!(
        parse_pr_url("https://github.com/o/r/pull/12\n"),
        Some(PrRef {
            number: 12,
            url: "https://github.com/o/r/pull/12".into()
        })
    );
    assert_eq!(
        parse_pr_url("Creating pull request...\nhttps://github.com/o/r/pull/7."),
        Some(PrRef {
            number: 7,
            url: "https://github.com/o/r/pull/7".into()
        })
    );
    assert_eq!(parse_pr_url("no url here"), None);
    let view = parse_pr_view(
        r#"{"state":"MERGED","mergedAt":"2026-09-19T00:00:00Z","url":"u","mergeable":null}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(view.state, "MERGED");
    assert_eq!(view.merged_at.as_deref(), Some("2026-09-19T00:00:00Z"));
    assert_eq!(view.mergeable, None);
    assert!(parse_pr_view("not json").is_err());
}

/// ADR-0043 D5 の `merge`: 人のチェックアウトが `main` を出していて綺麗なら fast-forward する。
#[test]
fn merge_fast_forwards_the_default_branch_when_it_is_checked_out_and_clean() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    std::fs::write(tree.join("src.txt"), b"a\n").unwrap_or_else(|e| panic!("{e}"));
    git_must(&tree, &["add", "-A"]);
    git_must(&tree, &["commit", "-q", "-m", "add src"]);

    let outcome = merge_into_default_branch(
        &repo,
        "celeris/01TASK",
        "main",
        &root.path().join("tmp/one"),
    );
    let sha = match outcome {
        MergeOutcome::Merged {
            sha,
            fast_forwarded,
        } => {
            assert!(fast_forwarded, "作業ツリーごと早送りする");
            sha
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        git_line(&repo, &["rev-parse", "refs/heads/main"]).as_deref(),
        Some(sha.as_str())
    );
    assert!(
        repo.join("src.txt").is_file(),
        "人の作業ツリーにも反映される"
    );
    assert!(
        !root.path().join("tmp/one").exists(),
        "一時 worktree は片付ける"
    );

    remove_worktree_and_branch(&repo, Some(&tree), "celeris/01TASK")
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!tree.exists());
    assert!(!git_ok(
        &repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/celeris/01TASK"
        ]
    ));
    // 2 回目も落ちない。
    remove_worktree_and_branch(&repo, Some(&tree), "celeris/01TASK")
        .unwrap_or_else(|e| panic!("{e}"));
}

/// 人が別のブランチを出していれば ref だけ動かす（作業ツリーには触らない）。
#[test]
fn merge_updates_the_ref_when_another_branch_is_checked_out() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    std::fs::write(tree.join("src.txt"), b"a\n").unwrap_or_else(|e| panic!("{e}"));
    git_must(&tree, &["add", "-A"]);
    git_must(&tree, &["commit", "-q", "-m", "add src"]);
    // 人は別のブランチで作業していて、しかも汚れている（それでも `main` は動かせる）。
    git_must(&repo, &["checkout", "-q", "-b", "wip"]);
    std::fs::write(repo.join("scratch.txt"), b"mine\n").unwrap_or_else(|e| panic!("{e}"));

    let before = git_line(&repo, &["rev-parse", "refs/heads/main"]).unwrap_or_default();
    let outcome = merge_into_default_branch(
        &repo,
        "celeris/01TASK",
        "main",
        &root.path().join("tmp/two"),
    );
    match outcome {
        MergeOutcome::Merged {
            sha,
            fast_forwarded,
        } => {
            assert!(!fast_forwarded, "ref だけ動かす");
            assert_ne!(sha, before);
            assert_eq!(
                git_line(&repo, &["rev-parse", "refs/heads/main"]).as_deref(),
                Some(sha.as_str())
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        current_branch(&repo).as_deref(),
        Some("wip"),
        "人のチェックアウトは動かさない"
    );
    assert!(!repo.join("src.txt").exists(), "人の作業ツリーには触らない");
    assert!(repo.join("scratch.txt").is_file());
}

/// 人が `main` を編集中なら 409（何も触らない）。
#[test]
fn merge_refuses_while_the_default_branch_is_being_edited() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    std::fs::write(tree.join("src.txt"), b"a\n").unwrap_or_else(|e| panic!("{e}"));
    git_must(&tree, &["add", "-A"]);
    git_must(&tree, &["commit", "-q", "-m", "add src"]);
    std::fs::write(repo.join("README.md"), b"human is editing\n").unwrap_or_else(|e| panic!("{e}"));

    let before = git_line(&repo, &["rev-parse", "refs/heads/main"]).unwrap_or_default();
    match merge_into_default_branch(
        &repo,
        "celeris/01TASK",
        "main",
        &root.path().join("tmp/three"),
    ) {
        MergeOutcome::Busy { detail } => assert_eq!(detail, "main が編集中"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        git_line(&repo, &["rev-parse", "refs/heads/main"]).as_deref(),
        Some(before.as_str())
    );
    assert!(tree.join("src.txt").is_file(), "worktree は残る");
}

/// rebase が衝突したら `--abort` して衝突したファイルを返す（worktree もブランチも残す）。
#[test]
fn a_conflicting_rebase_is_aborted_and_reports_the_files() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    std::fs::write(tree.join("README.md"), b"task side\n").unwrap_or_else(|e| panic!("{e}"));
    git_must(&tree, &["add", "-A"]);
    git_must(&tree, &["commit", "-q", "-m", "task"]);
    // `main` 側も同じ行を動かす。
    std::fs::write(repo.join("README.md"), b"human side\n").unwrap_or_else(|e| panic!("{e}"));
    git_must(&repo, &["add", "-A"]);
    git_must(&repo, &["commit", "-q", "-m", "human"]);

    match merge_into_default_branch(
        &repo,
        "celeris/01TASK",
        "main",
        &root.path().join("tmp/four"),
    ) {
        MergeOutcome::Conflict { files } => assert_eq!(files, vec!["README.md".to_string()]),
        other => panic!("{other:?}"),
    }
    assert!(tree.join("README.md").is_file(), "worktree はそのまま");
    assert!(git_ok(
        &repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/celeris/01TASK"
        ]
    ));
    assert!(
        !root.path().join("tmp/four").exists(),
        "一時 worktree は片付ける"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap_or_default(),
        "human side\n",
        "人の作業ツリーには触らない"
    );
}

/// `discard` は worktree もブランチも消す。
#[test]
fn discard_removes_the_worktree_and_the_branch() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    std::fs::write(tree.join("wip.txt"), b"x\n").unwrap_or_else(|e| panic!("{e}"));

    discard(&repo, Some(&tree), "celeris/01TASK").unwrap_or_else(|e| panic!("{e}"));
    assert!(!tree.exists());
    assert!(!git_ok(
        &repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/celeris/01TASK"
        ]
    ));
    assert!(repo.join("README.md").is_file(), "元のリポジトリは無事");
}

/// `default_branch` は設定 → `origin/HEAD` → `main` → `master` の順。
#[test]
fn the_default_branch_is_configured_then_detected() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    assert_eq!(default_branch(&repo, Some("trunk")), "trunk");
    assert_eq!(default_branch(&repo, Some("  ")), "main");
    assert_eq!(default_branch(&repo, None), "main");
    git_must(&repo, &["branch", "-m", "main", "master"]);
    assert_eq!(default_branch(&repo, None), "master");
    assert!(!has_origin(&repo));
}

/// 子プロセスの待ち時間: 終わらないコマンドは殺して `timed_out` を立てる。
#[test]
fn a_command_that_never_finishes_is_killed() {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "sleep 30"]);
    let out = run(cmd, Duration::from_millis(200)).unwrap_or_else(|| panic!("spawn"));
    assert!(out.timed_out);
    assert!(!out.ok);
    assert_eq!(out.why(), "コマンドが時間内に終わりませんでした");
}

fn head_of(dir: &Path) -> String {
    git_line(dir, &["rev-parse", "HEAD"]).unwrap_or_else(|| panic!("HEAD"))
}

fn commit_file(dir: &Path, name: &str, body: &[u8], msg: &str) {
    std::fs::write(dir.join(name), body).unwrap_or_else(|e| panic!("{e}"));
    git_must(dir, &["add", "-A"]);
    git_must(dir, &["commit", "-q", "-m", msg]);
}

/// ADR-0118 D2: target が先に進んでいれば task の worktree をその上へ rebase し、成果を保つ。
#[test]
fn sync_onto_target_rebases_the_task_branch_onto_an_advanced_target() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    commit_file(&tree, "task.txt", b"task\n", "task");
    let before = head_of(&tree);
    commit_file(&repo, "other.txt", b"other\n", "other task landed");
    let target = head_of(&repo);

    let outcome = sync_onto_target(&tree, "main");
    let SyncOutcome::Rebased {
        target_sha,
        before_sha,
        head_sha,
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert_eq!(target_sha, target);
    assert_eq!(before_sha, before);
    assert_eq!(head_sha, head_of(&tree));
    assert_ne!(head_sha, before);
    assert!(git_ok(
        &tree,
        &["merge-base", "--is-ancestor", &target, &head_sha]
    ));
    assert_eq!(
        git_line(&tree, &["rev-parse", "refs/heads/celeris/01TASK"]).as_deref(),
        Some(head_sha.as_str()),
        "ブランチ ref も進む"
    );
    assert!(tree.join("task.txt").is_file() && tree.join("other.txt").is_file());
    assert_eq!(head_of(&repo), target, "target は動かさない");
}

/// ADR-0118 D2: target が既に HEAD の祖先なら rebase しない。
#[test]
fn sync_onto_target_reports_up_to_date_without_rebasing() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    commit_file(&tree, "task.txt", b"task\n", "task");
    let before = head_of(&tree);
    let target = head_of(&repo);

    assert_eq!(
        sync_onto_target(&tree, "main"),
        SyncOutcome::UpToDate {
            target_sha: target,
            head_sha: before.clone(),
        }
    );
    assert_eq!(head_of(&tree), before);
}

/// ADR-0118 D6: 衝突したら `rebase --abort` し、ブランチ・worktree・未 push の commit を元の HEAD のまま残す。
#[test]
fn sync_onto_target_keeps_the_original_head_on_conflict() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    commit_file(&tree, "README.md", b"task side\n", "task");
    let before = head_of(&tree);
    commit_file(&repo, "README.md", b"human side\n", "human");
    let target = head_of(&repo);

    assert_eq!(
        sync_onto_target(&tree, "main"),
        SyncOutcome::Conflict {
            target_sha: target.clone(),
            files: vec!["README.md".to_string()],
        }
    );
    assert_eq!(head_of(&tree), before, "元の HEAD のまま");
    assert_eq!(
        git_line(&tree, &["rev-parse", "refs/heads/celeris/01TASK"]).as_deref(),
        Some(before.as_str())
    );
    assert_eq!(
        git_line(&tree, &["symbolic-ref", "-q", "HEAD"]).as_deref(),
        Some("refs/heads/celeris/01TASK"),
        "ブランチを出したまま"
    );
    assert_eq!(is_dirty(&tree), Some(false));
    assert_eq!(
        std::fs::read_to_string(tree.join("README.md")).unwrap_or_default(),
        "task side\n"
    );
    assert_eq!(head_of(&repo), target, "target は動かさない");
}

/// ADR-0118 D2: 未コミットの変更があれば何も触らない（stash・reset もしない）。
#[test]
fn sync_onto_target_leaves_a_dirty_worktree_untouched() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    commit_file(&tree, "task.txt", b"task\n", "task");
    let before = head_of(&tree);
    commit_file(&repo, "other.txt", b"other\n", "other");
    std::fs::write(tree.join("task.txt"), b"edited\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(tree.join("new.txt"), b"new\n").unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(sync_onto_target(&tree, "main"), SyncOutcome::Dirty);
    assert_eq!(head_of(&tree), before);
    assert_eq!(
        std::fs::read_to_string(tree.join("task.txt")).unwrap_or_default(),
        "edited\n"
    );
    assert!(tree.join("new.txt").is_file());
}

/// ADR-0118 D2: target が読めなければ触らずに失敗する。
#[test]
fn sync_onto_target_fails_when_the_target_is_missing() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = root.path().join("code");
    init_repo(&repo);
    let tree = root.path().join("ws/01TASK/repos/code");
    add_worktree(&repo, &tree, "celeris/01TASK");
    let before = head_of(&tree);
    match sync_onto_target(&tree, "no-such-branch") {
        SyncOutcome::Failed { detail } => assert!(detail.contains("no-such-branch"), "{detail}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(head_of(&tree), before);
}
