//! A daemon bootstrap and dispatch tick must tolerate durable events written by the
//! pre-removal routing implementation.
use celeris::{ClusterMasters, Config, build_dispatcher};
use rusqlite::params;
use task_core::{
    Budget, Check, Criterion, Event, SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, Tier,
    WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

#[test]
fn startup_loads_legacy_routing_decided_event() {
    let temp = tempfile::tempdir().expect("temporary database directory");
    let db_path = temp.path().join("celeris.sqlite3");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "db = {:?}\nworkspace_root = {:?}\n",
            db_path.display().to_string(),
            temp.path().join("workspaces").display().to_string()
        ),
    )
    .expect("write config");
    let config = Config::load(&config_path).expect("load config");

    // Opening the temporary DB applies the production store migrations before fixture rows are added.
    let store = SqliteStore::open(&config.db.path).expect("open migrated store");
    let now = OffsetDateTime::now_utc();
    let task = Task {
        harness_id: None,
        requirements: Default::default(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "legacy routing fixture".into(),
        objective: "exercise daemon startup".into(),
        acceptance: vec![Criterion {
            text: "tick succeeds".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: temp.path().join("workspaces"),
            mode: None,
        },
        repos: vec![],
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 60,
            max_retries: 0,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        labels: vec![],
        category: Default::default(),
        skills: vec![],
        mode: Default::default(),
        conversation: None,
        routing: None,
        tree: None,
        paused_at: None,
    };
    store.insert(&task).expect("insert task row");

    // Preserve the old binary's durable JSON shape verbatim; only task_id is fixture-specific.
    let mut event = serde_json::json!({"type":"routing_decided", "run_id":"legacy-run", "record": {
        "org_node": null, "harness": null,
        "decision": {"lane":"standard", "proposed":"standard", "source":"default", "rule_id":"default", "policy_version":"lane-policy/1",
            "features":{"judgment":"low","ambiguity":"low","verifiability":"low","reversibility":"low","consequence":"low","context_size":"low","tool_intensity":"low","expected_length":"low","cross_cutting":"low"}, "reasons":[], "clamped_by":null,"hint":null,"escalation":null,"shadow":null},
        "resolution":{}, "quota_reason":null, "work_unit_id":null, "escalation":null
    }});
    event["record"]["optimizer"] = serde_json::json!({
        "decision_id":"legacy-decision", "task_id":task.id.to_string(), "run_id":"legacy-run", "stage":"dispatch", "mode":"legacy",
        "policy_version":"phase1-v1", "catalog_version":"legacy-catalog", "feature_version":"1", "estimator_version":"heuristic-1",
        "snapshot_id":"legacy-snapshot", "requested_lane":"standard", "candidates":[], "fallback_order":[], "reasons":[]
    });
    let payload = event.to_string();
    let conn = rusqlite::Connection::open(&config.db.path).expect("open fixture SQL connection");
    conn.execute(
        "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, 0, ?2, ?3)",
        params![
            task.id.to_string(),
            now.format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            payload
        ],
    )
    .expect("insert legacy durable payload");
    let snapshot = || {
        conn.query_row(
            "SELECT COUNT(*), MIN(json) FROM events WHERE task_id = ?1",
            [task.id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("read raw event")
    };
    let before = snapshot();
    assert_eq!(before, (1, payload.clone()));
    assert!(matches!(
        store.events_for(task.id).expect("read event").as_slice(),
        [(_, Event::RoutingDecided { .. })]
    ));

    // Use the exact daemon startup entry point, followed by its normal first dispatch tick.
    let mut dispatcher = build_dispatcher(&config, ClusterMasters::default())
        .expect("daemon bootstrap accepts legacy event");
    dispatcher.tick().expect("first dispatch tick");
    let after = snapshot();
    assert_eq!(
        after.0, before.0,
        "tick must not append an event for this draft task"
    );
    assert_eq!(after.1, before.1, "loading must not rewrite durable JSON");
    let legacy_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE json LIKE '%\"mode\":\"legacy\"%'",
            [],
            |row| row.get(0),
        )
        .expect("count legacy mode events");
    assert_eq!(legacy_count, 1, "legacy mode must not be newly written");
}
