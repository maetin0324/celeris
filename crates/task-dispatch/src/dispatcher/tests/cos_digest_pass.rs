//! ADR 2026-11-02-ts-control-layer-boundary 付記 S4-12〜S4-15（unit pass-wire）: digest pass の mode 3 つと fallback。
//!
//! 偽の `PolicyScript` は op の送り口越しに list・read を行う参照の pass（`test_pass::reference_pass`）の出力を
//! `answer` で変えて返す。V8 は起こさない。mode `ts` の pass は dispatcher が実時間 200 ms を上限に待つ。

use super::*;
use crate::script_policy::test_pass::{op, reference_pass};
use crate::script_policy::{CosDigestOp, CosDigestOpReply, CosDigestPassCall};
use std::sync::Mutex as StdMutex;
use task_core::chat::digest::{CosDigestCommand, CosDigestPassOutput};
use task_core::script_compare::CosDigestAppliedBy;

enum PassAnswer {
    Output(serde_json::Value),
    Fail,
    Hang,
}

type AnswerFn = dyn Fn(CosDigestPassOutput) -> PassAnswer + Send + Sync;

struct TsPass {
    answer: Box<AnswerFn>,
    /// TS の返事の前に走る（mode の切り替えなど）。
    before: Box<dyn Fn() + Send + Sync>,
    /// `Some` なら op を 1 つも送らずにこれを走らせ、返事をしない（別の pass の後ろで待たされた call）。
    stall_at_start: Option<Box<dyn Fn() + Send + Sync>>,
    /// list に無い run を読む。
    read_unlisted: bool,
    unlisted_replies: StdMutex<Vec<CosDigestOpReply>>,
    calls: AtomicUsize,
}

fn ts_pass(answer: impl Fn(CosDigestPassOutput) -> PassAnswer + Send + Sync + 'static) -> TsPass {
    TsPass {
        answer: Box::new(answer),
        before: Box::new(|| {}),
        stall_at_start: None,
        read_unlisted: false,
        unlisted_replies: StdMutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    }
}

fn echo(out: CosDigestPassOutput) -> PassAnswer {
    PassAnswer::Output(serde_json::to_value(out).expect("json"))
}

impl PolicyScript for TsPass {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: sha("cd"),
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if let Some(stall) = &self.stall_at_start {
                stall();
                tokio::time::sleep(Duration::from_secs(3600)).await;
                return Err(PolicyScriptError::new("exception", "woke up"));
            }
            let output = reference_pass(&call.sink).await?;
            if self.read_unlisted {
                let reply = op(
                    &call.sink,
                    CosDigestOp::ReadRun,
                    serde_json::json!({ "thread": "x", "run_id": "run-unlisted" }),
                )
                .await;
                self.unlisted_replies.lock().expect("lock").push(reply);
            }
            (self.before)();
            match (self.answer)(output) {
                PassAnswer::Output(output) => Ok(crate::script_policy::PolicyReply {
                    output,
                    isolate_epoch: 2,
                }),
                PassAnswer::Fail => Err(PolicyScriptError::new("exception", "boom")),
                PassAnswer::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(PolicyScriptError::new("exception", "woke up"))
                }
            }
        })
    }
}

/// mode を `ts` にする（同じ generation の `compare` から `canary` を経て、人の actor で。付記 S5-4）。
fn set_ts(f: &Fixture, generation: &str) {
    super::promote_cos_digest_to_ts(&f.store, generation, "pass-wire ts", f.d.now_utc());
}

/// 2 つの run（completed・interrupted）が digest を待つ場面。
fn two_runs(f: &mut Fixture) {
    f.claim_run("run-a-done");
    f.finish_run("run-a-done", ChatRunState::Completed);
    f.claim_run("run-b-intr");
    f.finish_run("run-b-intr", ChatRunState::Interrupted);
    f.observe_run_end();
}

