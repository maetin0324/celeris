//! ADR-0090（Phase R7-1）: クラスタ job の durable wait。
//!
//! - run が `wait` で終わると task は `blocked(waiting_for_cluster_jobs)`、run は `runs.status = waiting` で閉じ、
//!   attempts・continuation の回数に数えず、lease・provider の枠を持たない。
//! - daemon の poll（偽の `qstat -xf`）が状態の変化だけを `ClusterJobWaitPolled` に残し、すべての job が `F` に
//!   なったら `satisfied` と `cluster_job_resume` で task を戻し、次の run は job の最終状態を前置きに持つ continuation。
//! - 上限を過ぎたら `timed_out` と人への質問（回答で再開）。中止は `cancelled`（qdel しない）。
//! - replay の差分 0。
//!
//! すべて偽のアダプタ・偽の poll と一時ディレクトリだけで、外部ネットワークにも ssh にも出ない。

use std::collections::VecDeque;

use task_core::cluster_job::{ClusterJobState, ClusterJobWaitState};

use super::tree::{assert_replay_is_clean, leaf, stage, tree_dispatcher, unit, v3_plan};
use super::*;

/// 用意した終端を順に返す（尽きたら `Done`）。run ごとに `RunContext` を控える。
struct ScriptedAdapter {
    script: StdMutex<VecDeque<Terminal>>,
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
}

