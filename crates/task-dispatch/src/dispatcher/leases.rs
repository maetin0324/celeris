//! lease の回収・abort・orphan の引き取り・drain の打ち切り。`run_holds_lease` を確かめてから結果を適用し、`stop_run` は kill_tree → abort の順。ADR-0082 の L3。

use super::*;

impl Dispatcher {
    /// ADR-0040 D4: `[handoff] drain_timeout_secs` を超えたときに、残っている run とレビューを
    /// 打ち切る。DB の状態は変えない（リースが切れて新しい active が従来の「リース切れ」の経路で拾う）。
    /// 打ち切った数を返す。
    pub fn abort_all_runs(&mut self) -> usize {
        // ADR-0044 §5 Phase 53 追記（Phase 55）: drain も他の 4 つと同じ止め方
        // （プロセスグループへ SIGTERM → `kill_grace_secs` → SIGKILL）。
        let kill_grace = self.config.kill_grace;
        let mut aborted = 0;
        for (key, entry) in self.running.drain() {
            let task_id = key.task;
            tracing::warn!(task_id = %task_id, run_id = %entry.run_id, "drain timeout; aborting the run (the lease will expire and the new active will reclaim it)");
            // Phase 55/56 の合流: コンテナで走っている run はラベル越しにも止める（P55-4 / P56-7）。
            task_worker::kill_tree_with(&entry.run_id, kill_grace, entry.container);
            entry.handle.abort();
            aborted += 1;
        }
        let reviewing: Vec<(TaskId, ReviewEntry)> = self.reviewing.drain().collect();
        for (task_id, entry) in reviewing {
            tracing::warn!(task_id = %task_id, "drain timeout; aborting the review");
            task_worker::kill_tree(&entry.run_id, kill_grace);
            if let Some(review_run_id) = &entry.review_run_id {
                task_worker::kill_tree(review_run_id, kill_grace);
                // Phase F5-fix3: Reviewer run は lease を持たない（新しい active は review をやり直すだけで
                // この run を閉じない）ので、ここで `runs` 行ごと閉じる。Task の状態は変えない。
                self.close_aborted_run(
                    task_id,
                    review_run_id,
                    Some(RunRole::Reviewer),
                    "review aborted (drain timeout)",
                );
            }
            entry.handle.abort();
            aborted += 1;
        }
        // Phase F5-fix2: 検査中の WU と工程の統合も止める（DB は変えない。lease 切れの経路で
        // 新しい active が拾う。検査前の run の result.json があれば、そこから確定させる）。
        for (run_id, entry) in self.checking.drain() {
            tracing::warn!(task_id = %entry.task_id, %run_id, "drain timeout; aborting the work unit checks");
            entry.handle.abort();
            aborted += 1;
        }
        for (task_id, entry) in self.integrating.drain() {
            tracing::warn!(%task_id, work_unit = %entry.work_unit_id, "drain timeout; aborting the phase integration");
            entry.handle.abort();
            aborted += 1;
        }
        self.pending_subjects.clear();
        aborted
    }

