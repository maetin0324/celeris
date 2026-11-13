//! ADR 2026-11-11 dispatcher-state-ownership D1「run の集合と lease の照合」: 実行中の run の表（run の集合の正本）・
//! review run の表・infra backoff・lease の照合の状態の owner（[`RunSupervisor`]）と、lease の回収の判断（D3、純粋関数）、
//! worker 完了と lease の回収が共有する再試行の判定（D4 retry-backoff の [`infra_retry_after`]）。
//!
//! task・WU を鍵にした待ち（WU の check・統合の process・人の承認待ち・持ち越した review 対象）は持たない
//! （`ExecutionCoordination` の行。D1）。lease の回収でそれらを消すときは、ここが返す鍵を入力に受けて呼び元が消す。
//! この module は `dispatcher` module を使わない（D2 の 3）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use task_core::{AccountAdapter, Event, HarnessErrorClass, TaskId, TaskStore, Trigger};
use time::OffsetDateTime;
use tokio::task::JoinHandle;

use crate::policy::ProviderId;
use crate::review::ReviewSubject;

/// ADR 2026-10-11 llm-source-harness-selection: run が admission で予約した `(source, account)`。
pub(crate) type RouteReservation = (
    task_core::llm_route::SourceId,
    task_core::llm_route::AccountId,
);

/// Phase F5-fix6: 持ち主のデーモンが居なくなった run を回収するときの理由の文。
pub(crate) const ORPHAN_WHY: &str = "orphan_takeover: the daemon holding this run is gone";

/// ADR-0074 D1.5（Phase F2）: 実行中の run の鍵。`work_unit` は v2 の並列 WU の run だけ `Some(work_units.id)`
/// （atomic・planner・v1 の WU の run は `None`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RunKey {
    pub(crate) task: TaskId,
    pub(crate) work_unit: Option<String>,
}

/// 手元で走っている worker run の 1 件。
pub(crate) struct RunEntry {
    pub(crate) run_id: String,
    pub(crate) provider: ProviderId,
    pub(crate) handle: JoinHandle<()>,
    /// dispatch した時刻（デーモンのスナップショット用。ADR-0013 D4）。
    pub(crate) since: OffsetDateTime,
    /// ADR-0018: コマンドを実行するクラスタ（ローカル実行なら `None`）。並列度の会計に使う。
    pub(crate) cluster: Option<String>,
    /// ADR-0024 D2/D3: プールから選んだアカウント（プールを使わないプロバイダなら `None`）。
    pub(crate) account: Option<String>,
    /// ADR-0025 D1: `account` が属するアダプタ（`account` が `None` なら `None`）。
    pub(crate) account_adapter: Option<AccountAdapter>,
    /// Phase 55/56 の合流（ADR-0044 P55-4 / ADR-0043 P56-7）: この run をコンテナで走らせているなら、
    /// ラベルでコンテナを止める口。`killpg` はコンテナの中の PID 名前空間には届かないので、
    /// 止めるときに `task_worker::kill_tree_with` に渡す。ホスト実行なら `None`。
    pub(crate) container: Option<Arc<dyn task_worker::ContainerStopper>>,
    /// ADR-0089（Phase R6-5）: CoS の対話 run か（`max_concurrency`・プールの `concurrency` に数えない）。
    pub(crate) cos: bool,
    /// ADR 2026-10-11 llm-source-harness-selection: the admitted `(source, account)` reservation owner.
    pub(crate) route: Option<RouteReservation>,
}

/// 手元で判定している review の 1 件（判定コマンドの run と、起こしていれば Reviewer run）。
pub(crate) struct ReviewEntry {
    /// Keep ownership until the verdict transaction has completed, across daemon handoff.
    pub(crate) _review_lock: Arc<std::fs::File>,
    pub(crate) handle: JoinHandle<()>,
    /// `Reviewer` run を起動する場合に選んだ `(source, account)`（並列度と admission の枠を消費する。
    /// ADR 2026-10-11 llm-source-harness-selection）。
    pub(crate) route: Option<RouteReservation>,
    /// レビューを延期（Reviewer run の供給側失敗）するときに次 tick へ持ち越す `done` の内容。
    pub(crate) subject: ReviewSubject,
    /// レビュー対象の run（デーモンのスナップショット用）。
    pub(crate) run_id: String,
    /// ADR-0018: 判定コマンドを実行するクラスタ（ローカルなら `None`）。
    pub(crate) cluster: Option<String>,
    /// Reviewer run を起動した場合のその run の id（スナップショットの `in_flight` 用。ADR-0014 D1）。
    pub(crate) review_run_id: Option<String>,
    pub(crate) since: OffsetDateTime,
    /// ADR-0024 D2/D3: プールから選んだアカウント（プールを使わない、または Reviewer run 自体を起動しない場合は `None`）。
    pub(crate) account: Option<String>,
    /// ADR-0025 D1: `account` が属するアダプタ（`account` が `None` なら `None`）。
    pub(crate) account_adapter: Option<AccountAdapter>,
}

