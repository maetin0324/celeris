use super::*;
use crate::required_checks::health::{CheckerId, CheckerSource};

fn record(job_id: CronJobId, repo_id: RepoId) -> HealthInventoryRun {
    HealthInventoryRun {
        id: Ulid::new(),
        job_id,
        repo_id,
        scheduled_for: OffsetDateTime::UNIX_EPOCH,
        created_at: OffsetDateTime::UNIX_EPOCH,
        outcome: HealthInventoryOutcome::Evaluated,
        evidence: Some(HealthEvidence {
            repo_id,
            base_sha: "base".into(),
            head_sha: "head".into(),
            head_tree: "tree".into(),
            policy_sha256: "policy".into(),
            checker: Some(CheckerId {
                source: CheckerSource::Bundled,
                sha256: "checker".into(),
            }),
            report_sha256: "report".into(),
            tool_version: "1".into(),
            verdict: HealthVerdict::Fail,
            blocking: vec!["fingerprint".into()],
            reason: Some("reason".into()),
            reused_from: Some("previous".into()),
            reviewable: true,
        }),
        task_ids: vec![TaskId::new()],
        existing_skipped: 2,
        limit_remaining: 3,
    }
}

// Invariant: repeating a job/repo appends history, and duplicate primary keys cannot overwrite it.
#[test]
fn health_inventory_store_appends_only() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = record(CronJobId::new(), RepoId::new());
    let mut b = a.clone();
    b.id = Ulid::new();
    store.health_inventory_append(&a).unwrap();
    store.health_inventory_append(&b).unwrap();
    assert!(store.health_inventory_append(&a).is_err());
    assert_eq!(store.health_inventory_rows(a.job_id).unwrap(), vec![a, b]);
}

// Invariant: inventory stores the shared evidence without dropping any field.
#[test]
fn health_inventory_store_roundtrips_evidence() {
    let store = SqliteStore::open_in_memory().unwrap();
    let row = record(CronJobId::new(), RepoId::new());
    store.health_inventory_append(&row).unwrap();
    assert_eq!(
        store
            .health_inventory_latest_evaluated(row.job_id, row.repo_id)
            .unwrap(),
        Some(row)
    );
}

// Invariant: unsuccessful/skipped observations never advance the diff base; time is not text order.
#[test]
fn health_inventory_store_latest_evaluated_skips_error_and_unchanged() {
    let store = SqliteStore::open_in_memory().unwrap();
    let first = record(CronJobId::new(), RepoId::new());
    let mut latest = first.clone();
    latest.id = Ulid::new();
    latest.scheduled_for += time::Duration::hours(1);
    latest.scheduled_for = latest
        .scheduled_for
        .to_offset(time::UtcOffset::from_hms(-5, 0, 0).unwrap());
    store.health_inventory_append(&latest).unwrap();
    store.health_inventory_append(&first).unwrap(); // append order differs from scheduled order
    let mut error = latest.clone();
    error.id = Ulid::new();
    error.scheduled_for += time::Duration::hours(1);
    error.evidence.as_mut().unwrap().verdict = HealthVerdict::Error;
    store.health_inventory_append(&error).unwrap();
    for outcome in [
        HealthInventoryOutcome::Unchanged,
        HealthInventoryOutcome::NotApplicable,
    ] {
        let mut skipped = error.clone();
        skipped.id = Ulid::new();
        skipped.outcome = outcome;
        skipped.evidence = None;
        store.health_inventory_append(&skipped).unwrap();
    }
    let other_repo = record(first.job_id, RepoId::new());
    store.health_inventory_append(&other_repo).unwrap();
    let other_job = record(CronJobId::new(), first.repo_id);
    store.health_inventory_append(&other_job).unwrap();
    assert_eq!(
        store
            .health_inventory_latest_evaluated(first.job_id, first.repo_id)
            .unwrap(),
        Some(latest)
    );
    assert!(
        store
            .health_inventory_latest_evaluated(CronJobId::new(), first.repo_id)
            .unwrap()
            .is_none()
    );
}

// Invariant: all fire counts derive solely from the matching rows, including skipped repos.
#[test]
fn health_inventory_store_derives_fire_counts() {
    let store = SqliteStore::open_in_memory().unwrap();
    let row = record(CronJobId::new(), RepoId::new());
    store.health_inventory_append(&row).unwrap();
    for outcome in [
        HealthInventoryOutcome::Unchanged,
        HealthInventoryOutcome::NotApplicable,
    ] {
        let mut skipped = record(row.job_id, RepoId::new());
        skipped.outcome = outcome;
        skipped.evidence = None;
        skipped.task_ids.clear();
        skipped.existing_skipped = 0;
        skipped.limit_remaining = 0;
        skipped.scheduled_for = skipped
            .scheduled_for
            .to_offset(time::UtcOffset::from_hms(9, 0, 0).unwrap());
        store.health_inventory_append(&skipped).unwrap();
    }
    let mut other_fire = row.clone();
    other_fire.id = Ulid::new();
    other_fire.scheduled_for += time::Duration::seconds(1);
    store.health_inventory_append(&other_fire).unwrap();
    store
        .health_inventory_append(&record(CronJobId::new(), row.repo_id))
        .unwrap();
    assert_eq!(
        store
            .health_inventory_fire_counts(row.job_id, row.scheduled_for)
            .unwrap(),
        HealthInventoryFireCounts {
            repos: 3,
            evaluated: 1,
            unchanged: 1,
            not_applicable: 1,
            tasks_created: 1,
            existing_skipped: 2,
            limit_remaining: 3
        }
    );
}

#[test]
fn health_inventory_store_rejects_inconsistent_evidence() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut row = record(CronJobId::new(), RepoId::new());
    row.evidence.as_mut().unwrap().repo_id = RepoId::new();
    assert!(store.health_inventory_append(&row).is_err());
    row.evidence = None;
    assert!(store.health_inventory_append(&row).is_err());
    row.outcome = HealthInventoryOutcome::Unchanged;
    store.health_inventory_append(&row).unwrap();
}
