//! ADR 2026-11-03-cos-digest-persist: the apply of a digest pass, its compare records and the
//! script-guard count are stored together or not at all.
//!
//! Crashes are injected by the test hook of the apply transaction (a hook error drops the
//! transaction like a process that dies at that stage) and, once, by a child process that really
//! aborts inside the transaction. Restarts rebuild the dispatcher over the same DB. Nothing waits
//! on the wall clock: the dispatcher's clock is injected and tokio's clock is paused.
//!
//! Every scenario checks, after the restart, that each digest message exists once, every item was
//! requeued at most once, every digested run of a `compare`/`ts` pass has exactly one record, the
//! shadow outbox is empty and the script-guard count equals the counted records.

use super::*;
use crate::dispatcher::cos_chat::digest::{set_test_apply_hook, set_test_before_apply};
use task_core::chat::digest_apply::{
    CosDigestApplyPass, CosDigestApplyRun, CosDigestApplyStage, CosDigestRecordPlan,
};
use task_core::script_compare::ScriptDomain;
use task_core::script_guard::ScriptGuardCall;

/// Clears the thread's hooks when a test ends (also on a failed assertion).
struct Hooks;

impl Drop for Hooks {
    fn drop(&mut self) {
        set_test_apply_hook(None);
        set_test_before_apply(None);
    }
}

const DIED: &str = "test: the process died inside the apply transaction";

/// Kill the "process" at the apply's `n`-th hook call (1-based): the hook panics, so neither the
/// commit nor the rest of the tick runs (the transaction is rolled back as the stack unwinds, like
/// SQLite's journal rollback after a real crash). Returns the names of every stage seen.
fn crash_at(n: usize) -> (Hooks, Arc<StdMutex<Vec<String>>>) {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let log = seen.clone();
    set_test_apply_hook(Some(Box::new(move |stage: CosDigestApplyStage<'_>| {
        let count = {
            let mut log = log.lock().expect("log");
            log.push(format!("{stage:?}"));
            log.len()
        };
        if count == n {
            panic!("{DIED}");
        }
        Ok(())
    })));
    (Hooks, seen)
}

/// One tick that dies inside the apply; then the store is opened again (the dead process's
/// connection and dispatcher are gone).
fn tick_and_die(f: &mut Fixture) {
    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.tick()));
    let Err(panic) = died else {
        panic!("the tick did not reach the crash");
    };
    let text = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default();
    assert!(text.contains(DIED), "{text}");
    f.store = Arc::new(SqliteStore::open(&f.db_path).expect("reopen"));
}

/// The modes of the crash scenarios.
#[derive(Clone, Copy, Debug)]
enum Mode {
    Rust,
    Compare,
    Ts,
}

fn setup_mode(mode: Mode) -> Fixture {
    match mode {
        Mode::Rust => super::setup(Ts::Without).0,
        Mode::Compare => super::setup(Ts::Compare(Answer::Echo)).0,
        Mode::Ts => setup_ts(true, Some(go())),
    }
}

fn restart_mode(f: &mut Fixture, mode: Mode) {
    match mode {
        Mode::Rust => {
            f.restart_digest(Ts::Without);
        }
        Mode::Compare => {
            f.restart_digest(Ts::Compare(Answer::Echo));
        }
        Mode::Ts => restart_ts(f, Some(go())),
    }
}

/// Two finished triage runs whose items are still `running` (both get a requeue).
fn two_runs(f: &mut Fixture) {
    f.claim_run("run-a-stop");
    f.finish_run("run-a-stop", ChatRunState::Stopped);
    f.claim_run("run-b-intr");
    f.finish_run("run-b-intr", ChatRunState::Interrupted);
    f.observe_run_end();
}

/// The script-guard calls counted for the generation the mode row points to.
fn guard_calls(f: &Fixture) -> u64 {
    let generation = f
        .store
        .script_domain_mode("cos_digest")
        .expect("row")
        .generation_id;
    guard_calls_of(f, generation)
}

