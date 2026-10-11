use std::collections::BTreeSet;

use super::{SqliteStore, TaskStore, tests::sample_task};
use crate::Status;

#[test]
fn migration_harness_adds_task_and_run_selection_columns() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let store = SqliteStore::open(&dir.path().join("migration.db")).expect("open migrated store");
    let conn = store.lock().expect("lock store");
    for (table, expected) in [
        ("tasks", vec!["harness_id"]),
        (
            "runs",
            vec![
                "selected_harness_id",
                "selected_source_id",
                "selected_model_id",
                "selected_account_id",
                "selection_metadata_json",
            ],
        ),
    ] {
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("inspect migrated table");
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("read columns")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect columns");
        for column in expected {
            assert!(columns.iter().any(|actual| actual == column), "{table}.{column}");
        }
    }
}

#[test]
fn migration_harness_resolves_registered_and_audits_unresolved_once() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let store = SqliteStore::open(&dir.path().join("migration.db")).expect("open migrated store");
    let mut resolved = sample_task(Status::Draft);
    resolved.worker_hint.adapter = Some("harness-a".into());
    let resolved_id = resolved.id;
    store.insert(&resolved).expect("insert resolvable task");
    let mut unresolved = sample_task(Status::Draft);
    unresolved.worker_hint.adapter = Some("removed-harness".into());
    let unresolved_id = unresolved.id;
    store.insert(&unresolved).expect("insert unresolved task");

    let registered = BTreeSet::from([crate::llm_route::HarnessId::from("harness-a")]);
    assert_eq!(store.resolve_migrated_task_harnesses(&registered).expect("resolve"), 1);
    assert_eq!(store.resolve_migrated_task_harnesses(&registered).expect("repeat"), 0);

    let resolved = store.get(resolved_id).expect("read resolved task").expect("task");
    assert_eq!(resolved.harness_id, Some(crate::llm_route::HarnessId::from("harness-a")));
    let unresolved = store.get(unresolved_id).expect("read unresolved task").expect("task");
    assert_eq!(unresolved.harness_id, None);
    let events = store.events_for(unresolved_id).expect("read events");
    assert_eq!(events.iter().filter(|(_, event)| matches!(event, crate::Event::HarnessMigrationUnresolved { .. })).count(), 1);
}
