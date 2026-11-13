//! ADR 2026-11-11 dispatcher-state-ownership D1: task・WU の実行の調整の状態の owner（[`ExecutionCoordination`]）。
//!
//! unit own-coord-a が移したもの（D1 の表で unit 列が own-coord-a の行）: write set の予約と見送りの記録（ADR-0130 D3）、
//! actual write-set の採取場所（ADR-0130 D2）、review 前 sync の待ち行列（ADR-0130 D5）と再同期の上限で止めた review
//! （ADR 2026-10-27）、既定 branch に入った root の依存先（ADR 2026-10-17）、WU の worktree の用意の失敗の回数
//! （ADR-0074 F5-fix7）、credential 待ちに入れた atomic task と再開の処理中の印（ADR 2026-10-15 D4）、
//! browser の適合台帳の見張り（ADR 2026-10-08 D2）、クラスタ待ちの task（ADR-0018 D2）。
//!
//! run の集合は持たない（D1「run の集合と lease の照合」。正本は `crate::run_supervisor::RunSupervisor`）。write set の
//! blocker の絞り込みと予約の刈り込みは、呼び元が `RunSupervisor::run_keys` で渡す実行中の run の鍵だけを見る。
//! store・時計・試験の ablation は引数で受ける。この module は `dispatcher` module を使わない（D2 の 3）。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use task_core::{StoreError, Task, TaskId, TaskStore};
use task_ops::daemon::SnapshotReviewResyncHalted;
use task_worker::browser_ledger::LedgerWatch;
use time::OffsetDateTime;

use crate::run_supervisor::RunKey;

/// 連続してこの回数見送られた ready task は、待機開始の古い順に候補の先頭へ移す（ADR-0130 D3）。
pub(crate) const WRITE_SET_STARVATION_TICKS: u32 = 3;

/// run を起こした時点の hint で作る予約（後から hint を変えても変わらない。ADR-0130 D3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteReservation {
    /// repo の鍵（`repo:<RepoId>` / `path:<dir>` / `remote:<cluster>:<path>`）。
    repos: Vec<String>,
    /// 正規化した prefix（`task_core::write_set::normalize_write_paths` 済み）。
    paths: Vec<String>,
    /// WU の hint が task の有効 hint をそのまま継いだものか（同じ task の兄弟とは照合しない）。
    inherited_from_task: bool,
}

impl WriteReservation {
    pub(crate) fn new(repos: Vec<String>, paths: Vec<String>, inherited_from_task: bool) -> Self {
        Self {
            repos,
            paths,
            inherited_from_task,
        }
    }
}

/// 見送りの記録（`RunKey` ごと）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteSetWait {
    pub(crate) since_tick: u64,
    pub(crate) last_tick: u64,
    pub(crate) consecutive: u32,
    /// 待機を作った順の通し番号（同じ `since_tick` の待機の先後）。
    first_seq: u64,
    /// 見送られた候補の予約。連続回数が閾値に届いたら、後から待ち始めた重なる候補への先取りになる。
    reservation: WriteReservation,
}

impl WriteSetWait {
    /// 先取りが効くか（連続 `WRITE_SET_STARVATION_TICKS` 回以上待った）。
    fn claims(&self) -> bool {
        self.consecutive >= WRITE_SET_STARVATION_TICKS
    }
}

/// 待機の先後（待機開始の古い順、同じ tick なら先に待ち始めた順）。
fn wait_order(wait: &WriteSetWait) -> (u64, u64) {
    (wait.since_tick, wait.first_seq)
}

/// 2 つの予約が強く重なるか（同じ repo 鍵を共有し、prefix が等しいか segment 境界の祖先）。
pub(crate) fn reservations_overlap(
    a_task: TaskId,
    a: &WriteReservation,
    b_task: TaskId,
    b: &WriteReservation,
) -> bool {
    if a_task == b_task && a.inherited_from_task && b.inherited_from_task {
        return false;
    }
    a.repos.iter().any(|repo| b.repos.contains(repo))
        && task_core::write_set::write_set_overlap(&a.paths, &b.paths)
}

