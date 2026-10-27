//! ADR 2026-10-27-cron-cos-review: a `cos_review` cron job queues one input per scheduled time in
//! the general CoS thread. Injected clock; no CoS capacity, so no run (and no LLM) is started and
//! the test only sees what the dispatcher itself wrote.

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use crate::dispatcher::cos_chat::review::review_key;
use task_core::chat::{ChatMessageRole, ChatMessageState, ChatOpenThreadRequest};
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
    let mut d = dispatcher(store.clone(), adapter, 0);
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

/// User inputs of the general thread, oldest first: (client key, text, state).
fn inputs(store: &SqliteStore) -> Vec<(String, String, ChatMessageState)> {
    let thread = general_thread(store);
    store
        .chat_message_list(&thread, &Default::default())
        .unwrap()
        .items
        .into_iter()
        .filter(|m| m.role == ChatMessageRole::User)
        .map(|m| (m.client_message_id.unwrap_or_default(), m.text, m.state))
        .collect()
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
    // The dispatcher only queued the input: no CoS run was started.
    assert_eq!(chat_runs(&dir), 0);

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
    let thread = general_thread(&store);
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
