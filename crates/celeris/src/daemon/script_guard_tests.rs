//! 付記 S5-7 の受信箱の節: 記録 1 件に項目 1 件・再起動後も増えない・人の actor では作らない。

use std::path::Path;

use task_core::script_compare::{
    NewScriptGeneration, ScriptDomain, ScriptGenerationState, ScriptGenerationTransition,
    ScriptMode, ScriptModeChange,
};
use task_core::script_inbox::ScriptInboxStore;
use task_core::{ScriptInboxItem, SqliteStore};
use time::OffsetDateTime;

use super::{ingest, reason_code};

const HUMAN: &str = "human:alice";
const GUARD: &str = "system:script-guard";
const STARTUP: &str = "system:script-startup";

fn open(path: &Path) -> SqliteStore {
    SqliteStore::open(path).expect("open store")
}

fn generation(store: &SqliteStore, sha: &str) -> String {
    store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha.to_owned(),
                host_api: "1".to_owned(),
                schema_sha256: "0".repeat(64),
                manifest_json: serde_json::json!({"domains": ["lane_rules", "route_priority"]})
                    .to_string(),
                state: ScriptGenerationState::Loaded,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("generation")
        .id
}

fn set_mode(
    store: &SqliteStore,
    domain: ScriptDomain,
    mode: ScriptMode,
    generation: Option<&str>,
    actor: &str,
    reason: &str,
) {
    let row = store
        .script_domain_mode(domain.as_str())
        .expect("read mode");
    store
        .script_domain_set_mode(
            &ScriptModeChange {
                domain: domain.as_str().to_owned(),
                to_mode: mode,
                to_generation: generation.map(str::to_owned),
                expected_version: row.version,
                actor: actor.to_owned(),
                reason: reason.to_owned(),
            },
            OffsetDateTime::now_utc(),
        )
        .expect("set mode");
}

/// drain の済んだ generation を `retired` にする（人でも script の actor でもない主体。項目を作らず閉じもしない）。
fn retire(store: &SqliteStore, id: &str) {
    store
        .script_generation_transition(
            &ScriptGenerationTransition {
                id: id.to_owned(),
                to: ScriptGenerationState::Retired,
                actor: "system:script-runtime".to_owned(),
                reason: "drained".to_owned(),
                cancelled_in_flight: Some(0),
            },
            OffsetDateTime::now_utc(),
        )
        .expect("retire");
}

fn open_items(store: &SqliteStore) -> Vec<ScriptInboxItem> {
    store.script_inbox_open().expect("open items")
}

/// 付記 S5-12 の `script_guard_inbox_item_once_per_event`（この unit では `script_inbox` の名前）。
#[test]
fn script_inbox_item_once_per_event_survives_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("celeris.db");
    let store = open(&path);
    let a = generation(&store, &"a".repeat(64));
    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Compare,
        Some(&a),
        HUMAN,
        "try",
    );
    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Rust,
        None,
        STARTUP,
        "script_generation_missing: bundle file not found",
    );
    let b = generation(&store, &"b".repeat(64));
    set_mode(
        &store,
        ScriptDomain::RoutePriority,
        ScriptMode::Compare,
        Some(&b),
        HUMAN,
        "try",
    );
    set_mode(
        &store,
        ScriptDomain::RoutePriority,
        ScriptMode::Rust,
        None,
        GUARD,
        "guard_mismatch: mismatch=1 calls=12",
    );

    let first = ingest(&store).expect("ingest");
    assert_eq!(first.opened, 2, "one item per system event");
    let items = open_items(&store);
    assert_eq!(items.len(), 2);
    let lane = items
        .iter()
        .find(|i| i.domain.as_deref() == Some("lane_rules"))
        .expect("lane item");
    assert_eq!(lane.actor, STARTUP);
    assert_eq!(lane.code, "script_generation_missing");
    assert_eq!(lane.generation_id.as_deref(), Some(a.as_str()));
    assert_eq!(lane.change.as_deref(), Some("compare → rust"));
    let route = items
        .iter()
        .find(|i| i.domain.as_deref() == Some("route_priority"))
        .expect("route item");
    assert_eq!(route.actor, GUARD);
    assert_eq!(route.code, "guard_mismatch");
    assert!(route.id.starts_with("domain-"), "{}", route.id);

    // 同じ process でもう一度読んでも増えない。
    assert_eq!(ingest(&store).expect("again").opened, 0);
    drop(store);

    // 再起動（store を開き直す）後も増えない。
    let store = open(&path);
    assert_eq!(ingest(&store).expect("after restart").opened, 0);
    assert_eq!(open_items(&store).len(), 2);
    drop(store);

    // cursor を失っても冪等 key（記録の表と event の id）で 2 件目を作らない。
    let conn = rusqlite::Connection::open(&path).expect("raw open");
    conn.execute("DELETE FROM script_inbox_cursors", [])
        .expect("drop cursors");
    drop(conn);
    let store = open(&path);
    assert_eq!(ingest(&store).expect("after cursor loss").opened, 0);
    assert_eq!(open_items(&store).len(), 2);
}

