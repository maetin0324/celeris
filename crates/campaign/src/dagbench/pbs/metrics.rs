//! 新 raw（`celeris.dagbench-scheduler-raw/1`）だけからの再集計（ADR 2026-10-25-recovery-scheduler-backend 付記 I14 (6)）。
//!
//! [`regenerate`] は [`raw::validate`] を通った測定 dir の `run.json`・`scheduler-events.jsonl`・`trials.jsonl`・
//! `recovery-trials.jsonl`・intent 台帳・消費台帳と、それらが hash で指す file だけを読む。偽 cluster の state dir・
//! controller の memory は読まない。出力は新しい dir だけに新規作成する（既にあれば Err、raw の内側なら Err）。
//! 旧 `celeris.dagbench-durable-raw/1`・ext-sim の raw・main-3・K4 の資産は版で拒否され、読まれも書かれもしない。
//!
//! 壁時計（`wall_*_ms`）と論理時計（`logical_*_secs`、`clock_kind` 付き）は別の表（[`WALL_TSV`]・[`LOGICAL_TSV`]）と
//! JSON の別の欄（`wall`・`logical`）に出し、混ぜない。未観測は null のまま（TSV では `null`）で 0 にしない。
//! `validated_completed` は scheduler unknown・取得失敗・unresolved・rejected を数えない。
//! 出力に時刻・絶対 path を入れないので、同じ raw（とその写し）からの出力は byte 一致する。
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

use super::{
    raw::{self, RawReport, SCHEDULER_RAW_SCHEMA, TrialStatus},
    recovery::{
        ClockKind, HumanIntervention, RecoveryRecord, RecoveryStatus, RejectReason, Rework,
        SchedulerClass,
    },
    transport::{CallOutcome, PbsCall},
};

pub const METRICS_SCHEMA: &str = "celeris.dagbench-scheduler-metrics/1";
pub const METRICS_JSON: &str = "scheduler_metrics.json";
pub const CELLS_TSV: &str = "scheduler_cells.tsv";
pub const TRIALS_TSV: &str = "scheduler_trials.tsv";
pub const WALL_TSV: &str = "scheduler_wall.tsv";
pub const LOGICAL_TSV: &str = "scheduler_logical.tsv";

/// arm × condition の件数。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellCounts {
    pub arm: String,
    pub condition: String,
    pub trials: u64,
    pub recovered: u64,
    pub not_recovered: u64,
    pub rejected: u64,
    pub unobserved: u64,
    pub validated_completed: u64,
}

/// trial ごとの結果（時計の欄を持たない）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialRow {
    pub trial_key: String,
    pub arm: String,
    pub condition: String,
    pub job_ref: Option<String>,
    pub binding_status: TrialStatus,
    pub scheduler_class: SchedulerClass,
    pub recovery_status: RecoveryStatus,
    pub reject_reason: Option<RejectReason>,
    pub validated_completed: bool,
    pub stopped_qstat_records: u64,
    pub rework: Rework,
    pub human_intervention: Option<HumanIntervention>,
}

/// 壁時計（実時間、ms）の行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallRow {
    pub trial_key: String,
    pub arm: String,
    pub condition: String,
    pub spawn_to_wait_saved_ms: Option<u64>,
    pub stop_ms: Option<u64>,
    pub recovery_ms: Option<u64>,
    pub collect_ms: Option<u64>,
    pub grade_ms: Option<u64>,
    pub trial_ms: Option<u64>,
}

/// 論理時計（注入した時計、秒）の行。`clock_kind=file_fake` は実秒ではない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalRow {
    pub trial_key: String,
    pub arm: String,
    pub condition: String,
    pub clock_kind: ClockKind,
    pub submit_secs: Option<u64>,
    pub wait_saved_secs: Option<u64>,
    pub stop_secs: Option<u64>,
    pub resume_secs: Option<u64>,
    pub consume_secs: Option<u64>,
}

/// `scheduler_metrics.json`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerMetrics {
    pub schema: String,
    pub raw_schema: String,
    pub dataset: String,
    pub measurement_id: String,
    pub spec_sha256: String,
    /// intent 台帳の受理・照合済み job の数（`run.json` と一致を validator が確かめた値）。
    pub real_jobs: u64,
    /// 原票のうち成功した qsub の数。
    pub qsub_records: u64,
    pub trials: u64,
    /// `trials.jsonl` にあって recovery 記録の無い trial（集計に入れない）。
    pub trials_without_recovery_record: u64,
    pub validated_completed: u64,
    pub cells: Vec<CellCounts>,
    pub trial_rows: Vec<TrialRow>,
    pub wall: Vec<WallRow>,
    pub logical: Vec<LogicalRow>,
}

