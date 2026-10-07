use super::*;

use crate::policy::{ProviderSpec, StaticPolicy};

use async_trait::async_trait;

use std::sync::atomic::{AtomicUsize, Ordering};

use task_core::*;

use task_worker::Evidence;

/// 同プロセスで即座に終端を返すテスト用アダプタ（サブプロセスは起動しない）。
struct InstantAdapter {
    terminal: Terminal,
    delay: Duration,
}

#[async_trait]
impl WorkerAdapter for InstantAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        sink.progress("working");
        std::fs::write(req.workspace.join("touched"), "1").unwrap();
        tokio::time::sleep(self.delay).await;
        Ok(RunOutcome {
            terminal: self.terminal.clone(),
            exit_code: Some(0),
        })
    }
}

fn new_task(dir: &std::path::Path, check: Check, max_retries: u32) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: dir.to_path_buf(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 30,
            max_retries,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn dispatcher(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    max_concurrency: usize,
) -> Dispatcher {
    dispatcher_with_test_clock(store, adapter, max_concurrency, false)
}

fn dispatcher_with_test_clock(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    max_concurrency: usize,
    use_test_clock: bool,
) -> Dispatcher {
    dispatcher_with_adapter_id(store, adapter, max_concurrency, use_test_clock, "instant")
}

/// `dispatcher_with_test_clock` と同じだが、provider `p1` のアダプタ id を選べる
/// （ADR-0140: continuation の resume は `claude-code` だけが対象）。
fn dispatcher_with_adapter_id(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    max_concurrency: usize,
    use_test_clock: bool,
    adapter_id: &str,
) -> Dispatcher {
    let mut policy = StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            adapter: adapter_id.into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: max_concurrency,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    );
    let policy_clock = Arc::new(StdMutex::new(Instant::now()));
    if use_test_clock {
        policy.set_test_clock(policy_clock.clone());
    }
    let mut adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>> = HashMap::new();
    adapters.insert("p1".into(), adapter);
    let mut dispatcher = Dispatcher::new(
        store,
        Box::new(policy),
        HashMap::from([("p1".to_string(), "m".to_string())]),
        adapters,
        std::collections::HashSet::new(),
        DispatchConfig {
            delivery: Default::default(),
            max_concurrency,
            lease_grace: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
            review_timeout: Duration::from_secs(5),
            workspace_root: PathBuf::from("/nonexistent"),
            plan_auto_accept: false,
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            reviewer_hint: crate::review::reviewer_hint(),
            reviewer_tier_override: None,
            clusters: HashMap::new(),
            cluster_cooldown: Duration::from_secs(1),
            max_requeues: 5,
            max_reviewer_retries: 3,
            max_infra_retries: 5,
            min_free_disk_mb: 5120,
            roles: Vec::new(),
            genres: Vec::new(),
            delegation: DelegationLimits::default(),
            accounts: None,
            memory_dir: None,
            worktree_branch_prefix: task_worker::DEFAULT_BRANCH_PREFIX.to_string(),
            releases_dir: None,
            containers: ContainersRuntimeConfig::default(),
            knowledge: KnowledgeRuntimeConfig::default(),
            session_rollover_tokens: 400_000,
            // ADR-0066（Phase 110b）: 既定の試験用 dispatcher では無効にしておく（他のテストへの
            // 副作用を避ける。専用のテストが明示的に有効化する）。
            shared_build_cache: false,
            build_cache_dir: PathBuf::from("/nonexistent-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution: ExecutionConfig::default(),
        },
    );
    #[cfg(test)]
    {
        if use_test_clock {
            dispatcher.test_now = Some(Arc::new(StdMutex::new(OffsetDateTime::now_utc())));
            dispatcher.test_policy_clock = Some(policy_clock);
        }
    }
    dispatcher
}

// For single-run tests: await the worker wrapper, not just the adapter. The wrapper
// finishes filesystem work and queues Completion::Worker before its handle resolves.
async fn await_worker_completion(d: &mut Dispatcher, task_id: TaskId) {
    let key = RunKey {
        task: task_id,
        work_unit: None,
    };
    (&mut d
        .running
        .get_mut(&key)
        .expect("worker was dispatched")
        .handle)
        .await
        .expect("worker task panicked");
}

// These tests have exactly one worker and one command review, with no retry/backoff.
// Each tick consumes an explicitly completed stage, regardless of filesystem speed.
async fn finish_worker_and_review(d: &mut Dispatcher, task_id: TaskId) {
    await_worker_completion(d, task_id).await;
    assert_eq!(d.tick().unwrap().finished, 1);
    (&mut d
        .reviewing
        .get_mut(&task_id)
        .expect("review was spawned")
        .handle)
        .await
        .expect("review task panicked");
    let report = d.tick().unwrap();
    assert_eq!(report.reviewed, 1);
    assert!(report.idle);
}

pub(super) async fn run_until_idle(d: &mut Dispatcher, max_ticks: usize) -> TickReport {
    let mut last = TickReport::default();
    for _ in 0..max_ticks {
        last = d.tick().unwrap();
        if last.idle {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    last
}

pub(super) async fn run_until_task_terminal(
    d: &mut Dispatcher,
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last = TickReport::default();
    loop {
        let task = store.get(task_id).unwrap().expect("task exists");
        if task.status.is_terminal() {
            assert_eq!(task.status, Status::Done, "{task:?}");
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let runs = store.runs_for_task(task_id).unwrap();
            panic!(
                "task did not reach Done before 30s: status={:?}, runs={runs:?}, last_tick={last:?}",
                task.status
            );
        }
        last = d.tick().unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// ADR-0036: 自分の `artifacts_dir` に結果ファイルと成果物を書き、少し待ってから読み直して
/// 「兄弟に上書きされていない」ことを確かめるアダプタ（実機の事故の再現条件を作る）。
struct SiblingAdapter {
    delay: Duration,
}

#[async_trait]
impl WorkerAdapter for SiblingAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let id = req.task.id.to_string();
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        std::fs::write(
            req.artifacts_dir.join("result.json"),
            format!(r#"{{"summary":"{id}","evidence":[]}}"#),
        )
        .unwrap();
        std::fs::write(req.artifacts_dir.join("report.md"), &id).unwrap();
        // 兄弟の run と重なる窓。
        tokio::time::sleep(self.delay).await;
        let back = std::fs::read_to_string(req.artifacts_dir.join("result.json")).unwrap();
        assert!(back.contains(&id), "兄弟に上書きされた: {back}");
        let rel = format!("{}/report.md", req.artifacts_rel());
        let artifact =
            task_worker::artifact::resolve(&req.workspace, "report.md", &rel, None).unwrap();
        sink.artifact(&artifact);
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: id,
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// `artifacts/plan.json` を書くテスト用アダプタ（Plan kind）、または `artifacts/review.json` を書く（Review kind）。
struct FileAdapter {
    plan_json: String,
    review_json: String,
    delay: Duration,
}

#[async_trait]
impl WorkerAdapter for FileAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        match req.task.kind {
            TaskKind::Plan => {
                // 2 回目以降（prior_review あり）は正しい plan を書き、1 回目は plan_json をそのまま書く。
                let text = if req.context.prior_review.is_empty() {
                    self.plan_json.clone()
                } else {
                    VALID_PLAN.to_string()
                };
                std::fs::write(req.artifacts_dir.join("plan.json"), text).unwrap();
            }
            TaskKind::Review => {
                assert!(req.context.review.is_some());
                std::fs::write(req.artifacts_dir.join("review.json"), &self.review_json).unwrap();
            }
            _ => {
                std::fs::write(req.workspace.join("touched"), "1").unwrap();
            }
        }
        sink.progress("working");
        tokio::time::sleep(self.delay).await;
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: format!("{:?}", req.task.kind),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

const VALID_PLAN: &str = r#"{"tasks":[
    {"title":"a","objective":"do a","acceptance":[{"text":"touched","check":{"type":"command","cmd":"test -f touched","expect_exit":0}}]},
    {"title":"b","objective":"do b","acceptance":[{"text":"touched","check":{"type":"command","cmd":"test -f touched","expect_exit":0}}],"depends_on":[0]},
    {"title":"c","objective":"do c","acceptance":[{"text":"looks good","check":{"type":"reviewer"}}],"depends_on":[0,1],"tier":"cheap"}
]}"#;

fn plan_task(dir: &std::path::Path, max_retries: u32) -> Task {
    let mut t = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        max_retries,
    );
    t.kind = TaskKind::Plan;
    t.acceptance.clear();
    t.worker_hint.tier = Tier::Frontier;
    t
}

/// (k) の偽アダプタ: Reviewer run にだけ `usage` を付ける（`FileAdapter` は常に `usage: None`）。
struct ReviewerUsageAdapter {
    review_json: String,
}

#[async_trait]
impl WorkerAdapter for ReviewerUsageAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let usage = match req.task.kind {
            TaskKind::Review => {
                std::fs::write(req.artifacts_dir.join("review.json"), &self.review_json).unwrap();
                Some(task_core::Usage {
                    input_tokens: Some(1000),
                    output_tokens: Some(200),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: Some(0.03),
                    duplicate_reads: None,
                    session_resumed: None,
                })
            }
            _ => {
                std::fs::write(req.workspace.join("touched"), "1").unwrap();
                None
            }
        };
        sink.progress("working");
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: format!("{:?}", req.task.kind),
                evidence: vec![],
                usage,
            },
            exit_code: Some(0),
        })
    }
}

