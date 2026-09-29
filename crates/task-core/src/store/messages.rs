use crate::comment::{CommentAuthorKind, TaskComment};
use crate::message::{Message, MessageId, MessageRole};
use crate::model::TaskId;
use crate::org::ProjectId;

use super::{SqliteStore, StoreError, parse_rfc3339};

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
}
