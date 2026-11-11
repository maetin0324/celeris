//! ADR 2026-11-11-disk-aware-dispatch: dispatch の前の空きの見込み。
//!
//! - D1: `空き − Σ 走っている target の増分 − 今の pass で通した run の見込み − この run の見込み ≥ 予備` でなければ、
//!   新しい target を作る run を始めず `ready` のまま待たせる。repo の見込みは同じ `repo_key` の owner の測った大きさの
//!   最大（seed は数えない）。worker run が 1 本も走っていなければ 1 本は通す（詰まらない）。
//! - D2: 予備（空きの下限）は filesystem の容量 × `[dispatch] min_free_disk_pct`（[`min_free_bytes`]）。
//! - D4: 見込みは `ScratchStatus.admission`（[`forecast`]）。待たせている run は [`Gate`] のメモリが正本で、変わったら pool
//!   の `admission.json` に書き出す（`celerisctl scratch status` は [`read_held`] で読む）。
//!
//! dispatcher の module の外に置く（Dispatcher の field・method を増やさない）。**LLM は呼ばない。** 判定は注入できる
//! 値（[`Inputs`]）だけで決まる。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use task_ops::daemon::{
    ScratchAdmissionView, ScratchHeldRunView, ScratchOwnerForecastView, ScratchOwnerView,
};

/// pool の根に daemon が書き出す、待たせている run の一覧。
pub const HELD_FILE: &str = "admission.json";

/// D2: filesystem の容量 `total` に対する空きの下限（byte）。`pct` が 0 以下なら 0（無効）。
pub fn min_free_bytes(total: u64, pct: f64) -> u64 {
    if pct <= 0.0 || !pct.is_finite() {
        return 0;
    }
    ((total as f64) * pct / 100.0).ceil() as u64
}

