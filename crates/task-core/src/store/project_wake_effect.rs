//! ADR 2026-11-04-project-controller D12・I7・I13・付記 A1: `pending` の効果の門と再試行。
//!
//! - 門（[`SqliteStore::project_controller_effect_admit`]）: 効果を実行する直前に案件の controller の
//!   mode・phase と案件の状態を読み、`off`・`dry_run`・`shadow`・phase `paused`・`cancelled`・案件の
//!   `paused`・`cancelled`・`done`・archive なら効果を実行せず `suppressed`（終端）にする。理由は
//!   `effect_ref` の `halted:<理由>` に残す。判断の `human_epoch` が今の値と違えば（判断の後に人が操作した。I7）
//!   `halted:stale_human_epoch` で同じく止める。再評価は人の操作が積んだ wake `human:<epoch>` が起こす。
//!   phase `blocked` は `pending` のまま待つ（人が mode を置き直すと続く）。
//! - 書き込みの tx の照合（[`effect_gate_open_tx`]）: root の作成と CoS 入力の挿入は、同じ tx で門をもう一度
//!   読む。門を通った直後の人の操作でも古い判断の効果は書かれない（付記 A1 の 11）。
//! - 再試行（[`SqliteStore::project_controller_effect_defer`]）: 効果の `Err` と CoS 入力の Busy は
//!   `pending` のまま `effect_next_at` を backoff（`retry_base_secs × 2^(n-1)`、上限 `retry_max_secs`）で
//!   先へ送る。`Err` が `max_retries` を超えたら `failed` にし、案件の phase を `blocked` にする。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::project_wake::{ControllerMode, ControllerPhase, backoff_secs};

/// `halted:<理由>` の理由: 判断の後に人が操作した（I7）。
const STALE_HUMAN_EPOCH: &str = "stale_human_epoch";

/// 効果を実行してよいか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectAdmit {
    /// 実行してよい。
    Run,
    /// 実行しない。`pending` のまま（phase `blocked`、または試行時刻の前）。
    Hold,
    /// 実行しない。`suppressed` にした（`effect_ref` の理由）。
    Halted(String),
}

/// 延期の理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectDeferKind {
    /// CoS 入力の backpressure（前の入力が未処理）。`max_retries` には数えない。
    Busy,
    /// 効果が失敗した（一時的な DB エラーなど）。`max_retries` に数える。
    Failed,
}

/// [`SqliteStore::project_controller_effect_defer`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectDeferral {
    /// `pending` のまま。次の試行は `retry_at`。
    Retry {
        attempts: u32,
        failures: u32,
        retry_at: OffsetDateTime,
    },
    /// 上限を超えた。`failed` にし、案件の phase を `blocked` にした。
    Exhausted { failures: u32 },
    /// 既に `pending` でない（他の経路が先に進めた）。何もしない。
    NotPending,
}

/// 効果の再試行の欄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectRetryState {
    pub attempts: u32,
    pub failures: u32,
    pub next_at: Option<String>,
}

enum Gate {
    Open,
    Hold,
    Halt(String),
}

/// 止める理由は mode・phase・案件の状態・`human_epoch` の順に見る（人が止めた理由を先に残す）。
/// `blocked` の Hold は最後で、古い判断は `blocked` の間も待たせない。
fn gate(conn: &Connection, project_id: &str, decision_epoch: i64) -> Result<Gate, StoreError> {
    let state: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT mode, phase, human_epoch FROM project_controller_state WHERE project_id = ?1",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((mode, phase, current_epoch)) = state else {
        // 状態の行が無い案件は mode off（I13）。
        return Ok(Gate::Halt("mode_off".into()));
    };
    let mode = ControllerMode::parse(&mode)
        .ok_or_else(|| StoreError::Invalid(format!("controller mode {mode}")))?;
    let phase = ControllerPhase::parse(&phase)
        .ok_or_else(|| StoreError::Invalid(format!("controller phase {phase}")))?;
    if mode != ControllerMode::Enabled {
        return Ok(Gate::Halt(format!("mode_{}", mode.as_str())));
    }
    if matches!(phase, ControllerPhase::Paused | ControllerPhase::Cancelled) {
        return Ok(Gate::Halt(format!("phase_{}", phase.as_str())));
    }
    let project: Option<(String, bool)> = conn
        .query_row(
            "SELECT status, archived_at IS NOT NULL FROM projects WHERE id = ?1",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match project {
        None => Gate::Halt("project_missing".into()),
        Some((_, true)) => Gate::Halt("project_archived".into()),
        Some((status, false)) if matches!(status.as_str(), "paused" | "cancelled" | "done") => {
            Gate::Halt(format!("project_{status}"))
        }
        Some(_) if current_epoch != decision_epoch => Gate::Halt(STALE_HUMAN_EPOCH.into()),
        Some(_) if phase == ControllerPhase::Blocked => Gate::Hold,
        Some(_) => Gate::Open,
    })
}

