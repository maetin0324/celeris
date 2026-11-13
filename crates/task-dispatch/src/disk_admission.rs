//! ADR 2026-11-11-disk-aware-dispatch: dispatch の前の空きの見込み。
//!
//! - D1: `空き − Σ 走っている target の増分 − 通したがまだ走査に出ない run の見込み − この run の見込み ≥ 予備` で
//!   なければ、新しい target を作る run を始めず `ready` のまま待たせる。repo の見込みは target が pool にある同じ
//!   `repo_key` の owner の測った大きさの最大（GC の推定と同じ `task_worker::scratch::repo_max_sizes`）。増分を数えるのは
//!   run か review の検査が今 target に書いている owner（`task_worker::scratch::target_written_now`）と、lease の生きた
//!   release-build（build ごとに repo の見込みまで育つ）だけ。通した run は、worker が lease を書いて走査に書いている
//!   owner として出るまで（多くは 10〜30 秒、長いと数分）、[`Gate`] のメモリで見込みを数え続ける（pass をまたぐ）。
//!   数え始めるのは run を起動したとき（[`Gate::started`]。dispatcher が `running` に入れた直後）で、判定で通しても
//!   その後に起動を見送った run（経路の選択の拒否・分類の待ち・credential の待ち・lease が取れない）は数えない
//!   （[`Admission::Go`] の見込みを捨てる。attempt 10 の review の指摘）。
//! - D1「1 本は通す」: 終わる見込みのある書き手（この daemon が run・review・WU の検査を今走らせている task の、走査で
//!   増分を持つ owner と、通したがまだ走査に出ない run）が 1 本も無ければ 1 本は通す（詰まらない）。増分を持つかは走査の
//!   DB の状態から決まるが、`reviewing` で子・人の承認・再同期の停止を待つ task は target を書かず、待っても終わらない
//!   ので書き手に数えない（attempt 7 の review の指摘）。owner は worker と同じ（[`task_worker::scratch::Owner::of_run`]）。
//! - D1 付記（2026-11-12、親の数え方）: WU を待っているだけの `running` の親（Task の target に書く run・検査が無い）は、
//!   走査の分類（`task waiting for work units`。[`task_worker::scratch::task_awaits_work_units`]）で増分も持たない。親の
//!   owner に書く run（Task 単位の run・Task の worktree を共有する WU）はこの判定で見込みを数え、走ると `task running`
//!   として走査の増分に入る。
//! - D2: 予備（空きの下限）は filesystem の容量 × `[dispatch] min_free_disk_pct`（[`min_free_bytes`]）。
//! - D1「記録」: 待たせた run の判定は task の待ちの理由 `TaskWait::DiskForecast`（ADR 2026-10-11-task-wait-reasons の M12）
//!   として返し、dispatcher がその場で `dispatch_waits` に入れる（[`Gate::judge_run`]）。task の `waits`・`runnable`・停滞の
//!   判定は待ちとして見る（理由の無い `ready` として受信箱の `task_unscheduled` に出ない）。
//! - D4: 見込みは `ScratchStatus.admission`（[`forecast`]）。待たせている run の正本は daemon の [`Gate`] のメモリだけで、
//!   file には書かない（同じ pool を見る verify の daemon が上書きできない）。daemon の snapshot
//!   （`GET /api/v1/metrics/scratch`）を組むときに [`Gate::with_held`] で `held` を埋める。pool の走査だけで組む status
//!   （`celerisctl scratch status`）の `held` は `None`（分からない）。
//!
//! dispatcher の module の外に置く（Dispatcher の field・method を増やさない）。**LLM は呼ばない。** 判定は注入できる
//! 値（[`Inputs`]）だけで決まる。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use task_core::{Task, TaskWait, WorkUnitRow, WorkspaceSpec};

use task_ops::daemon::{
    ScratchAdmissionView, ScratchHeldRunView, ScratchOwnerForecastView, ScratchOwnerView,
    ScratchStatus,
};

/// D2: filesystem の容量 `total` に対する空きの下限（byte）。`pct` が 0 以下なら 0（無効）。
pub fn min_free_bytes(total: u64, pct: f64) -> u64 {
    if pct <= 0.0 || !pct.is_finite() {
        return 0;
    }
    ((total as f64) * pct / 100.0).ceil() as u64
}

