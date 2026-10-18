//! Diagnose misplaced reports without adopting their untrusted contents.
//! See docs/adr/2026-10-17-wu-result-location.md.

use std::path::Path;

use crate::Terminal;

/// Stable diagnostic marker used to prevent automatic replan of the same path error.
pub const MISPLACED_RESULT_MARKER: &str = "result.json misplaced:";

/// Only the expected report is authoritative. Search the current artifact root and
/// cwd, never follow symlinks or inspect adapter run records / dependency trees.
/// A stray may be old: report its location, but never claim or consume its contents.
///
/// ADR 2026-10-18-result-location-recovery: when exactly one stray was written during this run
/// (`mtime >= since`, with 2 s slack for coarse mtimes), copy it to the expected path and return `None` so the normal validation
/// reads it. The stray itself is left in place. Zero or several candidates, or a stale one, stay errors.
pub async fn misplaced_result(
    artifacts_dir: &Path,
    cwd: &Path,
    since: std::time::SystemTime,
) -> Option<Terminal> {
    let expected = artifacts_dir.join("result.json");
    if !matches!(tokio::fs::symlink_metadata(&expected).await,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return None;
    }
    let root = artifacts_dir.parent()?;
    let mut pending = vec![root.to_path_buf()];
    if !cwd.starts_with(root) {
        pending.push(cwd.to_path_buf());
    }
    let mut found = Vec::new();
    while let Some(dir) = pending.pop() {
        let Ok(meta) = tokio::fs::symlink_metadata(&dir).await else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            let name = entry.file_name();
            if kind.is_dir() {
                if !matches!(
                    name.to_str(),
                    Some("runs" | ".git" | "target" | "node_modules")
                ) {
                    pending.push(entry.path());
                }
            } else if kind.is_file() && name == "result.json" {
                found.push(entry.path());
            }
        }
    }
    if found.is_empty() {
        return None;
    }
    found.sort();
    // File mtimes are coarse (kernel tick), so allow slack before the run start.
    let since = since - std::time::Duration::from_secs(2);
    if let [only] = found.as_slice()
        && let Ok(meta) = tokio::fs::metadata(only).await
        && meta.modified().is_ok_and(|m| m >= since)
        && tokio::fs::create_dir_all(artifacts_dir).await.is_ok()
        && tokio::fs::copy(only, &expected).await.is_ok()
    {
        return None;
    }
    Some(Terminal::Error {
        message: format!(
            "{MISPLACED_RESULT_MARKER} expected {}; found {}; reports were not adopted",
            expected.display(),
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        retryable: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn misplaced_result_search_is_scoped_and_expected_report_wins() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wu/e2e");
        let cwd = root.join("repos/repo");
        let artifacts = root.join("artifacts");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&artifacts).unwrap();
        for excluded in ["runs/old", "target", ".git", "node_modules"] {
            let path = root.join(excluded);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("result.json"), "{}").unwrap();
        }
        let sibling = dir.path().join("wu/other");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("result.json"), "{}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&sibling, cwd.join("linked")).unwrap();
        let stale = std::time::SystemTime::now() + std::time::Duration::from_secs(7200);
        assert!(misplaced_result(&artifacts, &cwd, stale).await.is_none());
        let stray = root.join("repos/artifacts");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("result.json"), "not even JSON").unwrap();
        let Some(Terminal::Error {
            message,
            retryable: false,
        }) = misplaced_result(&artifacts, &cwd, stale).await
        else {
            panic!("misplacement must be permanent");
        };
        assert!(message.contains(&stray.join("result.json").display().to_string()));
        assert!(message.contains(&artifacts.join("result.json").display().to_string()));
        // A second stray is ambiguous: never guess.
        std::fs::write(root.join("repos/result.json"), "{}").unwrap();
        let fresh = std::time::SystemTime::UNIX_EPOCH;
        assert!(misplaced_result(&artifacts, &cwd, fresh).await.is_some());
        std::fs::remove_file(root.join("repos/result.json")).unwrap();
        // A single stray written during the run is copied to the expected path (reviewer or worker).
        std::fs::write(stray.join("result.json"), r#"{"summary":"ok"}"#).unwrap();
        assert!(misplaced_result(&artifacts, &cwd, fresh).await.is_none());
        assert_eq!(
            std::fs::read_to_string(artifacts.join("result.json")).unwrap(),
            r#"{"summary":"ok"}"#
        );
        assert!(stray.join("result.json").exists());
    }
}
