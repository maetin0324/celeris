//! ADR 2026-10-27-cron-cos-review: a `cos_review` cron job queues one input per scheduled time.
//! ADR 2026-10-28-cos-review-back-thread D2: the input goes to the review (back) thread, never to
//! the general CoS thread. Injected clock; most tests leave no CoS capacity, so no run (and no LLM)
//! is started and they only see what the dispatcher itself wrote. The run-context test starts a
//! fake-harness run (a shell script, no LLM).

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use crate::dispatcher::cos_chat::review::review_key;
use task_core::chat::{
    ChatMessageRole, ChatMessageState, ChatOpenThreadRequest, ChatPostMessageRequest, ChatRunState,
    ChatSendMode,
};
use task_core::{
    Project, ProjectBrief, ProjectBriefInput, ProjectId, ProjectStatus, WaitingOn, WaitingOnKind,
};
use task_ops::cron_jobs::{CronFireContext, NewCronJob, create_job};
use task_ops::stagnation::{StagnationFinding, StagnationKind};
use task_worker::fake::FakeAdapter;
use time::format_description::well_known::Rfc3339;

fn at(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap()
}

/// One daemon instance on the shared DB. A second call is a restart.
fn daemon(
    dir: &crate::test_support::WritableTempDir,
    store: &Arc<SqliteStore>,
    clock: &Arc<StdMutex<OffsetDateTime>>,
) -> Dispatcher {
    // The worker command would fail if it ran. No worker slot and max_cos_runs = 0 (CoS shares
    // the worker slots) leave no CoS capacity, so it never does.
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec!["false".into()]));
    daemon_with(dir, store, clock, adapter, 0)
}

