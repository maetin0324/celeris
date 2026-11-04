use time::OffsetDateTime;

use super::*;
use crate::script_compare::{
    NewScriptGeneration, ScriptGeneration, ScriptGenerationState, ScriptModeChange,
};
use crate::script_exhaustive::LANE_RULES_EXHAUSTIVE_CALLS;
use crate::script_guard::{
    SCRIPT_GUARD_READY_PURE, SCRIPT_GUARD_READY_PURE_EXHAUSTIVE, ScriptGuardCall,
    ScriptGuardObservation,
};

fn reference() -> &'static str {
    current_reference_sha256().unwrap()
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn generation(store: &SqliteStore, c: char) -> ScriptGeneration {
    store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: c.to_string().repeat(64),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: serde_json::json!({ "domains": ["lane_rules"] }).to_string(),
                state: ScriptGenerationState::Loaded,
            },
            now(),
        )
        .unwrap()
}

fn change(to: ScriptMode, generation: Option<&str>, expected: u64) -> ScriptModeChange {
    ScriptModeChange {
        domain: "lane_rules".into(),
        to_mode: to,
        to_generation: generation.map(str::to_owned),
        expected_version: expected,
        actor: "human:web".into(),
        reason: "test".into(),
    }
}

fn version(store: &SqliteStore) -> u64 {
    store.script_domain_mode("lane_rules").unwrap().version
}

fn set_compare(store: &SqliteStore, g: &ScriptGeneration) {
    store
        .script_domain_set_mode(
            &change(ScriptMode::Compare, Some(&g.id), version(store)),
            now(),
        )
        .unwrap();
}

fn new_run(g: &ScriptGeneration, instance: &str) -> NewScriptExhaustiveRun {
    NewScriptExhaustiveRun {
        domain: ScriptDomain::LaneRules,
        bundle_sha256: g.bundle_sha256.clone(),
        daemon_instance_id: instance.into(),
        requested_by: "human:ryo".into(),
        reason: "test".into(),
        reference_sha256: reference().into(),
    }
}

fn good() -> ScriptExhaustiveCounts {
    ScriptExhaustiveCounts {
        calls: LANE_RULES_EXHAUSTIVE_CALLS,
        matches: LANE_RULES_EXHAUSTIVE_CALLS,
        latency_p50_us: 100,
        latency_p95_us: 300,
        latency_max_us: 900,
        ..Default::default()
    }
}

fn pass(store: &SqliteStore, g: &ScriptGeneration) -> ScriptExhaustiveRun {
    let run = store
        .script_exhaustive_begin(&new_run(g, "inst-1"), now())
        .unwrap();
    store
        .script_exhaustive_finish(
            &run.id,
            ScriptExhaustiveState::Passed,
            None,
            &good(),
            None,
            now(),
        )
        .unwrap()
        .unwrap()
        .run
}

fn observe_ok(store: &SqliteStore, g: &ScriptGeneration, n: u64, ts_side: bool) {
    for _ in 0..n {
        store
            .script_guard_observe(
                ScriptDomain::LaneRules,
                &g.id,
                ScriptGuardObservation {
                    call: ScriptGuardCall::Ok,
                    safety_violation: false,
                    ts_side,
                },
                now(),
            )
            .unwrap();
    }
}

fn standing(store: &SqliteStore, g: &ScriptGeneration) -> ScriptExhaustiveStanding {
    store
        .script_exhaustive_standing(ScriptDomain::LaneRules, &g.id, reference())
        .unwrap()
}