/// 照合の周期を持つ 2 つの処理（D1 の lease の照合の状態）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reconcile {
    /// ADR-0079 付記 R6-1 D4: 終端の task の `running` のままの `runs` 行と統合依頼を閉じる。
    TerminalRecords,
    /// 持ち主の居ない `running` の `runs` 行を閉じる（`reconcile_ownerless_runs`）。
    OwnerlessRuns,
}

/// 実行中の run の表と lease の監督の状態（ADR 2026-11-11 dispatcher-state-ownership D1）。
/// field は private で、`Dispatcher` は method を通して読み書きする（D2）。
#[derive(Default)]
pub(crate) struct RunSupervisor {
    /// run の集合の正本。登録は dispatch（`register_run`）、削除は完了（`take_run_by_id`）・lease の回収
    /// （`take_task_runs`）・abort（`take_run`）・drain と shutdown（`take_runs`）。
    running: HashMap<RunKey, RunEntry>,
    /// 判定中の review の表（task ごとに 1 件）。
    reviewing: HashMap<TaskId, ReviewEntry>,
    /// ADR-0070 D3（Phase 116）: インフラ都合の requeue の後、この時刻までその task を dispatch しない。
    /// プロセス内メモリのみ（DB に持たない。再起動で消えるのは許容）。
    infra_backoff: HashMap<TaskId, OffsetDateTime>,
    /// ADR-0002 D9: この tick で abort した task（この tick では dispatch し直さない）。
    just_aborted: HashSet<TaskId>,
    /// handle が終わったのを 1 回見た review（次の照合でも残っていれば、判定の結果を受け取り損ねたとみなす）。
    lost_review_watch: HashSet<TaskId>,
    /// 最後に持ち主の居ない `running` の `runs` 行を照合した時刻。
    ownerless_reconciled_at: Option<OffsetDateTime>,
    /// 最後に終端の task の `runs` 行・統合依頼を照合した時刻。
    terminal_records_reconciled_at: Option<OffsetDateTime>,
}

