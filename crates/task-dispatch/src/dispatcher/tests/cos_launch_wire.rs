//! ADR 2026-11-06-cos-run-launch-accounting-and-drain D1・D4（launch-wire）: CoS chat run の起動境界
//! （`launched_at`・`prelaunch_reason`）・attempt ごとの usage 記録・人の stop と再起動回収を
//! dispatcher の実経路で固定する。時刻は注入時計、worker の終わりは handle の join で待つ。
//! process を伴う試験は shell の stub（LLM・外部ネットワークなし）。

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::chat::{
    ChatCostState, ChatPostMessageRequest, ChatPrelaunchReason, ChatRun, ChatRunState, ChatSendMode,
};
use task_worker::claude_code::{ClaudeCodeAdapter, ClaudeCodeConfig};

const TEXT_THEN_DONE: &str = "cat >/dev/null; \
     echo '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'";

struct Fixture {
    dir: crate::test_support::WritableTempDir,
    db_path: PathBuf,
    store: Arc<SqliteStore>,
    d: Dispatcher,
    /// Keeps the stub command's directory alive for the whole test.
    _stub: Option<crate::test_support::WritableTempDir>,
}

fn stub(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\nset -eu\n{body}\n"));
    path.to_string_lossy().into_owned()
}

fn fixture_with(harness: &str, adapter: Arc<dyn WorkerAdapter>) -> Fixture {
    let dir = crate::test_support::WritableTempDir::new();
    let db_path = dir.path().join("celeris.db");
    let store = Arc::new(SqliteStore::open(&db_path).expect("store"));
    let mut d = dispatcher_with_test_clock(store.clone(), adapter, 2, true);
    d.test_now = Some(Arc::new(StdMutex::new(
        OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("clock"),
    )));
    d.config.execution.max_cos_runs = 1;
    d.config.min_free_disk_mb = 0;
    d.config.knowledge.root = dir.path().join("knowledge");
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
            harness: harness.into(),
            max_turns: 2,
            max_wall_secs: 30,
            inline_read_max_commands: 20,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: db_path.clone(),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "this-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    Fixture {
        dir,
        db_path,
        store,
        d,
        _stub: None,
    }
}

fn fake_fixture() -> Fixture {
    fixture_with(
        "fake",
        Arc::new(task_worker::fake::FakeAdapter::new(vec![
            "sh".into(),
            "-c".into(),
            TEXT_THEN_DONE.into(),
        ])),
    )
}

impl Fixture {
    fn thread(&self, key: &str) -> String {
        crate::test_support::project_cos_thread(&self.store, key)
    }

    fn post(&self, thread: &str, key: &str) {
        self.store
            .chat_message_post(
                thread,
                &ChatPostMessageRequest {
                    client_message_id: key.into(),
                    text: key.into(),
                    attachment_ids: vec![],
                    reply_to_id: None,
                    mode: ChatSendMode::Queue,
                    resume_queue: false,
                },
                self.d.now_utc(),
            )
            .expect("post");
    }

    fn tick(&mut self) {
        if let Some(clock) = &self.d.test_now {
            *clock.lock().expect("clock") += time::Duration::seconds(1);
        }
        self.d.tick_cos_chat_launch();
    }

    fn expire_leases(&mut self) {
        if let Some(clock) = &self.d.test_now {
            *clock.lock().expect("clock") += time::Duration::hours(2);
        }
    }

    fn sql(&self, sql: &str) {
        rusqlite::Connection::open(&self.db_path)
            .expect("db")
            .execute(sql, [])
            .expect("sql");
    }

    /// The run ids of a thread, oldest first.
    fn run_ids(&self, thread: &str) -> Vec<String> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let mut stmt = conn
            .prepare("SELECT run_id FROM chat_runs WHERE thread_id=?1 ORDER BY started_at, run_id")
            .expect("prepare");
        stmt.query_map([thread], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<String>, _>>()
            .expect("rows")
    }

