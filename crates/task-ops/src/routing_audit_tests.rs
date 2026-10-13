use super::*;
use crate::add::{NewTaskSpec, create_task};
use task_core::{Event, SqliteStore};

fn new_task(store: &SqliteStore) -> task_core::Task {
    let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "t", "objective": "o",
        "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]
    }))
    .unwrap();
    create_task(store, spec, time::OffsetDateTime::now_utc()).unwrap()
}

#[test]
fn legacy_shadow_history_fixture_deserializes_and_appears_in_audit() {
    let event: Event = serde_json::from_str(include_str!(
        "../tests/fixtures/routing_shadow_recorded.json"
    ))
    .unwrap();
    let Event::RoutingShadowRecorded { record } = &event else {
        panic!("fixture must decode as history");
    };
    assert_eq!(record.shadow_id, "shadow-legacy-001");
    assert_eq!(record.request_id, None);
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(&store);
    let started = Event::WorkerStarted {
        run_id: "run-legacy-001".into(),
        adapter: "old".into(),
        model: "model-primary".into(),
        provider: None,
        account: None,
        role: None,
        task_role: None,
    };
    let audit = routing_audit_with_requests(&task, &[started, event], &store);
    let shadow = &audit.runs[0].routing_shadow.as_ref().unwrap()[0];
    assert_eq!(shadow.shadow_id, "shadow-legacy-001");
    assert_eq!(
        shadow.kind,
        task_core::routing_shadow_history::ShadowKind::Estimator
    );
}
