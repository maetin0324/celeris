//! Persistent scope snapshots, separate from the WU's integration ancestry.
use std::io;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Snapshot {
    tracked: String,
    full: String,
}

fn git(cwd: &Path, index: Option<&Path>, args: &[&str]) -> io::Result<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd).args(args);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    // Synthetic commits need no configured identity and must not run hooks.
    cmd.env("GIT_AUTHOR_NAME", "Celeris WU snapshot")
        .env("GIT_AUTHOR_EMAIL", "wu@celeris.invalid")
        .env("GIT_COMMITTER_NAME", "Celeris WU snapshot")
        .env("GIT_COMMITTER_EMAIL", "wu@celeris.invalid");
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    String::from_utf8(out.stdout).map_err(io::Error::other)
}

/// `create` is only true before starting a worker, never when running its checks.
/// The metadata lives outside the working tree, survives daemon restart, and is keyed
/// by WU id (not attempt). The real index, HEAD and working files are untouched.
pub(super) fn scope_env(cwd: &Path, id: &str, create: bool) -> io::Result<Vec<(String, String)>> {
    // Non-git workspaces (including remote mirrors) may be created by the worker.
    if !cwd.is_dir() {
        return Ok(Vec::new());
    }
    let probe = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()?;
    if !probe.status.success() || probe.stdout != b"true\n" {
        return Ok(Vec::new());
    }
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err(io::Error::other("invalid work unit id"));
    }
    let root = git(cwd, None, &["rev-parse", "--show-toplevel"])?;
    let root = Path::new(root.trim_end());
    let git_dir = git(root, None, &["rev-parse", "--absolute-git-dir"])?;
    let dir = Path::new(git_dir.trim_end())
        .join("celeris-wu-bases")
        .join(id);
    let manifest = dir.join("snapshot.json");
    if !manifest.exists() {
        if !create {
            // Pre-upgrade WUs have only their integration base.
            return Ok(Vec::new());
        }
        std::fs::create_dir_all(&dir)?;
        let index = dir.join("index");
        let result = (|| {
            // Include staged additions, but keep pre-existing untracked paths out of
            // the conventional `git diff $CELERIS_WU_BASE` comparison.
            let original = git(
                root,
                None,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            )?;
            if Path::new(original.trim_end()).is_file() {
                std::fs::copy(original.trim_end(), &index)?;
            } else {
                git(root, Some(&index), &["read-tree", "HEAD"])?;
            }
            git(root, Some(&index), &["add", "-u", "--", "."])?;
            let tree = git(root, Some(&index), &["write-tree"])?;
            let head = git(root, None, &["rev-parse", "HEAD"])?;
            let tracked = git(
                root,
                None,
                &[
                    "commit-tree",
                    tree.trim(),
                    "-p",
                    head.trim(),
                    "-m",
                    "WU tracked scope base",
                ],
            )?;
            let untracked = git(root, None, &["ls-files", "--others", "--exclude-standard"])?;
            std::fs::write(dir.join("untracked"), untracked)?;
            git(root, Some(&index), &["add", "-A", "--", "."])?;
            let tree = git(root, Some(&index), &["write-tree"])?;
            let full = git(
                root,
                None,
                &[
                    "commit-tree",
                    tree.trim(),
                    "-p",
                    tracked.trim(),
                    "-m",
                    "WU full scope base",
                ],
            )?;
            let snapshot = Snapshot {
                tracked: tracked.trim().into(),
                full: full.trim().into(),
            };
            // Keep both objects reachable across git gc and retries.
            git(
                root,
                None,
                &[
                    "update-ref",
                    &format!("refs/celeris/wu-base/{id}"),
                    &snapshot.full,
                ],
            )?;
            let script = SCOPE_PATHS.replace("@BASE@", &snapshot.full);
            std::fs::write(dir.join("paths.sh"), script)?;
            std::fs::write(
                dir.join("snapshot.json.tmp"),
                serde_json::to_vec(&snapshot)?,
            )?;
            std::fs::rename(dir.join("snapshot.json.tmp"), &manifest)
        })();
        let _ = std::fs::remove_file(&index);
        result?;
    }
    let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(manifest)?)?;
    Ok(vec![
        (
            task_core::execution_plan::WU_BASE_ENV.into(),
            snapshot.tracked,
        ),
        (
            "CELERIS_WU_BASE_UNTRACKED".into(),
            dir.join("untracked").to_string_lossy().into_owned(),
        ),
        (
            "CELERIS_WU_SCOPE_PATHS".into(),
            dir.join("paths.sh").to_string_lossy().into_owned(),
        ),
    ])
}