#[test]
fn script_inbox_human_actor_creates_no_item_and_closes_open_ones() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(&dir.path().join("celeris.db"));
    let a = generation(&store, &"a".repeat(64));
    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Compare,
        Some(&a),
        HUMAN,
        "try",
    );
    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Rust,
        None,
        HUMAN,
        "back",
    );
    retire(&store, &a);
    // CoS・operator の actor も人ではない（項目は作らないが、閉じもしない）。
    let c = generation(&store, &"c".repeat(64));
    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Compare,
        Some(&c),
        "cos:general",
        "try",
    );
    assert_eq!(ingest(&store).expect("ingest").opened, 0);
    assert!(open_items(&store).is_empty());

    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Rust,
        None,
        GUARD,
        "guard_failure_streak: consecutive=3",
    );
    retire(&store, &c);
    assert_eq!(ingest(&store).expect("guard").opened, 1);
    // 別の領域の人の変更では閉じない。
    let d = generation(&store, &"d".repeat(64));
    set_mode(
        &store,
        ScriptDomain::RoutePriority,
        ScriptMode::Compare,
        Some(&d),
        "operator:claude-code",
        "x",
    );
    set_mode(
        &store,
        ScriptDomain::RoutePriority,
        ScriptMode::Rust,
        None,
        HUMAN,
        "x",
    );
    retire(&store, &d);
    assert_eq!(ingest(&store).expect("other domain").resolved, 0);
    assert_eq!(open_items(&store).len(), 1);
    // 同じ領域の人の変更で閉じる。
    let e = generation(&store, &"e".repeat(64));
    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Compare,
        Some(&e),
        HUMAN,
        "retry",
    );
    let applied = ingest(&store).expect("human");
    assert_eq!((applied.opened, applied.resolved), (0, 1));
    assert!(open_items(&store).is_empty());
}

#[test]
fn script_inbox_generation_invalidated_and_halt_items() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(&dir.path().join("celeris.db"));
    let a = generation(&store, &"a".repeat(64));
    set_mode(
        &store,
        ScriptDomain::LaneRules,
        ScriptMode::Compare,
        Some(&a),
        HUMAN,
        "try",
    );
    store
        .script_generation_transition(
            &ScriptGenerationTransition {
                id: a.clone(),
                to: ScriptGenerationState::Invalid,
                actor: GUARD.to_owned(),
                reason: "guard_safety_violation: unvalidated command".to_owned(),
                cancelled_in_flight: None,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("invalidate");
    ingest(&store).expect("ingest");
    let items = open_items(&store);
    let gen_item = items
        .iter()
        .find(|i| i.event_type == "script_generation_invalidated")
        .expect("generation item");
    assert_eq!(gen_item.generation_id.as_deref(), Some(a.as_str()));
    assert_eq!(gen_item.code, "guard_safety_violation");
    assert!(gen_item.id.starts_with("generation-"));
    // 無効化で領域を rust に戻した mode の変更も 1 件（同じ actor・同じ code）。
    assert!(
        items.iter().any(
            |i| i.domain.as_deref() == Some("lane_rules") && i.code == "guard_safety_violation"
        )
    );

    store
        .script_guard_halt(
            "guard_unvalidated_effect: halted",
            OffsetDateTime::now_utc(),
        )
        .expect("halt");
    ingest(&store).expect("halt ingest");
    let halt = open_items(&store)
        .into_iter()
        .find(|i| i.event_type == super::SCRIPT_GUARD_HALTED)
        .expect("halt item");
    assert_eq!(halt.code, "guard_unvalidated_effect");

    store
        .script_guard_clear(HUMAN, "checked", OffsetDateTime::now_utc())
        .expect("clear");
    ingest(&store).expect("clear ingest");
    assert!(
        open_items(&store)
            .iter()
            .all(|i| i.event_type != super::SCRIPT_GUARD_HALTED),
        "the human clear closes the halt item"
    );
}

#[test]
fn script_inbox_reason_code_takes_leading_code() {
    assert_eq!(
        reason_code("guard_timeout_rate: 2/1000"),
        "guard_timeout_rate"
    );
    assert_eq!(
        reason_code("script_effect_unverified (run x)"),
        "script_effect_unverified"
    );
    assert_eq!(
        reason_code("  bundle_hash_mismatch"),
        "bundle_hash_mismatch"
    );
    assert_eq!(reason_code("理由なし"), "unknown");
}
