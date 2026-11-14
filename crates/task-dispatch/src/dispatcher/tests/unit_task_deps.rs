//! ADR 2026-11-11-unit-task-dependencies: 計画の unit の `task:<id>` 依存（木の外の task の完了・取り込みを待つ）。
//! temp DB と一時 git repo だけ（外部ネットワークなし）。生存確認の 600 秒は注入した時計で進め、固定の sleep で順序を作らない。
//! 既存の試験は unit の依存を同じ計画の key でしか書けなかったので、待ち・始まり・失敗の知らせ・検証をここで固定する。

use super::*;

fn git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    store: Arc<dyn TaskStore>,
    /// 依存先の案件の repo（依存先の branch `celeris/<id>` を持つ）。
    dep_repo: std::path::PathBuf,
    project: ProjectId,
    dep: Task,
    branch: String,
    /// 計画を持つ task の repo と workspace の root。
    _owner_repo: tempfile::TempDir,
    root: tempfile::TempDir,
    owner: Task,
}

/// 依存先の案件（一時 git repo を登録）に `dep_status` の root `dep` を作る（done なら main に無い commit の branch も）。
/// 計画を持つ root `owner`（別の一時 repo の git task）に v2 の計画（段 `s`）を採用し、`with_dep` の unit の依存を
/// `task:<dep>` にして pending に戻す。
fn fixture(
    dir: &std::path::Path,
    dep_status: Status,
    units: &[&str],
    with_dep: &[&str],
) -> Fixture {
    let dep_repo = dir.join("dep-repo");
    std::fs::create_dir_all(&dep_repo).unwrap();
    git(&dep_repo, &["init", "-b", "main"]);
    std::fs::write(dep_repo.join("base"), "base").unwrap();
    git(&dep_repo, &["add", "base"]);
    git(&dep_repo, &["commit", "-m", "base"]);

    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    store
        .repo_create(&ProjectRepo {
            auto_integrate: true,
            id: RepoId::new(),
            project_id: project.id,
            name: "r".into(),
            kind: RepoKind::Git,
            location: WorkspaceSpec::local(dep_repo.clone()),
            default_branch: Some("main".into()),
            sync: None,
            run: RepoRun::Auto,
            is_primary: true,
            created_at: now,
        })
        .unwrap();
    let mut dep = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    dep.status = dep_status;
    dep.project_id = Some(project.id);
    store.insert(&dep).unwrap();
    let branch = format!("celeris/{}", dep.id);
    git(&dep_repo, &["switch", "-c", &branch]);
    std::fs::write(dep_repo.join("dep"), "dep").unwrap();
    git(&dep_repo, &["add", "dep"]);
    git(&dep_repo, &["commit", "-m", "dep"]);
    git(&dep_repo, &["switch", "main"]);

    let owner_repo = tempfile::tempdir().unwrap();
    init_test_repo(owner_repo.path());
    let root = tempfile::tempdir().unwrap();
    let owner = parallel_task(owner_repo.path(), "true");
    store.insert(&owner).unwrap();
    adopt_v2_plan(
        &store,
        owner.id,
        &["s"],
        units.iter().map(|k| v2_wu(k, "s", &[])).collect(),
    );
    let dep_key = task_core::task_dependency_key(dep.id);
    for u in store.work_units_for(owner.id).unwrap() {
        if !with_dep.contains(&u.key.as_str()) {
            continue;
        }
        let mut row = u.clone();
        row.status = task_core::WorkUnitStatus::Pending;
        row.depends_on = vec![dep_key.clone()];
        row.spec.depends_on = vec![dep_key.clone()];
        store
            .work_unit_transition(
                owner.id,
                row,
                Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: u.status,
                    to: task_core::WorkUnitStatus::Pending,
                    reason: "test_shape".into(),
                    run_id: None,
                },
            )
            .unwrap();
    }
    Fixture {
        store,
        dep_repo,
        project: project.id,
        dep,
        branch,
        _owner_repo: owner_repo,
        root,
        owner,
    }
}

