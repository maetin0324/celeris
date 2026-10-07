//! ADR 2026-10-07-build-tmp-hygiene D4: ディスク使用率の監視（dispatcher の tick の段 `disk_watch`）。
//!
//! config `[[maintenance.disk_watch]]` の path（既定 `/`・`/local`・`/tmp`）の使用率を 60 秒ごとに
//! `statvfs`（注入できる [`DiskProbe`]）で測り、`warn_pct`（既定 80%）以上で通知（ADR-0133 `NoticeKind::Disk`、
//! group_key `disk:<path>`）、`critical_pct`（既定 95%）以上で受信箱（`disk_full`。`disk_watch_state` の
//! `critical` の行から派生）に出す。**LLM は呼ばない。** run は止めない（止めるのは ADR-0074 の
//! `min_free_disk_mb`）。
//!
//! - 状態は SQLite の `disk_watch_state`（[`task_core::DiskWatchStore`]）。daemon のメモリに持つのは
//!   測る間隔の時刻だけ。
//! - 通知・受信箱は **level が上がったとき**だけ作る。`critical` への遷移は受信箱だけ（ADR-0133 D1.5「1 出来事 →
//!   ちょうど 1 経路」）。同じ `warn` が続くあいだは 24 時間ごとに 1 回だけ再通知（未読の束の `count` が増える）。
//! - level を下げるのは、しきい値より [`HYSTERESIS_PCT`] ポイント下回ったとき。
//! - 存在しない path は `unavailable` として 1 回だけ記録する（続くあいだは書かない）。

use std::path::{Path, PathBuf};

use task_core::feed::{NoticeEvent, NoticeKind, NoticeRecordOutcome, NoticeTarget};
use task_core::store::StoreError;
use task_core::{DiskLevel, DiskWatchState, DiskWatchStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 既定の警告しきい値（%）。
pub const DEFAULT_WARN_PCT: f64 = 80.0;
/// 既定の critical しきい値（%）。
pub const DEFAULT_CRITICAL_PCT: f64 = 95.0;
/// 既定の対象 path。
pub const DEFAULT_PATHS: [&str; 3] = ["/", "/local", "/tmp"];
/// level を下げるのに要る、しきい値からの下回り幅（ポイント）。
pub const HYSTERESIS_PCT: f64 = 5.0;
/// 測る間隔（秒）。
pub const INTERVAL_SECS: i64 = 60;
/// 同じ level が続くときの再通知の間隔（秒。24 時間）。
pub const RENOTIFY_SECS: i64 = 24 * 3600;
/// `last_pct` だけの変化を書き込む幅（ポイント）。これより小さい揺れは DB に書かない。
const PCT_WRITE_STEP: f64 = 1.0;

/// 監視する 1 つの path としきい値。
#[derive(Debug, Clone, PartialEq)]
pub struct DiskWatchEntry {
    pub path: PathBuf,
    pub warn_pct: f64,
    pub critical_pct: f64,
}

impl DiskWatchEntry {
    /// 既定の 3 つ（`/`・`/local`・`/tmp`、80% / 95%）。
    pub fn defaults() -> Vec<Self> {
        DEFAULT_PATHS
            .iter()
            .map(|p| Self {
                path: PathBuf::from(p),
                warn_pct: DEFAULT_WARN_PCT,
                critical_pct: DEFAULT_CRITICAL_PCT,
            })
            .collect()
    }

    fn key(&self) -> String {
        self.path.display().to_string()
    }
}

/// `statvfs` のうち使用率の計算に使う 3 つ（単位は fragment。比しか使わないので揃っていればよい）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
}

impl DiskUsage {
    /// 使用率（%）`(blocks − bfree) / (blocks − bfree + bavail)`。`df` の Use% と同じ（root 予約分は
    /// 分母に入れない）。分母が 0 なら `None`。
    pub fn used_pct(&self) -> Option<f64> {
        let used = self.blocks.saturating_sub(self.bfree);
        let denom = used.saturating_add(self.bavail);
        (denom > 0).then(|| used as f64 * 100.0 / denom as f64)
    }
}

