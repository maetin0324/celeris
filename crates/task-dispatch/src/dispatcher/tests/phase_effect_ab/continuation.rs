//! scenario continuation: implement WU が予算切れ → yield → 完了の 3 run を、
//! `continuation_session_resume = false`（off: 続きも毎回新しい session）と `true`（on: 同じ session を
//! `--resume`）で走らせる。偽アダプタ（id は `claude-code`）は新しい session なら全文脈の、resume なら
//! 差分の固定入力 token を返し、注入した時計（`SimClock`）を固定秒だけ進める。

use std::collections::VecDeque;
use std::sync::atomic::AtomicU64;

use super::super::*;
use super::{AbMetric, Variant, assert_on_reduces_runs_or_fresh_sessions, print_ab_metric};

const SCENARIO: &str = "continuation";
/// 新しい session が読み直す全文脈の入力 token。
const FULL_CONTEXT_TOKENS: u64 = 40_000;
/// resume した session に足される差分の入力 token。
const RESUME_DELTA_TOKENS: u64 = 4_000;
/// 新しい session の 1 run の壁時計（文脈の読み直しを含む）。
const FRESH_RUN_SECS: u64 = 120;
/// resume した 1 run の壁時計。
const RESUME_RUN_SECS: u64 = 80;

/// 偽アダプタが進める試験時計（秒）。実時間には依らない。
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

/// implement WU の run に台本の終端を順に返し、session の新旧で入力 token と時計を進める。
struct TokenScriptAdapter {
    script: StdMutex<VecDeque<Terminal>>,
    clock: SimClock,
    metric: StdMutex<AbMetric>,
}

impl TokenScriptAdapter {
    fn new(script: Vec<Terminal>) -> Self {
        TokenScriptAdapter {
            script: StdMutex::new(script.into_iter().collect()),
            clock: SimClock::default(),
            metric: StdMutex::new(AbMetric::default()),
        }
    }
}

#[async_trait]
impl WorkerAdapter for TokenScriptAdapter {
    fn id(&self) -> &str {
        "claude-code"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        assert!(
            req.context.execution_planner.is_none(),
            "the scenario adopts a fixture plan; no planner run is expected"
        );
        let resumed = req.context.session.as_ref().is_some_and(|s| s.resume);
        let (tokens, secs) = if resumed {
            (RESUME_DELTA_TOKENS, RESUME_RUN_SECS)
        } else {
            (FULL_CONTEXT_TOKENS, FRESH_RUN_SECS)
        };
        self.clock.advance(secs);
        {
            let mut m = self.metric.lock().unwrap();
            m.runs += 1;
            m.input_tokens += tokens;
            if !resumed {
                m.fresh_sessions += 1;
            }
            m.wall_secs = self.clock.now_secs();
        }
        let terminal = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            });
        if matches!(terminal, Terminal::Done { .. }) {
            std::fs::write(
                req.artifacts_dir.join("result.json"),
                serde_json::json!({"summary": "ok", "evidence": []}).to_string(),
            )
            .expect("write result.json");
        }
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

/// implement WU 1 枚だけの計画を採用した task。
fn implement_only_task(dir: &std::path::Path, store: &Arc<dyn TaskStore>) -> TaskId {
    let task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        5,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "implement only".to_string(),
        work_units: vec![wu_spec("implement", &[])],
        phases: Vec::new(),
        children: Vec::new(),
    };
    task_ops::execution::adopt_plan(
        store.as_ref(),
        task_id,
        spec,
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("adopt plan");
    task_id
}

async fn run_variant(variant: Variant) -> AbMetric {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task_id = implement_only_task(dir.path(), &store);
    let adapter = Arc::new(TokenScriptAdapter::new(vec![
        Terminal::BudgetExhausted {
            kind: task_core::BudgetKind::Turns,
            message: "max turns".into(),
            usage: None,
        },
        Terminal::Yielded {
            checkpoint: serde_json::json!({
                "completed": ["下ごしらえ"],
                "remaining": ["仕上げ"],
                "next_action": "仕上げに入る",
            }),
            usage: None,
        },
    ]));
    let mut d = dispatcher_with_adapter_id(store.clone(), adapter.clone(), 1, false, "claude-code");
    d.config.execution.max_continuations_per_work_unit = 10;
    d.config.execution.continuation_session_resume = variant == Variant::On;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);
    let metric = *adapter.metric.lock().unwrap();
    assert_eq!(metric.runs, 3, "budget → yield → done in {variant:?}");
    print_ab_metric(SCENARIO, variant, &metric);
    metric
}

/// off は 3 run とも新しい session（全文脈を毎回読む）、on は最初の 1 run だけが新しい session。
#[tokio::test]
async fn phase_effect_ab_continuation_resume_reduces_fresh_sessions() {
    let off = run_variant(Variant::Off).await;
    let on = run_variant(Variant::On).await;
    assert_eq!(off.fresh_sessions, 3, "{off:?}");
    assert_eq!(on.fresh_sessions, 1, "{on:?}");
    assert!(on.fresh_sessions < off.fresh_sessions);
    assert!(on.input_tokens < off.input_tokens, "off={off:?} on={on:?}");
    assert!(on.wall_secs < off.wall_secs, "off={off:?} on={on:?}");
    assert_on_reduces_runs_or_fresh_sessions(SCENARIO, &off, &on);
}
