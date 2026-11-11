//! ADR 2026-11-06-cos-run-launch-accounting-and-drain「実装で試験に固定すること」の横断試験（WU cross）。
//! 起動通知（task-worker の adapter/sink）→ dispatch（launch・枠待ち・回収）→ 案件予算の和（task-ops）が
//! 実経路の契約どおりにつながることを、一時 DB・注入時計・出来事待ち（Semaphore・join）で固定する。
//! LLM・外部ネットワーク・userns・負荷台本は使わない。process を伴う試験は shell の stub。

use super::cos_chat_launch::{join, post, runs, thread};
use super::cos_launch_wire::{
    CLAUDE_OK, LaunchingAdapter, claude_fixture, fake_fixture, fixture_with,
};
use super::cos_slot_wait::{advance, run_of, waiting_notices};
use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::chat::{ChatCostState, ChatMessageQuery, ChatPrelaunchReason, ChatRunState};
use task_core::pricing::{BudgetRate, BudgetRates};
use task_ops::project_budget::{BudgetKind, BudgetLimits, BudgetUsage, exceeded};
use task_worker::claude_code::{ClaudeCodeAdapter, ClaudeCodeConfig};
use tokio::sync::Semaphore;

const CLOCK: i64 = 1_790_000_000;
const KNOWN_USD_MICROS: u64 = 500_000;

fn known_usage() -> task_core::Usage {
    task_core::Usage {
        input_tokens: Some(100),
        output_tokens: Some(20),
        cost_usd: Some(0.5),
        ..Default::default()
    }
}

fn budget(store: &SqliteStore, thread: &str, now: OffsetDateTime) -> BudgetUsage {
    let project = store
        .chat_thread_get(thread)
        .expect("thread")
        .expect("exists")
        .project_id
        .expect("project thread");
    // model `m`（試験の route が選ぶ）の単価。既知 usage（input 100・output 20）が 500_000 micro-USD になる。
    let rates = BudgetRates::new([(
        "m".to_string(),
        BudgetRate {
            input_per_million: 2_000.0,
            output_per_million: 15_000.0,
            cache_read_per_million: 0.0,
            cache_write_per_million: 0.0,
        },
    )])
    .expect("rates");
    task_ops::project_budget::usage(store, &project, now, &rates).expect("usage")
}

fn blocked_by(u: &BudgetUsage) -> Option<BudgetKind> {
    exceeded(&BudgetLimits::default(), u)
}

fn message_count(store: &SqliteStore, thread: &str) -> usize {
    store
        .chat_message_list(
            thread,
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .len()
}

// ---- (1) route 失敗の CoS run は not_launched で案件予算を止めない ----

#[tokio::test]
async fn route_failure_run_is_not_launched_and_does_not_stop_the_project_budget() {
    let mut f = fake_fixture();
    let until = (f.d.now_unix_fn)() + 3600;
    f.d.source_cooldowns.insert(
        "p1".into(),
        crate::accounts::AccountCooldown {
            until,
            reason: crate::accounts::AccountCooldownReason::Throttled,
        },
    );
    let t = f.thread("cross-route");
    let id = f.start(&t, "one");
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Failed, "{run:?}");
    assert!(run.launched_at.is_none());
    assert_eq!(
        run.prelaunch_reason,
        Some(ChatPrelaunchReason::RouteUnavailable)
    );
    assert_eq!(run.cost_state, ChatCostState::NotLaunched);
    let u = budget(&f.store, &t, f.d.now_utc());
    assert_eq!(u.not_launched_runs, 1, "{u:?}");
    assert_eq!(u.usd_unknown_runs, 0, "a not-launched run is not unknown");
    assert_eq!(blocked_by(&u), None, "{u:?}");
}

// ---- (2) 起動通知後の spawn 失敗・usage null は費用不明で止める。既知の usage は止めない ----

