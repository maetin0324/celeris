//! Periodic, deterministic detection of completed task branches.

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use task_core::{InstanceRole, RepoId, SharedRole, Status, TaskStore, WorkspaceSpec};
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
    let mut repos = RepoGitCache::default();
    let (with_repos, repoless): (Vec<_>, Vec<_>) = tasks
        .into_iter()
        .filter(|t| t.parent_id.is_none())
        .partition(|t| !t.repos.is_empty());
    if !repoless.is_empty() {
        records.extend(repoless_records(store, &repoless, &mut repos)?);
    }
    for task in with_repos {
        for reference in &task.repos {
            let repo = store
                .repo_get(reference.repo_id)
                .map_err(|e| e.to_string())?;
            let branch = format!("celeris/{}", task.id);
            let (path, default_branch, head, not_applicable) = match repo {
                Some(repo) => match repo.location {
                    WorkspaceSpec::Local { path, .. } => {
                        // Resolve HEAD before entering task_ops::stranded::sweep. No store lock
                        // is held here; the repository's refs are read once per sweep.
                        let git = repos.get(repo.id, &path, repo.default_branch.as_deref());
                        let head = git.head(&branch);
                        let default_branch = git.default_branch.clone();
                        (Some(path), default_branch, head, false)
                    }
                    WorkspaceSpec::Remote { .. } => (
                        None,
                        Err(task_ops::changes::DefaultBranchError(
                            "remote repository: not checked by the sweep".into(),
                        )),
                        None,
                        true,
                    ),
                },
                None => (None, default_branch_for(None, None), None, false),
            };
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
                not_applicable,
            });
        }
    }
    let changed = task_ops::stranded::sweep(store, &records, now).map_err(|e| e.to_string())?;
    // ADR 2026-10-27-done-task-auto-reintegrate: 同じ single-flight の中で、main が進んだだけの行を取り込み直す。
    let outcomes =
        task_ops::stranded::reintegrate::run(store, &records, now).map_err(|e| e.to_string())?;
    for outcome in outcomes {
        tracing::info!(?outcome, "auto reintegrate");
    }
    // ADR 2026-10-28-cos-integrate-task D3: 結んだ integrate task の決着（取り込みで依頼を閉じる・失敗を人に回す）。
    let settled = task_ops::integrate_task::settle(store).map_err(|e| e.to_string())?;
    for outcome in settled {
        tracing::info!(?outcome, "integrate task settled");
    }
    // ADR 2026-10-29-phase-integration-auto-repair D3: 自動の解消を経ずに人に回した段の統合の依頼を、統合のやり直し
    // （衝突は merge の repair WU が解く）へ戻す。
    let retried = task_ops::integrate_task::phase::retry_unrepaired_requests(store)
        .map_err(|e| e.to_string())?;
    for (task_id, request_id) in retried {
        tracing::info!(%task_id, %request_id, "phase integration request handed to the automatic merge repair");
    }
    Ok(changed)
}

/// ADR 2026-10-12-stranded-sweep-single-flight D3: per sweep, each Local repository's
/// `refs/heads/celeris/*` and default branch are read from git once.
#[derive(Default)]
struct RepoGitCache {
    repos: HashMap<RepoId, RepoGit>,
}

struct RepoGit {
    /// `None` when `git for-each-ref` failed; every head of that repository is then unknown.
    heads: Option<HashMap<String, String>>,
    default_branch: Result<String, task_ops::changes::DefaultBranchError>,
}

impl RepoGit {
    fn head(&self, branch: &str) -> Option<String> {
        self.heads.as_ref()?.get(branch).cloned()
    }
}

impl RepoGitCache {
    fn get(&mut self, id: RepoId, path: &PathBuf, configured: Option<&str>) -> &RepoGit {
        self.repos.entry(id).or_insert_with(|| RepoGit {
            heads: celeris_heads(path),
            default_branch: default_branch_for(Some(path), configured),
        })
    }
}

/// One `git for-each-ref` for all task branches of a repository, keyed by `celeris/<id>`.
fn celeris_heads(path: &PathBuf) -> Option<HashMap<String, String>> {
    let refs = git_output(
        path,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/celeris/",
        ],
    )?;
    Some(
        refs.lines()
            .filter_map(|line| line.split_once(' '))
            .filter_map(|(name, sha)| {
                Some((name.strip_prefix("refs/heads/")?.to_owned(), sha.to_owned()))
            })
            .collect(),
    )
}

/// ADR 2026-10-11 D3: a done root task without `task.repos` is resolved through the registered
/// Local repositories. Each repository's `refs/heads/celeris/*` is read once and matched against
/// the task ids; a task whose branch exists nowhere did not change a repository and is skipped.
fn repoless_records(
    store: &dyn TaskStore,
    tasks: &[task_core::Task],
    cache: &mut RepoGitCache,
) -> Result<Vec<task_ops::stranded::BranchRecord>, String> {
    let mut repos = Vec::new();
    for project in store.project_list().map_err(|e| e.to_string())? {
        repos.extend(store.repo_list(project.id).map_err(|e| e.to_string())?);
    }
    let mut records = Vec::new();
    for repo in repos {
        let WorkspaceSpec::Local { path, .. } = repo.location else {
            continue;
        };
        // One git read per repository; no store lock is held here.
        let git = cache.get(repo.id, &path, repo.default_branch.as_deref());
        let Some(heads) = git.heads.as_ref() else {
            continue;
        };
        for task in tasks {
            let branch = format!("celeris/{}", task.id);
            let Some(head) = heads.get(branch.as_str()) else {
                continue;
            };
            records.push(task_ops::stranded::BranchRecord {
                task_id: task.id,
                repo_id: repo.id,
                repo_path: Some(path.clone()),
                default_branch: git.default_branch.clone(),
                branch: Some(branch),
                head: Some(head.clone()),
                done: true,
                root: true,
                changed_repo: true,
                not_applicable: false,
            });
        }
    }
    Ok(records)
}