/// mode `mode`（`None` = rust・script 無し）で 2 run の pass を 1 回走らせる。
async fn run_pass(
    mode: Option<ScriptMode>,
    script: Option<Arc<TsPass>>,
    prepare: impl FnOnce(&Fixture, &str),
) -> (Fixture, World) {
    let mut f = fixture(false);
    f.install_requeue_log();
    f.d.set_script_domain_store(f.store.clone());
    if let Some(mode) = mode {
        let generation = set_mode(&f, ScriptMode::Compare, "cd");
        if mode == ScriptMode::Ts {
            set_ts(&f, &generation);
        }
        prepare(&f, &generation);
    }
    f.d.set_policy_script(script.map(|s| s as Arc<dyn PolicyScript>));
    two_runs(&mut f);
    let world = settle(&mut f).await;
    (f, world)
}

async fn baseline() -> World {
    run_pass(None, None, |_, _| {}).await.1
}

fn assert_same_as_rust(world: &World, base: &World) {
    assert_eq!(world.messages, base.messages, "messages equal mode rust");
    assert_eq!(world.items, base.items, "items equal mode rust");
    assert_eq!(world.requeues, base.requeues, "requeues equal mode rust");
}

fn fallback_codes(world: &World) -> Vec<(CosDigestAppliedBy, Option<String>)> {
    world
        .records
        .iter()
        .map(|r| (r.applied_by, r.fallback.as_ref().map(|f| f.code.clone())))
        .collect()
}

#[tokio::test]
async fn cos_digest_pass_rust_reads_each_once_and_applies_rust() {
    let base = baseline().await;
    assert!(base.records.is_empty(), "mode rust records nothing");
    assert!(base.messages.contains("run-a-done") && base.messages.contains("run-b-intr"));
    assert_eq!(base.requeues.len(), 1);
    // The memo answers the second list/read without the store: rename the table in between.
    let mut f = fixture(false);
    two_runs(&mut f);
    let mut reads = crate::dispatcher::cos_chat::digest::CosDigestPassReads::default();
    let store = f.store.clone();
    let list = reads.answer_for_test(Some(&store), CosDigestOp::ListRuns, serde_json::json!({}));
    let CosDigestOpReply::Ok(list) = list else {
        panic!("list: {list:?}")
    };
    let run = &list["runs"][0];
    let req = serde_json::json!({ "thread": run["thread"], "run_id": run["run_id"] });
    assert!(matches!(
        reads.answer_for_test(Some(&store), CosDigestOp::ReadRun, req.clone()),
        CosDigestOpReply::Ok(_)
    ));
    f.fail_reads(true);
    assert_eq!(
        reads.answer_for_test(Some(&store), CosDigestOp::ListRuns, serde_json::json!({})),
        CosDigestOpReply::Ok(list.clone())
    );
    assert!(matches!(
        reads.answer_for_test(Some(&store), CosDigestOp::ReadRun, req),
        CosDigestOpReply::Ok(_)
    ));
    f.fail_reads(false);
    let outcomes: Vec<&str> = reads.ops.iter().map(|o| o.log.outcome.as_str()).collect();
    assert_eq!(outcomes, ["ok", "ok", "memo", "memo"]);
}

#[tokio::test]
async fn cos_digest_pass_compare_answers_ops_from_frozen_snapshot() {
    let base = baseline().await;
    let script = Arc::new(ts_pass(echo));
    let mut f = fixture(false);
    f.install_requeue_log();
    f.d.set_script_domain_store(f.store.clone());
    set_mode(&f, ScriptMode::Compare, "cd");
    f.d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    two_runs(&mut f);
    f.tick();
    // Rust applied; the TS pass has not run yet. A store read now would fail.
    f.fail_reads(true);
    f.d.wait_script_compares().await;
    f.fail_reads(false);
    let world = f.robust_world();
    assert_same_as_rust(&world, &base);
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    assert_eq!(world.records.len(), 2, "{:?}", world.records);
    for r in &world.records {
        assert_eq!(r.outcome, CompareOutcome::Match, "{r:?}");
        assert_eq!(
            (r.mode, r.applied_by),
            (ScriptMode::Compare, CosDigestAppliedBy::Rust)
        );
        assert!(r.ops.iter().all(|o| o.outcome == "memo"), "{:?}", r.ops);
        assert_eq!(r.ops.len(), 2, "list and the run's read");
    }
}

