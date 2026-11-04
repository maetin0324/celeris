//! ADR 2026-11-04-lane-rules-exhaustive-evidence: 一時 DB の daemon の generation（`celeris::ScriptGenerations`、本物の
//! deno_core の isolate）へ `lane_rules` の全入力を流す網羅。
//!
//! - 本物の bundle の試験は control-ts の source から `$TMPDIR` に作った bundle を使う（`support/script_compare.rs`）。
//!   作れなければ SKIP（`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗）。常に違う答えを返す bundle は、この file の中の
//!   手書きの JS で作る（bundle を作れなくても走る）。
//! - 待ちは出来事待ち（runner の遅延フック `ExhaustiveTuning::before_call`、`wait_exhaustive`）で決める。CPU 負荷はかけない。
//!   本物の bundle の全件の試験だけは、負荷のかかった試験機で 50 ms の deadline を超えないよう deadline を長くし、
//!   間隔を 0 にする（50 ms の deadline・間隔・timeout の扱いは `src/daemon/script_exhaustive_tests.rs` が時計の差し替えで
//!   確かめる）。外部ネットワークに出ない。

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use celeris::{ExhaustiveTuning, ScriptGenerations};
use celeris_script::manifest::{deploy_bundle, deployed_dir, host_schema_sha256, sha256_hex};
use celeris_script::{GenerationRegistry, RuntimeLimits};
use serde_json::json;
use support::script_compare::{Env as CompareEnv, dist_bundle, dist_dir};
use task_api::script_generations::{ScriptGenerationControl, ScriptGenerationError};
use task_core::SqliteStore;
use task_core::script_compare::{
    CompareOutcome, ScriptDomain, ScriptGeneration, ScriptMode, ScriptModeChange,
};
use task_core::script_exhaustive::{
    LANE_RULES_EXHAUSTIVE_CALLS, ScriptExhaustiveRun, ScriptExhaustiveStanding,
    ScriptExhaustiveState, ScriptExhaustiveStopCode,
};
use task_core::store::current_reference_sha256;
use time::OffsetDateTime;
use tokio::sync::Notify;

/// 常に違う答えを返す bundle（登録の検査の `host_selftest` には答える）。`__TAG__` で hash を変える。
const DIFFERENT: &str = r#"
import { hostVersion, selftestEcho, cosDigestListRuns } from "celeris:host";

globalThis.__celerisDispatch = async (domain, input, ctx) => {
  if (domain === "host_selftest") {
    return { host_api: hostVersion(), reply: await selftestEcho(ctx, input) };
  }
  if (domain === "lane_rules") {
    if (input === null) throw new Error("bad input");
    return { rule_id: "always/other", proposed: "frontier", condition: "__TAG__" };
  }
  if (domain === "cos_digest") {
    const list = await cosDigestListRuns(ctx, {});
    return { runs: [], list };
  }
  throw new Error(`unknown domain: ${domain}`);
};
"#;

const LONG_DEADLINE: Duration = Duration::from_secs(30);

fn deploy_raw(state: &Path, code: &str) -> String {
    let sha256 = sha256_hex(code.as_bytes());
    let manifest = json!({
        "schema": "celeris.script-manifest/1",
        "bundle_sha256": sha256,
        "host_api": "^1",
        "schema_sha256": host_schema_sha256(),
        "toolchain": {"typescript": "fixture", "esbuild": "fixture", "node": "fixture"},
        "source_commit": "fixture",
        "domains": ["cos_digest", "host_selftest", "lane_rules"],
        "capabilities": ["cos_digest:list_runs", "cos_digest:read_run", "selftest:echo_task"],
        "checkpoint_versions": [],
    });
    write_deployed(state, &sha256, code, &manifest);
    sha256
}

fn write_deployed(state: &Path, sha256: &str, code: &str, manifest: &serde_json::Value) {
    let dir = deployed_dir(state, sha256);
    std::fs::create_dir_all(&dir).expect("deployed dir");
    std::fs::write(dir.join("bundle.mjs"), code).expect("bundle");
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(manifest).expect("manifest"),
    )
    .expect("manifest");
}

/// 一時 DB と配備先。
struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.dir.path().join("t.sqlite3")
    }

    fn state(&self) -> std::path::PathBuf {
        self.dir.path().join("state")
    }

    fn open(&self) -> Arc<SqliteStore> {
        Arc::new(SqliteStore::open(&self.db_path()).expect("store"))
    }

    /// daemon の起動 1 回（新しい instance id・新しい登録簿、起動時の復元まで）。
    async fn daemon(&self, db: &Arc<SqliteStore>) -> Arc<ScriptGenerations> {
        let g = ScriptGenerations::new(
            GenerationRegistry::new(RuntimeLimits::default()),
            self.state(),
            Arc::clone(db),
        );
        g.restore().await.expect("restore");
        g
    }
}

