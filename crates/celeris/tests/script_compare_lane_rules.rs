//! 付記 S2: 本物の deno_core host で `lane_rules` の compare を確かめる。userns を使わない。外部ネットワークに出ない。
//!
//! `control-ts/dist` の bundle を使う。無ければ SKIP を出して抜ける。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗にする。
//! 例外を投げる bundle と常に違う答えを返す bundle は、この file の中の本文を同じ host で評価する（dist が無くても走る）。
//!
//! 待ちの方針（`timeout` のときだけ新しい task でやり直す）と共通の helper は `support/script_compare.rs`。

mod support;

use std::sync::Arc;

use celeris::{Config, Exit, RunOptions};
use celeris_script::manifest::deploy_bundle;
use support::script_compare::{
    Env, LONG_DEADLINE, TIMEOUT_RETRIES, dist_bundle, dist_dir, is_timeout, load_config, repo_root,
    start_code, start_policy,
};
use task_core::model_policy::{TaskFeatures, match_rule};
use task_core::script_compare::{
    CompareOutcome, LaneRulesInput, ScriptCompareRecord, ScriptDomain, ScriptGenerationState,
    ScriptMode, recorded_input,
};
use task_core::{DaemonMode, LaneDecision, RoutingRecord, Task, Tier};
use task_dispatch::Dispatcher;
use task_dispatch::script_policy::{
    LaneRulesOutput, PolicyScript, TsResult, call_lane_rules, classify_lane_rules,
};

const DOMAIN: &str = "lane_rules";
const FIXTURE_LINES: usize = 19_683;
const THROWING_BUNDLE: &str =
    "globalThis.__celerisDispatch = () => { throw new Error(\"boom in lane_rules\"); };\n";
const DIFFERENT_BUNDLE: &str = "globalThis.__celerisDispatch = () => ({ rule_id: \"always/other\", \
     proposed: \"frontier\", condition: \"always a different answer\" });\n";

/// 1 件を dispatch し、task・`routing_decided`・compare の記録を返す。
async fn dispatch(
    env: &Env,
    d: &mut Dispatcher,
) -> (Task, RoutingRecord, Vec<ScriptCompareRecord>) {
    let (task, records) = env.dispatch(d).await;
    let routing = env.routing(&task);
    (task, routing, records)
}

/// `timeout` 以外の結果が出るまで dispatch する（support の待ちの方針）。
async fn dispatch_until_compared(
    env: &Env,
    d: &mut Dispatcher,
) -> (Task, RoutingRecord, ScriptCompareRecord) {
    let (task, record) = env.dispatch_until_compared(d).await;
    let routing = env.routing(&task);
    (task, routing, record)
}

/// mode rust・判定の口なしの判定（比べる基準）。
async fn baseline(env: &Env) -> RoutingRecord {
    let task = env.baseline().await;
    env.routing(&task)
}

fn decision(routing: &RoutingRecord) -> LaneDecision {
    routing.decision.clone()
}

/// `routing_decided` の記録の byte 列（S2-6）。比較の有無で変わってはいけない。
/// dispatch ごとに新しく作られる id（run・decision・task）だけを除く。
fn decision_bytes(routing: &RoutingRecord) -> String {
    let mut value = serde_json::to_value(routing).expect("routing json");
    let route = value["features"]["decision"]
        .as_object_mut()
        .expect("route decision");
    for id in ["decision_id", "run_id", "subject_id"] {
        assert!(route.remove(id).is_some(), "{id}");
    }
    serde_json::to_string(&value).expect("routing json")
}

fn fixture_features() -> Vec<TaskFeatures> {
    let path = repo_root().join("control-ts/fixtures/lane_rules/all-features.jsonl");
    let text = std::fs::read_to_string(&path).expect("fixture");
    text.lines()
        .map(|line| {
            let row: serde_json::Value = serde_json::from_str(line).expect("fixture line");
            let features: TaskFeatures =
                serde_json::from_value(row["features"].clone()).expect("features");
            // fixture の期待値と Rust の参照実装が食い違っていれば、比べる前に落とす。
            let rust = match_rule(&features);
            assert_eq!(row["rule_id"], rust.rule_id, "{line}");
            assert_eq!(row["condition"], rust.condition, "{line}");
            assert_eq!(
                row["proposed"],
                serde_json::to_value(rust.lane).expect("tier"),
                "{line}"
            );
            features
        })
        .collect()
}

