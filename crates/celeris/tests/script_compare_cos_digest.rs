//! 付記 S4: 本物の deno_core host で `cos_digest`（CoS 受信箱の digest pass）の compare と replay を確かめる。
//! mode ts（付記 S4-14・S4-15）では、TS の async pass が op を await し、その command を Rust が検証してから apply する
//! こと、TS が使えない pass で Rust の command を apply すること、daemon の再起動で message が増えず bundle が無ければ
//! rust に戻ることも確かめる（file の後半）。
//! userns を使わない。外部ネットワークに出ない。LLM を呼ばない（triage run の結果は store に直接書く）。
//!
//! `control-ts/dist` の bundle を使う試験は、無い・古いなら SKIP を出して抜ける。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1`
//! なら失敗にする。例外を投げる bundle・常に違う答えを返す bundle は、この file の中の本文を同じ host で評価する
//! （dist が無くても走る）。`CELERIS_SCRIPT_BUNDLE_DIR` が在ればその dir の bundle を使い、使えないなら失敗にする（付記 S5-9）。
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
    CompareOutcome, CosDigestAppliedBy, CosDigestCommandsSummary, CosDigestCompareRecord,
    ScriptDomain, ScriptGenerationState, ScriptMode, sha256_digest,
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

/// (1) cases.jsonl の全行が本物の host で Rust の `cos_digest_decide` と byte 一致する。
/// 同じ isolate に正順と逆順で流し、global に残した値で答えが変わらないことも見る（付記 S4-6）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_real_host_matches_rust_on_all_fixtures() {
    let Some(bundle) = dist_bundle(
        "script_compare_cos_digest_real_host_matches_rust_on_all_fixtures",
        DOMAIN,
    ) else {
        return;
    };
    let rows = fixture_rows("cases.jsonl");
    assert!(rows.len() >= 2_000, "{}", rows.len());
    let (handle, policy) = start_policy(bundle.source(), bundle.manifest.bundle_sha256.clone());
    let mut compared = 0;
    for (name, input, rust) in rows.iter().chain(rows.iter().rev()) {
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
    let (name, input, rust) = fixture_rows("cases.jsonl")
        .into_iter()
        .next()
        .expect("a case");
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

// ---------------------------------------------------------------------------------------------
// mode ts（付記 S4-14・S4-15）: TS の async pass が op を await し、その command を Rust が検証してから apply する。
// ---------------------------------------------------------------------------------------------

/// op を呼んだ後、一覧に無い run を読む pass（dispatcher の memo が `denied` で答え、TS は throw する）。
const DENIED_BUNDLE: &str = r#"import { cosDigestListRuns, cosDigestReadRun } from "celeris:host";
globalThis.__celerisDispatch = async (_domain, _input, ctx) => {
  await cosDigestListRuns(ctx, {});
  await cosDigestReadRun(ctx, { thread: "thread-not-listed", run_id: "run-not-listed" });
  return { runs: [] };
};
"#;
/// 決して答えない pass（dispatcher の deadline 200 ms を超える）。CPU は使わない。
const NEVER_BUNDLE: &str = "globalThis.__celerisDispatch = () => new Promise(() => {});\n";
/// 本物の bundle の pass の文面の末尾に印を足す（規則 1〜9 には通る）。DB の message に印があれば TS の command を
/// apply した証拠になる（TS と Rust の出力が同じ本物の bundle では、DB を見ても区別できないため）。
const TS_MARK: &str = "\n(ts-applied)";
const MARK_WRAPPER: &str = r#"
const __celerisRealDispatch = globalThis.__celerisDispatch;
globalThis.__celerisDispatch = async (domain, input, ctx) => {
  const out = await __celerisRealDispatch(domain, input, ctx);
  for (const run of out.runs) {
    for (const command of run.commands) {
      if (command.kind === "post_digest") command.text += "\n(ts-applied)";
    }
  }
  return out;
};
"#;

/// 本物の bundle の capability（`cos_digest:list_runs`・`cos_digest:read_run`）を持たせて `code` を同じ host で起こす。
fn start_with_caps(
    bundle: &celeris_script::manifest::VerifiedBundle,
    code: &str,
) -> (ScriptRuntimeHandle, Arc<dyn PolicyScript>, String) {
    let sha256 = celeris_script::manifest::sha256_hex(code.as_bytes());
    let mut source = bundle.source();
    source.name = format!("celeris:bundle/{sha256}.mjs");
    source.code = code.to_owned();
    let (handle, policy) = start_policy(source, sha256.clone());
    (handle, policy, sha256)
}

/// mode ts へは compare を経てだけ入れる（付記 S4-14。actor は人）。
fn to_ts(env: &Env, generation: String) {
    env.set_mode(ScriptMode::Compare, Some(generation.clone()));
    env.set_mode(ScriptMode::Ts, Some(generation));
    assert_eq!(env.mode(), ScriptMode::Ts);
}

fn is_ts_timeout(record: &CosDigestCompareRecord) -> bool {
    record
        .fallback
        .as_ref()
        .is_some_and(|f| f.code == "ts_error")
        && record.error.as_ref().is_some_and(|e| e.kind == "timeout")
}

/// 一時 DB の dispatcher を mode ts にし、completed と stopped の run を 1 つずつ digest する。
/// 2 回目の tick（再配送）と、作り直した dispatcher の tick（再起動）で message も記録も増えないことを確かめる。
async fn ts_digest_pass(policy: &Arc<dyn PolicyScript>, sha256: &str) -> Pass {
    let env = env();
    let generation = env.generation(sha256);
    to_ts(&env, generation.id);
    assert_eq!(env.mode(), ScriptMode::Ts);
    let mut d = env.dispatcher();
    d.set_policy_script(Some(policy.clone()));
    seed_run(&env.store, "run-a-done", ChatRunState::Completed);
    seed_run(&env.store, "run-b-stop", ChatRunState::Stopped);
    d.tick().expect("tick");
    d.wait_script_compares().await;
    let (messages, items) = world(&env);
    let mut records = log(&env.store);
    records.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    for again in [false, true] {
        if again {
            d = env.dispatcher();
            d.set_policy_script(Some(policy.clone()));
        }
        d.tick().expect("tick");
        d.wait_script_compares().await;
        assert_eq!(
            world(&env),
            (messages.clone(), items.clone()),
            "no re-apply"
        );
        assert_eq!(log(&env.store).len(), records.len(), "no new record");
    }
    Pass {
        env,
        messages,
        items,
        records,
    }
}

/// mode ts の pass を、dispatcher の 200 ms の deadline に間に合うまでやり直す（`timeout` は環境の遅さ）。
/// 記録は run ごとに 1 件で、mode は `ts`。
async fn ts_pass_until_answered(policy: &Arc<dyn PolicyScript>, sha256: &str) -> Pass {
    for _ in 0..TIMEOUT_RETRIES {
        let pass = ts_digest_pass(policy, sha256).await;
        assert_eq!(
            pass.records.len(),
            2,
            "one record per run: {:?}",
            pass.records
        );
        if pass.records.iter().any(is_ts_timeout) {
            continue;
        }
        for record in &pass.records {
            assert_eq!(record.mode, ScriptMode::Ts);
            assert_eq!(record.call_site, CALL_SITE);
            assert_eq!(record.bundle_sha256, sha256);
            assert_eq!(record.input, None, "no body in the log");
            assert!(!record.applied.message_id.is_empty());
        }
        assert_eq!(pass.records[0].pass_id, pass.records[1].pass_id, "one pass");
        assert_eq!(pass.records[0].run_id, "run-a-done");
        assert_eq!(pass.records[1].run_id, "run-b-stop");
        assert_eq!(pass.records[1].applied.requeued, 1);
        return pass;
    }
    panic!("every ts pass timed out ({TIMEOUT_RETRIES} attempts)");
}

fn op_outcomes(record: &CosDigestCompareRecord) -> Vec<(String, String)> {
    record
        .ops
        .iter()
        .map(|o| (o.op.clone(), o.outcome.clone()))
        .collect()
}

/// mode ts・本物の bundle: TS の pass が `list_runs`・`read_run` を await し（store から読む、`ok`）、
/// その command が検証の後に apply される。message・item の行は mode rust の基準と byte 一致し、
/// 記録の `applied_by` は `ts`、`fallback` は無い。印を足した bundle では DB の message が TS の文面になる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_ts_mode_applies_ts_commands() {
    let Some(bundle) = dist_bundle(
        "script_compare_cos_digest_ts_mode_applies_ts_commands",
        DOMAIN,
    ) else {
        return;
    };
    let base = baseline().await;
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256.clone());
    let policy: Arc<dyn PolicyScript> = policy;
    warm(policy.as_ref()).await;
    let before = handle.stats().completed;

    let pass = ts_pass_until_answered(&policy, &sha256).await;
    assert_eq!(
        pass.messages, base.messages,
        "TS's messages are Rust's, byte for byte"
    );
    assert_eq!(
        pass.items, base.items,
        "TS's requeue is Rust's, byte for byte"
    );
    assert!(handle.stats().completed > before, "TS ran the pass");
    for record in &pass.records {
        assert_eq!(record.applied_by, CosDigestAppliedBy::Ts, "{record:?}");
        assert_eq!(record.fallback, None, "{record:?}");
        assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
        assert!(record.safety_violations.is_empty() && record.error.is_none());
        assert_eq!(record.ts.as_ref(), Some(&record.rust));
        // TS の op は store を読んだ（mode ts の memo は store を通す）。list は pass に 1 回、read は run に 1 回。
        assert_eq!(
            op_outcomes(record),
            vec![
                ("cos_digest:list_runs".to_owned(), "ok".to_owned()),
                ("cos_digest:read_run".to_owned(), "ok".to_owned()),
            ],
            "{record:?}"
        );
    }
    assert_eq!(pass.records[0].rust.command_kinds, vec!["post_digest"]);
    assert_eq!(
        pass.records[1].rust.command_kinds,
        vec!["post_digest", "requeue_run"]
    );
    handle.shutdown();

    // TS の文面に印を足した bundle: DB の message は TS の文面（Rust の文面ではない）。requeue は同じ。
    let marked = format!("{}{MARK_WRAPPER}", bundle.code);
    let (handle, policy, sha256) = start_with_caps(&bundle, &marked);
    warm_pass(policy.as_ref()).await;
    let pass = ts_pass_until_answered(&policy, &sha256).await;
    assert_eq!(pass.items, base.items, "requeue is the same");
    assert_ne!(pass.messages, base.messages);
    assert_eq!(
        pass.messages.matches("(ts-applied)").count(),
        2,
        "{}",
        pass.messages
    );
    let texts: Vec<String> = digest_texts(&pass.env);
    assert_eq!(texts.len(), 2);
    assert!(texts.iter().all(|t| t.ends_with(TS_MARK)), "{texts:?}");
    for record in &pass.records {
        assert_eq!(record.applied_by, CosDigestAppliedBy::Ts, "{record:?}");
        assert_eq!(record.fallback, None);
        assert_eq!(record.outcome, CompareOutcome::Mismatch, "{record:?}");
        assert!(record.safety_violations.is_empty(), "{record:?}");
    }
    handle.shutdown();
}

