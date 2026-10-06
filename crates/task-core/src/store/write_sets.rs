//! ADR-0130 D1-D2: durable write-set observations. Git collection belongs to callers.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::execution_plan::ExecutionPlanSpec;
use crate::model::TaskId;
use crate::write_set::{
    WriteSetRecord, WriteSetStatus, inherit_write_paths, normalize_write_paths,
};

use super::{SqliteStore, StoreError};

/// Bound for the child-task → parent-unit inheritance walk (ADR-0079 max_depth is 3).
const HINT_INHERIT_DEPTH: usize = 8;

impl SqliteStore {
    /// ADR-0130 D1: set (or clear with `None` / empty) a task's explicit hint.
    /// Values are validated and normalized; invalid prefixes are rejected.
    pub fn set_task_expected_write_paths(
        &self,
        task_id: TaskId,
        paths: Option<&[String]>,
        now: &str,
    ) -> Result<(), StoreError> {
        let normalized = match paths {
            Some(paths) if !paths.is_empty() => {
                Some(normalize_write_paths(paths).map_err(StoreError::Invalid)?)
            }
            _ => None,
        };
        let conn = self.lock()?;
        Self::set_task_expected_write_paths_tx(&conn, task_id, normalized, now)
    }

    /// Write an already-normalized hint inside a caller's transaction (ADR 2026-10-05 D3).
    pub fn set_task_expected_write_paths_tx(
        conn: &rusqlite::Connection,
        task_id: TaskId,
        normalized: Option<Vec<String>>,
        now: &str,
    ) -> Result<(), StoreError> {
        match normalized {
            Some(paths) => {
                conn.execute(
                    "INSERT INTO task_write_hints (task_id, paths_json, updated_at) \
                     VALUES (?1, ?2, ?3) ON CONFLICT(task_id) DO UPDATE SET \
                     paths_json = excluded.paths_json, updated_at = excluded.updated_at",
                    params![task_id.to_string(), serde_json::to_string(&paths)?, now],
                )?;
            }
            None => {
                conn.execute(
                    "DELETE FROM task_write_hints WHERE task_id = ?1",
                    params![task_id.to_string()],
                )?;
            }
        }
        Ok(())
    }

    /// The task's own explicit hint (no inheritance).
    pub fn task_expected_write_paths(
        &self,
        task_id: TaskId,
    ) -> Result<Option<Vec<String>>, StoreError> {
        self.with_read_conn(|conn| Self::task_hint_conn(conn, &task_id.to_string()))
    }

    /// ADR-0130 D1: the task's own hint, else (for a child task) its parent unit's
    /// effective hint. Returns `None` when nothing up the tree specifies one.
    pub fn effective_task_write_paths(
        &self,
        task_id: TaskId,
    ) -> Result<Option<Vec<String>>, StoreError> {
        self.with_read_conn(|conn| Self::effective_task_hint_conn(conn, &task_id.to_string(), 0))
    }

    /// ADR-0130 D1: the WU's /3 plan unit hint, else its task's effective hint.
    pub fn work_unit_expected_write_paths(
        &self,
        work_unit_id: &str,
    ) -> Result<Option<Vec<String>>, StoreError> {
        self.with_read_conn(|conn| Self::work_unit_hint_conn(conn, work_unit_id, 0))
    }

