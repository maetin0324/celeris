//! ADR 2026-11-02-ts-control-layer-boundary 付記 S4-14・S4-18・S4-19（unit robust-ts）: mode `ts` の
//! async pass の頑健性。
//!
//! 親の 11 件（順序・再配送・retry 3 件・cancel 2 件・再起動 2 件・policy_version と session freshness・
//! mode と generation の切替）を mode `ts` で走らせ、加えて op を await している途中の出来事を試す。
//! - 偽の TS（[`AsyncTs`]）は op の送り口越しに `cos_digest:list_runs`・`cos_digest:read_run` を await し、
//!   参照の判定（`cos_digest_decide`）で command を返す。dispatcher はその command を検証して apply する。
//! - 読み出し口の試験専用の遅延フック（[`Hook`]）が、op を送る前と応答を受けた後（TS から見ればまだ op を
//!   await している間）に走る。フックは DB を書き換え（item の判断・mode と generation の切替・表の改名）、
//!   遅らせ（deadline の cancel）、generation の作り直し（`generation_gone`）、dispatcher の停止（panic で
//!   tick を巻き戻す）を起こす。
//! - mode `ts` の pass は dispatcher が専用の current-thread runtime で待つ。偽の TS はその runtime の時計を
//!   最初に `tokio::time::pause` で止めるので、deadline（200 ms）は実時間を待たずに自動で進む。CPU を焼く
//!   負荷・実時間の sleep は使わない。
//!
//! どの場面でも [`assert_invariants`] で次を確かめる: message は冪等 key ごとに 1 件、requeue は item ごとに
//! 1 回まで、記録は pass の run ごとに 1 件まで。Rust の基準（mode `rust`・PolicyScript 無し）と比べられる
//! 場面では、message・item・requeue が基準と一致することも確かめる。

use super::*;
use crate::script_policy::test_pass::op as send_op;
use crate::script_policy::{CosDigestOp, CosDigestOpReply, CosDigestOpSink, PolicyReply};
use std::sync::atomic::AtomicBool;
use task_core::chat::digest::{
    CosDigestInput, CosDigestListRunsReply, CosDigestPassOutput, CosDigestPassRun,
    CosDigestReadRunRequest, cos_digest_decide,
};
use task_core::script_compare::CosDigestAppliedBy;

/// フックが走る位置（TS の op 1 回の前後）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// op を送る前。
    Before,
    /// 応答を受けた後、TS の続きに渡す前（TS はまだ op を await している）。
    After,
}

/// フックの返事。
enum Step {
    Go,
    /// TS に応答を渡すのを遅らせる（止めた時計で）。
    Delay(Duration),
    /// isolate の generation が作り直された（S4-18: `generation_gone`）。
    GenerationGone,
    /// dispatcher が止まる（tick ごと panic で巻き戻す）。
    Stop,
}

type Hook = dyn Fn(Phase, CosDigestOp, &serde_json::Value) -> Step + Send + Sync;

const STOP: &str = "test: dispatcher stopped while TS awaited an op";

/// op を await し、command を返す偽の TS（`cosDigestPass` と同じ手順）。
struct AsyncTs {
    hook: Box<Hook>,
    calls: AtomicUsize,
}

fn async_ts(
    hook: impl Fn(Phase, CosDigestOp, &serde_json::Value) -> Step + Send + Sync + 'static,
) -> Arc<AsyncTs> {
    Arc::new(AsyncTs {
        hook: Box::new(hook),
        calls: AtomicUsize::new(0),
    })
}

fn go() -> Arc<AsyncTs> {
    async_ts(|_, _, _| Step::Go)
}

fn script_error(e: impl std::fmt::Display) -> PolicyScriptError {
    PolicyScriptError::new("exception", e.to_string())
}

impl AsyncTs {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    async fn step(
        &self,
        phase: Phase,
        op: CosDigestOp,
        request: &serde_json::Value,
    ) -> Result<(), PolicyScriptError> {
        match (self.hook)(phase, op, request) {
            Step::Go => Ok(()),
            Step::Delay(d) => {
                tokio::time::sleep(d).await;
                Ok(())
            }
            Step::GenerationGone => Err(PolicyScriptError::new(
                "generation_gone",
                "the isolate was rebuilt while the op was awaited",
            )),
            Step::Stop => panic!("{STOP}"),
        }
    }

    /// op 1 回（前後にフック）。
    async fn op(
        &self,
        sink: &CosDigestOpSink,
        op: CosDigestOp,
        request: serde_json::Value,
    ) -> Result<CosDigestOpReply, PolicyScriptError> {
        self.step(Phase::Before, op, &request).await?;
        let reply = send_op(sink, op, request.clone()).await;
        self.step(Phase::After, op, &request).await?;
        Ok(reply)
    }

