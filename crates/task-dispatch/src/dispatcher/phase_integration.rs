//! 工程の統合（`start_integration`・merge repair・`finish_phase_integration`）と途中報（phase report）、再起動時の統合と WU run の照合。ADR-0082 の L3。

use super::*;
use std::collections::BTreeMap;

/// ADR-0118 D4 / D5: 同じ子の merge candidate が古くなったときの自動の再 sync・再レビューの回数
/// （初回に加えて 2 回 = 合計 3 attempt）。超えたら統合を諦めて理由を残す。
const MAX_STALE_CANDIDATE_RETRIES: usize = 2;

/// PhaseIntegrated check record for an auto-resolve action.
const AUTO_RESOLVE_CHECK: &str = "auto_resolve";

/// ADR-0118 D5: 子の候補が古くなって統合 WU・子の unit を戻すときの `WorkUnitTransitioned.reason`。
pub(super) const MERGE_CANDIDATE_STALE_REASON: &str = "merge_candidate_stale";

/// ADR-0118 D5: 子の unit の key → リポジトリ → その子の review が固定した merge candidate。
pub(super) type ChildCandidates =
    BTreeMap<String, BTreeMap<task_core::RepoId, crate::integration::MergeCandidate>>;

/// Repair に渡す unit の変更許可パスと差分範囲 check を重複なく安定した順序にする。
fn repair_scope_from_units<'a>(
    units: impl IntoIterator<Item = &'a task_core::WorkUnitRow>,
) -> task_core::RepairScope {
    let mut allowed_paths = std::collections::BTreeSet::new();
    let mut scope_checks = std::collections::BTreeSet::new();
    for unit in units {
        allowed_paths.extend(unit.spec.context.paths.iter().cloned());
        scope_checks.extend(
            unit.spec
                .checks
                .iter()
                .filter(|check| check.cmd.contains("git diff"))
                .map(|check| check.cmd.clone()),
        );
    }
    task_core::RepairScope {
        allowed_paths: allowed_paths.into_iter().collect(),
        scope_checks: scope_checks.into_iter().collect(),
    }
}

/// 2026-10-04 統合の検査の進み具合 D2: 統合の検査のログを置く Task の作業ディレクトリ直下のディレクトリ。
pub(crate) const INTEGRATION_CHECK_LOG_DIR: &str = "integration-checks";
/// 2026-10-04 WU 検査の引き継ぎ D3: 葉の WU の受け入れ検査のログの置き場所（`<task_dir>/work-unit-checks/<wu_key>`）。
pub(crate) const WORK_UNIT_CHECK_LOG_DIR: &str = "work-unit-checks";

/// 統合の検査を event とログに残すための、その統合 WU の識別とログの置き場所。
/// 2026-10-04 WU 検査の引き継ぎ D3: 葉の WU の受け入れ検査にも使う（`run_id` が `Some` なら
/// `WorkUnitCheckStarted` / `WorkUnitCheckFinished` を、`None`〈統合〉なら `IntegrationCheck*` を残す）。
pub(crate) struct ObservedIntegration {
    pub work_unit_id: String,
    pub key: String,
    /// 検査を起こした worker run（葉の WU の受け入れ検査のときだけ）。
    pub run_id: Option<String>,
    /// `<task_dir>/integration-checks/<wu_key>`。検査 1 件ごとに `<started_ms>-<index>.log` を作る。
    pub log_dir: PathBuf,
}

/// 2026-10-04 統合の検査の進み具合 D1/D2: 統合の検査を順に走らせ、1 件ごとに `IntegrationCheckStarted` /
/// `IntegrationCheckFinished` を追記し、出力をログファイルへ逐次書く。判定は `run_work_unit_checks` と同じ
/// （再実行・merge-base の修復を含む）。event を書けなくても検査は続ける（見るための記録で、判定には使わない）。
pub(crate) async fn run_integration_checks(
    store: &dyn task_core::TaskStore,
    task_id: TaskId,
    observed: &ObservedIntegration,
    ws: &task_worker::LocalWorkspace,
    checks: &[task_core::WorkUnitCheck],
    command_timeout: Duration,
) -> Vec<(bool, String)> {
    let total = u32::try_from(checks.len()).unwrap_or(u32::MAX);
    let attempt_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let mut out = Vec::with_capacity(checks.len());
    for (i, c) in checks.iter().enumerate() {
        let index = u32::try_from(i).unwrap_or(u32::MAX);
        let log_path = observed.log_dir.join(format!("{attempt_ms}-{index}.log"));
        let started = match &observed.run_id {
            Some(run_id) => Event::WorkUnitCheckStarted {
                work_unit_id: observed.work_unit_id.clone(),
                key: observed.key.clone(),
                run_id: run_id.clone(),
                index,
                total,
                cmd: c.cmd.clone(),
                log_path: log_path.display().to_string(),
                started_at: rfc3339(OffsetDateTime::now_utc()),
            },
            None => Event::IntegrationCheckStarted {
                work_unit_id: observed.work_unit_id.clone(),
                key: observed.key.clone(),
                index,
                total,
                cmd: c.cmd.clone(),
                log_path: log_path.display().to_string(),
                started_at: rfc3339(OffsetDateTime::now_utc()),
            },
        };
        if let Err(e) = store.append_event(task_id, &started) {
            tracing::warn!(%task_id, work_unit = %observed.key, error = %e, "could not record the integration check start");
        }
        let clock = std::time::Instant::now();
        let check_ws = ws.clone().with_output_log(&log_path);
        let outcome = crate::review::exec_check_outcome(
            &check_ws,
            &c.cmd,
            c.expect_exit,
            command_timeout,
            "",
        )
        .await;
        let duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        let finished = match &observed.run_id {
            Some(run_id) => Event::WorkUnitCheckFinished {
                work_unit_id: observed.work_unit_id.clone(),
                key: observed.key.clone(),
                run_id: run_id.clone(),
                index,
                total,
                cmd: c.cmd.clone(),
                pass: outcome.pass,
                exit: outcome.exit,
                timed_out: outcome.timed_out,
                duration_ms,
            },
            None => Event::IntegrationCheckFinished {
                work_unit_id: observed.work_unit_id.clone(),
                key: observed.key.clone(),
                index,
                total,
                cmd: c.cmd.clone(),
                pass: outcome.pass,
                exit: outcome.exit,
                timed_out: outcome.timed_out,
                duration_ms,
            },
        };
        if let Err(e) = store.append_event(task_id, &finished) {
            tracing::warn!(%task_id, work_unit = %observed.key, error = %e, "could not record the integration check finish");
        }
        out.push((outcome.pass, outcome.reason));
    }
    out
}

impl Dispatcher {
    /// ADR parallel integration D4: 統合の依頼を追記事象として一件残す。同じ組の未回答依頼があれば追記しない
    /// （再 tick で重ならない）。一般通知（notice）には書かない。受信箱の項目は事象から投影する。
    fn record_integration_request(
        &self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        request: &crate::auto_resolve::IntegrationRequest,
    ) -> Result<(), DispatchError> {
        self.store
            .integration_request_record(task.id, request, &format!("phase:{}", integ.key))?;
        Ok(())
    }
    /// ADR-0074 D1.7（Phase F2）: 走らせている spawn の無い `integrate-<phase>`（running）を pending に
    /// 戻す（次の tick で冪等な手順でやり直す）。
    pub(super) fn reconcile_integration(
        &self,
        task_id: TaskId,
        reason: &str,
    ) -> Result<(), DispatchError> {
        for wu in self.store.work_units_for(task_id)? {
            if wu.kind != task_core::WorkUnitKind::Integrate
                || wu.status != task_core::WorkUnitStatus::Running
            {
                continue;
            }
            let mut updated = wu.clone();
            updated.status = task_core::WorkUnitStatus::Pending;
            updated.clear_lease();
            updated.updated_at = rfc3339(OffsetDateTime::now_utc());
            self.store.work_unit_transition(
                task_id,
                updated,
                Event::WorkUnitTransitioned {
                    work_unit_id: wu.id.clone(),
                    key: wu.key.clone(),
                    from: task_core::WorkUnitStatus::Running,
                    to: task_core::WorkUnitStatus::Pending,
                    reason: reason.to_string(),
                    run_id: None,
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn reconcile_work_unit_run(
        &self,
        task_id: TaskId,
        run_id: &str,
        reason: &str,
    ) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task_id)?;
        let Some(wu) = units.iter().find(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.last_run_id.as_deref() == Some(run_id)
        }) else {
            return Ok(());
        };
        let has_checkpoint = self
            .store
            .runs_for_work_unit(&wu.id)?
            .iter()
            .any(|r| r.checkpoint.is_some());
        let mut updated = wu.clone();
        updated.status = if has_checkpoint {
            task_core::WorkUnitStatus::NeedsContinuation
        } else {
            task_core::WorkUnitStatus::Ready
        };
        updated.clear_lease();
        self.store.work_unit_transition(
            task_id,
            updated.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: wu.id.clone(),
                key: wu.key.clone(),
                from: task_core::WorkUnitStatus::Running,
                to: updated.status,
                reason: reason.to_string(),
                run_id: Some(run_id.to_string()),
            },
        )?;
        Ok(())
    }

