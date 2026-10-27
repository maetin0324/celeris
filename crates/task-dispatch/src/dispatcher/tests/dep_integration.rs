//! ADR 2026-10-17-root-dependency-waits-for-integration: root 同士の depends_on は、依存先が done でも
//! 既定 branch に入るまで dispatch しない。temp DB と一時 git repo だけ（外部ネットワークなし）。

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

fn slow() -> Arc<InstantAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_secs(30),
    })
}

fn true_task(dir: &std::path::Path) -> Task {
    new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    )
}

/// 一時 git repo（main に 1 commit）を案件に登録し、done の依存先 root と、それを待つ ready の root を作る。
/// 依存先の branch `celeris/<id>` には main に無い commit がある。
fn setup(
    dir: &std::path::Path,
    pred_tree: Option<TreeInfo>,
) -> (Arc<dyn TaskStore>, std::path::PathBuf, Task, Task, String) {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("base"), "base").unwrap();
    git(&repo, &["add", "base"]);
    git(&repo, &["commit", "-m", "base"]);

    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        auto_integrate: true,
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
            id: RepoId::new(),
            project_id: project.id,
            name: "r".into(),
            kind: RepoKind::Git,
            location: WorkspaceSpec::local(repo.clone()),
            default_branch: Some("main".into()),
            sync: None,
            run: RepoRun::Auto,
            is_primary: true,
            created_at: now,
        })
        .unwrap();

    let mut pred = true_task(dir);
    pred.status = Status::Done;
    pred.project_id = Some(project.id);
    pred.tree = pred_tree;
    store.insert(&pred).unwrap();
    let branch = format!("celeris/{}", pred.id);
    git(&repo, &["switch", "-c", &branch]);
    std::fs::write(repo.join("pred"), "pred").unwrap();
    git(&repo, &["add", "pred"]);
    git(&repo, &["commit", "-m", "pred"]);
    git(&repo, &["switch", "main"]);

    let mut dependent = true_task(dir);
    dependent.depends_on = vec![pred.id];
    store.insert(&dependent).unwrap();
    (store, repo, pred, dependent, branch)
}

fn publish(d: &mut Dispatcher) -> tokio::sync::watch::Receiver<Option<DaemonSnapshot>> {
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-10-17T00:00:00Z".into(),
        tick_ms: 50,
    });
    rx
}

fn wait_of(
    rx: &tokio::sync::watch::Receiver<Option<DaemonSnapshot>>,
    id: TaskId,
) -> Option<TaskWait> {
    rx.borrow()
        .as_ref()
        .expect("snapshot")
        .task_waits
        .iter()
        .find(|w| w.task_id == id)
        .map(|w| w.wait.clone())
}

/// done だが未取り込みの依存先は待ち（ready のまま・理由が載る）、main へ入ると次の tick で dispatch される。
#[tokio::test]
async fn root_dependent_waits_until_dependency_branch_is_integrated() {
    let dir = tempfile::tempdir().unwrap();
    let (store, repo, pred, dependent, branch) = setup(dir.path(), None);
    let mut d = dispatcher(store.clone(), slow(), 4);
    let rx = publish(&mut d);

    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(
        store.get(dependent.id).unwrap().unwrap().status,
        Status::Ready
    );
    match wait_of(&rx, dependent.id) {
        Some(TaskWait::DependsOnIntegration {
            task_id,
            cause: IntegrationWaitCause::Unmerged,
            detail,
        }) => {
            assert_eq!(task_id, pred.id);
            assert!(detail.contains(&branch), "{detail}");
        }
        other => panic!("unexpected wait: {other:?}"),
    }
    // 次の tick でも待つ（再起動なしで記憶が誤って「取り込み済み」にならない）。
    assert_eq!(d.tick().unwrap().dispatched, 0);

    git(&repo, &["merge", "--no-ff", "-m", "merge", &branch]);
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(
        store.get(dependent.id).unwrap().unwrap().status,
        Status::Running
    );
    assert_eq!(wait_of(&rx, dependent.id), None);
}

/// delivery が取り込みを見送った依存先は、待ち理由に見送りの code が載る。
#[tokio::test]
async fn root_dependent_wait_names_delivery_skip() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _repo, pred, dependent, _branch) = setup(dir.path(), None);
    store
        .append_event(
            pred.id,
            &Event::DeliverySkipped {
                reason: DeliverySkipReason::DepartmentUnresolved,
                detail: "部が決まらない".into(),
                head: None,
            },
        )
        .unwrap();
    let mut d = dispatcher(store.clone(), slow(), 4);
    let rx = publish(&mut d);
    assert_eq!(d.tick().unwrap().dispatched, 0);
    match wait_of(&rx, dependent.id) {
        Some(TaskWait::DependsOnIntegration {
            cause: IntegrationWaitCause::DeliverySkipped,
            detail,
            ..
        }) => assert!(detail.contains("department_unresolved"), "{detail}"),
        other => panic!("unexpected wait: {other:?}"),
    }
}

/// 木の内側（依存先が子 task）の depends_on は、branch が未取り込みでも従来どおり done で進む。
#[tokio::test]
async fn tree_child_dependency_is_not_gated_on_integration() {
    let dir = tempfile::tempdir().unwrap();
    let mut tree = TreeInfo::root(TaskId::new());
    tree.depth = 2;
    tree.parent_unit = Some(ParentUnit {
        task_id: TaskId::new(),
        plan_id: "plan".into(),
        unit_key: "u".into(),
        stage: "s".into(),
        attempt: 1,
    });
    let (store, _repo, _pred, dependent, _branch) = setup(dir.path(), Some(tree));
    let mut d = dispatcher(store.clone(), slow(), 4);
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(
        store.get(dependent.id).unwrap().unwrap().status,
        Status::Running
    );
}