    /// list を読み、list の run を順に read し（失敗した run は飛ばす）、run ごとに決める。
    async fn pass(&self, sink: &CosDigestOpSink) -> Result<CosDigestPassOutput, PolicyScriptError> {
        let list = match self
            .op(sink, CosDigestOp::ListRuns, serde_json::json!({}))
            .await?
        {
            CosDigestOpReply::Ok(v) => {
                serde_json::from_value::<CosDigestListRunsReply>(v).map_err(script_error)?
            }
            CosDigestOpReply::Error { code, message } => {
                return Err(PolicyScriptError::new(
                    "exception",
                    format!("{code}: {message}"),
                ));
            }
        };
        let mut runs = Vec::new();
        for run in list.runs {
            let request = serde_json::to_value(CosDigestReadRunRequest {
                thread: run.thread.clone(),
                run_id: run.run_id.clone(),
            })
            .map_err(script_error)?;
            if let CosDigestOpReply::Ok(v) = self.op(sink, CosDigestOp::ReadRun, request).await? {
                let input: CosDigestInput = serde_json::from_value(v).map_err(script_error)?;
                runs.push(CosDigestPassRun {
                    run_id: input.run_id.clone(),
                    commands: cos_digest_decide(&input),
                });
            }
        }
        Ok(CosDigestPassOutput { runs })
    }
}

impl PolicyScript for AsyncTs {
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

    fn cos_digest_pass(
        &self,
        call: crate::script_policy::CosDigestPassCall,
        _deadline: Duration,
    ) -> PolicyFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            // mode `ts` の pass は dispatcher が pass ごとに作る current-thread runtime で走る。その時計を止める
            // （deadline は自動で進む。実時間を待たない）。
            tokio::time::pause();
            let output = self.pass(&call.sink).await?;
            Ok(PolicyReply {
                output: serde_json::to_value(output).map_err(script_error)?,
                isolate_epoch: 3,
            })
        })
    }
}

fn sql_at(db: &std::path::Path, sql: &str) {
    rusqlite::Connection::open(db)
        .expect("db")
        .execute_batch(sql)
        .expect("sql");
}

fn cos_digest_row(f: &Fixture) -> task_core::script_compare::ScriptDomainModeRow {
    cos_digest_row_of(&f.store)
}

fn cos_digest_row_of(store: &SqliteStore) -> task_core::script_compare::ScriptDomainModeRow {
    store
        .script_domain_modes()
        .expect("modes")
        .into_iter()
        .find(|r| r.domain.as_str() == "cos_digest")
        .expect("cos_digest row")
}

/// `compare`（同じ generation）→ `canary` → `ts`。人の actor で（付記 S5-4）。
fn set_ts_of(store: &SqliteStore, generation: &str) {
    super::promote_cos_digest_to_ts(
        store,
        generation,
        "robust-ts",
        time::OffsetDateTime::now_utc(),
    );
}

