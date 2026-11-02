use time::OffsetDateTime;

use super::*;
use crate::model::Status;
use crate::script_compare::{CompareError, CompareOutcome};
use crate::store::TaskStore;
use crate::store::tests::sample_task;

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn change(
    domain: &str,
    to: ScriptMode,
    generation: Option<&str>,
    expected: u64,
) -> ScriptModeChange {
    ScriptModeChange {
        domain: domain.into(),
        to_mode: to,
        to_generation: generation.map(str::to_owned),
        expected_version: expected,
        actor: "human:web".into(),
        reason: "test".into(),
    }
}

fn loaded_generation(store: &SqliteStore, sha: &str, domains: &[&str]) -> ScriptGeneration {
    store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha.into(),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: serde_json::json!({ "domains": domains }).to_string(),
                state: ScriptGenerationState::Loaded,
            },
            now(),
        )
        .unwrap()
}

fn event_count(store: &SqliteStore) -> usize {
    store.script_domain_events(None, 100).unwrap().len()
}

#[test]
fn script_domain_initial_modes_are_rust() {
    let store = SqliteStore::open_in_memory().unwrap();
    let rows = store.script_domain_modes().unwrap();
    assert_eq!(
        rows.iter().map(|r| r.domain).collect::<Vec<_>>(),
        vec![
            ScriptDomain::CosDigest,
            ScriptDomain::LaneRules,
            ScriptDomain::RoutePriority
        ]
    );
    for r in &rows {
        assert_eq!(r.mode, ScriptMode::Rust);
        assert_eq!(r.generation_id, None);
        assert_eq!(r.version, 0);
    }
    assert_eq!(
        store.script_domain_active_generation("lane_rules").unwrap(),
        None
    );
    assert_eq!(event_count(&store), 0);
}

#[test]
fn script_domain_unknown_domain_is_rejected() {
    let store = SqliteStore::open_in_memory().unwrap();
    assert!(matches!(
        store.script_domain_mode("routing_shadow"),
        Err(ScriptDomainError::UnknownDomain(d)) if d == "routing_shadow"
    ));
    assert!(matches!(
        store.script_domain_active_generation("direct_route"),
        Err(ScriptDomainError::UnknownDomain(_))
    ));
    assert!(matches!(
        store.script_domain_set_mode(&change("nope", ScriptMode::Rust, None, 0), now()),
        Err(ScriptDomainError::UnknownDomain(_))
    ));
    assert_eq!(event_count(&store), 0);
}

#[test]
fn script_domain_canary_and_ts_are_rejected_without_change() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &"a".repeat(64), &["lane_rules"]);
    for to in [ScriptMode::Canary, ScriptMode::Ts] {
        let err = store
            .script_domain_set_mode(&change("lane_rules", to, Some(&g.id), 0), now())
            .unwrap_err();
        assert!(
            matches!(err, ScriptDomainError::ModeNotEnabled(m) if m == to),
            "{err}"
        );
    }
    // compare の後でも canary・ts へは進めない。
    store
        .script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap();
    for to in [ScriptMode::Canary, ScriptMode::Ts] {
        assert!(matches!(
            store.script_domain_set_mode(&change("lane_rules", to, Some(&g.id), 1), now()),
            Err(ScriptDomainError::ModeNotEnabled(_))
        ));
    }
    let row = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!((row.mode, row.version), (ScriptMode::Compare, 1));
    assert_eq!(event_count(&store), 1);
}

