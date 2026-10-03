//! ADR-0140 D1/D2: WU の execute continuation（予算切れ・yield の続き）は、同じ Task・同じ WU・
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
    /// claude の `CLAUDE_CONFIG_DIR` の代わり（`with_config_dir`）。あれば claude と同じく session の jsonl を
    /// `<dir>/projects/<cwd を変換した名前>/<session_id>.jsonl` に書き、`--resume` では読む。
    config_dir: Option<PathBuf>,
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
            config_dir: None,
        }
    }

    fn with_config_dir(mut self, dir: &std::path::Path) -> Self {
        self.config_dir = Some(dir.to_path_buf());
        self
    }

    /// claude を真似た session の jsonl の扱い。新しい session なら 1 行目を書く。`--resume` なら読み、
    /// 無い・1 行でも JSON でない（壊れている）なら claude の拒否の文言を返す（このとき run は失敗する）。
    fn touch_session_file(&self, req: &RunRequest) -> Result<(), String> {
        let (Some(dir), Some(session)) = (&self.config_dir, req.context.session.as_ref()) else {
            return Ok(());
        };
        let path = session_jsonl(dir, req.cwd(), &session.session_id);
        let rejected = || {
            format!(
                "No conversation found with session ID: {}",
                session.session_id
            )
        };
        let line = serde_json::json!({"type": "user", "sessionId": session.session_id});
        if session.resume {
            let body = std::fs::read_to_string(&path).map_err(|_| rejected())?;
            let valid = !body.trim().is_empty()
                && body
                    .lines()
                    .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok());
            if !valid {
                return Err(rejected());
            }
            std::fs::write(&path, format!("{body}{line}\n")).expect("append session jsonl");
        } else {
            std::fs::create_dir_all(path.parent().expect("projects dir")).expect("mkdir");
            std::fs::write(&path, format!("{line}\n")).expect("write session jsonl");
        }
        Ok(())
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
        if let Err(message) = self.touch_session_file(&req) {
            // claude の resume 拒否と同じ: 拒否を報告し、結果なしで終わる（台本は次の run に残す）。
            sink.session_resume_failed(&message);
            return Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: "worker exited without a result message (exit=1)".into(),
                    retryable: true,
                },
                exit_code: Some(1),
            });
        }
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

// ========== ADR-0140 D3: ssh remote workspace の continuation ==========
// LLM（claude）は手元で動き、クラスタにはコマンドと同期だけを出す（ADR-0018/0019）。session jsonl も cwd も
// 手元なので、remote workspace の WU でも同じ account・同じ手元 cwd なら resume する。ssh / rsync は偽の
// コマンド（`true`）に差し替え、外部ネットワークには出ない。

/// remote workspace（クラスタ `sirius`、写しは `workspace_root/<task_id>`）の three-step Task を作り、
/// dispatcher にクラスタと偽の ssh を設定する。
fn remote_three_step(
    dir: &std::path::Path,
    store: &Arc<dyn TaskStore>,
    adapter: Arc<ClaudeScriptAdapter>,
) -> (TaskId, Dispatcher) {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        5,
    );
    task.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: std::path::PathBuf::from("/work/x"),
        mode: None,
    };
    let task_id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(store, task_id);
    let mut d = claude_dispatcher(store, adapter);
    d.config.workspace_root = dir.to_path_buf();
    d.config.clusters.insert(
        "sirius".into(),
        cluster_spec_with_auth("sirius", "sirius", "manual"),
    );
    d.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| true));
    d.set_cluster_ssh_command_override(vec!["true".to_string()]);
    (task_id, d)
}

/// run が remote 経路（`push_remote_after_run`）を通ったことの印。
fn remote_push_lines(store: &Arc<dyn TaskStore>, task_id: TaskId) -> usize {
    store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .filter(|(_, e)| {
            matches!(e, Event::WorkerProgress { msg, .. }
                if msg.starts_with("pushed the workspace to cluster sirius"))
        })
        .count()
}

