//! ADR 2026-10-07-build-tmp-hygiene D1.4: cron 雛形の `extra.action` から発火した task の**決定的な保守
//! executor**。worker（LLM）にも reviewer にも渡さず、dispatcher の tick の中で I/O 層
//! （`crate::target_sweep::run_sweep`）を呼び、結果の `Event::TargetSweepRan` を task に追記して `done`
//! （走査・削除の失敗があれば `failed`）にする。**LLM は呼ばない。**
//!
//! 状態は既存の遷移だけで進める: `ready -(dispatch)-> running -(worker_done)-> reviewing -(review_pass)-> done`、
//! 失敗は `running -(worker_error{retryable:false})-> failed`。どの段も lease・worktree・run を作らない。

use std::path::PathBuf;
use std::time::SystemTime;

use task_core::model::TargetSweepMode;
use task_core::{Task, Trigger};
use task_worker::target_sweep::SweepParams;

use super::{DispatchError, Dispatcher};
use crate::target_sweep::{SweepEnv, run_sweep};

/// `[maintenance.target_sweep]` を渡されなかったときの 2 つ目の既定 root（D1.1）。
const DEFAULT_DEV_BUILD_CACHE: &str = "/var/tmp/agent-platform-build";

/// 掃除の時計を dispatcher の時計（試験では差し替えた時計）に揃える。大きさ・使用時刻は `stat` のまま。
struct DispatcherClock(SystemTime);

impl SweepEnv for DispatcherClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

impl Dispatcher {
    /// ADR 2026-10-07-build-tmp-hygiene D1.4: 掃除の roots と上限（celeris が `[maintenance.target_sweep]` から渡す）。
    /// 渡されなければ `SweepParams::default()` と `<build_cache_dir>/cargo`・`/var/tmp/agent-platform-build`。
    pub fn set_target_sweep(&mut self, params: SweepParams) {
        self.target_sweep = Some(params);
    }

    fn target_sweep_params(&self) -> SweepParams {
        self.target_sweep.clone().unwrap_or_else(|| SweepParams {
            roots: vec![
                self.config.build_cache_dir.join("cargo"),
                PathBuf::from(DEFAULT_DEV_BUILD_CACHE),
            ],
            ..SweepParams::default()
        })
    }

    /// `ready` の保守 task（cron 雛形の `action` ラベル付き）なら executor で走らせて `true`。
    /// それ以外（ラベル無し・`ready` でない）は何もせず `false`（通常の dispatch に回す）。
    pub(super) fn run_maintenance_task(&mut self, task: &Task) -> Result<bool, DispatchError> {
        let Some(action) = task_ops::cron_jobs::task_action(task) else {
            return Ok(false);
        };
        if task.status != task_core::Status::Ready {
            return Ok(false);
        }
        match action {
            "target_sweep" => self.run_target_sweep_task(task)?,
            other => {
                // task_action は実装済みの action しか返さないので来ない。来ても worker には渡さない。
                tracing::warn!(task_id = %task.id, action = other, "unknown maintenance action");
                self.store
                    .apply_transition(task.id, Trigger::Dispatch, None)?;
                self.store.apply_transition(
                    task.id,
                    Trigger::WorkerError { retryable: false },
                    None,
                )?;
            }
        }
        Ok(true)
    }

    /// cron で今作った task を、同じ tick のうちに executor へ回す（worker の枠の空きに依らない）。
    pub(super) fn run_maintenance_tasks(&mut self, task_ids: &[task_core::TaskId]) {
        for id in task_ids {
            let task = match self.store.get(*id) {
                Ok(Some(task)) => task,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(task_id = %id, error = %error, "maintenance task lookup failed");
                    continue;
                }
            };
            if let Err(error) = self.run_maintenance_task(&task) {
                tracing::warn!(task_id = %id, error = %error, "maintenance task failed to run");
            }
        }
    }

    fn run_target_sweep_task(&mut self, task: &Task) -> Result<(), DispatchError> {
        let mode = if task_ops::cron_jobs::task_mode(task) == "apply" {
            TargetSweepMode::Apply
        } else {
            TargetSweepMode::DryRun
        };
        let params = self.target_sweep_params();
        let clock = DispatcherClock(SystemTime::from(self.now_utc()));
        self.store
            .apply_transition(task.id, Trigger::Dispatch, None)?;
        let report = run_sweep(&params.roots, &params, mode, &clock);
        let event = report.to_event();
        if report.errors.is_empty() {
            self.store
                .apply_transition(task.id, Trigger::WorkerDone, Some(event))?;
            self.store
                .apply_transition(task.id, Trigger::ReviewPass, None)?;
            tracing::info!(
                task_id = %task.id,
                ?mode,
                deleted_bytes = report.deleted_bytes(),
                deleted_items = report.deleted.len(),
                skipped = report.skipped.len(),
                "target sweep finished"
            );
        } else {
            self.store.apply_transition(
                task.id,
                Trigger::WorkerError { retryable: false },
                Some(event),
            )?;
            tracing::warn!(
                task_id = %task.id,
                ?mode,
                errors = ?report.errors,
                "target sweep finished with errors"
            );
        }
        Ok(())
    }
}