impl ScriptedAdapter {
    fn new(script: Vec<Terminal>) -> Self {
        ScriptedAdapter {
            script: StdMutex::new(script.into_iter().collect()),
            seen: Arc::new(StdMutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl WorkerAdapter for ScriptedAdapter {
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
        self.seen.lock().unwrap().push(req.context.clone());
        let terminal = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Terminal::Done {
                summary: "collected the results".into(),
                evidence: vec![],
                usage: None,
            });
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

fn wait_terminal(jobs: &[&str], timeout_secs: Option<u64>) -> Terminal {
    Terminal::Waiting {
        request: task_core::cluster_job::ClusterJobWaitRequest {
            cluster: Some("sirius".into()),
            scheduler: task_core::cluster_job::ClusterScheduler::Pbs,
            jobs: jobs.iter().map(|j| j.to_string()).collect(),
            poll_secs: Some(10),
            timeout_secs,
            summary: "submitted E1 v2".into(),
        },
        checkpoint: Some(serde_json::json!({
            "completed": ["submitted E1 v2 (42634, 42635)"],
            "remaining": ["collect the results"],
            "next_action": "collect results/e1 and compare"
        })),
        usage: None,
    }
}

fn qstat(jobs: &[(&str, &str, Option<i32>)]) -> task_worker::RemoteCommandOutput {
    let mut out = String::new();
    for (id, state, exit) in jobs {
        out.push_str(&format!(
            "Job Id: {id}.sirius-pbs\n    Job_Name = benchfs\n    job_state = {state}\n    queue = regular\n"
        ));
        if let Some(code) = exit {
            out.push_str(&format!("    Exit_status = {code}\n"));
        }
        out.push('\n');
    }
    task_worker::RemoteCommandOutput {
        exit: Some(0),
        stdout: out,
        stderr: String::new(),
    }
}

/// 偽の poll: 用意した出力を順に返し（尽きたら最後のものを繰り返す）、要求を控える。
type PollScript = Arc<StdMutex<VecDeque<task_worker::RemoteCommandOutput>>>;

fn fake_poller(
    script: PollScript,
    seen: Arc<StdMutex<Vec<ClusterJobPollRequest>>>,
) -> ClusterJobPoller {
    let last: Arc<StdMutex<Option<task_worker::RemoteCommandOutput>>> =
        Arc::new(StdMutex::new(None));
    Arc::new(move |req: &ClusterJobPollRequest| {
        seen.lock().unwrap().push(req.clone());
        let next = script.lock().unwrap().pop_front();
        let mut last = last.lock().unwrap();
        if let Some(n) = next {
            *last = Some(n);
        }
        last.clone().ok_or_else(|| "no scripted output".to_string())
    })
}

fn cluster(job_wait: task_core::cluster_job::ClusterJobWaitLimits) -> ClusterSpec {
    ClusterSpec {
        id: "sirius".into(),
        host: "sirius".into(),
        concurrency: 1,
        sync: SyncMode::None,
        delete_on_push: false,
        setup: vec!["module load openpbs".into()],
        env: vec![],
        rsync_excludes: vec![],
        worktree: Default::default(),
        auth: "manual".into(),
        forwards: vec![],
        work_dir: None,
        keepalive_secs: 0,
        liveness_probe_secs: 0,
        job_wait,
    }
}

/// 試験の組（store・dispatcher・task・アダプタ・poll の要求の控え）。
type Setup = (
    Arc<dyn TaskStore>,
    Dispatcher,
    Task,
    Arc<ScriptedAdapter>,
    Arc<StdMutex<Vec<ClusterJobPollRequest>>>,
);

fn setup(
    dir: &std::path::Path,
    script: Vec<Terminal>,
    polls: Vec<task_worker::RemoteCommandOutput>,
) -> Setup {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(ScriptedAdapter::new(script));
    let mut d = dispatcher_with_test_clock(store.clone(), adapter.clone(), 1, true);
    // 試験の組織では実 ssh を打たない（クラスタの接続確認も偽物）。
    d.set_cluster_liveness_probe(Arc::new(|_: &[String], _: &str| true));
    d.config.clusters.insert(
        "sirius".into(),
        cluster(task_core::cluster_job::ClusterJobWaitLimits {
            poll_secs: 30,
            max_wait_secs: 3600,
        }),
    );
    let seen = Arc::new(StdMutex::new(Vec::new()));
    d.set_cluster_job_poller(fake_poller(
        Arc::new(StdMutex::new(polls.into_iter().collect())),
        seen.clone(),
    ));
    (store, d, task, adapter, seen)
}

fn advance(d: &Dispatcher, secs: i64) {
    if let Some(clock) = &d.test_now {
        *clock.lock().unwrap() += time::Duration::seconds(secs);
    }
}

/// ADR-0125 (a)+(b): `pred`（store の状態・event・poll の控え）が真になるまで tick を駆動する。poll のスレッドと
/// worker は実 scheduler で進むので、tick の回数は失敗条件にせず、壁時計の保険（[`STATE_WAIT_GUARD`]）だけで止める。
/// poll の間隔・上限の判定は注入時計（[`advance`]）で進める。
async fn tick_until(d: &mut Dispatcher, mut pred: impl FnMut(&Dispatcher) -> bool) {
    let started = Instant::now();
    let mut ticks = 0usize;
    loop {
        d.tick().unwrap();
        ticks += 1;
        if pred(d) {
            return;
        }
        if started.elapsed() >= STATE_WAIT_GUARD {
            panic!(
                "condition not reached within {:?} ({ticks} ticks, in_flight {}, polls in flight {})",
                STATE_WAIT_GUARD,
                d.in_flight(),
                d.cluster_job_polls.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 走っている poll の結果を tick が拾い終えるまで待つ（poll のスレッドの完了は scheduler 任せなので出来事で待つ）。
async fn settle_polls(d: &mut Dispatcher) {
    tick_until(d, |d| d.cluster_job_polls.is_empty()).await;
}

/// 注入時計を進めない tick は、poll の結果を拾うだけで次の poll を起こさない。poll は tick の中で同期に
/// `cluster_job_polls` へ載るので、1 回の tick の直後に空であれば「起こしていない」が決まる（スレッドの速さに依らない）。
fn tick_starts_no_poll(d: &mut Dispatcher) {
    assert!(d.cluster_job_polls.is_empty(), "a poll is still in flight");
    d.tick().unwrap();
    assert!(
        d.cluster_job_polls.is_empty(),
        "a tick started a poll before poll_secs passed"
    );
}

fn last_reason(store: &Arc<dyn TaskStore>, id: TaskId) -> Option<String> {
    let events = store.events_for(id).unwrap();
    task_ops::phase_gate::last_transition_reason(&events).map(str::to_string)
}

/// 開く → poll（状態の変化だけ event）→ satisfied → continuation の run（前置きに job の最終状態）→ done。
#[tokio::test]
async fn a_wait_parks_the_task_polls_and_resumes_as_a_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut d, task, adapter, polls) = setup(
        dir.path(),
        vec![wait_terminal(&["42634", "42635"], None)],
        vec![
            qstat(&[("42634", "R", None), ("42635", "Q", None)]),
            qstat(&[("42634", "R", None), ("42635", "Q", None)]),
            qstat(&[("42634", "F", Some(0)), ("42635", "F", Some(271))]),
        ],
    );

    // 1. run が wait で終わる: task は blocked、attempts 据え置き、run は waiting で閉じる。
    let id = task.id;
    tick_until(&mut d, |_| {
        store.get(id).unwrap().unwrap().status == Status::Blocked
    })
    .await;
    let t = store.get(id).unwrap().unwrap();
    assert_eq!(t.attempts, 0);
    assert!(t.lease.is_none());
    assert_eq!(
        last_reason(&store, id).as_deref(),
        Some(task_core::cluster_job::REASON_WAITING)
    );
    assert_eq!(d.in_flight(), 0, "a waiting task holds no run slot");
    let runs = store.runs_for_task(id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, task_core::RunIndexStatus::Waiting);
    let waits = store.cluster_job_waits_for_task(id).unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].state, ClusterJobWaitState::Waiting);
    // 申告の poll_secs（10）はクラスタの既定（30）より短くできない。
    assert_eq!(waits[0].poll_secs, 30);
    assert_eq!(waits[0].timeout_secs, 3600);
    // waiting の間は一般の回答で戻せない。
    assert!(task_ops::gate::answer(store.as_ref(), id, "go".into(), None).is_err());
    // 受信箱の質問にも出ない。
    let questions = inbox_questions(&store);
    assert!(!questions.contains(&id), "{questions:?}");

    // 2. 最初の poll（状態が変わった: 未知 → R / Q）。
    tick_until(&mut d, |_| {
        store
            .events_for(id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::ClusterJobWaitPolled { .. }))
    })
    .await;
    {
        let seen = polls.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].host, "sirius");
        assert_eq!(
            seen[0].script,
            "{ module load openpbs; } && qstat -xf 42634 42635"
        );
    }
    let view = task_ops::view::active_cluster_job_wait(store.as_ref(), id)
        .unwrap()
        .expect("active wait in the task view");
    assert_eq!(view.status_line, "42634 (R) 42635 (Q)");
    assert!(view.next_poll_at.is_some());

    // poll_secs が経つまでは次の poll を起こさない（最初の poll は上で拾い終えている）。
    settle_polls(&mut d).await;
    tick_starts_no_poll(&mut d);
    assert_eq!(
        polls.lock().unwrap().len(),
        1,
        "at most one poll per poll_secs"
    );

    // 3. 2 回目の poll は状態が同じなので event を出さない。
    advance(&d, 31);
    // 2 回目の poll を起こし、その結果を tick が拾い終えるまで待つ（結果の反映前に event を数えない）。
    tick_until(&mut d, |d| {
        polls.lock().unwrap().len() == 2 && d.cluster_job_polls.is_empty()
    })
    .await;
    tick_starts_no_poll(&mut d);
    assert_eq!(
        polls.lock().unwrap().len(),
        2,
        "at most one poll per poll_secs"
    );
    let polled = store
        .events_for(id)
        .unwrap()
        .iter()
        .filter(|(_, e)| matches!(e, Event::ClusterJobWaitPolled { .. }))
        .count();
    assert_eq!(polled, 1, "an unchanged poll records no event");
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Blocked);