fn current_session_of(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
    key: &str,
) -> task_core::WorkUnitSession {
    let units = store.work_units_for(task_id).unwrap();
    let wu = units.iter().find(|u| u.key == key).unwrap();
    store
        .work_unit_session_current(task_id, Some(&wu.id))
        .unwrap()
        .expect("a current continuation session")
}

/// 同じ account・同じ手元 cwd の remote WU の continuation（予算切れ → yield → done）は、同じ session id を
/// `resume = true` で受け取る。session 行の cwd はクラスタの path ではなく手元の写し。
#[tokio::test]
async fn session_resume_remote_same_account_and_local_cwd_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        Vec::new(),
        vec![(
            "a",
            vec![(budget_exhausted(), Hook::None), (yielded(), Hook::None)],
        )],
    ));
    let (task_id, mut d) = remote_three_step(dir.path(), &store, adapter.clone());
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(
        store.get(task_id).unwrap().unwrap().status,
        Status::Done,
        "{:?}",
        store.events_for(task_id).unwrap()
    );

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 3, "budget → yield → done");
    assert!(
        remote_push_lines(&store, task_id) >= 3,
        "every run went through the remote workspace"
    );
    let first = session_of(&a[0]);
    assert!(!first.resume);
    for later in &a[1..] {
        let s = session_of(later);
        assert!(s.resume, "a remote continuation resumes the same session");
        assert_eq!(s.session_id, first.session_id);
        assert!(later.continuation.is_some());
    }
    let lines = session_lines(&store, task_id);
    assert_eq!(
        lines
            .iter()
            .filter(|l| **l
                == format!(
                    "continuation session: resumed (session={})",
                    first.session_id
                ))
            .count(),
        2,
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("surface_unsupported")),
        "{lines:?}"
    );
    let stored = current_session_of(&store, task_id, "a");
    assert_eq!(stored.session_id, first.session_id);
    let cwd = stored.cwd.expect("the session row keeps the cwd");
    assert!(
        std::path::Path::new(&cwd).starts_with(dir.path()),
        "the session cwd is the local copy, not the cluster path: {cwd}"
    );
    assert!(!cwd.starts_with("/work/x"), "{cwd}");
}

/// 保存 session の account が今回と違う（別 account に倒れた）remote WU の continuation は resume せず、
/// checkpoint 前置きの新しい session にする。他 account の session id は `--resume` に渡らない。
#[tokio::test]
async fn session_resume_remote_other_account_falls_back_to_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(ClaudeScriptAdapter::new(
        &store,
        Vec::new(),
        vec![(
            "a",
            vec![
                (
                    budget_exhausted(),
                    Hook::RewriteStored {
                        adapter: "claude-code",
                        account: Some("acct-other"),
                    },
                ),
                (yielded(), Hook::None),
            ],
        )],
    ));
    let (task_id, mut d) = remote_three_step(dir.path(), &store, adapter.clone());
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 3, "budget → fresh fallback (yield) → resume");
    assert!(remote_push_lines(&store, task_id) >= 3);
    // a[0] の session は run の途中で別 account（acct-other）の行に書き換わった。
    let other_account_session = session_of(&a[0]).session_id.clone();
    let s2 = session_of(&a[1]);
    assert!(!s2.resume, "another account's session is never resumed");
    assert_ne!(s2.session_id, other_account_session);
    let cont = a[1]
        .continuation
        .as_ref()
        .expect("the fallback carries the checkpoint");
    assert_eq!(cont.previous_end, "budget_exhausted");
    // 次の continuation は fallback で作った（今回の account の）session を resume する。
    let s3 = session_of(&a[2]);
    assert!(s3.resume);
    assert_eq!(s3.session_id, s2.session_id);
    for ctx in &a {
        let s = session_of(ctx);
        assert!(
            !(s.resume && s.session_id == other_account_session),
            "the other account's session id must not reach --resume: {s:?}"
        );
    }
    let lines = session_lines(&store, task_id);
    assert!(
        lines
            .iter()
            .any(|l| l == "continuation session: fresh (reason=account_changed)"),
        "{lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains(&format!("resumed (session={other_account_session})"))),
        "{lines:?}"
    );
    let stored = current_session_of(&store, task_id, "a");
    assert_eq!(stored.session_id, s2.session_id);
    assert_eq!(stored.account_id, None, "the acct-other row was retired");
}

