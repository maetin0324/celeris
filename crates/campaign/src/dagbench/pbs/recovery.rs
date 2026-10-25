//! recovery 比較の per-trial 記録 `recovery-trials.jsonl`（`celeris.recovery-trial/1`、ADR
//! 2026-10-25-recovery-scheduler-backend 付記 I14 (4)）。
//!
//! `trials.jsonl`（[`TrialBinding`]）とは `trial_key` で結合し、その欄と `derive_status` は変えない。
//! - 壁時計（親の `Instant`、実時間）は `wall_*_ms`、注入した時計（[`super::file_pbs::FileClock`] = replay/偽の時計）は
//!   `logical_*_secs` の別の欄に置き、同じ欄に混ぜない。論理欄の出自は `clock_kind`（`file_fake` の論理秒を実秒と読まない）。
//! - 未観測は null と `missing` の理由で残し、0 で埋めない（`human_intervention`・`rework.redone_local_nodes`・normal の停止欄）。
//! - `scheduler_class` は結合行から、`rework` は intent 台帳と消費台帳から、`recovery_status` は [`derive_recovery`] で導く。
//!   writer と validator（[`check_records`]）が同じ関数を使い、scheduler unknown・取得失敗・unresolved を成功にしない。
use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    backend::{CONSUMED_LEDGER, ConsumedRow},
    intent::{IntentEvent, IntentLedger, IntentRow},
    raw::{Missing, SchedulerEvent, TrialBinding, TrialStatus},
    transport::PbsCall,
};
use crate::{grader::sha256_file, observe::CollectClass};

pub const RECOVERY_FILE: &str = "recovery-trials.jsonl";
pub const RECOVERY_SCHEMA: &str = "celeris.recovery-trial/1";

/// 測定成功にしない理由（付記 I14 (5)）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    WaitNotSaved,
    SavedEvidenceMissing,
    JobMismatch,
    DuplicateSubmit,
    DuplicateConsume,
    StopBoundaryInvalid,
}

/// 論理欄の時計の出自。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockKind {
    /// state dir の `clock.json`（偽 cluster・replay の時計。実秒ではない）。
    FileFake,
    System,
}

/// 最後の原票の分類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerClass {
    FinishedExitZero,
    FinishedNonzero,
    Unknown,
    FetchFailed,
    Mismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    Recovered,
    NotRecovered,
    Rejected,
    Unobserved,
}

/// controller の 1 incarnation（`process-events.jsonl` と親の停止報告から）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerRun {
    pub incarnation: u64,
    pub pid: u32,
    pub starttime: u64,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<i32>,
    pub killed_by_runner: bool,
}

/// 親が独立に読み直した保存済み Waiting の参照（file は測定 dir 相対）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedWaitRef {
    pub source: String,
    pub wait_id: Option<String>,
    pub job_id: String,
    pub file: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rework {
    /// 同じ intent の `sent` 行の 2 本目以降（0 が合格）。
    pub resubmits: u64,
    /// 同じ intent の消費行の 2 本目以降（0 が合格）。
    pub reconsumes: u64,
    /// worker の進捗記録で再実行が数えられなければ null。
    pub redone_local_nodes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanIntervention {
    pub count: u64,
    pub minutes: u64,
}

/// `recovery-trials.jsonl` の 1 行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecord {
    pub schema: String,
    pub seq: u64,
    pub measurement_id: String,
    pub arm: String,
    pub condition: String,
    pub trial_key: String,
    /// 結合した `trials.jsonl` の行の seq。
    pub binding_seq: u64,
    pub logical_job_id: String,
    pub job_ref: Option<String>,
    pub controller: Vec<ControllerRun>,
    pub saved_wait: Option<SavedWaitRef>,
    /// 停止中に親が取った原票（`scheduler-events.jsonl` の seq）。normal は空。
    pub stopped_qstat_seqs: Vec<u64>,
    /// 最初の controller の spawn → 親の独立読み直しで保存を確かめた時点（投入は子の中なので spawn 起点）。
    pub wall_spawn_to_wait_saved_ms: Option<u64>,
    /// kill → 再開 controller の spawn（A2 のみ）。
    pub wall_stop_ms: Option<u64>,
    /// kill → 再開 controller の exit（消費の後、A2 のみ）。
    pub wall_recovery_ms: Option<u64>,
    pub wall_collect_ms: Option<u64>,
    pub wall_grade_ms: Option<u64>,
    pub wall_trial_ms: Option<u64>,
    /// intent 台帳の最初の `sent` 行の時刻（注入した時計）。
    pub logical_submit_secs: Option<u64>,
    pub logical_wait_saved_secs: Option<u64>,
    pub logical_stop_secs: Option<u64>,
    pub logical_resume_secs: Option<u64>,
    /// 消費台帳の最初の行の時刻（注入した時計）。
    pub logical_consume_secs: Option<u64>,
    pub clock_kind: ClockKind,
    pub rework: Rework,
    pub human_intervention: Option<HumanIntervention>,
    pub scheduler_class: SchedulerClass,
    pub recovery_status: RecoveryStatus,
    pub reject_reason: Option<RejectReason>,
    pub missing: Vec<Missing>,
}

