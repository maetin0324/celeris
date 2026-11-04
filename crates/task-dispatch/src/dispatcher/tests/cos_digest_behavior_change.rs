//! ADR 2026-11-03-ts-intended-behavior-change（e2e）: 実際の digest pass の入力（失敗した run の長い理由）で、
//! TS の方針改善（S6 pilot の 200 文字の打ち切り）を mode `ts` で apply し、記録する。
//! - 人が承認した仕様の期待差分と一致すれば outcome `explained`、script-guard は戻さず、昇格の前提の件数に入る。
//! - 承認が無い・取り消した・期待差分と 1 文字でも違う差分は `mismatch` で、script-guard が `rust` へ戻す。
//!
//! - mode `canary`: 選ばれた pass は TS を apply し、選ばれない pass は Rust を apply して影で比べる。どちらの側でも説明できない
//!   差分は `rust` へ戻す。TS の側の `explained` は `ts_side_clean_streak` を進め、`canary→ts` の前提に数える。
//!
//! 偽の TS は op の送り口越しに参照の pass（`reference_pass`）を走らせ、その出力の見出しの理由を `cap` で切る。V8 は起こさない。

use super::*;
use crate::script_policy::CosDigestPassCall;
use crate::script_policy::test_pass::reference_pass;
use task_core::chat::digest::{CosDigestCommand, CosDigestPassOutput};
use task_core::script_behavior_change::BehaviorChangeSpec;
use task_core::script_compare::{CosDigestAppliedBy, ScriptDomain, canary_selected};
use task_core::script_guard::{
    SCRIPT_GUARD_READY_COS, ScriptGuardCall, ScriptGuardCounters, ScriptGuardObservation, readiness,
};

const LONG: usize = 500;

/// 見出しの run の失敗の理由を `max` code point で切り、`ellipsis` を足す TS。
struct CappingTs {
    max: usize,
    ellipsis: &'static str,
}

impl PolicyScript for CappingTs {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: sha("be"),
            host_api: "1.1.0".into(),
        }
    }

    fn lane_rules(&self, _input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn route_priority(&self, _input: RoutePriorityInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn cos_digest_pass(&self, call: CosDigestPassCall, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async move {
            let mut out: CosDigestPassOutput = reference_pass(&call.sink).await?;
            let long = "E".repeat(LONG);
            let capped = format!(
                "{}{}",
                "E".repeat(self.max.min(LONG)),
                if self.max < LONG { self.ellipsis } else { "" }
            );
            for run in &mut out.runs {
                for c in &mut run.commands {
                    if let CosDigestCommand::PostDigest { text, .. } = c {
                        *text = text.replacen(
                            &format!("。run は失敗: {long})"),
                            &format!("。run は失敗: {capped})"),
                            1,
                        );
                    }
                }
            }
            Ok(crate::script_policy::PolicyReply {
                output: serde_json::to_value(out).expect("json"),
                isolate_epoch: 2,
            })
        })
    }
}

fn pilot_spec() -> BehaviorChangeSpec {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-ts/behavior-changes/cos_digest/cos-digest-cap-run-reason.v1.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("spec")).expect("spec json")
}

/// 失敗した run（理由 500 文字）が digest を待つ場面を mode `ts` で 1 pass 走らせる。
async fn ts_pass_with(script: CappingTs, approve: bool, revoke: bool) -> (Fixture, World, String) {
    pass_with(ScriptMode::Ts, script, approve, revoke).await
}