/// (1) fixture の全件が本物の host で Rust の `match_rule` と一致する。
/// 同じ isolate に正順と逆順で流し、global に残した値で答えが変わらないことも見る（付記 S2-7）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_real_host_matches_rust_on_all_fixtures() {
    let Some(bundle) = dist_bundle(
        "script_compare_real_host_matches_rust_on_all_fixtures",
        DOMAIN,
    ) else {
        return;
    };
    assert!(bundle.manifest.domains.iter().any(|d| d == "lane_rules"));
    let features = fixture_features();
    assert_eq!(features.len(), FIXTURE_LINES);
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256);
    let ceiling = task_core::model_policy::LaneCeiling::default();
    let forward = features.iter();
    let backward = features.iter().rev();
    let mut compared = 0;
    for f in forward.chain(backward) {
        let rust = match_rule(f);
        let rust_lane = rust.lane;
        let rust = LaneRulesOutput::from(rust);
        let (result, _) =
            call_lane_rules(policy.as_ref(), LaneRulesInput::redacted(f), LONG_DEADLINE).await;
        assert!(matches!(result, TsResult::Reply(_)), "{f:?}: {result:?}");
        let classified = classify_lane_rules(&rust, rust_lane, &ceiling, &result);
        assert_eq!(
            classified.outcome,
            CompareOutcome::Match,
            "{f:?}: {classified:?}"
        );
        compared += 1;
    }
    assert_eq!(compared, FIXTURE_LINES * 2);
    assert_eq!(handle.stats().restarts, 0, "no isolate restart");
    handle.shutdown();
}

/// (2) mode compare の dispatch で `match` が残り、lane は Rust の結果。
/// (3) mode を rust に戻すと TS を呼ばない。
/// (5) 記録の入力を TS に通し直すと同じ出力が出る（replay）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_dispatch_match_rollback_and_replay() {
    let Some(bundle) = dist_bundle("script_compare_dispatch_match_rollback_and_replay", DOMAIN)
    else {
        return;
    };
    let env = Env::new(DOMAIN);
    let base = baseline(&env).await;
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let generation = env.generation(&sha256);
    let (handle, policy) = start_policy(bundle.source(), sha256.clone());
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy.clone() as Arc<dyn PolicyScript>));

    // mode rust の間は bundle が在っても呼ばない。
    let (_, routing, records) = dispatch(&env, &mut d).await;
    assert!(records.is_empty(), "{records:?}");
    assert_eq!(decision_bytes(&routing), decision_bytes(&base));
    assert_eq!(handle.stats().completed, 0);

    // (2)
    env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
    let (_, routing, record) = dispatch_until_compared(&env, &mut d).await;
    assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
    assert_eq!(record.domain, ScriptDomain::LaneRules);
    assert_eq!(record.generation, generation.id);
    assert_eq!(record.bundle_sha256, sha256);
    assert_eq!(record.isolate_epoch, 1);
    assert_eq!(record.run_id, None);
    assert_eq!(record.ts.as_ref(), Some(&record.rust));
    assert_eq!(decision_bytes(&routing), decision_bytes(&base));
    let lane = decision(&routing);
    assert_eq!(
        record.rust["proposed"],
        serde_json::to_value(lane.proposed).unwrap()
    );
    assert_eq!(record.rust["rule_id"], lane.rule_id);

    // (5) 記録した入力は 9 軸だけで、digest が合い、TS に通し直すと同じ出力になる。
    let raw_input = record.input.clone().expect("recorded input");
    let input: LaneRulesInput = serde_json::from_value(raw_input.clone()).expect("input DTO");
    assert_eq!(serde_json::to_value(&input).unwrap(), raw_input);
    assert_eq!(input.features, lane.features);
    assert_eq!(
        recorded_input(&input).expect("digest").input_digest,
        record.input_digest
    );
    let (replayed, _) = call_lane_rules(policy.as_ref(), input, LONG_DEADLINE).await;
    let TsResult::Reply(reply) = replayed else {
        panic!("replay failed: {replayed:?}");
    };
    assert_eq!(
        serde_json::to_string(&reply.output).unwrap(),
        serde_json::to_string(record.ts.as_ref().expect("ts")).unwrap()
    );

    // (3)
    env.set_mode(ScriptMode::Rust, None);
    d.wait_script_compares().await;
    let before = handle.stats().completed;
    // mode は tick ごとに読む。次の tick の dispatch から rust になる。
    let (_, routing, records) = dispatch(&env, &mut d).await;
    assert!(records.is_empty(), "{records:?}");
    assert_eq!(handle.stats().completed, before, "TS must not be called");
    assert_eq!(decision_bytes(&routing), decision_bytes(&base));
    handle.shutdown();
}