/// 使用率を測る口（試験は偽の値を返す実装を注入する）。
pub trait DiskProbe: Send {
    /// `path` の使用量。path が無い・測れないときは `Err`（理由の文）。
    fn usage(&self, path: &Path) -> Result<DiskUsage, String>;
}

/// 本物の `statvfs(2)`。path そのものを測る（祖先へは辿らない。無ければ `unavailable`）。
pub struct StatvfsProbe;

impl DiskProbe for StatvfsProbe {
    fn usage(&self, path: &Path) -> Result<DiskUsage, String> {
        let stat =
            nix::sys::statvfs::statvfs(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(DiskUsage {
            blocks: stat.blocks(),
            bfree: stat.blocks_free(),
            bavail: stat.blocks_available(),
        })
    }
}

/// `prev`（`Unavailable` は `Ok` として扱う）から使用率 `pct` での level。上げるのはしきい値以上で即時、
/// 下げるのはしきい値より [`HYSTERESIS_PCT`] 下回ったときだけ（1 回の測定で 2 段下がることもある）。
pub fn next_level(prev: DiskLevel, pct: f64, entry: &DiskWatchEntry) -> DiskLevel {
    let raw = if pct >= entry.critical_pct {
        DiskLevel::Critical
    } else if pct >= entry.warn_pct {
        DiskLevel::Warn
    } else {
        DiskLevel::Ok
    };
    let prev = if prev == DiskLevel::Unavailable {
        DiskLevel::Ok
    } else {
        prev
    };
    if raw >= prev {
        return raw;
    }
    let mut level = prev;
    while level > raw {
        let (threshold, lower) = match level {
            DiskLevel::Critical => (entry.critical_pct, DiskLevel::Warn),
            DiskLevel::Warn => (entry.warn_pct, DiskLevel::Ok),
            _ => break,
        };
        if pct < threshold - HYSTERESIS_PCT {
            level = lower;
        } else {
            break;
        }
    }
    level
}

/// 1 つの path の 1 回の測定で起きたこと。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskWatchAction {
    /// 何も起きない（同じ level が続く・測れないまま）。
    Unchanged,
    /// `warn` に上がった（通知）。
    Warned,
    /// `warn` が 24 時間続いた（再通知。束の `count` が増える）。
    Renotified,
    /// `critical` に上がった（受信箱。通知は出さない）。
    Critical,
    /// level が下がった（`critical` から下がれば受信箱の項目が消える）。
    Lowered,
    /// path が無い・測れない（1 回だけ記録）。
    Unavailable,
}

/// [`evaluate`] の結果: 書き込む状態（`None` なら書かない）と、同じ transaction で記録する通知。
#[derive(Debug, Clone, PartialEq)]
pub struct DiskWatchStep {
    pub path: String,
    pub level: DiskLevel,
    pub pct: Option<f64>,
    pub action: DiskWatchAction,
    pub write: Option<DiskWatchState>,
    pub notice: Option<NoticeEvent>,
}

fn ts(t: OffsetDateTime) -> String {
    t.format(&Rfc3339)
        .unwrap_or_else(|_| t.unix_timestamp().to_string())
}

fn warn_notice(
    entry: &DiskWatchEntry,
    pct: f64,
    since: OffsetDateTime,
    now: OffsetDateTime,
) -> NoticeEvent {
    let path = entry.key();
    let title = format!("ディスク使用率 {pct:.1}%: {path}");
    NoticeEvent {
        // 同じ遷移（同じ since）と同じ時刻の再通知は 1 回だけ数える。
        source_key: format!("disk:{path}:warn:{}:{}", ts(since), ts(now)),
        kind: NoticeKind::Disk,
        group_key: format!("disk:{path}"),
        summary: format!(
            "{path} の使用率が {pct:.1}% で警告のしきい値 {:.0}% を超えた（{:.0}% で受信箱）。",
            entry.warn_pct, entry.critical_pct
        ),
        title,
        project_id: None,
        task_id: None,
        target: Some(NoticeTarget {
            kind: "disk".into(),
            id: path,
        }),
        links: Vec::new(),
        at: now,
    }
}

