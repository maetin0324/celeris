//! ADR 2026-11-04-lane-rules-exhaustive-evidence D4・D5: 表 `script_exhaustive_runs`（migration 0110）。
//!
//! - 実行 1 回につき 1 行。開始・進捗・終了だけを書く。live の `script_guard_state`・`script_compare_log`・`events` には触れない（D8）。
//! - 終了の書き込みは `WHERE state='running'` の CAS。起動時に `aborted` にされた行を古い daemon が上書きしない。
//! - 不合格の終了は同じ transaction で live の戻し（`write_mode_tx`・`invalidate_tx`）を行う（D5）。

use std::sync::OnceLock;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use time::OffsetDateTime;

use super::script_domains::{invalidate_tx, read_generation_tx, read_mode_tx, write_mode_tx};
use super::{SqliteStore, StoreError, format_rfc3339};
use crate::script_compare::{ScriptDomain, ScriptDomainError, ScriptGenerationState, ScriptMode};
use crate::script_exhaustive::{
    NewScriptExhaustiveRun, ScriptExhaustiveCounts, ScriptExhaustiveRun, ScriptExhaustiveStanding,
    ScriptExhaustiveState, ScriptExhaustiveStopCode, exhaustive_standing,
    lane_rules_reference_sha256,
};
use crate::script_guard::SCRIPT_GUARD_ACTOR;

const COLUMNS: &str = "id, domain, generation_id, bundle_sha256, mode_at_start, daemon_instance_id, \
    requested_by, reason, state, stop_code, reference_sha256, started_at, finished_at, calls, matches, \
    mismatches, timeouts, failures, safety_violations, uncovered_inputs, latency_p50_us, \
    latency_p95_us, latency_max_us, first_problem_json";

/// `script_exhaustive_finish` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptExhaustiveFinish {
    /// 書いた後の行。
    pub run: ScriptExhaustiveRun,
    /// 戻した領域（D5）。
    pub rolled_back: Vec<ScriptDomain>,
    /// generation を `invalid` にしたか（安全違反）。
    pub invalidated: bool,
}

/// 今の binary の参照の hash（D2）。process の間は変わらないので 1 回だけ計算する。
pub fn current_reference_sha256() -> Result<&'static str, StoreError> {
    static REFERENCE: OnceLock<String> = OnceLock::new();
    if let Some(v) = REFERENCE.get() {
        return Ok(v.as_str());
    }
    let v = lane_rules_reference_sha256().map_err(StoreError::from)?;
    Ok(REFERENCE.get_or_init(|| v).as_str())
}