    // 4. すべて F: satisfied → ready → continuation の run → review → done。
    advance(&d, 31);
    tick_until(&mut d, |_| {
        store.get(id).unwrap().unwrap().status == Status::Done
    })
    .await;
    let waits = store.cluster_job_waits_for_task(id).unwrap();
    assert_eq!(waits[0].state, ClusterJobWaitState::Satisfied);
    assert_eq!(waits[0].last_status[1].state, ClusterJobState::Finished);
    assert_eq!(waits[0].last_status[1].exit_status, Some(271));
    let reasons: Vec<String> = store
        .events_for(id)
        .unwrap()
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Transitioned { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(
        reasons
            .windows(2)
            .any(|w| w[0] == task_core::cluster_job::REASON_RESUME && w[1] == "dispatch"),
        "{reasons:?}"
    );
    let t = store.get(id).unwrap().unwrap();
    assert_eq!(t.attempts, 0, "the wait consumed no attempt");

    let seen = adapter.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].continuation.is_none());
    let cont = seen[1]
        .continuation
        .clone()
        .expect("the resumed run is a continuation");
    assert_eq!(cont.previous_end, "waiting(cluster_jobs)");
    assert_eq!(
        cont.checkpoint["next_action"],
        "collect results/e1 and compare"
    );
    let jobs = cont.cluster_jobs.clone().expect("cluster job results");
    assert_eq!(jobs.cluster, "sirius");
    assert_eq!(jobs.state, "satisfied");
    assert_eq!(jobs.summary, "submitted E1 v2");
    assert!(
        jobs.jobs[0].starts_with("42634: finished (Exit_status 0"),
        "{jobs:?}"
    );
    assert!(
        jobs.jobs[1].starts_with("42635: finished (Exit_status 271"),
        "{jobs:?}"
    );
    let preamble = task_worker::preamble::continuation_section(&seen[1]);
    assert!(preamble.contains("クラスタ job の結果"), "{preamble}");
    assert!(
        preamble.contains("42635: finished (Exit_status 271"),
        "{preamble}"
    );
    drop(seen);

    // wait の checkpoint は continuation の回数に数えない（`waiting_for_cluster_jobs` / `cluster_job_resume` は読み飛ばす）。
    assert_eq!(consecutive_continuations(&store.events_for(id).unwrap()), 0);
    assert!(
        task_ops::view::active_cluster_job_wait(store.as_ref(), id)
            .unwrap()
            .is_none()
    );
    assert_replay_is_clean(&store);
}

