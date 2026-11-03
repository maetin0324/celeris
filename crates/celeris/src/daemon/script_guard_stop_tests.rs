//! 付記 S5-7 の停止基準（(f) の数値のまま）: ちょうどしきい値で戻る・手前で戻らない・安全違反で `invalid`・検証の外の
//! 副作用で全体を止める・自動で昇格しない・計数が再起動の後も残る。時計は固定の時刻から差し替える。

use task_core::script_compare::{
    CompareOutcome, CosDigestApplied, CosDigestAppliedBy, CosDigestCommandsSummary,
    CosDigestCompareRecord, NewScriptGeneration, ScriptDomain, ScriptGenerationState, ScriptMode,
    ScriptModeChange,
};
use task_core::script_guard::{ScriptGuardCall, ScriptGuardObservation, ScriptGuardOutcome};
use task_core::script_inbox::ScriptInboxStore;
use task_core::{ScriptInboxItem, SqliteStore};
use time::{Duration, OffsetDateTime};

use super::{GUARD_UNVALIDATED_EFFECT, GuardState, ingest, sweep, unvalidated_effect};

const HUMAN: &str = "human:alice";
const GUARD: &str = "system:script-guard";

/// 差し替えた時計の起点（2026-11-03T00:00:00Z）。
fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_793_664_000).expect("t0")
}

fn at(secs: i64) -> OffsetDateTime {
    t0() + Duration::seconds(secs)
}

fn generation(store: &SqliteStore, sha: char) -> String {
    store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha.to_string().repeat(64),
                host_api: "1".to_owned(),
                schema_sha256: "0".repeat(64),
                manifest_json: serde_json::json!({
                    "domains": ["lane_rules", "route_priority", "cos_digest"]
                })
                .to_string(),
                state: ScriptGenerationState::Loaded,
            },
            t0(),
        )
        .expect("generation")
        .id
}

fn change(
    store: &SqliteStore,
    domain: ScriptDomain,
    mode: ScriptMode,
    g: &str,
) -> ScriptModeChange {
    let row = store
        .script_domain_mode(domain.as_str())
        .expect("read mode");
    ScriptModeChange {
        domain: domain.as_str().to_owned(),
        to_mode: mode,
        to_generation: Some(g.to_owned()),
        expected_version: row.version,
        actor: HUMAN.to_owned(),
        reason: "test".to_owned(),
    }
}

fn set_compare(store: &SqliteStore, domain: ScriptDomain, g: &str) {
    store
        .script_domain_set_mode(&change(store, domain, ScriptMode::Compare, g), t0())
        .expect("compare");
}

fn obs(call: ScriptGuardCall, ts_side: bool) -> ScriptGuardObservation {
    ScriptGuardObservation {
        call,
        safety_violation: false,
        ts_side,
    }
}

fn observe(
    store: &SqliteStore,
    domain: ScriptDomain,
    g: &str,
    o: ScriptGuardObservation,
    now: OffsetDateTime,
) -> ScriptGuardOutcome {
    store
        .script_guard_observe(domain, g, o, now)
        .expect("observe")
}

fn observe_ok(store: &SqliteStore, domain: ScriptDomain, g: &str, n: u64, ts_side: bool) {
    for _ in 0..n {
        observe(store, domain, g, obs(ScriptGuardCall::Ok, ts_side), t0());
    }
}

/// 人が `compare` で 1000 件数えてから `canary` にする（S5-4 の進む条件）。
fn canary(store: &SqliteStore, domain: ScriptDomain, g: &str) {
    set_compare(store, domain, g);
    let required = match domain {
        ScriptDomain::CosDigest => 200,
        _ => 1000,
    };
    observe_ok(store, domain, g, required, false);
    store
        .script_domain_set_canary(&change(store, domain, ScriptMode::Canary, g), 5000, t0())
        .expect("canary");
}

fn mode(store: &SqliteStore, domain: ScriptDomain) -> (ScriptMode, String) {
    let row = store
        .script_domain_mode(domain.as_str())
        .expect("read mode");
    (row.mode, row.updated_by)
}

fn items(store: &SqliteStore) -> Vec<ScriptInboxItem> {
    ingest(store).expect("ingest");
    store.script_inbox_open().expect("open items")
}