/// writer に渡す観測値（導出欄は [`build_record`] が埋める）。
#[derive(Clone, Debug)]
pub struct RecoveryInput {
    pub condition: String,
    pub controller: Vec<ControllerRun>,
    pub saved_wait: Option<SavedWaitRef>,
    pub stopped_qstat_seqs: Vec<u64>,
    pub wall_spawn_to_wait_saved_ms: Option<u64>,
    pub wall_stop_ms: Option<u64>,
    pub wall_recovery_ms: Option<u64>,
    pub wall_collect_ms: Option<u64>,
    pub wall_grade_ms: Option<u64>,
    pub wall_trial_ms: Option<u64>,
    pub logical_wait_saved_secs: Option<u64>,
    pub logical_stop_secs: Option<u64>,
    pub logical_resume_secs: Option<u64>,
    pub clock_kind: ClockKind,
    pub reject_reason: Option<RejectReason>,
}

fn unix_of(utc: &str) -> Option<u64> {
    OffsetDateTime::parse(utc, &Rfc3339)
        .ok()
        .and_then(|t| u64::try_from(t.unix_timestamp()).ok())
}

/// 結合行の最後の原票を分類する。照会失敗は `fetch_failed`、別 job・別 trial は `mismatch`、終了を観測できなければ `unknown`。
pub fn scheduler_class_of(b: &TrialBinding) -> SchedulerClass {
    match b.collect_class {
        Some(CollectClass::FetchFailed) => return SchedulerClass::FetchFailed,
        Some(CollectClass::IdentityMismatch) => return SchedulerClass::Mismatch,
        Some(_) => return SchedulerClass::Unknown,
        None => {}
    }
    if b.job_ref.is_none() || b.qstat.is_none() || b.observed.is_none() {
        return SchedulerClass::Unknown;
    }
    match b.exit_status {
        Some(0) => SchedulerClass::FinishedExitZero,
        Some(_) => SchedulerClass::FinishedNonzero,
        None => SchedulerClass::Unknown,
    }
}

/// 結果の導出（writer と validator が同じ関数を使う）。scheduler unknown・取得失敗・unresolved は成功にしない。
pub fn derive_recovery(b: &TrialBinding, r: &RecoveryRecord) -> RecoveryStatus {
    if r.reject_reason.is_some() || r.scheduler_class == SchedulerClass::Mismatch {
        return RecoveryStatus::Rejected;
    }
    if matches!(
        r.scheduler_class,
        SchedulerClass::Unknown | SchedulerClass::FetchFailed
    ) || matches!(b.status, TrialStatus::Unobserved | TrialStatus::Unresolved)
    {
        return RecoveryStatus::Unobserved;
    }
    let a2_ok = r.condition != "a2"
        || (r.saved_wait.is_some()
            && !r.stopped_qstat_seqs.is_empty()
            && r.controller
                .first()
                .is_some_and(|c| c.killed_by_runner && c.exit_signal == Some(9)));
    if b.status == TrialStatus::ValidatedCompleted
        && r.scheduler_class == SchedulerClass::FinishedExitZero
        && r.rework.resubmits == 0
        && r.rework.reconsumes == 0
        && a2_ok
    {
        RecoveryStatus::Recovered
    } else {
        RecoveryStatus::NotRecovered
    }
}

/// 台帳から導く欄: (最初の sent の論理秒, sent 行数, 最初の消費の論理秒, 消費行数)。
fn ledger_facts(
    rows: &[IntentRow],
    consumed: &[ConsumedRow],
    intent_id: &str,
) -> (Option<u64>, u64, Option<u64>, u64) {
    let sent: Vec<&IntentRow> = rows
        .iter()
        .filter(|r| r.submit_intent_id == intent_id && matches!(r.event, IntentEvent::Sent))
        .collect();
    let used: Vec<&ConsumedRow> = consumed
        .iter()
        .filter(|r| r.submit_intent_id == intent_id)
        .collect();
    (
        sent.first().and_then(|r| unix_of(&r.at_utc)),
        sent.len() as u64,
        used.first().and_then(|r| unix_of(&r.at_utc)),
        used.len() as u64,
    )
}

