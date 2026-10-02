//! ADR-0124 D2/D3: atomic coding task の planner なし直行経路（決定的 routing）の dispatcher 回帰試験。
//! 試験名は ADR-0124 D6 の規約どおり `direct_route` と `fast_path` の両方を含む。

use super::*;

/// 実行 run・planner run・reviewer run を受け、見た `RunContext` を残す偽アダプタ。
/// planner run には 1 unit の計画を書き、reviewer run は全条件を合格にする。
struct RouteAdapter {
    seen: Arc<StdMutex<Vec<(TaskKind, task_worker::RunContext)>>>,
}

impl RouteAdapter {
    fn new() -> Arc<Self> {
        Arc::new(RouteAdapter {
            seen: Arc::new(StdMutex::new(Vec::new())),
        })
    }

    fn planner_runs(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, c)| c.execution_planner.is_some())
            .count()
    }

    fn worker_contexts(&self) -> Vec<task_worker::RunContext> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, c)| *k == TaskKind::Execute && c.execution_planner.is_none())
            .map(|(_, c)| c.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for RouteAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen
            .lock()
            .unwrap()
            .push((req.task.kind, req.context.clone()));
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        if req.task.kind == TaskKind::Review {
            let verdicts: Vec<String> = (0..req.task.acceptance.len().max(4))
                .map(|i| format!(r#"{{"criterion":{i},"pass":true,"reason":"ok"}}"#))
                .collect();
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                format!(r#"{{"verdicts":[{}]}}"#, verdicts.join(",")),
            )
            .unwrap();
        } else if req.context.execution_planner.is_some() {
            std::fs::write(
                req.artifacts_dir.join("execution-plan.json"),
                plan_json(vec![wu_spec("a", &[])]),
            )
            .unwrap();
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// 秘書の下に部署 `engineering`（既定ハーネス coding）と課 `software` を仕込む。
fn seed_coding_org(store: &Arc<dyn TaskStore>) {
    store
        .org_upsert(&org_node_of(
            "secretary",
            None,
            OrgKind::Secretary,
            Some("secretary"),
        ))
        .unwrap();
    let mut dept = org_node_of("engineering", Some("secretary"), OrgKind::Department, None);
    dept.profile.harnesses.default = Some("coding".into());
    dept.profile.skills = vec!["rust".into()];
    store.org_upsert(&dept).unwrap();
    store
        .org_upsert(&org_node_of(
            "software",
            Some("engineering"),
            OrgKind::Section,
            None,
        ))
        .unwrap();
}

/// 直行の条件を全部満たす task: 担当 `software`（coding）・repo 1 つ・command check あり・
/// 規則表のスコアだけで compound/score になる features（文の長さに引きずられた compound を模す）。
fn atomic_coding_task(
    store: &Arc<dyn TaskStore>,
    root: &std::path::Path,
    repo_names: &[&str],
) -> Task {
    seed_coding_org(store);
    let dirs: Vec<PathBuf> = repo_names
        .iter()
        .map(|n| {
            let p = root.join("src").join(n);
            std::fs::create_dir_all(&p).unwrap();
            p
        })
        .collect();
    let specs: Vec<(&str, &std::path::Path, task_core::RepoKind)> = repo_names
        .iter()
        .zip(dirs.iter())
        .map(|(n, p)| (*n, p.as_path(), task_core::RepoKind::Dir))
        .collect();
    let (project_id, repos) = project_with_repos(store, &specs);
    let mut task = new_task(
        &dirs[0],
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.acceptance.push(Criterion {
        text: "レビューで確かめる".into(),
        check: Check::Reviewer,
    });
    task.project_id = Some(project_id);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    task.assignee = Some("software".into());
    task.budget.max_turns = 40;
    task.routing = Some(task_core::TaskRouting {
        features: Some(task_core::model_policy::TaskFeatureHints {
            context_size: Some(task_core::model_policy::Level::High),
            expected_length: Some(task_core::model_policy::Level::High),
            tool_intensity: Some(task_core::model_policy::Level::High),
            ..Default::default()
        }),
        ..Default::default()
    });
    task
}

fn route_dispatcher(
    store: &Arc<dyn TaskStore>,
    adapter: Arc<RouteAdapter>,
    root: &std::path::Path,
    gate: task_core::GateMode,
) -> Dispatcher {
    let mut d = worktree_dispatcher(store.clone(), adapter, &root.join("ws"), None);
    d.config.execution.gate = gate;
    d.config.execution.planner.adapter = "instant".to_string();
    d
}

fn recorded_route(store: &Arc<dyn TaskStore>, id: TaskId) -> task_core::RouteDecision {
    store
        .get(id)
        .unwrap()
        .unwrap()
        .routing
        .and_then(|r| r.route)
        .expect("route decision recorded")
}

fn routed_events(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<task_core::RouteDecision> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::ExecutionRouted { decision } => Some(*decision),
            _ => None,
        })
        .collect()
}

fn failed_rules(decision: &task_core::RouteDecision) -> Vec<String> {
    decision
        .reasons
        .iter()
        .filter(|r| !r.ok)
        .map(|r| r.rule_id.clone())
        .collect()
}

/// planned の task を planner が起きるまで回し、経路の記録と planner run を確かめる。
async fn assert_planned_with_planner(
    store: &Arc<dyn TaskStore>,
    adapter: &Arc<RouteAdapter>,
    d: &mut Dispatcher,
    id: TaskId,
    failed_rule: &str,
) {
    let a = adapter.clone();
    assert!(
        run_until(d, 400, move || a.planner_runs() > 0).await,
        "a planner run should be dispatched for a planned task"
    );
    let decision = recorded_route(store, id);
    assert_eq!(decision.route, task_core::Route::Planned, "{decision:?}");
    assert!(
        failed_rules(&decision).iter().any(|r| r == failed_rule),
        "{decision:?}"
    );
    assert!(!decision.overrode_gate);
    assert_eq!(routed_events(store, id).len(), 1);
    assert!(
        adapter
            .worker_contexts()
            .iter()
            .all(|c| c.direct_route.is_none()),
        "a planned task's runs must not carry the direct route section"
    );
}

/// ADR-0124 D3: 直行の条件を満たす task は planner run なしで 1 implementation run → command checks →
/// final reviewer を通って done になり、`ExecutionRouted{route: direct}` が 1 件残る。
#[tokio::test]
async fn direct_route_fast_path_atomic_coding_task_skips_planner() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = atomic_coding_task(&store, root.path(), &["code"]);
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::On,
    );
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let gate = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(gate.mode, task_core::ExecutionMode::Compound);
    assert_eq!(gate.rule_id, "compound/score");

    let decision = recorded_route(&store, id);
    assert_eq!(decision.route, task_core::Route::Direct, "{decision:?}");
    assert!(decision.overrode_gate);
    assert!(!decision.shadow);
    assert_eq!(routed_events(&store, id), vec![decision.clone()]);

    // planner run も計画も無い。worker run 1 本と reviewer run 1 本だけ。
    assert_eq!(adapter.planner_runs(), 0);
    assert!(store.execution_plan_active(id).unwrap().is_none());
    assert!(store.work_units_for(id).unwrap().is_empty());
    let runs = store.runs_for_task(id).unwrap();
    assert!(
        runs.iter()
            .all(|r| r.role != task_core::RunIndexRole::Planner),
        "{runs:?}"
    );
    let workers = runs
        .iter()
        .filter(|r| r.role == task_core::RunIndexRole::Worker)
        .count();
    let reviewers = runs
        .iter()
        .filter(|r| r.role == task_core::RunIndexRole::Reviewer)
        .count();
    assert_eq!((workers, reviewers), (1, 1), "{runs:?}");

    // worker には直行の節の材料が届き、reviewer には届かない。
    let worker_contexts = adapter.worker_contexts();
    assert_eq!(worker_contexts.len(), 1);
    let ctx = worker_contexts[0]
        .direct_route
        .as_ref()
        .expect("direct route context on the implementation run");
    assert_eq!(ctx.policy_version, task_core::DIRECT_ROUTE_POLICY_VERSION);
    assert!(ctx.overrode_gate);
    assert!(
        ctx.reasons
            .iter()
            .any(|r| r.starts_with("direct/single-repo"))
    );
    assert!(
        adapter
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k == TaskKind::Review)
            .all(|(_, c)| c.direct_route.is_none())
    );

    // command check（criterion 0）は決定的に再実行されて合格している。
    assert!(
        events_of(&store, id).iter().any(|e| matches!(
            e,
            Event::ReviewVerdict {
                criterion_idx: 0,
                pass: true,
                ..
            }
        )),
        "the command check verdict should be recorded"
    );
}

