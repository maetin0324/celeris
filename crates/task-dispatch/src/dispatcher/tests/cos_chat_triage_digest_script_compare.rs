//! ADR 2026-11-02-ts-control-layer-boundary 付記 S4-4: digest pass の `cos_digest` の compare。偽の `PolicyScript`
//! だけを使い、V8 は起こさない。どの場合も inbox thread の message と `cos_inbox_items` が mode `rust`・
//! PolicyScript 無しのときと byte 一致であること（apply するのは常に Rust の command）を確かめる。

use super::*;
use crate::script_policy::{
    PolicyFuture, PolicyReply, PolicyScript, PolicyScriptBundle, PolicyScriptError,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use task_core::chat::digest::{CosDigestCommand, CosDigestInput, cos_digest_decide};
use task_core::chat::{ChatMessageRole, ChatRunState};
use task_core::script_compare::{
    CompareOutcome, CosDigestCompareRecord, LaneRulesInput, NewScriptGeneration,
    RoutePriorityInput, ScriptGenerationState, ScriptMode, ScriptModeChange,
};

const SHA: &str = "cd";

fn sha() -> String {
    SHA.repeat(32)
}

/// 偽の TS の答え。
enum Reply {
    Ok(serde_json::Value),
    Err(PolicyScriptError),
    /// 返らない（dispatcher の deadline で打ち切られる）。
    Hang,
}

type Answer = dyn Fn(&CosDigestInput) -> Reply + Send + Sync;

struct DigestScript {
    sha: String,
    answer: Box<Answer>,
    calls: AtomicUsize,
}

impl DigestScript {
    fn new(answer: impl Fn(&CosDigestInput) -> Reply + Send + Sync + 'static) -> Arc<Self> {
        Self::with_sha(sha(), answer)
    }

    fn with_sha(
        sha: String,
        answer: impl Fn(&CosDigestInput) -> Reply + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            sha,
            answer: Box::new(answer),
            calls: AtomicUsize::new(0),
        })
    }
}

impl PolicyScript for DigestScript {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: self.sha.clone(),
            host_api: "1.0.0".into(),
        }
    }

    fn lane_rules(&self, _input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn route_priority(&self, _input: RoutePriorityInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn cos_digest(&self, input: CosDigestInput, _deadline: Duration) -> PolicyFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = (self.answer)(&input);
        Box::pin(async move {
            match reply {
                Reply::Ok(output) => Ok(PolicyReply {
                    output,
                    isolate_epoch: 3,
                }),
                Reply::Err(e) => Err(e),
                Reply::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(PolicyScriptError::new("exception", "woke up"))
                }
            }
        })
    }
}

/// Rust と同じ command を `{ "commands": [...] }` で返す。
fn echo(input: &CosDigestInput) -> Reply {
    Reply::Ok(serde_json::json!({ "commands": cos_digest_decide(input) }))
}

/// Rust の command を `edit` で書き換えて返す。
fn edited(
    edit: impl Fn(&CosDigestInput, &mut Vec<CosDigestCommand>) + Send + Sync + 'static,
) -> impl Fn(&CosDigestInput) -> Reply + Send + Sync + 'static {
    move |input| {
        let mut commands = cos_digest_decide(input);
        edit(input, &mut commands);
        Reply::Ok(serde_json::json!({ "commands": commands }))
    }
}

/// 試験の場面: mode・注入する script。
struct Setup {
    compare: bool,
    script: Option<Arc<DigestScript>>,
}

/// 結果: inbox thread の digest と item の状態（id・時刻を除いた直列化）と compare の記録。
struct World {
    messages: String,
    items: String,
    records: Vec<CosDigestCompareRecord>,
}