fn reviewer_routing_record(store: &dyn TaskStore, task_id: TaskId) -> task_core::RoutingRecord {
    store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::RoutingDecided { record, .. } => Some(*record),
            _ => None,
        })
        .expect("routing_decided for the reviewer run")
}

/// ADR-0054 D1 / Phase 67b 追記: `resume` を頼まれた Reviewer run では `session_resume_failed` を
/// 報告する（実機の「resume が拒否された」を模す fake アダプタ）。それ以外の run は通常どおり
/// `review.json` を書いて完了する。
struct ResumeRejectingReviewAdapter {
    review_json: String,
}

#[async_trait]
impl WorkerAdapter for ResumeRejectingReviewAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let resuming = req.context.session.as_ref().is_some_and(|s| s.resume);
        if resuming {
            // 実機の crash 分類（`provider::looks_like_resume_rejection`）と同じ経路: resume を
            // 拒否されたら run の成否に関わらず報告する（ADR-0054 D1「失敗も同じ経路で作り直す」）。
            sink.session_resume_failed("simulated: no conversation found for session");
        }
        std::fs::write(req.artifacts_dir.join("review.json"), &self.review_json).unwrap();
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

/// Phase 113 D1/D2/D4(b)（ADR-0054 追記）: 1 回目の（resume を頼まれた）reviewer run は、実機の
/// `claude_code.rs`（Phase 113 で直した後）と同じ挙動を模す — resume 拒否を `session_resume_failed`
/// で報告**しつつ**、`Terminal::Error{retryable: true}` で終わる（`fail_all` されない、D2）。
/// 2 回目（session が retire されて新規セッションになった run）は、正常に `review.json` を書いて
/// `Terminal::Done` で終わる。
struct ResumeRejectingThenSucceedingReviewAdapter {
    review_json: String,
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for ResumeRejectingThenSucceedingReviewAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let resuming = req.context.session.as_ref().is_some_and(|s| s.resume);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if resuming {
            // ADR-0054 Phase 113 D1: `claude_code.rs::run_claude_code` が `result` メッセージを
            // 観測できた run（`error_during_execution`/`is_error`）でも、resume 拒否の文言が
            // stderr にあれば `session_resume_failed` を報告するようになった。
            sink.session_resume_failed(
                "No conversation found with session ID: 01a0d017-e32a-4cad-b10c-0cb63869ae13",
            );
            return Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: "claude result: error_during_execution".into(),
                    retryable: true,
                },
                exit_code: Some(1),
            });
        }
        std::fs::write(req.artifacts_dir.join("review.json"), &self.review_json).unwrap();
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

/// Phase 113 D3/D4(d)（ADR-0054 追記）: `Review` kind の run だけを見る。1 回目（`n=0`）は
/// `Reviewer` criterion（idx 1）を不合格にし、2 回目以降（`rereview` 後）は合格にする
/// （resume やインフラ都合の失敗ではなく、判定そのものが変わるケースを模す）。
struct TogglingReviewAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for TogglingReviewAdapter {
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
        if req.task.kind != TaskKind::Review {
            return Ok(done_outcome());
        }
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let pass = n >= 1;
        std::fs::write(
            req.artifacts_dir.join("review.json"),
            format!(r#"{{"verdicts":[{{"criterion":1,"pass":{pass},"reason":"r"}}]}}"#),
        )
        .unwrap();
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

fn done_outcome() -> RunOutcome {
    RunOutcome {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    }
}

/// 1 回目は供給側失敗（Throttled）、2 回目以降は `touched` を作って done を返すアダプタ。
struct FlakyProviderAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for FlakyProviderAdapter {
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
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(AdapterError::Throttled {
                retry_after: Duration::from_secs(3600),
            });
        }
        std::fs::write(req.workspace.join("touched"), "1").unwrap();
        Ok(done_outcome())
    }
}

/// heartbeat を送りながら少し待ってから done を返すアダプタ。
struct HeartbeatAdapter;

#[async_trait]
impl WorkerAdapter for HeartbeatAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        for _ in 0..8 {
            sink.heartbeat();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        std::fs::write(req.workspace.join("touched"), "1").unwrap();
        Ok(done_outcome())
    }
}

/// レビューが無く、`sink.heartbeat()` も呼ばない（テストが lease の自然な更新に邪魔されないよう
/// に）、しばらく走り続けるだけのアダプタ。
struct SlowNoHeartbeatAdapter;

#[async_trait]
impl WorkerAdapter for SlowNoHeartbeatAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(done_outcome())
    }
}

/// 2 回目以降の run でだけ `second` を作るアダプタ。
struct CountingAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for CountingAdapter {
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
        if self.calls.fetch_add(1, Ordering::SeqCst) >= 1 {
            std::fs::write(req.workspace.join("second"), "1").unwrap();
        }
        Ok(done_outcome())
    }
}

/// Review run の 1 回目だけ供給側失敗を返し、以降は pass の `review.json` を書くアダプタ。
struct FlakyReviewerAdapter {
    review_calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for FlakyReviewerAdapter {
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
        if req.task.kind == TaskKind::Review {
            if self.review_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(AdapterError::Throttled {
                    retry_after: Duration::from_millis(200),
                });
            }
            std::fs::create_dir_all(&req.artifacts_dir).unwrap();
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"fine"}]}"#,
            )
            .unwrap();
        }
        Ok(done_outcome())
    }
}

/// 常に供給側失敗（短い cooldown）を返すアダプタ。`review_only` なら Review run だけ失敗し、ワーカー run は done。
struct AlwaysThrottledAdapter {
    calls: AtomicUsize,
    review_only: bool,
}

#[async_trait]
impl WorkerAdapter for AlwaysThrottledAdapter {
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
        if self.review_only && req.task.kind != TaskKind::Review {
            return Ok(done_outcome());
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::Throttled {
            retry_after: Duration::from_millis(10),
        })
    }
}

/// Phase 113（ADR-0054 D2）: `AlwaysThrottledAdapter` と違い、`Err(AdapterError::Throttled)`
/// （プロバイダが分類できる供給側失敗）ではなく `Ok(Terminal::Error{retryable: true})` を返す
/// （`is_error` の結果・クラッシュ相当。分類できない「reviewer run 自身のインフラ都合の失敗」）。
struct AlwaysInfraFailingReviewAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for AlwaysInfraFailingReviewAdapter {
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
        if req.task.kind != TaskKind::Review {
            return Ok(done_outcome());
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(RunOutcome {
            terminal: Terminal::Error {
                message: "claude result: error_during_execution".into(),
                retryable: true,
            },
            exit_code: Some(1),
        })
    }
}

fn transition_reasons(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::Transitioned { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// 常に分類できない `Err`（`AdapterError::Other`）を返すワーカー run（ADR-0070 D3。resume 拒否や
/// result.json 不在などが実機で取る経路のスタンド・イン）。
struct AlwaysInfraFailingWorkerAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for AlwaysInfraFailingWorkerAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::Other("session resume rejected".into()))
    }
}

