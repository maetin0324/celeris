//! ADR 2026-11-08-health-enforcement-loop (6) D6.1・D6.2: cron の種類 `health_inventory` の検証・発火・既定で
//! 無効であること。時計は `now` 引数で注入し、実時間の sleep は使わない。

use super::*;
use task_core::{CronJobStore, SqliteStore, Tier};
use time::format_description::well_known::Rfc3339;

fn t(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).expect("rfc3339")
}

const PROJECT: &str = "01M6ZDF99MKHFN57PN0H7G6Z9Z";

fn input_job(kind: &str, overlap: CronOverlap) -> NewCronJob {
    let mut template = CronTaskTemplate {
        title: "定期の入力".to_string(),
        ..CronTaskTemplate::default()
    };
    template
        .extra
        .insert("kind".into(), serde_json::json!(kind));
    if kind != CRON_KIND_COS_REVIEW {
        template
            .extra
            .insert("project_id".into(), serde_json::json!(PROJECT));
    }
    NewCronJob {
        name: format!("{kind}-job"),
        schedule: "0 3 * * 1".to_string(),
        timezone: "UTC".to_string(),
        overlap,
        catch_up: CronCatchUp::Latest,
        enabled: true,
        template,
    }
}

fn inventory_job() -> NewCronJob {
    input_job(CRON_KIND_HEALTH_INVENTORY, CronOverlap::Skip)
}

/// `field`（[`INPUT_KIND_TASK_FIELDS`] の名前）だけを書いた雛形。`extra.mode`・`extra.action` は種類に依らない
/// 検査を通る値にする（task の欄の検査で落ちることを見るため）。
fn with_task_field(mut job: NewCronJob, field: &str) -> NewCronJob {
    let t = &mut job.template;
    match field {
        "acceptance" => t.acceptance = vec![serde_json::json!({"type": "reviewer", "text": "x"})],
        "assignee" => t.assignee = Some("cluster-hpc".into()),
        "harness" => t.harness = Some("claude-code".into()),
        "lane" => t.lane = Some(Tier::Cheap),
        "project" => t.project = Some("agent-platform".into()),
        "repos" => t.repos = vec!["agent-platform".into()],
        "priority" => t.priority = Some(serde_json::json!("P2")),
        "extra.mode" => {
            t.extra.insert("mode".into(), serde_json::json!("dry_run"));
        }
        "extra.action" => {
            t.extra
                .insert("action".into(), serde_json::json!("target_sweep"));
        }
        other => panic!("unknown task field {other}"),
    }
    job
}

/// `NewCronJob` を検証に渡せる `CronJob` にする（id と時刻は検証に使われない）。
fn as_job(new: &NewCronJob) -> CronJob {
    let now = t("2026-11-09T00:00:00Z");
    CronJob {
        id: CronJobId::new(),
        name: new.name.clone(),
        enabled: new.enabled,
        schedule: new.schedule.clone(),
        timezone: new.timezone.clone(),
        overlap: new.overlap,
        catch_up: new.catch_up,
        template: new.template.clone(),
        next_fire_at: None,
        created_at: now,
        updated_at: now,
    }
}

/// 積まれた棚卸しを記録する sink。`busy` は前の棚卸しがまだ終わっていない dispatcher の答え。
#[derive(Default)]
struct InventorySink {
    calls: Vec<(OffsetDateTime, String)>,
    busy: bool,
}

impl CronInputSink for InventorySink {
    fn post_cos_review(
        &mut self,
        _job: &CronJob,
        _scheduled_for: OffsetDateTime,
        _previous: Option<OffsetDateTime>,
    ) -> Result<CronInputPost, String> {
        Err("not a cos_review job".into())
    }

    fn post_health_inventory(
        &mut self,
        _job: &CronJob,
        scheduled_for: OffsetDateTime,
        project_id: &str,
    ) -> Result<CronInputPost, String> {
        self.calls.push((scheduled_for, project_id.to_owned()));
        Ok(if self.busy {
            CronInputPost::Busy("inventory running".into())
        } else {
            CronInputPost::Posted("inventory queued".into())
        })
    }
}

