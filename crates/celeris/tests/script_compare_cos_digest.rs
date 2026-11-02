//! 付記 S4: 本物の deno_core host で `cos_digest`（CoS 受信箱の digest pass）の compare と replay を確かめる。
//! userns を使わない。外部ネットワークに出ない。LLM を呼ばない（triage run の結果は store に直接書く）。
//!
//! `control-ts/dist` の bundle を使う試験は、無い・古いなら SKIP を出して抜ける。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1`
//! なら失敗にする。例外を投げる bundle・常に違う答えを返す bundle は、この file の中の本文を同じ host で評価する
//! （dist が無くても走る）。
//!
//! 待ちの方針: dispatcher の deadline は 50 ms の固定。`timeout` は環境の遅さなので、そのときだけ新しい一時 DB で
//! 場面をやり直す（`support/script_compare.rs` と同じ方針。digest は run ごとに 1 回なので、同じ DB ではやり直せない）。
//!
//! config は `[cos] enabled = false`。digest pass は CoS が無効でも走り（取り込みと同じ）、CoS の run は起動しない。

mod support;

use std::sync::Arc;

use celeris::{Config, Exit, RunOptions};
use celeris_script::ScriptRuntimeHandle;
use celeris_script::manifest::deploy_bundle;
use support::script_compare::{
    Env, LONG_DEADLINE, TIMEOUT_RETRIES, dist_bundle, dist_dir, load_config, repo_root, start_code,
    start_policy,
};
use task_core::chat::digest::{CosDigestCommand, CosDigestInput, cos_digest_decide};
use task_core::chat::digest::{
    CosDigestPassOutput, CosDigestPassRead, CosDigestPassRun, CosDigestRunRef,
};
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{ChatMessageQuery, ChatMessageRole, ChatOpenThreadRequest, ChatRunState};
use task_core::script_compare::{
    CompareOutcome, CosDigestCommandsSummary, CosDigestCompareRecord, ScriptDomain,
    ScriptGenerationState, ScriptMode, sha256_digest,
};
use task_core::{DaemonMode, SqliteStore};
use task_dispatch::script_policy::{
    CosDigestOp, CosDigestOpReply, CosDigestPassCall, PolicyScript, TsResult,
    classify_cos_digest_pass,
};
use time::OffsetDateTime;

const DOMAIN: &str = "cos_digest";
const CALL_SITE: &str = "cos_digest.pass";
/// CoS の run を起動しない（偽 harness も呼ばない）。
const COS_OFF: &str = "[cos]\nenabled = false\n";
const DIGEST_KEY_PREFIX: &str = "cos-triage-digest:";
const THROWING_BUNDLE: &str =
    "globalThis.__celerisDispatch = () => { throw new Error(\"boom in cos_digest\"); };\n";
/// op を呼ばず（capability が無い）、どの run も digest しない pass（規則 8 の `run_missing`）。
const DIFFERENT_BUNDLE: &str = "globalThis.__celerisDispatch = () => ({ runs: [] });\n";

fn env() -> Env {
    Env::with_config(DOMAIN, "", COS_OFF)
}

/// fixture の 1 行（`{name, input, commands}`）。`commands` は Rust の参照実装の出力と byte 一致を確かめてから返す。
fn fixture_rows(file: &str) -> Vec<(String, CosDigestInput, Vec<CosDigestCommand>)> {
    let path = repo_root()
        .join("control-ts/fixtures/cos_digest")
        .join(file);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.lines()
        .map(|line| {
            let row: serde_json::Value = serde_json::from_str(line).expect("fixture line");
            let name = row["name"].as_str().expect("name").to_owned();
            let input: CosDigestInput =
                serde_json::from_value(row["input"].clone()).expect("input DTO");
            let rust = cos_digest_decide(&input);
            // `Value` は key を並べ替えるので、行の本文のまま比べる（行の最後の欄が `commands`）。
            let rust_json = serde_json::to_string(&rust).expect("rust json");
            assert!(
                line.ends_with(&format!("\"commands\":{rust_json}}}")),
                "{file} {name}: the fixture is not Rust's output: {rust_json}"
            );
            (name, input, rust)
        })
        .collect()
}

