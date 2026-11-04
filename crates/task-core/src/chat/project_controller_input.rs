//! ADR 2026-11-04-project-controller D12・A1: controller が案件 thread に積む `role=system` の入力（I10）。
//!
//! 既存の `chat_system_queue_once`（key で冪等）を使う。新しい thread 種別は足さない（案件の固定 thread、
//! `cos_thread_ensure_tx`）。backpressure: 同じ thread で controller の入力（key `pc-input:`）がまだ
//! `queued` なら次を積まない（`max_open`、D12 `max_cos_inputs_open_per_project`）。I7（付記 A1 の 11）: 挿入と
//! 同じ transaction で効果の門を照合する。

use rusqlite::{OptionalExtension, params};
use time::OffsetDateTime;

use super::store::{chat_ts, cos_thread_ensure_tx, immediate, writer};
use super::system_request::chat_system_queue_once_tx;
use super::*;
use crate::project_wake::COS_INPUT_KEY_PREFIX;
use crate::store::SqliteStore;
use crate::store::project_wake_effect::effect_gate_open_tx;

/// [`SqliteStore::project_controller_queue_cos_input`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerCosInput {
    /// 積んだ（または同じ key の入力が既にあった）。message id。
    Queued(String),
    /// 前の controller の入力がまだ待ち行列にある。
    Busy,
    /// 効果の門が閉じていた（人の操作の後の古い判断、`off` など）。何も書いていない。
    Refused,
}

impl SqliteStore {
    /// 案件 thread を用意し、controller の未処理の入力が `max_open` 未満なら `key` で 1 件積む。門の照合
    /// （判断の `human_epoch` と今の値、mode・phase・案件の状態）・backpressure・挿入は 1 つの transaction。
    /// 同じ key の入力が既にあれば、門が閉じていてもその入力を返す（効果は既に効いている）。
    pub fn project_controller_queue_cos_input(
        &self,
        project_id: &str,
        key: &str,
        text: &str,
        max_open: u32,
        decision_human_epoch: i64,
        now: OffsetDateTime,
    ) -> Result<ControllerCosInput, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let thread = cos_thread_ensure_tx(&tx, Some(project_id), &at)?;
        let existing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE thread_id=?1 AND client_message_id=?2)",
            params![thread, key],
            |r| r.get(0),
        )?;
        if !existing {
            if !effect_gate_open_tx(&tx, project_id, decision_human_epoch)? {
                return Ok(ControllerCosInput::Refused);
            }
            let open: i64 = tx.query_row(
                "SELECT COUNT(*) FROM chat_messages WHERE thread_id=?1 AND role='system' \
                 AND state='queued' AND substr(client_message_id,1,length(?2))=?2",
                params![thread, COS_INPUT_KEY_PREFIX],
                |r| r.get(0),
            )?;
            if open >= i64::from(max_open) {
                // thread の用意は残す（空の thread は案件に 1 つの固定 thread で、害がない）。
                tx.commit()?;
                return Ok(ControllerCosInput::Busy);
            }
        }
        let message = chat_system_queue_once_tx(&tx, &thread, key, text, &at)?;
        tx.commit()?;
        Ok(ControllerCosInput::Queued(message.id))
    }

    /// `key`・`text` の controller の入力が案件 thread に既にあればその message id（読むだけ。効果が
    /// 既に効いているかの判定。thread を作らない）。
    pub fn project_controller_cos_input_existing(
        &self,
        project_id: &str,
        key: &str,
        text: &str,
    ) -> Result<Option<String>, ChatError> {
        Ok(self.with_read_conn(|c| {
            Ok(c.query_row(
                "SELECT m.id FROM chat_messages m JOIN chat_threads t ON t.id = m.thread_id \
                 WHERE t.kind='project' AND t.project_id=?1 AND m.client_message_id=?2 \
                 AND m.role='system' AND m.text=?3",
                params![project_id, key, text],
                |r| r.get(0),
            )
            .optional()?)
        })?)
    }
}
