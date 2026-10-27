//! ADR 2026-10-27-web-terminal D4: 表 `terminal_session_events`（migration 0099。追記専用）。
//!
//! web 端末の開始（`opened`）と終了（`closed`）を 1 行ずつ書く。入出力・token・パスワードは扱わない。
//! UPDATE・DELETE は表の trigger が拒む。1 端末に `opened`・`closed` はそれぞれ高々 1 行（一意制約）。

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{SqliteStore, StoreError};

/// 監査の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSessionEvent {
    pub session_id: String,
    pub ts: String,
    /// `opened` / `closed`。
    pub kind: String,
    /// `local` / `cluster:<id>`。
    pub target: String,
    /// 主体（`human:web`）。
    pub actor: String,
    /// 接続元（gateway が渡す Host と address）。
    pub client: Option<String>,
    /// 終了理由（`closed` だけ）。
    pub reason: Option<String>,
    pub duration_secs: Option<i64>,
    pub exit_code: Option<i64>,
}

const COLUMNS: &str =
    "session_id, ts, kind, target, actor, client, reason, duration_secs, exit_code";

impl SqliteStore {
    /// 1 行を追記する。同じ session・kind の 2 行目は `Ok(false)`（書かない）。
    pub fn terminal_session_event_append(
        &self,
        event: &TerminalSessionEvent,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let n = conn.execute(
            &format!(
                "INSERT OR IGNORE INTO terminal_session_events ({COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            ),
            params![
                event.session_id,
                event.ts,
                event.kind,
                event.target,
                event.actor,
                event.client,
                event.reason,
                event.duration_secs,
                event.exit_code,
            ],
        )?;
        Ok(n == 1)
    }

    /// session の `opened` 行。
    pub fn terminal_session_opened(
        &self,
        session_id: &str,
    ) -> Result<Option<TerminalSessionEvent>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(conn
                .query_row(
                    &format!(
                        "SELECT {COLUMNS} FROM terminal_session_events \
                         WHERE session_id = ?1 AND kind = 'opened'"
                    ),
                    params![session_id],
                    read_row,
                )
                .optional()?)
        })
    }

    /// 新しい順に `limit` 行。
    pub fn terminal_session_events_recent(
        &self,
        limit: u32,
    ) -> Result<Vec<TerminalSessionEvent>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM terminal_session_events ORDER BY id DESC LIMIT ?1"
            ))?;
            let rows = stmt.query_map(params![limit], read_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerminalSessionEvent> {
    Ok(TerminalSessionEvent {
        session_id: row.get(0)?,
        ts: row.get(1)?,
        kind: row.get(2)?,
        target: row.get(3)?,
        actor: row.get(4)?,
        client: row.get(5)?,
        reason: row.get(6)?,
        duration_secs: row.get(7)?,
        exit_code: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(id: &str) -> TerminalSessionEvent {
        TerminalSessionEvent {
            session_id: id.into(),
            ts: "2026-10-27T00:00:00Z".into(),
            kind: "opened".into(),
            target: "local".into(),
            actor: "human:web".into(),
            client: Some("localhost:7720 127.0.0.1".into()),
            reason: None,
            duration_secs: None,
            exit_code: None,
        }
    }

    #[test]
    fn append_only_rows() {
        let store = SqliteStore::open_in_memory().unwrap();
        assert!(store.terminal_session_event_append(&opened("t1")).unwrap());
        // 同じ session の 2 行目の opened は書かない。
        assert!(!store.terminal_session_event_append(&opened("t1")).unwrap());
        let closed = TerminalSessionEvent {
            kind: "closed".into(),
            reason: Some("idle_timeout".into()),
            duration_secs: Some(1800),
            exit_code: Some(129),
            ..opened("t1")
        };
        assert!(store.terminal_session_event_append(&closed).unwrap());
        assert_eq!(
            store.terminal_session_opened("t1").unwrap(),
            Some(opened("t1"))
        );
        let recent = store.terminal_session_events_recent(10).unwrap();
        assert_eq!(recent, vec![closed, opened("t1")]);
        // UPDATE・DELETE は trigger が拒む。
        let conn = store.lock().unwrap();
        let update = conn.execute("UPDATE terminal_session_events SET reason = 'x'", []);
        assert!(update.unwrap_err().to_string().contains("append-only"));
        let delete = conn.execute("DELETE FROM terminal_session_events", []);
        assert!(delete.unwrap_err().to_string().contains("append-only"));
    }
}
