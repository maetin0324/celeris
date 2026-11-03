//! 付記 S5-3・S5-6: `cos_digest` の canary と副作用の後の照合（unit canary-dispatch）。
//!
//! 親（`cos_digest_script.rs`）の fixture・偽の TS（[`FakeDigest`]）・不変条件（[`assert_invariants`]: message は
//! 冪等 key ごとに 1 件、requeue は item ごとに 1 回、記録は run ごとに 1 件）を使う。選択の単位は pass で、
//! mode `ts` は全 pass が TS の側、`canary` は pass id の hash で決まる（記録の `pass_id` から再現できる）。

use super::*;
use task_core::script_compare::{CosDigestAppliedBy, ScriptDomain, canary_selected};
use task_core::script_guard::{SCRIPT_GUARD_READY_COS, ScriptGuardCall, ScriptGuardObservation};

const BPS: u32 = 5000;

/// actor `system:script-guard` の mode の変更の `reason`。
fn guard_reasons(f: &Fixture) -> Vec<String> {
    crate::dispatcher::tests::script_canary::guard_reasons(&f.store)
}

/// `cos_digest` を `compare` → `canary`（`BPS`）にする。generation の id を返す。
fn enter_canary(f: &Fixture) -> String {
    let generation = set_mode(f, ScriptMode::Compare, "cd");
    let now = time::OffsetDateTime::now_utc();
    for _ in 0..SCRIPT_GUARD_READY_COS {
        f.store
            .script_guard_observe(
                ScriptDomain::CosDigest,
                &generation,
                ScriptGuardObservation {
                    call: ScriptGuardCall::Ok,
                    safety_violation: false,
                    ts_side: false,
                },
                now,
            )
            .expect("observe");
    }
    let version = f
        .store
        .script_domain_mode("cos_digest")
        .expect("row")
        .version;
    f.store
        .script_domain_set_canary(
            &ScriptModeChange {
                domain: "cos_digest".into(),
                to_mode: ScriptMode::Canary,
                to_generation: Some(generation.clone()),
                expected_version: version,
                actor: "human:test".into(),
                reason: "canary-dispatch".into(),
            },
            BPS,
            now,
        )
        .expect("canary");
    generation
}

/// mode `ts`（全 pass が TS の側）にして Echo の TS を注入する。
fn setup_ts() -> (Fixture, Arc<FakeDigest>) {
    let mut f = fixture(false);
    f.install_requeue_log();
    let generation = set_mode(&f, ScriptMode::Compare, "cd");
    crate::dispatcher::tests::promote_cos_digest_to_ts(
        &f.store,
        &generation,
        "canary-dispatch",
        time::OffsetDateTime::now_utc(),
    );
    let script = attach(&mut f, Ts::Compare(Answer::Echo)).expect("script");
    (f, script)
}

fn digest_due(f: &Fixture) -> bool {
    f.d.cos_chat_launch
        .as_ref()
        .expect("launch")
        .triage
        .digest_due
}

/// canary の pass は pass id の hash で TS の側か Rust の側かが決まり、記録の `canary_selected`・`applied_by`・
/// `canary_bps` がその選択と一致する。どちらの側でも Rust の結果（message・item・requeue）は mode `rust` と同じ。
#[tokio::test(start_paused = true)]
async fn canary_digest_pass_selection_matches_record() {
    let mut f = fixture(false);
    f.install_requeue_log();
    let generation = enter_canary(&f);
    let script = attach(&mut f, Ts::Compare(Answer::Echo)).expect("script");
    f.claim_run("run-a-done");
    f.finish_run("run-a-done", ChatRunState::Completed);
    f.claim_run("run-b-intr");
    f.finish_run("run-b-intr", ChatRunState::Interrupted);
    f.observe_run_end();
    let world = settle(&mut f).await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    assert_eq!(world.records.len(), 2, "{:?}", world.records);
    let pass_id = world.records[0].pass_id.clone();
    let selected = canary_selected(ScriptDomain::CosDigest, &generation, &pass_id, BPS);
    for r in &world.records {
        assert_eq!(r.pass_id, pass_id, "one pass");
        assert_eq!(r.mode, ScriptMode::Canary);
        assert_eq!(r.canary_selected, selected, "{r:?}");
        assert_eq!(r.canary_bps, Some(BPS));
        assert_eq!(r.outcome, CompareOutcome::Match);
        let by = if selected {
            CosDigestAppliedBy::Ts
        } else {
            CosDigestAppliedBy::Rust
        };
        assert_eq!(r.applied_by, by, "{r:?}");
    }
    assert_eq!(world.requeues.len(), 1, "{:?}", world.requeues);
    assert_eq!(
        f.store.script_domain_mode("cos_digest").expect("row").mode,
        ScriptMode::Canary,
        "matches keep canary"
    );
}

