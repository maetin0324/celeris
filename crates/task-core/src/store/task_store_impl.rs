//! `impl TaskStore for SqliteStore`（1 ブロック）。領域へ移した method は `<method>_impl` への 1 行転送。

use std::time::Duration as StdDuration;

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use crate::comment::TaskComment;
use crate::execution_plan::{
    ExecutionPlanRow, RunIndexRole, RunIndexStatus, RunRow, WorkUnitRow, WorkUnitStatus,
};
use crate::instance::{DaemonInstance, InstanceRole, SELECT_INSTANCE, row_to_instance};
use crate::integrations::{IntegrationId, TaskIntegration};
use crate::message::Message;
use crate::model::{Event, Status, Task, TaskId, WorkspaceSpec};
use crate::org::{
    Milestone, MilestoneId, MilestoneStatus, OrgNode, Project, ProjectId, ProjectStatus,
};
use crate::repos::{ProjectRepo, RepoId};
use crate::transition::{Outcome, Trigger};

use super::query::{parse_status, usize_to_i64};
use super::{
    ClusterConnectionRecord, ClusterSettings, EventRow, ExecutionMetricsLatestRun,
    ExecutionMetricsTaskRow, ListFilter, ListOrder, Page, ProjectPlanApply, SqliteStore,
    StoreError, TaskStore, TreeAdoption, format_rfc3339, parse_rfc3339, status_str,
};

impl TaskStore for SqliteStore {
    fn insert(&self, task: &Task) -> Result<(), StoreError> {
        self.insert_impl(task)
    }

    fn get(&self, id: TaskId) -> Result<Option<Task>, StoreError> {
        self.get_impl(id)
    }

    fn list(&self, filter: Option<Status>) -> Result<Vec<Task>, StoreError> {
        self.list_impl(filter)
    }

    fn append_event(&self, task_id: TaskId, event: &Event) -> Result<u64, StoreError> {
        self.append_event_impl(task_id, event)
    }