/// remote WU の continuation で resume が拒否された（session が無い。config dir の掃除・daemon restart 後など）
/// ときは、その session を retire し、checkpoint 前置きの新しい session で 1 回やり直す（`resume_rejected`）。
#[tokio::test]
async fn session_resume_remote_rejected_resume_falls_back_to_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let (task_id, mut d) = remote_three_step(dir.path(), &store, adapter.clone());
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);

    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 3, "yield → rejected resume → fresh retry");
    assert!(remote_push_lines(&store, task_id) >= 3);
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
            .any(|l| l == "continuation session: fresh (reason=resume_rejected)"),
        "{lines:?}"
    );
    assert_eq!(
        current_session_of(&store, task_id, "a").session_id,
        s3.session_id
    );
}

// --- ADR-0140 D3: daemon の再起動（同じ DB で Dispatcher を作り直す）後の continuation ---

/// claude の session の置き場所（ADR-0140 D3）: `<config>/projects/<cwd の英数字以外を '-' にした名前>/<id>.jsonl`。
fn session_jsonl(config: &std::path::Path, cwd: &std::path::Path, session_id: &str) -> PathBuf {
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config
        .join("projects")
        .join(project)
        .join(format!("{session_id}.jsonl"))
}

/// 再起動を挟む試験の dispatcher。時計は注入した試験用の時計（`test_now`）。
fn restart_dispatcher(store: &Arc<dyn TaskStore>, adapter: Arc<ClaudeScriptAdapter>) -> Dispatcher {
    let mut d = dispatcher_with_adapter_id(store.clone(), adapter, 1, true, "claude-code");
    d.config.execution.max_continuations_per_work_unit = 10;
    d
}

/// 再起動前の daemon が残したもの（DB の path・Task・WU `a` の session 行・session の jsonl）。
struct BeforeRestart {
    db: PathBuf,
    task_id: TaskId,
    session: task_core::WorkUnitSession,
    jsonl: PathBuf,
}

/// 1 つ目の daemon: file の SQLite で WU `a` の最初の run を走らせ（yield で止まる）、drain
/// （`set_accepting_new_work(false)`、ADR-0040 D4）で手元の run を片付けてから Dispatcher・アダプタ・
/// ストアを drop する。continuation はまだ dispatch しない（`a` は `needs_continuation`、`node_sessions` に行が残る）。
async fn first_daemon_until_yield(
    dir: &std::path::Path,
    config: &std::path::Path,
) -> BeforeRestart {
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let db = state.join("celeris.db");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let task_id = three_step_task(dir, &store);
    let adapter = Arc::new(
        ClaudeScriptAdapter::new(
            &store,
            Vec::new(),
            vec![("a", vec![(yielded(), Hook::None)])],
        )
        .with_config_dir(config),
    );
    let mut d = restart_dispatcher(&store, adapter.clone());
    for _ in 0..50 {
        d.tick().unwrap();
        if !d.running.is_empty() {
            break;
        }
    }
    assert_eq!(d.running.len(), 1, "the first run of a was dispatched");
    d.set_accepting_new_work(false);
    for _ in 0..50 {
        for entry in d.running.values_mut() {
            if !entry.handle.is_finished() {
                (&mut entry.handle).await.expect("worker task panicked");
            }
        }
        d.tick().unwrap();
        if d.in_flight() == 0 {
            break;
        }
    }
    assert_eq!(d.in_flight(), 0, "the draining daemon finished its run");
    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 1, "no continuation before the restart");
    let first = session_of(&a[0]).clone();
    assert!(!first.resume);

    let wu = store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    assert_eq!(
        wu.status,
        task_core::WorkUnitStatus::NeedsContinuation,
        "{wu:?}"
    );
    let session = current_session_of(&store, task_id, "a");
    assert_eq!(session.session_id, first.session_id);
    let cwd = session.cwd.clone().expect("the session row keeps the cwd");
    let jsonl = session_jsonl(config, std::path::Path::new(&cwd), &first.session_id);
    assert!(jsonl.is_file(), "claude wrote {}", jsonl.display());
    drop(d);
    drop(adapter);
    drop(store);
    BeforeRestart {
        db,
        task_id,
        session,
        jsonl,
    }
}

