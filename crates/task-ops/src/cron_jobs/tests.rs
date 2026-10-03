//! ADR-0131 D9: 発火の規則を注入した時計（`now` 引数）で決定的に検査する。実時間の sleep は使わない。

use super::*;
use task_core::{CronJobStore, SqliteStore, Tier, Trigger};
use time::format_description::well_known::Rfc3339;

fn t(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).expect("rfc3339")
}

fn template() -> CronTaskTemplate {
    CronTaskTemplate {
        title: "知識整理 {date}".to_string(),
        objective: "KB と受信箱を整理する".to_string(),
        acceptance: vec![serde_json::json!({"type": "reviewer", "text": "整理の記録がある"})],
        lane: Some(Tier::Cheap),
        ..CronTaskTemplate::default()
    }
}

fn new_job(overlap: CronOverlap, catch_up: CronCatchUp) -> NewCronJob {
    NewCronJob {
        name: "daily-curation".to_string(),
        schedule: "0 3 * * *".to_string(),
        timezone: "UTC".to_string(),
        overlap,
        catch_up,
        enabled: true,
        template: template(),
    }
}

fn setup(overlap: CronOverlap, catch_up: CronCatchUp) -> (SqliteStore, CronJob) {
    let store = SqliteStore::open_in_memory().expect("open store");
    let job = create_job(
        &store,
        &CronFireContext::default(),
        new_job(overlap, catch_up),
        t("2026-10-01T00:00:00Z"),
    )
    .expect("create job");
    (store, job)
}

/// tick 1 回分: 有効な job を全部評価して、作った task の id を返す。
fn tick(store: &SqliteStore, now: &str) -> Vec<TaskId> {
    fire_due(store, t(now))
        .into_iter()
        .map(|r| r.expect("fire"))
        .filter_map(|o| o.task_id)
        .collect()
}

fn history(store: &SqliteStore, job: &CronJob) -> Vec<(CronTrigger, CronRunOutcome, String)> {
    let mut runs = store.cron_job_runs(job.id, None).expect("runs");
    runs.reverse();
    runs.into_iter()
        .map(|r| (r.trigger, r.outcome, format_time(r.scheduled_for)))
        .collect()
}

fn finish(store: &SqliteStore, id: TaskId) {
    store
        .apply_transition(id, Trigger::Cancel, None)
        .expect("cancel");
}

fn next_fire(store: &SqliteStore, job: &CronJob) -> Option<OffsetDateTime> {
    store
        .cron_job_get(job.id)
        .expect("get")
        .expect("job")
        .next_fire_at
}

#[test]
fn creates_a_normal_ready_task_from_the_template_once_per_scheduled_time() {
    let (store, job) = setup(CronOverlap::Skip, CronCatchUp::Latest);
    assert_eq!(job.next_fire_at, Some(t("2026-10-01T03:00:00Z")));

    assert!(tick(&store, "2026-10-01T02:59:59Z").is_empty());
    let created = tick(&store, "2026-10-01T03:00:04Z");
    assert_eq!(created.len(), 1);
    // 同じ tick の繰り返し・次の tick では増えない。
    assert!(tick(&store, "2026-10-01T03:00:04Z").is_empty());
    assert!(tick(&store, "2026-10-01T03:00:09Z").is_empty());
    assert_eq!(next_fire(&store, &job), Some(t("2026-10-02T03:00:00Z")));

    let task = store.get(created[0]).expect("get").expect("task");
    assert_eq!(task.title, "知識整理 2026-10-01");
    assert_eq!(task.status, Status::Ready);
    assert_eq!(task.worker_hint.tier, Tier::Cheap);
    assert_eq!(task.labels, vec![CRON_TASK_LABEL.to_string()]);
    assert_eq!(task.acceptance.len(), 1);
    assert_eq!(
        history(&store, &job),
        vec![(
            CronTrigger::Schedule,
            CronRunOutcome::Created,
            "2026-10-01T03:00:00Z".to_string()
        )]
    );
    let last = store
        .cron_job_run_last_created(job.id)
        .expect("last")
        .expect("created");
    assert_eq!(last.task_id, Some(created[0]));
}

