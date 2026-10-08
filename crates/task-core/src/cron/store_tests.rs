//! ADR-0131 D1: `cron_jobs` / `cron_job_runs` の store 試験と、既存 DB（版数 37）への migration 0046。

use rusqlite::{Connection, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::*;
use crate::store::{SCHEMA_VERSION, SqliteStore, StoreError, TaskStore};

fn t(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).unwrap()
}

fn job(name: &str, next: Option<&str>) -> CronJob {
    let now = t("2026-10-02T00:00:00Z");
    CronJob {
        id: CronJobId::new(),
        name: name.to_string(),
        enabled: next.is_some(),
        schedule: "30 4 * * *".to_string(),
        timezone: "Asia/Tokyo".to_string(),
        overlap: CronOverlap::Skip,
        catch_up: CronCatchUp::Latest,
        template: CronTaskTemplate {
            title: "日次整理: {date}".to_string(),
            objective: "整理する".to_string(),
            harness: Some("knowledge-curation".to_string()),
            lane: Some(Tier::Cheap),
            ..Default::default()
        },
        next_fire_at: next.map(t),
        created_at: now,
        updated_at: now,
    }
}

fn run(job_id: CronJobId, at: &str, trigger: CronTrigger, outcome: CronRunOutcome) -> CronJobRun {
    CronJobRun {
        id: CronJobRunId::new(),
        job_id,
        scheduled_for: t(at),
        trigger,
        outcome,
        task_id: None,
        detail: None,
        recorded_at: t(at),
    }
}

#[test]
fn cron_job_store_crud_round_trips() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = job("daily-curation", Some("2026-10-02T19:30:00Z"));
    store.cron_job_insert(&a).unwrap();
    assert_eq!(store.cron_job_get(a.id).unwrap(), Some(a.clone()));
    assert_eq!(
        store.cron_job_get_by_name("daily-curation").unwrap(),
        Some(a.clone())
    );
    assert_eq!(store.cron_job_get(CronJobId::new()).unwrap(), None);

    // 同じ name は 409 相当。
    let dup = job("daily-curation", None);
    assert!(matches!(
        store.cron_job_insert(&dup),
        Err(StoreError::InUse {
            kind: "cron_job",
            ..
        })
    ));

    let b = job("a-weekly", None);
    store.cron_job_insert(&b).unwrap();
    let names: Vec<String> = store
        .cron_job_list()
        .unwrap()
        .into_iter()
        .map(|j| j.name)
        .collect();
    assert_eq!(names, vec!["a-weekly", "daily-curation"]);

    // 更新（一時停止で next_fire_at を消す）。
    let mut paused = a.clone();
    paused.enabled = false;
    paused.next_fire_at = None;
    paused.schedule = "0 5 * * *".to_string();
    paused.overlap = CronOverlap::Queue;
    paused.catch_up = CronCatchUp::Skip;
    paused.updated_at = t("2026-10-02T01:00:00Z");
    assert!(store.cron_job_update(&paused).unwrap());
    assert_eq!(store.cron_job_get(a.id).unwrap(), Some(paused.clone()));
    // name を他の job と重ねる更新は拒む。
    let mut clash = b.clone();
    clash.name = "daily-curation".to_string();
    assert!(matches!(
        store.cron_job_update(&clash),
        Err(StoreError::InUse { .. })
    ));
    assert!(!store.cron_job_update(&job("missing", None)).unwrap());

    // 削除は履歴も消す。
    store
        .cron_job_record(
            a.id,
            None,
            t("2026-10-02T02:00:00Z"),
            &[run(
                a.id,
                "2026-10-01T19:30:00Z",
                CronTrigger::Manual,
                CronRunOutcome::Created,
            )],
        )
        .unwrap();
    assert!(store.cron_job_delete(a.id).unwrap());
    assert!(!store.cron_job_delete(a.id).unwrap());
    assert_eq!(store.cron_job_get(a.id).unwrap(), None);
    assert!(store.cron_job_runs(a.id, None).unwrap().is_empty());
}

#[test]
fn cron_job_due_filters_by_time_not_by_string_order() {
    let store = SqliteStore::open_in_memory().unwrap();
    let early = job("early", Some("2026-10-02T04:00:00Z"));
    // 小数秒つき（文字列では "…00.5Z" < "…00Z" になるが、時刻としては後）。
    let fractional = job("fractional", Some("2026-10-02T05:00:00.5Z"));
    let later = job("later", Some("2026-10-02T06:00:00Z"));
    let paused = job("paused", None);
    for j in [&early, &fractional, &later, &paused] {
        store.cron_job_insert(j).unwrap();
    }
    let due: Vec<String> = store
        .cron_job_due(t("2026-10-02T05:00:00Z"))
        .unwrap()
        .into_iter()
        .map(|j| j.name)
        .collect();
    assert_eq!(due, vec!["early"]);
    let due: Vec<String> = store
        .cron_job_due(t("2026-10-02T06:00:00Z"))
        .unwrap()
        .into_iter()
        .map(|j| j.name)
        .collect();
    assert_eq!(due, vec!["early", "fractional", "later"]);
}