/// `ts` → `rust`（戻しは誰でも行える。ここでは daemon の actor）。
fn back_to_rust_of(store: &SqliteStore) {
    let row = cos_digest_row_of(store);
    store
        .script_domain_set_mode(
            &ScriptModeChange {
                domain: "cos_digest".into(),
                to_mode: ScriptMode::Rust,
                to_generation: None,
                expected_version: row.version,
                actor: "system:test".into(),
                reason: "robust-ts back to rust".into(),
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("back to rust");
}

/// 新しい generation の `compare` を経て `ts` にする。generation の id を返す。
fn enter_ts(f: &Fixture) -> String {
    let generation = set_mode(f, ScriptMode::Compare, "cd");
    set_ts_of(&f.store, &generation);
    generation
}

/// 場面の用意。`ts` なら mode `ts` にし、`script` を注入する。`false` は mode `rust`・script 無し（基準）。
fn setup_ts(ts: bool, script: Option<Arc<AsyncTs>>) -> Fixture {
    let mut f = fixture(false);
    f.install_requeue_log();
    f.d.set_script_domain_store(f.store.clone());
    if ts {
        enter_ts(&f);
        f.d.set_policy_script(script.map(|s| s as Arc<dyn PolicyScript>));
    } else {
        f.d.set_policy_script(None);
    }
    f
}

/// 同じ DB で dispatcher を作り直す（daemon の再起動）。mode の行は DB にあるので `ts` のまま。
fn restart_ts(f: &mut Fixture, script: Option<Arc<AsyncTs>>) {
    f.d = triage_dispatcher(f.dir.path(), &f.db_path, f.store.clone(), false, "true");
    f.d.set_script_domain_store(f.store.clone());
    f.d.set_policy_script(script.map(|s| s as Arc<dyn PolicyScript>));
}

/// mode `rust` の基準と mode `ts` を同じ場面で走らせ、Rust の結果（message・item・requeue）が一致することを
/// 確かめる。`scenario(true)` が mode `ts`。
async fn ts_same_as_rust<F, Fut>(scenario: F) -> World
where
    F: Fn(bool) -> Fut,
    Fut: std::future::Future<Output = World>,
{
    let base = scenario(false).await;
    let ts = scenario(true).await;
    assert_eq!(ts.messages, base.messages, "messages equal mode rust");
    assert_eq!(ts.items, base.items, "item states equal mode rust");
    assert_eq!(ts.requeues, base.requeues, "requeues equal mode rust");
    assert!(base.records.is_empty(), "mode rust records nothing");
    ts
}

/// どの記録も mode `ts` で TS の command を apply した（fallback 無し・Rust の参照と一致）。
fn assert_ts_applied(world: &World, runs: usize) {
    assert_eq!(world.records.len(), runs, "{:?}", world.records);
    for r in &world.records {
        assert_eq!(r.mode, ScriptMode::Ts, "{r:?}");
        assert_eq!(r.applied_by, CosDigestAppliedBy::Ts, "{r:?}");
        assert_eq!(r.fallback, None, "{r:?}");
        assert_eq!(r.outcome, CompareOutcome::Match, "{r:?}");
    }
}

/// どの記録も Rust の command を `code` の fallback で apply した。
fn assert_rust_fallback(world: &World, runs: usize, code: &str) {
    assert_eq!(world.records.len(), runs, "{:?}", world.records);
    for r in &world.records {
        assert_eq!(r.mode, ScriptMode::Ts, "{r:?}");
        assert_eq!(r.applied_by, CosDigestAppliedBy::Rust, "{r:?}");
        assert_eq!(
            r.fallback.as_ref().map(|f| f.code.as_str()),
            Some(code),
            "{r:?}"
        );
    }
}

fn read_of(request: &serde_json::Value, run: &str) -> bool {
    request.get("run_id").and_then(|v| v.as_str()) == Some(run)
}

// ---------------------------------------------------------------- (1) 順序

/// 親の (1) を mode `ts` で: item の判断 → run の終了 → digest と、run の終了 → digest → 遅れた判断。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_order_item_update_before_and_after_run_end() {
    async fn scenario(ts: bool) -> World {
        let mut f = setup_ts(ts, Some(go()));
        let early = f.claim_run("run-1-item-first");
        f.resolve_item(&early);
        f.finish_run("run-1-item-first", ChatRunState::Completed);
        let late_done = f.claim_run("run-2-run-first");
        f.finish_run("run-2-run-first", ChatRunState::Completed);
        let late_stop = f.claim_run("run-3-run-first-stop");
        f.finish_run("run-3-run-first-stop", ChatRunState::Stopped);
        f.observe_run_end();
        let first = settle(&mut f).await;
        for item in [&late_done, &late_stop] {
            assert!(
                !f.store
                    .cos_triage_resolve(item, "observed", None, Some("遅れた判断"), f.d.now_utc())
                    .expect("resolve"),
                "the item already went to the person"
            );
        }
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(world.messages, first.messages, "no second digest");
        assert_eq!(world.records.len(), first.records.len());
        assert!(first.messages.contains("確認済み"), "{}", first.messages);
        assert_eq!(f.digest_counts().len(), 3);
        assert_eq!(
            world.requeues,
            vec![(
                "run-3-run-first-stop".to_owned(),
                "run-3-run-first-stop".to_owned()
            )]
        );
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 3);
}

/// await 中に item が更新される順序: TS が `read_run` を送る前の判断は digest に載り、応答を受けた後（TS が
/// まだ await している間）の判断は digest に載らない（pass は読んだ snapshot で決める）。後者でも requeue の
/// `WHERE state='running'` が判断済みの item を `pending` に戻さない。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_item_updated_during_op_await() {
    for phase in [Phase::Before, Phase::After] {
        let mut f = setup_ts(true, None);
        let item = f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        let (store, now, once) = (
            f.store.clone(),
            f.d.now_utc(),
            Arc::new(AtomicBool::new(false)),
        );
        let item_id = item.clone();
        let script = async_ts(move |p, op, request| {
            if p == phase
                && op == CosDigestOp::ReadRun
                && read_of(request, "run-stop")
                && !once.swap(true, Ordering::SeqCst)
            {
                assert!(
                    store
                        .cos_triage_resolve(&item_id, "observed", None, Some("await 中の判断"), now)
                        .expect("resolve"),
                    "the item is still running while TS awaits"
                );
            }
            Step::Go
        });
        f.d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1, "{phase:?}");
        assert_eq!(
            world.messages.contains("await 中の判断"),
            phase == Phase::Before,
            "{phase:?}: {}",
            world.messages
        );
        assert!(world.requeues.is_empty(), "{phase:?}: {:?}", world.requeues);
        assert!(
            world.items.contains(r#""run-stop","observed","run-stop""#),
            "{phase:?}: {}",
            world.items
        );
        assert_ts_applied(&world, 1);
        assert_eq!(world.records[0].applied.requeued, 0, "{phase:?}");
        // 再配送しても何も増えない。
        f.observe_run_end();
        f.next_tick();
        let again = settle(&mut f).await;
        assert_eq!(again.messages, world.messages);
        assert_eq!(again.records.len(), 1);
        assert_eq!(script.calls(), 2, "{phase:?}: the empty pass also asks TS");
    }
}

