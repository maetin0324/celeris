//! S4 (ADR 2026-11-02-ts-control-layer-boundary S4-2/S4-8): recorded `cos_digest` traces.
//!
//! Each scenario ends a triage run through the real dispatcher path with the fake harness and
//! the recorded CoS outcomes (no LLM). Right before the digest pass the test reads the same
//! snapshot the pass reads and pairs it with `cos_digest_decide`. After the pass it checks that
//! the message and the requeue written by the dispatcher match those commands.
//!
//! The lines form `control-ts/fixtures/cos_digest/traces.jsonl`, compared byte for byte.
//! `CELERIS_UPDATE_FIXTURES=1` rewrites the file. The clock is the injected test clock and each
//! ULID is replaced by a fixed one in order of first appearance.
//!
//! `digest_snapshot` is private to the production module, so [`trace_snapshot`] repeats its
//! three store calls. The check after the pass fails if the two ever disagree.

use super::*;
use std::collections::{BTreeSet, HashMap};
use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, cos_digest_decide, validate_cos_digest_commands,
};
use task_core::chat::{ChatMessageRole, ChatPostMessageRequest, ChatSendMode};

const FAIL_SCRIPT: &str = "cat >/dev/null; echo boom >&2; exit 3";
const TRACES_ENV: &str = "CELERIS_UPDATE_FIXTURES";

fn traces_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../control-ts/fixtures/cos_digest/traces.jsonl")
}

#[derive(serde::Serialize)]
struct TraceLine<'a> {
    name: &'a str,
    input: &'a CosDigestInput,
    commands: &'a [CosDigestCommand],
}

/// One snapshot of the pass and the commands decided from it, before normalization.
struct Captured {
    input: CosDigestInput,
    commands: Vec<CosDigestCommand>,
}

/// The snapshot `digest_snapshot` reads, for every run the next pass digests.
fn trace_snapshot(f: &Fixture) -> Vec<Captured> {
    f.store
        .cos_triage_runs_without_digest(20)
        .expect("runs without digest")
        .into_iter()
        .map(|(thread, run_id, run_state)| {
            let items = f.store.cos_triage_run_report(&run_id).expect("report");
            let reason = f
                .store
                .chat_run_get(&thread, &run_id)
                .ok()
                .and_then(|run| run.reason);
            let input = CosDigestInput {
                thread,
                run_id,
                run_state,
                reason,
                items,
            };
            let commands = cos_digest_decide(&input);
            validate_cos_digest_commands(&input, &commands).expect("reference commands are valid");
            Captured { input, commands }
        })
        .collect()
}

