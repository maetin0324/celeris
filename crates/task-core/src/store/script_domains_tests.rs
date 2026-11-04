use time::OffsetDateTime;

use super::*;
use crate::model::Status;
use crate::script_compare::{CompareError, CompareOutcome};
use crate::script_guard::{
    self, ScriptGuardCall, ScriptGuardCounters, ScriptGuardObservation, ScriptGuardStop,
};
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
            ScriptDomain::ExecutionGate,
            ScriptDomain::LaneRules,
            ScriptDomain::ProjectController,
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

fn change_by(
    domain: &str,
    to: ScriptMode,
    generation: Option<&str>,
    expected: u64,
    actor: &str,
) -> ScriptModeChange {
    ScriptModeChange {
        actor: actor.into(),
        ..change(domain, to, generation, expected)
    }
}

/// S5-4: `ts` は人の actor だけ・同じ generation の `canary` からだけ（S4 の `compare→ts` は消した）。
#[test]
fn script_domain_ts_needs_human_and_same_generation_canary() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &"c".repeat(64), &["cos_digest", "route_priority"]);
    let g2 = loaded_generation(&store, &"d".repeat(64), &["cos_digest"]);
    // rust からの直行は拒む。
    assert!(matches!(
        store.script_domain_set_mode(&change("cos_digest", ScriptMode::Ts, Some(&g.id), 0), now()),
        Err(ScriptDomainError::RequiresPreviousStage {
            to: ScriptMode::Ts,
            current: ScriptMode::Rust
        })
    ));
    // generation が無い ts は拒む。
    assert!(matches!(
        store.script_domain_set_mode(&change("cos_digest", ScriptMode::Ts, None, 0), now()),
        Err(ScriptDomainError::GenerationRequired)
    ));
    store
        .script_domain_set_mode(
            &change("cos_digest", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap();
    // 人以外の actor は compare の後でも拒む。
    for actor in [
        "cos:01THREAD",
        "operator:claude-code",
        "mcp:client",
        "system:script-startup",
        "human:",
    ] {
        assert!(
            matches!(
                store.script_domain_set_mode(
                    &change_by("cos_digest", ScriptMode::Ts, Some(&g.id), 1, actor),
                    now()
                ),
                Err(ScriptDomainError::TsRequiresHuman(ref a)) if a == actor
            ),
            "{actor}"
        );
    }
    // generation を同時に変える要求は拒む。
    assert!(matches!(
        store.script_domain_set_mode(
            &change("cos_digest", ScriptMode::Ts, Some(&g2.id), 1),
            now()
        ),
        Err(ScriptDomainError::RequiresPreviousStage {
            to: ScriptMode::Ts,
            current: ScriptMode::Compare
        })
    ));
    // 他の領域の ts は compare から入れない（canary を経る。S5-4）。
    store
        .script_domain_set_mode(
            &change("route_priority", ScriptMode::Compare, Some(&g.id), 0),
            now(),
        )
        .unwrap();
    assert!(matches!(
        store.script_domain_set_mode(
            &change("route_priority", ScriptMode::Ts, Some(&g.id), 1),
            now()
        ),
        Err(ScriptDomainError::RequiresPreviousStage {
            to: ScriptMode::Ts,
            current: ScriptMode::Compare
        })
    ));
    // S5-4: cos_digest も人の actor で compare から ts へ直行できない（canary を経る）。
    assert!(matches!(
        store.script_domain_set_mode(&change("cos_digest", ScriptMode::Ts, Some(&g.id), 1), now()),
        Err(ScriptDomainError::RequiresPreviousStage {
            to: ScriptMode::Ts,
            current: ScriptMode::Compare
        })
    ));
    assert_eq!(event_count(&store), 2);
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
    let active = store
        .script_domain_active_generation("lane_rules")
        .unwrap()
        .unwrap();
    assert_eq!(
        (active.id.as_str(), active.state),
        (g.id.as_str(), ScriptGenerationState::Active)
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
        .script_generation_record_invalid(
            &NewScriptGeneration {
                bundle_sha256: "9".repeat(64),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["lane_rules"]}"#.into(),
                state: ScriptGenerationState::Invalid,
            },
            "selftest failed",
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
        applied_by: None,
        fallback: None,
        canary_selected: false,
        canary_bps: None,
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
        call_site: "cos_digest.pass".into(),
        mode: crate::script_compare::ScriptMode::Ts,
        pass_id: "p1".into(),
        applied_by: crate::script_compare::CosDigestAppliedBy::Ts,
        fallback: None,
        ops: Vec::new(),
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
        canary_selected: false,
        canary_bps: None,
        explained_by: None,
    };
    assert!(store.script_compare_log_append(&record, now).unwrap());
    assert!(!store.script_compare_log_append(&record, now).unwrap());
    let mut other = record.clone();
    other.outcome = CompareOutcome::Mismatch;
    assert!(store.script_compare_log_append(&other, now).unwrap());
    // S4-15: `applied_by` is part of the duplicate key.
    let mut by_rust = record.clone();
    by_rust.applied_by = crate::script_compare::CosDigestAppliedBy::Rust;
    assert!(store.script_compare_log_append(&by_rust, now).unwrap());
    assert!(!store.script_compare_log_append(&by_rust, now).unwrap());
    let rows = store.script_compare_log(Some("r1")).unwrap();
    assert_eq!(rows.len(), 3);
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

// ---- 付記 S5: generation の状態・canary・script-guard ----

fn sha(c: char) -> String {
    c.to_string().repeat(64)
}

const ALL_DOMAINS: &[&str] = &["lane_rules", "route_priority", "cos_digest"];

fn generation_events(store: &SqliteStore) -> Vec<ScriptGenerationEventRow> {
    store.script_generation_events_after(0, 1000).unwrap()
}

fn generation_kinds(store: &SqliteStore) -> Vec<ScriptGenerationEventKind> {
    generation_events(store)
        .into_iter()
        .map(|r| r.event.kind)
        .collect()
}

fn state_of(store: &SqliteStore, id: &str) -> ScriptGenerationState {
    store.script_generation_get(id).unwrap().unwrap().state
}

fn transition(id: &str, to: ScriptGenerationState) -> ScriptGenerationTransition {
    ScriptGenerationTransition {
        id: id.into(),
        to,
        actor: "human:web".into(),
        reason: "test".into(),
        cancelled_in_flight: None,
    }
}

fn observe(call: ScriptGuardCall, ts_side: bool) -> ScriptGuardObservation {
    ScriptGuardObservation {
        call,
        safety_violation: false,
        ts_side,
    }
}

fn set_compare(store: &SqliteStore, domain: &str, generation: &str) -> u64 {
    let version = store.script_domain_mode(domain).unwrap().version;
    store
        .script_domain_set_mode(
            &change(domain, ScriptMode::Compare, Some(generation), version),
            now(),
        )
        .unwrap();
    version + 1
}

/// `cos_digest` を compare にし、(f) の 200 件を数えて canary に進める。
fn canary_cos_digest(store: &SqliteStore, generation: &str, bps: u32) {
    let version = set_compare(store, "cos_digest", generation);
    for _ in 0..script_guard::SCRIPT_GUARD_READY_COS {
        store
            .script_guard_observe(
                ScriptDomain::CosDigest,
                generation,
                observe(ScriptGuardCall::Ok, false),
                now(),
            )
            .unwrap();
    }
    store
        .script_domain_set_canary(
            &change("cos_digest", ScriptMode::Canary, Some(generation), version),
            bps,
            now(),
        )
        .unwrap();
}

/// S5-1 の遷移の表の全組（5×5）。許されるのは表の 9 通りだけ。
#[test]
fn script_generation_state_transition_table() {
    use ScriptGenerationState::{Active, Draining, Invalid, Loaded, Retired};
    let allowed = [
        (Loaded, Active),
        (Loaded, Retired),
        (Loaded, Invalid),
        (Active, Draining),
        (Active, Invalid),
        (Draining, Retired),
        (Draining, Invalid),
        (Retired, Loaded),
        (Retired, Invalid),
    ];
    for from in ScriptGenerationState::ALL {
        for to in ScriptGenerationState::ALL {
            assert_eq!(
                from.may_become(to),
                allowed.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
        }
    }
    assert_eq!(ScriptGenerationState::parse("rejected"), None);
}

/// S5-1: 登録 → active → draining → retired → 再登録 → invalid、毎回 1 行追記する。
#[test]
fn script_generation_state_transitions() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), &["lane_rules", "route_priority"]);
    assert_eq!(g.state, ScriptGenerationState::Loaded);
    assert_eq!(
        generation_kinds(&store),
        vec![ScriptGenerationEventKind::ScriptGenerationLoaded]
    );

    set_compare(&store, "lane_rules", &g.id);
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Active);
    store
        .script_domain_set_mode(&change("lane_rules", ScriptMode::Rust, None, 1), now())
        .unwrap();
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Draining);

    let mut retire = transition(&g.id, ScriptGenerationState::Retired);
    retire.cancelled_in_flight = Some(2);
    store.script_generation_transition(&retire, now()).unwrap();
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Retired);

    // 退役した hash は再登録で loaded に戻る（行は増えない）。
    let again = loaded_generation(&store, &sha('a'), &["lane_rules", "route_priority"]);
    assert_eq!(again.id, g.id);
    assert_eq!(again.state, ScriptGenerationState::Loaded);

    store
        .script_generation_transition(&transition(&g.id, ScriptGenerationState::Invalid), now())
        .unwrap();
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Invalid);

    use ScriptGenerationEventKind as K;
    assert_eq!(
        generation_kinds(&store),
        vec![
            K::ScriptGenerationLoaded,
            K::ScriptGenerationActivated,
            K::ScriptGenerationDraining,
            K::ScriptGenerationRetired,
            K::ScriptGenerationLoaded,
            K::ScriptGenerationInvalidated,
        ]
    );
    let events = generation_events(&store);
    assert_eq!(events[3].event.cancelled_in_flight, Some(2));
    assert_eq!(
        events[3].event.from_state,
        Some(ScriptGenerationState::Draining)
    );
    assert_eq!(events[1].event.actor, "human:web");
}

