//! `impl TaskStore for SqliteStore`（1 ブロック）。領域へ移した method は `<method>_impl` への 1 行転送。

use std::time::Duration as StdDuration;

use time::OffsetDateTime;

use crate::comment::TaskComment;
use crate::execution_plan::{
    ExecutionPlanRow, RunIndexStatus, RunRow, WorkUnitRow, WorkUnitStatus,
};
use crate::instance::{DaemonInstance, InstanceRole};
use crate::integrations::{IntegrationId, TaskIntegration};
use crate::message::Message;
use crate::model::{Event, Status, Task, TaskId, WorkspaceSpec};
use crate::org::{
    Milestone, MilestoneId, MilestoneStatus, OrgNode, Project, ProjectId, ProjectStatus,
};
use crate::repos::{ProjectRepo, RepoId};
use crate::transition::{Outcome, Trigger};
use crate::write_set::WriteSetRecord;

use super::{
    ClusterConnectionRecord, ClusterSettings, EventRow, ExecutionMetricsTaskRow, ListFilter,
    ListOrder, Page, ProjectPlanApply, SqliteStore, StoreError, TaskStore, TaskWithEvents,
    TreeAdoption,
};

impl TaskStore for SqliteStore {
    fn record_behind_target(
        &self,
        obs: &crate::behind_target::BehindTargetObservation,
    ) -> Result<crate::behind_target::BehindTargetSnapshot, StoreError> {
        SqliteStore::record_behind_target(self, obs)
    }

