//! ADR 2026-11-04-project-controller D12・I7: 人の操作を controller の判断より優先させる `human_epoch`。
//!
//! 数えるのは `Principal::Human` の要求だけ。`Operator`・`Mcp`・`Cos`・daemon の呼び出しと、
//! `/cos/operations` の監査経路（`*_op` の `audit = Some`）は数えない。増やすのは書き込みと同じ transaction
//! の中（[`HumanEpoch::bump_tx`]）で、片方だけ残ることはない。

use task_core::store::SqliteStore;
use task_core::{ProjectId, StoreError};
use time::OffsetDateTime;

use crate::{Caller, Principal};

/// API は daemon の設定を持たないので、`POST …/controller/wake` と同じ既定の値（D12 `coalesce_secs`）で畳む。
pub(crate) const COALESCE_SECS: u64 = 30;

/// この要求が `human_epoch` を増やすか。認証の結果（[`Caller`]）からだけ作る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HumanEpoch {
    Counted,
    NotCounted,
}

impl HumanEpoch {
    pub(crate) fn of(caller: &Caller) -> Self {
        match caller.0 {
            Principal::Human { .. } => Self::Counted,
            Principal::Operator { .. }
            | Principal::Mcp { .. }
            | Principal::Cos { .. }
            | Principal::Unauthenticated => Self::NotCounted,
        }
    }

    /// 呼び出し側の transaction の中で増やす。案件が無い task、状態の行が無い案件、`off` の案件では何もしない（I13）。
    pub(crate) fn bump_tx(
        self,
        tx: &rusqlite::Connection,
        project: Option<ProjectId>,
    ) -> Result<(), StoreError> {
        let (Self::Counted, Some(project)) = (self, project) else {
            return Ok(());
        };
        SqliteStore::project_controller_human_bump_tx(
            tx,
            &project.to_string(),
            OffsetDateTime::now_utc(),
            COALESCE_SECS,
        )?;
        Ok(())
    }
}