/// 今の mode と generation が違えば拒む。拒否は表を変えない。
#[test]
fn script_exhaustive_begin_checks_mode_and_generation() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    // mode が rust。
    assert!(matches!(
        store.script_exhaustive_begin(&new_run(&g, "i"), now()),
        Err(ScriptDomainError::ExhaustiveModeConflict(_))
    ));
    set_compare(&store, &g);
    // 別の bundle。
    let other = generation(&store, 'b');
    assert!(matches!(
        store.script_exhaustive_begin(&new_run(&other, "i"), now()),
        Err(ScriptDomainError::ExhaustiveModeConflict(_))
    ));
    // lane_rules 以外。
    let mut r = new_run(&g, "i");
    r.domain = ScriptDomain::CosDigest;
    assert!(matches!(
        store.script_exhaustive_begin(&r, now()),
        Err(ScriptDomainError::ExhaustiveDomainUnsupported(_))
    ));
    assert!(
        store
            .script_exhaustive_runs(ScriptDomain::LaneRules, None, 10)
            .unwrap()
            .is_empty()
    );
    let run = store
        .script_exhaustive_begin(&new_run(&g, "i"), now())
        .unwrap();
    assert_eq!(run.state, ScriptExhaustiveState::Running);
    assert_eq!(run.generation_id, g.id);
    assert_eq!(run.mode_at_start, "compare");
    assert_eq!(run.counts, ScriptExhaustiveCounts::default());
}

#[test]
fn script_exhaustive_begin_refuses_while_guard_halted() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    store.script_guard_halt("test halt", now()).unwrap();
    assert!(matches!(
        store.script_exhaustive_begin(&new_run(&g, "i"), now()),
        Err(ScriptDomainError::GuardHalted(_))
    ));
}

#[test]
fn script_exhaustive_one_running_per_domain() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let first = store
        .script_exhaustive_begin(&new_run(&g, "i"), now())
        .unwrap();
    match store.script_exhaustive_begin(&new_run(&g, "i"), now()) {
        Err(ScriptDomainError::ExhaustiveRunning(id)) => assert_eq!(id, first.id),
        other => panic!("expected ExhaustiveRunning, got {other:?}"),
    }
    // 終われば次を始められる。
    store
        .script_exhaustive_finish(
            &first.id,
            ScriptExhaustiveState::Aborted,
            Some(ScriptExhaustiveStopCode::WallLimit),
            &ScriptExhaustiveCounts::default(),
            None,
            now(),
        )
        .unwrap();
    store
        .script_exhaustive_begin(&new_run(&g, "i"), now())
        .unwrap();
}

#[test]
fn script_exhaustive_progress_updates_only_running() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let run = store
        .script_exhaustive_begin(&new_run(&g, "i"), now())
        .unwrap();
    let mid = ScriptExhaustiveCounts {
        calls: 1000,
        matches: 999,
        timeouts: 1,
        ..Default::default()
    };
    assert!(
        store
            .script_exhaustive_progress(&run.id, &mid, Some("{\"pass\":1}"))
            .unwrap()
    );
    let row = &store
        .script_exhaustive_runs(ScriptDomain::LaneRules, None, 10)
        .unwrap()[0];
    assert_eq!(row.counts, mid);
    assert_eq!(row.first_problem_json.as_deref(), Some("{\"pass\":1}"));
    store
        .script_exhaustive_finish(
            &run.id,
            ScriptExhaustiveState::Aborted,
            Some(ScriptExhaustiveStopCode::DaemonStopped),
            &mid,
            None,
            now(),
        )
        .unwrap();
    assert!(
        !store
            .script_exhaustive_progress(&run.id, &good(), None)
            .unwrap()
    );
}

/// 終わった行は UPDATE・DELETE できない。終了の CAS は 2 回目に None。
#[test]
fn script_exhaustive_terminal_rows_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let run = pass(&store, &g);
    assert_eq!(run.state, ScriptExhaustiveState::Passed);
    assert!(run.finished_at.is_some());
    // 2 回目の終了は書かない。
    assert!(
        store
            .script_exhaustive_finish(
                &run.id,
                ScriptExhaustiveState::Failed,
                Some(ScriptExhaustiveStopCode::Mismatch),
                &ScriptExhaustiveCounts::default(),
                None,
                now(),
            )
            .unwrap()
            .is_none()
    );
    let raw = rusqlite::Connection::open(&path).unwrap();
    assert!(
        raw.execute(
            "UPDATE script_exhaustive_runs SET state = 'failed' WHERE id = ?1",
            [&run.id]
        )
        .is_err()
    );
    assert!(
        raw.execute(
            "DELETE FROM script_exhaustive_runs WHERE id = ?1",
            [&run.id]
        )
        .is_err()
    );
    assert_eq!(
        store
            .script_exhaustive_runs(ScriptDomain::LaneRules, None, 10)
            .unwrap()[0]
            .state,
        ScriptExhaustiveState::Passed
    );
}