// ---------------------------------------------------------------- (2) 再配送

/// 親の (2) を mode `ts` で: 同じ run を何度 digest_due にしても message・requeue・記録は 1 回。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_redelivery_same_run_twice() {
    async fn scenario(ts: bool) -> World {
        let script = go();
        let mut f = setup_ts(ts, Some(script.clone()));
        f.claim_run("run-a-done");
        f.finish_run("run-a-done", ChatRunState::Completed);
        f.claim_run("run-b-intr");
        f.finish_run("run-b-intr", ChatRunState::Interrupted);
        f.observe_run_end();
        let first = settle(&mut f).await;
        for _ in 0..2 {
            f.observe_run_end();
            f.next_tick();
            f.d.wait_script_compares().await;
        }
        f.observe_run_end();
        f.tick();
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(world.messages, first.messages);
        assert_eq!(world.items, first.items);
        assert_eq!(world.requeues.len(), 1, "{:?}", world.requeues);
        assert_eq!(
            world.records.len(),
            first.records.len(),
            "no record for an empty pass"
        );
        if ts {
            assert!(script.calls() >= 2, "every due pass asks TS first");
        }
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 2);
}

// ---------------------------------------------------------------- (3) retry

/// 親の (3) message の書き込み失敗を mode `ts` で: TS の command の apply が落ちた run は記録されず、次の
/// tick で TS の command を apply し直す（message は 1 件）。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_retry_message_write_failure() {
    async fn scenario(ts: bool) -> World {
        let mut f = setup_ts(ts, Some(go()));
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.fail_message_write(true);
        f.observe_run_end();
        let failed = settle(&mut f).await;
        assert!(f.digest_counts().is_empty());
        assert!(failed.requeues.is_empty());
        assert!(failed.records.is_empty(), "no record of a failed apply");
        f.fail_message_write(false);
        assert!(
            failed.items.contains(r#""run-stop","fallback","run-stop""#),
            "{}",
            failed.items
        );
        let world = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1);
        assert!(
            world.messages.contains("CoS 不在の退避"),
            "{}",
            world.messages
        );
        assert!(world.requeues.is_empty());
        settle(&mut f).await;
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 1);
}

/// 親の (3) 走査の失敗を mode `ts` で: tick の間ずっと `cos_inbox_items` が読めない。tick は digest の pass に
/// 届く前に落ち、TS は呼ばれない。次の tick で TS の command を apply する（Rust の基準と一致）。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_retry_scan_failure() {
    async fn scenario(ts: bool) -> World {
        let script = go();
        let mut f = setup_ts(ts, Some(script.clone()));
        f.claim_run("run-intr");
        f.finish_run("run-intr", ChatRunState::Interrupted);
        f.observe_run_end();
        f.fail_reads(true);
        f.tick();
        f.d.wait_script_compares().await;
        f.fail_reads(false);
        assert!(f.digest_counts().is_empty(), "the read failed");
        assert!(f.robust_world().records.is_empty());
        if ts {
            assert_eq!(script.calls(), 0, "the failed tick never reached TS");
        }
        let world = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1);
        assert_eq!(world.requeues.len(), 1);
        if ts {
            assert_eq!(script.calls(), 1);
        }
        settle(&mut f).await;
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 1);
}