#[tokio::test]
async fn spawn_failure_after_the_notice_and_usage_null_stop_the_budget() {
    // spawn 自体の失敗: 通知は済んでいる（launched_at が残る）ので起動前扱いにしない。
    let mut f = fixture_with(
        "claude-code",
        Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
            command: "/nonexistent/claude-for-cross".into(),
            ..Default::default()
        })),
    );
    let t = f.thread("cross-spawnfail");
    let id = f.start(&t, "one");
    f.join(&t).await;
    let run = f.run(&t, &id);
    assert!(run.launched_at.is_some(), "{run:?}");
    assert_eq!(run.prelaunch_reason, None);
    let u = budget(&f.store, &t, f.d.now_utc());
    assert_eq!((u.not_launched_runs, u.usd_unknown_runs), (0, 1), "{u:?}");
    assert_eq!(blocked_by(&u), Some(BudgetKind::UsdUnknown));

    // 起動後の usage null は完了していても費用不明。既知の usage は止めず USD に載る。
    for (usage, unknown, usd, kind) in [
        (None, 1, 0, Some(BudgetKind::UsdUnknown)),
        (Some(known_usage()), 0, KNOWN_USD_MICROS, None),
    ] {
        let mut f = fixture_with("fake", Arc::new(LaunchingAdapter(usage)));
        let t = f.thread("cross-usage");
        let id = f.start(&t, "one");
        f.join(&t).await;
        assert_eq!(f.run(&t, &id).state, ChatRunState::Completed);
        let u = budget(&f.store, &t, f.d.now_utc());
        assert_eq!(
            (u.not_launched_runs, u.usd_unknown_runs, u.usd_micros),
            (0, unknown, usd),
            "{u:?}"
        );
        assert_eq!(blocked_by(&u), kind, "{u:?}");
    }
}

// ---- (5) 通知（launched_at の永続化）に失敗したら spawn しない ----

#[tokio::test]
async fn a_failed_launch_notice_prevents_the_spawn_and_leaves_no_usage() {
    let mut f = claude_fixture(CLAUDE_OK);
    let t = f.thread("cross-notice");
    let id = f.start(&t, "one");
    // launched_at の書き込みが DB エラーになる（Conflict ではなく実際の書き込み失敗）。
    f.sql(
        "CREATE TRIGGER no_launch BEFORE UPDATE OF launched_at ON chat_runs \
         BEGIN SELECT RAISE(ABORT, 'launch notice failed'); END",
    );
    f.join(&t).await;
    assert!(
        !f.workspace(&t).join("spawned.marker").exists(),
        "the stub process must not start"
    );
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Failed, "{run:?}");
    assert!(run.launched_at.is_none(), "{run:?}");
    assert!(run.usage.is_none(), "{run:?}");
    assert_eq!(runs(&f.store, &t).len(), 1, "no retry run");
}

// ---- 枠待ち用の部品: 役割ごとに Semaphore で止まる adapter ----

#[derive(Clone)]
struct Gates {
    worker: Arc<Semaphore>,
    review: Arc<Semaphore>,
    cos: Arc<Semaphore>,
    /// false の間、CoS run は即終わる。true なら `cos` の permit が出るまで起動したまま止まる。
    hold_cos: bool,
}

impl Gates {
    fn new(hold_cos: bool) -> Self {
        Self {
            worker: Arc::new(Semaphore::new(0)),
            review: Arc::new(Semaphore::new(0)),
            cos: Arc::new(Semaphore::new(0)),
            hold_cos,
        }
    }
}

async fn pass(gate: &Semaphore) {
    gate.acquire().await.expect("gate").forget();
}

#[async_trait]
impl WorkerAdapter for Gates {
    fn id(&self) -> &str {
        "fake"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let mut usage = None;
        if req.workspace.to_string_lossy().contains("cos/threads") {
            sink.before_spawn().map_err(AdapterError::Other)?;
            if self.hold_cos {
                pass(&self.cos).await;
            }
            usage = Some(known_usage());
        } else if req.task.kind == TaskKind::Review {
            pass(&self.review).await;
            std::fs::create_dir_all(&req.artifacts_dir).unwrap();
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#,
            )
            .unwrap();
        } else {
            pass(&self.worker).await;
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage,
            },
            exit_code: Some(0),
        })
    }
    fn configure(&self, settings: &task_worker::AdapterSettings) -> task_worker::Configured {
        settings.configured(
            &[
                task_worker::AdapterSetting::Model,
                task_worker::AdapterSetting::Env,
            ],
            || Arc::new(self.clone()),
        )
    }
}