/// 戻っていないこと（mode が `canary` のまま）を確かめ、戻ったら code を返す。
fn assert_still_canary(store: &SqliteStore, domain: ScriptDomain, out: &ScriptGuardOutcome) {
    assert!(out.rolled_back.is_empty(), "{out:?}");
    assert_eq!(mode(store, domain).0, ScriptMode::Canary);
}

fn assert_rolled_back(
    store: &SqliteStore,
    domain: ScriptDomain,
    out: &ScriptGuardOutcome,
    code: &str,
) {
    assert_eq!(out.rolled_back, vec![domain], "{out:?}");
    assert_eq!(out.stop.map(|s| s.code()), Some(code));
    assert_eq!(mode(store, domain), (ScriptMode::Rust, GUARD.to_owned()));
    let item = items(store)
        .into_iter()
        .find(|i| i.domain.as_deref() == Some(domain.as_str()) && i.code == code)
        .unwrap_or_else(|| panic!("inbox item for {code}"));
    assert_eq!(item.actor, GUARD);
    assert_eq!(item.change.as_deref(), Some("canary → rust"));
}

#[test]
fn script_guard_rolls_back_on_one_mismatch() {
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, ScriptDomain::LaneRules, &g);
    // 手前: 一致だけなら何件でも戻らない。
    for _ in 0..50 {
        let out = observe(
            &store,
            ScriptDomain::LaneRules,
            &g,
            obs(ScriptGuardCall::Ok, true),
            t0(),
        );
        assert_still_canary(&store, ScriptDomain::LaneRules, &out);
    }
    // ちょうど: 説明のつかない不一致 1 件で戻る（窓が 1000 件に届く前でも）。
    let out = observe(
        &store,
        ScriptDomain::LaneRules,
        &g,
        obs(ScriptGuardCall::Mismatch, false),
        t0(),
    );
    assert_rolled_back(&store, ScriptDomain::LaneRules, &out, "guard_mismatch");

    // `compare` は挙動が Rust のままなので mode を変えず、計数（昇格の前提）だけが崩れる。
    let h = generation(&store, 'b');
    set_compare(&store, ScriptDomain::RoutePriority, &h);
    observe_ok(&store, ScriptDomain::RoutePriority, &h, 10, false);
    let out = observe(
        &store,
        ScriptDomain::RoutePriority,
        &h,
        obs(ScriptGuardCall::Mismatch, false),
        t0(),
    );
    assert!(out.rolled_back.is_empty());
    assert_eq!(out.counters.clean_streak, 0);
    assert_eq!(
        mode(&store, ScriptDomain::RoutePriority).0,
        ScriptMode::Compare
    );
}

#[test]
fn script_guard_rolls_back_on_timeout_rate_and_burst() {
    let timeout = obs(ScriptGuardCall::Timeout, true);

    // burst 手前: 0 秒・300 秒・601 秒の 3 件は「10 分間に 3 件」に当たらない（0 秒の 1 件は 10 分を過ぎた）。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, ScriptDomain::LaneRules, &g);
    for secs in [0, 300, 601] {
        let out = observe(&store, ScriptDomain::LaneRules, &g, timeout, at(secs));
        assert_still_canary(&store, ScriptDomain::LaneRules, &out);
    }
    // burst ちょうど: 0 秒・300 秒・600 秒の 3 件（10 分の窓の端を含む）で戻る。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, ScriptDomain::LaneRules, &g);
    for secs in [0, 300] {
        let out = observe(&store, ScriptDomain::LaneRules, &g, timeout, at(secs));
        assert_still_canary(&store, ScriptDomain::LaneRules, &out);
    }
    let out = observe(&store, ScriptDomain::LaneRules, &g, timeout, at(600));
    assert_rolled_back(&store, ScriptDomain::LaneRules, &out, "guard_timeout_burst");

    // 率: 窓が 1000 件に届く前は 2 件でも率では戻らない。1000 件の窓で 1 件（0.1%）は戻らず、2 件（0.2%）で戻る。
    // timeout は 1 時間空けて置き、burst に当てない。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, ScriptDomain::LaneRules, &g);
    let d = ScriptDomain::LaneRules;
    observe(&store, d, &g, timeout, at(0));
    observe_ok(&store, d, &g, 500, true);
    let out = observe(&store, d, &g, timeout, at(3600));
    assert_still_canary(&store, d, &out);
    assert_eq!(out.counters.window.len(), 502);
    observe_ok(&store, d, &g, 497, true);
    // 999 件目まで: 2 件の timeout は窓に在るが、窓が 1000 件に届いていない。
    let out = observe(&store, d, &g, obs(ScriptGuardCall::Ok, true), at(3600));
    assert_eq!(out.counters.window.len(), 1000);
    assert_rolled_back(&store, d, &out, "guard_timeout_rate");

    // 1000 件の窓に 1 件だけなら戻らない（0.1% は超えていない）。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, d, &g);
    observe(&store, d, &g, timeout, at(0));
    observe_ok(&store, d, &g, 998, true);
    let out = observe(&store, d, &g, obs(ScriptGuardCall::Ok, true), at(3600));
    assert_eq!(
        (out.counters.window.len(), out.counters.timeouts_in_window()),
        (1000, 1)
    );
    assert_still_canary(&store, d, &out);
}