async fn pass_with(
    mode: ScriptMode,
    script: CappingTs,
    approve: bool,
    revoke: bool,
) -> (Fixture, World, String) {
    let mut f = fixture(false);
    f.install_requeue_log();
    f.d.set_script_domain_store(f.store.clone());
    let generation = set_mode(&f, ScriptMode::Compare, "be");
    if mode == ScriptMode::Ts {
        super::promote_cos_digest_to_ts(
            &f.store,
            &generation,
            "behavior-change e2e",
            f.d.now_utc(),
        );
    }
    let spec = pilot_spec();
    if approve {
        f.store
            .script_behavior_change_approve(
                &spec,
                &spec.sha256(),
                "human:test",
                "S6 pilot の改善",
                f.d.now_utc(),
            )
            .expect("approve");
    }
    if revoke {
        f.store
            .script_behavior_change_revoke(&spec.sha256(), "cos", "取り消し", f.d.now_utc())
            .expect("revoke");
    }
    f.d.set_policy_script(Some(Arc::new(script) as Arc<dyn PolicyScript>));
    f.claim_run("run-f-long");
    f.store
        .chat_run_finish(
            "run-f-long",
            ChatRunState::Failed,
            None,
            Some(&"E".repeat(LONG)),
            f.d.now_utc(),
        )
        .expect("finish");
    f.observe_run_end();
    let world = settle(&mut f).await;
    (f, world, generation)
}

fn cos_digest_mode(f: &Fixture) -> ScriptMode {
    f.store.script_domain_mode("cos_digest").expect("row").mode
}

#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_explained_delta_is_applied_and_counted_as_ready() {
    let pilot = CappingTs {
        max: 200,
        ellipsis: "…",
    };
    let (f, world, generation) = ts_pass_with(pilot, true, false).await;
    assert_eq!(world.records.len(), 1, "{:?}", world.records);
    let r = &world.records[0];
    assert_eq!(
        (r.mode, r.applied_by, r.outcome),
        (
            ScriptMode::Ts,
            CosDigestAppliedBy::Ts,
            CompareOutcome::Explained
        ),
        "{r:?}"
    );
    let spec = pilot_spec();
    let by = r.explained_by.as_ref().expect("explained_by");
    assert_eq!(
        (by.id.as_str(), by.version, by.spec_sha256.as_str()),
        (spec.id.as_str(), 1, spec.sha256().as_str())
    );
    // 差分は隠さない: 要約と TS の見出しの先頭が残り、正規化は空のまま。
    assert!(r.normalized.is_empty());
    assert!(r.ts_commands_head.is_some());
    assert_ne!(Some(&r.rust), r.ts.as_ref());
    // apply したのは TS の改善した文面。
    assert!(
        world
            .messages
            .contains(&format!("。run は失敗: {}…)", "E".repeat(200))),
        "{}",
        world.messages
    );
    assert!(!world.messages.contains(&"E".repeat(201)));
    // script-guard は戻さない。説明済みの 1 件は窓に `e` として入り、昇格の前提の件数を進める。
    assert_eq!(cos_digest_mode(&f), ScriptMode::Ts);
    let (_, counters) = f
        .store
        .script_guard_counters(
            task_core::script_compare::ScriptDomain::CosDigest,
            &generation,
        )
        .expect("read")
        .expect("row");
    assert_eq!(counters.explained_in_window(), 1);
    assert_eq!(counters.mismatches_in_window(), 0);
    assert!(counters.ts_side_clean_streak >= 1, "{counters:?}");
}

#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_unapproved_or_revoked_delta_rolls_back() {
    for (approve, revoke) in [(false, false), (true, true)] {
        let pilot = CappingTs {
            max: 200,
            ellipsis: "…",
        };
        let (f, world, _) = ts_pass_with(pilot, approve, revoke).await;
        let r = &world.records[0];
        assert_eq!(
            r.outcome,
            CompareOutcome::Mismatch,
            "{approve} {revoke}: {r:?}"
        );
        assert_eq!(r.explained_by, None);
        assert_eq!(cos_digest_mode(&f), ScriptMode::Rust, "{approve} {revoke}");
    }
}

