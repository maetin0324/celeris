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

#[tokio::test]
async fn preamble_is_empty_without_a_rules_source() {
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(preamble(cwd.path()).await.unwrap(), "");
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