/// ADR-0124 D1: repo が 2 つ（cross-repo）の task は従来の planner 経路。
#[tokio::test]
async fn direct_route_fast_path_cross_repo_uses_planner() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = atomic_coding_task(&store, root.path(), &["code", "bench"]);
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::On,
    );
    assert_planned_with_planner(&store, &adapter, &mut d, id, "direct/single-repo").await;
}

/// ADR-0124 D1: 部をまたぐ認可がこの task に記録されている（cross-department）なら従来の planner 経路。
#[tokio::test]
async fn direct_route_fast_path_cross_department_uses_planner() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = atomic_coding_task(&store, root.path(), &["code"]);
    let id = task.id;
    store.insert(&task).unwrap();
    let now = OffsetDateTime::now_utc();
    store
        .approval_append(&task_core::Approval {
            id: task_core::ApprovalId::new(),
            project_id: task.project_id,
            node_id: "software".into(),
            task_id: Some(id),
            question: "cross-department: software -> research: 文献を調べる".into(),
            decision: Some(task_core::Decision::Once),
            answer: None,
            created_at: now,
            decided_at: Some(now),
        })
        .unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::On,
    );
    assert_planned_with_planner(&store, &adapter, &mut d, id, "direct/single-department").await;
}

/// ADR-0124 D1: 担当の profile に無い skill を要する task も cross-department として従来経路。
#[tokio::test]
async fn direct_route_fast_path_foreign_skill_uses_planner() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = atomic_coding_task(&store, root.path(), &["code"]);
    task.skills = vec!["paper-survey".into()];
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::On,
    );
    assert_planned_with_planner(&store, &adapter, &mut d, id, "direct/single-department").await;
}