#[test]
fn overlap_skip_does_not_stack_while_the_previous_task_runs() {
    let (store, job) = setup(CronOverlap::Skip, CronCatchUp::Latest);
    let first = tick(&store, "2026-10-01T03:00:01Z");
    assert_eq!(first.len(), 1);

    // 前回 task が ready のまま次の予定時刻: 作らず skipped_overlap、次回へ進む。
    assert!(tick(&store, "2026-10-02T03:00:01Z").is_empty());
    assert_eq!(next_fire(&store, &job), Some(t("2026-10-03T03:00:00Z")));
    let runs = store.cron_job_runs(job.id, Some(1)).expect("runs");
    assert_eq!(runs[0].outcome, CronRunOutcome::SkippedOverlap);
    assert!(
        runs[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains(&first[0].to_string()))
    );

    // 前回 task が終端になった後の予定時刻では作る。
    finish(&store, first[0]);
    assert!(tick(&store, "2026-10-02T12:00:00Z").is_empty());
    assert_eq!(tick(&store, "2026-10-03T03:00:01Z").len(), 1);
    assert_eq!(
        history(&store, &job)
            .into_iter()
            .map(|(_, o, _)| o)
            .collect::<Vec<_>>(),
        vec![
            CronRunOutcome::Created,
            CronRunOutcome::SkippedOverlap,
            CronRunOutcome::Created
        ]
    );
}

#[test]
fn overlap_queue_keeps_one_pending_run_and_creates_it_after_the_previous_task_ends() {
    let (store, job) = setup(CronOverlap::Queue, CronCatchUp::Latest);
    let first = tick(&store, "2026-10-01T03:00:01Z");
    assert_eq!(first.len(), 1);

    assert!(tick(&store, "2026-10-02T03:00:01Z").is_empty());
    // 溜めるのは高々 1 件: 次の予定時刻は skipped_overlap。
    assert!(tick(&store, "2026-10-03T03:00:01Z").is_empty());
    assert_eq!(
        history(&store, &job),
        vec![
            (
                CronTrigger::Schedule,
                CronRunOutcome::Created,
                "2026-10-01T03:00:00Z".to_string()
            ),
            (
                CronTrigger::Schedule,
                CronRunOutcome::Queued,
                "2026-10-02T03:00:00Z".to_string()
            ),
            (
                CronTrigger::Schedule,
                CronRunOutcome::SkippedOverlap,
                "2026-10-03T03:00:00Z".to_string()
            ),
        ]
    );
    // 前回が動いている間の tick は何もしない。
    assert!(tick(&store, "2026-10-03T04:00:00Z").is_empty());

    // 前回が終端になった後の最初の tick で queued の行を created にして作る。
    finish(&store, first[0]);
    let drained = tick(&store, "2026-10-03T05:00:00Z");
    assert_eq!(drained.len(), 1);
    let queued_row = store
        .cron_job_run_last_created(job.id)
        .expect("last")
        .expect("created");
    assert_eq!(queued_row.scheduled_for, t("2026-10-02T03:00:00Z"));
    assert_eq!(queued_row.task_id, Some(drained[0]));
    assert!(store.cron_job_run_queued(job.id).expect("queued").is_none());
    let task = store.get(drained[0]).expect("get").expect("task");
    assert_eq!(task.title, "知識整理 2026-10-02");
    // 次の tick では増えない。
    assert!(tick(&store, "2026-10-03T05:00:05Z").is_empty());
}

#[test]
fn missed_times_while_stopped_fire_only_the_latest_with_catch_up_latest() {
    let (store, job) = setup(CronOverlap::Skip, CronCatchUp::Latest);
    // daemon が 10-01 03:00 〜 10-04 03:00 の 4 回を過ぎるまで止まっていた。
    let created = tick(&store, "2026-10-04T10:00:00Z");
    assert_eq!(created.len(), 1);
    assert_eq!(
        history(&store, &job),
        vec![
            (
                CronTrigger::CatchUp,
                CronRunOutcome::SkippedMissed,
                "2026-10-01T03:00:00Z".to_string()
            ),
            (
                CronTrigger::CatchUp,
                CronRunOutcome::Created,
                "2026-10-04T03:00:00Z".to_string()
            ),
        ]
    );
    let missed = &store.cron_job_runs(job.id, None).expect("runs")[1];
    assert!(
        missed
            .detail
            .as_deref()
            .is_some_and(|d| d.starts_with("missed 3 "))
    );
    let task = store.get(created[0]).expect("get").expect("task");
    assert_eq!(task.title, "知識整理 2026-10-04");
    assert_eq!(next_fire(&store, &job), Some(t("2026-10-05T03:00:00Z")));
    assert!(tick(&store, "2026-10-04T10:00:05Z").is_empty());
}