/// 上限を過ぎた wait は `timed_out` と人への質問。回答で ready に戻り、次の run は job の状態と回答を受け取る。
#[tokio::test]
async fn a_timed_out_wait_asks_a_human_and_the_answer_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut d, task, adapter, _polls) = setup(
        dir.path(),
        vec![wait_terminal(&["42634"], Some(60))],
        vec![qstat(&[("42634", "R", None)])],
    );
    let id = task.id;
    tick_until(&mut d, |_| {
        store.get(id).unwrap().unwrap().status == Status::Blocked
    })
    .await;
    // 上限は申告の 60 秒（poll_secs の 30 以上）。
    assert_eq!(
        store.cluster_job_waits_for_task(id).unwrap()[0].timeout_secs,
        60
    );
    tick_until(&mut d, |_| {
        store
            .events_for(id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::ClusterJobWaitPolled { .. }))
    })
    .await;
    advance(&d, 61);
    tick_until(&mut d, |_| {
        store.cluster_job_waits_for_task(id).unwrap()[0].state == ClusterJobWaitState::TimedOut
    })
    .await;
    let t = store.get(id).unwrap().unwrap();
    assert_eq!(t.status, Status::Blocked);
    let events = store.events_for(id).unwrap();
    let question = task_ops::derive::latest_question(&events);
    assert!(question.contains("上限（60 秒）"), "{question}");
    assert!(question.contains("42634 (R)"), "{question}");
    assert!(question.contains("延長する"), "{question}");
    assert!(inbox_questions(&store).contains(&id));

    // 回答で ready に戻り、次の run は continuation（timed_out の job の状態）と回答を受け取る。
    task_ops::gate::answer(store.as_ref(), id, "延長して待ち直す".into(), None).unwrap();
    tick_until(&mut d, |_| {
        store.get(id).unwrap().unwrap().status == Status::Done
    })
    .await;
    let seen = adapter.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let cont = seen[1].continuation.clone().expect("continuation");
    let jobs = cont.cluster_jobs.expect("cluster job results");
    assert_eq!(jobs.state, "timed_out");
    assert!(jobs.jobs[0].starts_with("42634: Running (R"), "{jobs:?}");
    assert!(
        seen[1]
            .answers
            .iter()
            .any(|a| a.answer == "延長して待ち直す"),
        "{:?}",
        seen[1].answers
    );
    drop(seen);
    assert_replay_is_clean(&store);
}

/// 中止すると wait は `cancelled`（job は qdel しない）になり、もう poll しない。
#[tokio::test]
async fn cancelling_a_waiting_task_cancels_the_wait_without_qdel() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut d, task, _adapter, polls) = setup(
        dir.path(),
        vec![wait_terminal(&["42634"], None)],
        vec![qstat(&[("42634", "Q", None)])],
    );
    let id = task.id;
    tick_until(&mut d, |_| {
        store.get(id).unwrap().unwrap().status == Status::Blocked
    })
    .await;
    tick_until(&mut d, |_| polls.lock().unwrap().len() == 1).await;
    store
        .apply_transition_with_events(id, Trigger::Cancel, vec![])
        .unwrap();
    let waits = store.cluster_job_waits_for_task(id).unwrap();
    assert_eq!(waits[0].state, ClusterJobWaitState::Cancelled);
    let events = store.events_for(id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::ClusterJobWaitFinished { state: ClusterJobWaitState::Cancelled, detail, .. }
            if detail.contains("no qdel")
    )));
    // 上限も poll_secs も過ぎた時刻でも、cancelled の wait は poll しない（tick の中で同期に決まる）。
    settle_polls(&mut d).await;
    advance(&d, 3600);
    tick_starts_no_poll(&mut d);
    assert_eq!(
        polls.lock().unwrap().len(),
        1,
        "a cancelled wait is not polled"
    );
    for seen in polls.lock().unwrap().iter() {
        assert!(!seen.script.contains("qdel"));
    }
    assert_replay_is_clean(&store);
}