#[test]
fn script_guard_rolls_back_on_failure_rate_and_streak() {
    let failure = obs(ScriptGuardCall::Failure, true);
    let ok = obs(ScriptGuardCall::Ok, true);
    let d = ScriptDomain::RoutePriority;

    // 連続: 2 件は戻らず、間に成功が挟まれば数え直す。連続 3 件ちょうどで戻る。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, d, &g);
    for o in [failure, failure, ok, failure, failure] {
        let out = observe(&store, d, &g, o, t0());
        assert_still_canary(&store, d, &out);
    }
    let out = observe(&store, d, &g, failure, t0());
    assert_eq!(out.counters.consecutive_failures, 3);
    assert_rolled_back(&store, d, &out, "guard_failure_streak");

    // 率: 1000 件の窓で連続しない失敗 1 件は戻らず、2 件目で戻る。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, d, &g);
    // 窓は canary にした時点で空から数え直す。最古の 1 件を成功にして、2 件目の失敗で 1 件目が窓から落ちないようにする。
    observe(&store, d, &g, ok, t0());
    observe(&store, d, &g, failure, t0());
    observe_ok(&store, d, &g, 997, true);
    let out = observe(&store, d, &g, ok, t0());
    assert_eq!(
        (out.counters.window.len(), out.counters.failures_in_window()),
        (1000, 1)
    );
    assert_still_canary(&store, d, &out);
    let out = observe(&store, d, &g, failure, t0());
    assert_eq!(out.counters.failures_in_window(), 2);
    assert_rolled_back(&store, d, &out, "guard_failure_rate");
}

#[test]
fn script_guard_safety_violation_invalidates_generation() {
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, ScriptDomain::LaneRules, &g);
    set_compare(&store, ScriptDomain::RoutePriority, &g);
    // 手前: 違反 0 件では何も変わらない。
    observe_ok(&store, ScriptDomain::RoutePriority, &g, 20, false);
    assert_eq!(
        store
            .script_generation_get(&g)
            .expect("get")
            .map(|x| x.state),
        Some(ScriptGenerationState::Active)
    );
    // ちょうど: 1 件（`compare` の領域でも）で generation を `invalid` にし、指す全領域を `rust` に戻す。
    let out = observe(
        &store,
        ScriptDomain::RoutePriority,
        &g,
        ScriptGuardObservation {
            call: ScriptGuardCall::Ok,
            safety_violation: true,
            ts_side: false,
        },
        t0(),
    );
    assert_eq!(out.stop.map(|s| s.code()), Some("guard_safety_violation"));
    assert!(out.invalidated);
    assert_eq!(
        store
            .script_generation_get(&g)
            .expect("get")
            .map(|x| x.state),
        Some(ScriptGenerationState::Invalid)
    );
    for d in [ScriptDomain::LaneRules, ScriptDomain::RoutePriority] {
        assert_eq!(mode(&store, d), (ScriptMode::Rust, GUARD.to_owned()));
    }
    // 受信箱: 無効化の項目 1 件と、戻した領域ごとの項目。
    let open = items(&store);
    let invalidated: Vec<_> = open
        .iter()
        .filter(|i| i.event_type == "script_generation_invalidated")
        .collect();
    assert_eq!(invalidated.len(), 1, "{open:?}");
    assert_eq!(invalidated[0].generation_id.as_deref(), Some(g.as_str()));
    assert_eq!(invalidated[0].code, "guard_safety_violation");
    for d in [ScriptDomain::LaneRules, ScriptDomain::RoutePriority] {
        assert!(
            open.iter()
                .any(|i| i.domain.as_deref() == Some(d.as_str())
                    && i.code == "guard_safety_violation"),
            "{open:?}"
        );
    }
    // 2 回目の取り込みで増えない。
    assert_eq!(items(&store).len(), open.len());
}

