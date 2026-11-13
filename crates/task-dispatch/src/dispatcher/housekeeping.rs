//! disk guard・scratch pool の GC・後片付けの `Dispatcher` の側の入口（ADR-0074 F5-1、ADR-0075、ADR-0043 D2）。
//! ADR-0082 の L1。状態と処理は [`HostMaintenance`]（`crate::host_maintenance`。ADR 2026-11-11
//! dispatcher-state-ownership D1・D2）が持つ。ここは [`MaintenanceInputs`] と、実行中の run・判定中の review の
//! task の集合を組んで owner の method を呼ぶ入口と、verify の判定・承認と通知 feed の照合・検査の
//! `CARGO_TARGET_DIR` だけを持つ。

use super::*;
use crate::host_maintenance::{HostMaintenance, MaintenanceInputs};

impl Dispatcher {
    /// ADR-0040 D3 / ADR-0041 D5: `--mode verify` の煙試験インスタンスか（`set_eligible_tasks` が目印。celeris は
    /// verify のときだけ呼ぶ）。verify は本番のコピー（staging DB）を検査するだけなので、smoke 以外の task・wait・
    /// 承認・通知を書き換える裏の処理と、外部（クラスタ・共有の scratch）への副作用を起こさない。
    pub(super) fn verify_smoke_only(&self) -> bool {
        self.eligible.is_some()
    }

    /// Phase F7: 認可元のタスクが終端のまま未決で残った認可の要求を閉じる（照合。通常は遷移が閉じる）。
    /// ADR-0080 D4: 期限の過ぎた browser の wait を一度だけ終端化する（起動直後の最初の tick が
    /// 再起動時の照合を兼ねる）。人待ちの task は lease を持たないので worker slot は使っていない。
    /// ADR-0041 D5: verify は staging DB の承認と browser の wait（とその task）を書き換えない。
    pub(super) fn expire_stale_approvals_and_browser_waits(&self) {
        if self.verify_smoke_only() {
            return;
        }
        let now = OffsetDateTime::now_utc();
        if let Err(e) = crate::approvals::withdraw_stale_approvals(self.store.as_ref(), now) {
            tracing::warn!(error = %e, "failed to withdraw stale approvals");
        }
        match task_ops::browser::expire_due(self.store.as_ref(), now) {
            Ok(expired) => {
                for w in expired {
                    tracing::info!(
                        task_id = %w.task_id,
                        wait_id = %w.wait_id,
                        reason = w.reason.as_str(),
                        "browser wait expired"
                    );
                }
            }
            Err(e) => tracing::warn!(error = %e, "failed to expire browser waits"),
        }
    }

    /// Derive the notice feed once per tick, after task transitions have settled.
    pub(super) fn sync_notice_feed(&self, now: OffsetDateTime) {
        // ADR-0041 D5: verify は staging DB の通知 feed を書かない。
        if self.verify_smoke_only() {
            return;
        }
        if let Err(error) = task_ops::notify_feed::sync_notifications(self.store.as_ref(), now) {
            tracing::warn!(%error, "failed to synchronize notice feed");
        }
    }
    /// owner と、owner の外の値（store・config の値・注入時計・verify の印）を分けて借りる。
    pub(super) fn maintenance_parts(&mut self) -> (&mut HostMaintenance, MaintenanceInputs<'_>) {
        let now = self.now_utc();
        let verify_smoke = self.verify_smoke_only();
        (
            &mut self.maint,
            MaintenanceInputs {
                store: &self.store,
                scratch: &self.config.scratch,
                shared_build_cache: self.config.shared_build_cache,
                build_cache_dir: &self.config.build_cache_dir,
                releases_dir: self.config.releases_dir.as_deref(),
                min_free_disk_pct: self.config.min_free_disk_pct,
                workspace_roots: &self.config.workspace_roots,
                workspace_prune_after_secs: self.config.workspace_prune_after_secs,
                verify_smoke,
                now,
            },
        )
    }

    /// ADR-0075: scratch pool を使うか（`shared_build_cache` かつ `[scratch]` が有効〈NFS で無効化されていない〉）。
    pub(super) fn scratch_active(&self) -> bool {
        crate::host_maintenance::scratch_active(
            self.config.shared_build_cache,
            &self.config.scratch,
        )
    }

    /// tick の頭（drain より前）: 新しい run を始めてよい空きがあるかを決める（draining なら常に可）。
    pub(super) fn refresh_disk_ready(&mut self) {
        let accepting = self.accepting_new_work;
        let (maint, inputs) = self.maintenance_parts();
        maint.refresh_disk_ready(&inputs, accepting);
    }

    /// tick の段 `scratch_and_build_caches`（ADR-0075 D2・ADR-0074 F5-fix・ADR-0129 (4)(5)）。
    pub(super) fn tick_build_caches(&mut self) {
        let (maint, inputs) = self.maintenance_parts();
        maint.tick_build_caches(&inputs);
    }

