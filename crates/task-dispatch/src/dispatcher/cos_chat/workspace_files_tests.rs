//! ADR 2026-10-08-cos-workspace-files-in-chat D1: 走査・差分・本文の path 照合（file system だけ。時計なし）。

use super::*;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, body).expect("write");
}

#[test]
fn cos_workspace_files_mention_boundaries() {
    let abs = "/local/celeris/data/db/cos/threads/T/workspace/artifacts/setup.md";
    assert!(first_mention(&format!("`{abs}` を見て"), "artifacts/setup.md").is_some());
    assert!(first_mention("artifacts/setup.mdを開く", "artifacts/setup.md").is_some());
    assert!(first_mention("手順は ./notes.md に書いた。", "notes.md").is_some());
    assert!(first_mention("see notes.md.", "notes.md").is_some());
    // 別の file 名の一部・別の dir の下・拡張子の続きは触れていない。
    assert_eq!(first_mention("old-notes.md", "notes.md"), None);
    assert_eq!(first_mention("other/notes.md", "notes.md"), None);
    assert_eq!(first_mention("notes.md.bak", "notes.md"), None);
    assert_eq!(first_mention("/tmp/x/notes.md", "notes.md"), None);
}

#[test]
fn cos_workspace_files_scan_excludes_internal_and_symlinks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path();
    write(ws, "artifacts/setup.md", "# setup");
    write(ws, ".taskd/chat-runs/r/result.json", "{}");
    write(ws, ".claude/skills/x/SKILL.md", "x");
    write(ws, "attachments/A/screen.png", "png");
    write(ws, "runs/r/stdout.jsonl", "{}");
    write(ws, "deep/a/b/c/d/e/f/g.md", "too deep");
    std::os::unix::fs::symlink("/etc/hostname", ws.join("link.md")).expect("symlink");
    let snap = snapshot(ws);
    let keys: Vec<&str> = snap.files.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["artifacts/setup.md"]);
}

#[test]
fn cos_workspace_files_candidates_mentions_first_then_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path();
    write(ws, "old.txt", "unchanged");
    write(ws, "notes.md", "before");
    write(ws, "edited.csv", "a");
    let before = snapshot(ws);
    write(ws, "edited.csv", "a,b");
    write(ws, "z-new.md", "new");
    write(ws, "artifacts/setup.md", "new");
    let after = snapshot(ws);
    let text = "結論: `artifacts/setup.md` を開く。前の notes.md も参照。";
    assert_eq!(
        candidates(&before, &after, text),
        vec!["artifacts/setup.md", "notes.md", "edited.csv", "z-new.md"]
    );
    assert!(candidates(&before, &before, "").is_empty());
}