    fn events_for(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError> {
        self.events_for_impl(task_id)
    }

    fn events_for_with_global_ids(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError> {
        self.events_for_with_global_ids_impl(task_id)
    }

    fn acquire_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError> {
        self.acquire_lease_impl(task_id, worker_run_id, ttl)
    }

    fn release_lease(&self, task_id: TaskId, worker_run_id: &str) -> Result<(), StoreError> {
        self.release_lease_impl(task_id, worker_run_id)
    }

    fn halted_by_pause(&self, task: &Task) -> Result<bool, StoreError> {
        self.halted_by_pause_impl(task)
    }

    fn ready_tasks(&self, limit: usize) -> Result<Vec<Task>, StoreError> {
        self.ready_tasks_impl(limit)
    }

    fn apply_transition_with_events(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        self.apply_transition_with_events_impl(task_id, trigger, extra_events)
    }

    fn complete_plan(
        &self,
        plan_id: TaskId,
        verdict_events: Vec<Event>,
        children: Vec<Task>,
        accept_children: bool,
    ) -> Result<Outcome, StoreError> {
        self.complete_plan_impl(plan_id, verdict_events, children, accept_children)
    }

    fn create_task(&self, task: &Task, extra_events: Vec<Event>) -> Result<(), StoreError> {
        self.create_task_impl(task, extra_events)
    }

    fn project_plan_decide_apply(
        &self,
        plan_task_id: TaskId,
        milestones: &[MilestoneId],
        milestone_status: MilestoneStatus,
        tasks: &[TaskId],
        trigger: Trigger,
        decided_event: Event,
    ) -> Result<(), StoreError> {
        self.project_plan_decide_apply_impl(
            plan_task_id,
            milestones,
            milestone_status,
            tasks,
            trigger,
            decided_event,
        )
    }

    fn project_plan_apply(&self, apply: &ProjectPlanApply) -> Result<(), StoreError> {
        self.project_plan_apply_impl(apply)
    }

    fn delegate_children(
        &self,
        parent_id: TaskId,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError> {
        self.delegate_children_impl(parent_id, run_id, children)
    }

    fn retry_task(&self, original: TaskId, new_task: &Task) -> Result<Vec<TaskId>, StoreError> {
        self.retry_task_impl(original, new_task)
    }

    fn children(&self, parent_id: TaskId) -> Result<Vec<Task>, StoreError> {
        self.children_impl(parent_id)
    }

    fn renew_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError> {
        self.renew_lease_impl(task_id, worker_run_id, ttl)
    }

    fn events_since(&self, after_id: u64, limit: usize) -> Result<Vec<EventRow>, StoreError> {
        self.events_since_impl(after_id, limit)
    }

    fn latest_event_id(&self) -> Result<u64, StoreError> {
        self.latest_event_id_impl()
    }

    fn event_rows_for(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        self.event_rows_for_impl(task_id, after_seq, limit)
    }

    fn list_page(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError> {
        self.list_page_impl(filter, order, cursor, limit)
    }

    fn count_by_status(&self) -> Result<Vec<(Status, u64)>, StoreError> {
        self.count_by_status_impl()
    }

    // ---- ADR-0033 D1: 組織 ----
    fn org_list(&self) -> Result<Vec<OrgNode>, StoreError> {
        self.org_list_impl()
    }

    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError> {
        self.org_get_impl(id)
    }

    fn org_upsert(&self, node: &OrgNode) -> Result<OrgNode, StoreError> {
        self.org_upsert_impl(node)
    }

    fn org_seed(&self, nodes: &[OrgNode]) -> Result<(), StoreError> {
        self.org_seed_impl(nodes)
    }

    fn org_delete(&self, id: &str) -> Result<bool, StoreError> {
        self.org_delete_impl(id)
    }

    // ---- ADR-0033 D2: 案件と途中目標 ----
    fn project_create(&self, project: &Project) -> Result<(), StoreError> {
        self.project_create_impl(project)
    }

    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        self.project_get_impl(id)
    }

    fn project_list(&self) -> Result<Vec<Project>, StoreError> {
        self.project_list_impl()
    }

    fn project_set_status(&self, id: ProjectId, status: ProjectStatus) -> Result<bool, StoreError> {
        self.project_set_status_impl(id, status)
    }

    fn project_set_lifecycle(
        &self,
        id: ProjectId,
        status: ProjectStatus,
        paused_from: Option<Option<ProjectStatus>>,
    ) -> Result<bool, StoreError> {
        self.project_set_lifecycle_impl(id, status, paused_from)
    }

    fn project_set_archived_at(
        &self,
        id: ProjectId,
        at: Option<OffsetDateTime>,
    ) -> Result<bool, StoreError> {
        self.project_set_archived_at_impl(id, at)
    }

    fn project_set_slug(&self, id: ProjectId, slug: &str) -> Result<bool, StoreError> {
        self.project_set_slug_impl(id, slug)
    }

    fn project_set_text(
        &self,
        id: ProjectId,
        title: Option<&str>,
        request: Option<&str>,
    ) -> Result<bool, StoreError> {
        self.project_set_text_impl(id, title, request)
    }

    fn project_set_workspace(
        &self,
        id: ProjectId,
        workspace: Option<&WorkspaceSpec>,
    ) -> Result<bool, StoreError> {
        self.project_set_workspace_impl(id, workspace)
    }

    // ---- ADR-0043 D1（Phase 52）: 案件のリポジトリ ----
    fn repo_create(&self, repo: &ProjectRepo) -> Result<(), StoreError> {
        self.repo_create_impl(repo)
    }

    fn repo_get(&self, id: RepoId) -> Result<Option<ProjectRepo>, StoreError> {
        self.repo_get_impl(id)
    }

    fn repo_list(&self, project_id: ProjectId) -> Result<Vec<ProjectRepo>, StoreError> {
        self.repo_list_impl(project_id)
    }

    fn repo_update(&self, repo: &ProjectRepo) -> Result<bool, StoreError> {
        self.repo_update_impl(repo)
    }

    fn repo_delete(&self, id: RepoId) -> Result<bool, StoreError> {
        self.repo_delete_impl(id)
    }

    fn repo_set_primary(&self, id: RepoId) -> Result<bool, StoreError> {
        self.repo_set_primary_impl(id)
    }

    fn repo_active_tasks(&self, id: RepoId) -> Result<Vec<TaskId>, StoreError> {
        self.repo_active_tasks_impl(id)
    }

    // ---- ADR-0043 D5（Phase 54）: 変更の取り込み ----
    fn integration_put(&self, integration: &TaskIntegration) -> Result<(), StoreError> {
        self.integration_put_impl(integration)
    }

    fn integration_get(&self, id: IntegrationId) -> Result<Option<TaskIntegration>, StoreError> {
        self.integration_get_impl(id)
    }

    fn integration_list_for_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<TaskIntegration>, StoreError> {
        self.integration_list_for_task_impl(task_id)
    }

    fn integration_latest(
        &self,
        task_id: TaskId,
        repo: &str,
    ) -> Result<Option<TaskIntegration>, StoreError> {
        self.integration_latest_impl(task_id, repo)
    }

    fn integration_list_for_project(
        &self,
        project_id: ProjectId,
        limit: usize,
    ) -> Result<Vec<TaskIntegration>, StoreError> {
        self.integration_list_for_project_impl(project_id, limit)
    }

    fn milestone_create(
        &self,
        project_id: ProjectId,
        title: &str,
        description: &str,
        status: MilestoneStatus,
    ) -> Result<Milestone, StoreError> {
        self.milestone_create_impl(project_id, title, description, status)
    }

    fn milestone_set_plan_key(&self, id: MilestoneId, plan_key: &str) -> Result<bool, StoreError> {
        self.milestone_set_plan_key_impl(id, plan_key)
    }

    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError> {
        self.milestone_list_impl(project_id)
    }

    fn milestone_get(&self, id: MilestoneId) -> Result<Option<Milestone>, StoreError> {
        self.milestone_get_impl(id)
    }

    fn milestone_set_status(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
    ) -> Result<bool, StoreError> {
        self.milestone_set_status_impl(id, status)
    }

    fn milestone_transition_status(
        &self,
        id: MilestoneId,
        from: MilestoneStatus,
        to: MilestoneStatus,
    ) -> Result<bool, StoreError> {
        self.milestone_transition_status_impl(id, from, to)
    }

    fn milestone_set_lifecycle(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
        paused_from: Option<Option<MilestoneStatus>>,
    ) -> Result<bool, StoreError> {
        self.milestone_set_lifecycle_impl(id, status, paused_from)
    }

    // ---- ADR-0033 D4（Phase 24）: 対話 ----
    fn message_append(&self, message: &Message) -> Result<(), StoreError> {
        self.message_append_impl(message)
    }

    fn message_list(
        &self,
        node_id: &str,
        project_id: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        self.message_list_impl(node_id, project_id, limit)
    }

    fn message_page(
        &self,
        node_id: Option<&str>,
        project_id: Option<ProjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        self.message_page_impl(node_id, project_id, after, limit)
    }

    // ---- ADR-0048 D3（Phase 60b）: CoS の actions の冪等性 ----
    fn console_action_run_claim(
        &self,
        run_id: &str,
        task_id: TaskId,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        self.console_action_run_claim_impl(run_id, task_id, now)
    }

    // ---- ADR-0040 D4（Phase 47）: `daemon_instances` ----

    // ---- ADR-0044 D1/D2（Phase 53）----
    fn update_task(&self, task: &Task, event: Event) -> Result<Task, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = Self::get_locked(&tx, task.id)? else {
            return Err(StoreError::Invalid(format!("task not found: {}", task.id)));
        };
        // 状態機械が持つ 3 つ（`status` / `attempts` / `lease`）だけは**この tx の中で読んだ行**の値を使う。
        // 編集を組み立てている間にディスパッチャが `acquire_lease` を通していたら、渡された `task` は
        // 古い `ready` / `lease: None` を持っている。そのまま書くと `json` と `status` 列が食い違い、
        // そのタスクは二度と dispatch されず run の結果も捨てられる（Phase 53 の監査で発見）。
        let merged = Task {
            status: current.status,
            attempts: current.attempts,
            lease: current.lease.clone(),
            ..task.clone()
        };
        Self::update_task_tx(&tx, &merged)?;
        Self::append_event_tx(&tx, task.id, &event)?;
        tx.commit()?;
        Ok(merged)
    }

    fn comment_add(
        &self,
        comment: &TaskComment,
        transition: Option<(Trigger, Vec<Event>)>,
    ) -> Result<Option<Outcome>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
                &tx,
                comment.task_id,
                trigger,
                extra_events,
            )?),
            None => None,
        };
        tx.commit()?;
        Ok(outcome)
    }

    fn comments_for(&self, task_id: TaskId) -> Result<Vec<TaskComment>, StoreError> {
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

    fn instance_register(&self, instance: &DaemonInstance) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO daemon_instances \
             (instance_id, \"release\", pid, role, started_at, heartbeat_at, handoff_requested_at, drained_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                instance.instance_id,
                instance.release,
                i64::from(instance.pid),
                instance.role.as_str(),
                format_rfc3339(instance.started_at)?,
                format_rfc3339(instance.heartbeat_at)?,
                instance.handoff_requested_at.map(format_rfc3339).transpose()?,
                instance.drained_at.map(format_rfc3339).transpose()?,
            ],
        )?;
        Ok(())
    }

    fn instance_heartbeat(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET heartbeat_at = ?1 WHERE instance_id = ?2",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_request_handoff(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET handoff_requested_at = ?1 \
             WHERE instance_id = ?2 AND handoff_requested_at IS NULL",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_set_role(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET role = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![role.as_str(), format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_mark_drained(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let ts = format_rfc3339(at)?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET drained_at = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![ts.clone(), ts, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_list(&self) -> Result<Vec<DaemonInstance>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{SELECT_INSTANCE} ORDER BY started_at ASC, instance_id ASC"
            ))?;
            let rows = stmt.query_map([], row_to_instance)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn instance_delete(&self, instance_id: &str) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM daemon_instances WHERE instance_id = ?1",
            params![instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_delete_stale(
        &self,
        keep: &str,
        heartbeat_before: OffsetDateTime,
    ) -> Result<Vec<String>, StoreError> {
        let mut conn = self.lock()?;
        let before = format_rfc3339(heartbeat_before)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT instance_id FROM daemon_instances \
                 WHERE instance_id <> ?1 AND (drained_at IS NOT NULL OR heartbeat_at < ?2)",
            )?;
            let rows = stmt.query_map(params![keep, before], |row| row.get::<_, String>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            ids
        };
        removed.sort();
        for id in &removed {
            tx.execute(
                "DELETE FROM daemon_instances WHERE instance_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    fn cluster_settings_get(
        &self,
        cluster_id: &str,
    ) -> Result<Option<ClusterSettings>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT cluster_id, work_dir, updated_at FROM cluster_settings WHERE cluster_id = ?1",
                params![cluster_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(cluster_id, work_dir, updated_at)| {
                Ok(ClusterSettings {
                    cluster_id,
                    work_dir,
                    updated_at: parse_rfc3339(&updated_at)?,
                })
            })
            .transpose()
        })
    }

    fn cluster_settings_list(&self) -> Result<Vec<ClusterSettings>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cluster_id, work_dir, updated_at FROM cluster_settings ORDER BY cluster_id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (cluster_id, work_dir, updated_at) = row?;
                out.push(ClusterSettings {
                    cluster_id,
                    work_dir,
                    updated_at: parse_rfc3339(&updated_at)?,
                });
            }
            Ok(out)
        })
    }

    fn cluster_settings_set(
        &self,
        cluster_id: &str,
        work_dir: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        match work_dir {
            Some(work_dir) => {
                conn.execute(
                    "INSERT INTO cluster_settings (cluster_id, work_dir, updated_at) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(cluster_id) DO UPDATE SET work_dir = excluded.work_dir, \
                     updated_at = excluded.updated_at",
                    params![cluster_id, work_dir, format_rfc3339(now)?],
                )?;
            }
            None => {
                conn.execute(
                    "DELETE FROM cluster_settings WHERE cluster_id = ?1",
                    params![cluster_id],
                )?;
            }
        }
        Ok(())
    }

    fn cluster_connection_record(
        &self,
        record: &ClusterConnectionRecord,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let uptime = record
            .uptime_secs
            .map(|v| i64::try_from(v).unwrap_or(i64::MAX));
        conn.execute(
            "INSERT INTO cluster_connection_log (cluster_id, kind, method, cause, uptime_secs, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.cluster_id,
                record.kind,
                record.method,
                record.cause,
                uptime,
                format_rfc3339(record.at)?
            ],
        )?;
        Ok(())
    }

    fn cluster_connection_list_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<ClusterConnectionRecord>, StoreError> {
        // RFC 3339 の文字列は小数秒の桁数で順序が崩れうるので、比較は読んでから時刻で行う
        // （行は接続・切断ごとに 1 行で少ない）。
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cluster_id, kind, method, cause, uptime_secs, at FROM cluster_connection_log \
                 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (cluster_id, kind, method, cause, uptime, at) = row?;
                let at = parse_rfc3339(&at)?;
                if at < since {
                    continue;
                }
                out.push(ClusterConnectionRecord {
                    cluster_id,
                    kind,
                    method,
                    cause,
                    uptime_secs: uptime.and_then(|v| u64::try_from(v).ok()),
                    at,
                });
            }
            Ok(out)
        })
    }

    // ---- ADR-0072 D5（Phase E2）: execution_plans / work_units / runs ----
    fn execution_plan_adopt(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        tx.commit()?;
        Ok(())
    }

    fn execution_plan_adopt_delegating(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        let mut ids = Vec::with_capacity(children.len());
        for child in &children {
            if child.parent_id != Some(task_id) {
                return Err(StoreError::Invalid(format!(
                    "child {} does not belong to task {task_id}",
                    child.id
                )));
            }
            Self::insert_tx(&tx, child)?;
            Self::append_event_tx(
                &tx,
                child.id,
                &Event::Created {
                    task: Box::new(child.clone()),
                    origin: None,
                },
            )?;
            if child.status == Status::Draft {
                Self::apply_transition_tx(&tx, child.id, Trigger::Accept, vec![])?;
            }
            ids.push(child.id);
        }
        if !ids.is_empty() {
            Self::append_event_tx(
                &tx,
                task_id,
                &Event::Delegated {
                    run_id: run_id.to_string(),
                    task_ids: ids.clone(),
                },
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    fn execution_plan_active(
        &self,
        task_id: TaskId,
    ) -> Result<Option<ExecutionPlanRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, task_id, version, origin, planner_run_id, status, json, created_at, \
                 superseded_at FROM execution_plans WHERE task_id = ?1 AND status = 'active'",
                params![task_id.to_string()],
                Self::row_to_execution_plan,
            )
            .optional()?
            .transpose()
        })
    }

    fn execution_plan_list(&self, task_id: TaskId) -> Result<Vec<ExecutionPlanRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, version, origin, planner_run_id, status, json, created_at, \
                 superseded_at FROM execution_plans WHERE task_id = ?1 ORDER BY version ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_execution_plan)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn work_units_for(&self, task_id: TaskId) -> Result<Vec<WorkUnitRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units \
                 WHERE task_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_work_unit)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn decisions_list(
        &self,
        root_id: Option<TaskId>,
    ) -> Result<Vec<crate::decision::DecisionRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut out = Vec::new();
            match root_id {
                Some(root) => {
                    let sql = format!(
                        "SELECT {} FROM decisions WHERE root_id = ?1 ORDER BY created_at, id",
                        Self::DECISION_COLUMNS
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![root.to_string()], Self::row_to_decision)?;
                    for row in rows {
                        out.push(row??);
                    }
                }
                None => {
                    let sql = format!(
                        "SELECT {} FROM decisions ORDER BY created_at, id",
                        Self::DECISION_COLUMNS
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], Self::row_to_decision)?;
                    for row in rows {
                        out.push(row??);
                    }
                }
            }
            Ok(out)
        })
    }

    fn decision_get(&self, id: &str) -> Result<Option<crate::decision::DecisionRow>, StoreError> {
        self.with_read_conn(|conn| Self::decision_get_tx(conn, id))
    }

    fn decisions_replace(&self, rows: Vec<crate::decision::DecisionRow>) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM decisions", [])?;
        for row in &rows {
            Self::insert_decision_tx(&tx, row)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn decision_resolve_apply(
        &self,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match Self::decision_get_tx(&tx, decision_id)? {
            Some(row) if row.status == expect && row.task_id == task_id => {}
            _ => return Ok(false),
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn work_unit_get(&self, id: &str) -> Result<Option<WorkUnitRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1",
                params![id],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()
        })
    }

    fn work_unit_transition(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::update_work_unit_tx(&tx, &updated)?;
        Self::append_event_tx(&tx, task_id, &event)?;
        tx.commit()?;
        Ok(())
    }

    fn acquire_work_unit_lease(
        &self,
        task_id: TaskId,
        work_unit_id: &str,
        run_id: &str,
        ttl: StdDuration,
        branch: Option<String>,
        base_commit: Option<String>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        if task.status != Status::Running {
            return Ok(false);
        }
        let Some(current) = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, \
                 child_task_id, needs_decisions_json \
                 FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![work_unit_id, task_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?
        else {
            return Ok(false);
        };
        if !matches!(
            current.status,
            WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
        ) {
            return Ok(false);
        }
        let now = OffsetDateTime::now_utc();
        let expires_at = now + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let mut updated = current.clone();
        updated.status = WorkUnitStatus::Running;
        updated.blocked_reason = None;
        updated.runs += 1;
        updated.last_run_id = Some(run_id.to_string());
        updated.updated_at = format_rfc3339(now)?;
        updated.lease_run_id = Some(run_id.to_string());
        updated.lease_expires_at = Some(format_rfc3339(expires_at)?);
        if branch.is_some() {
            updated.branch = branch;
        }
        if base_commit.is_some() {
            updated.base_commit = base_commit;
        }
        Self::update_work_unit_tx(&tx, &updated)?;
        Self::append_event_tx(
            &tx,
            task_id,
            &Event::WorkUnitTransitioned {
                work_unit_id: current.id.clone(),
                key: current.key.clone(),
                from: current.status,
                to: WorkUnitStatus::Running,
                reason: "dispatch".to_string(),
                run_id: Some(run_id.to_string()),
            },
        )?;
        // Task の lease の期限を延ばす（保持者はそのまま。短くはしない）。
        if let Some(lease) = task.lease.as_mut()
            && lease.expires_at < expires_at
        {
            lease.expires_at = expires_at;
            tx.execute(
                "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                params![
                    format_rfc3339(expires_at)?,
                    serde_json::to_string(&task)?,
                    task_id.to_string(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn work_units_apply(
        &self,
        task_id: TaskId,
        inserted: Vec<WorkUnitRow>,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for wu in &inserted {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn tree_tasks(&self, root_id: TaskId) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT json FROM tasks WHERE id = ?1 OR root_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map(params![root_id.to_string()], |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    fn tasks_with_open_task_units(&self) -> Result<Vec<TaskId>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT task_id FROM work_units WHERE kind = 'task' AND status IN \
                 ('pending', 'ready', 'running', 'needs_continuation', 'blocked') ORDER BY task_id",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let ids: Vec<String> = rows.collect::<Result<_, _>>()?;
            ids.iter().map(|s| Self::parse_id(s)).collect()
        })
    }

    fn tree_child_create(
        &self,
        parent_id: TaskId,
        child: &Task,
        unit: WorkUnitRow,
        parent_events: Vec<Event>,
        replaces: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(parent) = Self::get_locked(&tx, parent_id)? else {
            return Ok(false);
        };
        if parent.status.is_terminal() || child.parent_id != Some(parent_id) {
            return Ok(false);
        }
        let current = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![unit.id, parent_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?;
        let Some(current) = current else {
            return Ok(false);
        };
        let expected = match replaces {
            None => current.status == WorkUnitStatus::Ready && current.child_task_id.is_none(),
            Some(prev) => {
                current.status == WorkUnitStatus::Running
                    && current.child_task_id.as_deref() == Some(prev)
            }
        };
        if !expected || current.kind != crate::execution_plan::WorkUnitKind::Task {
            return Ok(false);
        }
        Self::insert_tx(&tx, child)?;
        Self::append_event_tx(
            &tx,
            child.id,
            &Event::Created {
                task: Box::new(child.clone()),
                origin: Some(crate::model::CreatedOrigin::PlanUnit),
            },
        )?;
        Self::update_work_unit_tx(&tx, &unit)?;
        for ev in &parent_events {
            Self::append_event_tx(&tx, parent_id, ev)?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn execution_plan_adopt_tree(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        after_events: Vec<Event>,
        adoptions: Vec<TreeAdoption>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for a in &adoptions {
            if !Self::tree_adoption_ok_tx(&tx, a)? {
                return Ok(false);
            }
        }
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        for ev in &after_events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        for a in &adoptions {
            Self::apply_tree_adoption_tx(&tx, a)?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn tree_adopt_apply(
        &self,
        owner_id: TaskId,
        unit_id: &str,
        expect_unit_status: WorkUnitStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        adoption: TreeAdoption,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(owner) = Self::get_locked(&tx, owner_id)? else {
            return Ok(false);
        };
        if owner.status.is_terminal() {
            return Ok(false);
        }
        let current = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![unit_id, owner_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?;
        let Some(current) = current else {
            return Ok(false);
        };
        if current.status != expect_unit_status
            || current.child_task_id.is_some()
            || current.kind != crate::execution_plan::WorkUnitKind::Task
        {
            return Ok(false);
        }
        if !Self::tree_adoption_ok_tx(&tx, &adoption)? {
            return Ok(false);
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, owner_id, ev)?;
        }
        Self::apply_tree_adoption_tx(&tx, &adoption)?;
        tx.commit()?;
        Ok(true)
    }

    fn extend_task_lease(&self, task_id: TaskId, ttl: StdDuration) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        if task.status != Status::Running {
            return Ok(false);
        }
        let expires_at = OffsetDateTime::now_utc()
            + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let Some(lease) = task.lease.as_mut() else {
            return Ok(false);
        };
        if lease.expires_at < expires_at {
            lease.expires_at = expires_at;
            tx.execute(
                "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                params![
                    format_rfc3339(expires_at)?,
                    serde_json::to_string(&task)?,
                    task_id.to_string(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn running_tasks_with_runnable_work_units(
        &self,
        limit: usize,
    ) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.json FROM tasks t WHERE t.status = ?1 AND EXISTS ( \
                 SELECT 1 FROM work_units w WHERE w.task_id = t.id AND w.phase IS NOT NULL \
                 AND w.kind NOT IN ('integrate', 'task') AND w.status IN ('ready', 'needs_continuation')) \
                 ORDER BY t.created_at ASC LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                params![status_str(Status::Running), usize_to_i64(limit)],
                |row| row.get::<_, String>(0),
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    fn run_index_start(&self, row: RunRow) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::insert_run_row_tx(&conn, &row)
    }

    fn run_index_finish(
        &self,
        run_id: &str,
        status: RunIndexStatus,
        checkpoint: Option<crate::execution::Checkpoint>,
        usage: Option<crate::model::Usage>,
        metrics: Option<crate::model::RunMetrics>,
        finished_at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE runs SET status = ?1, checkpoint_json = ?2, usage_json = ?3, \
             metrics_json = ?4, finished_at = ?5 WHERE run_id = ?6",
            params![
                status.as_str(),
                checkpoint.as_ref().map(serde_json::to_string).transpose()?,
                usage.as_ref().map(serde_json::to_string).transpose()?,
                metrics.as_ref().map(serde_json::to_string).transpose()?,
                format_rfc3339(finished_at)?,
                run_id,
            ],
        )?;
        Ok(n > 0)
    }

    fn run_index_get(&self, run_id: &str) -> Result<Option<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE run_id = ?1",
                params![run_id],
                Self::row_to_run,
            )
            .optional()?
            .transpose()
        })
    }

    fn runs_for_task(&self, task_id: TaskId) -> Result<Vec<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE task_id = ?1 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn runs_for_work_unit(&self, work_unit_id: &str) -> Result<Vec<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE work_unit_id = ?1 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map(params![work_unit_id], Self::row_to_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn execution_metrics_task_rows(
        &self,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<ExecutionMetricsTaskRow>, StoreError> {
        let since_text = since.map(format_rfc3339).transpose()?;
        self.with_read_conn(|conn| {
            // Aggregate each child table before joining: raw joins multiply counts.
            let mut stmt = conn.prepare(
                "WITH eligible AS MATERIALIZED (\
                   SELECT id, status, genre, assignee, json, updated_at FROM tasks \
                   WHERE (?1 IS NULL OR julianday(updated_at) >= julianday(?1) - 1.0 / 86400000)), \
                 wu AS (\
                   SELECT w.task_id, SUM(CASE WHEN w.kind = 'repair' THEN 1 ELSE 0 END) repairs, \
                          SUM(w.continuations) continuations, SUM(w.retries) retries \
                   FROM work_units w JOIN eligible t ON t.id = w.task_id GROUP BY w.task_id), \
                 plans AS (\
                   SELECT p.task_id, SUM(CASE WHEN p.version > 1 THEN 1 ELSE 0 END) replans \
                   FROM execution_plans p JOIN eligible t ON t.id = p.task_id GROUP BY p.task_id), \
                 run_counts AS (\
                   SELECT r.task_id, COUNT(*) runs_count, \
                          SUM(CASE WHEN r.status = 'budget_exhausted' THEN 1 ELSE 0 END) budget_exhausted_runs \
                   FROM runs r JOIN eligible t ON t.id = r.task_id GROUP BY r.task_id) \
                 SELECT t.id, t.status, t.genre, t.assignee, json_extract(t.json, '$.routing'), \
                        COALESCE(wu.repairs, 0), COALESCE(plans.replans, 0), \
                        COALESCE(wu.continuations, 0), COALESCE(wu.retries, 0), \
                        COALESCE(run_counts.runs_count, 0), COALESCE(run_counts.budget_exhausted_runs, 0), \
                        latest.run_id, latest.role, latest.status, latest.adapter, latest.model, \
                        latest.metrics_json, latest.usage_json, \
                        t.updated_at, \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') IN \
                          ('execution_planned', 'work_unit_transitioned', 'worker_started', \
                           'worker_finished', 'transitioned', 'routing_decided') LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'worker_finished' AND \
                          json_extract(e.json, '$.end.type') = 'budget_exhausted' LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'transitioned' AND \
                          json_extract(e.json, '$.reason') IN \
                          ('continue', 'work_unit_retry', 'worker_error') LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'quota_estimated' LIMIT 1) \
                 FROM eligible t \
                 LEFT JOIN wu ON wu.task_id = t.id \
                 LEFT JOIN plans ON plans.task_id = t.id \
                 LEFT JOIN run_counts ON run_counts.task_id = t.id \
                 LEFT JOIN runs latest ON latest.run_id = (\
                   SELECT r.run_id FROM runs r WHERE r.task_id = t.id \
                   ORDER BY r.started_at DESC, r.run_id DESC LIMIT 1) \
                 ORDER BY t.id",
            )?;
            let rows = stmt.query_map(params![since_text], |row| {
                Ok((
                    row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?, row.get::<_, u32>(5)?,
                    row.get::<_, u32>(6)?, row.get::<_, u32>(7)?,
                    row.get::<_, u32>(8)?, row.get::<_, u32>(9)?,
                    row.get::<_, u32>(10)?, row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?, row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?, row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?, row.get::<_, Option<String>>(17)?,
                    row.get::<_, String>(18)?, row.get::<_, bool>(19)?, row.get::<_, bool>(20)?,
                    row.get::<_, bool>(21)?, row.get::<_, bool>(22)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, status, genre, assignee, routing_json, repairs, replans,
                    continuations, retries, runs_count, budget_exhausted_runs, run_id,
                    role, run_status, adapter, model, metrics_json, usage_json, updated_at,
                    has_execution_events, has_budget_events, has_transition_metrics,
                    has_quota_events) = row?;
                // SQLite julianday has millisecond resolution. Keep a 1 ms candidate margin in
                // SQL, then apply the original OffsetDateTime comparison exactly here.
                if let Some(since) = since && parse_rfc3339(&updated_at)? < since {
                    continue;
                }
                let latest_run = if let Some(run_id) = run_id {
                    let role = role.and_then(|s| RunIndexRole::parse(&s)).ok_or_else(|| {
                        StoreError::Invalid(format!("invalid latest runs.role for {run_id}"))
                    })?;
                    let status = run_status
                        .and_then(|s| RunIndexStatus::parse(&s))
                        .ok_or_else(|| StoreError::Invalid(format!("invalid latest runs.status for {run_id}")))?;
                    Some(ExecutionMetricsLatestRun {
                        run_id, role, status, adapter, model, metrics_json, usage_json,
                    })
                } else {
                    None
                };
                out.push(ExecutionMetricsTaskRow {
                    task_id: id.parse().map_err(|_| StoreError::Invalid(format!("invalid tasks.id: {id}")))?,
                    status: parse_status(&status)?, genre, assignee, routing_json,
                    repairs, replans, continuations, retries, runs_count,
                    budget_exhausted_runs, latest_run, has_execution_events, has_budget_events,
                    has_transition_metrics, has_quota_events,
                });
            }
            Ok(out)
        })
    }

    fn work_units_replace(
        &self,
        task_id: TaskId,
        rows: Vec<WorkUnitRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM work_units WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for wu in &rows {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn runs_replace(&self, task_id: TaskId, rows: Vec<RunRow>) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM runs WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for r in &rows {
            Self::insert_run_row_tx(&tx, r)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn execution_plans_replace(
        &self,
        task_id: TaskId,
        rows: Vec<ExecutionPlanRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM execution_plans WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for plan in &rows {
            tx.execute(
                "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, \
                 status, json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    plan.id,
                    plan.task_id,
                    plan.version,
                    plan.origin.as_str(),
                    plan.planner_run_id,
                    plan.status.as_str(),
                    serde_json::to_string(&plan.spec)?,
                    plan.created_at,
                    plan.superseded_at,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- ADR-0072 D16/D17（Phase E4）: reviewer repair / replan ----
    fn review_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.repair_apply(
            task_id,
            Trigger::ReviewRepair,
            extra_events,
            new_plan,
            work_units,
        )
    }

    fn delivery_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.repair_apply(task_id, Trigger::Reopen, extra_events, new_plan, work_units)
    }

    #[allow(clippy::too_many_arguments)]
    fn execution_plan_replan(
        &self,
        task_id: TaskId,
        old_plan_id: String,
        new_plan: ExecutionPlanRow,
        updated_work_units: Vec<WorkUnitRow>,
        new_work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        plan_event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let superseded_at = format_rfc3339(OffsetDateTime::now_utc())?;
        let n = tx.execute(
            "UPDATE execution_plans SET status = 'superseded', superseded_at = ?1 \
             WHERE id = ?2 AND task_id = ?3 AND status = 'active'",
            params![superseded_at, old_plan_id, task_id.to_string()],
        )?;
        if n == 0 {
            return Err(StoreError::InUse {
                kind: "execution_plan",
                id: old_plan_id,
                detail: "no active execution plan to replan (changed concurrently?)".to_string(),
            });
        }
        tx.execute(
            "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, \
             status, json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                new_plan.id,
                new_plan.task_id,
                new_plan.version,
                new_plan.origin.as_str(),
                new_plan.planner_run_id,
                new_plan.status.as_str(),
                serde_json::to_string(&new_plan.spec)?,
                new_plan.created_at,
                new_plan.superseded_at,
            ],
        )?;
        for wu in &updated_work_units {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for wu in &new_work_units {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        for ev in &extra_events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        Self::append_event_tx(&tx, task_id, &plan_event)?;
        tx.commit()?;
        Ok(())
    }
}
