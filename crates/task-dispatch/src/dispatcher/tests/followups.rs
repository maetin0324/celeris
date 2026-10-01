//! ADR-0098（Phase R7-10）: worker の run が作る task は、その run の task の案件とリポジトリを継ぐ。
//!
//! - 後続（`followups.json`）: run に渡した `CELERIS_FOLLOWUPS_FILE` へ書く（run の中の `celerisctl add` と同じ
//!   `task_ops::followup::append_to_file`）→ run の後に daemon が X の案件で作り、出自を `worker_run` で残す。
//! - lease を失った run の宣言は作らない。
//! - 委譲の子（既存の経路）も案件と primary を継ぐ（回帰）。

use super::*;
use std::sync::Mutex as SyncMutex;

/// `with_env` で受け取った env を持ち、run の中で `CELERIS_FOLLOWUPS_FILE` に後続を 1 件追記して done になる。
struct FollowupAdapter {
    env: Vec<(String, String)>,
    seen_env: Arc<SyncMutex<Vec<(String, String)>>>,
}

#[async_trait]
impl WorkerAdapter for FollowupAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut env = self.env.clone();
        env.extend(extra.iter().cloned());
        Some(Arc::new(FollowupAdapter {
            env,
            seen_env: self.seen_env.clone(),
        }))
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        *self.seen_env.lock().unwrap() = self.env.clone();
        let Some((_, path)) = self
            .env
            .iter()
            .rev()
            .find(|(k, _)| k == task_ops::followup::ENV_FOLLOWUPS_FILE)
        else {
            return Err(AdapterError::Other("no CELERIS_FOLLOWUPS_FILE".into()));
        };
        let spec: task_ops::add::NewTaskSpec = serde_json::from_value(serde_json::json!({
            "title": "follow-up from the run",
            "objective": "continue",
            "acceptance": [{"type": "command", "cmd": "true"}],
        }))
        .unwrap();
        task_ops::followup::append_to_file(std::path::Path::new(path), &spec)
            .map_err(|e| AdapterError::Other(e.to_string()))?;
        Ok(done_outcome())
    }
}

fn project_with_primary(store: &Arc<dyn TaskStore>, dir: &std::path::Path) -> (ProjectId, RepoId) {
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "agent-platform".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "agent-platform".into(),
        kind: RepoKind::Dir,
        location: WorkspaceSpec::local(dir.join("repo")),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&repo).unwrap();
    (project.id, repo.id)
}

fn worker_run_id(store: &Arc<dyn TaskStore>, id: TaskId) -> String {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted { run_id, .. } => Some(run_id),
            _ => None,
        })
        .expect("worker started")
}

/// D1/D2/D3/D5/D6: 案件 P の task X の run が env の書き先に後続を宣言すると、run の後に daemon が P の task を
/// `draft` で作り、`Event::Created.origin` が `worker_run{X, run}` になる。env は run に渡っている。
#[tokio::test]
async fn a_run_declared_followup_is_created_in_the_origin_project_with_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project, primary) = project_with_primary(&store, dir.path());
    let mut x = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    // 案件に属すが repos は持たない（worktree の準備を避ける）→ 後続は案件の primary を継ぐ（D3-2）。
    x.project_id = Some(project);
    store.insert(&x).unwrap();
    let seen_env = Arc::new(SyncMutex::new(Vec::new()));
    let adapter = Arc::new(FollowupAdapter {
        env: Vec::new(),
        seen_env: seen_env.clone(),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.tick().unwrap();
    await_worker_completion(&mut d, x.id).await;

    let run_id = worker_run_id(&store, x.id);
    let env = seen_env.lock().unwrap().clone();
    assert!(env.contains(&(
        task_ops::followup::ENV_TASK_ID.to_string(),
        x.id.to_string()
    )));
    assert!(env.contains(&(task_ops::followup::ENV_RUN_ID.to_string(), run_id.clone())));

    let created: Vec<Task> = store
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|t| t.title == "follow-up from the run")
        .collect();
    assert_eq!(created.len(), 1, "{:?}", progress_msgs(&store, x.id));
    let f = &created[0];
    assert_eq!(f.project_id, Some(project));
    assert_eq!(
        f.repos.iter().map(|r| r.repo_id).collect::<Vec<_>>(),
        vec![primary]
    );
    assert_eq!(f.status, Status::Draft);
    assert_eq!(f.parent_id, None);
    let origin = store
        .events_for(f.id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::Created { origin, .. } => Some(origin),
            _ => None,
        })
        .flatten();
    assert_eq!(
        origin,
        Some(CreatedOrigin::WorkerRun {
            task_id: x.id,
            run_id: run_id.clone(),
        })
    );
    assert!(
        progress_msgs(&store, x.id)
            .iter()
            .any(|m| m.starts_with("follow-up created: "))
    );
}

/// D1: lease を持たない run（回収済み・別の run に移った）の宣言は作らず、ファイルも残す。
#[test]
fn followups_of_a_run_without_the_lease_are_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project, _) = project_with_primary(&store, dir.path());
    let mut x = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    x.project_id = Some(project);
    store.insert(&x).unwrap();
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    let spec: task_ops::add::NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "stale", "objective": "o", "acceptance": [{"type": "command", "cmd": "true"}],
    }))
    .unwrap();
    let file = artifacts.join(task_ops::followup::FOLLOWUPS_FILE_NAME);
    task_ops::followup::append_to_file(&file, &spec).unwrap();

    // `ready`（lease 無し）の task に対する run の宣言。
    let outcome = super::super::followups::absorb_run_followups(
        store.as_ref(),
        x.id,
        "run-gone",
        &artifacts,
        &[],
        &[],
    );
    assert!(outcome.is_none());
    assert!(file.exists(), "次の run の開始時に消える");
    assert!(store.list(None).unwrap().iter().all(|t| t.title != "stale"));
}

/// 回帰（ADR-0098 の調査）: 委譲の子は親の案件を継ぎ、親が repos を持たなければ案件の primary を継ぐ。
#[tokio::test]
async fn delegated_children_inherit_the_parent_project_and_primary_repo() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project, primary) = project_with_primary(&store, dir.path());
    let mut parent = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    parent.project_id = Some(project);
    store.insert(&parent).unwrap();
    let adapter = Arc::new(DelegatingAdapter {
        proposals: vec![proposal("child", vec![])],
        child_delay: Duration::from_millis(1),
        seen_role: std::sync::Mutex::new(None),
        aggregate_children: AtomicUsize::new(0),
        write_summary: false,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.tick().unwrap();
    await_worker_completion(&mut d, parent.id).await;
    let children = store.children(parent.id).unwrap();
    assert_eq!(children.len(), 1, "{:?}", progress_msgs(&store, parent.id));
    assert_eq!(children[0].project_id, Some(project));
    assert_eq!(
        children[0]
            .repos
            .iter()
            .map(|r| r.repo_id)
            .collect::<Vec<_>>(),
        vec![primary]
    );
}
