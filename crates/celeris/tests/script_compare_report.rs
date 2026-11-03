//! 境界 ADR 付記 P1-E: 集計 script（`scripts/dev/script-compare.py report`）を、本物の store が書いた一時 DB で確かめる。
//!
//! 記録は `script_compare_append_observed`・`cos_digest_shadow_finish`（本番の dispatcher と同じ書き込み）か、本物の
//! dispatcher の dispatch で作る。script は `sqlite3 -readonly` で読むだけで、DB の main file を変えないことも確かめる。
//! 本番 DB には触れない。userns・外部ネットワークを使わない。

mod support;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::Value;
use support::script_compare::{Env, dist_bundle, repo_root, start_code, start_policy};
use task_core::TaskId;
use task_core::chat::digest_apply::{CosDigestApplyPass, CosDigestApplyRun, CosDigestRecordPlan};
use task_core::script_compare::{
    CompareError, CompareOutcome, CosDigestApplied, CosDigestAppliedBy, CosDigestCommandsSummary,
    CosDigestCompareRecord, ScriptCompareRecord, ScriptDomain, ScriptMode, ScriptModeChange,
};
use task_core::script_guard::{ScriptGuardCall, ScriptGuardObservation, readiness};
use task_dispatch::script_policy::PolicyScript;
use time::OffsetDateTime;

const DIFFERENT_BUNDLE: &str = "globalThis.__celerisDispatch = () => ({ rule_id: \"always/other\", \
     proposed: \"frontier\", condition: \"always a different answer\" });\n";

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// script を走らせ、exit と標準出力を返す。
fn run_script(db: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new("python3")
        .arg("-I")
        .arg(repo_root().join("scripts/dev/script-compare.py"))
        .arg("report")
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("python3");
    assert!(
        out.status.code().is_some_and(|c| c != 2),
        "script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        out.status.code().expect("exit"),
        String::from_utf8(out.stdout).expect("utf-8"),
    )
}

fn report(env: &Env, args: &[&str]) -> (i32, Value) {
    let (code, stdout) = run_script(&env.config.db.path, args);
    (code, serde_json::from_str(&stdout).expect("report json"))
}

fn domain<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["domains"]
        .as_array()
        .expect("domains")
        .iter()
        .find(|d| d["domain"] == name)
        .unwrap_or_else(|| panic!("{name} in {report}"))
}

fn lane_record(generation: &str, n: u64, outcome: CompareOutcome) -> ScriptCompareRecord {
    let rust = serde_json::json!({"rule_id": "standard/default", "proposed": "standard", "condition": "c"});
    let ts = match outcome {
        CompareOutcome::Match => Some(rust.clone()),
        CompareOutcome::Mismatch => Some(serde_json::json!({
            "rule_id": "other", "proposed": "frontier", "condition": "c"
        })),
        _ => None,
    };
    ScriptCompareRecord {
        domain: ScriptDomain::LaneRules,
        generation: generation.into(),
        isolate_epoch: 1,
        bundle_sha256: "a".repeat(64),
        host_api: "1.0.0".into(),
        call_site: "dispatch_run.decide_lane".into(),
        run_id: None,
        input_digest: format!("sha256:{n}"),
        input: None,
        input_truncated: false,
        rust,
        ts,
        outcome,
        error: None,
        normalized: vec![],
        safety_violations: vec![],
        elapsed_us: n + 1,
        applied_by: None,
        fallback: None,
        canary_selected: false,
        canary_bps: None,
    }
}

fn append(env: &Env, task: TaskId, record: &ScriptCompareRecord) {
    let (appended, _) = env
        .store
        .script_compare_append_observed(task, record, false, now())
        .expect("append");
    assert!(appended, "{record:?}");
}

fn compare_env(domain_name: &'static str) -> (Env, String) {
    let env = Env::new(domain_name);
    let generation = env.generation(&"a".repeat(64));
    env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
    (env, generation.id)
}

