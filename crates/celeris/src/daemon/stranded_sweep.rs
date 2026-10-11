//! Periodic, deterministic detection of completed task branches.

use std::{path::PathBuf, process::Command, sync::Arc, time::Duration};

use task_core::{InstanceRole, SharedRole, Status, TaskStore, WorkspaceSpec};
use time::OffsetDateTime;

pub(crate) const INTERVAL: Duration = Duration::from_secs(300);

/// Runs once immediately, then once per interval. The callback is synchronous by design; callers
/// must put filesystem/git work on a blocking thread.
pub(crate) async fn periodic<F>(period: Duration, mut sweep: F)
where
    F: FnMut() + Send + 'static,
{
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now(), period);
    loop {
        interval.tick().await;
        sweep();
    }
}

pub(crate) fn run(store: &dyn TaskStore, now: OffsetDateTime) -> Result<usize, String> {
    let tasks = store.list(Some(Status::Done)).map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    for task in tasks.into_iter().filter(|t| t.parent_id.is_none()) {
        for reference in &task.repos {
            let repo = store
                .repo_get(reference.repo_id)
                .map_err(|e| e.to_string())?;
            let (path, default_branch) = match repo {
                Some(repo) => match repo.location {
                    WorkspaceSpec::Local { path, .. } => (Some(path), repo.default_branch),
                    WorkspaceSpec::Remote { .. } => (None, repo.default_branch),
                },
                None => (None, None),
            };
            let branch = format!("celeris/{}", task.id);
            // Resolve HEAD before entering task_ops::stranded::sweep. No store lock is held here.
            let head = path.as_ref().and_then(|path| {
                git_output(
                    path,
                    &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
                )
            });
            records.push(task_ops::stranded::BranchRecord {
                task_id: task.id,
                repo_id: reference.repo_id,
                repo_path: path,
                default_branch,
                branch: Some(branch),
                head,
                done: true,
                root: true,
                changed_repo: true,
            });
        }
    }
    task_ops::stranded::sweep(store, &records, now).map_err(|e| e.to_string())
}

fn git_output(repo: &PathBuf, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn spawn(store: Arc<dyn TaskStore>, role: SharedRole) -> tokio::task::JoinHandle<()> {
    tokio::spawn(periodic(INTERVAL, move || {
        if role.get() != InstanceRole::Active {
            return;
        }
        let store = Arc::clone(&store);
        // The interval driver never performs filesystem/git work on a runtime worker.
        tokio::task::spawn_blocking(move || {
            if let Err(error) = run(store.as_ref(), OffsetDateTime::now_utc()) {
                tracing::warn!(%error, "stranded branch sweep failed");
            }
        });
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(start_paused = true)]
    async fn stranded_sweep_runs_immediately_and_repeats_on_period() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let task = tokio::spawn(periodic(Duration::from_secs(10), move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        task.abort();
    }
}