    fn run(&self, thread: &str, id: &str) -> ChatRun {
        self.store.chat_run_get(thread, id).expect("run")
    }

    async fn join(&mut self, thread: &str) {
        let handle = self
            .d
            .cos_chat_launch
            .as_mut()
            .expect("launch")
            .running
            .remove(thread)
            .expect("worker launched");
        tokio::time::timeout(std::time::Duration::from_secs(30), handle)
            .await
            .expect("worker deadline")
            .expect("worker join");
    }

    /// Claim and start the queued message of `thread`; returns the run id (the worker has not polled yet).
    fn start(&mut self, thread: &str, key: &str) -> String {
        self.post(thread, key);
        self.tick();
        self.run_ids(thread).pop().expect("claimed run")
    }

    fn workspace(&self, thread: &str) -> PathBuf {
        self.dir
            .path()
            .join("cos/threads")
            .join(thread)
            .join("workspace")
    }
}

fn claude_fixture(script: &str) -> Fixture {
    let dir = crate::test_support::WritableTempDir::new();
    let command = stub(dir.path(), "claude.sh", script);
    let adapter = Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
        command,
        ..Default::default()
    }));
    let mut f = fixture_with("claude-code", adapter);
    f._stub = Some(dir);
    f
}

const CLAUDE_OK: &str = r#"
echo spawned >> spawned.marker
cat > /dev/null
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":"hi"}'
"#;

// ---- 受け入れ 0: 起動直前に launched_at、書けなければ spawn しない ----

#[tokio::test]
async fn launched_at_is_persisted_before_the_process_spawns() {
    let mut f = claude_fixture(CLAUDE_OK);
    let t = f.thread("lw-launch");
    let id = f.start(&t, "one");
    let before = f.run(&t, &id);
    assert!(
        before.launched_at.is_none(),
        "nothing spawned yet: {before:?}"
    );
    let at = f.d.now_utc();
    f.join(&t).await;
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Completed, "{run:?}");
    assert!(f.workspace(&t).join("spawned.marker").is_file());
    let launched = run.launched_at.clone().expect("launched_at recorded");
    let parsed =
        OffsetDateTime::parse(&launched, &time::format_description::well_known::Rfc3339).unwrap();
    assert_eq!(parsed, at, "the injected clock decides launched_at");
    assert_eq!(run.prelaunch_reason, None);
}

#[tokio::test]
async fn a_launch_record_failure_prevents_the_spawn() {
    let mut f = claude_fixture(CLAUDE_OK);
    let t = f.thread("lw-nospawn");
    let id = f.start(&t, "one");
    // The row cannot record its launch boundary: mark_launched answers Conflict.
    f.sql(&format!(
        "UPDATE chat_runs SET launch_tracked=0 WHERE run_id='{id}'"
    ));
    f.join(&t).await;
    assert!(
        !f.workspace(&t).join("spawned.marker").exists(),
        "the stub process must not start"
    );
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Failed, "{run:?}");
    assert!(run.launched_at.is_none(), "{run:?}");
}

#[tokio::test]
async fn a_spawn_failure_keeps_launched_at() {
    let mut f = fixture_with(
        "claude-code",
        Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
            command: "/nonexistent/claude-for-launch-wire".into(),
            ..Default::default()
        })),
    );
    let t = f.thread("lw-spawnfail");
    let id = f.start(&t, "one");
    f.join(&t).await;
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Failed, "{run:?}");
    assert!(run.launched_at.is_some(), "may have started: {run:?}");
    assert_eq!(run.prelaunch_reason, None);
    assert_eq!(run.cost_state, ChatCostState::Unknown);
}

// ---- 受け入れ 1: 起動前に閉じる経路は prelaunch、起動後の usage null は閉じない ----