/// run 1 つの pass を本物の host で走らせる（付記 S4-16 の組み直し: list の答え = その run、read の答え = `input`）。
/// op は試験の中の op server が答える。
async fn one_run_pass(policy: &dyn PolicyScript, input: &CosDigestInput) -> TsResult {
    let (sink, mut rx) = tokio::sync::mpsc::channel(64);
    let list = serde_json::json!({ "runs": [{
        "thread": input.thread, "run_id": input.run_id, "run_state": input.run_state,
    }] });
    let read = serde_json::to_value(input).expect("input json");
    let server = tokio::spawn(async move {
        while let Some(op) = rx.recv().await {
            let op: task_dispatch::script_policy::CosDigestOpCall = op;
            let reply = match op.op {
                CosDigestOp::ListRuns => CosDigestOpReply::Ok(list.clone()),
                CosDigestOp::ReadRun => CosDigestOpReply::Ok(read.clone()),
            };
            let _ = op.reply.send(reply);
        }
    });
    let call = CosDigestPassCall {
        pass_id: "01M6TEST000000000000000000".into(),
        sink,
    };
    let result = match policy.cos_digest_pass(call, LONG_DEADLINE).await {
        Ok(reply) => TsResult::Reply(reply),
        Err(e) => TsResult::Error(e),
    };
    server.abort();
    result
}

/// TS の pass の答えを Rust の command と比べ、`match` で byte 一致であることを確かめる。
async fn assert_ts_matches(
    policy: &dyn PolicyScript,
    name: &str,
    input: &CosDigestInput,
    rust: &[CosDigestCommand],
) -> CosDigestCommandsSummary {
    let result = one_run_pass(policy, input).await;
    let TsResult::Reply(reply) = &result else {
        panic!("{name}: {result:?}")
    };
    let ts: CosDigestPassOutput =
        serde_json::from_value(reply.output.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(ts.runs.len(), 1, "{name}");
    assert_eq!(
        serde_json::to_string(&ts.runs[0].commands).expect("ts json"),
        serde_json::to_string(rust).expect("rust json"),
        "{name}"
    );
    let reads = [CosDigestPassRead {
        run: CosDigestRunRef {
            thread: input.thread.clone(),
            run_id: input.run_id.clone(),
            run_state: input.run_state.clone(),
        },
        input: Some(input.clone()),
    }];
    let rust_out = CosDigestPassOutput {
        runs: vec![CosDigestPassRun {
            run_id: input.run_id.clone(),
            commands: rust.to_vec(),
        }],
    };
    let classified = classify_cos_digest_pass(&reads, &rust_out, &result);
    let run = classified.run(&input.run_id).expect("run").clone();
    assert_eq!(run.outcome, CompareOutcome::Match, "{name}: {run:?}");
    assert!(run.safety_violations.is_empty(), "{name}");
    run.ts.expect("ts summary")
}

/// (1) cases.jsonl と traces.jsonl の全行が本物の host で Rust の `cos_digest_decide` と byte 一致する。
/// 同じ isolate に正順と逆順で流し、global に残した値で答えが変わらないことも見る（付記 S4-6）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_real_host_matches_rust_on_all_fixtures() {
    let Some(bundle) = dist_bundle(
        "script_compare_cos_digest_real_host_matches_rust_on_all_fixtures",
        DOMAIN,
    ) else {
        return;
    };
    let cases = fixture_rows("cases.jsonl");
    let traces = fixture_rows("traces.jsonl");
    assert!(cases.len() >= 2_000, "{}", cases.len());
    assert!(traces.len() >= 10, "{}", traces.len());
    let rows: Vec<_> = cases.iter().chain(traces.iter()).collect();
    let (handle, policy) = start_policy(bundle.source(), bundle.manifest.bundle_sha256.clone());
    let mut compared = 0;
    for (name, input, rust) in rows.iter().copied().chain(rows.iter().rev().copied()) {
        assert_ts_matches(policy.as_ref(), name, input, rust).await;
        compared += 1;
    }
    assert_eq!(compared, rows.len() * 2);
    assert_eq!(handle.stats().restarts, 0, "no isolate restart");
    handle.shutdown();
}