#[test]
fn missed_times_with_catch_up_skip_create_nothing_but_on_time_fires_still_work() {
    let (store, job) = setup(CronOverlap::Skip, CronCatchUp::Skip);
    assert!(tick(&store, "2026-10-04T10:00:00Z").is_empty());
    let runs = store.cron_job_runs(job.id, None).expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, CronRunOutcome::SkippedMissed);
    assert!(
        runs[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.starts_with("missed 4 "))
    );
    assert_eq!(next_fire(&store, &job), Some(t("2026-10-05T03:00:00Z")));

    // 通常運転（遅れが猶予以内）なら catch_up = skip でも発火する。
    assert_eq!(tick(&store, "2026-10-05T03:01:00Z").len(), 1);
    let last = &store.cron_job_runs(job.id, Some(1)).expect("runs")[0];
    assert_eq!(last.trigger, CronTrigger::Schedule);
    assert_eq!(last.outcome, CronRunOutcome::Created);
}

#[test]
fn late_beyond_grace_is_recorded_as_catch_up() {
    let (store, job) = setup(CronOverlap::Skip, CronCatchUp::Latest);
    assert_eq!(tick(&store, "2026-10-01T03:30:00Z").len(), 1);
    assert_eq!(
        history(&store, &job),
        vec![(
            CronTrigger::CatchUp,
            CronRunOutcome::Created,
            "2026-10-01T03:00:00Z".to_string()
        )]
    );
}

#[test]
fn manual_run_follows_overlap_rules_and_keeps_the_schedule() {
    let (store, job) = setup(CronOverlap::Skip, CronCatchUp::Latest);
    let now = t("2026-10-01T01:00:00Z");
    let out = run_now(&store, job.id, now).expect("run now");
    let manual_task = out.task_id.expect("manual creates a task");
    assert_eq!(out.runs.len(), 1);
    assert_eq!(out.runs[0].trigger, CronTrigger::Manual);
    assert_eq!(out.runs[0].scheduled_for, now);
    assert_eq!(next_fire(&store, &job), Some(t("2026-10-01T03:00:00Z")));

    // 同じ時刻にもう一度押す: UNIQUE で拒否され、task は増えない。
    let tasks_before = store.list(None).expect("list").len();
    assert!(matches!(
        run_now(&store, job.id, now),
        Err(OpsError::Store(StoreError::InUse { .. }))
    ));
    assert_eq!(store.list(None).expect("list").len(), tasks_before);

    // 手動の task が動いている間の手動実行・定時の発火は重ねない。
    let again = run_now(&store, job.id, t("2026-10-01T01:00:30Z")).expect("run now");
    assert_eq!(again.task_id, None);
    assert_eq!(again.runs[0].outcome, CronRunOutcome::SkippedOverlap);
    assert!(tick(&store, "2026-10-01T03:00:01Z").is_empty());

    finish(&store, manual_task);
    assert_eq!(tick(&store, "2026-10-02T03:00:01Z").len(), 1);
}

#[test]
fn pause_closes_the_queue_and_resume_does_not_catch_up_paused_time() {
    let (store, job) = setup(CronOverlap::Queue, CronCatchUp::Latest);
    let first = tick(&store, "2026-10-01T03:00:01Z");
    assert!(tick(&store, "2026-10-02T03:00:01Z").is_empty());
    assert!(store.cron_job_run_queued(job.id).expect("queued").is_some());

    let paused = pause_job(&store, job.id, t("2026-10-02T04:00:00Z")).expect("pause");
    assert!(!paused.enabled);
    assert_eq!(paused.next_fire_at, None);
    assert!(store.cron_job_run_queued(job.id).expect("queued").is_none());
    let closed = &store.cron_job_runs(job.id, Some(1)).expect("runs")[0];
    assert_eq!(closed.outcome, CronRunOutcome::SkippedOverlap);
    assert_eq!(closed.detail.as_deref(), Some("paused"));

    // 一時停止中は何も起きない（前回 task が終わっても）。
    finish(&store, first[0]);
    assert!(tick(&store, "2026-10-05T03:00:01Z").is_empty());

    // 再開: 一時停止中に過ぎた時刻は取りこぼしにしない。
    let resumed = resume_job(&store, job.id, t("2026-10-05T12:00:00Z")).expect("resume");
    assert_eq!(resumed.next_fire_at, Some(t("2026-10-06T03:00:00Z")));
    assert!(tick(&store, "2026-10-05T12:00:05Z").is_empty());
    assert_eq!(tick(&store, "2026-10-06T03:00:01Z").len(), 1);
    assert!(
        store
            .cron_job_runs(job.id, None)
            .expect("runs")
            .iter()
            .all(|r| r.outcome != CronRunOutcome::SkippedMissed)
    );
}