#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_unexplained_delta_rolls_back() {
    // 承認した仕様（200 文字と「…」）と違う切り方は説明できない差分。
    for (max, ellipsis) in [(199, "…"), (200, ""), (200, "...")] {
        let (f, world, _) = ts_pass_with(CappingTs { max, ellipsis }, true, false).await;
        let r = &world.records[0];
        assert_eq!(
            r.outcome,
            CompareOutcome::Mismatch,
            "{max} {ellipsis:?}: {r:?}"
        );
        assert_eq!(r.explained_by, None);
        assert_eq!(cos_digest_mode(&f), ScriptMode::Rust, "{max} {ellipsis:?}");
    }
}

/// mode `compare` の影の pass: 説明済みの差分は `clean_streak` を進め（段階昇格の候補）、説明できない差分は 0 に戻す。
/// compare では apply は Rust のまま。
#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_compare_counts_explained_toward_promotion() {
    let streak = |f: &Fixture, g: &str| {
        f.store
            .script_guard_counters(task_core::script_compare::ScriptDomain::CosDigest, g)
            .expect("read")
            .expect("row")
            .1
    };
    let pilot = CappingTs {
        max: 200,
        ellipsis: "…",
    };
    let (f, world, generation) = pass_with(ScriptMode::Compare, pilot, true, false).await;
    let r = &world.records[0];
    assert_eq!(
        (r.applied_by, r.outcome),
        (CosDigestAppliedBy::Rust, CompareOutcome::Explained),
        "{r:?}"
    );
    assert!(
        world.messages.contains(&"E".repeat(LONG)),
        "compare applies Rust"
    );
    let c = streak(&f, &generation);
    assert_eq!((c.clean_streak, c.explained_in_window()), (1, 1));
    assert_eq!(cos_digest_mode(&f), ScriptMode::Compare);

    let off = CappingTs {
        max: 199,
        ellipsis: "…",
    };
    let (f, world, generation) = pass_with(ScriptMode::Compare, off, true, false).await;
    assert_eq!(world.records[0].outcome, CompareOutcome::Mismatch);
    let c = streak(&f, &generation);
    assert_eq!((c.clean_streak, c.mismatches_in_window()), (0, 1));
    // compare は mode を変えない（S5-7）。前提が 0 に戻るだけ。
    assert_eq!(cos_digest_mode(&f), ScriptMode::Compare);
}

const CANARY_BPS: u32 = 5000;
const PILOT: CappingTs = CappingTs {
    max: 200,
    ellipsis: "…",
};
const OFF_BY_ONE: CappingTs = CappingTs {
    max: 199,
    ellipsis: "…",
};

fn observe_clean(f: &Fixture, generation: &str, ts_side: bool, n: u64) {
    for _ in 0..n {
        f.store
            .script_guard_observe(
                ScriptDomain::CosDigest,
                generation,
                ScriptGuardObservation {
                    call: ScriptGuardCall::Ok,
                    safety_violation: false,
                    ts_side,
                },
                f.d.now_utc(),
            )
            .expect("observe");
    }
}

fn counters_of(f: &Fixture, generation: &str) -> ScriptGuardCounters {
    f.store
        .script_guard_counters(ScriptDomain::CosDigest, generation)
        .expect("read")
        .expect("row")
        .1
}

fn mode_change(f: &Fixture, to_mode: ScriptMode, generation: &str) -> ScriptModeChange {
    ScriptModeChange {
        domain: "cos_digest".into(),
        to_mode,
        to_generation: Some(generation.to_owned()),
        expected_version: f
            .store
            .script_domain_mode("cos_digest")
            .expect("row")
            .version,
        actor: "human:test".into(),
        reason: "behavior-change e2e".into(),
    }
}