/// `id` の triage run に 1 件の item を渡し、`state` で終える（completed は item を observed にする）。
/// LLM の代わりに、CoS が書いたはずの結果（resolve）を store に書く。
fn seed_run(store: &SqliteStore, id: &str, state: ChatRunState) {
    let now = OffsetDateTime::now_utc();
    store
        .cos_triage_ingest_batch(
            "digest-compare-test",
            id,
            &[CosTriageSource {
                source_kind: "notice".into(),
                source_key: id.into(),
                source_revision: "1".into(),
                source_event_id: None,
                operation_id: None,
                summary: format!("知らせ {id}"),
                detail: String::new(),
                policy_version: "1".into(),
                task_id: None,
            }],
            now,
        )
        .expect("ingest");
    let general = general_thread(store);
    let claim = store
        .cos_triage_claim(&general, id, now)
        .expect("claim")
        .expect("run");
    assert_eq!(claim.item_ids.len(), 1, "{id}: only its own item");
    if state == ChatRunState::Completed {
        store
            .cos_triage_resolve(&claim.item_ids[0], "observed", None, Some("確認済み"), now)
            .expect("resolve");
    }
    store
        .chat_run_finish(id, state, None, Some("人が止めた"), now)
        .expect("finish");
}

fn general_thread(store: &SqliteStore) -> String {
    store
        .cos_thread_open(
            &ChatOpenThreadRequest { project_id: None },
            OffsetDateTime::now_utc(),
        )
        .expect("general")
        .thread
        .id
}

/// digest の message と item の行（id・時刻を除いた直列化）。比較の有無で変わってはいけない。
fn world(env: &Env) -> (String, String) {
    let messages: Vec<serde_json::Value> = env
        .store
        .chat_message_list(
            &general_thread(&env.store),
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .filter(|m| {
            m.role == ChatMessageRole::Assistant
                && m.client_message_id
                    .as_deref()
                    .is_some_and(|k| k.starts_with(DIGEST_KEY_PREFIX))
        })
        .map(|m| {
            serde_json::json!({
                "key": m.client_message_id,
                "text": m.text,
                "cards": m.cards,
                "run_id": m.run_id,
            })
        })
        .collect();
    let conn = rusqlite::Connection::open(&env.config.db.path).expect("db");
    let mut stmt = conn
        .prepare(
            "SELECT source_key, state, run_id, reason FROM cos_inbox_items ORDER BY source_key",
        )
        .expect("prepare");
    let items: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            Ok(serde_json::json!([
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ]))
        })
        .expect("items")
        .map(|r| r.expect("row"))
        .collect();
    (
        serde_json::to_string(&messages).expect("json"),
        serde_json::to_string(&items).expect("json"),
    )
}

fn log(store: &SqliteStore) -> Vec<CosDigestCompareRecord> {
    store
        .script_compare_log(None)
        .expect("log")
        .into_iter()
        .map(|row| {
            assert_eq!(row.domain, ScriptDomain::CosDigest);
            assert_eq!(row.thread_id, row.record.thread_id);
            assert_eq!(row.run_id, row.record.run_id);
            row.record
        })
        .collect()
}

fn is_timeout(record: &CosDigestCompareRecord) -> bool {
    record.outcome == CompareOutcome::TsError
        && record.error.as_ref().is_some_and(|e| e.kind == "timeout")
}

/// 1 回の digest pass の結果。
struct Pass {
    env: Env,
    messages: String,
    items: String,
    records: Vec<CosDigestCompareRecord>,
}