/// 監査 M-1〜M-3（ADR-0034 D2）: 報告はタスクの終端状態に合わせて作るテスト向けの、最小の組織（秘書 → coding → coding-poc）。
fn seed_org_for_reports(store: &Arc<dyn TaskStore>) {
    let now = OffsetDateTime::now_utc();
    for (id, parent, kind) in [
        ("secretary", None, OrgKind::Secretary),
        ("coding", Some("secretary"), OrgKind::Department),
        ("coding-poc", Some("coding"), OrgKind::Section),
    ] {
        store
            .org_upsert(&OrgNode {
                profile: Default::default(),
                id: id.into(),
                parent_id: parent.map(str::to_string),
                name: id.into(),
                kind,
                genre: None,
                brief: String::new(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
}

/// 2 回目以降の run で `ready` ファイルを作るアダプタ(レビューが 1 回差し戻されてから通る状況を作る)。
struct AttemptGatedAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for AttemptGatedAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        sink.progress("working");
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n >= 1 {
            std::fs::write(req.workspace.join("ready"), "1").unwrap();
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

/// worker が返す `retryable: true` の `error` を毎回返すアダプタ(供給側失敗ではなく、ワーカー自身の申告)。
struct RetryableErrorAdapter;

#[async_trait]
impl WorkerAdapter for RetryableErrorAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        sink.progress("working");
        Ok(RunOutcome {
            terminal: Terminal::Error {
                message: "flaky".into(),
                retryable: true,
            },
            exit_code: Some(1),
        })
    }
}

// ---- ADR-0078: 接続の維持・再接続の抑制・切断の通知と回数（偽のフックで決定的に） ----

fn instant_done_dispatcher(store: Arc<dyn TaskStore>) -> Dispatcher {
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    dispatcher(store, adapter, 1)
}

/// `refresh_cluster_liveness` を 1 巡させる: 間引きを外して呼び、別スレッドの実通信 probe が
/// 始まったら、その結果を拾い終わるまで呼び続ける（SIGSTOP stutter の教訓: 固定の sleep で待たず、
/// 結果を読み切ってから判定する）。
fn liveness_round(d: &mut Dispatcher, id: &str) {
    d.last_cluster_liveness = None;
    d.last_cluster_command_probe.clear();
    d.refresh_cluster_liveness();
    let deadline = Instant::now() + Duration::from_secs(30);
    while d
        .cluster_conn
        .get(id)
        .is_some_and(|s| s.probe_inflight.is_some())
    {
        assert!(
            Instant::now() < deadline,
            "the liveness probe did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
        d.last_cluster_liveness = None;
        d.refresh_cluster_liveness();
    }
}

fn connection_log(store: &Arc<dyn TaskStore>) -> Vec<task_core::ClusterConnectionRecord> {
    store
        .cluster_connection_list_since(OffsetDateTime::UNIX_EPOCH)
        .unwrap()
}

fn cluster_spec_with_auth(id: &str, host: &str, auth: &str) -> ClusterSpec {
    ClusterSpec {
        id: id.into(),
        host: host.into(),
        concurrency: 1,
        sync: SyncMode::Rsync,
        delete_on_push: false,
        setup: vec![],
        env: vec![],
        rsync_excludes: vec![],
        worktree: Default::default(),
        auth: auth.into(),
        forwards: vec![],
        work_dir: None,
        keepalive_secs: 0,
        liveness_probe_secs: 0,
        job_wait: Default::default(),
    }
}

/// ADR-0053 D3（Phase 66）: forward 付きの `ClusterSpec` を作る（`refresh_cluster_tunnels` 用）。
/// `probe_interval_secs` は既定（`DEFAULT_TUNNEL_PROBE_INTERVAL_SECS`）にしておく。ADR-0066 D3
/// 以降、target probe のバックオフは `d.tunnel_probe_state`（専用スレッドが書く）に移った。
/// 間引きを避けたいテストは `d.last_cluster_tunnel_refresh` を過去にずらす（`CLUSTER_LIVENESS_INTERVAL`
/// の間引きだけがテストから直接いじれる。target probe 側は 1 forward につき最初の 1 回だけ同期に
/// 種を蒔くので、通常のテストはそれだけで足りる）。
fn cluster_spec_with_forward(
    id: &str,
    host: &str,
    auth: &str,
    listen: &str,
    target: &str,
) -> ClusterSpec {
    let mut spec = cluster_spec_with_auth(id, host, auth);
    spec.forwards = vec![ClusterForwardSpec {
        listen: listen.into(),
        target: target.into(),
        probe_interval_secs: DEFAULT_TUNNEL_PROBE_INTERVAL_SECS,
    }];
    spec
}

/// ADR-0059 D3（Phase 99）: 実 ssh を起こさずに worktree 準備の exit コードを模す。`-O check`
/// （多重接続の確認。`control_master_alive` から呼ばれうる）には常に成功で答え、それ以外の呼び出しは
/// `exit_code` を返す（中身は見ない。`ensure_worktree` の分岐はコードそのものだけで決まる）。
fn write_stub_ssh(dir: &std::path::Path, exit_code: i32) -> PathBuf {
    let script = dir.join("stub-ssh.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$3\" = \"-O\" ] && [ \"$4\" = \"check\" ]; then exit 0; fi\nexit {exit_code}\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    script
}

fn remote_task(
    dir: &std::path::Path,
    mode: Option<WorkspaceMode>,
    repos: Vec<task_core::RepoRef>,
) -> Task {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/work/proj"),
        mode,
    };
    task.repos = repos;
    task
}

fn done_adapter() -> Arc<dyn WorkerAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_worker_for_test(
    store: Arc<dyn TaskStore>,
    task_id: TaskId,
    dir: PathBuf,
    settings: SshSettings,
) -> Result<RunOutcome, AdapterError> {
    run_worker(
        store,
        done_adapter(),
        Vec::new(),
        task_id,
        Tier::Standard,
        dir,
        "run1",
        RunLimits {
            wall_clock: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        LeaseRenewal {
            ttl: Duration::from_secs(60),
            every: Duration::from_secs(30),
        },
        Some(settings),
        None,
        RunExtras::default(),
        Vec::new(),
        Vec::new(),
        DelegationLimits::default(),
        None,
        None,
        ContainerDecision::Host,
        CargoTargetPlan::None,
    )
    .await
}

/// `task_id` の直接の `Approval` 子タスクが現れるまで tick を回す（Human check の生成を待つ）。
/// ADR-0016: 親 run で `delegate` を出し、子 run は少し待って done、集約 run は `artifacts/summary.md` を書くアダプタ。
struct DelegatingAdapter {
    proposals: Vec<DelegateTask>,
    child_delay: Duration,
    seen_role: std::sync::Mutex<Option<RoleContext>>,
    aggregate_children: AtomicUsize,
    write_summary: bool,
}

#[async_trait]
impl WorkerAdapter for DelegatingAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let done = |summary: &str| {
            Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: summary.into(),
                    evidence: vec![],
                    usage: None,
                },
                exit_code: Some(0),
            })
        };
        // 提案した子は role = implementer。親（lead / 役割なし）と区別する。
        if req.task.role.as_deref() == Some("implementer") {
            tokio::time::sleep(self.child_delay).await;
            return done("child");
        }
        *self.seen_role.lock().unwrap() = req.context.role.clone();
        if !req.context.children.is_empty() {
            self.aggregate_children
                .store(req.context.children.len(), Ordering::SeqCst);
            if self.write_summary {
                std::fs::create_dir_all(&req.artifacts_dir).unwrap();
                std::fs::write(req.artifacts_dir.join("summary.md"), "# summary\n").unwrap();
            }
            return done("aggregated");
        }
        sink.delegate(&self.proposals);
        done("delegated")
    }
}

fn proposal(title: &str, deps: Vec<task_core::DelegateDep>) -> DelegateTask {
    DelegateTask {
        requirements: Default::default(),
        title: title.into(),
        objective: format!("do {title}"),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        role: Some("implementer".into()),
        genre: None,
        depends_on: deps,
        tier: None,
        assignee: None,
        workspace: None,
    }
}

fn roles() -> Vec<RoleSpec> {
    vec![
        RoleSpec {
            id: "lead".into(),
            instructions: Some("You lead; delegate implementation.".into()),
            ..RoleSpec::default()
        },
        RoleSpec {
            id: "implementer".into(),
            tier: Some(Tier::Cheap),
            ..RoleSpec::default()
        },
    ]
}

fn progress_msgs(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } => Some(msg),
            _ => None,
        })
        .collect()
}

/// ADR-0027 D1 / 受け入れ 3: 委譲できる run（lead, genre=coding）のプロンプト文脈に設定済みの
/// 全分野（`available_genres`）が渡り、`DelegateTask.genre` で子を別分野（literature）に委譲できる。
struct GenreDelegatingAdapter {
    proposal: DelegateTask,
    seen_available_genres: std::sync::Mutex<Option<Vec<task_worker::GenreContext>>>,
}

#[async_trait]
impl WorkerAdapter for GenreDelegatingAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let done = |summary: &str| {
            Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: summary.into(),
                    evidence: vec![],
                    usage: None,
                },
                exit_code: Some(0),
            })
        };
        if req.task.role.as_deref() == Some("literature-reader") {
            return done("child");
        }
        *self.seen_available_genres.lock().unwrap() = Some(req.context.available_genres.clone());
        sink.delegate(std::slice::from_ref(&self.proposal));
        done("delegated")
    }
}

async fn wait_for_approval_child(
    d: &mut Dispatcher,
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
) -> Task {
    for _ in 0..100 {
        d.tick().unwrap();
        if let Some(child) = store
            .list(None)
            .unwrap()
            .into_iter()
            .find(|t| t.parent_id == Some(task_id) && t.kind == TaskKind::Approval)
        {
            return child;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("approval child was not created for task {task_id}");
}

// ---- ADR-0024: account pool ----

/// 走った run の env を記録し、`with_env` を実装するテスト用アダプタ（ADR-0024 D2）。
type CapturedEnvs = Arc<StdMutex<Vec<Vec<(String, String)>>>>;

#[derive(Clone)]
struct PoolAdapter {
    terminal_or_throttled: Result<Terminal, Duration>,
    delay: Duration,
    observation: Option<RateLimitObservation>,
    env: Vec<(String, String)>,
    captured: CapturedEnvs,
    /// S10: `true` なら `AdapterError::Spawn` を返す（`terminal_or_throttled` より優先）。
    spawn_failure: bool,
}

#[async_trait]
impl WorkerAdapter for PoolAdapter {
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
        self.captured.lock().unwrap().push(self.env.clone());
        if let Some(obs) = self.observation.clone() {
            sink.rate_limit(obs);
        }
        std::fs::write(req.workspace.join("touched"), "1").unwrap();
        tokio::time::sleep(self.delay).await;
        if self.spawn_failure {
            return Err(AdapterError::Spawn(std::io::Error::other("boom")));
        }
        match &self.terminal_or_throttled {
            Ok(terminal) => Ok(RunOutcome {
                terminal: terminal.clone(),
                exit_code: Some(0),
            }),
            Err(retry_after) => Err(AdapterError::Throttled {
                retry_after: *retry_after,
            }),
        }
    }
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        self.with_env(&[("TEST_MODEL".into(), model.into())])
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut env = self.env.clone();
        env.extend(extra.iter().cloned());
        Some(Arc::new(PoolAdapter {
            env,
            ..self.clone()
        }))
    }
}

/// 2 アカウント（`a`, `b`。両方ログイン済み）を持つ一時ディレクトリを作る。
fn accounts_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for id in ["a", "b"] {
        let acct = dir.path().join(id);
        std::fs::create_dir_all(&acct).unwrap();
        std::fs::write(acct.join(".credentials.json"), "{}").unwrap();
    }
    dir
}

