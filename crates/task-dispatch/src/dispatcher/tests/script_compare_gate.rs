//! ADR 2026-11-04-ts-execution-gate-domain: `execution_gate_if_needed` の compare・canary・ts と fail-closed・切替戻し。
//! 偽の `PolicyScript` だけを使い、V8 は起こさない。選択は task id の hash で決まるので、試験は task id を選んで側を作る。

// `script_compare.rs` の子 module（`#[path]`）。`super::super` が dispatcher の試験 module。
use super::super::lane_assist::{hinted_task, question_adapter};
use super::super::*;
use super::{Behavior, FakeScript, bundle_sha};
use crate::script_policy::{PolicyScript, PolicyScriptError};
use task_core::execution_gate::ExecutionGateDecision;
use task_core::script_compare::{
    CompareOutcome, CosDigestAppliedBy, NewScriptGeneration, ScriptCompareRecord, ScriptDomain,
    ScriptGenerationState, ScriptMode, ScriptModeChange, canary_selected,
};
use task_core::script_guard::{SCRIPT_GUARD_READY_PURE, ScriptGuardCall, ScriptGuardObservation};

const SECRET_TITLE: &str = "GATE-SECRET-TITLE";
const SECRET_OBJECTIVE: &str = "GATE-SECRET-OBJECTIVE を調査して設計して実装する";
const BPS: u32 = 5000;

#[derive(Clone, Copy, PartialEq)]
enum Setup {
    Rust,
    Compare,
    Canary,
    Ts,
}

fn change(generation: &str, to_mode: ScriptMode, expected_version: u64) -> ScriptModeChange {
    ScriptModeChange {
        domain: "execution_gate".into(),
        to_mode,
        to_generation: Some(generation.to_owned()),
        expected_version,
        actor: "human:test".into(),
        reason: "script_compare_gate test".into(),
    }
}

/// 人の actor で `setup` の mode まで上げる（昇格の前提の計数を積む）。generation の id を返す。
fn enter(store: &SqliteStore, setup: Setup) -> String {
    let now = OffsetDateTime::now_utc();
    let generation = store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: bundle_sha(),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["execution_gate"]}"#.into(),
                state: ScriptGenerationState::Loaded,
            },
            now,
        )
        .unwrap()
        .id;
    if setup == Setup::Rust {
        return generation;
    }
    store
        .script_domain_set_mode(&change(&generation, ScriptMode::Compare, 0), now)
        .unwrap();
    if setup == Setup::Compare {
        return generation;
    }
    let observe = |ts_side: bool| {
        for _ in 0..SCRIPT_GUARD_READY_PURE {
            store
                .script_guard_observe(
                    ScriptDomain::ExecutionGate,
                    &generation,
                    ScriptGuardObservation {
                        call: ScriptGuardCall::Ok,
                        safety_violation: false,
                        ts_side,
                    },
                    now,
                )
                .unwrap();
        }
    };
    let version = || store.script_domain_mode("execution_gate").unwrap().version;
    observe(false);
    store
        .script_domain_set_canary(
            &change(&generation, ScriptMode::Canary, version()),
            BPS,
            now,
        )
        .unwrap();
    if setup == Setup::Canary {
        return generation;
    }
    observe(true);
    store
        .script_domain_set_mode(&change(&generation, ScriptMode::Ts, version()), now)
        .unwrap();
    generation
}

fn gate_task(dir: &std::path::Path) -> Task {
    let mut task = hinted_task(dir, task_core::TierSource::Hint);
    task.title = SECRET_TITLE.into();
    task.objective = SECRET_OBJECTIVE.into();
    task
}

struct Out {
    decision: ExecutionGateDecision,
    records: Vec<ScriptCompareRecord>,
    mode: ScriptMode,
    clean_streak: u64,
    /// `execution_gated` の event の直列化（task id を除く）。
    gated_json: String,
    script: Option<Arc<FakeScript>>,
    _ws: tempfile::TempDir,
}

