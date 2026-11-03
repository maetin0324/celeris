//! 付記 S2: 本物の deno_core host で `route_priority` の compare を確かめる。userns を使わない。外部ネットワークに出ない。
//!
//! bundle を使う試験は、control-ts の source から `$TMPDIR` に作った bundle を読む（`control-ts/dist` は読まない。付記 S5-9）。
//! 作れない（node・node_modules が無い等）なら SKIP を出して抜ける。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗にする。
//! `CELERIS_SCRIPT_BUNDLE_DIR` が在ればその dir の bundle を使い、使えないなら失敗にする。
//! 例外を投げる bundle・常に違う答えを返す bundle・table に無い候補を返す bundle は、この file の中の本文を
//! 同じ host で評価する（bundle を作れなくても走る）。
//! 待ちの方針（`timeout` のときだけ新しい task でやり直す）と共通の helper は `support/script_compare.rs`。

mod support;

use std::sync::Arc;

use support::script_compare::{
    Env, LONG_DEADLINE, build_bundle_cached, check_bundle, control_ts_inputs_sha256, dist_bundle,
    repo_root, require_bundle, start_code, start_policy, take_bundle, take_bundle_logged,
};
use task_core::llm_route::{
    CandidateKey, ExecutionClass, HarnessId, ModelId, Registry, SourceId, route_priority_decide,
};
use task_core::script_compare::{
    CompareCandidate, CompareOutcome, RoutePriorityInput, RoutePriorityOutput, ScriptCompareRecord,
    ScriptDomain, ScriptMode, recorded_input,
};
use task_core::{Event, Task, TaskStore};
use task_dispatch::script_policy::{
    PolicyScript, TsResult, call_route_priority, classify_route_priority,
};

const DOMAIN: &str = "route_priority";
const CALL_SITE: &str = "route_select.route_select_one";
/// `cases.jsonl` の行数（core unit の手書き 26 件 + 生成 2000 件）。
const FIXTURE_LINES: usize = 2_026;
const THROWING_BUNDLE: &str =
    "globalThis.__celerisDispatch = () => { throw new Error(\"boom in route_priority\"); };\n";
/// 宣言済みの列の候補だが、basis が常に違う（Rust は `harness`）。
const DIFFERENT_BUNDLE: &str = "globalThis.__celerisDispatch = () => ({ basis: \"lane\", \
     candidates: [{ source: \"local\", model: \"fake\" }] });\n";
/// 入力の table のどの列にも無い候補を返す。
const UNDECLARED_BUNDLE: &str = "globalThis.__celerisDispatch = () => ({ basis: \"harness\", \
     candidates: [{ source: \"elsewhere\", model: \"ghost\" }] });\n";

/// S2-3 で捨てる値（source の base URL・鍵の参照（環境変数名））。記録に出てはいけない。
/// account は `llm_route_selected` の `account_id`（実際に選ばれた account）で確かめる。
const SECRET_ENDPOINT: &str = "http://route-priority-secret.invalid/v1";
const SECRET_CREDENTIAL_REF: &str = "ROUTE_PRIORITY_SECRET_ENV";

fn secret_sources() -> String {
    format!("endpoint = \"{SECRET_ENDPOINT}\"\ncredential_ref = \"{SECRET_CREDENTIAL_REF}\"\n")
}

/// この task の `llm_route_selected` が選んだ account。
fn selected_account(env: &Env, task: &Task) -> String {
    env.events(task)
        .into_iter()
        .find_map(|e| match e {
            Event::LlmRouteSelected { account_id, .. } => Some(account_id),
            _ => None,
        })
        .expect("llm_route_selected")
}

/// Rust が出すはずの出力（config の `route_defaults.fake` の列）。
fn expected_rust() -> serde_json::Value {
    serde_json::json!({ "basis": "harness", "candidates": [{ "source": "local", "model": "fake" }] })
}