impl SqliteStore {
    /// 網羅を始める行（`running`）を書く。
    ///
    /// - `lane_rules` 以外は `ExhaustiveDomainUnsupported`。
    /// - 全体の停止の間は `GuardHalted`。
    /// - mode が `compare`・`canary` でない、または `bundle_sha256` が mode の行の generation と違えば `ExhaustiveModeConflict`。
    /// - 同じ領域で `running` の行があれば `ExhaustiveRunning`。
    ///
    /// どの拒否でも表を変えない。`generation_id` と `mode_at_start` は mode の行から決める。
    pub fn script_exhaustive_begin(
        &self,
        new: &NewScriptExhaustiveRun,
        now: OffsetDateTime,
    ) -> Result<ScriptExhaustiveRun, ScriptDomainError> {
        if new.domain != ScriptDomain::LaneRules {
            return Err(ScriptDomainError::ExhaustiveDomainUnsupported(
                new.domain.as_str().to_owned(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reason) = super::script_guard::read_halted_tx(&tx)? {
            return Err(ScriptDomainError::GuardHalted(reason));
        }
        let mode = read_mode_tx(&tx, new.domain)?
            .ok_or_else(|| ScriptDomainError::UnknownDomain(new.domain.as_str().to_owned()))?;
        if !matches!(mode.mode, ScriptMode::Compare | ScriptMode::Canary) {
            return Err(ScriptDomainError::ExhaustiveModeConflict(format!(
                "mode is `{}`; only `compare` and `canary` can run an exhaustive run",
                mode.mode.as_str()
            )));
        }
        let generation = match mode.generation_id.as_deref() {
            Some(id) => read_generation_tx(&tx, id)?,
            None => None,
        };
        let Some(generation) = generation.filter(|g| g.bundle_sha256 == new.bundle_sha256) else {
            return Err(ScriptDomainError::ExhaustiveModeConflict(format!(
                "bundle `{}` is not the generation of the current `{}` row",
                new.bundle_sha256,
                mode.mode.as_str()
            )));
        };
        if let Some(running) = running_id_tx(&tx, new.domain)? {
            return Err(ScriptDomainError::ExhaustiveRunning(running));
        }
        let id = ulid::Ulid::new().to_string();
        let ts = format_rfc3339(now)?;
        tx.execute(
            "INSERT INTO script_exhaustive_runs (id, domain, generation_id, bundle_sha256, \
             mode_at_start, daemon_instance_id, requested_by, reason, state, reference_sha256, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'running', ?9, ?10)",
            params![
                id,
                new.domain.as_str(),
                generation.id,
                generation.bundle_sha256,
                mode.mode.as_str(),
                new.daemon_instance_id,
                new.requested_by,
                new.reason,
                new.reference_sha256,
                ts,
            ],
        )?;
        let run = read_run_tx(&tx, &id)?
            .ok_or_else(|| StoreError::Invalid(format!("exhaustive run `{id}` vanished")))?;
        tx.commit()?;
        Ok(run)
    }

    /// 進捗（件数・latency・最初の問題）を `running` の行にだけ書く。書けたら `true`。
    pub fn script_exhaustive_progress(
        &self,
        id: &str,
        counts: &ScriptExhaustiveCounts,
        first_problem_json: Option<&str>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let changed = update_counts(&conn, id, counts, first_problem_json)?;
        Ok(changed > 0)
    }

    /// 実行を終える。`state` は `passed`・`failed`・`aborted`。
    ///
    /// - `passed` は `counts.verdict()` が真のときだけ（偽なら `Invalid`）。
    /// - 行が `running` でなければ（起動時に `aborted` にされた等）何も書かず `None`。
    /// - `failed` で `mismatches` が 1 以上、その時点で mode が同じ generation の `canary`・`ts` なら `rust` へ戻す（D5。`compare` は変えない）。
    /// - `safety_violations` が 1 以上なら generation を `invalid` にし、指す全領域を `rust` へ戻す。
    ///
    /// 戻しは `system:script-guard` の actor で、行の書き込みと同じ transaction。
    pub fn script_exhaustive_finish(
        &self,
        id: &str,
        state: ScriptExhaustiveState,
        stop_code: Option<ScriptExhaustiveStopCode>,
        counts: &ScriptExhaustiveCounts,
        first_problem_json: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<Option<ScriptExhaustiveFinish>, ScriptDomainError> {
        if state == ScriptExhaustiveState::Running {
            return Err(StoreError::Invalid("finish needs a terminal state".into()).into());
        }
        if state == ScriptExhaustiveState::Passed && (!counts.verdict() || stop_code.is_some()) {
            return Err(StoreError::Invalid(
                "a passed exhaustive run needs a passing verdict and no stop code".into(),
            )
            .into());
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = format_rfc3339(now)?;
        let changed = tx.execute(
            "UPDATE script_exhaustive_runs SET state = ?2, stop_code = ?3, finished_at = ?4, \
             calls = ?5, matches = ?6, mismatches = ?7, timeouts = ?8, failures = ?9, \
             safety_violations = ?10, uncovered_inputs = ?11, latency_p50_us = ?12, \
             latency_p95_us = ?13, latency_max_us = ?14, first_problem_json = ?15 \
             WHERE id = ?1 AND state = 'running'",
            params![
                id,
                state.as_str(),
                stop_code.map(ScriptExhaustiveStopCode::as_str),
                ts,
                to_i64(counts.calls)?,
                to_i64(counts.matches)?,
                to_i64(counts.mismatches)?,
                to_i64(counts.timeouts)?,
                to_i64(counts.failures)?,
                to_i64(counts.safety_violations)?,
                to_i64(counts.uncovered_inputs)?,
                to_i64(counts.latency_p50_us)?,
                to_i64(counts.latency_p95_us)?,
                to_i64(counts.latency_max_us)?,
                first_problem_json,
            ],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let run = read_run_tx(&tx, id)?
            .ok_or_else(|| StoreError::Invalid(format!("exhaustive run `{id}` vanished")))?;
        let mut rolled_back = Vec::new();
        let mut invalidated = false;
        if state == ScriptExhaustiveState::Failed {
            if counts.safety_violations > 0 {
                if let Some(generation) = read_generation_tx(&tx, &run.generation_id)?
                    && generation.state != ScriptGenerationState::Invalid
                {
                    let reason = format!(
                        "guard_exhaustive_safety_violation: run {id}, safety_violations {}, calls {}",
                        counts.safety_violations, counts.calls
                    );
                    let (_, back) =
                        invalidate_tx(&tx, &generation, SCRIPT_GUARD_ACTOR, &reason, &ts)?;
                    rolled_back = back;
                    invalidated = true;
                }
            } else if counts.mismatches > 0
                && let Some(current) = read_mode_tx(&tx, run.domain)?
                && current.generation_id.as_deref() == Some(run.generation_id.as_str())
                && matches!(current.mode, ScriptMode::Canary | ScriptMode::Ts)
            {
                let reason = format!(
                    "guard_exhaustive_mismatch: run {id}, mismatches {}, calls {}",
                    counts.mismatches, counts.calls
                );
                write_mode_tx(
                    &tx,
                    &current,
                    ScriptMode::Rust,
                    None,
                    None,
                    SCRIPT_GUARD_ACTOR,
                    &reason,
                    &ts,
                )?;
                rolled_back.push(run.domain);
            }
        }
        tx.commit()?;
        Ok(Some(ScriptExhaustiveFinish {
            run,
            rolled_back,
            invalidated,
        }))
    }

    /// `daemon_instance_id` が `current_instance` でない `running` の行を `aborted`（`daemon_restarted`）にする。
    /// 件数を返す（起動時の復元が呼ぶ。D3）。途中からは再開しない。
    pub fn script_exhaustive_abort_stale(
        &self,
        current_instance: &str,
        now: OffsetDateTime,
    ) -> Result<usize, StoreError> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE script_exhaustive_runs SET state = 'aborted', stop_code = 'daemon_restarted', \
             finished_at = ?2 WHERE state = 'running' AND daemon_instance_id <> ?1",
            params![current_instance, format_rfc3339(now)?],
        )?;
        Ok(n)
    }

    /// 領域の行を新しい順に最大 `limit` 行（`generation_id` を渡せばその generation だけ）。
    pub fn script_exhaustive_runs(
        &self,
        domain: ScriptDomain,
        generation_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ScriptExhaustiveRun>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM script_exhaustive_runs \
                 WHERE domain = ?1 AND (?2 IS NULL OR generation_id = ?2) \
                 ORDER BY started_at DESC, id DESC LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![domain.as_str(), generation_id, limit], raw_run)?;
            rows.map(|r| run_of(r?)).collect()
        })
    }

    /// (domain, generation) の証拠の立場（D4）。`reference` は今の binary の `reference_sha256`。
    pub fn script_exhaustive_standing(
        &self,
        domain: ScriptDomain,
        generation_id: &str,
        reference: &str,
    ) -> Result<ScriptExhaustiveStanding, StoreError> {
        self.with_read_conn(|conn| standing_tx(conn, domain, generation_id, reference))
    }
}

/// 昇格の gate が同じ transaction で立場を読む（D6）。
pub(super) fn standing_tx(
    conn: &Connection,
    domain: ScriptDomain,
    generation_id: &str,
    reference: &str,
) -> Result<ScriptExhaustiveStanding, StoreError> {
    if domain != ScriptDomain::LaneRules {
        return Ok(ScriptExhaustiveStanding::NotApplicable);
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM script_exhaustive_runs WHERE domain = ?1 AND generation_id = ?2 \
         AND reference_sha256 = ?3"
    ))?;
    let runs = stmt
        .query_map(params![domain.as_str(), generation_id, reference], raw_run)?
        .map(|r| run_of(r?))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(exhaustive_standing(domain, generation_id, &runs, reference))
}