fn daemon_with(
    dir: &crate::test_support::WritableTempDir,
    store: &Arc<SqliteStore>,
    clock: &Arc<StdMutex<OffsetDateTime>>,
    adapter: Arc<dyn WorkerAdapter>,
    slots: usize,
) -> Dispatcher {
    let mut d = dispatcher(store.clone(), adapter, slots);
    d.test_now = Some(clock.clone());
    d.config.execution.max_cos_runs = 0;
    d.config.min_free_disk_mb = 0;
    // Fixed findings: the scan is kept from replacing them (its last check is in the future).
    d.stagnation.checked_at = Some(at("2100-01-01T00:00:00Z"));
    d.stagnation.findings = vec![StagnationFinding {
        kind: StagnationKind::ReleaseUnpromoted,
        key: "release_unpromoted-selfdeploy".into(),
        project_id: None,
        task_id: None,
        subject: "abcdef123456".into(),
        since: "2026-10-27T00:10:00Z".into(),
        detail: "verify 済みで昇格されていない".into(),
    }];
    d.set_cos_chat_launch(
        store.clone(),
        CosChatLaunchConfig {
            enabled: true,
            worker_reserve_five_hour: 0.90,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: dir.path().join("celeris.db"),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
    d
}

fn tick_at(d: &mut Dispatcher, clock: &Arc<StdMutex<OffsetDateTime>>, value: &str) {
    *clock.lock().unwrap() = at(value);
    d.tick().unwrap();
}

fn review_job(store: &SqliteStore) -> CronJob {
    let mut template = CronTaskTemplate {
        title: "定期見直し".into(),
        ..CronTaskTemplate::default()
    };
    template
        .extra
        .insert("kind".into(), serde_json::json!("cos_review"));
    create_job(
        store,
        &CronFireContext::default(),
        NewCronJob {
            name: "cos-review".into(),
            schedule: "0 * * * *".into(),
            timezone: "UTC".into(),
            overlap: CronOverlap::Skip,
            catch_up: CronCatchUp::Latest,
            enabled: true,
            template,
        },
        at("2026-10-27T00:00:00Z"),
    )
    .unwrap()
}

fn project_with_brief(store: &SqliteStore, title: &str, updated: OffsetDateTime) -> Project {
    let p = Project {
        id: ProjectId::new(),
        title: title.into(),
        request: "依頼".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        auto_integrate: false,
        created_at: updated,
        updated_at: updated,
    };
    store.project_create(&p).unwrap();
    let brief = ProjectBrief::from_input(
        p.id,
        ProjectBriefInput {
            objective: "目的".into(),
            state: "計画の承認待ち".into(),
            open_issues: vec![],
            next_actions: vec![],
            waiting_on: vec![WaitingOn {
                kind: WaitingOnKind::Human,
                target: None,
                note: "計画の承認".into(),
            }],
            evidence_refs: vec![],
        },
        "cos:t",
        updated,
    );
    assert!(store.project_brief_put(&brief).unwrap());
    p
}

fn general_thread(store: &SqliteStore) -> String {
    store
        .cos_thread_open(
            &ChatOpenThreadRequest { project_id: None },
            at("2026-10-27T00:00:00Z"),
        )
        .unwrap()
        .thread
        .id
}

fn review_thread(store: &SqliteStore) -> String {
    store
        .cos_review_thread_get()
        .unwrap()
        .expect("review thread")
        .id
}

/// User inputs of the review thread, oldest first: (client key, text, state). Empty before the
/// review thread exists.
fn inputs(store: &SqliteStore) -> Vec<(String, String, ChatMessageState)> {
    let Some(thread) = store.cos_review_thread_get().unwrap() else {
        return Vec::new();
    };
    store
        .chat_message_list(&thread.id, &Default::default())
        .unwrap()
        .items
        .into_iter()
        .filter(|m| m.role == ChatMessageRole::User)
        .map(|m| (m.client_message_id.unwrap_or_default(), m.text, m.state))
        .collect()
}

/// Every message of the general (person's) thread: the review never writes one.
fn general_messages(store: &SqliteStore) -> usize {
    let thread = general_thread(store);
    store
        .chat_message_list(&thread, &Default::default())
        .unwrap()
        .items
        .len()
}

fn outcomes(store: &SqliteStore, job: &CronJob) -> Vec<(String, CronRunOutcome)> {
    let mut runs = store.cron_job_runs(job.id, None).unwrap();
    runs.sort_by_key(|r| r.scheduled_for);
    runs.into_iter()
        .map(|r| (r.scheduled_for.format(&Rfc3339).unwrap(), r.outcome))
        .collect()
}

fn chat_runs(dir: &crate::test_support::WritableTempDir) -> i64 {
    rusqlite::Connection::open(dir.path().join("celeris.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM chat_runs", [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn cos_review_queues_one_input_per_period_and_a_restart_does_not_duplicate_it() {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).unwrap());
    let clock = Arc::new(StdMutex::new(at("2026-10-27T00:59:00Z")));
    let job = review_job(&store);
    let project = project_with_brief(&store, "案件A", at("2026-10-27T00:30:00Z"));
    let mut d = daemon(&dir, &store, &clock);

    tick_at(&mut d, &clock, "2026-10-27T00:59:59Z");
    assert!(inputs(&store).is_empty(), "not before the scheduled time");
    assert!(store.cos_review_thread_get().unwrap().is_none());

    tick_at(&mut d, &clock, "2026-10-27T01:00:00Z");
    let first = inputs(&store);
    assert_eq!(first.len(), 1, "{first:?}");
    let first_key = review_key(job.id, at("2026-10-27T01:00:00Z"));
    assert_eq!(first[0].0, first_key);
    assert_eq!(first[0].2, ChatMessageState::Queued);
    let text = &first[0].1;
    assert!(
        text.starts_with("[定期見直し] 定期の見直しです。"),
        "{text}"
    );
    assert!(text.contains("前回の見直し なし"), "{text}");
    assert!(text.contains(&format!("案件A ({})", project.id)), "{text}");
    assert!(text.contains("human: 計画の承認"), "{text}");
    assert!(
        text.contains("release_unpromoted abcdef123456 (since 2026-10-27T00:10:00Z)"),
        "{text}"
    );
    assert_eq!(
        outcomes(&store, &job),
        vec![("2026-10-27T01:00:00Z".into(), CronRunOutcome::Created)]
    );
    // The dispatcher only queued the input: no CoS run was started, and the person's general
    // thread got neither the input nor a reply.
    assert_eq!(chat_runs(&dir), 0);
    assert_eq!(general_messages(&store), 0);

    // Later ticks of the same period add nothing.
    tick_at(&mut d, &clock, "2026-10-27T01:00:30Z");
    assert_eq!(inputs(&store).len(), 1);

    // Restart inside the crash window (input posted, history and next_fire_at not written):
    // the new daemon fires the same scheduled time again and finds the same input.
    drop(d);
    rusqlite::Connection::open(dir.path().join("celeris.db"))
        .unwrap()
        .execute("DELETE FROM cron_job_runs", [])
        .unwrap();
    let mut rewound = store.cron_job_get(job.id).unwrap().unwrap();
    rewound.next_fire_at = Some(at("2026-10-27T01:00:00Z"));
    store.cron_job_update(&rewound).unwrap();
    let mut d = daemon(&dir, &store, &clock);
    tick_at(&mut d, &clock, "2026-10-27T01:01:00Z");
    assert_eq!(inputs(&store).len(), 1, "{:?}", inputs(&store));
    let runs = store.cron_job_runs(job.id, None).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, CronRunOutcome::Created);
    assert!(
        runs[0].detail.as_deref().unwrap_or("").contains("already"),
        "{runs:?}"
    );

    // The next period while the first input is still queued: skipped, not stacked.
    tick_at(&mut d, &clock, "2026-10-27T02:00:00Z");
    assert_eq!(inputs(&store).len(), 1);
    assert_eq!(
        outcomes(&store, &job).last().unwrap(),
        &(
            "2026-10-27T02:00:00Z".into(),
            CronRunOutcome::SkippedOverlap
        )
    );

    // Once the first input left the queue, the next period queues a second input; the brief
    // did not change since the previous review, so it is not listed again.
    let thread = review_thread(&store);
    let first_id = store
        .chat_message_by_client_id(&thread, &first_key)
        .unwrap()
        .unwrap()
        .id;
    store
        .chat_message_cancel(&thread, &first_id, at("2026-10-27T02:10:00Z"))
        .unwrap();
    tick_at(&mut d, &clock, "2026-10-27T03:00:00Z");
    let all = inputs(&store);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1].0, review_key(job.id, at("2026-10-27T03:00:00Z")));
    assert!(
        all[1].1.contains("前回の見直し 2026-10-27T01:00:00Z"),
        "{}",
        all[1].1
    );
    assert!(
        all[1].1.contains("要約が変わった案件（0 件）"),
        "{}",
        all[1].1
    );

    // A plain restart after the period was recorded adds nothing either.
    drop(d);
    let mut d = daemon(&dir, &store, &clock);
    tick_at(&mut d, &clock, "2026-10-27T03:00:30Z");
    assert_eq!(inputs(&store).len(), 2);
    assert_eq!(chat_runs(&dir), 0);
    assert_eq!(general_messages(&store), 0);
}

#[tokio::test]
async fn cos_review_without_cos_chat_records_an_error_and_posts_nothing() {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).unwrap());
    let clock = Arc::new(StdMutex::new(at("2026-10-27T00:59:00Z")));
    let job = review_job(&store);
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec!["false".into()]));
    let mut d = dispatcher(store.clone(), adapter, 2);
    d.test_now = Some(clock.clone());
    d.config.min_free_disk_mb = 0;
    tick_at(&mut d, &clock, "2026-10-27T01:00:00Z");
    assert!(inputs(&store).is_empty());
    assert!(store.cos_review_thread_get().unwrap().is_none());
    let runs = store.cron_job_runs(job.id, None).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, CronRunOutcome::Error);
    assert!(
        runs[0]
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("not configured"),
        "{runs:?}"
    );
}

