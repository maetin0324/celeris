use super::*;

#[tokio::test]
async fn preamble_carries_the_whole_claude_md() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join(RULES_SOURCE), "# 規則\n\n- unwrap() 禁止\n").unwrap();
    let text = preamble(cwd.path()).await.unwrap();
    assert!(text.starts_with(RULES_HEADING), "{text}");
    assert!(text.contains("# 規則\n\n- unwrap() 禁止"), "{text}");
    // 作業場所には何も書かない。
    let names: Vec<_> = std::fs::read_dir(cwd.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from(RULES_SOURCE)]);
}

/// 正本を追跡する repo の作業ツリーから CLAUDE.md が消えたら、規則なしで進めずに `Err`。
#[tokio::test]
async fn preamble_fails_when_the_tracked_rules_source_is_missing() {
    use crate::test_support::*;
    let root = tempfile::tempdir().unwrap();
    let wt = fresh_worktree_with_rules(root.path(), AgentsMdFixture::Absent);
    std::fs::remove_file(wt.join(RULES_SOURCE)).unwrap();
    let err = preamble(&wt).await.unwrap_err();
    assert!(
        err.to_string().contains("required rules source is missing"),
        "{err}"
    );
    assert!(err.to_string().contains(&wt.display().to_string()), "{err}");
    // commit 前で index にだけある正本も追跡として扱う。
    let staged = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(staged.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(staged.path().join(RULES_SOURCE), "# 規則\n").unwrap();
    git(&["add", RULES_SOURCE]);
    std::fs::remove_file(staged.path().join(RULES_SOURCE)).unwrap();
    let err = preamble(staged.path()).await.unwrap_err();
    assert!(err.to_string().contains("missing"), "{err}");
}

/// git でない dir と、CLAUDE.md を追跡したことのない repo は規則を要求しない（空の前置き）。
#[tokio::test]
async fn preamble_is_empty_where_no_rules_source_is_tracked() {
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(preamble(plain.path()).await.unwrap(), "");
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(repo.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(repo.path().join("README.md"), "x\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-q", "-m", "seed"]);
    assert_eq!(preamble(repo.path()).await.unwrap(), "");
}

#[tokio::test]
async fn preamble_fails_when_the_rules_cannot_be_read() {
    // 参照先なしの symlink。
    let dangling = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/nonexistent/rules.md", dangling.path().join(RULES_SOURCE))
        .unwrap();
    let err = preamble(dangling.path()).await.unwrap_err();
    assert!(err.to_string().contains("dangling"), "{err}");
    // 空の正本。
    let empty = tempfile::tempdir().unwrap();
    std::fs::write(empty.path().join(RULES_SOURCE), "\n \n").unwrap();
    let err = preamble(empty.path()).await.unwrap_err();
    assert!(err.to_string().contains("empty"), "{err}");
    // 読めない（ディレクトリが居座っている）。
    let blocked = tempfile::tempdir().unwrap();
    std::fs::create_dir(blocked.path().join(RULES_SOURCE)).unwrap();
    let err = preamble(blocked.path()).await.unwrap_err();
    assert!(err.to_string().contains("cannot read"), "{err}");
}

/// fresh worktree の形 4 通りすべてで、正本 CLAUDE.md の全文が 1 度だけ前置きになり、作業ツリーは変わらない。
#[tokio::test]
async fn preamble_from_fresh_worktrees_does_not_touch_the_tree() {
    use crate::test_support::*;
    for fixture in AGENTS_MD_FIXTURES {
        let root = tempfile::tempdir().unwrap();
        let wt = fresh_worktree_with_rules(root.path(), fixture);
        let before = agents_md_state(&wt);
        for _ in 0..3 {
            assert_required_rules_in(&preamble(&wt).await.unwrap());
        }
        assert_eq!(agents_md_state(&wt), before, "{fixture:?}");
        assert_eq!(worktree_changes(&wt), "", "{fixture:?}");
    }
}

/// この repo では CLAUDE.md が追跡され（fresh worktree に必ず出る）、AGENTS.md は追跡されない。
#[test]
fn this_repo_tracks_claude_md_and_not_agents_md() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ls = |path: &str| {
        let out = std::process::Command::new("git")
            .args(["ls-files", "--", path])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(ls("CLAUDE.md").trim(), "CLAUDE.md");
    assert_eq!(ls("AGENTS.md"), "");
}
