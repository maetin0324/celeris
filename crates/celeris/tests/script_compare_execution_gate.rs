//! ADR 2026-11-04-ts-execution-gate-domain: 本物の deno_core host で `execution_gate` の compare を確かめる。
//! userns を使わない。外部ネットワークに出ない。bundle の扱いと待ちの方針は `support/script_compare.rs`。
//!
//! `CELERIS_P2F_LATENCY_OUT=<file>` を渡すと (1) が 1 呼び出しの時間の raw（JSONL）を書く（計測専用）。

mod support;

use std::sync::Arc;
use std::time::Duration;

use support::script_compare::{
    Env, LONG_DEADLINE, dist_bundle, repo_root, start_code, start_policy,
};
use task_core::execution_gate::{ExecutionGateDecision, ExecutionGateInput, score};
use task_core::script_compare::{CompareOutcome, ScriptMode};
use task_core::{Event, Task};
use task_dispatch::script_policy::{PolicyScript, TsResult, classify_execution_gate};

const DOMAIN: &str = "execution_gate";
const CALL_SITE: &str = "work_units.execution_gate_if_needed";
/// `cases.jsonl` の行数（手書き・5 軸の全組み合わせ・生成 2000 件）。
const FIXTURE_LINES: usize = 2_337;
const THROWING_BUNDLE: &str = "globalThis.__celerisDispatch = (_d, input) => { throw new Error(\"boom \" + input.title); };\n";
const DIFFERENT_BUNDLE: &str = "globalThis.__celerisDispatch = () => ({ mode: \"compound\", score: 9, \
     rule_id: \"compound/score\", signals: [{ name: \"X1\", weight: 9, detail: \"ts only\" }] });\n";
/// Rust だけが書く rule_id を名乗る。
const RESERVED_BUNDLE: &str = "globalThis.__celerisDispatch = () => ({ mode: \"compound\", score: 0, \
     rule_id: \"human/explicit\", signals: [] });\n";

fn fixture_inputs() -> Vec<ExecutionGateInput> {
    let path = repo_root().join("control-ts/fixtures/execution_gate/cases.jsonl");
    std::fs::read_to_string(path)
        .expect("read fixture")
        .lines()
        .map(|line| {
            let row: serde_json::Value = serde_json::from_str(line).expect("row");
            serde_json::from_value(row["input"].clone()).expect("input")
        })
        .collect()
}

fn gated(env: &Env, task: &Task) -> ExecutionGateDecision {
    env.events(task)
        .into_iter()
        .find_map(|e| match e {
            Event::ExecutionGated { decision } => Some(*decision),
            _ => None,
        })
        .expect("execution_gated")
}

/// (1) fixture の全件が本物の host で Rust の `score` と byte 一致する（正順と逆順）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_gate_real_host_matches_rust_on_all_fixtures() {
    let Some(bundle) = dist_bundle(
        "script_compare_gate_real_host_matches_rust_on_all_fixtures",
        DOMAIN,
    ) else {
        return;
    };
    let inputs = fixture_inputs();
    assert_eq!(inputs.len(), FIXTURE_LINES);
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256);
    let mut latency = Vec::with_capacity(inputs.len() * 2);
    for input in inputs.iter().chain(inputs.iter().rev()) {
        let rust = score(input);
        let started = std::time::Instant::now();
        let reply = policy.execution_gate(input.clone(), LONG_DEADLINE).await;
        latency.push(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX));
        let result = TsResult::Reply(reply.expect("reply"));
        let classified = classify_execution_gate(&rust, input, &result);
        assert_eq!(classified.outcome, CompareOutcome::Match, "{classified:?}");
    }
    assert_eq!(latency.len(), FIXTURE_LINES * 2);
    if let Some(out) = std::env::var_os("CELERIS_P2F_LATENCY_OUT") {
        let body: String = latency
            .iter()
            .enumerate()
            .map(|(i, us)| format!("{{\"call\":{i},\"elapsed_us\":{us}}}\n"))
            .collect();
        std::fs::write(out, body).expect("write latency");
    }
    handle.shutdown();
}

