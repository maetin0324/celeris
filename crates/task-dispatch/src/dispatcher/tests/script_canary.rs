//! 付記 S5-3・S5-6・S5-8: dispatcher の canary（`lane_rules`・`route_priority` と 3 領域の選択、tick の予算、
//! 戻しの書き込みの失敗）。偽の `PolicyScript` だけを使い、V8 は起こさない。選択は決定的な hash なので、
//! 試験は task id を選んで TS の側・Rust の側を作る（乱数・実時間の sleep を使わない）。

use super::lane_assist::{hinted_task, question_adapter, routing_decided};
use super::script_compare::{Behavior, FakeScript, bundle_sha};
use super::*;
use crate::dispatcher::dispatch_run::script_canary_unit_selected;
use crate::script_policy::{PolicyScript, PolicyScriptError, ScriptDomainStore};
use task_core::script_compare::{
    CompareOutcome, CosDigestAppliedBy, NewScriptGeneration, ScriptCompareRecord, ScriptDomain,
    ScriptDomainError, ScriptGenerationState, ScriptMode, ScriptModeChange, canary_selected,
};
use task_core::script_guard::{
    SCRIPT_GUARD_ACTOR, SCRIPT_GUARD_READY_PURE, ScriptGuardCall, ScriptGuardObservation,
};

const BPS: u32 = 5000;

/// `lane_rules` を `compare` → `canary`（`BPS`）にする（人の actor・昇格の前提の計数を積む）。generation の id を返す。
fn enter_lane_canary(store: &SqliteStore) -> String {
    let now = OffsetDateTime::now_utc();
    let generation = store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: bundle_sha(),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["lane_rules"]}"#.into(),
                state: ScriptGenerationState::Loaded,
            },
            now,
        )
        .unwrap();
    let change = |to_mode, expected_version| ScriptModeChange {
        domain: "lane_rules".into(),
        to_mode,
        to_generation: Some(generation.id.clone()),
        expected_version,
        actor: "human:test".into(),
        reason: "script_canary test".into(),
    };
    store
        .script_domain_set_mode(&change(ScriptMode::Compare, 0), now)
        .unwrap();
    for _ in 0..SCRIPT_GUARD_READY_PURE {
        store
            .script_guard_observe(
                ScriptDomain::LaneRules,
                &generation.id,
                ScriptGuardObservation {
                    call: ScriptGuardCall::Ok,
                    safety_violation: false,
                    ts_side: false,
                },
                now,
            )
            .unwrap();
    }
    let version = store.script_domain_mode("lane_rules").unwrap().version;
    store
        .script_domain_set_canary(&change(ScriptMode::Canary, version), BPS, now)
        .unwrap();
    generation.id
}

/// TS の側（`ts_side`）か Rust の側に選ばれる task を作る（選択は task id の hash で決まる）。
fn task_on_side(dir: &std::path::Path, generation: &str, ts_side: bool) -> Task {
    loop {
        let task = hinted_task(dir, task_core::TierSource::Hint);
        if canary_selected(
            ScriptDomain::LaneRules,
            generation,
            &task.id.to_string(),
            BPS,
        ) == ts_side
        {
            return task;
        }
    }
}

struct Outcome {
    decision: task_core::LaneDecision,
    records: Vec<ScriptCompareRecord>,
    mode: task_core::script_compare::ScriptDomainModeRow,
    /// actor `system:script-guard` の mode の変更の `reason`。
    guard_reasons: Vec<String>,
    _ws: tempfile::TempDir,
}

/// mode `canary` の `lane_rules` で 1 件の task を 1 tick だけ dispatch する。`canary` が偽なら mode `rust`・
/// PolicyScript 無し（基準）。
async fn run_lane(canary: bool, ts_side: bool, script: Option<Arc<FakeScript>>) -> Outcome {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let task = if canary {
        let generation = enter_lane_canary(&sqlite);
        task_on_side(ws.path(), &generation, ts_side)
    } else {
        hinted_task(ws.path(), task_core::TierSource::Hint)
    };
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_script_domain_store(sqlite.clone());
    d.set_policy_script(script.map(|s| s as Arc<dyn PolicyScript>));
    d.tick().unwrap();
    d.wait_script_compares().await;
    let records = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ScriptCompareRecorded { record } => Some(*record),
            _ => None,
        })
        .collect();
    Outcome {
        decision: routing_decided(&store, &task).decision,
        records,
        mode: sqlite.script_domain_mode("lane_rules").unwrap(),
        guard_reasons: guard_reasons(&sqlite),
        _ws: ws,
    }
}