impl Fixture {
    /// `id` の run に 1 件の item を渡し、`state` で終える（completed は item を observed にする）。
    fn seed_compare_run(&self, id: &str, state: ChatRunState) {
        let now = self.d.now_utc();
        self.store
            .cos_triage_ingest_batch(
                "digest-compare-test",
                id,
                &[task_core::chat::triage::CosTriageSource {
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
        let general = self
            .store
            .cos_thread_open(
                &task_core::chat::ChatOpenThreadRequest { project_id: None },
                now,
            )
            .expect("general")
            .thread
            .id;
        let claim = self
            .store
            .cos_triage_claim(&general, id, now)
            .expect("claim")
            .expect("run");
        if state == ChatRunState::Completed {
            self.store
                .cos_triage_resolve(&claim.item_ids[0], "observed", None, Some("確認済み"), now)
                .expect("resolve");
        }
        self.store
            .chat_run_finish(id, state, None, Some("人が止めた"), now)
            .expect("finish");
    }

    fn world(&self) -> World {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
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
        let inbox = self.inbox_thread().expect("inbox thread");
        let messages: Vec<serde_json::Value> = self
            .store
            .chat_message_list(
                &inbox,
                &ChatMessageQuery {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .expect("messages")
            .items
            .into_iter()
            .filter(|m| m.role == ChatMessageRole::Assistant)
            .map(|m| {
                serde_json::json!({
                    "key": m.client_message_id,
                    "text": m.text,
                    "cards": m.cards,
                    "run_id": m.run_id,
                })
            })
            .collect();
        World {
            messages: serde_json::to_string(&messages).expect("json"),
            items: serde_json::to_string(&items).expect("json"),
            records: self
                .store
                .script_compare_log(None)
                .expect("log")
                .into_iter()
                .map(|row| row.record)
                .collect(),
        }
    }
}

/// completed と stopped の run を 1 つずつ置き、digest pass を 1 回走らせて compare を待つ。
async fn run_digest(setup: Setup) -> World {
    let mut f = fixture(false);
    let now = f.d.now_utc();
    let generation = f
        .store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha(),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["cos_digest"]}"#.into(),
                state: ScriptGenerationState::Loaded,
            },
            now,
        )
        .expect("generation");
    if setup.compare {
        f.store
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: "cos_digest".into(),
                    to_mode: ScriptMode::Compare,
                    to_generation: Some(generation.id.clone()),
                    expected_version: 0,
                    actor: "human:test".into(),
                    reason: "script_compare test".into(),
                },
                now,
            )
            .expect("mode");
    }
    f.d.set_script_domain_store(f.store.clone());
    f.d.set_policy_script(setup.script.map(|s| s as Arc<dyn PolicyScript>));
    f.seed_compare_run("run-a-done", ChatRunState::Completed);
    f.seed_compare_run("run-b-stop", ChatRunState::Stopped);
    f.tick();
    f.d.wait_script_compares().await;
    let world = f.world();
    // 再配送: もう一度 digest pass が立っても message も記録も増えない。
    f.d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .triage
        .digest_due = true;
    f.tick();
    f.d.wait_script_compares().await;
    let again = f.world();
    assert_eq!(again.messages, world.messages);
    assert_eq!(again.records.len(), world.records.len());
    world
}

async fn baseline() -> World {
    run_digest(Setup {
        compare: false,
        script: None,
    })
    .await
}