/// 1 件の task を 1 tick だけ dispatch する。`pick` が偽を返す task は使わない（canary の側を選ぶ）。
async fn run(
    setup: Setup,
    behavior: Option<Behavior>,
    edit: &dyn Fn(&mut Task),
    pick: &dyn Fn(&str, &Task) -> bool,
) -> Out {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let generation = enter(&sqlite, setup);
    let mut task = gate_task(ws.path());
    while !pick(&generation, &task) {
        task = gate_task(ws.path());
    }
    edit(&mut task);
    store.insert(&task).unwrap();
    let script = behavior.map(FakeScript::new);
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_script_domain_store(sqlite.clone());
    d.set_policy_script(script.clone().map(|s| s as Arc<dyn PolicyScript>));
    d.tick().unwrap();
    d.wait_script_compares().await;
    let mut records = Vec::new();
    let mut decision = None;
    for (_, event) in store.events_for(task.id).unwrap() {
        match event {
            Event::ScriptCompareRecorded { record } => records.push(*record),
            Event::ExecutionGated { decision: d } => decision = Some(*d),
            _ => {}
        }
    }
    let decision = decision.expect("execution_gated");
    let clean_streak = sqlite
        .script_guard_counters(ScriptDomain::ExecutionGate, &generation)
        .unwrap()
        .map_or(0, |(_, c)| c.clean_streak);
    Out {
        clean_streak,
        gated_json: serde_json::to_string(&decision).unwrap(),
        decision,
        records,
        mode: sqlite.script_domain_mode("execution_gate").unwrap().mode,
        script,
        _ws: ws,
    }
}

fn any_task(_: &str, _: &Task) -> bool {
    true
}

fn no_edit(_: &mut Task) {}

fn only(records: &[ScriptCompareRecord]) -> &ScriptCompareRecord {
    assert_eq!(records.len(), 1, "{records:?}");
    &records[0]
}

/// 記録の直列化に本文が無い（S2-3 の例外の条件。ADR D6）。
fn assert_no_text(record: &ScriptCompareRecord) {
    let json = serde_json::to_string(record).unwrap();
    assert!(!json.contains("GATE-SECRET"), "{json}");
}

/// Rust の判定と同じ mode・rule を持つが、`score`・`rule_id` を変えた妥当な TS の答え。
fn different_ts() -> serde_json::Value {
    serde_json::json!({
        "mode": "compound",
        "score": 9,
        "rule_id": "compound/score",
        "signals": [{ "name": "X1", "weight": 9, "detail": "ts only" }],
    })
}

async fn base() -> Out {
    run(Setup::Rust, None, &no_edit, &any_task).await
}

#[tokio::test]
async fn script_compare_gate_rust_mode_never_calls_ts() {
    let out = run(Setup::Rust, Some(Behavior::Echo), &no_edit, &any_task).await;
    assert_eq!(out.script.unwrap().gate_inputs.lock().unwrap().len(), 0);
    assert!(out.records.is_empty());
    assert_eq!(out.gated_json, base().await.gated_json);
}

#[tokio::test]
async fn script_compare_gate_compare_records_match_and_keeps_rust_bytes() {
    let out = run(Setup::Compare, Some(Behavior::Echo), &no_edit, &any_task).await;
    assert_eq!(out.gated_json, base().await.gated_json);
    let record = only(&out.records);
    assert_eq!(record.domain, ScriptDomain::ExecutionGate);
    assert_eq!(record.call_site, "work_units.execution_gate_if_needed");
    assert_eq!(record.outcome, CompareOutcome::Match);
    assert_eq!(record.applied_by, None);
    assert_no_text(record);
    let input = record.input.as_ref().expect("recorded input");
    assert!(input["text_sha256"].as_str().is_some_and(|d| !d.is_empty()));
    assert!(input.get("title").is_none() && input.get("objective").is_none());
    // TS は本文と Rust の閾値を受ける。
    let script = out.script.unwrap();
    let inputs = script.gate_inputs.lock().unwrap();
    assert_eq!(inputs[0].title, SECRET_TITLE);
    assert_eq!(inputs[0].objective, SECRET_OBJECTIVE);
    assert_eq!(inputs[0].threshold, out.decision.threshold);
}

#[tokio::test]
async fn script_compare_gate_compare_mismatch_keeps_rust_and_resets_the_streak() {
    let out = run(
        Setup::Compare,
        Some(Behavior::Fixed(different_ts())),
        &no_edit,
        &any_task,
    )
    .await;
    assert_eq!(out.gated_json, base().await.gated_json);
    assert_eq!(only(&out.records).outcome, CompareOutcome::Mismatch);
    // compare は Rust の判定のまま（戻す相手が無い）。不一致で連続の数が 0 に戻り、canary に上がれない。
    assert_eq!(out.mode, ScriptMode::Compare);
    assert_eq!(out.clean_streak, 0);
}