/// digest の message の本文（run の順）。
fn digest_texts(env: &Env) -> Vec<String> {
    let (messages, _) = world(env);
    let messages: Vec<serde_json::Value> = serde_json::from_str(&messages).expect("json");
    messages
        .iter()
        .map(|m| m["text"].as_str().expect("text").to_owned())
        .collect()
}

/// 印の bundle は fixture の command と一致しないので、温めは pass を 1 回走らせるだけにする。
async fn warm_pass(policy: &dyn PolicyScript) {
    let (_, input, _) = fixture_rows("cases.jsonl")
        .into_iter()
        .next()
        .expect("a case");
    let result = one_run_pass(policy, &input).await;
    assert!(matches!(result, TsResult::Reply(_)), "{result:?}");
}

/// mode compare・本物の bundle: TS の op は Rust が apply の前に読んだ snapshot（凍結した memo）から答える。
/// store の読み出しは Rust の 1 回だけで、TS の op はすべて `memo`。TS が走る前に表の名前を変えても TS は答え、
/// 記録は `match`、`applied_by` は `rust`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_compare_answers_ops_from_rust_snapshot() {
    let Some(bundle) = dist_bundle(
        "script_compare_cos_digest_compare_answers_ops_from_rust_snapshot",
        DOMAIN,
    ) else {
        return;
    };
    let base = baseline().await;
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha256.clone());
    let policy: Arc<dyn PolicyScript> = policy;
    warm(policy.as_ref()).await;
    for _ in 0..TIMEOUT_RETRIES {
        let env = env();
        let generation = env.generation(&sha256);
        env.set_mode(ScriptMode::Compare, Some(generation.id));
        let mut d = env.dispatcher();
        d.set_policy_script(Some(policy.clone()));
        seed_run(&env.store, "run-a-done", ChatRunState::Completed);
        seed_run(&env.store, "run-b-stop", ChatRunState::Stopped);
        d.tick().expect("tick");
        // Rust の apply は済んだ。TS の pass が store を読めば、ここから先は失敗する。
        let conn = rusqlite::Connection::open(&env.config.db.path).expect("db");
        for (from, to) in [
            ("cos_inbox_items", "cos_inbox_items_hidden"),
            ("chat_runs", "chat_runs_hidden"),
        ] {
            conn.execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}"))
                .expect("rename");
        }
        d.wait_script_compares().await;
        for (from, to) in [
            ("cos_inbox_items_hidden", "cos_inbox_items"),
            ("chat_runs_hidden", "chat_runs"),
        ] {
            conn.execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}"))
                .expect("rename back");
        }
        let mut records = log(&env.store);
        records.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        assert_eq!(records.len(), 2, "{records:?}");
        if records.iter().any(is_timeout) {
            continue;
        }
        assert_eq!(world(&env), (base.messages.clone(), base.items.clone()));
        for record in &records {
            assert_eq!(record.mode, ScriptMode::Compare);
            assert_eq!(record.applied_by, CosDigestAppliedBy::Rust);
            assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
            assert_eq!(
                op_outcomes(record),
                vec![
                    ("cos_digest:list_runs".to_owned(), "memo".to_owned()),
                    ("cos_digest:read_run".to_owned(), "memo".to_owned()),
                ],
                "TS's ops are answered from Rust's snapshot: {record:?}"
            );
        }
        handle.shutdown();
        return;
    }
    panic!("every compare timed out ({TIMEOUT_RETRIES} attempts)");
}

