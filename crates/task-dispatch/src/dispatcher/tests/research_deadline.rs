//! ADR 2026-11-15-research-adapter-deadline: checkpoint から続けられない adapter（`WorkerAdapter::continues_from_checkpoint`
//! が `false`）の run は、予算切れ・yield で終わっても continuation の経路に乗らない。atomic task と WU の両経路で、
//! 文言の `Error`・`BudgetExhausted{WallClock}`・`Yielded` の 3 つの終わり方を見る。続けられる adapter の同じ run が
//! continuation に回ることも同じ試験で確かめ、試験が性質の違いだけで結果を分けていることを示す。
//! LLM・外部ネットワーク・sleep は使わない（終端を返すだけの偽アダプタ）。

use super::*;

/// 同じ終端を毎回返す偽アダプタ。`continues` が性質の値。
struct EndsWith {
    terminal: Terminal,
    continues: bool,
}

#[async_trait]
impl WorkerAdapter for EndsWith {
    fn id(&self) -> &str {
        "instant"
    }
    fn continues_from_checkpoint(&self) -> bool {
        self.continues
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        Ok(RunOutcome {
            terminal: self.terminal.clone(),
            exit_code: Some(0),
        })
    }
}

/// 3 つの終わり方と、settle が付ける `RunEnd`。
fn ends() -> Vec<(Terminal, task_core::RunEnd)> {
    let wall_clock = task_core::RunEnd::BudgetExhausted {
        kind: task_core::BudgetKind::WallClock,
    };
    vec![
        (
            Terminal::Error {
                message: "wall clock exceeded".into(),
                retryable: true,
                usage: None,
            },
            wall_clock,
        ),
        (
            Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::WallClock,
                message: "wall clock exceeded".into(),
                usage: None,
            },
            wall_clock,
        ),
        (
            Terminal::Yielded {
                checkpoint: serde_json::json!({
                    "completed": ["step"],
                    "remaining": ["more"],
                    "next_action": "続ける",
                }),
                usage: None,
            },
            task_core::RunEnd::Yielded,
        ),
    ]
}

fn wu_plan(store: &Arc<SqliteStore>, task_id: TaskId) {
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "research only".to_string(),
        work_units: vec![wu_spec("research", &[])],
        phases: Vec::new(),
        children: Vec::new(),
        design_brief: None,
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
}

/// 1 つの task を `[execution] continuation = true` で動かし、events を返す。
async fn run_task(work_unit: bool, terminal: Terminal, continues: bool) -> Vec<(u64, Event)> {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    if work_unit {
        wu_plan(&store, task_id);
    }
    let adapter = Arc::new(EndsWith {
        terminal,
        continues,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.execution.continuation = true;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    store.events_for(task_id).unwrap()
}

fn continued(events: &[(u64, Event)]) -> bool {
    events
        .iter()
        .any(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "continue"))
}

fn saved_checkpoint(events: &[(u64, Event)]) -> bool {
    events
        .iter()
        .any(|(_, e)| matches!(e, Event::CheckpointSaved { .. }))
}

/// WU が continuation 待ち（`needs_continuation`）に移ったか。WU の retry（`work_unit_retry`）は `ready` に戻る。
fn needs_continuation(events: &[(u64, Event)]) -> bool {
    events.iter().any(|(_, e)| {
        matches!(
            e,
            Event::WorkUnitTransitioned {
                to: task_core::WorkUnitStatus::NeedsContinuation,
                ..
            }
        )
    })
}

fn worker_ends(events: &[(u64, Event)]) -> Vec<(Option<task_core::RunEnd>, String)> {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                role: None,
                end,
                outcome,
                ..
            } => Some((*end, outcome.clone())),
            _ => None,
        })
        .collect()
}