/// 純粋判定の gate: 999 件では ready でなく、`unavailable` は分母に入れず、1000 件で ready。latency は記録の値。
#[test]
fn script_compare_report_pure_gate_counts_real_records() {
    let (env, generation) = compare_env("lane_rules");
    let task = env.insert_task().id;
    for n in 0..999 {
        append(
            &env,
            task,
            &lane_record(&generation, n, CompareOutcome::Match),
        );
    }
    let (code, r) = report(&env, &["--require-ready", "lane_rules"]);
    let d = domain(&r, "lane_rules");
    assert_eq!(code, 4, "{d}");
    assert_eq!(d["calls"], 999);
    assert_eq!(d["ready"], false);
    assert!(d["ready_blockers"][0].as_str().unwrap().contains("999"));

    // `unavailable` は分母に入らず、streak を進めない。
    append(
        &env,
        task,
        &lane_record(&generation, 5000, CompareOutcome::Unavailable),
    );
    let (code, r) = report(&env, &["--require-ready", "lane_rules"]);
    assert_eq!(code, 4);
    let d = domain(&r, "lane_rules");
    assert_eq!(d["calls"], 999);
    assert_eq!(d["outcomes"]["unavailable"], 1);

    append(
        &env,
        task,
        &lane_record(&generation, 999, CompareOutcome::Match),
    );
    let db = &env.config.db.path;
    let before = std::fs::read(db).expect("db bytes");
    let (code, first) = run_script(db, &["--require-ready", "lane_rules"]);
    let (_, second) = run_script(db, &["--require-ready", "lane_rules"]);
    assert_eq!(code, 0, "{first}");
    assert_eq!(first, second, "same DB, same bytes");
    assert_eq!(
        std::fs::read(db).expect("db bytes"),
        before,
        "report must not write the DB"
    );
    let r: Value = serde_json::from_str(&first).expect("json");
    assert_eq!(r["problems"], serde_json::json!([]));
    let d = domain(&r, "lane_rules");
    assert_eq!(d["ready"], true, "{d}");
    assert_eq!(d["match_rate"], 1.0);
    assert_eq!(d["outcomes"]["match"], 1000);
    assert_eq!(d["current_mode"], "compare");
    assert_eq!(
        d["latency"],
        serde_json::json!({"n": 1000, "p50_us": 500, "p95_us": 950, "max_us": 1000})
    );

    // API の判定（`readiness`）と同じ答えになる。
    let (_, counters) = env
        .store
        .script_guard_counters(ScriptDomain::LaneRules, &generation)
        .expect("counters")
        .expect("row");
    let api = readiness(ScriptDomain::LaneRules, ScriptMode::Compare, &counters);
    assert!(api.ready);
    assert_eq!(d["clean_streak"], api.clean_streak);

    // `bounds` の id で切ると同じ記録の集計が出るが、計数と時点が違うので ready にしない。
    let upto = r["bounds"]["upto_event_id"].as_u64().expect("bound");
    append(
        &env,
        task,
        &lane_record(&generation, 1000, CompareOutcome::Match),
    );
    let upto = upto.to_string();
    let (code, bounded) = report(&env, &["--upto-event-id", &upto]);
    assert_eq!(code, 0);
    let b = domain(&bounded, "lane_rules");
    assert_eq!(b["outcomes"]["match"], 1000);
    assert_eq!(b["ready"], false);
    assert_eq!(b["latency"], d["latency"]);
}

/// 不一致・timeout・失敗の分類（S2-8）。不一致で streak が 0 に戻り、API と同じく ready でなくなる。
#[test]
fn script_compare_report_classifies_every_outcome() {
    let (env, generation) = compare_env("lane_rules");
    let task = env.insert_task().id;
    let mut timeout = lane_record(&generation, 1, CompareOutcome::TsError);
    timeout.error = Some(CompareError::new("timeout", "deadline 50ms"));
    let mut exception = lane_record(&generation, 2, CompareOutcome::TsError);
    exception.error = Some(CompareError::new("exception", "boom"));
    let invalid = lane_record(&generation, 3, CompareOutcome::InvalidOutput);
    let ok = lane_record(&generation, 4, CompareOutcome::Match);
    let mismatch = lane_record(&generation, 5, CompareOutcome::Mismatch);
    let after = lane_record(&generation, 6, CompareOutcome::Match);
    for record in [&timeout, &exception, &invalid, &ok, &mismatch, &after] {
        append(&env, task, record);
    }
    let (code, r) = report(&env, &["--require-ready", "lane_rules"]);
    assert_eq!(code, 4);
    assert_eq!(r["problems"], serde_json::json!([]));
    let d = domain(&r, "lane_rules");
    assert_eq!(
        d["outcomes"],
        serde_json::json!({
            "match": 2, "mismatch": 1, "ts_error": 2, "invalid_output": 1, "unavailable": 0
        })
    );
    assert_eq!(d["timeouts"], 1);
    assert_eq!(d["failures"], 2);
    assert_eq!(d["clean_streak"], 1);
    assert_eq!(d["match_rate"], 0.333333);
    assert_eq!(d["guard_state"]["calls"], 6);
    assert_eq!(d["guard_state"]["clean_streak"], 1);
}