/// D2: `paths` のうち空きが下限（容量 × `pct`）を割る最初のものの理由（filesystem が読めなければそれも理由。
/// 安全側に倒して run を始めない）。
pub fn find_low_disk(paths: &[std::path::PathBuf], pct: f64) -> Option<String> {
    paths
        .iter()
        .find_map(|path| match crate::scratch_gc::fs_stats(path) {
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
    /// class が `p0`（走っている）。
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

/// scratch の走査の行から材料を作る（seed・野良・repo の分からない owner は数えない）。
pub fn samples_from_owners(owners: &[ScratchOwnerView]) -> Vec<Sample> {
    owners
        .iter()
        .filter(|o| o.class != "seed" && o.kind != "stray")
        .filter_map(|o| {
            Some(Sample {
                owner: o.owner.clone(),
                repo_key: o.repo_key.clone()?,
                running: o.class == "p0",
                size_bytes: o.size_bytes,
            })
        })
        .collect()
}

fn scaled(bytes: u64, ratio: f64) -> u64 {
    ((bytes as f64) * ratio).ceil() as u64
}

/// D1/D4: 見込みを組む。`held` は待たせている run（[`read_held`] か [`Gate::held_views`]）。
pub fn forecast(inputs: &Inputs, held: Vec<ScratchHeldRunView>) -> ScratchAdmissionView {
    let mut expected_by_repo: BTreeMap<String, u64> = BTreeMap::new();
    for s in &inputs.samples {
        let e = expected_by_repo.entry(s.repo_key.clone()).or_insert(0);
        *e = (*e).max(s.size_bytes.unwrap_or(0));
    }
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

/// dispatcher の待たせている run（`ScratchState.admission`）。pass ごとに [`Gate::begin_pass`] を呼ぶ。
#[derive(Default)]
pub struct Gate {
    pub probe: Option<Probe>,
    holds: BTreeMap<HoldKey, ScratchHeldRunView>,
    seen: BTreeSet<HoldKey>,
    admitted_bytes: u64,
    published: Option<Vec<ScratchHeldRunView>>,
    /// 待ち始めの記録（log）を残した回数（試験が 1 回だけかを確かめる）。
    pub notes: u64,
}

impl Gate {
    /// pass の始め: 前の pass で判定に来なかった run（始まった・`ready` でなくなった）の記録を消し、通した量を 0 に
    /// 戻す。一覧が変わっていれば `pool_root` の `admission.json` に書き出す。
    pub fn begin_pass(&mut self, pool_root: Option<&Path>) {
        let seen = std::mem::take(&mut self.seen);
        self.holds.retain(|k, _| seen.contains(k));
        self.admitted_bytes = 0;
        let held = self.held_views();
        if let Some(root) = pool_root
            && self.published.as_ref() != Some(&held)
        {
            match write_held(root, &held) {
                Ok(()) => self.published = Some(held),
                Err(e) => tracing::warn!(error = %e, "disk forecast: cannot write the held runs"),
            }
        }
    }

    /// 待たせている run（task・WU の順）。
    pub fn held_views(&self) -> Vec<ScratchHeldRunView> {
        self.holds.values().cloned().collect()
    }

    /// 判定に使う見込み。`probe` があればそれ、無ければ scratch の走査の結果（scratch が無効なら `None`）。
    pub fn forecast(
        &self,
        view: Option<&task_ops::daemon::ScratchStatus>,
    ) -> Option<ScratchAdmissionView> {
        match &self.probe {
            Some(probe) => Some(forecast(&probe(), Vec::new())),
            None => view.filter(|v| v.enabled).map(|v| v.admission.clone()),
        }
    }

    /// D1: run（task・WU）を待たせるなら `true`。待ち始めに 1 回だけ log を残す（受信箱・通知には出さない）。
    pub fn hold(
        &mut self,
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
        match decide(view, &owner, repo_key, self.admitted_bytes, runs_in_flight) {
            Decision::Skip => {
                self.holds.remove(&key);
                false
            }
            Decision::Admit { need_bytes } => {
                self.holds.remove(&key);
                self.admitted_bytes = self.admitted_bytes.saturating_add(need_bytes);
                false
            }
            Decision::Hold { need_bytes, reason } => {
                self.seen.insert(key.clone());
                let since = self
                    .holds
                    .get(&key)
                    .map(|h| h.since.clone())
                    .unwrap_or_else(|| {
                        tracing::info!(task_id, work_unit_id, %owner, %reason, "disk forecast: run held");
                        self.notes += 1;
                        task_worker::scratch::rfc3339(now)
                    });
                self.holds.insert(
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

fn write_held(root: &Path, held: &[ScratchHeldRunView]) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(held).map_err(std::io::Error::other)?;
    let tmp = root.join(format!(".{HELD_FILE}.tmp"));
    std::fs::create_dir_all(root)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, root.join(HELD_FILE))
}

/// daemon が書き出した待たせている run（無い・読めなければ空）。
pub fn read_held(root: &Path) -> Vec<ScratchHeldRunView> {
    std::fs::read(root.join(HELD_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
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
            Vec::new(),
        );
        assert_eq!(v.expected_by_repo.get("ap"), Some(&(100 * GB)));
        assert_eq!(v.running_growth_bytes, 60 * GB + 100 * GB);
        assert_eq!(need_bytes(&v, "task-new", "ap"), 100 * GB);
        assert_eq!(need_bytes(&v, "task-b", "ap"), 60 * GB);
        assert_eq!(need_bytes(&v, "task-a", "ap"), 0);
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
        let v = forecast(&i, Vec::new());
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
            Vec::new(),
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

    #[test]
    fn held_runs_round_trip_through_the_pool_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_held(dir.path()).is_empty());
        let v = forecast(
            &inputs(10 * GB, vec![sample("task-a", "ap", true, Some(50 * GB))]),
            Vec::new(),
        );
        let mut gate = Gate::default();
        assert!(gate.hold(&v, "t1", None, "ap", 1, SystemTime::UNIX_EPOCH));
        gate.begin_pass(Some(dir.path()));
        let held = read_held(dir.path());
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].owner, "task-t1");
        // 次の pass で判定に来なければ消える。
        gate.begin_pass(Some(dir.path()));
        assert!(read_held(dir.path()).is_empty());
    }
}