/// 不変条件: 続けられない adapter の run は、3 つの終わり方のどれでも continuation の trigger
/// （`Trigger::Continue{why: Continue}`、reason `continue`）にならず、WU を `needs_continuation` にせず、`CheckpointSaved` を
/// 残さず、`WorkerFinished.end` は予算切れ・yield のまま。atomic task は continuation の使えない枝
/// （`(continuation disabled)`）の `WorkerError{retryable:true}`、WU は WU の retry（`Continue{why: WorkUnitRetry}`、
/// reason `work_unit_retry`。WU の retryable な失敗の表し方）になる。続けられる adapter の同じ run は continuation に
/// 回る。既存の continuation の試験は adapter の性質を持たない偽アダプタだけを使い、研究 harness の run が
/// continuation に回ること（最初からのやり直しを続きと数えること）を検出できなかった。
async fn assert_path(work_unit: bool) {
    for (terminal, end) in ends() {
        let events = run_task(work_unit, terminal.clone(), false).await;
        let label = format!("work_unit={work_unit} end={end:?}");
        assert!(!continued(&events), "{label}: {events:?}");
        assert!(!saved_checkpoint(&events), "{label}: {events:?}");
        assert!(!needs_continuation(&events), "{label}: {events:?}");
        let finished = worker_ends(&events);
        assert!(!finished.is_empty(), "{label}: {events:?}");
        for (got, outcome) in &finished {
            assert_eq!(*got, Some(end), "{label}: {outcome}");
            // atomic task は continuation の使えない枝の文。WU は WU の retry（最初からやり直し）、retry を使い切った後は
            // WU の失敗の経路（自動 replan など）で、どちらも continuation の文（`continue: `）にならない。
            if work_unit {
                assert!(!outcome.starts_with("continue: "), "{label}: {outcome}");
            } else {
                assert!(
                    outcome.ends_with("(continuation disabled)"),
                    "{label}: {outcome}"
                );
            }
        }

        let events = run_task(work_unit, terminal, true).await;
        assert!(continued(&events), "{label} (continuing): {events:?}");
        assert!(
            saved_checkpoint(&events),
            "{label} (continuing): {events:?}"
        );
    }
}

#[tokio::test]
async fn research_deadline_atomic_run_neither_continues_nor_saves_a_checkpoint() {
    assert_path(false).await;
}

#[tokio::test]
async fn research_deadline_work_unit_run_neither_continues_nor_saves_a_checkpoint() {
    assert_path(true).await;
}

/// 不変条件: 継続 session を持てる adapter（`sessions::SUPPORTED_ADAPTERS`）は checkpoint から続けられる adapter の
/// 部分集合で、研究 harness の 3 本は続けられない。両方の表は別の性質（session の resume と checkpoint からの続き）で、
/// どちらも名前の一覧を足さずに adapter から引く。既存の試験は 2 つの性質の関係を見ていなかった。
#[test]
fn research_deadline_session_adapters_continue_and_research_adapters_do_not() {
    use task_worker::{
        AcpAdapter, AcpConfig, ClaudeCodeAdapter, ClaudeCodeConfig, CodexAdapter, CodexConfig,
        LangMemAdapter, LangMemConfig, LdrAdapter, LdrConfig, PaperQaAdapter, PaperQaConfig,
    };
    let adapters: Vec<Arc<dyn WorkerAdapter>> = vec![
        Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig::default())),
        Arc::new(CodexAdapter::new(CodexConfig::default())),
        Arc::new(AcpAdapter::new(AcpConfig::default())),
        Arc::new(LangMemAdapter::new(LangMemConfig::default())),
        Arc::new(LdrAdapter::new(LdrConfig::default())),
        Arc::new(PaperQaAdapter::new(PaperQaConfig::default())),
    ];
    let by_id = |id: &str| {
        adapters
            .iter()
            .find(|a| a.id() == id)
            .unwrap_or_else(|| panic!("no adapter {id}"))
            .continues_from_checkpoint()
    };
    for id in crate::sessions::SUPPORTED_ADAPTERS {
        assert!(by_id(id), "{id}");
    }
    for id in [LangMemAdapter::ID, LdrAdapter::ID, PaperQaAdapter::ID] {
        assert!(!by_id(id), "{id}");
        assert!(!crate::sessions::adapter_supports_sessions(id), "{id}");
    }
}