fn outcomes(store: &SqliteStore, job: &CronJob) -> Vec<(CronRunOutcome, Option<String>)> {
    let mut runs = store.cron_job_runs(job.id, None).expect("runs");
    runs.reverse();
    runs.into_iter().map(|r| (r.outcome, r.detail)).collect()
}

#[test]
fn health_inventory_requires_project_id() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let ctx = CronFireContext::default();
    let now = t("2026-11-09T00:00:00Z");
    for project_id in [
        None,
        Some(serde_json::json!("  ")),
        Some(serde_json::json!(42)),
    ] {
        let mut job = inventory_job();
        match project_id {
            Some(v) => job.template.extra.insert("project_id".into(), v),
            None => job.template.extra.remove("project_id"),
        };
        let err = create_job(&store, &ctx, job, now).expect_err("project_id");
        assert!(
            err.to_string()
                .contains("a health_inventory job needs template.extra.project_id"),
            "{err}"
        );
    }
    assert!(store.cron_job_list().expect("list").is_empty());
    let job = create_job(&store, &ctx, inventory_job(), now).expect("valid");
    assert_eq!(
        template_kind(&job.template),
        Ok(CronJobKind::HealthInventory)
    );
    assert_eq!(template_project_id(&job).as_deref(), Some(PROJECT));
}

#[test]
fn health_inventory_rejects_task_fields_and_overlap() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let ctx = CronFireContext::default();
    let now = t("2026-11-09T00:00:00Z");
    for field in INPUT_KIND_TASK_FIELDS {
        let err = create_job(&store, &ctx, with_task_field(inventory_job(), field), now)
            .expect_err(field);
        assert_eq!(
            err.to_string(),
            OpsError::Validation(format!(
                "template.{field} is a task field; a health_inventory job does not create a task"
            ))
            .to_string()
        );
    }
    let err = create_job(
        &store,
        &ctx,
        input_job(CRON_KIND_HEALTH_INVENTORY, CronOverlap::Queue),
        now,
    )
    .expect_err("queue");
    assert!(
        err.to_string()
            .contains("a health_inventory job must use overlap \"skip\""),
        "{err}"
    );
    assert!(store.cron_job_list().expect("list").is_empty());
}

#[test]
fn health_inventory_disabled_by_default() {
    // No job, no fire: an empty store posts nothing however long the clock runs.
    let store = SqliteStore::open_in_memory().expect("open store");
    let ctx = CronFireContext::default();
    let mut sink = InventorySink::default();
    for now in ["2026-11-09T03:00:00Z", "2026-11-16T03:00:00Z"] {
        assert!(fire_due_with(&store, &ctx, &mut sink, t(now)).is_empty());
    }
    assert!(sink.calls.is_empty());
    // A paused job (the state a `[[cron.seed]]` gets when `enabled` is omitted) never fires either.
    let mut paused = inventory_job();
    paused.enabled = false;
    let job = create_job(&store, &ctx, paused, t("2026-11-09T00:00:00Z")).expect("create");
    assert_eq!(job.next_fire_at, None);
    assert!(fire_due_with(&store, &ctx, &mut sink, t("2026-11-16T03:00:00Z")).is_empty());
    assert!(sink.calls.is_empty());
    assert!(outcomes(&store, &job).is_empty());
    // The shipped config examples seed no inventory job (D6.2: the person adds it).
    let config = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let mut examples = 0;
    for entry in std::fs::read_dir(&config).expect("config dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "toml") {
            examples += 1;
            let text = std::fs::read_to_string(&path).expect("read example");
            assert!(
                !text.contains(CRON_KIND_HEALTH_INVENTORY),
                "{} seeds a {CRON_KIND_HEALTH_INVENTORY} job",
                path.display()
            );
        }
    }
    assert!(
        examples > 0,
        "no config examples under {}",
        config.display()
    );
}