/// 2 つ目の daemon: 同じ DB を開き直し、新しい Dispatcher で残りを最後まで走らせる。注入した時計は
/// 停止していた間（1 時間）だけ進めておく。
async fn second_daemon_to_idle(
    before: &BeforeRestart,
    config: &std::path::Path,
) -> (Arc<dyn TaskStore>, Arc<ClaudeScriptAdapter>) {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&before.db).unwrap());
    let adapter =
        Arc::new(ClaudeScriptAdapter::new(&store, Vec::new(), Vec::new()).with_config_dir(config));
    let mut d = restart_dispatcher(&store, adapter.clone());
    if let Some(clock) = &d.test_now {
        *clock.lock().unwrap() += Duration::from_secs(3600);
    }
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let task = store.get(before.task_id).unwrap().unwrap();
    assert_eq!(
        task.status,
        Status::Done,
        "{:?}",
        store.events_for(before.task_id).unwrap()
    );
    (store, adapter)
}

/// ADR-0140 D4 の `fresh_fallback_by_reason`（`GET /tasks/{id}/execution` の continuation の欄）。
fn fallback_reasons(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
) -> std::collections::BTreeMap<String, u64> {
    let task = store.get(task_id).unwrap().unwrap();
    let events: Vec<Event> = store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    task_core::execution_metrics::summarize(&task, &events)
        .continuation
        .fresh_fallback_by_reason
}

/// 再起動後、session 行は残るが jsonl が無い・壊れているときの共通の確かめ: 保存 id の `--resume` は
/// 1 回だけ試されて拒否され、retire のうえ直近の checkpoint を前置きにした新しい session でやり直し、
/// その run は成功する（拒否された run は attempts に数えない）。
fn assert_restart_fallback(
    store: &Arc<dyn TaskStore>,
    adapter: &ClaudeScriptAdapter,
    before: &BeforeRestart,
    config: &std::path::Path,
) {
    let task_id = before.task_id;
    let stored_id = before.session.session_id.as_str();
    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 2, "rejected resume → fresh retry");
    let (s1, s2) = (session_of(&a[0]), session_of(&a[1]));
    assert!(
        s1.resume && s1.session_id == stored_id,
        "the new daemon resumes the stored session from node_sessions first: {s1:?}"
    );
    assert!(!s2.resume, "the rejected session is not resumed again");
    assert_ne!(s2.session_id, stored_id);
    let cont = a[1]
        .continuation
        .as_ref()
        .expect("the retry carries the checkpoint from before the restart");
    assert_eq!(cont.checkpoint["next_action"], "仕上げに入る");

    let lines = session_lines(store, task_id);
    assert!(
        lines
            .iter()
            .any(|l| *l == format!("continuation session: resumed (session={stored_id})")),
        "{lines:?}"
    );
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
    assert_eq!(
        fallback_reasons(store, task_id).get("resume_rejected"),
        Some(&1),
        "the fallback is counted in the execution metrics"
    );

    let wu = store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    let runs = store.runs_for_work_unit(&wu.id).unwrap();
    let last = runs.iter().max_by_key(|r| r.seq).unwrap();
    assert_eq!(
        last.status,
        task_core::RunIndexStatus::Completed,
        "{runs:?}"
    );
    assert_eq!(last.session_id.as_deref(), Some(s2.session_id.as_str()));
    assert_eq!(
        runs.iter()
            .filter(|r| r.status == task_core::RunIndexStatus::Failed
                || r.status == task_core::RunIndexStatus::HarnessError)
            .count(),
        1,
        "only the rejected resume failed: {runs:?}"
    );

    let current = current_session_of(store, task_id, "a");
    assert_eq!(
        current.session_id, s2.session_id,
        "the stored row was retired"
    );
    assert_eq!(
        current.cwd, before.session.cwd,
        "same worktree after the restart"
    );
    let cwd = current.cwd.clone().unwrap();
    assert!(
        session_jsonl(config, std::path::Path::new(&cwd), &s2.session_id).is_file(),
        "the fresh session has its own jsonl"
    );
}