fn arm(d: &mut Dispatcher, store: &Arc<SqliteStore>, dir: &Path, max_cos_runs: usize) {
    d.test_now = Some(Arc::new(StdMutex::new(
        OffsetDateTime::from_unix_timestamp(CLOCK).expect("clock"),
    )));
    d.config.execution.max_cos_runs = max_cos_runs;
    d.config.min_free_disk_mb = 0;
    d.config.knowledge.root = dir.join("knowledge");
    for name in ["cos-operator", "cos-inbox-triage"] {
        let skill_dir = d.config.knowledge.root.join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n# {name}\n"),
        )
        .expect("skill");
    }
    d.set_cos_chat_launch(
        store.clone(),
        CosChatLaunchConfig {
            enabled: true,
            worker_reserve_five_hour: 0.90,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            inline_read_max_commands: 20,
            unavailable_reason: None,
            data_dir: dir.to_path_buf(),
            db_path: dir.join("celeris.db"),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
}

/// 暗黙 account `p1`（max_runs=1）。worker・reviewer・CoS が同じ枠を取り合う。
fn implicit_fixture(
    gates: &Gates,
    max_cos_runs: usize,
) -> (
    crate::test_support::WritableTempDir,
    Arc<SqliteStore>,
    Dispatcher,
) {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("store"));
    let mut d = dispatcher_with_test_clock(store.clone(), Arc::new(gates.clone()), 2, true);
    d.set_account_limit("p1", "default", Some(1)).unwrap();
    d.set_account_limit("implicit", "p1", Some(1)).unwrap();
    arm(&mut d, &store, dir.path(), max_cos_runs);
    (dir, store, d)
}

fn command_task(ws: &Path, check: Check) -> Task {
    new_task(ws, check, 0)
}

/// 待機中の複数 tick で run・入力・失敗が増えず、Waiting の記録は 1 件のまま。
fn assert_still_waiting(d: &mut Dispatcher, store: &SqliteStore, t: &str, run: &str, msgs: usize) {
    for _ in 0..3 {
        advance(d, 1);
        d.tick_cos_chat_launch();
        let again = run_of(store, t);
        assert_eq!(again.id, run, "same run");
        assert_eq!(again.state, ChatRunState::Running);
        assert!(again.launched_at.is_none());
        assert_eq!(message_count(store, t), msgs, "no new input");
        assert_eq!(waiting_notices(store, t, run), 1, "one Waiting record");
        assert_eq!(d.cos_in_flight(), 0, "a waiting run is not counted");
    }
}

/// 枠が空いたあと、`t` の run が起動するまで tick する。ほかの CoS run（task 完了の triage など）は
/// 終わりを待ってから（出来事待ち）次の tick に進む。
async fn launch_after_release(d: &mut Dispatcher, t: &str) {
    for _ in 0..6 {
        let launch = d.cos_chat_launch.as_mut().expect("launch");
        if launch.running.contains_key(t) {
            return;
        }
        let others: Vec<String> = launch.running.keys().cloned().collect();
        for key in others {
            let handle = launch.running.remove(&key).expect("handle");
            handle.await.expect("other CoS run");
        }
        advance(d, 1);
        d.tick_cos_chat_launch();
    }
    assert!(d.cos_chat_launch.as_ref().unwrap().running.contains_key(t));
}

fn assert_launched_with_usage(store: &SqliteStore, t: &str, run: &str, d: &Dispatcher) {
    let done = run_of(store, t);
    assert_eq!(done.id, run, "the same run launched");
    assert_eq!(done.state, ChatRunState::Completed, "{done:?}");
    assert!(done.launched_at.is_some());
    assert_eq!(done.prelaunch_reason, None);
    assert_eq!(done.cost_state, ChatCostState::Known, "{done:?}");
    let u = budget(store, t, d.now_utc());
    assert_eq!(
        (u.usd_micros, u.usd_unknown_runs, u.not_launched_runs),
        (KNOWN_USD_MICROS, 0, 0),
        "usage reached the budget: {u:?}"
    );
    assert_eq!(blocked_by(&u), None);
}

// ---- (3) 枠のみの失敗は待って、次 tick で同じ run が起動する（worker との重なり） ----

#[tokio::test]
async fn a_worker_holding_the_implicit_slot_makes_cos_wait_then_the_same_run_launches() {
    let gates = Gates::new(false);
    let (_dir, store, mut d) = implicit_fixture(&gates, 2);
    let ws = tempfile::tempdir().unwrap();
    let leaf = command_task(
        ws.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&leaf).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1, "the worker holds p1");
    assert_eq!(d.implicit_account_in_use("p1"), 1);
    // 暗黙 account は CoS の +1 が無い（実 account なら載る状況でも待つ）。
    assert_eq!(
        d.implicit_account_limit("p1").map(|(n, _)| n),
        Some(1),
        "effective_max_runs"
    );

    let t = thread(&store, "cross-worker");
    post(&store, &t, "b1");
    d.tick_cos_chat_launch();
    let waiting = run_of(&store, &t);
    assert_eq!(waiting.state, ChatRunState::Running);
    assert!(waiting.launched_at.is_none());
    assert_eq!(waiting_notices(&store, &t, &waiting.id), 1);
    let msgs = message_count(&store, &t);
    assert_still_waiting(&mut d, &store, &t, &waiting.id, msgs);

    gates.worker.add_permits(1);
    await_worker_completion(&mut d, leaf.id).await;
    d.tick().unwrap();
    launch_after_release(&mut d, &t).await;
    join(&mut d, &t).await;
    assert_launched_with_usage(&store, &t, &waiting.id, &d);
    assert_eq!(runs(&store, &t).len(), 1, "no failed or extra run");
}