/// 26 文字の ULID（task・run・decision の id）を伏せる。
fn mask_ulids(s: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() == 26 {
            out.push_str("<ULID>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() || c.is_ascii_uppercase() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `harness_selected`・`llm_route_selected` の byte 列（S2-6）。比較の有無で変わってはいけない。
/// dispatch ごとに新しく作られる id（task・run）だけを伏せる。
fn route_bytes(env: &Env, task: &Task) -> String {
    let events: Vec<serde_json::Value> = env
        .events(task)
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Event::HarnessSelected { .. } | Event::LlmRouteSelected { .. }
            )
        })
        .map(|e| serde_json::to_value(e).expect("event json"))
        .collect();
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|e| e.get("type").and_then(|t| t.as_str()))
        .collect();
    assert!(kinds.contains(&"harness_selected"), "{kinds:?}");
    assert!(kinds.contains(&"llm_route_selected"), "{kinds:?}");
    mask_ulids(&serde_json::to_string(&events).expect("events json"))
}

fn to_keys(list: &[CompareCandidate]) -> Vec<CandidateKey> {
    list.iter()
        .map(|c| CandidateKey {
            source: SourceId::from(c.source.as_str()),
            model: ModelId::from(c.model.as_str()),
        })
        .collect()
}

/// 入力 DTO（1 harness の行だけ）から registry を組み直す（`task-core/tests/script_fixtures.rs` と同じ）。
fn registry_from_input(input: &RoutePriorityInput) -> Registry {
    let harness = HarnessId::from(input.harness.as_str());
    let mut r = Registry {
        cos_route_candidates: to_keys(&input.table.cos_route_candidates),
        ..Registry::default()
    };
    // route_defaults が空配列なら「宣言なし」と同じ（付記 S2-10。どちらも basis none・候補は空）。
    if !input.table.route_defaults.is_empty() {
        r.route_defaults
            .insert(harness.clone(), to_keys(&input.table.route_defaults));
    }
    for lane in &input.table.lane_route_defaults {
        r.lane_route_defaults.insert(
            (harness.clone(), lane.lane.clone()),
            to_keys(&lane.candidates),
        );
    }
    for class in &input.table.class_defaults {
        r.route_class_defaults
            .entry(harness.clone())
            .or_default()
            .insert(
                ExecutionClass::from(class.execution_class.as_str()),
                to_keys(&class.candidates),
            );
    }
    r
}

/// fixture の 1 行（入力と、期待値を Rust の `route_priority_decide` に当て直した出力）。
fn fixture_cases() -> Vec<(String, RoutePriorityInput, RoutePriorityOutput)> {
    let path = repo_root().join("control-ts/fixtures/route_priority/cases.jsonl");
    let text = std::fs::read_to_string(&path).expect("fixture");
    text.lines()
        .map(|line| {
            let row: serde_json::Value = serde_json::from_str(line).expect("fixture line");
            let case = row["case"].as_str().expect("case").to_owned();
            let input: RoutePriorityInput =
                serde_json::from_value(row["input"].clone()).expect("input DTO");
            let expected =
                serde_json::json!({ "basis": row["basis"], "candidates": row["candidates"] });
            // fixture の期待値と Rust の参照実装が食い違っていれば、比べる前に落とす。
            let registry = registry_from_input(&input);
            let rust = RoutePriorityOutput::from_decision(route_priority_decide(
                &registry,
                &HarnessId::from(input.harness.as_str()),
                input.lane.as_deref(),
                input.cos,
                input
                    .execution_class
                    .as_deref()
                    .map(ExecutionClass::from)
                    .as_ref(),
            ));
            assert_eq!(
                serde_json::to_value(&rust).expect("rust json"),
                expected,
                "{case}"
            );
            (case, input, rust)
        })
        .collect()
}