#[test]
fn create_and_update_validate_and_reschedule() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let ctx = CronFireContext::default();
    let now = t("2026-10-01T00:00:00Z");
    let bad = |f: fn(&mut NewCronJob)| {
        let mut new = new_job(CronOverlap::Skip, CronCatchUp::Latest);
        f(&mut new);
        create_job(&store, &ctx, new, now)
    };
    assert!(matches!(
        bad(|j| j.schedule = "61 * * * *".into()),
        Err(OpsError::Validation(_))
    ));
    assert!(matches!(
        bad(|j| j.schedule = "0 0 31 2 *".into()),
        Err(OpsError::Validation(_))
    ));
    assert!(matches!(
        bad(|j| j.timezone = "Mars/Olympus".into()),
        Err(OpsError::Validation(_))
    ));
    assert!(matches!(
        bad(|j| j.template.acceptance.clear()),
        Err(OpsError::Validation(_))
    ));
    assert!(matches!(
        bad(|j| j.template.project = Some("no such project".into())),
        Err(OpsError::Validation(_))
    ));
    assert!(store.cron_job_list().expect("list").is_empty());

    let job = create_job(
        &store,
        &ctx,
        new_job(CronOverlap::Skip, CronCatchUp::Latest),
        now,
    )
    .expect("create");
    assert!(matches!(
        create_job(
            &store,
            &ctx,
            new_job(CronOverlap::Skip, CronCatchUp::Latest),
            now
        ),
        Err(OpsError::Store(StoreError::InUse { .. }))
    ));
    assert_eq!(
        resolve_job(&store, "daily-curation")
            .expect("resolve")
            .map(|j| j.id),
        Some(job.id)
    );
    assert_eq!(
        resolve_job(&store, &job.id.to_string())
            .expect("resolve")
            .map(|j| j.id),
        Some(job.id)
    );

    let updated = update_job(
        &store,
        &ctx,
        job.id,
        CronJobPatch {
            schedule: Some("30 21 * * *".into()),
            timezone: Some("Asia/Tokyo".into()),
            overlap: Some(CronOverlap::Queue),
            ..CronJobPatch::default()
        },
        t("2026-10-01T12:00:00Z"),
    )
    .expect("update");
    // Asia/Tokyo 21:30 = 12:30 UTC。
    assert_eq!(updated.next_fire_at, Some(t("2026-10-01T12:30:00Z")));
    assert_eq!(updated.overlap, CronOverlap::Queue);
    assert!(matches!(
        update_job(
            &store,
            &ctx,
            job.id,
            CronJobPatch {
                timezone: Some("Nowhere/Zone".into()),
                ..CronJobPatch::default()
            },
            now
        ),
        Err(OpsError::Validation(_))
    ));

    assert!(delete_job(&store, job.id).expect("delete"));
    assert!(
        resolve_job(&store, "daily-curation")
            .expect("resolve")
            .is_none()
    );
}

#[test]
fn disabled_job_is_not_fired_and_starts_without_next_time() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut new = new_job(CronOverlap::Skip, CronCatchUp::Latest);
    new.enabled = false;
    let job = create_job(
        &store,
        &CronFireContext::default(),
        new,
        t("2026-10-01T00:00:00Z"),
    )
    .expect("create");
    assert_eq!(job.next_fire_at, None);
    assert!(tick(&store, "2026-10-09T03:00:00Z").is_empty());
    assert!(store.cron_job_runs(job.id, None).expect("runs").is_empty());
}

/// ADR-0131 付記 D10 (3): 雛形の `mode` は発火時に task のラベルへ写り、後の job の変更で変わらない。
#[test]
fn knowledge_curation_job_mode_is_snapshotted_at_fire_time() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut new = new_job(CronOverlap::Skip, CronCatchUp::Latest);
    new.template
        .extra
        .insert("mode".to_string(), serde_json::json!("apply"));
    let job = create_job(
        &store,
        &CronFireContext::default(),
        new,
        t("2026-10-01T00:00:00Z"),
    )
    .expect("create job");
    let created = tick(&store, "2026-10-01T03:00:04Z");
    assert_eq!(created.len(), 1);

    // 発火後に job を dry_run へ戻しても、作った task の mode は apply のまま。
    let mut changed = store.cron_job_get(job.id).expect("get").expect("job");
    changed
        .template
        .extra
        .insert("mode".to_string(), serde_json::json!("dry_run"));
    assert!(store.cron_job_update(&changed).expect("update"));

    let task = store.get(created[0]).expect("get").expect("task");
    assert!(task.labels.contains(&MODE_APPLY_LABEL.to_string()));
    assert_eq!(task_mode(&task), "apply");

    finish(&store, created[0]);
    let next = tick(&store, "2026-10-02T03:00:04Z");
    let task = store.get(next[0]).expect("get").expect("task");
    assert_eq!(task_mode(&task), "dry_run");
}