/// write-set gate の結果（ADR-0130 D3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WriteSetAdmission {
    /// 起こしてよい。起こしたら [`ExecutionCoordination::reserve_write_set`] にこの予約を渡す（`None` は予約しない）。
    Admit(Option<WriteReservation>),
    /// 重なる run（または先取りを持つ待機）があるので次 tick に回す。見送りは記録済み。
    Hold { blocker: RunKey },
}

/// run 1 本・repo 1 つ分の actual write-set の採取場所と開始 HEAD（ADR-0130 D2。プロセス内メモリのみ）。
#[derive(Debug, Clone)]
pub(crate) struct RunWriteBase {
    pub(crate) repo_id: task_core::RepoId,
    pub(crate) dir: PathBuf,
    /// `None` は開始 HEAD を読めなかった（終了時に `unavailable`）。
    pub(crate) start_sha: Option<String>,
}

/// review 前 sync の待ち行列の 1 件（ADR-0130 D5。プロセス内メモリのみ、再起動で作り直す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReviewSyncWait {
    /// Wait start order (monotonic per process).
    pub(crate) seq: u64,
    /// Consecutive eligible ticks in which the review was not started.
    pub(crate) skipped_ticks: u32,
}

/// 同じ WU の worktree の用意が連続して失敗した回数と、次に試してよい時刻（プロセス内メモリのみ）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct WuPrepareFailures {
    pub(crate) count: u32,
    pub(crate) retry_at: OffsetDateTime,
}

/// task・WU の実行の調整の状態（ADR 2026-11-11 dispatcher-state-ownership D1）。
/// field は private で、呼び元は method を通して読み書きする（D2）。
pub(crate) struct ExecutionCoordination {
    /// ADR-0130 D3: 走っている run の expected write-set の予約（run を起こした時点の hint）。
    write_reservations: HashMap<RunKey, WriteReservation>,
    /// ADR-0130 D3: write-set の重なりで見送った候補の待機記録（公平性）。
    write_set_waits: HashMap<RunKey, WriteSetWait>,
    /// 待機を作った順の通し番号（同じ tick に待ち始めた待機の先後を決定的に決める。2026-10-03-write-set-no-starvation）。
    write_set_hold_seq: u64,
    /// ADR-0130 D2: run（id）ごとの actual write-set の採取場所と開始 HEAD。dispatch で入れ、worker の
    /// 終了処理で取り出す。プロセス内メモリのみ（再起動後に終わった run は `unavailable` として残す）。
    run_write_bases: HashMap<String, Vec<RunWriteBase>>,
    /// ADR-0130 D5: review 前 sync を待つ reviewing task（待機開始順と、選ばれなかった連続 tick 数）。
    review_sync_queue: HashMap<TaskId, ReviewSyncWait>,
    /// ADR-0130 D5: 次の待機開始順。
    review_sync_seq: u64,
    /// ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse D3: 再同期の上限で止めている reviewing の task
    /// （snapshot の `review_resync_halted`。受信箱の attention の元）。review 入口で毎回作り直す。
    review_resync_halted: BTreeMap<TaskId, SnapshotReviewResyncHalted>,
    /// ADR 2026-10-17-root-dependency-waits-for-integration D2: 既定 branch の祖先と分かった root の依存先（再起動で忘れてよい）。
    integrated_deps: BTreeSet<TaskId>,
    /// ADR-0074「Phase F5-fix7 実装時の明確化」: WU（id）の worktree の用意の一時的な失敗の回数と
    /// 次に試してよい時刻。成功・blocked にしたら消す。プロセス内メモリのみ（再起動で数え直す）。
    wu_prepare_failures: HashMap<String, WuPrepareFailures>,
    /// credential 待ちに入れた計画なしの task（tick ごとに同じ wait を書き直さない。再起動で `restore_credential_waits` が戻す）。
    credential_held: HashSet<TaskId>,
    /// ADR 2026-10-15-mlsys-min-decision-assisted D4: credential 待ちの検証・再開の thread が走っているか（同時に 1 つまで）。
    credential_busy: Arc<AtomicBool>,
    /// ADR 2026-10-08-browser-prod-enablement D2: 適合台帳の状態の cache（file が変わったときだけ計算し直す）。
    browser_ledger: LedgerWatch,
    /// 台帳が変わった（または差し替えた）ので、`browser_prerequisite` で止めた task を次の tick で見直す。
    browser_prereq_rescan: bool,
    /// ADR-0018 D2（監査 4-1）: この tick でクラスタの多重接続が無い／cooldown 中のため待っている ready タスク。「人のログイン待ち」で
    /// 経路なし（`unroutable`）とは別物。`is_idle` の待ち対象から外すだけで、スナップショットには出さない（受信箱の (d) が知らせる）。
    cluster_waiting: HashSet<TaskId>,
}