/// (1) fixture の全件が本物の host で Rust の `route_priority_decide` と一致する。
/// 同じ isolate に正順と逆順で流し、global に残した値で答えが変わらないことも見る（付記 S2-7）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_route_priority_real_host_matches_rust_on_all_fixtures() {
    let Some(bundle) = dist_bundle(
        "script_compare_route_priority_real_host_matches_rust_on_all_fixtures",
        DOMAIN,
    ) else {
        return;
    };
    assert!(bundle.manifest.domains.iter().any(|d| d == DOMAIN));
    let cases = fixture_cases();
    assert_eq!(cases.len(), FIXTURE_LINES);
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256);
    let mut compared = 0;
    for (case, input, rust) in cases.iter().chain(cases.iter().rev()) {
        let (result, _) = call_route_priority(policy.as_ref(), input.clone(), LONG_DEADLINE).await;
        assert!(matches!(result, TsResult::Reply(_)), "{case}: {result:?}");
        let classified = classify_route_priority(rust, input, &result);
        assert_eq!(
            classified.outcome,
            CompareOutcome::Match,
            "{case}: {classified:?}"
        );
        assert!(classified.safety_violations.is_empty(), "{case}");
        compared += 1;
    }
    assert_eq!(compared, FIXTURE_LINES * 2);
    assert_eq!(handle.stats().restarts, 0, "no isolate restart");
    handle.shutdown();
}

/// 記録の共通の確かめ（領域・call site・run・Rust の出力）。
fn assert_route_record(record: &ScriptCompareRecord) {
    assert_eq!(record.domain, ScriptDomain::RoutePriority);
    assert_eq!(record.call_site, CALL_SITE);
    assert!(
        record.run_id.as_deref().is_some_and(|id| id.len() == 26),
        "{record:?}"
    );
    assert_eq!(record.rust, expected_rust());
}

/// (2) mode compare の dispatch で `match` が 1 件残り、route の event は mode rust の基準と byte 一致。
/// (3) mode を rust に戻すと記録が無く、TS を呼ばない。
/// (5) 記録の入力に S2-3 で捨てる値が無く、digest が合い、TS に通し直すと同じ出力が出る（replay）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_route_priority_dispatch_match_rollback_and_replay() {
    let Some(bundle) = dist_bundle(
        "script_compare_route_priority_dispatch_match_rollback_and_replay",
        DOMAIN,
    ) else {
        return;
    };
    let env = Env::with_sources(DOMAIN, &secret_sources());
    let base_task = env.baseline().await;
    let base = route_bytes(&env, &base_task);
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let generation = env.generation(&sha256);
    let (handle, policy) = start_policy(bundle.source(), sha256.clone());
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy.clone() as Arc<dyn PolicyScript>));

    // mode rust の間は bundle が在っても呼ばない。
    let (task, records) = env.dispatch(&mut d).await;
    assert!(records.is_empty(), "{records:?}");
    assert_eq!(route_bytes(&env, &task), base);
    assert_eq!(handle.stats().completed, 0);

    // (2)
    env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
    let (task, record) = env.dispatch_until_compared(&mut d).await;
    assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
    assert_route_record(&record);
    assert_eq!(record.generation, generation.id);
    assert_eq!(record.bundle_sha256, sha256);
    assert_eq!(record.isolate_epoch, 1);
    assert_eq!(record.ts.as_ref(), Some(&record.rust));
    assert!(record.safety_violations.is_empty() && record.error.is_none());
    assert_eq!(route_bytes(&env, &task), base);
    // 記録の run は、この task に実際に作った run。
    let runs = env.store.runs_for_task(task.id).expect("runs");
    assert!(
        runs.iter()
            .any(|r| Some(&r.run_id) == record.run_id.as_ref()),
        "{runs:?} {record:?}"
    );

    // (5) 記録した入力は DTO だけで、S2-3 で捨てる値を持たない。
    let raw_input = record.input.clone().expect("recorded input");
    let raw_text = serde_json::to_string(&raw_input).expect("input json");
    let account = selected_account(&env, &task);
    assert!(!account.is_empty());
    for secret in [
        SECRET_ENDPOINT,
        "route-priority-secret",
        SECRET_CREDENTIAL_REF,
        account.as_str(),
        "typo",
    ] {
        assert!(!raw_text.contains(secret), "{secret} in {raw_text}");
    }
    assert!(!record.input_truncated);
    let input: RoutePriorityInput = serde_json::from_value(raw_input.clone()).expect("input DTO");
    assert_eq!(serde_json::to_value(&input).unwrap(), raw_input);
    assert_eq!(input.harness, "fake");
    assert!(!input.cos);
    assert_eq!(
        recorded_input(&input).expect("digest").input_digest,
        record.input_digest
    );
    let (replayed, _) = call_route_priority(policy.as_ref(), input, LONG_DEADLINE).await;
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
    let (task, records) = env.dispatch(&mut d).await;
    assert!(records.is_empty(), "{records:?}");
    assert_eq!(handle.stats().completed, before, "TS must not be called");
    assert_eq!(route_bytes(&env, &task), base);
    handle.shutdown();
}