fn holding(keys: &[&str]) -> Arc<ParallelWuAdapter> {
    let mut adapter = ParallelWuAdapter::new(Duration::ZERO);
    for k in keys {
        adapter = adapter.holding(k);
    }
    Arc::new(adapter)
}

fn unit_status(store: &Arc<dyn TaskStore>, task: TaskId, key: &str) -> task_core::WorkUnitStatus {
    store
        .work_units_for(task)
        .unwrap()
        .into_iter()
        .find(|u| u.key == key)
        .unwrap()
        .status
}

fn publish(d: &mut Dispatcher) -> tokio::sync::watch::Receiver<Option<DaemonSnapshot>> {
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-11-11T00:00:00Z".into(),
        tick_ms: 50,
    });
    rx
}

/// 一覧・詳細の `waits`（task-ops の `task_waits`。M11 はここで組む）のうち `task_dependency`。
fn wait_of(store: &Arc<dyn TaskStore>, id: TaskId) -> Option<TaskWait> {
    let task = store.get(id).unwrap().unwrap();
    task_ops::wait::task_waits_one(
        store.as_ref(),
        &task,
        &task_ops::wait::MemoryWaits::default(),
    )
    .unwrap()
    .into_iter()
    .find(|w| matches!(w, TaskWait::TaskDependency { .. }))
}