/// 受け入れ条件 1: 不正な遷移は表も記録も変えない。
#[test]
fn script_domain_illegal_generation_transition_changes_nothing() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), &["lane_rules"]);
    set_compare(&store, "lane_rules", &g.id);
    let before = generation_events(&store);
    let row = store.script_generation_get(&g.id).unwrap();

    // active の退役は 409 の元。loaded・active・draining は store の口から書けない。
    assert!(matches!(
        store.script_generation_transition(
            &transition(&g.id, ScriptGenerationState::Retired),
            now()
        ),
        Err(ScriptDomainError::GenerationActive(_))
    ));
    for to in [
        ScriptGenerationState::Loaded,
        ScriptGenerationState::Active,
        ScriptGenerationState::Draining,
    ] {
        assert!(
            matches!(
                store.script_generation_transition(&transition(&g.id, to), now()),
                Err(ScriptDomainError::IllegalTransition { .. })
            ),
            "{to:?}"
        );
    }
    assert!(matches!(
        store.script_generation_transition(
            &transition("nope", ScriptGenerationState::Retired),
            now()
        ),
        Err(ScriptDomainError::GenerationNotFound(_))
    ));
    assert_eq!(store.script_generation_get(&g.id).unwrap(), row);
    assert_eq!(generation_events(&store), before);

    // invalid は終点。再登録も拒む（直した bundle は別の hash になる）。
    store
        .script_generation_transition(&transition(&g.id, ScriptGenerationState::Invalid), now())
        .unwrap();
    let before = generation_events(&store);
    for to in [
        ScriptGenerationState::Retired,
        ScriptGenerationState::Invalid,
    ] {
        assert!(matches!(
            store.script_generation_transition(&transition(&g.id, to), now()),
            Err(ScriptDomainError::IllegalTransition { .. })
        ));
    }
    assert!(matches!(
        store.script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha('a'),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: "{}".into(),
                state: ScriptGenerationState::Loaded,
            },
            now()
        ),
        Err(ScriptDomainError::GenerationInvalidated(_))
    ));
    assert_eq!(generation_events(&store), before);
}

