//! ADR 2026-11-03-ts-intended-behavior-change（e2e）: 実際の digest pass の入力（失敗した run の長い理由）で、
//! TS の方針改善（S6 pilot の 200 文字の打ち切り）を mode `ts` で apply し、記録する。
//! - 人が承認した仕様の期待差分と一致すれば outcome `explained`、script-guard は戻さず、昇格の前提の件数に入る。
//! - 承認が無い・取り消した・期待差分と 1 文字でも違う差分は `mismatch` で、script-guard が `rust` へ戻す。
//!
//! 偽の TS は op の送り口越しに参照の pass（`reference_pass`）を走らせ、その出力の見出しの理由を `cap` で切る。V8 は起こさない。

use super::*;
use crate::script_policy::CosDigestPassCall;
use crate::script_policy::test_pass::reference_pass;
use task_core::chat::digest::{CosDigestCommand, CosDigestPassOutput};
use task_core::script_behavior_change::BehaviorChangeSpec;
use task_core::script_compare::CosDigestAppliedBy;

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

#[tokio::test]
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

#[tokio::test]
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

#[tokio::test]
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
#[tokio::test]
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