#[tokio::test]
async fn route_failure_closes_prelaunch_and_counts_as_not_launched() {
    let mut f = fake_fixture();
    f.d.cos_chat_launch
        .as_mut()
        .unwrap()
        .config
        .unavailable_reason = Some("CoS unavailable (test)".into());
    let t = f.thread("lw-route");
    let id = f.start(&t, "one");
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Failed, "{run:?}");
    assert!(run.launched_at.is_none());
    assert_eq!(
        run.prelaunch_reason,
        Some(ChatPrelaunchReason::RouteUnavailable)
    );
    assert_eq!(run.cost_state, ChatCostState::NotLaunched);
    assert!(run.usage.is_none());
}

#[tokio::test]
async fn missing_skills_close_prelaunch_as_skills_unavailable() {
    let mut f = fake_fixture();
    std::fs::remove_dir_all(f.d.config.knowledge.root.join("skills")).expect("remove skills");
    let t = f.thread("lw-skills");
    let id = f.start(&t, "one");
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Failed, "{run:?}");
    assert_eq!(
        run.prelaunch_reason,
        Some(ChatPrelaunchReason::SkillsUnavailable)
    );
    assert_eq!(run.cost_state, ChatCostState::NotLaunched);
}

/// A harness that launches (sends the pre-spawn notice) and then ends with the given usage.
struct LaunchingAdapter(Option<task_core::Usage>);

#[async_trait]
impl WorkerAdapter for LaunchingAdapter {
    fn id(&self) -> &str {
        "fake"
    }

    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        sink.before_spawn().map_err(AdapterError::Other)?;
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: self.0,
            },
            exit_code: Some(0),
        })
    }

    fn with_env(&self, _extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(LaunchingAdapter(self.0)))
    }
}

#[tokio::test]
async fn usage_null_after_launch_stays_unknown_and_known_usage_is_known() {
    for (usage, expected) in [
        (None, ChatCostState::Unknown),
        (
            Some(task_core::Usage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                cost_usd: Some(0.5),
                ..Default::default()
            }),
            ChatCostState::Known,
        ),
    ] {
        let mut f = fixture_with("fake", Arc::new(LaunchingAdapter(usage)));
        let t = f.thread("lw-usage");
        let id = f.start(&t, "one");
        f.join(&t).await;
        let run = f.run(&t, &id);
        assert_eq!(run.state, ChatRunState::Completed, "{run:?}");
        assert!(run.launched_at.is_some());
        assert_eq!(
            run.prelaunch_reason, None,
            "a launched run is never prelaunch"
        );
        assert_eq!(run.cost_state, expected, "{run:?}");
        assert_eq!(
            run.usage.and_then(|u| u.input_tokens),
            usage.and_then(|u| u.input_tokens)
        );
    }
}

// ---- 受け入れ 3: attempt の欠測が記録される ----

#[tokio::test]
async fn a_missing_attempt_usage_is_recorded_and_hides_the_known_part() {
    use crate::dispatcher::cos_chat::sink::{ChatFinish, ChatRunSink};
    use task_worker::adapter::EventSink as _;
    let f = fake_fixture();
    let t = f.thread("lw-attempt");
    f.post(&t, "one");
    let now = f.d.now_utc();
    let run = f
        .store
        .chat_run_claim_next(&t, "attempt-run", &serde_json::json!({}), now)
        .expect("claim")
        .expect("run");
    let clock: crate::dispatcher::cos_chat::sink::ChatClock = Arc::new(move || now);
    let sink = |_: u8| ChatRunSink::new(f.store.clone(), &t, &run.id, clock.clone(), vec![]);
    let known = task_core::Usage {
        input_tokens: Some(100),
        output_tokens: Some(20),
        cost_usd: Some(0.25),
        ..Default::default()
    };
    // An attempt whose process never launched records nothing (no attempt happened).
    let idle = sink(0);
    idle.record_attempt_usage(Some(known));
    let first = sink(1);
    first.before_spawn().expect("launch notice");
    first.record_attempt_usage(Some(known));
    let second = sink(2);
    second
        .before_spawn()
        .expect("second launch keeps the first time");
    second.record_attempt_usage(None);
    let done = second
        .finish(
            &ChatFinish {
                state: ChatRunState::Completed,
                final_text: Some("ok".into()),
                reason: None,
                context_exhausted: false,
            },
            &task_worker::result_report::ParsedActions::default(),
        )
        .expect("finish")
        .expect("run");
    assert_eq!(done.cost_state, ChatCostState::Unknown, "{done:?}");
    assert_eq!(
        done.usage.and_then(|u| u.input_tokens),
        Some(100),
        "the known attempt's usage is kept"
    );
}