/// ADR 2026-10-11-stranded-done-branches F1: the sweep uses the same resolver as delivery.
/// An unresolvable repository path is reported by the sweep before the default branch is read.
fn default_branch_for(
    path: Option<&PathBuf>,
    configured: Option<&str>,
) -> Result<String, task_ops::changes::DefaultBranchError> {
    match path {
        Some(path) => task_ops::changes::resolve_default_branch(path, configured),
        None => Err(task_ops::changes::DefaultBranchError(
            "repository cannot be resolved".to_owned(),
        )),
    }
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

/// ADR 2026-10-12-stranded-sweep-single-flight D1: at most one sweep runs at a time. The slot is
/// released by [`FlightGuard`]'s drop, so an error or a panic inside the sweep frees it as well.
#[derive(Default)]
pub(crate) struct SingleFlight {
    started: Mutex<Option<Instant>>,
}

pub(crate) struct FlightGuard(Arc<SingleFlight>);

impl SingleFlight {
    /// Takes the slot, or returns how long the sweep holding it has been running.
    pub(crate) fn try_begin(self: &Arc<Self>) -> Result<FlightGuard, Duration> {
        let mut started = self
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(at) = *started {
            return Err(at.elapsed());
        }
        *started = Some(Instant::now());
        Ok(FlightGuard(Arc::clone(self)))
    }
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        *self
            .0
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

pub(crate) fn spawn(store: Arc<dyn TaskStore>, role: SharedRole) -> tokio::task::JoinHandle<()> {
    let flight = Arc::new(SingleFlight::default());
    tokio::spawn(periodic(INTERVAL, move || {
        if role.get() != InstanceRole::Active {
            return;
        }
        let guard = match flight.try_begin() {
            Ok(guard) => guard,
            Err(running) => {
                tracing::warn!(
                    running_secs = running.as_secs(),
                    "stranded branch sweep skipped: the previous sweep is still running"
                );
                return;
            }
        };
        let store = Arc::clone(&store);
        // The interval driver never performs filesystem/git work on a runtime worker.
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
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

    #[tokio::test(start_paused = true)]
    async fn stranded_sweep_overlapping_ticks_do_not_start_a_second_run() {
        let flight = Arc::new(SingleFlight::default());
        let started = Arc::new(AtomicUsize::new(0));
        let skipped = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let driver = {
            let (flight, started, skipped) = (
                Arc::clone(&flight),
                Arc::clone(&started),
                Arc::clone(&skipped),
            );
            tokio::spawn(periodic(Duration::from_secs(10), move || {
                let Ok(guard) = flight.try_begin() else {
                    skipped.fetch_add(1, Ordering::SeqCst);
                    return;
                };
                started.fetch_add(1, Ordering::SeqCst);
                let (release_rx, done_tx) = (Arc::clone(&release_rx), done_tx.clone());
                // Stands in for a sweep blocked on slow git: it holds the slot until released.
                tokio::task::spawn_blocking(move || {
                    let _ = release_rx.lock().expect("release lock").recv();
                    drop(guard);
                    let _ = done_tx.send(());
                });
            }))
        };
        tokio::task::yield_now().await;
        assert_eq!(started.load(Ordering::SeqCst), 1);
        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(10)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(skipped.load(Ordering::SeqCst), 3);
        release_tx.send(()).expect("release first sweep");
        done_rx.recv().await.expect("first sweep finished");
        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(started.load(Ordering::SeqCst), 2);
        release_tx.send(()).expect("release second sweep");
        done_rx.recv().await.expect("second sweep finished");
        driver.abort();
    }

    #[tokio::test]
    async fn stranded_sweep_slot_is_released_after_a_panicking_sweep() {
        let flight = Arc::new(SingleFlight::default());
        let guard = flight.try_begin().expect("first sweep takes the slot");
        assert!(flight.try_begin().is_err());
        let result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            panic!("sweep panicked");
        })
        .await;
        assert!(result.is_err());
        assert!(flight.try_begin().is_ok());
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn stranded_heads_are_read_once_per_repository() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().to_path_buf();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        git(&repo, &["config", "user.name", "test"]);
        std::fs::write(repo.join("base"), "base").expect("write");
        git(&repo, &["add", "base"]);
        git(&repo, &["commit", "-m", "base"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["branch", "celeris/one"]);
        let mut cache = RepoGitCache::default();
        let id = RepoId::new();
        let git_view = cache.get(id, &repo, None);
        assert_eq!(git_view.head("celeris/one"), Some(head.clone()));
        assert_eq!(git_view.head("celeris/two"), None);
        assert_eq!(git_view.default_branch.as_deref().ok(), Some("main"));
        // Cached for this sweep: a branch created afterwards is not seen until the next sweep.
        git(&repo, &["branch", "celeris/two"]);
        assert_eq!(cache.get(id, &repo, None).head("celeris/two"), None);
        assert_eq!(
            RepoGitCache::default()
                .get(id, &repo, None)
                .head("celeris/two"),
            Some(head)
        );
    }
}
