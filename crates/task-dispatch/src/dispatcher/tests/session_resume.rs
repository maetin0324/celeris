//! ADR-0124 D1/D2: WU の execute continuation（予算切れ・yield の続き）は、同じ Task・同じ WU・
//! `claude-code`・同じ account/provider なら保存した session を `--resume` し、条件が崩れたら
//! checkpoint 前置きの新しい session に倒す。planner・別 WU は fresh のまま。偽のアダプタ（id は
//! `claude-code`）と in-memory のストアだけで、実 claude・外部ネットワークには出ない。

use std::collections::VecDeque;

use super::*;

/// run の途中で偽アダプタがすること。
#[derive(Debug, Clone, Copy)]
enum Hook {
    None,
    /// `--resume` を頼まれていたら、claude-code の resume 拒否として `session_resume_failed` を報告する。
    RejectResume,
    /// 保存 session を別アダプタ・別 account の行に置き換える（設定変更・プールの付け替えの再現）。
    RewriteStored {
        adapter: &'static str,
        account: Option<&'static str>,
    },
}

struct ClaudeScriptAdapter {
    store: Arc<dyn TaskStore>,
    plans: StdMutex<VecDeque<String>>,
    script: StdMutex<HashMap<String, VecDeque<(Terminal, Hook)>>>,
    /// `(WU の key、planner なら "planner"、RunContext)`。
    seen: StdMutex<Vec<(String, task_worker::RunContext)>>,
}

impl ClaudeScriptAdapter {
    fn new(
        store: &Arc<dyn TaskStore>,
        plans: Vec<String>,
        script: Vec<(&str, Vec<(Terminal, Hook)>)>,
    ) -> Self {
        ClaudeScriptAdapter {
            store: store.clone(),
            plans: StdMutex::new(plans.into_iter().collect()),
            script: StdMutex::new(
                script
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.into_iter().collect()))
                    .collect(),
            ),
            seen: StdMutex::new(Vec::new()),
        }
    }

    fn runs_of(&self, key: &str) -> Vec<task_worker::RunContext> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, c)| c.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for ClaudeScriptAdapter {
    fn id(&self) -> &str {
        "claude-code"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        if req.context.execution_planner.is_some() {
            self.seen
                .lock()
                .unwrap()
                .push(("planner".to_string(), req.context.clone()));
            if let Some(json) = self.plans.lock().unwrap().pop_front() {
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json)
                    .expect("write execution-plan.json");
            }
            return Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: "planned".into(),
                    evidence: vec![],
                    usage: None,
                },
                exit_code: Some(0),
            });
        }
        let key = req
            .context
            .work_unit
            .as_ref()
            .map(|w| w.key.clone())
            .unwrap_or_else(|| "atomic".to_string());
        self.seen
            .lock()
            .unwrap()
            .push((key.clone(), req.context.clone()));
        let (terminal, hook) = self
            .script
            .lock()
            .unwrap()
            .get_mut(&key)
            .and_then(|q| q.pop_front())
            .unwrap_or((
                Terminal::Done {
                    summary: "ok".into(),
                    evidence: vec![],
                    usage: None,
                },
                Hook::None,
            ));
        match hook {
            Hook::None => {}
            Hook::RejectResume => {
                if req.context.session.as_ref().is_some_and(|s| s.resume) {
                    sink.session_resume_failed(
                        "No conversation found with session ID: 550e8400-e29b-41d4-a716-446655440000",
                    );
                }
            }
            Hook::RewriteStored { adapter, account } => {
                let wu = self
                    .store
                    .work_units_for(req.task.id)
                    .unwrap()
                    .into_iter()
                    .find(|u| u.key == key)
                    .expect("work unit of this run");
                let current = self
                    .store
                    .work_unit_session_current(req.task.id, Some(&wu.id))
                    .unwrap()
                    .expect("the dispatcher created a continuation session for this run");
                self.store
                    .work_unit_session_retire(req.task.id, Some(&wu.id), OffsetDateTime::now_utc())
                    .unwrap();
                let rewritten = task_core::WorkUnitSession {
                    id: ulid::Ulid::new().to_string(),
                    adapter: adapter.to_string(),
                    account_id: account.map(str::to_string),
                    ..current
                };
                self.store.work_unit_session_create(&rewritten).unwrap();
            }
        }
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

fn budget_exhausted() -> Terminal {
    Terminal::BudgetExhausted {
        kind: task_core::BudgetKind::Turns,
        message: "max turns".into(),
        usage: None,
    }
}

