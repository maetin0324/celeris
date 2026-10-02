//! ADR-0130 D3: 同じ repo で走っている run の expected write-set と候補の expected write-set が
//! 強く重なれば、候補の起動を次 tick に回す（待たせるだけ。失敗・attempts・状態遷移なし）。
//! hint の無い run は予約を作らず、照合もしない（従来どおり）。
//!
//! D3 の配線で決めた細部（ADR 本文への反映は verify 葉で行う）:
//!
//! - **予約の置き場所**: dispatcher のメモリ上の側表（`RunKey` → repo 鍵と正規化 prefix）に、run を
//!   起こした時点の hint で置く。照合は `running` に残る run の予約だけを見るので、run の終了・lease
//!   回収・中止で `running` から外れた時点で予約も効かなくなる。task の lease は 1 つの daemon だけが
//!   持つ（ADR-0040）ので、この Phase では store の予約表は作らない。
//! - **照合の位置**: `dispatch_one` の retry / infra backoff の後、クラスタ解決・作業場所の用意・
//!   lease・`Trigger::Dispatch` より前。見送りは `Ok(false)` を返すだけ。
//! - **対象の run**: implementation の run（atomic task の run と WU の run）。planner run と CoS の
//!   対話 run は予約も照合もしない。どちらかが hint なしなら重ならない（D1 の従来互換）。
//! - **repo 鍵**: `task.repos` があれば `RepoId`、無い旧 task はローカル作業場所のパス（`path:`）、
//!   リモートは `remote:<cluster>:<path>`。
//! - **同じ task の WU 同士**: 両方が task の有効 hint をそのまま継いだものなら照合しない。unit 固有の
//!   hint を持つ WU は兄弟とも照合する。
//! - **見送りの記録**: event は増やさず `tracing` の log（`write-set overlap; deferring the run`）に残す。
//! - **公平性**: `RunKey` ごとに待機開始 tick と連続回数を持ち、連続 3 回以上見送られた ready task は
//!   待機開始の古い順に候補の先頭へ移す。
//! - **actual の併用**: 走行中の run の既知の actual はこの Phase では照合に使わない（expected だけ）。

use super::*;

/// 連続してこの回数見送られた ready task は、待機開始の古い順に候補の先頭へ移す（ADR-0130 D3）。
pub(super) const WRITE_SET_STARVATION_TICKS: u32 = 3;

/// run を起こした時点の hint で作る予約（後から hint を変えても変わらない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WriteReservation {
    /// repo の鍵（`repo:<RepoId>` / `path:<dir>` / `remote:<cluster>:<path>`）。
    repos: Vec<String>,
    /// 正規化した prefix（`task_core::write_set::normalize_write_paths` 済み）。
    paths: Vec<String>,
    /// WU の hint が task の有効 hint をそのまま継いだものか（同じ task の兄弟とは照合しない）。
    inherited_from_task: bool,
}

/// 見送りの記録（`RunKey` ごと）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WriteSetWait {
    pub(super) since_tick: u64,
    pub(super) last_tick: u64,
    pub(super) consecutive: u32,
}