/// S5-1: 検証に落ちた行は invalid で残り、上限に数えない。
#[test]
fn script_domain_generation_limit_two_live() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = loaded_generation(&store, &sha('a'), &["lane_rules"]);
    let _b = loaded_generation(&store, &sha('b'), &["lane_rules"]);
    let before = generation_events(&store);
    let third = NewScriptGeneration {
        bundle_sha256: sha('c'),
        host_api: "1.0.0".into(),
        schema_sha256: "0".repeat(64),
        manifest_json: "{}".into(),
        state: ScriptGenerationState::Loaded,
    };
    assert!(matches!(
        store.script_generation_record(&third, now()),
        Err(ScriptDomainError::GenerationLimit { live: 2, max: 2 })
    ));
    assert_eq!(generation_events(&store), before);
    assert_eq!(store.script_generations().unwrap().len(), 2);
    // 同じ hash の再登録は今の行を返す。invalid は上限に数えない。
    assert_eq!(
        loaded_generation(&store, &sha('a'), &["lane_rules"]).id,
        a.id
    );
    let invalid = store
        .script_generation_record_invalid(&third, "selftest failed", now())
        .unwrap();
    assert_eq!(invalid.state, ScriptGenerationState::Invalid);
    let last = generation_events(&store).pop().unwrap();
    assert_eq!(last.event.reason, "selftest failed");
    assert_eq!(last.event.from_state, None);

    // 退役させれば空く。
    store
        .script_generation_transition(&transition(&a.id, ScriptGenerationState::Retired), now())
        .unwrap();
    let d = NewScriptGeneration {
        bundle_sha256: sha('d'),
        ..third
    };
    store.script_generation_record(&d, now()).unwrap();
}

/// S5-2: A から B への切替は同じ transaction で B を active、A を draining にする。
/// 別の領域が A を指している間は A は active のまま。
#[test]
fn script_domain_mode_cas_moves_generation_states() {
    let store = SqliteStore::open_in_memory().unwrap();
    let domains = ["lane_rules", "route_priority"];
    let a = loaded_generation(&store, &sha('a'), &domains);
    let b = loaded_generation(&store, &sha('b'), &domains);
    set_compare(&store, "lane_rules", &a.id);
    set_compare(&store, "route_priority", &a.id);

    set_compare(&store, "lane_rules", &b.id);
    assert_eq!(state_of(&store, &a.id), ScriptGenerationState::Active);
    assert_eq!(state_of(&store, &b.id), ScriptGenerationState::Active);

    set_compare(&store, "route_priority", &b.id);
    assert_eq!(state_of(&store, &a.id), ScriptGenerationState::Draining);
    assert_eq!(state_of(&store, &b.id), ScriptGenerationState::Active);
    let active = store.script_domain_active_generation("lane_rules").unwrap();
    assert_eq!(active.unwrap().id, b.id);
    let all = store.script_domain_active_generations().unwrap();
    assert_eq!(
        all.iter()
            .map(|(row, g)| (row.domain, g.as_ref().map(|g| g.id.clone())))
            .collect::<Vec<_>>(),
        vec![
            (ScriptDomain::CosDigest, None),
            (ScriptDomain::ExecutionGate, None),
            (ScriptDomain::LaneRules, Some(b.id.clone())),
            (ScriptDomain::ProjectController, None),
            (ScriptDomain::RoutePriority, Some(b.id.clone())),
        ]
    );

    // draining へは戻れない（S5-4 の前提は loaded・active）。
    let version = store.script_domain_mode("lane_rules").unwrap().version;
    assert!(matches!(
        store.script_domain_set_mode(
            &change("lane_rules", ScriptMode::Compare, Some(&a.id), version),
            now()
        ),
        Err(ScriptDomainError::GenerationNotLoaded(_))
    ));
}

/// 受け入れ条件 1: CAS の競合は mode の表・generation の状態・両方の記録を変えない。
#[test]
fn script_domain_cas_conflict_leaves_generation_state_and_records() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = loaded_generation(&store, &sha('a'), &["cos_digest"]);
    let b = loaded_generation(&store, &sha('b'), &["cos_digest"]);
    canary_cos_digest(&store, &a.id, 100);
    let row = store.script_domain_mode("cos_digest").unwrap();
    let domain_events = store.script_domain_events(None, 100).unwrap();
    let gen_events = generation_events(&store);

    for (to, generation, bps) in [
        (ScriptMode::Compare, Some(b.id.as_str()), None),
        (ScriptMode::Rust, None, None),
        (ScriptMode::Canary, Some(a.id.as_str()), Some(200)),
    ] {
        let stale = change("cos_digest", to, generation, row.version - 1);
        let err = match bps {
            Some(bps) => store.script_domain_set_canary(&stale, bps, now()),
            None => store.script_domain_set_mode(&stale, now()),
        }
        .unwrap_err();
        assert!(
            matches!(err, ScriptDomainError::VersionConflict { .. }),
            "{err}"
        );
    }
    assert_eq!(store.script_domain_mode("cos_digest").unwrap(), row);
    assert_eq!(state_of(&store, &a.id), ScriptGenerationState::Active);
    assert_eq!(state_of(&store, &b.id), ScriptGenerationState::Loaded);
    assert_eq!(
        store.script_domain_events(None, 100).unwrap(),
        domain_events
    );
    assert_eq!(generation_events(&store), gen_events);
}

/// 状態の記録の追記が落ちたら mode の更新も戻る（1 つの transaction）。
#[test]
fn script_domain_mode_rolls_back_when_generation_event_fails() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), &["lane_rules"]);
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER test_fail_gen BEFORE INSERT ON script_generation_events \
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
    assert_eq!((row.mode, row.version), (ScriptMode::Rust, 0));
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Loaded);
    assert_eq!(event_count(&store), 0);
}

#[test]
fn script_generation_events_append_only() {
    let store = SqliteStore::open_in_memory().unwrap();
    loaded_generation(&store, &sha('a'), &["lane_rules"]);
    let conn = store.lock().unwrap();
    for sql in [
        "UPDATE script_generation_events SET json = '{}'",
        "DELETE FROM script_generation_events",
    ] {
        let err = conn.execute(sql, []).unwrap_err().to_string();
        assert!(err.contains("append-only"), "{sql}: {err}");
    }
}