/// ADR-0124 D1: 人の承認（`Check::Human`）を要する task は従来の planner 経路。
#[tokio::test]
async fn direct_route_fast_path_human_approval_uses_planner() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = atomic_coding_task(&store, root.path(), &["code"]);
    task.acceptance.push(Criterion {
        text: "人が確かめる".into(),
        check: Check::Human,
    });
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::On,
    );
    assert_planned_with_planner(&store, &adapter, &mut d, id, "direct/no-human-approval").await;
}

/// ADR-0124 D1: 人が compound を明示した（`human/explicit`）task は、他の条件を満たしても従来の planner 経路。
#[tokio::test]
async fn direct_route_fast_path_human_explicit_compound_uses_planner() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = atomic_coding_task(&store, root.path(), &["code"]);
    task.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        }),
        ..Default::default()
    });
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::On,
    );
    assert_planned_with_planner(&store, &adapter, &mut d, id, "direct/gate").await;
}

/// ADR-0124 D2: `gate = "shadow"` では経路を記録するだけ（`shadow = true`）で、worker に直行の節を渡さない。
#[tokio::test]
async fn direct_route_fast_path_shadow_records_only() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = atomic_coding_task(&store, root.path(), &["code"]);
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::Shadow,
    );
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Done);
    let decision = recorded_route(&store, id);
    assert_eq!(decision.route, task_core::Route::Direct);
    assert!(decision.shadow);
    assert_eq!(adapter.planner_runs(), 0);
    assert!(
        adapter
            .worker_contexts()
            .iter()
            .all(|c| c.direct_route.is_none())
    );
}

/// ADR-0124 D2: `gate = "off"` では経路を評価しない（記録なし・従来どおり）。
#[tokio::test]
async fn direct_route_fast_path_gate_off_records_nothing() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = atomic_coding_task(&store, root.path(), &["code"]);
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = RouteAdapter::new();
    let mut d = route_dispatcher(
        &store,
        adapter.clone(),
        root.path(),
        task_core::GateMode::Off,
    );
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Done);
    assert!(routed_events(&store, id).is_empty());
    assert!(
        store
            .get(id)
            .unwrap()
            .unwrap()
            .routing
            .and_then(|r| r.route)
            .is_none()
    );
}