/// 長い理由で失敗した run の digest を mode `canary` の 1 pass で走らせる。`selected` は pass が TS の側か。
/// 走らせる前の `ts_side_seed` 件の TS の側の clean を積む（`canary→ts` の前提の手前まで進めるため）。
async fn canary_pass(
    script: CappingTs,
    approve: bool,
    selected: bool,
    ts_side_seed: u64,
) -> (Fixture, World, String) {
    let mut f = fixture(false);
    f.install_requeue_log();
    f.d.set_script_domain_store(f.store.clone());
    let generation = set_mode(&f, ScriptMode::Compare, "be");
    observe_clean(&f, &generation, false, SCRIPT_GUARD_READY_COS);
    f.store
        .script_domain_set_canary(
            &mode_change(&f, ScriptMode::Canary, &generation),
            CANARY_BPS,
            f.d.now_utc(),
        )
        .expect("canary");
    observe_clean(&f, &generation, true, ts_side_seed);
    if approve {
        let spec = pilot_spec();
        f.store
            .script_behavior_change_approve(
                &spec,
                &spec.sha256(),
                "human:test",
                "S6 pilot の改善",
                f.d.now_utc(),
            )
            .expect("approve");
    }
    f.d.set_policy_script(Some(Arc::new(script) as Arc<dyn PolicyScript>));
    // 選択は pass id の hash で決まる（乱数を使わない）。欲しい側に落ちる id を探して渡す。
    f.d.script_compare.next_digest_pass_id = (0u32..).map(|i| format!("pass-{i}")).find(|id| {
        canary_selected(ScriptDomain::CosDigest, &generation, id, CANARY_BPS) == selected
    });
    f.claim_run("run-f-long");
    f.store
        .chat_run_finish(
            "run-f-long",
            ChatRunState::Failed,
            None,
            Some(&"E".repeat(LONG)),
            f.d.now_utc(),
        )
        .expect("finish");
    f.observe_run_end();
    let world = settle(&mut f).await;
    assert_eq!(world.records.len(), 1, "{:?}", world.records);
    let r = &world.records[0];
    assert_eq!(
        (r.mode, r.canary_selected, r.canary_bps),
        (ScriptMode::Canary, selected, Some(CANARY_BPS)),
        "{r:?}"
    );
    (f, world, generation)
}

/// canary の TS の側: TS の改善を apply → 記録 `explained` → `ts_side_clean_streak` が進み、mode は `canary` のまま。
/// この 1 件で 200 件に届くと `readiness(CosDigest, Canary, …)` が ready になり、人の `canary→ts` が通る（段階昇格の候補）。
#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_canary_ts_side_explained_becomes_promotion_candidate() {
    let (f, world, generation) = canary_pass(PILOT, true, true, SCRIPT_GUARD_READY_COS - 1).await;
    let r = &world.records[0];
    assert_eq!(
        (r.applied_by, r.outcome),
        (CosDigestAppliedBy::Ts, CompareOutcome::Explained),
        "{r:?}"
    );
    assert_eq!(
        r.explained_by.as_ref().map(|b| b.spec_sha256.clone()),
        Some(pilot_spec().sha256())
    );
    assert!(r.normalized.is_empty(), "{r:?}");
    assert!(
        world
            .messages
            .contains(&format!("。run は失敗: {}…)", "E".repeat(200))),
        "{}",
        world.messages
    );
    assert!(!world.messages.contains(&"E".repeat(201)));
    assert_eq!(cos_digest_mode(&f), ScriptMode::Canary);
    let c = counters_of(&f, &generation);
    assert_eq!(c.explained_in_window(), 1);
    assert_eq!(c.mismatches_in_window(), 0);
    assert_eq!(c.ts_side_clean_streak, SCRIPT_GUARD_READY_COS);
    let ready = readiness(ScriptDomain::CosDigest, ScriptMode::Canary, &c);
    assert!(ready.ready, "{ready:?}");
    assert_eq!(ready.clean_streak, SCRIPT_GUARD_READY_COS);
    f.store
        .script_domain_set_mode(&mode_change(&f, ScriptMode::Ts, &generation), f.d.now_utc())
        .expect("canary→ts after the explained pass");
    assert_eq!(cos_digest_mode(&f), ScriptMode::Ts);
}