impl RunSupervisor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    // ---- 実行中の run の表 ----

    /// dispatch した run を登録する。
    pub(crate) fn register_run(&mut self, key: RunKey, entry: RunEntry) {
        self.running.insert(key, entry);
    }

    /// 実行中の run の表（読み取り専用）。
    pub(crate) fn runs(&self) -> &HashMap<RunKey, RunEntry> {
        &self.running
    }

    /// 実行中の run の鍵（読み取り専用）。
    pub(crate) fn run_keys(&self) -> impl Iterator<Item = &RunKey> {
        self.running.keys()
    }

    /// その run id の run を手元に持っているか。
    pub(crate) fn holds_run_id(&self, run_id: &str) -> bool {
        self.running.values().any(|e| e.run_id == run_id)
    }

    /// ADR-0074 D1.5: その task の鍵を持つ run の数（v1・atomic なら 0 か 1）。
    pub(crate) fn runs_for_task(&self, task_id: TaskId) -> usize {
        self.running.keys().filter(|k| k.task == task_id).count()
    }

    /// その task の run を 1 本でも手元に持っているか。
    pub(crate) fn has_task_run(&self, task_id: TaskId) -> bool {
        self.running.keys().any(|k| k.task == task_id)
    }

    /// その task の run のうち、`alive` が真を返す最初の run（lease の延長の判断用）。
    pub(crate) fn alive_task_run(
        &self,
        task_id: TaskId,
        alive: impl Fn(&str) -> bool,
    ) -> Option<&RunEntry> {
        self.running
            .iter()
            .filter(|(k, _)| k.task == task_id)
            .map(|(_, e)| e)
            .find(|e| alive(&e.run_id))
    }

    /// worker run（CoS の対話 run を除く）の数。
    pub(crate) fn worker_run_count(&self) -> usize {
        self.running.values().filter(|e| !e.cos).count()
    }

    /// CoS の対話 run の数。
    pub(crate) fn cos_run_count(&self) -> usize {
        self.running.values().filter(|e| e.cos).count()
    }

    /// 鍵の run を表から外して返す（abort）。
    pub(crate) fn take_run(&mut self, key: &RunKey) -> Option<RunEntry> {
        self.running.remove(key)
    }

    /// ADR-0074 D1.5: `run_id` の run を表から外して返す（`Completion` は `run_id` を運ぶ）。
    pub(crate) fn take_run_by_id(&mut self, run_id: &str) -> Option<RunEntry> {
        let key = self
            .running
            .iter()
            .find(|(_, e)| e.run_id == run_id)
            .map(|(k, _)| k.clone())?;
        self.running.remove(&key)
    }

    /// lease の回収: [`reclaim_keys`] が選んだ鍵の run を表から外して返す（止めるのは呼び元）。
    pub(crate) fn take_task_runs(&mut self, task_id: TaskId) -> Vec<RunEntry> {
        reclaim_keys(self.running.keys(), task_id)
            .iter()
            .filter_map(|key| self.running.remove(key))
            .collect()
    }

    /// drain の打ち切りと shutdown の割り込み: 表の run をすべて外して返す。
    pub(crate) fn take_runs(&mut self) -> Vec<(RunKey, RunEntry)> {
        self.running.drain().collect()
    }

    /// ADR-0040 D4: drain の打ち切り。手元の run をすべてプロセスグループごと止め（SIGTERM →
    /// `kill_grace` → SIGKILL、コンテナならラベル越しにも）、止めた数を返す。DB は変えない。
    pub(crate) fn abort_runs(&mut self, kill_grace: Duration) -> usize {
        let mut aborted = 0;
        for (key, entry) in self.running.drain() {
            tracing::warn!(task_id = %key.task, run_id = %entry.run_id, "drain timeout; aborting the run (the lease will expire and the new active will reclaim it)");
            Self::stop_process(&entry.run_id, entry.handle, entry.container, kill_grace);
            aborted += 1;
        }
        aborted
    }

    /// ADR-0044 §5 Phase 53 追記（Phase 55）: run のプロセスグループに SIGTERM → `kill_grace` → SIGKILL を
    /// 送り（コンテナならラベル越しにも同じ 2 段）、tokio の task を即座に abort する。
    pub(crate) fn stop_process(
        run_id: &str,
        handle: JoinHandle<()>,
        container: Option<Arc<dyn task_worker::ContainerStopper>>,
        kill_grace: Duration,
    ) {
        task_worker::kill_tree_with(run_id, kill_grace, container);
        handle.abort();
    }

    /// 手元に run・review が 1 件も無いか。
    pub(crate) fn is_idle(&self) -> bool {
        self.running.is_empty() && self.reviewing.is_empty()
    }

    // ---- review run の表 ----

    pub(crate) fn register_review(&mut self, task_id: TaskId, entry: ReviewEntry) {
        self.reviewing.insert(task_id, entry);
    }

    /// 判定中の review の表（読み取り専用）。
    pub(crate) fn reviews(&self) -> &HashMap<TaskId, ReviewEntry> {
        &self.reviewing
    }

    pub(crate) fn is_reviewing(&self, task_id: TaskId) -> bool {
        self.reviewing.contains_key(&task_id)
    }

    pub(crate) fn take_review(&mut self, task_id: TaskId) -> Option<ReviewEntry> {
        self.reviewing.remove(&task_id)
    }

    pub(crate) fn take_reviews(&mut self) -> Vec<(TaskId, ReviewEntry)> {
        self.reviewing.drain().collect()
    }

    /// その run id を worker run・review の対象 run・Reviewer run のどれかとして手元に持っているか。
    pub(crate) fn holds_run_or_review(&self, run_id: &str) -> bool {
        self.holds_run_id(run_id)
            || self
                .reviewing
                .values()
                .any(|e| e.run_id == run_id || e.review_run_id.as_deref() == Some(run_id))
    }

    /// review 側（判定コマンドの run と Reviewer run）を止める。run は 2 本ありうるので両方に送る。
    pub(crate) fn stop_review(entry: ReviewEntry, kill_grace: Duration) {
        task_worker::kill_tree(&entry.run_id, kill_grace);
        if let Some(review_run_id) = &entry.review_run_id {
            task_worker::kill_tree(review_run_id, kill_grace);
        }
        entry.handle.abort();
    }

    /// 見失った review（handle が終わったのを前の照合でも見た）と固まった review（`since + ttl` を過ぎた）を
    /// 表から外して理由とともに返す。`ttl_of` が `None` を返した review は飛ばす。今回初めて終わりを見た review は
    /// 見張りに入れて次の照合まで待つ（判定の結果がまだ届いていないだけかもしれない）。
    pub(crate) fn take_lost_reviews(
        &mut self,
        now: OffsetDateTime,
        mut ttl_of: impl FnMut(TaskId) -> Option<Duration>,
        lost_why: &'static str,
        stuck_why: &'static str,
    ) -> Vec<(TaskId, ReviewEntry, &'static str)> {
        let reviewing = &self.reviewing;
        self.lost_review_watch
            .retain(|id| reviewing.contains_key(id));
        let mut lost: Vec<(TaskId, &'static str)> = Vec::new();
        let mut finished_now: Vec<TaskId> = Vec::new();
        for (task_id, entry) in &self.reviewing {
            if entry.handle.is_finished() {
                if self.lost_review_watch.contains(task_id) {
                    lost.push((*task_id, lost_why));
                } else {
                    finished_now.push(*task_id);
                }
                continue;
            }
            let Some(ttl) = ttl_of(*task_id) else {
                continue;
            };
            if now >= entry.since + ttl {
                lost.push((*task_id, stuck_why));
            }
        }
        self.lost_review_watch.extend(finished_now);
        lost.into_iter()
            .filter_map(|(task_id, why)| {
                self.lost_review_watch.remove(&task_id);
                self.reviewing
                    .remove(&task_id)
                    .map(|entry| (task_id, entry, why))
            })
            .collect()
    }

    // ---- infra backoff（D4 retry-backoff の期限） ----

    pub(crate) fn infra_backoff_until(&self, task_id: TaskId) -> Option<OffsetDateTime> {
        self.infra_backoff.get(&task_id).copied()
    }

    pub(crate) fn start_infra_backoff(&mut self, task_id: TaskId, until: OffsetDateTime) {
        self.infra_backoff.insert(task_id, until);
    }

    pub(crate) fn clear_infra_backoff(&mut self, task_id: TaskId) {
        self.infra_backoff.remove(&task_id);
    }

    // ---- abort した task ----

    pub(crate) fn clear_just_aborted(&mut self) {
        self.just_aborted.clear();
    }

    pub(crate) fn mark_just_aborted(&mut self, task_id: TaskId) {
        self.just_aborted.insert(task_id);
    }

    pub(crate) fn was_just_aborted(&self, task_id: TaskId) -> bool {
        self.just_aborted.contains(&task_id)
    }

    pub(crate) fn just_aborted(&self) -> Vec<TaskId> {
        self.just_aborted.iter().copied().collect()
    }

    // ---- lease の照合の周期 ----

    /// 前回から `interval_secs` 秒以上たっていれば（または初回なら）今を記録して `true`。
    pub(crate) fn reconcile_due(
        &mut self,
        which: Reconcile,
        now: OffsetDateTime,
        interval_secs: i64,
    ) -> bool {
        let last = match which {
            Reconcile::TerminalRecords => &mut self.terminal_records_reconciled_at,
            Reconcile::OwnerlessRuns => &mut self.ownerless_reconciled_at,
        };
        if let Some(prev) = *last
            && (now - prev).whole_seconds() < interval_secs
        {
            return false;
        }
        *last = Some(now);
        true
    }

    // ---- lease の解放と run の終了記録（D3 reclaim_expired_leases の副作用） ----

    /// [`reclaim_verdict`] の提案で lease を解放する: lease を持っていた run（`lease_runs`）ごとに終了記録
    /// （`WorkerFinished`）を作り、遷移と同じ transaction で積む。成功して提案が backoff を持てば期限を立てる。
    pub(crate) fn release_lease(
        &mut self,
        store: &dyn TaskStore,
        task_id: TaskId,
        verdict: &ReclaimVerdict,
        lease_runs: &[String],
        metrics: Option<task_core::RunMetrics>,
        now: OffsetDateTime,
    ) -> Result<task_core::Outcome, task_core::StoreError> {
        let finished: Vec<Event> = lease_runs
            .iter()
            .map(|run_id| Event::WorkerFinished {
                run_id: run_id.clone(),
                outcome: verdict.outcome.clone(),
                usage: None,
                role: None,
                metrics,
                end: Some(task_core::RunEnd::HarnessError {
                    class: verdict.class,
                }),
            })
            .collect();
        let outcome =
            store.apply_transition_with_events(task_id, verdict.trigger.clone(), finished)?;
        if let Some(delay) = verdict.backoff {
            self.start_infra_backoff(task_id, now + delay);
        }
        Ok(outcome)
    }
}

