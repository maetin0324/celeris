//! 持ち主の居ない `running` の `runs` 行の照合（lease を持たない reviewer run 等の取り残し）。ADR-0082 の L3。

use super::*;

/// 手元のレビューの tokio task が判定の完了を届けずに終わった（panic・abort）ときの、reviewer run を閉じる
/// 理由（`WorkerFinished` の `interrupted: <why>`）。
pub(super) const LOST_REVIEW_WHY: &str =
    "review lost (the review ended without delivering a verdict)";
/// 手元のレビューが期限（[`Dispatcher::ownerless_ttl`]）を越えても終わらないときの理由。
pub(super) const STUCK_REVIEW_WHY: &str = "review stopped (it outlived its deadline)";

impl Dispatcher {
    /// 手元のレビュー（`reviewing`）の取りこぼしを閉じる。毎 tick 走る（active・draining を問わない。
    /// 残ったままだと `in_flight` が 0 にならず、drain も「running が 0 になるのを待つ」運用も終わらない）。
    ///
    /// - **完了が届かない**: レビューの tokio task は終わっている（`JoinHandle::is_finished`）のに、判定の
    ///   完了（`Completion::Review`）が `drain_completions` に届かなかった（panic・abort）。完了は task が
    ///   終わる前に送られ、次の tick の冒頭の `drain_completions` が拾うので、**2 tick 続けて**終わって
    ///   見えた task だけを取りこぼしと判定する（1 tick 目は `lost_review_watch` に覚えるだけ）。
    /// - **期限越え**: `since + ownerless_ttl(reviewer)`（検査と reviewer run の timeout の合計を十分に
    ///   越える長さ）を過ぎても終わらない。検査・reviewer run は各々 timeout で終わるので、越えるのは
    ///   何かで固まったレビューだけ。`since` と同じ実時計で測る（試験は `since` を差し替えて再現する）。
    ///
    /// どちらも reviewer run を起こしていれば quota を解放し、`WorkerFinished{role: reviewer, end:
    /// cancelled, outcome: "interrupted: <why>"}` で `runs` 行を閉じ、レビューを止める。Task は
    /// `reviewing` のまま（同じ tick の `recover_reviews` がレビューをやり直す。attempts は消費しない）。
    pub(super) fn reap_lost_reviews(&mut self) {
        // `ReviewEntry.since` は実時計で記録される（`spawn_review`）ので、期限も同じ実時計で測る
        // （差し替えた試験用の時計と混ぜると、時計を進める試験で生きたレビューを期限越えと誤る）。
        let now = OffsetDateTime::now_utc();
        self.lost_review_watch
            .retain(|id| self.reviewing.contains_key(id));
        let mut lost: Vec<(TaskId, &'static str)> = Vec::new();
        let mut finished_now: Vec<TaskId> = Vec::new();
        for (task_id, entry) in &self.reviewing {
            if entry.handle.is_finished() {
                if self.lost_review_watch.contains(task_id) {
                    lost.push((*task_id, LOST_REVIEW_WHY));
                } else {
                    finished_now.push(*task_id);
                }
                continue;
            }
            let ttl = match self.store.get(*task_id) {
                Ok(Some(task)) => self.ownerless_ttl(task_core::RunIndexRole::Reviewer, &task),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(%task_id, error = %e, "could not read a reviewing task to check its review deadline");
                    continue;
                }
            };
            if now >= entry.since + ttl {
                lost.push((*task_id, STUCK_REVIEW_WHY));
            }
        }
        self.lost_review_watch.extend(finished_now);
        for (task_id, why) in lost {
            self.lost_review_watch.remove(&task_id);
            let Some(entry) = self.reviewing.remove(&task_id) else {
                continue;
            };
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
                self.close_aborted_run(task_id, &review_run_id, Some(RunRole::Reviewer), why);
            }
            tracing::warn!(
                %task_id, run_id = %entry.run_id, review_run_id = ?entry.review_run_id, why,
                "closing a review this daemon lost track of; the task is re-reviewed"
            );
            self.stop_review(entry);
        }
    }

    /// `runs` 索引で `running` のまま、どのインスタンスも抱えていない run の行を閉じる。Reviewer run は
    /// lease を持たないので、`reclaim_expired_leases` と孤児の回収（`crate::orphan`）の対象にならず、
    /// 持ち主のデーモンが居なくなる（クラッシュ等）と `runs` 行が Task の終端まで `running` で残っていた。
    ///
    /// active（新しい仕事を受けている）で孤児の回収が有効なときだけ、その最初の tick と
    /// [`RUNS_RECONCILE_INTERVAL_SECS`] ごとに走る（`--mode verify`・設定の無い組み立てでは何もしない）。
    /// 行ごとに:
    ///
    /// 1. このインスタンスが run id で抱えている（`running` / `reviewing` / `checking`）なら飛ばす。
    /// 2. Task が無い・面倒を見ない・終端（`reconcile_terminal_records` の受け持ち）なら飛ばす。
    /// 3. Task / WU の lease を持つ run は飛ばす（lease の経路が Task と WU の状態ごと戻す）。
    /// 4. [`crate::orphan::ownerless_run_decision`] で決める: 他に生きているインスタンスが無ければ閉じる。あれば
    ///    run の期限（[`Self::ownerless_ttl`]）を過ぎるまで待つ。
    /// 5. 閉じる = `WorkerFinished{end: cancelled, outcome: "interrupted: <why>"}`（store が同じトランザクション
    ///    で `runs` 行を閉じる）。Task は遷移させない（`reviewing` のまま持ち主の居ないレビューは、同じ tick の
    ///    `recover_reviews` がやり直す）。
    pub(super) fn reconcile_ownerless_runs(&mut self) {
        if !self.accepting_new_work || self.orphan_takeover.is_none() {
            return;
        }
        let now = self.now_utc();
        if let Some(last) = self.ownerless_reconciled_at
            && (now - last).whole_seconds() < RUNS_RECONCILE_INTERVAL_SECS
        {
            return;
        }
        self.ownerless_reconciled_at = Some(now);
        let rows = match self.store.runs_running() {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "failed to list the running rows of the runs index");
                return;
            }
        };
        let mut holders_gone: Option<bool> = None;
        for row in rows {
            if self.holds_run_in_hand(&row.run_id) {
                continue;
            }
            let Ok(task_id) = row.task_id.parse::<TaskId>() else {
                tracing::warn!(task_id = %row.task_id, run_id = %row.run_id, "runs index row with an invalid task id");
                continue;
            };
            let task = match self.store.get(task_id) {
                Ok(Some(t)) => t,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(%task_id, run_id = %row.run_id, error = %e, "could not read the task of a running runs index row");
                    return;
                }
            };
            if !self.is_eligible(&task) || task.status.is_terminal() {
                continue;
            }
            match self.run_holds_lease(&task, &row.run_id) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(%task_id, run_id = %row.run_id, error = %e, "could not check the lease of a running runs index row");
                    return;
                }
            }
            let gone = self.lease_holders_gone(&mut holders_gone, now);
            let ttl = self.ownerless_ttl(row.role, &task);
            // 読めない `started_at` は「今始まった」とみなす（期限では閉じない。持ち主が居なければ閉じる）。
            let started_at = OffsetDateTime::parse(&row.started_at, &Rfc3339).unwrap_or(now);
            let crate::orphan::OwnerlessRun::Close { why } =
                crate::orphan::ownerless_run_decision(gone, started_at, ttl, now)
            else {
                continue;
            };
            let role = match row.role {
                task_core::RunIndexRole::Worker => Some(RunRole::Worker),
                task_core::RunIndexRole::Reviewer => Some(RunRole::Reviewer),
                task_core::RunIndexRole::Planner => Some(RunRole::Planner),
                task_core::RunIndexRole::WrapUp => None,
            };
            tracing::warn!(
                %task_id, run_id = %row.run_id, role = row.role.as_str(), why,
                "closing a runs index row left running without an owner"
            );
            self.close_aborted_run(task_id, &row.run_id, role, why);
        }
    }

    /// 他に生きているインスタンスがあるとき、lease を持たない run の行を閉じるまで待つ長さ（`started_at` から）。
    ///
    /// Reviewer run の行は `spawn_review` が決定的な検査より**前**に書く。検査は 1 本あたり timeout と
    /// 2 倍の timeout の再試行で最大 `3 × review_timeout` かかり、その後に reviewer run（`review_timeout`）が
    /// 走る。暗黙の検査（repo の検査・証拠）の分を 1 本余分に見て `3 × (条件数 + 2) × review_timeout` とする
    /// （長めに倒す。生きている持ち主の run を早く閉じると、後から届く本当の終わりが索引に載らない）。
    /// それ以外の role は Task の `max_wall_secs`。どちらも `lease_grace` を足す。
    pub(super) fn ownerless_ttl(&self, role: task_core::RunIndexRole, task: &Task) -> Duration {
        let ttl = match role {
            task_core::RunIndexRole::Reviewer => {
                let checks = u32::try_from(task.acceptance.len()).unwrap_or(u32::MAX);
                self.config
                    .review_timeout
                    .saturating_mul(checks.saturating_add(2).saturating_mul(3))
            }
            _ => Duration::from_secs(task.budget.max_wall_secs),
        };
        ttl.saturating_add(self.config.lease_grace)
    }

    /// このインスタンスがその run（run id）を手元に持っているか（worker run・レビューの対象 run と
    /// Reviewer run・WU の検査）。
    fn holds_run_in_hand(&self, run_id: &str) -> bool {
        self.running.values().any(|e| e.run_id == run_id)
            || self
                .reviewing
                .values()
                .any(|e| e.run_id == run_id || e.review_run_id.as_deref() == Some(run_id))
            || self.checking.contains_key(run_id)
    }

    /// テスト専用: デーモンのクラッシュ（SIGKILL・OOM 等）を模す。手元の run・レビュー・検査・統合の
    /// task を止めるだけで、DB には何も書かない（`daemon_instances` の行もそのまま）。
    #[cfg(test)]
    pub(super) fn crash_for_test(&mut self) {
        for (_, entry) in self.running.drain() {
            entry.handle.abort();
        }
        for (_, entry) in self.reviewing.drain() {
            entry.handle.abort();
        }
        for (_, entry) in self.checking.drain() {
            entry.handle.abort();
        }
        for (_, entry) in self.integrating.drain() {
            entry.handle.abort();
        }
        self.pending_subjects.clear();
    }
}