/// 一時 DB の dispatcher で completed と stopped の run を 1 つずつ digest する。`script` があれば mode compare。
/// 2 回目の tick（再配送）で message も記録も増えないことも確かめる。
async fn digest_pass(script: Option<(&Arc<dyn PolicyScript>, &str)>) -> Pass {
    let env = env();
    let mut d = env.dispatcher();
    if let Some((policy, sha256)) = script {
        let generation = env.generation(sha256);
        env.set_mode(ScriptMode::Compare, Some(generation.id));
        d.set_policy_script(Some(policy.clone()));
    }
    seed_run(&env.store, "run-a-done", ChatRunState::Completed);
    seed_run(&env.store, "run-b-stop", ChatRunState::Stopped);
    d.tick().expect("tick");
    d.wait_script_compares().await;
    let (messages, items) = world(&env);
    let records = log(&env.store);
    d.tick().expect("tick");
    d.wait_script_compares().await;
    assert_eq!(world(&env), (messages.clone(), items.clone()));
    assert_eq!(log(&env.store).len(), records.len());
    let mut records = records;
    records.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Pass {
        env,
        messages,
        items,
        records,
    }
}

/// mode rust・判定の口なしの digest（比べる基準）。
async fn baseline() -> Pass {
    let base = digest_pass(None).await;
    assert!(base.records.is_empty());
    assert_eq!(
        base.messages.matches(DIGEST_KEY_PREFIX).count(),
        2,
        "{}",
        base.messages
    );
    // completed の run の item は observed で run に残る。stopped の run の item は requeue で run から外れる
    // （CoS が無効なので、同じ tick の退避が fallback にする）。
    assert!(
        base.items
            .contains(r#"["run-a-done","observed","run-a-done","#),
        "{}",
        base.items
    );
    assert!(
        base.items.contains(r#"["run-b-stop","fallback",null,"#),
        "{}",
        base.items
    );
    base
}

/// compare の digest pass を `timeout` 以外の結果が出るまでやり直す。message・item は基準と byte 一致。
async fn compare_pass(policy: &Arc<dyn PolicyScript>, sha256: &str, base: &Pass) -> Pass {
    for _ in 0..TIMEOUT_RETRIES {
        let pass = digest_pass(Some((policy, sha256))).await;
        assert_eq!(pass.messages, base.messages, "messages are Rust's");
        assert_eq!(pass.items, base.items, "requeue is Rust's");
        assert_eq!(pass.records.len(), 2, "one record per run");
        if pass.records.iter().any(is_timeout) {
            continue;
        }
        let r = &pass.records;
        for record in r {
            assert_eq!(record.call_site, CALL_SITE);
            assert_eq!(record.bundle_sha256, sha256);
            assert_eq!(record.input, None, "no body in the log");
            assert!(record.input_digest.starts_with("sha256:"));
            assert!(!record.applied.message_id.is_empty());
        }
        assert_eq!(r[0].run_id, "run-a-done");
        assert_eq!(r[0].rust.command_kinds, vec!["post_digest"]);
        assert_eq!(r[0].applied.requeued, 0);
        assert_eq!(r[1].run_id, "run-b-stop");
        assert_eq!(r[1].rust.command_kinds, vec!["post_digest", "requeue_run"]);
        assert_eq!(r[1].applied.requeued, 1);
        return pass;
    }
    panic!("every compare timed out ({TIMEOUT_RETRIES} attempts)");
}

/// 新しい isolate の最初の呼び出しは遅い。dispatcher の 50 ms の deadline の前に 1 回温める。
async fn warm(policy: &dyn PolicyScript) {
    let (name, input, rust) = fixture_rows("traces.jsonl")
        .into_iter()
        .next()
        .expect("a trace");
    assert_ts_matches(policy, &name, &input, &rust).await;
}

/// 記録した `thread_id`・`run_id` から store で入力を組み直す（付記 S4-5 の replay）。
fn rebuild_input(store: &SqliteStore, record: &CosDigestCompareRecord) -> CosDigestInput {
    let run = store
        .chat_run_get(&record.thread_id, &record.run_id)
        .expect("run");
    let run_state = serde_json::to_value(run.state)
        .expect("state json")
        .as_str()
        .expect("state string")
        .to_owned();
    CosDigestInput {
        thread: record.thread_id.clone(),
        run_id: record.run_id.clone(),
        run_state,
        reason: run.reason,
        items: store.cos_triage_run_report(&record.run_id).expect("report"),
    }
}

/// (2) mode compare の digest pass で `match` が run ごとに残り、message と requeue は Rust の結果。
/// (5) 記録から入力を組み直して TS に通すと同じ command（replay。digest の合わない run は drifted として数える）。
/// (3) mode を rust に戻すと TS を呼ばない（actor の完了数が増えない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_dispatch_match_replay_and_rollback() {
    let Some(bundle) = dist_bundle(
        "script_compare_cos_digest_dispatch_match_replay_and_rollback",
        DOMAIN,
    ) else {
        return;
    };
    let base = baseline().await;
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256.clone());
    let policy: Arc<dyn PolicyScript> = policy;
    warm(policy.as_ref()).await;

    // (2)
    let pass = compare_pass(&policy, &sha256, &base).await;
    for record in &pass.records {
        assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
        assert!(record.safety_violations.is_empty() && record.error.is_none());
        assert_eq!(record.ts.as_ref(), Some(&record.rust));
        assert_eq!(record.ts_commands_head, None);
        assert_eq!(record.isolate_epoch, 1);
    }

    // (5) replay。requeue した run は item が run から外れたので、入力の digest が変わる（drifted）。
    let mut replayed = Vec::new();
    let mut drifted = Vec::new();
    for record in &pass.records {
        let input = rebuild_input(&pass.env.store, record);
        let digest = sha256_digest(&serde_json::to_vec(&input).expect("input json"));
        if digest != record.input_digest {
            drifted.push(record.run_id.clone());
            continue;
        }
        let rust = cos_digest_decide(&input);
        assert_eq!(
            CosDigestCommandsSummary::of(&rust).expect("summary"),
            record.rust
        );
        let ts = assert_ts_matches(policy.as_ref(), &record.run_id, &input, &rust).await;
        assert_eq!(
            Some(&ts),
            record.ts.as_ref(),
            "replay gives the same TS commands"
        );
        replayed.push(record.run_id.clone());
    }
    assert_eq!(replayed, vec!["run-a-done"]);
    assert_eq!(drifted, vec!["run-b-stop"]);

    // (3) 同じ DB で mode を rust に戻し、新しい run を digest する。
    pass.env.set_mode(ScriptMode::Rust, None);
    let before = handle.stats().completed;
    let mut d = pass.env.dispatcher();
    d.set_policy_script(Some(policy.clone()));
    seed_run(&pass.env.store, "run-c-rust", ChatRunState::Completed);
    d.tick().expect("tick");
    d.wait_script_compares().await;
    let (messages, _) = world(&pass.env);
    assert!(
        messages.contains("cos-triage-digest:run-c-rust"),
        "{messages}"
    );
    assert_eq!(handle.stats().completed, before, "TS must not be called");
    assert_eq!(log(&pass.env.store).len(), 2, "no record in mode rust");
    handle.shutdown();
}

