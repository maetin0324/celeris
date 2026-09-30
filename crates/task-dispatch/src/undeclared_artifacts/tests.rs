use super::*;

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn finds_markdown_outside_artifacts_dir_and_skips_declared_and_excluded() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "docs/paper/phase1/framing-candidates.md", "# framing\n");
    write(ws, "artifacts/result.json", "{}");
    write(ws, "artifacts/notes.md", "already inside artifacts/, skip");
    write(ws, "node_modules/pkg/readme.md", "skip: excluded dir");
    write(ws, "README.md", "declared already, skip");
    write(ws, "empty.md", "");

    let mut existing = HashSet::new();
    existing.insert("README.md".to_string());

    let found = scan_undeclared_markdown_artifacts(ws, &ws.join("artifacts"), &existing);
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, vec!["docs/paper/phase1/framing-candidates.md"]);
    assert!(!found[0].declared);
    assert_eq!(found[0].name, "framing-candidates.md");
    assert_eq!(found[0].kind, "md");
}

#[test]
fn caps_at_max_files() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    for i in 0..(MAX_FILES + 5) {
        write(ws, &format!("doc-{i:02}.md"), "x");
    }
    let found = scan_undeclared_markdown_artifacts(ws, &ws.join("artifacts"), &HashSet::new());
    assert_eq!(found.len(), MAX_FILES);
}

#[test]
fn skips_files_over_the_byte_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "big.md", &"x".repeat((MAX_BYTES + 1) as usize));
    write(ws, "small.md", "ok");
    let found = scan_undeclared_markdown_artifacts(ws, &ws.join("artifacts"), &HashSet::new());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "small.md");
}

// ---- ADR-0074 D6.3（Phase F1 (j)）: git worktree の Task の走査 ----

/// git worktree の Task で `artifacts/report.md` が未申告成果物として拾われる。リポジトリ全体
/// （worktree のコード）は走査しない（成果物ディレクトリの外は見ない）。
#[test]
fn git_worktree_task_registers_report_md_as_an_artifact() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/report.md", "# report\n");
    write(ws, "artifacts/result.json", "{}"); // 機械的なファイル、拾わない
    write(ws, "artifacts/checkpoint.json", "{}");
    write(ws, "src/lib.rs", "fn main() {}"); // worktree のコード、拾わない（成果物置き場の外）
    write(ws, "README.md", "repo readme, not an artifact"); // 同上

    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &HashSet::new());
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, vec!["artifacts/report.md"], "{found:?}");
    assert!(!found[0].declared);
    assert_eq!(found[0].kind, "md");
}

/// md 以外の人が読む拡張子（html/pdf/csv/png）も拾う。
#[test]
fn git_worktree_task_registers_other_human_readable_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/dashboard.html", "<html></html>");
    write(ws, "artifacts/data.csv", "a,b\n1,2\n");
    write(ws, "artifacts/notes.txt", "not a tracked extension");

    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &HashSet::new());
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["artifacts/dashboard.html", "artifacts/data.csv"],
        "{found:?}"
    );
}

/// 既に `Event::ArtifactProduced` で登録済みのパスは拾わない。
#[test]
fn git_worktree_task_skips_already_declared_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/report.md", "# report\n");
    let mut existing = HashSet::new();
    existing.insert("artifacts/report.md".to_string());
    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &existing);
    assert!(found.is_empty(), "{found:?}");
}
