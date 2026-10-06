use rusqlite::{OptionalExtension, TransactionBehavior, params, params_from_iter};
use time::OffsetDateTime;

use crate::comment::{CommentAuthorKind, TaskComment};
use crate::message::{Message, MessageId, MessageRole};
use crate::model::{Event, TaskId};
use crate::org::ProjectId;
use crate::transition::{Outcome, Trigger};

use super::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

impl SqliteStore {
    /// ADR-0033 D4（Phase 24）: `messages` の 1 行。
    pub(super) fn message_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<Message, StoreError>> {
        let id: String = row.get(0)?;
        let project_id: Option<String> = row.get(2)?;
        let role_col: String = row.get(3)?;
        let created_at: String = row.get(6)?;
        let (Ok(id), Some(role)) = (id.parse::<MessageId>(), MessageRole::parse(&role_col)) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid message row: id={id} role={role_col}"
            ))));
        };
        let project_id = match project_id {
            Some(raw) => match raw.parse::<ProjectId>() {
                Ok(p) => Some(p),
                Err(_) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid message row: id={id} project_id={raw}"
                    ))));
                }
            },
            None => None,
        };
        // migration 0007（R4）: 対話用タスクの id。導入前の行は NULL。
        let task_id: Option<String> = row.get(7)?;
        let task_id = match task_id {
            Some(raw) => match raw.parse::<TaskId>() {
                Ok(t) => Some(t),
                Err(_) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid message row: id={id} task_id={raw}"
                    ))));
                }
            },
            None => None,
        };
        // ADR-0048 D3（Phase 60b）: `metadata_json`（actions の実行結果）は補助表示用なので、
        // 形が壊れていても run は落とさない（`None` として読む）。
        let metadata_json: Option<String> = row.get(8)?;
        let metadata = metadata_json.and_then(|raw| serde_json::from_str(&raw).ok());
        Ok((|| {
            Ok(Message {
                id,
                node_id: row.get(1)?,
                project_id,
                role,
                text: row.get(4)?,
                run_id: row.get(5)?,
                task_id,
                metadata,
                created_at: parse_rfc3339(&created_at)?,
            })
        })())
    }

    /// ADR-0044 D2: `task_comments` の 1 行を `TaskComment` にする。
    pub(super) fn comment_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<TaskComment, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let author_kind: String = row.get(2)?;
        let author: Option<String> = row.get(3)?;
        let body: String = row.get(4)?;
        let run_id: Option<String> = row.get(5)?;
        let created_at: String = row.get(6)?;
        Ok((|| {
            let Some(author_kind) = CommentAuthorKind::parse(&author_kind) else {
                return Err(StoreError::Invalid(format!(
                    "invalid author_kind in task_comments: {author_kind}"
                )));
            };
            Ok(TaskComment {
                id: id
                    .parse()
                    .map_err(|_| StoreError::Invalid(format!("invalid comment id: {id}")))?,
                task_id: Self::parse_id(&task_id)?,
                author_kind,
                author,
                body,
                run_id,
                created_at: parse_rfc3339(&created_at)?,
            })
        })())
    }

    pub(super) fn message_append_impl(&self, message: &Message) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let metadata_json = message
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            "INSERT INTO messages (id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                message.id.to_string(),
                message.node_id,
                message.project_id.map(|p| p.to_string()),
                message.role.as_str(),
                message.text,
                message.run_id,
                format_rfc3339(message.created_at)?,
                message.task_id.map(|t| t.to_string()),
                metadata_json,
            ],
        )?;
        Ok(())
    }

    pub(super) fn message_list_impl(
        &self,
        node_id: &str,
        project_id: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        self.with_read_conn(|conn| {
            // 新しい順に `limit` 件取ってから古い順に戻す（直近のやり取りを時系列で渡すため）。
            let sql = match project_id {
                Some(_) => {
                    "SELECT id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json FROM messages \
                     WHERE node_id = ?1 AND project_id = ?2 ORDER BY created_at DESC, id DESC LIMIT ?3"
                }
                None => {
                    "SELECT id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json FROM messages \
                     WHERE node_id = ?1 AND project_id IS NULL ORDER BY created_at DESC, id DESC LIMIT ?3"
                }
            };
            let mut stmt = conn.prepare(sql)?;
            let project = project_id.map(|p| p.to_string()).unwrap_or_default();
            let rows =
                stmt.query_map(params![node_id, project, limit as i64], Self::message_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            out.reverse();
            Ok(out)
        })
    }

    /// ADR-0048 D1（Phase 60a）: Console の一本の流れ用（絞り込みは任意、`after` は閉区間）。
    pub(super) fn message_page_impl(
        &self,
        node_id: Option<&str>,
        project_id: Option<ProjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        self.with_read_conn(|conn| {
            let mut where_sql = String::from("1 = 1");
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(node_id) = node_id {
                where_sql.push_str(" AND node_id = ?");
                args.push(Box::new(node_id.to_string()));
            }
            if let Some(project_id) = project_id {
                where_sql.push_str(" AND project_id = ?");
                args.push(Box::new(project_id.to_string()));
            }
            if let Some(after) = after {
                where_sql.push_str(" AND created_at >= ?");
                args.push(Box::new(after.to_string()));
            }
            // `after` 有り = 古い順にその先から、無し = 新しい順に `limit` 件取って戻す。
            let order = if after.is_some() { "ASC" } else { "DESC" };
            let sql = format!(
                "SELECT id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json FROM messages \
                 WHERE {where_sql} ORDER BY created_at {order}, id {order} LIMIT ?"
            );
            args.push(Box::new(limit as i64));
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                params_from_iter(args.iter().map(|a| a.as_ref())),
                Self::message_row,
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            if after.is_none() {
                out.reverse();
            }
            Ok(out)
        })
    }

    pub(super) fn console_action_run_claim_impl(
        &self,
        run_id: &str,
        task_id: TaskId,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "INSERT OR IGNORE INTO console_action_runs (run_id, task_id, executed_at) \
             VALUES (?1, ?2, ?3)",
            params![run_id, task_id.to_string(), format_rfc3339(now)?],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn comment_add_impl(
        &self,
        comment: &TaskComment,
        transition: Option<(Trigger, Vec<Event>)>,
    ) -> Result<Option<Outcome>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = Self::comment_add_tx(&tx, comment, transition)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// `comment_add_impl` inside a caller's transaction (ADR 2026-10-05 D3).
    pub fn comment_add_tx(
        tx: &rusqlite::Connection,
        comment: &TaskComment,
        transition: Option<(Trigger, Vec<Event>)>,
    ) -> Result<Option<Outcome>, StoreError> {
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM tasks WHERE id = ?1",
                params![comment.task_id.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(StoreError::Invalid(format!(
                "task not found: {}",
                comment.task_id
            )));
        }
        tx.execute(
            "INSERT INTO task_comments (id, task_id, author_kind, author, body, run_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                comment.id.to_string(),
                comment.task_id.to_string(),
                comment.author_kind.as_str(),
                comment.author.clone(),
                comment.body,
                comment.run_id.clone(),
                format_rfc3339(comment.created_at)?,
            ],
        )?;
        let outcome = match transition {
            Some((trigger, extra_events)) => Some(Self::apply_transition_tx(
                tx,
                comment.task_id,
                trigger,
                extra_events,
            )?),
            None => None,
        };
        Ok(outcome)
    }

    pub(super) fn comments_for_impl(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<TaskComment>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, author_kind, author, body, run_id, created_at FROM task_comments \
                 WHERE task_id = ?1 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::comment_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }
}