/// (4) 例外を投げる bundle でも lane は Rust の結果で、outcome は `ts_error`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_throwing_bundle_keeps_rust_lane() {
    let env = Env::new(DOMAIN);
    let base = baseline(&env).await;
    let (handle, policy, sha256) = start_code(THROWING_BUNDLE);
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (_, routing, record) = dispatch_until_compared(&env, &mut d).await;
    assert_eq!(record.outcome, CompareOutcome::TsError, "{record:?}");
    let error = record.error.as_ref().expect("error");
    assert_eq!(error.kind, "exception");
    assert!(error.message.contains("boom in lane_rules"), "{error:?}");
    assert_eq!(record.ts, None);
    assert_eq!(decision_bytes(&routing), decision_bytes(&base));
    handle.shutdown();
}

/// 付記 S2-6: 常に違う答えを返す bundle でも、判定の byte 列は rust のときと同じ。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_different_answer_keeps_rust_decision_bytes() {
    let env = Env::new(DOMAIN);
    let base = baseline(&env).await;
    let (handle, policy, sha256) = start_code(DIFFERENT_BUNDLE);
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (_, routing, record) = dispatch_until_compared(&env, &mut d).await;
    assert_eq!(record.outcome, CompareOutcome::Mismatch, "{record:?}");
    assert_eq!(record.ts.as_ref().expect("ts")["proposed"], "frontier");
    assert_eq!(decision_bytes(&routing), decision_bytes(&base));
    assert_ne!(decision(&routing).lane, Tier::Frontier);
    handle.shutdown();
}

async fn run_daemon(config: Config) -> Exit {
    celeris::run(
        config,
        RunOptions {
            until_idle: false,
            max_ticks: 3,
            mode: DaemonMode::Normal,
            release: None,
        },
    )
    .await
    .expect("daemon run")
}

/// daemon の結線: `[script]` の起動で generation の行ができ、compare の dispatch が記録される。
/// `[script]` を外す・別の bundle にすると、起動時に `system:script-startup` が rust へ戻す。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_daemon_injects_records_and_restores_rust() {
    if dist_bundle(
        "script_compare_daemon_injects_records_and_restores_rust",
        DOMAIN,
    )
    .is_none()
    {
        return;
    }
    let env = Env::new(DOMAIN);
    let state = env.dir.path().join("state");
    let deployed = deploy_bundle(&dist_dir(), &state).expect("deploy");
    let sha256 = deployed.manifest.bundle_sha256.clone();
    let with_script = |sha: &str| {
        load_config(
            env.dir.path(),
            &format!("[script]\nstate_dir = \"state\"\nactive_bundle_sha256 = \"{sha}\"\n"),
        )
    };

    // 起動で `loaded` の行ができる。mode は変えない。
    assert_eq!(run_daemon(with_script(&sha256)).await, Exit::MaxTicks);
    let generation = env
        .store
        .script_generation_by_sha256(&sha256)
        .expect("read")
        .expect("generation recorded at startup");
    assert_eq!(generation.state, ScriptGenerationState::Loaded);
    assert!(generation.declares_domain(ScriptDomain::LaneRules));
    assert_eq!(env.mode(), ScriptMode::Rust);

    // daemon の dispatch で match が記録される（注入と停止前の待ちの確認）。
    env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
    let mut matched = None;
    for _ in 0..TIMEOUT_RETRIES {
        let task = env.insert_task();
        assert_eq!(run_daemon(with_script(&sha256)).await, Exit::MaxTicks);
        let records = env.records(&task);
        assert_eq!(records.len(), 1, "{records:?}");
        if !is_timeout(&records[0]) {
            matched = Some((task, records[0].clone()));
            break;
        }
    }
    let (task, record) = matched.expect("a compare that did not time out");
    assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
    assert_eq!(record.generation, generation.id);
    assert_eq!(
        record.rust["proposed"],
        serde_json::to_value(decision(&env.routing(&task)).proposed).unwrap()
    );
    assert_eq!(env.mode(), ScriptMode::Compare, "same bundle keeps compare");

    // `[script]` が無い daemon は compare を rust へ戻す。
    assert_eq!(
        run_daemon(load_config(env.dir.path(), "")).await,
        Exit::MaxTicks
    );
    assert_eq!(env.mode(), ScriptMode::Rust);
    let row = env.store.script_domain_mode(DOMAIN).expect("mode");
    assert_eq!(row.updated_by, "system:script-startup");

    // 配備先に無い bundle を指す daemon も同じ（読めない bundle では compare を続けない）。
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let task = env.insert_task();
    assert_eq!(
        run_daemon(with_script(&"b".repeat(64))).await,
        Exit::MaxTicks
    );
    assert_eq!(env.mode(), ScriptMode::Rust);
    assert!(env.records(&task).is_empty());
}