/// (2) daemon の dispatch: 本物の bundle で match を記録し、判定は `rust` のときと byte 一致、記録に本文は無い。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_gate_real_bundle_records_match_without_text() {
    let Some(bundle) = dist_bundle(
        "script_compare_gate_real_bundle_records_match_without_text",
        DOMAIN,
    ) else {
        return;
    };
    let env = Env::new(DOMAIN);
    let base = env.baseline().await;
    let base = serde_json::to_string(&gated(&env, &base)).unwrap();
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256.clone());
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (task, record) = env.dispatch_until_compared(&mut d).await;
    assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
    assert_eq!(record.call_site, CALL_SITE);
    assert_eq!(serde_json::to_string(&gated(&env, &task)).unwrap(), base);
    let json = serde_json::to_string(&record).unwrap();
    assert!(!json.contains(&task.objective), "{json}");
    assert!(!json.contains(&task.title), "{json}");
    handle.shutdown();
}

/// (3) 例外を投げる bundle: 判定は Rust のまま、例外文（本文を写している）は記録に残らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_gate_throwing_bundle_keeps_rust_and_withholds_text() {
    let env = Env::new(DOMAIN);
    let base = env.baseline().await;
    let base = serde_json::to_string(&gated(&env, &base)).unwrap();
    let (handle, policy, sha256) = start_code(THROWING_BUNDLE);
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (task, record) = env.dispatch_until_compared(&mut d).await;
    assert_eq!(record.outcome, CompareOutcome::TsError, "{record:?}");
    assert_eq!(serde_json::to_string(&gated(&env, &task)).unwrap(), base);
    let json = serde_json::to_string(&record).unwrap();
    assert!(!json.contains(&task.title), "{json}");
    handle.shutdown();
}

/// (4) 違う答えの bundle: mismatch を記録し、判定は Rust のまま。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_gate_different_answer_keeps_rust_decision_bytes() {
    let env = Env::new(DOMAIN);
    let base = env.baseline().await;
    let base = serde_json::to_string(&gated(&env, &base)).unwrap();
    let (handle, policy, sha256) = start_code(DIFFERENT_BUNDLE);
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (task, record) = env.dispatch_until_compared(&mut d).await;
    assert_eq!(record.outcome, CompareOutcome::Mismatch, "{record:?}");
    assert_eq!(serde_json::to_string(&gated(&env, &task)).unwrap(), base);
    handle.shutdown();
}

/// (5) Rust だけが書く rule_id を名乗る bundle: 安全違反で、判定は Rust のまま。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_gate_reserved_rule_is_a_safety_violation() {
    let env = Env::new(DOMAIN);
    let base = env.baseline().await;
    let base = serde_json::to_string(&gated(&env, &base)).unwrap();
    let (handle, policy, sha256) = start_code(RESERVED_BUNDLE);
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (task, record) = env.dispatch_until_compared(&mut d).await;
    assert_eq!(record.outcome, CompareOutcome::InvalidOutput, "{record:?}");
    assert_eq!(record.safety_violations, ["gate_rule_not_allowed"]);
    assert_eq!(serde_json::to_string(&gated(&env, &task)).unwrap(), base);
    handle.shutdown();
}

/// 本番の deadline（50 ms）の中で本文の大きい入力（上限ちょうどの 256 KiB）も評価できる。deadline は長くして時間を確かめず、
/// 答えが Rust と一致することだけを見る（時間の raw は (1) の計測で読む）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_gate_largest_input_still_matches() {
    let Some(bundle) = dist_bundle("script_compare_gate_largest_input_still_matches", DOMAIN)
    else {
        return;
    };
    let mut input = fixture_inputs().remove(0);
    input.objective = "調査 design 🚀".repeat(256 * 1024 / 18);
    let rust = score(&input);
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256);
    let reply = policy
        .execution_gate(input.clone(), Duration::from_secs(30))
        .await
        .expect("reply");
    let classified = classify_execution_gate(&rust, &input, &TsResult::Reply(reply));
    assert_eq!(classified.outcome, CompareOutcome::Match, "{classified:?}");
    handle.shutdown();
}
