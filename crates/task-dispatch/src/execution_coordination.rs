//! ADR 2026-11-11 dispatcher-state-ownership D1: task・WU の実行の調整の状態の owner（[`ExecutionCoordination`]）。
//!
//! unit own-coord-a が移したもの（D1 の表で unit 列が own-coord-a の行）: write set の予約と見送りの記録（ADR-0130 D3）、
//! actual write-set の採取場所（ADR-0130 D2）、review 前 sync の待ち行列（ADR-0130 D5）と再同期の上限で止めた review
//! （ADR 2026-10-27）、既定 branch に入った root の依存先（ADR 2026-10-17）、WU の worktree の用意の失敗の回数
//! （ADR-0074 F5-fix7）、credential 待ちの検証・再開の処理中の印（ADR 2026-10-15 D4）、
//! browser の適合台帳の見張り（ADR 2026-10-08 D2）、クラスタ待ちの task（ADR-0018 D2）。
//! credential の待ちそのものはメモリに写さない。正本は store の `credential_waits` で、gate が毎回読む
//! （ADR 2026-11-14-atomic-task-credential-verified D1 と付記 2026-11-14 cred-cache）。
//!
//! unit own-coord-b が移したもの（D1 の表で unit 列が own-coord-b の行）: task・WU の待ち（見送りの理由・人の承認待ち・
//! 子待ち・review の持ち越し）、WU の check と段の統合の process、完了の channel、tick の数と間隔、新しい仕事を
//! 受けるか、全体の並列度、停滞の見張り（ADR-0079 D10・ADR 2026-10-23 D5）。review の持ち越しの値 `S` と完了の
//! 値 `C` は dispatcher の型で、この module は型引数で受ける（review の module と `dispatcher` module を使わない）。
//! lease の回収と abort で待ちを消すときは、`RunSupervisor` が手放した run の鍵（`TaskId`）を method の引数で受ける。
//!
//! WU の check と段の統合の process（`checking`・`integrating`）は下位 owner [`ProcessTable`]、停滞の見張り
//! （`stall_watch`・`stalled_tasks`・`liveness_checked_at`）は下位 owner [`LivenessWatch`] に置く。どちらも
//! `ExecutionCoordination` の private な field としてだけ作り、呼び元は `processes()`・`liveness()`（と `_mut`）から
//! method を呼ぶ。子待ち・review の持ち越しと合わせて見る `holds_task`・`abort_in_hand` は `ExecutionCoordination` が持つ。
//! write set の blocker の判断は予約と待機の表を引数で受ける純粋関数 `write_set_blocker`、見送りの記録は owner の method。
//!
//! run の集合は持たない（D1「run の集合と lease の照合」。正本は `crate::run_supervisor::RunSupervisor`）。write set の
//! blocker の絞り込みと予約の刈り込みは、呼び元が `RunSupervisor::run_keys` で渡す実行中の run の鍵だけを見る。
//! store・時計・試験の ablation は引数で受ける。この module は `dispatcher` module を使わない（D2 の 3）。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use task_core::plan::PlanOutput;
use task_core::{AccountAdapter, SqliteStore, StoreError, Task, TaskId, TaskWait};
use task_ops::daemon::{SnapshotReviewResyncHalted, SnapshotStalledTask};
use task_worker::Terminal;
use task_worker::browser_ledger::LedgerWatch;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::policy::ProviderId;
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

