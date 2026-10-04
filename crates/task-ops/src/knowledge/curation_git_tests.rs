//! ADR-0131 付記（2026-10-04）: 日次整理の commit と push。remote は一時ディレクトリの bare repository
//! だけを使い、外部ネットワークに出ない。

use super::*;

fn kb_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    init(&root).expect("init");
    (dir, root)
}

fn bare_remote(dir: &Path, root: &Path) -> PathBuf {
    let bare = dir.join("remote.git");
    let out = std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&bare)
        .output()
        .expect("git init --bare");
    assert!(out.status.success(), "{out:?}");
    let url = bare.to_string_lossy().to_string();
    let o = git(root, &["remote", "add", "origin", &url], GIT_TIMEOUT).expect("remote add");
    assert!(o.ok, "{}", o.why());
    bare
}

fn line(dir: &Path, args: &[&str]) -> String {
    git(dir, args, GIT_TIMEOUT)
        .expect("git")
        .stdout
        .trim()
        .to_string()
}

fn write_curation(root: &Path, date: &str) -> Vec<String> {
    std::fs::create_dir_all(root.join("_curation")).expect("mkdir");
    let report = format!("_curation/{date}.md");
    std::fs::write(root.join(&report), "# 日次整理\n").expect("write report");
    std::fs::write(
        root.join("user/profile.md"),
        "# 人のプロフィール\n\n修正後\n",
    )
    .expect("write page");
    vec![
        report,
        "user/profile.md".to_string(),
        INDEX_FILE.to_string(),
        "README.md".to_string(),
    ]
}

const COUNTS: CurationCounts = CurationCounts {
    merged: 2,
    new: 1,
    deleted: 0,
    fixed: 3,
};

/// 変更 path が 1 commit になり、題に日付と件数、本文に task id、作者は KB の agent。
/// push 後は bare 側の branch が手元の HEAD と一致する。
#[test]
fn curation_commit_pushes_one_commit_to_bare_origin() {
    let (dir, root) = kb_dir();
    let bare = bare_remote(dir.path(), &root);
    let before = line(&root, &["rev-parse", "HEAD"]);
    let paths = write_curation(&root, "2026-10-04");
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();

    let sha = commit_curation(&root, "2026-10-04", COUNTS, "01TASK", &refs).expect("commit");
    assert_eq!(sha, line(&root, &["rev-parse", "HEAD"]));
    assert_eq!(
        line(&root, &["rev-parse", "HEAD~1"]),
        before,
        "1 commit だけ"
    );
    assert_eq!(
        line(&root, &["log", "-1", "--format=%s"]),
        "knowledge curation 2026-10-04: 統合 2・新規 1・削除 0・修正 3"
    );
    assert!(line(&root, &["log", "-1", "--format=%b"]).contains("task: 01TASK"));
    assert_eq!(
        line(&root, &["log", "-1", "--format=%an <%ae>"]),
        format!("{} <{}>", kb::AGENT_AUTHOR_NAME, kb::AGENT_AUTHOR_EMAIL)
    );
    let files = line(&root, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains("_curation/2026-10-04.md"), "{files}");
    assert!(files.contains("user/profile.md"), "{files}");
    assert!(line(&root, &["status", "--porcelain"]).is_empty());

    let branch = line(&root, &["symbolic-ref", "--short", "HEAD"]);
    match push_remote(&root) {
        PushOutcome::Pushed { remote, branch: b } => {
            assert_eq!(remote, "origin");
            assert_eq!(b, branch);
        }
        other => panic!("push: {other:?}"),
    }
    assert_eq!(
        line(&bare, &["rev-parse", &format!("refs/heads/{branch}")]),
        sha
    );
}

/// 変更が無ければ新しい commit を作らず HEAD を返す。
#[test]
fn curation_commit_without_changes_returns_head() {
    let (_dir, root) = kb_dir();
    let head_before = line(&root, &["rev-parse", "HEAD"]);
    let sha = commit_curation(
        &root,
        "2026-10-04",
        CurationCounts::default(),
        "01TASK",
        &["README.md", INDEX_FILE],
    )
    .expect("commit");
    assert_eq!(sha, head_before);
    assert_eq!(line(&root, &["rev-parse", "HEAD"]), head_before);
}

/// remote が無ければ push を省いて `NoRemote`。
#[test]
fn curation_commit_push_without_remote_is_no_remote() {
    let (_dir, root) = kb_dir();
    assert_eq!(push_remote(&root), PushOutcome::NoRemote);
}

/// 到達不能な remote（存在しない path）なら `Failed` を返し、手元の commit は残る。
/// 次回の push で溜まった commit がまとめて送られる。
#[test]
fn curation_commit_push_failure_keeps_commit() {
    let (dir, root) = kb_dir();
    let missing = dir.path().join("no-such-remote.git");
    let url = missing.to_string_lossy().to_string();
    assert!(
        git(&root, &["remote", "add", "origin", &url], GIT_TIMEOUT)
            .expect("remote add")
            .ok
    );
    let paths = write_curation(&root, "2026-10-04");
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let sha = commit_curation(&root, "2026-10-04", COUNTS, "01TASK", &refs).expect("commit");

    match push_remote(&root) {
        PushOutcome::Failed(why) => assert!(!why.is_empty()),
        other => panic!("push: {other:?}"),
    }
    assert_eq!(line(&root, &["rev-parse", "HEAD"]), sha, "commit は残る");

    // remote が戻れば、次回の push で溜まった commit が送られる。
    let out = std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&missing)
        .output()
        .expect("git init --bare");
    assert!(out.status.success());
    assert!(matches!(push_remote(&root), PushOutcome::Pushed { .. }));
    let branch = line(&root, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(
        line(&missing, &["rev-parse", &format!("refs/heads/{branch}")]),
        sha
    );
}