fn fast() -> ExhaustiveTuning {
    ExhaustiveTuning {
        deadline: LONG_DEADLINE,
        gap: Duration::ZERO,
        ..ExhaustiveTuning::default()
    }
}

async fn register(g: &Arc<ScriptGenerations>, sha256: &str) -> ScriptGeneration {
    let control = g.control();
    let sha256 = sha256.to_owned();
    tokio::task::spawn_blocking(move || control.register(&sha256))
        .await
        .expect("register task")
        .expect("register")
}

fn set_mode(db: &SqliteStore, mode: ScriptMode, generation: &ScriptGeneration) {
    let row = db.script_domain_mode("lane_rules").expect("mode");
    db.script_domain_set_mode(
        &ScriptModeChange {
            domain: "lane_rules".into(),
            to_mode: mode,
            to_generation: Some(generation.id.clone()),
            expected_version: row.version,
            actor: "human:test".into(),
            reason: "exhaustive test".into(),
        },
        OffsetDateTime::now_utc(),
    )
    .expect("set mode");
}

/// API と同じく blocking の thread から始める。
async fn start(
    g: &Arc<ScriptGenerations>,
    sha256: &str,
) -> Result<ScriptExhaustiveRun, ScriptGenerationError> {
    let control = g.control();
    let sha256 = sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        control.exhaustive_start(
            ScriptDomain::LaneRules,
            &sha256,
            "operator:claude-code",
            "exhaustive test",
        )
    })
    .await
    .expect("start task")
}

fn run_row(db: &SqliteStore, id: &str) -> ScriptExhaustiveRun {
    db.script_exhaustive_runs(ScriptDomain::LaneRules, None, 50)
        .expect("runs")
        .into_iter()
        .find(|r| r.id == id)
        .expect("run row")
}

fn standing(db: &SqliteStore, generation: &ScriptGeneration) -> ScriptExhaustiveStanding {
    db.script_exhaustive_standing(
        ScriptDomain::LaneRules,
        &generation.id,
        current_reference_sha256().expect("reference"),
    )
    .expect("standing")
}

/// live の計数（D8）: `script_guard_state` の行と `script_compare_log` の行。
fn live_counts(db: &SqliteStore, generation: &ScriptGeneration) -> (String, usize) {
    let counters = db
        .script_guard_counters(ScriptDomain::LaneRules, &generation.id)
        .expect("counters");
    let log = db.script_compare_log_after(0, 1000).expect("log").len();
    (format!("{counters:?}"), log)
}

/// 本物の bundle の isolate に全 19,683 通り × 2 回を流すと全件 `match` で合格し、証拠は再起動の後も残る。
/// 別の generation にすると、その generation の証拠は無い（`missing`）。live の計数は動かない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustive_real_bundle_all_inputs_match_and_persist() {
    let Some(bundle) = dist_bundle(
        "exhaustive_real_bundle_all_inputs_match_and_persist",
        "lane_rules",
    ) else {
        return;
    };
    let env = Env::new();
    let db = env.open();
    let deployed = deploy_bundle(&dist_dir(), &env.state()).expect("deploy");
    assert_eq!(
        deployed.manifest.bundle_sha256,
        bundle.manifest.bundle_sha256
    );
    let sha = deployed.manifest.bundle_sha256.clone();
    let g = env.daemon(&db).await;
    g.set_exhaustive_tuning(fast());
    let generation = register(&g, &sha).await;
    set_mode(&db, ScriptMode::Compare, &generation);
    let live_before = live_counts(&db, &generation);
    assert_eq!(
        standing(&db, &generation),
        ScriptExhaustiveStanding::Missing
    );

    let run = start(&g, &sha).await.expect("start");
    assert_eq!(run.state, ScriptExhaustiveState::Running);
    assert_eq!(run.daemon_instance_id, g.instance_id());
    assert_eq!(run.mode_at_start, "compare");
    g.wait_exhaustive().await;
    let done = run_row(&db, &run.id);
    assert_eq!(done.state, ScriptExhaustiveState::Passed, "{done:?}");
    assert_eq!(done.stop_code, None);
    assert_eq!(done.counts.calls, LANE_RULES_EXHAUSTIVE_CALLS);
    assert_eq!(done.counts.matches, LANE_RULES_EXHAUSTIVE_CALLS);
    assert_eq!(done.counts.uncovered_inputs, 0);
    assert!(done.counts.latency_max_us >= done.counts.latency_p95_us);
    assert!(done.first_problem_json.is_none());
    assert_eq!(standing(&db, &generation), ScriptExhaustiveStanding::Passed);
    // D8: live の計数は動かない。
    assert_eq!(live_counts(&db, &generation), live_before);
    assert_eq!(
        db.script_domain_mode("lane_rules").expect("mode").mode,
        ScriptMode::Compare
    );

    // 再起動: DB を開き直し、別の instance で復元しても証拠は残る。
    g.shutdown().await;
    drop(g);
    drop(db);
    let db = env.open();
    let g = env.daemon(&db).await;
    assert_eq!(run_row(&db, &run.id).state, ScriptExhaustiveState::Passed);
    assert_eq!(standing(&db, &generation), ScriptExhaustiveStanding::Passed);

    // generation を替える（同じ本文に注釈を足した別の hash）と、新しい generation の証拠は無い。
    let code = format!("{}\n// second generation\n", deployed.code);
    let sha2 = sha256_hex(code.as_bytes());
    let mut manifest = serde_json::to_value(&deployed.manifest).expect("manifest");
    manifest["bundle_sha256"] = json!(sha2);
    write_deployed(&env.state(), &sha2, &code, &manifest);
    let second = register(&g, &sha2).await;
    set_mode(&db, ScriptMode::Compare, &second);
    assert_eq!(standing(&db, &second), ScriptExhaustiveStanding::Missing);
    g.shutdown().await;
}

