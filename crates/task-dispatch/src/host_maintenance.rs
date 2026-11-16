//! ADR 2026-11-11 dispatcher-state-ownership D1・D2: disk・scratch・build cache・repo target の掃除の状態の owner
//! （ADR-0043 D2、ADR-0066 D2、ADR-0074 F5-fix、ADR-0075、ADR-0129、ADR 2026-10-07-build-tmp-hygiene、
//! ADR 2026-10-10-local-disk-growth-paths）。
//!
//! - [`HostMaintenance`] の field は private で、`Dispatcher` の側の薄い入口（`dispatcher/housekeeping.rs`・
//!   `dispatcher/maintenance.rs`）は method だけを呼ぶ。method は `Dispatcher` を受けず、store・config の値・
//!   時計・`--mode verify` の印は [`MaintenanceInputs`] で受ける。
//! - 実行中の run と判定中の review の作業場所を消さない条件（`running`・`reviewing` を見る部分）は、
//!   `Dispatcher` が組んだ task の集合（`in_flight`）を [`HostMaintenance::cleanup_cancelled_worktrees`] に渡す。
//!   消すかどうかの判定は純粋関数 [`plan_workspace_cleanup`] が決める。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use task_core::local_machine::WorkspaceRoots;
use task_core::model::TargetSweepMode;
use task_core::{Event, SqliteStore, Status, StoreError, Task, TaskId, Trigger, WorkspaceSpec};
use task_ops::derive::last_run_id;
use task_worker::scratch::{AdoptCandidate, ScratchSettings};
use task_worker::target_sweep::SweepParams;
use time::OffsetDateTime;

use crate::disk_watch::{DiskProbe, DiskWatchEntry, DiskWatchRunner, TargetCensus};
use crate::target_sweep::{SweepEnv, SweepScope, run_sweep_with_scratch, sweep_workspace_targets};

/// ADR 2026-10-10-local-disk-growth-paths D2: 常設の repo target 掃除の間隔（秒）。
pub(crate) const REPO_TARGET_GC_INTERVAL_SECS: i64 = 600;

/// 付記 A4: disk_watch の `critical` で止める run の genre。
const CODING_GENRE: &str = "coding";

/// owner の外の値（store・config の値・注入時計・`--mode verify` の印）。`Dispatcher` の入口が組んで渡す。
pub(crate) struct MaintenanceInputs<'a> {
    pub(crate) store: &'a Arc<SqliteStore>,
    pub(crate) scratch: &'a ScratchSettings,
    pub(crate) shared_build_cache: bool,
    pub(crate) build_cache_dir: &'a Path,
    pub(crate) releases_dir: Option<&'a Path>,
    /// ADR 2026-11-11-disk-aware-dispatch D2: 空きの下限（filesystem の容量 × この %。0 以下は検査しない）。
    pub(crate) min_free_disk_pct: f64,
    pub(crate) workspace_roots: &'a WorkspaceRoots,
    pub(crate) workspace_prune_after_secs: u64,
    /// ADR-0040 D3 / ADR-0041 D5: `--mode verify` の煙試験インスタンス（本番と共有の物を消さない・staging DB に書かない）。
    pub(crate) verify_smoke: bool,
    /// `Dispatcher::now_utc` の注入時計の値（試験は差し替えた時計）。
    pub(crate) now: OffsetDateTime,
}

impl MaintenanceInputs<'_> {
    fn scratch_active(&self) -> bool {
        scratch_active(self.shared_build_cache, self.scratch)
    }
}

/// ADR-0075: scratch pool を使うか（`shared_build_cache` かつ `[scratch]` が有効〈NFS で無効化されていない〉）。
pub(crate) fn scratch_active(shared_build_cache: bool, scratch: &ScratchSettings) -> bool {
    shared_build_cache && scratch.enabled
}

/// 保守 executor の失敗（store の書き込みと scratch pool の lock）。
#[derive(Debug, thiserror::Error)]
pub(crate) enum MaintenanceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// ADR-0075（Phase G1）: scratch pool の GC の状態（プロセス内メモリだけ。DB に書かない）。
#[derive(Default)]
struct ScratchState {
    /// 測定スレッドの結果（パス → サイズ）。
    sizes: crate::scratch_gc::SizeCache,
    extents: crate::scratch_gc::SharedExtentCache,
    /// 削除スレッド・測定スレッドが動いている間は `true`（重ねて起こさない）。
    removing: Arc<AtomicBool>,
    measuring: Arc<AtomicBool>,
    last_measure: Option<Instant>,
    /// 直近の走査で P3 だった target（run の開始時の adopt の候補）。
    candidates: Vec<AdoptCandidate>,
    /// ADR 2026-11-11-disk-aware-dispatch D1: 空きの見込みで待たせている run。
    admission: crate::disk_admission::Gate,
    /// `DaemonSnapshot.scratch`。
    view: Option<task_ops::daemon::ScratchStatus>,
    last_gc: Option<task_ops::daemon::ScratchGcView>,
    /// journal の遷移（watermark の到達・解除）を 1 回だけ出すための直前の値。
    pressure: Option<task_worker::scratch::Pressure>,
    /// 実効上限の縮小を 1 回だけ出すための直前の値（GiB）。
    effective_warned_gib: Option<u64>,
    /// ディスク不足の通知の本文に足す P0 の一覧（直近の走査）。
    pinned_summary: Option<String>,
    /// この tick で緊急 GC を回した（通常の `scratch_gc` phase を重ねない）。
    ran_this_tick: bool,
    /// ADR-0129 (4): 直近に seed の更新を確かめた時刻（`None` = 起動後まだ。昇格の後の起動で直ちに確かめる）。
    seed_last_check: Option<Instant>,
    /// seed の更新スレッドが動いている間は `true`。
    seed_refreshing: Arc<AtomicBool>,
}