// ---- 同（reviewer との重なり） ----

#[tokio::test]
async fn a_reviewer_holding_the_implicit_slot_makes_cos_wait_then_the_same_run_launches() {
    let gates = Gates::new(false);
    let (_dir, store, mut d) = implicit_fixture(&gates, 2);
    let ws = tempfile::tempdir().unwrap();
    let leaf = command_task(ws.path(), Check::Reviewer);
    store.insert(&leaf).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    gates.worker.add_permits(1);
    await_worker_completion(&mut d, leaf.id).await;
    d.tick().unwrap();
    assert!(
        d.reviewing
            .get(&leaf.id)
            .is_some_and(|review| review.route.is_some()),
        "the reviewer run holds a route"
    );
    assert_eq!(d.implicit_account_in_use("p1"), 1);

    let t = thread(&store, "cross-reviewer");
    post(&store, &t, "b1");
    d.tick_cos_chat_launch();
    let waiting = run_of(&store, &t);
    assert!(waiting.launched_at.is_none());
    assert_eq!(waiting_notices(&store, &t, &waiting.id), 1);
    let msgs = message_count(&store, &t);
    assert_still_waiting(&mut d, &store, &t, &waiting.id, msgs);

    gates.review.add_permits(1);
    (&mut d.reviewing.get_mut(&leaf.id).expect("review").handle)
        .await
        .expect("review task");
    d.tick().unwrap();
    launch_after_release(&mut d, &t).await;
    join(&mut d, &t).await;
    assert_launched_with_usage(&store, &t, &waiting.id, &d);
}

// ---- (3) 実 account は全 in_use と effective_max_runs+1 を比べる ----

#[tokio::test]
async fn a_real_account_admits_cos_at_the_limit_plus_one_and_waits_beyond_it() {
    let gates = Gates::new(true);
    let dir = crate::test_support::WritableTempDir::new();
    let accounts = tempfile::tempdir().unwrap();
    let acct = accounts.path().join("a");
    std::fs::create_dir_all(&acct).unwrap();
    std::fs::write(acct.join(".credentials.json"), "{}").unwrap();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("store"));
    let as_dyn: Arc<dyn TaskStore> = store.clone();
    let mut d = pool_dispatcher(
        as_dyn,
        Arc::new(gates.clone()),
        None,
        accounts.path().to_path_buf(),
        2,
        4,
    );
    arm(&mut d, &store, dir.path(), 2);
    let ws = tempfile::tempdir().unwrap();
    let mut leaves = Vec::new();
    for _ in 0..2 {
        let leaf = command_task(
            ws.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        );
        store.insert(&leaf).unwrap();
        leaves.push(leaf);
    }
    for _ in 0..4 {
        d.tick().unwrap();
    }
    let in_use = |d: &mut Dispatcher| {
        let (_, _, _, items) = d.accounts_snapshot();
        items.into_iter().find(|a| a.id == "a").expect("a").in_use
    };
    assert_eq!(in_use(&mut d), 2, "two workers fill max_runs_per_account");

    // 上限（1）を下回らせると、全 in_use 2 は CoS の上限 1+1=2 に達して待つ。
    d.set_account_limit("claude-code", "a", Some(1)).unwrap();
    let t = thread(&store, "cross-real");
    post(&store, &t, "b1");
    d.tick_cos_chat_launch();
    let waiting = run_of(&store, &t);
    assert!(waiting.launched_at.is_none(), "full: in_use 2 >= 1+1");
    assert_eq!(waiting_notices(&store, &t, &waiting.id), 1);
    let msgs = message_count(&store, &t);
    assert_still_waiting(&mut d, &store, &t, &waiting.id, msgs);
    assert_eq!(in_use(&mut d), 2);

    // 上限を 2 にすると CoS は +1 の枠（in_use 2 < 2+1）で、待っていた同じ run が起動する。
    d.set_account_limit("claude-code", "a", Some(2)).unwrap();
    advance(&d, 1);
    d.tick_cos_chat_launch();
    assert!(d.cos_chat_launch.as_ref().unwrap().running.contains_key(&t));
    assert_eq!(run_of(&store, &t).id, waiting.id);
    gates.cos.add_permits(1);
    join(&mut d, &t).await;
    assert_launched_with_usage(&store, &t, &waiting.id, &d);
}