/// 消費台帳（無ければ空）。
pub fn read_consumed(dir: &Path) -> Result<Vec<ConsumedRow>, String> {
    let path = dir.join(CONSUMED_LEDGER);
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(&path)
        .map_err(|e| e.to_string())?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{CONSUMED_LEDGER}: {e}")))
        .collect()
}

fn intent_rows(dir: &Path) -> Result<Vec<IntentRow>, String> {
    IntentLedger::open(dir, "recovery-reader")
        .map_err(|e| e.to_string())?
        .rows()
        .map_err(|e| e.to_string())
}

const NO_HUMAN_OPS: &str = "no op_start/op_end record (automatic run); not counted as 0";
const NO_LOCAL_REDO: &str =
    "the controller progress record does not count re-executed local nodes; not counted as 0";

/// 観測値と測定 dir の台帳から 1 行を作る（seq は呼び出し側）。
pub fn build_record(
    dir: &Path,
    b: &TrialBinding,
    input: RecoveryInput,
    seq: u64,
) -> Result<RecoveryRecord, String> {
    let rows = intent_rows(dir)?;
    let consumed = read_consumed(dir)?;
    let (submit, sent, consume, used) = ledger_facts(&rows, &consumed, &b.submit_intent_id);
    let mut missing = vec![
        Missing {
            field: "human_intervention".into(),
            reason: NO_HUMAN_OPS.into(),
        },
        Missing {
            field: "rework.redone_local_nodes".into(),
            reason: NO_LOCAL_REDO.into(),
        },
    ];
    let mut miss = |field: &str, v: bool, reason: &str| {
        if v {
            missing.push(Missing {
                field: field.into(),
                reason: reason.into(),
            });
        }
    };
    let a2 = input.condition == "a2";
    let not_stopped = "normal does not stop the controller";
    for (f, v) in [
        ("wall_stop_ms", input.wall_stop_ms.is_none()),
        ("wall_recovery_ms", input.wall_recovery_ms.is_none()),
        ("logical_stop_secs", input.logical_stop_secs.is_none()),
        ("logical_resume_secs", input.logical_resume_secs.is_none()),
    ] {
        miss(f, v, if a2 { "not observed" } else { not_stopped });
    }
    for (f, v) in [
        (
            "wall_spawn_to_wait_saved_ms",
            input.wall_spawn_to_wait_saved_ms.is_none(),
        ),
        ("wall_collect_ms", input.wall_collect_ms.is_none()),
        ("wall_grade_ms", input.wall_grade_ms.is_none()),
        ("wall_trial_ms", input.wall_trial_ms.is_none()),
        (
            "logical_wait_saved_secs",
            input.logical_wait_saved_secs.is_none(),
        ),
        ("saved_wait", input.saved_wait.is_none()),
    ] {
        miss(f, v, "not observed");
    }
    miss(
        "logical_submit_secs",
        submit.is_none(),
        "no sent row in the intent ledger",
    );
    miss(
        "logical_consume_secs",
        consume.is_none(),
        "no consumed row in the consumed ledger",
    );
    let mut r = RecoveryRecord {
        schema: RECOVERY_SCHEMA.into(),
        seq,
        measurement_id: b.measurement_id.clone(),
        arm: b.arm.clone(),
        condition: input.condition,
        trial_key: b.trial_key.clone(),
        binding_seq: b.seq,
        logical_job_id: b.logical_job_id.clone(),
        job_ref: b.job_ref.clone(),
        controller: input.controller,
        saved_wait: input.saved_wait,
        stopped_qstat_seqs: input.stopped_qstat_seqs,
        wall_spawn_to_wait_saved_ms: input.wall_spawn_to_wait_saved_ms,
        wall_stop_ms: input.wall_stop_ms,
        wall_recovery_ms: input.wall_recovery_ms,
        wall_collect_ms: input.wall_collect_ms,
        wall_grade_ms: input.wall_grade_ms,
        wall_trial_ms: input.wall_trial_ms,
        logical_submit_secs: submit,
        logical_wait_saved_secs: input.logical_wait_saved_secs,
        logical_stop_secs: input.logical_stop_secs,
        logical_resume_secs: input.logical_resume_secs,
        logical_consume_secs: consume,
        clock_kind: input.clock_kind,
        rework: Rework {
            resubmits: sent.saturating_sub(1),
            reconsumes: used.saturating_sub(1),
            redone_local_nodes: None,
        },
        human_intervention: None,
        scheduler_class: scheduler_class_of(b),
        recovery_status: RecoveryStatus::Unobserved,
        reject_reason: input.reject_reason,
        missing,
    };
    r.recovery_status = derive_recovery(b, &r);
    Ok(r)
}