    fn behind_targets(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<crate::behind_target::BehindTargetSnapshot>, StoreError> {
        SqliteStore::behind_targets(self, task_id)
    }

    fn record_run_write_set(&self, record: &WriteSetRecord) -> Result<(), StoreError> {
        SqliteStore::record_run_write_set(self, record)
    }

    fn run_write_sets(&self, run_id: &str) -> Result<Vec<WriteSetRecord>, StoreError> {
        SqliteStore::run_write_sets(self, run_id)
    }

    fn record_work_unit_write_set(&self, record: &WriteSetRecord) -> Result<(), StoreError> {
        SqliteStore::record_work_unit_write_set(self, record)
    }

    fn work_unit_write_sets(&self, work_unit_id: &str) -> Result<Vec<WriteSetRecord>, StoreError> {
        SqliteStore::work_unit_write_sets(self, work_unit_id)
    }

    fn set_task_expected_write_paths(
        &self,
        task_id: TaskId,
        paths: Option<&[String]>,
        now: &str,
    ) -> Result<(), StoreError> {
        SqliteStore::set_task_expected_write_paths(self, task_id, paths, now)
    }

    fn task_expected_write_paths(
        &self,
        task_id: TaskId,
    ) -> Result<Option<Vec<String>>, StoreError> {
        SqliteStore::task_expected_write_paths(self, task_id)
    }

    fn effective_task_write_paths(
        &self,
        task_id: TaskId,
    ) -> Result<Option<Vec<String>>, StoreError> {
        SqliteStore::effective_task_write_paths(self, task_id)
    }

    fn work_unit_expected_write_paths(
        &self,
        work_unit_id: &str,
    ) -> Result<Option<Vec<String>>, StoreError> {
        SqliteStore::work_unit_expected_write_paths(self, work_unit_id)
    }
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

    fn tasks_with_events(&self) -> Result<Vec<TaskWithEvents>, StoreError> {
        self.tasks_with_events_impl()
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
        self.create_task_impl(task, None, extra_events)
    }

    fn create_task_with_origin(
        &self,
        task: &Task,
        origin: Option<crate::model::CreatedOrigin>,
        extra_events: Vec<Event>,
    ) -> Result<(), StoreError> {
        self.create_task_impl(task, origin, extra_events)
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

    fn update_task(&self, task: &Task, event: Event) -> Result<Task, StoreError> {
        self.update_task_impl(task, event)
    }

    fn comment_add(
        &self,
        comment: &TaskComment,
        transition: Option<(Trigger, Vec<Event>)>,
    ) -> Result<Option<Outcome>, StoreError> {
        self.comment_add_impl(comment, transition)
    }

    fn comments_for(&self, task_id: TaskId) -> Result<Vec<TaskComment>, StoreError> {
        self.comments_for_impl(task_id)
    }

    fn instance_register(&self, instance: &DaemonInstance) -> Result<(), StoreError> {
        self.instance_register_impl(instance)
    }

    fn instance_heartbeat(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        self.instance_heartbeat_impl(instance_id, at)
    }

    fn instance_request_handoff(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        self.instance_request_handoff_impl(instance_id, at)
    }

    fn instance_set_role(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        self.instance_set_role_impl(instance_id, role, at)
    }

    fn instance_mark_drained(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        self.instance_mark_drained_impl(instance_id, at)
    }

    fn instance_list(&self) -> Result<Vec<DaemonInstance>, StoreError> {
        self.instance_list_impl()
    }

    fn instance_delete(&self, instance_id: &str) -> Result<bool, StoreError> {
        self.instance_delete_impl(instance_id)
    }

    fn instance_delete_stale(
        &self,
        keep: &str,
        heartbeat_before: OffsetDateTime,
    ) -> Result<Vec<String>, StoreError> {
        self.instance_delete_stale_impl(keep, heartbeat_before)
    }

    fn cluster_settings_get(
        &self,
        cluster_id: &str,
    ) -> Result<Option<ClusterSettings>, StoreError> {
        self.cluster_settings_get_impl(cluster_id)
    }

    fn cluster_settings_list(&self) -> Result<Vec<ClusterSettings>, StoreError> {
        self.cluster_settings_list_impl()
    }

    fn cluster_settings_set(
        &self,
        cluster_id: &str,
        work_dir: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        self.cluster_settings_set_impl(cluster_id, work_dir, now)
    }

    fn cluster_connection_record(
        &self,
        record: &ClusterConnectionRecord,
    ) -> Result<(), StoreError> {
        self.cluster_connection_record_impl(record)
    }

    fn cluster_connection_list_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<ClusterConnectionRecord>, StoreError> {
        self.cluster_connection_list_since_impl(since)
    }

    fn execution_plan_adopt(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError> {
        self.execution_plan_adopt_impl(task_id, plan, work_units, extra_events, event)
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
        self.execution_plan_adopt_delegating_impl(
            task_id,
            plan,
            work_units,
            extra_events,
            event,
            run_id,
            children,
        )
    }

    fn execution_plan_active(
        &self,
        task_id: TaskId,
    ) -> Result<Option<ExecutionPlanRow>, StoreError> {
        self.execution_plan_active_impl(task_id)
    }

    fn execution_plan_list(&self, task_id: TaskId) -> Result<Vec<ExecutionPlanRow>, StoreError> {
        self.execution_plan_list_impl(task_id)
    }

    fn work_units_for(&self, task_id: TaskId) -> Result<Vec<WorkUnitRow>, StoreError> {
        self.work_units_for_impl(task_id)
    }

    fn decisions_list(
        &self,
        root_id: Option<TaskId>,
    ) -> Result<Vec<crate::decision::DecisionRow>, StoreError> {
        self.decisions_list_impl(root_id)
    }

    fn decision_get(&self, id: &str) -> Result<Option<crate::decision::DecisionRow>, StoreError> {
        self.decision_get_impl(id)
    }

    fn decisions_replace(&self, rows: Vec<crate::decision::DecisionRow>) -> Result<(), StoreError> {
        self.decisions_replace_impl(rows)
    }

    fn decision_resolve_apply(
        &self,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError> {
        self.decision_resolve_apply_impl(task_id, decision_id, expect, updated, events)
    }

    fn work_unit_get(&self, id: &str) -> Result<Option<WorkUnitRow>, StoreError> {
        self.work_unit_get_impl(id)
    }

    fn work_unit_transition(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        event: Event,
    ) -> Result<(), StoreError> {
        self.work_unit_transition_impl(task_id, updated, event)
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
        self.acquire_work_unit_lease_impl(task_id, work_unit_id, run_id, ttl, branch, base_commit)
    }

    fn work_units_apply(
        &self,
        task_id: TaskId,
        inserted: Vec<WorkUnitRow>,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        self.work_units_apply_impl(task_id, inserted, updated, events)
    }

    fn tree_tasks(&self, root_id: TaskId) -> Result<Vec<Task>, StoreError> {
        self.tree_tasks_impl(root_id)
    }

    fn tasks_with_open_task_units(&self) -> Result<Vec<TaskId>, StoreError> {
        self.tasks_with_open_task_units_impl()
    }

    fn tree_child_create(
        &self,
        parent_id: TaskId,
        child: &Task,
        unit: WorkUnitRow,
        parent_events: Vec<Event>,
        replaces: Option<&str>,
    ) -> Result<bool, StoreError> {
        self.tree_child_create_impl(parent_id, child, unit, parent_events, replaces)
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
        self.execution_plan_adopt_tree_impl(
            task_id,
            plan,
            work_units,
            extra_events,
            event,
            after_events,
            adoptions,
        )
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
        self.tree_adopt_apply_impl(
            owner_id,
            unit_id,
            expect_unit_status,
            updated,
            events,
            adoption,
        )
    }

    fn extend_task_lease(&self, task_id: TaskId, ttl: StdDuration) -> Result<bool, StoreError> {
        self.extend_task_lease_impl(task_id, ttl)
    }

    fn running_tasks_with_runnable_work_units(
        &self,
        limit: usize,
    ) -> Result<Vec<Task>, StoreError> {
        self.running_tasks_with_runnable_work_units_impl(limit)
    }

    fn run_index_start(&self, row: RunRow) -> Result<(), StoreError> {
        self.run_index_start_impl(row)
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
        self.run_index_finish_impl(run_id, status, checkpoint, usage, metrics, finished_at)
    }

    fn run_index_get(&self, run_id: &str) -> Result<Option<RunRow>, StoreError> {
        self.run_index_get_impl(run_id)
    }

    fn runs_for_task(&self, task_id: TaskId) -> Result<Vec<RunRow>, StoreError> {
        self.runs_for_task_impl(task_id)
    }

    fn close_runs_of_terminal_tasks(&self) -> Result<Vec<(TaskId, String)>, StoreError> {
        self.close_runs_of_terminal_tasks_impl()
    }

    fn runs_for_work_unit(&self, work_unit_id: &str) -> Result<Vec<RunRow>, StoreError> {
        self.runs_for_work_unit_impl(work_unit_id)
    }

    fn execution_metrics_task_rows(
        &self,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<ExecutionMetricsTaskRow>, StoreError> {
        self.execution_metrics_task_rows_impl(since)
    }

    fn work_units_replace(
        &self,
        task_id: TaskId,
        rows: Vec<WorkUnitRow>,
    ) -> Result<(), StoreError> {
        self.work_units_replace_impl(task_id, rows)
    }

    fn runs_replace(&self, task_id: TaskId, rows: Vec<RunRow>) -> Result<(), StoreError> {
        self.runs_replace_impl(task_id, rows)
    }

    fn execution_plans_replace(
        &self,
        task_id: TaskId,
        rows: Vec<ExecutionPlanRow>,
    ) -> Result<(), StoreError> {
        self.execution_plans_replace_impl(task_id, rows)
    }

    fn review_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.review_repair_apply_impl(task_id, extra_events, new_plan, work_units)
    }

    fn delivery_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.delivery_repair_apply_impl(task_id, extra_events, new_plan, work_units)
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
        self.execution_plan_replan_impl(
            task_id,
            old_plan_id,
            new_plan,
            updated_work_units,
            new_work_units,
            extra_events,
            plan_event,
        )
    }
}
