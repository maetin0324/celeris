//! ADR 2026-11-10-taskstore-consumer-ports D2: task を読む能力（一覧のページ・1 task の event 行）。
//! `get` は具体型で呼ぶ file が多く（D2）、`TaskStore` に残す。
//!
//! `TaskStore` の supertrait（`dyn TaskStore`・`T: TaskStore` の呼び手は import なしで使える）。
//! task の一覧と event 行だけを読む consumer（celeris-mcp の `tools/tasks`・`tools/projects`）はこの trait に依存する。
//! 実装（SQL）は下の `impl TaskQuery for SqliteStore` にだけある（前例は `model_catalog.rs`）。cursor・
//! 絞り込みの部品は `query.rs`、行の変換は `tasks.rs` の `row_to_task`・`events.rs` の `read_event_rows` を使う。

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter};

use crate::model::{Task, TaskId};

use super::events::read_event_rows;
use super::query::{
    CursorPayload, ListFilter, ListOrder, Page, decode_cursor, encode_cursor, filter_predicate,
    keyset_predicate, order_by_sql, u64_to_i64, usize_to_i64,
};
use super::{EventRow, SqliteStore, StoreError};

pub trait TaskQuery: Send + Sync {
    /// ADR-0013 D10: `filter` に一致する `tasks` を `order` で keyset ページングして返す。`cursor` は
    /// 前回の `Page::next_cursor`（不透明な文字列）。不正な `cursor` は `StoreError::Invalid`。
    fn list_page(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError>;
    /// ADR-0013（Phase 9b）: 1 タスクのイベントを `seq` 昇順で、`after_seq` より後（`None` なら最初から）最大 `limit` 件、
    /// グローバル `id` と `ts` 付きで返す（API の `GET /tasks/{id}/events` と run の要約が使う）。
    fn event_rows_for(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError>;
}

impl TaskQuery for SqliteStore {
    fn list_page(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let (filter_sql, filter_params) = filter_predicate(filter);

            let total: i64 = {
                let sql = format!("SELECT COUNT(*) FROM tasks WHERE {filter_sql}");
                conn.query_row(&sql, params_from_iter(filter_params.iter()), |row| {
                    row.get(0)
                })?
            };

            let mut where_sql = format!("({filter_sql})");
            let mut query_params = filter_params;
            if let Some(c) = cursor {
                let payload = decode_cursor(c)?;
                let (keyset_sql, keyset_params) = keyset_predicate(order, &payload);
                where_sql.push_str(&format!(" AND ({keyset_sql})"));
                query_params.extend(keyset_params);
            }

            let order_sql = order_by_sql(order);
            let fetch_limit = usize_to_i64(limit.saturating_add(1));
            let sql =
                format!("SELECT json FROM tasks WHERE {where_sql} ORDER BY {order_sql} LIMIT ?");
            query_params.push(SqlValue::Integer(fetch_limit));

            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(query_params.iter()), |row| {
                row.get::<_, String>(0)
            })?;
            let mut items = Vec::new();
            for row in rows {
                items.push(Self::row_to_task(row?)?);
            }

            let has_more = items.len() > limit;
            if has_more {
                items.truncate(limit);
            }
            let next_cursor = if has_more {
                match items.last() {
                    Some(last) => Some(encode_cursor(&CursorPayload::from_task(last)?)?),
                    None => None,
                }
            } else {
                None
            };

            Ok(Page {
                items,
                next_cursor,
                total: total as u64,
            })
        })
    }

    fn event_rows_for(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, seq, ts, json FROM events WHERE task_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
            )?;
            let after: i64 = after_seq.map(u64_to_i64).unwrap_or(-1);
            read_event_rows(stmt.query(params![task_id.to_string(), after, usize_to_i64(limit)])?)
        })
    }
}