/// 2 つの予約が強く重なるか（同じ repo 鍵を共有し、prefix が等しいか segment 境界の祖先）。
pub(super) fn reservations_overlap(
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

impl Dispatcher {
    /// 候補の run の予約。hint なし・planner run・CoS の対話 run は `None`（gate に掛けない）。
    pub(super) fn write_reservation_for(
        &self,
        task: &Task,
        wu: Option<&task_core::WorkUnitRow>,
    ) -> Result<Option<WriteReservation>, DispatchError> {
        if task_core::is_conversation(task) {
            return Ok(None);
        }
        let task_hint = self.store.effective_task_write_paths(task.id)?;
        let (paths, inherited_from_task) = match wu {
            Some(wu) => {
                let hint = self.store.work_unit_expected_write_paths(&wu.id)?;
                let inherited = hint.is_some() && hint == task_hint;
                (hint, inherited)
            }
            None => (task_hint, false),
        };
        let Some(paths) = paths.filter(|p| !p.is_empty()) else {
            return Ok(None);
        };
        let paths = match task_core::write_set::normalize_write_paths(&paths) {
            Ok(paths) => paths,
            Err(e) => {
                // 保存時に検証済みのはず。読めない hint は従来互換で「hint なし」に倒す。
                tracing::warn!(task_id = %task.id, error = %e, "invalid expected_write_paths; ignoring the hint (ADR-0130 D1)");
                return Ok(None);
            }
        };
        let repos = write_set_repo_keys(task);
        if repos.is_empty() {
            return Ok(None);
        }
        Ok(Some(WriteReservation {
            repos,
            paths,
            inherited_from_task,
        }))
    }

    /// 走っている run の予約のうち、`candidate` と強く重なる最初のもの（`RunKey` の順で決定的）。
    pub(super) fn write_set_blocker(
        &self,
        key: &RunKey,
        candidate: &WriteReservation,
    ) -> Option<RunKey> {
        let mut blockers: Vec<&RunKey> = self
            .write_reservations
            .iter()
            .filter(|(other, _)| *other != key && self.running.contains_key(*other))
            .filter(|(other, res)| reservations_overlap(key.task, candidate, other.task, res))
            .map(|(other, _)| other)
            .collect();
        blockers.sort_by(|a, b| {
            (a.task.to_string(), &a.work_unit).cmp(&(b.task.to_string(), &b.work_unit))
        });
        blockers.first().map(|k| (*k).clone())
    }

    /// 見送りを記録して log に残す（状態遷移・attempts には触らない）。
    pub(super) fn note_write_set_hold(&mut self, key: &RunKey, blocker: &RunKey) {
        let tick = self.ticks;
        let wait = self
            .write_set_waits
            .entry(key.clone())
            .or_insert(WriteSetWait {
                since_tick: tick,
                last_tick: tick,
                consecutive: 0,
            });
        if wait.last_tick + 1 < tick {
            // 直前の tick に見送られていない: 数え直す。
            *wait = WriteSetWait {
                since_tick: tick,
                last_tick: tick,
                consecutive: 0,
            };
        }
        if wait.consecutive == 0 || wait.last_tick != tick {
            wait.consecutive += 1;
        }
        wait.last_tick = tick;
        let wait = *wait;
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
    pub(super) fn reserve_write_set(&mut self, key: RunKey, reservation: Option<WriteReservation>) {
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

    /// tick の dispatch の前に、終わった run の予約と古い待機記録を捨てる。
    pub(super) fn prune_write_set_state(&mut self) {
        let running = &self.running;
        self.write_reservations
            .retain(|k, _| running.contains_key(k));
        let tick = self.ticks;
        self.write_set_waits.retain(|_, w| w.last_tick + 1 >= tick);
    }

    /// ADR-0130 D3 の公平性: 連続 `WRITE_SET_STARVATION_TICKS` 回以上見送られた ready task を、待機開始の
    /// 古い順に先頭へ移す（それ以外の候補の相対順は変えない）。
    pub(super) fn order_write_set_starved_first(&self, candidates: Vec<Task>) -> Vec<Task> {
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
}

/// task の repo 鍵（ADR-0130 付記）。子 task は親の `repos` を継ぐので同じ鍵になる。
fn write_set_repo_keys(task: &Task) -> Vec<String> {
    if !task.repos.is_empty() {
        let mut keys: Vec<String> = task
            .repos
            .iter()
            .map(|r| format!("repo:{}", r.repo_id))
            .collect();
        keys.sort();
        keys.dedup();
        return keys;
    }
    match &task.workspace {
        WorkspaceSpec::Local { path, .. } => {
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            vec![format!("path:{}", path.display())]
        }
        WorkspaceSpec::Remote { cluster, path, .. } => {
            vec![format!("remote:{cluster}:{}", path.display())]
        }
    }
}

#[cfg(test)]
impl WriteReservation {
    pub(super) fn for_test(repos: &[&str], paths: &[&str], inherited_from_task: bool) -> Self {
        WriteReservation {
            repos: repos.iter().map(|s| s.to_string()).collect(),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            inherited_from_task,
        }
    }
}