/// file の中の bundle を compare にして digest pass を走らせる。message と requeue は基準と byte 一致。
async fn compare_with_code(code: &str) -> (ScriptRuntimeHandle, Vec<CosDigestCompareRecord>) {
    let base = baseline().await;
    let (handle, policy, sha256) = start_code(code);
    let policy: Arc<dyn PolicyScript> = policy;
    let pass = compare_pass(&policy, &sha256, &base).await;
    (handle, pass.records)
}

/// (4) 例外を投げる bundle は `ts_error`（`exception`）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_throwing_bundle_keeps_rust_digest() {
    let (handle, records) = compare_with_code(THROWING_BUNDLE).await;
    for record in &records {
        assert_eq!(record.outcome, CompareOutcome::TsError, "{record:?}");
        let error = record.error.as_ref().expect("error");
        assert_eq!(error.kind, "exception");
        assert!(error.message.contains("boom in cos_digest"), "{error:?}");
        assert_eq!(record.ts, None);
    }
    handle.shutdown();
}

/// (4) どの run も digest しない pass を返す bundle は `invalid_output`（規則 8 の `run_missing`）。
/// 安全違反は無く、message と requeue は Rust の結果のまま。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_different_answer_keeps_rust_digest() {
    let (handle, records) = compare_with_code(DIFFERENT_BUNDLE).await;
    for record in &records {
        assert_eq!(record.outcome, CompareOutcome::InvalidOutput, "{record:?}");
        assert!(record.safety_violations.is_empty(), "{record:?}");
        assert!(
            record
                .error
                .as_ref()
                .is_some_and(|e| e.message.contains("is missing")
                    || e.message.contains("is not in the output exactly once")),
            "{record:?}"
        );
    }
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