/// generation を替えると、前の generation の不合格も合格も新しい generation の立場に入らない（`missing`）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustive_new_generation_invalidates_evidence() {
    let env = Env::new();
    let db = env.open();
    let g = env.daemon(&db).await;
    g.set_exhaustive_tuning(fast());
    let a = deploy_raw(&env.state(), &DIFFERENT.replace("__TAG__", "A"));
    let ga = register(&g, &a).await;
    set_mode(&db, ScriptMode::Compare, &ga);
    let run = start(&g, &a).await.expect("start");
    g.wait_exhaustive().await;
    assert_eq!(run_row(&db, &run.id).state, ScriptExhaustiveState::Failed);
    assert_eq!(standing(&db, &ga), ScriptExhaustiveStanding::Failed);

    let b = deploy_raw(&env.state(), &DIFFERENT.replace("__TAG__", "B"));
    let gb = register(&g, &b).await;
    set_mode(&db, ScriptMode::Compare, &gb);
    assert_eq!(standing(&db, &gb), ScriptExhaustiveStanding::Missing);
    // 前の generation の bundle では始められない（mode の行の generation と違う）。
    let err = start(&g, &a).await.expect_err("old generation");
    assert!(
        matches!(
            err,
            ScriptGenerationError::Domain(
                task_core::script_compare::ScriptDomainError::ExhaustiveModeConflict(_)
            )
        ),
        "{err:?}"
    );
    g.shutdown().await;
}

/// 常に違う答えを返す bundle は 1 件目で不合格（`mismatch`）。`compare` の mode は変えず、live の計数も動かない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustive_always_different_bundle_fails() {
    let env = Env::new();
    let db = env.open();
    let g = env.daemon(&db).await;
    g.set_exhaustive_tuning(fast());
    let sha = deploy_raw(&env.state(), &DIFFERENT.replace("__TAG__", "X"));
    let generation = register(&g, &sha).await;
    set_mode(&db, ScriptMode::Compare, &generation);
    let run = start(&g, &sha).await.expect("start");
    g.wait_exhaustive().await;
    let done = run_row(&db, &run.id);
    assert_eq!(done.state, ScriptExhaustiveState::Failed);
    assert_eq!(done.stop_code, Some(ScriptExhaustiveStopCode::Mismatch));
    assert_eq!((done.counts.calls, done.counts.mismatches), (1, 1));
    let problem: serde_json::Value =
        serde_json::from_str(done.first_problem_json.as_deref().expect("problem")).expect("json");
    assert_eq!(problem["ts"]["condition"], "X");
    assert_eq!(problem["pass"], "forward");
    assert_eq!(standing(&db, &generation), ScriptExhaustiveStanding::Failed);
    assert_eq!(
        db.script_domain_mode("lane_rules").expect("mode").mode,
        ScriptMode::Compare
    );
    g.shutdown().await;
}