/// 件数を計数だけで補った DB（記録の無い `script_guard_observe`）を ready にせず、exit 1 にする。
#[test]
fn script_compare_report_refuses_counts_without_records() {
    let (env, generation) = compare_env("lane_rules");
    let observe = || {
        env.store
            .script_guard_observe(
                ScriptDomain::LaneRules,
                &generation,
                ScriptGuardObservation {
                    call: ScriptGuardCall::Ok,
                    safety_violation: false,
                    ts_side: false,
                },
                now(),
            )
            .expect("observe");
    };
    for _ in 0..1000 {
        observe();
    }
    let (code, r) = report(&env, &["--require-ready", "lane_rules"]);
    assert_eq!(code, 1, "{r}");
    assert!(
        r["problems"][0].as_str().unwrap().contains("no records"),
        "{r}"
    );

    // 本物の記録が 1 件あっても、計数と合わなければ ready にしない。
    let task = env.insert_task().id;
    append(
        &env,
        task,
        &lane_record(&generation, 1, CompareOutcome::Match),
    );
    let (code, r) = report(&env, &[]);
    assert_eq!(code, 1, "{r}");
    let d = domain(&r, "lane_rules");
    assert_eq!(d["ready"], false);
    assert!(r["problems"][0].as_str().unwrap().contains("disagrees"));
}