/// `explained` の 1 件が無ければ前提に届かない（上の試験の対照）。人の `canary→ts` は拒まれ、mode は `canary` のまま。
#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_canary_short_of_the_count_is_not_a_candidate() {
    let (f, world, generation) = canary_pass(PILOT, true, true, SCRIPT_GUARD_READY_COS - 2).await;
    assert_eq!(world.records[0].outcome, CompareOutcome::Explained);
    let c = counters_of(&f, &generation);
    assert_eq!(c.ts_side_clean_streak, SCRIPT_GUARD_READY_COS - 1);
    assert!(!readiness(ScriptDomain::CosDigest, ScriptMode::Canary, &c).ready);
    assert!(
        f.store
            .script_domain_set_mode(&mode_change(&f, ScriptMode::Ts, &generation), f.d.now_utc())
            .is_err()
    );
    assert_eq!(cos_digest_mode(&f), ScriptMode::Canary);
}

/// canary の Rust の側（選ばれない pass）: Rust を apply し、影の比較で `explained`。`clean_streak` は進むが、
/// `canary→ts` の前提（`ts_side_clean_streak`）は進まない。mode は `canary` のまま。
#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_canary_rust_side_explained_keeps_ts_side_streak() {
    let (f, world, generation) = canary_pass(PILOT, true, false, 3).await;
    let r = &world.records[0];
    assert_eq!(
        (r.applied_by, r.outcome),
        (CosDigestAppliedBy::Rust, CompareOutcome::Explained),
        "{r:?}"
    );
    assert!(
        world.messages.contains(&"E".repeat(LONG)),
        "the Rust side applies Rust: {}",
        world.messages
    );
    assert_eq!(cos_digest_mode(&f), ScriptMode::Canary);
    let c = counters_of(&f, &generation);
    assert_eq!(c.explained_in_window(), 1);
    assert_eq!(c.ts_side_clean_streak, 3);
    // `clean_streak` は canary に入るときに 0 から数え直す。積んだ 3 件と、この pass の 1 件。
    assert_eq!(c.clean_streak, 3 + 1);
    assert_eq!(
        readiness(ScriptDomain::CosDigest, ScriptMode::Canary, &c).clean_streak,
        3
    );
}

/// canary のどちらの側でも、説明できない差分（承認の無い改善・承認した仕様と違う切り方）は `mismatch` で、
/// 前提の連続を 0 に戻し、script-guard が `rust` へ戻す。`canary→ts` の直前でも同じ。
#[tokio::test(start_paused = true)]
async fn cos_digest_behavior_change_canary_unexplained_delta_rolls_back_on_both_sides() {
    let cases = [(PILOT, false), (OFF_BY_ONE, true)];
    for selected in [true, false] {
        for (script, approve) in &cases {
            let script = CappingTs {
                max: script.max,
                ellipsis: script.ellipsis,
            };
            let label = format!("selected={selected} max={} approve={approve}", script.max);
            let (f, world, generation) =
                canary_pass(script, *approve, selected, SCRIPT_GUARD_READY_COS - 1).await;
            let r = &world.records[0];
            assert_eq!(r.outcome, CompareOutcome::Mismatch, "{label}: {r:?}");
            assert_eq!(r.explained_by, None, "{label}");
            let by = if selected {
                CosDigestAppliedBy::Ts
            } else {
                CosDigestAppliedBy::Rust
            };
            assert_eq!(r.applied_by, by, "{label}: {r:?}");
            assert_eq!(cos_digest_mode(&f), ScriptMode::Rust, "{label}");
            assert_eq!(
                crate::dispatcher::tests::script_canary::guard_reasons(&f.store).len(),
                1,
                "{label}"
            );
            let c = counters_of(&f, &generation);
            assert_eq!(c.mismatches_in_window(), 1, "{label}");
            assert_eq!((c.clean_streak, c.ts_side_clean_streak), (0, 0), "{label}");
            assert!(
                !readiness(ScriptDomain::CosDigest, ScriptMode::Canary, &c).ready,
                "{label}"
            );
        }
    }
}