/// 効果を書く transaction の中で門を読む。閉じていれば（`Hold` を含む）呼び出し側は書かずに戻り、
/// 判断の記録は [`SqliteStore::project_controller_effect_admit`] に任せる。
pub(crate) fn effect_gate_open_tx(
    conn: &Connection,
    project_id: &str,
    decision_epoch: i64,
) -> Result<bool, StoreError> {
    Ok(matches!(
        gate(conn, project_id, decision_epoch)?,
        Gate::Open
    ))
}

fn parse_at(s: &str) -> Result<OffsetDateTime, StoreError> {
    super::parse_rfc3339(s)
}

impl SqliteStore {
    /// 効果を実行してよいか決める（1 つの transaction）。止めるなら同じ transaction で `suppressed` にする。
    pub fn project_controller_effect_admit(
        &self,
        decision_id: &str,
        now: OffsetDateTime,
    ) -> Result<EffectAdmit, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(String, String, Option<String>, i64)> = tx
            .query_row(
                "SELECT project_id, effect_state, effect_next_at, human_epoch \
                 FROM project_controller_decisions WHERE id = ?1",
                [decision_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((project_id, state, next_at, decision_epoch)) = row else {
            return Ok(EffectAdmit::Hold);
        };
        if state != "pending" {
            return Ok(EffectAdmit::Hold);
        }
        let out = match gate(&tx, &project_id, decision_epoch)? {
            Gate::Hold => EffectAdmit::Hold,
            Gate::Halt(reason) => {
                let r = format!("halted:{reason}");
                tx.execute(
                    "UPDATE project_controller_decisions SET effect_state = 'suppressed', \
                     effect_ref = ?2, applied_at = ?3 WHERE id = ?1 AND effect_state = 'pending'",
                    params![decision_id, r, at],
                )?;
                EffectAdmit::Halted(r)
            }
            Gate::Open => match next_at {
                Some(t) if parse_at(&t)? > now => EffectAdmit::Hold,
                _ => EffectAdmit::Run,
            },
        };
        tx.commit()?;
        Ok(out)
    }

    /// 効果を延期する（Busy・`Err`）。`(retry_base_secs, retry_max_secs, max_retries)`。
    pub fn project_controller_effect_defer(
        &self,
        decision_id: &str,
        kind: EffectDeferKind,
        error: Option<&str>,
        (base, max, max_retries): (u64, u64, u32),
        now: OffsetDateTime,
    ) -> Result<EffectDeferral, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(String, String, u32, u32)> = tx
            .query_row(
                "SELECT project_id, effect_state, effect_attempts, effect_failures \
                 FROM project_controller_decisions WHERE id = ?1",
                [decision_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((project_id, state, attempts, failures)) = row else {
            return Ok(EffectDeferral::NotPending);
        };
        if state != "pending" {
            return Ok(EffectDeferral::NotPending);
        }
        let attempts = attempts + 1;
        let failures = failures + u32::from(kind == EffectDeferKind::Failed);
        let note = match (kind, error) {
            (EffectDeferKind::Busy, _) => format!("busy (attempt {attempts})"),
            (EffectDeferKind::Failed, e) => {
                format!("error (failure {failures}): {}", e.unwrap_or("-"))
            }
        };
        if kind == EffectDeferKind::Failed && failures > max_retries {
            tx.execute(
                "UPDATE project_controller_decisions SET effect_state = 'failed', effect_ref = ?2, \
                 effect_attempts = ?3, effect_failures = ?4, effect_next_at = NULL, applied_at = ?5 \
                 WHERE id = ?1 AND effect_state = 'pending'",
                params![decision_id, note, attempts, failures, at],
            )?;
            tx.execute(
                "UPDATE project_controller_state SET phase = 'blocked', updated_at = ?2, \
                 updated_by = 'daemon' WHERE project_id = ?1",
                params![project_id, at],
            )?;
            tx.commit()?;
            return Ok(EffectDeferral::Exhausted { failures });
        }
        let retry_at = now + time::Duration::seconds(backoff_secs(attempts, base, max) as i64);
        tx.execute(
            "UPDATE project_controller_decisions SET effect_ref = ?2, effect_attempts = ?3, \
             effect_failures = ?4, effect_next_at = ?5 WHERE id = ?1 AND effect_state = 'pending'",
            params![
                decision_id,
                note,
                attempts,
                failures,
                format_rfc3339(retry_at)?
            ],
        )?;
        tx.commit()?;
        Ok(EffectDeferral::Retry {
            attempts,
            failures,
            retry_at,
        })
    }

    /// 効果の再試行の欄（試験・診断）。
    pub fn project_controller_effect_retry_state(
        &self,
        decision_id: &str,
    ) -> Result<Option<EffectRetryState>, StoreError> {
        self.with_read_conn(|c| {
            Ok(c.query_row(
                "SELECT effect_attempts, effect_failures, effect_next_at \
                 FROM project_controller_decisions WHERE id = ?1",
                [decision_id],
                |r| {
                    Ok(EffectRetryState {
                        attempts: r.get(0)?,
                        failures: r.get(1)?,
                        next_at: r.get(2)?,
                    })
                },
            )
            .optional()?)
        })
    }
}