/// actor `system:script-guard` の mode の変更の `reason`（S5-6・S5-7 の戻し。受信箱の項目はこの記録から作る）。
pub(super) fn guard_reasons(store: &SqliteStore) -> Vec<String> {
    store
        .script_domain_events(None, 1000)
        .unwrap()
        .into_iter()
        .filter_map(|row| match row.event {
            Event::ScriptDomainModeChanged { actor, reason, .. } if actor == SCRIPT_GUARD_ACTOR => {
                Some(reason)
            }
            _ => None,
        })
        .collect()
}

fn only(records: &[ScriptCompareRecord]) -> &ScriptCompareRecord {
    assert_eq!(records.len(), 1, "{records:?}");
    &records[0]
}

fn frontier() -> serde_json::Value {
    serde_json::json!({
        "rule_id": "frontier/always",
        "proposed": "frontier",
        "condition": "always frontier",
    })
}

/// 3 領域とも、同じ (domain, generation, unit) は同じ選択で、比率 0 は誰も・10000 は全員を選ぶ。
/// generation が変われば対象の集合が変わる。mode `ts` は全部、`compare` は誰も TS の側にしない。
#[test]
fn canary_selection_is_deterministic_per_unit() {
    for domain in [
        ScriptDomain::LaneRules,
        ScriptDomain::RoutePriority,
        ScriptDomain::CosDigest,
    ] {
        let units: Vec<String> = (0..400).map(|i| format!("01UNIT{i:05}")).collect();
        let pick = |generation: &str, bps| -> Vec<bool> {
            units
                .iter()
                .map(|u| canary_selected(domain, generation, u, bps))
                .collect()
        };
        assert_eq!(pick("G1", BPS), pick("G1", BPS), "{domain:?}");
        assert!(pick("G1", 0).iter().all(|s| !s));
        assert!(pick("G1", 10_000).iter().all(|s| *s));
        let half = pick("G1", BPS).iter().filter(|s| **s).count();
        assert!((120..=280).contains(&half), "{domain:?}: {half}");
        assert_ne!(pick("G1", BPS), pick("G2", BPS), "{domain:?}");
        // 比率を上げても、前に選ばれた単位は選ばれたまま（同じ hash を閾値で切る）。
        for (low, high) in pick("G1", 1000).iter().zip(pick("G1", 9000)) {
            assert!(!low || high);
        }
    }
    // 固定値（ADR S5-3 の式を変えたら落ちる）。
    assert_eq!(
        (0..16)
            .map(|i| canary_selected(ScriptDomain::LaneRules, "G", &format!("u{i}"), BPS))
            .filter(|s| *s)
            .count(),
        (0..16)
            .filter(|i| {
                use sha2::Digest;
                let mut h = sha2::Sha256::new();
                h.update(format!("celeris.canary/1\0lane_rules\0G\0u{i}").as_bytes());
                let d = h.finalize();
                let mut b = [0u8; 8];
                b.copy_from_slice(&d[..8]);
                u64::from_be_bytes(b) % 10_000 < u64::from(BPS)
            })
            .count()
    );
    let target = |mode, canary_bps| {
        super::super::dispatch_run::script_canary_target_for_test(mode, canary_bps)
    };
    assert!(script_canary_unit_selected(
        &target(ScriptMode::Ts, None),
        ScriptDomain::CosDigest,
        "p"
    ));
    assert!(!script_canary_unit_selected(
        &target(ScriptMode::Compare, None),
        ScriptDomain::CosDigest,
        "p"
    ));
}

/// TS の側の task は、Rust の検証（形・天井）を通った TS の lane で dispatch する。記録は Rust の参照と比べ、
/// `applied_by = ts`・`canary_selected`・`canary_bps` を持つ。説明のつかない不一致 1 件で script-guard が
/// 同じ transaction で `rust` に戻す（S5-7）。
#[tokio::test]
async fn canary_ts_side_uses_validated_ts_result() {
    let base = run_lane(false, false, None).await;
    assert_ne!(base.decision.lane, Tier::Frontier, "fixture must differ");
    let script = FakeScript::new(Behavior::Fixed(frontier()));
    let out = run_lane(true, true, Some(script)).await;
    assert_eq!(out.decision.lane, Tier::Frontier, "TS lane is used");
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Mismatch);
    assert_eq!(r.applied_by, Some(CosDigestAppliedBy::Ts));
    assert!(r.canary_selected);
    assert_eq!(r.canary_bps, Some(BPS));
    assert_eq!(r.fallback, None);
    assert_eq!(r.rust["rule_id"], serde_json::json!(base.decision.rule_id));
    assert_eq!(out.mode.mode, ScriptMode::Rust, "guard rolled back");
    assert!(
        out.guard_reasons
            .iter()
            .any(|r| r.starts_with("guard_mismatch")),
        "{:?}",
        out.guard_reasons
    );
}