fn digest_record(
    applied_by: CosDigestAppliedBy,
    violations: &[&str],
    run: &str,
) -> CosDigestCompareRecord {
    let summary = CosDigestCommandsSummary {
        commands_digest: "sha256:00".into(),
        command_kinds: vec!["post_digest".into()],
    };
    CosDigestCompareRecord {
        domain: ScriptDomain::CosDigest,
        generation: "g1".into(),
        isolate_epoch: 1,
        bundle_sha256: "ab".repeat(32),
        host_api: "1.0.0".into(),
        call_site: "cos_digest.pass".into(),
        mode: ScriptMode::Ts,
        pass_id: format!("p-{run}"),
        applied_by,
        fallback: None,
        ops: Vec::new(),
        thread_id: "t1".into(),
        run_id: run.into(),
        input_digest: "sha256:11".into(),
        input: None,
        rust: summary.clone(),
        ts: Some(summary),
        ts_commands_head: None,
        outcome: CompareOutcome::Match,
        error: None,
        normalized: Vec::new(),
        safety_violations: violations.iter().map(|v| (*v).to_owned()).collect(),
        elapsed_us: 5,
        applied: CosDigestApplied {
            message_id: "m1".into(),
            requeued: 0,
        },
        // 付記 S5-3: mode `ts` は全単位が TS の側（`unit_selected`）で、`canary_bps` は `canary` のときだけ持つ。
        canary_selected: true,
        canary_bps: None,
    }
}

#[test]
fn script_guard_unvalidated_effect_halts_all() {
    // 判定: TS を apply してよいのは canary・ts で検証を通った出力だけ。
    let valid = digest_record(CosDigestAppliedBy::Ts, &[], "r0");
    assert_eq!(unvalidated_effect(&valid), None);
    assert_eq!(
        unvalidated_effect(&digest_record(
            CosDigestAppliedBy::Rust,
            &["key_mismatch"],
            "r0"
        )),
        None
    );
    let mut compare_mode = valid.clone();
    compare_mode.mode = ScriptMode::Compare;
    assert_eq!(
        unvalidated_effect(&compare_mode),
        Some("ts_applied_outside_ts_mode")
    );
    assert_eq!(
        unvalidated_effect(&digest_record(
            CosDigestAppliedBy::Ts,
            &["key_mismatch"],
            "r0"
        )),
        Some("ts_applied_with_safety_violation")
    );
    let mut errored = valid.clone();
    errored.outcome = CompareOutcome::TsError;
    errored.ts = None;
    assert_eq!(
        unvalidated_effect(&errored),
        Some("ts_applied_without_valid_output")
    );

    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, ScriptDomain::LaneRules, &g);
    set_compare(&store, ScriptDomain::RoutePriority, &g);
    let mut state = GuardState::default();

    // 手前: 検証を通った TS の apply では止めない。
    assert!(
        store
            .script_compare_log_append(&valid, at(10))
            .expect("append")
    );
    let out = sweep(&store, &mut state, at(20)).expect("sweep");
    assert_eq!(out.halted, None);
    assert_eq!(mode(&store, ScriptDomain::LaneRules).0, ScriptMode::Canary);

    // ちょうど: 検証の外の副作用 1 件で全領域を `rust` に戻して止める。
    let bad = digest_record(CosDigestAppliedBy::Ts, &["key_mismatch"], "r1");
    assert!(
        store
            .script_compare_log_append(&bad, at(30))
            .expect("append")
    );
    let out = sweep(&store, &mut state, at(40)).expect("sweep");
    let reason = out.halted.expect("halted");
    assert!(reason.starts_with(GUARD_UNVALIDATED_EFFECT), "{reason}");
    assert!(reason.contains("run r1"), "{reason}");
    assert_eq!(
        out.rolled_back,
        vec![ScriptDomain::LaneRules, ScriptDomain::RoutePriority]
    );
    for d in [ScriptDomain::LaneRules, ScriptDomain::RoutePriority] {
        assert_eq!(mode(&store, d), (ScriptMode::Rust, GUARD.to_owned()));
    }
    assert_eq!(store.script_guard_halted().expect("halted"), Some(reason));
    let open = items(&store);
    assert_eq!(
        open.iter()
            .filter(|i| i.event_type == "script_guard_halted" && i.code == GUARD_UNVALIDATED_EFFECT)
            .count(),
        1,
        "{open:?}"
    );
    // 止まっている間は `rust` 以外へ進めない（S5-4）。
    assert!(
        store
            .script_domain_set_mode(
                &change(&store, ScriptDomain::LaneRules, ScriptMode::Compare, &g),
                at(50)
            )
            .is_err()
    );

    // 人が解いた後は、解く前の記録で再び止めない（再起動で cursor を失っても）。
    assert!(
        store
            .script_guard_clear(HUMAN, "checked", at(60))
            .expect("clear")
    );
    let mut restarted = GuardState::default();
    let out = sweep(&store, &mut restarted, at(70)).expect("sweep");
    assert_eq!(out.halted, None);
    assert_eq!(store.script_guard_halted().expect("halted"), None);
    // 解いた後の新しい記録では止める。
    let again = digest_record(CosDigestAppliedBy::Ts, &["scope_mismatch"], "r2");
    assert!(
        store
            .script_compare_log_append(&again, at(80))
            .expect("append")
    );
    let out = sweep(&store, &mut restarted, at(90)).expect("sweep");
    assert!(out.halted.expect("halted").contains("run r2"));
}