/// S5-3: 表の CHECK（`canary` は 1〜9999、他は NULL）と store の入口の両方。
#[test]
fn script_domain_canary_bps_check() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), &["cos_digest"]);
    let version = set_compare(&store, "cos_digest", &g.id);
    {
        let conn = store.lock().unwrap();
        for sql in [
            "UPDATE script_domain_modes SET canary_bps = 100 WHERE domain = 'cos_digest'",
            "UPDATE script_domain_modes SET mode = 'canary' WHERE domain = 'cos_digest'",
            "UPDATE script_domain_modes SET mode = 'canary', canary_bps = 0 WHERE domain = 'cos_digest'",
            "UPDATE script_domain_modes SET mode = 'canary', canary_bps = 10000 WHERE domain = 'cos_digest'",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
        conn.execute(
            "UPDATE script_domain_modes SET mode = 'canary', canary_bps = 9999 WHERE domain = 'cos_digest'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE script_domain_modes SET mode = 'compare', canary_bps = NULL WHERE domain = 'cos_digest'",
            [],
        )
        .unwrap();
    }
    let canary = change("cos_digest", ScriptMode::Canary, Some(&g.id), version);
    for bad in [0, 10_000] {
        assert!(matches!(
            store.script_domain_set_canary(&canary, bad, now()),
            Err(ScriptDomainError::CanaryBpsInvalid(Some(b))) if b == bad
        ));
    }
    // canary は比率の要る口からだけ、比率は canary にだけ。
    assert!(matches!(
        store.script_domain_set_mode(&canary, now()),
        Err(ScriptDomainError::CanaryBpsInvalid(None))
    ));
    assert!(matches!(
        store.script_domain_set_canary(
            &change("cos_digest", ScriptMode::Compare, Some(&g.id), version),
            100,
            now()
        ),
        Err(ScriptDomainError::CanaryBpsInvalid(Some(100)))
    ));
    let row = store.script_domain_mode("cos_digest").unwrap();
    assert_eq!((row.mode, row.canary_bps), (ScriptMode::Compare, None));
}

/// S5-4: `rust→compare→canary→ts`、比率の変更、戻しは誰でも。戻すと `canary_bps` は NULL に戻る。
#[test]
fn script_domain_promote_through_canary_to_ts_and_back() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), &["lane_rules"]);
    let mut version = set_compare(&store, "lane_rules", &g.id);
    let observe_many = |n: u64, ts_side: bool| {
        for _ in 0..n {
            store
                .script_guard_observe(
                    ScriptDomain::LaneRules,
                    &g.id,
                    observe(ScriptGuardCall::Ok, ts_side),
                    now(),
                )
                .unwrap();
        }
    };
    let canary = |version| change("lane_rules", ScriptMode::Canary, Some(&g.id), version);

    // (f): 純粋判定は連続 1000 呼び出し。999 件では進めない。
    observe_many(999, false);
    let err = store
        .script_domain_set_canary(&canary(version), 500, now())
        .unwrap_err();
    assert!(
        matches!(&err, ScriptDomainError::CanaryNotReady { detail, .. }
            if detail.clean_streak == 999 && detail.required == 1000 && !detail.ready),
        "{err}"
    );
    observe_many(1, false);
    let outcome = store
        .script_domain_set_canary(&canary(version), 500, now())
        .unwrap();
    version += 1;
    let ScriptModeChangeOutcome::Changed { row, event_id } = outcome else {
        panic!("changed");
    };
    assert_eq!((row.mode, row.canary_bps), (ScriptMode::Canary, Some(500)));
    let recorded = store.script_domain_events(None, 1).unwrap().remove(0);
    assert_eq!((recorded.id, recorded.canary_bps), (event_id, Some(500)));
    assert!(matches!(
        recorded.event,
        Event::ScriptDomainModeChanged {
            to_mode: ScriptMode::Canary,
            ..
        }
    ));

    // 同じ比率は変更なし、別の比率は記録付きの変更。
    assert!(matches!(
        store.script_domain_set_canary(&canary(version), 500, now()),
        Ok(ScriptModeChangeOutcome::Unchanged { .. })
    ));
    store
        .script_domain_set_canary(&canary(version), 1000, now())
        .unwrap();
    version += 1;

    // canary→ts は TS 側の単位だけで数える（Rust 側の影は足りても進めない）。
    observe_many(1000, false);
    let to_ts = change("lane_rules", ScriptMode::Ts, Some(&g.id), version);
    assert!(matches!(
        store.script_domain_set_mode(&to_ts, now()),
        Err(ScriptDomainError::CanaryNotReady { .. })
    ));
    observe_many(1000, true);
    store.script_domain_set_mode(&to_ts, now()).unwrap();
    version += 1;
    let row = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!((row.mode, row.canary_bps), (ScriptMode::Ts, None));

    // 戻しは daemon の actor でもよい。
    let mut back = change("lane_rules", ScriptMode::Rust, None, version);
    back.actor = "system:script-startup".into();
    store.script_domain_set_mode(&back, now()).unwrap();
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Draining);
}

/// S5-4: 前の段を飛ばす遷移・人でない主体の昇格は表も記録も変えない。
#[test]
fn script_domain_stage_and_human_rules_change_nothing() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = loaded_generation(&store, &sha('a'), &["lane_rules", "cos_digest"]);
    let b = loaded_generation(&store, &sha('b'), &["lane_rules", "cos_digest"]);
    let count = |store: &SqliteStore| (event_count(store), generation_events(store).len());
    let before = count(&store);
    // rust から canary・ts へ直行できない。
    assert!(matches!(
        store.script_domain_set_canary(
            &change("lane_rules", ScriptMode::Canary, Some(&a.id), 0),
            100,
            now()
        ),
        Err(ScriptDomainError::RequiresPreviousStage {
            to: ScriptMode::Canary,
            current: ScriptMode::Rust
        })
    ));
    assert!(matches!(
        store.script_domain_set_mode(&change("lane_rules", ScriptMode::Ts, Some(&a.id), 0), now()),
        Err(ScriptDomainError::RequiresPreviousStage {
            to: ScriptMode::Ts,
            ..
        })
    ));
    assert_eq!(count(&store), before);
    assert_eq!(state_of(&store, &a.id), ScriptGenerationState::Loaded);

    let version = set_compare(&store, "lane_rules", &a.id);
    let before = count(&store);
    let row = store.script_domain_mode("lane_rules").unwrap();
    // 別の generation の canary、compare からの ts（lane_rules は S4 の例外の外）。
    assert!(matches!(
        store.script_domain_set_canary(
            &change("lane_rules", ScriptMode::Canary, Some(&b.id), version),
            100,
            now()
        ),
        Err(ScriptDomainError::RequiresPreviousStage { .. })
    ));
    assert!(matches!(
        store.script_domain_set_mode(
            &change("lane_rules", ScriptMode::Ts, Some(&a.id), version),
            now()
        ),
        Err(ScriptDomainError::RequiresPreviousStage { .. })
    ));
    // 人でない主体。
    for actor in [
        "cos:general",
        "operator:claude-code",
        "mcp:x",
        "system:script-guard",
    ] {
        let mut c = change("lane_rules", ScriptMode::Canary, Some(&a.id), version);
        c.actor = actor.into();
        assert!(matches!(
            store.script_domain_set_canary(&c, 100, now()),
            Err(ScriptDomainError::CanaryRequiresHuman(_))
        ));
    }
    assert_eq!(store.script_domain_mode("lane_rules").unwrap(), row);
    assert_eq!(count(&store), before);
}