/// file の中の bundle を compare にして 1 件 dispatch し、記録と route の byte 列の基準一致を確かめる。
async fn compare_with_code(code: &str) -> ScriptCompareRecord {
    let env = Env::new(DOMAIN);
    let base_task = env.baseline().await;
    let base = route_bytes(&env, &base_task);
    let (handle, policy, sha256) = start_code(code);
    let generation = env.generation(&sha256);
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy as Arc<dyn PolicyScript>));
    let (task, record) = env.dispatch_until_compared(&mut d).await;
    assert_route_record(&record);
    // TS の答えによらず route は Rust のまま。
    assert_eq!(route_bytes(&env, &task), base);
    handle.shutdown();
    record
}

/// (4) 例外を投げる bundle は `ts_error`（`exception`）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_route_priority_throwing_bundle_keeps_rust_route() {
    let record = compare_with_code(THROWING_BUNDLE).await;
    assert_eq!(record.outcome, CompareOutcome::TsError, "{record:?}");
    let error = record.error.as_ref().expect("error");
    assert_eq!(error.kind, "exception");
    assert!(
        error.message.contains("boom in route_priority"),
        "{error:?}"
    );
    assert_eq!(record.ts, None);
}

/// (4) 常に違う答え（宣言済みの候補・違う basis）を返す bundle は `mismatch`、安全違反なし。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_route_priority_different_answer_keeps_rust_route() {
    let record = compare_with_code(DIFFERENT_BUNDLE).await;
    assert_eq!(record.outcome, CompareOutcome::Mismatch, "{record:?}");
    assert_eq!(record.ts.as_ref().expect("ts")["basis"], "lane");
    assert!(record.safety_violations.is_empty(), "{record:?}");
}

/// (4) table に無い候補を返す bundle は `mismatch` で `undeclared_candidate`（S2-8）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_route_priority_undeclared_candidate_keeps_rust_route() {
    let record = compare_with_code(UNDECLARED_BUNDLE).await;
    assert_eq!(record.outcome, CompareOutcome::Mismatch, "{record:?}");
    assert_eq!(record.safety_violations, vec!["undeclared_candidate"]);
    assert_eq!(
        record.ts.as_ref().expect("ts")["candidates"][0]["source"],
        "elsewhere"
    );
}