/// mode ts で TS が使えない pass は、pass 全体で Rust の command を apply する。message・item は基準と byte 一致し、
/// 記録の `applied_by` は `rust`、`fallback.code` は `code`。
async fn ts_falls_back(
    policy: Arc<dyn PolicyScript>,
    sha256: &str,
    code: &str,
    base: &Pass,
) -> Vec<CosDigestCompareRecord> {
    let pass = ts_digest_pass(&policy, sha256).await;
    assert_eq!(pass.messages, base.messages, "{code}: messages are Rust's");
    assert_eq!(pass.items, base.items, "{code}: requeue is Rust's");
    assert_eq!(pass.records.len(), 2, "{code}: {:?}", pass.records);
    for record in &pass.records {
        assert_eq!(record.mode, ScriptMode::Ts);
        assert_eq!(
            record.applied_by,
            CosDigestAppliedBy::Rust,
            "{code}: {record:?}"
        );
        let fallback = record.fallback.as_ref().expect("fallback");
        assert_eq!(fallback.code, code, "{record:?}");
    }
    pass.records
}

/// mode ts で、常に違う答え（どの run も digest しない）・throw・op の denied・deadline 超えの bundle では
/// Rust の command が apply される。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_ts_mode_falls_back_to_rust() {
    let Some(bundle) = dist_bundle(
        "script_compare_cos_digest_ts_mode_falls_back_to_rust",
        DOMAIN,
    ) else {
        return;
    };
    let base = baseline().await;

    // 常に違う答え: 規則 8（run の欠け）。
    let (handle, policy, sha256) = start_code(DIFFERENT_BUNDLE);
    let records = ts_falls_back(policy, &sha256, "run_missing", &base).await;
    for record in &records {
        assert_eq!(record.outcome, CompareOutcome::InvalidOutput, "{record:?}");
    }
    handle.shutdown();

    // throw。
    let (handle, policy, sha256) = start_code(THROWING_BUNDLE);
    let records = ts_falls_back(policy, &sha256, "ts_error", &base).await;
    for record in &records {
        let error = record.error.as_ref().expect("error");
        if error.kind != "timeout" {
            assert_eq!(error.kind, "exception", "{record:?}");
            assert!(error.message.contains("boom in cos_digest"), "{error:?}");
        }
    }
    handle.shutdown();

    // op の denied: 一覧に無い run の read を dispatcher が拒み、TS は throw する。
    let (handle, policy, sha256) = start_with_caps(&bundle, DENIED_BUNDLE);
    let records =
        loop_until_not_timeout(|| ts_falls_back(policy.clone(), &sha256, "ts_error", &base)).await;
    for record in &records {
        assert!(
            record.safety_violations.iter().any(|v| v == "denied_scope"),
            "{record:?}"
        );
        assert!(
            record.ops.iter().any(|o| o.op == "cos_digest:list_runs"),
            "{record:?}"
        );
    }
    handle.shutdown();

    // deadline 超え: 答えない pass。dispatcher は 200 ms で待ちを切って Rust を apply する。
    let (handle, policy, sha256) = start_code(NEVER_BUNDLE);
    let records = ts_falls_back(policy, &sha256, "ts_error", &base).await;
    for record in &records {
        assert!(is_ts_timeout(record), "{record:?}");
        assert_eq!(record.ts, None);
    }
    handle.shutdown();
}