/// D2: `paths` のうち空きが下限（容量 × `pct`）を割る最初のものの理由（filesystem が読めなければそれも理由。
/// 安全側に倒して run を始めない）。`fs` は (容量, 空き) を返す statvfs（`scratch_gc::fs_stats`）。
pub fn find_low_disk(
    paths: &[std::path::PathBuf],
    pct: f64,
    fs: impl Fn(&Path) -> Option<(u64, u64)>,
) -> Option<String> {
    paths.iter().find_map(|path| match fs(path) {
        Some((total, free)) if free < min_free_bytes(total, pct) => Some(format!(
            "{}: {} free (minimum {} = {pct}% of {})",
            path.display(),
            gb(free),
            gb(min_free_bytes(total, pct)),
            gb(total),
        )),
        Some(_) => None,
        None => Some(format!("{}: cannot read the filesystem", path.display())),
    })
}

/// 見込みの材料の owner 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub owner: String,
    pub repo_key: String,
    /// 増分を予約する: class `p0` で [`task_worker::scratch::target_written_now`] か、lease の生きた release-build。
    /// 「1 本は通す」の書き手かどうかは別（[`Gate::begin_pass`] の `runs`・`others`）。
    pub grows: bool,
    pub size_bytes: Option<u64>,
}

/// 見込みの材料（試験は値を直接与える）。
#[derive(Debug, Clone, PartialEq)]
pub struct Inputs {
    pub free_bytes: Option<u64>,
    pub reserve_bytes: u64,
    pub ratio: f64,
    pub samples: Vec<Sample>,
}

/// scratch の走査の行から材料を作る。target が pool にある owner を数える（GC の推定と同じ入力。target が消えて
/// lease の記録だけが残る owner の古い大きさは数えない）。ただし増分を予約する owner
/// （`grows`）は、target がまだ無くても数える: 割り当ては seed を `.seed-copy-*` に写し終えてから `target/` に rename
/// するので、写している数分のあいだ `has_target` は false で、その run の増分が 0 に見えて次の tick の run が通ってしまう。
/// その owner の大きさは分からないものとして（`None`）、増分を repo の見込み全体で数える。野良・repo の分からない owner は
/// 数えない。
pub fn samples_from_owners(owners: &[ScratchOwnerView]) -> Vec<Sample> {
    owners
        .iter()
        .filter(|o| o.kind != "stray")
        .filter_map(|o| {
            // release-build の lease が生きているあいだは、次の release の build が repo の見込みまで target を育てる
            // （build ごとに約 88 GB。終わりに上限で作り直される）ので、増える owner として数える。
            let grows = o.class == "p0"
                && (task_worker::scratch::target_written_now(&o.reason)
                    || (o.kind == "release" && o.reason == task_worker::scratch::LEASE_LIVE));
            if !o.has_target && !grows {
                return None;
            }
            Some(Sample {
                owner: o.owner.clone(),
                repo_key: o.repo_key.clone()?,
                grows,
                size_bytes: if o.has_target { o.size_bytes } else { None },
            })
        })
        .collect()
}

fn scaled(bytes: u64, ratio: f64) -> u64 {
    ((bytes as f64) * ratio).ceil() as u64
}

/// D1/D4: 見込みを組む。`held` は待たせている run（daemon の [`Gate::held_views`]。pool の走査だけなら `None`）。
pub fn forecast(inputs: &Inputs, held: Option<Vec<ScratchHeldRunView>>) -> ScratchAdmissionView {
    let expected_by_repo = task_worker::scratch::repo_max_sizes(
        inputs
            .samples
            .iter()
            .filter_map(|s| Some((s.repo_key.as_str(), s.size_bytes?))),
    );
    let mut owners: Vec<ScratchOwnerForecastView> = inputs
        .samples
        .iter()
        .map(|s| {
            let expected = expected_by_repo.get(&s.repo_key).copied().unwrap_or(0);
            let growth = if s.grows {
                scaled(
                    expected.saturating_sub(s.size_bytes.unwrap_or(0)),
                    inputs.ratio,
                )
            } else {
                0
            };
            ScratchOwnerForecastView {
                owner: s.owner.clone(),
                repo_key: s.repo_key.clone(),
                grows: s.grows,
                size_bytes: s.size_bytes,
                expected_bytes: expected,
                growth_bytes: growth,
            }
        })
        .collect();
    owners.sort_by(|a, b| a.owner.cmp(&b.owner));
    ScratchAdmissionView {
        free_bytes: inputs.free_bytes,
        reserve_bytes: inputs.reserve_bytes,
        ratio: inputs.ratio,
        running_growth_bytes: owners.iter().map(|o| o.growth_bytes).sum(),
        expected_by_repo,
        owners,
        held,
    }
}