#[allow(clippy::too_many_arguments)]
fn pool_dispatcher(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    second_provider: Option<(&str, Arc<dyn WorkerAdapter>)>,
    accounts_root: PathBuf,
    max_runs_per_account: usize,
    max_concurrency: usize,
) -> Dispatcher {
    pool_dispatcher_with_requeues(
        store,
        adapter,
        second_provider,
        accounts_root,
        max_runs_per_account,
        max_concurrency,
        5,
    )
}

#[allow(clippy::too_many_arguments)]
fn pool_dispatcher_with_requeues(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    second_provider: Option<(&str, Arc<dyn WorkerAdapter>)>,
    accounts_root: PathBuf,
    max_runs_per_account: usize,
    max_concurrency: usize,
    max_requeues: u32,
) -> Dispatcher {
    let mut providers = vec![ProviderSpec {
        id: "p1".into(),
        adapter: "claude-code".into(),
        tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
        concurrency: max_concurrency,
        model: "m".into(),
    }];
    let mut adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>> = HashMap::new();
    adapters.insert("p1".into(), adapter);
    if let Some((id, a)) = second_provider {
        providers.push(ProviderSpec {
            id: id.into(),
            adapter: "instant".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: max_concurrency,
            model: "m".into(),
        });
        adapters.insert(id.into(), a);
    }
    let policy = StaticPolicy::new(providers, Duration::from_secs(1));
    let account_pool_providers: std::collections::HashSet<ProviderId> = ["p1".to_string()].into();
    Dispatcher::new(
        store,
        Box::new(policy),
        HashMap::from([("p1".to_string(), "m".to_string())]),
        adapters,
        account_pool_providers,
        DispatchConfig {
            delivery: Default::default(),
            max_concurrency,
            lease_grace: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
            review_timeout: Duration::from_secs(5),
            workspace_root: PathBuf::from("/nonexistent"),
            plan_auto_accept: false,
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            reviewer_hint: crate::review::reviewer_hint(),
            reviewer_tier_override: None,
            clusters: HashMap::new(),
            cluster_cooldown: Duration::from_secs(1),
            max_requeues,
            max_reviewer_retries: 3,
            max_infra_retries: 5,
            min_free_disk_mb: 5120,
            roles: Vec::new(),
            genres: Vec::new(),
            delegation: DelegationLimits::default(),
            accounts: Some(AccountsRuntimeConfig {
                roots: HashMap::from([(AccountAdapter::ClaudeCode, accounts_root)]),
                max_runs_per_account,
                check_model: "haiku".into(),
                fallback_cooldown_secs: 300,
            }),
            memory_dir: None,
            worktree_branch_prefix: task_worker::DEFAULT_BRANCH_PREFIX.to_string(),
            releases_dir: None,
            containers: ContainersRuntimeConfig::default(),
            knowledge: KnowledgeRuntimeConfig::default(),
            session_rollover_tokens: 400_000,
            shared_build_cache: false,
            build_cache_dir: PathBuf::from("/nonexistent-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution: ExecutionConfig::default(),
        },
    )
}

fn usage_window(utilization: f64, resets_at_secs_from_now: i64) -> RateLimitObservation {
    RateLimitObservation {
        one_month: None,
        five_hour: Some(task_core::RateWindow {
            utilization,
            resets_at: 10_000 + resets_at_secs_from_now,
        }),
        seven_day: None,
        status: None,
        resets_at: None,
        observed_at: 10_000,
    }
}

// ---- ADR-0033 D4 / D6（Phase 24）: 対話と記憶 ----

/// 渡された `RunContext` を記録し、結果ファイルに `memory` を書いてから終端を返す。
/// `proposals` があれば委譲も提案する。
struct PersonAdapter {
    terminal: Terminal,
    seen: Arc<StdMutex<Option<RunContext>>>,
    memory: Option<&'static str>,
    proposals: Vec<DelegateTask>,
}

#[async_trait]
impl WorkerAdapter for PersonAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        *self.seen.lock().unwrap() = Some(req.context.clone());
        if let Some(memory) = self.memory {
            let artifacts = req.artifacts_dir.clone();
            std::fs::create_dir_all(&artifacts).unwrap();
            std::fs::write(artifacts.join("result.json"), memory).unwrap();
        }
        if !self.proposals.is_empty() && req.task.assignee.as_deref() == Some("research-survey") {
            sink.delegate(&self.proposals);
        }
        Ok(RunOutcome {
            terminal: self.terminal.clone(),
            exit_code: Some(0),
        })
    }
}

fn person_adapter(terminal: Terminal) -> PersonAdapter {
    PersonAdapter {
        terminal,
        seen: Arc::new(StdMutex::new(None)),
        memory: None,
        proposals: Vec::new(),
    }
}

fn org_node_of(id: &str, parent: Option<&str>, kind: OrgKind, genre: Option<&str>) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: format!("{id} 課"),
        kind,
        genre: genre.map(str::to_string),
        brief: format!("{id} の担当"),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn seed_conversation_org(store: &dyn TaskStore) {
    for n in [
        org_node_of("secretary", None, OrgKind::Secretary, Some("secretary")),
        org_node_of("research", Some("secretary"), OrgKind::Department, None),
        org_node_of("research-survey", Some("research"), OrgKind::Section, None),
        org_node_of("research-data", Some("research"), OrgKind::Section, None),
        org_node_of("coding", Some("secretary"), OrgKind::Department, None),
        org_node_of("coding-poc", Some("coding"), OrgKind::Section, None),
    ] {
        store.org_upsert(&n).unwrap();
    }
}

fn person_dispatcher(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    workspace_root: PathBuf,
    memory_dir: Option<PathBuf>,
) -> Dispatcher {
    let mut d = dispatcher(store, adapter, 2);
    d.config.workspace_root = workspace_root;
    d.config.memory_dir = memory_dir;
    d.config.genres = vec![GenreSpec {
        id: "secretary".into(),
        description: "人と話す".into(),
        default_role: Some("secretary".into()),
        roles: vec!["secretary".into()],
        ..GenreSpec::default()
    }];
    d
}

fn assigned_task(workspace_root: &std::path::Path, name: &str, assignee: &str) -> Task {
    let dir = workspace_root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let mut task = new_task(&dir, Check::Human, 0);
    task.assignee = Some(assignee.to_string());
    task
}

fn delegate_to(assignee: &str) -> DelegateTask {
    DelegateTask {
        requirements: Default::default(),
        title: "任せたい仕事".into(),
        objective: "やっておいて".into(),
        // ADR-0067 D2: `human` チェックには artifacts か知識ベースの参照が要る。
        acceptance: vec![
            Criterion {
                text: "できた".into(),
                check: Check::Human,
            },
            Criterion {
                text: "result.md exists".into(),
                check: Check::ArtifactExists {
                    name: "result.md".into(),
                },
            },
        ],
        role: None,
        genre: None,
        depends_on: vec![],
        tier: None,
        assignee: Some(assignee.to_string()),
        workspace: None,
    }
}

// ---- Phase 33（ADR-0033 D4 追記。実機の事故の再発防止）: 担当は自分の仕事を知っている ----

/// `assignee` の仕事を 1 件作る（`kind = execute`、`Check::Human` はダミー。テストが `status` /
/// `project_id` / `updated_at` を直接指定できるようにするだけの下請け）。
fn work_task(
    title: &str,
    status: Status,
    assignee: &str,
    project_id: Option<ProjectId>,
    updated_at: OffsetDateTime,
) -> Task {
    let dir = std::path::PathBuf::from("/nonexistent");
    let mut t = new_task(&dir, Check::Human, 0);
    t.title = title.to_string();
    t.status = status;
    t.assignee = Some(assignee.to_string());
    t.project_id = project_id;
    t.updated_at = updated_at;
    t
}

fn titled_project(title: &str) -> Project {
    let now = OffsetDateTime::now_utc();
    Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: title.to_string(),
        request: "r".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    }
}

// ---- Phase 49（ADR-0041 D1）: ローカルの作業場所もタスクごとに worktree ----