/// disk・scratch・build cache・repo target の掃除と、中止された task の作業場所の片付けの状態。
pub struct HostMaintenance<C> {
    /// ADR 2026-10-07-build-tmp-hygiene D1.4: cron の保守 executor（`target_sweep`）の roots と上限。
    /// `None` は既定（[`fallback_sweep_roots`]）。
    target_sweep: Option<SweepParams>,
    /// ADR 2026-10-07-build-tmp-hygiene D4: ディスク使用率の監視（`None` は無効）。
    disk_watch: Option<DiskWatchRunner>,
    /// ADR 2026-10-07-build-tmp-hygiene 付記 A2・A3: 保守 executor の掃除の範囲。
    target_sweep_scope: SweepScope,
    /// Phase 116: ディスク不足を通知済み（空きが戻るまで重ねて通知しない）。
    disk_low: bool,
    /// tick の頭で決める「新しい run を始めてよい空きがある」（draining なら常に `true`）。
    disk_ready: bool,
    /// ADR-0074 F5-fix: 終端の WU の target を消す別スレッドが動いている間は `true`（重ねて起こさない）。
    removing_build_caches: Arc<AtomicBool>,
    /// ADR 2026-10-10-local-disk-growth-paths D2: 終端 task の repo 直下 target の常設の掃除（cron に依らない）。
    /// 前回走った時刻（注入時計）と、削除スレッドが動いている間の印。
    repo_target_gc_at: Option<OffsetDateTime>,
    removing_repo_targets: Arc<AtomicBool>,
    /// ADR-0075（Phase G1）: scratch pool の GC の状態（測定の cache・削除 / 測定スレッドの印・adopt の候補・観測値）。
    scratch: ScratchState,
    /// ADR-0041 D1 / ADR-0043 D2: この celeris が用意したローカルの作業場所（1 つ以上のリポジトリ）。
    /// **終端では消さない**（差分を見るために残す）。消すのは**中止**（cancel）のときだけで、worktree と
    /// ブランチを消す。再起動では失われる（残った worktree は人が `git worktree remove` する）。
    task_workspaces: HashMap<TaskId, task_worker::TaskWorkspaces>,
    /// ADR 2026-11-08-health-enforcement-loop D6: 実行中の全体棚卸し（cron の job ごとの Busy の印）。
    health_inventory: InventoryRuns,
    /// ADR 2026-11-02-ts-control-layer-boundary 付記 S2・S5: TS の比較の状態（mode の表・記録の store・in-flight の比較・
    /// tick の待ち・戻しの失敗・canary の lane・digest の影）。型は `dispatcher` の `ScriptCompareState`（この module は
    /// `dispatcher` を使わないので型引数で受ける）。
    script_compare: C,
}

impl<C: Default> Default for HostMaintenance<C> {
    fn default() -> Self {
        Self {
            target_sweep: None,
            disk_watch: None,
            target_sweep_scope: SweepScope::default(),
            disk_low: false,
            disk_ready: true,
            removing_build_caches: Arc::new(AtomicBool::new(false)),
            repo_target_gc_at: None,
            removing_repo_targets: Arc::new(AtomicBool::new(false)),
            scratch: ScratchState::default(),
            task_workspaces: HashMap::new(),
            health_inventory: InventoryRuns::default(),
            script_compare: C::default(),
        }
    }
}

/// Busy の印: job ごとの実行中の棚卸し（`HostMaintenance` の欄 `health_inventory`）。プロセス内メモリのみ（再起動の後は
/// 走っていない。次の発火で同じ予定時刻の行が無い repo から評価し直す）。
#[derive(Debug, Default)]
pub struct InventoryRuns {
    running: HashMap<task_core::CronJobId, tokio::task::JoinHandle<()>>,
}

impl InventoryRuns {
    /// 終わっていない棚卸しの数（`Dispatcher::in_flight` の 1 項）。
    pub fn in_flight(&self) -> usize {
        self.running.values().filter(|h| !h.is_finished()).count()
    }

    /// `job` の棚卸しが走っているか（終わった印はここで消す）。
    pub(crate) fn is_running(&mut self, job: task_core::CronJobId) -> bool {
        self.running.retain(|_, handle| !handle.is_finished());
        self.running.contains_key(&job)
    }

    /// `work` を tick の外（tokio の blocking pool）で始め、`job` の印にする。
    pub(crate) fn start(
        &mut self,
        job: task_core::CronJobId,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| format!("no tokio runtime for the health inventory: {e}"))?;
        self.running.insert(job, runtime.spawn_blocking(work));
        Ok(())
    }

    /// 試験用: 走っている棚卸しの終わりを待つ。
    #[cfg(test)]
    pub(crate) async fn join_all(&mut self) {
        for (_, handle) in self.running.drain() {
            let _ = handle.await;
        }
    }
}

/// 中止・終端の task の作業場所をどう片付けるか（[`plan_workspace_cleanup`] の提案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceCleanup {
    /// run か review が走っている（やり直しは同じ worktree を使い回す）。触らない。
    InFlight,
    /// task ごと消えた。記録だけ落とす（worktree は人の手に残す）。
    Forget,
    /// 中止された。worktree とブランチを消す。
    RemoveCancelled,
    /// 終端（done / failed など）。残し、未コミットの変更があれば 1 行記録する。
    KeepTerminal,
    /// まだ終端でない。何もしない。
    Wait,
}

/// ADR-0043 D2: 作業場所の片付けの判断（純粋関数）。`in_flight` は `Dispatcher` が `running`・`reviewing` から
/// 渡す「この task の run か review が走っている」、`status` は store の状態（`None` は task が無い）。
/// 走っている間は状態に依らず消さない。
pub(crate) fn plan_workspace_cleanup(in_flight: bool, status: Option<Status>) -> WorkspaceCleanup {
    if in_flight {
        return WorkspaceCleanup::InFlight;
    }
    match status {
        None => WorkspaceCleanup::Forget,
        Some(Status::Cancelled) => WorkspaceCleanup::RemoveCancelled,
        Some(status) if status.is_terminal() => WorkspaceCleanup::KeepTerminal,
        Some(_) => WorkspaceCleanup::Wait,
    }
}

/// 掃除の時計を dispatcher の時計（試験では差し替えた時計）に揃える。大きさ・使用時刻は `stat` のまま。
struct DispatcherClock(SystemTime);

impl SweepEnv for DispatcherClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

impl<C> HostMaintenance<C> {
    /// 実行中の全体棚卸しの Busy の印（`Dispatcher::in_flight` に数え、cron の sink が印を付ける）。
    pub(crate) fn health_inventory(&self) -> &InventoryRuns {
        &self.health_inventory
    }

    /// 全体棚卸しの Busy の印を付ける・外す口（cron の sink の `Port` に渡す）。
    pub(crate) fn health_inventory_mut(&mut self) -> &mut InventoryRuns {
        &mut self.health_inventory
    }

    /// TS の比較の状態。
    pub(crate) fn script_compare(&self) -> &C {
        &self.script_compare
    }

    /// TS の比較の状態を書き換える口（`dispatcher::script_compare`・`script_canary`・CoS の digest pass が使う）。
    pub(crate) fn script_compare_mut(&mut self) -> &mut C {
        &mut self.script_compare
    }

    /// ADR 2026-10-07-build-tmp-hygiene D1.4: 掃除の roots と上限（celeris が `[maintenance.target_sweep]` から渡す）。
    pub(crate) fn set_target_sweep(&mut self, params: SweepParams) {
        self.target_sweep = Some(params);
    }

    /// 付記 A2・A3（2026-10-09）: 保守 executor が足す範囲（scratch の root・終わった task の作業場所）。
    pub(crate) fn set_target_sweep_scope(&mut self, scope: SweepScope) {
        self.target_sweep_scope = scope;
    }

