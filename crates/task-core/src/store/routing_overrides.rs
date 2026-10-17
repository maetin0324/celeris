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
        // 自身の行が無ければ `parent_id` の鎖を辿り、最も近い祖先の固定を継承する
        // （campaign root の B/C 固定が子・孫・再試行でも保たれる。全体 config へは落ちない）。
        self.with_read_conn(|conn| {
            let mut current = task_id.to_string();
            for _ in 0..ROUTING_ANCESTOR_LIMIT {
                let json: Option<String> = conn
                    .query_row(
                        "SELECT override_json FROM task_routing_overrides WHERE task_id = ?1",
                        params![current],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(j) = json {
                    return Ok(Some(serde_json::from_str(&j)?));
                }
                let parent: Option<String> = conn
                    .query_row(
                        "SELECT parent_id FROM tasks WHERE id = ?1",
                        params![current],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                match parent {
                    Some(p) => current = p,
                    None => return Ok(None),
                }
            }
            Ok(None)
        })
    }
}

const ROUTING_ANCESTOR_LIMIT: usize = 32;

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
            evidence_mode: crate::route_select::EvidenceMode::Pinned,
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

    #[test]
    fn routing_override_is_inherited_by_descendants_and_nearest_wins() {
        let store = SqliteStore::open_in_memory().unwrap();
        let root = crate::store::tests::sample_task(crate::model::Status::Ready);
        store.insert(&root).unwrap();
        let mut child = crate::store::tests::sample_task(crate::model::Status::Ready);
        child.parent_id = Some(root.id);
        store.insert(&child).unwrap();
        let mut grand = crate::store::tests::sample_task(crate::model::Status::Ready);
        grand.parent_id = Some(child.id);
        store.insert(&grand).unwrap();
        let o = RoutingOverride {
            mode: RoutingMode::Fixed,
            lower_policy: None,
            evidence_mode: crate::route_select::EvidenceMode::Latest,
            evidence_table_id: None,
            online_update: false,
        };
        store
            .set_task_routing_override(root.id, &o, "2026-10-16")
            .unwrap();
        assert_eq!(
            store.task_routing_override(grand.id).unwrap(),
            Some(o.clone())
        );
        let nearer = RoutingOverride {
            mode: RoutingMode::DecisionAssisted,
            ..o.clone()
        };
        store
            .set_task_routing_override(child.id, &nearer, "2026-10-16")
            .unwrap();
        assert_eq!(store.task_routing_override(grand.id).unwrap(), Some(nearer));
        assert_eq!(store.task_routing_override(root.id).unwrap(), Some(o));
    }

    #[test]
    fn evidence_mode_none_survives_child_grandchild_and_retry_resolution() {
        use crate::route_select::{EvidenceMode, resolve_routing};
        let store = SqliteStore::open_in_memory().unwrap();
        let root = crate::store::tests::sample_task(crate::model::Status::Ready);
        store.insert(&root).unwrap();
        let mut child = crate::store::tests::sample_task(crate::model::Status::Ready);
        child.parent_id = Some(root.id);
        store.insert(&child).unwrap();
        let mut grand = crate::store::tests::sample_task(crate::model::Status::Ready);
        grand.parent_id = Some(child.id);
        store.insert(&grand).unwrap();
        let o = RoutingOverride {
            mode: RoutingMode::DecisionAssisted,
            lower_policy: None,
            evidence_mode: EvidenceMode::None,
            evidence_table_id: None,
            online_update: false,
        };
        store
            .set_task_routing_override(root.id, &o, "2026-10-17")
            .unwrap();
        // retry は同じ task id を再実行するので同じ解決を通る。
        for id in [root.id, child.id, grand.id, grand.id] {
            let got = store.task_routing_override(id).unwrap();
            assert_eq!(got.as_ref(), Some(&o));
            let r = resolve_routing(RoutingMode::Fixed, got.as_ref());
            assert_eq!(r.evidence_mode, EvidenceMode::None);
            assert_eq!(r.evidence_table_id, None);
        }
    }
}