/// The stored `(state, run_id)` of one item.
fn trace_item_row(f: &Fixture, item_id: &str) -> (String, Option<String>) {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.query_row(
        "SELECT state,run_id FROM cos_inbox_items WHERE id=?1",
        [item_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("item")
}

/// Compare what the digest pass wrote with the commands decided from the snapshot.
fn trace_verify(f: &Fixture, c: &Captured) -> Result<(), String> {
    let mut requeue = false;
    for command in &c.commands {
        match command {
            CosDigestCommand::PostDigest {
                thread,
                idempotency_key,
                text,
                cards,
                run_id,
            } => {
                let written: Vec<_> = f
                    .store
                    .chat_message_list(
                        thread,
                        &ChatMessageQuery {
                            limit: Some(200),
                            ..Default::default()
                        },
                    )
                    .map_err(|e| e.to_string())?
                    .items
                    .into_iter()
                    .filter(|m| m.client_message_id.as_deref() == Some(idempotency_key.as_str()))
                    .collect();
                let [message] = written.as_slice() else {
                    return Err(format!("{} digest messages", written.len()));
                };
                if message.role != ChatMessageRole::Assistant {
                    return Err(format!("role {:?}", message.role));
                }
                if message.text != *text {
                    return Err(format!("text differs: {:?} != {text:?}", message.text));
                }
                if message.cards != *cards {
                    return Err(format!("cards differ: {:?} != {cards:?}", message.cards));
                }
                if message.run_id.as_deref() != Some(run_id.as_str()) {
                    return Err(format!("run id differs: {:?}", message.run_id));
                }
            }
            CosDigestCommand::RequeueRun { .. } => requeue = true,
        }
    }
    for item in &c.input.items {
        let (state, run_id) = trace_item_row(f, &item.item_id);
        let requeued = state == "pending" && run_id.is_none();
        if item.state == "running" {
            if requeued != requeue {
                return Err(format!(
                    "item {} is {state} with run {run_id:?}, requeue command: {requeue}",
                    item.item_id
                ));
            }
        } else if requeued || run_id.as_deref() != Some(c.input.run_id.as_str()) {
            return Err(format!(
                "item {} was {} and is now {state} with run {run_id:?}",
                item.item_id, item.state
            ));
        }
    }
    Ok(())
}

/// Replace ULIDs by fixed ones in order of first appearance, and the temporary data directory by
/// a fixed path. The replacement keeps the length of an id.
fn trace_normalize(
    text: &str,
    dir: &str,
    scenario: usize,
    ids: &mut HashMap<String, String>,
) -> String {
    const CROCKFORD: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let text = text.replace(dir, "/celeris-trace-data");
    let chars: Vec<char> = text.chars().collect();
    let is_id = |c: char| CROCKFORD.contains(c);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let end = chars[i..]
            .iter()
            .position(|c| !is_id(*c))
            .map_or(chars.len(), |n| i + n);
        if end == i {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let token: String = chars[i..end].iter().collect();
        let boundary = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if token.len() == 26 && boundary {
            let next = ids.len() + 1;
            out.push_str(
                ids.entry(token)
                    .or_insert_with(|| format!("01TRACE{scenario:02}{next:017}")),
            );
        } else {
            out.push_str(&token);
        }
        i = end;
    }
    out
}

/// A question's `source_revision` is the wall-clock time of its task event (the test clock does
/// not reach the task store). The digest does not read it, so it becomes the item's fixed time.
fn trace_fix_revisions(input: &CosDigestInput) -> CosDigestInput {
    let mut fixed = input.clone();
    for item in &mut fixed.items {
        if item.source_revision.contains('T') {
            item.source_revision = item.created_at.clone();
        }
    }
    fixed
}

fn trace_line(name: &str, input: &CosDigestInput, commands: &[CosDigestCommand]) -> String {
    serde_json::to_string(&TraceLine {
        name,
        input,
        commands,
    })
    .expect("trace line")
}

#[derive(Default)]
struct Recorder {
    scenarios: usize,
    lines: Vec<String>,
}

impl Recorder {
    /// End the scenario: wait for its runs, snapshot, run the digest pass, verify, normalize.
    /// Launches stay off during the pass so a requeued item is not claimed again at once.
    async fn record(&mut self, f: &mut Fixture, name: &str) {
        trace_finish_runs(f).await;
        self.scenarios += 1;
        f.d.accepting_new_work = false;
        let captured = trace_snapshot(f);
        assert!(!captured.is_empty(), "{name}: nothing to digest");
        f.tick();
        let dir = f.dir.path().display().to_string();
        let mut ids = HashMap::new();
        let many = captured.len() > 1;
        for (index, c) in captured.iter().enumerate() {
            trace_verify(f, c).unwrap_or_else(|e| panic!("{name}: {e}"));
            let line_name = if many {
                format!("{name}/{}", index + 1)
            } else {
                name.to_owned()
            };
            let raw = trace_line(&line_name, &trace_fix_revisions(&c.input), &c.commands);
            let line = trace_normalize(&raw, &dir, self.scenarios, &mut ids);
            // The normalized line must replay: the fixture is what the TS side is compared with.
            let value: serde_json::Value = serde_json::from_str(&line).expect("json");
            let input: CosDigestInput =
                serde_json::from_value(value["input"].clone()).expect("input");
            let commands: Vec<CosDigestCommand> =
                serde_json::from_value(value["commands"].clone()).expect("commands");
            assert_eq!(cos_digest_decide(&input), commands, "{line_name}");
            assert_eq!(trace_line(&line_name, &input, &commands), line);
            self.lines.push(line);
        }
        // Idempotent: another pass writes nothing more.
        f.d.cos_chat_launch
            .as_mut()
            .expect("launch")
            .triage
            .digest_due = true;
        f.tick();
        assert!(trace_snapshot(f).is_empty(), "{name}: digest repeated");
        for c in &captured {
            trace_verify(f, c).unwrap_or_else(|e| panic!("{name} after a second pass: {e}"));
        }
    }
}

/// Await every live CoS run of every thread (the worker tasks only progress while awaited).
async fn trace_finish_runs(f: &mut Fixture) {
    let launch = f.d.cos_chat_launch.as_mut().expect("launch");
    let handles: Vec<_> = launch.running.drain().map(|(_, handle)| handle).collect();
    for handle in handles {
        tokio::time::timeout(std::time::Duration::from_secs(30), handle)
            .await
            .expect("run deadline")
            .expect("worker did not panic");
    }
}

/// Bring the waits in one tick apart, so `created_at` fixes the report order, then start one run
/// on all of them. Returns the item ids in report order.
fn trace_start_run(f: &mut Fixture, waits: &[&str]) -> Vec<String> {
    f.tick();
    f.d.accepting_new_work = false;
    for wait in waits {
        match wait.strip_prefix("notice:") {
            Some(key) => {
                f.notice(key, NoticeKind::Report);
            }
            None => {
                f.question(wait, vec![]);
            }
        }
        f.tick();
    }
    f.d.accepting_new_work = true;
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    trace_items(f)
}

fn trace_items(f: &Fixture) -> Vec<String> {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    let mut stmt = conn
        .prepare("SELECT id FROM cos_inbox_items ORDER BY created_at,id")
        .expect("prepare");
    stmt.query_map([], |r| r.get(0))
        .expect("items")
        .collect::<Result<_, _>>()
        .expect("items")
}

fn trace_latest_run(f: &Fixture) -> (String, String) {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.query_row(
        "SELECT thread_id,run_id FROM chat_runs ORDER BY started_at DESC,run_id DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("run")
}

/// The recorded outcome of a CoS `escalate`: the outbox packet, then the item state.
fn trace_escalate(f: &Fixture, item: &str, reason: &str, packet: serde_json::Value) {
    let now = f.d.now_utc();
    f.store
        .cos_triage_outbox_claim(item, "escalation", &packet.to_string(), now)
        .expect("claim")
        .expect("routed");
    assert!(
        f.store
            .cos_triage_resolve(item, "escalated", Some("op-escalate"), Some(reason), now)
            .expect("resolve")
    );
}

fn trace_packet() -> serde_json::Value {
    serde_json::json!({
        "summary": "公開してよいか",
        "background": "依頼「資料の公開」の task が公開先を決められず止まった",
        "on_answer": "選んだ方で task が再開する",
        "options": [{"key":"publish","label":"公開する"},{"key":"hold","label":"保留"}],
        "recommended": "hold",
        "recommendation_reason": "公開先が未確認",
        "web_path": "/tasks/recorded",
    })
}

fn trace_resolve(f: &Fixture, item: &str, outcome: &str, op: Option<&str>, reason: Option<&str>) {
    assert!(
        f.store
            .cos_triage_resolve(item, outcome, op, reason, f.d.now_utc())
            .expect("resolve")
    );
}

fn trace_stop(f: &Fixture) {
    let (thread, run) = trace_latest_run(f);
    let stop = f
        .store
        .chat_run_stop(&thread, &run, f.d.now_utc())
        .expect("stop");
    assert!(stop.accepted);
}

fn trace_interrupt(f: &Fixture) {
    let inbox = f.inbox_thread().expect("inbox thread");
    f.store
        .chat_message_post(
            &inbox,
            &ChatPostMessageRequest {
                client_message_id: "h-interrupt".into(),
                text: "(b) にして".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Interrupt,
                resume_queue: false,
            },
            f.d.now_utc(),
        )
        .expect("post");
}

/// All scenarios in file order. One fixture per scenario; ticks are counted, so times are fixed.
async fn trace_scenarios() -> Vec<String> {
    let mut out = Recorder::default();

    // completed: CoS escalated with a decision packet.
    let mut f = fixture(true);
    let items = trace_start_run(&mut f, &["deploy"]);
    trace_escalate(&f, &items[0], "外部公開なので人の判断", trace_packet());
    out.record(&mut f, "completed/escalated-packet").await;

    // completed: the packet carries CoS's text for each option.
    let mut f = fixture(true);
    let items = trace_start_run(&mut f, &["deploy"]);
    trace_escalate(
        &f,
        &items[0],
        "どちらも戻せない",
        serde_json::json!({
            "summary": "どちらの host に入れるか",
            "options": [
                {"key":"a","label":"host A","detail":"既存の job が止まる"},
                {"key":"b","label":"host B"},
            ],
            "recommended": "a",
            "web_path": "/tasks/recorded",
        }),
    );
    out.record(&mut f, "completed/escalated-option-details")
        .await;

    // completed: CoS did not touch the item; the fallback of the same tick takes it.
    let mut f = fixture(true);
    trace_start_run(&mut f, &["deploy"]);
    out.record(&mut f, "completed/running-unhandled").await;

    // completed: two handled items, a third left running.
    let mut f = fixture(true);
    let items = trace_start_run(&mut f, &["deploy", "notice:report:1", "publish"]);
    trace_resolve(
        &f,
        &items[0],
        "answered",
        Some("op-answer"),
        Some("方針どおり"),
    );
    trace_resolve(&f, &items[1], "observed", None, Some("知らせだけ"));
    out.record(&mut f, "completed/answered-observed-running")
        .await;

    // completed: the unavailable fallback routed the item while the run was still live.
    let mut f = fixture(true);
    trace_start_run(&mut f, &["deploy"]);
    f.advance(130);
    f.tick();
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='fallback'"),
        1
    );
    out.record(&mut f, "completed/fallback-while-live").await;

    // completed: the wait closed while the run was live.
    let mut f = fixture(true);
    f.tick();
    let task = f.question("deploy", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    f.store
        .apply_transition_with_events(
            task,
            Trigger::Answer,
            vec![Event::Answered {
                by: None,
                question: "deploy?".into(),
                answer: "A".into(),
            }],
        )
        .expect("answer");
    f.tick();
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='resolved'"),
        1
    );
    out.record(&mut f, "completed/resolved-source-closed").await;

    // completed: a stored state the digest does not know (`handled`, as in the cases fixture).
    // The table's CHECK does not list it, so this one row bypasses the CHECK on purpose.
    let mut f = fixture(true);
    let items = trace_start_run(&mut f, &["deploy"]);
    trace_finish_runs(&mut f).await;
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.pragma_update(None, "ignore_check_constraints", true)
        .expect("pragma");
    conn.execute(
        "UPDATE cos_inbox_items SET state='handled',reason='CoS が処理した' WHERE id=?1",
        [&items[0]],
    )
    .expect("handled");
    drop(conn);
    out.record(&mut f, "completed/handled-unknown-state").await;

    // failed: the harness exits non-zero with the item unhandled.
    let mut f = fixture_script(true, FAIL_SCRIPT);
    trace_start_run(&mut f, &["deploy"]);
    out.record(&mut f, "failed/running-unhandled").await;

    // failed: CoS escalated one item, then the run failed.
    let mut f = fixture_script(true, FAIL_SCRIPT);
    let items = trace_start_run(&mut f, &["deploy", "publish"]);
    trace_escalate(&f, &items[0], "人の判断", trace_packet());
    out.record(&mut f, "failed/escalated-and-running").await;

    // failed: the unavailable fallback routed the item while the run was still live.
    let mut f = fixture_script(true, FAIL_SCRIPT);
    trace_start_run(&mut f, &["deploy"]);
    f.advance(130);
    f.tick();
    out.record(&mut f, "failed/fallback-while-live").await;

    // stopped: a person stopped the run; the item goes back to the queue.
    let mut f = fixture(true);
    trace_start_run(&mut f, &["deploy"]);
    trace_stop(&f);
    out.record(&mut f, "stopped/running-requeued").await;

    // stopped: only the item still running goes back.
    let mut f = fixture(true);
    let items = trace_start_run(&mut f, &["deploy", "publish", "notice:report:1"]);
    trace_escalate(&f, &items[0], "人の判断", trace_packet());
    trace_resolve(&f, &items[2], "observed", None, Some("知らせだけ"));
    trace_stop(&f);
    out.record(&mut f, "stopped/escalated-observed-running")
        .await;

    // interrupted: a human message interrupted the run before it polled.
    let mut f = fixture(true);
    trace_start_run(&mut f, &["deploy"]);
    trace_interrupt(&f);
    out.record(&mut f, "interrupted/running-requeued").await;

    // interrupted: handled items stay, the running one goes back.
    let mut f = fixture(true);
    let items = trace_start_run(&mut f, &["deploy", "publish"]);
    trace_resolve(
        &f,
        &items[0],
        "answered",
        Some("op-answer"),
        Some("方針どおり"),
    );
    trace_interrupt(&f);
    out.record(&mut f, "interrupted/answered-running").await;

    // interrupted: an escalated item with the fallback of a second one.
    let mut f = fixture(true);
    let items = trace_start_run(&mut f, &["deploy", "publish"]);
    trace_escalate(&f, &items[0], "人の判断", trace_packet());
    f.advance(130);
    f.tick();
    trace_interrupt(&f);
    out.record(&mut f, "interrupted/escalated-fallback").await;

    // Two threads in one pass: a run of the general CoS and a run of a project CoS.
    let mut f = fixture(true);
    f.d.config.execution.max_cos_runs = 2;
    f.tick();
    f.question("general wait", vec![]);
    f.tick();
    crate::test_support::project_cos_thread(&f.store, "案件A");
    let project: task_core::ProjectId = f
        .count_text("SELECT project_id FROM chat_threads WHERE kind='project'")
        .parse()
        .expect("project id");
    f.question_in("project wait", Some(project), None, vec![]);
    f.tick();
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_runs"), 2);
    out.record(&mut f, "completed/two-threads").await;

    out.lines
}

#[tokio::test]
async fn cos_digest_trace_matches_committed_fixture() {
    let lines = trace_scenarios().await;
    assert!(lines.len() >= 10, "{} traces", lines.len());

    let values: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let names: BTreeSet<&str> = values
        .iter()
        .map(|v| v["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names.len(), lines.len(), "names are unique");
    let run_states: BTreeSet<&str> = values
        .iter()
        .map(|v| v["input"]["run_state"].as_str().expect("run state"))
        .collect();
    for state in ["completed", "failed", "stopped", "interrupted"] {
        assert!(
            run_states.contains(state),
            "run state {state}: {run_states:?}"
        );
    }
    let item_states: BTreeSet<&str> = values
        .iter()
        .flat_map(|v| v["input"]["items"].as_array().expect("items"))
        .map(|i| i["state"].as_str().expect("state"))
        .collect();
    for state in [
        "escalated",
        "fallback",
        "running",
        "handled",
        "answered",
        "observed",
        "resolved",
    ] {
        assert!(
            item_states.contains(state),
            "item state {state}: {item_states:?}"
        );
    }

    let generated = lines.join("\n") + "\n";
    let path = traces_path();
    if std::env::var(TRACES_ENV).as_deref() == Ok("1") {
        std::fs::write(&path, &generated).expect("write traces");
    }
    let committed = std::fs::read_to_string(&path).expect("read traces");
    assert!(
        committed == generated,
        "traces.jsonl differs from the generated traces; rewrite with {TRACES_ENV}=1\n\
         first difference at line {:?}",
        committed
            .lines()
            .zip(generated.lines())
            .position(|(a, b)| a != b)
    );
}

/// The check after the pass catches a command that differs from what the dispatcher wrote.
#[tokio::test]
async fn cos_digest_trace_verify_rejects_commands_that_differ_from_the_writes() {
    let mut f = fixture(true);
    trace_start_run(&mut f, &["deploy"]);
    trace_stop(&f);
    trace_finish_runs(&mut f).await;
    f.d.accepting_new_work = false;
    let mut captured = trace_snapshot(&f);
    let mut c = captured.pop().expect("one run");
    assert_eq!(c.input.run_state, "stopped");
    f.tick();
    trace_verify(&f, &c).expect("the real commands match");

    let original = c.commands.clone();
    // A different text.
    if let CosDigestCommand::PostDigest { text, .. } = &mut c.commands[0] {
        text.push('x');
    }
    assert!(
        trace_verify(&f, &c)
            .expect_err("text")
            .contains("text differs")
    );
    // Different cards.
    c.commands = original.clone();
    if let CosDigestCommand::PostDigest { cards, .. } = &mut c.commands[0] {
        cards.clear();
    }
    assert!(
        trace_verify(&f, &c)
            .expect_err("cards")
            .contains("cards differ")
    );
    // The items were requeued although no command said so.
    c.commands = original;
    c.commands.truncate(1);
    assert!(
        trace_verify(&f, &c)
            .expect_err("requeue")
            .contains("requeue command")
    );
}

#[test]
fn cos_digest_trace_normalize_fixes_ids_in_first_seen_order() {
    let a = "01M6EVP9GT07K2G01EMW0E6HNC";
    let b = "01M6EYH4MAHQPXZZ221BVBTG2E";
    let mut ids = HashMap::new();
    let text = format!("/data/x {b}/{a} question-{b} X{a} {b}");
    let out = trace_normalize(&text, "/data/x", 7, &mut ids);
    let first = "01TRACE0700000000000000001";
    let second = "01TRACE0700000000000000002";
    assert_eq!(first.len(), 26);
    assert_eq!(
        out,
        format!("/celeris-trace-data {first}/{second} question-{first} X{a} {first}")
    );
}