#[test]
fn script_exhaustive_passed_needs_verdict() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let run = store
        .script_exhaustive_begin(&new_run(&g, "i"), now())
        .unwrap();
    let mut short = good();
    short.calls -= 1;
    assert!(
        store
            .script_exhaustive_finish(
                &run.id,
                ScriptExhaustiveState::Passed,
                None,
                &short,
                None,
                now()
            )
            .is_err()
    );
    // 行は running のまま。
    assert_eq!(
        store
            .script_exhaustive_runs(ScriptDomain::LaneRules, None, 1)
            .unwrap()[0]
            .state,
        ScriptExhaustiveState::Running
    );
}

/// 受け入れ条件 1: 再起動（DB を開き直す）の後も証拠が読める。
#[test]
fn script_exhaustive_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite3");
    let gen_id;
    let run_id;
    {
        let store = SqliteStore::open(&path).unwrap();
        let g = generation(&store, 'a');
        set_compare(&store, &g);
        let run = pass(&store, &g);
        gen_id = g.id.clone();
        run_id = run.id;
        assert_eq!(standing(&store, &g), ScriptExhaustiveStanding::Passed);
    }
    let store = SqliteStore::open(&path).unwrap();
    let rows = store
        .script_exhaustive_runs(ScriptDomain::LaneRules, Some(&gen_id), 10)
        .unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, run_id);
    assert_eq!(row.state, ScriptExhaustiveState::Passed);
    assert_eq!(row.counts, good());
    assert_eq!(row.daemon_instance_id, "inst-1");
    assert_eq!(row.reference_sha256, reference());
    assert_eq!(
        store
            .script_exhaustive_standing(ScriptDomain::LaneRules, &gen_id, reference())
            .unwrap(),
        ScriptExhaustiveStanding::Passed
    );
}

/// 受け入れ条件 1: generation が変わる・参照が変わると、最新の証拠の読み出しは空（`Missing`）になる。
#[test]
fn script_exhaustive_generation_change_is_missing() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    pass(&store, &g);
    assert_eq!(standing(&store, &g), ScriptExhaustiveStanding::Passed);
    let g2 = generation(&store, 'b');
    assert_eq!(standing(&store, &g2), ScriptExhaustiveStanding::Missing);
    assert!(
        store
            .script_exhaustive_runs(ScriptDomain::LaneRules, Some(&g2.id), 10)
            .unwrap()
            .is_empty()
    );
    // 規則表が変わった binary（参照の違い）でも Missing。
    assert_eq!(
        store
            .script_exhaustive_standing(ScriptDomain::LaneRules, &g.id, "reference-b")
            .unwrap(),
        ScriptExhaustiveStanding::Missing
    );
    // 他の領域は対象外。
    assert_eq!(
        store
            .script_exhaustive_standing(ScriptDomain::CosDigest, &g.id, reference())
            .unwrap(),
        ScriptExhaustiveStanding::NotApplicable
    );
}

fn mismatch_counts() -> ScriptExhaustiveCounts {
    ScriptExhaustiveCounts {
        calls: 10,
        matches: 9,
        mismatches: 1,
        ..Default::default()
    }
}

/// canary にする。証拠の合格と live の 100 件を満たして昇格する。
fn to_canary(store: &SqliteStore, g: &ScriptGeneration) {
    set_compare(store, g);
    pass(store, g);
    observe_ok(store, g, SCRIPT_GUARD_READY_PURE_EXHAUSTIVE, false);
    store
        .script_domain_set_canary(
            &change(ScriptMode::Canary, Some(&g.id), version(store)),
            1000,
            now(),
        )
        .unwrap();
}