/// `list_runs` の op の読み出し失敗の retry: TS が `list_runs` を await している間だけ store が読めない。TS は
/// `exception` を受けて落ち、Rust も memo の同じ失敗で何も apply しない（記録も無い）。同じ tick の退避が
/// interrupted の run の item を人へ渡すので、次の tick の digest は TS の command で message だけを書き、
/// requeue しない（item は running に残らない）。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_retry_list_runs_failure_during_op() {
    let mut f = setup_ts(true, None);
    let (db, phases) = (f.db_path.clone(), Arc::new(AtomicUsize::new(0)));
    let seen = phases.clone();
    let script = async_ts(move |phase, op, _| {
        if op == CosDigestOp::ListRuns && seen.load(Ordering::SeqCst) < 2 {
            seen.fetch_add(1, Ordering::SeqCst);
            sql_at(
                &db,
                if phase == Phase::Before {
                    "ALTER TABLE cos_inbox_items RENAME TO test_cos_inbox_items_away;"
                } else {
                    "ALTER TABLE test_cos_inbox_items_away RENAME TO cos_inbox_items;"
                },
            );
        }
        Step::Go
    });
    f.d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    f.claim_run("run-intr");
    f.finish_run("run-intr", ChatRunState::Interrupted);
    f.observe_run_end();
    let failed = settle(&mut f).await;
    assert_eq!(
        phases.load(Ordering::SeqCst),
        2,
        "TS awaited the failing list_runs"
    );
    assert_eq!(script.calls(), 1);
    assert!(f.digest_counts().is_empty(), "the list read failed");
    assert!(failed.records.is_empty(), "{:?}", failed.records);
    assert!(
        failed.items.contains(r#""run-intr","fallback","run-intr""#),
        "{}",
        failed.items
    );
    let world = settle(&mut f).await;
    assert_eq!(f.digest_counts().len(), 1);
    assert!(world.requeues.is_empty(), "{:?}", world.requeues);
    assert_ts_applied(&world, 1);
    assert_eq!(world.records[0].applied.requeued, 0);
    let again = settle(&mut f).await;
    assert_eq!(again.messages, world.messages);
    assert_eq!(again.records.len(), 1);
}

/// op の読み出し失敗の retry: 1 つの run の `read_run` の store 読み出しを TS の await の間だけ失敗させる。
/// TS も Rust もその run を飛ばし（規則 8）、もう 1 つの run だけ TS の command を apply する。次の tick で
/// 飛ばした run を読み直して apply する。message は run ごとに 1 件、記録も run ごとに 1 件。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_retry_read_run_failure_during_op() {
    let mut f = setup_ts(true, None);
    f.claim_run("run-a-done");
    f.finish_run("run-a-done", ChatRunState::Completed);
    f.claim_run("run-b-intr");
    f.finish_run("run-b-intr", ChatRunState::Interrupted);
    let (db, once) = (f.db_path.clone(), Arc::new(AtomicBool::new(false)));
    let script = async_ts(move |phase, op, request| {
        if op == CosDigestOp::ReadRun && read_of(request, "run-a-done") {
            match phase {
                Phase::Before if !once.load(Ordering::SeqCst) => {
                    sql_at(
                        &db,
                        "ALTER TABLE cos_inbox_items RENAME TO test_cos_inbox_items_away;",
                    );
                }
                Phase::After if !once.swap(true, Ordering::SeqCst) => {
                    sql_at(
                        &db,
                        "ALTER TABLE test_cos_inbox_items_away RENAME TO cos_inbox_items;",
                    );
                }
                _ => {}
            }
        }
        Step::Go
    });
    f.d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    f.observe_run_end();
    let first = settle(&mut f).await;
    let digested: Vec<String> = f.digest_counts().into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        digested,
        vec![format!("{DIGEST_KEY}run-b-intr")],
        "only the readable run"
    );
    assert_ts_applied(&first, 1);
    assert_eq!(first.records[0].run_id, "run-b-intr");
    assert_eq!(first.requeues.len(), 1);
    let world = settle(&mut f).await;
    assert_eq!(
        f.digest_counts().len(),
        2,
        "the skipped run is digested on the next tick"
    );
    assert_ts_applied(&world, 2);
    assert_eq!(world.requeues, first.requeues, "no second requeue");
    let again = settle(&mut f).await;
    assert_eq!(again.records.len(), 2);
    assert_eq!(again.messages, world.messages);
}

/// 親の (3) requeue の書き込み失敗を mode `ts` で: その run は message・記録も含めて何も残らず、書き込みが
/// 直った次の pass で message・記録が 1 件ずつ（requeue は item が退避済みなので 0 件）。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_retry_requeue_write_failure() {
    async fn scenario(ts: bool) -> World {
        let mut f = setup_ts(ts, Some(go()));
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.fail_requeue_write(true);
        f.observe_run_end();
        let failed = settle(&mut f).await;
        assert!(f.digest_counts().is_empty(), "the run was rolled back");
        assert!(failed.requeues.is_empty());
        assert!(failed.records.is_empty(), "{:?}", failed.records);
        f.fail_requeue_write(false);
        assert!(
            failed.items.contains(r#""run-stop","fallback","run-stop""#),
            "{}",
            failed.items
        );
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert!(world.requeues.is_empty(), "{:?}", world.requeues);
        assert_eq!(world.items, failed.items);
        assert_eq!(f.digest_counts().len(), 1);
        settle(&mut f).await;
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 1);
}