#[test]
fn cron_job_record_is_atomic_and_rejects_double_fire() {
    let store = SqliteStore::open_in_memory().unwrap();
    let j = job("daily", Some("2026-10-02T19:30:00Z"));
    store.cron_job_insert(&j).unwrap();
    let missed = CronJobRun {
        detail: Some("2 missed 2026-09-30..2026-10-01".to_string()),
        ..run(
            j.id,
            "2026-09-30T19:30:00Z",
            CronTrigger::CatchUp,
            CronRunOutcome::SkippedMissed,
        )
    };
    let mut created = run(
        j.id,
        "2026-10-01T19:30:00Z",
        CronTrigger::CatchUp,
        CronRunOutcome::Created,
    );
    created.task_id = Some(TaskId::new());
    store
        .cron_job_record(
            j.id,
            Some(t("2026-10-02T19:30:00Z")),
            t("2026-10-02T03:00:00Z"),
            &[missed.clone(), created.clone()],
        )
        .unwrap();
    let got = store.cron_job_get(j.id).unwrap().unwrap();
    assert_eq!(got.next_fire_at, Some(t("2026-10-02T19:30:00Z")));
    assert_eq!(got.updated_at, t("2026-10-02T03:00:00Z"));
    // 新しい順。
    assert_eq!(
        store.cron_job_runs(j.id, None).unwrap(),
        vec![created.clone(), missed.clone()]
    );
    assert_eq!(
        store.cron_job_runs(j.id, Some(1)).unwrap(),
        vec![created.clone()]
    );
    assert_eq!(
        store.cron_job_run_last_created(j.id).unwrap(),
        Some(created.clone())
    );

    // 同じ (job, 予定時刻, trigger) の 2 回目は何も書かない（next_fire_at も動かない）。
    let again = run(
        j.id,
        "2026-10-01T19:30:00Z",
        CronTrigger::CatchUp,
        CronRunOutcome::Created,
    );
    let err = store
        .cron_job_record(
            j.id,
            Some(t("2026-10-03T19:30:00Z")),
            t("2026-10-02T04:00:00Z"),
            &[again],
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            StoreError::InUse {
                kind: "cron_job_run",
                ..
            }
        ),
        "{err:?}"
    );
    let got = store.cron_job_get(j.id).unwrap().unwrap();
    assert_eq!(got.next_fire_at, Some(t("2026-10-02T19:30:00Z")));
    assert_eq!(store.cron_job_runs(j.id, None).unwrap().len(), 2);
    // trigger が違えば同じ予定時刻でも別の行（手動と定時）。
    store
        .cron_job_record(
            j.id,
            Some(t("2026-10-02T19:30:00Z")),
            t("2026-10-02T04:00:00Z"),
            &[run(
                j.id,
                "2026-10-01T19:30:00Z",
                CronTrigger::Manual,
                CronRunOutcome::SkippedOverlap,
            )],
        )
        .unwrap();
    // 無い job への記録は拒む。
    assert!(matches!(
        store.cron_job_record(CronJobId::new(), None, t("2026-10-02T04:00:00Z"), &[]),
        Err(StoreError::Invalid(_))
    ));
}

#[test]
fn cron_job_queued_run_is_closed_by_update() {
    let store = SqliteStore::open_in_memory().unwrap();
    let j = job("queue-job", Some("2026-10-02T19:30:00Z"));
    store.cron_job_insert(&j).unwrap();
    let queued = run(
        j.id,
        "2026-10-02T19:30:00Z",
        CronTrigger::Schedule,
        CronRunOutcome::Queued,
    );
    store
        .cron_job_record(
            j.id,
            Some(t("2026-10-03T19:30:00Z")),
            t("2026-10-02T19:30:00Z"),
            std::slice::from_ref(&queued),
        )
        .unwrap();
    assert_eq!(
        store.cron_job_run_queued(j.id).unwrap(),
        Some(queued.clone())
    );
    assert_eq!(store.cron_job_run_last_created(j.id).unwrap(), None);

    let task = TaskId::new();
    let update = CronJobRunUpdate {
        outcome: CronRunOutcome::Created,
        task_id: Some(task),
        detail: None,
        recorded_at: t("2026-10-02T21:00:00Z"),
    };
    assert!(store.cron_job_run_update(queued.id, &update).unwrap());
    assert_eq!(store.cron_job_run_queued(j.id).unwrap(), None);
    let created = store.cron_job_run_last_created(j.id).unwrap().unwrap();
    assert_eq!(created.task_id, Some(task));
    assert_eq!(created.scheduled_for, queued.scheduled_for);
    assert_eq!(created.recorded_at, t("2026-10-02T21:00:00Z"));
    assert!(
        !store
            .cron_job_run_update(CronJobRunId::new(), &update)
            .unwrap()
    );
}