impl ExecutionCoordination {
    pub(crate) fn new() -> Self {
        Self {
            write_reservations: HashMap::new(),
            write_set_waits: HashMap::new(),
            write_set_hold_seq: 0,
            run_write_bases: HashMap::new(),
            review_sync_queue: HashMap::new(),
            review_sync_seq: 0,
            review_resync_halted: BTreeMap::new(),
            integrated_deps: BTreeSet::new(),
            wu_prepare_failures: HashMap::new(),
            credential_held: HashSet::new(),
            credential_busy: Arc::new(AtomicBool::new(false)),
            browser_ledger: LedgerWatch::from_process(),
            browser_prereq_rescan: true,
            cluster_waiting: HashSet::new(),
        }
    }

    // ---- write set の予約（ADR-0130 D3） ----

    /// write-set gate: `reservation` が、実行中の run（`running`。`RunSupervisor::run_keys`）の予約か、先に待ち始めて
    /// 先取りを持つ待機と強く重なれば、見送りを記録して `Hold` を返す。`gate_disabled` は試験の ablation
    /// （phase_effect_ab::write_set の off。本番の入口は常に `false` を渡す）で、`true` なら照合も予約もしない。
    pub(crate) fn admit_write_set<'a>(
        &mut self,
        key: &RunKey,
        reservation: Option<WriteReservation>,
        running_keys: impl IntoIterator<Item = &'a RunKey>,
        tick: u64,
        gate_disabled: bool,
    ) -> WriteSetAdmission {
        if gate_disabled {
            return WriteSetAdmission::Admit(None);
        }
        let Some(reservation) = reservation else {
            return WriteSetAdmission::Admit(None);
        };
        match self.write_set_blocker(key, &reservation, running_keys) {
            Some(blocker) => {
                self.note_write_set_hold(key, &blocker, &reservation, tick);
                WriteSetAdmission::Hold { blocker }
            }
            None => WriteSetAdmission::Admit(Some(reservation)),
        }
    }

    /// 実行中の run の予約のうち、`candidate` と強く重なる最初のもの（`RunKey` の順で決定的）。
    /// 実行中の run に重ならなくても、`candidate` より先に待ち始めて先取りを持つ待機と重なれば、その
    /// 待機を返す（後から来た重なる候補が、長く待った run を追い越し続けない）。
    fn write_set_blocker<'a>(
        &self,
        key: &RunKey,
        candidate: &WriteReservation,
        running_keys: impl IntoIterator<Item = &'a RunKey>,
    ) -> Option<RunKey> {
        let mut blockers: Vec<&RunKey> = running_keys
            .into_iter()
            .filter(|other| *other != key)
            .filter(|other| {
                self.write_reservations
                    .get(*other)
                    .is_some_and(|res| reservations_overlap(key.task, candidate, other.task, res))
            })
            .collect();
        blockers.sort_by(|a, b| {
            (a.task.to_string(), &a.work_unit).cmp(&(b.task.to_string(), &b.work_unit))
        });
        if let Some(k) = blockers.first() {
            return Some((*k).clone());
        }
        let own = self.write_set_waits.get(key).map(wait_order);
        self.write_set_waits
            .iter()
            .filter(|(other, w)| *other != key && w.claims())
            .map(|(other, w)| (wait_order(w), other, w))
            .filter(|(order, _, _)| own.as_ref().is_none_or(|own| order < own))
            .filter(|(_, other, w)| {
                reservations_overlap(key.task, candidate, other.task, &w.reservation)
            })
            .min_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, other, _)| other.clone())
    }

    /// 見送りを記録して log に残す（状態遷移・attempts には触らない）。
    fn note_write_set_hold(
        &mut self,
        key: &RunKey,
        blocker: &RunKey,
        reservation: &WriteReservation,
        tick: u64,
    ) {
        self.write_set_hold_seq += 1;
        let fresh = WriteSetWait {
            since_tick: tick,
            last_tick: tick,
            consecutive: 0,
            first_seq: self.write_set_hold_seq,
            reservation: reservation.clone(),
        };
        let wait = self
            .write_set_waits
            .entry(key.clone())
            .or_insert_with(|| fresh.clone());
        if wait.last_tick + 1 < tick {
            // 直前の tick に見送られていない（容量切れで照合されなかった tick は引き継ぎ済み）: 数え直す。
            *wait = fresh;
        }
        if wait.consecutive == 0 || wait.last_tick != tick {
            wait.consecutive += 1;
        }
        wait.last_tick = tick;
        wait.reservation = reservation.clone();
        let wait = wait.clone();
        tracing::info!(
            task_id = %key.task,
            work_unit = ?key.work_unit,
            blocked_by_task = %blocker.task,
            blocked_by_work_unit = ?blocker.work_unit,
            since_tick = wait.since_tick,
            consecutive = wait.consecutive,
            "write-set overlap; deferring the run (ADR-0130 D3)"
        );
    }

    /// run を起こした: 予約を置き、待機記録を消す。
    pub(crate) fn reserve_write_set(&mut self, key: RunKey, reservation: Option<WriteReservation>) {
        self.write_set_waits.remove(&key);
        match reservation {
            Some(r) => {
                self.write_reservations.insert(key, r);
            }
            None => {
                self.write_reservations.remove(&key);
            }
        }
    }

    /// tick の dispatch の前に、終わった run（`running` に無い鍵）の予約と古い待機記録を捨てる。
    pub(crate) fn prune_write_set_state<'a>(
        &mut self,
        running_keys: impl IntoIterator<Item = &'a RunKey>,
        tick: u64,
    ) {
        let running: HashSet<&RunKey> = running_keys.into_iter().collect();
        self.write_reservations.retain(|k, _| running.contains(k));
        self.write_set_waits.retain(|_, w| w.last_tick + 1 >= tick);
    }

    /// 容量が尽きて走査が途中で切れた tick の終わりに呼ぶ: この tick に照合されなかった待機を「待ち続けた」
    /// として引き継ぐ（連続回数を 1 つ進め、次 tick の `prune_write_set_state` で捨てられないようにする）。
    /// 容量切れのたびに数えが戻ると、先頭に移す公平性と先取りが永久に効かない
    /// （2026-10-03-write-set-no-starvation）。
    pub(crate) fn carry_write_set_waits(&mut self, tick: u64) {
        for wait in self.write_set_waits.values_mut() {
            if wait.last_tick + 1 == tick {
                wait.consecutive += 1;
                wait.last_tick = tick;
            }
        }
    }

    /// ADR-0130 D3 の公平性: 連続 `WRITE_SET_STARVATION_TICKS` 回以上見送られた ready task を、待機開始の
    /// 古い順に先頭へ移す（それ以外の候補の相対順は変えない）。
    pub(crate) fn order_write_set_starved_first(&self, candidates: Vec<Task>) -> Vec<Task> {
        let starved_since = |task: &Task| {
            self.write_set_waits
                .get(&RunKey {
                    task: task.id,
                    work_unit: None,
                })
                .into_iter()
                .chain(
                    self.write_set_waits
                        .iter()
                        .filter(|(k, _)| k.task == task.id && k.work_unit.is_some())
                        .map(|(_, w)| w),
                )
                .filter(|w| w.consecutive >= WRITE_SET_STARVATION_TICKS)
                .map(|w| w.since_tick)
                .min()
        };
        let (mut starved, rest): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .map(|t| (starved_since(&t), t))
            .partition(|(since, _)| since.is_some());
        starved.sort_by_key(|(since, _)| *since);
        starved.into_iter().chain(rest).map(|(_, t)| t).collect()
    }

    // ---- actual write-set の採取場所（ADR-0130 D2） ----

    /// dispatch の直前に固定した run の開始 HEAD を置く（空なら置かない）。
    pub(crate) fn put_run_write_bases(&mut self, run_id: &str, bases: Vec<RunWriteBase>) {
        if !bases.is_empty() {
            self.run_write_bases.insert(run_id.to_string(), bases);
        }
    }

    /// worker run の終了時に開始 HEAD を取り出す（daemon の再起動の後は `None`）。
    pub(crate) fn take_run_write_bases(&mut self, run_id: &str) -> Option<Vec<RunWriteBase>> {
        self.run_write_bases.remove(run_id)
    }

    // ---- review 前 sync の待ち行列（ADR-0130 D5）と再同期の上限（ADR 2026-10-27） ----

    /// `candidates` の待ちだけを残し、新しく待ち始めた task に次の待機開始順を振る（`candidates` の順）。
    pub(crate) fn enter_review_sync_queue(&mut self, candidates: &[Task]) {
        self.review_sync_queue
            .retain(|id, _| candidates.iter().any(|t| t.id == *id));
        for task in candidates {
            if !self.review_sync_queue.contains_key(&task.id) {
                let seq = self.review_sync_seq;
                self.review_sync_seq += 1;
                self.review_sync_queue.insert(
                    task.id,
                    ReviewSyncWait {
                        seq,
                        skipped_ticks: 0,
                    },
                );
            }
        }
    }

    /// task の待ち（行列に無ければ最後尾の扱い）。
    pub(crate) fn review_sync_wait(&self, task_id: TaskId) -> ReviewSyncWait {
        self.review_sync_queue
            .get(&task_id)
            .copied()
            .unwrap_or(ReviewSyncWait {
                seq: u64::MAX,
                skipped_ticks: 0,
            })
    }

    /// 1 回の申し出の結果: review を始めたら行列から外す。人の承認待ち（`awaiting_human`）でない見送りは
    /// FIFO の救済に数える。
    pub(crate) fn note_review_sync_offer(
        &mut self,
        task_id: TaskId,
        started: bool,
        awaiting_human: bool,
    ) {
        if started {
            self.review_sync_queue.remove(&task_id);
            return;
        }
        if awaiting_human {
            return;
        }
        if let Some(wait) = self.review_sync_queue.get_mut(&task_id) {
            wait.skipped_ticks = wait.skipped_ticks.saturating_add(1);
        }
    }

    /// 再同期の上限で止めた review の一覧（snapshot の `review_resync_halted`）。
    pub(crate) fn review_resync_halted(&self) -> &BTreeMap<TaskId, SnapshotReviewResyncHalted> {
        &self.review_resync_halted
    }

    /// 止めた review の 1 件を記録する（既にあれば最初の記録を残す）。
    pub(crate) fn note_review_resync_halt(&mut self, halted: SnapshotReviewResyncHalted) {
        self.review_resync_halted
            .entry(halted.task_id)
            .or_insert(halted);
    }

    /// 上限の内側に戻った・再開した review の記録を消す。
    pub(crate) fn clear_review_resync_halt(&mut self, task_id: TaskId) {
        self.review_resync_halted.remove(&task_id);
    }

    // ---- root の依存先の統合（ADR 2026-10-17） ----

    /// root 間の依存先の成果が既定 branch に入るまで待つか（`task_ops::dep_integration::integration_wait`。
    /// 祖先と分かった依存先を覚える）。
    pub(crate) fn integration_wait(
        &mut self,
        store: &dyn TaskStore,
        task: &Task,
    ) -> Result<Option<task_core::TaskWait>, StoreError> {
        task_ops::dep_integration::integration_wait(store, task, &mut self.integrated_deps)
    }

    /// 既定 branch の祖先と分かった root の依存先。
    pub(crate) fn integrated_deps(&self) -> &BTreeSet<TaskId> {
        &self.integrated_deps
    }

    // ---- WU の worktree の用意の失敗（ADR-0074 F5-fix7） ----

    /// `now` がまだ一時的な失敗の後の待ちの中なら、その記録。
    pub(crate) fn wu_prepare_backoff(
        &self,
        wu_id: &str,
        now: OffsetDateTime,
    ) -> Option<WuPrepareFailures> {
        self.wu_prepare_failures
            .get(wu_id)
            .copied()
            .filter(|f| now < f.retry_at)
    }

    /// 一時的な失敗を 1 回数える。`max_attempts` 回目に届かなければ `now + backoff(回数)` まで待つ記録を置いて返し、
    /// 届いたら記録を消して `None`（呼び元が WU を blocked にする）。
    pub(crate) fn retry_wu_prepare(
        &mut self,
        wu_id: &str,
        now: OffsetDateTime,
        max_attempts: u32,
        backoff: impl Fn(u32) -> time::Duration,
    ) -> Option<WuPrepareFailures> {
        let count = self
            .wu_prepare_failures
            .get(wu_id)
            .map_or(0, |f| f.count)
            .saturating_add(1);
        if count < max_attempts {
            let failures = WuPrepareFailures {
                count,
                retry_at: now + backoff(count),
            };
            self.wu_prepare_failures.insert(wu_id.to_string(), failures);
            return Some(failures);
        }
        self.wu_prepare_failures.remove(wu_id);
        None
    }

    /// 用意できた・blocked にした WU の記録を消す。
    pub(crate) fn clear_wu_prepare_failures(&mut self, wu_id: &str) {
        self.wu_prepare_failures.remove(wu_id);
    }

    // ---- credential の待ち（ADR 2026-10-15 D4） ----

    /// 計画の無い task を credential 待ちとして覚える。既に覚えていれば `false`（同じ wait を書き直さない）。
    pub(crate) fn hold_credential(&mut self, task_id: TaskId) -> bool {
        self.credential_held.insert(task_id)
    }

    /// credential が登録済みになった task の印を消す。
    pub(crate) fn release_credential_hold(&mut self, task_id: TaskId) {
        self.credential_held.remove(&task_id);
    }

    /// credential 待ちの検証・再開の thread を 1 つ始めてよいか。始めてよければ印を立てて返す
    /// （thread の終わりに `false` に戻す）。既に走っていれば `None`。
    pub(crate) fn begin_credential_processing(&self) -> Option<Arc<AtomicBool>> {
        if self.credential_busy.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(Arc::clone(&self.credential_busy))
    }

    // ---- browser の適合台帳（ADR 2026-10-08 D2） ----

    /// 台帳の見張りを差し替え、止めた task を次の tick で見直す。
    pub(crate) fn set_browser_ledger_watch(&mut self, watch: LedgerWatch) {
        self.browser_ledger = watch;
        self.browser_prereq_rescan = true;
    }

    /// 台帳の file が変わっていれば読み直し、止めた task の見直しを予約する。
    pub(crate) fn refresh_browser_ledger(&mut self) {
        if self.browser_ledger.refresh() {
            self.browser_prereq_rescan = true;
        }
    }

    /// 台帳の見張り（読み取り専用）。
    pub(crate) fn browser_ledger(&self) -> &LedgerWatch {
        &self.browser_ledger
    }

    /// 見直しの予約を取り出す（取り出したら消す）。
    pub(crate) fn take_browser_prereq_rescan(&mut self) -> bool {
        std::mem::take(&mut self.browser_prereq_rescan)
    }

    // ---- クラスタ待ち（ADR-0018 D2） ----

    /// tick の dispatch の頭で、前の tick のクラスタ待ちを忘れる。
    pub(crate) fn clear_cluster_waiting(&mut self) {
        self.cluster_waiting.clear();
    }

    /// この tick にクラスタの接続待ちで見送った task を覚える。
    pub(crate) fn note_cluster_waiting(&mut self, task_id: TaskId) {
        self.cluster_waiting.insert(task_id);
    }

    /// この tick にクラスタの接続待ちで見送ったか。
    pub(crate) fn is_cluster_waiting(&self, task_id: TaskId) -> bool {
        self.cluster_waiting.contains(&task_id)
    }
}

