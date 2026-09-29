use rusqlite::{Connection, params};

use crate::integrations::{IntegrationId, IntegrationMethod, IntegrationState, TaskIntegration};
use crate::model::TaskId;
use crate::repos::RepoId;

use super::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

impl SqliteStore {
    // ---- ADR-0043 D5（Phase 54）: 変更の取り込み（`task_integrations`）----
    pub(super) const INTEGRATION_COLUMNS: &'static str = "id, task_id, repo_id, repo_name, method, state, pr_number, \
         pr_url, merged_at, detail, created_at, updated_at";

    pub(super) fn integration_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<TaskIntegration, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let repo_id: Option<String> = row.get(2)?;
        let method_col: String = row.get(4)?;
        let state_col: String = row.get(5)?;
        let (Ok(id), Ok(task_id), Some(method), Some(state)) = (
            id.parse::<IntegrationId>(),
            task_id.parse::<TaskId>(),
            IntegrationMethod::parse(&method_col),
            IntegrationState::parse(&state_col),
        ) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid task_integrations row: id={id} task_id={task_id} method={method_col} state={state_col}"
            ))));
        };
        let repo_id = match repo_id.as_deref() {
            None => None,
            Some(raw) => match raw.parse::<RepoId>() {
                Ok(v) => Some(v),
                Err(_) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid task_integrations repo_id for {id}: {raw:?}"
                    ))));
                }
            },
        };
        Ok((|| {
            let merged_at: Option<String> = row.get(8)?;
            let created_at: String = row.get(10)?;
            let updated_at: String = row.get(11)?;
            Ok(TaskIntegration {
                id,
                task_id,
                repo_id,
                repo: row.get(3)?,
                method,
                state,
                pr_number: row.get(6)?,
                pr_url: row.get(7)?,
                merged_at: merged_at.as_deref().map(parse_rfc3339).transpose()?,
                detail: row.get(9)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    pub(super) fn integration_put_tx(
        conn: &Connection,
        integration: &TaskIntegration,
    ) -> Result<(), StoreError> {
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO task_integrations ({}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                Self::INTEGRATION_COLUMNS
            ),
            params![
                integration.id.to_string(),
                integration.task_id.to_string(),
                integration.repo_id.map(|r| r.to_string()),
                integration.repo,
                integration.method.as_str(),
                integration.state.as_str(),
                integration.pr_number,
                integration.pr_url,
                integration.merged_at.map(format_rfc3339).transpose()?,
                integration.detail,
                format_rfc3339(integration.created_at)?,
                format_rfc3339(integration.updated_at)?,
            ],
        )?;
        Ok(())
    }

    pub(super) fn integration_query_tx(
        conn: &Connection,
        where_sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<TaskIntegration>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM task_integrations WHERE {where_sql} ORDER BY created_at DESC, id DESC",
            Self::INTEGRATION_COLUMNS
        ))?;
        let rows = stmt.query_map(params, Self::integration_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }
}