/// Rust の側の task は Rust の lane のまま。TS は影で走り、`applied_by = rust`・`canary_selected = false` で記録する。
#[tokio::test]
async fn canary_rust_side_still_compares() {
    let base = run_lane(false, false, None).await;
    let script = FakeScript::new(Behavior::Echo);
    let out = run_lane(true, false, Some(script)).await;
    assert_eq!(out.decision, base.decision);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Match);
    assert_eq!(r.applied_by, Some(CosDigestAppliedBy::Rust));
    assert!(!r.canary_selected);
    assert_eq!(r.canary_bps, Some(BPS));
    assert_eq!(out.mode.mode, ScriptMode::Canary, "a match keeps canary");
}

/// 副作用の前の TS の失敗・検証落ちは Rust の lane を使い、`fallback` に code を残す。
#[tokio::test]
async fn canary_fallback_before_effect_uses_rust() {
    let base = run_lane(false, false, None).await;
    let cases = [
        (
            Behavior::Fail(PolicyScriptError::new("exception", "boom")),
            "ts_error",
        ),
        (
            Behavior::Fixed(
                serde_json::json!({"rule_id": "x", "proposed": "huge", "condition": ""}),
            ),
            "invalid_output",
        ),
        (Behavior::Hang, "ts_error"),
    ];
    for (behavior, code) in cases {
        let out = run_lane(true, true, Some(FakeScript::new(behavior))).await;
        assert_eq!(out.decision.lane, base.decision.lane, "{code}");
        let r = only(&out.records);
        assert_eq!(r.applied_by, Some(CosDigestAppliedBy::Rust), "{code}");
        assert!(r.canary_selected);
        assert_eq!(
            r.fallback.as_ref().map(|f| f.code.as_str()),
            Some(code),
            "{r:?}"
        );
    }
    // 呼べない generation（PolicyScript が無い）も Rust（`unavailable`、計数しない）。
    let out = run_lane(true, true, None).await;
    assert_eq!(out.decision.lane, base.decision.lane);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Unavailable);
    assert_eq!(
        r.fallback.as_ref().map(|f| f.code.as_str()),
        Some("unavailable")
    );
    assert_eq!(out.mode.mode, ScriptMode::Canary);
}

/// 認可の拒否（`denied`、capability の安全違反つき）は許可側に倒さない: TS の lane を使わず Rust で答える。
#[tokio::test]
async fn canary_denied_never_fails_open() {
    let base = run_lane(false, false, None).await;
    let mut denied = PolicyScriptError::new("denied", "capability not granted");
    denied.safety_violation = Some("denied_capability".into());
    let out = run_lane(true, true, Some(FakeScript::new(Behavior::Fail(denied)))).await;
    assert_eq!(out.decision.lane, base.decision.lane);
    let r = only(&out.records);
    assert_eq!(r.applied_by, Some(CosDigestAppliedBy::Rust));
    assert_eq!(
        r.fallback.as_ref().map(|f| f.code.as_str()),
        Some("safety_violation")
    );
    // 拒否だけ（安全違反の code 無し）も `denied` の fallback で Rust。
    let out = run_lane(
        true,
        true,
        Some(FakeScript::new(Behavior::Fail(PolicyScriptError::new(
            "denied", "scope",
        )))),
    )
    .await;
    assert_eq!(out.decision.lane, base.decision.lane);
    let r = only(&out.records);
    assert_eq!(r.applied_by, Some(CosDigestAppliedBy::Rust));
    assert_eq!(r.fallback.as_ref().map(|f| f.code.as_str()), Some("denied"));
}

/// 付記 S5-8: 1 tick の TS の待ちは 200 ms まで。50 ms の判定を 4 回待つと予算を使い切り、5 回目は TS を
/// 呼ばずに `tick_budget`。予算は tick ごとに戻り、tick の遅れの p95 は 200 ms 以下。待った時間は差し替えた値
/// （時計を持たない）で数える。
#[test]
fn tick_budget_200ms_falls_back_after_budget() {
    use crate::dispatcher::dispatch_run::{SCRIPT_TICK_WAIT_BUDGET, ScriptTickWait};
    let mut wait = ScriptTickWait::default();
    let call = Duration::from_millis(50);
    let mut fell_back = Vec::new();
    for _ in 0..5 {
        if wait.remaining().is_zero() {
            wait.budget_hit();
            fell_back.push(true);
        } else {
            wait.charge(call.min(wait.remaining()));
            fell_back.push(false);
        }
    }
    assert_eq!(fell_back, [false, false, false, false, true]);
    assert_eq!(SCRIPT_TICK_WAIT_BUDGET, Duration::from_millis(200));
    wait.end_tick();
    assert_eq!(wait.remaining(), SCRIPT_TICK_WAIT_BUDGET, "budget returns");
    for i in 0..99u64 {
        wait.charge(Duration::from_millis(i % 7));
        wait.end_tick();
    }
    let s = wait.summary();
    assert_eq!(s.ticks, 100);
    assert_eq!(s.tick_budget, 1);
    assert_eq!(s.max_us, 200_000);
    assert!(s.p95_us <= 200_000, "{s:?}");
    assert!(s.p50_us <= s.p95_us);
}