/// tempdir の `dist/` に `domains` の manifest を持つ検証の通る bundle を置き、`bundle.config.json` には
/// `host_selftest`・`lane_rules`・`route_priority` を宣言する。返り値は (tempdir, dist, config)。
fn stale_dist_fixture(
    domains: &[&str],
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    use celeris_script::manifest::{
        BUNDLE_FILE, MANIFEST_FILE, MANIFEST_SCHEMA, host_schema_sha256, sha256_hex,
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(&dist).expect("dist");
    let code = "globalThis.__celerisDispatch = () => null;\n";
    std::fs::write(dist.join(BUNDLE_FILE), code).expect("bundle");
    let manifest = serde_json::json!({
        "schema": MANIFEST_SCHEMA,
        "bundle_sha256": sha256_hex(code.as_bytes()),
        "host_api": "^1",
        "schema_sha256": host_schema_sha256(),
        "toolchain": { "typescript": "0", "esbuild": "0", "node": "0" },
        "source_commit": "0000000",
        "domains": domains,
        "capabilities": [],
        "checkpoint_versions": [],
    });
    std::fs::write(dist.join(MANIFEST_FILE), manifest.to_string()).expect("manifest");
    let config = dir.path().join("bundle.config.json");
    let config_json = serde_json::json!({
        "host_api": "^1",
        "domains": { "host_selftest": [], "lane_rules": [], "route_priority": [] },
        "checkpoint_versions": [],
    });
    std::fs::write(&config, config_json.to_string()).expect("config");
    (dir, dist, config)
}

const ALL_DOMAINS: [&str; 3] = ["host_selftest", "lane_rules", "route_priority"];

/// route_priority を足す前の bundle（domains が host_selftest・lane_rules だけ）は使えない。
/// config の領域の集合と違うので、manifest に在る lane_rules の試験からも使わない。
#[test]
fn script_compare_stale_dist_missing_domain_is_skipped() {
    let (_dir, dist, config) = stale_dist_fixture(&["host_selftest", "lane_rules"]);
    assert!(check_bundle(&dist, &config, DOMAIN).is_err());
    assert!(check_bundle(&dist, &config, "lane_rules").is_err());
}

/// manifest の domains が config と揃っていれば使う。
#[test]
fn script_compare_stale_dist_matching_domains_is_used() {
    let (_dir, dist, config) = stale_dist_fixture(&ALL_DOMAINS);
    let bundle = check_bundle(&dist, &config, DOMAIN).expect("usable");
    assert!(bundle.manifest.domains.iter().any(|d| d == DOMAIN));
    assert!(check_bundle(&dist, &config, "lane_rules").is_ok());
}

/// config に無い領域を要る試験は、揃った bundle でも使わない。
#[test]
fn script_compare_stale_dist_unknown_domain_is_skipped() {
    let (_dir, dist, config) = stale_dist_fixture(&ALL_DOMAINS);
    assert!(check_bundle(&dist, &config, "no_such_domain").is_err());
}

/// `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` 相当（`require`）では古い bundle を失敗にする。
#[test]
#[should_panic(expected = "stale bundle")]
fn script_compare_stale_dist_required_panics() {
    let (_dir, dist, config) = stale_dist_fixture(&["host_selftest", "lane_rules"]);
    let _ = take_bundle_logged(
        check_bundle(&dist, &config, DOMAIN),
        &dist,
        "script_compare_stale_dist_required_panics",
        true,
        None,
    );
}

/// `control-ts/` の形の tempdir（`src/`・package の file・`scripts/build-bundle.mjs`・host の schema）。
/// `build-bundle.mjs` は `buildBundle({ outDir })` で `outDir` に `manifest.json` を書くだけの偽物。
/// host の schema の置き場（`control-ts/` の親からの相対 path）。
const SCHEMA: &str = "docs/api/script-host/host-v1.schema.json";

fn fake_control_ts() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cts = dir.path().join("control-ts");
    let files = [
        ("src/main.ts", "export {};\n"),
        ("src/domains/x.ts", "export const x = 1;\n"),
        ("bundle.config.json", "{}\n"),
        ("package.json", "{\"name\":\"t\"}\n"),
        ("pnpm-lock.yaml", "lockfileVersion: 9\n"),
        (
            "scripts/build-bundle.mjs",
            "import { mkdirSync, writeFileSync } from \"node:fs\";\n\
             export async function buildBundle({ outDir }) {\n\
               mkdirSync(outDir, { recursive: true });\n\
               writeFileSync(`${outDir}/manifest.json`, \"{}\\n\");\n\
             }\n",
        ),
        ("schema", "{}\n"),
    ];
    for (rel, text) in files {
        let path = if rel == "schema" {
            dir.path().join(SCHEMA)
        } else {
            cts.join(rel)
        };
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    (dir, cts)
}

/// 付記 S5-9: 試験は source の内容 hash の dir に作った bundle を読む。source を変えると hash が変わり、作り直す
/// （古い bundle を読まない）。同じ source なら作った dir を使い回す。
#[test]
fn script_compare_fresh_bundle_follows_source() {
    let test = "script_compare_fresh_bundle_follows_source";
    let node = std::process::Command::new("node").args(["-e", ""]).status();
    if !node.as_ref().is_ok_and(|s| s.success()) {
        let why = format!("node does not run: {node:?}");
        let _ = take_bundle(
            Err(why),
            std::path::Path::new("node"),
            test,
            require_bundle(),
        );
        return;
    }
    let (_dir, cts) = fake_control_ts();
    let before = control_ts_inputs_sha256(&cts).expect("inputs");
    let cache = cts.join("../cache");
    let built = build_bundle_cached(&cts, &cache).expect("build");
    assert_eq!(built, cache.join(&before));
    assert!(built.join("manifest.json").is_file());
    // 2 回目は作らない（作った dir に印を置いて、残ることを見る）。
    std::fs::write(built.join("mark"), "").expect("mark");
    assert_eq!(build_bundle_cached(&cts, &cache).expect("reuse"), built);
    assert!(built.join("mark").is_file());

    for path in [
        cts.join("src/domains/x.ts"),
        cts.join("scripts/build-bundle.mjs"),
        cts.join("..").join(SCHEMA),
    ] {
        let mut text = std::fs::read_to_string(&path).expect("read");
        text.push_str("// edit\n");
        std::fs::write(&path, text).expect("edit");
        let after = control_ts_inputs_sha256(&cts).expect("inputs");
        assert_ne!(
            after,
            before,
            "{} must change the inputs hash",
            path.display()
        );
        let rebuilt = build_bundle_cached(&cts, &cache).expect("rebuild");
        assert_eq!(rebuilt, cache.join(&after));
        assert!(!rebuilt.join("mark").exists());
    }
    // 作業中の dir と lock は残らない。
    let names: Vec<String> = std::fs::read_dir(&cache)
        .expect("cache")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names
            .iter()
            .all(|n| !n.contains(".tmp-") && !n.ends_with(".lock")),
        "{names:?}"
    );
}