/// D4 retry-backoff: infra 障害の連続 `n` 回目（1 始まり）を再試行するなら、その前に待つ時間を返す。
/// `n` が `max_infra_retries` 以内なら `task_ops::derive::infra_backoff_delay(n)`、超えたら `None`（打ち切り）。
/// worker 完了（`worker_finish.rs`）・lease の回収・shutdown の割り込み（`leases.rs`）が使う。
pub(crate) fn infra_retry_after(n: u32, max_infra_retries: u32) -> Option<Duration> {
    (n <= max_infra_retries).then(|| task_ops::derive::infra_backoff_delay(n))
}

/// lease を回収する理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReclaimReason {
    /// lease の期限が過ぎた。
    Expired,
    /// Phase F5-fix6: lease はまだ切れていないが、持ち主のデーモンが居ない（`crate::orphan`）。
    Orphaned,
}

/// D3: lease を回収するか。期限が過ぎていれば [`ReclaimReason::Expired`]。切れていなければ、工程の lease でなく
/// 手元にも無く、持ち主のデーモンが居ない（`holders_gone`。必要なときだけ 1 回呼ぶ）ときだけ
/// [`ReclaimReason::Orphaned`]。それ以外は `None`（回収しない）。
pub(crate) fn reclaim_reason(
    expires_at: OffsetDateTime,
    now: OffsetDateTime,
    phase_lease: bool,
    in_hand: bool,
    holders_gone: impl FnOnce() -> bool,
) -> Option<ReclaimReason> {
    if expires_at <= now {
        return Some(ReclaimReason::Expired);
    }
    (!phase_lease && !in_hand && holders_gone()).then_some(ReclaimReason::Orphaned)
}