fn validated(r: &RecoveryRecord, status: TrialStatus) -> bool {
    status == TrialStatus::ValidatedCompleted
        && r.scheduler_class == SchedulerClass::FinishedExitZero
        && matches!(
            r.recovery_status,
            RecoveryStatus::Recovered | RecoveryStatus::NotRecovered
        )
}

/// 検証済みの raw から集計する（file は書かない）。
pub fn compute(report: &RawReport, qsub_records: u64) -> SchedulerMetrics {
    let mut cells: BTreeMap<(String, String), CellCounts> = BTreeMap::new();
    let (mut trial_rows, mut wall, mut logical) = (Vec::new(), Vec::new(), Vec::new());
    let mut total_validated = 0;
    for (key, r) in &report.recovery {
        let status = report
            .trials
            .get(key)
            .map_or(TrialStatus::Unobserved, |b| b.status);
        let ok = validated(r, status);
        total_validated += u64::from(ok);
        let c = cells
            .entry((r.arm.clone(), r.condition.clone()))
            .or_insert_with(|| CellCounts {
                arm: r.arm.clone(),
                condition: r.condition.clone(),
                ..CellCounts::default()
            });
        c.trials += 1;
        c.validated_completed += u64::from(ok);
        match r.recovery_status {
            RecoveryStatus::Recovered => c.recovered += 1,
            RecoveryStatus::NotRecovered => c.not_recovered += 1,
            RecoveryStatus::Rejected => c.rejected += 1,
            RecoveryStatus::Unobserved => c.unobserved += 1,
        }
        trial_rows.push(TrialRow {
            trial_key: key.clone(),
            arm: r.arm.clone(),
            condition: r.condition.clone(),
            job_ref: r.job_ref.clone(),
            binding_status: status,
            scheduler_class: r.scheduler_class,
            recovery_status: r.recovery_status,
            reject_reason: r.reject_reason,
            validated_completed: ok,
            stopped_qstat_records: r.stopped_qstat_seqs.len() as u64,
            rework: r.rework.clone(),
            human_intervention: r.human_intervention.clone(),
        });
        wall.push(WallRow {
            trial_key: key.clone(),
            arm: r.arm.clone(),
            condition: r.condition.clone(),
            spawn_to_wait_saved_ms: r.wall_spawn_to_wait_saved_ms,
            stop_ms: r.wall_stop_ms,
            recovery_ms: r.wall_recovery_ms,
            collect_ms: r.wall_collect_ms,
            grade_ms: r.wall_grade_ms,
            trial_ms: r.wall_trial_ms,
        });
        logical.push(LogicalRow {
            trial_key: key.clone(),
            arm: r.arm.clone(),
            condition: r.condition.clone(),
            clock_kind: r.clock_kind,
            submit_secs: r.logical_submit_secs,
            wait_saved_secs: r.logical_wait_saved_secs,
            stop_secs: r.logical_stop_secs,
            resume_secs: r.logical_resume_secs,
            consume_secs: r.logical_consume_secs,
        });
    }
    let run = &report.run;
    SchedulerMetrics {
        schema: METRICS_SCHEMA.into(),
        raw_schema: SCHEDULER_RAW_SCHEMA.into(),
        dataset: run.dataset.clone(),
        measurement_id: run.measurement_id.clone(),
        spec_sha256: run.spec_sha256.clone(),
        real_jobs: run.real_jobs,
        qsub_records,
        trials: report.recovery.len() as u64,
        trials_without_recovery_record: report
            .trials
            .keys()
            .filter(|k| !report.recovery.contains_key(*k))
            .count() as u64,
        validated_completed: total_validated,
        cells: cells.into_values().collect(),
        trial_rows,
        wall,
        logical,
    }
}

fn opt(v: Option<u64>) -> String {
    v.map_or_else(|| "null".into(), |n| n.to_string())
}

fn name<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "null".into())
}

fn tsv(header: &[&str], rows: impl Iterator<Item = Vec<String>>) -> String {
    let mut s = header.join("\t");
    s.push('\n');
    for r in rows {
        s.push_str(&r.join("\t"));
        s.push('\n');
    }
    s
}