fn guard_calls_of(f: &Fixture, generation: Option<String>) -> u64 {
    generation
        .and_then(|g| {
            f.store
                .script_guard_counters(ScriptDomain::CosDigest, &g)
                .expect("counters")
        })
        .map(|(_, c)| c.calls)
        .unwrap_or(0)
}

fn running_items(f: &Fixture) -> usize {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.query_row(
        "SELECT count(*) FROM cos_inbox_items WHERE state='running'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| usize::try_from(n).unwrap_or(0))
    .expect("count")
}

/// Nothing of the pass was stored.
fn assert_nothing_stored(f: &Fixture, calls_before: u64, at: &str) {
    let world = f.robust_world();
    assert!(f.digest_counts().is_empty(), "{at}: no message");
    assert!(world.requeues.is_empty(), "{at}: no requeue");
    assert!(world.records.is_empty(), "{at}: no record");
    assert!(
        f.store
            .cos_digest_shadow_pending()
            .expect("outbox")
            .is_empty(),
        "{at}: no outbox row"
    );
    assert_eq!(guard_calls(f), calls_before, "{at}: no count");
    assert_eq!(running_items(f), 2, "{at}: items still running");
}

/// After the restart: one message per run, one record per digested run (modes with records), the
/// outbox empty and the count equal to the counted records.
fn assert_complete(f: &Fixture, mode: Mode, world: &World) {
    let digested: Vec<String> = f
        .digest_counts()
        .into_iter()
        .map(|(key, n)| {
            assert_eq!(n, 1, "{key}: one message");
            key
        })
        .collect();
    assert_eq!(digested.len(), 2, "{digested:?}");
    assert!(
        f.store
            .cos_digest_shadow_pending()
            .expect("outbox")
            .is_empty(),
        "outbox closed"
    );
    match mode {
        Mode::Rust => assert!(world.records.is_empty()),
        Mode::Compare | Mode::Ts => {
            for key in &digested {
                let run = key.trim_start_matches(DIGEST_KEY);
                let n = world.records.iter().filter(|r| r.run_id == run).count();
                assert_eq!(n, 1, "{run}: one record, got {:?}", world.records);
            }
            let counted = world
                .records
                .iter()
                .filter(|r| {
                    ScriptGuardCall::classify(r.outcome, r.error.as_ref().map(|e| e.kind.as_str()))
                        .is_some()
                })
                .count() as u64;
            let generation = world.records.first().map(|r| r.generation.clone());
            assert_eq!(
                guard_calls_of(f, generation),
                counted,
                "the count follows the records"
            );
        }
    }
}

/// Every stage of the apply (before, between the commands, after the records, before the commit)
/// and after the commit: a crash there, then a restart, leaves no duplicate and no missing record.
async fn crash_everywhere(mode: Mode) {
    // The stages of one clean pass.
    let stages = {
        let mut f = setup_mode(mode);
        two_runs(&mut f);
        let (_hooks, seen) = crash_at(usize::MAX);
        let world = settle(&mut f).await;
        assert_complete(&f, mode, &world);
        let seen = seen.lock().expect("seen").clone();
        assert_eq!(seen.first().map(String::as_str), Some("Begin"), "{seen:?}");
        assert_eq!(
            seen.last().map(String::as_str),
            Some("BeforeCommit"),
            "{seen:?}"
        );
        seen
    };
    // 1 + 2 runs × (post_digest, requeue_run, record) + 1.
    assert_eq!(stages.len(), 8, "{stages:?}");
    for n in 1..=stages.len() {
        let at = &stages[n - 1];
        let mut f = setup_mode(mode);
        two_runs(&mut f);
        let calls_before = guard_calls(&f);
        {
            let _hooks = crash_at(n);
            tick_and_die(&mut f);
        }
        assert_nothing_stored(&f, calls_before, at);
        restart_mode(&mut f, mode);
        let world = settle(&mut f).await;
        assert_complete(&f, mode, &world);
        assert_eq!(world.requeues.len(), 2, "{at}: each item once");
        let again = settle(&mut f).await;
        assert_eq!(again.messages, world.messages, "{at}");
        assert_eq!(again.records, world.records, "{at}");
    }
    // After the commit: the pass is stored whole; the restart adds nothing.
    let mut f = setup_mode(mode);
    two_runs(&mut f);
    f.tick();
    // The compare shadow dies with the process (its outbox rows stay).
    f.d.abort_script_compares().await;
    let committed = f.robust_world();
    assert_eq!(f.digest_counts().len(), 2);
    assert_eq!(committed.requeues.len(), 2);
    restart_mode(&mut f, mode);
    let world = settle(&mut f).await;
    assert_complete(&f, mode, &world);
    assert_eq!(world.messages, committed.messages);
    assert_eq!(world.requeues, committed.requeues);
    if let Mode::Compare = mode {
        // The lost shadow is closed by the restarted dispatcher with `unavailable` records.
        assert!(
            world
                .records
                .iter()
                .all(|r| r.outcome == CompareOutcome::Unavailable)
        );
    }
}

/// Mode `rust` (production): the message and the requeue of a run are written together. Before,
/// a crash between them left a digested run whose items were never requeued.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_crash_at_each_stage_rust() {
    crash_everywhere(Mode::Rust).await;
}