/// `timeout` は環境の遅さなので、`timeout` 以外の記録が出るまでやり直す。
async fn loop_until_not_timeout<F, Fut>(mut f: F) -> Vec<CosDigestCompareRecord>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<CosDigestCompareRecord>>,
{
    for _ in 0..TIMEOUT_RETRIES {
        let records = f().await;
        if !records.iter().any(is_ts_timeout) {
            return records;
        }
    }
    panic!("every ts pass timed out ({TIMEOUT_RETRIES} attempts)");
}

/// daemon の結線（mode ts）: 同じ bundle の daemon は ts を保ち、digest の message は冪等 key で 1 件のまま
/// （再起動で増えない）。`[script]` の無い daemon は起動時に `system:script-startup` が rust へ戻し、
/// Rust が digest して記録は増えない（付記 S4-14）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_compare_cos_digest_daemon_ts_restart_keeps_one_message() {
    if dist_bundle(
        "script_compare_cos_digest_daemon_ts_restart_keeps_one_message",
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
    let with_script = load_config(
        env.dir.path(),
        &format!("{COS_OFF}[script]\nstate_dir = \"state\"\nactive_bundle_sha256 = \"{sha256}\"\n"),
    );
    assert_eq!(run_daemon(with_script.clone()).await, Exit::MaxTicks);
    let generation = env
        .store
        .script_generation_by_sha256(&sha256)
        .expect("read")
        .expect("generation recorded at startup");
    to_ts(&env, generation.id.clone());

    seed_run(&env.store, "run-a-done", ChatRunState::Completed);
    assert_eq!(run_daemon(with_script.clone()).await, Exit::MaxTicks);
    assert_eq!(env.mode(), ScriptMode::Ts, "same bundle keeps ts");
    let records = log(&env.store);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].run_id, "run-a-done");
    assert_eq!(records[0].mode, ScriptMode::Ts);
    assert_eq!(records[0].generation, generation.id);
    // daemon の最初の呼び出しは遅い isolate で 200 ms を超えうる。そのときは Rust を apply する（それも正しい）。
    assert!(
        (records[0].applied_by == CosDigestAppliedBy::Ts && records[0].fallback.is_none())
            || is_ts_timeout(&records[0]),
        "{:?}",
        records[0]
    );
    let (first, _) = world(&env);
    assert_eq!(
        first.matches("cos-triage-digest:run-a-done").count(),
        1,
        "{first}"
    );

    // mode ts のまま再起動: message は 1 件のまま、記録も増えない。
    assert_eq!(run_daemon(with_script).await, Exit::MaxTicks);
    assert_eq!(env.mode(), ScriptMode::Ts);
    assert_eq!(world(&env).0, first, "the restart writes no second digest");
    assert_eq!(log(&env.store).len(), 1);

    // bundle の無い daemon: 起動時に rust へ戻り、Rust が新しい run を digest する。記録は増えない。
    seed_run(&env.store, "run-b-stop", ChatRunState::Stopped);
    assert_eq!(
        run_daemon(load_config(env.dir.path(), COS_OFF)).await,
        Exit::MaxTicks
    );
    assert_eq!(env.mode(), ScriptMode::Rust);
    let row = env.store.script_domain_mode(DOMAIN).expect("mode");
    assert_eq!(row.updated_by, "system:script-startup");
    let (messages, _) = world(&env);
    assert_eq!(messages.matches("cos-triage-digest:run-a-done").count(), 1);
    assert_eq!(messages.matches("cos-triage-digest:run-b-stop").count(), 1);
    assert_eq!(log(&env.store).len(), 1);
}