#[test]
fn health_inventory_without_sink_fails_fire() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let job = create_job(
        &store,
        &CronFireContext::default(),
        inventory_job(),
        t("2026-11-09T00:00:00Z"),
    )
    .expect("create");
    let fired = fire_due(&store, t("2026-11-09T03:00:00Z"));
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].as_ref().expect("fire").task_id, None);
    assert_eq!(
        outcomes(&store, &job),
        vec![(
            CronRunOutcome::Error,
            Some("health_inventory runs are posted only by the daemon tick".into())
        )]
    );
    // Like project_reconcile, only the schedule fires it.
    let err = run_now(&store, job.id, t("2026-11-09T03:10:00Z")).expect_err("manual");
    assert!(
        err.to_string().contains("fires only on its schedule"),
        "{err}"
    );
    assert!(check_manual_run(&job).is_err());
    assert!(store.list(None).expect("tasks").is_empty());
}

#[test]
fn health_inventory_fires_through_the_sink_once_per_time_and_never_creates_a_task() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let ctx = CronFireContext::default();
    let job = create_job(&store, &ctx, inventory_job(), t("2026-11-09T00:00:00Z")).expect("create");
    let mut sink = InventorySink::default();
    assert!(fire_due_with(&store, &ctx, &mut sink, t("2026-11-09T02:59:59Z")).is_empty());
    assert_eq!(
        fire_due_with(&store, &ctx, &mut sink, t("2026-11-09T03:00:00Z")).len(),
        1
    );
    assert!(fire_due_with(&store, &ctx, &mut sink, t("2026-11-09T03:00:30Z")).is_empty());
    sink.busy = true;
    fire_due_with(&store, &ctx, &mut sink, t("2026-11-16T03:00:00Z"));
    assert_eq!(
        sink.calls,
        vec![
            (t("2026-11-09T03:00:00Z"), PROJECT.to_owned()),
            (t("2026-11-16T03:00:00Z"), PROJECT.to_owned()),
        ]
    );
    assert_eq!(
        outcomes(&store, &job),
        vec![
            (CronRunOutcome::Created, Some("inventory queued".into())),
            (
                CronRunOutcome::SkippedOverlap,
                Some("inventory running".into())
            ),
        ]
    );
    assert!(store.list(None).expect("tasks").is_empty());
}

#[test]
fn health_inventory_shares_input_kind_validation() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let ctx = CronFireContext::default();
    let now = t("2026-11-09T00:00:00Z");
    let kinds = [
        CRON_KIND_COS_REVIEW,
        CRON_KIND_PROJECT_RECONCILE,
        CRON_KIND_HEALTH_INVENTORY,
    ];
    for kind in kinds {
        let mut cases: Vec<NewCronJob> = INPUT_KIND_TASK_FIELDS
            .iter()
            .map(|field| with_task_field(input_job(kind, CronOverlap::Skip), field))
            .collect();
        cases.push(input_job(kind, CronOverlap::Queue));
        for case in cases {
            let shared = validate_input_kind(&as_job(&case), kind).expect_err("shared check");
            let via_kind = validate_job(&store, &ctx, &as_job(&case), now).expect_err(kind);
            assert_eq!(via_kind.to_string(), shared.to_string(), "{kind}");
        }
        // A job without task fields passes the shared check for every kind.
        validate_input_kind(&as_job(&input_job(kind, CronOverlap::Skip)), kind).expect("clean");
        validate_job(
            &store,
            &ctx,
            &as_job(&input_job(kind, CronOverlap::Skip)),
            now,
        )
        .expect(kind);
    }
    // project_reconcile reads its target with the same function as health_inventory.
    let reconcile = as_job(&input_job(CRON_KIND_PROJECT_RECONCILE, CronOverlap::Skip));
    assert_eq!(template_project_id(&reconcile).as_deref(), Some(PROJECT));
}