/// 不正な wait（task が local で、設定に無いクラスタ）は retryable な失敗（attempts を 1 使う）で、待たない。
#[tokio::test]
async fn a_wait_on_an_unknown_cluster_is_a_retryable_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut bad = wait_terminal(&["1"], None);
    if let Terminal::Waiting { request, .. } = &mut bad {
        request.cluster = Some("nowhere".into());
    }
    let (store, mut d, task, _adapter, _polls) = setup(dir.path(), vec![bad], vec![]);
    let id = task.id;
    tick_until(&mut d, |_| store.get(id).unwrap().unwrap().attempts == 1).await;
    let t = store.get(id).unwrap().unwrap();
    assert_ne!(t.status, Status::Blocked);
    assert!(store.cluster_job_waits_for_task(id).unwrap().is_empty());
    let events = store.events_for(id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerFinished { outcome, .. } if outcome.contains("invalid cluster job wait")
    )));
}

fn inbox_questions(store: &Arc<dyn TaskStore>) -> Vec<TaskId> {
    let ctx = task_ops::view::ViewContext {
        workspace_root: PathBuf::from("/nonexistent"),
        retry_backoff_base: Duration::ZERO,
        retry_backoff_max: Duration::ZERO,
        max_requeues: 5,
        clusters: Default::default(),
    };
    task_ops::inbox::inbox(
        store.as_ref(),
        None,
        &ctx,
        OffsetDateTime::now_utc(),
        &|_, _| Vec::new(),
    )
    .unwrap()
    .questions
    .iter()
    .map(|q| q.task.id)
    .collect()
}

/// planner run には計画を書き、leaf `a` の最初の run だけ wait で終わる（他の run は `Done`）。
struct TreeWaitAdapter {
    plan: StdMutex<Option<String>>,
    waited: std::sync::atomic::AtomicBool,
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
}

#[async_trait]
impl WorkerAdapter for TreeWaitAdapter {
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
        self.seen.lock().unwrap().push(req.context.clone());
        if req.context.execution_planner.is_some() {
            if let Some(json) = self.plan.lock().unwrap().take() {
                std::fs::create_dir_all(&req.artifacts_dir).ok();
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json).unwrap();
            }
        } else if req.context.work_unit.as_ref().is_some_and(|w| w.key == "a")
            && !self.waited.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(RunOutcome {
                terminal: wait_terminal(&["42634"], None),
                exit_code: Some(0),
            });
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