#[test]
fn script_guard_no_automatic_promotion() {
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    let mut state = GuardState::default();
    let ready = |store: &SqliteStore, state: &mut GuardState, d: ScriptDomain| {
        sweep(store, state, t0())
            .expect("sweep")
            .readiness
            .into_iter()
            .find(|r| r.domain == d)
            .expect("readiness")
    };
    let events = |store: &SqliteStore| {
        store
            .script_domain_events(None, 10_000)
            .expect("events")
            .len()
    };

    // 純粋判定: 999 件では進む条件を満たさず、1000 件で満たす。どちらでも mode は `compare` のまま。
    set_compare(&store, ScriptDomain::LaneRules, &g);
    observe_ok(&store, ScriptDomain::LaneRules, &g, 999, false);
    let r = ready(&store, &mut state, ScriptDomain::LaneRules);
    assert!(!r.readiness.ready);
    assert_eq!(
        (r.readiness.clean_streak, r.readiness.required),
        (999, 1000)
    );
    observe_ok(&store, ScriptDomain::LaneRules, &g, 1, false);
    let before = events(&store);
    for _ in 0..3 {
        let r = ready(&store, &mut state, ScriptDomain::LaneRules);
        assert!(r.readiness.ready, "{r:?}");
        assert_eq!(r.mode, ScriptMode::Compare);
    }
    assert_eq!(mode(&store, ScriptDomain::LaneRules).0, ScriptMode::Compare);
    assert_eq!(
        events(&store),
        before,
        "the sweep must not write a mode change"
    );

    // CoS の経路: 199 件では満たさず、200 件で満たす。
    set_compare(&store, ScriptDomain::CosDigest, &g);
    observe_ok(&store, ScriptDomain::CosDigest, &g, 199, false);
    assert!(
        !ready(&store, &mut state, ScriptDomain::CosDigest)
            .readiness
            .ready
    );
    observe_ok(&store, ScriptDomain::CosDigest, &g, 1, false);
    let r = ready(&store, &mut state, ScriptDomain::CosDigest);
    assert_eq!((r.readiness.ready, r.readiness.required), (true, 200));
    assert_eq!(mode(&store, ScriptDomain::CosDigest).0, ScriptMode::Compare);

    // canary→ts の条件は TS 側の単位で数える。Rust 側の影が 1000 件でも満たさず、mode は `canary` のまま。
    let h = generation(&store, 'b');
    canary(&store, ScriptDomain::RoutePriority, &h);
    observe_ok(&store, ScriptDomain::RoutePriority, &h, 1000, false);
    let r = ready(&store, &mut state, ScriptDomain::RoutePriority);
    assert_eq!((r.mode, r.readiness.ready), (ScriptMode::Canary, false));
    observe_ok(&store, ScriptDomain::RoutePriority, &h, 1000, true);
    let before = events(&store);
    let r = ready(&store, &mut state, ScriptDomain::RoutePriority);
    assert_eq!((r.mode, r.readiness.ready), (ScriptMode::Canary, true));
    assert_eq!(
        mode(&store, ScriptDomain::RoutePriority).0,
        ScriptMode::Canary
    );
    assert_eq!(events(&store), before);
}