/// D4: only the review thread's queue holds a review back. A person's queued message in the general
/// thread does not make the review `skipped_overlap`.
#[tokio::test]
async fn cos_review_is_not_held_back_by_the_persons_queued_message_in_the_general_thread() {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).unwrap());
    let clock = Arc::new(StdMutex::new(at("2026-10-27T00:59:00Z")));
    let job = review_job(&store);
    let general = general_thread(&store);
    store
        .chat_message_post(
            &general,
            &ChatPostMessageRequest {
                client_message_id: "human-1".into(),
                text: "進捗は?".into(),
                attachment_ids: Vec::new(),
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            at("2026-10-27T00:59:30Z"),
        )
        .unwrap();
    let mut d = daemon(&dir, &store, &clock);
    tick_at(&mut d, &clock, "2026-10-27T01:00:00Z");
    assert_eq!(
        outcomes(&store, &job),
        vec![("2026-10-27T01:00:00Z".into(), CronRunOutcome::Created)]
    );
    assert_eq!(inputs(&store).len(), 1);
    // The general thread still holds only the person's message.
    assert_eq!(general_messages(&store), 1);
}

/// A person's stop of a review run pauses the review thread's queue, and nobody may resume it
/// (`resume-queue` is 403 on the review thread). The next period still queues its review: an input
/// left waiting is cancelled and the new input, which covers its period too, resumes the queue.
#[tokio::test]
async fn cos_review_queues_the_next_period_after_a_person_stopped_a_review_run() {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).unwrap());
    let clock = Arc::new(StdMutex::new(at("2026-10-27T00:59:00Z")));
    let job = review_job(&store);
    project_with_brief(&store, "案件A", at("2026-10-27T01:30:00Z"));
    let mut d = daemon(&dir, &store, &clock);
    let paused = |store: &SqliteStore| {
        store
            .chat_thread_get(&review_thread(store))
            .unwrap()
            .unwrap()
            .queue_paused
    };

    tick_at(&mut d, &clock, "2026-10-27T01:00:00Z");
    let thread = review_thread(&store);
    // A run takes the 01:00 input; the 02:00 input waits behind it; the person stops the run.
    store
        .chat_run_claim_next(
            &thread,
            "r1",
            &serde_json::json!({}),
            at("2026-10-27T01:01:00Z"),
        )
        .unwrap()
        .expect("claim r1");
    tick_at(&mut d, &clock, "2026-10-27T02:00:00Z");
    assert_eq!(inputs(&store).len(), 2);
    let stop = store
        .chat_run_stop(&thread, "r1", at("2026-10-27T02:05:00Z"))
        .unwrap();
    assert!(stop.accepted && stop.response.queue_paused);
    store
        .chat_run_finish(
            "r1",
            ChatRunState::Stopped,
            None,
            None,
            at("2026-10-27T02:06:00Z"),
        )
        .unwrap();
    assert!(
        store
            .chat_thread_resume_queue(&thread, 0, at("2026-10-27T02:07:00Z"))
            .is_err()
    );

    // The paused queue with a waiting input does not turn every later period into an overlap.
    tick_at(&mut d, &clock, "2026-10-27T03:00:00Z");
    let all = inputs(&store);
    assert_eq!(all.len(), 3, "{all:?}");
    assert_eq!(all[1].0, review_key(job.id, at("2026-10-27T02:00:00Z")));
    assert_eq!(all[1].2, ChatMessageState::Cancelled);
    assert_eq!(all[2].0, review_key(job.id, at("2026-10-27T03:00:00Z")));
    assert_eq!(all[2].2, ChatMessageState::Queued);
    // The new input covers the cancelled one's period: from the input the stopped run took.
    assert!(
        all[2].1.contains("前回の見直し 2026-10-27T01:00:00Z"),
        "{}",
        all[2].1
    );
    assert!(all[2].1.contains("案件A"), "{}", all[2].1);
    assert!(!paused(&store), "the new input resumed the queue");
    assert_eq!(
        outcomes(&store, &job).last().unwrap(),
        &("2026-10-27T03:00:00Z".into(), CronRunOutcome::Created)
    );

    // Stopped again with nothing waiting: the next period queues normally and resumes the queue.
    store
        .chat_run_claim_next(
            &thread,
            "r2",
            &serde_json::json!({}),
            at("2026-10-27T03:01:00Z"),
        )
        .unwrap()
        .expect("claim r2");
    store
        .chat_run_stop(&thread, "r2", at("2026-10-27T03:05:00Z"))
        .unwrap();
    store
        .chat_run_finish(
            "r2",
            ChatRunState::Stopped,
            None,
            None,
            at("2026-10-27T03:06:00Z"),
        )
        .unwrap();
    assert!(paused(&store));
    tick_at(&mut d, &clock, "2026-10-27T04:00:00Z");
    let all = inputs(&store);
    assert_eq!(all.len(), 4, "{all:?}");
    assert_eq!(all[3].0, review_key(job.id, at("2026-10-27T04:00:00Z")));
    assert!(
        all[3].1.contains("前回の見直し 2026-10-27T03:00:00Z"),
        "{}",
        all[3].1
    );
    assert!(!paused(&store));
    assert_eq!(
        outcomes(&store, &job).last().unwrap(),
        &("2026-10-27T04:00:00Z".into(), CronRunOutcome::Created)
    );
    assert_eq!(general_messages(&store), 0);
}