// ---------------------------------------------------------------- (4) cancel

/// 親の (4) deadline を mode `ts` で、op の await の途中に: 1 つ目の `read_run`（run-intr）の応答の後で TS が止まる。
/// pass の deadline（止めた時計で 200 ms）が打ち切り、Rust の command を `ts_error`（`timeout`）で apply する。
/// 打ち切りの後は op を受け付けないので、TS は 2 つ目の run を読まない。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_cancel_deadline_during_op_await() {
    async fn scenario(ts: bool) -> World {
        let script = async_ts(|phase, op, request| {
            if phase == Phase::After && op == CosDigestOp::ReadRun && read_of(request, "run-intr") {
                Step::Delay(Duration::from_secs(3600))
            } else {
                Step::Go
            }
        });
        let mut f = setup_ts(ts, Some(script.clone()));
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.claim_run("run-intr");
        f.finish_run("run-intr", ChatRunState::Interrupted);
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(world.requeues.len(), 2, "{:?}", world.requeues);
        if ts {
            assert_eq!(script.calls(), 1);
            assert_rust_fallback(&world, 2, "ts_error");
            for r in &world.records {
                assert_eq!(
                    r.error.as_ref().map(|e| e.kind.as_str()),
                    Some("timeout"),
                    "{r:?}"
                );
                let reads: Vec<&str> = r
                    .ops
                    .iter()
                    .filter(|o| o.op == "cos_digest:read_run")
                    .map(|o| o.outcome.as_str())
                    .collect();
                // list は run_id 順（run-intr → run-stop）。run-intr は TS が読んだ（ok）。run-stop は
                // deadline の後なので TS の op は無い（Rust が読み足した）。
                let expected: &[&str] = if r.run_id == "run-intr" { &["ok"] } else { &[] };
                assert_eq!(reads, expected, "{r:?}");
            }
        }
        f.observe_run_end();
        let again = settle(&mut f).await;
        assert_eq!(again.records.len(), world.records.len());
        world
    }
    ts_same_as_rust(scenario).await;
}

/// 親の (4) dispatcher の停止を mode `ts` で、op の await の途中に: TS が `read_run` の応答を待つ間に
/// dispatcher が止まる（tick が panic で巻き戻る）。apply は TS が返った後の 1 か所だけなので、何も書かれて
/// いない。作り直した dispatcher は再起動後の reconcile で pass をやり直し、run ごとに 1 回だけ apply する。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_cancel_dispatcher_stop_during_op_await() {
    async fn scenario(ts: bool) -> World {
        let stopper = async_ts(|phase, op, request| {
            if phase == Phase::After && op == CosDigestOp::ReadRun && read_of(request, "run-stop") {
                Step::Stop
            } else {
                Step::Go
            }
        });
        let mut f = setup_ts(ts, Some(stopper.clone()));
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.claim_run("run-done");
        f.finish_run("run-done", ChatRunState::Completed);
        f.observe_run_end();
        if ts {
            let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.tick()));
            let panic = stopped.expect_err("the dispatcher stopped mid-await");
            let text = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            assert!(text.contains(STOP), "{text}");
            assert_eq!(stopper.calls(), 1);
            let stopped = f.robust_world();
            assert!(
                f.digest_counts().is_empty(),
                "nothing applied before the stop"
            );
            assert!(stopped.requeues.is_empty());
            assert!(stopped.records.is_empty());
            assert!(
                stopped.items.contains(r#""run-stop","running","run-stop""#),
                "{}",
                stopped.items
            );
        }
        let script = go();
        restart_ts(&mut f, ts.then(|| script.clone()));
        let world = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 2);
        assert_eq!(world.requeues.len(), 1);
        if ts {
            assert_eq!(
                script.calls(),
                1,
                "the restarted dispatcher redoes the pass once"
            );
        }
        f.observe_run_end();
        let again = settle(&mut f).await;
        assert_eq!(again.messages, world.messages);
        assert_eq!(again.records.len(), world.records.len());
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 2);
}

// ---------------------------------------------------------------- (5) 再起動