fn stalls(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::StallDetected { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// (a) done でない依存先: unit は始まらず、待ちの理由に依存先と `unfinished` が出る。
#[tokio::test]
async fn unit_task_dependency_waits_while_the_dependency_is_not_done() {
    let dir = tempfile::tempdir().unwrap();
    let fx = fixture(dir.path(), Status::Blocked, &["final"], &["final"]);
    let mut d = parallel_dispatcher(
        fx.store.clone(),
        holding(&["final"]),
        fx.root.path(),
        2,
        2,
        2,
    );
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(
        unit_status(&fx.store, fx.owner.id, "final"),
        task_core::WorkUnitStatus::Pending
    );
    match wait_of(&fx.store, fx.owner.id) {
        Some(TaskWait::TaskDependency {
            task_id,
            cause: task_core::TaskDependencyCause::Unfinished,
            unit_keys,
            ..
        }) => {
            assert_eq!(task_id, fx.dep.id);
            assert_eq!(unit_keys, vec!["final".to_string()]);
        }
        other => panic!("unexpected wait: {other:?}"),
    }
}

/// (a) root 同士で依存先は done だが既定 branch に入っていない: 始まらず `unmerged` が出る。main に入ると次の tick で始まる。
#[tokio::test]
async fn unit_task_dependency_starts_on_the_tick_after_the_dependency_is_integrated() {
    let dir = tempfile::tempdir().unwrap();
    let fx = fixture(dir.path(), Status::Done, &["final"], &["final"]);
    let mut d = parallel_dispatcher(
        fx.store.clone(),
        holding(&["final"]),
        fx.root.path(),
        2,
        2,
        2,
    );
    assert_eq!(d.tick().unwrap().dispatched, 0);
    match wait_of(&fx.store, fx.owner.id) {
        Some(TaskWait::TaskDependency {
            task_id,
            cause: task_core::TaskDependencyCause::Unmerged,
            detail,
            ..
        }) => {
            assert_eq!(task_id, fx.dep.id);
            assert!(detail.contains(&fx.branch), "{detail}");
        }
        other => panic!("unexpected wait: {other:?}"),
    }
    assert_eq!(d.tick().unwrap().dispatched, 0, "still waiting");

    git(
        &fx.dep_repo,
        &["merge", "--no-ff", "-m", "merge", &fx.branch],
    );
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(
        unit_status(&fx.store, fx.owner.id, "final"),
        task_core::WorkUnitStatus::Running
    );
    let met = fx.store.events_for(fx.owner.id).unwrap().into_iter().any(|(_, e)| {
        matches!(e, Event::WorkUnitTransitioned { key, reason, .. } if key == "final" && reason == "task_dependency_met")
    });
    assert!(met, "promoted by the task dependency");
}

/// (b) 同じ計画の、task 依存を持たない unit は待たずに進む（依存を持つ unit は pending のまま）。
#[tokio::test]
async fn units_without_a_task_dependency_run_while_another_unit_waits() {
    let dir = tempfile::tempdir().unwrap();
    let fx = fixture(dir.path(), Status::Blocked, &["free", "final"], &["final"]);
    let mut d = parallel_dispatcher(
        fx.store.clone(),
        holding(&["free", "final"]),
        fx.root.path(),
        2,
        2,
        2,
    );
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(
        unit_status(&fx.store, fx.owner.id, "free"),
        task_core::WorkUnitStatus::Running
    );
    assert_eq!(
        unit_status(&fx.store, fx.owner.id, "final"),
        task_core::WorkUnitStatus::Pending
    );
}

/// (c) 依存先が failed・cancelled: unit は始まらず（replan も起こさない）、生存確認が `task_dependency_ended` の
/// `StallDetected` を 1 回だけ出し、受信箱の `stalled` の元（snapshot の `stalled_tasks`）に 1 件載る。
#[tokio::test]
async fn ended_task_dependency_keeps_the_unit_and_notifies_once() {
    for status in [Status::Failed, Status::Cancelled] {
        let dir = tempfile::tempdir().unwrap();
        let fx = fixture(dir.path(), status, &["final"], &["final"]);
        let mut d = parallel_dispatcher(
            fx.store.clone(),
            holding(&["final"]),
            fx.root.path(),
            2,
            2,
            2,
        );
        let t0 = OffsetDateTime::now_utc() + time::Duration::minutes(5);
        let clock = Arc::new(StdMutex::new(t0));
        d.test_now = Some(clock.clone());
        assert_eq!(d.tick().unwrap().dispatched, 0);
        *clock.lock().unwrap() = t0 + time::Duration::seconds(599);
        d.tick().unwrap();
        assert!(
            stalls(&fx.store, fx.owner.id).is_empty(),
            "not before 600 s"
        );
        *clock.lock().unwrap() = t0 + time::Duration::seconds(630);
        d.tick().unwrap();
        *clock.lock().unwrap() = t0 + time::Duration::seconds(1300);
        d.tick().unwrap();
        assert_eq!(
            stalls(&fx.store, fx.owner.id),
            vec!["task_dependency_ended".to_string()],
            "{status:?}: exactly one notice"
        );
        assert_eq!(d.stalled_tasks.len(), 1);
        let snap = d.stalled_tasks.get(&fx.owner.id).expect("stalled task");
        assert_eq!(snap.reason, "task_dependency_ended");
        assert_eq!(
            unit_status(&fx.store, fx.owner.id, "final"),
            task_core::WorkUnitStatus::Pending
        );
        assert_eq!(
            fx.store.get(fx.owner.id).unwrap().unwrap().status,
            Status::Ready
        );
    }
}

/// (e) task 依存の待ちは人の操作待ちではない: `human_wait_secs`（既定 2 時間）を大きく過ぎても、受信箱に
/// `human_wait_aging` も、人の操作待ちの項目も、`task_unscheduled` も出ない。
#[tokio::test]
async fn task_dependency_wait_does_not_raise_human_wait_aging() {
    let dir = tempfile::tempdir().unwrap();
    let fx = fixture(dir.path(), Status::Done, &["final"], &["final"]);
    let mut d = parallel_dispatcher(
        fx.store.clone(),
        holding(&["final"]),
        fx.root.path(),
        2,
        2,
        2,
    );
    let rx = publish(&mut d);
    let t0 = OffsetDateTime::now_utc();
    let clock = Arc::new(StdMutex::new(t0));
    d.test_now = Some(clock.clone());
    d.tick().unwrap();
    let later = t0 + time::Duration::hours(15);
    *clock.lock().unwrap() = later;
    d.tick().unwrap();
    assert!(matches!(
        wait_of(&fx.store, fx.owner.id),
        Some(TaskWait::TaskDependency { .. })
    ));
    let snapshot = rx.borrow().clone();
    let inbox = task_ops::human_inbox::human_inbox(
        fx.store.as_ref(),
        snapshot.as_ref(),
        &task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                PathBuf::from("/nonexistent"),
                None,
            ),
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            max_requeues: 5,
            clusters: Default::default(),
        },
        later,
        &|_, _| Vec::new(),
    )
    .unwrap();
    assert!(
        !inbox.items.iter().any(|i| {
            // 待ちの理由があるので `task_unscheduled`（理由なく予定されない）にもならない。
            i.kind == task_ops::human_inbox::InboxKind::HumanWaitAging
                || i.kind == task_ops::human_inbox::InboxKind::TaskUnscheduled
                || task_ops::human_inbox::InboxKind::HUMAN_WAITS.contains(&i.kind)
        }),
        "{:?}",
        inbox.items.iter().map(|i| i.kind).collect::<Vec<_>>()
    );
}