/// 1 つの path の判定（純関数。時計と使用率は引数）。`prev` はその path の今の行（無ければ `ok` 扱い）。
pub fn evaluate(
    prev: Option<&DiskWatchState>,
    entry: &DiskWatchEntry,
    reading: Result<f64, String>,
    now: OffsetDateTime,
) -> DiskWatchStep {
    let path = entry.key();
    let prev_level = prev.map_or(DiskLevel::Ok, |p| p.level);
    let pct = match reading {
        Ok(pct) => pct,
        Err(_) => {
            let first = prev_level != DiskLevel::Unavailable;
            return DiskWatchStep {
                path: path.clone(),
                level: DiskLevel::Unavailable,
                pct: None,
                action: if first {
                    DiskWatchAction::Unavailable
                } else {
                    DiskWatchAction::Unchanged
                },
                write: first.then(|| DiskWatchState {
                    path,
                    level: DiskLevel::Unavailable,
                    since: now,
                    last_pct: None,
                    last_notified_at: prev.and_then(|p| p.last_notified_at),
                    updated_at: now,
                }),
                notice: None,
            };
        }
    };
    let base = if prev_level == DiskLevel::Unavailable {
        DiskLevel::Ok
    } else {
        prev_level
    };
    let level = next_level(base, pct, entry);
    let mut since = prev
        .filter(|_| level == prev_level)
        .map_or(now, |p| p.since);
    let mut last_notified_at = prev.and_then(|p| p.last_notified_at);
    let mut notice = None;
    let action = if level > base {
        since = now;
        if level == DiskLevel::Warn {
            notice = Some(warn_notice(entry, pct, since, now));
            last_notified_at = Some(now);
            DiskWatchAction::Warned
        } else {
            // critical への遷移は受信箱だけ（通知は出さない）。
            last_notified_at = Some(now);
            DiskWatchAction::Critical
        }
    } else if level < base {
        since = now;
        DiskWatchAction::Lowered
    } else if level == DiskLevel::Warn
        && last_notified_at.is_none_or(|t| (now - t).whole_seconds() >= RENOTIFY_SECS)
    {
        notice = Some(warn_notice(entry, pct, since, now));
        last_notified_at = Some(now);
        DiskWatchAction::Renotified
    } else {
        DiskWatchAction::Unchanged
    };
    let pct_moved = prev
        .and_then(|p| p.last_pct)
        .is_none_or(|last| (pct - last).abs() >= PCT_WRITE_STEP);
    let write =
        (prev.is_none() || level != prev_level || notice.is_some() || pct_moved).then(|| {
            DiskWatchState {
                path: path.clone(),
                level,
                since,
                last_pct: Some(pct),
                last_notified_at,
                updated_at: now,
            }
        });
    DiskWatchStep {
        path,
        level,
        pct: Some(pct),
        action,
        write,
        notice,
    }
}

/// ADR 2026-10-07-build-tmp-hygiene D1.5 / D4.3: target sweep が上限まで下げきれなかった
/// （`over_cap_unresolved`）ときの通知（group_key `disk:target_sweep`）。`over_roots` は上限を超えたままの
/// root と掃除後の大きさ。同じ task からは 1 回だけ数える。
pub fn target_sweep_notice(
    task_id: task_core::TaskId,
    over_roots: &[(String, u64)],
    max_bytes_per_root: u64,
    now: OffsetDateTime,
) -> NoticeEvent {
    const GIB: f64 = (1u64 << 30) as f64;
    let roots = over_roots
        .iter()
        .map(|(root, bytes)| format!("{root}（{:.1} GiB）", *bytes as f64 / GIB))
        .collect::<Vec<_>>()
        .join("、");
    let summary = format!(
        "共有 cargo target の掃除で上限 {:.0} GiB まで下げきれなかった: {}。build 中の profile は消していない。",
        max_bytes_per_root as f64 / GIB,
        if roots.is_empty() {
            "-".to_string()
        } else {
            roots
        }
    );
    NoticeEvent {
        source_key: format!("disk:target_sweep:{task_id}"),
        kind: NoticeKind::Disk,
        group_key: "disk:target_sweep".into(),
        title: "target sweep: 上限を超えたまま".into(),
        summary,
        project_id: None,
        task_id: Some(task_id.to_string()),
        target: Some(NoticeTarget {
            kind: "task".into(),
            id: task_id.to_string(),
        }),
        links: Vec::new(),
        at: now,
    }
}