#[test]
fn script_domain_compare_cas_updates_row_and_appends_event() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &"b".repeat(64), &["lane_rules", "route_priority"]);
    let out = store
        .script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap();
    let ScriptModeChangeOutcome::Changed { row, event_id } = out else {
        panic!("expected Changed, got {out:?}");
    };
    assert_eq!(row.mode, ScriptMode::Compare);
    assert_eq!(row.generation_id.as_deref(), Some(g.id.as_str()));
    assert_eq!(row.version, 1);
    assert_eq!(row.updated_by, "human:web");
    assert_eq!(
        store.script_domain_active_generation("lane_rules").unwrap(),
        Some(g.clone())
    );
    // 他の領域は変わらない。
    assert_eq!(
        store.script_domain_mode("route_priority").unwrap().mode,
        ScriptMode::Rust
    );

    let events = store.script_domain_events(None, 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, event_id);
    assert_eq!(events[0].domain, ScriptDomain::LaneRules);
    assert_eq!(
        events[0].event,
        Event::ScriptDomainModeChanged {
            domain: ScriptDomain::LaneRules,
            from_mode: ScriptMode::Rust,
            to_mode: ScriptMode::Compare,
            from_generation: None,
            to_generation: Some(g.id.clone()),
            version: 1,
            actor: "human:web".into(),
            reason: "test".into(),
        }
    );
    let json = serde_json::to_value(&events[0].event).unwrap();
    assert_eq!(json["type"], "script_domain_mode_changed");

    // compare → rust（戻し）。
    let back = store
        .script_domain_set_mode(&change("lane_rules", ScriptMode::Rust, None, 1), now())
        .unwrap();
    assert!(
        matches!(back, ScriptModeChangeOutcome::Changed { ref row, .. }
        if row.mode == ScriptMode::Rust && row.generation_id.is_none() && row.version == 2)
    );
    assert_eq!(
        store
            .script_domain_events(Some(ScriptDomain::LaneRules), 10)
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .script_domain_events(Some(ScriptDomain::RoutePriority), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn script_domain_cas_conflict_leaves_row_and_events() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &"c".repeat(64), &["lane_rules"]);
    // 2 人が version 0 を読んだ。先の 1 人だけが通る。
    store
        .script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap();
    let err = store
        .script_domain_set_mode(&change("lane_rules", ScriptMode::Rust, None, 0), now())
        .unwrap_err();
    assert!(
        matches!(
            err,
            ScriptDomainError::VersionConflict {
                expected: 0,
                found: 1,
                ..
            }
        ),
        "{err}"
    );
    let row = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!((row.mode, row.version), (ScriptMode::Compare, 1));
    assert_eq!(event_count(&store), 1);
    // 先の version も競合（未来の version を名乗っても通らない）。
    assert!(matches!(
        store.script_domain_set_mode(&change("lane_rules", ScriptMode::Rust, None, 7), now()),
        Err(ScriptDomainError::VersionConflict { found: 1, .. })
    ));
}

#[test]
fn script_domain_cas_conflict_across_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite3");
    let a = SqliteStore::open(&path).unwrap();
    let b = SqliteStore::open(&path).unwrap();
    let g = loaded_generation(&a, &"d".repeat(64), &["route_priority"]);
    let read_a = a.script_domain_mode("route_priority").unwrap().version;
    let read_b = b.script_domain_mode("route_priority").unwrap().version;
    assert_eq!((read_a, read_b), (0, 0));
    a.script_domain_set_mode(
        &change("route_priority", ScriptMode::Compare, Some(&g.id), read_a),
        now(),
    )
    .unwrap();
    let mut c = change("route_priority", ScriptMode::Rust, None, read_b);
    c.actor = "cos:general".into();
    assert!(matches!(
        b.script_domain_set_mode(&c, now()),
        Err(ScriptDomainError::VersionConflict { .. })
    ));
    assert_eq!(b.script_domain_events(None, 10).unwrap().len(), 1);
}

/// 受け入れ条件 0: 表の更新と記録の追記は 1 つの transaction。記録の追記が落ちたら表も戻る。
#[test]
fn script_domain_mode_update_rolls_back_when_event_append_fails() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &"e".repeat(64), &["lane_rules"]);
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER test_fail_event BEFORE INSERT ON script_domain_events \
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
        .unwrap();
    let err = store
        .script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("injected"), "{err}");
    let row = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!(row.mode, ScriptMode::Rust);
    assert_eq!(row.generation_id, None);
    assert_eq!(row.version, 0);

    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER test_fail_event;")
        .unwrap();
    store
        .script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap();
    assert_eq!(event_count(&store), 1);
}