/// D8: 網羅は live の `script_guard_state`・`script_compare_log` を動かさない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustive_does_not_move_live_counters() {
    let env = Env::new();
    let db = env.open();
    let g = env.daemon(&db).await;
    g.set_exhaustive_tuning(fast());
    let sha = deploy_raw(&env.state(), &DIFFERENT.replace("__TAG__", "L"));
    let generation = register(&g, &sha).await;
    set_mode(&db, ScriptMode::Compare, &generation);
    let before = live_counts(&db, &generation);
    let run = start(&g, &sha).await.expect("start");
    g.wait_exhaustive().await;
    assert_eq!(run_row(&db, &run.id).counts.calls, 1);
    assert_eq!(live_counts(&db, &generation), before);
    assert_eq!(before.1, 0, "no compare log row");
    g.shutdown().await;
}

/// 呼び出しの前で止まるフック。`n == at` で `reached` を知らせ、`release` まで待つ。
fn park_at(at: u64, reached: Arc<Notify>, release: Arc<Notify>) -> ExhaustiveTuning {
    ExhaustiveTuning {
        before_call: Some(Arc::new(move |n| {
            let reached = Arc::clone(&reached);
            let release = Arc::clone(&release);
            Box::pin(async move {
                if n == at {
                    reached.notify_one();
                    release.notified().await;
                }
            })
        })),
        ..fast()
    }
}

