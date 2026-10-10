//! disk guard・scratch pool の GC・後片付け（ADR-0074 F5-1、ADR-0075、ADR-0043 D2）。ADR-0082 の L1。

use super::*;

impl Dispatcher {
    /// Derive the notice feed once per tick, after task transitions have settled.
    pub(super) fn sync_notice_feed(&self, now: OffsetDateTime) {
        if let Err(error) = task_ops::notify_feed::sync_notifications(self.store.as_ref(), now) {
            tracing::warn!(%error, "failed to synchronize notice feed");
        }
    }
    /// ADR-0075: scratch pool を使うか（`shared_build_cache` かつ `[scratch]` が有効〈NFS で無効化されていない〉）。
    pub(super) fn scratch_active(&self) -> bool {
        self.config.shared_build_cache && self.config.scratch.enabled
    }

    /// ADR-0075 D2（Phase G1）: tick の `scratch_gc` phase。lease と DB の状態を読み、`plan_gc` で決めた target を
    /// `.deleting-*` へ rename する（`remove_dir_all` は削除スレッド、サイズは測定スレッド）。`emergency` は空き <
    /// `min_free_disk_mb` のときの緊急モード。rename した件数を返す。
    pub(super) fn scratch_gc(&mut self, emergency: bool) -> usize {
        use task_worker::scratch::{GIB, Pressure};
        let settings = self.config.scratch.clone();
        let legacy = crate::scratch_gc::legacy_paths(
            &self.config.build_cache_dir,
            self.config.releases_dir.as_deref(),
        );
        let sizes = self
            .scratch
            .sizes
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default();
        let min_free = self.config.min_free_disk_mb.saturating_mul(1024 * 1024);
        let lookup = task_worker::scratch::StoreLookup(self.store.as_ref());
        let run = crate::scratch_gc::run_gc(
            &settings,
            &legacy,
            &lookup,
            true,
            &sizes,
            min_free,
            emergency,
            &std::collections::BTreeSet::new(),
            false,
        );
        let now = std::time::SystemTime::now();
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
        let removed: std::collections::BTreeSet<String> =
            executed.removed.iter().map(|p| p.id.clone()).collect();
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
        if !self
            .scratch
            .removing
            .load(std::sync::atomic::Ordering::SeqCst)
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
            && !self
                .scratch
                .measuring
                .load(std::sync::atomic::Ordering::SeqCst)
            && let Some((path, lease)) = crate::scratch_gc::next_to_measure(&run.scan, &sizes)
        {
            self.scratch.last_measure = Some(Instant::now());
            crate::scratch_gc::spawn_measure(
                path,
                lease,
                self.scratch.sizes.clone(),
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
    pub(super) fn seed_housekeeping(&mut self, now: Instant) {
        use std::sync::atomic::Ordering;
        if !self.scratch_active() || !self.config.scratch.seed_reflink {
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
        let repos = self.seed_repos();
        let settings = self.config.scratch.clone();
        let pool = settings.pool();
        let registered: std::collections::BTreeSet<String> = repos
            .iter()
            .map(|(p, _)| task_worker::build_cache::repo_cache_key(p))
            .collect();
        let gc = task_worker::scratch::seed_gc(&pool, &registered, std::time::SystemTime::now());
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
        let min_free = self.config.min_free_disk_mb.saturating_mul(1024 * 1024);
        let busy = self.scratch.seed_refreshing.clone();
        let spawned = std::thread::Builder::new()
            .name("celeris-scratch-seed".to_string())
            .spawn({
                let busy = busy.clone();
                move || {
                    let seed_repos: Vec<crate::scratch_gc::SeedRepo> = repos
                        .iter()
                        .map(|(path, configured)| {
                            let branch =
                                task_ops::changes::default_branch(path, configured.as_deref());
                            let commit = task_worker::scratch::resolve_commit(path, &branch);
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
                        task_worker::scratch::seed_refresh_hold(
                            pressure_high,
                            crate::scratch_gc::fs_stats(&root).map(|f| f.1),
                            min_free,
                            seed_bytes,
                        )
                    };
                    crate::scratch_gc::refresh_seeds(
                        &settings,
                        &seed_repos,
                        &task_worker::scratch::rustc_version,
                        &hold,
                        &task_worker::scratch::SeedBuildOps::real(),
                        std::time::SystemTime::now(),
                    );
                    busy.store(false, Ordering::SeqCst);
                }
            });
        if let Err(e) = spawned {
            busy.store(false, Ordering::SeqCst);
            tracing::warn!(error = %e, "scratch: could not spawn the seed refresh thread");
        }
    }

    /// seed の対象: 登録された全案件の local の git repo（path と設定の既定ブランチ）。
    fn seed_repos(&self) -> Vec<(PathBuf, Option<String>)> {
        let projects = match self.store.project_list() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "scratch: could not list projects for the seeds");
                return Vec::new();
            }
        };
        let mut out: Vec<(PathBuf, Option<String>)> = Vec::new();
        for project in projects {
            let Ok(repos) = self.store.repo_list(project.id) else {
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

    /// ディスク不足は Phase 116 の infra 障害として一度だけ通知し、空きが戻ると自動で解除する。
    /// 検査不能も安全側に倒して run を開始しない。
    pub(super) fn check_disk_space(&mut self) -> bool {
        if self.config.min_free_disk_mb == 0 {
            self.disk_low = false;
            return true;
        }
        let mut paths: Vec<PathBuf> = vec![
            PathBuf::from("/"),
            self.config.workspace_root.clone(),
            self.config.build_cache_dir.clone(),
        ];
        // ADR-0075 D3: scratch pool の filesystem も見る。
        if self.scratch_active() {
            paths.push(self.config.scratch.dir.clone());
        }
        let min_free = self.config.min_free_disk_mb;
        let find_low = move |paths: &[PathBuf]| {
            paths.iter().find_map(|path| match free_disk_mb(path) {
                Ok(free) if free < min_free => Some(format!(
                    "{}: {free} MiB free (minimum {min_free} MiB)",
                    path.display(),
                )),
                Err(error) => Some(error),
                _ => None,
            })
        };
        let mut low = find_low(&paths);
        // ADR-0075 D3: 空きが足りなければ、新しい run を始める前にこの tick で緊急 GC（rename まで）を回す。
        // 削除は別スレッドなので空きが戻るのは数 tick 後。その間は下の保留と通知 1 回（attempts を消費しない）。
        let mut pinned_note = None;
        if low.is_some() && self.scratch_active() {
            let selected = self.scratch_gc(true);
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
                    match self.store.notice_record(&task_core::feed::NoticeEvent {
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
            allocate_scratch_target(&settings, &[], &owner, repo, None);
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
        match adapter.with_env(&env.set) {
            Some(wrapped) => {
                let target = env
                    .set
                    .iter()
                    .find(|(k, _)| k == "CARGO_TARGET_DIR")
                    .map(|(_, v)| PathBuf::from(v));
                (wrapped, target)
            }
            None => {
                let rust = self
                    .cargo_target_repo(task)
                    .is_some_and(|r| r.dir.join("Cargo.toml").is_file());
                if rust {
                    tracing::warn!(task_id = %task.id, adapter = %adapter.id(), "reviewer adapter does not support with_env; CARGO_TARGET_DIR was not applied (ADR 2026-10-07-build-tmp-hygiene A1)");
                }
                (adapter, None)
            }
        }
    }

    /// ADR-0074 F5-fix（不具合 1）: 終端（done / cancelled / superseded）になった WU の target
    /// （`<build_cache_dir>/cargo/<repo-key>/wu-<id>`）を消す。消す前に同じ親の中で
    /// `.deleting-wu-<id>` へ rename し（この tick のうちにパスから消える）、中身の削除は別スレッドで
    /// 行う（数 GB になりうるため tick を止めない）。前回途中で残った `.deleting-*` も消す。
    /// 行が見つからない WU（別の DB・消えた Task）の target には触れない。
    pub(super) fn cleanup_work_unit_build_caches(&mut self) {
        if !self.config.shared_build_cache {
            return;
        }
        if self
            .removing_build_caches
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let root = task_worker::build_cache::cargo_root(&self.config.build_cache_dir);
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
                match self.store.work_unit_get(id) {
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
        busy.store(true, std::sync::atomic::Ordering::SeqCst);
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
                    busy.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            });
        if let Err(e) = spawned {
            busy.store(false, std::sync::atomic::Ordering::SeqCst);
            tracing::warn!(error = %e, "build cache: could not spawn the removal thread");
        }
    }

    /// ADR-0043 D2（Phase 52。ADR-0041 D1 の後片付けを改める）: worktree は**終端では消さない**
    /// （`done` で未取り込みのものも `failed` も、差分を見るために残す）。消えるのは**中止**（cancel）
    /// のときだけで、worktree を消し、ブランチも `git branch -D` する（人の指示）。
    ///
    /// `done` / `failed` のときは、未コミットの変更があれば `WorkerProgress` を 1 行積んで記録だけ落とす。
    /// **celeris はコミットしない**（ADR-0019 D2）。
    pub(super) fn cleanup_cancelled_worktrees(&mut self) -> Result<(), DispatchError> {
        let ids: Vec<TaskId> = self.task_workspaces.keys().copied().collect();
        for id in ids {
            // まだ走っている／判定中なら触らない（やり直しは同じ worktree を使い回す）。
            if self.running_for_task(id) > 0 || self.reviewing.contains_key(&id) {
                continue;
            }
            let status = match self.store.get(id)? {
                Some(t) => Some(t.status),
                // タスクごと消えていれば、記録も落とす（worktree は人の手に残す）。
                None => None,
            };
            match status {
                None => {
                    self.task_workspaces.remove(&id);
                }
                Some(Status::Cancelled) => {
                    let (targets, _) = task_worker::workspace_targets::task_targets(
                        &self.config.workspace_root.join(id.to_string()),
                    );
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
                Some(status) if status.is_terminal() => {
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
                        let run_id = last_run_id(&self.store.events_for(id)?).unwrap_or_default();
                        self.store.append_event(
                            id,
                            &Event::worker_progress(
                                run_id,
                                format!("未コミットの変更が残っています: {}", dirty.join(", ")),
                            ),
                        )?;
                    }
                }
                Some(_) => {}
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
    pub(super) fn prune_one_workspace(&mut self) {
        if self.eligible.is_some() {
            return;
        }
        if self.config.workspace_prune_after_secs == 0 {
            return;
        }
        let now = OffsetDateTime::now_utc();
        let candidate = match task_worker::workspace_prune::find_prune_candidate(
            self.store.as_ref(),
            &self.config.workspace_root,
            now,
            self.config.workspace_prune_after_secs,
        ) {
            Ok(Some(c)) => c,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "workspace prune: could not scan for candidates");
                return;
            }
        };
        let store = self.store.clone();
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
}
