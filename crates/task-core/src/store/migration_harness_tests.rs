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
            assert!(
                columns.iter().any(|actual| actual == column),
                "{table}.{column}"
            );
        }
    }
}

#[test]
fn migration_harness_resolves_registered_and_audits_unresolved_once() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let store = SqliteStore::open(&dir.path().join("migration.db")).expect("open migrated store");
    let resolved = sample_task(Status::Draft);
    let resolved_id = resolved.id;
    store.insert(&resolved).expect("insert resolvable task");
    set_legacy_hint_adapter(&store, resolved_id, "harness-a");
    let unresolved = sample_task(Status::Draft);
    let unresolved_id = unresolved.id;
    store.insert(&unresolved).expect("insert unresolved task");
    set_legacy_hint_adapter(&store, unresolved_id, "removed-harness");

    let registered = BTreeSet::from([crate::llm_route::HarnessId::from("harness-a")]);
    assert_eq!(
        store
            .resolve_migrated_task_harnesses(&registered)
            .expect("resolve"),
        1
    );
    assert_eq!(
        store
            .resolve_migrated_task_harnesses(&registered)
            .expect("repeat"),
        0
    );

    let resolved = store
        .get(resolved_id)
        .expect("read resolved task")
        .expect("task");
    assert_eq!(
        resolved.harness_id,
        Some(crate::llm_route::HarnessId::from("harness-a"))
    );
    let unresolved = store
        .get(unresolved_id)
        .expect("read unresolved task")
        .expect("task");
    assert_eq!(unresolved.harness_id, None);
    let events = store.events_for(unresolved_id).expect("read events");
    assert_eq!(
        events
            .iter()
            .filter(|(_, event)| matches!(event, crate::Event::HarnessMigrationUnresolved { .. }))
            .count(),
        1
    );
}

fn set_legacy_hint_adapter(store: &SqliteStore, task_id: crate::TaskId, value: &str) {
    let conn = store.lock().expect("lock store");
    let mut task: serde_json::Value = serde_json::from_str(
        &conn
            .query_row(
                "SELECT json FROM tasks WHERE id=?1",
                [task_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .expect("read task json"),
    )
    .expect("parse task json");
    task["worker_hint"]["adapter"] = serde_json::json!(value);
    conn.execute(
        "UPDATE tasks SET json=?2 WHERE id=?1",
        rusqlite::params![task_id.to_string(), serde_json::to_string(&task).expect("serialize task")],
    )
    .expect("write legacy task json");
}
