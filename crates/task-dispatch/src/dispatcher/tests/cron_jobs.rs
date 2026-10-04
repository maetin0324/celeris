//! ADR-0131 D9: dispatcher の tick と cron 発火を注入時計で検査する。

use super::*;
use task_ops::cron_jobs::{CronFireContext, NewCronJob, create_job};
use time::format_description::well_known::Rfc3339;

fn at(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap()
}

fn job(store: &SqliteStore, name: &str, catch_up: CronCatchUp) -> CronJob {
    create_job(
        store,
        &CronFireContext::default(),
        NewCronJob {
            name: name.into(),
            schedule: "0 3 * * *".into(),
            timezone: "UTC".into(),
            overlap: CronOverlap::Skip,
            catch_up,
            enabled: true,
            template: CronTaskTemplate {
                title: format!("{name} {{date}}"),
                objective: "日次の整理".into(),
                acceptance: vec![serde_json::json!({"type": "reviewer", "text": "整理した"})],
                lane: Some(Tier::Cheap),
                ..CronTaskTemplate::default()
            },
        },
        at("2026-10-01T00:00:00Z"),
    )
    .unwrap()
}

fn setup(store: Arc<SqliteStore>) -> (Dispatcher, Arc<StdMutex<OffsetDateTime>>) {
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    // worker 枠を設けず、作成された task を非終端のまま観察する。
    let mut d = dispatcher(store, adapter, 0);
    let clock = Arc::new(StdMutex::new(at("2026-10-01T02:59:59Z")));
    d.test_now = Some(clock.clone());
    (d, clock)
}

fn tick_at(d: &mut Dispatcher, clock: &Arc<StdMutex<OffsetDateTime>>, value: &str) {
    *clock.lock().unwrap() = at(value);
    d.tick().unwrap();
}

#[tokio::test]
async fn tick_fires_once_skips_nonterminal_overlap_and_ignores_disabled_job() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let scheduled = job(&store, "scheduled", CronCatchUp::Latest);
    let disabled = job(&store, "disabled", CronCatchUp::Latest);
    let mut disabled_row = disabled.clone();
    disabled_row.enabled = false;
    store.cron_job_update(&disabled_row).unwrap();
    let (mut d, clock) = setup(store.clone());

    tick_at(&mut d, &clock, "2026-10-01T02:59:59Z");
    assert!(store.cron_job_runs(scheduled.id, None).unwrap().is_empty());
    tick_at(&mut d, &clock, "2026-10-01T03:00:01Z");
    tick_at(&mut d, &clock, "2026-10-01T03:00:01Z");
    let runs = store.cron_job_runs(scheduled.id, None).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, CronRunOutcome::Created);
    let task_id = runs[0].task_id.unwrap();
    let task = store.get(task_id).unwrap().unwrap();
    assert_eq!(task.status, Status::Ready);
    assert_eq!(task.title, "scheduled 2026-10-01");
    assert_eq!(task.worker_hint.tier, Tier::Cheap);

    tick_at(&mut d, &clock, "2026-10-02T03:00:01Z");
    let runs = store.cron_job_runs(scheduled.id, None).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].outcome, CronRunOutcome::SkippedOverlap);
    assert_eq!(runs[1].task_id, Some(task_id));
    assert!(store.cron_job_runs(disabled.id, None).unwrap().is_empty());
}

#[tokio::test]
async fn tick_after_daemon_gap_applies_each_job_misfire_rule() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let latest = job(&store, "latest", CronCatchUp::Latest);
    let skip = job(&store, "skip", CronCatchUp::Skip);
    let (mut d, clock) = setup(store.clone());

    // daemon が止まっていた間の 3 日分を、次の 1 tick で評価する。
    tick_at(&mut d, &clock, "2026-10-03T12:00:00Z");
    let latest_runs = store.cron_job_runs(latest.id, None).unwrap();
    assert_eq!(latest_runs.len(), 2);
    assert_eq!(latest_runs[0].outcome, CronRunOutcome::Created);
    assert_eq!(latest_runs[0].trigger, CronTrigger::CatchUp);
    assert_eq!(latest_runs[0].scheduled_for, at("2026-10-03T03:00:00Z"));
    assert_eq!(latest_runs[1].outcome, CronRunOutcome::SkippedMissed);
    let skip_runs = store.cron_job_runs(skip.id, None).unwrap();
    assert_eq!(skip_runs.len(), 1);
    assert_eq!(skip_runs[0].outcome, CronRunOutcome::SkippedMissed);
    assert!(skip_runs[0].task_id.is_none());

    tick_at(&mut d, &clock, "2026-10-03T12:00:00Z");
    assert_eq!(store.cron_job_runs(latest.id, None).unwrap().len(), 2);
    assert_eq!(store.cron_job_runs(skip.id, None).unwrap().len(), 1);
}
