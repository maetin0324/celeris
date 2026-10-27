//! ADR 2026-10-27-unblocks-reopen-on-promotion D1・D4: `task_unblocks` の読み書き。

use rusqlite::{Connection, TransactionBehavior, params};

use crate::model::TaskId;
use crate::transition::Trigger;
use crate::unblocks::{UnblockAction, UnblockFire, UnblockLink};

use super::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

const COLUMNS: &str =
    "fix_task_id, waiting_task_id, created_at, fired_release, fired_action, fired_at";

type RawLink = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn link_from_raw(raw: RawLink) -> Result<UnblockLink, StoreError> {
    let (fix, waiting, created_at, fired_release, fired_action, fired_at) = raw;
    let id = |s: &str| {
        s.parse::<TaskId>()
            .map_err(|_| StoreError::Invalid(format!("task_unblocks: invalid task id {s:?}")))
    };
    Ok(UnblockLink {
        fix_task_id: id(&fix)?,
        waiting_task_id: id(&waiting)?,
        created_at: parse_rfc3339(&created_at)?,
        fired_release,
        fired_action: match fired_action.as_deref() {
            None => None,
            Some(s) => Some(UnblockAction::parse(s).ok_or_else(|| {
                StoreError::Invalid(format!("task_unblocks: unknown fired_action {s:?}"))
            })?),
        },
        fired_at: fired_at.as_deref().map(parse_rfc3339).transpose()?,
    })
}

fn query_links(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<UnblockLink>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(args, |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for raw in rows {
        out.push(link_from_raw(raw?)?);
    }
    Ok(out)
}

impl SqliteStore {
    /// D1: 修正 task `fix` の作成と同じ transaction で結びを書く。`waiting` は呼び出し側が検証済み
    /// （存在する・重複しない・上限以内）。同じ結びの二重登録は主キーで 1 行に畳む。
    pub fn unblocks_insert_tx(
        tx: &Connection,
        fix: TaskId,
        waiting: &[TaskId],
        now: time::OffsetDateTime,
    ) -> Result<(), StoreError> {
        let ts = format_rfc3339(now)?;
        for w in waiting {
            tx.execute(
                "INSERT OR IGNORE INTO task_unblocks (fix_task_id, waiting_task_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![fix.to_string(), w.to_string(), ts],
            )?;
        }
        Ok(())
    }

    pub(super) fn unblock_pending_impl(&self) -> Result<Vec<UnblockLink>, StoreError> {
        self.with_read_conn(|conn| {
            query_links(
                conn,
                &format!(
                    "SELECT {COLUMNS} FROM task_unblocks WHERE fired_at IS NULL \
                     ORDER BY fix_task_id, waiting_task_id"
                ),
                [],
            )
        })
    }

    pub(super) fn unblocks_of_impl(&self, fix: TaskId) -> Result<Vec<UnblockLink>, StoreError> {
        self.with_read_conn(|conn| {
            query_links(
                conn,
                &format!(
                    "SELECT {COLUMNS} FROM task_unblocks WHERE fix_task_id = ?1 \
                     ORDER BY waiting_task_id"
                ),
                params![fix.to_string()],
            )
        })
    }

    pub(super) fn unblock_fire_impl(&self, fire: &UnblockFire) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // D4: 発火していない行だけを更新する（同じ結びを二度発火させない）。
        let updated = tx.execute(
            "UPDATE task_unblocks SET fired_release = ?3, fired_action = ?4, fired_at = ?5 \
             WHERE fix_task_id = ?1 AND waiting_task_id = ?2 AND fired_at IS NULL",
            params![
                fire.fix_task_id.to_string(),
                fire.waiting_task_id.to_string(),
                fire.release,
                fire.action.as_str(),
                format_rfc3339(fire.now)?,
            ],
        )?;
        if updated == 0 {
            return Ok(false);
        }
        if let Some(comment) = &fire.comment {
            let transition =
                (fire.action == UnblockAction::Reopened).then(|| (Trigger::Reopen, Vec::new()));
            Self::comment_add_tx(&tx, comment, transition)?;
        } else if fire.action == UnblockAction::Reopened {
            Self::apply_transition_tx(&tx, fire.waiting_task_id, Trigger::Reopen, Vec::new())?;
        }
        tx.commit()?;
        Ok(true)
    }
}
