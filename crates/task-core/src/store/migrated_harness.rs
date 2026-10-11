use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::llm_route::HarnessId;
use crate::model::{Event, TaskId};

use super::{SqliteStore, StoreError};

impl SqliteStore {
    /// Resolve legacy task harness values against the declarations loaded by the daemon.
    /// Unresolved values remain on the legacy hint and produce one audit event per task.
    pub fn resolve_migrated_task_harnesses(
        &self,
        registered: &BTreeSet<HarnessId>,
    ) -> Result<usize, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, json FROM tasks WHERE json_extract(json, '$.harness_id') IS NULL AND json_type(json, '$.worker_hint.adapter') = 'text'",
            )?;
            stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut resolved = 0;
        for (id_text, json_text) in rows {
            let task_id: TaskId = id_text.parse().map_err(|error| {
                StoreError::Invalid(format!("invalid task id in task table: {error}"))
            })?;
            let mut value: serde_json::Value = serde_json::from_str(&json_text)?;
            let Some(original) = value
                .pointer("/worker_hint/adapter")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            if registered.contains(&HarnessId::from(original.clone())) {
                value["harness_id"] = serde_json::json!(original);
                resolved += 1;
            } else {
                let prior: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM events WHERE task_id=?1 AND json_extract(json, '$.type')='harness_migration_unresolved' LIMIT 1",
                        params![id_text],
                        |row| row.get(0),
                    )
                    .optional()?;
                if prior.is_none() {
                    Self::append_event_tx(
                        &tx,
                        task_id,
                        &Event::HarnessMigrationUnresolved {
                            task_id: id_text.clone(),
                            original_value: original,
                        },
                    )?;
                }
            }
            tx.execute(
                "UPDATE tasks SET json=?2 WHERE id=?1",
                params![id_text, serde_json::to_string(&value)?],
            )?;
        }
        tx.commit()?;
        Ok(resolved)
    }
}
