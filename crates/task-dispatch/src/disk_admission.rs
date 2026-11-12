//! ADR 2026-11-11-disk-aware-dispatch: dispatch の前の空きの見込み。
//!
//! - D1: `空き − Σ 走っている target の増分 − 今の pass で通した run の見込み − この run の見込み ≥ 予備` でなければ、
//!   新しい target を作る run を始めず `ready` のまま待たせる。repo の見込みは target が pool にある同じ `repo_key` の
//!   owner の測った大きさの最大（GC の推定と同じ `task_worker::scratch::repo_max_sizes`）。増分を数えるのは run か
//!   review の検査が今 target に書いている owner（`task_worker::scratch::target_written_now`）と、lease の生きた
//!   release-build（build ごとに repo の見込みまで育つ）だけ。worker run が 1 本も
//!   走っていなければ 1 本は通す（詰まらない）。
//! - D2: 予備（空きの下限）は filesystem の容量 × `[dispatch] min_free_disk_pct`（[`min_free_bytes`]）。
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

use task_core::{Task, WorkspaceSpec};

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
    /// run か review の検査が今 target に書いている（class `p0` で [`task_worker::scratch::target_written_now`]）。
    pub running: bool,
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
/// lease の記録だけが残る owner の古い大きさは数えない）。ただし run か review の検査が今書いている owner
/// （`running`）は、target がまだ無くても数える: 割り当ては seed を `.seed-copy-*` に写し終えてから `target/` に rename
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
            let running = o.class == "p0"
                && (task_worker::scratch::target_written_now(&o.reason)
                    || (o.kind == "release" && o.reason == task_worker::scratch::LEASE_LIVE));
            if !o.has_target && !running {
                return None;
            }
            Some(Sample {
                owner: o.owner.clone(),
                repo_key: o.repo_key.clone()?,
                running,
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
            let growth = if s.running {
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
                running: s.running,
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

/// D1 の式。`admitted_bytes` は同じ pass で先に通した run の見込みの合計、`runs_in_flight` は走っている worker run の数。
pub fn decide(
    view: &ScratchAdmissionView,
    owner: &str,
    repo_key: &str,
    admitted_bytes: u64,
    runs_in_flight: usize,
) -> Decision {
    let Some(free) = view.free_bytes else {
        return Decision::Skip;
    };
    let need = need_bytes(view, owner, repo_key);
    if need == 0 {
        return Decision::Skip;
    }
    if runs_in_flight == 0 {
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

/// dispatcher の待たせている run（`ScratchState.admission`）。pass ごとに [`Gate::begin_pass`] を呼ぶ。dispatch の
/// 途中で `&self` の Dispatcher から呼べるよう、状態は中の Mutex に持つ（daemon の tick の thread だけが触る）。
#[derive(Default)]
pub struct Gate {
    pub probe: Option<Probe>,
    state: Mutex<GateState>,
}

#[derive(Default)]
struct GateState {
    holds: BTreeMap<HoldKey, ScratchHeldRunView>,
    seen: BTreeSet<HoldKey>,
    admitted_bytes: u64,
    /// 待ち始めの記録（log）を残した回数（試験が 1 回だけかを確かめる）。
    notes: u64,
}

impl Gate {
    fn state(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 候補を照合する pass の始め（枠が埋まって照合しない pass では呼ばない。記録はそのまま残る）: 前の pass で判定に
    /// 来なかった run（始まった・`ready` でなくなった）の記録を消し、通した量を 0 に戻す。
    pub fn begin_pass(&self) {
        let mut st = self.state();
        let seen = std::mem::take(&mut st.seen);
        st.holds.retain(|k, _| seen.contains(k));
        st.admitted_bytes = 0;
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
    /// 結び付ける checkout の source）を引いて [`Gate::hold`] で判定する。待たせるなら `true`。
    pub fn holds_run(
        &self,
        view: Option<&ScratchStatus>,
        task: &Task,
        work_unit_id: Option<&str>,
        runs_in_flight: usize,
        repo_of: impl FnOnce() -> Option<PathBuf>,
    ) -> bool {
        if !matches!(task.workspace, WorkspaceSpec::Local { .. }) {
            return false;
        }
        let Some(view) = self
            .forecast(view)
            .filter(|v| !v.expected_by_repo.is_empty())
        else {
            return false;
        };
        let Some(repo) = repo_of() else {
            return false;
        };
        let repo_key = task_worker::build_cache::repo_cache_key(&repo);
        let id = task.id.to_string();
        self.hold(
            &view,
            &id,
            work_unit_id,
            &repo_key,
            runs_in_flight,
            SystemTime::now(),
        )
    }

    /// D1: run（task・WU）を待たせるなら `true`。待ち始めに 1 回だけ log を残す（受信箱・通知には出さない）。
    pub fn hold(
        &self,
        view: &ScratchAdmissionView,
        task_id: &str,
        work_unit_id: Option<&str>,
        repo_key: &str,
        runs_in_flight: usize,
        now: SystemTime,
    ) -> bool {
        let owner = match work_unit_id {
            Some(wu) => task_worker::scratch::Owner::work_unit(task_id, wu),
            None => task_worker::scratch::Owner::task(task_id),
        }
        .to_string();
        let key: HoldKey = (task_id.to_string(), work_unit_id.map(str::to_string));
        let mut st = self.state();
        match decide(view, &owner, repo_key, st.admitted_bytes, runs_in_flight) {
            Decision::Skip => {
                st.holds.remove(&key);
                false
            }
            Decision::Admit { need_bytes } => {
                st.holds.remove(&key);
                st.admitted_bytes = st.admitted_bytes.saturating_add(need_bytes);
                false
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
                        reason,
                        since,
                    },
                );
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn sample(owner: &str, repo: &str, running: bool, size: Option<u64>) -> Sample {
        Sample {
            owner: owner.into(),
            repo_key: repo.into(),
            running,
            size_bytes: size,
        }
    }

    fn inputs(free: u64, samples: Vec<Sample>) -> Inputs {
        Inputs {
            free_bytes: Some(free),
            reserve_bytes: 30 * GB,
            ratio: 1.0,
            samples,
        }
    }

    /// 不変条件: 予備は容量に比例し、既定 10% は disk_watch の critical（95%）より手前で止める。
    #[test]
    fn the_reserve_is_proportional_to_the_filesystem() {
        assert_eq!(min_free_bytes(300 * GB, 10.0), 30 * GB);
        assert_eq!(min_free_bytes(300 * GB, 0.0), 0);
        assert!(min_free_bytes(300 * GB, 10.0) > 300 * GB / 100 * 5);
    }

    /// 不変条件: repo の見込みは同じ repo の最大、走っている owner だけが増分を持つ。
    #[test]
    fn expectations_are_the_repo_maximum_and_growth_counts_running_owners() {
        let v = forecast(
            &inputs(
                100 * GB,
                vec![
                    sample("task-a", "ap", false, Some(100 * GB)),
                    sample("task-b", "ap", true, Some(40 * GB)),
                    sample("task-c", "ap", true, None),
                    sample("task-d", "other", true, Some(5 * GB)),
                ],
            ),
            None,
        );
        assert_eq!(v.expected_by_repo.get("ap"), Some(&(100 * GB)));
        assert_eq!(v.running_growth_bytes, 60 * GB + 100 * GB);
        assert_eq!(need_bytes(&v, "task-new", "ap"), 100 * GB);
        assert_eq!(need_bytes(&v, "task-b", "ap"), 60 * GB);
        assert_eq!(need_bytes(&v, "task-a", "ap"), 0);
    }

    fn owner_row(
        owner: &str,
        kind: &str,
        class: &str,
        reason: &str,
        target: bool,
        size: u64,
    ) -> ScratchOwnerView {
        ScratchOwnerView {
            owner: owner.into(),
            kind: kind.into(),
            class: class.into(),
            reason: reason.into(),
            has_target: target,
            size_bytes: Some(size),
            estimated_bytes: size,
            measured_at: None,
            lease_mtime: None,
            repo_key: Some("ap".into()),
            base_commit: None,
            adopted_from: None,
            work_unit_key: None,
        }
    }

    /// 不変条件（attempt 3 の review の申し送り）: repo の見込みは target が pool にある owner だけから決まり（GC の推定と
    /// 同じ入力）、増分は run か review の検査が今書いている owner だけが持つ。release-build の lease・まだ始まらない WU・
    /// target の消えた lease の記録は数えない。上の試験は `Sample` を直接与えるので、走査の行からの絞り込みを確かめない。
    #[test]
    fn samples_count_present_targets_and_grow_only_where_a_run_writes_now() {
        let rows = vec![
            owner_row("release-abc", "release", "p0", "lease live", true, 60 * GB),
            owner_row("task-gone", "task", "p3", "task done", false, 150 * GB),
            owner_row("task-p3", "task", "seed", "task done", true, 90 * GB),
            owner_row("task-r", "task", "p0", "task running", true, 40 * GB),
            owner_row("task-v", "task", "p0", "task reviewing", true, 70 * GB),
            owner_row(
                "task-w/wu-q",
                "work_unit",
                "p0",
                "work unit queued",
                true,
                10 * GB,
            ),
            owner_row(
                "task-w/wu-r",
                "work_unit",
                "p0",
                "work unit running",
                true,
                20 * GB,
            ),
        ];
        let samples = samples_from_owners(&rows);
        assert!(samples.iter().all(|s| s.owner != "task-gone"));
        // target をまだ写している running の owner（lease だけある）は、大きさ不明の running として数える。
        let copying = samples_from_owners(&[owner_row(
            "task-seeding",
            "task",
            "p0",
            "task running",
            false,
            150 * GB,
        )]);
        assert_eq!(copying.len(), 1);
        assert!(copying[0].running && copying[0].size_bytes.is_none());
        let v = forecast(&inputs(100 * GB, [samples.clone(), copying].concat()), None);
        let seeding = v.owners.iter().find(|o| o.owner == "task-seeding");
        assert_eq!(seeding.map(|o| o.growth_bytes), Some(90 * GB));
        let v = forecast(&inputs(100 * GB, samples), None);
        assert_eq!(v.expected_by_repo.get("ap"), Some(&(90 * GB)));
        let growing: Vec<&str> = v
            .owners
            .iter()
            .filter(|o| o.growth_bytes > 0)
            .map(|o| o.owner.as_str())
            .collect();
        assert_eq!(
            growing,
            vec!["release-abc", "task-r", "task-v", "task-w/wu-r"]
        );
        // lease の生きた release-build（target 60 GB）は次の build で見込み（90 GB）まで育つ増分 30 GB を持つ。
        assert_eq!(v.running_growth_bytes, (30 + 50 + 20 + 70) * GB);
    }

    /// 不変条件: 係数は増分と見込みにだけ掛かる（disk 上の量に直す）。
    #[test]
    fn the_ratio_scales_needs_and_growth() {
        let mut i = inputs(
            100 * GB,
            vec![
                sample("task-a", "ap", false, Some(100 * GB)),
                sample("task-b", "ap", true, Some(60 * GB)),
            ],
        );
        i.ratio = 0.5;
        let v = forecast(&i, None);
        assert_eq!(v.running_growth_bytes, 20 * GB);
        assert_eq!(need_bytes(&v, "task-new", "ap"), 50 * GB);
    }

    #[test]
    fn decide_holds_when_the_forecast_crosses_the_reserve_and_never_blocks_the_only_run() {
        let v = forecast(
            &inputs(
                150 * GB,
                vec![
                    sample("task-a", "ap", false, Some(100 * GB)),
                    sample("task-b", "ap", true, Some(80 * GB)),
                ],
            ),
            None,
        );
        // 150 − 20 − 0 − 100 = 30 ≥ 30: 通す。2 本目は 30 − 100 < 30: 待つ。
        assert_eq!(
            decide(&v, "task-x", "ap", 0, 1),
            Decision::Admit {
                need_bytes: 100 * GB
            }
        );
        assert!(matches!(
            decide(&v, "task-y", "ap", 100 * GB, 1),
            Decision::Hold { .. }
        ));
        // 走っている run が 0 なら通す。
        assert!(matches!(
            decide(&v, "task-y", "ap", 100 * GB, 0),
            Decision::Admit { .. }
        ));
        // 見込みの無い repo は判定しない。
        assert_eq!(decide(&v, "task-z", "unknown", 0, 1), Decision::Skip);
    }

    /// 不変条件（D4）: daemon の status の `held` は自分の Gate のメモリだけから来る。別の Gate（同じ pool を見る verify の
    /// daemon）が空の pass を回しても変わらない。旧い試験 `held_runs_round_trip_through_the_pool_file` は pool の
    /// `admission.json`（廃止した共有の写し）を確かめていたので、この試験に置き換えた。
    #[test]
    fn the_daemon_status_carries_its_own_held_runs_from_memory() {
        let v = forecast(
            &inputs(10 * GB, vec![sample("task-a", "ap", true, Some(50 * GB))]),
            None,
        );
        assert_eq!(
            v.held, None,
            "a pool scan alone does not know the held runs"
        );
        let mut status = crate::scratch_gc::disabled_status(
            &task_worker::scratch::ScratchSettings::default(),
            SystemTime::UNIX_EPOCH,
        );
        status.admission = v.clone();
        let (live, verify) = (Gate::default(), Gate::default());
        assert!(live.hold(&v, "t1", None, "ap", 1, SystemTime::UNIX_EPOCH));
        live.begin_pass();
        verify.begin_pass();
        let held = live.with_held(Some(&status)).and_then(|s| s.admission.held);
        let owners: Vec<String> = held
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.owner)
            .collect();
        assert_eq!(owners, vec!["task-t1".to_string()]);
        assert_eq!(
            verify
                .with_held(Some(&status))
                .and_then(|s| s.admission.held),
            Some(Vec::new())
        );
        assert_eq!(live.with_held(None), None);
        // 次の pass で判定に来なければ消える。
        live.begin_pass();
        assert!(live.held_views().is_empty());
    }
}