/// D3': the review thread's run gets what a general CoS run gets (every project's summary, every
/// thread's open inbox items, the same skills) in its own session, not the person's.
#[test]
fn cos_review_thread_runs_get_the_general_cos_context_scope() {
    use crate::dispatcher::cos_chat::launch::{CosContextScope, cos_context_scope};
    use task_core::chat::ChatThreadKind;
    assert_eq!(
        cos_context_scope(Some(ChatThreadKind::Review)),
        cos_context_scope(Some(ChatThreadKind::General))
    );
    assert_eq!(
        cos_context_scope(Some(ChatThreadKind::Review)),
        CosContextScope::All
    );
    assert_eq!(
        cos_context_scope(Some(ChatThreadKind::Project)),
        CosContextScope::Project
    );
    assert_eq!(
        cos_context_scope(Some(ChatThreadKind::Legacy)),
        CosContextScope::None
    );
}

#[tokio::test]
async fn cos_review_run_has_the_general_cos_context_and_its_own_session() {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).unwrap());
    let clock = Arc::new(StdMutex::new(at("2026-10-27T00:59:00Z")));
    review_job(&store);
    let a = project_with_brief(&store, "案件A", at("2026-10-27T00:30:00Z"));
    let b = project_with_brief(&store, "案件B", at("2026-10-27T00:40:00Z"));
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec![
        "sh".into(),
        "-c".into(),
        "cat >/dev/null; echo '{\"type\":\"done\",\"summary\":\"変化なし\",\"evidence\":[]}'"
            .into(),
    ]));
    let mut d = daemon_with(&dir, &store, &clock, adapter, 2);
    d.config.execution.max_cos_runs = 1;
    d.config.knowledge.root = dir.path().join("knowledge");
    for name in ["cos-operator", "cos-inbox-triage"] {
        let skill_dir = d.config.knowledge.root.join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n# {name}\n"),
        )
        .unwrap();
    }

    tick_at(&mut d, &clock, "2026-10-27T01:00:00Z");
    tick_at(&mut d, &clock, "2026-10-27T01:00:01Z");
    let thread = review_thread(&store);
    let handle = d
        .cos_chat_launch
        .as_mut()
        .unwrap()
        .running
        .remove(&thread)
        .expect("review run launched");
    tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("review run deadline")
        .unwrap();

    let runs = store.chat_run_list(&thread, None, None).unwrap().items;
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].state, ChatRunState::Completed, "{runs:?}");
    let request = dir
        .path()
        .join("cos/threads")
        .join(&thread)
        .join("workspace/runs")
        .join(&runs[0].id)
        .join("request.json");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&request).unwrap()).unwrap();
    let chat = &json["context"]["cos_chat"];
    assert_eq!(
        chat["skills"],
        serde_json::json!(["cos-operator", "cos-inbox-triage"]),
        "{chat}"
    );
    let mut projects: Vec<String> = chat["projects"]
        .as_array()
        .expect("every project's summary")
        .iter()
        .map(|p| p["project_id"].as_str().unwrap_or_default().to_string())
        .collect();
    projects.sort();
    let mut expected = vec![a.id.to_string(), b.id.to_string()];
    expected.sort();
    assert_eq!(projects, expected, "{chat}");

    // The run's session belongs to the review thread, not to the person's general thread.
    let session = store
        .chat_run_session_record(&runs[0].id)
        .unwrap()
        .session_row_id
        .expect("session");
    let owner: String = rusqlite::Connection::open(dir.path().join("celeris.db"))
        .unwrap()
        .query_row(
            "SELECT thread_id FROM node_sessions WHERE id=?1",
            [&session],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(owner, thread);
    assert_ne!(owner, general_thread(&store));
    assert_eq!(general_messages(&store), 0);
}