#[tokio::test]
async fn script_compare_gate_ts_uses_validated_output_but_rust_fields_stay() {
    let base = base().await;
    let out = run(
        Setup::Ts,
        Some(Behavior::Fixed(different_ts())),
        &no_edit,
        &any_task,
    )
    .await;
    assert_eq!(out.decision.mode, task_core::ExecutionMode::Compound);
    assert_eq!(out.decision.score, 9);
    assert_eq!(out.decision.rule_id, "compound/score");
    assert_eq!(out.decision.signals[0].name, "X1");
    // Rust が決める欄は TS の答えで変わらない。
    assert_eq!(out.decision.threshold, base.decision.threshold);
    assert_eq!(out.decision.source, base.decision.source);
    assert_eq!(out.decision.shadow, base.decision.shadow);
    assert_eq!(out.decision.depth, base.decision.depth);
    assert_eq!(out.decision.policy_version, base.decision.policy_version);
    let record = only(&out.records);
    assert_eq!(record.applied_by, Some(CosDigestAppliedBy::Ts));
    assert!(record.canary_selected);
    // 説明のつかない不一致 1 件で script-guard が同じ transaction で戻す（切替戻し）。
    assert_eq!(out.mode, ScriptMode::Rust);
}

/// TS の失敗・検証落ちでは Rust の判定を byte 一致で書く。
async fn assert_fails_closed(behavior: Behavior, code: &str, violation: Option<&str>) {
    let base = base().await;
    let out = run(Setup::Ts, Some(behavior), &no_edit, &any_task).await;
    assert_eq!(out.gated_json, base.gated_json, "{code}");
    let record = only(&out.records);
    assert_eq!(record.applied_by, Some(CosDigestAppliedBy::Rust), "{code}");
    assert_eq!(
        record.fallback.as_ref().map(|f| f.code.as_str()),
        Some(code)
    );
    assert_no_text(record);
    assert_eq!(
        record.safety_violations,
        violation.map(str::to_owned).into_iter().collect::<Vec<_>>()
    );
    if violation.is_some() {
        assert_eq!(
            out.mode,
            ScriptMode::Rust,
            "a safety violation stops at once"
        );
    }
}

#[tokio::test]
async fn script_compare_gate_ts_exception_falls_back_without_the_message() {
    let mut message = String::from("boom ");
    message.push_str(SECRET_OBJECTIVE);
    assert_fails_closed(
        Behavior::Fail(PolicyScriptError::new("exception", &message)),
        "ts_error",
        None,
    )
    .await;
}

#[tokio::test]
async fn script_compare_gate_ts_timeout_falls_back() {
    assert_fails_closed(Behavior::Hang, "ts_error", None).await;
}

#[tokio::test]
async fn script_compare_gate_ts_denied_falls_back() {
    let mut denied = PolicyScriptError::new("denied", "no");
    denied.safety_violation = Some("denied_capability".into());
    assert_fails_closed(
        Behavior::Fail(denied),
        "safety_violation",
        Some("denied_capability"),
    )
    .await;
}

#[tokio::test]
async fn script_compare_gate_invalid_output_falls_back_and_is_not_copied() {
    assert_fails_closed(
        Behavior::Fixed(serde_json::json!({ "mode": SECRET_OBJECTIVE })),
        "invalid_output",
        None,
    )
    .await;
}

/// `human/explicit`・対象外の rule_id は Rust だけが書く。TS が名乗ったら安全違反で使わない。
#[tokio::test]
async fn script_compare_gate_reserved_rule_is_a_safety_violation() {
    for rule_id in [
        "human/explicit",
        "atomic/out-of-scope",
        "atomic/knowledge-curation",
    ] {
        assert_fails_closed(
            Behavior::Fixed(serde_json::json!({
                "mode": "atomic", "score": 0, "rule_id": rule_id, "signals": [],
            })),
            "safety_violation",
            Some("gate_rule_not_allowed"),
        )
        .await;
    }
}

/// 閾値に合わない判定（深さの閾値を Rust が当てる。(a)）は安全違反で使わない。
#[tokio::test]
async fn script_compare_gate_threshold_contradiction_is_a_safety_violation() {
    for (mode, score, rule_id, weight) in [
        ("compound", 0, "compound/score", 0),
        ("atomic", 100, "atomic/score", 100),
        ("atomic", 100, "compound/score", 100),
        ("compound", 100, "compound/score", 1),
    ] {
        let signals = if weight == 0 {
            serde_json::json!([])
        } else {
            serde_json::json!([{ "name": "X", "weight": weight, "detail": "d" }])
        };
        assert_fails_closed(
            Behavior::Fixed(serde_json::json!({
                "mode": mode, "score": score, "rule_id": rule_id, "signals": signals,
            })),
            "safety_violation",
            Some("gate_threshold_violated"),
        )
        .await;
    }
}

#[tokio::test]
async fn script_compare_gate_runtime_missing_is_unavailable_and_rust_decides() {
    let base = base().await;
    let out = run(Setup::Ts, None, &no_edit, &any_task).await;
    assert_eq!(out.gated_json, base.gated_json);
    let record = only(&out.records);
    assert_eq!(record.outcome, CompareOutcome::Unavailable);
    assert_eq!(
        record.fallback.as_ref().map(|f| f.code.as_str()),
        Some("unavailable")
    );
}