// Rebuild the current tree using a disposable index. This also sees changes and
// deletions of baseline untracked files, symlinks, modes, and files later committed.
// Output uses Git's usual quoted path format, just like git diff --name-only.
const SCOPE_PATHS: &str = r#"#!/bin/sh
set -eu
cd "$(git rev-parse --show-toplevel)"
index=$(mktemp "$(dirname "$0")/check-index.XXXXXX")
trap 'rm -f "$index" "$index.lock"' EXIT HUP INT TERM
rm -f "$index"
source_index=$(git rev-parse --path-format=absolute --git-path index)
if [ -f "$source_index" ]; then cp "$source_index" "$index"; fi
export GIT_INDEX_FILE="$index"
if [ ! -f "$index" ]; then git read-tree HEAD; fi
git add -A -- .
git diff --cached --name-only @BASE@
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn init(cwd: &Path) {
        git(cwd, None, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(cwd.join("tracked"), "head\n").unwrap();
        git(cwd, None, &["add", "."]).unwrap();
        git(cwd, None, &["commit", "-qm", "initial"]).unwrap();
    }

    fn paths(cwd: &Path, env: &[(String, String)]) -> String {
        let script = &env
            .iter()
            .find(|(k, _)| k == "CELERIS_WU_SCOPE_PATHS")
            .unwrap()
            .1;
        let out = Command::new("sh")
            .current_dir(cwd)
            .arg(script)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn wu_base_snapshot_preserves_index_head_and_untracked_contents() {
        let repo = tempfile::tempdir().unwrap();
        let cwd = repo.path();
        init(cwd);
        std::fs::write(cwd.join("tracked"), "predecessor\n").unwrap();
        std::fs::write(cwd.join("staged"), "predecessor staged\n").unwrap();
        git(cwd, None, &["add", "staged"]).unwrap();
        std::fs::write(cwd.join("prior untracked"), "predecessor untracked\n").unwrap();
        std::fs::write(cwd.join("deleted"), "predecessor untracked\n").unwrap();
        std::os::unix::fs::symlink("tracked", cwd.join("link")).unwrap();
        let index = cwd.join(".git/index");
        let before = std::fs::read(&index).unwrap();
        let head = git(cwd, None, &["rev-parse", "HEAD"]).unwrap();
        let env = scope_env(cwd, "unit-1", true).unwrap();
        assert_eq!(std::fs::read(&index).unwrap(), before);
        assert_eq!(git(cwd, None, &["rev-parse", "HEAD"]).unwrap(), head);
        let base = &env.iter().find(|(k, _)| k == "CELERIS_WU_BASE").unwrap().1;
        assert_eq!(git(cwd, None, &["diff", "--name-only", base]).unwrap(), "");
        assert_eq!(paths(cwd, &env), "");
        assert_eq!(std::fs::read(&index).unwrap(), before);
        std::fs::write(cwd.join("prior untracked"), "unit edit\n").unwrap();
        std::fs::remove_file(cwd.join("deleted")).unwrap();
        std::fs::remove_file(cwd.join("link")).unwrap();
        std::os::unix::fs::symlink("staged", cwd.join("link")).unwrap();
        std::fs::write(cwd.join("new"), "unit addition\n").unwrap();
        assert_eq!(paths(cwd, &env), "deleted\nlink\nnew\nprior untracked\n");
        assert_eq!(scope_env(cwd, "unit-1", true).unwrap(), env, "retry");
        assert_eq!(
            scope_env(cwd, "unit-1", false).unwrap(),
            env,
            "checks after restart"
        );
        // Committing a predecessor file unchanged must not make it this unit's change.
        git(cwd, None, &["add", "-A"]).unwrap();
        git(cwd, None, &["commit", "-qm", "unit commit"]).unwrap();
        assert_eq!(paths(cwd, &env), "deleted\nlink\nnew\nprior untracked\n");
    }

    #[test]
    fn wu_base_own_worktrees_have_independent_dirty_snapshots() {
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        let parent = tempfile::tempdir().unwrap();
        let a = parent.path().join("a");
        let b = parent.path().join("b");
        git(
            repo.path(),
            None,
            &["worktree", "add", "-qb", "a", a.to_str().unwrap()],
        )
        .unwrap();
        git(
            repo.path(),
            None,
            &["worktree", "add", "-qb", "b", b.to_str().unwrap()],
        )
        .unwrap();
        std::fs::write(a.join("tracked"), "a predecessor\n").unwrap();
        std::fs::write(a.join("prior"), "a untracked\n").unwrap();
        std::fs::write(b.join("tracked"), "b predecessor\n").unwrap();
        let env_a = scope_env(&a, "unit-a", true).unwrap();
        let env_b = scope_env(&b, "unit-b", true).unwrap();
        assert_ne!(env_a, env_b);
        assert_eq!(paths(&a, &env_a), "");
        assert_eq!(paths(&b, &env_b), "");
        std::fs::write(a.join("outside"), "out of scope\n").unwrap();
        assert_eq!(paths(&a, &env_a), "outside\n");
        assert_eq!(paths(&b, &env_b), "");
        git(repo.path(), None, &["gc", "--prune=now"]).unwrap();
        assert_eq!(scope_env(&a, "unit-a", false).unwrap(), env_a);
        assert_eq!(paths(&a, &env_a), "outside\n");
    }

    #[test]
    fn wu_base_non_git_workspace_has_no_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            scope_env(&dir.path().join("not-created"), "unit", true)
                .unwrap()
                .is_empty()
        );
        assert!(scope_env(dir.path(), "unit", true).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
