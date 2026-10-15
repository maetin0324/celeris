//! `root_done_summaries` の読み書き（ADR 2026-10-15-root-done-cos-summary D1/D2）。
//!
//! root task の done 遷移（task_id + done 遷移の seq）ごとに 1 行。`new → queued → notified | fallback` と
//! 一方向にだけ進める（条件付き UPDATE。後戻り・二重確定はしない）。LLM 呼び出しは無い。
use rusqlite::{OptionalExtension, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::model::TaskId;

/// まとめの進み具合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootDoneState {
    /// 検知しただけ（スレッドも run も無い）。
    New,
    /// 専用スレッドに入力を置き、CoS run を待っている。
    Queued,
    /// CoS のまとめで Discord 通知の行を作った。
    Notified,
    /// CoS が使えず、決定的な短文で通知の行を作った。
    Fallback,
}

impl RootDoneState {
    pub fn as_str(self) -> &'static str {
        match self {
            RootDoneState::New => "new",
            RootDoneState::Queued => "queued",
            RootDoneState::Notified => "notified",
            RootDoneState::Fallback => "fallback",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "new" => RootDoneState::New,
            "queued" => RootDoneState::Queued,
            "notified" => RootDoneState::Notified,
            "fallback" => RootDoneState::Fallback,
            _ => return None,
        })
    }
}

/// `root_done_summaries` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootDoneSummary {
    pub task_id: TaskId,
    pub done_seq: u64,
    pub state: RootDoneState,
    pub thread_id: Option<String>,
    pub input_message_id: Option<String>,
    pub created_at: OffsetDateTime,
}

impl SqliteStore {
    /// 行が無ければ `new` で作る。作ったら `true`（同じ (task, seq) の 2 回目は `false`）。
    pub fn root_done_insert(
        &self,
        task_id: TaskId,
        done_seq: u64,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        let n = self.lock()?.execute(
            "INSERT OR IGNORE INTO root_done_summaries \
             (task_id, done_seq, state, created_at, updated_at) VALUES (?1, ?2, 'new', ?3, ?3)",
            params![task_id.to_string(), done_seq as i64, at],
        )?;
        Ok(n > 0)
    }

    /// 未確定（`new`・`queued`）の行を古い順に返す。
    pub fn root_done_open(&self) -> Result<Vec<RootDoneSummary>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT task_id, done_seq, state, thread_id, input_message_id, created_at \
             FROM root_done_summaries WHERE state IN ('new', 'queued') ORDER BY created_at, task_id, done_seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (task, seq, state, thread_id, input_message_id, created) = row?;
            let task_id = task
                .parse::<TaskId>()
                .map_err(|e| StoreError::Invalid(format!("root_done task id: {e}")))?;
            let state = RootDoneState::parse(&state)
                .ok_or_else(|| StoreError::Invalid(format!("root_done state {state}")))?;
            let created_at = time::OffsetDateTime::parse(
                &created,
                &time::format_description::well_known::Rfc3339,
            )
            .map_err(|e| StoreError::Invalid(e.to_string()))?;
            out.push(RootDoneSummary {
                task_id,
                done_seq: seq as u64,
                state,
                thread_id,
                input_message_id,
                created_at,
            });
        }
        Ok(out)
    }

    /// 行の状態を 1 方向に進める。`from` と一致する行だけが変わる（変わったら `true`）。
    pub fn root_done_advance(
        &self,
        task_id: TaskId,
        done_seq: u64,
        from: RootDoneState,
        to: RootDoneState,
        thread: Option<(&str, &str)>,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        let (thread_id, message_id) = match thread {
            Some((t, m)) => (Some(t), Some(m)),
            None => (None, None),
        };
        let n = self.lock()?.execute(
            "UPDATE root_done_summaries SET state = ?4, \
             thread_id = COALESCE(?5, thread_id), input_message_id = COALESCE(?6, input_message_id), \
             updated_at = ?7 WHERE task_id = ?1 AND done_seq = ?2 AND state = ?3",
            params![
                task_id.to_string(),
                done_seq as i64,
                from.as_str(),
                to.as_str(),
                thread_id,
                message_id,
                at
            ],
        )?;
        Ok(n > 0)
    }

    /// 試験・診断用: 1 行の状態。
    pub fn root_done_state(
        &self,
        task_id: TaskId,
        done_seq: u64,
    ) -> Result<Option<RootDoneState>, StoreError> {
        let raw: Option<String> = self
            .lock()?
            .query_row(
                "SELECT state FROM root_done_summaries WHERE task_id = ?1 AND done_seq = ?2",
                params![task_id.to_string(), done_seq as i64],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|s| RootDoneState::parse(&s)))
    }
}