/// 再起動の間に config dir の jsonl が消えた（掃除等）: `node_sessions` の行だけが残った状態から、
/// resume 拒否として検出して checkpoint fallback の新しい session に倒れる。panic・run 失敗にならない。
#[tokio::test]
async fn session_resume_after_restart_missing_session_file_falls_back_to_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("claude-config");
    let before = first_daemon_until_yield(dir.path(), &config).await;
    std::fs::remove_file(&before.jsonl).unwrap();

    let (store, adapter) = second_daemon_to_idle(&before, &config).await;
    assert_restart_fallback(&store, &adapter, &before, &config);
    assert!(
        !before.jsonl.exists(),
        "celeris does not recreate the stored session's jsonl (ADR-0140 D3)"
    );
}

/// 再起動の間に jsonl の中身が壊れた（途中で切れた・JSON でない行）: 同じく resume 拒否として
/// checkpoint fallback に倒れ、壊れた file は celeris が読まず・書き換えない。
#[tokio::test]
async fn session_resume_after_restart_corrupt_session_file_falls_back_to_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("claude-config");
    let before = first_daemon_until_yield(dir.path(), &config).await;
    let corrupt = "{\"type\":\"user\",\"sessionId\":\n\u{0}\u{0}garbage";
    std::fs::write(&before.jsonl, corrupt).unwrap();

    let (store, adapter) = second_daemon_to_idle(&before, &config).await;
    assert_restart_fallback(&store, &adapter, &before, &config);
    assert_eq!(
        std::fs::read_to_string(&before.jsonl).unwrap(),
        corrupt,
        "celeris neither reads nor rewrites the jsonl (ADR-0140 D3)"
    );
}

/// 再起動の間も jsonl が残り、account・cwd が同じなら、新しい daemon は `node_sessions` の行から
/// 同じ session id を `--resume` する（fallback しない）。
#[tokio::test]
async fn session_resume_after_restart_kept_session_file_resumes_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("claude-config");
    let before = first_daemon_until_yield(dir.path(), &config).await;

    let (store, adapter) = second_daemon_to_idle(&before, &config).await;
    let task_id = before.task_id;
    let stored_id = before.session.session_id.as_str();
    let a = adapter.runs_of("a");
    assert_eq!(a.len(), 1, "the continuation finished in one resumed run");
    let s = session_of(&a[0]);
    assert!(s.resume, "{s:?}");
    assert_eq!(s.session_id, stored_id);
    assert!(
        a[0].continuation.is_some(),
        "the checkpoint stays in the preamble"
    );

    let current = current_session_of(&store, task_id, "a");
    assert_eq!(
        current.id, before.session.id,
        "the same row, not a new session"
    );
    assert_eq!(current.account_id, before.session.account_id);
    assert_eq!(current.cwd, before.session.cwd);
    let lines = session_lines(&store, task_id);
    assert!(
        lines
            .iter()
            .any(|l| *l == format!("continuation session: resumed (session={stored_id})")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("resume rejected")),
        "{lines:?}"
    );
    assert!(fallback_reasons(&store, task_id).is_empty());
    let body = std::fs::read_to_string(&before.jsonl).unwrap();
    assert_eq!(
        body.lines().count(),
        2,
        "claude appended to the same session"
    );
}
