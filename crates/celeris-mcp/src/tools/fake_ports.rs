//! ADR 2026-11-10-taskstore-consumer-ports: `OrgDirectory`・`TaskQuery` の試験用の fake と、
//! `tools/projects`・`tools/tasks` の一覧の本体をこの fake で確かめる試験（SQLite を開かない）。
//!
//! fake はメモリの `Vec` を持つだけで、SQL の振る舞い（並び・絞り込み・cursor）は task-core の store の
//! 試験が確かめる。`list_page` は道具が使う形（`ListOrder::UpdatedDesc`・cursor なし・`statuses` と
//! `project_id` の絞り込み）だけを実装し、それ以外の並びと cursor は `StoreError::Invalid` で断る。

use std::sync::Mutex;
use task_core::{
    Budget, EventRow, ListFilter, ListOrder, Milestone, MilestoneId, MilestoneStatus, OrgDirectory,
    OrgNode, Page, Project, ProjectId, ProjectStatus, Status, StoreError, Task, TaskId, TaskKind,
    TaskQuery, Tier, WorkerHint, WorkspaceSpec,
};
use time::{Duration, OffsetDateTime};

use super::projects::{get_project, list_projects};
use super::tasks::list_tasks;

#[derive(Default)]
struct FakePorts {
    org_nodes: Mutex<Vec<OrgNode>>,
    projects: Vec<Project>,
    milestones: Vec<Milestone>,
    tasks: Vec<Task>,
}

/// 試験の時刻: UNIX 紀元から `secs` 秒後（並びを決めるため、試験ごとに値を決めて渡す）。
fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + Duration::seconds(secs)
}

impl FakePorts {
    fn add_project(&mut self, title: &str, status: ProjectStatus, created_secs: i64) -> ProjectId {
        let id = ProjectId::new();
        self.projects.push(Project {
            id,
            title: title.to_string(),
            request: format!("request of {title}"),
            status,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            auto_integrate: false,
            created_at: at(created_secs),
            updated_at: at(created_secs),
        });
        id
    }

    fn add_milestone(&mut self, project_id: ProjectId, seq: i64, title: &str) {
        self.milestones.push(Milestone {
            id: MilestoneId::new(),
            project_id,
            seq,
            title: title.to_string(),
            description: String::new(),
            status: MilestoneStatus::Approved,
            paused_from: None,
            plan_key: None,
            created_at: at(0),
            updated_at: at(0),
        });
    }

    fn add_task(
        &mut self,
        title: &str,
        status: Status,
        project_id: Option<ProjectId>,
        updated_secs: i64,
    ) -> TaskId {
        let id = TaskId::new();
        self.tasks.push(Task {
            harness_id: None,
            requirements: Default::default(),
            tree: None,
            paused_at: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id,
            parent_id: None,
            kind: TaskKind::Execute,
            title: title.to_string(),
            objective: "objective".to_string(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
            },
            workspace: WorkspaceSpec::Local {
                path: "/work".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 10,
                max_wall_secs: 600,
                max_retries: 2,
                max_tokens: None,
            },
            attempts: 0,
            lease: None,
            created_at: at(0),
            updated_at: at(updated_secs),
            role: None,
            genre: None,
            aggregate: false,
            project_id,
            milestone_id: None,
            assignee: Some("cluster-hpc".to_string()),
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        });
        id
    }
}

impl OrgDirectory for FakePorts {
    fn org_list(&self) -> Result<Vec<OrgNode>, StoreError> {
        let mut out = self
            .org_nodes
            .lock()
            .map_err(|_| StoreError::Invalid("fake org list lock poisoned".to_string()))?
            .clone();
        out.sort_by_key(|node| (node.position, node.id.clone()));
        Ok(out)
    }

    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError> {
        Ok(self
            .org_nodes
            .lock()
            .map_err(|_| StoreError::Invalid("fake org get lock poisoned".to_string()))?
            .iter()
            .find(|n| n.id == id)
            .cloned())
    }

    fn project_list(&self) -> Result<Vec<Project>, StoreError> {
        let mut out = self.projects.clone();
        out.sort_by_key(|p| std::cmp::Reverse((p.created_at, p.id)));
        Ok(out)
    }

    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        Ok(self.projects.iter().find(|p| p.id == id).cloned())
    }

    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError> {
        let mut out: Vec<_> = self
            .milestones
            .iter()
            .filter(|m| m.project_id == project_id)
            .cloned()
            .collect();
        out.sort_by_key(|m| m.seq);
        Ok(out)
    }

    fn org_upsert(&self, node: &OrgNode) -> Result<OrgNode, StoreError> {
        let mut nodes = self
            .org_nodes
            .lock()
            .map_err(|_| StoreError::Invalid("fake org upsert lock poisoned".to_string()))?;
        if let Some(existing) = nodes.iter_mut().find(|existing| existing.id == node.id) {
            *existing = node.clone();
        } else {
            nodes.push(node.clone());
        }
        Ok(node.clone())
    }
}

impl TaskQuery for FakePorts {
    fn list_page(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError> {
        if !matches!(order, ListOrder::UpdatedDesc) || cursor.is_some() {
            return Err(StoreError::Invalid(
                "the fake only lists by UpdatedDesc without a cursor".to_string(),
            ));
        }
        let mut items: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| filter.statuses.is_empty() || filter.statuses.contains(&t.status))
            .filter(|t| filter.project_id.is_none() || t.project_id == filter.project_id)
            .cloned()
            .collect();
        items.sort_by_key(|t| std::cmp::Reverse((t.updated_at, t.id)));
        let total = items.len() as u64;
        items.truncate(limit);
        Ok(Page {
            items,
            next_cursor: None,
            total,
        })
    }