/// `recovery-trials.jsonl` を検証する（raw だけから再計算し、食い違えば Err）。戻り値は trial_key ごとの最新の行。
pub fn check_records(
    dir: &Path,
    records: &[RecoveryRecord],
    bindings: &[TrialBinding],
    events: &[SchedulerEvent],
) -> Result<BTreeMap<String, RecoveryRecord>, String> {
    let rows = intent_rows(dir)?;
    let consumed = read_consumed(dir)?;
    let mut out = BTreeMap::new();
    for (i, r) in records.iter().enumerate() {
        let at = format!("{RECOVERY_FILE} seq {} ({})", r.seq, r.trial_key);
        if r.schema != RECOVERY_SCHEMA || r.seq != i as u64 + 1 {
            return Err(format!("{at}: schema or seq is broken (append-only)"));
        }
        let b = bindings
            .iter()
            .find(|b| b.seq == r.binding_seq)
            .ok_or_else(|| format!("{at}: trial binding seq {} is missing", r.binding_seq))?;
        if r.trial_key != b.trial_key
            || r.measurement_id != b.measurement_id
            || r.arm != b.arm
            || r.logical_job_id != b.logical_job_id
            || r.job_ref != b.job_ref
        {
            return Err(format!(
                "{at}: identity differs from the trial binding (job ID/trial mismatch)"
            ));
        }
        if !matches!(r.condition.as_str(), "normal" | "a2") {
            return Err(format!("{at}: unknown condition {}", r.condition));
        }
        let class = scheduler_class_of(b);
        if r.scheduler_class != class {
            return Err(format!(
                "{at}: scheduler_class {:?} is not derived from the binding ({class:?})",
                r.scheduler_class
            ));
        }
        let (submit, sent, consume, used) = ledger_facts(&rows, &consumed, &b.submit_intent_id);
        if r.rework.resubmits != sent.saturating_sub(1)
            || r.rework.reconsumes != used.saturating_sub(1)
            || r.logical_submit_secs != submit
            || r.logical_consume_secs != consume
        {
            return Err(format!(
                "{at}: rework or ledger-derived logical times differ from the ledgers"
            ));
        }
        if let Some(s) = &r.saved_wait {
            let got = sha256_file(&dir.join(&s.file))
                .map_err(|e| format!("{at}: saved wait {} is missing: {e}", s.file))?;
            if got != s.sha256 {
                return Err(format!(
                    "{at}: saved wait {} sha256 differs (altered)",
                    s.file
                ));
            }
            if b.job_ref.as_deref().and_then(|j| j.rsplit(':').next()) != Some(s.job_id.as_str()) {
                return Err(format!("{at}: saved wait job differs from job_ref"));
            }
        }
        for seq in &r.stopped_qstat_seqs {
            let ev = events
                .iter()
                .find(|e| e.seq == *seq)
                .ok_or_else(|| format!("{at}: stopped qstat event {seq} is missing"))?;
            if ev.call != PbsCall::QstatFull || ev.identity.trial_key != r.trial_key {
                return Err(format!(
                    "{at}: stopped qstat event {seq} is not this trial's qstat"
                ));
            }
        }
        if r.condition == "normal"
            && (!r.stopped_qstat_seqs.is_empty()
                || r.wall_stop_ms.is_some()
                || r.wall_recovery_ms.is_some()
                || r.logical_stop_secs.is_some())
        {
            return Err(format!("{at}: normal has stop fields"));
        }
        // null は理由と組で残す（0 で埋めない）。
        let names: Vec<&str> = r.missing.iter().map(|m| m.field.as_str()).collect();
        for (f, null) in [
            ("human_intervention", r.human_intervention.is_none()),
            (
                "rework.redone_local_nodes",
                r.rework.redone_local_nodes.is_none(),
            ),
            ("wall_stop_ms", r.wall_stop_ms.is_none()),
            ("wall_recovery_ms", r.wall_recovery_ms.is_none()),
            ("wall_trial_ms", r.wall_trial_ms.is_none()),
            ("logical_stop_secs", r.logical_stop_secs.is_none()),
            ("logical_submit_secs", r.logical_submit_secs.is_none()),
            ("logical_consume_secs", r.logical_consume_secs.is_none()),
            ("saved_wait", r.saved_wait.is_none()),
        ] {
            if null && !names.contains(&f) {
                return Err(format!("{at}: {f} is null without a reason"));
            }
        }
        let derived = derive_recovery(b, r);
        if derived != r.recovery_status {
            return Err(format!(
                "{at}: recovery_status {:?} is not supported by the evidence (derived {derived:?})",
                r.recovery_status
            ));
        }
        out.insert(r.trial_key.clone(), r.clone());
    }
    Ok(out)
}
