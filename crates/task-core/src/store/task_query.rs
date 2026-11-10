//! ADR 2026-11-10-taskstore-consumer-ports D2: task を読む能力（一覧のページ・1 task の event 行）。
//! `get` は具体型で呼ぶ file が多く（D2）、`TaskStore` に残す。
//!
//! `TaskStore` の supertrait（`dyn TaskStore`・`T: TaskStore` の呼び手は import なしで使える）。
//! task の一覧と event 行だけを読む consumer（celeris-mcp の `tools/tasks`・`tools/projects`）はこの trait に依存する。
//! 実装は `tasks.rs`・`events.rs` の `<method>_impl` への 1 行転送で、SQL はここに無い。

use crate::model::{Task, TaskId};

use super::query::{ListFilter, ListOrder, Page};
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
        self.list_page_impl(filter, order, cursor, limit)
    }

    fn event_rows_for(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        self.event_rows_for_impl(task_id, after_seq, limit)
    }
}