/// v3 の計画の leaf の run が wait で終わると、その unit だけが `blocked(cluster_jobs)` になり、同じ段階の兄弟は
/// 走り切る。task は `ready` のまま run を起こさず、生存確認は名指しの待ち（StallDetected を出さない）。job が終われば
/// unit は `needs_continuation` に戻り、続きの run（前置きに job の結果）の後に段階の統合と最終レビューへ進む。
#[tokio::test]
async fn a_leaf_unit_waits_alone_and_liveness_names_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    let adapter = Arc::new(TreeWaitAdapter {
        plan: StdMutex::new(Some(plan)),
        waited: std::sync::atomic::AtomicBool::new(false),
        seen: Arc::new(StdMutex::new(Vec::new())),
    });
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.set_cluster_liveness_probe(Arc::new(|_: &[String], _: &str| true));
    d.config.clusters.insert(
        "sirius".into(),
        cluster(task_core::cluster_job::ClusterJobWaitLimits {
            poll_secs: 30,
            max_wait_secs: 7200,
        }),
    );
    // 尽きたら最後の出力（R）を繰り返す。
    let polls = Arc::new(StdMutex::new(VecDeque::from(vec![qstat(&[(
        "42634", "R", None,
    )])])));
    let seen_polls = Arc::new(StdMutex::new(Vec::new()));
    let poll_script = polls.clone();
    // 偽の時計（生存確認の 600 秒・poll の間隔を進める）。
    let t0 = OffsetDateTime::now_utc();
    let clock = Arc::new(StdMutex::new(t0));
    d.test_now = Some(clock.clone());

    // 1. a は wait、b は done。task は ready で、unit a は blocked(cluster_jobs)。
    tick_until(&mut d, |_| {
        let units = store.work_units_for(root_id).unwrap();
        units.iter().any(|u| {
            u.key == "a" && u.blocked_reason == Some(task_core::WorkUnitBlockedReason::ClusterJobs)
        }) && units
            .iter()
            .any(|u| u.key == "b" && u.status == task_core::WorkUnitStatus::Done)
            && store.get(root_id).unwrap().unwrap().status == Status::Ready
    })
    .await;
    let waits = store.cluster_job_waits_for_task(root_id).unwrap();
    assert_eq!(waits.len(), 1);
    let a_id = unit(&store.work_units_for(root_id).unwrap(), "a")
        .id
        .clone();
    assert_eq!(waits[0].work_unit_id.as_deref(), Some(a_id.as_str()));
    let a_runs = store.runs_for_work_unit(&a_id).unwrap();
    assert_eq!(
        a_runs.last().unwrap().status,
        task_core::RunIndexStatus::Waiting
    );
    assert_eq!(
        unit(&store.work_units_for(root_id).unwrap(), "a").continuations,
        0
    );

    // 2. 生存確認: 名指しの待ち（600 秒を超えても StallDetected にしない）。poll は R のまま。
    d.set_cluster_job_poller(fake_poller(poll_script, seen_polls.clone()));
    for secs in [0i64, 40, 640, 1300] {
        *clock.lock().unwrap() = t0 + time::Duration::seconds(secs);
        // この時刻の tick（生存確認・due なら poll）を回し、起こした poll の結果を拾い終えるまで待つ。
        d.tick().unwrap();
        settle_polls(&mut d).await;
    }
    let events = store.events_for(root_id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::StallDetected { .. })),
        "a unit waiting on cluster jobs is a named wait"
    );
    let facts = task_ops::tree::node_liveness_facts(
        store.as_ref(),
        &store.get(root_id).unwrap().unwrap(),
        true,
        true,
    )
    .unwrap();
    let verdict = task_core::tree::liveness(&task_core::TreeSnapshot { nodes: vec![facts] });
    assert_eq!(
        verdict[0].class,
        task_core::LivenessClass::Waiting,
        "{verdict:?} {:?}",
        store
            .work_units_for(root_id)
            .unwrap()
            .iter()
            .map(|u| (u.key.clone(), u.status, u.blocked_reason))
            .collect::<Vec<_>>()
    );
    assert_eq!(verdict[0].reason, "cluster_jobs");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);

    // 3. F になったら unit a は needs_continuation → 続きの run → 統合 → 最終レビュー → done。
    polls.lock().unwrap().clear();
    polls
        .lock()
        .unwrap()
        .push_back(qstat(&[("42634", "F", Some(0))]));
    *clock.lock().unwrap() = t0 + time::Duration::seconds(1400);
    tick_until(&mut d, |_| {
        store.get(root_id).unwrap().unwrap().status == Status::Done
    })
    .await;
    let waits = store.cluster_job_waits_for_task(root_id).unwrap();
    assert_eq!(waits[0].state, ClusterJobWaitState::Satisfied);
    let reasons = super::tree::unit_reasons(&store.events_for(root_id).unwrap(), "a");
    assert!(
        reasons.iter().any(|r| r == "cluster_jobs_finished"),
        "{reasons:?}"
    );
    let seen = adapter.seen.lock().unwrap();
    let cont = seen
        .iter()
        .filter(|c| c.work_unit.as_ref().is_some_and(|w| w.key == "a"))
        .nth(1)
        .and_then(|c| c.continuation.clone())
        .expect("the second run of a is a continuation");
    assert_eq!(cont.previous_end, "waiting(cluster_jobs)");
    let jobs = cont.cluster_jobs.expect("cluster job results");
    assert_eq!(jobs.state, "satisfied");
    assert!(
        jobs.jobs[0].starts_with("42634: finished (Exit_status 0"),
        "{jobs:?}"
    );
    drop(seen);
    assert_replay_is_clean(&store);
}