    /// ADR 2026-10-07-build-tmp-hygiene D4: 監視する path としきい値と、測り方（probe）・付記 A4 の census。
    pub(crate) fn set_disk_watch(
        &mut self,
        entries: Vec<DiskWatchEntry>,
        probe: Box<dyn DiskProbe>,
        census: Option<Box<dyn TargetCensus>>,
    ) {
        self.disk_watch = Some(DiskWatchRunner {
            entries,
            probe,
            census,
            last_at: None,
            critical: false,
        });
    }

    /// 付記 A4: 最後の測定で監視している path のどれかが `critical` か（新しい coding run を起こさない）。
    pub(crate) fn disk_watch_critical(&self) -> bool {
        self.disk_watch.as_ref().is_some_and(|r| r.critical)
    }

    /// 付記 A4: この task の run は `critical` のあいだ止める coding run か（genre が `coding` か未指定）。
    pub(crate) fn held_by_disk_critical(&self, task: &Task) -> bool {
        self.disk_watch_critical() && task.genre.as_deref().is_none_or(|g| g == CODING_GENRE)
    }

    /// 付記 A4: 監視している path の一覧（`TaskWait::DiskCritical` の本文）。
    pub(crate) fn disk_watch_paths(&self) -> String {
        crate::disk_watch::paths_label(self.disk_watch.as_ref())
    }

    /// tick の段 `disk_watch`（前回から 60 秒経っていれば測る。時計は注入時計）。
    pub(crate) fn tick_disk_watch(&mut self, inputs: &MaintenanceInputs<'_>) {
        // ADR-0041 D5: verify は staging DB に disk の監視の通知・依頼を書かない。
        if inputs.verify_smoke {
            return;
        }
        if let Some(runner) = self.disk_watch.as_mut() {
            runner.tick(inputs.store.as_ref(), inputs.now);
        }
    }

    /// tick の頭で決めた「新しい run を始めてよい空きがある」。
    pub(crate) fn disk_ready(&self) -> bool {
        self.disk_ready
    }

    /// tick の頭: draining（`accepting_new_work = false`）なら常に `true`、そうでなければ [`Self::check_disk_space`]。
    pub(crate) fn refresh_disk_ready(
        &mut self,
        inputs: &MaintenanceInputs<'_>,
        accepting_new_work: bool,
    ) {
        self.disk_ready = !accepting_new_work || self.check_disk_space(inputs);
    }

    /// `DaemonSnapshot.scratch`（ADR-0075 D6）。
    pub(crate) fn scratch_view(&self) -> Option<&task_ops::daemon::ScratchStatus> {
        self.scratch.view.as_ref()
    }

    /// ADR 2026-11-11-disk-aware-dispatch D1: dispatch の前の空きの見込みの門（待たせている run の唯一の正本）。
    pub(crate) fn admission(&self) -> &crate::disk_admission::Gate {
        &self.scratch.admission
    }

    /// 試験用: 門の空きの見込みの probe を差し替える（大きな file を作らない）。
    #[cfg(test)]
    pub(crate) fn admission_mut(&mut self) -> &mut crate::disk_admission::Gate {
        &mut self.scratch.admission
    }

    /// 直近の走査で P3 だった target（run の開始時の adopt の候補。ADR-0075 D3）。
    pub(crate) fn scratch_candidates(&self) -> Vec<AdoptCandidate> {
        self.scratch.candidates.clone()
    }

    /// ADR-0043 D2: 中止されたときに片付けられるよう、run で使う作業場所を覚えておく。
    pub(crate) fn remember_task_workspaces(
        &mut self,
        task_id: TaskId,
        workspaces: task_worker::TaskWorkspaces,
    ) {
        self.task_workspaces.insert(task_id, workspaces);
    }

    /// 試験用: 最後に通知したディスク不足が続いているか。
    #[cfg(test)]
    pub(crate) fn disk_low(&self) -> bool {
        self.disk_low
    }

    /// 試験用: 作業場所の記録があるか。
    #[cfg(test)]
    pub(crate) fn remembers_task_workspaces(&self, task_id: TaskId) -> bool {
        self.task_workspaces.contains_key(&task_id)
    }

    /// ADR-0075 D2・ADR-0074 F5-fix・ADR-0129 (4)(5): tick の段 `scratch_and_build_caches`。scratch が有効なら
    /// semantic GC（緊急 GC を回した tick は重ねない）と seed の後始末、無効なら終端の WU の target を消す。
    pub(crate) fn tick_build_caches(&mut self, inputs: &MaintenanceInputs<'_>) {
        if inputs.scratch_active() {
            if !self.scratch.ran_this_tick {
                self.scratch_gc(inputs, false);
            }
            self.seed_housekeeping(inputs, Instant::now());
        } else {
            self.cleanup_work_unit_build_caches(inputs);
            self.scratch.view = inputs
                .shared_build_cache
                .then(|| crate::scratch_gc::disabled_status(inputs.scratch, SystemTime::now()));
        }
        self.scratch.ran_this_tick = false;
    }

