use super::*;
use std::fs;
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
        slug: None,
        id: ProjectId::new(),
        title: "test".into(),
        request: "fix".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
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
    let branch = format!("celeris/{}", task.id);
    git(&["checkout", "-b", &branch]);
    git(&["commit", "--allow-empty", "-m", "implemented"]);
    let head = git(&["rev-parse", "HEAD"]);
    let root = dir.path().join("tasks");
    let taskdir = root.join(task.id.to_string());
    fs::create_dir_all(&taskdir).unwrap();
    fs::write(taskdir.join("worktree.json"),serde_json::json!({"repo":p,"dir":p,"branch":branch,"base":"","base_kind":"main","repos":[{"name":"repo","kind":"git","source":p,"dir":p,"branch":branch}]}).to_string()).unwrap();
    let policy = DeliveryPolicy {
        projects: vec![project.id.to_string()],
        repo: p,
    };
    // ADR-0079 D6（Phase R1c）: 同じ task が木の子なら、取り込みの判定も `deliveries` の行も作らない
    // （成果は親の段階の統合で親のブランチへ）。root は下のとおり今までと同じ（回帰）。
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
        begin(&store, &mut as_child, &root, &policy, "w", "r")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        as_child.acceptance.len(),
        1,
        "no merge criterion for a child"
    );
    assert!(store.delivery_get(task.id).unwrap().is_none());
    let d = begin(
        &store,
        &mut task,
        &root,
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
        begin(&store, &mut unrelated, &root, &policy, "w", "r")
            .unwrap()
            .is_none()
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