    fn task_hint_conn(
        conn: &rusqlite::Connection,
        task_id: &str,
    ) -> Result<Option<Vec<String>>, StoreError> {
        let json: Option<String> = conn
            .query_row(
                "SELECT paths_json FROM task_write_hints WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
    }

    fn effective_task_hint_conn(
        conn: &rusqlite::Connection,
        task_id: &str,
        depth: usize,
    ) -> Result<Option<Vec<String>>, StoreError> {
        let own = Self::task_hint_conn(conn, task_id)?;
        if own.as_ref().is_some_and(|p| !p.is_empty()) || depth >= HINT_INHERIT_DEPTH {
            return Ok(own);
        }
        let parent_unit: Option<String> = conn
            .query_row(
                "SELECT id FROM work_units WHERE child_task_id = ?1 ORDER BY created_at LIMIT 1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()?;
        let inherited = match parent_unit {
            Some(unit) => Self::work_unit_hint_conn(conn, &unit, depth + 1)?,
            None => None,
        };
        Ok(inherit_write_paths(own.as_deref(), inherited.as_deref()))
    }

    fn work_unit_hint_conn(
        conn: &rusqlite::Connection,
        work_unit_id: &str,
        depth: usize,
    ) -> Result<Option<Vec<String>>, StoreError> {
        let row: Option<(String, String, Option<String>)> = conn
            .query_row(
                "SELECT w.task_id, w.key, p.json FROM work_units w \
                 LEFT JOIN execution_plans p ON p.id = w.plan_id WHERE w.id = ?1",
                params![work_unit_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((task_id, key, plan_json)) = row else {
            return Ok(None);
        };
        let own = match plan_json {
            Some(json) => {
                let spec: ExecutionPlanSpec = serde_json::from_str(&json)?;
                spec.units
                    .into_iter()
                    .find(|unit| unit.key == key)
                    .and_then(|unit| unit.expected_write_paths)
            }
            None => None,
        };
        if own.as_ref().is_some_and(|p| !p.is_empty()) || depth >= HINT_INHERIT_DEPTH {
            return Ok(own);
        }
        let inherited = Self::effective_task_hint_conn(conn, &task_id, depth + 1)?;
        Ok(inherit_write_paths(own.as_deref(), inherited.as_deref()))
    }

    pub fn record_run_write_set(&self, record: &WriteSetRecord) -> Result<(), StoreError> {
        self.record_write_set(record, false)
    }

    pub fn record_work_unit_write_set(&self, record: &WriteSetRecord) -> Result<(), StoreError> {
        self.record_write_set(record, true)
    }

    fn record_write_set(&self, record: &WriteSetRecord, work_unit: bool) -> Result<(), StoreError> {
        let table = if work_unit {
            "work_unit_write_sets"
        } else {
            "run_write_sets"
        };
        let key = if work_unit { "work_unit_id" } else { "run_id" };
        if record.status == WriteSetStatus::Complete
            && (record.base_sha.is_none() || record.head_sha.is_none())
        {
            return Err(StoreError::Invalid(
                "complete write-set needs both Git SHAs".into(),
            ));
        }
        let paths: Vec<_> = record
            .paths
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(Option<String>, Option<String>, String)> = tx
            .query_row(
                &format!("SELECT base_sha, head_sha, status FROM {table} WHERE {key} = ?1 AND repo_id = ?2"),
                params![record.owner_id, record.repo_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((base, head, status)) = &prior {
            if base != &record.base_sha || head != &record.head_sha {
                return Err(StoreError::Invalid(
                    "write-set snapshot SHAs cannot change".into(),
                ));
            }
            if status == "complete" && record.status != WriteSetStatus::Complete {
                return Ok(());
            }
        }
        let columns = if work_unit {
            "work_unit_id, task_id, repo_id, base_sha, head_sha, paths_json, status, reason, recorded_at"
        } else {
            "run_id, task_id, work_unit_id, repo_id, base_sha, head_sha, paths_json, status, reason, recorded_at"
        };
        let placeholders = if work_unit {
            "?1,?2,?3,?4,?5,?6,?7,?8,?9"
        } else {
            "?1,?2,?3,?4,?5,?6,?7,?8,?9,?10"
        };
        let sql = format!(
            "INSERT INTO {table} ({columns}) VALUES ({placeholders}) \
                           ON CONFLICT({key},repo_id) DO UPDATE SET paths_json=excluded.paths_json, status=excluded.status, reason=excluded.reason, recorded_at=excluded.recorded_at"
        );
        let paths_json = serde_json::to_string(&paths)?;
        if work_unit {
            tx.execute(
                &sql,
                params![
                    record.owner_id,
                    record.task_id.to_string(),
                    record.repo_id.to_string(),
                    record.base_sha,
                    record.head_sha,
                    paths_json,
                    record.status.as_str(),
                    record.reason,
                    record.recorded_at
                ],
            )?;
        } else {
            tx.execute(
                &sql,
                params![
                    record.owner_id,
                    record.task_id.to_string(),
                    record.work_unit_id,
                    record.repo_id.to_string(),
                    record.base_sha,
                    record.head_sha,
                    paths_json,
                    record.status.as_str(),
                    record.reason,
                    record.recorded_at
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn run_write_sets(&self, run_id: &str) -> Result<Vec<WriteSetRecord>, StoreError> {
        self.read_write_sets(run_id, false)
    }

    pub fn work_unit_write_sets(
        &self,
        work_unit_id: &str,
    ) -> Result<Vec<WriteSetRecord>, StoreError> {
        self.read_write_sets(work_unit_id, true)
    }

    fn read_write_sets(
        &self,
        owner_id: &str,
        work_unit: bool,
    ) -> Result<Vec<WriteSetRecord>, StoreError> {
        let table = if work_unit {
            "work_unit_write_sets"
        } else {
            "run_write_sets"
        };
        let key = if work_unit { "work_unit_id" } else { "run_id" };
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT task_id, work_unit_id, repo_id, base_sha, head_sha, paths_json, status, reason, recorded_at FROM {table} WHERE {key}=?1 ORDER BY repo_id"
            ))?;
            let rows = stmt.query_map(params![owner_id], |row| {
                Ok((
                    row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?, row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?, row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (task, work_unit_id, repo, base_sha, head_sha, paths, status, reason, recorded_at) = row?;
                out.push(WriteSetRecord {
                    owner_id: owner_id.to_owned(),
                    task_id: task.parse().map_err(|_| StoreError::Invalid("invalid write-set task ID".into()))?,
                    work_unit_id,
                    repo_id: repo.parse().map_err(|_| StoreError::Invalid("invalid write-set repo ID".into()))?,
                    base_sha,
                    head_sha,
                    paths: serde_json::from_str(&paths)?,
                    status: match status.as_str() {
                        "complete" => WriteSetStatus::Complete,
                        "incomplete" => WriteSetStatus::Incomplete,
                        "unavailable" => WriteSetStatus::Unavailable,
                        _ => return Err(StoreError::Invalid("invalid write-set status".into())),
                    },
                    reason,
                    recorded_at,
                });
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TaskId;
    use crate::repos::RepoId;

    #[test]
    fn write_set_record_is_durable_and_sha_immutable() {
        let store = SqliteStore::open_in_memory().unwrap();
        let mut record = WriteSetRecord {
            owner_id: "run-1".into(),
            task_id: TaskId::new(),
            work_unit_id: Some("wu-1".into()),
            repo_id: RepoId::new(),
            base_sha: Some("base".into()),
            head_sha: Some("head".into()),
            paths: vec!["src/b.rs".into(), "src/a.rs".into(), "src/a.rs".into()],
            status: WriteSetStatus::Complete,
            reason: None,
            recorded_at: "2026-10-02T00:00:00Z".into(),
        };
        store.record_run_write_set(&record).unwrap();
        store.record_run_write_set(&record).unwrap();
        assert_eq!(
            store.run_write_sets("run-1").unwrap()[0].paths,
            vec!["src/a.rs", "src/b.rs"]
        );
        record.head_sha = Some("other".into());
        assert!(store.record_run_write_set(&record).is_err());

        record.head_sha = Some("head".into());
        record.owner_id = "wu-1".into();
        store.record_work_unit_write_set(&record).unwrap();
        assert_eq!(
            store.work_unit_write_sets("wu-1").unwrap()[0].status,
            WriteSetStatus::Complete
        );
    }

    #[test]
    fn write_set_hints_resolve_from_plan_unit_task_and_parent_unit() {
        use crate::execution_plan::{
            ExecutionLimits, ExecutionPlanRow, PlanOrigin, PlanStatus, materialize_work_units,
            validate,
        };
        use crate::model::{Event, Status};
        use crate::store::TaskStore;
        let store = SqliteStore::open_in_memory().unwrap();
        let task = crate::store::tests::sample_task(Status::Ready);
        store.insert(&task).unwrap();
        let now = "2026-10-02T00:00:00Z";

        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/execution-plan/v3-browser.json"
        ))
        .unwrap();
        let mut spec: ExecutionPlanSpec = serde_json::from_str(&text).unwrap();
        let p1 = spec.units.iter().position(|u| u.key == "p1").unwrap();
        spec.units[p1].expected_write_paths = Some(vec!["crates/a/".into()]);
        let limits = ExecutionLimits {
            tree: crate::tree::TreeLimits {
                enabled: true,
                ..crate::tree::TreeLimits::default()
            },
            ..ExecutionLimits::default()
        };
        let validated = validate(&spec, limits, &[]).unwrap();
        let plan_id = "plan-hints".to_string();
        let mut n = 0;
        let rows = materialize_work_units(
            &task.id.to_string(),
            &plan_id,
            &validated.spec,
            &validated.topological_order,
            now,
            &mut |wu| {
                n += 1;
                format!("wu-{n}-{}", wu.key)
            },
        );
        let id_of = |key: &str| {
            rows.iter()
                .find(|r| r.key == key)
                .map(|r| r.id.clone())
                .unwrap()
        };
        let (p1_id, p3_id) = (id_of("p1"), id_of("p3"));
        let plan = ExecutionPlanRow {
            id: plan_id.clone(),
            task_id: task.id.to_string(),
            version: 1,
            origin: PlanOrigin::Human,
            planner_run_id: None,
            status: PlanStatus::Active,
            spec: validated.spec,
            created_at: now.into(),
            superseded_at: None,
        };
        let event = Event::ExecutionPlanned {
            plan_id,
            version: 1,
            origin: PlanOrigin::Human,
            supersedes: None,
            reason: None,
            plan: Box::new(plan.spec.clone()),
        };
        store
            .execution_plan_adopt(task.id, plan, rows, Vec::new(), event)
            .unwrap();

        // 未指定は従来どおり hint なし。
        assert_eq!(store.work_unit_expected_write_paths(&p3_id).unwrap(), None);
        assert_eq!(store.effective_task_write_paths(task.id).unwrap(), None);
        // unit の明示値（正規化済み）が WU の hint、無ければ task の hint を継ぐ。
        let crates_a = Some(vec!["crates/a".to_string()]);
        assert_eq!(
            store.work_unit_expected_write_paths(&p1_id).unwrap(),
            crates_a
        );
        store
            .set_task_expected_write_paths(task.id, Some(&["docs/".into(), "docs".into()]), now)
            .unwrap();
        let docs = Some(vec!["docs".to_string()]);
        assert_eq!(store.task_expected_write_paths(task.id).unwrap(), docs);
        assert_eq!(store.work_unit_expected_write_paths(&p3_id).unwrap(), docs);
        assert_eq!(
            store.work_unit_expected_write_paths(&p1_id).unwrap(),
            crates_a
        );

        // 子 task は自分の明示値が無ければ親 unit の値を継ぐ。
        let child = crate::store::tests::sample_task(Status::Ready);
        store.insert(&child).unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE work_units SET child_task_id = ?1 WHERE id = ?2",
                params![child.id.to_string(), p1_id],
            )
            .unwrap();
        assert_eq!(store.task_expected_write_paths(child.id).unwrap(), None);
        assert_eq!(
            store.effective_task_write_paths(child.id).unwrap(),
            crates_a
        );
        store
            .set_task_expected_write_paths(child.id, Some(&["crates/a/src".into()]), now)
            .unwrap();
        assert_eq!(
            store.effective_task_write_paths(child.id).unwrap(),
            Some(vec!["crates/a/src".to_string()])
        );

        // 不正な形式は拒否し、空配列は消去（= 未指定）。
        assert!(
            store
                .set_task_expected_write_paths(task.id, Some(&["../x".into()]), now)
                .is_err()
        );
        assert_eq!(store.task_expected_write_paths(task.id).unwrap(), docs);
        store
            .set_task_expected_write_paths(task.id, Some(&[]), now)
            .unwrap();
        assert_eq!(store.work_unit_expected_write_paths(&p3_id).unwrap(), None);
    }
}