#[test]
fn script_domain_same_mode_is_unchanged() {
    let store = SqliteStore::open_in_memory().unwrap();
    let out = store
        .script_domain_set_mode(&change("lane_rules", ScriptMode::Rust, None, 0), now())
        .unwrap();
    assert!(matches!(out, ScriptModeChangeOutcome::Unchanged { ref row } if row.version == 0));
    assert_eq!(event_count(&store), 0);
}

#[test]
fn script_domain_compare_needs_loaded_generation_declaring_domain() {
    let store = SqliteStore::open_in_memory().unwrap();
    assert!(matches!(
        store.script_domain_set_mode(&change("lane_rules", ScriptMode::Compare, None, 0), now()),
        Err(ScriptDomainError::GenerationRequired)
    ));
    assert!(matches!(
        store.script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some("01JUNKNOWN"), 0),
            now()
        ),
        Err(ScriptDomainError::GenerationNotFound(_))
    ));
    let other = loaded_generation(&store, &"f".repeat(64), &["route_priority"]);
    assert!(matches!(
        store.script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&other.id), 0),
            now()
        ),
        Err(ScriptDomainError::DomainNotDeclared { .. })
    ));
    let rejected = store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: "9".repeat(64),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["lane_rules"]}"#.into(),
                state: ScriptGenerationState::Rejected,
            },
            now(),
        )
        .unwrap();
    assert!(matches!(
        store.script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&rejected.id), 0),
            now()
        ),
        Err(ScriptDomainError::GenerationNotLoaded(_))
    ));
    assert!(matches!(
        store.script_domain_set_mode(
            &change("lane_rules", ScriptMode::Rust, Some(&other.id), 0),
            now()
        ),
        Err(ScriptDomainError::GenerationNotAllowed)
    ));
    let row = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!((row.mode, row.version), (ScriptMode::Rust, 0));
    assert_eq!(event_count(&store), 0);
}

#[test]
fn script_domain_generation_reused_by_bundle_hash() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = loaded_generation(&store, &"1".repeat(64), &["lane_rules"]);
    let b = loaded_generation(&store, &"1".repeat(64), &["lane_rules"]);
    assert_eq!(a, b);
    assert_eq!(
        store.script_generation_by_sha256(&"1".repeat(64)).unwrap(),
        Some(a.clone())
    );
    assert!(a.declares_domain(ScriptDomain::LaneRules));
    assert!(!a.declares_domain(ScriptDomain::RoutePriority));
}

#[test]
fn script_domain_events_table_is_append_only() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &"2".repeat(64), &["lane_rules"]);
    store
        .script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap();
    let conn = store.lock().unwrap();
    assert!(
        conn.execute("UPDATE script_domain_events SET json = '{}'", [])
            .is_err()
    );
    assert!(
        conn.execute("DELETE FROM script_domain_events", [])
            .is_err()
    );
    // mode の CHECK: rust 以外は generation が要る、未知の mode は入らない。
    assert!(
        conn.execute(
            "UPDATE script_domain_modes SET mode = 'compare', generation_id = NULL WHERE domain = 'route_priority'",
            [],
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "UPDATE script_domain_modes SET mode = 'shadow' WHERE domain = 'route_priority'",
            [],
        )
        .is_err()
    );
}

fn record(generation: &str, digest: &str, outcome: CompareOutcome) -> ScriptCompareRecord {
    ScriptCompareRecord {
        domain: ScriptDomain::LaneRules,
        generation: generation.into(),
        isolate_epoch: 1,
        bundle_sha256: "3".repeat(64),
        host_api: "1.0.0".into(),
        call_site: "dispatch_run.decide_lane".into(),
        run_id: None,
        input_digest: digest.into(),
        input: Some(serde_json::json!({"features": {}})),
        input_truncated: false,
        rust: serde_json::json!({"rule_id": "standard/default"}),
        ts: None,
        outcome,
        error: (outcome == CompareOutcome::TsError)
            .then(|| CompareError::new("timeout", "deadline 50ms")),
        normalized: vec![],
        safety_violations: vec![],
        elapsed_us: 120,
    }
}