/// S5-5: 保存済みの版を含まない generation へは移れない。今の 3 領域の V は空で通る。
#[test]
fn script_domain_checkpoint_versions_must_cover_stored() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha('a'),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: serde_json::json!({
                    "domains": ["lane_rules"],
                    "checkpoint_versions": [1],
                })
                .to_string(),
                state: ScriptGenerationState::Loaded,
            },
            now(),
        )
        .unwrap();
    assert_eq!(g.checkpoint_versions(), vec![1]);
    let compare = change("lane_rules", ScriptMode::Compare, Some(&g.id), 0);
    let err = store
        .set_mode_inner(&compare, None, &[1, 2], now())
        .unwrap_err();
    assert!(
        matches!(&err, ScriptDomainError::CheckpointIncompatible { stored, .. } if stored == &[1, 2]),
        "{err}"
    );
    assert_eq!(event_count(&store), 0);
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Loaded);
    store.set_mode_inner(&compare, None, &[1], now()).unwrap();
}

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap()
}

fn ok() -> ScriptGuardObservation {
    observe(ScriptGuardCall::Ok, false)
}

fn counted(calls: &[ScriptGuardCall], t: OffsetDateTime) -> ScriptGuardCounters {
    let mut c = ScriptGuardCounters::default();
    for call in calls {
        c.observe(observe(*call, false), t);
    }
    c
}

/// (f) の 4 行と 10 分・連続 3 件を、しきい値ちょうどと手前で確かめる。
#[test]
fn script_guard_evaluate_each_stop_condition() {
    use ScriptGuardCall::{Failure, Mismatch, Ok as Good, Timeout};
    let eval = |c: &ScriptGuardCounters, t| script_guard::evaluate(c, t);
    let t0 = at(0);

    // 説明のつかない不一致 1 件。
    assert_eq!(eval(&counted(&[Good; 5], t0), t0), None);
    assert_eq!(
        eval(&counted(&[Good, Mismatch], t0), t0),
        Some(ScriptGuardStop::GuardMismatch)
    );

    // 連続の失敗: 2 件は戻さず、3 件で戻す。間に成功があれば数え直す。
    assert_eq!(eval(&counted(&[Failure, Failure], t0), t0), None);
    assert_eq!(
        eval(&counted(&[Failure, Failure, Good, Failure], t0), t0),
        None
    );
    assert_eq!(
        eval(&counted(&[Failure, Failure, Failure], t0), t0),
        Some(ScriptGuardStop::GuardFailureStreak)
    );

    // 10 分間に timeout 3 件: 2 件は戻さない。600 秒ちょうどは窓の中、601 秒は外。
    let mut c = counted(&[Timeout, Timeout], t0);
    assert_eq!(eval(&c, t0), None);
    c.observe(observe(Timeout, false), at(600));
    assert_eq!(eval(&c, at(600)), Some(ScriptGuardStop::GuardTimeoutBurst));
    let mut c = counted(&[Timeout, Timeout], t0);
    c.observe(observe(Timeout, false), at(601));
    assert_eq!(c.timeouts_10min.len(), 1, "古い時刻は捨てる");
    assert_eq!(eval(&c, at(601)), None);

    // 率は 1000 件に届いた窓で判定する。0.1% 以下（1 件）は戻さず、超え（2 件）で戻す。
    let window = |bad: ScriptGuardCall, n: usize| {
        let mut calls = vec![Good; script_guard::SCRIPT_GUARD_WINDOW - n];
        // 連続や 10 分の条件に当たらないよう、離して混ぜ、窓の中に n 件置く。
        for i in 0..n {
            calls.insert(i * 100, bad);
        }
        calls
    };
    let long_ago = |calls: &[ScriptGuardCall]| {
        let mut c = ScriptGuardCounters::default();
        for (i, call) in calls.iter().enumerate() {
            let t = at(i64::try_from(i).unwrap() * 700);
            c.observe(observe(*call, false), t);
        }
        c
    };
    let end = at(700 * 1000);
    assert_eq!(eval(&long_ago(&window(Timeout, 1)), end), None);
    assert_eq!(
        eval(&long_ago(&window(Timeout, 2)), end),
        Some(ScriptGuardStop::GuardTimeoutRate)
    );
    assert_eq!(eval(&long_ago(&window(Failure, 1)), end), None);
    assert_eq!(
        eval(&long_ago(&window(Failure, 2)), end),
        Some(ScriptGuardStop::GuardFailureRate)
    );
    // 999 件の窓では率で戻さない。
    let mut short = window(Failure, 2);
    short.pop();
    assert_eq!(short.len(), 999);
    assert_eq!(eval(&long_ago(&short), end), None);

    // 安全違反 1 件。
    let mut c = ScriptGuardCounters::default();
    c.observe(
        ScriptGuardObservation {
            call: Good,
            safety_violation: true,
            ts_side: false,
        },
        t0,
    );
    assert_eq!(eval(&c, t0), Some(ScriptGuardStop::GuardSafetyViolation));

    // 窓は 1000 件で古いものから落ちる。
    let mut c = counted(&vec![Good; 1000], t0);
    c.observe(observe(Failure, false), t0);
    assert_eq!(c.window.len(), 1000);
    assert_eq!(c.failures_in_window(), 1);

    // S2-8 の分類。
    let classify = ScriptGuardCall::classify;
    assert_eq!(classify(CompareOutcome::Match, None), Some(Good));
    assert_eq!(classify(CompareOutcome::Mismatch, None), Some(Mismatch));
    for kind in ["timeout", "terminated"] {
        assert_eq!(classify(CompareOutcome::TsError, Some(kind)), Some(Timeout));
    }
    for kind in ["exception", "busy", "generation_gone"] {
        assert_eq!(classify(CompareOutcome::TsError, Some(kind)), Some(Failure));
    }
    assert_eq!(classify(CompareOutcome::InvalidOutput, None), Some(Failure));
    assert_eq!(classify(CompareOutcome::Unavailable, None), None);
    assert_eq!(
        classify(CompareOutcome::Explained, None),
        Some(ScriptGuardCall::Explained)
    );
}