    /// ADR-0075 D2（Phase G1）: tick の `scratch_gc` phase。lease と DB の状態を読み、`plan_gc` で決めた target を
    /// `.deleting-*` へ rename する（`remove_dir_all` は削除スレッド、サイズは測定スレッド）。`emergency` は空き <
    /// 空きの下限（`min_free_disk_pct`）のときの緊急モード。rename した件数を返す。
    pub(crate) fn scratch_gc(&mut self, inputs: &MaintenanceInputs<'_>, emergency: bool) -> usize {
        use task_worker::scratch::{GIB, Pressure};
        // ADR-0041 D5: verify は本番と同じ scratch pool を見る。staging DB の状態で本番の target を消さない。
        if inputs.verify_smoke {
            return 0;
        }
        let settings = inputs.scratch.clone();
        let legacy = crate::scratch_gc::legacy_paths(inputs.build_cache_dir, inputs.releases_dir);
        let sizes = self
            .scratch
            .sizes
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default();
        let lookup = task_worker::scratch::StoreLookup(inputs.store.as_ref());
        let run = crate::scratch_gc::run_gc(
            &settings,
            &legacy,
            &lookup,
            true,
            &sizes,
            inputs.min_free_disk_pct,
            emergency,
            &BTreeSet::new(),
            false,
        );
        let now = SystemTime::now();
        // journal: watermark の到達・解除（tracing だけ。満杯の瞬間に DB へ書かない。D2）。
        if self.scratch.pressure != Some(run.plan.pressure) {
            match run.plan.pressure {
                Pressure::None => {
                    if self.scratch.pressure.is_some() {
                        tracing::info!(
                            targets_bytes = run.plan.used_bytes,
                            "scratch: below the watermark again"
                        );
                    }
                }
                p => tracing::warn!(
                    pressure = p.as_str(),
                    targets_bytes = run.plan.used_bytes,
                    pinned_bytes = run.plan.pinned_bytes,
                    high = (settings.targets_max_bytes as f64 * settings.high_watermark) as u64,
                    free_bytes = ?run.fs.map(|f| f.1),
                    "scratch: watermark reached; reclaiming targets"
                ),
            }
            self.scratch.pressure = Some(run.plan.pressure);
        }
        let min_free = run.min_free_bytes;
        let eff = crate::scratch_gc::effective_max(
            settings.total_max_bytes,
            run.fs,
            run.plan.used_bytes,
            min_free,
        );
        let eff_gib = eff / GIB;
        if eff < settings.total_max_bytes && self.scratch.effective_warned_gib != Some(eff_gib) {
            tracing::warn!(
                "scratch: 実効上限 {eff_gib} GB（設定 {} GB）。pool の外の使用量で縮んでいる（ADR-0075 D1）",
                settings.total_max_bytes / GIB
            );
            self.scratch.effective_warned_gib = Some(eff_gib);
        }
        let executed = run.executed.clone().unwrap_or(crate::scratch_gc::Executed {
            removed: Vec::new(),
            reclaimed_bytes: 0,
        });
        let removed: BTreeSet<String> = executed.removed.iter().map(|p| p.id.clone()).collect();
        if !executed.removed.is_empty() {
            tracing::info!(
                removed = executed.removed.len(),
                reclaimed_bytes = executed.reclaimed_bytes,
                emergency,
                pressure = run.plan.pressure.as_str(),
                "scratch gc: moved targets aside"
            );
            self.scratch.last_gc = Some(crate::scratch_gc::gc_view(
                &run.plan, &executed, emergency, now,
            ));
        }
        self.scratch.candidates = run
            .scan
            .candidates
            .iter()
            .filter(|c| !removed.contains(&c.owner.to_string()))
            .cloned()
            .collect();
        self.scratch.pinned_summary = Some(crate::scratch_gc::pinned_summary(&run.scan, &run.plan));
        // 削除スレッド（同時に 1 本）。
        let roots = crate::scratch_gc::deleting_roots(&settings.pool(), &legacy);
        if !self.scratch.removing.load(Ordering::SeqCst)
            && crate::scratch_gc::has_pending_deletes(&roots)
        {
            crate::scratch_gc::spawn_removal(roots, self.scratch.removing.clone());
        }
        // 測定スレッド（同時に 1 本、間隔ごとに 1 つ）。
        let due = self
            .scratch
            .last_measure
            .is_none_or(|t| t.elapsed() >= crate::scratch_gc::measure_interval(&settings));
        if due
            && !self.scratch.measuring.load(Ordering::SeqCst)
            && let Some((path, lease)) = crate::scratch_gc::next_to_measure(&run.scan, &sizes)
        {
            self.scratch.last_measure = Some(Instant::now());
            crate::scratch_gc::spawn_measure(
                path,
                lease,
                self.scratch.sizes.clone(),
                self.scratch.extents.clone(),
                self.scratch.measuring.clone(),
            );
        }
        // ADR-0129 (1): sccache と cache server は Celeris の外（host の cargo 設定）。`build_status` は両欄を `None` にする。
        let view = crate::scratch_gc::build_status(
            &settings,
            &run.scan,
            &run.plan,
            run.fs,
            min_free,
            &sizes,
            self.scratch.last_gc.clone(),
            now,
        );
        self.scratch.view = Some(view);
        executed.removed.len()
    }

    /// ADR-0129 (4)(5): seed の後始末と更新。`SEED_CHECK_INTERVAL_SECS` ごと（起動直後〈＝昇格の後〉は直ちに）に、
    /// 登録された local の git repo を読み、seed の GC（current 以外の世代・放棄された build・猶予を過ぎた登録外の
    /// repo を `.deleting-*` へ rename）を tick の中で行い、main の前進の確認と build は別スレッド（同時に 1 本）で行う。
    /// seed は owner の semantic GC（`scratch_gc`）の対象にしない。
    pub(crate) fn seed_housekeeping(&mut self, inputs: &MaintenanceInputs<'_>, now: Instant) {
        // ADR-0041 D5: verify は本番と共有の seed を作り直さない。
        if inputs.verify_smoke {
            return;
        }
        if !inputs.scratch_active() || !inputs.scratch.seed_reflink {
            return;
        }
        if !task_worker::scratch::seed_check_due(
            self.scratch.seed_last_check,
            now,
            Duration::from_secs(task_worker::scratch::SEED_CHECK_INTERVAL_SECS),
        ) {
            return;
        }
        self.scratch.seed_last_check = Some(now);
        let repos = seed_repos(inputs.store.as_ref());
        let settings = inputs.scratch.clone();
        let pool = settings.pool();
        let registered: BTreeSet<String> = repos
            .iter()
            .map(|(p, _)| task_worker::build_cache::repo_cache_key(p))
            .collect();
        let gc = task_worker::scratch::seed_gc(&pool, &registered, SystemTime::now());
        if !gc.retired.is_empty() && !self.scratch.removing.load(Ordering::SeqCst) {
            crate::scratch_gc::spawn_removal(
                vec![task_worker::scratch::seed_deleting_root(&pool)],
                self.scratch.removing.clone(),
            );
        }
        if repos.is_empty() || self.scratch.seed_refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        let pressure_high = self
            .scratch
            .pressure
            .is_some_and(|p| p != task_worker::scratch::Pressure::None);
        let min_free_pct = inputs.min_free_disk_pct;
        let busy = self.scratch.seed_refreshing.clone();
        let spawned = std::thread::Builder::new()
            .name("celeris-scratch-seed".to_string())
            .spawn({
                let busy = busy.clone();
                move || {
                    let seed_repos: Vec<crate::scratch_gc::SeedRepo> = repos
                        .iter()
                        .map(|(path, configured)| {
                            let commit = task_ops::changes::resolve_default_branch(
                                path,
                                configured.as_deref(),
                            )
                            .ok()
                            .and_then(|branch| task_worker::scratch::resolve_commit(path, &branch));
                            let cargo = commit.as_deref().is_some_and(|c| {
                                task_worker::scratch::commit_has_cargo_manifest(path, c)
                            });
                            crate::scratch_gc::SeedRepo {
                                path: path.clone(),
                                commit,
                                cargo,
                            }
                        })
                        .collect();
                    let root = settings.pool().root().to_path_buf();
                    let hold = |seed_bytes: Option<u64>| {
                        let fs = crate::scratch_gc::fs_stats(&root);
                        let min_free =
                            fs.map(|f| crate::disk_admission::min_free_bytes(f.0, min_free_pct));
                        task_worker::scratch::seed_refresh_hold(
                            pressure_high,
                            fs.map(|f| f.1),
                            min_free.unwrap_or(0),
                            seed_bytes,
                        )
                    };
                    crate::scratch_gc::refresh_seeds(
                        &settings,
                        &seed_repos,
                        &task_worker::scratch::rustc_version,
                        &hold,
                        &task_worker::scratch::SeedBuildOps::real(),
                        SystemTime::now(),
                    );
                    busy.store(false, Ordering::SeqCst);
                }
            });
        if let Err(e) = spawned {
            busy.store(false, Ordering::SeqCst);
            tracing::warn!(error = %e, "scratch: could not spawn the seed refresh thread");
        }
    }