#[tokio::test]
async fn cos_digest_pass_compare_denies_unlisted_read() {
    let mut script = ts_pass(echo);
    script.read_unlisted = true;
    let script = Arc::new(script);
    let (_f, world) = run_pass(Some(ScriptMode::Compare), Some(script.clone()), |_, _| {}).await;
    let replies = script.unlisted_replies.lock().expect("lock").clone();
    assert!(
        matches!(&replies[..], [CosDigestOpReply::Error { code, .. }] if code == "denied"),
        "{replies:?}"
    );
    for r in &world.records {
        assert!(
            r.safety_violations.contains(&"denied_scope".to_owned()),
            "{r:?}"
        );
        // The denied read names another run, so it is not in this run's `ops` (S4-15).
        assert!(r.ops.iter().all(|o| o.outcome == "memo"), "{:?}", r.ops);
    }
}

#[tokio::test]
async fn cos_digest_pass_ts_applies_validated_ts_commands() {
    let base = baseline().await;
    let script = Arc::new(ts_pass(echo));
    let (_f, world) = run_pass(Some(ScriptMode::Ts), Some(script.clone()), |_, _| {}).await;
    assert_same_as_rust(&world, &base);
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    assert_eq!(world.records.len(), 2);
    for r in &world.records {
        assert_eq!(
            (r.mode, r.applied_by),
            (ScriptMode::Ts, CosDigestAppliedBy::Ts)
        );
        assert_eq!(r.fallback, None);
        assert_eq!(r.outcome, CompareOutcome::Match);
    }
    // The applied commands are TS's: a valid TS text that differs from Rust's is what is written.
    let edited = Arc::new(ts_pass(|mut out: CosDigestPassOutput| {
        for run in &mut out.runs {
            if let CosDigestCommand::PostDigest { text, .. } = &mut run.commands[0] {
                *text = format!("TS: {text}");
            }
        }
        echo(out)
    }));
    let (_f, world) = run_pass(Some(ScriptMode::Ts), Some(edited), |_, _| {}).await;
    assert!(world.messages.contains("TS: "), "{}", world.messages);
    assert_eq!(world.items, base.items);
    for r in &world.records {
        assert_eq!(
            (r.applied_by, r.outcome),
            (CosDigestAppliedBy::Ts, CompareOutcome::Mismatch)
        );
    }
}

#[tokio::test]
async fn cos_digest_pass_ts_falls_back_on_error_timeout_invalid() {
    let base = baseline().await;
    let cases: Vec<(&str, TsPass)> = vec![
        ("ts_error", ts_pass(|_| PassAnswer::Fail)),
        ("ts_error", ts_pass(|_| PassAnswer::Hang)),
        (
            "invalid_output",
            ts_pass(|_| PassAnswer::Output(serde_json::json!({ "commands": [] }))),
        ),
        (
            "invalid_output",
            ts_pass(|mut out: CosDigestPassOutput| {
                if let CosDigestCommand::PostDigest {
                    idempotency_key, ..
                } = &mut out.runs[0].commands[0]
                {
                    *idempotency_key = "cos-triage-digest:other".into();
                }
                echo(out)
            }),
        ),
        (
            "run_order",
            ts_pass(|mut out: CosDigestPassOutput| {
                out.runs.reverse();
                echo(out)
            }),
        ),
        (
            "run_missing",
            ts_pass(|mut out: CosDigestPassOutput| {
                out.runs.pop();
                echo(out)
            }),
        ),
    ];
    for (code, script) in cases {
        let (_f, world) = run_pass(Some(ScriptMode::Ts), Some(Arc::new(script)), |_, _| {}).await;
        assert_same_as_rust(&world, &base);
        assert_eq!(world.records.len(), 2, "{code}: {:?}", world.records);
        for (by, fallback) in fallback_codes(&world) {
            assert_eq!(
                (by, fallback.as_deref()),
                (CosDigestAppliedBy::Rust, Some(code))
            );
        }
    }
}