    /// ADR-0074 D1.4（Phase F2b）: 統合の間の Task の lease（工程の保持者）の期限。
    pub(super) fn integration_ttl(&self) -> Duration {
        self.config.review_timeout.saturating_mul(4) + self.config.lease_grace
    }

    /// ADR-0074 D1.4/D1.7（Phase F2b）: Ready の Task で統合を始める（再起動の照合で pending に戻った
    /// 統合のやり直しなど）。Task の lease を工程の保持者で取ってから [`Self::start_integration`]。
    pub(super) fn start_integration_from_ready(
        &mut self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> Result<(), DispatchError> {
        if self.just_aborted.contains(&task.id) || !self.is_eligible(task) {
            return Ok(());
        }
        let holder = format!(
            "{PHASE_LEASE_PREFIX}{}:{}:{}",
            wu.plan_id,
            wu.phase.as_deref().unwrap_or_default(),
            ulid::Ulid::new()
        );
        if !self
            .store
            .acquire_lease(task.id, &holder, self.integration_ttl())?
        {
            return Ok(());
        }
        self.start_integration(task, &wu.id)
    }

    /// ADR-0079 D6（Phase R1c）: kind task の unit `unit` から作る子 task の worktree の基点（親の先頭の git
    /// リポジトリの sha）。葉の WU と同じ規則（`prepare_work_unit_workspace`）: 同じ段階の依存先があれば
    /// `integration::dependency_base`（依存先が子 task ならその子のブランチの HEAD）、無ければ親の task
    /// ブランチの HEAD（= 段階の基点。親のブランチは段階の途中では動かない）。親が WU の worktree を持たない
    /// （並列 1 に倒した: remote / shared / git でない）なら `None`（子もブランチを持たず、統合は子を merge しない。D5）。
    /// `Err` は子を作れない理由（unit を `failed` にする）。
    pub(super) fn child_base_commit(
        &self,
        parent: &Task,
        unit: &task_core::WorkUnitRow,
        units: &[task_core::WorkUnitRow],
    ) -> Result<Option<String>, String> {
        let mode = self.parallel_mode(parent).map_err(|e| e.to_string())?;
        if !mode.worktrees || mode.fallback.is_some() {
            return Ok(None);
        }
        let Some(ws) = self.task_workspaces_for(parent) else {
            return Ok(None);
        };
        let Some((repo, task_wt)) = ws
            .repos
            .iter()
            .find_map(|r| r.worktree.as_ref().map(|wt| (r, wt)))
        else {
            return Ok(None);
        };
        // 親の task ブランチ（段階の基点）を先に用意する（段階の unit が子だけのときはまだ無い）。
        task_wt
            .ensure_blocking()
            .map_err(|e| format!("cannot prepare the parent's worktree: {e}"))?;
        let intra_dep = unit.depends_on.iter().find_map(|d| {
            units
                .iter()
                .find(|u| &u.key == d && u.phase.is_some() && u.phase == unit.phase)
        });
        let base = match intra_dep {
            Some(dep) => crate::integration::dependency_base(
                &repo.source,
                &parent.id.to_string(),
                dep,
                &task_wt.branch,
                &self.config.worktree_branch_prefix,
            )?,
            None => crate::integration::rev_parse(
                &repo.source,
                &format!("refs/heads/{}", task_wt.branch),
            )
            .ok_or_else(|| format!("the parent's task branch {} does not exist", task_wt.branch))?,
        };
        Ok(Some(base))
    }

    /// ADR-0079 D6（Phase R1c）: done になった子 task の worktree で、残った変更を決定的に commit する
    /// （WU の完了時と同じ `integration::commit_all`、メッセージ `task/<child_id>: <title>`。変更が無ければ
    /// commit しない）。戻り値は子のブランチと、先頭の git リポジトリのその HEAD。子の worktree が既に無ければ
    /// ブランチの HEAD を元のリポジトリで引く。子がブランチを持たない（shared / remote）なら `None`。
    pub(super) fn commit_child_branch(&self, child: &Task) -> Option<(String, String)> {
        let ws = self.task_workspaces_for(child)?;
        let mut first: Option<(String, String)> = None;
        for repo in &ws.repos {
            let Some(wt) = &repo.worktree else {
                continue;
            };
            let head = if wt.dir.is_dir() {
                match crate::integration::commit_all(
                    &wt.dir,
                    &format!("task/{}: {}", child.id, child.title),
                ) {
                    Ok((head, _)) => Some(head),
                    Err(e) => {
                        tracing::warn!(child_id = %child.id, repo = %repo.name, error = %e, "could not commit the child task's remaining changes (ADR-0079 D6)");
                        crate::integration::rev_parse(&wt.dir, "HEAD")
                    }
                }
            } else {
                crate::integration::rev_parse(&wt.repo, &format!("refs/heads/{}", wt.branch))
            };
            if let Some(head) = head {
                first.get_or_insert((wt.branch.clone(), head));
            }
        }
        first
    }

    /// ADR-0079 D6（Phase R1c）: 段階 `phase` の統合が済んだ子 task の worktree を消す（ブランチは残す）。
    /// 失敗は警告だけ（取り込みは済んでいる。後片付けで段階を止めない）。
    pub(super) fn remove_integrated_child_worktrees(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
        phase: &str,
    ) {
        // ADR-0079 D15（Phase R5b-prep）: 採用（`adopt`）した task の worktree は木が作ったものではないので消さない
        // （採用しても対象の履歴・作業場所は変えない）。
        let adopted: std::collections::BTreeSet<String> = self
            .store
            .execution_plan_active(task_id)
            .ok()
            .flatten()
            .map(|p| {
                p.spec
                    .units
                    .iter()
                    .filter(|u| u.adopt.is_some())
                    .map(|u| u.key.clone())
                    .collect()
            })
            .unwrap_or_default();
        for u in units.iter().filter(|u| {
            u.kind == task_core::WorkUnitKind::Task
                && u.status == task_core::WorkUnitStatus::Done
                && u.phase.as_deref() == Some(phase)
                && !adopted.contains(&u.key)
        }) {
            let Some(child) = u
                .child_task_id
                .as_deref()
                .and_then(|id| id.parse::<TaskId>().ok())
                .and_then(|id| self.store.get(id).ok().flatten())
            else {
                continue;
            };
            let Some(ws) = self.task_workspaces_for(&child) else {
                continue;
            };
            for wt in ws.repos.iter().filter_map(|r| r.worktree.as_ref()) {
                if let Err(e) = crate::integration::remove_wu_worktree(wt) {
                    tracing::warn!(%task_id, work_unit = %u.key, child_id = %child.id, error = %e, "could not remove the child task's worktree after integration (ADR-0079 D6)");
                }
            }
        }
    }

    /// ADR-0074 D1.4 / ADR-0079 D5・D6（Phase R1c）: 段階 `phase` の統合で merge するブランチ（`seq` 順）と、
    /// ブランチが必ずあるはずの子（`(unit key, branch)`。子の worktree を切った = `tree.base_commit` を持つ子）。
    /// - 葉の WU: `celeris-wu/<task>/<key>`（`phase_leaves`。従来どおり）。
    /// - done の kind task の unit: 子 task のブランチ `<prefix><child_id>`（`MergeItem::child_task`。
    ///   葉かどうかに関わらず入れる。子に依存する葉があっても子の commit は 1 度だけ入る）。子が
    ///   `workspace_mode = shared` か remote でブランチを持たなければ（`base_commit` が無い）、どのリポジトリにも
    ///   無くて構わない（D5 の「merge しない」）。
    pub(super) fn integration_items(
        &self,
        units: &[task_core::WorkUnitRow],
        phase: &str,
    ) -> (
        Vec<crate::integration::MergeItem>,
        Vec<(String, String)>,
        ChildCandidates,
    ) {
        let mut ordered: Vec<(u32, crate::integration::MergeItem)> =
            task_core::phase_leaves(units, phase)
                .into_iter()
                .filter_map(|u| {
                    u.branch.clone().map(|branch| {
                        (
                            u.seq,
                            crate::integration::MergeItem::work_unit(&u.key, branch),
                        )
                    })
                })
                .collect();
        let mut expected = Vec::new();
        let mut candidates = ChildCandidates::new();
        for u in units.iter().filter(|u| {
            u.kind == task_core::WorkUnitKind::Task
                && u.status == task_core::WorkUnitStatus::Done
                && u.phase.as_deref() == Some(phase)
        }) {
            let Some(child_id) = u.child_task_id.as_deref() else {
                continue;
            };
            let branch = format!("{}{child_id}", self.config.worktree_branch_prefix);
            let child = child_id
                .parse::<TaskId>()
                .ok()
                .and_then(|id| self.store.get(id).ok().flatten());
            let has_branch = child
                .as_ref()
                .is_some_and(|child| task_core::tree::child_base_commit(child).is_some());
            if let Some(child) = &child {
                let recorded = self.child_merge_candidates(child.id);
                if !recorded.is_empty() {
                    candidates.insert(u.key.clone(), recorded);
                }
            }
            if has_branch {
                expected.push((u.key.clone(), branch.clone()));
            }
            ordered.push((
                u.seq,
                crate::integration::MergeItem::child_task(&u.key, branch),
            ));
        }
        ordered.sort_by_key(|(seq, _)| *seq);
        (
            ordered.into_iter().map(|(_, item)| item).collect(),
            expected,
            candidates,
        )
    }

    /// ADR-0118 D3 / D5: 子 task の最新の review attempt が記録した merge candidate（リポジトリごと）。
    /// `ReviewTargetSynced` は追記順なので、同じリポジトリの後の記録が前の記録を上書きする。記録の無い
    /// 移行前の子は空（照合しない）。event を読めなければ空にして従来どおりにする。
    pub(super) fn child_merge_candidates(
        &self,
        child_id: TaskId,
    ) -> BTreeMap<task_core::RepoId, crate::integration::MergeCandidate> {
        let mut out = BTreeMap::new();
        let events = match self.store.events_for(child_id) {
            Ok(events) => events,
            Err(e) => {
                tracing::warn!(%child_id, error = %e, "could not read the child's review snapshots (ADR-0118 D5)");
                return out;
            }
        };
        for (_, event) in events {
            if let Event::ReviewTargetSynced {
                repo_id,
                target_sha,
                merge_candidate_sha,
                ..
            } = event
            {
                out.insert(
                    repo_id,
                    crate::integration::MergeCandidate {
                        target_sha,
                        merge_candidate_sha,
                    },
                );
            }
        }
        out
    }

    /// ADR-0074 D1.4（Phase F2b）: 工程の統合を始める（統合 WU を running にし、Task の lease を延ばし、
    /// 葉の WU のブランチの merge と検査の再実行を spawn する。git の I/O は tick を止めない）。
    /// 並列 1 に倒した Task（WU の worktree が無い）では no-op（すぐに done）。
    pub(super) fn start_integration(
        &mut self,
        task: &Task,
        integ_id: &str,
    ) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task.id)?;
        let Some(integ) = units.iter().find(|u| u.id == integ_id).cloned() else {
            return Ok(());
        };
        if !matches!(
            integ.status,
            task_core::WorkUnitStatus::Pending | task_core::WorkUnitStatus::Ready
        ) {
            return Ok(());
        }
        let phase = integ.phase.clone().unwrap_or_default();
        let mut running = integ.clone();
        running.status = task_core::WorkUnitStatus::Running;
        running.blocked_reason = None;
        running.updated_at = rfc3339(OffsetDateTime::now_utc());
        self.store.work_unit_transition(
            task.id,
            running,
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: integ.status,
                to: task_core::WorkUnitStatus::Running,
                reason: "integrate".to_string(),
                run_id: None,
            },
        )?;
        if let Err(e) = self
            .store
            .extend_task_lease(task.id, self.integration_ttl())
        {
            tracing::warn!(task_id = %task.id, error = %e, "could not extend the task lease for the integration");
        }
        let mode = self.parallel_mode(task)?;
        let workspaces = self.task_workspaces_for(task);
        let (true, Some(ws)) = (mode.worktrees, workspaces) else {
            // 並列 1（WU は Task の worktree を共有した）: merge するブランチは無い（D1.2）。
            return self.on_integration_finished(task.id, &integ.id, Ok(IntegrationRun::default()));
        };
        // ADR-0079 D15（Phase R5b-prep）: 段階の unit が採用（adopt）した task だけのとき、この段階では leaf も子も
        // 走っていないので Task の worktree（段階の基点のブランチ）がまだ無い。統合の前に用意する（子の生成の前に
        // 親の worktree を用意する `child_base_commit` と同じ。既にあれば何もしない）。
        for wt in ws.repos.iter().filter_map(|r| r.worktree.as_ref()) {
            if let Err(e) = wt.ensure_blocking() {
                return self.on_integration_finished(
                    task.id,
                    &integ.id,
                    Err(format!(
                        "cannot prepare the task's worktree for the integration: {e}"
                    )),
                );
            }
        }
        // ADR-0079 D5 / D6（Phase R1c）: 葉の WU のブランチに、この段階の done の kind task の unit の
        // 子 task のブランチ `celeris/<child_id>` を足し、`seq` 順に merge する（子に依存する同じ段階の葉は
        // 子の HEAD から切られているので、どちらが先でも子の commit は 1 度だけ入る。既に入っていれば飛ばす）。
        let (items, expected_children, candidates) = self.integration_items(&units, &phase);
        let repos: Vec<PathBuf> = ws
            .repos
            .iter()
            .filter(|r| r.is_git())
            .map(|r| r.dir.clone())
            .collect();
        // ADR-0118 D5: リポジトリごとに、子の item へその子の review が固定した merge candidate を付ける。
        let repo_items: Vec<(
            PathBuf,
            Option<task_core::RepoId>,
            Vec<crate::integration::MergeItem>,
        )> = ws
            .repos
            .iter()
            .filter(|r| r.is_git())
            .map(|r| {
                let repo_id = task
                    .repos
                    .iter()
                    .find(|reference| reference.name == r.name)
                    .map(|reference| reference.repo_id);
                let items = items
                    .iter()
                    .map(|item| {
                        let candidate = repo_id.and_then(|id| {
                            candidates.get(&item.key).and_then(|c| c.get(&id)).cloned()
                        });
                        item.clone().with_candidate(candidate)
                    })
                    .collect();
                (r.dir.clone(), repo_id, items)
            })
            .collect();
        // D1.4 の 4: その工程の WU の checks（重複を除く）と workspace.toml の check。
        let mut checks: Vec<task_core::WorkUnitCheck> = Vec::new();
        for u in units.iter().filter(|u| {
            u.status.is_active()
                && u.kind != task_core::WorkUnitKind::Integrate
                && u.phase.as_deref() == Some(phase.as_str())
        }) {
            for c in &u.spec.checks {
                if !checks.iter().any(|x| x.cmd == c.cmd) {
                    checks.push(c.clone());
                }
            }
        }
        for cmd in self.default_checks(task) {
            if !checks.iter().any(|x| x.cmd == cmd) {
                checks.push(task_core::WorkUnitCheck {
                    cmd,
                    expect_exit: 0,
                });
            }
        }
        let task_dir = ws.task_dir.clone();
        // ADR-0074 F5-fix: 統合 WU の検査は Task の worktree で走るので `<repo-key>`。
        let check_env = self.check_cargo_target_env(task, None);
        let timeout = self.config.review_timeout;
        let tx = self.tx.clone();
        let task_id = task.id;
        let work_unit_id = integ.id.clone();
        let wu_id_for_entry = integ.id.clone();
        let store = self.store.clone();
        let observed = ObservedIntegration {
            work_unit_id: integ.id.clone(),
            key: integ.key.clone(),
            run_id: None,
            log_dir: ws.task_dir.join(INTEGRATION_CHECK_LOG_DIR).join(&integ.key),
        };
        let handle = tokio::spawn(async move {
            let phase_for_merge = phase.clone();
            let merged = tokio::task::spawn_blocking(move || -> Result<IntegrationRun, String> {
                let mut run = IntegrationRun::default();
                for (i, (dir, repo_id, items)) in repo_items.iter().enumerate() {
                    let out = crate::integration::integrate(dir, items, &phase_for_merge)?;
                    // Preserve auto-resolve actions alongside each reviewed candidate.
                    run.checks.extend(out.actions.iter().map(|action| {
                        let summary = serde_json::to_string(action)
                            .unwrap_or_else(|_| format!("{action:?}"));
                        (AUTO_RESOLVE_CHECK.to_string(), true, summary)
                    }));
                    if i == 0 {
                        run.merged = out.merged.clone();
                        run.head = out.head.clone();
                    } else {
                        // ADR-0079 D6: 子のブランチは子の repos（親の部分集合）にだけある。先頭の
                        // リポジトリに無かった子の merge も `merged` に残す（1 件目の commit）。
                        for m in &out.merged {
                            if !run.merged.iter().any(|x| x.key == m.key) {
                                run.merged.push(m.clone());
                            }
                        }
                    }
                    if out.conflict.is_some() {
                        run.conflict = out.conflict;
                        break;
                    }
                    if let Some(stale) = out.stale {
                        // candidate は repo_id のあるリポジトリにだけ付くので、ここでは必ず `Some`。
                        let repo_id = repo_id.ok_or_else(|| {
                            format!("merge candidate of {} has no repository id", stale.key)
                        })?;
                        run.stale = Some((repo_id, stale));
                        break;
                    }
                }
                if run.conflict.is_none()
                    && run.stale.is_none()
                    && let Some((key, branch)) = expected_children
                        .iter()
                        .find(|(key, _)| !run.merged.iter().any(|m| &m.key == key))
                {
                    return Err(format!(
                        "child task unit {key}: its branch {branch} exists in none of the task's repositories \
                         (ADR-0079 D6)"
                    ));
                }
                Ok(run)
            })
            .await
            .map_err(|e| format!("integration task: {e}"))
            .and_then(|r| r);
            let result = match merged {
                Ok(mut run)
                    if run.conflict.is_none() && run.stale.is_none() && !checks.is_empty() =>
                {
                    let ws = match repos.first() {
                        Some(w) if w.is_dir() => {
                            task_worker::LocalWorkspace::new(&task_dir).with_work_dir(w)
                        }
                        _ => task_worker::LocalWorkspace::new(&task_dir),
                    }
                    .with_cargo_env(check_env);
                    let results = run_integration_checks(
                        store.as_ref(),
                        task_id,
                        &observed,
                        &ws,
                        &checks,
                        timeout,
                    )
                    .await;
                    run.checks = checks
                        .iter()
                        .zip(results)
                        .map(|(c, (pass, reason))| (c.cmd.clone(), pass, reason))
                        .collect();
                    Ok(run)
                }
                other => other,
            };
            let _ = tx.send(Completion::Integration {
                task_id,
                work_unit_id,
                result: Box::new(result),
            });
        });
        self.integrating.insert(
            task.id,
            IntegrationEntry {
                work_unit_id: wu_id_for_entry,
                handle,
            },
        );
        Ok(())
    }

    /// ADR-0074 D1.4（Phase F2b）: 工程の統合の結果。衝突 → merge の repair WU、検査の失敗 → repair
    /// （分類に当たる）か replan（当たらない）、成功 → `PhaseIntegrated` と次の工程。
    pub(super) fn on_integration_finished(
        &mut self,
        task_id: TaskId,
        work_unit_id: &str,
        result: Result<IntegrationRun, String>,
    ) -> Result<(), DispatchError> {
        if self
            .integrating
            .get(&task_id)
            .is_some_and(|e| e.work_unit_id == work_unit_id)
        {
            self.integrating.remove(&task_id);
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        let units = self.store.work_units_for(task_id)?;
        let Some(integ) = units.iter().find(|u| u.id == work_unit_id).cloned() else {
            return Ok(());
        };
        if task.status != Status::Running || integ.status != task_core::WorkUnitStatus::Running {
            tracing::warn!(%task_id, work_unit = %integ.key, status = ?task.status, "stale integration result discarded");
            return Ok(());
        }
        let run = match result {
            Ok(run) => run,
            Err(msg) => {
                return self.integration_gives_up(
                    &task,
                    &integ,
                    &format!(
                        "phase {} の統合を実行できませんでした: {msg}",
                        integ.phase.as_deref().unwrap_or_default()
                    ),
                );
            }
        };
        if let Some(conflict) = run.conflict.clone() {
            if let Some(request) = &conflict.request {
                // 同じ origin の古い組の依頼は store が `superseded` で閉じる（D4 付記）。
                self.record_integration_request(&task, &integ, request)?;
                return self.block_for_integration_request(&task, &integ);
            }
            return self.schedule_merge_repair(&task, &integ, &units, &conflict);
        }
        // D4 付記: merge が衝突なしで通った（人が手で統合した後の再実行で source が既に祖先、または merge
        // 成功）ので、この統合 WU が出した未回答の依頼は統合済み。検査の成否にかかわらず受信箱から消す。
        let closed = self.store.integration_requests_close(
            task_id,
            &format!("phase:{}", integ.key),
            task_core::integration_request::INTEGRATED_ANSWER,
            Some(&format!(
                "統合済み: {} の統合が衝突なしで通った（HEAD {}）",
                integ.key, run.head
            )),
        )?;
        if !closed.is_empty() {
            tracing::info!(%task_id, work_unit = %integ.key, requests = ?closed, "integration requests closed as integrated");
        }
        if let Some((repo_id, stale)) = run.stale.clone() {
            return self.requeue_stale_merge_candidate(&task, &integ, &units, repo_id, &stale);
        }
        if run.checks.iter().any(|(_, pass, _)| !pass) {
            return self.schedule_integration_check_repair(&task, &integ, &units, &run);
        }
        self.finish_phase_integration(&task, &integ, run)
    }

    /// ADR-0074 D1.4 3.（Phase F2b）: 衝突したら repair WU `merge-<phase>-<key>`（Task の worktree で
    /// 走る、最小の context）を作り、統合 WU をそれに依存させて pending に戻す。repair が done になったら
    /// 統合は続きから再開する（済んだ merge は飛ばす）。工程ごとに 2 回まで、超えたら replan。
    pub(super) fn schedule_merge_repair(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        units: &[task_core::WorkUnitRow],
        conflict: &crate::integration::Conflict,
    ) -> Result<(), DispatchError> {
        let phase = integ.phase.clone().unwrap_or_default();
        let prefix = format!("merge-{phase}-");
        let merge_repairs = units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair && u.key.starts_with(&prefix))
            .count();
        if merge_repairs >= MAX_MERGE_REPAIRS_PER_PHASE {
            return self.integration_gives_up(
                task,
                integ,
                &format!(
                    "phase {phase} の統合で WorkUnit {} の merge が衝突し、merge の repair の上限（{MAX_MERGE_REPAIRS_PER_PHASE} 回）に達しました",
                    conflict.key
                ),
            );
        }
        let mut key = format!("{prefix}{}", conflict.key);
        let mut n = 2;
        while units.iter().any(|u| u.key == key) {
            key = format!("{prefix}{}-{n}", conflict.key);
            n += 1;
        }
        let conflicted = units.iter().find(|u| u.key == conflict.key);
        let merged_already: Vec<&task_core::WorkUnitRow> = task_core::phase_leaves(units, &phase)
            .into_iter()
            .filter(|u| u.key != conflict.key)
            .collect();
        let decisions_of = |u: &task_core::WorkUnitRow| -> Vec<String> {
            u.last_run_id
                .as_deref()
                .and_then(|rid| self.store.run_index_get(rid).ok().flatten())
                .and_then(|r| r.checkpoint)
                .map(|cp| {
                    cp.decisions
                        .iter()
                        .map(|d| format!("{}（{}）", d.what, d.why))
                        .collect()
                })
                .unwrap_or_default()
        };
        let diff_stat = self
            .task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().find(|r| r.is_git()))
            .map(|r| crate::integration::diff_stat(&r.dir, "HEAD", &conflict.branch))
            .unwrap_or_default();
        let mut objective = format!(
            "工程 {phase} の統合で、WorkUnit {} のブランチ `{}` を Task のブランチへ merge したところ衝突しました。\n\
             Task の作業ツリーで `git merge --no-ff {}` を実行し、**衝突だけを解消して** commit してください。\
             設計は変えないこと。他の WorkUnit の成果を消さないこと。push はしないこと。\n\n衝突したファイル:\n",
            conflict.key, conflict.branch, conflict.branch
        );
        for file in &conflict.files {
            objective.push_str(&format!("- {file}\n"));
        }
        if let Some(c) = conflicted {
            objective.push_str(&format!("\n{} の目的: {}\n", c.key, c.spec.objective));
            for d in decisions_of(c) {
                objective.push_str(&format!("- 決定: {d}\n"));
            }
        }
        for u in &merged_already {
            objective.push_str(&format!(
                "\n既に入っている {} の目的: {}\n",
                u.key, u.spec.objective
            ));
            for d in decisions_of(u) {
                objective.push_str(&format!("- 決定: {d}\n"));
            }
        }
        if !diff_stat.is_empty() {
            objective.push_str(&format!(
                "\n`git diff --stat HEAD...{}`:\n{diff_stat}\n",
                conflict.branch
            ));
        }
        let (max_turns, max_wall_secs) = task_core::execution::RepairClass::MergeConflict.budget();
        let spec = task_core::WorkUnitSpec {
            key: key.clone(),
            kind: task_core::WorkUnitKind::Repair,
            title: format!(
                "repair (merge_conflict): {} の merge の衝突を解消",
                conflict.key
            ),
            objective,
            depends_on: vec![],
            done_when: vec![format!(
                "`git merge-base --is-ancestor {} HEAD` が成り立つ（衝突を解消して merge 済み）",
                conflict.branch
            )],
            checks: vec![task_core::WorkUnitCheck {
                cmd: format!("git merge-base --is-ancestor {} HEAD", conflict.branch),
                expect_exit: 0,
            }],
            context: task_core::WorkUnitContext {
                paths: conflict.files.clone(),
                ..Default::default()
            },
            harness: None,
            features: None,
            budget: Some(task_core::WorkUnitBudget {
                max_turns: Some(max_turns),
                max_wall_secs: Some(max_wall_secs),
            }),
            outputs: vec![],
            phase: Some(phase.clone()),
        };
        self.add_integration_repair(
            task,
            integ,
            spec,
            task_core::execution::RepairClass::MergeConflict.bucket(),
            "merge_conflict",
        )
    }

    /// ADR-0118 D5: 子のブランチ HEAD が記録済みの merge candidate と違った（review 後に子のブランチが
    /// 動いた）。merge せずに `MergeCandidateStale` を残し、子を再レビュー（`Trigger::Rereview`。review の入口が
    /// 再 sync → 全 checks → reviewer を行う）に戻し、子の unit を running・統合 WU を pending に戻す
    /// （子が再び done になると統合が続きから再開する）。stale は不合格ではないので review の試行回数に数えない。
    /// 同じ子で上限（[`MAX_STALE_CANDIDATE_RETRIES`]）を超えた、または再レビューできない子（実装型でない・
    /// done でない）は、統合を諦めて理由を残す（無言で merge しない）。
    pub(super) fn requeue_stale_merge_candidate(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        units: &[task_core::WorkUnitRow],
        repo_id: task_core::RepoId,
        stale: &crate::integration::StaleCandidate,
    ) -> Result<(), DispatchError> {
        let phase = integ.phase.clone().unwrap_or_default();
        let unit = units
            .iter()
            .find(|u| u.key == stale.key && u.kind == task_core::WorkUnitKind::Task);
        let child = match unit
            .and_then(|u| u.child_task_id.as_deref())
            .and_then(|id| id.parse::<TaskId>().ok())
        {
            Some(id) => self.store.get(id)?,
            None => None,
        };
        let (Some(unit), Some(child)) = (unit, child) else {
            return self.integration_gives_up(
                task,
                integ,
                &format!(
                    "phase {phase} の統合で {} のブランチ HEAD {} が merge candidate {} と異なりますが、子 task が見つかりません（ADR-0118 D5）",
                    stale.key, stale.head_sha, stale.merge_candidate_sha
                ),
            );
        };
        let stale_event = Event::MergeCandidateStale {
            phase: phase.clone(),
            work_unit_id: integ.id.clone(),
            key: stale.key.clone(),
            child_task: child.id,
            repo_id,
            branch: stale.branch.clone(),
            merge_candidate_sha: stale.merge_candidate_sha.clone(),
            head_sha: stale.head_sha.clone(),
            target_sha: stale.target_sha.clone(),
        };
        let previous = self
            .store
            .events_for(task.id)?
            .iter()
            .filter(|(_, e)| {
                matches!(e, Event::MergeCandidateStale { work_unit_id, key, .. }
                    if *work_unit_id == integ.id && *key == stale.key)
            })
            .count();
        let why = if previous >= MAX_STALE_CANDIDATE_RETRIES {
            Some(format!(
                "再 sync・再レビューの上限（{MAX_STALE_CANDIDATE_RETRIES} 回）に達しました"
            ))
        } else if child.status != Status::Done || child.kind != TaskKind::Execute {
            Some(format!(
                "子 task は再レビューできません（status={:?} kind={:?}）",
                child.status, child.kind
            ))
        } else {
            None
        };
        if let Some(why) = why {
            self.store.append_event(task.id, &stale_event)?;
            return self.integration_gives_up(
                task,
                integ,
                &format!(
                    "phase {phase} の統合で {} のブランチ HEAD {} が review 済みの merge candidate {} と異なるため merge しませんでした: {why}（ADR-0118 D5）",
                    stale.key, stale.head_sha, stale.merge_candidate_sha
                ),
            );
        }
        match self
            .store
            .apply_transition_with_events(child.id, Trigger::Rereview, Vec::new())
        {
            Ok(_) => {}
            Err(StoreError::InvalidTransition(e)) => {
                self.store.append_event(task.id, &stale_event)?;
                return self.integration_gives_up(
                    task,
                    integ,
                    &format!(
                        "phase {phase} の統合で {} の merge candidate が古くなりましたが、子 task を再レビューに戻せません: {e}（ADR-0118 D5）",
                        stale.key
                    ),
                );
            }
            Err(e) => return Err(e.into()),
        }
        tracing::info!(task_id = %task.id, work_unit = %stale.key, child_id = %child.id, head = %stale.head_sha, candidate = %stale.merge_candidate_sha, "child branch moved after its review; sending the child back to review before integration (ADR-0118 D5)");
        let now = rfc3339(OffsetDateTime::now_utc());
        let mut pending = integ.clone();
        pending.status = task_core::WorkUnitStatus::Pending;
        pending.clear_lease();
        pending.updated_at = now.clone();
        let mut running = unit.clone();
        running.status = task_core::WorkUnitStatus::Running;
        running.blocked_reason = None;
        running.updated_at = now;
        let events = vec![
            stale_event,
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: task_core::WorkUnitStatus::Running,
                to: task_core::WorkUnitStatus::Pending,
                reason: MERGE_CANDIDATE_STALE_REASON.to_string(),
                run_id: None,
            },
            Event::WorkUnitTransitioned {
                work_unit_id: unit.id.clone(),
                key: unit.key.clone(),
                from: unit.status,
                to: task_core::WorkUnitStatus::Running,
                reason: MERGE_CANDIDATE_STALE_REASON.to_string(),
                run_id: None,
            },
        ];
        self.store
            .work_units_apply(task.id, Vec::new(), vec![pending, running], events)?;
        Ok(())
    }

    /// ADR-0074 D1.4 4.（Phase F2b）: 統合後の検査の失敗。D16 の分類に当たれば repair WU を Task の
    /// worktree に作り（`max_repairs`・同じ class の上限の内なら）、当たらなければ replan。
    pub(super) fn schedule_integration_check_repair(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        units: &[task_core::WorkUnitRow],
        run: &IntegrationRun,
    ) -> Result<(), DispatchError> {
        let phase = integ.phase.clone().unwrap_or_default();
        let failing: Vec<task_core::FailedCheck> = run
            .checks
            .iter()
            .filter(|(_, pass, _)| !pass)
            .map(|(cmd, _, reason)| task_core::FailedCheck {
                check: Check::Command {
                    cmd: cmd.clone(),
                    expect_exit: 0,
                },
                reason: reason.clone(),
                repair_hint: None,
            })
            .collect();
        let summary: Vec<String> = failing.iter().map(|f| f.reason.clone()).collect();
        let why = format!(
            "phase {phase} の統合後の検査が失敗しました: {}",
            summary.join("; ")
        );
        let class = match task_core::classify_review_failure(&failing) {
            task_core::RepairDecision::Repairable(c) => c,
            task_core::RepairDecision::Substantive => {
                return self.integration_gives_up(task, integ, &why);
            }
        };
        let repairs: Vec<&task_core::WorkUnitRow> = units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
            .collect();
        let same_class = repairs
            .iter()
            .filter(|u| Self::repair_bucket_of_title(&u.spec.title) == Some(class.bucket()))
            .count();
        if repairs.len() as u32 >= self.config.execution.max_repairs
            || same_class as u32 >= self.config.execution.max_repairs_per_class
        {
            return self.integration_gives_up(task, integ, &why);
        }
        let prefix = format!("repair-{phase}-");
        let n = units.iter().filter(|u| u.key.starts_with(&prefix)).count() + 1;
        let diff_stat = self
            .task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().find(|r| r.is_git()))
            .and_then(|r| {
                let branch = r.branch().map(str::to_string).unwrap_or_default();
                crate::checkpoint::gather_repo_facts(Some(&r.dir), &branch)
                    .0
                    .map(|s| s.diff_stat)
            });
        let scope = repair_scope_from_units(units.iter().filter(|unit| {
            unit.phase.as_deref() == Some(phase.as_str())
                && !matches!(
                    unit.kind,
                    task_core::WorkUnitKind::Repair | task_core::WorkUnitKind::Integrate
                )
        }));
        let objective = task_core::build_repair_objective(
            class,
            &summary,
            &task.title,
            &task.objective,
            diff_stat.as_deref(),
            Some(&scope),
        );
        let (max_turns, max_wall_secs) = class.budget();
        let spec = task_core::WorkUnitSpec {
            key: format!("{prefix}{n}"),
            kind: task_core::WorkUnitKind::Repair,
            title: format!("repair ({}): 工程 {phase} の統合後の検査", class.bucket()),
            objective,
            depends_on: vec![],
            done_when: vec![],
            checks: vec![],
            context: Default::default(),
            harness: None,
            features: None,
            budget: Some(task_core::WorkUnitBudget {
                max_turns: Some(max_turns),
                max_wall_secs: Some(max_wall_secs),
            }),
            outputs: vec![],
            phase: Some(phase.clone()),
        };
        self.add_integration_repair(
            task,
            integ,
            spec,
            class.bucket(),
            "integration_check_failed",
        )
    }

    /// 統合の repair WU を足し（ready）、統合 WU をそれに依存させて pending に戻し、Task を
    /// `Continue{advance}`（Running → Ready）にする（repair の run は通常の dispatch に乗る）。
    pub(super) fn add_integration_repair(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        spec: task_core::WorkUnitSpec,
        class: &str,
        reason: &str,
    ) -> Result<(), DispatchError> {
        let now = rfc3339(OffsetDateTime::now_utc());
        let key = spec.key.clone();
        let row = task_core::WorkUnitRow::new(
            task_core::new_id(),
            task.id.to_string(),
            integ.plan_id.clone(),
            integ.seq,
            spec,
            task_core::WorkUnitStatus::Ready,
            now.clone(),
        );
        let mut pending = integ.clone();
        pending.status = task_core::WorkUnitStatus::Pending;
        pending.clear_lease();
        pending.updated_at = now;
        if !pending.depends_on.contains(&key) {
            pending.depends_on.push(key.clone());
            pending.spec.depends_on.push(key.clone());
        }
        let events = vec![
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: task_core::WorkUnitStatus::Running,
                to: task_core::WorkUnitStatus::Pending,
                reason: reason.to_string(),
                run_id: None,
            },
            Event::WorkUnitTransitioned {
                work_unit_id: row.id.clone(),
                key: row.key.clone(),
                from: task_core::WorkUnitStatus::Pending,
                to: task_core::WorkUnitStatus::Ready,
                reason: "integration_repair".to_string(),
                run_id: None,
            },
            Event::RepairScheduled {
                work_unit_id: row.id.clone(),
                key: row.key.clone(),
                class: class.to_string(),
                origin: task_core::execution::RepairOrigin::Integration,
            },
        ];
        self.store
            .work_units_apply(task.id, vec![row], vec![pending], events)?;
        match self.store.apply_transition_with_events(
            task.id,
            Trigger::Continue {
                why: task_core::ContinueWhy::Advance,
            },
            vec![],
        ) {
            Ok(_) | Err(StoreError::InvalidTransition(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// 統合を諦める（merge の repair の上限・分類に当たらない検査の失敗・git の失敗）。統合 WU を
    /// failed にし、replan の余地があれば `Continue{replan}`（replan は統合 WU を pending に戻す）、
    /// 無ければ人に聞く（blocked。統合 WU は blocked(question) にして、回答で再開できるようにする）。
    /// The request event is the sole inbox item. Keep the integration WU blocked until its answer.
    fn block_for_integration_request(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
    ) -> Result<(), DispatchError> {
        let mut row = integ.clone();
        row.clear_lease();
        row.updated_at = rfc3339(OffsetDateTime::now_utc());
        row.status = task_core::WorkUnitStatus::Blocked;
        row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
        self.store.work_unit_transition(
            task.id,
            row,
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: integ.status,
                to: task_core::WorkUnitStatus::Blocked,
                reason: "integration_request".to_string(),
                run_id: None,
            },
        )?;
        self.store
            .apply_transition_with_events(task.id, Trigger::WorkerQuestion, vec![])?;
        Ok(())
    }

    pub(super) fn integration_gives_up(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        why: &str,
    ) -> Result<(), DispatchError> {
        let replans_so_far = self.counted_replans(task.id)?;
        let can_replan = replans_so_far < self.effective_max_replans(task.id)?;
        let mut row = integ.clone();
        row.clear_lease();
        row.updated_at = rfc3339(OffsetDateTime::now_utc());
        if can_replan {
            row.status = task_core::WorkUnitStatus::Failed;
            row.blocked_reason = None;
        } else {
            row.status = task_core::WorkUnitStatus::Blocked;
            row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
        }
        self.store.work_unit_transition(
            task.id,
            row.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: integ.status,
                to: row.status,
                reason: "integration_failed".to_string(),
                run_id: None,
            },
        )?;
        let run_id = last_run_id(&self.store.events_for(task.id)?).unwrap_or_default();
        let progress = Event::worker_progress(run_id.clone(), why.to_string());
        let mut events = vec![progress];
        let trigger = if can_replan {
            Trigger::Continue {
                why: task_core::ContinueWhy::Replan,
            }
        } else {
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                task,
                why,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the integration failure");
            }
            // ADR-0079 付記「R6-1」D7: 質問の本文（受信箱の `question`）は `WorkerFinished` / `QuestionRaised` から
            // 読まれる。統合の失敗は run の終わりではないので `QuestionRaised` に失敗の要約（`worker_progress` と
            // 同じ文）を残す。以前は残さず、受信箱の質問が空文だった（web Phase 1 の子、2026-09-30 04:57Z）。
            events.push(Event::QuestionRaised {
                run_id,
                text: why.to_string(),
            });
            Trigger::WorkerQuestion
        };
        match self
            .store
            .apply_transition_with_events(task.id, trigger, events)
        {
            Ok(_) | Err(StoreError::InvalidTransition(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0074 D1.4 5.（Phase F2b）: 統合の成功。`PhaseIntegrated` を残し、統合 WU を done にし、
    /// その工程の WU の worktree を消し（ブランチは残す）、次の工程の WU を ready にする。次の工程が
    /// あれば `Continue{advance}`、最後の工程なら `WorkerDone`（→ 最終レビュー）。
    pub(super) fn finish_phase_integration(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        run: IntegrationRun,
    ) -> Result<(), DispatchError> {
        let task_id = task.id;
        let phase = integ.phase.clone().unwrap_or_default();
        let mut done = integ.clone();
        done.status = task_core::WorkUnitStatus::Done;
        done.clear_lease();
        done.updated_at = rfc3339(OffsetDateTime::now_utc());
        if !run.head.is_empty() {
            done.integrated_commit = Some(run.head.clone());
        }
        self.store.work_unit_transition(
            task_id,
            done,
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: task_core::WorkUnitStatus::Running,
                to: task_core::WorkUnitStatus::Done,
                reason: "integrated".to_string(),
                run_id: None,
            },
        )?;
        let units = self.store.work_units_for(task_id)?;
        // D1.2: 統合が済んだ WU の worktree を消す（ブランチは Task の終端まで残す）。
        if let Some(ws) = self.task_workspaces_for(task) {
            for u in units
                .iter()
                .filter(|u| u.phase.as_deref() == Some(phase.as_str()) && u.branch.is_some())
            {
                for repo in ws.repos.iter().filter(|r| r.is_git()) {
                    let lwt = crate::integration::wu_worktree(
                        &ws.task_dir,
                        &task_id.to_string(),
                        &u.key,
                        &repo.name,
                        &repo.source,
                        u.base_commit.as_deref().unwrap_or_default(),
                    );
                    if let Err(e) = crate::integration::remove_wu_worktree(&lwt) {
                        tracing::warn!(%task_id, work_unit = %u.key, error = %e, "could not remove the work unit worktree after integration");
                    }
                }
            }
        }
        // ADR-0079 D6（Phase R1c）: 取り込んだ子 task の worktree を消す（ブランチ `celeris/<child_id>` は
        // root の終端まで残す。監査のため）。
        self.remove_integrated_child_worktrees(task_id, &units, &phase);
        // 次の工程の WU（工程の障壁が外れた）を ready にするのは、途中確認で止めるかを決めた後（下）。
        // ADR-0079 付記「R6-1」D2: 途中確認（`review: human`・`pause_after`）で止めるなら次の工程は `pending` の
        // まま残す（ADR-0074 D2.2「次の工程の WU を ready にする前に適用する」。以前は先に `ready` にしていたので、
        // task が `blocked(awaiting_human)` でも木の照合が次の段階の kind task の unit から子を作った: P-R5b-5）。
        let phase_event = Event::PhaseIntegrated {
            phase: phase.clone(),
            work_unit_id: integ.id.clone(),
            merged: run
                .merged
                .iter()
                .map(|m| task_core::PhaseMerged {
                    key: m.key.clone(),
                    commit: m.commit.clone(),
                    skipped: m.skipped,
                    target_sha: m.target_sha.clone(),
                    parent_head: m.parent_head.clone(),
                })
                .collect(),
            head: run.head.clone(),
            checks: run
                .checks
                .iter()
                .map(|(cmd, pass, summary)| task_core::PhaseCheckResult {
                    cmd: cmd.clone(),
                    pass: *pass,
                    summary: summary.clone(),
                })
                .collect(),
        };
        let all_done = matches!(
            crate::execution_scheduler::settle_phase(&units),
            crate::execution_scheduler::PhaseSettle::AllDone
        );
        // ADR-0074 D2.2/D2.3（Phase F3 途中確認）: 次の工程があり、かつこの工程が停止点として
        // 解決されているなら、`Continue{advance}` の代わりに `PhaseGate` で Blocked にする
        // （F2b からの申し送り: `finish_phase_integration` が `Continue{advance}`/`WorkerDone` を
        // 選ぶ場所に判定を差し込む）。最後の工程（`all_done`）では常に `WorkerDone`（D1.6 の表）。
        let mut extra_events = vec![phase_event];
        let mut trigger = if all_done {
            Trigger::WorkerDone
        } else {
            Trigger::Continue {
                why: task_core::ContinueWhy::Advance,
            }
        };
        if !all_done
            && let Ok(Some(active)) = self.store.execution_plan_active(task_id)
            && let Ok(events) = self.store.events_for(task_id)
        {
            let resolved = events
                .iter()
                .rev()
                .find_map(|(_, e)| match e {
                    Event::PausePointsResolved {
                        plan_id, phases, ..
                    } if *plan_id == active.id => Some(phases.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if resolved.iter().any(|p| p == &phase) {
                let report = self.build_phase_report(task, &phase, &units, &active, &run, &events);
                if let Some(artifact_event) = self.write_phase_report_artifact(
                    task,
                    &phase,
                    resolved
                        .iter()
                        .position(|p| p == &phase)
                        .map(|i| i + 1)
                        .unwrap_or(1),
                    &report,
                ) {
                    extra_events.push(artifact_event);
                }
                extra_events.push(Event::PhaseReported {
                    phase: phase.clone(),
                    report: Box::new(report),
                });
                trigger = Trigger::PhaseGate {
                    phase: phase.clone(),
                };
            }
        }
        if !matches!(trigger, Trigger::PhaseGate { .. }) {
            self.promote_newly_ready(task_id, &units)?;
        }
        let units = self.store.work_units_for(task_id)?;
        match self
            .store
            .apply_transition_with_events(task_id, trigger, extra_events)
        {
            Ok(outcome) => {
                tracing::info!(%task_id, %phase, next = ?outcome.next, "phase integrated");
                if outcome.next == Status::Reviewing {
                    let subject = ReviewSubject {
                        summary: self.plan_summary(&units),
                        evidence: Vec::new(),
                    };
                    let run_id = units
                        .iter()
                        .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
                        .filter_map(|u| u.last_run_id.clone())
                        .max()
                        .unwrap_or_default();
                    if !self.spawn_review(task_id, run_id, &subject)? {
                        self.pending_subjects.insert(task_id, subject);
                    }
                }
                Ok(())
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "phase integration result could not be applied");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0074 D2.3（Phase F3 途中確認）: 停止点の工程の統合の後に、決定的に途中報告を組み立てる
    /// （LLM は使わない。checkpoint・`PhaseIntegrated` の材料・quota の記録済み値を機械的に束ねる）。
    pub(super) fn build_phase_report(
        &self,
        task: &Task,
        phase: &str,
        units: &[task_core::WorkUnitRow],
        active: &task_core::ExecutionPlanRow,
        run: &IntegrationRun,
        events: &[(u64, Event)],
    ) -> task_core::PhaseReport {
        // ADR-0079（Phase R1b）: /3 の段階は `internal_view` で /2 の工程に写して読む。
        let view = task_core::internal_view(&active.spec);
        let phase_title_of = |key: &str| -> String {
            view.phases
                .iter()
                .find(|p| p.key == key)
                .map(|p| p.title.clone())
                .unwrap_or_else(|| key.to_string())
        };
        let phase_order: Vec<&str> = view.phases.iter().map(|p| p.key.as_str()).collect();
        let current_idx = phase_order.iter().position(|k| *k == phase).unwrap_or(0);

        // 済んだ工程の一覧（現在の工程より前の工程だけ。工程の障壁により、それらは既に統合済み）。
        let mut phases_done = Vec::new();
        for &key in &phase_order[..current_idx] {
            let phase_units: Vec<&task_core::WorkUnitRow> = units
                .iter()
                .filter(|u| {
                    u.phase.as_deref() == Some(key) && u.kind != task_core::WorkUnitKind::Integrate
                })
                .collect();
            let n_wu = phase_units.len();
            let n_run: u32 = phase_units.iter().map(|u| u.runs).sum();
            let start = phase_units
                .iter()
                .filter_map(|u| parse_rfc3339(&u.created_at))
                .min();
            let end = phase_units
                .iter()
                .filter_map(|u| parse_rfc3339(&u.updated_at))
                .max();
            let wall = match (start, end) {
                (Some(s), Some(e)) => {
                    task_core::format_wall_ms((e - s).whole_milliseconds() as i64)
                }
                _ => "0m".to_string(),
            };
            phases_done.push(format!(
                "{}: {n_wu} WU / {n_run} run / {wall}",
                phase_title_of(key)
            ));
        }

        // この工程の WU ごとの要約（最終 checkpoint の completed 上位 5 件・decisions・known_failures）。
        let mut work_units = Vec::new();
        for u in units.iter().filter(|u| {
            u.phase.as_deref() == Some(phase) && u.kind != task_core::WorkUnitKind::Integrate
        }) {
            let mut parts = vec![u.spec.title.clone()];
            if let Some(cp) = task_ops::derive::latest_checkpoint(events, Some(&u.key)) {
                if !cp.completed.is_empty() {
                    let top: Vec<String> = cp.completed.iter().take(5).cloned().collect();
                    parts.push(format!("completed: {}", top.join("; ")));
                }
                if !cp.decisions.is_empty() {
                    let d: Vec<String> = cp
                        .decisions
                        .iter()
                        .map(|d| format!("{} ({})", d.what, d.why))
                        .collect();
                    parts.push(format!("decisions: {}", d.join("; ")));
                }
                if !cp.known_failures.is_empty() {
                    let f: Vec<String> = cp
                        .known_failures
                        .iter()
                        .map(|f| match &f.detail {
                            Some(detail) => format!("{}: {detail}", f.what),
                            None => f.what.clone(),
                        })
                        .collect();
                    parts.push(format!("known_failures: {}", f.join("; ")));
                }
            }
            work_units.push(format!("{}: {}", u.key, parts.join(" / ")));
        }

        // 統合の結果（merge・検査。`run` は今まさに終えた統合そのもの）。
        let mut integration: Vec<String> = run
            .merged
            .iter()
            .map(|m| {
                if m.skipped {
                    format!("{}: already integrated (skipped)", m.key)
                } else {
                    format!("merged {} @ {}", m.key, m.commit)
                }
            })
            .collect();
        for (cmd, pass, summary) in &run.checks {
            integration.push(format!(
                "{cmd}: {} ({summary})",
                if *pass { "pass" } else { "fail" }
            ));
        }

        let diff_stat = self.phase_diff_stat(task, units, &run.head);

        let next_key = phase_order.get(current_idx + 1).copied();
        let next_phase_work_units: Vec<String> = next_key
            .map(|k| {
                active
                    .spec
                    .work_units
                    .iter()
                    .filter(|w| w.phase.as_deref() == Some(k))
                    .map(|w| w.title.clone())
                    .collect()
            })
            .unwrap_or_default();

        let plain_events: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
        let metrics = task_core::summarize_execution_metrics(task, &plain_events);
        let quota_summary = task_core::quota_summary_line(&metrics);

        // 成果物へのリンク（この工程の WU の run が出した `ArtifactProduced`）。
        let phase_wu_ids: std::collections::HashSet<&str> = units
            .iter()
            .filter(|u| {
                u.phase.as_deref() == Some(phase) && u.kind != task_core::WorkUnitKind::Integrate
            })
            .map(|u| u.id.as_str())
            .collect();
        let phase_run_ids: std::collections::HashSet<&str> = events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::WorkUnitTransitioned {
                    work_unit_id,
                    to: task_core::WorkUnitStatus::Running,
                    run_id: Some(rid),
                    ..
                } if phase_wu_ids.contains(work_unit_id.as_str()) => Some(rid.as_str()),
                _ => None,
            })
            .collect();
        let mut artifact_paths: Vec<String> = events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::ArtifactProduced { run_id, artifact }
                    if phase_run_ids.contains(run_id.as_str()) =>
                {
                    Some(artifact.path.clone())
                }
                _ => None,
            })
            .collect();
        artifact_paths.sort();
        artifact_paths.dedup();

        // ADR-0079 D5 / D11（Phase R4a）: この段階の子 task ごとの要約の行（状態・subtree の run と定価・子の報告の見出し）。
        let child_units = task_ops::tree_view::stage_child_summaries(self.store.as_ref(), units, phase)
            .unwrap_or_else(|e| {
                tracing::warn!(task_id = %task.id, %phase, error = %e, "could not summarise the stage's child tasks");
                Vec::new()
            });
        let mut report = task_core::PhaseReport {
            phase: phase.to_string(),
            phase_title: phase_title_of(phase),
            phases_done,
            work_units,
            child_units,
            integration,
            diff_stat,
            next_phase: next_key.map(|k| k.to_string()),
            next_phase_work_units,
            quota_summary,
            artifact_paths,
        };
        task_core::truncate_phase_report(&mut report);
        report
    }

    /// D2.3: `git diff --stat <base>..<head>` の要約（最大 30 行）。base はこの Task の v2 計画で
    /// 最初に走った WU の `base_commit`（工程をまたいだ全体の差分）。git が使えない・base が無い・
    /// コマンドが失敗した場合は空（決定的な組み立ての一部として、失敗を報告に混ぜない）。
    pub(super) fn phase_diff_stat(
        &self,
        task: &Task,
        units: &[task_core::WorkUnitRow],
        head: &str,
    ) -> Vec<String> {
        if head.is_empty() {
            return Vec::new();
        }
        let Some(ws) = self.task_workspaces_for(task) else {
            return Vec::new();
        };
        let Some(repo) = ws.repos.iter().find(|r| r.is_git()) else {
            return Vec::new();
        };
        let Some(base) = units
            .iter()
            .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
            .min_by_key(|u| u.seq)
            .and_then(|u| u.base_commit.clone())
        else {
            return Vec::new();
        };
        let output = std::process::Command::new("git")
            .current_dir(&repo.dir)
            .args(["diff", "--stat", &format!("{base}..{head}")])
            .output();
        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(30)
                .map(|s| s.to_string())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// D2.3: 途中報告を `artifacts/phase-reports/<n>-<phase>.md`（人が読める形。ADR-0067）として書き、
    /// `Event::ArtifactProduced` を返す（書けなければ `None`。工程を止めること自体は諦めない）。
    /// `run_id` は特定のワーカー run に属さない daemon 発の成果物なので、`PhaseIntegrated`/
    /// `WorkUnitCommitted` と同じ「daemon が決定的に作る」ことが分かる合成の値にする。
    pub(super) fn write_phase_report_artifact(
        &self,
        task: &Task,
        phase: &str,
        n: usize,
        report: &task_core::PhaseReport,
    ) -> Option<Event> {
        let workspace_dir = self.task_dir(task)?;
        let artifacts_dir = self.artifacts_dir(task, &workspace_dir);
        let rel_prefix = task_core::artifacts::artifacts_rel_for(task, &workspace_dir);
        let filename = format!("{n}-{phase}.md");
        let abs_path = artifacts_dir.join("phase-reports").join(&filename);
        if let Some(parent) = abs_path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        std::fs::write(&abs_path, render_phase_report_markdown(report)).ok()?;
        let sha256 = task_worker::artifact::sha256_file(&abs_path).ok()?;
        Some(Event::ArtifactProduced {
            run_id: format!("daemon:phase-gate:{phase}"),
            artifact: ArtifactRef {
                name: filename.clone(),
                path: format!("{rel_prefix}/phase-reports/{filename}"),
                sha256,
                kind: "md".to_string(),
                declared: true,
            },
        })
    }
}

/// ADR-0074 D2.3（Phase F3 途中確認）: `PhaseReport` を人が読める Markdown にする（決定的。LLM 不使用）。
/// `Event::PhaseReported.report` と同じ内容を `artifacts/phase-reports/<n>-<phase>.md` にも残す。
fn render_phase_report_markdown(report: &task_core::PhaseReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# 途中報告: {} ({})\n\n",
        report.phase_title, report.phase
    ));
    if !report.phases_done.is_empty() {
        out.push_str("## 済んだ工程\n\n");
        for line in &report.phases_done {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !report.work_units.is_empty() {
        out.push_str("## この工程の WU\n\n");
        for line in &report.work_units {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    // ADR-0079 D11（Phase R4a）: 子 task の要約。
    if !report.child_units.is_empty() {
        out.push_str("## この段階の子 task\n\n");
        for line in &report.child_units {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !report.integration.is_empty() {
        out.push_str("## 統合\n\n");
        for line in &report.integration {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !report.diff_stat.is_empty() {
        out.push_str("## 差分\n\n```\n");
        for line in &report.diff_stat {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("```\n\n");
    }
    out.push_str("## 次の工程\n\n");
    match &report.next_phase {
        Some(p) => {
            out.push_str(&format!("- {p}\n"));
            for wu in &report.next_phase_work_units {
                out.push_str(&format!("  - {wu}\n"));
            }
        }
        None => out.push_str("- (この工程が最後。最終レビューへ)\n"),
    }
    out.push('\n');
    out.push_str(&format!("## quota\n\n{}\n\n", report.quota_summary));
    if !report.artifact_paths.is_empty() {
        out.push_str("## 成果物\n\n");
        for p in &report.artifact_paths {
            out.push_str(&format!("- {p}\n"));
        }
    }
    out
}