fn wait_source(kind: &str, key: &str, summary: &str) -> task_core::chat::triage::CosTriageSource {
    task_core::chat::triage::CosTriageSource {
        source_kind: kind.into(),
        source_key: key.into(),
        source_revision: "1".into(),
        source_event_id: Some(1),
        operation_id: None,
        summary: summary.into(),
        detail: String::new(),
        policy_version: "v1".into(),
        task_id: None,
    }
}

/// ADR 2026-10-30-cos-review-open-waits D1: the review input lists the waits open at the review
/// time and, apart from them, the ones closed since the previous review. The three errors of the
/// 2026-10-30 12:00Z report do not reappear: the answered decision is only on the closed side, and
/// a task waiting on cluster jobs or a done task (no inbox item) is not an open wait.
#[test]
fn review_text_lists_open_waits_apart_from_closed_ones() {
    let store = SqliteStore::open_in_memory().unwrap();
    let job = review_job(&store);
    store
        .cos_triage_reconcile(
            "decision",
            &[
                wait_source("decision", "T-ISO:iso-boundary", "iso-boundary を決める"),
                wait_source("decision", "T-OTHER:pick", "どちらを採るか"),
            ],
            at("2026-10-30T11:00:00Z"),
        )
        .unwrap();
    store
        .cos_triage_reconcile(
            "plan_approval",
            &[wait_source("plan_approval", "T-ISO", "計画の承認")],
            at("2026-10-30T11:00:00Z"),
        )
        .unwrap();
    // 11:43:38Z: the person answers iso-boundary; the next snapshot no longer has it.
    store
        .cos_triage_reconcile(
            "decision",
            &[wait_source("decision", "T-OTHER:pick", "どちらを採るか")],
            at("2026-10-30T11:43:38Z"),
        )
        .unwrap();
    let previous = Some(at("2026-10-30T11:00:00Z"));
    let text = crate::dispatcher::cos_chat::review::review_text(
        &store,
        &job,
        at("2026-10-30T12:00:00Z"),
        previous,
        &[],
    )
    .unwrap();
    let (open, closed) = text
        .split_once("前回から閉じた人待ち（報告しない）")
        .expect("closed section");
    let open = open.split_once("いま開いている人待ち").expect("open").1;
    assert!(open.contains("（2 件）"), "{text}");
    assert!(open.contains("decision T-OTHER:pick"), "{text}");
    assert!(open.contains("plan_approval T-ISO"), "{text}");
    assert!(!open.contains("iso-boundary"), "{text}");
    assert!(closed.contains("（1 件）"), "{text}");
    assert!(closed.contains("decision T-ISO:iso-boundary"), "{text}");
    assert!(closed.contains("開いた 2026-10-30T11:00:00.000Z"), "{text}");
    // A second review with the same previous time is identical (deterministic input).
    let again = crate::dispatcher::cos_chat::review::review_text(
        &store,
        &job,
        at("2026-10-30T12:00:00Z"),
        previous,
        &[],
    )
    .unwrap();
    assert_eq!(text, again);
}