/// 戻しを書けない store: `script_domain_rollback` だけを `fail` の間失敗させる。
struct FlakyRollback {
    inner: Arc<SqliteStore>,
    fail: std::sync::atomic::AtomicBool,
}

impl ScriptDomainStore for FlakyRollback {
    fn script_domain_modes(
        &self,
    ) -> Result<Vec<task_core::script_compare::ScriptDomainModeRow>, task_core::StoreError> {
        self.inner.script_domain_modes()
    }
    fn script_generation_get(
        &self,
        id: &str,
    ) -> Result<Option<task_core::script_compare::ScriptGeneration>, task_core::StoreError> {
        self.inner.script_generation_get(id)
    }
    fn script_compare_append_observed(
        &self,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        ts_side: bool,
        now: OffsetDateTime,
    ) -> Result<(bool, Option<task_core::script_guard::ScriptGuardOutcome>), ScriptDomainError>
    {
        ScriptDomainStore::script_compare_append_observed(
            self.inner.as_ref(),
            task_id,
            record,
            ts_side,
            now,
        )
    }
    fn script_domain_rollback(
        &self,
        domain: ScriptDomain,
        expected_version: u64,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<(), ScriptDomainError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ScriptDomainError::Store(task_core::StoreError::Invalid(
                "injected rollback write failure".into(),
            )));
        }
        ScriptDomainStore::script_domain_rollback(
            self.inner.as_ref(),
            domain,
            expected_version,
            reason,
            now,
        )
    }
    fn script_checkpoint_versions_stored(&self, domain: ScriptDomain) -> Vec<u32> {
        self.inner.script_checkpoint_versions_stored(domain)
    }
}

/// 付記 S5-6: 戻しの書き込みに失敗した領域は、process の中で `rust` と扱う（TS を呼ばない）。tick ごとに書き直し、
/// 書けたら表も `rust` になる。
#[tokio::test]
async fn rollback_write_failure_treated_as_rust() {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let generation = enter_lane_canary(&sqlite);
    let flaky = Arc::new(FlakyRollback {
        inner: sqlite.clone(),
        fail: std::sync::atomic::AtomicBool::new(true),
    });
    let script = FakeScript::new(Behavior::Fixed(frontier()));
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_script_domain_store(flaky.clone());
    d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    let version = sqlite.script_domain_mode("lane_rules").unwrap().version;
    d.script_rollback(
        ScriptDomain::LaneRules,
        version,
        "script_effect_unverified: injected",
    );
    assert_eq!(d.script_domains_forced_rust(), [ScriptDomain::LaneRules]);
    assert_eq!(
        sqlite.script_domain_mode("lane_rules").unwrap().mode,
        ScriptMode::Canary
    );
    let task = task_on_side(ws.path(), &generation, true);
    store.insert(&task).unwrap();
    d.tick().unwrap();
    d.wait_script_compares().await;
    // TS の側の task でも TS を呼ばず、記録もしない（mode `rust` と同じ）。
    let lane = routing_decided(&store, &task).decision.lane;
    assert_ne!(lane, Tier::Frontier);
    assert_eq!(script.calls(), 0);
    assert_eq!(d.script_domains_forced_rust(), [ScriptDomain::LaneRules]);
    // 書けるようになった次の tick で表を `rust` に戻し、process の中の扱いを外す。
    flaky.fail.store(false, Ordering::SeqCst);
    d.tick().unwrap();
    assert!(d.script_domains_forced_rust().is_empty());
    let row = sqlite.script_domain_mode("lane_rules").unwrap();
    assert_eq!(row.mode, ScriptMode::Rust);
    assert!(
        guard_reasons(&sqlite)
            .iter()
            .any(|r| r.starts_with("script_effect_unverified")),
        "{:?}",
        guard_reasons(&sqlite)
    );
}
