//! ADR 2026-10-16-mlsys-measure-task-routing D2: task 単位の routing 固定（`task_routing_overrides`）。
//! 作成時に 1 行書き、以後は変えない。行の無い task は従来どおり config の mode に従う。

use rusqlite::{OptionalExtension, params};

use crate::model::TaskId;
use crate::route_select::RoutingOverride;

use super::{SqliteStore, StoreError};

impl SqliteStore {
    /// task の作成と同じ transaction で固定を書く（`POST /tasks` の `routing_override`）。
    pub fn set_task_routing_override_tx(
        conn: &rusqlite::Connection,
        task_id: TaskId,
        routing_override: &RoutingOverride,
        now: &str,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO task_routing_overrides (task_id, override_json, created_at) VALUES (?1, ?2, ?3)",
            params![
                task_id.to_string(),
                serde_json::to_string(routing_override)?,
                now
            ],
        )?;
        Ok(())
    }

    /// `set_task_routing_override_tx` の単独版（試験・取り込み用。task の作成は API が tx 版で一緒に書く）。
    pub fn set_task_routing_override(
        &self,
        task_id: TaskId,
        routing_override: &RoutingOverride,
        now: &str,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::set_task_routing_override_tx(&conn, task_id, routing_override, now)
    }

    pub(super) fn task_routing_override_impl(
        &self,
        task_id: TaskId,
    ) -> Result<Option<RoutingOverride>, StoreError> {
        self.with_read_conn(|conn| {
            let json: Option<String> = conn
                .query_row(
                    "SELECT override_json FROM task_routing_overrides WHERE task_id = ?1",
                    params![task_id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            json.map(|j| serde_json::from_str(&j).map_err(StoreError::from))
                .transpose()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskStore;
    use crate::route_select::RoutingMode;

    #[test]
    fn routing_override_round_trips_and_absent_is_none() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = crate::store::tests::sample_task(crate::model::Status::Ready);
        store.insert(&task).unwrap();
        assert_eq!(store.task_routing_override(task.id).unwrap(), None);
        let o = RoutingOverride {
            mode: RoutingMode::Fixed,
            lower_policy: None,
            evidence_table_id: Some("ev-1".into()),
            online_update: false,
        };
        {
            let conn = store.lock().unwrap();
            SqliteStore::set_task_routing_override_tx(&conn, task.id, &o, "2026-10-16").unwrap();
            // 固定は作成時の 1 回だけ（上書きしない）。
            assert!(
                SqliteStore::set_task_routing_override_tx(&conn, task.id, &o, "2026-10-16")
                    .is_err()
            );
        }
        assert_eq!(store.task_routing_override(task.id).unwrap(), Some(o));
    }
}