/// この run の見込み（disk 上の量）。owner の target が既にあれば増分だけ、無ければ repo の見込み全体。
pub fn need_bytes(view: &ScratchAdmissionView, owner: &str, repo_key: &str) -> u64 {
    let expected = view.expected_by_repo.get(repo_key).copied().unwrap_or(0);
    let existing = view
        .owners
        .iter()
        .find(|o| o.owner == owner)
        .and_then(|o| o.size_bytes)
        .unwrap_or(0);
    scaled(expected.saturating_sub(existing), view.ratio)
}

/// 1 run の判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 見込み 0・空きが読めない（判定しない）。
    Skip,
    Admit {
        need_bytes: u64,
    },
    Hold {
        need_bytes: u64,
        reason: String,
    },
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// D1 の式。`admitted_bytes` は通したがまだ走査に書いている owner として出ない run の見込みの合計、`writers` は終わる
/// 見込みのある書き手の数（[`Gate::judge`]）。`writers` が 0 なら通す（待っても空きを戻す run の終わりが来ないので、
/// 待たせると詰まる）。
pub fn decide(
    view: &ScratchAdmissionView,
    owner: &str,
    repo_key: &str,
    admitted_bytes: u64,
    writers: usize,
) -> Decision {
    let Some(free) = view.free_bytes else {
        return Decision::Skip;
    };
    let need = need_bytes(view, owner, repo_key);
    if need == 0 {
        return Decision::Skip;
    }
    if writers == 0 {
        return Decision::Admit { need_bytes: need };
    }
    let available = free
        .saturating_sub(view.running_growth_bytes)
        .saturating_sub(admitted_bytes);
    if available >= view.reserve_bytes.saturating_add(need) {
        return Decision::Admit { need_bytes: need };
    }
    Decision::Hold {
        need_bytes: need,
        reason: format!(
            "free {} − running growth {} − admitted {} − need {} < reserve {} (repo {repo_key}, ratio {})",
            gb(free),
            gb(view.running_growth_bytes),
            gb(admitted_bytes),
            gb(need),
            gb(view.reserve_bytes),
            view.ratio,
        ),
    }
}

/// 試験が空きと target の大きさを与える口（本番は `None` で、scratch の走査の結果を使う）。
pub type Probe = Arc<dyn Fn() -> Inputs + Send + Sync>;

type HoldKey = (String, Option<String>);

/// 1 run の判定の結果（[`Gate::judge`]。判定しなかった run は `None`）。
#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    /// 始めてよい。見込みは、run を起動したときに [`Gate::started`] に渡して数え始める。起動の前に見送ったら捨てる
    /// （数えない）。
    Go(Reservation),
    /// 待たせる。task の待ちの理由（呼び手が `dispatch_waits` に入れる）。
    Wait(TaskWait),
}

/// 判定で通した run の見込み。dispatcher が run を `running` に入れた後で [`Gate::started`] に渡したときだけ、通したが
/// まだ走査に出ない run として数え始める。判定の時点で数えると、その後に経路の選択などで起動を見送った run が同じ
/// pass の後続の書き手と見込みに残り、走っている run が 0 本でも後続が待つ（拒否が続く間、毎 pass。attempt 10 の
/// review の指摘）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    key: HoldKey,
    owner: String,
    need_bytes: u64,
}

/// dispatcher の待たせている run と、通したがまだ走査に出ない run（`ScratchState.admission`）。pass ごとに
/// [`Gate::begin_pass`] を呼ぶ。dispatch の途中で `&self` の Dispatcher から呼べるよう、状態は中の Mutex に持つ
/// （daemon の tick の thread だけが触る）。
#[derive(Default)]
pub struct Gate {
    pub probe: Option<Probe>,
    state: Mutex<GateState>,
}

