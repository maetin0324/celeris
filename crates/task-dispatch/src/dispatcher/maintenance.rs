//! ADR 2026-10-07-build-tmp-hygiene D1.4・D4: cron 雛形の `extra.action` から発火した task の**決定的な保守
//! executor** と disk 監視の `Dispatcher` の側の入口。状態と処理は [`HostMaintenance`]
//! （`crate::host_maintenance`。ADR 2026-11-11 dispatcher-state-ownership D1・D2）が持ち、ここは owner の外の値
//! （store・config・注入時計）を渡して呼ぶだけ。executor は worker（LLM）にも reviewer にも渡さず、
//! lease・worktree・run を作らない。**LLM は呼ばない。**

use task_core::Task;
use task_worker::target_sweep::SweepParams;

use super::{DispatchError, Dispatcher};
use crate::host_maintenance::{HostMaintenance, MaintenanceError};

impl Dispatcher {
    /// ADR 2026-10-07-build-tmp-hygiene D1.4: 掃除の roots と上限（celeris が `[maintenance.target_sweep]` から渡す）。
    /// 渡されなければ `SweepParams::default()` と `<build_cache_dir>/cargo`・（scratch が有効なら）`<scratch>/dev-targets`。
    pub fn set_target_sweep(&mut self, params: SweepParams) {
        self.maint.set_target_sweep(params);
    }

    /// 付記 A2・A3（2026-10-09）: 保守 executor が足す範囲（scratch の root・終わった task の作業場所）。
    /// 渡されなければ [`crate::target_sweep::SweepScope::default`]。
    pub fn set_target_sweep_scope(&mut self, scope: crate::target_sweep::SweepScope) {
        self.maint.set_target_sweep_scope(scope);
    }

    /// ADR 2026-10-07-build-tmp-hygiene D4: 監視する path としきい値（celeris が `[[maintenance.disk_watch]]` から
    /// 渡す）。測るのは本物の `statvfs`。空なら監視しない。
    pub fn set_disk_watch(&mut self, entries: Vec<crate::disk_watch::DiskWatchEntry>) {
        // 付記 A4: warn の通知に載せる大きい target は、作業場所・scratch・共有 target から測る。
        let census = crate::disk_watch::FsTargetCensus {
            store: self.store.clone(),
            workspace_roots: self.config.workspace_roots.clone(),
            scratch_targets: self
                .scratch_active()
                .then(|| self.config.scratch.pool().targets_dir()),
            build_cache_cargo: self.config.build_cache_dir.join("cargo"),
        };
        self.set_disk_watch_with(
            entries,
            Box::new(crate::disk_watch::StatvfsProbe),
            Some(Box::new(census)),
        );
    }

    /// [`Self::set_disk_watch`] の測り方を差し替える（試験は偽の使用率を返す probe を渡す）。
    pub fn set_disk_watch_with_probe(
        &mut self,
        entries: Vec<crate::disk_watch::DiskWatchEntry>,
        probe: Box<dyn crate::disk_watch::DiskProbe>,
    ) {
        self.set_disk_watch_with(entries, probe, None);
    }

    /// [`Self::set_disk_watch`] の probe と census（付記 A4）を差し替える（試験は host を見ない偽物を渡す）。
    pub fn set_disk_watch_with(
        &mut self,
        entries: Vec<crate::disk_watch::DiskWatchEntry>,
        probe: Box<dyn crate::disk_watch::DiskProbe>,
        census: Option<Box<dyn crate::disk_watch::TargetCensus>>,
    ) {
        self.maint.set_disk_watch(entries, probe, census);
    }

    /// 付記 A4: 最後の測定で監視している path のどれかが `critical` か（新しい coding run を起こさない）。
    pub fn disk_watch_critical(&self) -> bool {
        self.maint.disk_watch_critical()
    }

    /// 付記 A4: この task の run は `critical` のあいだ止める coding run か（genre が `coding` か未指定）。
    pub(super) fn held_by_disk_critical(&self, task: &Task) -> bool {
        self.maint.held_by_disk_critical(task)
    }

    /// tick の段 `disk_watch`（前回から 60 秒経っていれば測る。時計は注入時計）。
    pub(super) fn tick_disk_watch(&mut self) {
        let (maint, inputs) = self.maintenance_parts();
        maint.tick_disk_watch(&inputs);
    }

    /// `ready` の保守 task（cron 雛形の `action` ラベル付き）なら executor で走らせて `true`。
    /// それ以外（ラベル無し・`ready` でない）は何もせず `false`（通常の dispatch に回す）。
    pub(super) fn run_maintenance_task(&mut self, task: &Task) -> Result<bool, DispatchError> {
        let (maint, inputs) = self.maintenance_parts();
        HostMaintenance::run_maintenance_task(maint, &inputs, task).map_err(|e| match e {
            MaintenanceError::Store(e) => DispatchError::from(e),
            MaintenanceError::Io(e) => DispatchError::from(e),
        })
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
}
