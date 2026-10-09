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

// ---- ADR 2026-10-07-build-tmp-hygiene D1.4: `extra.action` の決定的な保守 executor ----

fn sweep_job(store: &SqliteStore, action: &str, mode: &str) -> Result<CronJob, task_ops::OpsError> {
    let mut extra = std::collections::BTreeMap::new();
    extra.insert("action".to_string(), serde_json::json!(action));
    extra.insert("mode".to_string(), serde_json::json!(mode));
    create_job(
        store,
        &CronFireContext::default(),
        NewCronJob {
            name: "target-sweep".into(),
            schedule: "0 3 * * *".into(),
            timezone: "UTC".into(),
            overlap: CronOverlap::Skip,
            catch_up: CronCatchUp::Latest,
            enabled: true,
            template: CronTaskTemplate {
                title: "target sweep {date}".into(),
                objective: "共有 cargo target の掃除".into(),
                extra,
                ..CronTaskTemplate::default()
            },
        },
        at("2026-10-01T00:00:00Z"),
    )
}

fn set_time(path: &std::path::Path, t: std::time::SystemTime) {
    let f = std::fs::File::open(path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_accessed(t).set_modified(t))
        .unwrap();
}

/// `<profile>/deps/lib<key>.rlib`（key は cargo と同じ `<name>-<hash>`）を書き、時刻を `t` にする。
fn rlib(profile: &std::path::Path, key: &str, t: std::time::SystemTime) -> std::path::PathBuf {
    let deps = profile.join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    let path = deps.join(format!("lib{key}.rlib"));
    std::fs::write(&path, vec![0u8; 4096]).unwrap();
    set_time(&path, t);
    path
}

/// 雛形の `action = "target_sweep"` から発火した task は worker に渡らず（`worker_started` が無い）、
/// 同じ tick で掃除が走って `target_sweep_ran` が追記され `done` になる。古い項目は消え、新しい項目と
/// `.cargo-lock` を他が保持中の profile の項目は残る（実 flock）。時計は dispatcher の注入時計。
#[tokio::test]
async fn target_sweep_cron_fires_deterministic_executor_and_records_event() {
    use nix::fcntl::{Flock, FlockArg};
    use std::time::{Duration as StdDuration, SystemTime};

    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let job = sweep_job(&store, "target_sweep", "apply").unwrap();

    let fire = at("2026-10-01T03:00:01Z");
    let now = SystemTime::from(fire);
    let day = StdDuration::from_secs(24 * 60 * 60);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cargo");
    let target = root.join("agent-platform");
    let debug = target.join("debug");
    let release = target.join("release");
    for p in [&debug, &release] {
        std::fs::create_dir_all(p).unwrap();
        std::fs::write(p.join(".cargo-lock"), b"").unwrap();
    }
    let old = rlib(&debug, "old-0000000a", now - day * 30);
    let fresh = rlib(&debug, "fresh-0000000b", now - day);
    let building = rlib(&release, "building-0000000c", now - day * 30);
    // release は build 中（他の cargo が `.cargo-lock` を保持している）。
    let lock = Flock::lock(
        std::fs::File::open(release.join(".cargo-lock")).unwrap(),
        FlockArg::LockExclusiveNonblock,
    )
    .map_err(|(_, e)| e)
    .unwrap();

    // worker の枠を 1 つ設ける（executor が拾わなければ worker に dispatch される状態）。
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let clock = Arc::new(StdMutex::new(at("2026-10-01T02:59:59Z")));
    d.test_now = Some(clock.clone());
    d.set_target_sweep(task_worker::target_sweep::SweepParams {
        roots: vec![root.clone()],
        ..Default::default()
    });

    tick_at(&mut d, &clock, "2026-10-01T03:00:01Z");
    let runs = store.cron_job_runs(job.id, None).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, CronRunOutcome::Created);
    let task_id = runs[0].task_id.unwrap();
    let task = store.get(task_id).unwrap().unwrap();
    assert_eq!(task.status, Status::Done);
    assert!(
        task.labels
            .iter()
            .any(|l| l == "maintenance-action-target-sweep"),
        "{:?}",
        task.labels
    );

    let events = store.events_for(task_id).unwrap();
    assert!(
        !events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkerStarted { .. } | Event::RoutingDecided { .. }
        )),
        "保守 task は worker run を起こさない: {events:?}"
    );
    let ran: Vec<&Event> = events
        .iter()
        .map(|(_, e)| e)
        .filter(|e| matches!(e, Event::TargetSweepRan { .. }))
        .collect();
    let [
        Event::TargetSweepRan {
            mode,
            roots,
            skipped,
            ..
        },
    ] = ran.as_slice()
    else {
        panic!("TargetSweepRan が 1 件でない: {events:?}")
    };
    assert_eq!(*mode, task_core::model::TargetSweepMode::Apply);
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].root, root.display().to_string());
    assert!(roots[0].deleted_items >= 1, "{roots:?}");
    assert_eq!(roots[0].by_reason.age, roots[0].deleted_items, "{roots:?}");
    assert!(
        skipped.iter().any(|s| s.reason == "build_in_progress"
            && s.path.starts_with(&release.display().to_string())),
        "{skipped:?}"
    );
    assert!(!old.exists(), "古い項目は消える");
    assert!(fresh.exists(), "新しい項目は残る");
    assert!(building.exists(), "build 中の profile の項目は残る");

    // 次の tick でも worker には渡らない（task は終端のまま）。
    tick_at(&mut d, &clock, "2026-10-01T03:00:05Z");
    let events = store.events_for(task_id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { .. }))
    );
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);
    drop(lock);
}