#[test]
fn script_compare_append_dedups_same_generation_digest_outcome() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Ready);
    store.create_task(&task, vec![]).unwrap();
    let before = store.events_for(task.id).unwrap().len();

    let r1 = record("G1", "sha256:aa", CompareOutcome::Match);
    assert!(store.script_compare_append(task.id, &r1, now()).unwrap());
    // 同じ generation・digest・outcome は足さない（elapsed などが違っても）。
    let mut r1b = r1.clone();
    r1b.elapsed_us = 999;
    assert!(!store.script_compare_append(task.id, &r1b, now()).unwrap());
    // outcome が変われば足す。
    let r2 = record("G1", "sha256:aa", CompareOutcome::TsError);
    assert!(store.script_compare_append(task.id, &r2, now()).unwrap());
    // 直前と比べるので、元に戻れば足す。
    assert!(store.script_compare_append(task.id, &r1, now()).unwrap());
    // 別の領域は別に数える。
    let mut other = r1.clone();
    other.domain = ScriptDomain::RoutePriority;
    other.call_site = "route_select.route_select_one".into();
    assert!(store.script_compare_append(task.id, &other, now()).unwrap());
    assert!(!store.script_compare_append(task.id, &r1, now()).unwrap());

    let events = store.events_for(task.id).unwrap();
    let compares: Vec<_> = events[before..]
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ScriptCompareRecorded { record } => Some(record.as_ref().clone()),
            _ => None,
        })
        .collect();
    assert_eq!(compares, vec![r1.clone(), r2, r1, other]);
    // 状態は変えない。
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Ready);
    // event の直列化は flatten した欄を持つ。
    let json = serde_json::to_value(Event::ScriptCompareRecorded {
        record: Box::new(compares[0].clone()),
    })
    .unwrap();
    assert_eq!(json["type"], "script_compare_recorded");
    assert_eq!(json["domain"], "lane_rules");
    assert_eq!(json["outcome"], "match");
}

/// 付記 S4-5: `script_compare_log` は追記専用で、同じ run・generation・input_digest・outcome の行を足さない。
#[test]
fn script_compare_log_appends_once_per_outcome_and_is_append_only() {
    use crate::script_compare::{
        CosDigestApplied, CosDigestCommandsSummary, CosDigestCompareRecord,
    };
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::UNIX_EPOCH;
    let summary = CosDigestCommandsSummary {
        commands_digest: "sha256:00".into(),
        command_kinds: vec!["post_digest".into()],
    };
    let record = CosDigestCompareRecord {
        domain: ScriptDomain::CosDigest,
        generation: "g1".into(),
        isolate_epoch: 1,
        bundle_sha256: "ab".repeat(32),
        host_api: "1.0.0".into(),
        call_site: "cos_digest.digest_one".into(),
        thread_id: "t1".into(),
        run_id: "r1".into(),
        input_digest: "sha256:11".into(),
        input: None,
        rust: summary.clone(),
        ts: Some(summary),
        ts_commands_head: None,
        outcome: CompareOutcome::Match,
        error: None,
        normalized: Vec::new(),
        safety_violations: Vec::new(),
        elapsed_us: 5,
        applied: CosDigestApplied {
            message_id: "m1".into(),
            requeued: 0,
        },
    };
    assert!(store.script_compare_log_append(&record, now).unwrap());
    assert!(!store.script_compare_log_append(&record, now).unwrap());
    let mut other = record.clone();
    other.outcome = CompareOutcome::Mismatch;
    assert!(store.script_compare_log_append(&other, now).unwrap());
    let rows = store.script_compare_log(Some("r1")).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].record, record);
    assert_eq!(rows[1].record, other);
    assert!(store.script_compare_log(Some("r2")).unwrap().is_empty());
    let conn = store.lock().unwrap();
    assert!(
        conn.execute("UPDATE script_compare_log SET json = '{}'", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM script_compare_log", []).is_err());
}
