//! Git-derived outcome label values, computed from persisted write sets.
use std::{collections::BTreeSet, process::Command};

use task_core::{
    SqliteStore, TaskStore, WorkspaceSpec,
    route_extract::ChangeCount,
    route_record::RouteOutcome,
    store::StoreError,
    write_set::{WriteSetRecord, WriteSetStatus},
};

/// Extract an outcome, selecting its run write set first and the WU prediction
/// only when a WU run has no run-specific observations.
pub fn outcome_labels_for_run(
    store: &SqliteStore,
    run_id: &str,
    work_unit_id: Option<&str>,
) -> Result<RouteOutcome, StoreError> {
    let run_sets = store.run_write_sets(run_id)?;
    let records = if run_sets.is_empty() {
        match work_unit_id {
            Some(id) => store.work_unit_write_sets(id)?,
            None => run_sets,
        }
    } else {
        run_sets
    };
    let changes = count_write_set_changes(store, &records);
    task_core::route_extract::outcome_labels_for_run(store, run_id, changes)
}

/// Compute labels' changed_* values. Invalid, incomplete, remote, or unavailable
/// observations return `None`, which task-core serializes as null plus missing.
pub fn count_write_set_changes<S: TaskStore + ?Sized>(
    store: &S,
    records: &[WriteSetRecord],
) -> Option<ChangeCount> {
    if records.is_empty() {
        return None;
    }
    let mut lines = 0u64;
    let mut paths = BTreeSet::new();
    let mut crates = BTreeSet::new();
    for record in records {
        if record.status != WriteSetStatus::Complete {
            return None;
        }
        let (Some(base), Some(head)) = (record.base_sha.as_deref(), record.head_sha.as_deref())
        else {
            return None;
        };
        let repo = store.repo_get(record.repo_id).ok()??;
        let WorkspaceSpec::Local { path, .. } = repo.location else {
            return None;
        };
        let (repo_lines, repo_paths, repo_crates) = git_numstat(&path, base, head)?;
        lines = lines.checked_add(repo_lines)?;
        paths.extend(repo_paths.into_iter().map(|path| (record.repo_id, path)));
        crates.extend(repo_crates);
    }
    Some(ChangeCount {
        changed_lines: lines,
        changed_files_n: paths.len() as u64,
        changed_crates_n: crates.len() as u64,
    })
}

fn git_numstat(
    repo: &std::path::Path,
    base: &str,
    head: &str,
) -> Option<(u64, BTreeSet<String>, BTreeSet<String>)> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--numstat", &format!("{base}..{head}")])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let (mut lines, mut paths, mut crates) = (0u64, BTreeSet::new(), BTreeSet::new());
    for line in text.lines() {
        let mut columns = line.splitn(3, '\t');
        let added = columns.next()?;
        let deleted = columns.next()?;
        let path = columns.next()?;
        let number = |value: &str| {
            if value == "-" {
                Some(0)
            } else {
                value.parse::<u64>().ok()
            }
        };
        lines = lines
            .checked_add(number(added)?)?
            .checked_add(number(deleted)?)?;
        paths.insert(path.to_owned());
        if let Some(name) = path
            .strip_prefix("crates/")
            .and_then(|rest| rest.split_once('/').map(|(name, _)| name))
            .filter(|name| !name.is_empty())
        {
            crates.insert(name.to_owned());
        }
    }
    Some((lines, paths, crates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, process::Command};

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn routing_outcome_git_numstat_counts_lines_files_crates() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q"]);
        git(repo, &["config", "user.email", "test@example.invalid"]);
        git(repo, &["config", "user.name", "Fixture"]);
        fs::create_dir_all(repo.join("crates/a")).unwrap();
        fs::create_dir_all(repo.join("crates/b")).unwrap();
        fs::create_dir_all(repo.join("notes")).unwrap();
        fs::write(repo.join("crates/a/lib.rs"), "old\n").unwrap();
        fs::write(repo.join("crates/b/lib.rs"), "same\n").unwrap();
        fs::write(repo.join("notes/readme.txt"), "before\n").unwrap();
        fs::write(repo.join("notes/image.bin"), [0u8, 1, 2]).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        fs::write(repo.join("crates/a/lib.rs"), "new\nsecond\n").unwrap();
        fs::write(repo.join("crates/b/lib.rs"), "changed\n").unwrap();
        fs::write(repo.join("notes/readme.txt"), "after\n").unwrap();
        fs::write(repo.join("notes/image.bin"), [3u8, 4, 5]).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "change"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        let (lines, files, crates) = git_numstat(repo, &base, &head).unwrap();
        assert_eq!(lines, 7); // text changes add+delete; binary contributes zero
        assert_eq!(files.len(), 4);
        assert_eq!(crates.len(), 2);
    }
}