/// 雛形の `extra.action` は予約語だけ。未知の値と未実装の `tmp_sweep` は job の作成で拒否する。
#[test]
fn target_sweep_cron_rejects_unknown_action() {
    let store = SqliteStore::open_in_memory().unwrap();
    for action in ["rm_rf", "tmp_sweep"] {
        let err = sweep_job(&store, action, "apply").unwrap_err();
        assert!(
            matches!(&err, task_ops::OpsError::Validation(m) if m.contains("extra.action")),
            "{action}: {err:?}"
        );
    }
    assert!(store.cron_job_list().unwrap().is_empty());
    sweep_job(&store, "target_sweep", "dry_run").unwrap();
}

#[tokio::test]
async fn target_sweep_executor_cleans_finished_workspaces_and_scratch_at_critical() {
    use nix::fcntl::{Flock, FlockArg};
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("workspaces");
    let scratch = tmp.path().join("scratch");
    let now = at("2026-10-09T12:00:00Z");
    let mut targets = Vec::new();
    for (status, age) in [
        (Status::Done, 7),
        (Status::Cancelled, 7),
        (Status::Failed, 7),
        (Status::Done, 1),
        (Status::Running, 7),
        (Status::Done, 7),
    ] {
        let task = new_task(tmp.path(), Check::Reviewer, 0);
        store.insert(&task).unwrap();
        if status == Status::Cancelled {
            store
                .apply_transition(task.id, Trigger::Cancel, None)
                .unwrap();
        } else {
            store
                .apply_transition(task.id, Trigger::Dispatch, None)
                .unwrap();
            if status == Status::Failed {
                store
                    .apply_transition(task.id, Trigger::WorkerError { retryable: false }, None)
                    .unwrap();
            } else if status == Status::Done {
                store
                    .apply_transition(task.id, Trigger::WorkerDone, None)
                    .unwrap();
                store
                    .apply_transition(task.id, Trigger::ReviewPass, None)
                    .unwrap();
            }
        }
        let mut terminal = store.get(task.id).unwrap().unwrap();
        terminal.updated_at = now - time::Duration::hours(age);
        store
            .update_task(&terminal, Event::worker_progress("test", "backdated"))
            .unwrap();
        let repo = ws.join(task.id.to_string()).join("repos/r");
        let target = repo.join("target");
        std::fs::create_dir_all(target.join("debug")).unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        std::fs::write(target.join("debug/.cargo-lock"), "").unwrap();
        std::fs::write(repo.join("source.rs"), "preserve").unwrap();
        std::fs::create_dir_all(repo.join(".deleting-user-data")).unwrap();
        let scratch_target = scratch
            .join("targets")
            .join(format!("task-{}", task.id))
            .join("target/debug");
        std::fs::create_dir_all(&scratch_target).unwrap();
        std::fs::write(scratch_target.join(".cargo-lock"), "").unwrap();
        let old = rlib(
            &scratch_target,
            "old-0000000a",
            std::time::SystemTime::from(now - time::Duration::days(8)),
        );
        targets.push((repo, target, old));
    }
    let lock = Flock::lock(
        std::fs::File::open(targets[5].1.join("debug/.cargo-lock")).unwrap(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.workspace_root = ws.clone();
    scratch_on(&mut d, &scratch);
    d.test_now = Some(Arc::new(StdMutex::new(now)));
    d.set_target_sweep(task_worker::target_sweep::SweepParams {
        roots: vec![],
        ..Default::default()
    });
    struct Critical;
    impl crate::disk_watch::DiskProbe for Critical {
        fn usage(&self, _: &std::path::Path) -> Result<crate::disk_watch::DiskUsage, String> {
            Ok(crate::disk_watch::DiskUsage {
                blocks: 100,
                bfree: 1,
                bavail: 1,
            })
        }
    }
    d.set_disk_watch_with_probe(
        vec![crate::disk_watch::DiskWatchEntry {
            path: "/fake/local".into(),
            warn_pct: 80.0,
            critical_pct: 95.0,
        }],
        Box::new(Critical),
    );
    d.tick_disk_watch();
    assert!(d.disk_watch_critical());
    let mut maintenance = new_task(tmp.path(), Check::Reviewer, 0);
    maintenance.labels = vec![
        "maintenance-action-target-sweep".into(),
        "mode-apply".into(),
    ];
    store.insert(&maintenance).unwrap();
    assert!(d.run_maintenance_task(&maintenance).unwrap());
    assert_eq!(
        store.get(maintenance.id).unwrap().unwrap().status,
        Status::Done
    );
    for (i, (repo, target, scratch_item)) in targets.iter().enumerate() {
        assert_eq!(target.exists(), i >= 3, "{}", target.display());
        assert_eq!(scratch_item.exists(), i == 4, "{}", scratch_item.display());
        assert!(repo.join("source.rs").exists());
        assert!(repo.join(".deleting-user-data").exists());
    }
    drop(lock);
}