#[tokio::test]
async fn cos_digest_pass_ts_rejects_run_outside_list() {
    let base = baseline().await;
    let script = Arc::new(ts_pass(|mut out: CosDigestPassOutput| {
        let mut extra = out.runs[0].clone();
        extra.run_id = "run-outside".into();
        out.runs.push(extra);
        echo(out)
    }));
    let (_f, world) = run_pass(Some(ScriptMode::Ts), Some(script), |_, _| {}).await;
    assert_same_as_rust(&world, &base);
    assert!(!world.messages.contains("run-outside"));
    for r in &world.records {
        assert_eq!(r.applied_by, CosDigestAppliedBy::Rust);
        assert_eq!(
            r.fallback.as_ref().map(|f| f.code.as_str()),
            Some("run_outside_list")
        );
        assert!(
            r.safety_violations.contains(&"run_outside_list".to_owned()),
            "{r:?}"
        );
    }
}

#[tokio::test]
async fn cos_digest_pass_ts_mode_changed_before_apply_uses_rust() {
    let base = baseline().await;
    let store: Arc<StdMutex<Option<Arc<SqliteStore>>>> = Arc::new(StdMutex::new(None));
    let mut script = ts_pass(|mut out: CosDigestPassOutput| {
        for run in &mut out.runs {
            if let CosDigestCommand::PostDigest { text, .. } = &mut run.commands[0] {
                *text = format!("TS: {text}");
            }
        }
        echo(out)
    });
    let hook = store.clone();
    script.before = Box::new(move || {
        let store = hook.lock().expect("lock").clone().expect("store");
        let row = store
            .script_domain_modes()
            .expect("modes")
            .into_iter()
            .find(|r| r.domain.as_str() == "cos_digest")
            .expect("row");
        store
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: "cos_digest".into(),
                    to_mode: ScriptMode::Rust,
                    to_generation: None,
                    expected_version: row.version,
                    actor: "system:test".into(),
                    reason: "back to rust mid-pass".into(),
                },
                time::OffsetDateTime::now_utc(),
            )
            .expect("rollback");
    });
    let (_f, world) = run_pass(Some(ScriptMode::Ts), Some(Arc::new(script)), |f, _| {
        *store.lock().expect("lock") = Some(f.store.clone());
    })
    .await;
    assert_same_as_rust(&world, &base);
    for (by, fallback) in fallback_codes(&world) {
        assert_eq!(
            (by, fallback.as_deref()),
            (CosDigestAppliedBy::Rust, Some("mode_changed"))
        );
    }
}