/// ADR 2026-11-03-ts-intended-behavior-change D2: 説明済みの差分は戻す条件に入らず、昇格の前提の件数に入る。
/// 説明のつかない不一致は今までどおり 1 件で戻し、前提を 0 にする。
#[test]
fn script_guard_explained_counts_toward_readiness_but_mismatch_still_stops() {
    let t0 = now();
    let obs = |call, ts_side| ScriptGuardObservation {
        call,
        safety_violation: false,
        ts_side,
    };
    let mut c = ScriptGuardCounters::default();
    for i in 0..200 {
        let call = if i % 4 == 0 {
            ScriptGuardCall::Explained
        } else {
            ScriptGuardCall::Ok
        };
        c.observe(obs(call, true), t0);
    }
    assert_eq!(c.explained_in_window(), 50);
    assert_eq!(c.mismatches_in_window(), 0);
    assert_eq!(script_guard::evaluate(&c, t0), None);
    let ready = script_guard::readiness(
        ScriptDomain::CosDigest,
        ScriptMode::Canary,
        &c,
        crate::script_exhaustive::ScriptExhaustiveStanding::NotApplicable,
    );
    assert!(ready.ready, "{ready}");
    assert_eq!(ready.clean_streak, 200);
    // 安全違反は説明済みの行でも戻す（説明は安全違反を救わない）。
    let mut unsafe_c = c.clone();
    unsafe_c.observe(
        ScriptGuardObservation {
            call: ScriptGuardCall::Explained,
            safety_violation: true,
            ts_side: true,
        },
        t0,
    );
    assert_eq!(
        script_guard::evaluate(&unsafe_c, t0),
        Some(ScriptGuardStop::GuardSafetyViolation)
    );
    c.observe(obs(ScriptGuardCall::Mismatch, true), t0);
    assert_eq!(
        script_guard::evaluate(&c, t0),
        Some(ScriptGuardStop::GuardMismatch)
    );
    assert_eq!(c.clean_streak, 0);
    assert!(
        !script_guard::readiness(
            ScriptDomain::CosDigest,
            ScriptMode::Canary,
            &c,
            crate::script_exhaustive::ScriptExhaustiveStanding::NotApplicable
        )
        .ready
    );
}

/// generation ごとに別の行で数える。compare は計数だけで mode を変えない。
#[test]
fn script_guard_observe_counts_per_generation() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = loaded_generation(&store, &sha('a'), &["lane_rules"]);
    let b = loaded_generation(&store, &sha('b'), &["lane_rules"]);
    let version = set_compare(&store, "lane_rules", &a.id);
    let d = ScriptDomain::LaneRules;
    store.script_guard_observe(d, &a.id, ok(), now()).unwrap();
    store.script_guard_observe(d, &a.id, ok(), now()).unwrap();
    store.script_guard_observe(d, &b.id, ok(), now()).unwrap();
    let count = |id: &str| store.script_guard_counters(d, id).unwrap().unwrap().1.calls;
    assert_eq!((count(&a.id), count(&b.id)), (2, 1));

    let outcome = store
        .script_guard_observe(d, &a.id, observe(ScriptGuardCall::Mismatch, false), now())
        .unwrap();
    assert_eq!(outcome.stop, Some(ScriptGuardStop::GuardMismatch));
    assert!(outcome.rolled_back.is_empty());
    assert_eq!(outcome.counters.clean_streak, 0, "昇格の前提が崩れる");
    let row = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!((row.mode, row.version), (ScriptMode::Compare, version));
}

/// canary で条件に当たれば同じ transaction で `rust` に戻し、actor `system:script-guard`、reason に code。
#[test]
fn script_guard_observe_rolls_back_canary_with_guard_actor() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), &["cos_digest"]);
    canary_cos_digest(&store, &g.id, 500);
    let d = ScriptDomain::CosDigest;
    // 数え始めの mode が変わったので、compare の計数は引き継がない。
    let first = store.script_guard_observe(d, &g.id, ok(), now()).unwrap();
    assert_eq!(first.counters.calls, 1);
    let outcome = store
        .script_guard_observe(d, &g.id, observe(ScriptGuardCall::Mismatch, true), now())
        .unwrap();
    assert_eq!(outcome.rolled_back, vec![d]);
    assert!(!outcome.invalidated);
    let row = store.script_domain_mode("cos_digest").unwrap();
    assert_eq!(
        (row.mode, row.generation_id, row.canary_bps),
        (ScriptMode::Rust, None, None)
    );
    assert_eq!(row.updated_by, script_guard::SCRIPT_GUARD_ACTOR);
    let last = store.script_domain_events(None, 1).unwrap().remove(0);
    let Event::ScriptDomainModeChanged { actor, reason, .. } = last.event else {
        panic!("mode change");
    };
    assert_eq!(actor, "system:script-guard");
    assert!(reason.starts_with("guard_mismatch: "), "{reason}");
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Draining);

    // 自動で昇格も再開もしない。遅れて届いた結果は計数だけ。
    let before = event_count(&store);
    store.script_guard_observe(d, &g.id, ok(), now()).unwrap();
    assert_eq!(event_count(&store), before);
    assert_eq!(
        store.script_domain_mode("cos_digest").unwrap().mode,
        ScriptMode::Rust
    );
}

