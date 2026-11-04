//! ADR 2026-11-04-project-controller D12・I7: 人の操作を controller の判断より優先させる `human_epoch`。
//!
//! 数えるのは `Principal::Human` の要求と、人の発言を運ぶ `…/relays`・`chat.relay_forward`（[`HumanEpoch::Counted`]
//! を直接渡す）。それ以外の `Operator`・`Mcp`・`Cos`・daemon の呼び出しと、`/cos/operations` の監査経路
//! （`*_op` の `audit = Some`）は数えない。増やすのは書き込みと同じ transaction の中（[`HumanEpoch::bump_tx`]）で、
//! 片方だけ残ることはない。表は ADR の付記 A1 の 9。

use task_core::chat::{ChatError, ChatMessagePosted};
use task_core::store::SqliteStore;
use task_core::{ProjectId, StoreError, TaskId};
use task_ops::op_writes::{OpWrites, WritesAlso};
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

    /// `task` の案件で数える（呼び出し側の transaction の中で案件を読む）。
    pub(crate) fn bump_task_tx(
        self,
        tx: &rusqlite::Connection,
        task: TaskId,
    ) -> Result<(), StoreError> {
        if self == Self::NotCounted {
            return Ok(());
        }
        self.bump_tx(tx, SqliteStore::task_project_id_tx(tx, task)?)
    }

    /// 発言を数える（案件の固定 thread の新しい発言だけ。同じ `client_message_id` の再送は数えない）。
    pub(crate) fn bump_posted_tx(
        self,
        tx: &rusqlite::Connection,
        thread: &str,
        posted: &ChatMessagePosted,
    ) -> Result<(), ChatError> {
        if self == Self::NotCounted {
            return Ok(());
        }
        let project = task_core::chat::chat_posted_project_tx(tx, thread, posted)?
            .map(|id| {
                id.parse::<ProjectId>()
                    .map_err(|e| StoreError::Invalid(format!("thread project id {id}: {e}")))
            })
            .transpose()?;
        self.bump_tx(tx, project)?;
        Ok(())
    }

    /// `task_ops` の人の回答の書き込みを、数える transaction で書く口。
    pub(crate) fn writes(self, store: &SqliteStore) -> impl OpWrites + '_ {
        WritesAlso {
            store,
            also: move |tx: &rusqlite::Connection, task: TaskId| self.bump_task_tx(tx, task),
        }
    }
}