    /// Phase F5-fix6: SIGTERM / SIGINT（`systemctl restart`、`promote.sh` の停止→起動）で止まる直前に、
    /// 手元の worker run の終わりを DB に記録する。本番 2026-09-28 17:05:50Z: 止まるデーモンは SIGTERM を
    /// 受けた tick でループを抜け、0.6 秒後にアダプタが `result.json`（exit=143）を書いたが、完了を
    /// 受け取る者が居ないまま exit し、`worker_finished` も Task の遷移も残らなかった（新しいデーモンは
    /// lease の失効まで 15 分待った）。
    ///
    /// 1. 既に届いた完了を記録する（`drain_completions`）。
    /// 2. 残りの run はプロセスグループごと止め（`stop_run`）、`WorkerFinished{outcome: "interrupted:
    ///    daemon shutdown …", end: cancelled}` を残す。Task の lease を持つ run は `InfraRequeue`
    ///    （attempts を消費しない。上限超過は `infra failure ×N`）、WU の run は WU を ready /
    ///    needs_continuation に戻す（reason `shutdown`）。
    /// 3. ただし run が既に error 以外の終端の `result.json`（done 等）を書き終えていたら DB は触らない
    ///    （次のデーモンが孤児の回収でその内容から確定させる。ここで検査・レビューを spawn しても exit で
    ///    失われるため）。
    ///
    /// 4. 手元のレビューも止める。Reviewer run を起こしていれば quota を閉じ、`WorkerFinished{role:
    ///    reviewer, outcome: "interrupted: review interrupted (daemon shutdown …)", end: cancelled}` で
    ///    `runs` 行を閉じる（Reviewer run は lease を持たないので、閉じないと次のデーモンの誰も閉じない）。
    ///    Task は `reviewing` のまま（次のデーモンの `recover_reviews` がレビューをやり直す。Task ごとの
    ///    レビューの flock は止めた task とこのプロセスの終わりで外れる）。
    ///
    /// WU の検査・工程の統合は触らない（次のデーモンの孤児の回収が拾う）。DB に記録した worker run の数と
    /// 止めたレビューの数の和を返す。
    pub fn interrupt_runs_on_shutdown(&mut self) -> usize {
        if let Err(e) = self.drain_completions() {
            tracing::warn!(error = %e, "failed to record the completions received before the shutdown");
        }
        let entries: Vec<(RunKey, RunEntry)> = self.running.drain().collect();
        let mut recorded = 0;
        for (key, entry) in entries {
            let run_id = entry.run_id.clone();
            let since = entry.since;
            self.stop_run(&run_id, entry.handle, entry.container);
            let task = match self.store.get(key.task) {
                Ok(Some(t)) => t,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(task_id = %key.task, %run_id, error = %e, "could not read the task while recording the shutdown");
                    continue;
                }
            };
            if let Some(dir) = self.task_dir(&task)
                && let Some(terminal) = terminal_from_run_dir(&dir, &run_id)
                && !matches!(terminal, Terminal::Error { .. })
            {
                tracing::info!(task_id = %task.id, %run_id, "the run already wrote a terminal result.json; leaving it to the next daemon's orphan takeover (Phase F5-fix6)");
                continue;
            }
            match self.record_shutdown_interrupt(&task, &run_id, since) {
                Ok(()) => {
                    tracing::warn!(task_id = %task.id, %run_id, "daemon shutdown: the run was stopped and recorded as interrupted (Phase F5-fix6)");
                    recorded += 1;
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the shutdown interrupt; the next daemon's orphan takeover will pick it up");
                }
            }
        }
        let reviewing: Vec<(TaskId, ReviewEntry)> = self.reviewing.drain().collect();
        for (task_id, entry) in reviewing {
            if let Some(review_run_id) = entry.review_run_id.clone() {
                if let Some(provider) = entry.provider.clone() {
                    self.release_quota_if_tracked(
                        &review_run_id,
                        entry.account.as_deref(),
                        entry.account_adapter,
                        &provider,
                        task_id,
                    );
                }
                self.close_aborted_run(
                    task_id,
                    &review_run_id,
                    Some(RunRole::Reviewer),
                    &format!("review interrupted ({SHUTDOWN_WHY})"),
                );
            }
            tracing::warn!(%task_id, review_run_id = ?entry.review_run_id, "daemon shutdown: the review was stopped; the next daemon re-reviews the task");
            self.stop_review(entry);
            recorded += 1;
        }
        recorded
    }

    pub(super) fn record_shutdown_interrupt(
        &mut self,
        task: &Task,
        run_id: &str,
        since: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        let events = self.store.events_for(task.id)?;
        if events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id))
        {
            return Ok(());
        }
        let finished = |outcome: String| Event::WorkerFinished {
            run_id: run_id.to_string(),
            outcome,
            usage: None,
            role: None,
            metrics: Some(task_core::RunMetrics {
                wall_ms: wall_ms_since(since),
                retries: task.attempts,
                peak_context_tokens: None,
                turns: None,
            }),
            end: Some(task_core::RunEnd::Cancelled),
        };
        let holds_task_lease = task.status == Status::Running
            && task
                .lease
                .as_ref()
                .is_some_and(|l| l.worker_run_id == run_id);
        if holds_task_lease {
            let infra_n = consecutive_infra_requeues(&events) + 1;
            let (trigger, outcome) = if infra_n <= self.config.max_infra_retries {
                (
                    Trigger::InfraRequeue,
                    format!("interrupted: {SHUTDOWN_WHY} (run_id={run_id})"),
                )
            } else {
                (
                    Trigger::WorkerError { retryable: false },
                    format!("{INFRA_FAILURE_MARKER}{infra_n}: {SHUTDOWN_WHY} (run_id={run_id})"),
                )
            };
            match self
                .store
                .apply_transition_with_events(task.id, trigger, vec![finished(outcome)])
            {
                Ok(_) | Err(StoreError::InvalidTransition(_)) => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            self.store.append_event(
                task.id,
                &finished(format!("interrupted: {SHUTDOWN_WHY} (run_id={run_id})")),
            )?;
        }
        self.reconcile_work_unit_run(task.id, run_id, "shutdown")
    }

    /// ADR-0070 D5（Phase 116）: lease が期限切れでも、**このインスタンスが持っている run**
    /// （`self.running` に entry がある）のプロセスがまだ生きていれば（`process_group::group_alive`）、
    /// reclaim せず lease を延長して続行する（DB busy で数 tick 更新できなかっただけ、という実際に
    /// 起きた事故〈PROGRESS Phase 116〉をここで救う）。延長にも失敗したら次の tick に持ち越す。
    /// 死んでいる（またはこのインスタンスの管理外）ときだけ、ADR-0070 D3 の分岐
    /// （`InfraRequeue` でバックオフ再試行、`max_infra_retries` 到達で打ち切り）に乗せる。
    /// `Trigger::LeaseExpired`（無条件に attempts を消費する）はもう使わない。
    ///
    /// Phase F5-fix6: lease がまだ切れていなくても、持ち主のデーモンが居ない run（孤児。定義は
    /// `crate::orphan`）は同じ経路で**すぐに**回収する（result.json があればその内容で確定、無ければ
    /// `interrupted: orphan_takeover` で requeue）。v2 の工程の lease は `reconcile_parallel_tasks` が
    /// WU ごとに扱う。
    pub(super) fn reclaim_expired_leases(&mut self) -> Result<usize, DispatchError> {
        let now = OffsetDateTime::now_utc();
        let mut count = 0;
        let mut holders_gone: Option<bool> = None;
        for task in self.store.list(Some(Status::Running))? {
            // ADR-0041 D5: 面倒を見ないタスクのリースは奪わない（verify は本番のコピーの行を書き換えない）。
            if !self.is_eligible(&task) {
                continue;
            }
            let Some(lease) = &task.lease else { continue };
            let orphaned = if lease.expires_at > now {
                if is_phase_lease_holder(&lease.worker_run_id)
                    || self.holds_task_in_hand(task.id)
                    || !self.lease_holders_gone(&mut holders_gone, now)
                {
                    continue;
                }
                true
            } else {
                false
            };
            if orphaned {
                self.note_orphan_takeover(&task, &lease.worker_run_id, lease.expires_at);
            }
            // ADR-0074 D1.5（Phase F2）: v2 の並列 WU では同じ Task の run が複数ありうる。
            // どれか 1 本でも生きていれば、その run の lease（WU の lease）を延ばす
            // （`renew_lease` が Task の lease も延ばす）。
            let alive_entry = self
                .running
                .iter()
                .filter(|(k, _)| k.task == task.id)
                .map(|(_, e)| e)
                .find(|e| task_worker::process_group::group_alive(&e.run_id));
            if let Some(entry) = alive_entry {
                let ttl = Duration::from_secs(task.budget.max_wall_secs) + self.config.lease_grace;
                match self.store.renew_lease(task.id, &entry.run_id, ttl) {
                    Ok(true) => {
                        tracing::warn!(task_id = %task.id, run_id = %entry.run_id, "lease expired but the run's process is still alive; extended instead of reclaiming (ADR-0070 D5)");
                        continue;
                    }
                    Ok(false) => {
                        // 一致しない（レース。他の何かがリースを動かした）。下の通常の reclaim へ。
                    }
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, run_id = %entry.run_id, error = %e, "failed to extend the lease for a still-alive run; will retry reclaiming next tick");
                        continue;
                    }
                }
            }
            // Phase F5-fix2: run は終わったが WU の `checks` がまだ走っている（`checking`）。検査が
            // 延ばした lease（`review_timeout × (2n+1)`）より長引いても、検査の完了を受け取るまで回収しない。
            let checking_run = self
                .checking
                .iter()
                .find(|(_, e)| e.task_id == task.id)
                .map(|(run_id, _)| run_id.clone());
            if let Some(run_id) = checking_run {
                let ttl = self.config.review_timeout + self.config.lease_grace;
                match self.store.renew_lease(task.id, &run_id, ttl) {
                    Ok(true) => {
                        tracing::warn!(task_id = %task.id, %run_id, "lease expired while the work unit checks are still running; extended instead of reclaiming (Phase F5-fix2)");
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to extend the lease for running work unit checks; will retry reclaiming next tick");
                        continue;
                    }
                }
            }
            // ADR-0061（Phase 104）: `entry` を消費する前に `since`（wall time 計算用）を取っておく。
            let mut run_since: Option<OffsetDateTime> = None;
            let keys: Vec<RunKey> = self
                .running
                .keys()
                .filter(|k| k.task == task.id)
                .cloned()
                .collect();
            for key in keys {
                if let Some(entry) = self.running.remove(&key) {
                    run_since = run_since.or(Some(entry.since));
                    // ADR-0044 Phase 53 追記: リース喪失も同じ止め方（プロセスグループごと。
                    // コンテナで走っていればラベル越しにも同じ 2 段を送る）。
                    self.stop_run(&entry.run_id, entry.handle, entry.container);
                }
            }
            // ADR-0074 D1.5/D1.7（Phase F2）: 工程の lease（v2）なら、lease を持っていた run は
            // `running` の WU の run（`lease_run_id`）。それぞれに `WorkerFinished` を残し、WU を
            // 照合で戻す（統合の途中なら `integrate-<phase>` も pending に戻す）。
            let phase_lease = is_phase_lease_holder(&lease.worker_run_id);
            let wu_runs: Vec<String> = if phase_lease {
                self.store
                    .work_units_for(task.id)?
                    .into_iter()
                    .filter(|u| {
                        u.status == task_core::WorkUnitStatus::Running
                            && u.kind != task_core::WorkUnitKind::Integrate
                    })
                    .filter_map(|u| u.lease_run_id.or(u.last_run_id))
                    .collect()
            } else {
                vec![lease.worker_run_id.clone()]
            };
            // Phase F5-fix2（P-F5-3）: 終端の result.json を残して消えた run は、requeue せずに
            // その内容で確定させる。1 本でも確定させたらこの tick の回収はやめる（Task の遷移・lease は
            // 確定の経路が決めた。残りの死んだ WU の run は次の tick の照合が拾う）。
            let mut finalised_any = false;
            for run_id in &wu_runs {
                if self.finalise_from_result_json(&task, run_id) {
                    finalised_any = true;
                }
            }
            if finalised_any {
                count += 1;
                continue;
            }
            let metrics = run_since.map(|since| task_core::RunMetrics {
                wall_ms: wall_ms_since(since),
                retries: task.attempts,
                peak_context_tokens: None,
                turns: None,
            });
            let infra_n = consecutive_infra_requeues(&self.store.events_for(task.id)?) + 1;
            // Phase F5-fix6: 孤児は lease 切れではなく「持ち主のデーモンが居なくなって中断された run」。
            let why = if orphaned {
                ORPHAN_WHY
            } else {
                "lease expired"
            };
            let (trigger, outcome_text) = if infra_n <= self.config.max_infra_retries {
                (
                    Trigger::InfraRequeue,
                    if orphaned {
                        format!("interrupted: {why} (run_id={})", lease.worker_run_id)
                    } else {
                        format!("infra_requeue: {why} (run_id={})", lease.worker_run_id)
                    },
                )
            } else {
                (
                    Trigger::WorkerError { retryable: false },
                    format!(
                        "{INFRA_FAILURE_MARKER}{infra_n}: {why} (run_id={})",
                        lease.worker_run_id
                    ),
                )
            };
            let class = if orphaned {
                task_core::HarnessErrorClass::Infra
            } else {
                task_core::HarnessErrorClass::LeaseExpired
            };
            let wu_reason = if orphaned {
                crate::orphan::ORPHAN_TAKEOVER_REASON
            } else {
                "restart_reconcile"
            };
            let finished: Vec<Event> = wu_runs
                .iter()
                .map(|run_id| Event::WorkerFinished {
                    run_id: run_id.clone(),
                    outcome: outcome_text.clone(),
                    usage: None,
                    role: None,
                    metrics,
                    end: Some(task_core::RunEnd::HarnessError { class }),
                })
                .collect();
            match self
                .store
                .apply_transition_with_events(task.id, trigger.clone(), finished)
            {
                Ok(outcome) => {
                    tracing::warn!(task_id = %task.id, run_id = %lease.worker_run_id, next = ?outcome.next, attempts = outcome.attempts, orphaned, "{why}; reclaimed");
                    // 孤児（再起動の中断）はバックオフしない（インフラの不調ではなく人の再起動）。
                    if matches!(trigger, Trigger::InfraRequeue) && !orphaned {
                        let until = now + infra_backoff_delay(infra_n);
                        self.infra_backoff.insert(task.id, until);
                    }
                    // ADR-0072 D15（Phase E2）: この run が計画のある Task の WU のものだったなら、
                    // その WU の行も `running` のまま残さず、checkpoint があれば `needs_continuation`、
                    // 無ければ `ready` に戻す（`WorkUnitTransitioned{reason: "restart_reconcile"}`）。
                    for run_id in &wu_runs {
                        if let Err(e) = self.reconcile_work_unit_run(task.id, run_id, wu_reason) {
                            tracing::warn!(task_id = %task.id, run_id = %run_id, error = %e, "failed to reconcile the work unit for a reclaimed lease");
                        }
                    }
                    if phase_lease
                        && let Err(e) = self.reconcile_integration(task.id, "restart_reconcile")
                    {
                        tracing::warn!(task_id = %task.id, error = %e, "failed to reconcile the phase integration for a reclaimed lease");
                    }
                    count += 1;
                }
                Err(StoreError::InvalidTransition(e)) => {
                    tracing::warn!(task_id = %task.id, error = %e, "lease reclaim skipped");
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(count)
    }

    /// ADR-0044 §5 Phase 53 追記（Phase 55）: **run の止め方はこれ 1 つ**。
    ///
    /// `cancel` / 人のコメントによる割り込み（`Interrupt`）/ 実時間・無入力のタイムアウト /
    /// リース喪失 / drain タイムアウトのどれも、ここを通って
    /// **ワーカーのプロセスグループに SIGTERM → `kill_grace_secs` → SIGKILL** を送る
    /// （`task_worker::kill_tree`）。ハーネスが起こした孫（`cargo test`、`node`、シェル）まで届く。
    /// タイムアウトだけは `task_worker::subprocess` の中でも同じ手順を踏むが、そちらが先に終わって
    /// いれば登録が無いので、ここは何もしない（二重には送らない）。
    ///
    /// tokio の `JoinHandle::abort()` は**従来どおり即座に**行う（run の記録を止めるための帳簿）。
    ///
    /// Phase 55/56 の合流（ADR-0044 P55-4 / ADR-0043 P56-7）: `container` が `Some`（= ADR-0043 D3 で
    /// コンテナ実行に倒した run）なら、`killpg` と**同じ 2 段**を
    /// `--label celeris.task=<task_id>` 越しにも送る（`<runtime> kill --signal TERM` → `grace` →
    /// `<runtime> rm -f`）。`killpg` は `<runtime> run` のクライアントにしか届かず、
    /// コンテナの中は別の PID 名前空間なので、これが無いと中のハーネスが生き残る。
    pub(super) fn stop_run(
        &self,
        run_id: &str,
        handle: JoinHandle<()>,
        container: Option<Arc<dyn task_worker::ContainerStopper>>,
    ) {
        task_worker::kill_tree_with(run_id, self.config.kill_grace, container);
        handle.abort();
    }

    /// レビュー側（判定コマンドの run と Reviewer run）の停止。run は 2 本ありうるので両方に送る。
    pub(super) fn stop_review(&self, entry: ReviewEntry) {
        task_worker::kill_tree(&entry.run_id, self.config.kill_grace);
        if let Some(review_run_id) = &entry.review_run_id {
            task_worker::kill_tree(review_run_id, self.config.kill_grace);
        }
        entry.handle.abort();
    }

    /// Phase F5-fix3: 止めた run に `WorkerFinished` がまだ無ければ、`interrupted: <why>`（`end =
    /// cancelled`。GUI では割り込みと同じ「失敗ではない」扱い、コメントの割り込みも消費しない）を追記する。
    /// ストアが同じトランザクションで `runs` 行を `cancelled` にする。失敗しても警告だけ。
    pub(super) fn close_aborted_run(
        &self,
        task_id: TaskId,
        run_id: &str,
        role: Option<RunRole>,
        why: &str,
    ) {
        let already = match self.store.events_for(task_id) {
            Ok(events) => events
                .iter()
                .any(|(_, e)| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id)),
            Err(e) => {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to read events before closing an aborted run");
                return;
            }
        };
        if already {
            return;
        }
        let finished = Event::WorkerFinished {
            run_id: run_id.to_string(),
            outcome: format!("interrupted: {why}"),
            usage: None,
            role,
            metrics: None,
            end: Some(task_core::RunEnd::Cancelled),
        };
        if let Err(e) = self.store.append_event(task_id, &finished) {
            tracing::warn!(%task_id, %run_id, error = %e, "failed to close an aborted run");
        }
    }

    /// ADR-0079 付記「R6-1」D4: 終端（done / failed / cancelled）の task なのに `runs` 索引で `running` のままの行を
    /// 閉じる（`TaskStore::close_runs_of_terminal_tasks`。`WorkerFinished{end: Cancelled}` を積む）。起動後の最初の
    /// tick と [`RUNS_RECONCILE_INTERVAL_SECS`] ごと。閉じた行は 1 行ずつ 1 回だけログに残す（閉じた行は二度と
    /// 見つからない）。終端への遷移は store が同じトランザクションで閉じるので、見つかるのは R6-1 より前の行だけ。
    pub(super) fn reconcile_terminal_runs(&mut self) {
        let now = self.now_utc();
        if let Some(last) = self.runs_reconciled_at
            && (now - last).whole_seconds() < RUNS_RECONCILE_INTERVAL_SECS
        {
            return;
        }
        self.runs_reconciled_at = Some(now);
        match self.store.close_runs_of_terminal_tasks() {
            Ok(closed) => {
                for (task_id, run_id) in closed {
                    tracing::warn!(%task_id, %run_id, "closed a runs index row left running on a terminal task (ADR-0079 R6-1)");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to close the runs index rows of terminal tasks");
            }
        }
    }

    /// ADR-0002 D9: ストア上で `running` でなくなった（cancel / ADR-0044 D2 の割り込み等）run を
    /// 強制終了する。打ち切ったタスクは `just_aborted` に入れ、**この tick では dispatch し直さない**。
    pub(super) fn abort_stale_runs(&mut self) -> Result<(), DispatchError> {
        self.just_aborted.clear();
        let keys: Vec<RunKey> = self.running.keys().cloned().collect();
        for key in keys {
            let id = key.task;
            let current = self.store.get(id)?;
            let still_ours = match (&current, self.running.get(&key)) {
                (Some(t), Some(entry)) => self.run_holds_lease(t, &entry.run_id)?,
                _ => false,
            };
            if !still_ours && let Some(entry) = self.running.remove(&key) {
                tracing::warn!(task_id = %id, run_id = %entry.run_id, "aborting run (task no longer running under this lease)");
                // ADR-0044 Phase 53 追記: プロセスグループごと止める（孫まで。コンテナならその中も）。
                let run_id = entry.run_id.clone();
                self.stop_run(&entry.run_id, entry.handle, entry.container);
                // Phase F5-fix3: cancel 等で止めた run は誰も `WorkerFinished` を書かない（割り込み・lease の
                // 回収なら書いてある）。書かれていなければ閉じる（`runs` 行も終端になる）。
                self.close_aborted_run(
                    id,
                    &run_id,
                    None,
                    "aborted (task no longer running under this lease)",
                );
                self.just_aborted.insert(id);
                // ADR-0074 D1.6/D1.7（Phase F2）: v2 の WU の run を止めたなら、WU を `running` の
                // まま残さない（割り込み・lease 喪失なら checkpoint の有無で needs_continuation /
                // ready に戻す。Cancel は `cancel_open_work_units` が cancelled にする）。
                if key.work_unit.is_some()
                    && let Some(t) = &current
                    && !t.status.is_terminal()
                    && let Err(e) = self.reconcile_work_unit_run(id, &entry.run_id, "aborted")
                {
                    tracing::warn!(task_id = %id, run_id = %entry.run_id, error = %e, "failed to reconcile the work unit of an aborted run");
                }
            }
        }
        // ADR-0074 D1.4/D1.6（Phase F2b）: Running でなくなった Task の統合も止める。Cancel なら
        // 未完了の WU を cancelled にし、WU の worktree とブランチを消す。
        let integrating: Vec<TaskId> = self.integrating.keys().copied().collect();
        for id in integrating {
            let still_running =
                matches!(self.store.get(id)?, Some(t) if t.status == Status::Running);
            if !still_running && let Some(entry) = self.integrating.remove(&id) {
                tracing::warn!(task_id = %id, "aborting the phase integration (task no longer running)");
                entry.handle.abort();
                self.just_aborted.insert(id);
            }
        }
        // Phase F5-fix2: Running でなくなった Task の WU の検査も止める（結果は捨てられるだけなので、
        // draining のインスタンスを待たせない）。
        let checking: Vec<(String, TaskId)> = self
            .checking
            .iter()
            .map(|(run_id, e)| (run_id.clone(), e.task_id))
            .collect();
        for (run_id, id) in checking {
            let still_running =
                matches!(self.store.get(id)?, Some(t) if t.status == Status::Running);
            if !still_running && let Some(entry) = self.checking.remove(&run_id) {
                tracing::warn!(task_id = %id, %run_id, "aborting the work unit checks (task no longer running)");
                entry.handle.abort();
            }
        }
        let aborted: Vec<TaskId> = self.just_aborted.iter().copied().collect();
        for id in aborted {
            if let Some(t) = self.store.get(id)?
                && t.status == Status::Cancelled
                && self.store.execution_plan_active(id)?.is_some()
            {
                self.cancel_open_work_units(&t)?;
            }
        }
        // レビュー中に cancel されたタスクの判定（Reviewer run を含む）も中断する。
        let ids: Vec<TaskId> = self.reviewing.keys().copied().collect();
        for id in ids {
            let still_reviewing =
                matches!(self.store.get(id)?, Some(t) if t.status == Status::Reviewing);
            if !still_reviewing && let Some(entry) = self.reviewing.remove(&id) {
                tracing::warn!(task_id = %id, "aborting review (task no longer reviewing)");
                // ADR-0076: 止めた Reviewer run の `QuotaActivity` も閉じる（Event は残さない）。
                if let (Some(review_run_id), Some(provider)) =
                    (entry.review_run_id.clone(), entry.provider.clone())
                {
                    self.release_quota_if_tracked(
                        &review_run_id,
                        entry.account.as_deref(),
                        entry.account_adapter,
                        &provider,
                        id,
                    );
                }
                if let Some(review_run_id) = entry.review_run_id.clone() {
                    self.close_aborted_run(
                        id,
                        &review_run_id,
                        Some(RunRole::Reviewer),
                        "review aborted (task no longer reviewing)",
                    );
                }
                self.stop_review(entry);
                self.pending_subjects.remove(&id);
            }
        }
        Ok(())
    }

    pub(super) fn recover_reviews(&mut self) -> Result<(), DispatchError> {
        let reviewing_tasks = self.store.list(Some(Status::Reviewing))?;
        // 承認待ちの記録は、まだ reviewing のタスクだけに保つ（cancel 等で抜けたものをスナップショットに残さない。ADR-0013 D4）。
        self.awaiting_human
            .retain(|id| reviewing_tasks.iter().any(|t| t.id == *id));
        for task in reviewing_tasks {
            if self.reviewing.contains_key(&task.id)
                || self.awaiting_children.contains_key(&task.id)
            {
                continue;
            }
            // ADR-0041 D5: 面倒を見ないタスクのレビューは拾わない（verify は他人のタスクを判定しない）。
            if !self.is_eligible(&task) {
                continue;
            }
            let events = self.store.events_for(task.id)?;
            let run_id = last_run_id(&events).unwrap_or_default();
            // 前 tick で見送った場合はメモリ上の done 内容、再起動後は runs/<run_id>/result.json から復元。
            let subject = match self.pending_subjects.remove(&task.id) {
                Some(s) => s,
                None => self
                    .task_dir(&task)
                    .map(|dir| subject_from_run_dir(&dir, &run_id))
                    .unwrap_or_default(),
            };
            if !self.spawn_review(task.id, run_id, &subject)? {
                self.pending_subjects.insert(task.id, subject);
            }
        }
        Ok(())
    }

    /// ADR-0074 D1.5（Phase F2）: その Task の鍵を持つ run の数（v1・atomic なら 0 か 1）。
    pub(super) fn running_for_task(&self, task_id: TaskId) -> usize {
        self.running.keys().filter(|k| k.task == task_id).count()
    }

    /// ADR-0074 D1.5: `run_id` の run を `running` から取り除いて返す（`Completion` は `run_id` を
    /// 運ぶので、終わった run の鍵はここで引く。並列度の上限は小さいので線形探索でよい）。
    pub(super) fn take_running_by_run_id(&mut self, run_id: &str) -> Option<RunEntry> {
        let key = self
            .running
            .iter()
            .find(|(_, e)| e.run_id == run_id)
            .map(|(k, _)| k.clone())?;
        self.running.remove(&key)
    }

    /// Phase F5-fix6: このインスタンスがその Task の run・検査・統合・レビューを手元に持っているか
    /// （`crate::orphan` の定義の 1.）。
    pub(super) fn holds_task_in_hand(&self, task_id: TaskId) -> bool {
        self.running.keys().any(|k| k.task == task_id)
            || self.checking.values().any(|e| e.task_id == task_id)
            || self.integrating.contains_key(&task_id)
            || self.reviewing.contains_key(&task_id)
            || self.awaiting_children.contains_key(&task_id)
    }

    /// Phase F5-fix6: run を抱えうる他のデーモンが 1 つも生きていないか（`crate::orphan::holder_gone`）。
    /// 孤児の回収が無効（設定なし）・このインスタンスが新しい仕事を受けていない（draining / standby）・
    /// `daemon_instances` が読めないときは `false`（従来どおり lease の失効を待つ）。1 回の照合の中では
    /// `cache` に覚えて DB を 1 回だけ読む。
    pub(super) fn lease_holders_gone(&self, cache: &mut Option<bool>, now: OffsetDateTime) -> bool {
        if let Some(v) = *cache {
            return v;
        }
        let gone = match &self.orphan_takeover {
            Some(t) if self.accepting_new_work => match self.store.instance_list() {
                Ok(rows) => crate::orphan::holder_gone(
                    &rows,
                    &t.instance_id,
                    now,
                    t.freshness,
                    t.pid_alive.as_ref(),
                ),
                Err(e) => {
                    tracing::warn!(error = %e, "could not read daemon_instances; not taking over orphaned runs this tick");
                    false
                }
            },
            _ => false,
        };
        *cache = Some(gone);
        gone
    }

    /// Phase F5-fix6: 孤児を回収することを log と event（`worker_progress`、`orphan_takeover: …`）に残す。
    pub(super) fn note_orphan_takeover(
        &self,
        task: &Task,
        run_id: &str,
        lease_until: OffsetDateTime,
    ) {
        tracing::warn!(
            task_id = %task.id, %run_id, lease_expires_at = %rfc3339(lease_until),
            reason = crate::orphan::ORPHAN_TAKEOVER_REASON,
            "the daemon that held this run is gone (no live active/draining instance besides this one, and the run is not in hand); taking it over without waiting for the lease (Phase F5-fix6)"
        );
        let ev = Event::worker_progress(
            run_id.to_string(),
            format!(
                "{}: このランを持っていたデーモンが居ないため、lease の期限（{}）を待たずに回収します。",
                crate::orphan::ORPHAN_TAKEOVER_REASON,
                rfc3339(lease_until)
            ),
        );
        if let Err(e) = self.store.append_event(task.id, &ev) {
            tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the orphan takeover");
        }
    }

    /// Phase F5-fix6: 工程の lease（v2）の WU の孤児 run で result.json が無いもの。`WorkerFinished`
    /// （`interrupted: …`、`harness_error(infra)`）で `runs` 行を閉じ、WU を ready / needs_continuation
    /// に戻す（reason `orphan_takeover`。Task は遷移させない）。
    pub(super) fn requeue_orphaned_work_unit_run(
        &mut self,
        task: &Task,
        run_id: &str,
    ) -> Result<(), DispatchError> {
        let already = self
            .store
            .events_for(task.id)?
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id));
        if !already {
            self.store.append_event(
                task.id,
                &Event::WorkerFinished {
                    run_id: run_id.to_string(),
                    outcome: format!("interrupted: {ORPHAN_WHY} (run_id={run_id})"),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: Some(task_core::RunEnd::HarnessError {
                        class: task_core::HarnessErrorClass::Infra,
                    }),
                },
            )?;
        }
        self.reconcile_work_unit_run(task.id, run_id, crate::orphan::ORPHAN_TAKEOVER_REASON)
    }

    pub(super) fn run_holds_lease(&self, task: &Task, run_id: &str) -> Result<bool, DispatchError> {
        if task.status != Status::Running {
            return Ok(false);
        }
        let Some(lease) = task.lease.as_ref() else {
            return Ok(false);
        };
        if lease.worker_run_id == run_id {
            return Ok(true);
        }
        if !is_phase_lease_holder(&lease.worker_run_id) {
            return Ok(false);
        }
        Ok(self.store.work_units_for(task.id)?.iter().any(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.lease_run_id.as_deref() == Some(run_id)
        }))
    }
}

/// ADR-0061（Phase 104）: dispatch した時刻（`RunEntry`/`ReviewEntry` の `since`）から今までの
/// 壁時計時間をミリ秒で計算する。`since` が未来（時計のずれ等）なら 0 に丸める。
pub(super) fn wall_ms_since(since: OffsetDateTime) -> u64 {
    (OffsetDateTime::now_utc() - since)
        .whole_milliseconds()
        .max(0) as u64
}
