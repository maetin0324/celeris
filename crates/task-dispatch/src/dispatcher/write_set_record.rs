//! ADR-0130 D2: 実装 run の actual write-set（Git の確定差分）を run ごと・WU ごとに残す。
//!
//! dispatch の時点で repo ごとの開始 HEAD を固定し（[`Dispatcher::capture_run_write_bases`]）、
//! worker の終了処理（WU の自動 commit の後）で `<開始 HEAD>..HEAD` を採って
//! `run_write_sets` へ、done になった v2 WU は `base_commit..HEAD` を `work_unit_write_sets` へ書く。
//! git が読めない・開始 HEAD が分からない（daemon の再起動）・手元に Git worktree が無い
//! （remote / shared）ときは `unavailable` と理由を残す。どの失敗も run の遷移を変えない。

use super::*;

/// run 1 本・repo 1 つ分の採取場所と開始 HEAD（プロセス内メモリのみ）。
#[derive(Debug, Clone)]
pub(super) struct RunWriteBase {
    repo_id: task_core::RepoId,
    dir: PathBuf,
    /// `None` は開始 HEAD を読めなかった（終了時に `unavailable`）。
    start_sha: Option<String>,
}

struct GitSite {
    repo_id: task_core::RepoId,
    name: String,
    dir: PathBuf,
    worktree: task_worker::LocalWorktree,
}

fn now_rfc3339() -> String {
    rfc3339(OffsetDateTime::now_utc())
}

/// `workspaces` の Git worktree を、task の `repos`（名前 → RepoId）に対応づける。
/// 案件の repo を選んでいない旧来の 1 worktree は RepoId が無いので数えない。
fn git_sites(task: &Task, workspaces: &task_worker::TaskWorkspaces) -> Vec<GitSite> {
    workspaces
        .repos
        .iter()
        .filter_map(|r| {
            let wt = r.worktree.as_ref()?;
            let id = task.repos.iter().find(|rr| rr.name == r.name)?.repo_id;
            Some(GitSite {
                repo_id: id,
                name: r.name.clone(),
                dir: r.dir.clone(),
                worktree: wt.clone(),
            })
        })
        .collect()
}

impl Dispatcher {
    /// dispatch の直前（worker を起こす前）に、この run が書く worktree の開始 HEAD を固定する。
    pub(super) fn capture_run_write_bases(
        &mut self,
        task: &Task,
        run_id: &str,
        workspaces: Option<&task_worker::TaskWorkspaces>,
    ) {
        let Some(ws) = workspaces else {
            return;
        };
        let bases: Vec<RunWriteBase> = git_sites(task, ws)
            .into_iter()
            .map(|site| RunWriteBase {
                start_sha: task_ops::changes::run_start_head(
                    &site.dir,
                    &site.worktree.repo,
                    &site.worktree.branch,
                    &site.worktree.base.sha,
                ),
                repo_id: site.repo_id,
                dir: site.dir,
            })
            .collect();
        if !bases.is_empty() {
            self.run_write_bases.insert(run_id.to_string(), bases);
        }
    }

    /// worker run の終了時（WU の自動 commit の後）に actual write-set を残す。失敗は warn だけ。
    pub(super) fn record_run_write_sets(
        &mut self,
        task: &Task,
        run_id: &str,
        wu: Option<&task_core::WorkUnitRow>,
        wu_completed: bool,
    ) {
        let bases = self.run_write_bases.remove(run_id);
        let work_unit_id = wu.map(|w| w.id.clone());
        let records: Vec<task_core::write_set::WriteSetRecord> = match bases {
            Some(bases) => bases
                .into_iter()
                .map(|b| {
                    let observed = match &b.start_sha {
                        Some(start) => task_ops::changes::committed_write_set(&b.dir, start),
                        None => Err("run start HEAD could not be read".to_string()),
                    };
                    write_set_record(task, run_id, work_unit_id.clone(), b.repo_id, observed)
                })
                .collect(),
            // 開始 HEAD を持っていない: daemon の再起動、または手元に Git worktree が無い
            // （remote / shared / 非 Git）。数えられない事実だけを残す（ADR-0130 D2・D6）。
            None => {
                let local: Vec<_> = self
                    .task_workspaces_for(task)
                    .map(|ws| git_sites(task, &ws))
                    .unwrap_or_default();
                let reason = if local.is_empty() {
                    "no local Git worktree (remote, shared or non-Git repo)"
                } else {
                    "run start HEAD unknown (dispatcher restarted)"
                };
                task.repos
                    .iter()
                    .map(|r| {
                        write_set_record(
                            task,
                            run_id,
                            work_unit_id.clone(),
                            r.repo_id,
                            Err(reason.to_string()),
                        )
                    })
                    .collect()
            }
        };
        for record in &records {
            if let Err(e) = self.store.record_run_write_set(record) {
                tracing::warn!(task_id = %task.id, %run_id, repo = %record.repo_id, error = %e, "failed to record the run's write-set");
            }
        }
        if let Some(wu) = wu.filter(|_| wu_completed) {
            self.record_work_unit_write_sets(task, wu);
        }
    }

    /// ADR-0130 D2: done になった v2 WU の `base_commit..HEAD` を WU の最終 snapshot として残す。
    fn record_work_unit_write_sets(&self, task: &Task, wu: &task_core::WorkUnitRow) {
        if wu.phase.is_none() {
            return;
        }
        let Some(ws) = self.task_workspaces_for(task) else {
            return;
        };
        let wu_dir = wu
            .branch
            .as_ref()
            .map(|_| crate::integration::wu_dir(&ws.task_dir, &wu.key));
        for site in git_sites(task, &ws) {
            let repo_id = site.repo_id;
            let dir = match &wu_dir {
                Some(wu_dir) => wu_dir
                    .join(task_worker::task_repos::REPOS_DIR_NAME)
                    .join(&site.name),
                None => site.dir,
            };
            let observed = match &wu.base_commit {
                Some(base) => task_ops::changes::committed_write_set(&dir, base),
                None => Err("work unit has no base commit".to_string()),
            };
            let mut record = write_set_record(task, &wu.id, Some(wu.id.clone()), repo_id, observed);
            record.owner_id = wu.id.clone();
            if let Err(e) = self.store.record_work_unit_write_set(&record) {
                tracing::warn!(task_id = %task.id, work_unit = %wu.key, repo = %repo_id, error = %e, "failed to record the work unit's write-set");
            }
        }
    }
}

fn write_set_record(
    task: &Task,
    owner_id: &str,
    work_unit_id: Option<String>,
    repo_id: task_core::RepoId,
    observed: Result<task_ops::changes::CommittedWriteSet, String>,
) -> task_core::write_set::WriteSetRecord {
    let (base_sha, head_sha, paths, status, reason) = match observed {
        Ok(o) => {
            let status = if o.dirty {
                task_core::write_set::WriteSetStatus::Incomplete
            } else {
                task_core::write_set::WriteSetStatus::Complete
            };
            let reason = o.dirty.then(|| "uncommitted changes remain".to_string());
            (Some(o.base_sha), Some(o.head_sha), o.paths, status, reason)
        }
        Err(why) => (
            None,
            None,
            Vec::new(),
            task_core::write_set::WriteSetStatus::Unavailable,
            Some(why),
        ),
    };
    task_core::write_set::WriteSetRecord {
        owner_id: owner_id.to_string(),
        task_id: task.id,
        work_unit_id,
        repo_id,
        base_sha,
        head_sha,
        paths,
        status,
        reason,
        recorded_at: now_rfc3339(),
    }
}
