//! ADR 2026-11-03-cos-digest-persist: one digest pass is applied in one SQLite transaction.
//!
//! The dispatcher builds a [`CosDigestApplyPass`] (the commands of each run and what to record for
//! it); `SqliteStore::cos_digest_apply_pass` writes, per run under its own SAVEPOINT, the digest
//! message, the requeue and either the compare record with its script-guard count or the compare
//! shadow's outbox row. A run whose write fails leaves nothing behind and is listed again by the
//! next pass. The types here carry no SQL.

use crate::chat::digest::CosDigestCommand;
use crate::script_compare::{CosDigestApplied, CosDigestCompareRecord, ScriptMode};
use crate::script_guard::ScriptGuardOutcome;

/// What one run records in the same SAVEPOINT as its effects.
#[derive(Debug, Clone)]
pub enum CosDigestRecordPlan {
    /// Append this record to `script_compare_log` and count it (S5-7). `applied` is replaced by
    /// the result of the apply. `ts_side`: the pass was selected for TS (canary/ts).
    Append {
        record: Box<CosDigestCompareRecord>,
        ts_side: bool,
    },
    /// Mode `compare` and the Rust side of `canary`: the TS shadow runs after the apply, so store
    /// an outbox row holding the record to write if the shadow never finishes (`unavailable`).
    /// The shadow (or the reconcile) closes the row with `cos_digest_shadow_finish`.
    Shadow { lost: Box<CosDigestCompareRecord> },
}

/// One run of a pass.
#[derive(Debug, Clone)]
pub struct CosDigestApplyRun {
    pub thread_id: String,
    pub run_id: String,
    pub commands: Vec<CosDigestCommand>,
    /// `None`: mode `rust` (no record).
    pub record: Option<CosDigestRecordPlan>,
}

/// One pass to apply.
#[derive(Debug, Clone)]
pub struct CosDigestApplyPass {
    pub pass_id: String,
    /// S5-6: the commands are a TS output; every effect is checked against its idempotency key
    /// and the stored record.
    pub verify: bool,
    /// S4-14: the `cos_digest` mode row (mode, generation, version) the TS output was validated
    /// under. Checked inside the transaction; a different row writes nothing (`ModeChanged`).
    pub expect_mode: Option<(ScriptMode, Option<String>, u64)>,
    pub runs: Vec<CosDigestApplyRun>,
}

/// Where the apply is; the test hook is called at each stage (a hook error drops the transaction,
/// like a process that dies there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CosDigestApplyStage<'a> {
    /// The transaction has started; nothing is written yet.
    Begin,
    /// One command of `run_id` is written (`post_digest` or `requeue_run`).
    AfterCommand { run_id: &'a str, kind: &'static str },
    /// The record (or the outbox row) of `run_id` is written.
    AfterRecord { run_id: &'a str },
    /// Every run is written; the commit is next.
    BeforeCommit,
}

/// The result of one run.
#[derive(Debug, Clone)]
pub enum CosDigestRunApply {
    Applied {
        applied: CosDigestApplied,
        /// The record was appended (the duplicate suppression of S4-15 may skip it).
        record_appended: bool,
        /// The script-guard outcome of the appended record.
        guard: Option<ScriptGuardOutcome>,
    },
    /// The run's SAVEPOINT was rolled back: nothing of this run was stored.
    Failed(String),
}

/// The result of a pass.
#[derive(Debug, Clone, Default)]
pub struct CosDigestPassApplied {
    /// The runs in pass order up to the one that stopped the pass.
    pub runs: Vec<(String, CosDigestRunApply)>,
    /// S5-6: a stored effect disagrees with a TS command. That run was rolled back and the rest
    /// of the pass was not written.
    pub unverified: Option<String>,
}

/// Why a pass wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CosDigestApplyError {
    /// S4-14: the mode row is not the one the TS output was validated under.
    #[error("{0}")]
    ModeChanged(String),
    /// The test hook stopped the pass.
    #[error("interrupted: {0}")]
    Interrupted(String),
    /// The transaction could not start or commit.
    #[error("store: {0}")]
    Store(String),
}