/// Mode `compare`: the apply and the shadow's outbox rows are one transaction; the shadow or the
/// reconcile writes exactly one record per run.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_crash_at_each_stage_compare() {
    crash_everywhere(Mode::Compare).await;
}

/// Mode `ts`: the TS output's effects, the records (`applied_by = ts`) and the count are one
/// transaction.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_crash_at_each_stage_ts() {
    crash_everywhere(Mode::Ts).await;
    let mut f = setup_mode(Mode::Ts);
    two_runs(&mut f);
    let world = settle(&mut f).await;
    assert_ts_applied(&world, 2);
}

const ABORT_ENV: &str = "CELERIS_TEST_COS_DIGEST_PERSIST_ABORT";

/// A real process abort inside the transaction (after the first run's record is written): the
/// parent opens the same DB file, finds nothing of the pass and digests it once after the restart.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_process_abort_mid_transaction() {
    if let Ok(handoff) = std::env::var(ABORT_ENV) {
        // The child: mode `ts`, two runs, abort at the first `AfterRecord`.
        let mut f = setup_mode(Mode::Ts);
        two_runs(&mut f);
        std::fs::write(&handoff, f.db_path.to_string_lossy().as_bytes()).expect("handoff");
        set_test_apply_hook(Some(Box::new(|stage: CosDigestApplyStage<'_>| {
            if matches!(stage, CosDigestApplyStage::AfterRecord { .. }) {
                std::process::abort();
            }
            Ok(())
        })));
        f.tick();
        panic!("the child did not abort");
    }
    let handoff_dir = crate::test_support::WritableTempDir::new();
    let handoff = handoff_dir.path().join("db-path");
    let name = format!(
        "{}::cos_digest_persist_process_abort_mid_transaction",
        module_path!()
            .split_once("::")
            .map(|(_, rest)| rest)
            .unwrap_or_default()
    );
    let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([name.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(ABORT_ENV, &handoff)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("child");
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc_sigabrt()), "{status:?}");
    }
    let db_path = PathBuf::from(std::fs::read_to_string(&handoff).expect("db path"));
    let store = Arc::new(SqliteStore::open(&db_path).expect("reopen"));
    let dir = crate::test_support::WritableTempDir::new();
    let d = triage_dispatcher(dir.path(), &db_path, store.clone(), false, "true");
    let mut f = Fixture {
        dir,
        db_path: db_path.clone(),
        store,
        d,
    };
    let calls_before = guard_calls(&f);
    assert_nothing_stored(&f, calls_before, "process abort");
    restart_mode(&mut f, Mode::Ts);
    let world = settle(&mut f).await;
    assert_complete(&f, Mode::Ts, &world);
    assert_ts_applied(&world, 2);
    assert_eq!(world.requeues.len(), 2);
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