fn yielded() -> Terminal {
    Terminal::Yielded {
        checkpoint: serde_json::json!({
            "completed": ["A の下ごしらえ"],
            "remaining": ["A の仕上げ"],
            "next_action": "仕上げに入る",
        }),
        usage: None,
    }
}

fn claude_dispatcher(store: &Arc<dyn TaskStore>, adapter: Arc<ClaudeScriptAdapter>) -> Dispatcher {
    let mut d = dispatcher_with_adapter_id(store.clone(), adapter, 1, false, "claude-code");
    d.config.execution.max_continuations_per_work_unit = 10;
    d
}

fn three_step_task(dir: &std::path::Path, store: &Arc<dyn TaskStore>) -> TaskId {
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
    adopt_three_step_plan(store, task_id);
    task_id
}

/// run の進行に残った continuation の session の判断の行（古い順）。
fn session_lines(store: &Arc<dyn TaskStore>, task_id: TaskId) -> Vec<String> {
    store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } if msg.starts_with("continuation session") => {
                Some(msg)
            }
            _ => None,
        })
        .collect()
}

fn session_of(ctx: &task_worker::RunContext) -> &task_worker::protocol::SessionHandle {
    ctx.session
        .as_ref()
        .expect("a claude-code work unit run carries a continuation session")
}

/// 同じ WU の予算切れ・yield の続きは、同じ session id を `resume = true` で受け取る（`--resume`）。
/// 別 WU（b・c）の最初の run は新しい session（`resume = false`、別の id）。`runs.session_id` にも残る。
#[tokio::test]
async fn session_resume_session_reuse_same_work_unit_continuation_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task_id = three_step_task(dir.path(), &store);
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        Vec::new(),
        vec![(
            "a",
            vec![(budget_exhausted(), Hook::None), (yielded(), Hook::None)],
        )],
    ));
    let mut d = claude_dispatcher(&store, adapter.clone());
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 3, "budget → yield → done");
    let first = session_of(&a[0]);
    assert!(
        !first.resume,
        "the first run of a work unit starts a session"
    );
    assert!(task_worker::provider::is_valid_uuid(&first.session_id));
    for later in &a[1..] {
        let s = session_of(later);
        assert!(s.resume, "continuations resume the same session");
        assert_eq!(s.session_id, first.session_id);
        assert!(
            later.continuation.is_some(),
            "the checkpoint stays in the preamble"
        );
    }
    let b = adapter.runs_of("b");
    assert_eq!(b.len(), 1);
    let sb = session_of(&b[0]);
    assert!(!sb.resume, "an independent work unit starts fresh");
    assert_ne!(sb.session_id, first.session_id);

    let lines = session_lines(&store, task_id);
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("continuation session: resumed"))
            .count(),
        2,
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "continuation session: fresh (reason=independent_wu)"),
        "{lines:?}"
    );
    let runs = store.runs_for_task(task_id).unwrap();
    assert!(
        runs.iter()
            .filter(|r| r.session_id.as_deref() == Some(first.session_id.as_str()))
            .count()
            == 3,
        "{runs:?}"
    );
}

/// 保存 session の account が今回の account と違う（プールが別 account に倒れた）なら resume せず、
/// checkpoint 前置きの新しい session にする（別 account の session を `--resume` しない）。
#[tokio::test]
async fn session_resume_fresh_session_on_account_change() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task_id = three_step_task(dir.path(), &store);
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        Vec::new(),
        vec![(
            "a",
            vec![(
                yielded(),
                Hook::RewriteStored {
                    adapter: "claude-code",
                    account: Some("acct-other"),
                },
            )],
        )],
    ));
    let mut d = claude_dispatcher(&store, adapter.clone());
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 2);
    let (s1, s2) = (session_of(&a[0]), session_of(&a[1]));
    assert!(!s2.resume, "another account's session is never resumed");
    assert_ne!(s2.session_id, s1.session_id);
    assert!(
        a[1].continuation.is_some(),
        "fallback carries the checkpoint"
    );
    let lines = session_lines(&store, task_id);
    assert!(
        lines
            .iter()
            .any(|l| l == "continuation session: fresh (reason=account_changed)"),
        "{lines:?}"
    );
}