// ---- 受け入れ 2: 再起動回収の分岐と人の stop ----

#[tokio::test]
async fn restart_recovery_splits_unlaunched_launched_and_legacy_rows() {
    let mut f = fake_fixture();
    let t = f.thread("lw-recover");
    f.post(&t, "original");
    let now = f.d.now_utc();
    let mut ids = Vec::new();
    for (i, kind) in ["unlaunched", "launched", "legacy"].iter().enumerate() {
        let thread = f.thread(&format!("lw-recover-{kind}"));
        f.post(&thread, "original");
        let run = f
            .store
            .chat_run_claim_next(
                &thread,
                &format!("recover-{i}"),
                &serde_json::json!({}),
                now,
            )
            .expect("claim")
            .expect("run");
        match *kind {
            "launched" => {
                f.store
                    .chat_run_mark_launched(&run.id, now)
                    .expect("launch");
            }
            "legacy" => f.sql(&format!(
                "UPDATE chat_runs SET launch_tracked=0 WHERE run_id='{}'",
                run.id
            )),
            _ => {}
        }
        ids.push((thread, run.id));
    }
    f.expire_leases();
    f.tick();
    let get = |i: usize| f.run(&ids[i].0, &ids[i].1);
    let unlaunched = get(0);
    assert_eq!(
        unlaunched.state,
        ChatRunState::Interrupted,
        "{unlaunched:?}"
    );
    assert_eq!(
        unlaunched.prelaunch_reason,
        Some(ChatPrelaunchReason::DaemonRestartBeforeLaunch)
    );
    assert_eq!(unlaunched.cost_state, ChatCostState::NotLaunched);
    for (i, label) in [(1, "launched"), (2, "legacy")] {
        let run = get(i);
        assert_eq!(run.state, ChatRunState::Interrupted, "{label}: {run:?}");
        assert_eq!(run.prelaunch_reason, None, "{label}: {run:?}");
        assert_eq!(run.cost_state, ChatCostState::Unknown, "{label}: {run:?}");
    }
}

#[tokio::test]
async fn human_stop_before_launch_closes_as_stopped_before_launch() {
    let mut f = fake_fixture();
    let t = f.thread("lw-stop");
    let id = f.start(&t, "one");
    // The stop is committed between the claim and the worker's first poll.
    f.store
        .chat_run_stop(&t, &id, f.d.now_utc())
        .expect("stop accepted");
    f.join(&t).await;
    let run = f.run(&t, &id);
    assert_eq!(run.state, ChatRunState::Stopped, "{run:?}");
    assert!(run.launched_at.is_none());
    assert_eq!(
        run.prelaunch_reason,
        Some(ChatPrelaunchReason::StoppedBeforeLaunch)
    );
    assert_eq!(run.cost_state, ChatCostState::NotLaunched);
}

#[tokio::test]
async fn human_stop_of_an_ownerless_unlaunched_run_is_stopped_before_launch() {
    let mut f = fake_fixture();
    let t = f.thread("lw-stop-orphan");
    f.post(&t, "one");
    let now = f.d.now_utc();
    let run = f
        .store
        .chat_run_claim_next(&t, "stop-orphan", &serde_json::json!({}), now)
        .expect("claim")
        .expect("run");
    f.store.chat_run_stop(&t, &run.id, now).expect("stop");
    f.expire_leases();
    f.tick();
    let run = f.run(&t, &run.id);
    assert_eq!(run.state, ChatRunState::Stopped, "{run:?}");
    assert_eq!(
        run.prelaunch_reason,
        Some(ChatPrelaunchReason::StoppedBeforeLaunch)
    );
}