/// D3: 回収する手元の run の鍵（その task の鍵すべて）。
pub(crate) fn reclaim_keys<'a>(
    keys: impl IntoIterator<Item = &'a RunKey>,
    task_id: TaskId,
) -> Vec<RunKey> {
    keys.into_iter()
        .filter(|k| k.task == task_id)
        .cloned()
        .collect()
}

/// D3: lease の回収の提案（遷移・lease を持っていた run の終了記録の文と分類・WU を戻す理由・backoff）。
#[derive(Debug, Clone)]
pub(crate) struct ReclaimVerdict {
    pub(crate) trigger: Trigger,
    pub(crate) outcome: String,
    pub(crate) class: HarnessErrorClass,
    pub(crate) wu_reason: &'static str,
    /// `InfraRequeue` で、孤児でない（インフラの不調）ときだけ `Some`。
    pub(crate) backoff: Option<Duration>,
}

/// D3: lease の回収の trigger を決める。`infra_n` は連続の infra requeue の回数 + 1、`lease_run_id` は lease の持ち主。
/// 上限以内なら `InfraRequeue`（孤児は `interrupted:`、期限切れは `infra_requeue:` の文）、超えたら
/// `WorkerError{retryable: false}`（`infra failure ×n`）。孤児（人の再起動）は backoff しない。
pub(crate) fn reclaim_verdict(
    reason: ReclaimReason,
    infra_n: u32,
    max_infra_retries: u32,
    lease_run_id: &str,
) -> ReclaimVerdict {
    let orphaned = reason == ReclaimReason::Orphaned;
    let why = if orphaned {
        ORPHAN_WHY
    } else {
        "lease expired"
    };
    let retry = infra_retry_after(infra_n, max_infra_retries);
    let (trigger, outcome) = match retry {
        Some(_) if orphaned => (
            Trigger::InfraRequeue,
            format!("interrupted: {why} (run_id={lease_run_id})"),
        ),
        Some(_) => (
            Trigger::InfraRequeue,
            format!("infra_requeue: {why} (run_id={lease_run_id})"),
        ),
        None => (
            Trigger::WorkerError { retryable: false },
            format!(
                "{}{infra_n}: {why} (run_id={lease_run_id})",
                task_ops::derive::INFRA_FAILURE_MARKER
            ),
        ),
    };
    ReclaimVerdict {
        trigger,
        outcome,
        class: if orphaned {
            HarnessErrorClass::Infra
        } else {
            HarnessErrorClass::LeaseExpired
        },
        wu_reason: if orphaned {
            crate::orphan::ORPHAN_TAKEOVER_REASON
        } else {
            "restart_reconcile"
        },
        backoff: retry.filter(|_| !orphaned),
    }
}