#[cfg(test)]
impl WriteReservation {
    pub(crate) fn for_test(repos: &[&str], paths: &[&str], inherited_from_task: bool) -> Self {
        WriteReservation {
            repos: repos.iter().map(|s| s.to_string()).collect(),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            inherited_from_task,
        }
    }
}

#[cfg(test)]
impl ExecutionCoordination {
    pub(crate) fn write_set_waits(&self) -> &HashMap<RunKey, WriteSetWait> {
        &self.write_set_waits
    }

    pub(crate) fn has_run_write_bases(&self) -> bool {
        !self.run_write_bases.is_empty()
    }

    pub(crate) fn review_sync_queue(&self) -> &HashMap<TaskId, ReviewSyncWait> {
        &self.review_sync_queue
    }

    pub(crate) fn wu_prepare_failures(&self, wu_id: &str) -> Option<WuPrepareFailures> {
        self.wu_prepare_failures.get(wu_id).copied()
    }

    /// 待ちの期限だけを過ぎたことにする（眠らずに次の試行を許す）。
    pub(crate) fn expire_wu_prepare_backoff(&mut self, wu_id: &str) {
        if let Some(f) = self.wu_prepare_failures.get_mut(wu_id) {
            f.retry_at = OffsetDateTime::UNIX_EPOCH;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(task: TaskId) -> RunKey {
        RunKey {
            task,
            work_unit: None,
        }
    }

    /// 不変条件（D1「run の集合と lease の照合」）: 予約が blocker になるのは、呼び元が渡した実行中の run の鍵に
    /// ある run だけで、刈り込みは渡された鍵に無い予約を捨てる（owner は run の集合を持たない）。
    /// 既存の試験で検出できない理由: dispatcher 経由の試験は実行中の run の表と予約がいつも同じ tick で揃うので、
    /// 渡された鍵を無視して自分の予約の表だけを見る実装にしても通る。
    #[test]
    fn write_set_blockers_come_from_the_running_keys_passed_in() {
        let (a, b) = (TaskId::new(), TaskId::new());
        let res = WriteReservation::for_test(&["repo:r"], &["src/"], false);
        let mut c = ExecutionCoordination::new();
        c.reserve_write_set(key(a), Some(res.clone()));
        // a の予約はあるが、a は実行中の run に無い: b を止めない。
        assert_eq!(
            c.admit_write_set(&key(b), Some(res.clone()), [].iter(), 1, false),
            WriteSetAdmission::Admit(Some(res.clone()))
        );
        // a が実行中なら b を止め、見送りを記録する。
        let running = [key(a)];
        assert_eq!(
            c.admit_write_set(&key(b), Some(res.clone()), running.iter(), 1, false),
            WriteSetAdmission::Hold { blocker: key(a) }
        );
        assert_eq!(c.write_set_waits()[&key(b)].consecutive, 1);
        // ablation で切ると照合も予約もしない。
        assert_eq!(
            c.admit_write_set(&key(b), Some(res.clone()), running.iter(), 1, true),
            WriteSetAdmission::Admit(None)
        );
        // 実行中の run に無い予約は刈り込みで消える。
        c.prune_write_set_state([].iter(), 1);
        assert_eq!(
            c.admit_write_set(&key(b), Some(res.clone()), running.iter(), 2, false),
            WriteSetAdmission::Admit(Some(res))
        );
    }

    /// 不変条件: WU の用意の一時的な失敗は上限の手前まで待ちの記録を置き、上限で記録を消して `None` を返す。
    /// 待ちの中かどうかは `retry_at` と時刻だけで決まる。
    /// 既存の試験で検出できない理由: dispatcher の試験は実時計の `now` で 1 回目の待ちしか見ず、上限の境目で記録が
    /// 消えることを owner の method 単独では確かめていない。
    #[test]
    fn wu_prepare_failures_count_up_to_the_limit() {
        let mut c = ExecutionCoordination::new();
        let now = OffsetDateTime::UNIX_EPOCH;
        let backoff = |n: u32| time::Duration::seconds(i64::from(n));
        for n in 1..3 {
            let f = c.retry_wu_prepare("wu", now, 3, backoff).expect("retry");
            assert_eq!(f.count, n);
            assert!(c.wu_prepare_backoff("wu", now).is_some());
            assert!(c.wu_prepare_backoff("wu", f.retry_at).is_none());
        }
        assert!(c.retry_wu_prepare("wu", now, 3, backoff).is_none());
        assert!(c.wu_prepare_failures("wu").is_none());
    }

    /// 不変条件: ADR 2026-11-11 D2 の 1〜3・7（field が private、dispatcher の本体を受けない・`dispatcher` module を
    /// 使わない、field は D1 の表の own-coord-a の 14 件で run の集合を持たない。`#[cfg(test)]` の field は
    /// `dispatcher/tests/structure_guard.rs` の `OWNER_STRUCTS` が数える）、`crate::review` を使わない（review は
    /// dispatcher に依存し、使うと deps.cycle の循環が広がる）。既存の試験は振る舞いだけを見るので、field を `pub` に
    /// 戻しても、method が dispatcher の本体を引数に受けても、run の表を field に足しても通る。
    #[test]
    fn structure_guard_execution_coordination() {
        let src = include_str!("execution_coordination.rs");
        let src = &src[..src
            .find("#[cfg(test)]\nimpl WriteReservation")
            .expect("the test-only impl is in the source")];
        let start = src
            .find("pub(crate) struct ExecutionCoordination {")
            .expect("ExecutionCoordination is defined");
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
                "an ExecutionCoordination field is not private: {line}"
            );
            if let Some((name, _)) = t.split_once(": ") {
                fields.push(name.to_string());
            }
        }
        fields.sort();
        assert_eq!(
            fields,
            [
                "browser_ledger",
                "browser_prereq_rescan",
                "cluster_waiting",
                "credential_busy",
                "credential_held",
                "integrated_deps",
                "review_resync_halted",
                "review_sync_queue",
                "review_sync_seq",
                "run_write_bases",
                "write_reservations",
                "write_set_hold_seq",
                "write_set_waits",
                "wu_prepare_failures",
            ],
            "ExecutionCoordination owns the 14 own-coord-a fields of ADR D1"
        );
        let owner = ["Dis", "patcher"].concat();
        for bad in [
            format!("&mut {owner}"),
            format!("&{owner}"),
            format!("impl {owner}"),
            format!("crate::{}", owner.to_lowercase()),
            format!("super::{}", owner.to_lowercase()),
            "crate::review".to_string(),
            "RunEntry".to_string(),
            "RunSupervisor<".to_string(),
        ] {
            assert!(
                !src.contains(&bad),
                "execution_coordination.rs must not contain {bad:?}"
            );
        }
    }
}