#[derive(Default)]
struct GateState {
    holds: BTreeMap<HoldKey, ScratchHeldRunView>,
    seen: BTreeSet<HoldKey>,
    /// この pass の始めに、この daemon が run・review・WU の検査を走らせている task（[`Gate::begin_pass`]）。
    busy: BTreeSet<String>,
    /// 起動した run（task・WU）→ (owner, 見込み)（[`Gate::started`]）。worker が lease を書き、走査に書いている owner
    /// として出るまで（その後は走査の増分が数える）、または run が終わるまで持つ。pass ごとに 0 に戻すと、直前の tick で通した run の見込みが
    /// lease の現れるまで消え、まとめて来た run が見込みを超えて始まる（attempt 5 の review の指摘）。
    admitted: BTreeMap<HoldKey, (String, u64)>,
    /// 待ち始めの記録（log）を残した回数（試験が 1 回だけかを確かめる）。
    notes: u64,
}

impl Gate {
    fn state(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 候補を照合する pass の始め（枠が埋まって照合しない pass では呼ばない。記録はそのまま残る）: 前の pass で判定に
    /// 来なかった run（始まった・`ready` でなくなった）の待ちの記録を消し、通した run のうち走っていないもの
    /// （`runs` に無い。lease を書く前に終わった・起動の前に見送った）を消す。`runs` は dispatcher の走っている run の鍵
    /// （task・WU）、`others` は review・WU の検査を今走らせている task。「1 本は通す」の書き手は、この 2 つの task の
    /// 増分を持つ owner だけ。
    pub fn begin_pass(
        &self,
        runs: impl IntoIterator<Item = (impl ToString, Option<String>)>,
        others: impl IntoIterator<Item = impl ToString>,
    ) {
        let runs: BTreeSet<HoldKey> = (runs.into_iter())
            .map(|(task, wu)| (task.to_string(), wu))
            .collect();
        let mut st = self.state();
        st.busy = (runs.iter().map(|(task, _)| task.clone()))
            .chain(others.into_iter().map(|task| task.to_string()))
            .collect();
        let seen = std::mem::take(&mut st.seen);
        st.holds.retain(|k, _| seen.contains(k));
        st.admitted.retain(|k, _| runs.contains(k));
    }

    /// 通したがまだ走査に出ない run の見込みの合計。
    #[cfg(test)]
    pub(crate) fn admitted_bytes(&self) -> u64 {
        self.state().admitted.values().map(|(_, b)| b).sum()
    }

    /// D4: daemon の snapshot の scratch の status（pool の走査で組んだ `view`）に、待たせている run をメモリから載せる。
    pub fn with_held(&self, view: Option<&ScratchStatus>) -> Option<ScratchStatus> {
        let mut status = view?.clone();
        status.admission.held = Some(self.held_views());
        Some(status)
    }

    /// 待たせている run（task・WU の順）。
    pub fn held_views(&self) -> Vec<ScratchHeldRunView> {
        self.state().holds.values().cloned().collect()
    }

    /// 待ち始めの log を残した回数。
    pub fn notes(&self) -> u64 {
        self.state().notes
    }

    /// 判定に使う見込み。`probe` があればそれ、無ければ scratch の走査の結果（scratch が無効なら `None`）。
    pub fn forecast(&self, view: Option<&ScratchStatus>) -> Option<ScratchAdmissionView> {
        match &self.probe {
            Some(probe) => Some(forecast(&probe(), None)),
            None => view.filter(|v| v.enabled).map(|v| v.admission.clone()),
        }
    }

    /// D1 の dispatch からの入口: Local の task で、測った見込みがあるときだけ `repo_of`（run の cargo の target を
    /// 結び付ける checkout の source）を引いて [`Gate::judge`] で判定する。`work_unit_id` は
    /// dispatcher の run の鍵（phase のある WU）、`target_work_unit` は worker の `CARGO_TARGET_DIR` の owner の WU
    /// （自分の worktree を持つ WU だけ。task の worktree を共有する WU は `None`）。
    pub fn judge_run(
        &self,
        view: Option<&ScratchStatus>,
        task: &Task,
        work_unit_id: Option<&str>,
        target_work_unit: Option<&WorkUnitRow>,
        repo_of: impl FnOnce() -> Option<PathBuf>,
    ) -> Option<Admission> {
        if !matches!(task.workspace, WorkspaceSpec::Local { .. }) {
            return None;
        }
        let view = self
            .forecast(view)
            .filter(|v| !v.expected_by_repo.is_empty())?;
        let repo = repo_of()?;
        let repo_key = task_worker::build_cache::repo_cache_key(&repo);
        let id = task.id.to_string();
        let target_work_unit = target_work_unit.map(|w| w.id.as_str());
        let owner = task_worker::scratch::Owner::of_run(&id, target_work_unit).to_string();
        self.judge(
            &view,
            &id,
            work_unit_id,
            &owner,
            &repo_key,
            SystemTime::now(),
        )
    }

    /// D1: run（task・WU。dispatcher の run の鍵）を判定する。待たせるなら、その task の待ちの理由
    /// （`TaskWait::DiskForecast`。待たせている run の記録と同じ判定から作る）。`owner` はその run の `CARGO_TARGET_DIR`
    /// の owner（worker と同じ [`task_worker::scratch::Owner::of_run`]）。待ち始めに 1 回だけ log を残す（受信箱・通知には
    /// 出さない）。通すなら見込み（[`Reservation`]）を返すだけで、まだ数えない（[`Gate::started`]）。判定しない（見込み 0・
    /// 空きが読めない）なら `None`。
    pub fn judge(
        &self,
        view: &ScratchAdmissionView,
        task_id: &str,
        work_unit_id: Option<&str>,
        owner: &str,
        repo_key: &str,
        now: SystemTime,
    ) -> Option<Admission> {
        let owner = owner.to_string();
        let key: HoldKey = (task_id.to_string(), work_unit_id.map(str::to_string));
        let growing: Vec<&str> = view
            .owners
            .iter()
            .filter(|o| o.grows)
            .map(|o| o.owner.as_str())
            .collect();
        let mut st = self.state();
        // 走査に増分を持つ owner として出た run は、走査の増分が数える（二重に数えない）。
        st.admitted
            .retain(|k, (o, _)| k != &key && !growing.contains(&o.as_str()));
        let admitted_bytes = st.admitted.values().map(|(_, b)| b).sum();
        // 「1 本は通す」の書き手: 増分を持つ owner のうち、この daemon が run・review・WU の検査を走らせている task の
        // もの。release-build（task が無い）と、reviewing のまま子・人を待つ task は入らない（待っても終わらないので、
        // 数えると詰まる）。WU を待つだけの running の親は、走査で増分を持たない（`task waiting for work units`）。
        let busy_writers = growing
            .iter()
            .filter_map(|o| task_worker::scratch::Owner::parse(o).ok())
            .filter(|o| o.task_id().is_some_and(|t| st.busy.contains(t)))
            .count();
        let writers = busy_writers + st.admitted.len();
        match decide(view, &owner, repo_key, admitted_bytes, writers) {
            Decision::Skip => {
                st.holds.remove(&key);
                None
            }
            Decision::Admit { need_bytes } => {
                st.holds.remove(&key);
                Some(Admission::Go(Reservation {
                    key,
                    owner,
                    need_bytes,
                }))
            }
            Decision::Hold { need_bytes, reason } => {
                st.seen.insert(key.clone());
                let since = match st.holds.get(&key) {
                    Some(h) => h.since.clone(),
                    None => {
                        tracing::info!(task_id, work_unit_id, %owner, %reason, "disk forecast: run held");
                        st.notes += 1;
                        task_worker::scratch::rfc3339(now)
                    }
                };
                st.holds.insert(
                    key,
                    ScratchHeldRunView {
                        task_id: task_id.to_string(),
                        work_unit_id: work_unit_id.map(str::to_string),
                        owner,
                        repo_key: repo_key.to_string(),
                        need_bytes,
                        reason: reason.clone(),
                        since,
                    },
                );
                Some(Admission::Wait(TaskWait::DiskForecast {
                    need_bytes,
                    detail: reason,
                }))
            }
        }
    }

    /// D1: dispatcher が run を `running` に入れた直後に、その run の判定の結果（[`Gate::judge_run`]）を渡す。通した
    /// （`Go`）run の見込みを数え始める。この後の判定は、走査に書いている owner として出るまで（または run が終わり次の
    /// pass の [`Gate::begin_pass`] の `runs` から消えるまで）、この見込みを差し引き、この run を「1 本は通す」の書き手に
    /// 数える。`Wait` の run は起動しないので来ない（来ても数えない）。
    pub fn started(&self, admission: Option<Admission>) {
        if let Some(Admission::Go(Reservation {
            key,
            owner,
            need_bytes,
        })) = admission
        {
            self.state().admitted.insert(key, (owner, need_bytes));
        }
    }
}

#[cfg(test)]
mod tests;
