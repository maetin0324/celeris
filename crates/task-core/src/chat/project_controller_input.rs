//! ADR 2026-11-04-project-controller D12・A1: controller が案件 thread に積む `role=system` の入力（I10）。
//!
//! 既存の `chat_system_queue_once`（key で冪等）を使う。新しい thread 種別は足さない（案件の固定 thread、
//! `cos_thread_ensure_tx`）。backpressure: 同じ thread で controller の入力（key `pc-input:`）がまだ
//! `queued` なら次を積まない（`max_open`、D12 `max_cos_inputs_open_per_project`）。

use rusqlite::params;
use time::OffsetDateTime;

use super::store::{chat_ts, cos_thread_ensure_tx, immediate, writer};
use super::*;
use crate::project_wake::COS_INPUT_KEY_PREFIX;
use crate::store::SqliteStore;

/// [`SqliteStore::project_controller_queue_cos_input`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerCosInput {
    /// 積んだ（または同じ key の入力が既にあった）。message id。
    Queued(String),
    /// 前の controller の入力がまだ待ち行列にある。
    Busy,
}

impl SqliteStore {
    /// 案件 thread を用意し、controller の未処理の入力が `max_open` 未満なら `key` で 1 件積む。
    pub fn project_controller_queue_cos_input(
        &self,
        project_id: &str,
        key: &str,
        text: &str,
        max_open: u32,
        now: OffsetDateTime,
    ) -> Result<ControllerCosInput, ChatError> {
        let at = chat_ts(now);
        let thread_id = {
            let mut conn = writer(self)?;
            let tx = immediate(&mut conn)?;
            let thread = cos_thread_ensure_tx(&tx, Some(project_id), &at)?;
            let existing: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE thread_id=?1 AND client_message_id=?2)",
                params![thread, key],
                |r| r.get(0),
            )?;
            let open: i64 = tx.query_row(
                "SELECT COUNT(*) FROM chat_messages WHERE thread_id=?1 AND role='system' \
                 AND state='queued' AND substr(client_message_id,1,length(?2))=?2",
                params![thread, COS_INPUT_KEY_PREFIX],
                |r| r.get(0),
            )?;
            tx.commit()?;
            if !existing && open >= i64::from(max_open) {
                return Ok(ControllerCosInput::Busy);
            }
            thread
        };
        let message = self.chat_system_queue_once(&thread_id, key, text, now)?;
        Ok(ControllerCosInput::Queued(message.id))
    }
}