    /// ディスク不足は Phase 116 の infra 障害として一度だけ通知し、空きが戻ると自動で解除する。
    /// 検査不能も安全側に倒して run を開始しない。
    pub(crate) fn check_disk_space(&mut self, inputs: &MaintenanceInputs<'_>) -> bool {
        if inputs.min_free_disk_pct <= 0.0 {
            self.disk_low = false;
            return true;
        }
        // ADR 2026-10-14-local-machine-settings D5: 新しい task の足回りを用意する実効の root を見る。
        let workspace_root = match inputs
            .workspace_roots
            .effective_root_in(inputs.store.as_ref())
        {
            Ok(r) => r.value,
            Err(e) => {
                tracing::warn!(error = %e, "disk space: cannot read the workspace root settings; checking the unpinned root");
                inputs.workspace_roots.unpinned_root().to_path_buf()
            }
        };
        let mut paths: Vec<PathBuf> = vec![
            PathBuf::from("/"),
            workspace_root,
            inputs.build_cache_dir.to_path_buf(),
        ];
        // ADR-0075 D3: scratch pool の filesystem も見る。
        if inputs.scratch_active() {
            paths.push(inputs.scratch.dir.clone());
        }
        let pct = inputs.min_free_disk_pct;
        let find_low = move |paths: &[PathBuf]| {
            crate::disk_admission::find_low_disk(paths, pct, crate::scratch_gc::fs_stats)
        };
        let mut low = find_low(&paths);
        // ADR-0075 D3: 空きが足りなければ、新しい run を始める前にこの tick で緊急 GC（rename まで）を回す。
        // 削除は別スレッドなので空きが戻るのは数 tick 後。その間は下の保留と通知 1 回（attempts を消費しない）。
        let mut pinned_note = None;
        if low.is_some() && inputs.scratch_active() {
            let selected = self.scratch_gc(inputs, true);
            self.scratch.ran_this_tick = true;
            if selected == 0 {
                pinned_note = self.scratch.pinned_summary.clone();
            }
            low = find_low(&paths);
        }
        let low = low.map(|reason| match pinned_note {
            Some(note) => format!("{reason}; {note}"),
            None => reason,
        });
        match low {
            Some(reason) => {
                if !self.disk_low {
                    let body = format!("ディスク不足 (infra): {reason}. 新規 run を保留します。");
                    tracing::warn!(%body, "dispatch paused for disk space");
                    match inputs.store.notice_record(&task_core::feed::NoticeEvent {
                        source_key: format!("dispatch:disk-low:{}", ulid::Ulid::new()),
                        kind: task_core::feed::NoticeKind::BadNews,
                        group_key: "bad_news:project:none".into(),
                        title: body.clone(),
                        summary: body.clone(),
                        project_id: None,
                        task_id: None,
                        target: None,
                        links: Vec::new(),
                        at: OffsetDateTime::now_utc(),
                    }) {
                        Ok(_) => self.disk_low = true,
                        Err(error) => {
                            tracing::error!(%error, "failed to record disk shortage notification")
                        }
                    }
                }
                false
            }
            None => {
                if self.disk_low {
                    tracing::info!("disk space recovered; dispatch resumed");
                }
                self.disk_low = false;
                true
            }
        }
    }