/// 実行中の run の予約のうち、`candidate` と強く重なる最初のもの（`RunKey` の順で決定的）。
/// 実行中の run に重ならなくても、`candidate` より先に待ち始めて先取りを持つ待機と重なれば、その
/// 待機を返す（後から来た重なる候補が、長く待った run を追い越し続けない）。予約と待機の表を引数で受ける純粋関数
/// （判断。見送りの記録は owner の `note_write_set_hold`）。
fn write_set_blocker<'a>(
    write_reservations: &HashMap<RunKey, WriteReservation>,
    write_set_waits: &HashMap<RunKey, WriteSetWait>,
    key: &RunKey,
    candidate: &WriteReservation,
    running_keys: impl IntoIterator<Item = &'a RunKey>,
) -> Option<RunKey> {
    let mut blockers: Vec<&RunKey> = running_keys
        .into_iter()
        .filter(|other| *other != key)
        .filter(|other| {
            write_reservations
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
    let own = write_set_waits.get(key).map(wait_order);
    write_set_waits
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

/// ADR-0074 D1.4（Phase F2b）: 走っている工程の統合。
pub(crate) struct IntegrationEntry {
    pub(crate) work_unit_id: String,
    pub(crate) handle: JoinHandle<()>,
}

/// ADR-0072 D14/D6・E4 (g) / Phase F5-fix2: WU の `checks` を走らせている run（`spawn_work_unit_checks` が spawn
/// した検査。run 自身は run の表から既に外れている）。`in_flight` に数え（draining のインスタンスが検査の途中で
/// exit して完了を失わないため）、lease の照合では「生きている run」とみなす。
///
/// ADR-0040 付記（2026-10-04、WU 検査の引き継ぎ）D1: draining になったら検査を止めて手を離す
/// （`hand_off_checks_in_hand`）ので、WU・検査の数・run の終端（`Done`）・quota の持ち主も持つ。
pub(crate) struct CheckingEntry {
    pub(crate) task_id: TaskId,
    pub(crate) handle: JoinHandle<()>,
    pub(crate) work_unit_id: String,
    pub(crate) key: String,
    pub(crate) checks: usize,
    pub(crate) terminal: Terminal,
    pub(crate) account: Option<String>,
    pub(crate) account_adapter: Option<AccountAdapter>,
    pub(crate) provider: ProviderId,
}

/// ADR-0016 M5: 子待ちの親について覚えておくもの。
pub(crate) struct AwaitingChildren {
    pub(crate) run_id: String,
    pub(crate) plan: Option<PlanOutput>,
}

/// drain・持ち主の喪失で手放す、手元の WU の check と段の統合（[`ExecutionCoordination::abort_in_hand`]）。
pub(crate) struct InHand {
    pub(crate) checking: Vec<(String, CheckingEntry)>,
    pub(crate) integrating: Vec<(TaskId, IntegrationEntry)>,
}

/// task・WU の実行の調整の状態（ADR 2026-11-11 dispatcher-state-ownership D1）。
/// field は private で、呼び元は method を通して読み書きする（D2）。`S` は review の持ち越し（`ReviewSubject`）、
/// `C` は完了の channel の値（`Completion`）で、どちらも dispatcher の型（この module は型引数で受ける）。
pub(crate) struct ExecutionCoordination<S, C> {
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
    /// ADR 2026-10-15-mlsys-min-decision-assisted D4: credential 待ちの検証・再開の thread が走っているか（同時に 1 つまで）。
    credential_busy: Arc<AtomicBool>,
    /// ADR 2026-10-08-browser-prod-enablement D2: 適合台帳の状態の cache（file が変わったときだけ計算し直す）。
    browser_ledger: LedgerWatch,
    /// 台帳が変わった（または差し替えた）ので、`browser_prerequisite` で止めた task を次の tick で見直す。
    browser_prereq_rescan: bool,
    /// ADR-0018 D2（監査 4-1）: この tick でクラスタの多重接続が無い／cooldown 中のため待っている ready タスク。「人のログイン待ち」で
    /// 経路なし（`unroutable`）とは別物。`is_idle` の待ち対象から外すだけで、スナップショットには出さない（受信箱の (d) が知らせる）。
    cluster_waiting: HashSet<TaskId>,
    /// レビューを開始できなかった（`Reviewer` run の枠が無い）タスクの `done` 内容。次 tick で使う。
    pending_subjects: HashMap<TaskId, S>,
    /// ADR-0079 D10・ADR 2026-10-23 D5: 停滞の見張り（下位 owner [`LivenessWatch`]。`stall_watch`・`stalled_tasks`・
    /// `liveness_checked_at`）。
    liveness: LivenessWatch,
    /// ADR 2026-10-11-task-wait-reasons D4: この tick で `dispatch_one` が見送った ready タスクの理由（メモリにしか無い M1〜M6・M9。`dispatch_ready` の先頭で空にし、同じ task の 2 件目は上書きしない）。snapshot の `task_waits` に載る。
    dispatch_waits: HashMap<TaskId, TaskWait>,
    /// 人間の承認待ちで延期中の reviewing タスク（`is_idle` 判定用。ADR-0010 D8）。
    awaiting_human: HashSet<TaskId>,
    /// ADR-0016 D2 / M5: レビューは全 pass だが、委譲した子が終端になるのを待っている reviewing タスク。値はその run の id と、Plan kind なら検証済みの plan（子が終わってから `complete_plan` する）。
    awaiting_children: HashMap<TaskId, AwaitingChildren>,
    /// 手元の WU の check と段の統合の process（下位 owner [`ProcessTable`]。`checking`・`integrating`）。
    processes: ProcessTable,
    /// worker・review・WU の check・統合・評価・設計 review の完了の channel（spawn した task が送り、`drain_completions` が受ける）。
    tx: mpsc::UnboundedSender<C>,
    rx: mpsc::UnboundedReceiver<C>,
    /// tick の回数（スナップショット用）。
    ticks: u64,
    /// ADR-0078 D5: 直前の tick の開始時刻と、その前の tick との間隔（`cluster ssh master lost` の `last_tick_gap_ms`。tick の停止による切断〈ADR-0078 C3〉を見分ける）。
    last_tick_started: Option<Instant>,
    last_tick_gap_ms: u64,
    /// ADR-0040 D4（Phase 47）: 新しい仕事を始めてよいか。`false`（= draining）のときは `dispatch_ready` も `recover_reviews` も動かさず、手元の run とレビューの面倒だけ見続ける。
    accepting_new_work: bool,
    /// 実効の全体並列度（ADR 2026-10-14-local-machine-settings D3・D4: store の `local_settings.max_concurrency` があれば store、無ければ `config.config_max_concurrency`）。tick の頭と admin の `LocalSettingsChanged` で解き直す。走行中の run は止めない。
    max_concurrency: usize,
}

impl<S, C> ExecutionCoordination<S, C> {
    pub(crate) fn new(max_concurrency: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
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
            credential_busy: Arc::new(AtomicBool::new(false)),
            browser_ledger: LedgerWatch::from_process(),
            browser_prereq_rescan: true,
            cluster_waiting: HashSet::new(),
            pending_subjects: HashMap::new(),
            liveness: LivenessWatch::default(),
            dispatch_waits: HashMap::new(),
            awaiting_human: HashSet::new(),
            awaiting_children: HashMap::new(),
            processes: ProcessTable::default(),
            tx,
            rx,
            ticks: 0,
            last_tick_started: None,
            last_tick_gap_ms: 0,
            accepting_new_work: true,
            max_concurrency,
        }
    }

    // ---- write set の予約（ADR-0130 D3） ----

    /// write-set gate: `reservation` が、実行中の run（`running`。`RunSupervisor::run_keys`）の予約か、先に待ち始めて
    /// 先取りを持つ待機と強く重なれば、見送りを記録して `Hold` を返す。
    pub(crate) fn admit_write_set<'a>(
        &mut self,
        key: &RunKey,
        reservation: Option<WriteReservation>,
        running_keys: impl IntoIterator<Item = &'a RunKey>,
        tick: u64,
    ) -> WriteSetAdmission {
        let Some(reservation) = reservation else {
            return WriteSetAdmission::Admit(None);
        };
        match write_set_blocker(
            &self.write_reservations,
            &self.write_set_waits,
            key,
            &reservation,
            running_keys,
        ) {
            Some(blocker) => {
                self.note_write_set_hold(key, &blocker, &reservation, tick);
                WriteSetAdmission::Hold { blocker }
            }
            None => WriteSetAdmission::Admit(Some(reservation)),
        }
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
    /// 返す値は stale 優先で並べ替えるか（候補が 2 件以上のとき）。
    pub(crate) fn enter_review_sync_queue(&mut self, candidates: &[Task]) -> bool {
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
        candidates.len() >= 2
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
    pub(crate) fn note_review_sync_offer(&mut self, task_id: TaskId, started: bool) {
        if started {
            self.review_sync_queue.remove(&task_id);
            return;
        }
        if self.awaiting_human.contains(&task_id) {
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
        store: &SqliteStore,
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
    #[cfg(test)]
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

    /// tick の dispatch の頭（`dispatch_ready`）で、前の tick のクラスタ待ちと見送りの理由（ADR 2026-10-11 D4）を忘れる。
    pub(crate) fn begin_dispatch_round(&mut self) {
        self.cluster_waiting.clear();
        self.dispatch_waits.clear();
    }

    /// この tick にクラスタの接続待ちで見送った task を覚える。
    pub(crate) fn note_cluster_waiting(&mut self, task_id: TaskId) {
        self.cluster_waiting.insert(task_id);
    }

    /// この tick にクラスタの接続待ちで見送ったか。
    pub(crate) fn is_cluster_waiting(&self, task_id: TaskId) -> bool {
        self.cluster_waiting.contains(&task_id)
    }

    // ---- 完了の channel・tick・新しい仕事・並列度 ----

    /// spawn する task に渡す完了の送り口。
    pub(crate) fn completion_sender(&self) -> mpsc::UnboundedSender<C> {
        self.tx.clone()
    }

    /// 届いている完了を 1 件取り出す（無ければ `None`）。
    pub(crate) fn try_recv_completion(&mut self) -> Option<C> {
        self.rx.try_recv().ok()
    }

    /// tick を 1 つ数える（tick の始め）。
    pub(crate) fn count_tick(&mut self) {
        self.ticks += 1;
    }

    /// tick の回数。
    pub(crate) fn ticks(&self) -> u64 {
        self.ticks
    }

    /// tick の開始時刻を記録し、前の tick との間隔（ms）を測り直す（ADR-0078 D5）。
    pub(crate) fn note_tick_started(&mut self, started: Instant) {
        if let Some(prev) = self.last_tick_started.replace(started) {
            self.last_tick_gap_ms =
                u64::try_from(started.duration_since(prev).as_millis()).unwrap_or(u64::MAX);
        }
    }

    /// 直前の tick とその前の tick の間隔（ms）。
    pub(crate) fn last_tick_gap_ms(&self) -> u64 {
        self.last_tick_gap_ms
    }

    /// 新しい仕事を始めてよいか（ADR-0040 D4）。
    pub(crate) fn accepting_new_work(&self) -> bool {
        self.accepting_new_work
    }

    pub(crate) fn set_accepting_new_work(&mut self, accepting: bool) {
        self.accepting_new_work = accepting;
    }

    /// 実効の全体並列度。
    pub(crate) fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    pub(crate) fn set_max_concurrency(&mut self, max_concurrency: usize) {
        self.max_concurrency = max_concurrency;
    }

    // ---- この tick の見送りの理由（ADR 2026-10-11-task-wait-reasons D4） ----

    /// 見送りの理由を記録する（同じ task の 2 件目は上書きしない）。
    pub(crate) fn note_dispatch_wait(&mut self, task_id: TaskId, wait: TaskWait) {
        self.dispatch_waits.entry(task_id).or_insert(wait);
    }

    pub(crate) fn dispatch_waits(&self) -> &HashMap<TaskId, TaskWait> {
        &self.dispatch_waits
    }

    // ---- review の待ち（持ち越し・人の承認待ち・子待ち） ----

    /// review を始められなかった task の `done` 内容を次の tick に持ち越す。
    pub(crate) fn defer_review(&mut self, task_id: TaskId, subject: S) {
        self.pending_subjects.insert(task_id, subject);
    }

    /// 持ち越した `done` 内容を取り出す。
    pub(crate) fn take_deferred_review(&mut self, task_id: TaskId) -> Option<S> {
        self.pending_subjects.remove(&task_id)
    }

    /// lease の回収・abort で `RunSupervisor` が review を手放した task（`take_review` が返した鍵）の持ち越しを消す
    /// （ADR 2026-11-11 D3 の reclaim_expired_leases の行）。
    pub(crate) fn release_reclaimed_review(&mut self, task_id: TaskId) {
        self.pending_subjects.remove(&task_id);
    }

    pub(crate) fn set_awaiting_human(&mut self, task_id: TaskId, awaiting: bool) {
        if awaiting {
            self.awaiting_human.insert(task_id);
        } else {
            self.awaiting_human.remove(&task_id);
        }
    }

    pub(crate) fn is_awaiting_human(&self, task_id: TaskId) -> bool {
        self.awaiting_human.contains(&task_id)
    }

    /// 承認待ちの記録を `keep` が真の task だけに保つ（まだ reviewing の task。ADR-0013 D4）。
    pub(crate) fn retain_awaiting_human(&mut self, keep: impl Fn(TaskId) -> bool) {
        self.awaiting_human.retain(|id| keep(*id));
    }

    pub(crate) fn awaiting_human(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.awaiting_human.iter().copied()
    }

    pub(crate) fn await_children(&mut self, task_id: TaskId, waiting: AwaitingChildren) {
        self.awaiting_children.insert(task_id, waiting);
    }

    pub(crate) fn is_awaiting_children(&self, task_id: TaskId) -> bool {
        self.awaiting_children.contains_key(&task_id)
    }

    pub(crate) fn awaiting_children_tasks(&self) -> Vec<TaskId> {
        self.awaiting_children.keys().copied().collect()
    }

    pub(crate) fn take_awaiting_children(&mut self, task_id: TaskId) -> Option<AwaitingChildren> {
        self.awaiting_children.remove(&task_id)
    }

    // ---- WU の check と段の統合の process（下位 owner `ProcessTable`） ----

    /// 手元の WU の check と段の統合の process（読み取り）。
    pub(crate) fn processes(&self) -> &ProcessTable {
        &self.processes
    }

    /// 手元の WU の check と段の統合の process（始める・取り出す）。
    pub(crate) fn processes_mut(&mut self) -> &mut ProcessTable {
        &mut self.processes
    }

    /// その task の check・統合・子待ちを手元に持っているか（`crate::orphan` の定義の 1. のうちこの owner の分）。
    pub(crate) fn holds_task(&self, task_id: TaskId) -> bool {
        self.processes
            .checking
            .values()
            .any(|e| e.task_id == task_id)
            || self.processes.integrating.contains_key(&task_id)
            || self.awaiting_children.contains_key(&task_id)
    }

    /// drain・持ち主の喪失で、手元の check と統合を全部手放し、review の持ち越しを消す（止めるのは呼び元）。
    pub(crate) fn abort_in_hand(&mut self) -> InHand {
        self.pending_subjects.clear();
        InHand {
            checking: self.processes.checking.drain().collect(),
            integrating: self.processes.integrating.drain().collect(),
        }
    }

    // ---- 停滞の見張り（下位 owner `LivenessWatch`。ADR-0079 D10・ADR 2026-10-23 D5） ----

    /// 停滞の見張り（読み取り。snapshot の `stalled_tasks`）。
    pub(crate) fn liveness(&self) -> &LivenessWatch {
        &self.liveness
    }

    /// 停滞の見張り（生存確認の 1 回分の更新）。
    pub(crate) fn liveness_mut(&mut self) -> &mut LivenessWatch {
        &mut self.liveness
    }
}

/// [`ExecutionCoordination`] の下位 owner: 手元の WU の check と段の統合の process（ADR 2026-11-11 D1 の表の
/// `checking`・`integrating`。owner は `ExecutionCoordination` で、この型はその field としてだけ作る）。
/// field は private で、`holds_task`・`abort_in_hand` のように子待ち・review の持ち越しと合わせて見る処理は
/// `ExecutionCoordination` の method が持つ。
#[derive(Default)]
pub(crate) struct ProcessTable {
    /// ADR-0074 D1.4（Phase F2b）: 工程の統合を走らせている Task（spawn した git 操作と検査）。再起動の照合（D1.7）は「running の統合 WU で、ここに無いもの」を pending に戻す。
    integrating: HashMap<TaskId, IntegrationEntry>,
    /// Phase F5-fix2: WU の `checks` を走らせている run（キーは run id）。
    checking: HashMap<String, CheckingEntry>,
}

impl ProcessTable {
    pub(crate) fn start_integration(&mut self, task_id: TaskId, entry: IntegrationEntry) {
        self.integrating.insert(task_id, entry);
    }

    pub(crate) fn is_integrating(&self, task_id: TaskId) -> bool {
        self.integrating.contains_key(&task_id)
    }

    pub(crate) fn integration_work_unit(&self, task_id: TaskId) -> Option<&str> {
        self.integrating
            .get(&task_id)
            .map(|e| e.work_unit_id.as_str())
    }

    pub(crate) fn integrating_tasks(&self) -> Vec<TaskId> {
        self.integrating.keys().copied().collect()
    }

    pub(crate) fn take_integration(&mut self, task_id: TaskId) -> Option<IntegrationEntry> {
        self.integrating.remove(&task_id)
    }

    pub(crate) fn start_checks(&mut self, run_id: String, entry: CheckingEntry) {
        self.checking.insert(run_id, entry);
    }

    pub(crate) fn is_checking(&self, run_id: &str) -> bool {
        self.checking.contains_key(run_id)
    }

    pub(crate) fn has_checks(&self) -> bool {
        !self.checking.is_empty()
    }

    pub(crate) fn checking_entry(&self, run_id: &str) -> Option<&CheckingEntry> {
        self.checking.get(run_id)
    }

    pub(crate) fn take_checks(&mut self, run_id: &str) -> Option<CheckingEntry> {
        self.checking.remove(run_id)
    }

    /// check を走らせている run の id と task（順は不定）。
    pub(crate) fn checking_runs(&self) -> Vec<(String, TaskId)> {
        self.checking
            .iter()
            .map(|(run_id, e)| (run_id.clone(), e.task_id))
            .collect()
    }

    /// その task の check を走らせている run の id（あれば 1 つ）。
    pub(crate) fn checking_run_of(&self, task_id: TaskId) -> Option<String> {
        self.checking
            .iter()
            .find(|(_, e)| e.task_id == task_id)
            .map(|(run_id, _)| run_id.clone())
    }

    /// check を走らせている task（重複あり）。
    pub(crate) fn checking_tasks(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.checking.values().map(|c| c.task_id)
    }

    /// 走っている check と統合の数（`in_flight` に数える）。
    pub(crate) fn in_flight(&self) -> usize {
        self.checking.len() + self.integrating.len()
    }
}

/// [`ExecutionCoordination`] の下位 owner: 停滞の見張り（ADR-0079 D10・ADR 2026-10-23 D5。ADR 2026-11-11 D1 の表の
/// `stall_watch`・`stalled_tasks`・`liveness_checked_at`。owner は `ExecutionCoordination` で、この型はその field として
/// だけ作る）。プロセス内メモリのみ（再起動で数え直す = 安全側）。
#[derive(Default)]
pub(crate) struct LivenessWatch {
    /// ADR-0079 D10（Phase R3b）: 理由なく止まっている（`LivenessClass::Unexplained`）と最初に見た木の節点と、その時の節点の最後の event の seq（seq が変われば数え直す）。プロセス内メモリのみ（再起動で数え直す = 安全側）。
    stall_watch: HashMap<TaskId, (u64, OffsetDateTime)>,
    /// ADR 2026-10-23-stage-order-and-plan-stall D5: `StallDetected` を出した後まだ何も起きていない task（snapshot の `stalled_tasks`。受信箱の attention の元）。生存確認ごとに作り直す。プロセス内メモリのみ。
    stalled_tasks: BTreeMap<TaskId, SnapshotStalledTask>,
    /// ADR-0079 D10（Phase R3b）: 最後に生存確認をした時刻（`LIVENESS_CHECK_INTERVAL_SECS` ごと）。
    liveness_checked_at: Option<OffsetDateTime>,
}

impl LivenessWatch {
    /// 生存確認の時刻か（前回から `interval_secs` 以上たったか）。時刻なら `now` を記録して `true`。
    pub(crate) fn liveness_due(&mut self, now: OffsetDateTime, interval_secs: i64) -> bool {
        if let Some(last) = self.liveness_checked_at
            && (now - last).whole_seconds() < interval_secs
        {
            return false;
        }
        self.liveness_checked_at = Some(now);
        true
    }

    /// 見張る節点が無い: 見張りと停滞の一覧を空にする。
    pub(crate) fn clear_stall_watch(&mut self) {
        self.stall_watch.clear();
        self.stalled_tasks.clear();
    }

    /// 既に知らせた停滞（前の生存確認の一覧）。
    pub(crate) fn stalled_task(&self, task_id: TaskId) -> Option<&SnapshotStalledTask> {
        self.stalled_tasks.get(&task_id)
    }

    pub(crate) fn stalled_tasks(&self) -> &BTreeMap<TaskId, SnapshotStalledTask> {
        &self.stalled_tasks
    }

    /// 理由なく止まっていると最初に見た時刻（最後の event の seq が変われば `now` から数え直す）。
    pub(crate) fn stall_since(
        &mut self,
        task_id: TaskId,
        last_seq: u64,
        now: OffsetDateTime,
    ) -> OffsetDateTime {
        match self.stall_watch.get(&task_id) {
            Some((seq, since)) if *seq == last_seq => *since,
            _ => {
                self.stall_watch.insert(task_id, (last_seq, now));
                now
            }
        }
    }

    /// 停滞を知らせた task の見張りを外す（次は何か起きてから数え直す）。
    pub(crate) fn forget_stall(&mut self, task_id: TaskId) {
        self.stall_watch.remove(&task_id);
    }

    /// 1 回の生存確認の終わり: 見張りを `watching` だけに保ち、停滞の一覧を `stalled` に置き換える。
    pub(crate) fn finish_stall_round(
        &mut self,
        watching: &HashSet<TaskId>,
        stalled: BTreeMap<TaskId, SnapshotStalledTask>,
    ) {
        self.stall_watch.retain(|id, _| watching.contains(id));
        self.stalled_tasks = stalled;
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
impl<S, C> ExecutionCoordination<S, C> {
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
impl LivenessWatch {
    pub(crate) fn stall_watch_is_empty(&self) -> bool {
        self.stall_watch.is_empty()
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
        let mut c = ExecutionCoordination::<(), ()>::new(1);
        c.reserve_write_set(key(a), Some(res.clone()));
        // a の予約はあるが、a は実行中の run に無い: b を止めない。
        assert_eq!(
            c.admit_write_set(&key(b), Some(res.clone()), [].iter(), 1),
            WriteSetAdmission::Admit(Some(res.clone()))
        );
        // a が実行中なら b を止め、見送りを記録する。
        let running = [key(a)];
        assert_eq!(
            c.admit_write_set(&key(b), Some(res.clone()), running.iter(), 1),
            WriteSetAdmission::Hold { blocker: key(a) }
        );
        assert_eq!(c.write_set_waits()[&key(b)].consecutive, 1);
        // 実行中の run に無い予約は刈り込みで消える。
        c.prune_write_set_state([].iter(), 1);
        assert_eq!(
            c.admit_write_set(&key(b), Some(res.clone()), running.iter(), 2),
            WriteSetAdmission::Admit(Some(res))
        );
    }

    /// 不変条件: WU の用意の一時的な失敗は上限の手前まで待ちの記録を置き、上限で記録を消して `None` を返す。
    /// 待ちの中かどうかは `retry_at` と時刻だけで決まる。
    /// 既存の試験で検出できない理由: dispatcher の試験は実時計の `now` で 1 回目の待ちしか見ず、上限の境目で記録が
    /// 消えることを owner の method 単独では確かめていない。
    #[test]
    fn wu_prepare_failures_count_up_to_the_limit() {
        let mut c = ExecutionCoordination::<(), ()>::new(1);
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

    fn idle_handle() -> JoinHandle<()> {
        tokio::spawn(async {})
    }

    fn checking_entry(task_id: TaskId) -> CheckingEntry {
        CheckingEntry {
            task_id,
            handle: idle_handle(),
            work_unit_id: "wu".into(),
            key: "k".into(),
            checks: 1,
            terminal: Terminal::Done {
                summary: String::new(),
                evidence: Vec::new(),
                usage: None,
            },
            account: None,
            account_adapter: None,
            provider: ProviderId::from("p"),
        }
    }

    /// 不変条件（D1「run の集合と lease の照合」・D3 の reclaim_expired_leases の行）: lease の回収・abort で
    /// `RunSupervisor` が手放した review の鍵を受けると、その task の review の持ち越しだけが消え、他の task の
    /// 持ち越しと check・統合・子待ちの保持は残る。drain（`abort_in_hand`）は check と統合を全部返し、持ち越しを
    /// 全部消す。既存の試験で検出できない理由: dispatcher 経由の試験（drain_hand_off・orphan_work_units）は手放した後の
    /// DB の状態を見るので、鍵を無視して持ち越しを全部消す実装や、check の保持を残したまま drain する実装でも通る。
    #[tokio::test]
    async fn reclaimed_review_keys_release_only_their_waits() {
        let (a, b, c) = (TaskId::new(), TaskId::new(), TaskId::new());
        let mut coord = ExecutionCoordination::<&str, ()>::new(2);
        coord.defer_review(a, "a");
        coord.defer_review(b, "b");
        coord
            .processes_mut()
            .start_checks("run-c".into(), checking_entry(c));
        coord.processes_mut().start_integration(
            b,
            IntegrationEntry {
                work_unit_id: "integrate-1".into(),
                handle: idle_handle(),
            },
        );
        assert!(coord.holds_task(b) && coord.holds_task(c) && !coord.holds_task(a));
        coord.release_reclaimed_review(a);
        assert_eq!(coord.take_deferred_review(a), None);
        assert_eq!(coord.take_deferred_review(b), Some("b"));
        coord.defer_review(b, "b");
        assert_eq!(
            coord.processes().checking_run_of(c).as_deref(),
            Some("run-c")
        );
        assert_eq!(coord.processes().in_flight(), 2);
        let in_hand = coord.abort_in_hand();
        assert_eq!(in_hand.checking.len(), 1);
        assert_eq!(in_hand.integrating.len(), 1);
        assert_eq!(coord.take_deferred_review(b), None);
        assert!(!coord.holds_task(b) && !coord.holds_task(c) && !coord.processes().has_checks());
    }

    /// 不変条件（ADR-0079 D10・ADR 2026-10-23 D5、停滞の通知の冪等性）: 同じ最後の seq の間は最初に見た時刻を
    /// 保ち、seq が変われば数え直す。見張りから外した task だけが消え、生存確認は間隔の内では 1 回だけ。
    /// 既存の試験で検出できない理由: tree_approval・finished_plan の試験は通知の event の数を見るので、見張りの
    /// 時刻を毎回 `now` に上書きする実装は timeout を長くしても同じ数の event で通る場合がある（時計を進めない試験）。
    #[test]
    fn stall_watch_keeps_the_first_sighting_until_the_seq_moves() {
        let (a, b) = (TaskId::new(), TaskId::new());
        let t0 = OffsetDateTime::UNIX_EPOCH;
        let t1 = t0 + time::Duration::seconds(30);
        let mut coord = ExecutionCoordination::<(), ()>::new(1);
        assert!(coord.liveness_mut().liveness_due(t0, 60));
        assert!(!coord.liveness_mut().liveness_due(t1, 60));
        assert!(
            coord
                .liveness_mut()
                .liveness_due(t0 + time::Duration::seconds(60), 60)
        );
        assert_eq!(coord.liveness_mut().stall_since(a, 5, t0), t0);
        assert_eq!(coord.liveness_mut().stall_since(a, 5, t1), t0);
        assert_eq!(coord.liveness_mut().stall_since(a, 6, t1), t1);
        assert_eq!(coord.liveness_mut().stall_since(b, 1, t0), t0);
        let watching: HashSet<TaskId> = [a].into_iter().collect();
        coord
            .liveness_mut()
            .finish_stall_round(&watching, BTreeMap::new());
        assert_eq!(coord.liveness_mut().stall_since(a, 6, t0), t1);
        assert_eq!(coord.liveness_mut().stall_since(b, 1, t1), t1);
        coord.liveness_mut().clear_stall_watch();
        assert!(
            coord.liveness().stall_watch_is_empty() && coord.liveness().stalled_tasks().is_empty()
        );
    }

    /// `src` の `pub(crate) struct <head> {` の本体の field の名（`///` と空行を除く）。field が `pub` なら落とす。
    fn private_fields(src: &str, head: &str) -> Vec<String> {
        let start = src
            .find(&format!("pub(crate) struct {head} {{"))
            .unwrap_or_else(|| panic!("{head} is defined"));
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
                "a {head} field is not private: {line}"
            );
            if let Some((name, _)) = t.split_once(": ") {
                fields.push(name.to_string());
            }
        }
        fields
    }

    /// 不変条件: ADR 2026-11-11 D2 の 1〜3・7（field が private、dispatcher の本体を受けない・`dispatcher` module を
    /// 使わない、field は D1 の表の own-coord-a の 14 件と own-coord-b の 16 件で run の集合を持たない。`#[cfg(test)]` の field は
    /// `dispatcher/tests/structure_guard.rs` の `OWNER_STRUCTS` が数える）、`crate::review` を使わない（review は
    /// dispatcher に依存し、使うと deps.cycle の循環が広がる）。下位 owner（`ProcessTable`・`LivenessWatch`）は
    /// `ExecutionCoordination` の field としてだけ置き、その field も private で、3 つの struct の field の和が D1 の表の 30 件になる。
    /// 既存の試験は振る舞いだけを見るので、field を `pub` に戻しても、method が dispatcher の本体を引数に受けても、
    /// run の表を field に足しても、下位 owner を `ExecutionCoordination` の外に置いても通る。
    #[test]
    fn structure_guard_execution_coordination() {
        let src = include_str!("execution_coordination.rs");
        let src = &src[..src
            .find("#[cfg(test)]\nimpl WriteReservation")
            .expect("the test-only impl is in the source")];
        let mut own = private_fields(src, "ExecutionCoordination<S, C>");
        own.sort();
        assert_eq!(
            own.iter()
                .filter(|f| *f == "processes" || *f == "liveness")
                .count(),
            2,
            "ExecutionCoordination holds ProcessTable and LivenessWatch as its own fields"
        );
        let mut fields: Vec<String> = own
            .into_iter()
            .filter(|f| f != "processes" && f != "liveness")
            .chain(private_fields(src, "ProcessTable"))
            .chain(private_fields(src, "LivenessWatch"))
            .collect();
        fields.sort();
        assert_eq!(
            fields,
            [
                "accepting_new_work",
                "awaiting_children",
                "awaiting_human",
                "browser_ledger",
                "browser_prereq_rescan",
                "checking",
                "cluster_waiting",
                "credential_busy",
                "dispatch_waits",
                "integrated_deps",
                "integrating",
                "last_tick_gap_ms",
                "last_tick_started",
                "liveness_checked_at",
                "max_concurrency",
                "pending_subjects",
                "review_resync_halted",
                "review_sync_queue",
                "review_sync_seq",
                "run_write_bases",
                "rx",
                "stall_watch",
                "stalled_tasks",
                "ticks",
                "tx",
                "write_reservations",
                "write_set_hold_seq",
                "write_set_waits",
                "wu_prepare_failures",
            ],
            "ExecutionCoordination (with its sub-owners) owns the 13 remaining own-coord-a and 16 own-coord-b fields of ADR D1 (the credential wait lives only in the store)"
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