/// 安全違反は mode にかかわらず generation を invalid にし、指す全領域を戻す。
#[test]
fn script_guard_safety_violation_invalidates_generation() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), &["lane_rules", "route_priority"]);
    set_compare(&store, "lane_rules", &g.id);
    set_compare(&store, "route_priority", &g.id);
    let outcome = store
        .script_guard_observe(
            ScriptDomain::LaneRules,
            &g.id,
            ScriptGuardObservation {
                call: ScriptGuardCall::Ok,
                safety_violation: true,
                ts_side: false,
            },
            now(),
        )
        .unwrap();
    assert!(outcome.invalidated);
    assert_eq!(
        outcome.rolled_back,
        vec![ScriptDomain::LaneRules, ScriptDomain::RoutePriority]
    );
    assert_eq!(state_of(&store, &g.id), ScriptGenerationState::Invalid);
    for r in store.script_domain_modes().unwrap() {
        assert_eq!(r.mode, ScriptMode::Rust, "{r:?}");
    }
    let event = generation_events(&store).pop().unwrap().event;
    assert_eq!(
        event.kind,
        ScriptGenerationEventKind::ScriptGenerationInvalidated
    );
    assert_eq!(event.actor, "system:script-guard");
    assert!(event.reason.starts_with("guard_safety_violation: "));

    // invalid を指す行は戻せる（起動時の復元の形）。直接 invalid を指す行を作って確かめる。
    let h = loaded_generation(&store, &sha('b'), &["lane_rules"]);
    let version = set_compare(&store, "lane_rules", &h.id);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE script_generations SET state = 'invalid' WHERE id = ?1",
            [&h.id],
        )
        .unwrap();
    let mut back = change("lane_rules", ScriptMode::Rust, None, version);
    back.actor = "system:script-startup".into();
    store.script_domain_set_mode(&back, now()).unwrap();
    assert_eq!(state_of(&store, &h.id), ScriptGenerationState::Invalid);
}

/// 全体の停止: 全領域を rust に戻し、`rust` 以外を拒む。解くのは人だけ。
#[test]
fn script_domain_refused_while_guard_halted() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = loaded_generation(&store, &sha('a'), ALL_DOMAINS);
    let h = loaded_generation(&store, &sha('b'), ALL_DOMAINS);
    set_compare(&store, "lane_rules", &g.id);
    assert_eq!(store.script_guard_halted().unwrap(), None);
    assert_eq!(
        store
            .script_guard_halt("unvalidated effect", now())
            .unwrap(),
        vec![ScriptDomain::LaneRules]
    );
    assert_eq!(
        store.script_guard_halted().unwrap().as_deref(),
        Some("unvalidated effect")
    );
    assert!(store.script_guard_halt("again", now()).unwrap().is_empty());

    let version = store.script_domain_mode("lane_rules").unwrap().version;
    let compare = change("lane_rules", ScriptMode::Compare, Some(&h.id), version);
    let events = event_count(&store);
    assert!(matches!(
        store.script_domain_set_mode(&compare, now()),
        Err(ScriptDomainError::GuardHalted(r)) if r == "unvalidated effect"
    ));
    assert_eq!(event_count(&store), events);
    assert!(matches!(
        store.script_domain_set_mode(
            &change("lane_rules", ScriptMode::Rust, None, version),
            now()
        ),
        Ok(ScriptModeChangeOutcome::Unchanged { .. })
    ));

    for actor in ["cos:general", "operator:claude-code", "system:script-guard"] {
        assert!(matches!(
            store.script_guard_clear(actor, "x", now()),
            Err(ScriptDomainError::GuardClearRequiresHuman(_))
        ));
    }
    assert!(store.script_guard_halted().unwrap().is_some());
    assert!(
        store
            .script_guard_clear("human:web", "checked", now())
            .unwrap()
    );
    assert!(
        !store
            .script_guard_clear("human:web", "checked", now())
            .unwrap()
    );
    assert_eq!(store.script_guard_halted().unwrap(), None);
    store.script_domain_set_mode(&compare, now()).unwrap();

    use ScriptGenerationEventKind as K;
    let events = generation_events(&store);
    let halts: Vec<_> = events
        .iter()
        .filter(|r| r.generation_id == "*")
        .map(|r| (r.event.kind, r.event.actor.as_str()))
        .collect();
    assert_eq!(
        halts,
        vec![
            (K::ScriptGuardHalted, "system:script-guard"),
            (K::ScriptGuardCleared, "human:web"),
        ]
    );
}

/// 再起動（開き直し）の後も計数・停止が残る。10 分の窓も注入した時計で数える。
#[test]
fn script_guard_state_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guard.db");
    let id = {
        let store = SqliteStore::open(&path).unwrap();
        let g = loaded_generation(&store, &sha('a'), &["cos_digest"]);
        canary_cos_digest(&store, &g.id, 500);
        for secs in [0, 300] {
            store
                .script_guard_observe(
                    ScriptDomain::CosDigest,
                    &g.id,
                    observe(ScriptGuardCall::Timeout, true),
                    at(secs),
                )
                .unwrap();
        }
        g.id
    };
    let store = SqliteStore::open(&path).unwrap();
    let (mode, counters) = store
        .script_guard_counters(ScriptDomain::CosDigest, &id)
        .unwrap()
        .unwrap();
    assert_eq!(mode, ScriptMode::Canary);
    assert_eq!((counters.calls, counters.timeouts_10min.len()), (2, 2));
    assert_eq!(state_of(&store, &id), ScriptGenerationState::Active);
    let row = store.script_domain_mode("cos_digest").unwrap();
    assert_eq!((row.mode, row.canary_bps), (ScriptMode::Canary, Some(500)));
    // 再起動の前の 2 件と合わせて 3 件目で戻す。
    let outcome = store
        .script_guard_observe(
            ScriptDomain::CosDigest,
            &id,
            observe(ScriptGuardCall::Timeout, true),
            at(600),
        )
        .unwrap();
    assert_eq!(outcome.stop, Some(ScriptGuardStop::GuardTimeoutBurst));
    assert_eq!(outcome.rolled_back, vec![ScriptDomain::CosDigest]);

    store.script_guard_halt("stop", now()).unwrap();
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.script_guard_halted().unwrap().as_deref(),
        Some("stop")
    );
}