/// `SIGABRT` (6 on Linux).
fn libc_sigabrt() -> i32 {
    6
}

/// The same pass delivered twice (the same pass id, then another pass id with the same commands):
/// one message, one requeue, one record and one count. The shadow's outbox row is closed once.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_duplicate_pass_delivery() {
    let mut f = setup_mode(Mode::Ts);
    f.claim_run("run-first");
    f.finish_run("run-first", ChatRunState::Completed);
    f.observe_run_end();
    let first = settle(&mut f).await;
    assert_ts_applied(&first, 1);
    let template = first.records[0].clone();
    let calls = guard_calls(&f);
    let row = f.store.script_domain_mode("cos_digest").expect("row");
    let now = f.d.now_utc();

    f.claim_run("run-dup");
    f.finish_run("run-dup", ChatRunState::Stopped);
    let plan = |pass_id: &str, record: CosDigestRecordPlan| CosDigestApplyPass {
        pass_id: pass_id.to_owned(),
        verify: true,
        expect_mode: Some((row.mode, row.generation_id.clone(), row.version)),
        runs: vec![CosDigestApplyRun {
            thread_id: template.thread_id.clone(),
            run_id: "run-dup".into(),
            commands: vec![
                task_core::chat::digest::CosDigestCommand::PostDigest {
                    thread: template.thread_id.clone(),
                    idempotency_key: format!("{DIGEST_KEY}run-dup"),
                    text: "digest of run-dup".into(),
                    cards: Vec::new(),
                    run_id: "run-dup".into(),
                },
                task_core::chat::digest::CosDigestCommand::RequeueRun {
                    run_id: "run-dup".into(),
                },
            ],
            record: Some(record),
        }],
    };
    let mut record = template.clone();
    record.run_id = "run-dup".into();
    record.input_digest = "sha256:run-dup".into();
    let append = || CosDigestRecordPlan::Append {
        record: Box::new(record.clone()),
        ts_side: true,
    };
    for pass_id in ["P1", "P1", "P2"] {
        let applied = f
            .store
            .cos_digest_apply_pass(&plan(pass_id, append()), now)
            .expect("apply");
        assert!(applied.unverified.is_none());
    }
    let world = f.robust_world();
    let dup: Vec<_> = world
        .records
        .iter()
        .filter(|r| r.run_id == "run-dup")
        .collect();
    assert_eq!(dup.len(), 1, "{dup:?}");
    assert_eq!(dup[0].applied.requeued, 1, "the first delivery requeued");
    assert_eq!(
        world
            .requeues
            .iter()
            .filter(|(_, run)| run == "run-dup")
            .count(),
        1
    );
    assert_eq!(guard_calls(&f), calls + 1, "counted once");
    assert_eq!(
        f.digest_counts()
            .into_iter()
            .filter(|(k, _)| k.ends_with("run-dup"))
            .map(|(_, n)| n)
            .collect::<Vec<_>>(),
        vec![1]
    );

    // The shadow path: the same outbox row twice, closed twice (the second close writes nothing).
    f.claim_run("run-shadow");
    f.finish_run("run-shadow", ChatRunState::Completed);
    let mut lost = template.clone();
    lost.run_id = "run-shadow".into();
    lost.input_digest = "sha256:run-shadow".into();
    lost.outcome = CompareOutcome::Unavailable;
    let shadow = |pass_id: &str| CosDigestApplyPass {
        pass_id: pass_id.to_owned(),
        verify: false,
        expect_mode: None,
        runs: vec![CosDigestApplyRun {
            thread_id: template.thread_id.clone(),
            run_id: "run-shadow".into(),
            commands: vec![task_core::chat::digest::CosDigestCommand::PostDigest {
                thread: template.thread_id.clone(),
                idempotency_key: format!("{DIGEST_KEY}run-shadow"),
                text: "digest of run-shadow".into(),
                cards: Vec::new(),
                run_id: "run-shadow".into(),
            }],
            record: Some(CosDigestRecordPlan::Shadow {
                lost: Box::new(lost.clone()),
            }),
        }],
    };
    for _ in 0..2 {
        f.store
            .cos_digest_apply_pass(&shadow("S1"), now)
            .expect("apply");
    }
    assert_eq!(
        f.store.cos_digest_shadow_pending().expect("outbox").len(),
        1
    );
    let closed = f
        .store
        .cos_digest_shadow_finish("S1", "run-shadow", None, now)
        .expect("finish");
    assert!(matches!(closed, Some((true, None))), "{closed:?}");
    assert_eq!(
        f.store
            .cos_digest_shadow_finish("S1", "run-shadow", None, now)
            .expect("finish"),
        None,
        "the row is gone: nothing written"
    );
    let shadows: Vec<_> = f
        .robust_world()
        .records
        .into_iter()
        .filter(|r| r.run_id == "run-shadow")
        .collect();
    assert_eq!(shadows.len(), 1);
    assert_eq!(guard_calls(&f), calls + 1, "unavailable is not counted");
}