/// 戻しの集計: canary で不一致 → `system:script-guard` が rust へ戻す。安全違反 → generation が `invalid` になる。
#[test]
fn script_compare_report_counts_rollbacks() {
    let (env, generation) = compare_env("lane_rules");
    let task = env.insert_task().id;
    for n in 0..1000 {
        append(
            &env,
            task,
            &lane_record(&generation, n, CompareOutcome::Match),
        );
    }
    let version = env
        .store
        .script_domain_mode("lane_rules")
        .expect("mode")
        .version;
    env.store
        .script_domain_set_canary(
            &ScriptModeChange {
                domain: "lane_rules".into(),
                to_mode: ScriptMode::Canary,
                to_generation: Some(generation.clone()),
                expected_version: version,
                actor: "human:test".into(),
                reason: "rehearsal".into(),
            },
            100,
            now(),
        )
        .expect("canary");
    append(
        &env,
        task,
        &lane_record(&generation, 2000, CompareOutcome::Mismatch),
    );
    assert_eq!(env.mode(), ScriptMode::Rust);
    let (code, r) = report(&env, &[]);
    // guard は mode の変わり目で数え直す（記録より少ない）。これは破れでない。
    assert_eq!(code, 0, "{r}");
    let rollback = &r["rollback"];
    assert_eq!(rollback["to_rust_by_actor"]["system:script-guard"], 1);
    let auto = &rollback["automatic_rollbacks"][0];
    assert!(
        auto["reason"]
            .as_str()
            .unwrap()
            .starts_with("guard_mismatch"),
        "{auto}"
    );
    let d = domain(&r, "lane_rules");
    assert_eq!(d["current_mode"], Value::Null);
    assert_eq!(d["ready"], false);
    assert_eq!(d["outcomes"]["mismatch"], 1);

    let (env, generation) = compare_env("lane_rules");
    let task = env.insert_task().id;
    let mut unsafe_record = lane_record(&generation, 1, CompareOutcome::Mismatch);
    unsafe_record.safety_violations = vec!["ceiling_exceeded".into()];
    append(&env, task, &unsafe_record);
    let (code, r) = report(&env, &[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(domain(&r, "lane_rules")["safety_violations"], 1);
    let events = r["rollback"]["generation_events"]
        .as_array()
        .expect("events");
    assert_eq!(events[0]["type"], "script_generation_invalidated", "{r}");
}

fn digest_record(generation: &str, run: &str, outcome: CompareOutcome) -> CosDigestCompareRecord {
    let summary = CosDigestCommandsSummary {
        commands_digest: "sha256:cc".into(),
        command_kinds: vec!["post_digest".into()],
    };
    CosDigestCompareRecord {
        domain: ScriptDomain::CosDigest,
        generation: generation.into(),
        isolate_epoch: 1,
        bundle_sha256: "a".repeat(64),
        host_api: "1.0.0".into(),
        call_site: "cos_digest.pass".into(),
        mode: ScriptMode::Compare,
        pass_id: format!("pass-{run}"),
        applied_by: CosDigestAppliedBy::Rust,
        fallback: None,
        ops: Vec::new(),
        thread_id: "t1".into(),
        run_id: run.into(),
        input_digest: format!("sha256:{run}"),
        input: None,
        rust: summary.clone(),
        ts: (outcome == CompareOutcome::Match).then_some(summary),
        ts_commands_head: None,
        outcome,
        error: None,
        normalized: Vec::new(),
        safety_violations: Vec::new(),
        elapsed_us: 7,
        applied: CosDigestApplied {
            message_id: String::new(),
            requeued: 0,
        },
        canary_selected: false,
        canary_bps: None,
    }
}

/// compare の本番の書き込み（apply で outbox、影の終わりで記録）を通す。
fn shadow_apply(env: &Env, generation: &str, run: &str) {
    let lost = digest_record(generation, run, CompareOutcome::Unavailable);
    env.store
        .cos_digest_apply_pass(
            &CosDigestApplyPass {
                pass_id: format!("pass-{run}"),
                verify: false,
                expect_mode: None,
                runs: vec![CosDigestApplyRun {
                    thread_id: "t1".into(),
                    run_id: run.into(),
                    commands: Vec::new(),
                    record: Some(CosDigestRecordPlan::Shadow {
                        lost: Box::new(lost),
                    }),
                }],
            },
            now(),
        )
        .expect("apply");
}

/// CoS の gate: 199 件では ready でなく、200 件で ready。閉じていない outbox の行は件数に入れず別に出す。
#[test]
fn script_compare_report_cos_gate_uses_the_log_table() {
    let (env, generation) = compare_env("cos_digest");
    for n in 0..200 {
        let run = format!("r{n}");
        shadow_apply(&env, &generation, &run);
        if n == 199 {
            let (code, r) = report(&env, &["--require-ready", "cos_digest"]);
            assert_eq!(code, 4, "{r}");
            assert_eq!(r["shadow_outbox_pending"], 1);
            assert_eq!(domain(&r, "cos_digest")["calls"], 199);
        }
        let record = digest_record(&generation, &run, CompareOutcome::Match);
        env.store
            .cos_digest_shadow_finish(&format!("pass-{run}"), &run, Some(&record), now())
            .expect("finish")
            .expect("row");
    }
    let (code, r) = report(&env, &["--require-ready", "cos_digest"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["shadow_outbox_pending"], 0);
    let d = domain(&r, "cos_digest");
    assert_eq!(d["required"], 200);
    assert_eq!(d["ready"], true, "{d}");
    assert_eq!(d["guard_state"]["calls"], 200);
}

/// 本物の dispatcher が書いた event の形を読む（常に違う答えの bundle → `mismatch`、実 bundle → `match`）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_report_reads_dispatched_records() {
    let env = Env::new("lane_rules");
    let (handle, policy, sha256) = start_code(DIFFERENT_BUNDLE);
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (_, record) = env.dispatch_until_compared(&mut d).await;
    assert_eq!(record.outcome, CompareOutcome::Mismatch, "{record:?}");
    handle.shutdown();
    let (code, r) = report(&env, &[]);
    assert_eq!(code, 0, "{r}");
    let row = domain(&r, "lane_rules");
    assert_eq!(row["generation"].as_str(), Some(generation.id.as_str()));
    assert_eq!(row["bundle_sha256"].as_str(), Some(sha256.as_str()));
    assert_eq!(row["outcomes"]["mismatch"], 1);
    assert_eq!(row["guard_state"]["calls"], row["calls"]);
    assert!(row["latency"]["max_us"].as_u64().unwrap() > 0, "{row}");

    let Some(bundle) = dist_bundle(
        "script_compare_report_reads_dispatched_records",
        "lane_rules",
    ) else {
        return;
    };
    let env = Env::new("lane_rules");
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let generation = env.generation(&sha256);
    let (handle, policy) = start_policy(bundle.source(), sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (_, record) = env.dispatch_until_compared(&mut d).await;
    assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
    handle.shutdown();
    let (code, r) = report(&env, &[]);
    assert_eq!(code, 0, "{r}");
    let row = domain(&r, "lane_rules");
    assert_eq!(row["outcomes"]["match"], 1, "{row}");
    assert_eq!(row["match_rate"], 1.0);
}

/// 表が無い DB（migration の前の本番と同じ状態）は exit 3 で、表を作らない。
#[test]
fn script_compare_report_before_migration_exits_3() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("old.sqlite3");
    let status = Command::new("sqlite3")
        .arg(&db)
        .arg("CREATE TABLE events (id INTEGER PRIMARY KEY, json TEXT);")
        .status()
        .expect("sqlite3");
    assert!(status.success());
    let before = std::fs::read(&db).expect("db bytes");
    let (code, stdout) = run_script(&db, &[]);
    assert_eq!(code, 3);
    assert!(stdout.contains("tables_missing"), "{stdout}");
    assert_eq!(std::fs::read(&db).expect("db bytes"), before);
}