/// 停止で runner を止めて `aborted`（`daemon_stopped`）を書く。止めずに消えた daemon の `running` の行は、次の起動の
/// 復元が `aborted`（`daemon_restarted`）にする。走っている間は 2 本目を始められない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustive_restart_marks_running_aborted() {
    let env = Env::new();
    let db = env.open();
    let sha = deploy_raw(&env.state(), &DIFFERENT.replace("__TAG__", "R"));

    // 1) 停止。
    let g = env.daemon(&db).await;
    let (reached, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    g.set_exhaustive_tuning(park_at(0, Arc::clone(&reached), Arc::clone(&release)));
    let generation = register(&g, &sha).await;
    set_mode(&db, ScriptMode::Compare, &generation);
    let run = start(&g, &sha).await.expect("start");
    reached.notified().await;
    let err = start(&g, &sha).await.expect_err("second run");
    assert!(
        matches!(
            err,
            ScriptGenerationError::Domain(
                task_core::script_compare::ScriptDomainError::ExhaustiveRunning(_)
            )
        ),
        "{err:?}"
    );
    g.shutdown().await;
    let stopped = run_row(&db, &run.id);
    assert_eq!(stopped.state, ScriptExhaustiveState::Aborted);
    assert_eq!(
        stopped.stop_code,
        Some(ScriptExhaustiveStopCode::DaemonStopped)
    );
    assert_eq!(stopped.counts.calls, 0);
    drop(g);

    // 2) 止めずに消えた daemon（runner はフックで止まったまま）→ 次の起動が `aborted` にする。
    let g1 = env.daemon(&db).await;
    let (reached, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    g1.set_exhaustive_tuning(park_at(0, Arc::clone(&reached), Arc::clone(&release)));
    let run = start(&g1, &sha).await.expect("start");
    reached.notified().await;
    let g2 = env.daemon(&db).await;
    let restarted = run_row(&db, &run.id);
    assert_eq!(restarted.state, ScriptExhaustiveState::Aborted);
    assert_eq!(
        restarted.stop_code,
        Some(ScriptExhaustiveStopCode::DaemonRestarted)
    );
    // 古い daemon の runner が後で終わっても、閉じた行を上書きしない（CAS）。
    release.notify_one();
    g1.wait_exhaustive().await;
    assert_eq!(run_row(&db, &run.id), restarted);
    g1.shutdown().await;
    g2.shutdown().await;
}

/// 網羅の実行中も dispatch の tick は止まらず、live の `lane_rules` の compare は `unavailable`・`busy` にならない。
/// runner は遅延フックで 1 件目の呼び出しの前に止め、その間に tick を回す（出来事待ち。時間で待たない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustive_tick_continues_while_running() {
    let env = CompareEnv::new("lane_rules");
    let db = Arc::new(SqliteStore::open(&env.config.db.path).expect("store"));
    let state = env.dir.path().join("state");
    let g = ScriptGenerations::new(
        GenerationRegistry::new(RuntimeLimits::default()),
        state.clone(),
        Arc::clone(&db),
    );
    g.restore().await.expect("restore");
    let (reached, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    g.set_exhaustive_tuning(park_at(0, Arc::clone(&reached), Arc::clone(&release)));
    let sha = deploy_raw(&state, &DIFFERENT.replace("__TAG__", "T"));
    let generation = register(&g, &sha).await;
    env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
    let mut d = env.dispatcher();
    d.set_policy_script_source(Some(g.source()));

    let run = start(&g, &sha).await.expect("start");
    reached.notified().await;
    for _ in 0..3 {
        let (_task, records) = env.dispatch(&mut d).await;
        assert_eq!(records.len(), 1, "the tick compared: {records:?}");
        let r = &records[0];
        assert_ne!(r.outcome, CompareOutcome::Unavailable, "{r:?}");
        assert!(
            !r.error
                .as_ref()
                .is_some_and(|e| matches!(e.kind.as_str(), "busy" | "generation_gone")),
            "{r:?}"
        );
        assert_eq!(r.generation, generation.id);
    }
    // runner は止まったまま（呼び出し 0 件）で、live の呼び出しが in-flight を残していない。
    assert_eq!(run_row(&db, &run.id).state, ScriptExhaustiveState::Running);
    assert_eq!(g.registry().get(&sha).expect("isolate").in_flight, 0);
    release.notify_one();
    g.wait_exhaustive().await;
    let done = run_row(&db, &run.id);
    assert_eq!(done.state, ScriptExhaustiveState::Failed);
    assert_eq!(done.stop_code, Some(ScriptExhaustiveStopCode::Mismatch));
    g.shutdown().await;
}

/// live の要求が in-flight の間は runner が呼ばない（live を先に通す）。live の要求は runner と重なっても
/// `busy`・`unavailable` にならずに終わる。live の in-flight は `cos_digest` の pass（試験が host op に答えるまで終わらない）
/// で作る（出来事待ち）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustive_waits_for_live_in_flight() {
    use task_dispatch::script_policy::{CosDigestOpReply, CosDigestPassCall, PolicyScriptSource};
    let env = Env::new();
    let db = env.open();
    let g = env.daemon(&db).await;
    let sha = deploy_raw(&env.state(), &DIFFERENT.replace("__TAG__", "V"));
    // runner の各呼び出しの直前に、live の in-flight を記録する。
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tuning = {
        let registry = g.registry().clone();
        let sha = sha.clone();
        let seen = Arc::clone(&seen);
        ExhaustiveTuning {
            before_call: Some(Arc::new(move |_n| {
                let in_flight = registry.get(&sha).map_or(usize::MAX, |i| i.in_flight);
                seen.lock().expect("seen").push(in_flight);
                Box::pin(async {})
            })),
            ..fast()
        }
    };
    g.set_exhaustive_tuning(tuning);
    let generation = register(&g, &sha).await;
    set_mode(&db, ScriptMode::Compare, &generation);
    let policy = PolicyScriptSource::pinned(g.as_ref(), &generation).expect("pinned");

    // live の pass を始め、op を受け取った（in-flight 1）ところで網羅を始める。
    let (sink, mut rx) = tokio::sync::mpsc::channel(4);
    let pass = {
        let policy = Arc::clone(&policy);
        tokio::spawn(async move {
            policy
                .cos_digest_pass(
                    CosDigestPassCall {
                        pass_id: "pass-1".into(),
                        sink,
                    },
                    LONG_DEADLINE,
                )
                .await
        })
    };
    let op = rx.recv().await.expect("the pass asked for an op");
    assert_eq!(g.registry().get(&sha).expect("isolate").in_flight, 1);
    let run = start(&g, &sha).await.expect("start");
    // runner が呼べない間に live の lane_rules を足しても busy・unavailable にならない。
    let input = task_core::script_compare::LaneRulesInput::redacted(
        &task_core::model_policy::all_features()[0],
    );
    let (live, _) =
        task_dispatch::script_policy::call_lane_rules(policy.as_ref(), input, LONG_DEADLINE).await;
    assert!(
        matches!(live, task_dispatch::script_policy::TsResult::Reply(_)),
        "{live:?}"
    );
    let _ = op.reply.send(CosDigestOpReply::Ok(json!({ "runs": [] })));
    let reply = pass.await.expect("pass task");
    assert!(reply.is_ok(), "{reply:?}");
    g.wait_exhaustive().await;
    assert_eq!(run_row(&db, &run.id).counts.calls, 1);
    // runner が呼んだ時点では、live の in-flight は 0 だった。
    assert_eq!(*seen.lock().expect("seen"), vec![0]);
    g.shutdown().await;
}