/// bundle を作れない（build-bundle.mjs が失敗する）環境は SKIP。SKIP は log の file に 1 行（試験名・tab・理由）
/// 残り、require なら失敗にする。
#[test]
fn script_compare_unbuildable_bundle_is_skipped_and_logged() {
    let (dir, cts) = fake_control_ts();
    std::fs::write(
        cts.join("scripts/build-bundle.mjs"),
        "export async function buildBundle() { throw new Error(\"boom\"); }\n",
    )
    .expect("broken build");
    let cache = dir.path().join("cache");
    let log = dir.path().join("skip.log");
    let test = "script_compare_unbuildable_bundle_is_skipped_and_logged";
    let checked = || {
        build_bundle_cached(&cts, &cache)
            .and_then(|d| check_bundle(&d, &cts.join("bundle.config.json"), DOMAIN))
    };
    for _ in 0..2 {
        assert!(take_bundle_logged(checked(), &cts, test, false, Some(&log)).is_none());
    }
    let text = std::fs::read_to_string(&log).expect("skip log");
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(
        text.lines().all(|l| l.starts_with(&format!("{test}\t"))
            && (l.contains("build-bundle.mjs failed") || l.contains("cannot run node"))),
        "{text}"
    );
    let required =
        std::panic::catch_unwind(|| take_bundle_logged(checked(), &cts, test, true, None));
    assert!(
        required.is_err(),
        "require must fail when no bundle can be built"
    );
}