/// compare の結果を見て、message と requeue が Rust（mode rust・PolicyScript 無し）と byte 一致かを確かめる。
async fn compare_with(script: Arc<DigestScript>) -> Vec<CosDigestCompareRecord> {
    let base = baseline().await;
    assert!(base.records.is_empty());
    let world = run_digest(Setup {
        compare: true,
        script: Some(script.clone()),
    })
    .await;
    assert_eq!(world.messages, base.messages, "messages are Rust's");
    assert_eq!(world.items, base.items, "requeue is Rust's");
    // The stopped run's item left `running` (requeued, then taken by the disabled-CoS fallback).
    assert!(
        base.items.contains(r#""run-b-stop","fallback",null"#),
        "{}",
        base.items
    );
    assert_eq!(script.calls.load(Ordering::SeqCst), 2, "one call per run");
    assert_eq!(world.records.len(), 2);
    let mut records = world.records;
    records.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    for r in &records {
        assert_eq!(r.call_site, "cos_digest.digest_one");
        assert_eq!(r.input, None, "no body in the log");
        assert!(r.input_digest.starts_with("sha256:"));
        assert!(!r.applied.message_id.is_empty());
    }
    assert_eq!(records[0].run_id, "run-a-done");
    assert_eq!(records[0].rust.command_kinds, vec!["post_digest"]);
    assert_eq!(records[0].applied.requeued, 0);
    assert_eq!(
        records[1].rust.command_kinds,
        vec!["post_digest", "requeue_run"]
    );
    assert_eq!(records[1].applied.requeued, 1);
    records
}

#[tokio::test]
async fn script_compare_cos_digest_match_records_and_keeps_rust_result() {
    let records = compare_with(DigestScript::new(echo)).await;
    for r in &records {
        assert_eq!(r.outcome, CompareOutcome::Match, "{r:?}");
        assert!(r.safety_violations.is_empty());
        assert_eq!(r.ts.as_ref(), Some(&r.rust));
        assert_eq!(r.ts_commands_head, None);
    }
}

#[tokio::test]
async fn script_compare_cos_digest_mismatch_keeps_rust_text() {
    let records = compare_with(DigestScript::new(edited(|_, commands| {
        if let CosDigestCommand::PostDigest { text, .. } = &mut commands[0] {
            text.insert_str(0, "TS: ");
        }
    })))
    .await;
    for r in &records {
        assert_eq!(r.outcome, CompareOutcome::Mismatch, "{r:?}");
        assert!(r.safety_violations.is_empty());
        assert!(
            r.ts_commands_head
                .as_deref()
                .is_some_and(|h| h.starts_with("TS: ")),
            "{r:?}"
        );
        assert_ne!(
            r.ts.as_ref().map(|t| &t.commands_digest),
            Some(&r.rust.commands_digest)
        );
    }
}

#[tokio::test]
async fn script_compare_cos_digest_error_is_ts_error() {
    let records = compare_with(DigestScript::new(|_| {
        Reply::Err(PolicyScriptError::new("exception", "boom"))
    }))
    .await;
    for r in &records {
        assert_eq!(r.outcome, CompareOutcome::TsError);
        assert_eq!(r.error.as_ref().map(|e| e.kind.as_str()), Some("exception"));
        assert_eq!(r.ts, None);
    }
}

#[tokio::test(start_paused = true)]
async fn script_compare_cos_digest_timeout_is_ts_error() {
    let records = compare_with(DigestScript::new(|_| Reply::Hang)).await;
    for r in &records {
        assert_eq!(r.outcome, CompareOutcome::TsError);
        assert_eq!(r.error.as_ref().map(|e| e.kind.as_str()), Some("timeout"));
    }
}

#[tokio::test]
async fn script_compare_cos_digest_malformed_output_is_invalid_output() {
    let records = compare_with(DigestScript::new(|_| {
        Reply::Ok(serde_json::json!({ "commands": "not a list" }))
    }))
    .await;
    for r in &records {
        assert_eq!(r.outcome, CompareOutcome::InvalidOutput);
        assert!(r.safety_violations.is_empty());
        assert_eq!(r.ts, None);
    }
}

/// 検証落ちの 1 件: `run_id` の run だけ `edit` を当てる。その記録の outcome と code を返す。
async fn violation(
    edit: impl Fn(&CosDigestInput, &mut Vec<CosDigestCommand>) + Send + Sync + 'static,
) -> Vec<CosDigestCompareRecord> {
    compare_with(DigestScript::new(edited(edit))).await
}

#[tokio::test]
async fn script_compare_cos_digest_rejects_card_of_another_item() {
    let records = violation(|_, commands| {
        if let CosDigestCommand::PostDigest { cards, .. } = &mut commands[0]
            && let Some(card) = cards.first_mut()
        {
            card.id = "someone-else".into();
        }
    })
    .await;
    for r in &records {
        assert_eq!(r.outcome, CompareOutcome::InvalidOutput, "{r:?}");
        assert_eq!(r.safety_violations, vec!["card_outside_items"]);
        assert!(r.ts.is_some());
    }
}

#[tokio::test]
async fn script_compare_cos_digest_rejects_other_idempotency_key() {
    let records = violation(|_, commands| {
        if let CosDigestCommand::PostDigest {
            idempotency_key, ..
        } = &mut commands[0]
        {
            *idempotency_key = "cos-triage-digest:other".into();
        }
    })
    .await;
    for r in &records {
        assert_eq!(r.outcome, CompareOutcome::InvalidOutput);
        assert_eq!(r.safety_violations, vec!["key_mismatch"]);
    }
}

#[tokio::test]
async fn script_compare_cos_digest_rejects_requeue_of_completed_run() {
    let records = violation(|input, commands| {
        if input.run_state == "completed" {
            commands.push(CosDigestCommand::RequeueRun {
                run_id: input.run_id.clone(),
            });
        }
    })
    .await;
    assert_eq!(records[0].outcome, CompareOutcome::InvalidOutput);
    assert_eq!(records[0].safety_violations, vec!["requeue_state_invalid"]);
    assert_eq!(records[1].outcome, CompareOutcome::Match);
}

#[tokio::test]
async fn script_compare_cos_digest_mode_rust_never_calls_ts() {
    let script = DigestScript::new(echo);
    let base = baseline().await;
    let world = run_digest(Setup {
        compare: false,
        script: Some(script.clone()),
    })
    .await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 0);
    assert!(world.records.is_empty());
    assert_eq!(world.messages, base.messages);
    assert_eq!(world.items, base.items);
}

#[tokio::test]
async fn script_compare_cos_digest_without_runtime_or_with_other_bundle_is_unavailable() {
    let base = baseline().await;
    let no_runtime = run_digest(Setup {
        compare: true,
        script: None,
    })
    .await;
    let other = DigestScript::with_sha("ef".repeat(32), echo);
    let other_bundle = run_digest(Setup {
        compare: true,
        script: Some(other.clone()),
    })
    .await;
    assert_eq!(
        other.calls.load(Ordering::SeqCst),
        0,
        "bundle differs: no call"
    );
    for world in [no_runtime, other_bundle] {
        assert_eq!(world.messages, base.messages);
        assert_eq!(world.items, base.items);
        assert_eq!(world.records.len(), 2);
        for r in &world.records {
            assert_eq!(r.outcome, CompareOutcome::Unavailable);
            assert_eq!(r.ts, None);
        }
    }
}