/// canary は task id で側が決まる。Rust の側は影の比較だけで、TS の側だけ TS の答えを使う。
#[tokio::test]
async fn script_compare_gate_canary_splits_by_task_id() {
    let selected = |generation: &str, task: &Task| {
        canary_selected(
            ScriptDomain::ExecutionGate,
            generation,
            &task.id.to_string(),
            BPS,
        )
    };
    let matching = Behavior::Echo;
    let ts_side = run(Setup::Canary, Some(matching.clone()), &no_edit, &selected).await;
    let record = only(&ts_side.records);
    assert!(record.canary_selected);
    assert_eq!(record.applied_by, Some(CosDigestAppliedBy::Ts));
    assert_eq!(record.outcome, CompareOutcome::Match);
    assert_eq!(ts_side.mode, ScriptMode::Canary);

    let rust_side = run(Setup::Canary, Some(matching), &no_edit, &|g, t| {
        !selected(g, t)
    })
    .await;
    let record = only(&rust_side.records);
    assert!(!record.canary_selected);
    assert_eq!(record.applied_by, Some(CosDigestAppliedBy::Rust));
    assert_eq!(record.canary_bps, Some(BPS));
    assert_eq!(rust_side.mode, ScriptMode::Canary);
}

/// 人の明示と対象外の規則には TS を呼ばない（ADR D3）。
#[tokio::test]
async fn script_compare_gate_human_and_out_of_scope_never_reach_ts() {
    let human = |task: &mut Task| {
        if let Some(routing) = task.routing.as_mut() {
            routing.execution_hint = Some(task_core::ExecutionHintSpec {
                mode: task_core::ExecutionMode::Compound,
                explicit: true,
            });
        }
    };
    let out = run(Setup::Ts, Some(Behavior::Echo), &human, &any_task).await;
    assert_eq!(out.decision.rule_id, "human/explicit");
    assert_eq!(out.script.unwrap().gate_inputs.lock().unwrap().len(), 0);
    assert!(out.records.is_empty());

    let pipeline = |task: &mut Task| task.genre = Some("literature".into());
    let out = run(Setup::Ts, Some(Behavior::Echo), &pipeline, &any_task).await;
    assert_eq!(out.decision.rule_id, "atomic/out-of-scope");
    assert_eq!(out.script.unwrap().gate_inputs.lock().unwrap().len(), 0);
    assert!(out.records.is_empty());
}

/// 上限を超える本文は TS に渡さず、Rust の判定と `unavailable` を残す（ADR D9）。
#[tokio::test]
async fn script_compare_gate_oversized_text_does_not_reach_ts() {
    let huge = |task: &mut Task| {
        task.objective = "x".repeat(crate::script_policy::EXECUTION_GATE_TEXT_MAX + 1);
    };
    let out = run(Setup::Ts, Some(Behavior::Echo), &huge, &any_task).await;
    assert_eq!(out.script.unwrap().gate_inputs.lock().unwrap().len(), 0);
    let record = only(&out.records);
    assert_eq!(record.outcome, CompareOutcome::Unavailable);
    assert_eq!(record.applied_by, Some(CosDigestAppliedBy::Rust));
    assert_eq!(
        record.fallback.as_ref().map(|f| f.code.as_str()),
        Some("unavailable")
    );
}

/// 人が `rust` に戻すと、次の task から TS を呼ばない（切替戻し）。
#[tokio::test]
async fn script_compare_gate_switch_back_to_rust_stops_calling_ts() {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let generation = enter(&sqlite, Setup::Ts);
    let script = FakeScript::new(Behavior::Echo);
    let mut d = dispatcher(store.clone(), question_adapter(), 4);
    d.set_script_domain_store(sqlite.clone());
    d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    let first = gate_task(ws.path());
    store.insert(&first).unwrap();
    d.tick().unwrap();
    d.wait_script_compares().await;
    assert_eq!(script.gate_inputs.lock().unwrap().len(), 1);

    let version = sqlite.script_domain_mode("execution_gate").unwrap().version;
    let mut back = change(&generation, ScriptMode::Rust, version);
    back.to_generation = None;
    sqlite
        .script_domain_set_mode(&back, OffsetDateTime::now_utc())
        .unwrap();
    let second = gate_task(ws.path());
    store.insert(&second).unwrap();
    d.tick().unwrap();
    d.wait_script_compares().await;
    assert_eq!(script.gate_inputs.lock().unwrap().len(), 1);
    let gated = store
        .events_for(second.id)
        .unwrap()
        .into_iter()
        .any(|(_, e)| matches!(e, Event::ExecutionGated { .. }));
    assert!(gated);
}
