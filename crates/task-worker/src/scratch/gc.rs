//! Scratch pool の分類、回収順、測定。

use super::*;

// ---------------------------------------------------------------------------
// 分類（D2）
// ---------------------------------------------------------------------------

/// ADR-0075 D2 の分類。`Legacy` は旧 `build_cache_dir/cargo/*` などの pool の外の target、`Stray` は lease の無い
/// （または owner として読めない）ディレクトリ。
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// P0 pinned: 絶対に消さない。
    Pinned,
    /// P1 waiting。
    Waiting,
    /// P2 retry。
    Retry,
    /// P3 completed（回収可能）。
    Completed,
    Legacy,
    Stray,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Pinned => "p0",
            Class::Waiting => "p1",
            Class::Retry => "p2",
            Class::Completed => "p3",
            Class::Legacy => "legacy",
            Class::Stray => "stray",
        }
    }
}

/// DB の状態の読み出し（dispatcher は store、`celerisctl` は読み取り専用の store か「DB 無し」）。
pub trait StatusLookup {
    fn task_status(&self, task_id: &str) -> Option<Status>;
    fn work_unit_status(&self, work_unit_id: &str) -> Option<WorkUnitStatus>;
}

/// DB を持たない呼び出し（DB が無い・開けない）。daemon 由来の owner は「DB に行が無い」＝ P3 にはせず、
/// **P0 扱い**にする（見えないものを消さない）。
pub struct NoDb;
impl StatusLookup for NoDb {
    fn task_status(&self, _: &str) -> Option<Status> {
        None
    }
    fn work_unit_status(&self, _: &str) -> Option<WorkUnitStatus> {
        None
    }
}

/// `dyn TaskStore` による lookup。
pub struct StoreLookup<'a>(pub &'a dyn task_core::store::TaskStore);
impl StatusLookup for StoreLookup<'_> {
    fn task_status(&self, task_id: &str) -> Option<Status> {
        let id: task_core::TaskId = task_id.parse().ok()?;
        self.0.get(id).ok().flatten().map(|t| t.status)
    }
    fn work_unit_status(&self, work_unit_id: &str) -> Option<WorkUnitStatus> {
        self.0
            .work_unit_get(work_unit_id)
            .ok()
            .flatten()
            .map(|w| w.status)
    }
}

fn age(now: SystemTime, t: SystemTime) -> u64 {
    now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0)
}

/// ADR-0075 D2 の表。`lease_mtime` は `lease.json` の mtime（lease が無ければディレクトリの mtime）。
/// `has_db = false` のとき（`NoDb`）、daemon 由来の owner で DB に行が見つからないものは P0（消さない）。
pub fn classify(
    owner: &Owner,
    lease: Option<&Lease>,
    lease_mtime: SystemTime,
    now: SystemTime,
    s: &ScratchSettings,
    lookup: &dyn StatusLookup,
    has_db: bool,
) -> (Class, &'static str) {
    let a = age(now, lease_mtime);
    if lease.is_none() {
        return if a >= IDLE_SECS {
            (Class::Stray, "no lease.json (idle >= 1h)")
        } else {
            (Class::Pinned, "no lease.json yet (being created)")
        };
    }
    let waiting = |why_keep: &'static str, why_expired: &'static str| {
        if a < s.waiting_keep_secs {
            (Class::Waiting, why_keep)
        } else {
            (Class::Completed, why_expired)
        }
    };
    let retry = || {
        if a < s.failed_keep_secs {
            (Class::Retry, "task failed (retry possible)")
        } else {
            (Class::Completed, "task failed (retry window passed)")
        }
    };
    match owner {
        Owner::Task { task_id } => match lookup.task_status(task_id) {
            Some(Status::Running) => (Class::Pinned, "task running"),
            Some(Status::Reviewing) => (Class::Pinned, "task reviewing"),
            Some(Status::Draft | Status::Ready | Status::Blocked) => {
                waiting("task waiting", "task waiting (keep window passed)")
            }
            Some(Status::Failed) => retry(),
            Some(Status::Done) => (Class::Completed, "task done"),
            Some(Status::Cancelled) => (Class::Completed, "task cancelled"),
            None if has_db => (Class::Completed, "task not in db"),
            None => (Class::Pinned, "no db (unknown task)"),
        },
        Owner::WorkUnit {
            task_id,
            work_unit_id,
        } => {
            let task = lookup.task_status(task_id);
            let wu = lookup.work_unit_status(work_unit_id);
            match (task, wu) {
                (None, _) | (_, None) if !has_db => (Class::Pinned, "no db (unknown work unit)"),
                (None, _) => (Class::Completed, "task not in db"),
                (_, None) => (Class::Completed, "work unit not in db"),
                (Some(Status::Failed), Some(_)) => retry(),
                (Some(t), Some(_)) if t.is_terminal() => (Class::Completed, "task finished"),
                (Some(_), Some(w)) => match w {
                    WorkUnitStatus::Pending
                    | WorkUnitStatus::Ready
                    | WorkUnitStatus::Running
                    | WorkUnitStatus::NeedsContinuation => (Class::Pinned, "work unit in progress"),
                    WorkUnitStatus::Blocked | WorkUnitStatus::Failed => waiting(
                        "work unit failed/blocked (replan may retry)",
                        "work unit failed/blocked (keep window passed)",
                    ),
                    WorkUnitStatus::Done => (Class::Completed, "work unit done"),
                    WorkUnitStatus::Superseded => (Class::Completed, "work unit superseded"),
                    WorkUnitStatus::Cancelled => (Class::Completed, "work unit cancelled"),
                },
            }
        }
        Owner::Release { .. } | Owner::Agent { .. } => {
            if lease.and_then(|l| l.released_at.as_ref()).is_some() {
                (Class::Completed, "lease released")
            } else if a
                >= lease
                    .and_then(|l| l.ttl_secs)
                    .unwrap_or(s.external_lease_ttl_secs)
            {
                (Class::Completed, "lease expired")
            } else {
                (Class::Pinned, "lease live")
            }
        }
    }
}