/// 追記と計数は 1 つの transaction。重複と `unavailable` は数えない（S2-8）。
#[test]
fn script_guard_append_observed_counts_after_dedup() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Draft);
    store.insert(&task).unwrap();
    let g = loaded_generation(&store, &sha('a'), &["lane_rules"]);
    let d = ScriptDomain::LaneRules;
    let (appended, outcome) = store
        .script_compare_append_observed(
            task.id,
            &record(&g.id, "d1", CompareOutcome::Match),
            false,
            now(),
        )
        .unwrap();
    assert!(appended);
    assert_eq!(outcome.unwrap().counters.calls, 1);
    let (appended, outcome) = store
        .script_compare_append_observed(
            task.id,
            &record(&g.id, "d1", CompareOutcome::Match),
            false,
            now(),
        )
        .unwrap();
    assert!(!appended);
    assert_eq!(outcome, None);
    let (appended, outcome) = store
        .script_compare_append_observed(
            task.id,
            &record(&g.id, "d2", CompareOutcome::Unavailable),
            false,
            now(),
        )
        .unwrap();
    assert!(appended);
    assert_eq!(outcome, None);
    assert_eq!(
        store
            .script_guard_counters(d, &g.id)
            .unwrap()
            .unwrap()
            .1
            .calls,
        1
    );

    // 計数の書き込みが落ちたら記録の追記も戻る。
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER test_fail_guard BEFORE UPDATE ON script_guard_state \
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
        .unwrap();
    let events = store.events_for(task.id).unwrap().len();
    assert!(
        store
            .script_compare_append_observed(
                task.id,
                &record(&g.id, "d3", CompareOutcome::Match),
                false,
                now()
            )
            .is_err()
    );
    assert_eq!(store.events_for(task.id).unwrap().len(), events);
}

/// migration 0104 までの DB を作り直し、0105 で行を失わず状態を移す。
#[test]
fn script_generation_rejected_rows_migrate_to_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        // 新しい表を消し、本物の 0103・0104 の SQL で移行前の形を作る。
        let store = SqliteStore::open(&path).unwrap();
        let conn = store.lock().unwrap();
        conn.execute_batch(
            "DROP TABLE script_guard_state; DROP TABLE script_generation_events; \
             DROP TABLE script_compare_log; DROP TABLE script_domain_events; \
             DROP TABLE script_domain_modes; DROP TABLE script_generations; \
             DELETE FROM schema_migrations WHERE version >= 103;",
        )
        .unwrap();
        conn.execute_batch(crate::store::migrations::MIGRATION_0103)
            .unwrap();
        conn.execute_batch(crate::store::migrations::MIGRATION_0104)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO schema_migrations (version, applied_at) VALUES \
                 (103, '2026-11-02T00:00:00Z'), (104, '2026-11-02T00:00:00Z'); \
             INSERT INTO script_generations VALUES \
                 ('G-ACTIVE', 'sha-active', '1.0.0', 's', '{\"domains\":[\"lane_rules\",\"cos_digest\"]}', 'loaded', '2026-11-01T00:00:00Z', '2026-11-01T00:00:01Z'), \
                 ('G-IDLE', 'sha-idle', '1.0.0', 's', '{\"domains\":[\"lane_rules\"]}', 'loaded', '2026-11-01T00:00:02Z', '2026-11-01T00:00:03Z'), \
                 ('G-REJECTED', 'sha-rejected', '1.0.0', 's', '{}', 'rejected', '2026-11-01T00:00:04Z', '2026-11-01T00:00:05Z'); \
             UPDATE script_domain_modes SET mode = 'compare', generation_id = 'G-ACTIVE', version = 3, \
                 updated_by = 'human:web' WHERE domain = 'lane_rules'; \
             UPDATE script_domain_modes SET mode = 'ts', generation_id = 'G-ACTIVE', version = 7, \
                 updated_by = 'human:web' WHERE domain = 'cos_digest'; \
             INSERT INTO script_domain_events (ts, domain, json) VALUES ('2026-11-01T00:00:06Z', 'lane_rules', \
                 '{\"type\":\"script_domain_mode_changed\",\"domain\":\"lane_rules\",\"from_mode\":\"rust\",\"to_mode\":\"compare\",\"from_generation\":null,\"to_generation\":\"G-ACTIVE\",\"version\":3,\"actor\":\"human:web\",\"reason\":\"old\"}');",
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    let rows = store.script_generations().unwrap();
    assert_eq!(
        rows.iter()
            .map(|g| (g.id.as_str(), g.state, g.updated_at.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (
                "G-ACTIVE",
                ScriptGenerationState::Active,
                "2026-11-01T00:00:01Z"
            ),
            (
                "G-IDLE",
                ScriptGenerationState::Retired,
                "2026-11-01T00:00:03Z"
            ),
            (
                "G-REJECTED",
                ScriptGenerationState::Invalid,
                "2026-11-01T00:00:05Z"
            ),
        ]
    );
    assert!(rows[0].declares_domain(ScriptDomain::CosDigest));

    // mode の行は欄を保ち、canary_bps は NULL。
    let lane = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!(
        (
            lane.mode,
            lane.generation_id.as_deref(),
            lane.version,
            lane.canary_bps
        ),
        (ScriptMode::Compare, Some("G-ACTIVE"), 3, None)
    );
    assert_eq!(store.script_domain_mode("cos_digest").unwrap().version, 7);
    assert_eq!(
        store
            .script_domain_active_generation("cos_digest")
            .unwrap()
            .unwrap()
            .id,
        "G-ACTIVE"
    );
    // 移行前の記録も読める（canary_bps の欄が無い）。
    let old = store.script_domain_events(None, 10).unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].canary_bps, None);

    // 移した状態は記録に残る。旧 rejected の語は残さない。
    let events = generation_events(&store);
    assert_eq!(
        events
            .iter()
            .map(|r| (r.generation_id.as_str(), r.event.kind, r.event.from_state))
            .collect::<Vec<_>>(),
        vec![
            (
                "G-ACTIVE",
                ScriptGenerationEventKind::ScriptGenerationActivated,
                Some(ScriptGenerationState::Loaded)
            ),
            (
                "G-IDLE",
                ScriptGenerationEventKind::ScriptGenerationRetired,
                Some(ScriptGenerationState::Loaded)
            ),
            (
                "G-REJECTED",
                ScriptGenerationEventKind::ScriptGenerationInvalidated,
                None
            ),
        ]
    );
    assert!(events.iter().all(|r| r.event.actor == "system:migration"));
    let rejected: i64 = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM script_generations WHERE state = 'rejected'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rejected, 0);

    // 退役に移した行は再登録で戻る（既知の bundle）。新しい表の CHECK は旧語を拒む。
    let idle = store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: "sha-idle".into(),
                host_api: "1.0.0".into(),
                schema_sha256: "s".into(),
                manifest_json: "{}".into(),
                state: ScriptGenerationState::Loaded,
            },
            now(),
        )
        .unwrap();
    assert_eq!(
        (idle.id.as_str(), idle.state),
        ("G-IDLE", ScriptGenerationState::Loaded)
    );
    assert!(
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE script_generations SET state = 'rejected' WHERE id = 'G-IDLE'",
                []
            )
            .is_err()
    );
}