/// 受け入れ条件 1: TS の側の pass を繰り返し（同じ run の再配送・同じ tick の 2 回目・daemon の再起動）、message は
/// 冪等 key ごとに 1 件、requeue は 1 回、記録は run ごとに 1 件のまま。2 回目以後の pass は照合で何も書かない。
#[tokio::test(start_paused = true)]
async fn canary_digest_no_double_message_or_effect() {
    let (mut f, _script) = setup_ts();
    f.claim_run("run-a-done");
    f.finish_run("run-a-done", ChatRunState::Completed);
    f.claim_run("run-b-intr");
    f.finish_run("run-b-intr", ChatRunState::Interrupted);
    f.observe_run_end();
    let first = settle(&mut f).await;
    assert_eq!(first.records.len(), 2);
    for r in &first.records {
        assert_eq!(r.applied_by, CosDigestAppliedBy::Ts, "{r:?}");
        assert!(r.canary_selected);
    }
    for _ in 0..2 {
        f.observe_run_end();
        f.next_tick();
    }
    f.observe_run_end();
    f.tick();
    let _ = f.restart_digest(Ts::Compare(Answer::Echo));
    f.observe_run_end();
    let world = settle(&mut f).await;
    assert_eq!(world.messages, first.messages);
    assert_eq!(world.items, first.items);
    assert_eq!(world.requeues.len(), 1, "{:?}", world.requeues);
    assert_eq!(world.records.len(), 2, "{:?}", world.records);
    assert!(guard_reasons(&f).is_empty(), "{:?}", guard_reasons(&f));
}

/// TS の pass の答えを返す直前に、同じ冪等 key の message を別の run の名で書く（list の後・apply の前に
/// 別の書き手が入った = 記録と状態が食い違う）。
struct ConflictTs {
    store: Arc<SqliteStore>,
    calls: AtomicUsize,
}

impl PolicyScript for ConflictTs {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: sha("cd"),
            host_api: "1.0.0".into(),
        }
    }

    fn lane_rules(&self, _input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn route_priority(&self, _input: RoutePriorityInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn cos_digest_pass(
        &self,
        call: crate::script_policy::CosDigestPassCall,
        _deadline: Duration,
    ) -> PolicyFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let reply = crate::script_policy::test_pass::reference_reply(&call).await?;
            let general = self
                .store
                .cos_thread_open(
                    &task_core::chat::ChatOpenThreadRequest { project_id: None },
                    time::OffsetDateTime::now_utc(),
                )
                .map_err(|e| PolicyScriptError::new("exception", e.to_string()))?
                .thread
                .id;
            self.store
                .chat_assistant_message_add_once(
                    &general,
                    &format!("{DIGEST_KEY}run-x"),
                    "別の run の digest",
                    &[],
                    Some("run-other"),
                    time::OffsetDateTime::now_utc(),
                )
                .map_err(|e| PolicyScriptError::new("exception", e.to_string()))?;
            Ok(reply)
        })
    }
}

/// 付記 S5-6: TS の command の副作用を冪等 key と記録で照合できない（同じ key の message が別の run のもの）と、
/// その pass の残りを書かずに止め、`digest_due` を下ろさず、actor `system:script-guard` で `rust` に戻す
/// （受信箱の項目はこの記録から script-inbox の経路で作る）。次の pass は Rust の経路で、message を増やさない。
#[tokio::test(start_paused = true)]
async fn canary_effect_unverified_stops_and_rolls_back() {
    let (mut f, _) = setup_ts();
    let script = Arc::new(ConflictTs {
        store: f.store.clone(),
        calls: AtomicUsize::new(0),
    });
    f.d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    f.claim_run("run-x");
    f.finish_run("run-x", ChatRunState::Completed);
    f.observe_run_end();
    f.tick();
    f.d.wait_script_compares().await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    assert!(digest_due(&f), "digest_due stays up");
    assert_eq!(
        f.store.script_domain_mode("cos_digest").expect("row").mode,
        ScriptMode::Rust
    );
    let reasons = guard_reasons(&f);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(
        reasons[0].starts_with("script_effect_unverified: post_digest cos-triage-digest:run-x"),
        "{reasons:?}"
    );
    let world = f.robust_world();
    assert_invariants(&f, &world);
    assert!(world.records.is_empty(), "{:?}", world.records);
    // 次の tick は Rust の経路（TS を呼ばない）。同じ key の message は 1 件のまま。
    f.next_tick();
    f.d.wait_script_compares().await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    for (key, n) in f.digest_counts() {
        assert_eq!(n, 1, "{key}");
    }
    assert_eq!(guard_reasons(&f).len(), 1);
}