/// 既存 DB（版数 37。cron の表も通知の表も無い）を開くと 0041・0046 が足され、既存の行は残る。
#[test]
fn cron_job_migration_applies_to_an_existing_schema_37_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.sqlite3");
    {
        let store = SqliteStore::open(&path).unwrap();
        store
            .cluster_settings_set("pegasus", Some("/work/NBB/x"), t("2026-10-01T00:00:00Z"))
            .unwrap();
    }
    {
        // 版数 37 の DB に戻す。
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TRIGGER chat_node_session_thread_insert; \
             DROP TRIGGER chat_node_session_thread_update; \
             DROP INDEX idx_node_sessions_cos_chat_active; \
             ALTER TABLE node_sessions DROP COLUMN thread_id; \
             ALTER TABLE node_sessions DROP COLUMN llm_source; \
             ALTER TABLE node_sessions DROP COLUMN model; \
             ALTER TABLE node_sessions DROP COLUMN summary_through_seq; \
             DROP TABLE chat_search; DROP TABLE chat_upload_reservations; \
             DROP TABLE chat_client_requests; DROP TABLE cos_legacy_notification_links; DROP TABLE cos_notification_routes; \
             DROP TABLE cos_run_credentials; DROP TABLE cos_operations; DROP TABLE cos_inbox_items; \
             DROP TABLE chat_attachment_refs; DROP TABLE chat_attachments; \
             DROP TABLE chat_events; DROP TABLE chat_runs; \
             DROP TABLE chat_messages; DROP TABLE chat_threads; \
             DROP TABLE cron_job_runs; DROP TABLE cron_jobs; \
             DROP TABLE feed_notices; DROP TABLE feed_sources; DROP TABLE feed_cursor; \
             DROP INDEX idx_events_integration_request; \
             DROP TABLE model_role_scopes; DROP TABLE model_role_assignments; \
             DROP TABLE routing_shadow_reservations; \
             DROP INDEX idx_events_routing_decided; DROP INDEX idx_llm_proxy_requests_decision; \
             ALTER TABLE llm_proxy_requests DROP COLUMN model; ALTER TABLE llm_proxy_requests DROP COLUMN source_id; \
             ALTER TABLE llm_proxy_requests DROP COLUMN task_id; ALTER TABLE llm_proxy_requests DROP COLUMN run_id; \
             ALTER TABLE llm_proxy_requests DROP COLUMN snapshot_id; ALTER TABLE llm_proxy_requests DROP COLUMN decision_id; \
             DROP TABLE task_behind_targets; \
             DROP TABLE run_write_sets; DROP TABLE work_unit_write_sets; \
             DROP TABLE task_write_hints; \
             ALTER TABLE deliveries DROP COLUMN target_sha; \
             ALTER TABLE deliveries DROP COLUMN reviewed_sha; \
             ALTER TABLE deliveries DROP COLUMN merge_candidate_sha; \
             DROP INDEX idx_node_sessions_work_unit_active; \
             ALTER TABLE node_sessions DROP COLUMN delivered_through_seq; \
             ALTER TABLE node_sessions DROP COLUMN last_context_tokens; \
             ALTER TABLE node_sessions DROP COLUMN billed_input_tokens; \
             ALTER TABLE node_sessions DROP COLUMN task_id; \
             ALTER TABLE node_sessions DROP COLUMN work_unit_id; \
             ALTER TABLE node_sessions DROP COLUMN provider; \
             ALTER TABLE node_sessions DROP COLUMN cwd;",
        )
        .unwrap();
        conn.execute(
            "DELETE FROM schema_migrations WHERE version > ?1",
            params![37],
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 63);
    assert_eq!(
        store
            .cluster_settings_get("pegasus")
            .unwrap()
            .and_then(|c| c.work_dir)
            .as_deref(),
        Some("/work/NBB/x")
    );
    let j = job("after-migration", Some("2026-10-02T19:30:00Z"));
    store.cron_job_insert(&j).unwrap();
    assert_eq!(store.cron_job_list().unwrap(), vec![j]);
    drop(store);
    // 2 回目の open は何もしない（冪等）。
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.cron_job_list().unwrap().len(), 1);
}