fn running_id_tx(conn: &Connection, domain: ScriptDomain) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id FROM script_exhaustive_runs WHERE domain = ?1 AND state = 'running'",
            params![domain.as_str()],
            |r| r.get(0),
        )
        .optional()?)
}

fn update_counts(
    conn: &Connection,
    id: &str,
    counts: &ScriptExhaustiveCounts,
    first_problem_json: Option<&str>,
) -> Result<usize, StoreError> {
    Ok(conn.execute(
        "UPDATE script_exhaustive_runs SET calls = ?2, matches = ?3, mismatches = ?4, timeouts = ?5, \
         failures = ?6, safety_violations = ?7, uncovered_inputs = ?8, latency_p50_us = ?9, \
         latency_p95_us = ?10, latency_max_us = ?11, first_problem_json = ?12 \
         WHERE id = ?1 AND state = 'running'",
        params![
            id,
            to_i64(counts.calls)?,
            to_i64(counts.matches)?,
            to_i64(counts.mismatches)?,
            to_i64(counts.timeouts)?,
            to_i64(counts.failures)?,
            to_i64(counts.safety_violations)?,
            to_i64(counts.uncovered_inputs)?,
            to_i64(counts.latency_p50_us)?,
            to_i64(counts.latency_p95_us)?,
            to_i64(counts.latency_max_us)?,
            first_problem_json,
        ],
    )?)
}