#[cfg(test)]
impl RunSupervisor {
    /// 見張っている review があるか（試験専用）。
    pub(crate) fn watches_lost_reviews(&self) -> bool {
        !self.lost_review_watch.is_empty()
    }

    /// 次の照合を周期に関わらず走らせる（試験専用）。
    pub(crate) fn reset_reconcile(&mut self, which: Reconcile) {
        match which {
            Reconcile::TerminalRecords => self.terminal_records_reconciled_at = None,
            Reconcile::OwnerlessRuns => self.ownerless_reconciled_at = None,
        }
    }

    /// 試験専用: run の表を直接触る（handle を待つ・`since` を差し替える）。
    pub(crate) fn runs_mut_for_test(&mut self) -> &mut HashMap<RunKey, RunEntry> {
        &mut self.running
    }

    /// 試験専用: review の表を直接触る。
    pub(crate) fn reviews_mut_for_test(&mut self) -> &mut HashMap<TaskId, ReviewEntry> {
        &mut self.reviewing
    }

    /// 試験専用: infra backoff をすべて外す。
    pub(crate) fn clear_infra_backoff_for_test(&mut self) {
        self.infra_backoff.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).expect("valid time")
    }

    /// 不変条件（D4 retry-backoff）: 上限以内の n だけ再試行し、待ち時間は `infra_backoff_delay(n)` と同じ。
    /// 既存の試験は worker 完了と lease の回収を別々に見るので、片方の比較が `<` に変わる（上限の 1 回を失う）と
    /// その経路の試験しか落ちない。共有の関数の境界をここで固定する。
    #[test]
    fn infra_retry_after_stops_after_the_limit() {
        assert_eq!(
            infra_retry_after(1, 3),
            Some(task_ops::derive::infra_backoff_delay(1))
        );
        assert_eq!(
            infra_retry_after(3, 3),
            Some(task_ops::derive::infra_backoff_delay(3))
        );
        assert_eq!(infra_retry_after(4, 3), None);
        assert_eq!(infra_retry_after(1, 0), None);
    }

    /// 不変条件（D3、lease の持ち主の判定）: 期限が過ぎた lease は持ち主に関わらず回収し、切れていない lease は
    /// 工程の lease・手元の run・生きている持ち主のどれかがあれば回収しない。持ち主の照合は必要なときだけ呼ぶ。
    /// 既存の試験（orphan_takeover）は tick 全体を通すので、`holders_gone` を不要に呼ぶ（DB を毎回読む）変化を検出しない。
    #[test]
    fn reclaim_reason_follows_expiry_and_holders() {
        let never = || panic!("holders_gone must not be called");
        assert_eq!(
            reclaim_reason(at(0), at(0), true, true, never),
            Some(ReclaimReason::Expired)
        );
        assert_eq!(reclaim_reason(at(10), at(0), true, false, never), None);
        assert_eq!(reclaim_reason(at(10), at(0), false, true, never), None);
        assert_eq!(reclaim_reason(at(10), at(0), false, false, || false), None);
        assert_eq!(
            reclaim_reason(at(10), at(0), false, false, || true),
            Some(ReclaimReason::Orphaned)
        );
    }

    /// 不変条件（D3）: 回収の提案は、期限切れなら backoff つきの `InfraRequeue`、孤児なら backoff なしの
    /// `InfraRequeue`（`interrupted:`）、上限を超えたらどちらも `WorkerError{retryable: false}`（`infra failure ×n`）。
    /// 既存の試験で検出できない理由: lease の回収の試験は上限超えの孤児と WU を戻す理由を組み合わせて見ていない。
    #[test]
    fn reclaim_verdict_chooses_trigger_backoff_and_reason() {
        let v = reclaim_verdict(ReclaimReason::Expired, 1, 3, "r1");
        assert!(matches!(v.trigger, Trigger::InfraRequeue));
        assert_eq!(v.outcome, "infra_requeue: lease expired (run_id=r1)");
        assert_eq!(v.class, HarnessErrorClass::LeaseExpired);
        assert_eq!(v.wu_reason, "restart_reconcile");
        assert_eq!(v.backoff, infra_retry_after(1, 3));

        let v = reclaim_verdict(ReclaimReason::Orphaned, 2, 3, "r1");
        assert!(matches!(v.trigger, Trigger::InfraRequeue));
        assert_eq!(v.outcome, format!("interrupted: {ORPHAN_WHY} (run_id=r1)"));
        assert_eq!(v.class, HarnessErrorClass::Infra);
        assert_eq!(v.wu_reason, crate::orphan::ORPHAN_TAKEOVER_REASON);
        assert_eq!(v.backoff, None);

        for reason in [ReclaimReason::Expired, ReclaimReason::Orphaned] {
            let v = reclaim_verdict(reason, 4, 3, "r1");
            assert!(matches!(
                v.trigger,
                Trigger::WorkerError { retryable: false }
            ));
            assert!(
                v.outcome
                    .starts_with(task_ops::derive::INFRA_FAILURE_MARKER)
            );
            assert!(v.outcome.contains("4: "));
            assert_eq!(v.backoff, None);
        }
    }

    /// 不変条件（D3）: 回収する鍵はその task の鍵すべて（v2 の並列 WU の run を含む）で、他の task の鍵を含まない。
    #[test]
    fn reclaim_keys_selects_the_task_runs() {
        let a = TaskId::new();
        let b = TaskId::new();
        let keys = [
            RunKey {
                task: a,
                work_unit: None,
            },
            RunKey {
                task: b,
                work_unit: None,
            },
            RunKey {
                task: a,
                work_unit: Some("wu".into()),
            },
        ];
        let got = reclaim_keys(keys.iter(), a);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|k| k.task == a));
    }

    /// 不変条件: 照合は初回と周期を過ぎた後だけ走り、2 つの照合の時刻は独立。
    #[test]
    fn reconcile_due_tracks_each_interval() {
        let mut s = RunSupervisor::new();
        assert!(s.reconcile_due(Reconcile::TerminalRecords, at(0), 60));
        assert!(!s.reconcile_due(Reconcile::TerminalRecords, at(59), 60));
        assert!(s.reconcile_due(Reconcile::OwnerlessRuns, at(59), 60));
        assert!(s.reconcile_due(Reconcile::TerminalRecords, at(60), 60));
        s.reset_reconcile(Reconcile::OwnerlessRuns);
        assert!(s.reconcile_due(Reconcile::OwnerlessRuns, at(61), 60));
    }

    /// 不変条件: ADR 2026-11-11 D2 の 1〜3・7（field が private、`Dispatcher` を受けない・`dispatcher` module を
    /// 使わない、field は D1 の表の own-supervisor の 7 件。`#[cfg(test)]` の field は
    /// `dispatcher/tests/structure_guard.rs` の `OWNER_STRUCTS` が数える）。既存の試験は振る舞いだけを見るので、
    /// field を `pub` に戻しても、method が `&mut Dispatcher` を受けても通る。
    #[test]
    fn structure_guard_run_supervisor() {
        let src = include_str!("run_supervisor.rs");
        let src = &src[..src
            .find("#[cfg(test)]\nimpl RunSupervisor")
            .expect("the test-only impl is in the source")];
        let start = src
            .find("pub(crate) struct RunSupervisor {")
            .expect("RunSupervisor is defined");
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
                "a RunSupervisor field is not private: {line}"
            );
            if let Some((name, _)) = t.split_once(": ") {
                fields.push(name.to_string());
            }
        }
        fields.sort();
        assert_eq!(
            fields,
            [
                "infra_backoff",
                "just_aborted",
                "lost_review_watch",
                "ownerless_reconciled_at",
                "reviewing",
                "running",
                "terminal_records_reconciled_at",
            ],
            "RunSupervisor owns the 7 own-supervisor fields of ADR D1"
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
                "run_supervisor.rs must not contain {bad:?}"
            );
        }
    }
}