/// テスト用の git リポジトリ（`main` に 1 コミット）。返すのは `main` の sha。
fn init_test_repo(dir: &std::path::Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    std::fs::write(dir.join("README.md"), b"hello\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "first"]] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    git_out(dir, &["rev-parse", "HEAD"])
}

/// (h) の偽アダプタ: worker run の中で自分のブランチに 1 コミットし、**別のファイル**を触る
/// コミットを `repo_dir`（`main`）にも積む（main が worker の作業中に先行した状態を作る。
/// 触るファイルが違うので merge は衝突しない）。
struct AdvancesMainWhileWorkingAdapter {
    repo_dir: PathBuf,
}

#[async_trait]
impl WorkerAdapter for AdvancesMainWhileWorkingAdapter {
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
        let cwd = req.cwd().to_path_buf();
        std::fs::write(cwd.join("worker-side.txt"), b"x").unwrap();
        git_out(&cwd, &["add", "-A"]);
        git_out(&cwd, &["commit", "-q", "-m", "worker change"]);
        std::fs::write(self.repo_dir.join("main-side.txt"), b"y").unwrap();
        git_out(&self.repo_dir, &["add", "-A"]);
        git_out(&self.repo_dir, &["commit", "-q", "-m", "advance main"]);
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

/// `git -C <dir> <args...>` の stdout（trim 済み）。失敗したら panic。
fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_ok(dir: &std::path::Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---- ADR-0044 D2（Phase 53）: 人のコメントによる割り込み ----

/// 走り続けて、渡された `RunContext` を記録するアダプタ。`hold` の間は終わらない
/// （1 回目の run を「走っている」状態にするため）。2 回目以降はすぐ `done` になる。
struct InterruptProbeAdapter {
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
    hold: Duration,
}

#[async_trait]
impl WorkerAdapter for InterruptProbeAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let first = match self.seen.lock() {
            Ok(mut seen) => {
                seen.push(req.context.clone());
                seen.len() == 1
            }
            Err(_) => false,
        };
        if first {
            // 1 回目: 人が割り込むまで走り続ける（tick がこの run を abort する）。
            sink.progress("working");
            tokio::time::sleep(self.hold).await;
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

/// `comment` を 1 行出してから終わるアダプタ。
struct CommentingAdapter;

#[async_trait]
impl WorkerAdapter for CommentingAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        sink.comment("ビルドが通った");
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

/// 走った run の cwd / workspace / artifacts_dir を記録し、`files` を cwd に作るアダプタ。
struct RecordingAdapter {
    seen: Arc<StdMutex<Vec<(PathBuf, PathBuf, PathBuf)>>>,
    files: Vec<String>,
}

#[async_trait::async_trait]
impl WorkerAdapter for RecordingAdapter {
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
        tokio::task::yield_now().await;
        if let Ok(mut seen) = self.seen.lock() {
            seen.push((
                req.cwd().to_path_buf(),
                req.workspace.clone(),
                req.artifacts_dir.clone(),
            ));
        }
        for name in &self.files {
            std::fs::write(req.cwd().join(name), b"x").unwrap();
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

/// `workspace_root` を実体のあるディレクトリにした dispatcher（既定の `/nonexistent` では worktree を作れない）。
fn worktree_dispatcher(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    workspace_root: &std::path::Path,
    releases_dir: Option<PathBuf>,
) -> Dispatcher {
    let mut d = dispatcher(store, adapter, 1);
    d.config.workspace_root = workspace_root.to_path_buf();
    d.config.releases_dir = releases_dir;
    d
}

fn git_task(repo: &std::path::Path, mode: Option<task_core::WorkspaceMode>, check: Check) -> Task {
    let mut task = new_task(repo, check, 0);
    task.workspace = WorkspaceSpec::Local {
        path: repo.to_path_buf(),
        mode,
    };
    task
}

// ---- ADR-0066 D1（Phase 110b）: 共有ビルドキャッシュの `CARGO_TARGET_DIR` ----

/// `req.context.workspace_note` を記録するだけのアダプタ。
struct WorkspaceNoteAdapter {
    seen: Arc<StdMutex<Vec<Option<String>>>>,
}

#[async_trait::async_trait]
impl WorkerAdapter for WorkspaceNoteAdapter {
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
        self.seen.lock().unwrap().push(req.context.workspace_note);
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

/// run の `artifacts_dir` に `report.md` を書くだけの偽アダプタ（`Event::ArtifactProduced` は
/// 出さない。claude-code/codex と同じく、成果物の申告をしないハーネスを模す）。
struct WritesReportAdapter;

#[async_trait]
impl WorkerAdapter for WritesReportAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        std::fs::write(req.artifacts_dir.join("report.md"), "# report\n").unwrap();
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

// ---- ADR-0043 D2 / D4（Phase 52）: 複数のリポジトリ ----

/// 案件と `project_repos` を仕込み、その案件のタスクを返す。
fn project_with_repos(
    store: &Arc<dyn TaskStore>,
    repos: &[(&str, &std::path::Path, task_core::RepoKind)],
) -> (task_core::ProjectId, Vec<task_core::ProjectRepo>) {
    let now = OffsetDateTime::now_utc();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: task_core::ProjectId::new(),
        title: "benchfs".into(),
        request: "測る".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let mut out = Vec::new();
    for (i, (name, path, kind)) in repos.iter().enumerate() {
        let repo = task_core::ProjectRepo {
            id: task_core::RepoId::new(),
            project_id: project.id,
            name: (*name).to_string(),
            kind: *kind,
            location: WorkspaceSpec::local(*path),
            default_branch: None,
            sync: None,
            run: task_core::RepoRun::Auto,
            is_primary: i == 0,
            created_at: now,
        };
        store.repo_create(&repo).unwrap();
        out.push(store.repo_get(repo.id).unwrap().unwrap());
    }
    (project.id, out)
}

/// ADR-0069（Phase 114）テスト用: 3 lane の束縛を持つ TieredAdapter（cheap だけ reasoning effort 付き）。
fn three_lane_adapter() -> Arc<dyn WorkerAdapter> {
    use task_core::model_routing::ModelBinding;
    Arc::new(task_worker::tiered::TieredAdapter {
        base: Arc::new(InstantAdapter {
            terminal: Terminal::Question {
                text: "not needed".into(),
            },
            delay: Duration::ZERO,
        }),
        models: [
            (Tier::Frontier, "frontier-id", None),
            (Tier::Standard, "standard-id", None),
            (Tier::Cheap, "cheap-id", Some("low")),
        ]
        .into_iter()
        .map(|(tier, id, effort)| {
            (
                tier,
                ModelBinding {
                    name: id.into(),
                    model_id: Some(id.into()),
                    unavailable_reason: None,
                    reasoning_effort: effort.map(str::to_string),
                },
            )
        })
        .collect(),
        account_id: None,
        credential_error: None,
    })
}

fn routing_record(events: &[(u64, Event)]) -> Option<task_core::RoutingRecord> {
    events.iter().rev().find_map(|(_, e)| match e {
        Event::RoutingDecided { record, .. } => Some((**record).clone()),
        _ => None,
    })
}

// ---- ADR-0056 D3（Phase 79）: mount された skills を run に届ける ----

fn write_kb_skill(kb_root: &std::path::Path, name: &str, description: &str, body: &str) {
    let dir = kb_root.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .unwrap();
}

// ========== ADR-0072（Phase E1）: run lifecycle / checkpoint / continuation ==========

/// 常に `Terminal::BudgetExhausted` を返す（checkpoint は書かない。mechanical だけになる）。
struct AlwaysBudgetExhaustedAdapter;

#[async_trait]
impl WorkerAdapter for AlwaysBudgetExhaustedAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        Ok(RunOutcome {
            terminal: Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::Turns,
                message: "error_max_turns".into(),
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// 1 回目は `Terminal::Yielded`（checkpoint で進捗を申告）、2 回目は `done` を返すアダプタ。
/// 渡された `RunContext` を記録する。
struct YieldThenDoneAdapter {
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
}

#[async_trait]
impl WorkerAdapter for YieldThenDoneAdapter {
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
        let n = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(req.context.clone());
            seen.len()
        };
        if n == 1 {
            Ok(RunOutcome {
                terminal: Terminal::Yielded {
                    checkpoint: serde_json::json!({
                        "completed": ["A を実装した"],
                        "remaining": ["B のテスト"],
                        "next_action": "B のテストを書く",
                    }),
                    usage: None,
                },
                exit_code: Some(0),
            })
        } else {
            Ok(done_outcome())
        }
    }
}

// ========== ADR-0072（Phase E2）: ExecutionPlan / WorkUnit の決定的 scheduler ==========

/// WU の `key`（`req.context.work_unit`。無ければ `"atomic"`）ごとに、呼ばれるたびに 1 つずつ
/// 消費する応答の列。無くなれば `Terminal::Done` を返す。渡された `RunContext` も記録する。
struct WuScriptAdapter {
    script: StdMutex<HashMap<String, std::collections::VecDeque<Terminal>>>,
    seen: Arc<StdMutex<Vec<(String, task_worker::RunContext)>>>,
}

impl WuScriptAdapter {
    fn new(script: HashMap<String, Vec<Terminal>>) -> Self {
        WuScriptAdapter {
            script: StdMutex::new(
                script
                    .into_iter()
                    .map(|(k, v)| (k, v.into_iter().collect()))
                    .collect(),
            ),
            seen: Arc::new(StdMutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl WorkerAdapter for WuScriptAdapter {
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
        let terminal = self
            .script
            .lock()
            .unwrap()
            .get_mut(&key)
            .and_then(|q| q.pop_front())
            .unwrap_or(Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            });
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

fn wu_spec(key: &str, depends_on: &[&str]) -> task_core::WorkUnitSpec {
    task_core::WorkUnitSpec {
        key: key.to_string(),
        kind: task_core::WorkUnitKind::Implement,
        title: format!("Work on {key}"),
        objective: format!("Implement {key} thoroughly and completely"),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        done_when: vec![format!("{key} is done")],
        checks: vec![],
        context: task_core::WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    }
}

fn adopt_three_step_plan(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
) -> task_core::ExecutionPlanRow {
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "A -> B -> C".to_string(),
        work_units: vec![
            wu_spec("a", &[]),
            wu_spec("b", &["a"]),
            wu_spec("c", &["b"]),
        ],
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
    .expect("adopt plan")
}

// ========== ADR-0072（Phase E3）: Complexity Gate と自動 planning ==========

/// planner run の応答を制御し、それ以外（WU/atomic の run）は内側の `WuScriptAdapter` に委譲する。
/// `plan_attempts` は planner run が呼ばれるたびに 1 つずつ消費する
/// （`Some(json)` は `execution-plan.json` に書く内容、`None` は「書かない」＝ 不正な試行）。
struct PlannerScriptAdapter {
    plan_attempts: StdMutex<std::collections::VecDeque<Option<String>>>,
    wu: WuScriptAdapter,
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
}

impl PlannerScriptAdapter {
    fn new(plan_attempts: Vec<Option<String>>, wu_script: HashMap<String, Vec<Terminal>>) -> Self {
        PlannerScriptAdapter {
            plan_attempts: StdMutex::new(plan_attempts.into_iter().collect()),
            wu: WuScriptAdapter::new(wu_script),
            seen: Arc::new(StdMutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl WorkerAdapter for PlannerScriptAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push(req.context.clone());
        if req.context.execution_planner.is_some() {
            let next = self.plan_attempts.lock().unwrap().pop_front().flatten();
            if let Some(json) = next {
                std::fs::create_dir_all(&req.artifacts_dir).ok();
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
        self.wu.run(req, run_id, limits, sink).await
    }
}

fn plan_json(work_units: Vec<task_core::WorkUnitSpec>) -> String {
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "test plan".to_string(),
        work_units,
        phases: Vec::new(),
        children: Vec::new(),
    };
    serde_json::to_string(&spec).unwrap()
}

/// 人の明示（`execution_hint.explicit = true`）を持つ compound task を作る（gate の規則表を
/// バイパスし、テストを score のチューニングから独立させる）。
fn compound_task(dir: &std::path::Path) -> Task {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        }),
        ..Default::default()
    });
    task
}

/// shadow の下で CoS のヒントから判定された（`shadow: true`、`source: hint`）判定を持つ Task。
fn shadow_gated_task(dir: &std::path::Path, mode: task_core::ExecutionMode) -> Task {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: false,
        }),
        execution: Some(task_core::ExecutionGateDecision {
            mode,
            source: task_core::GateSource::Hint,
            score: 13,
            threshold: 5,
            rule_id: "compound/long-and-broad".to_string(),
            signals: Vec::new(),
            policy_version: "exec-gate/1".to_string(),
            shadow: true,
            depth: None,
        }),
        ..Default::default()
    });
    task
}

/// 完了した WU の run の後、`key` が `repair-` で始まる repair run のときだけ `.repair-done` を
/// 作る（`Check::Command` の再実行が通るようにする）。それ以外は内側の `WuScriptAdapter` そのまま。
struct RepairFsAdapter(WuScriptAdapter);

#[async_trait]
impl WorkerAdapter for RepairFsAdapter {
    fn id(&self) -> &str {
        self.0.id()
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let is_repair = req
            .context
            .work_unit
            .as_ref()
            .is_some_and(|w| w.key.starts_with("repair-"));
        let cwd = req.cwd().to_path_buf();
        let result = self.0.run(req, run_id, limits, sink).await;
        if is_repair {
            std::fs::write(cwd.join(".repair-done"), "x").expect("write .repair-done");
        }
        result
    }
}

/// 2 回目以降の呼び出しでだけ `.checked` を作る（`WorkUnitCheck` を最初は失敗させ、retry で直させる）。
struct ChecksAdapter {
    wu: WuScriptAdapter,
    calls: StdMutex<HashMap<String, u32>>,
}

impl ChecksAdapter {
    fn new() -> Self {
        ChecksAdapter {
            wu: WuScriptAdapter::new(HashMap::new()),
            calls: StdMutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl WorkerAdapter for ChecksAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let key = req
            .context
            .work_unit
            .as_ref()
            .map(|w| w.key.clone())
            .unwrap_or_default();
        let cwd = req.cwd().to_path_buf();
        let call_n = {
            let mut calls = self.calls.lock().unwrap();
            let n = calls.entry(key).or_insert(0);
            *n += 1;
            *n
        };
        if call_n >= 2 {
            std::fs::write(cwd.join(".checked"), "x").expect("write .checked");
        }
        self.wu.run(req, run_id, limits, sink).await
    }
}

// ========== ADR-0074（Phase F2b）: WU の並列実行・WU の worktree・工程の統合 ==========

/// 並列 WU のテスト用アダプタ。WU の key ごとに、作業ツリー（`req.work_dir`）へ書くファイル
/// （`key -> [(path, content)]`）と終わり方の列を持つ。同時に走っている run の数の最大値を数える。
type WuAction = Box<dyn Fn(&std::path::Path) + Send + Sync>;

struct ParallelWuAdapter {
    files: HashMap<String, Vec<(String, String)>>,
    /// key ごとの作業（作業ツリーで走らせる。merge の衝突の解消など）。
    actions: HashMap<String, WuAction>,
    /// key ごとの run の長さ（無ければ `delay`）。
    delays: HashMap<String, Duration>,
    script: StdMutex<HashMap<String, std::collections::VecDeque<Terminal>>>,
    delay: Duration,
    active: std::sync::atomic::AtomicUsize,
    max_active: std::sync::atomic::AtomicUsize,
    /// `(key, work_dir, artifacts_dir)`。
    seen: StdMutex<Vec<(String, PathBuf, PathBuf)>>,
    /// この key の run は `gate` が開くまで終わらない（再起動・Cancel の試験用）。
    hold: StdMutex<std::collections::HashSet<String>>,
    gate: tokio::sync::Notify,
    /// planner run（`context.execution_planner`）が順に書く `execution-plan.json`（F5-fix）。
    planner_outputs: StdMutex<std::collections::VecDeque<String>>,
}

impl ParallelWuAdapter {
    fn new(delay: Duration) -> Self {
        ParallelWuAdapter {
            files: HashMap::new(),
            actions: HashMap::new(),
            delays: HashMap::new(),
            script: StdMutex::new(HashMap::new()),
            delay,
            active: std::sync::atomic::AtomicUsize::new(0),
            max_active: std::sync::atomic::AtomicUsize::new(0),
            seen: StdMutex::new(Vec::new()),
            hold: StdMutex::new(std::collections::HashSet::new()),
            gate: tokio::sync::Notify::new(),
            planner_outputs: StdMutex::new(std::collections::VecDeque::new()),
        }
    }

    fn with_planner_output(self, json: String) -> Self {
        self.planner_outputs.lock().unwrap().push_back(json);
        self
    }

    fn with_file(mut self, key: &str, path: &str, content: &str) -> Self {
        self.files
            .entry(key.to_string())
            .or_default()
            .push((path.to_string(), content.to_string()));
        self
    }

    fn with_action(
        mut self,
        key: &str,
        action: impl Fn(&std::path::Path) + Send + Sync + 'static,
    ) -> Self {
        self.actions.insert(key.to_string(), Box::new(action));
        self
    }

    fn with_delay(mut self, key: &str, delay: Duration) -> Self {
        self.delays.insert(key.to_string(), delay);
        self
    }

    fn holding(self, key: &str) -> Self {
        self.hold.lock().unwrap().insert(key.to_string());
        self
    }

    fn with_script(self, key: &str, terminals: Vec<Terminal>) -> Self {
        self.script
            .lock()
            .unwrap()
            .insert(key.to_string(), terminals.into_iter().collect());
        self
    }

    fn max_active(&self) -> usize {
        self.max_active.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn keys_seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(k, _, _)| k.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for ParallelWuAdapter {
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
        use std::sync::atomic::Ordering;
        if req.context.execution_planner.is_some() {
            let next = self.planner_outputs.lock().unwrap().pop_front();
            if let Some(json) = next {
                std::fs::create_dir_all(&req.artifacts_dir).unwrap();
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json).unwrap();
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
        let cwd = req
            .work_dir
            .clone()
            .unwrap_or_else(|| req.workspace.clone());
        self.seen
            .lock()
            .unwrap()
            .push((key.clone(), cwd.clone(), req.artifacts_dir.clone()));
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        if let Some(files) = self.files.get(&key) {
            for (path, content) in files {
                std::fs::write(cwd.join(path), content).unwrap();
            }
        }
        if let Some(action) = self.actions.get(&key) {
            action(&cwd);
        }
        tokio::time::sleep(self.delays.get(&key).copied().unwrap_or(self.delay)).await;
        let held = self.hold.lock().unwrap().contains(&key);
        if held {
            self.gate.notified().await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        let terminal = self
            .script
            .lock()
            .unwrap()
            .get_mut(&key)
            .and_then(|q| q.pop_front())
            .unwrap_or(Terminal::Done {
                summary: format!("{key} ok"),
                evidence: vec![],
                usage: None,
            });
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

fn v2_wu(key: &str, phase: &str, depends_on: &[&str]) -> task_core::WorkUnitSpec {
    let mut s = wu_spec(key, depends_on);
    s.phase = Some(phase.to_string());
    s
}

fn adopt_v2_plan(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
    phases: &[&str],
    work_units: Vec<task_core::WorkUnitSpec>,
) -> task_core::ExecutionPlanRow {
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
        rationale: "parallel".to_string(),
        work_units,
        phases: phases
            .iter()
            .map(|p| task_core::PhaseSpec {
                key: p.to_string(),
                kind: task_core::WorkUnitKind::Implement,
                title: p.to_string(),
            })
            .collect(),
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
    .expect("adopt v2 plan")
}

/// 並列 WU の dispatcher（`workspace_root` は実体、全体の並列度 `max_concurrency`、provider の
/// 並列度 `provider_concurrency`、Task ごとの並列 WU の上限 `limit`）。
fn parallel_dispatcher(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    workspace_root: &std::path::Path,
    max_concurrency: usize,
    provider_concurrency: usize,
    limit: usize,
) -> Dispatcher {
    let mut d = dispatcher(store, adapter, max_concurrency);
    d.config.workspace_root = workspace_root.to_path_buf();
    d.config.execution.max_parallel_work_units = limit;
    d.policy = Box::new(StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            adapter: "instant".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: provider_concurrency,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    ));
    d
}

fn parallel_task(repo: &std::path::Path, check_cmd: &str) -> Task {
    let mut task = git_task(
        repo,
        None,
        Check::Command {
            cmd: check_cmd.into(),
            expect_exit: 0,
        },
    );
    task.budget.max_retries = 2;
    task
}

fn events_of(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<Event> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

/// 3 つの独立した WU を `limit` で走らせ、観測された最大の同時実行数・タスク・イベントを返す。
async fn run_three_independent_units(
    limit: usize,
    provider_concurrency: usize,
) -> (
    usize,
    Task,
    Vec<task_core::WorkUnitRow>,
    Vec<Event>,
    PathBuf,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(
        repo.path(),
        "test -f a.txt && test -f b.txt && test -f c.txt",
    );
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![
            v2_wu("a", "build", &[]),
            v2_wu("b", "build", &[]),
            v2_wu("c", "build", &[]),
        ],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(300))
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b")
            .with_file("c", "c.txt", "c"),
    );
    let mut d = parallel_dispatcher(
        store.clone(),
        adapter.clone(),
        root.path(),
        3,
        provider_concurrency,
        limit,
    );
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    let units = store.work_units_for(task.id).unwrap();
    let events = events_of(&store, task.id);
    let task_dir = root.path().join(task.id.to_string());
    // ADR-0074 §5.2: 統合 WU・工程・commit の列は events から作り直せる（replay の突き合わせで差分 0）。
    let (wu_mismatches, _runs, plan_mismatches, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(wu_mismatches.is_empty(), "{wu_mismatches:?}");
    assert!(plan_mismatches.is_empty(), "{plan_mismatches:?}");
    (
        adapter.max_active(),
        stored,
        units,
        events,
        task_dir,
        repo,
        root,
    )
}

async fn run_until(d: &mut Dispatcher, max_ticks: usize, mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..max_ticks {
        d.tick().unwrap();
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// ADR-0125 (b): 状態待ちの保険。tick の回数ではなく壁時計で打ち切る（負荷で遅れても成立条件にしない）。
pub(super) const STATE_WAIT_GUARD: Duration = Duration::from_secs(60);

/// ADR-0125 (b): `done` が真になるまで tick を駆動する。判定は store の状態・event（`done`）だけで、tick の回数は
/// 失敗条件にしない。[`STATE_WAIT_GUARD`] を過ぎたら `false`（壊れたときに止まる保険）。tick の間の短い sleep は
/// worker・検査の task への譲りで、成立条件ではない。
pub(super) async fn run_until_state(d: &mut Dispatcher, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    loop {
        d.tick().unwrap();
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn wu_status(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    key: &str,
) -> Option<task_core::WorkUnitStatus> {
    store
        .work_units_for(id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == key)
        .map(|u| u.status)
}

fn event_index(events: &[Event], pred: impl Fn(&Event) -> bool) -> Option<usize> {
    events.iter().position(pred)
}

fn finished_index(events: &[Event], run_id: &str) -> Option<usize> {
    event_index(
        events,
        |e| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id),
    )
}

fn transitioned_index(events: &[Event], reason: &str) -> Option<usize> {
    event_index(
        events,
        |e| matches!(e, Event::Transitioned { reason: r, .. } if r == reason),
    )
}

/// `pause_after = after(design)` の 2 工程（design: a、build: b）の Task を、design の後の
/// 途中確認（`awaiting_human`）まで進める。
async fn paused_after_design() -> (
    Arc<dyn TaskStore>,
    Dispatcher,
    Arc<ParallelWuAdapter>,
    Task,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    task.routing = Some(task_core::TaskRouting {
        pause_after: task_core::PausePolicy::After {
            phases: vec!["design".to_string()],
        },
        ..Default::default()
    });
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["design", "build"],
        vec![v2_wu("a", "design", &[]), v2_wu("b", "build", &["a"])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    (store, d, adapter, task, repo, root)
}

/// ADR-0074 F5-fix（不具合 1）: `with_env` を受け、`runs/<run_id>/request.json` を書き（実アダプタの
/// `write_run_request` と同じく `RunRequest` をそのまま）、与えられた `CARGO_TARGET_DIR` に cargo の
/// 生成物の代わりのファイルを置く WU 用アダプタ。
#[derive(Clone)]
struct TargetDirAdapter {
    env: Vec<(String, String)>,
    delay: Duration,
    /// `(key, request.json のパス)`。
    seen: Arc<StdMutex<Vec<(String, PathBuf)>>>,
}

#[async_trait]
impl WorkerAdapter for TargetDirAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.env.extend(extra.iter().cloned());
        Some(Arc::new(next))
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let key = req
            .context
            .work_unit
            .as_ref()
            .map(|w| w.key.clone())
            .unwrap_or_else(|| "atomic".to_string());
        let run_dir = req.workspace.join("runs").join(run_id);
        std::fs::create_dir_all(&run_dir).unwrap();
        let request = run_dir.join("request.json");
        std::fs::write(&request, serde_json::to_string_pretty(&req).unwrap()).unwrap();
        if let Some((_, target)) = self.env.iter().find(|(k, _)| k == "CARGO_TARGET_DIR") {
            let debug = PathBuf::from(target).join("debug");
            std::fs::create_dir_all(&debug).unwrap();
            std::fs::write(debug.join("libtask_core.rlib"), key.as_bytes()).unwrap();
        }
        self.seen.lock().unwrap().push((key, request));
        tokio::time::sleep(self.delay).await;
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

// ---- Phase F5-fix2: WU run の完了が記録されない（dogfood 4 回目） ----

/// 本番の `gate` WU（kind = test、5 つの checks）と同じ形の 1 工程の v2 計画。1 回目の検査は
/// `flag` が無いので落ち（`work_unit_retry`。本番の replan v3 で failed → ready に戻った後と同じく
/// WU の `runs` が 2 になる）、2 回目は `flag` があるので `sleep` の後に通る（検査の窓）。
fn adopt_gate_plan(store: &Arc<dyn TaskStore>, task_id: TaskId, flag: &Path, sleep: &str) {
    let mut gate = v2_wu("gate", "gate", &[]);
    gate.kind = task_core::WorkUnitKind::Test;
    gate.checks = vec![
        task_core::WorkUnitCheck {
            cmd: "true".into(),
            expect_exit: 0,
            scope: false,
        },
        task_core::WorkUnitCheck {
            cmd: format!(
                "if [ -f {f} ]; then sleep {sleep}; else touch {f}; exit 1; fi",
                f = flag.display()
            ),
            expect_exit: 0,
            scope: false,
        },
    ];
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
        rationale: "gate".to_string(),
        work_units: vec![gate],
        phases: vec![task_core::PhaseSpec {
            key: "gate".to_string(),
            kind: task_core::WorkUnitKind::Test,
            title: "gate".to_string(),
        }],
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
    .expect("adopt gate plan");
}

/// 本番の result.json と同じ形（criterion 0/1 の evidence、~1.59M input tokens の usage）。
fn gate_done_terminal() -> Terminal {
    Terminal::Done {
        summary: "全体ゲートを再実行し、全項目成功を確認".into(),
        evidence: vec![
            Evidence {
                criterion: 0,
                command: Some("cargo test --workspace".into()),
                exit: Some(0),
                stdout_tail: Some("2515 passed, 0 failed, 5 ignored".into()),
            },
            Evidence {
                criterion: 1,
                command: Some("Reviewed gate.md".into()),
                exit: Some(0),
                stdout_tail: Some("gate.md lists every command".into()),
            },
        ],
        usage: Some(Usage {
            input_tokens: Some(1_594_550),
            output_tokens: Some(7_540),
            cache_read_tokens: Some(1_531_392),
            cache_creation_tokens: Some(0),
            cost_usd: None,
            duplicate_reads: None,
            session_resumed: None,
        }),
    }
}

fn gate_row(store: &Arc<dyn TaskStore>, id: TaskId) -> task_core::WorkUnitRow {
    store
        .work_units_for(id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "gate")
        .unwrap()
}

fn worker_finished_outcomes(store: &Arc<dyn TaskStore>, id: TaskId, run_id: &str) -> Vec<String> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerFinished {
                run_id: r, outcome, ..
            } if r == run_id => Some(outcome),
            _ => None,
        })
        .collect()
}

/// 1 回目の run（検査で落ちて retry）を終え、2 回目の run が終わって検査が走り始めたところまで
/// 進める。2 回目の run の id を返す。
async fn run_gate_until_second_checks(
    d: &mut Dispatcher,
    store: &Arc<dyn TaskStore>,
    id: TaskId,
) -> String {
    let s = store.clone();
    assert!(
        run_until(d, 500, || {
            let u = gate_row(&s, id);
            u.retries == 1 && u.runs == 2 && u.status == task_core::WorkUnitStatus::Running
        })
        .await,
        "{:?}",
        events_of(store, id)
    );
    let run_id = gate_row(store, id).last_run_id.unwrap();
    // 2 回目の run の完了を受け取り、検査を spawn するまで（run は `running` から外れる）。
    for _ in 0..200 {
        d.tick().unwrap();
        if d.running.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(d.running.is_empty());
    let u = gate_row(store, id);
    assert_eq!(u.status, task_core::WorkUnitStatus::Running, "{u:?}");
    assert!(worker_finished_outcomes(store, id, &run_id).is_empty());
    run_id
}

// ---- ADR-0075（Phase G1）: scratch pool ----

fn scratch_on(d: &mut Dispatcher, dir: &Path) -> task_worker::scratch::ScratchSettings {
    let s = task_worker::scratch::ScratchSettings::with_dir(dir);
    d.config.shared_build_cache = true;
    d.config.scratch = s.clone();
    s
}

fn done_pool_adapter(captured: &CapturedEnvs) -> Arc<PoolAdapter> {
    Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: captured.clone(),
        spawn_failure: false,
    })
}

fn no_deleting_left(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            !rd.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".deleting-"))
        })
        .unwrap_or(true)
}

/// ADR-0129 (1): scratch を有効にして Task 単位の run（`task-<id>`）を 1 回走らせ、run の env と reviewer の checks の
/// env を返す。checks の 1 行目は `RUSTC_WRAPPER|SCCACHE_DIR|SCCACHE_SERVER_PORT|CARGO_INCREMENTAL|CARGO_PROFILE_DEV_DEBUG|CARGO_TARGET_DIR`
/// （未設定は `unset`）、2 行目は子プロセスの sccache の族の全部（`parent_sccache_family` と同じ形）。
async fn run_scratch_env() -> (
    task_worker::scratch::ScratchSettings,
    task_worker::scratch::Owner,
    Vec<(String, String)>,
    String,
) {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let scratch_dir = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let log = logs.path().join("check-env.log");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: format!(
                "echo \"${{RUSTC_WRAPPER-unset}}|${{SCCACHE_DIR-unset}}|${{SCCACHE_SERVER_PORT-unset}}|$CARGO_INCREMENTAL|$CARGO_PROFILE_DEV_DEBUG|$CARGO_TARGET_DIR\" >> {log}; \
                 (env | grep -E '^(RUSTC_WRAPPER|RUSTC_WORKSPACE_WRAPPER|SCCACHE_)' | sort | tr '\\n' ' '; echo) >> {log}",
                log = log.display()
            ),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
    let mut d = worktree_dispatcher(
        store.clone(),
        done_pool_adapter(&captured),
        root.path(),
        None,
    );
    scratch_on(&mut d, scratch_dir.path());
    let settings = d.config.scratch.clone();
    run_until_task_terminal(&mut d, &store, task.id).await;
    let runs = captured.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let owner = task_worker::scratch::Owner::task(task.id.to_string());
    let check = std::fs::read_to_string(&log).unwrap();
    (settings, owner, runs[0].clone(), check)
}

/// この process の env の sccache の族（`KEY=VALUE ` を辞書順に連ねたもの。checks の 2 行目と同じ形）。
fn parent_sccache_family() -> String {
    let mut family: Vec<String> = std::env::vars()
        .filter(|(k, _)| {
            k == "RUSTC_WRAPPER" || k == "RUSTC_WORKSPACE_WRAPPER" || k.starts_with("SCCACHE_")
        })
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    family.sort();
    family.iter().map(|kv| format!("{kv} ")).collect()
}

mod build_cache;
mod cheap_local_first;
mod cleanup_and_disk;
mod cluster_tunnel;
mod coding_harness_default;
mod conversation_cos;
mod cos_chat_attach_handoff;
mod cron_jobs;
mod git_workspace;
mod integration_check_progress;
mod integration_ready_race;
mod planning_and_gate;
mod provider_and_retry;
mod review;
mod role_assignments;
mod routing_and_quota;
mod routing_enforce;
mod routing_shadow;
mod target_sync;
mod tick_and_dispatch;
mod ui_ux_skills;
mod work_units;

/// Phase F5-fix6: 再起動直後の孤児 run の回収（`src/dispatcher/tests/orphan_takeover.rs`）。
mod orphan_takeover;

/// 持ち主の居ない `running` の `runs` 行（lease を持たない reviewer run 等）の取り残しと照合
/// （`src/dispatcher/tests/ownerless_runs.rs`）。
mod ownerless_runs;

/// Phase F5-fix7: 依存 WU のブランチが無いときの基点と、準備の失敗で黙って止まらないこと
/// （`src/dispatcher/tests/work_unit_dependency_base.rs`）。
mod work_unit_dependency_base;

/// ADR-0079 §7 R1b: plan/3 の kind task の unit から子 task を作り、状態を写し、段階の完了・
/// 子待ち・subtree の中止・`review: human`（`src/dispatcher/tests/tree.rs`）。
mod tree;

/// ADR-0079 §7 R1c: 子のブランチの基点・統合での子のブランチの merge・子の最終レビューの基点・孫 → 子 → root
/// （`src/dispatcher/tests/tree_branches.rs`）。
mod tree_branches;

/// ADR-0079 §7 R2a: 深さの閾値・木の子は shadow でも採用・unit の gate（上げる / 下げる / 決定）・木の上限の
/// 超過は `kind: limit` の決定の要求（`src/dispatcher/tests/tree_gate.rs`）。
mod tree_gate;

/// ADR-0079 §7 R2b: /3 の planner の入力・2 回不正な /3 の `plan_invalid`・子の失敗 → 親の replan・子の基盤の
/// 失敗の再試行と障害通知・replan の上限（`src/dispatcher/tests/tree_replan.rs`）。
mod tree_replan;

/// ADR-0079 §7 R3a: 決定の要求の流れ（計画の決定の要求・依存だけの待ち・回答の注入・limit の raise-once・
/// plan_invalid の replan / atomic / cancel・取り下げ・worker の決定）（`src/dispatcher/tests/tree_decisions.rs`）。
mod tree_decisions;

/// ADR-0079 §7 R3b: root の計画の承認（決定・`review: human`・上限に近い）・承認不要の報告・生存確認
/// （`StallDetected`）・人の replan の 2 回目の試行（`src/dispatcher/tests/tree_approval.rs`）。
mod tree_approval;

/// ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画の最終レビュー（空の replan の事故の再現）と、
/// 木でない Task の生存確認（`src/dispatcher/tests/finished_plan.rs`）。
mod finished_plan;

/// ADR-0052（Phase 64）: 知識整理 run のフォールバック（`src/dispatcher/tests/knowledge_fallback.rs`）。
mod knowledge_fallback;

/// ADR-0079 R5b-fix2: remote workspace の run 後の push（`src/dispatcher/tests/remote_push_after_run.rs`）。
mod remote_push_after_run;
/// ADR-0067 付記 2026-10-07: remote の task と question / wait で終わった run の未申告成果物の登録。
mod undeclared_artifacts_scan;

mod cluster_job_wait;
mod cos_capacity;
#[path = "../cos_chat/harness_tests.rs"]
mod cos_chat_harness_tests;
mod cos_chat_launch;
mod cos_chat_rollover;
mod cos_chat_triage;
mod cos_live_fix_d4;
mod human_gates;

/// ADR-0079 付記 R7-5: WU の checks の不合格の記録・次の run と replan への伝達・usage
/// （`src/dispatcher/tests/work_unit_check_failures.rs`）。
mod work_unit_check_failures;

/// ADR-0098（Phase R7-10）: worker の run が作る task は元の task の案件とリポジトリを継ぐ
/// （`src/dispatcher/tests/followups.rs`）。
mod followups;
/// ADR-0074「R7-11 実装時の明確化」: planner / WU の run の予算が `RunRequest.task.budget` に届く
/// （`src/dispatcher/tests/planner_budget.rs`）。
mod planner_budget;
/// ADR-0079 付記 R7-9: 統合済みの段階に unit が増えたら段階の統合をやり直す（replan・修正前の行の reopen）
/// （`src/dispatcher/tests/stage_reopen.rs`）。
mod stage_reopen;
/// ADR-0130 D2: 実装 run・WU の actual write-set の記録（`src/dispatcher/tests/write_set_record.rs`）。
mod write_set_record;

/// ADR 2026-10-05-browser-department-web-live-view D2.0: run 時の task ∩ grant と grant 縮小の即時適用
/// （`src/dispatcher/tests/browser_allowed_domains.rs`）。
mod browser_allowed_domains;
mod browser_fallback;
/// ADR-0124: atomic coding task の planner なし直行経路（`src/dispatcher/tests/direct_route.rs`）。
mod direct_route;
/// ADR-0040 付記（2026-10-04、WU 検査の引き継ぎ）: draining の旧 instance は run の終わりで手を離す
/// （`src/dispatcher/tests/drain_hand_off.rs`）。
mod drain_hand_off;
/// 工程の効き目の A/B 試験（off/on の `ab-metric` 行と効き目の assert）
/// （`src/dispatcher/tests/phase_effect_ab.rs`）。
mod phase_effect_ab;
/// 多目的 routing Phase 3: run 開始時の RoutingContext（`src/dispatcher/tests/routing_context.rs`）。
mod routing_context;
/// ADR-0074 付記 2026-10-05: 範囲 check（`WorkUnitCheck.scope`）は WU の作業時だけ流す
/// （`src/dispatcher/tests/scope_checks.rs`）。
mod scope_checks;
/// ADR-0140 D1: WU の execute continuation の同一 session resume と checkpoint fallback
/// （`src/dispatcher/tests/session_resume.rs`）。
mod session_resume;
/// ADR-0130 D5: review 前 sync の待ち行列の stale 優先（`src/dispatcher/tests/stale_priority.rs`）。
mod stale_priority;
/// ADR-0130 D3: 同じ repo の expected write-set の重なりで run の起動を待たせる
/// （`src/dispatcher/tests/write_set_gate.rs`）。
mod write_set_gate;

mod orphan_work_units;