/// (d) 採用の前の検証（store の事実で）: 存在しない task・自分の木の task（子）・別の案件・循環（依存先がこの task を
/// `depends_on` で待つ）を拒み、木の外の同じ案件の task は通す。人の採用の経路（task-ops）も同じ検証で拒む。
#[tokio::test]
async fn plan_validation_rejects_missing_own_tree_other_project_and_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let fx = fixture(dir.path(), Status::Done, &["final"], &[]);
    let store = &fx.store;
    let mut owner = fx.owner.clone();
    owner.project_id = Some(fx.project);
    let mk = |project: Option<ProjectId>, parent: Option<TaskId>, deps: Vec<TaskId>| {
        let mut t = new_task(
            dir.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            0,
        );
        t.status = Status::Blocked;
        t.project_id = project;
        t.parent_id = parent;
        t.depends_on = deps;
        store.insert(&t).unwrap();
        t
    };
    let child = mk(Some(fx.project), Some(owner.id), vec![]);
    let other = mk(None, None, vec![]);
    let waits_back = mk(Some(fx.project), None, vec![owner.id]);
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../task-core/testdata/execution-plan/v3-browser.json"
    ))
    .unwrap();
    let spec_with = |id: TaskId| {
        let mut spec: task_core::ExecutionPlanSpec = serde_json::from_str(&text).unwrap();
        let p3 = spec.units.iter_mut().find(|u| u.key == "p3").unwrap();
        p3.depends_on.push(task_core::task_dependency_key(id));
        spec
    };
    let errors = |id: TaskId| -> Vec<task_core::TaskDependencyRejection> {
        task_ops::unit_task_deps::plan_errors(store.as_ref(), &owner, &spec_with(id))
            .unwrap()
            .into_iter()
            .map(|e| e.reason)
            .collect()
    };
    use task_core::TaskDependencyRejection as R;
    let missing = TaskId::new();
    assert_eq!(errors(missing), vec![R::NotFound]);
    assert_eq!(errors(child.id), vec![R::InOwnTree]);
    assert_eq!(errors(owner.id), vec![R::InOwnTree]);
    assert_eq!(errors(other.id), vec![R::OtherProject]);
    assert_eq!(errors(waits_back.id), vec![R::Cycle]);
    assert!(errors(fx.dep.id).is_empty());

    // 人の採用（task-ops の経路）も同じ検証で拒む。
    let mut fresh = mk(Some(fx.project), None, vec![]);
    fresh.status = Status::Ready;
    let refused = task_ops::execution::adopt_plan(
        store.as_ref(),
        fresh.id,
        spec_with(missing),
        task_core::PlanOrigin::Human,
        None,
        task_core::ExecutionLimits {
            tree: task_core::TreeLimits {
                enabled: true,
                ..task_core::TreeLimits::default()
            },
            ..task_core::ExecutionLimits::default()
        },
        OffsetDateTime::now_utc(),
    );
    assert!(
        matches!(&refused, Err(task_ops::OpsError::Validation(m)) if m.contains("no such task")),
        "{refused:?}"
    );
}