/// 親の (5) を mode `ts` で: digest 前に落ちた run は再起動後に 1 回だけ、digest 済みの run は増えない。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_restart_mid_pass_applies_once() {
    async fn scenario(ts: bool) -> World {
        let mut f = setup_ts(ts, Some(go()));
        f.claim_run("run-1-before-crash");
        f.finish_run("run-1-before-crash", ChatRunState::Interrupted);
        restart_ts(&mut f, ts.then(go));
        let first = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1, "reconcile after start digests");
        f.claim_run("run-2-after");
        f.finish_run("run-2-after", ChatRunState::Completed);
        restart_ts(&mut f, ts.then(go));
        let second = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 2);
        restart_ts(&mut f, ts.then(go));
        let world = settle(&mut f).await;
        assert_eq!(world.messages, second.messages);
        assert_eq!(world.records.len(), second.records.len());
        assert_eq!(world.requeues, first.requeues);
        assert_eq!(world.requeues.len(), 1);
        if ts {
            assert_eq!(
                cos_digest_row(&f).mode,
                ScriptMode::Ts,
                "the mode row survives restarts"
            );
        }
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 2);
}

/// 親の (5) requeue の失敗の後の再起動を mode `ts` で: 失敗した run は何も残らず、再起動後に message・記録が 1 件ずつ。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_restart_after_failed_requeue() {
    async fn scenario(ts: bool) -> World {
        let mut f = setup_ts(ts, Some(go()));
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.fail_requeue_write(true);
        f.observe_run_end();
        settle(&mut f).await;
        assert!(f.digest_counts().is_empty(), "the run was rolled back");
        f.fail_requeue_write(false);
        restart_ts(&mut f, ts.then(go));
        let world = settle(&mut f).await;
        assert!(world.requeues.is_empty(), "{:?}", world.requeues);
        assert!(
            world.items.contains(r#""run-stop","fallback","run-stop""#),
            "{}",
            world.items
        );
        assert_eq!(f.digest_counts().len(), 1);
        world
    }
    let ts = ts_same_as_rust(scenario).await;
    assert_ts_applied(&ts, 1);
}

// ---------------------------------------------------------------- (7) mode・generation の切替

/// 親の (7) を mode `ts` で、op の await の途中に: TS が `list_runs` の応答を待つ間に mode を `rust` に戻す。
/// pass は始めの generation で最後まで走るが、apply 直前の読み直しで `mode_changed` になり Rust の command を
/// apply する。`rust` の間の run は TS に渡らない。新しい generation で `ts` に戻した後の run は TS の command。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_mode_and_generation_change_during_op_await() {
    async fn scenario(ts: bool) -> World {
        let mut f = setup_ts(ts, None);
        let first_generation = cos_digest_row(&f).generation_id;
        let (store, once) = (f.store.clone(), Arc::new(AtomicBool::new(false)));
        let script = async_ts(move |phase, op, _| {
            if phase == Phase::After
                && op == CosDigestOp::ListRuns
                && !once.swap(true, Ordering::SeqCst)
            {
                back_to_rust_of(&store);
            }
            Step::Go
        });
        if ts {
            f.d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
        }
        f.claim_run("run-1");
        f.finish_run("run-1", ChatRunState::Stopped);
        f.observe_run_end();
        f.tick();
        let calls_first = script.calls();
        f.claim_run("run-2");
        f.finish_run("run-2", ChatRunState::Stopped);
        f.observe_run_end();
        f.next_tick();
        let calls_while_rust = script.calls();
        let second_generation = ts.then(|| enter_ts(&f));
        f.claim_run("run-3");
        f.finish_run("run-3", ChatRunState::Completed);
        f.observe_run_end();
        f.next_tick();
        let world = settle(&mut f).await;
        if ts {
            assert_eq!(
                (calls_first, calls_while_rust),
                (1, 1),
                "run-2 was not given to TS"
            );
            let mut records = world.records.clone();
            records.sort_by(|a, b| a.run_id.cmp(&b.run_id));
            assert_eq!(records.len(), 2, "{records:?}");
            assert_eq!(records[0].run_id, "run-1");
            assert_eq!(Some(&records[0].generation), first_generation.as_ref());
            assert_eq!(records[0].applied_by, CosDigestAppliedBy::Rust);
            assert_eq!(
                records[0].fallback.as_ref().map(|f| f.code.as_str()),
                Some("mode_changed")
            );
            assert_eq!(records[1].run_id, "run-3");
            assert_eq!(Some(&records[1].generation), second_generation.as_ref());
            assert_eq!(records[1].applied_by, CosDigestAppliedBy::Ts);
            assert_eq!(records[1].fallback, None);
        }
        assert_eq!(world.requeues.len(), 2);
        f.observe_run_end();
        f.next_tick();
        let again = settle(&mut f).await;
        assert_eq!(again.records.len(), world.records.len());
        world
    }
    ts_same_as_rust(scenario).await;
}