/// ADR-0075 D7: pool の外の旧い target（`build_cache_dir/cargo/*`、`release-build/.cargo-target`）の分類。
/// 最終書き込み（測定スレッドが木を辿って得た最新の mtime）が 1 時間以上前のときだけ回収できる（今動いている
/// 実装エージェントが手で指している target を途中で消さない）。未測定は消さない。
pub fn legacy_class(
    measured_last_write: Option<SystemTime>,
    now: SystemTime,
) -> (Class, &'static str) {
    match measured_last_write {
        None => (Class::Pinned, "legacy: not measured yet"),
        Some(t) if age(now, t) >= IDLE_SECS => (Class::Legacy, "legacy: idle >= 1h"),
        Some(_) => (Class::Pinned, "legacy: written within 1h"),
    }
}

// ---------------------------------------------------------------------------
// plan_gc（D2）
// ---------------------------------------------------------------------------

/// `plan_gc` の 1 行（owner の `target/`、legacy のディレクトリ、野良のディレクトリ）。
#[derive(Debug, Clone, PartialEq)]
pub struct GcEntry {
    /// 表示と同順位の決定に使う（owner の文字列、legacy / 野良はパス）。
    pub id: String,
    /// rename して消すディレクトリ。
    pub path: PathBuf,
    pub owner: Option<Owner>,
    pub repo_key: Option<String>,
    pub class: Class,
    pub reason: String,
    /// LRU の基準（lease の mtime。legacy は測定した最終書き込み）。
    pub last_write: SystemTime,
    /// 測定したサイズ（未測定は `None`）。
    pub size_bytes: Option<u64>,
    /// pool（`targets/`）の中か（legacy は pool の外）。
    pub in_pool: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pressure {
    None,
    /// `targets` が `targets_max × high_watermark` を超えた。
    HighWatermark,
    /// filesystem の空きが `min_free_disk_mb × 2` を下回った。
    LowDisk,
    /// 空きが `min_free_disk_mb` を下回った（dispatch の直前の緊急 GC）。
    Emergency,
}

impl Pressure {
    pub fn as_str(self) -> &'static str {
        match self {
            Pressure::None => "none",
            Pressure::HighWatermark => "high_watermark",
            Pressure::LowDisk => "low_disk",
            Pressure::Emergency => "emergency",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcParams {
    pub now: SystemTime,
    pub targets_max_bytes: u64,
    pub high_watermark: f64,
    pub low_watermark: f64,
    pub min_free_bytes: u64,
    /// filesystem の空き（statvfs。不明なら `None` で空きの条件は見ない）。
    pub fs_free_bytes: Option<u64>,
    /// 緊急モード（空き < `min_free_disk_mb`）。目標 = 空き `min_free × 3`。
    pub emergency: bool,
    pub completed_grace_secs: u64,
    pub warm_seeds_per_repo: usize,
    pub max_per_tick: usize,
    /// その repo を使う非終端の owner / Task がある repo_key（seed を残す条件）。
    pub active_repo_keys: BTreeSet<String>,
}

impl GcParams {
    pub fn from_settings(
        s: &ScratchSettings,
        now: SystemTime,
        min_free_bytes: u64,
        fs_free_bytes: Option<u64>,
        emergency: bool,
        active_repo_keys: BTreeSet<String>,
    ) -> Self {
        Self {
            now,
            targets_max_bytes: s.targets_max_bytes,
            high_watermark: s.high_watermark,
            low_watermark: s.low_watermark,
            min_free_bytes,
            fs_free_bytes,
            emergency,
            completed_grace_secs: s.completed_grace_secs,
            warm_seeds_per_repo: s.warm_seeds_per_repo,
            max_per_tick: s.gc_max_per_tick,
            active_repo_keys,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcPick {
    pub id: String,
    pub path: PathBuf,
    pub owner: Option<Owner>,
    pub class: Class,
    pub seed: bool,
    pub estimated_bytes: u64,
    /// `immediate`（watermark に関係なく即回収）か `pressure`（目標に届くまで）。
    pub why: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcPlan {
    pub pressure: Pressure,
    /// pool（`targets/`）の推定使用量。
    pub used_bytes: u64,
    pub pinned_bytes: u64,
    /// 目標に届くまでに pool から減らす量（watermark）。
    pub pool_need_bytes: u64,
    /// 目標に届くまでに filesystem で空ける量（空き）。
    pub disk_need_bytes: u64,
    pub selected: Vec<GcPick>,
    /// seed に選ばれた entry の id。
    pub seeds: BTreeSet<String>,
    /// entry ごとの推定サイズ（未測定は同じ repo の最大値）。
    pub estimated: BTreeMap<String, u64>,
}

/// 未測定の entry の推定（D2: 0 とみなさず同じ repo の最大値。repo に測定済みが無ければ全体の最大値）。
fn estimates(entries: &[GcEntry]) -> BTreeMap<String, u64> {
    let mut repo_max: BTreeMap<&str, u64> = BTreeMap::new();
    let mut all_max = 0u64;
    for e in entries {
        if let Some(sz) = e.size_bytes {
            all_max = all_max.max(sz);
            if let Some(k) = &e.repo_key {
                let m = repo_max.entry(k.as_str()).or_insert(0);
                *m = (*m).max(sz);
            }
        }
    }
    entries
        .iter()
        .map(|e| {
            let v = e.size_bytes.unwrap_or_else(|| {
                e.repo_key
                    .as_deref()
                    .and_then(|k| repo_max.get(k).copied())
                    .unwrap_or(all_max)
            });
            (e.id.clone(), v)
        })
        .collect()
}

/// ADR-0075 D2: 削除順を決める純粋関数。**P0 は決して選ばない**。① legacy / 野良 → ② P3（LRU）→ ③ seed（古い順）→
/// ④ P2（古い順）→ ⑤ P1（古い順）。同順位は id の文字列順。watermark / 空きの目標に届いたら止まる。目標が無くても
/// legacy・野良、P3 の WU・release（seed を除く）、猶予の過ぎた P3 の Task は即回収する。1 回に最大 `max_per_tick` 件。
pub fn plan_gc(entries: &[GcEntry], p: &GcParams) -> GcPlan {
    let estimated = estimates(entries);
    let est = |e: &GcEntry| estimated.get(&e.id).copied().unwrap_or(0);
    let used: u64 = entries.iter().filter(|e| e.in_pool).map(est).sum();
    let pinned: u64 = entries
        .iter()
        .filter(|e| e.in_pool && e.class == Class::Pinned)
        .map(est)
        .sum();

    // seed: P3 の owner のうち、repo ごとに最新の `warm_seeds_per_repo` 個（その repo が使われているときだけ）。
    let mut seeds = BTreeSet::new();
    if p.warm_seeds_per_repo > 0 {
        let mut by_repo: BTreeMap<&str, Vec<&GcEntry>> = BTreeMap::new();
        for e in entries
            .iter()
            .filter(|e| e.class == Class::Completed && e.owner.is_some() && e.in_pool)
        {
            if let Some(k) = e.repo_key.as_deref()
                && p.active_repo_keys.contains(k)
            {
                by_repo.entry(k).or_default().push(e);
            }
        }
        for list in by_repo.values_mut() {
            list.sort_by(|a, b| {
                b.last_write
                    .cmp(&a.last_write)
                    .then_with(|| a.id.cmp(&b.id))
            });
            for e in list.iter().take(p.warm_seeds_per_repo) {
                seeds.insert(e.id.clone());
            }
        }
    }

    let low_limit = (p.targets_max_bytes as f64 * p.low_watermark) as u64;
    let high_limit = (p.targets_max_bytes as f64 * p.high_watermark) as u64;
    let mut pressure = Pressure::None;
    let mut pool_need = 0u64;
    if used > high_limit {
        pressure = Pressure::HighWatermark;
        pool_need = used - low_limit;
    }
    let mut disk_need = 0u64;
    if let Some(free) = p.fs_free_bytes {
        let goal = p.min_free_bytes.saturating_mul(3);
        if p.emergency || free < p.min_free_bytes {
            pressure = Pressure::Emergency;
            disk_need = goal.saturating_sub(free);
        } else if free < p.min_free_bytes.saturating_mul(2) {
            if pressure == Pressure::None {
                pressure = Pressure::LowDisk;
            }
            disk_need = goal.saturating_sub(free);
        }
    } else if p.emergency {
        pressure = Pressure::Emergency;
    }

    let rank = |e: &GcEntry| -> Option<u8> {
        match e.class {
            Class::Pinned => None,
            Class::Legacy | Class::Stray => Some(1),
            Class::Completed if seeds.contains(&e.id) => Some(3),
            Class::Completed => Some(2),
            Class::Retry => Some(4),
            Class::Waiting => Some(5),
        }
    };
    let mut ordered: Vec<(u8, &GcEntry)> = entries
        .iter()
        .filter_map(|e| rank(e).map(|r| (r, e)))
        .collect();
    ordered.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then_with(|| a.last_write.cmp(&b.last_write))
            .then_with(|| a.id.cmp(&b.id))
    });
    let immediate = |r: u8, e: &GcEntry| -> bool {
        match r {
            1 => true,
            2 => match &e.owner {
                Some(Owner::WorkUnit { .. } | Owner::Release { .. }) => true,
                Some(Owner::Task { .. }) => age(p.now, e.last_write) >= p.completed_grace_secs,
                _ => false,
            },
            _ => false,
        }
    };

    let mut selected = Vec::new();
    let (mut pool_freed, mut disk_freed) = (0u64, 0u64);
    let pick = |e: &GcEntry, r: u8, why: &'static str| GcPick {
        id: e.id.clone(),
        path: e.path.clone(),
        owner: e.owner.clone(),
        class: e.class,
        seed: r == 3,
        estimated_bytes: est(e),
        why,
    };
    let mut done_ids = BTreeSet::new();
    for (r, e) in &ordered {
        if selected.len() >= p.max_per_tick {
            break;
        }
        if immediate(*r, e) {
            let sz = est(e);
            if e.in_pool {
                pool_freed += sz;
            }
            disk_freed += sz;
            selected.push(pick(e, *r, "immediate"));
            done_ids.insert(e.id.clone());
        }
    }
    for (r, e) in &ordered {
        if selected.len() >= p.max_per_tick {
            break;
        }
        if pool_freed >= pool_need && disk_freed >= disk_need {
            break;
        }
        if done_ids.contains(&e.id) {
            continue;
        }
        let sz = est(e);
        if e.in_pool {
            pool_freed += sz;
        }
        disk_freed += sz;
        selected.push(pick(e, *r, "pressure"));
    }
    GcPlan {
        pressure,
        used_bytes: used,
        pinned_bytes: pinned,
        pool_need_bytes: pool_need,
        disk_need_bytes: disk_need,
        selected,
        seeds,
        estimated,
    }
}

/// `.deleting-<flat>-<nanos>` の名前（同じ親の中へ rename する。filesystem をまたがない）。
pub fn deleting_name(flat: &str, now: SystemTime) -> String {
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    format!("{DELETING_PREFIX}{flat}-{nanos}")
}

/// `path` 以下の実ファイルの使用量（`st_blocks × 512`）と最新の mtime。symlink の先は辿らない。
/// 重い（大きな target では数秒〜）ので tick の中では呼ばない（測定スレッドだけ）。
pub fn measure_tree(path: &Path) -> io::Result<(u64, SystemTime)> {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0u64;
    let mut latest = UNIX_EPOCH;
    let mut pending = vec![path.to_path_buf()];
    while let Some(p) = pending.pop() {
        let meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        total = total.saturating_add(meta.blocks().saturating_mul(512));
        if let Ok(t) = meta.modified() {
            latest = latest.max(t);
        }
        if meta.file_type().is_dir()
            && let Ok(rd) = std::fs::read_dir(&p)
        {
            for e in rd.flatten() {
                pending.push(e.path());
            }
        }
    }
    Ok((total, latest))
}