fn fail_with(
    store: &SqliteStore,
    g: &ScriptGeneration,
    state_counts: ScriptExhaustiveCounts,
    stop: ScriptExhaustiveStopCode,
) -> ScriptExhaustiveFinish {
    let run = store
        .script_exhaustive_begin(&new_run(g, "inst-2"), now())
        .unwrap();
    store
        .script_exhaustive_finish(
            &run.id,
            ScriptExhaustiveState::Failed,
            Some(stop),
            &state_counts,
            Some("{\"code\":\"x\"}"),
            now(),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn script_exhaustive_finish_rolls_back_canary_on_mismatch() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    to_canary(&store, &g);
    let events_before = store.script_domain_events(None, 100).unwrap().len();
    let out = fail_with(
        &store,
        &g,
        mismatch_counts(),
        ScriptExhaustiveStopCode::Mismatch,
    );
    assert_eq!(out.rolled_back, vec![ScriptDomain::LaneRules]);
    assert!(!out.invalidated);
    let mode = store.script_domain_mode("lane_rules").unwrap();
    assert_eq!(mode.mode, ScriptMode::Rust);
    assert_eq!(mode.generation_id, None);
    let events = store
        .script_domain_events(Some(ScriptDomain::LaneRules), 100)
        .unwrap();
    assert_eq!(events.len(), events_before + 1);
    let newest = format!("{:?}", events[0].event);
    assert!(newest.contains("system:script-guard"), "{newest}");
    assert!(newest.contains("guard_exhaustive_mismatch"), "{newest}");
    // 不一致の行は後に合格しても Failed のまま（generation の間は消えない）。
    assert_eq!(standing(&store, &g), ScriptExhaustiveStanding::Failed);
}

#[test]
fn script_exhaustive_finish_keeps_compare_on_mismatch() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let before = store.script_domain_events(None, 100).unwrap().len();
    let out = fail_with(
        &store,
        &g,
        mismatch_counts(),
        ScriptExhaustiveStopCode::Mismatch,
    );
    assert!(out.rolled_back.is_empty() && !out.invalidated);
    assert_eq!(
        store.script_domain_mode("lane_rules").unwrap().mode,
        ScriptMode::Compare
    );
    assert_eq!(store.script_domain_events(None, 100).unwrap().len(), before);
    assert_eq!(standing(&store, &g), ScriptExhaustiveStanding::Failed);
}

#[test]
fn script_exhaustive_finish_invalidates_on_safety_violation() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let counts = ScriptExhaustiveCounts {
        calls: 5,
        matches: 4,
        safety_violations: 1,
        ..Default::default()
    };
    let out = fail_with(
        &store,
        &g,
        counts,
        ScriptExhaustiveStopCode::SafetyViolation,
    );
    assert!(out.invalidated);
    assert_eq!(out.rolled_back, vec![ScriptDomain::LaneRules]);
    assert_eq!(
        store.script_generation_get(&g.id).unwrap().unwrap().state,
        ScriptGenerationState::Invalid
    );
    assert_eq!(
        store.script_domain_mode("lane_rules").unwrap().mode,
        ScriptMode::Rust
    );
}

/// timeout・失敗だけで落ちた実行は mode を変えない（D5）。
#[test]
fn script_exhaustive_finish_rate_failure_keeps_mode() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    to_canary(&store, &g);
    let counts = ScriptExhaustiveCounts {
        calls: 100,
        matches: 90,
        timeouts: 10,
        ..Default::default()
    };
    let out = fail_with(&store, &g, counts, ScriptExhaustiveStopCode::RateExceeded);
    assert!(out.rolled_back.is_empty() && !out.invalidated);
    assert_eq!(
        store.script_domain_mode("lane_rules").unwrap().mode,
        ScriptMode::Canary
    );
}

/// D8: 網羅は live の計数に触れない。
#[test]
fn script_exhaustive_does_not_touch_guard_counters() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    observe_ok(&store, &g, 3, false);
    let before = store
        .script_guard_counters(ScriptDomain::LaneRules, &g.id)
        .unwrap();
    assert!(before.is_some());
    let events_before = store.script_domain_events(None, 100).unwrap().len();
    pass(&store, &g);
    fail_with(
        &store,
        &g,
        mismatch_counts(),
        ScriptExhaustiveStopCode::Mismatch,
    );
    assert_eq!(
        store
            .script_guard_counters(ScriptDomain::LaneRules, &g.id)
            .unwrap(),
        before
    );
    assert_eq!(
        store.script_domain_events(None, 100).unwrap().len(),
        events_before
    );
}