#[test]
fn script_guard_counts_survive_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("celeris.db");
    let d = ScriptDomain::LaneRules;
    let g = {
        let store = SqliteStore::open(&path).expect("open");
        let g = generation(&store, 'a');
        canary(&store, d, &g);
        let timeout = obs(ScriptGuardCall::Timeout, true);
        observe(&store, d, &g, timeout, at(0));
        observe(&store, d, &g, timeout, at(100));
        let failure = obs(ScriptGuardCall::Failure, true);
        observe(&store, d, &g, failure, at(110));
        let out = observe(&store, d, &g, failure, at(120));
        assert_still_canary(&store, d, &out);
        g
    };
    // 再起動（store を開き直す）の後も、連続の失敗と 10 分の timeout の時刻が残る。
    let store = SqliteStore::open(&path).expect("reopen");
    let (counted, counters) = store
        .script_guard_counters(d, &g)
        .expect("counters")
        .expect("row");
    assert_eq!(counted, ScriptMode::Canary);
    assert_eq!(counters.consecutive_failures, 2);
    assert_eq!(counters.timeouts_10min.len(), 2);
    let out = observe(&store, d, &g, obs(ScriptGuardCall::Failure, true), at(130));
    assert_rolled_back(&store, d, &out, "guard_failure_streak");
}

/// mode `canary` の `cos_digest` の記録（付記 S5-3）。`canary_selected` が偽なら Rust 側の単位（TS は影で走り、
/// Rust の結果を使う）、真なら TS 側の単位。
fn canary_digest_record(
    g: &str,
    selected: bool,
    outcome: CompareOutcome,
    violations: &[&str],
    run: &str,
) -> CosDigestCompareRecord {
    let applied_by = if selected && violations.is_empty() && outcome == CompareOutcome::Match {
        CosDigestAppliedBy::Ts
    } else {
        CosDigestAppliedBy::Rust
    };
    let mut r = digest_record(applied_by, violations, run);
    r.generation = g.to_owned();
    r.mode = ScriptMode::Canary;
    r.outcome = outcome;
    r.canary_selected = selected;
    r.canary_bps = Some(5000);
    r
}

/// dispatcher と同じく、記録の追記と計数を apply と 1 つの transaction で行い（ADR 2026-11-03-cos-digest-persist）、
/// `ts_side` に `canary_selected` を渡す。command は無い（記録と計数だけを見る）。
fn append(store: &SqliteStore, r: &CosDigestCompareRecord) -> ScriptGuardOutcome {
    use task_core::chat::digest_apply::{
        CosDigestApplyPass, CosDigestApplyRun, CosDigestRecordPlan, CosDigestRunApply,
    };
    let pass = CosDigestApplyPass {
        pass_id: r.pass_id.clone(),
        verify: false,
        expect_mode: None,
        runs: vec![CosDigestApplyRun {
            thread_id: r.thread_id.clone(),
            run_id: r.run_id.clone(),
            commands: Vec::new(),
            record: Some(CosDigestRecordPlan::Append {
                record: Box::new(r.clone()),
                ts_side: r.canary_selected,
            }),
        }],
    };
    let applied = store.cos_digest_apply_pass(&pass, t0()).expect("append");
    match applied.runs.into_iter().next() {
        Some((
            _,
            CosDigestRunApply::Applied {
                record_appended: true,
                guard: Some(out),
                ..
            },
        )) => out,
        other => panic!("not appended and counted: {other:?}"),
    }
}

