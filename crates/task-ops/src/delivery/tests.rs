use super::*;
use std::fs;
use std::path::PathBuf;
use task_core::*;
#[test]
fn existing_reviewer_gets_a_pinned_merge_criterion_without_extra_tasks() {
    let store = SqliteStore::open_in_memory().unwrap();
    let now = time::OffsetDateTime::now_utc();
    for (id, parent, kind) in [
        ("cos", None, OrgKind::Secretary),
        ("engineering", Some("cos"), OrgKind::Department),
        ("worker", Some("engineering"), OrgKind::Section),
    ] {
        store
            .org_upsert(&OrgNode {
                id: id.into(),
                parent_id: parent.map(str::to_string),
                name: id.into(),
                kind,
                genre: None,
                brief: String::new(),
                profile: Default::default(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
    let project = Project {
        auto_advance: false,
        auto_integrate: true,
        slug: None,
        id: ProjectId::new(),
        title: "test".into(),
        request: "fix".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repo");
    fs::create_dir(&p).unwrap();
    let git = |args: &[&str]| {
        let out = crate::changes::git(&p, args, Duration::from_secs(10)).unwrap();
        assert!(out.ok, "{}", out.stderr);
        out.stdout.trim().to_string()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "test"]);
    git(&["commit", "--allow-empty", "-m", "base"]);
    let row = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "repo".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::Local {
            path: p.clone(),
            mode: None,
        },
        default_branch: Some("main".into()),
        sync: None,
        run: Default::default(),
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&row).unwrap();
    let spec:crate::add::NewTaskSpec=serde_json::from_value(serde_json::json!({"title":"fix","objective":"implement","acceptance":[{"type":"reviewer","text":"works"}],"project_id":project.id,"assignee":"worker","repos":["repo"]})).unwrap();
    let mut task = crate::add::create_task_with_roles(&store, spec, &[], &[], now).unwrap();
    task.assignee = None;
    let branch = format!("celeris/{}", task.id);
    git(&["checkout", "-b", &branch]);
    git(&["commit", "--allow-empty", "-m", "implemented"]);
    let head = git(&["rev-parse", "HEAD"]);
    let root = dir.path().join("tasks");
    let taskdir = root.join(task.id.to_string());
    fs::create_dir_all(&taskdir).unwrap();
    fs::write(taskdir.join("worktree.json"),serde_json::json!({"repo":p,"dir":p,"branch":branch,"base":"","base_kind":"main","repos":[{"name":"repo","kind":"git","source":p,"dir":p,"branch":branch}]}).to_string()).unwrap();
    let policy = DeliveryPolicy {
        default_departments: [(project.id.to_string(), "engineering".into())]
            .into_iter()
            .collect(),
    };
    // 木の子は親の統合工程で扱い、delivery は作らず除外理由を記録する。
    let mut as_child = task.clone();
    let parent_id = TaskId::new();
    as_child.tree = Some(TreeInfo {
        root_id: parent_id,
        depth: 2,
        parent_unit: Some(ParentUnit {
            task_id: parent_id,
            plan_id: "plan".into(),
            unit_key: "c".into(),
            stage: "s1".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    assert!(
        begin(&store, &mut as_child, &roots(&root), &policy, "w", "r")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        as_child.acceptance.len(),
        1,
        "no merge criterion for a child"
    );
    assert!(store.delivery_get(task.id).unwrap().is_none());
    assert!(
        store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, event)| matches!(
                event,
                Event::DeliverySkipped {
                    reason: DeliverySkipReason::TreeChild,
                    ..
                }
            ))
    );
    let d = begin(
        &store,
        &mut task,
        &roots(&root),
        &policy,
        "worker-run",
        "review-run",
    )
    .unwrap()
    .unwrap();
    assert_eq!(d.head, head);
    assert_eq!(d.department, "engineering");
    assert_eq!(d.review_run, "review-run");
    assert_eq!(d.criterion_idx, 1);
    assert_eq!(task.acceptance.len(), 2);
    assert!(task.acceptance[1].text.contains(&head));
    assert_eq!(
        store
            .list_page(&ListFilter::default(), ListOrder::CreatedDesc, None, 20)
            .unwrap()
            .items
            .len(),
        1,
        "no extra supervisor or CoS tasks"
    );
    assert!(
        store
            .message_list("cos", Some(project.id), 20)
            .unwrap()
            .is_empty()
    );
    let mut next = d.clone();
    next.state = DeliveryState::MergeQueued;
    assert!(store.delivery_save(Some(&d), &next).unwrap());
    assert!(
        !store.delivery_save(Some(&d), &d).unwrap(),
        "stale scheduler cannot erase review"
    );
    let mut unrelated = task.clone();
    unrelated.project_id = None;
    assert!(
        begin(&store, &mut unrelated, &roots(&root), &policy, "w", "r")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, event)| matches!(
                event,
                Event::DeliverySkipped {
                    reason: DeliverySkipReason::NoProject,
                    ..
                }
            ))
    );
}

#[test]
fn target_advanced_count_resets_only_on_explicit_restart() {
    let repo = RepoId::new();
    let other = RepoId::new();
    let advanced = |repo_id| Event::ReviewTargetAdvanced {
        review_run: "review".into(),
        repo_id,
        reviewed_sha: "reviewed".into(),
        target_sha: "target".into(),
        attempt: 1,
    };
    let transitioned = |reason: &str| Event::Transitioned {
        from: Status::Done,
        to: Status::Reviewing,
        reason: reason.into(),
    };
    // 自動の再レビュー: rereview の直後に同じ transaction の再進行が続く。
    let mut events = vec![
        transitioned("rereview"),
        advanced(repo),
        advanced(other),
        transitioned("rereview"),
        advanced(repo),
    ];
    assert_eq!(target_restale_count(&events, repo), 2);
    assert_eq!(target_restale_count(&events, other), 1);
    // 人の明示的な再レビューは数え直す。
    events.push(transitioned("rereview"));
    assert_eq!(target_restale_count(&events, repo), 0);
    events.push(Event::worker_progress("review", "synced"));
    events.push(transitioned("rereview"));
    events.push(advanced(repo));
    assert_eq!(target_restale_count(&events, repo), 1);
}

fn enable_project(store: &SqliteStore, id: ProjectId) {
    let now = time::OffsetDateTime::now_utc();
    store
        .project_create(&Project {
            id,
            title: "p".into(),
            request: "r".into(),
            status: ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            auto_integrate: true,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
}

fn fallback_fixture() -> (SqliteStore, Task, DeliveryPolicy) {
    let store = SqliteStore::open_in_memory().unwrap();
    let now = time::OffsetDateTime::now_utc();
    for (id, parent, kind) in [
        ("cos", None, OrgKind::Secretary),
        ("engineering", Some("cos"), OrgKind::Department),
        ("research", Some("cos"), OrgKind::Department),
        ("software", Some("engineering"), OrgKind::Section),
        ("scientific", Some("research"), OrgKind::Section),
    ] {
        store
            .org_upsert(&OrgNode {
                id: id.into(),
                parent_id: parent.map(str::to_owned),
                name: id.into(),
                kind,
                genre: None,
                brief: String::new(),
                profile: Default::default(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
    let spec: crate::add::NewTaskSpec = serde_json::from_value(serde_json::json!({"title":"root","objective":"work","acceptance":[{"type":"reviewer","text":"works"}]})).unwrap();
    let task = crate::add::create_task_with_roles(&store, spec, &[], &[], now).unwrap();
    (store, task, DeliveryPolicy::default())
}

fn routed_event(store: &SqliteStore, task: &Task, run: &str, owner: &str, unit: Option<&str>) {
    let decision =
        task_core::model_policy::decide_for_task(task, &task_core::LaneCeiling::default()).unwrap();
    let record = RoutingRecord {
        features: None,
        org_node: Some(owner.into()),
        harness: Some("coding".into()),
        decision,
        quota_reason: None,
        work_unit_id: unit.map(str::to_owned),
        escalation: None,
    };
    store
        .append_event(
            task.id,
            &Event::RoutingDecided {
                run_id: run.into(),
                record: Box::new(record),
            },
        )
        .unwrap();
}

fn resolved(store: &SqliteStore, task: &Task, policy: &DeliveryPolicy) -> Option<String> {
    resolve_department(store, task, policy, &store.org_list().unwrap()).unwrap()
}

#[test]
fn department_fallback_assignee_wins() {
    let (store, mut task, mut policy) = fallback_fixture();
    task.assignee = Some("software".into());
    task.project_id = Some(ProjectId::new());
    policy
        .default_departments
        .insert(task.project_id.unwrap().to_string(), "research".into());
    assert_eq!(
        resolved(&store, &task, &policy).as_deref(),
        Some("engineering")
    );
}

#[test]
fn department_fallback_planner_assignee() {
    let (store, task, policy) = fallback_fixture();
    let spec = ExecutionPlanSpec {
        schema: EXECUTION_PLAN_SCHEMA.into(),
        rationale: "test".into(),
        stages: vec![],
        units: vec![],
        decisions: vec![],
        work_units: vec![],
        phases: vec![],
        children: vec![],
    };
    store
        .execution_plans_replace(
            task.id,
            vec![ExecutionPlanRow {
                id: "plan".into(),
                task_id: task.id.to_string(),
                version: 1,
                origin: PlanOrigin::Planner,
                planner_run_id: Some("planner-run".into()),
                status: PlanStatus::Active,
                spec,
                created_at: "2026-10-01T00:00:00Z".into(),
                superseded_at: None,
            }],
        )
        .unwrap();
    routed_event(&store, &task, "planner-run", "scientific", None);
    assert_eq!(
        resolved(&store, &task, &policy).as_deref(),
        Some("research")
    );
}

#[test]
fn department_fallback_child_and_work_unit_votes() {
    let (store, task, policy) = fallback_fixture();
    let mut child = task.clone();
    child.id = TaskId::new();
    child.assignee = Some("scientific".into());
    child.tree = Some(TreeInfo {
        root_id: task.id,
        depth: 2,
        parent_unit: Some(ParentUnit {
            task_id: task.id,
            plan_id: "plan".into(),
            unit_key: "child".into(),
            stage: "stage".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    store.insert(&child).unwrap();
    assert_eq!(
        resolved(&store, &task, &policy).as_deref(),
        Some("research")
    );

    let spec = WorkUnitSpec {
        key: "a".into(),
        kind: WorkUnitKind::Implement,
        title: "a".into(),
        objective: "a".into(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: Default::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    let unit = WorkUnitRow::new(
        "wu-a".into(),
        task.id.to_string(),
        "plan".into(),
        0,
        spec,
        WorkUnitStatus::Done,
        "2026-10-01T00:00:00Z".into(),
    );
    store.work_units_replace(task.id, vec![unit]).unwrap();
    store
        .runs_replace(
            task.id,
            vec![RunRow {
                run_id: "worker-run".into(),
                task_id: task.id.to_string(),
                work_unit_id: Some("wu-a".into()),
                role: RunIndexRole::Worker,
                seq: 1,
                status: RunIndexStatus::Completed,
                adapter: None,
                model: None,
                account: None,
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: "2026-10-01T00:00:00Z".into(),
                finished_at: None,
            }],
        )
        .unwrap();
    routed_event(&store, &task, "worker-run", "software", Some("wu-a"));
    assert_eq!(
        resolved(&store, &task, &policy).as_deref(),
        Some("engineering"),
        "tied votes use lexical department ID"
    );
}

#[test]
fn department_fallback_project_default() {
    let (store, mut task, mut policy) = fallback_fixture();
    task.project_id = Some(ProjectId::new());
    policy
        .default_departments
        .insert(task.project_id.unwrap().to_string(), "scientific".into());
    assert_eq!(
        resolved(&store, &task, &policy).as_deref(),
        Some("research")
    );
}

#[test]
fn department_fallback_unresolved_returns_none() {
    let (store, task, policy) = fallback_fixture();
    assert_eq!(resolved(&store, &task, &policy), None);
}

#[test]
fn department_fallback_tree_child_and_outside_project_create_no_delivery() {
    let (store, mut task, policy) = fallback_fixture();
    let project_id = ProjectId::new();
    task.project_id = Some(project_id);
    task.assignee = Some("software".into());
    let root = tempfile::tempdir().unwrap();
    assert!(
        begin(&store, &mut task, &roots(root.path()), &policy, "w", "r")
            .unwrap()
            .is_none()
    );
    enable_project(&store, project_id);
    task.tree = Some(TreeInfo {
        root_id: TaskId::new(),
        depth: 2,
        parent_unit: Some(ParentUnit {
            task_id: TaskId::new(),
            plan_id: "plan".into(),
            unit_key: "child".into(),
            stage: "stage".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    assert!(
        begin(&store, &mut task, &roots(root.path()), &policy, "w", "r")
            .unwrap()
            .is_none()
    );
    assert!(store.delivery_get(task.id).unwrap().is_none());
}

struct SkipFixture {
    store: SqliteStore,
    task: Task,
    policy: DeliveryPolicy,
    root: PathBuf,
    repo: PathBuf,
    branch: String,
    _dir: tempfile::TempDir,
}

/// ADR-0121 D2: 対象案件・repo 1 つ・assignee 無しの root（既定部署なし）。
fn skip_fixture() -> SkipFixture {
    let store = SqliteStore::open_in_memory().unwrap();
    let now = time::OffsetDateTime::now_utc();
    for (id, parent, kind) in [
        ("cos", None, OrgKind::Secretary),
        ("engineering", Some("cos"), OrgKind::Department),
        ("worker", Some("engineering"), OrgKind::Section),
    ] {
        store
            .org_upsert(&OrgNode {
                id: id.into(),
                parent_id: parent.map(str::to_string),
                name: id.into(),
                kind,
                genre: None,
                brief: String::new(),
                profile: Default::default(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
    let project = Project {
        auto_advance: false,
        auto_integrate: true,
        slug: None,
        id: ProjectId::new(),
        title: "test".into(),
        request: "fix".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repo");
    fs::create_dir(&p).unwrap();
    let git = |args: &[&str]| {
        let out = crate::changes::git(&p, args, Duration::from_secs(10)).unwrap();
        assert!(out.ok, "{}", out.stderr);
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "test"]);
    git(&["commit", "--allow-empty", "-m", "base"]);
    let row = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "repo".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::Local {
            path: p.clone(),
            mode: None,
        },
        default_branch: Some("main".into()),
        sync: None,
        run: Default::default(),
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&row).unwrap();
    let spec: crate::add::NewTaskSpec = serde_json::from_value(serde_json::json!({"title":"fix","objective":"implement","acceptance":[{"type":"reviewer","text":"works"}],"project_id":project.id,"repos":["repo"]})).unwrap();
    let mut task = crate::add::create_task_with_roles(&store, spec, &[], &[], now).unwrap();
    task.assignee = None;
    let branch = format!("celeris/{}", task.id);
    git(&["checkout", "-b", &branch]);
    git(&["commit", "--allow-empty", "-m", "implemented"]);
    let root = dir.path().join("tasks");
    let fixture = SkipFixture {
        store,
        task,
        policy: DeliveryPolicy {
            default_departments: Default::default(),
        },
        root,
        repo: p,
        branch,
        _dir: dir,
    };
    fixture.write_marker(serde_json::json!([{"name":"repo","kind":"git","source":fixture.repo,"dir":fixture.repo,"branch":fixture.branch}]));
    fixture
}

impl SkipFixture {
    fn write_marker(&self, repos: serde_json::Value) {
        let taskdir = self.root.join(self.task.id.to_string());
        fs::create_dir_all(&taskdir).unwrap();
        fs::write(
            taskdir.join("worktree.json"),
            serde_json::json!({"repo":self.repo,"dir":self.repo,"branch":self.branch,"base":"","base_kind":"main","repos":repos}).to_string(),
        )
        .unwrap();
    }

    fn begin(&mut self) -> Option<Delivery> {
        begin(
            &self.store,
            &mut self.task,
            &roots(&self.root),
            &self.policy,
            "w",
            "r",
        )
        .unwrap()
    }

    fn skips(&self) -> Vec<(DeliverySkipReason, String, Option<String>)> {
        self.store
            .events_for(self.task.id)
            .unwrap()
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::DeliverySkipped {
                    reason,
                    detail,
                    head,
                } => Some((reason, detail, head)),
                _ => None,
            })
            .collect()
    }

    fn head(&self) -> String {
        let out = crate::changes::git(
            &self.repo,
            &["rev-parse", &self.branch],
            Duration::from_secs(10),
        )
        .unwrap();
        out.stdout.trim().to_string()
    }
}

#[test]
fn delivery_skip_multiple_repos() {
    let mut f = skip_fixture();
    let extra = f.task.repos[0].clone();
    f.task.repos.push(extra);
    assert!(f.begin().is_none());
    let skips = f.skips();
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0].0, DeliverySkipReason::MultipleRepos);
    assert!(skips[0].1.contains('2'), "{}", skips[0].1);
    assert_eq!(skips[0].2, None);
    assert_eq!(f.task.acceptance.len(), 1, "no merge criterion");
}

#[test]
fn delivery_skip_no_marker() {
    let mut f = skip_fixture();
    fs::remove_file(f.root.join(f.task.id.to_string()).join("worktree.json")).unwrap();
    assert!(f.begin().is_none());
    let skips = f.skips();
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0].0, DeliverySkipReason::NoMarker);
    assert!(skips[0].1.contains("worktree.json"));
    assert!(f.store.delivery_get(f.task.id).unwrap().is_none());
}

#[test]
fn delivery_skip_branch_mismatch() {
    let mut f = skip_fixture();
    f.write_marker(serde_json::json!([{"name":"repo","kind":"git","source":f.repo,"dir":f.repo,"branch":"celeris/other"}]));
    assert!(f.begin().is_none());
    let skips = f.skips();
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0].0, DeliverySkipReason::BranchNameMismatch);
    assert!(skips[0].1.contains("celeris/other"));
}

#[test]
fn delivery_skip_department_unresolved() {
    let mut f = skip_fixture();
    assert!(f.begin().is_none());
    let skips = f.skips();
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0].0, DeliverySkipReason::DepartmentUnresolved);
    assert_eq!(skips[0].2.as_deref(), Some(f.head().as_str()));
    assert!(f.store.delivery_get(f.task.id).unwrap().is_none());
    // 既定部署を設定すれば同じ root でも delivery が作られる。
    let project = f.task.project_id.unwrap().to_string();
    f.policy
        .default_departments
        .insert(project, "worker".into());
    let d = f.begin().unwrap();
    assert_eq!(d.department, "engineering");
    assert_eq!(f.skips().len(), 1);
}

#[test]
fn delivery_skip_once_per_head() {
    let mut f = skip_fixture();
    assert!(f.begin().is_none());
    assert!(f.begin().is_none());
    assert_eq!(f.skips().len(), 1, "same (reason, head) is recorded once");
    let first = f.head();
    let out = crate::changes::git(
        &f.repo,
        &["commit", "--allow-empty", "-m", "more"],
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(out.ok, "{}", out.stderr);
    assert!(f.begin().is_none());
    assert!(f.begin().is_none());
    let skips = f.skips();
    assert_eq!(skips.len(), 2, "a new head is reported again");
    assert_eq!(skips[0].2.as_deref(), Some(first.as_str()));
    assert_eq!(skips[1].2.as_deref(), Some(f.head().as_str()));
    // head の無い理由も同じ理由なら 1 件。
    fs::remove_file(f.root.join(f.task.id.to_string()).join("worktree.json")).unwrap();
    assert!(f.begin().is_none());
    assert!(f.begin().is_none());
    assert_eq!(f.skips().len(), 3);
}

#[test]
fn delivery_skip_tree_child_and_out_of_policy_are_recorded() {
    let mut f = skip_fixture();
    // 対象外の案件（marker も部署も無いので project 除外理由を優先する）。
    f.store
        .project_set_auto_integrate(f.task.project_id.unwrap(), false)
        .unwrap();
    fs::remove_file(f.root.join(f.task.id.to_string()).join("worktree.json")).unwrap();
    assert!(f.begin().is_none());
    assert_eq!(f.skips()[0].0, DeliverySkipReason::ProjectNotEnabled);
    // 対象案件の木の子。
    f.store
        .project_set_auto_integrate(f.task.project_id.unwrap(), true)
        .unwrap();
    let parent = TaskId::new();
    f.task.tree = Some(TreeInfo {
        root_id: parent,
        depth: 2,
        parent_unit: Some(ParentUnit {
            task_id: parent,
            plan_id: "plan".into(),
            unit_key: "child".into(),
            stage: "stage".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    assert!(f.begin().is_none());
    assert_eq!(f.skips()[1].0, DeliverySkipReason::TreeChild);
    assert!(f.store.delivery_get(f.task.id).unwrap().is_none());
}

impl SkipFixture {
    fn git(&self, args: &[&str]) -> String {
        let out = crate::changes::git(&self.repo, args, Duration::from_secs(10)).unwrap();
        assert!(out.ok, "{}", out.stderr);
        out.stdout.trim().to_string()
    }

    fn with_department(mut self) -> Self {
        let project = self.task.project_id.unwrap().to_string();
        self.policy
            .default_departments
            .insert(project, "worker".into());
        self
    }

    fn assert_no_commits_skip(&mut self) {
        assert!(self.begin().is_none());
        assert_eq!(self.task.acceptance.len(), 1, "no merge criterion");
        assert!(self.store.delivery_get(self.task.id).unwrap().is_none());
        let skips = self.skips();
        assert_eq!(skips.len(), 1, "{skips:?}");
        assert_eq!(skips[0].0, DeliverySkipReason::NoCommits);
        assert_eq!(skips[0].0.code(), "no_commits");
        assert_eq!(skips[0].2.as_deref(), Some(self.head().as_str()));
    }
}

/// 2026-10-11 付記: task の commit が 0 件（head = base）の root は取り込み判定を作らない。
#[test]
fn delivery_skip_no_commits_when_head_equals_base() {
    let mut f = skip_fixture().with_department();
    f.git(&["reset", "--hard", "main"]);
    assert_eq!(f.head(), f.git(&["rev-parse", "main"]));
    f.assert_no_commits_skip();
    // 同じ head では 1 件だけ。
    assert!(f.begin().is_none());
    assert_eq!(f.skips().len(), 1);
}

/// head が base の祖先（main が先に進んだ）でも取り込む commit は無い。
#[test]
fn delivery_skip_no_commits_when_head_is_ancestor_of_base() {
    let mut f = skip_fixture().with_department();
    f.git(&["reset", "--hard", "main"]);
    f.git(&["checkout", "main"]);
    f.git(&["commit", "--allow-empty", "-m", "another task"]);
    assert_ne!(f.head(), f.git(&["rev-parse", "main"]));
    f.assert_no_commits_skip();
}

/// head ≠ base の criterion は審査範囲を base..head と明示する。
#[test]
fn merge_criterion_names_base_to_head_range() {
    let mut f = skip_fixture().with_department();
    let d = f.begin().unwrap();
    let base = f.git(&["rev-parse", "main"]);
    assert_eq!(d.base, base);
    assert!(f.skips().is_empty());
    let text = &f.task.acceptance[d.criterion_idx].text;
    assert!(text.contains(&format!("{base}..{}", d.head)), "{text}");
    assert!(text.contains(&format!("{base}...{}", d.head)), "{text}");
    assert!(text.contains("審査対象ではない"), "{text}");
    assert!(!text.contains("指定SHA"), "{text}");
}

/// 固定の無い task の resolver（設定ファイルの root が `root`）。
fn roots(root: &std::path::Path) -> task_core::local_machine::WorkspaceRoots {
    task_core::local_machine::WorkspaceRoots::new(root.to_path_buf(), None)
}