/// (6) daemon の結線: `[script]` の起動で generation の行ができ、compare の digest pass が記録される。
/// `[script]` を外す・配備先に無い bundle を指すと、起動時に `system:script-startup` が `cos_digest` を rust へ戻す。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_daemon_restart_without_bundle_restores_rust() {
    if dist_bundle(
        "script_compare_cos_digest_daemon_restart_without_bundle_restores_rust",
        DOMAIN,
    )
    .is_none()
    {
        return;
    }
    let env = env();
    let state = env.dir.path().join("state");
    let sha256 = deploy_bundle(&dist_dir(), &state)
        .expect("deploy")
        .manifest
        .bundle_sha256;
    let with_script = |sha: &str| {
        load_config(
            env.dir.path(),
            &format!(
                "{COS_OFF}[script]\nstate_dir = \"state\"\nactive_bundle_sha256 = \"{sha}\"\n"
            ),
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
    assert!(generation.declares_domain(ScriptDomain::CosDigest));
    assert_eq!(env.mode(), ScriptMode::Rust);

    // 同じ bundle の daemon は compare を保ち、digest pass の記録を残す（停止前に compare を待つ）。
    env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
    seed_run(&env.store, "run-a-done", ChatRunState::Completed);
    assert_eq!(run_daemon(with_script(&sha256)).await, Exit::MaxTicks);
    assert_eq!(env.mode(), ScriptMode::Compare, "same bundle keeps compare");
    let records = log(&env.store);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].run_id, "run-a-done");
    assert_eq!(records[0].generation, generation.id);
    assert!(
        records[0].outcome == CompareOutcome::Match || is_timeout(&records[0]),
        "{:?}",
        records[0]
    );

    // `[script]` が無い daemon は rust へ戻す。digest は Rust のまま書かれ、記録は増えない。
    seed_run(&env.store, "run-b-stop", ChatRunState::Stopped);
    assert_eq!(
        run_daemon(load_config(env.dir.path(), COS_OFF)).await,
        Exit::MaxTicks
    );
    assert_eq!(env.mode(), ScriptMode::Rust);
    let row = env.store.script_domain_mode(DOMAIN).expect("mode");
    assert_eq!(row.updated_by, "system:script-startup");
    let (messages, _) = world(&env);
    assert!(
        messages.contains("cos-triage-digest:run-b-stop"),
        "{messages}"
    );
    assert_eq!(log(&env.store).len(), 1);

    // 配備先に無い bundle を指す daemon も同じ（読めない bundle では compare を続けない）。
    env.set_mode(ScriptMode::Compare, Some(generation.id));
    seed_run(&env.store, "run-c-rust", ChatRunState::Completed);
    assert_eq!(
        run_daemon(with_script(&"b".repeat(64))).await,
        Exit::MaxTicks
    );
    assert_eq!(env.mode(), ScriptMode::Rust);
    let (messages, _) = world(&env);
    assert!(
        messages.contains("cos-triage-digest:run-c-rust"),
        "{messages}"
    );
    assert_eq!(log(&env.store).len(), 1);
}