// ---- (3) max_cos_runs の待ち ----

#[tokio::test]
async fn max_cos_runs_makes_the_next_input_wait_without_a_failed_run() {
    let gates = Gates::new(true);
    let (_dir, store, mut d) = implicit_fixture(&gates, 1);
    // p1 は暗黙 account の上限 1。CoS が 1 件起動すると max_cos_runs=1 に達する。
    let first = thread(&store, "cross-cos-1");
    post(&store, &first, "a1");
    d.tick_cos_chat_launch();
    assert!(
        d.cos_chat_launch
            .as_ref()
            .unwrap()
            .running
            .contains_key(&first)
    );
    let second = thread(&store, "cross-cos-2");
    post(&store, &second, "b1");
    for _ in 0..3 {
        advance(&d, 1);
        d.tick_cos_chat_launch();
        assert!(
            runs(&store, &second).is_empty(),
            "queued, not claimed and not failed"
        );
        assert_eq!(message_count(&store, &second), 1);
    }
    gates.cos.add_permits(1);
    join(&mut d, &first).await;
    advance(&d, 1);
    d.tick_cos_chat_launch();
    let launched = run_of(&store, &second);
    assert!(
        d.cos_chat_launch
            .as_ref()
            .unwrap()
            .running
            .contains_key(&second)
    );
    gates.cos.add_permits(1);
    join(&mut d, &second).await;
    assert_launched_with_usage(&store, &second, &launched.id, &d);
}

// ---- (4) 再起動回収: 枠待ちの run は起動前（daemon_restart_before_launch）で閉じ、入力は失われない ----

#[tokio::test]
async fn a_restart_while_waiting_closes_before_launch_and_the_input_launches_later() {
    let gates = Gates::new(false);
    let (_dir, store, mut d) = implicit_fixture(&gates, 2);
    let ws = tempfile::tempdir().unwrap();
    let leaf = command_task(
        ws.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&leaf).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    let t = thread(&store, "cross-restart");
    post(&store, &t, "b1");
    d.tick_cos_chat_launch();
    let waiting = run_of(&store, &t);
    assert!(waiting.launched_at.is_none());

    // 再起動: CoS 起動側の表（枠待ちの run 一覧）は空になり、前の持ち主の lease は切れている。
    let config = d.cos_chat_launch.as_ref().unwrap().config.clone();
    d.set_cos_chat_launch(store.clone(), config);
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "this-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    advance(&d, 7200);
    d.tick_cos_chat_launch();
    let closed = store.chat_run_get(&t, &waiting.id).expect("run");
    assert_eq!(closed.state, ChatRunState::Interrupted, "{closed:?}");
    assert_eq!(
        closed.prelaunch_reason,
        Some(ChatPrelaunchReason::DaemonRestartBeforeLaunch)
    );
    assert_eq!(closed.cost_state, ChatCostState::NotLaunched);
    let u = budget(&store, &t, d.now_utc());
    assert_eq!((u.not_launched_runs, u.usd_unknown_runs), (1, 0), "{u:?}");
    assert_eq!(blocked_by(&u), None, "{u:?}");

    // 入力は積み直され、枠が空いたあとで新しい run として起動し usage が予算に載る。
    gates.worker.add_permits(1);
    await_worker_completion(&mut d, leaf.id).await;
    d.tick().unwrap();
    launch_after_release(&mut d, &t).await;
    let all = runs(&store, &t);
    assert_eq!(all.len(), 2, "one closed run and one relaunch: {all:?}");
    join(&mut d, &t).await;
    let relaunched = runs(&store, &t)
        .into_iter()
        .find(|r| r.id != waiting.id)
        .expect("relaunch");
    assert!(relaunched.launched_at.is_some(), "{relaunched:?}");
    let u = budget(&store, &t, d.now_utc());
    assert_eq!(
        (u.usd_micros, u.usd_unknown_runs, u.not_launched_runs),
        (KNOWN_USD_MICROS, 0, 1),
        "{u:?}"
    );
    assert_eq!(blocked_by(&u), None);
}