/// (d) 委譲の木（`tree` を持たず `parent_id` だけで親を指す子）の検証。守る不変条件: 祖先（委譲の親・その親）を指す依存と、
/// 依存先の子を経由する循環（親は委譲の子を待つ）は採用しない。委譲の兄弟は木の外なので待てる。
/// 上の試験は計画を持つ task が root で、`depends_on` の辺だけの循環しか作らないので、`tree` だけで木を数える検証と
/// 子の辺の無い循環の探索がこれらを通しても落ちない（委譲の子の unit が親の done を待って黙って止まり続ける不具合）。
#[tokio::test]
async fn plan_validation_rejects_delegation_ancestors_and_cycles_through_children() {
    let dir = tempfile::tempdir().unwrap();
    let fx = fixture(dir.path(), Status::Done, &["final"], &[]);
    let store = &fx.store;
    let mk = |parent: Option<TaskId>, deps: Vec<TaskId>| {
        let mut t = new_task(
            dir.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            0,
        );
        t.status = Status::Blocked;
        t.project_id = Some(fx.project);
        t.parent_id = parent;
        t.depends_on = deps;
        store.insert(&t).unwrap();
        t
    };
    // 委譲の木: grandparent → parent → { owner（計画を持つ）, sibling }。owner の下に委譲の子 grandchild。
    let grandparent = mk(None, vec![]);
    let parent = mk(Some(grandparent.id), vec![]);
    let owner = mk(Some(parent.id), vec![]);
    let sibling = mk(Some(parent.id), vec![]);
    let grandchild = mk(Some(owner.id), vec![]);
    // owner の計画の kind task の子（木の節点）と、その委譲の子（木の節点の下の子孫）。
    let mut tree_child = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    tree_child.status = Status::Blocked;
    tree_child.project_id = Some(fx.project);
    tree_child.parent_id = Some(owner.id);
    tree_child.tree = Some(task_core::TreeInfo::child_of(
        &owner,
        task_core::ParentUnit {
            task_id: owner.id,
            plan_id: "plan".into(),
            unit_key: "c".into(),
            stage: "s".into(),
            attempt: 1,
        },
        None,
    ));
    store.insert(&tree_child).unwrap();
    let under_tree_child = mk(Some(tree_child.id), vec![]);
    // 木の外の root `holder` は、委譲の子 `blocked_on_owner` を通して owner を待つ（holder → 子 → owner）。
    let holder = mk(None, vec![]);
    let _blocked_on_owner = mk(Some(holder.id), vec![owner.id]);
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../task-core/testdata/execution-plan/v3-browser.json"
    ))
    .unwrap();
    let errors = |id: TaskId| -> Vec<task_core::TaskDependencyRejection> {
        let mut spec: task_core::ExecutionPlanSpec = serde_json::from_str(&text).unwrap();
        let p3 = spec.units.iter_mut().find(|u| u.key == "p3").unwrap();
        p3.depends_on.push(task_core::task_dependency_key(id));
        task_ops::unit_task_deps::plan_errors(store.as_ref(), &owner, &spec)
            .unwrap()
            .into_iter()
            .map(|e| e.reason)
            .collect()
    };
    use task_core::TaskDependencyRejection as R;
    assert_eq!(
        errors(parent.id),
        vec![R::InOwnTree],
        "the delegating parent"
    );
    assert_eq!(errors(grandparent.id), vec![R::InOwnTree], "an ancestor");
    assert_eq!(errors(grandchild.id), vec![R::InOwnTree], "a descendant");
    assert_eq!(errors(tree_child.id), vec![R::InOwnTree], "a tree node");
    assert_eq!(
        errors(under_tree_child.id),
        vec![R::InOwnTree],
        "a descendant under a tree node"
    );
    assert_eq!(
        errors(holder.id),
        vec![R::Cycle],
        "waits on owner via a child"
    );
    assert!(errors(sibling.id).is_empty(), "a delegated sibling");
}