/// 1 path の結果（tick の記録と試験の観測に使う）。
#[derive(Debug, Clone, PartialEq)]
pub struct DiskWatchOutcome {
    pub path: String,
    pub level: DiskLevel,
    pub pct: Option<f64>,
    pub action: DiskWatchAction,
    pub notice: Option<NoticeRecordOutcome>,
}

/// 全 entry を 1 回測って状態を進める（`statvfs` は軽いので同期）。書き込みは変化のあった path だけ。
pub fn run_disk_watch(
    store: &dyn DiskWatchStore,
    probe: &dyn DiskProbe,
    entries: &[DiskWatchEntry],
    now: OffsetDateTime,
) -> Result<Vec<DiskWatchOutcome>, StoreError> {
    let states = store.disk_watch_states()?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let key = entry.key();
        let prev = states.iter().find(|s| s.path == key);
        let reading = probe.usage(&entry.path).and_then(|u| {
            u.used_pct()
                .ok_or_else(|| format!("{key}: filesystem reports no blocks"))
        });
        if let Err(reason) = &reading
            && prev.is_none_or(|p| p.level != DiskLevel::Unavailable)
        {
            tracing::warn!(path = %key, %reason, "disk watch: path unavailable");
        }
        let step = evaluate(prev, entry, reading, now);
        let notice = match &step.write {
            Some(state) => store.disk_watch_apply(state, step.notice.as_ref())?,
            None => None,
        };
        match step.action {
            DiskWatchAction::Warned | DiskWatchAction::Renotified => {
                tracing::warn!(path = %key, pct = ?step.pct, "disk usage above warn threshold")
            }
            DiskWatchAction::Critical => {
                tracing::error!(path = %key, pct = ?step.pct, "disk usage above critical threshold")
            }
            DiskWatchAction::Lowered => {
                tracing::info!(path = %key, pct = ?step.pct, level = %step.level, "disk usage level lowered")
            }
            DiskWatchAction::Unavailable | DiskWatchAction::Unchanged => {}
        }
        out.push(DiskWatchOutcome {
            path: step.path,
            level: step.level,
            pct: step.pct,
            action: step.action,
            notice,
        });
    }
    Ok(out)
}

/// dispatcher が持つ監視の設定と間隔の時刻（[`crate::dispatcher::Dispatcher::set_disk_watch`]）。
pub(crate) struct DiskWatchRunner {
    pub(crate) entries: Vec<DiskWatchEntry>,
    pub(crate) probe: Box<dyn DiskProbe>,
    pub(crate) last_at: Option<OffsetDateTime>,
}

impl DiskWatchRunner {
    /// 前回から [`INTERVAL_SECS`] 経っていれば測る（時計は dispatcher の注入時計）。
    pub(crate) fn tick(
        &mut self,
        store: &dyn DiskWatchStore,
        now: OffsetDateTime,
    ) -> Option<Vec<DiskWatchOutcome>> {
        if self.entries.is_empty()
            || self
                .last_at
                .is_some_and(|t| (now - t).whole_seconds() < INTERVAL_SECS)
        {
            return None;
        }
        self.last_at = Some(now);
        match run_disk_watch(store, self.probe.as_ref(), &self.entries, now) {
            Ok(out) => Some(out),
            Err(error) => {
                tracing::warn!(%error, "disk watch failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