    /// ADR-0074 F5-fix（不具合 1）: 終端（done / cancelled / superseded）になった WU の target
    /// （`<build_cache_dir>/cargo/<repo-key>/wu-<id>`）を消す。消す前に同じ親の中で
    /// `.deleting-wu-<id>` へ rename し（この tick のうちにパスから消える）、中身の削除は別スレッドで
    /// 行う（数 GB になりうるため tick を止めない）。前回途中で残った `.deleting-*` も消す。
    /// 行が見つからない WU（別の DB・消えた Task）の target には触れない。
    pub(crate) fn cleanup_work_unit_build_caches(&mut self, inputs: &MaintenanceInputs<'_>) {
        // ADR-0041 D5: verify は staging DB の状態で本番と共有の WU target を消さない。
        if inputs.verify_smoke {
            return;
        }
        if !inputs.shared_build_cache {
            return;
        }
        if self.removing_build_caches.load(Ordering::SeqCst) {
            return;
        }
        let root = task_worker::build_cache::cargo_root(inputs.build_cache_dir);
        let Ok(repos) = std::fs::read_dir(&root) else {
            return;
        };
        let mut doomed: Vec<PathBuf> = Vec::new();
        for repo in repos.flatten() {
            let Ok(entries) = std::fs::read_dir(repo.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".deleting-") {
                    doomed.push(entry.path());
                    continue;
                }
                let Some(id) = name.strip_prefix(task_worker::build_cache::WORK_UNIT_TARGET_PREFIX)
                else {
                    continue;
                };
                match inputs.store.work_unit_get(id) {
                    Ok(Some(row)) if row.status.is_terminal() => {
                        let Some(_cargo_locks) =
                            task_worker::workspace_targets::try_lock_target(&entry.path())
                        else {
                            continue;
                        };
                        let trash = repo.path().join(format!(".deleting-{name}"));
                        match std::fs::rename(entry.path(), &trash) {
                            Ok(()) => {
                                tracing::info!(work_unit = %row.key, task_id = %row.task_id, path = %entry.path().display(), "build cache: removing the target of a finished work unit");
                                doomed.push(trash);
                            }
                            Err(e) => {
                                tracing::warn!(path = %entry.path().display(), error = %e, "build cache: could not move the work unit target aside")
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(work_unit_id = %id, error = %e, "build cache: could not read the work unit")
                    }
                }
            }
        }
        if doomed.is_empty() {
            return;
        }
        let busy = self.removing_build_caches.clone();
        busy.store(true, Ordering::SeqCst);
        let spawned = std::thread::Builder::new()
            .name("celeris-wu-target-rm".to_string())
            .spawn({
                let busy = busy.clone();
                move || {
                    for path in doomed {
                        if let Err(e) = std::fs::remove_dir_all(&path) {
                            tracing::warn!(path = %path.display(), error = %e, "build cache: could not remove a work unit target");
                        }
                    }
                    busy.store(false, Ordering::SeqCst);
                }
            });
        if let Err(e) = spawned {
            busy.store(false, Ordering::SeqCst);
            tracing::warn!(error = %e, "build cache: could not spawn the removal thread");
        }
    }

    /// ADR-0043 D2（Phase 52。ADR-0041 D1 の後片付けを改める）: worktree は**終端では消さない**
    /// （`done` で未取り込みのものも `failed` も、差分を見るために残す）。消えるのは**中止**（cancel）
    /// のときだけで、worktree を消し、ブランチも `git branch -D` する（人の指示）。
    ///
    /// `in_flight` は `Dispatcher` が `running`・`reviewing` から組んだ「run か review が走っている task」で、
    /// その作業場所は状態に依らず触らない（[`plan_workspace_cleanup`]）。
    /// `done` / `failed` のときは、未コミットの変更があれば `WorkerProgress` を 1 行積んで記録だけ落とす。
    /// **celeris はコミットしない**（ADR-0019 D2）。
    pub(crate) fn cleanup_cancelled_worktrees(
        &mut self,
        inputs: &MaintenanceInputs<'_>,
        in_flight: &HashSet<TaskId>,
    ) -> Result<(), StoreError> {
        let ids: Vec<TaskId> = self.task_workspaces.keys().copied().collect();
        for id in ids {
            // まだ走っている／判定中なら store も読まない（やり直しは同じ worktree を使い回す）。
            let busy = in_flight.contains(&id);
            let status = if busy {
                None
            } else {
                inputs.store.get(id)?.map(|t| t.status)
            };
            match plan_workspace_cleanup(busy, status) {
                WorkspaceCleanup::InFlight | WorkspaceCleanup::Wait => {}
                WorkspaceCleanup::Forget => {
                    self.task_workspaces.remove(&id);
                }
                WorkspaceCleanup::RemoveCancelled => {
                    let task_dir = match inputs.workspace_roots.task_dir(inputs.store.as_ref(), &id)
                    {
                        Ok(d) => d,
                        Err(e) => {
                            tracing::warn!(task_id = %id, error = %e, "cancelled: cannot resolve the workspace root; keeping the worktree");
                            continue;
                        }
                    };
                    let (targets, _) = task_worker::workspace_targets::task_targets(&task_dir);
                    let locks: Option<Vec<_>> = targets
                        .iter()
                        .map(|p| task_worker::workspace_targets::try_lock_target(p))
                        .collect();
                    let Some(_locks) = locks else {
                        continue;
                    };
                    let Some(workspaces) = self.task_workspaces.remove(&id) else {
                        continue;
                    };
                    for (name, outcome) in workspaces.remove_for_cancel() {
                        match outcome {
                            task_worker::CleanupOutcome::Removed => {
                                tracing::info!(task_id = %id, repo = %name, "cancelled: the worktree and its branch are gone");
                            }
                            task_worker::CleanupOutcome::AlreadyGone => {}
                            other => {
                                tracing::warn!(task_id = %id, repo = %name, outcome = ?other, "cancelled: could not remove the worktree; leaving it for a human");
                            }
                        }
                    }
                }
                WorkspaceCleanup::KeepTerminal => {
                    // ADR-0043 D2: 残す。未コミットの変更があることだけ 1 行記録する。
                    let Some(workspaces) = self.task_workspaces.remove(&id) else {
                        continue;
                    };
                    let dirty: Vec<String> = workspaces
                        .repos
                        .iter()
                        .filter(|r| {
                            r.is_git() && task_worker::status_is_clean(&r.dir) == Some(false)
                        })
                        .map(|r| r.dir.to_string_lossy().into_owned())
                        .collect();
                    if !dirty.is_empty() {
                        let run_id = last_run_id(&inputs.store.events_for(id)?).unwrap_or_default();
                        inputs.store.append_event(
                            id,
                            &Event::worker_progress(
                                run_id,
                                format!("未コミットの変更が残っています: {}", dirty.join(", ")),
                            ),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// ADR-0066 D2（Phase 110b）: 終端（done / failed / cancelled）になってから
    /// `workspace_prune_after_secs` 経った作業場所から、ビルド生成物（`target/` 等）だけを刈る
    /// （ソースツリーと `artifacts/` は残す。ADR-0043 D2 の「終端では worktree を消さない」は変えない）。
    ///
    /// **1 tick に最大 1 か所**。探すところ（ストアの読み取りとメタデータの存在確認）までは同期で行う
    /// （軽い）。実際の削除は別スレッドに逃がし、終わったら `workspace_pruned` イベントを積む
    /// （tick はそれを待たない。`--mode verify` の煙試験インスタンスでは何もしない）。
    pub(crate) fn prune_one_workspace(&mut self, inputs: &MaintenanceInputs<'_>) {
        if inputs.verify_smoke {
            return;
        }
        if inputs.workspace_prune_after_secs == 0 {
            return;
        }
        let now = OffsetDateTime::now_utc();
        let candidate = match task_worker::workspace_prune::find_prune_candidate(
            inputs.store.as_ref(),
            inputs.workspace_roots,
            now,
            inputs.workspace_prune_after_secs,
        ) {
            Ok(Some(c)) => c,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "workspace prune: could not scan for candidates");
                return;
            }
        };
        let store = inputs.store.clone();
        let task_id = candidate.task_id;
        let task_dir = candidate.task_dir.clone();
        let thread = std::thread::Builder::new()
            .name("celeris-workspace-prune".to_string())
            .spawn(move || {
                let removed = task_worker::workspace_prune::prune(&candidate);
                if removed.is_empty() {
                    return;
                }
                let removed = task_worker::workspace_prune::relative_removed(&task_dir, &removed);
                tracing::info!(task_id = %task_id, removed = ?removed, "workspace: pruned build artifacts");
                if let Err(e) =
                    store.append_event(task_id, &Event::WorkspacePruned { removed })
                {
                    tracing::warn!(task_id = %task_id, error = %e, "workspace prune: could not record the event");
                }
            });
        if let Err(e) = thread {
            tracing::warn!(task_id = %task_id, error = %e, "workspace prune: could not spawn the prune thread");
        }
    }

    /// ADR 2026-10-10-local-disk-growth-paths D2: checkout を持つ task とその子孫が全て終端になり、最後の終端から
    /// `[maintenance.target_sweep] workspace_target_after_hours`（既定 6 時間）経った木の repo 直下 target を消す。
    /// cron の `target_sweep` が無効でも走る（[`REPO_TARGET_GC_INTERVAL_SECS`] ごと、注入時計）。tick の中は
    /// lock を持った rename まで、削除と `statvfs` の前後は別スレッド。`--mode verify` の煙試験では何もしない。
    pub(crate) fn sweep_repo_targets(&mut self, inputs: &MaintenanceInputs<'_>) {
        let after_secs = self.target_sweep_scope.workspace_target_after_secs;
        if inputs.verify_smoke
            || after_secs == 0
            || self.removing_repo_targets.load(Ordering::SeqCst)
        {
            return;
        }
        let now = inputs.now;
        if self
            .repo_target_gc_at
            .is_some_and(|at| (now - at).whole_seconds() < REPO_TARGET_GC_INTERVAL_SECS)
        {
            return;
        }
        self.repo_target_gc_at = Some(now);
        let roots = inputs.workspace_roots;
        let store = inputs.store;
        let probe = roots.unpinned_root().to_path_buf();
        let gc = crate::target_sweep::repo_target_gc_move_aside(
            store.as_ref(),
            roots,
            after_secs,
            now,
            true,
        );
        for e in &gc.errors {
            tracing::warn!(error = %e, "repo target gc");
        }
        for held in &gc.held {
            tracing::debug!(path = %held.path.display(), reason = %held.reason, "repo target gc: kept");
        }
        for m in &gc.moved {
            tracing::info!(task_id = %m.task_id, path = %m.target.display(), bytes = m.bytes, "repo target gc: removing the target of a finished task tree");
            let task_dir = match roots.task_dir(store.as_ref(), &m.task_id) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(task_id = %m.task_id, error = %e, "repo target gc: cannot resolve the workspace root");
                    probe.join(m.task_id.to_string())
                }
            };
            let removed = task_worker::workspace_prune::relative_removed(
                &task_dir,
                std::slice::from_ref(&m.target),
            );
            if let Err(e) = store.append_event(m.task_id, &Event::WorkspacePruned { removed }) {
                tracing::warn!(task_id = %m.task_id, error = %e, "repo target gc: could not record the event");
            }
        }
        if gc.to_remove.is_empty() {
            return;
        }
        let busy = self.removing_repo_targets.clone();
        busy.store(true, Ordering::SeqCst);
        let bytes: u64 = gc.moved.iter().map(|m| m.bytes).sum();
        let spawned = std::thread::Builder::new()
            .name("celeris-repo-target-rm".to_string())
            .spawn({
                let busy = busy.clone();
                let paths = gc.to_remove;
                move || {
                    let out = crate::target_sweep::remove_moved_targets(&paths, &probe);
                    for e in &out.errors {
                        tracing::warn!(error = %e, "repo target gc");
                    }
                    tracing::info!(removed = out.removed, blocks_bytes = bytes, statvfs_freed = ?out.freed_statvfs, "repo target gc: removed");
                    busy.store(false, Ordering::SeqCst);
                }
            });
        if let Err(e) = spawned {
            busy.store(false, Ordering::SeqCst);
            tracing::warn!(error = %e, "repo target gc: could not spawn the removal thread");
        }
    }

    fn target_sweep_params(&self, inputs: &MaintenanceInputs<'_>) -> SweepParams {
        self.target_sweep.clone().unwrap_or_else(|| SweepParams {
            roots: fallback_sweep_roots(inputs.build_cache_dir, inputs.scratch),
            ..SweepParams::default()
        })
    }

    /// ADR 2026-10-07-build-tmp-hygiene D1.4: cron 雛形の `extra.action` から発火した `ready` の保守 task を
    /// 決定的な executor で走らせて `true`。それ以外（ラベル無し・`ready` でない）は何もせず `false`。
    /// worker（LLM）にも reviewer にも渡さず、lease・worktree・run を作らない。
    /// 状態は既存の遷移だけで進める: `ready -(dispatch)-> running -(worker_done)-> reviewing -(review_pass)-> done`、
    /// 失敗は `running -(worker_error{retryable:false})-> failed`。
    pub(crate) fn run_maintenance_task(
        &mut self,
        inputs: &MaintenanceInputs<'_>,
        task: &Task,
    ) -> Result<bool, MaintenanceError> {
        let Some(action) = task_ops::cron_jobs::task_action(task) else {
            return Ok(false);
        };
        if task.status != Status::Ready {
            return Ok(false);
        }
        match action {
            "target_sweep" => self.run_target_sweep_task(inputs, task)?,
            other => {
                // task_action は実装済みの action しか返さないので来ない。来ても worker には渡さない。
                tracing::warn!(task_id = %task.id, action = other, "unknown maintenance action");
                inputs
                    .store
                    .apply_transition(task.id, Trigger::Dispatch, None)?;
                inputs.store.apply_transition(
                    task.id,
                    Trigger::WorkerError { retryable: false },
                    None,
                )?;
            }
        }
        Ok(true)
    }

    fn run_target_sweep_task(
        &mut self,
        inputs: &MaintenanceInputs<'_>,
        task: &Task,
    ) -> Result<(), MaintenanceError> {
        let store = inputs.store;
        let mode = if task_ops::cron_jobs::task_mode(task) == "apply" {
            TargetSweepMode::Apply
        } else {
            TargetSweepMode::DryRun
        };
        let params = self.target_sweep_params(inputs);
        let clock = DispatcherClock(SystemTime::from(inputs.now));
        // 付記 A2: scratch の `<scratch>/targets` を owner 配置の root として足す。
        let scope = self.target_sweep_scope;
        let mut roots = params.roots.clone();
        let mut scratch_roots = Vec::new();
        if scope.scratch_targets && inputs.scratch_active() {
            let root = inputs.scratch.pool().targets_dir();
            if root.is_dir() {
                if !roots.contains(&root) {
                    roots.push(root.clone());
                }
                scratch_roots.push(root);
            }
        }
        // Serialize scratch allocation/adoption and GC while paths are scanned and renamed.
        // tick の中なので待たない。pool が使用中なら task を ready のまま次の tick に回す
        // （ADR 2026-11-11-scratch-lock-tick-stall）。
        let _pool_lock = if scratch_roots.is_empty() {
            None
        } else {
            let Some(lock) = inputs.scratch.pool().try_lock()? else {
                return Ok(());
            };
            Some(lock)
        };
        store.apply_transition(task.id, Trigger::Dispatch, None)?;
        let lookup = task_worker::scratch::StoreLookup(store.as_ref());
        let mut report =
            run_sweep_with_scratch(&roots, &scratch_roots, &params, mode, &clock, &lookup);
        drop(_pool_lock);
        // 付記 A3: 終わった task の作業場所に残った cargo target（build 中は残す）。
        if scope.workspace_target_after_secs > 0 {
            sweep_workspace_targets(
                &mut report,
                store.as_ref(),
                inputs.workspace_roots,
                scope.workspace_target_after_secs,
                inputs.now,
            );
        }
        let event = report.to_event();
        // D1.5: 上限まで下げきれなければ D4 の `disk` 通知（`disk:target_sweep`）も出す。
        if report.over_cap_unresolved {
            let over_roots: Vec<(String, u64)> = report
                .roots
                .iter()
                .filter(|r| r.after_bytes > params.max_bytes_per_root)
                .map(|r| (r.root.display().to_string(), r.after_bytes))
                .collect();
            let notice = crate::disk_watch::target_sweep_notice(
                task.id,
                &over_roots,
                params.max_bytes_per_root,
                inputs.now,
            );
            if let Err(error) = store.notice_record(&notice) {
                tracing::warn!(task_id = %task.id, %error, "failed to record target sweep disk notice");
            }
        }
        if report.errors.is_empty() {
            store.apply_transition(task.id, Trigger::WorkerDone, Some(event))?;
            store.apply_transition(task.id, Trigger::ReviewPass, None)?;
            tracing::info!(
                task_id = %task.id,
                ?mode,
                deleted_bytes = report.deleted_bytes(),
                deleted_items = report.deleted.len(),
                skipped = report.skipped.len(),
                "target sweep finished"
            );
        } else {
            store.apply_transition(
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

/// seed の対象: 登録された全案件の local の git repo（path と設定の既定ブランチ）。
fn seed_repos(store: &SqliteStore) -> Vec<(PathBuf, Option<String>)> {
    let projects = match store.project_list() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "scratch: could not list projects for the seeds");
            return Vec::new();
        }
    };
    let mut out: Vec<(PathBuf, Option<String>)> = Vec::new();
    for project in projects {
        let Ok(repos) = store.repo_list(project.id) else {
            continue;
        };
        for r in repos {
            if r.kind != task_core::repos::RepoKind::Git {
                continue;
            }
            if let WorkspaceSpec::Local { path, .. } = &r.location
                && !out.iter().any(|(p, _)| p == path)
            {
                out.push((path.clone(), r.default_branch.clone()));
            }
        }
    }
    out
}

/// `[maintenance.target_sweep]` を渡されなかったときの roots: `<build_cache_dir>/cargo` と、scratch が有効なら
/// `<scratch>/dev-targets`（ADR 2026-10-14-eval-trial-scratch-target D3。ルート fs の `/var/tmp` は使わない）。
fn fallback_sweep_roots(build_cache_dir: &Path, scratch: &ScratchSettings) -> Vec<PathBuf> {
    let mut roots = task_worker::target_sweep::default_roots(build_cache_dir, &scratch.dir);
    if !scratch.enabled || scratch.dir.as_os_str().is_empty() {
        roots.truncate(1);
    }
    roots
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use task_core::Status;
    use task_worker::scratch::ScratchSettings;

    use super::{WorkspaceCleanup, fallback_sweep_roots, plan_workspace_cleanup};

    #[test]
    fn target_sweep_fallback_roots_use_scratch_dev_targets_not_var_tmp() {
        let on = ScratchSettings::with_dir(PathBuf::from("/local/s"));
        assert!(on.enabled);
        assert_eq!(
            fallback_sweep_roots(Path::new("/x/bc"), &on),
            vec![
                PathBuf::from("/x/bc/cargo"),
                PathBuf::from("/local/s/dev-targets")
            ]
        );
        assert_eq!(
            fallback_sweep_roots(Path::new("/x/bc"), &ScratchSettings::disabled()),
            vec![PathBuf::from("/x/bc/cargo")]
        );
    }

    /// 不変条件（ADR-0043 D2、ADR 2026-11-11 dispatcher-state-ownership D2）: run か review が走っている task の
    /// 作業場所は、中止・終端・task の消失のどれでも片付けない。中止で走っていなければ消し、終端なら残す。
    /// 既存の試験（`git_workspace.rs` の cancel の試験）は走っていない task だけを見るので、`in_flight` を
    /// 判断が無視しても通る。
    #[test]
    fn plan_workspace_cleanup_never_touches_a_task_with_a_run_in_flight() {
        for status in [
            None,
            Some(Status::Cancelled),
            Some(Status::Done),
            Some(Status::Failed),
            Some(Status::Running),
            Some(Status::Reviewing),
        ] {
            assert_eq!(
                plan_workspace_cleanup(true, status),
                WorkspaceCleanup::InFlight,
                "{status:?}"
            );
        }
        assert_eq!(
            plan_workspace_cleanup(false, Some(Status::Cancelled)),
            WorkspaceCleanup::RemoveCancelled
        );
        assert_eq!(
            plan_workspace_cleanup(false, Some(Status::Done)),
            WorkspaceCleanup::KeepTerminal
        );
        assert_eq!(
            plan_workspace_cleanup(false, Some(Status::Failed)),
            WorkspaceCleanup::KeepTerminal
        );
        assert_eq!(
            plan_workspace_cleanup(false, None),
            WorkspaceCleanup::Forget
        );
        assert_eq!(
            plan_workspace_cleanup(false, Some(Status::Running)),
            WorkspaceCleanup::Wait
        );
    }
}

#[cfg(test)]
mod structure_guard_tests {
    /// 不変条件（ADR 2026-11-11 dispatcher-state-ownership D2 の 1〜3・7）: `HostMaintenance` の field は private で
    /// （`#[cfg(test)]` の field の無いことは `dispatcher/tests/structure_guard.rs`）、owner の module は組み立て役の型を受けず `dispatcher` module を使わない。
    /// 既存の試験は振る舞いだけを見るので、field を `pub` に戻しても、method が組み立て役を受けても通る。
    #[test]
    fn structure_guard_host_maintenance() {
        let src = include_str!("host_maintenance.rs");
        // 試験の module（この関数）より前だけを見る（この関数の中の文字列で自分に当たらないように）。
        let src = &src[..src
            .find("mod structure_guard_tests")
            .expect("the guard module is in the source")];
        let start = src
            .find("pub struct HostMaintenance<C> {")
            .expect("HostMaintenance is defined");
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").expect("the struct body ends")];
        let mut fields = Vec::new();
        for line in body.lines().skip(1) {
            let t = line.trim_start();
            if t.starts_with("///") || t.is_empty() {
                continue;
            }
            assert!(
                !t.starts_with("pub"),
                "a HostMaintenance field is not private: {line}"
            );
            if let Some((name, _)) = t.split_once(": ")
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            {
                fields.push(name.to_string());
            }
        }
        assert_eq!(
            fields,
            [
                "target_sweep",
                "disk_watch",
                "target_sweep_scope",
                "disk_low",
                "disk_ready",
                "removing_build_caches",
                "repo_target_gc_at",
                "removing_repo_targets",
                "scratch",
                "task_workspaces",
                "health_inventory",
                "script_compare",
            ],
            "HostMaintenance owns the host maintenance fields"
        );
        let owner = ["Dis", "patcher"].concat();
        for bad in [
            format!("&mut {owner}"),
            format!("&{owner}"),
            format!("impl {owner}"),
            format!("crate::{}", owner.to_lowercase()),
            format!("super::{}", owner.to_lowercase()),
        ] {
            assert!(
                !src.contains(&bad),
                "host_maintenance.rs must not contain {bad:?}"
            );
        }
    }
}