    fn event_rows_for(
        &self,
        _task_id: TaskId,
        _after_seq: Option<u64>,
        _limit: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        // 一覧の本体は event を読まない。fake は event を持たない。
        Ok(Vec::new())
    }
}

fn ids<T>(items: &[T], id: impl Fn(&T) -> &str) -> Vec<String> {
    items.iter().map(|i| id(i).to_string()).collect()
}

#[test]
fn port_fake_org_upsert_updates_in_memory_directory() {
    let fake = FakePorts::default();
    let now = OffsetDateTime::UNIX_EPOCH;
    let node = |name: &str, position| OrgNode {
        profile: Default::default(),
        id: "ops".to_string(),
        parent_id: None,
        name: name.to_string(),
        kind: task_core::org::OrgKind::Department,
        genre: None,
        brief: String::new(),
        position,
        created_at: now,
        updated_at: now,
    };

    fake.org_upsert(&node("Operations", 2)).expect("insert org");
    fake.org_upsert(&node("Cluster operations", 1))
        .expect("update org");

    let rows = fake.org_list().expect("list org");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Cluster operations");
    assert_eq!(rows[0].position, 1);
    assert_eq!(
        fake.org_get("ops").expect("get org").unwrap().name,
        "Cluster operations"
    );
}

#[test]
fn port_fake_projects_list_filters_by_status_and_limit() {
    let mut fake = FakePorts::default();
    let old_active = fake.add_project("old active", ProjectStatus::Active, 10);
    let done = fake.add_project("done", ProjectStatus::Done, 20);
    let new_active = fake.add_project("new active", ProjectStatus::Active, 30);

    let all = list_projects(&fake, None, 20).expect("list all");
    assert_eq!(
        ids(&all, |p| &p.id),
        vec![
            new_active.to_string(),
            done.to_string(),
            old_active.to_string()
        ]
    );
    assert_eq!(all[0].title, "new active");

    let active = list_projects(&fake, Some(ProjectStatus::Active), 20).expect("list active");
    assert_eq!(
        ids(&active, |p| &p.id),
        vec![new_active.to_string(), old_active.to_string()]
    );
    assert!(active.iter().all(|p| p.status == ProjectStatus::Active));

    let first = list_projects(&fake, Some(ProjectStatus::Active), 1).expect("list limited");
    assert_eq!(ids(&first, |p| &p.id), vec![new_active.to_string()]);

    let none = list_projects(&fake, Some(ProjectStatus::Cancelled), 20).expect("list none");
    assert!(none.is_empty());
}

#[test]
fn port_fake_projects_get_returns_milestones_and_tasks_or_none() {
    let mut fake = FakePorts::default();
    let project = fake.add_project("pluvio", ProjectStatus::Active, 10);
    let other = fake.add_project("other", ProjectStatus::Active, 20);
    fake.add_milestone(project, 2, "second");
    fake.add_milestone(project, 1, "first");
    fake.add_milestone(other, 1, "other milestone");
    let older = fake.add_task("older", Status::Ready, Some(project), 100);
    let newer = fake.add_task("newer", Status::Done, Some(project), 200);
    fake.add_task("elsewhere", Status::Ready, Some(other), 300);
    fake.add_task("no project", Status::Ready, None, 400);

    let got = get_project(&fake, &fake, project)
        .expect("get")
        .expect("project exists");
    assert_eq!(got.id, project.to_string());
    assert_eq!(got.title, "pluvio");
    assert_eq!(got.request, "request of pluvio");
    assert_eq!(got.status, ProjectStatus::Active);
    let milestone_titles: Vec<_> = got.milestones.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(milestone_titles, vec!["first", "second"]);
    assert_eq!(
        ids(&got.tasks, |t| &t.id),
        vec![newer.to_string(), older.to_string()]
    );
    let project_text = project.to_string();
    assert!(
        got.tasks
            .iter()
            .all(|t| t.project_id.as_deref() == Some(project_text.as_str()))
    );
    assert_eq!(got.tasks[0].status, Status::Done);
    assert_eq!(got.tasks[0].assignee.as_deref(), Some("cluster-hpc"));
    assert_eq!(got.tasks[0].updated_at, "1970-01-01T00:03:20Z");

    let missing = get_project(&fake, &fake, ProjectId::new()).expect("get missing");
    assert!(missing.is_none());
}

#[test]
fn port_fake_tasks_list_filters_by_status_project_and_limit() {
    let mut fake = FakePorts::default();
    let project = fake.add_project("pluvio", ProjectStatus::Active, 10);
    let ready_old = fake.add_task("ready old", Status::Ready, Some(project), 100);
    let ready_new = fake.add_task("ready new", Status::Ready, Some(project), 300);
    let done = fake.add_task("done", Status::Done, Some(project), 200);
    let elsewhere = fake.add_task("elsewhere", Status::Ready, None, 400);

    let all = list_tasks(&fake, &ListFilter::default(), 20).expect("list all");
    assert_eq!(
        ids(&all, |t| &t.id),
        vec![
            elsewhere.to_string(),
            ready_new.to_string(),
            done.to_string(),
            ready_old.to_string()
        ]
    );
    assert!(all[0].project_id.is_none());

    let filter = ListFilter {
        statuses: vec![Status::Ready],
        project_id: Some(project),
        ..ListFilter::default()
    };
    let ready = list_tasks(&fake, &filter, 20).expect("list ready in project");
    assert_eq!(
        ids(&ready, |t| &t.id),
        vec![ready_new.to_string(), ready_old.to_string()]
    );
    assert_eq!(ready[0].title, "ready new");

    let first = list_tasks(&fake, &filter, 1).expect("list limited");
    assert_eq!(ids(&first, |t| &t.id), vec![ready_new.to_string()]);
}