/// A mode CAS lands between the pass's mode check and the apply transaction (a human or the
/// script-guard moves `ts` to `rust`). The transaction sees the new row: the TS output is not
/// applied; the pass applies Rust's commands with fallback `mode_changed`, recorded once per run.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_mode_cas_race_applies_rust() {
    let mut f = setup_mode(Mode::Ts);
    two_runs(&mut f);
    let generation = f
        .store
        .script_domain_mode("cos_digest")
        .expect("row")
        .generation_id;
    let calls_before = guard_calls_of(&f, generation.clone());
    let store = f.store.clone();
    let fired = Arc::new(AtomicBool::new(false));
    let once = fired.clone();
    let _hooks = Hooks;
    set_test_before_apply(Some(Box::new(move |_| {
        if once.swap(true, Ordering::SeqCst) {
            return;
        }
        let row = store.script_domain_mode("cos_digest").expect("row");
        store
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: "cos_digest".into(),
                    to_mode: ScriptMode::Rust,
                    to_generation: None,
                    expected_version: row.version,
                    actor: "human:test".into(),
                    reason: "rollback during the pass".into(),
                },
                time::OffsetDateTime::now_utc(),
            )
            .expect("cas");
    })));
    let world = settle(&mut f).await;
    assert!(fired.load(Ordering::SeqCst));
    assert_rust_fallback(&world, 2, "mode_changed");
    assert_eq!(world.requeues.len(), 2);
    assert_eq!(f.digest_counts().len(), 2);
    assert!(
        f.store
            .cos_digest_shadow_pending()
            .expect("outbox")
            .is_empty()
    );
    // The rolled-back generation keeps counting in the mode it started with (S5-7): +2.
    assert_eq!(guard_calls_of(&f, generation), calls_before + 2);
    assert_eq!(
        f.store.script_domain_mode("cos_digest").expect("row").mode,
        ScriptMode::Rust
    );
    // The next pass is mode `rust`: nothing more is written.
    let again = settle(&mut f).await;
    assert_eq!(again.records, world.records);
    assert_eq!(again.messages, world.messages);
}

/// A TS pass that reads a run outside its list (`denied`) and then answers like Rust.
struct DeniedTs;