/// await 中の generation の作り直し（S4-18）: TS が `read_run` の応答を待つ間に isolate が作り直され、op は
/// `generation_gone` を返す。pass は `ScriptError::GenerationGone` で落ち、Rust の command を `ts_error` で
/// apply する。`generation_gone` は安全違反に数えない。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_generation_gone_during_await_falls_back() {
    async fn scenario(ts: bool) -> World {
        let script = async_ts(|phase, op, request| {
            if phase == Phase::After && op == CosDigestOp::ReadRun && read_of(request, "run-b-intr")
            {
                Step::GenerationGone
            } else {
                Step::Go
            }
        });
        let mut f = setup_ts(ts, Some(script));
        f.claim_run("run-a-done");
        f.finish_run("run-a-done", ChatRunState::Completed);
        f.claim_run("run-b-intr");
        f.finish_run("run-b-intr", ChatRunState::Interrupted);
        f.observe_run_end();
        let world = settle(&mut f).await;
        if ts {
            assert_rust_fallback(&world, 2, "ts_error");
            for r in &world.records {
                assert_eq!(
                    r.error.as_ref().map(|e| e.kind.as_str()),
                    Some("generation_gone"),
                    "{r:?}"
                );
                assert!(r.safety_violations.is_empty(), "{r:?}");
            }
        }
        f.observe_run_end();
        let again = settle(&mut f).await;
        assert_eq!(again.records.len(), world.records.len());
        assert_eq!(again.messages, world.messages);
        world
    }
    ts_same_as_rust(scenario).await;
}

// ---------------------------------------------------------------- (6) policy_version と session の freshness

/// 親の (6) を mode `ts` で: 本物の digest 経路（FakeAdapter の CoS run → worker の join → digest）で、
/// `[cos.triage] policy_version` を上げた後の run が fresh（policy_changed）で始まり、その run の digest も
/// 1 件、TS の command の記録も 1 件。
#[tokio::test]
async fn cos_digest_script_ts_policy_version_bump_session_fresh() {
    async fn scenario(ts: bool) -> (World, Vec<Option<String>>) {
        let mut f = fixture(true);
        f.install_requeue_log();
        f.d.set_script_domain_store(f.store.clone());
        if ts {
            enter_ts(&f);
            f.d.set_policy_script(Some(go() as Arc<dyn PolicyScript>));
        }
        f.tick();
        f.question("before-policy", vec![]);
        f.tick();
        assert!(f.join_inbox().await, "first triage run");
        f.tick();
        let mut cfg = f.d.cos_chat_launch.as_ref().expect("launch").config.clone();
        cfg.triage.policy_version = "7".into();
        f.d.reload_cos_chat_launch(cfg);
        f.question("after-policy", vec![]);
        f.tick();
        assert!(f.join_inbox().await, "second triage run");
        f.tick();
        f.observe_run_end();
        f.tick();
        f.join_inbox().await;
        f.tick();
        f.d.wait_script_compares().await;
        let world = f.robust_world();
        assert_invariants(&f, &world);
        let conn = rusqlite::Connection::open(&f.db_path).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT r.run_id FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id \
                 WHERE m.role='system' AND m.text LIKE '%item_ids=%' ORDER BY r.started_at, r.run_id",
            )
            .expect("runs");
        let runs: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .expect("runs")
            .map(|r| r.expect("row"))
            .collect();
        assert_eq!(runs.len(), 2, "two triage runs: {runs:?}");
        let reasons: Vec<Option<String>> = runs
            .iter()
            .map(|r| f.store.chat_run_session_record(r).expect("record").reason)
            .collect();
        assert_eq!(
            f.store.chat_run_policy_version(&runs[1]).expect("policy"),
            Some("7".into())
        );
        let digests = f.digest_counts();
        assert_eq!(digests.len(), 2, "one digest per triage run: {digests:?}");
        for run in &runs {
            assert!(
                digests.iter().any(|(k, _)| k.ends_with(run.as_str())),
                "{run}: {digests:?}"
            );
        }
        (world, reasons)
    }
    let (base, base_reasons) = scenario(false).await;
    let (ts, reasons) = scenario(true).await;
    assert_eq!(base_reasons, reasons);
    assert_eq!(reasons[1].as_deref(), Some("policy_changed"), "{reasons:?}");
    assert_eq!(base.requeues, ts.requeues);
    assert!(base.records.is_empty());
    assert_ts_applied(&ts, 2);
}

#[path = "cos_digest_persist.rs"]
mod persist;