/// script_generation_e2e の stage_switch_to_b の競合（2026-11-05 P4-D fix-digest-switch-race）を決定的に再現する。
/// `ts` の A で始まった pass の call が返事をしない間に、人が `compare` の B へ切り替え、その後で triage run が
/// 終わる。call は 200 ms で打ち切られ、Rust はその時点の store を走査するので、切替の後に終わった run も
/// この pass が拾う。記録は pass の始めの generation（A）・`mode_changed`・Rust の apply になる（仕様どおり。
/// ADR 2026-11-03-cos-digest-persist 2）。B の記録は次の tick の pass から出る。
#[tokio::test]
async fn cos_digest_pass_ts_switch_mid_pass_records_late_run_under_old_generation() {
    let store: Arc<StdMutex<Option<(Arc<SqliteStore>, String)>>> = Arc::new(StdMutex::new(None));
    let generation_a: Arc<StdMutex<Option<String>>> = Arc::new(StdMutex::new(None));
    let mut script = ts_pass(echo);
    let hook = store.clone();
    script.stall_at_start = Some(Box::new(move || {
        let (store, b) = hook.lock().expect("lock").clone().expect("store");
        let now = time::OffsetDateTime::now_utc();
        let row = store
            .script_domain_modes()
            .expect("modes")
            .into_iter()
            .find(|r| r.domain.as_str() == "cos_digest")
            .expect("row");
        store
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: "cos_digest".into(),
                    to_mode: ScriptMode::Compare,
                    to_generation: Some(b),
                    expected_version: row.version,
                    actor: "human:test".into(),
                    reason: "switch to B mid-pass".into(),
                },
                now,
            )
            .expect("switch to B");
        // 切替の後に終わる triage run。
        store
            .cos_triage_ingest_batch(
                "digest-switch-race",
                "run-late",
                &[task_core::chat::triage::CosTriageSource {
                    source_kind: "notice".into(),
                    source_key: "run-late".into(),
                    source_revision: "1".into(),
                    source_event_id: None,
                    operation_id: None,
                    summary: "知らせ run-late".into(),
                    detail: String::new(),
                    policy_version: "1".into(),
                    task_id: None,
                }],
                now,
            )
            .expect("ingest");
        let general = store
            .cos_thread_open(
                &task_core::chat::ChatOpenThreadRequest { project_id: None },
                now,
            )
            .expect("general")
            .thread
            .id;
        store
            .cos_triage_claim(&general, "run-late", now)
            .expect("claim")
            .expect("run");
        store
            .chat_run_finish("run-late", ChatRunState::Completed, None, None, now)
            .expect("finish");
    }));
    let seen = generation_a.clone();
    let (f, world) = run_pass(Some(ScriptMode::Ts), Some(Arc::new(script)), |f, a| {
        let b = f
            .store
            .script_generation_record(
                &NewScriptGeneration {
                    bundle_sha256: sha("ef"),
                    host_api: "1.0.0".into(),
                    schema_sha256: "0".repeat(64),
                    manifest_json: r#"{"domains":["cos_digest"]}"#.into(),
                    state: ScriptGenerationState::Loaded,
                },
                f.d.now_utc(),
            )
            .expect("generation B");
        *store.lock().expect("lock") = Some((f.store.clone(), b.id));
        *seen.lock().expect("lock") = Some(a.to_owned());
    })
    .await;
    let a = generation_a.lock().expect("lock").clone().expect("A");
    let late = world
        .records
        .iter()
        .find(|r| r.run_id == "run-late")
        .unwrap_or_else(|| panic!("the pass picked up the late run: {:?}", world.records));
    assert_eq!(late.generation, a, "{late:?}");
    assert_eq!(late.mode, ScriptMode::Ts, "{late:?}");
    assert_eq!(late.applied_by, CosDigestAppliedBy::Rust, "{late:?}");
    assert_eq!(
        late.fallback.as_ref().map(|f| f.code.as_str()),
        Some("mode_changed"),
        "{late:?}"
    );
    assert_eq!(late.outcome, CompareOutcome::TsError, "{late:?}");
    assert_eq!(world.records.len(), 3, "{:?}", world.records);
    for (by, fallback) in fallback_codes(&world) {
        assert_eq!(
            (by, fallback.as_deref()),
            (CosDigestAppliedBy::Rust, Some("mode_changed"))
        );
    }
    // late run の digest は 1 件だけ（Rust の apply）。
    let key = format!("{DIGEST_KEY}run-late");
    assert_eq!(
        world.messages.matches(&key).count(),
        1,
        "{}",
        world.messages
    );
    drop(f);
}

#[tokio::test]
async fn cos_digest_pass_log_records_applied_by_and_ops() {
    let script = Arc::new(ts_pass(echo));
    let (_f, world) = run_pass(Some(ScriptMode::Ts), Some(script), |_, _| {}).await;
    assert_eq!(world.records.len(), 2);
    let pass_id = &world.records[0].pass_id;
    for r in &world.records {
        assert_eq!(&r.pass_id, pass_id, "one pass id per pass");
        assert_eq!(r.call_site, "cos_digest.pass");
        assert_eq!(r.input, None, "no body in the log");
        let ops: Vec<(&str, &str)> = r
            .ops
            .iter()
            .map(|o| (o.op.as_str(), o.outcome.as_str()))
            .collect();
        assert_eq!(
            ops,
            [
                ("cos_digest:list_runs", "ok"),
                ("cos_digest:read_run", "ok")
            ]
        );
        for o in &r.ops {
            assert!(
                o.request_digest.starts_with("sha256:") && o.reply_digest.starts_with("sha256:")
            );
        }
    }
    // The same run's second pass adds nothing (the digest exists; the list is empty).
}