    /// ADR-0043 D2: ワーカーのカレントディレクトリ（先頭のリポジトリ）。レビューの判定コマンドもここで動かす。
    /// ADR-0074 F5-fix（不具合 1）: daemon が走らせる判定コマンド（WU の checks・統合 WU の検査・
    /// reviewer の checks）に与える `CARGO_TARGET_DIR`。`run_worker` と同じ条件（共有ビルドキャッシュが
    /// 有効、ローカルの git の作業場所）で、`work_unit_id` が `Some` なら `<repo-key>/wu-<id>`、`None` なら
    /// `<repo-key>`。条件に当たらなければ空（従来どおり daemon の環境を継ぐ）。
    /// ADR-0129 (1): `remove` は常に空。検査の子プロセスは daemon から継いだ env（`RUSTC_WRAPPER` / `SCCACHE_*` を
    /// 含む）をそのまま持ち、`set` を重ねるだけ。
    pub(super) fn check_cargo_target_env(
        &self,
        task: &Task,
        work_unit_id: Option<&str>,
    ) -> task_worker::scratch::CargoEnv {
        use task_worker::scratch::CargoEnv;
        if !self.config.shared_build_cache {
            return CargoEnv::default();
        }
        // ADR 2026-10-10-local-disk-growth-paths D1: 自分の git の作業場所が無い（`repos=[]` の子 task が親の
        // checkout を使う・shared の Rust checkout）ときも、run と同じ代わりの checkout に結び付ける。
        let Some(repo) = self.cargo_target_repo(task) else {
            return CargoEnv::default();
        };
        let repo = &repo;
        // ADR-0075 D3（Phase G1）: scratch が有効なら run と同じ owner の target（lease を touch する。adopt はしない）。
        if self.scratch_active() {
            let owner = match work_unit_id {
                Some(id) => task_worker::scratch::Owner::work_unit(task.id.to_string(), id),
                None => task_worker::scratch::Owner::task(task.id.to_string()),
            };
            let settings = task_worker::scratch::ScratchSettings {
                adopt: false,
                ..self.config.scratch.clone()
            };
            // tick の中なので待たず、seed も写さない（ADR 2026-11-11-scratch-lock-tick-stall）。
            task_worker::scratch::try_allocate_for_check(&settings.pool(), &owner, &repo.source);
            // ADR-0129 (1): run と同じ env（`CARGO_TARGET_DIR` と `[scratch.cargo]`。sccache 系は足さない）。
            return super::worker_task::scratch_cargo_env(&settings, &owner);
        }
        let dir = match work_unit_id {
            Some(id) => task_worker::build_cache::work_unit_cargo_target_dir(
                &self.config.build_cache_dir,
                &repo.source,
                id,
            ),
            None => task_worker::build_cache::cargo_target_dir(
                &self.config.build_cache_dir,
                &repo.source,
            ),
        };
        CargoEnv::set_only(vec![(
            task_worker::build_cache::CARGO_TARGET_DIR_VAR.to_string(),
            dir.display().to_string(),
        )])
    }

    /// ADR 2026-10-07-build-tmp-hygiene 付記 A1（2026-10-09）: reviewer run の adapter に、検査と同じ
    /// `CARGO_TARGET_DIR`（[`Self::check_cargo_target_env`] の `work_unit_id = None`）を重ねる。条件に当たらない
    /// （共有キャッシュ無効・git でない）なら env は空で、adapter をそのまま返す。重ねられない adapter で、
    /// 先頭 repo が Rust（`Cargo.toml` がある）なら警告する。
    pub(super) fn with_review_cargo_env(
        &self,
        task: &Task,
        adapter: Arc<dyn WorkerAdapter>,
    ) -> (Arc<dyn WorkerAdapter>, Option<PathBuf>) {
        let env = self.check_cargo_target_env(task, None);
        if env.set.is_empty() {
            return (adapter, None);
        }
        let target = env.set.iter().find(|(k, _)| k == "CARGO_TARGET_DIR");
        let target = target.map(|(_, v)| PathBuf::from(v));
        let Ok(wrapped) = task_worker::configure_env(&adapter, env.set) else {
            let rust = self
                .cargo_target_repo(task)
                .is_some_and(|r| r.dir.join("Cargo.toml").is_file());
            if rust {
                tracing::warn!(task_id = %task.id, adapter = %adapter.id(), "reviewer adapter does not take env; CARGO_TARGET_DIR was not applied (ADR 2026-10-07-build-tmp-hygiene A1)");
            }
            return (adapter, None);
        };
        (wrapped, target)
    }

    /// ADR-0043 D2: **中止**されたタスクの worktree とブランチを消す（終端では消さない）。実行中の run と
    /// 判定中の review の task は、ここで `running`・`reviewing` から組んで owner に渡す（その作業場所は消さない）。
    pub(super) fn cleanup_cancelled_worktrees(&mut self) -> Result<(), DispatchError> {
        let in_flight = self.tasks_in_flight();
        let (maint, inputs) = self.maintenance_parts();
        maint.cleanup_cancelled_worktrees(&inputs, &in_flight)?;
        Ok(())
    }

    /// run（`running`）か review（`reviewing`）が走っている task（`RunSupervisor::runs_for_task` が 1 以上か判定中）。
    pub(super) fn tasks_in_flight(&self) -> std::collections::HashSet<TaskId> {
        self.supervisor
            .run_keys()
            .map(|k| k.task)
            .chain(self.supervisor.reviews().keys().copied())
            .collect()
    }

    /// ADR-0066 D2（Phase 110b）: 終端から `prune_after_secs` 経った作業場所のビルド生成物を刈る（1 tick に最大 1 か所）。
    pub(super) fn prune_one_workspace(&mut self) {
        let (maint, inputs) = self.maintenance_parts();
        maint.prune_one_workspace(&inputs);
    }

    /// ADR 2026-10-10-local-disk-growth-paths D2: 終端の木の repo 直下 target の常設の掃除。
    pub(super) fn sweep_repo_targets(&mut self) {
        let (maint, inputs) = self.maintenance_parts();
        maint.sweep_repo_targets(&inputs);
    }
}