impl PolicyScript for DeniedTs {
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
        Box::pin(async move {
            let _ = send_op(
                &call.sink,
                CosDigestOp::ReadRun,
                serde_json::json!({ "thread": "elsewhere", "run_id": "not-listed" }),
            )
            .await;
            crate::script_policy::test_pass::reference_reply(&call).await
        })
    }
}

/// Rollback by script-guard in the apply transaction: a denied op makes every record a safety
/// violation, which invalidates the generation and rolls `cos_digest` back to `rust`. A crash
/// before the commit keeps the mode, the generation, the records and the count as they were; after
/// the restart the effects, the records, the count and the rollback land together, once.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_guard_rollback_with_records() {
    let mut f = setup_ts(true, None);
    f.d.set_policy_script(Some(Arc::new(DeniedTs) as Arc<dyn PolicyScript>));
    two_runs(&mut f);
    let generation = f
        .store
        .script_domain_mode("cos_digest")
        .expect("row")
        .generation_id
        .expect("generation");
    let calls_before = guard_calls(&f);
    {
        let _hooks = Hooks;
        set_test_apply_hook(Some(Box::new(|stage: CosDigestApplyStage<'_>| {
            if stage == CosDigestApplyStage::BeforeCommit {
                panic!("{DIED}");
            }
            Ok(())
        })));
        tick_and_die(&mut f);
    }
    assert_nothing_stored(&f, calls_before, "before the commit");
    assert_eq!(
        f.store.script_domain_mode("cos_digest").expect("row").mode,
        ScriptMode::Ts,
        "the rollback was not committed either"
    );
    assert_ne!(
        f.store
            .script_generation_get(&generation)
            .expect("read")
            .expect("generation")
            .state,
        ScriptGenerationState::Invalid
    );
    f.d = triage_dispatcher(f.dir.path(), &f.db_path, f.store.clone(), false, "true");
    f.d.set_script_domain_store(f.store.clone());
    f.d.set_policy_script(Some(Arc::new(DeniedTs) as Arc<dyn PolicyScript>));
    let world = settle(&mut f).await;
    assert_rust_fallback(&world, 2, "denied");
    assert!(
        world
            .records
            .iter()
            .all(|r| r.safety_violations.contains(&"denied_scope".to_owned()))
    );
    assert_eq!(world.requeues.len(), 2);
    assert_eq!(
        f.store.script_domain_mode("cos_digest").expect("row").mode,
        ScriptMode::Rust
    );
    assert_eq!(
        f.store
            .script_generation_get(&generation)
            .expect("read")
            .expect("generation")
            .state,
        ScriptGenerationState::Invalid
    );
    let counted = f
        .store
        .script_guard_counters(ScriptDomain::CosDigest, &generation)
        .expect("counters")
        .map(|(_, c)| c.calls);
    assert_eq!(counted, Some(2), "both records counted with the rollback");
    let again = settle(&mut f).await;
    assert_eq!(again.records, world.records);
}

/// The reconcile never closes the outbox row of a shadow that is still running in this process;
/// the shadow closes it with its own record.
#[tokio::test(start_paused = true)]
async fn cos_digest_persist_reconcile_skips_live_shadow() {
    let (mut f, _script) = super::setup(Ts::Compare(Answer::Hang));
    two_runs(&mut f);
    f.tick();
    assert_eq!(
        f.store.cos_digest_shadow_pending().expect("outbox").len(),
        2
    );
    // Force a reconcile on the next tick while the shadow still hangs.
    f.d.script_compare.digest_shadows.force_reconcile_for_test();
    f.tick();
    assert_eq!(
        f.store.cos_digest_shadow_pending().expect("outbox").len(),
        2,
        "the live shadow keeps its rows"
    );
    assert!(f.robust_world().records.is_empty());
    let world = settle(&mut f).await;
    assert_complete(&f, Mode::Compare, &world);
    assert!(
        world
            .records
            .iter()
            .all(|r| r.outcome == CompareOutcome::TsError),
        "{:?}",
        world.records
    );
}