#[test]
fn script_exhaustive_abort_stale_marks_other_instances() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let run = store
        .script_exhaustive_begin(&new_run(&g, "old"), now())
        .unwrap();
    // 同じ instance の行は触らない。
    assert_eq!(
        store.script_exhaustive_abort_stale("old", now()).unwrap(),
        0
    );
    assert_eq!(
        store.script_exhaustive_abort_stale("new", now()).unwrap(),
        1
    );
    let row = &store
        .script_exhaustive_runs(ScriptDomain::LaneRules, None, 1)
        .unwrap()[0];
    assert_eq!(row.id, run.id);
    assert_eq!(row.state, ScriptExhaustiveState::Aborted);
    assert_eq!(
        row.stop_code,
        Some(ScriptExhaustiveStopCode::DaemonRestarted)
    );
    // 古い daemon の終了は上書きできない。
    assert!(
        store
            .script_exhaustive_finish(
                &run.id,
                ScriptExhaustiveState::Passed,
                None,
                &good(),
                None,
                now()
            )
            .unwrap()
            .is_none()
    );
    // aborted は証拠に数えない。
    assert_eq!(standing(&store, &g), ScriptExhaustiveStanding::Missing);
}

/// D6: 昇格の gate が証拠の立場を読む。証拠が無ければ 100 件では足りず、合格なら 100 件で通る。
#[test]
fn script_domain_promote_uses_exhaustive_standing() {
    let reference = current_reference_sha256().unwrap();
    assert_eq!(reference.len(), 64);
    // 証拠なし: 100 件では 422 相当、required は 1000。
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    observe_ok(&store, &g, SCRIPT_GUARD_READY_PURE_EXHAUSTIVE, false);
    match store.script_domain_set_canary(
        &change(ScriptMode::Canary, Some(&g.id), version(&store)),
        1000,
        now(),
    ) {
        Err(ScriptDomainError::CanaryNotReady { detail, .. }) => {
            assert_eq!(detail.required, SCRIPT_GUARD_READY_PURE);
            assert_eq!(detail.exhaustive, ScriptExhaustiveStanding::Missing);
        }
        other => panic!("expected CanaryNotReady, got {other:?}"),
    }
    // 今の binary の参照で合格した証拠を足す: 100 件で通る。
    let run = store
        .script_exhaustive_begin(&new_run(&g, "i"), now())
        .unwrap();
    store
        .script_exhaustive_finish(
            &run.id,
            ScriptExhaustiveState::Passed,
            None,
            &good(),
            None,
            now(),
        )
        .unwrap();
    store
        .script_domain_set_canary(
            &change(ScriptMode::Canary, Some(&g.id), version(&store)),
            1000,
            now(),
        )
        .unwrap();
    assert_eq!(
        store.script_domain_mode("lane_rules").unwrap().mode,
        ScriptMode::Canary
    );
}

/// 参照の違う（古い規則表の）合格は昇格の前提に数えない。
#[test]
fn script_domain_promote_ignores_other_reference() {
    let store = SqliteStore::open_in_memory().unwrap();
    let g = generation(&store, 'a');
    set_compare(&store, &g);
    let mut r = new_run(&g, "i");
    r.reference_sha256 = "old-rule-table".into(); // 今の binary の値と違う。
    let run = store.script_exhaustive_begin(&r, now()).unwrap();
    store
        .script_exhaustive_finish(
            &run.id,
            ScriptExhaustiveState::Passed,
            None,
            &good(),
            None,
            now(),
        )
        .unwrap();
    observe_ok(&store, &g, SCRIPT_GUARD_READY_PURE_EXHAUSTIVE, false);
    assert!(matches!(
        store.script_domain_set_canary(
            &change(ScriptMode::Canary, Some(&g.id), version(&store)),
            1000,
            now(),
        ),
        Err(ScriptDomainError::CanaryNotReady { .. })
    ));
}