/// 表の本文（json, cells, trials, wall, logical）。
fn render(m: &SchedulerMetrics) -> Result<[(&'static str, String); 5], String> {
    let json = serde_json::to_string_pretty(m).map_err(|e| e.to_string())? + "\n";
    let cells = tsv(
        &[
            "arm",
            "condition",
            "trials",
            "recovered",
            "not_recovered",
            "rejected",
            "unobserved",
            "validated_completed",
        ],
        m.cells.iter().map(|c| {
            vec![
                c.arm.clone(),
                c.condition.clone(),
                c.trials.to_string(),
                c.recovered.to_string(),
                c.not_recovered.to_string(),
                c.rejected.to_string(),
                c.unobserved.to_string(),
                c.validated_completed.to_string(),
            ]
        }),
    );
    let trials = tsv(
        &[
            "trial_key",
            "arm",
            "condition",
            "job_ref",
            "binding_status",
            "scheduler_class",
            "recovery_status",
            "reject_reason",
            "validated_completed",
            "stopped_qstat_records",
            "resubmits",
            "reconsumes",
            "redone_local_nodes",
            "human_intervention_count",
            "human_intervention_minutes",
        ],
        m.trial_rows.iter().map(|t| {
            vec![
                t.trial_key.clone(),
                t.arm.clone(),
                t.condition.clone(),
                t.job_ref.clone().unwrap_or_else(|| "null".into()),
                name(&t.binding_status),
                name(&t.scheduler_class),
                name(&t.recovery_status),
                t.reject_reason.as_ref().map_or_else(|| "null".into(), name),
                t.validated_completed.to_string(),
                t.stopped_qstat_records.to_string(),
                t.rework.resubmits.to_string(),
                t.rework.reconsumes.to_string(),
                opt(t.rework.redone_local_nodes),
                opt(t.human_intervention.as_ref().map(|h| h.count)),
                opt(t.human_intervention.as_ref().map(|h| h.minutes)),
            ]
        }),
    );
    let wall = tsv(
        &[
            "trial_key",
            "arm",
            "condition",
            "spawn_to_wait_saved_ms",
            "stop_ms",
            "recovery_ms",
            "collect_ms",
            "grade_ms",
            "trial_ms",
        ],
        m.wall.iter().map(|w| {
            vec![
                w.trial_key.clone(),
                w.arm.clone(),
                w.condition.clone(),
                opt(w.spawn_to_wait_saved_ms),
                opt(w.stop_ms),
                opt(w.recovery_ms),
                opt(w.collect_ms),
                opt(w.grade_ms),
                opt(w.trial_ms),
            ]
        }),
    );
    let logical = tsv(
        &[
            "trial_key",
            "arm",
            "condition",
            "clock_kind",
            "submit_secs",
            "wait_saved_secs",
            "stop_secs",
            "resume_secs",
            "consume_secs",
        ],
        m.logical.iter().map(|l| {
            vec![
                l.trial_key.clone(),
                l.arm.clone(),
                l.condition.clone(),
                name(&l.clock_kind),
                opt(l.submit_secs),
                opt(l.wait_saved_secs),
                opt(l.stop_secs),
                opt(l.resume_secs),
                opt(l.consume_secs),
            ]
        }),
    );
    Ok([
        (METRICS_JSON, json),
        (CELLS_TSV, cells),
        (TRIALS_TSV, trials),
        (WALL_TSV, wall),
        (LOGICAL_TSV, logical),
    ])
}

fn write_new(path: &Path, text: &str) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// `raw` を検証してから集計し、新規の `out` に書く。`out` が既にある・`raw` の内側・raw の版違いは Err。
pub fn regenerate(raw_dir: &Path, out: &Path) -> Result<SchedulerMetrics, String> {
    if out.symlink_metadata().is_ok() {
        return Err(format!(
            "{}: metrics output must be a new directory (refusing to overwrite)",
            out.display()
        ));
    }
    let raw_c = fs::canonicalize(raw_dir).map_err(|e| format!("{}: {e}", raw_dir.display()))?;
    let leaf = out
        .file_name()
        .ok_or_else(|| format!("{}: no directory name", out.display()))?;
    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    if let Ok(p) = fs::canonicalize(&parent)
        && p.join(leaf).starts_with(&raw_c)
    {
        return Err(format!(
            "{}: metrics output must be outside the raw dir {}",
            out.display(),
            raw_c.display()
        ));
    }
    let report = raw::validate(&raw_c)?;
    let events: Vec<raw::SchedulerEvent> = fs::read_to_string(raw_c.join(raw::EVENTS_FILE))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", raw::EVENTS_FILE)))
        .collect::<Result<_, _>>()?;
    let qsub = events
        .iter()
        .filter(|e| e.call == PbsCall::Qsub && e.outcome == CallOutcome::Ok)
        .count() as u64;
    let m = compute(&report, qsub);
    let files = render(&m)?;
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let out_c = fs::canonicalize(&parent)
        .map_err(|e| e.to_string())?
        .join(leaf);
    if out_c.starts_with(&raw_c) {
        return Err(format!(
            "{}: metrics output must be outside the raw dir",
            out.display()
        ));
    }
    fs::create_dir(&out_c).map_err(|e| format!("{}: {e}", out_c.display()))?;
    for (f, text) in files {
        write_new(&out_c.join(f), &text).map_err(|e| format!("{f}: {e}"))?;
    }
    Ok(m)
}
