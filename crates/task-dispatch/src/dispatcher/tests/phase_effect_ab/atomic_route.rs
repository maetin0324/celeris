//! 単一 repo の coding task を明示 compound の旧経路と direct route で完了まで走らせる。
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::*;
use super::{AbMetric, Variant, print_ab_metric};

const SCENARIO: &str = "atomic_route";
const FULL_CONTEXT_TOKENS: u64 = 40_000;
const RESUME_DELTA_TOKENS: u64 = 4_000;
const FRESH_RUN_SECS: u64 = 120;
const RESUME_RUN_SECS: u64 = 80;

/// アダプタが進める注入時計。
#[derive(Default)]
struct SimClock(AtomicU64);

impl SimClock {
    fn advance(&self, secs: u64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
    fn now_secs(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct AtomicRouteAdapter {
    clock: SimClock,
    metric: StdMutex<AbMetric>,
}

impl AtomicRouteAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            clock: SimClock::default(),
            metric: StdMutex::new(AbMetric::default()),
        })
    }
}

#[async_trait]
impl WorkerAdapter for AtomicRouteAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).expect("create artifacts");
        let resumed = req.context.session.as_ref().is_some_and(|s| s.resume);
        let (tokens, secs) = if resumed {
            (RESUME_DELTA_TOKENS, RESUME_RUN_SECS)
        } else {
            (FULL_CONTEXT_TOKENS, FRESH_RUN_SECS)
        };
        self.clock.advance(secs);
        {
            let mut metric = self.metric.lock().unwrap();
            metric.runs += 1;
            metric.wall_secs = self.clock.now_secs();
            metric.input_tokens += tokens;
            metric.fresh_sessions += u64::from(!resumed);
        }
        if req.context.execution_planner.is_some() {
            std::fs::write(
                req.artifacts_dir.join("execution-plan.json"),
                plan_json(vec![wu_spec("implement", &[])]),
            )
            .expect("write execution plan");
        } else if req.task.kind == TaskKind::Review {
            let verdicts: Vec<_> = (0..req.task.acceptance.len().max(4))
                .map(|i| format!(r#"{{"criterion":{i},"pass":true,"reason":"ok"}}"#))
                .collect();
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                format!(r#"{{"verdicts":[{}]}}"#, verdicts.join(",")),
            )
            .expect("write review");
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: Some(task_core::Usage {
                    input_tokens: Some(tokens),
                    ..Default::default()
                }),
            },
            exit_code: Some(0),
        })
    }
}

fn atomic_coding_task(store: &Arc<dyn TaskStore>, root: &std::path::Path) -> Task {
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
    let repo = root.join("code");
    std::fs::create_dir_all(&repo).unwrap();
    let (project_id, repos) =
        project_with_repos(store, &[("code", &repo, task_core::RepoKind::Dir)]);
    let mut task = new_task(
        &repo,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.acceptance.push(Criterion {
        text: "review the change".into(),
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

async fn run_variant(variant: Variant) -> AbMetric {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = atomic_coding_task(&store, root.path());
    if variant == Variant::Off {
        // human/explicit compound では direct_route::evaluate の direct/gate が false。
        task.routing.as_mut().unwrap().execution_hint = Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        });
    }
    let task_id = task.id;
    store.insert(&task).unwrap();
    let adapter = AtomicRouteAdapter::new();
    let mut dispatcher = worktree_dispatcher(
        store.clone(),
        adapter.clone(),
        &root.path().join("ws"),
        None,
    );
    dispatcher.config.execution.gate = task_core::GateMode::On;
    dispatcher.config.execution.planner.adapter = "instant".into();
    let report = run_until_idle(&mut dispatcher, 400).await;
    assert!(report.idle, "{variant:?}: {report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);
    let decision = store
        .get(task_id)
        .unwrap()
        .unwrap()
        .routing
        .unwrap()
        .route
        .expect("route decision");
    let expected_route = if variant == Variant::Off {
        task_core::Route::Planned
    } else {
        task_core::Route::Direct
    };
    assert_eq!(decision.route, expected_route);
    let runs = store.runs_for_task(task_id).unwrap();
    let planner_runs = runs
        .iter()
        .filter(|run| run.role == task_core::RunIndexRole::Planner)
        .count();
    assert_eq!(planner_runs, usize::from(variant == Variant::Off));
    assert_eq!(store.work_units_for(task_id).unwrap().len(), planner_runs);
    let metric = *adapter.metric.lock().unwrap();
    assert_eq!(metric.runs, runs.len() as u64);
    assert_eq!(metric.wall_secs, metric.runs * FRESH_RUN_SECS);
    assert_eq!(metric.input_tokens, metric.runs * FULL_CONTEXT_TOKENS);
    assert_eq!(metric.fresh_sessions, metric.runs);
    print_ab_metric(SCENARIO, variant, &metric);
    metric
}

#[tokio::test]
async fn phase_effect_ab_atomic_route_reduces_runs() {
    let off = run_variant(Variant::Off).await;
    let on = run_variant(Variant::On).await;
    assert!(on.runs < off.runs, "off={off:?}, on={on:?}");
    assert!(on.wall_secs < off.wall_secs);
    assert!(on.input_tokens < off.input_tokens);
    assert!(on.fresh_sessions < off.fresh_sessions);
}