fn read_run_tx(conn: &Connection, id: &str) -> Result<Option<ScriptExhaustiveRun>, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM script_exhaustive_runs WHERE id = ?1"),
            params![id],
            raw_run,
        )
        .optional()?;
    raw.map(run_of).transpose()
}

fn to_i64(v: u64) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::Invalid(format!("count {v} out of range")))
}

fn to_u64(v: i64) -> Result<u64, StoreError> {
    u64::try_from(v).map_err(|_| StoreError::Invalid(format!("negative count {v}")))
}

struct RawRun {
    id: String,
    domain: String,
    generation_id: String,
    bundle_sha256: String,
    mode_at_start: String,
    daemon_instance_id: String,
    requested_by: String,
    reason: String,
    state: String,
    stop_code: Option<String>,
    reference_sha256: String,
    started_at: String,
    finished_at: Option<String>,
    nums: [i64; 10],
    first_problem_json: Option<String>,
}

/// 列の順は `COLUMNS`。
fn raw_run(row: &Row<'_>) -> rusqlite::Result<RawRun> {
    let mut nums = [0i64; 10];
    for (i, n) in nums.iter_mut().enumerate() {
        *n = row.get(13 + i)?;
    }
    Ok(RawRun {
        id: row.get(0)?,
        domain: row.get(1)?,
        generation_id: row.get(2)?,
        bundle_sha256: row.get(3)?,
        mode_at_start: row.get(4)?,
        daemon_instance_id: row.get(5)?,
        requested_by: row.get(6)?,
        reason: row.get(7)?,
        state: row.get(8)?,
        stop_code: row.get(9)?,
        reference_sha256: row.get(10)?,
        started_at: row.get(11)?,
        finished_at: row.get(12)?,
        nums,
        first_problem_json: row.get(23)?,
    })
}

fn run_of(raw: RawRun) -> Result<ScriptExhaustiveRun, StoreError> {
    let n = raw.nums;
    Ok(ScriptExhaustiveRun {
        domain: ScriptDomain::parse(&raw.domain).ok_or_else(|| {
            StoreError::Invalid(format!("unknown script domain `{}`", raw.domain))
        })?,
        id: raw.id,
        generation_id: raw.generation_id,
        bundle_sha256: raw.bundle_sha256,
        mode_at_start: raw.mode_at_start,
        daemon_instance_id: raw.daemon_instance_id,
        requested_by: raw.requested_by,
        reason: raw.reason,
        state: parse_state(&raw.state)?,
        stop_code: raw.stop_code.as_deref().map(parse_stop).transpose()?,
        reference_sha256: raw.reference_sha256,
        started_at: raw.started_at,
        finished_at: raw.finished_at,
        counts: ScriptExhaustiveCounts {
            calls: to_u64(n[0])?,
            matches: to_u64(n[1])?,
            mismatches: to_u64(n[2])?,
            timeouts: to_u64(n[3])?,
            failures: to_u64(n[4])?,
            safety_violations: to_u64(n[5])?,
            uncovered_inputs: to_u64(n[6])?,
            latency_p50_us: to_u64(n[7])?,
            latency_p95_us: to_u64(n[8])?,
            latency_max_us: to_u64(n[9])?,
        },
        first_problem_json: raw.first_problem_json,
    })
}

fn parse_state(s: &str) -> Result<ScriptExhaustiveState, StoreError> {
    Ok(match s {
        "running" => ScriptExhaustiveState::Running,
        "passed" => ScriptExhaustiveState::Passed,
        "failed" => ScriptExhaustiveState::Failed,
        "aborted" => ScriptExhaustiveState::Aborted,
        other => {
            return Err(StoreError::Invalid(format!(
                "unknown exhaustive state `{other}`"
            )));
        }
    })
}

fn parse_stop(s: &str) -> Result<ScriptExhaustiveStopCode, StoreError> {
    use ScriptExhaustiveStopCode::*;
    Ok(match s {
        "mismatch" => Mismatch,
        "safety_violation" => SafetyViolation,
        "consecutive_errors" => ConsecutiveErrors,
        "rate_exceeded" => RateExceeded,
        "generation_gone" => GenerationGone,
        "wall_limit" => WallLimit,
        "daemon_stopped" => DaemonStopped,
        "daemon_restarted" => DaemonRestarted,
        other => {
            return Err(StoreError::Invalid(format!(
                "unknown exhaustive stop code `{other}`"
            )));
        }
    })
}

#[cfg(test)]
#[path = "script_exhaustive_tests.rs"]
mod tests;