#[test]
fn script_guard_counts_canary_records_on_both_sides() {
    let d = ScriptDomain::CosDigest;

    // Rust 側の単位（canary_selected=false）の記録も数え、TS 側の連続には入れない。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'a');
    canary(&store, d, &g);
    let out = append(
        &store,
        &canary_digest_record(&g, false, CompareOutcome::Match, &[], "r0"),
    );
    assert_still_canary(&store, d, &out);
    let out = append(
        &store,
        &canary_digest_record(&g, true, CompareOutcome::Match, &[], "r1"),
    );
    assert_still_canary(&store, d, &out);
    let (counted, c) = store
        .script_guard_counters(d, &g)
        .expect("counters")
        .expect("row");
    assert_eq!(counted, ScriptMode::Canary);
    assert_eq!((c.calls, c.clean_streak, c.ts_side_clean_streak), (2, 2, 1));

    // Rust 側の単位の影の不一致 1 件でも `rust` へ戻す（(f) の「説明のつかない不一致 1 件」）。
    let out = append(
        &store,
        &canary_digest_record(&g, false, CompareOutcome::Mismatch, &[], "r2"),
    );
    assert_rolled_back(&store, d, &out, "guard_mismatch");

    // Rust 側の単位の安全違反でも generation を `invalid` にして戻す。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'b');
    canary(&store, d, &g);
    let out = append(
        &store,
        &canary_digest_record(&g, false, CompareOutcome::Match, &["key_mismatch"], "r0"),
    );
    assert_eq!(out.stop.map(|s| s.code()), Some("guard_safety_violation"));
    assert!(out.invalidated);
    assert_eq!(mode(&store, d), (ScriptMode::Rust, GUARD.to_owned()));
    assert_eq!(
        store
            .script_generation_get(&g)
            .expect("get")
            .map(|x| x.state),
        Some(ScriptGenerationState::Invalid)
    );

    // TS 側の単位の不一致でも戻す。
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'c');
    canary(&store, d, &g);
    let out = append(
        &store,
        &canary_digest_record(&g, true, CompareOutcome::Mismatch, &[], "r0"),
    );
    assert_rolled_back(&store, d, &out, "guard_mismatch");
}

/// P1-C（S5 未解決 6）: 見回りは `script_compare_log` を前の見回りが読み終えた id から差分だけ読む。page（500 行）を
/// またぐ行も読み切り、page の境の後ろにある検証の外の副作用でも止める。読み終えた行は次の見回りで読まない。
#[test]
fn script_guard_sweep_reads_compare_log_incrementally() {
    let store = SqliteStore::open_in_memory().expect("store");
    let g = generation(&store, 'b');
    canary(&store, ScriptDomain::LaneRules, &g);
    let mut state = GuardState::default();
    let mut n = 0u64;
    let mut append = |store: &SqliteStore, record: CosDigestCompareRecord, count: u64| {
        for _ in 0..count {
            n += 1;
            let mut r = record.clone();
            r.run_id = format!("run-{n}");
            r.pass_id = format!("pass-{n}");
            assert!(
                store
                    .script_compare_log_append(&r, at(n as i64))
                    .expect("append")
            );
        }
    };
    let valid = digest_record(CosDigestAppliedBy::Ts, &[], "r");
    append(&store, valid.clone(), 1_100);
    let out = sweep(&store, &mut state, at(2_000)).expect("sweep");
    assert_eq!(out.halted, None);
    assert_eq!(state.log_rows_read, 1_100);
    // 新しい行が無ければ読まない。
    sweep(&store, &mut state, at(2_001)).expect("sweep");
    assert_eq!(state.log_rows_read, 1_100);
    // 次の page の境（500 行）より後ろの違反でも止める。読むのは足した 501 行だけ。
    append(&store, valid, 500);
    append(
        &store,
        digest_record(CosDigestAppliedBy::Ts, &["key_mismatch"], "r"),
        1,
    );
    let out = sweep(&store, &mut state, at(3_000)).expect("sweep");
    assert_eq!(state.log_rows_read, 1_601);
    let reason = out.halted.expect("halted");
    assert!(reason.contains("run run-1601"), "{reason}");
    assert_eq!(mode(&store, ScriptDomain::LaneRules).0, ScriptMode::Rust);
}