/// 保存 session のアダプタが違う（設定変更）なら retire して checkpoint 前置きの新しい session。
#[tokio::test]
async fn session_resume_session_reuse_adapter_change_falls_back_to_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task_id = three_step_task(dir.path(), &store);
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        Vec::new(),
        vec![(
            "a",
            vec![(
                budget_exhausted(),
                Hook::RewriteStored {
                    adapter: "codex",
                    account: None,
                },
            )],
        )],
    ));
    let mut d = claude_dispatcher(&store, adapter.clone());
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 2);
    let s2 = session_of(&a[1]);
    assert!(!s2.resume);
    assert_ne!(s2.session_id, session_of(&a[0]).session_id);
    assert!(a[1].continuation.is_some());
    let lines = session_lines(&store, task_id);
    assert!(
        lines
            .iter()
            .any(|l| l == "continuation session: fresh (reason=adapter_changed)"),
        "{lines:?}"
    );
    let units = store.work_units_for(task_id).unwrap();
    let wu_a = units.iter().find(|u| u.key == "a").unwrap();
    let current = store
        .work_unit_session_current(task_id, Some(&wu_a.id))
        .unwrap()
        .unwrap();
    assert_eq!(current.adapter, "claude-code", "the codex row was retired");
    assert_eq!(current.session_id, s2.session_id);
}

/// resume が拒否された（session が無い）run の後は、その session を retire し、checkpoint 前置きの新しい
/// session でやり直す（理由 `resume_rejected`）。
#[tokio::test]
async fn session_resume_session_reuse_rejected_resume_falls_back_to_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task_id = three_step_task(dir.path(), &store);
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        Vec::new(),
        vec![(
            "a",
            vec![
                (yielded(), Hook::None),
                (
                    Terminal::Error {
                        message: "worker exited without a result message (exit=1)".into(),
                        retryable: true,
                    },
                    Hook::RejectResume,
                ),
            ],
        )],
    ));
    let mut d = claude_dispatcher(&store, adapter.clone());
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 3, "yield → rejected resume → fresh retry");
    let (s1, s2, s3) = (session_of(&a[0]), session_of(&a[1]), session_of(&a[2]));
    assert!(s2.resume && s2.session_id == s1.session_id);
    assert!(!s3.resume, "the rejected session is not resumed again");
    assert_ne!(s3.session_id, s1.session_id);
    let cont = a[2]
        .continuation
        .as_ref()
        .expect("the retry carries the latest checkpoint");
    assert_eq!(cont.checkpoint["next_action"], "仕上げに入る");
    let lines = session_lines(&store, task_id);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("continuation session resume rejected")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "continuation session: fresh (reason=resume_rejected)"),
        "{lines:?}"
    );
}

/// `[sessions] continuation_resume = false`（明示的な fresh 要求）では、continuation も session を持たず
/// checkpoint 前置きの fresh のまま（導入前の挙動）。
#[tokio::test]
async fn session_resume_session_reuse_fresh_request_keeps_the_checkpoint_only() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task_id = three_step_task(dir.path(), &store);
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        Vec::new(),
        vec![("a", vec![(yielded(), Hook::None)])],
    ));
    let mut d = claude_dispatcher(&store, adapter.clone());
    d.config.execution.continuation_session_resume = false;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 2);
    assert!(
        !a[1].session.as_ref().is_some_and(|s| s.resume),
        "{:?}",
        a[1].session
    );
    assert!(
        a[1].session.is_none(),
        "no session is started when fresh is requested"
    );
    assert!(a[1].continuation.is_some());
    let lines = session_lines(&store, task_id);
    assert!(
        lines
            .iter()
            .any(|l| l == "continuation session: fresh (reason=fresh_requested)"),
        "{lines:?}"
    );
}

/// planner run は session を持たず（`--no-session-persistence` のまま）、計画の leaf の continuation だけが
/// resume する。別 leaf（b）の最初の run は新しい session。
#[tokio::test]
async fn session_resume_fresh_session_for_planner_and_independent_work_unit() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();
    let plan = plan_json(vec![wu_spec("a", &[]), wu_spec("b", &["a"])]);
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        vec![plan],
        vec![("a", vec![(budget_exhausted(), Hook::None)])],
    ));
    let mut d = claude_dispatcher(&store, adapter.clone());
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "claude-code".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);

    let planners = adapter.runs_of("planner");
    assert!(!planners.is_empty());
    for p in &planners {
        assert!(
            p.session.is_none(),
            "planner runs stay fresh: {:?}",
            p.session
        );
    }
    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 2);
    assert!(session_of(&a[1]).resume);
    let b = adapter.runs_of("b");
    assert_eq!(b.len(), 1);
    assert!(!session_of(&b[0]).resume);
    assert_ne!(session_of(&b[0]).session_id, session_of(&a[0]).session_id);
    // planner の run には判断の行を足さない（WU の run の 3 本分だけ）。
    assert_eq!(session_lines(&store, task_id).len(), 3);
}
