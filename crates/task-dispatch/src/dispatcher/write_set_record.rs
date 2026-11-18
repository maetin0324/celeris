//! ADR-0130 D2: 実装 run の actual write-set（Git の確定差分）を run ごと・WU ごとに残す。
//!
//! dispatch の時点で repo ごとの開始 HEAD を固定し（[`run_write_bases`]。置き場は
//! `ExecutionCoordination`、ADR 2026-11-11 dispatcher-state-ownership D1）、
//! worker の終了処理（WU の自動 commit の後）で `<開始 HEAD>..HEAD` を採って
//! `run_write_sets` へ、done になった v2 WU は `base_commit..HEAD` を `work_unit_write_sets` へ書く。
//! git が読めない・開始 HEAD が分からない（daemon の再起動）・手元に Git worktree が無い
//! （remote / shared）ときは `unavailable` と理由を残す。どの失敗も run の遷移を変えない。
//!
//! この file は記録の組み立て（git の読み取りは `task_ops::changes`）だけを持ち、store にも設定にも触らない。
//! 開始 HEAD の固定（`dispatch_one`）と記録の書き込み（`record_run_write_sets`）は dispatch の入口 `dispatch_run.rs`
//! が行う（ADR 2026-11-18 dispatcher-store-access-pilot）。

use std::path::PathBuf;

use task_core::write_set::{WriteSetRecord, WriteSetStatus};
use task_core::{RepoId, Task, WorkUnitRow};
use task_worker::{LocalWorktree, TaskWorkspaces};
use time::OffsetDateTime;

use super::rfc3339;
use crate::execution_coordination::RunWriteBase;

pub(super) struct GitSite {
    repo_id: RepoId,
    name: String,
    dir: PathBuf,
    worktree: LocalWorktree,
}

fn now_rfc3339() -> String {
    rfc3339(OffsetDateTime::now_utc())
}

/// `workspaces` の Git worktree を、task の `repos`（名前 → RepoId）に対応づける。
/// 案件の repo を選んでいない旧来の 1 worktree は RepoId が無いので数えない。
pub(super) fn git_sites(task: &Task, workspaces: &TaskWorkspaces) -> Vec<GitSite> {
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

/// dispatch の直前（worker を起こす前）に固定する、この run が書く worktree の開始 HEAD。
/// 作業場所が無い（planner run・手元に作業場所の無い task）なら空。
pub(super) fn run_write_bases(
    task: &Task,
    workspaces: Option<&TaskWorkspaces>,
) -> Vec<RunWriteBase> {
    let Some(workspaces) = workspaces else {
        return Vec::new();
    };
    git_sites(task, workspaces)
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
        .collect()
}

/// worker run の終了時（WU の自動 commit の後）に残す run の actual write-set。`bases` は dispatch の時点の開始 HEAD
/// （`ExecutionCoordination::take_run_write_bases`）。`has_local_site` は開始 HEAD が無いときだけ意味を持ち、
/// 手元に Git worktree があるか（`unavailable` の理由を分ける）。
pub(super) fn run_records(
    task: &Task,
    run_id: &str,
    work_unit_id: Option<String>,
    bases: Option<Vec<RunWriteBase>>,
    has_local_site: bool,
) -> Vec<WriteSetRecord> {
    match bases {
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
            let reason = if has_local_site {
                "run start HEAD unknown (dispatcher restarted)"
            } else {
                "no local Git worktree (remote, shared or non-Git repo)"
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
    }
}

/// ADR-0130 D2: done になった v2 WU の `base_commit..HEAD`（WU の最終 snapshot）。`owner_id` は WU の id。
pub(super) fn work_unit_records(
    task: &Task,
    wu: &WorkUnitRow,
    workspaces: &TaskWorkspaces,
) -> Vec<WriteSetRecord> {
    let wu_dir = wu
        .branch
        .as_ref()
        .map(|_| crate::integration::wu_dir(&workspaces.task_dir, &wu.key));
    git_sites(task, workspaces)
        .into_iter()
        .map(|site| {
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
            write_set_record(task, &wu.id, Some(wu.id.clone()), site.repo_id, observed)
        })
        .collect()
}

fn write_set_record(
    task: &Task,
    owner_id: &str,
    work_unit_id: Option<String>,
    repo_id: RepoId,
    observed: Result<task_ops::changes::CommittedWriteSet, String>,
) -> WriteSetRecord {
    let (base_sha, head_sha, paths, status, reason) = match observed {
        Ok(o) => {
            let status = if o.dirty {
                WriteSetStatus::Incomplete
            } else {
                WriteSetStatus::Complete
            };
            let reason = o.dirty.then(|| "uncommitted changes remain".to_string());
            (Some(o.base_sha), Some(o.head_sha), o.paths, status, reason)
        }
        Err(why) => (
            None,
            None,
            Vec::new(),
            WriteSetStatus::Unavailable,
            Some(why),
        ),
    };
    WriteSetRecord {
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
