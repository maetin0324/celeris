//! ADR 2026-10-05 D2: `ChatRunSink` の写像を一時 SQLite の `chat_events`（SSE の replay と同じ並び）で固定する。
//! 時計は固定値を注入し、sleep もプロセスも使わない。

use std::sync::Arc;

use serde_json::json;
use task_core::chat::{
    ChatCardKind, ChatCreateThreadRequest, ChatEvent, ChatEventData, ChatEventQuery, ChatEventType,
    ChatMessageRole, ChatPostMessageRequest, ChatRunState, ChatSendMode, ChatStatusPhase,
    ChatToolState,
};
use task_core::{BudgetKind, ProgressFields, ProgressKind, SqliteStore};
use task_worker::adapter::{AdapterError, EventSink, RunOutcome, Terminal};
use task_worker::result_report::{ParsedActions, actions_from_result_json};
use time::OffsetDateTime;

use super::*;

const RUN: &str = "run-1";

fn at() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400).expect("valid ts")
}

struct Fixture {
    _dir: crate::test_support::WritableTempDir,
    store: Arc<SqliteStore>,
    thread: String,
    cursor: String,
}

/// 一時 SQLite に thread を作り、1 件の発言を `RUN` として claim した状態。`cursor` は claim 後の末尾。
fn fixture() -> Fixture {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("store"));
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "t".into(),
                project_id: None,
                client_thread_id: "c-thread".into(),
            },
            at(),
        )
        .expect("thread")
        .thread
        .id;
    store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "c-1".into(),
                text: "直して".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            at(),
        )
        .expect("post");
    store
        .chat_run_claim_next(&thread, RUN, &json!({"harness": "fake"}), at())
        .expect("claim")
        .expect("claimed");
    let cursor = all_events(&store, &thread, None)
        .last()
        .map(|e| e.id.clone())
        .unwrap_or_else(|| "0".into());
    Fixture {
        _dir: dir,
        store,
        thread,
        cursor,
    }
}

fn all_events(store: &SqliteStore, thread: &str, after: Option<&str>) -> Vec<ChatEvent> {
    store
        .chat_events_page(
            thread,
            &ChatEventQuery {
                after: after.map(str::to_string),
                run_id: None,
                limit: Some(500),
            },
        )
        .expect("events")
        .items
}

impl Fixture {
    fn sink(&self, secrets: Vec<String>) -> ChatRunSink {
        ChatRunSink::new(
            Arc::clone(&self.store),
            self.thread.clone(),
            RUN,
            Arc::new(at),
            secrets,
        )
    }

    /// claim 後に増えた events（SSE が `after=<cursor>` で流すのと同じ並び）。
    fn events(&self) -> Vec<ChatEvent> {
        all_events(&self.store, &self.thread, Some(&self.cursor))
    }
}

fn kinds(events: &[ChatEvent]) -> Vec<ChatEventType> {
    events.iter().map(|e| e.event_type).collect()
}

fn text(body: &str) -> ProgressFields {
    ProgressFields::of(ProgressKind::Text).with_detail(body)
}

fn tool_use(name: &str, summary: &str, detail: &str) -> ProgressFields {
    ProgressFields::of(ProgressKind::ToolUse)
        .with_tool(name)
        .with_summary(summary)
        .with_detail(detail)
}

fn tool_result(name: Option<&str>, body: &str, error: bool) -> ProgressFields {
    let f = ProgressFields::of(ProgressKind::ToolResult)
        .with_summary(body)
        .with_detail(body)
        .with_error(error);
    match name {
        Some(n) => f.with_tool(n),
        None => f,
    }
}

fn done(summary: &str) -> Result<RunOutcome, AdapterError> {
    Ok(RunOutcome {
        terminal: Terminal::Done {
            summary: summary.into(),
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    })
}

fn tool_of(e: &ChatEvent) -> &task_core::chat::ChatToolData {
    match &e.data {
        ChatEventData::Tool(t) => t,
        other => panic!("not a tool event: {other:?}"),
    }
}

#[test]
fn cos_chat_run_sink_maps_text_tool_thinking_status_in_order() {
    let fx = fixture();
    let sink = fx.sink(vec![]);
    sink.progress_with(
        "starting",
        &ProgressFields::of(ProgressKind::Status).with_summary("起動した"),
    );
    // thinking は公開要約だけ。本文（detail）は流さない。
    let mut thinking = ProgressFields::of(ProgressKind::Thinking).with_summary("方針を考える");
    thinking.detail = Some("SECRET-THOUGHT-BODY".into());
    sink.progress_with("thinking", &thinking);
    sink.progress_with("t", &text("こんにちは"));
    sink.heartbeat();
    sink.progress_with("u", &tool_use("Bash", "ls", "{\"command\":\"ls\"}"));
    sink.progress_with("r", &tool_result(Some("Bash"), "a.txt", false));
    sink.progress_with("t", &text("世界"));
    let run = sink
        .finish(
            &chat_finish_for(&done("こんにちは世界。"), ChatStopIntent::None),
            &ParsedActions::default(),
        )
        .expect("finish")
        .expect("finished now");
    assert_eq!(run.state, ChatRunState::Completed);

    let events = fx.events();
    assert_eq!(
        kinds(&events),
        vec![
            ChatEventType::Status,
            ChatEventType::Status,
            ChatEventType::TextDelta,
            ChatEventType::Tool,
            ChatEventType::Tool,
            ChatEventType::TextDelta,
            // 終端: 入力・出力の message、run、queue の順。
            ChatEventType::Message,
            ChatEventType::Message,
            ChatEventType::Run,
            ChatEventType::Queue,
        ]
    );
    match (&events[0].data, &events[1].data) {
        (ChatEventData::Status(a), ChatEventData::Status(b)) => {
            assert_eq!(a.phase, ChatStatusPhase::Working);
            assert_eq!(a.summary, "起動した");
            assert_eq!(b.phase, ChatStatusPhase::Thinking);
            assert_eq!(b.summary, "方針を考える");
        }
        other => panic!("unexpected {other:?}"),
    }
    // text_delta の offset は追記前の UTF-8 byte 長。
    let deltas: Vec<(u64, String)> = events
        .iter()
        .filter_map(|e| match &e.data {
            ChatEventData::TextDelta(d) => Some((d.offset, d.text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        vec![
            (0, "こんにちは".to_string()),
            ("こんにちは".len() as u64, "世界".to_string())
        ]
    );
    let (running, completed) = (tool_of(&events[3]), tool_of(&events[4]));
    assert_eq!(running.call_id, completed.call_id);
    assert_eq!(running.name, "Bash");
    assert_eq!(running.state, ChatToolState::Running);
    assert_eq!(completed.state, ChatToolState::Completed);
    assert_eq!(completed.detail.as_deref(), Some("a.txt"));
    // 全文が最終値になり、思考の本文はどの event にも出ない。
    match &events[7].data {
        ChatEventData::Message(m) => {
            assert_eq!(m.message.role, ChatMessageRole::Assistant);
            assert_eq!(m.message.text, "こんにちは世界。");
        }
        other => panic!("unexpected {other:?}"),
    }
    let all = serde_json::to_string(&events).expect("json");
    assert!(!all.contains("SECRET-THOUGHT-BODY"));
}

#[test]
fn cos_chat_run_sink_pairs_tool_calls_and_fails_open_calls_at_end() {
    let fx = fixture();
    let sink = fx.sink(vec![]);
    sink.progress_with("u", &tool_use("Read", "a.rs", "{}"));
    sink.progress_with("u", &tool_use("Bash", "cargo test", "{}"));
    sink.progress_with("u", &tool_use("Grep", "fn main", "{}"));
    // 名前付きの結果は同じ名前の call を閉じ、名前の無い結果は最も古い未完の call を閉じる。
    sink.progress_with("r", &tool_result(Some("Bash"), "failed", true));
    sink.progress_with("r", &tool_result(None, "ok", false));
    sink.finish(
        &chat_finish_for(&done("done"), ChatStopIntent::None),
        &ParsedActions::default(),
    )
    .expect("finish");

    let tools: Vec<_> = fx
        .events()
        .iter()
        .filter(|e| e.event_type == ChatEventType::Tool)
        .map(|e| {
            let t = tool_of(e);
            (t.call_id.clone(), t.name.clone(), t.state, t.error)
        })
        .collect();
    assert_eq!(
        tools,
        vec![
            (
                "call-1".into(),
                "Read".into(),
                ChatToolState::Running,
                false
            ),
            (
                "call-2".into(),
                "Bash".into(),
                ChatToolState::Running,
                false
            ),
            (
                "call-3".into(),
                "Grep".into(),
                ChatToolState::Running,
                false
            ),
            ("call-2".into(), "Bash".into(), ChatToolState::Failed, true),
            (
                "call-1".into(),
                "Read".into(),
                ChatToolState::Completed,
                false
            ),
            // 結果の来なかった call は終端の前に failed になる。
            ("call-3".into(), "Grep".into(), ChatToolState::Failed, true),
        ]
    );
}

#[test]
fn cos_chat_run_sink_redacts_tool_detail_and_caps_it() {
    let fx = fixture();
    let credential = "cosrun_7f3a9b2c";
    let sink = fx.sink(vec![credential.to_string()]);
    let detail = format!(
        "curl -s http://127.0.0.1/api\nAuthorization: Bearer abcdef\nexport CELERIS_TOKEN_VALUE={credential}x\nuse {credential} here\nok line"
    );
    sink.progress_with(
        "u",
        &tool_use("Bash", &format!("run with {credential}"), &detail),
    );
    // 4 KiB を超える detail は redact 後に切られ、truncated が付く。
    let long = "あいう ".repeat(2000);
    sink.progress_with("r", &tool_result(Some("Bash"), &long, false));

    let events = fx.events();
    let all = serde_json::to_string(&events).expect("json");
    assert!(!all.contains(credential), "credential leaked: {all}");
    assert!(!all.contains("abcdef"), "bearer leaked: {all}");
    let first = tool_of(&events[0]);
    let lines: Vec<&str> = first
        .detail
        .as_deref()
        .expect("detail")
        .split('\n')
        .collect();
    assert_eq!(lines[0], "curl -s http://127.0.0.1/api");
    assert_eq!(lines[1], "[redacted]");
    assert_eq!(lines[2], "[redacted]");
    assert_eq!(lines[3], "use [redacted] here");
    assert_eq!(lines[4], "ok line");
    assert_eq!(first.summary, "run with [redacted]");
    let second = tool_of(&events[1]);
    assert!(second.truncated);
    assert!(
        second.detail.as_deref().expect("detail").len()
            <= task_core::chat::CHAT_TOOL_DETAIL_MAX_BYTES
    );
}

#[test]
fn cos_chat_run_sink_sends_nothing_after_terminal() {
    let fx = fixture();
    let sink = fx.sink(vec![]);
    sink.progress_with("t", &text("途中"));
    sink.finish(
        &chat_finish_for(&done("最終"), ChatStopIntent::None),
        &ParsedActions::default(),
    )
    .expect("finish");
    let before = fx.events().len();
    sink.progress_with("t", &text("遅れた差分"));
    sink.progress_with("u", &tool_use("Bash", "ls", "{}"));
    sink.progress("late status");
    assert_eq!(fx.events().len(), before);
    // 2 回目の終端は何もしない。
    assert!(
        sink.finish(
            &chat_finish_for(&done("x"), ChatStopIntent::None),
            &ParsedActions::default()
        )
        .expect("finish again")
        .is_none()
    );
    assert_eq!(fx.events().len(), before);
}

#[test]
fn cos_chat_run_sink_stops_writing_when_run_was_finished_elsewhere() {
    let fx = fixture();
    let sink = fx.sink(vec![]);
    sink.progress_with("t", &text("前半"));
    fx.store
        .chat_run_stop(&fx.thread, RUN, at())
        .expect("stop requested");
    fx.store
        .chat_run_finish(RUN, ChatRunState::Stopped, None, None, at())
        .expect("finished by control");
    let before = fx.events().len();
    sink.progress_with("t", &text("後半"));
    assert!(sink.is_finished());
    sink.progress_with("t", &text("さらに後"));
    assert_eq!(fx.events().len(), before);
    assert!(
        sink.finish(
            &chat_finish_for(&done("x"), ChatStopIntent::Stop),
            &ParsedActions::default()
        )
        .expect("finish")
        .is_none()
    );
    let run = fx.store.chat_run_get(&fx.thread, RUN).expect("run");
    assert_eq!(run.state, ChatRunState::Stopped);
}

#[test]
fn cos_chat_run_sink_actions_become_error_card_not_executed() {
    let fx = fixture();
    let sink = fx.sink(vec![]);
    let actions = actions_from_result_json(
        r#"{"summary":"s","actions":[{"type":"ask_human","text":"q"},{"type":"bogus"}]}"#,
    );
    assert_eq!(actions.valid.len(), 1);
    let run = sink
        .finish(
            &chat_finish_for(&done("返事"), ChatStopIntent::None),
            &actions,
        )
        .expect("finish")
        .expect("finished");
    assert_eq!(run.state, ChatRunState::Completed);

    let events = fx.events();
    // card の message は run の終端より前に出る。
    let card_pos = events
        .iter()
        .position(|e| match &e.data {
            ChatEventData::Message(m) => !m.message.cards.is_empty(),
            _ => false,
        })
        .expect("card message");
    let run_pos = events
        .iter()
        .position(|e| e.event_type == ChatEventType::Run)
        .expect("run event");
    assert!(card_pos < run_pos);
    let ChatEventData::Message(m) = &events[card_pos].data else {
        panic!("not a message");
    };
    assert_eq!(m.message.role, ChatMessageRole::System);
    let card = &m.message.cards[0];
    assert_eq!(card.kind, ChatCardKind::Notice);
    assert_eq!(card.state, "error");
    let reason = card.reason.as_deref().expect("reason");
    assert!(reason.contains("result.actions を実行しない"), "{reason}");
    assert!(reason.contains("2 件"), "{reason}");
    assert!(reason.contains("ask_human"), "{reason}");
    // 何も起票されていない（actions は実行していない）。
    assert!(
        task_core::TaskStore::list(fx.store.as_ref(), None)
            .expect("tasks")
            .is_empty()
    );
}

#[test]
fn cos_chat_run_sink_records_session_signals_without_events() {
    let fx = fixture();
    let sink = fx.sink(vec![]);
    sink.heartbeat();
    sink.session_established("sess-1");
    sink.session_resume_failed("No conversation found");
    assert!(fx.events().is_empty(), "heartbeat/session add no events");
    sink.context_compacted();
    let finish = chat_finish_for(
        &Ok(RunOutcome {
            terminal: Terminal::BudgetExhausted {
                kind: BudgetKind::Context,
                message: "context full".into(),
                usage: None,
            },
            exit_code: None,
        }),
        ChatStopIntent::None,
    );
    assert!(finish.context_exhausted);
    let run = sink
        .finish(&finish, &ParsedActions::default())
        .expect("finish")
        .expect("finished");
    assert_eq!(run.state, ChatRunState::Interrupted);
    assert_eq!(
        sink.signals(),
        ChatSessionSignals {
            established: Some("sess-1".into()),
            resume_failed: Some("No conversation found".into()),
            compacted: 1,
            context_exhausted: true,
        }
    );
}

#[test]
fn cos_chat_run_sink_terminal_mapping() {
    let err = |message: &str| {
        Ok(RunOutcome {
            terminal: Terminal::Error {
                message: message.into(),
                retryable: true,
            },
            exit_code: Some(1),
        })
    };
    let f = chat_finish_for(&done("全文"), ChatStopIntent::None);
    assert_eq!(
        (f.state, f.final_text.as_deref()),
        (ChatRunState::Completed, Some("全文"))
    );
    let f = chat_finish_for(&done("  "), ChatStopIntent::None);
    assert_eq!((f.state, f.final_text), (ChatRunState::Completed, None));
    let f = chat_finish_for(&done("全文"), ChatStopIntent::Stop);
    assert_eq!((f.state, f.final_text), (ChatRunState::Stopped, None));
    let f = chat_finish_for(&done("全文"), ChatStopIntent::Interrupt);
    assert_eq!(f.state, ChatRunState::Interrupted);
    let f = chat_finish_for(&err("boom"), ChatStopIntent::None);
    assert_eq!(f.state, ChatRunState::Failed);
    assert!(f.reason.as_deref().is_some_and(|r| r.contains("boom")));
    let f = chat_finish_for(
        &Err(AdapterError::Other("spawn".into())),
        ChatStopIntent::None,
    );
    assert_eq!(f.state, ChatRunState::Failed);
    let f = chat_finish_for(
        &Ok(RunOutcome {
            terminal: Terminal::Question {
                text: "どれ?".into(),
            },
            exit_code: Some(0),
        }),
        ChatStopIntent::None,
    );
    assert_eq!(
        (f.state, f.final_text.as_deref()),
        (ChatRunState::Completed, Some("どれ?"))
    );
}

#[test]
fn cos_chat_usage_fixed_clock_skill_count_latency_retry_and_terminal_event() {
    let f = fixture();
    let sink = ChatRunSink::new(
        f.store.clone(),
        &f.thread,
        RUN,
        Arc::new(|| at() + time::Duration::seconds(2)),
        vec![],
    );
    for (tool, input) in [
        ("Skill", json!({"skill":"cos-operator"})),
        (
            "Read",
            json!({"file_path":"/long/.claude/skills/cos/SKILL.md"}),
        ),
        ("Read", json!({"path":".claude/skills/cos/./SKILL.md"})),
        ("Read", json!({"file_path":".claude/skills/../../SKILL.md"})),
        ("Read", json!({"file_path":"docs/SKILL.md"})),
        ("Read", json!({"file_path":".claude/skills/cos/README.md"})),
        ("Bash", json!({"command":"cat .claude/skills/cos/SKILL.md"})),
    ] {
        sink.progress_with(
            "",
            &ProgressFields::of(ProgressKind::ToolUse)
                .with_tool(tool)
                .with_detail(input.to_string()),
        );
        sink.progress_with(
            "",
            &ProgressFields::of(ProgressKind::ToolResult).with_tool(tool),
        );
    }
    sink.progress_with("", &ProgressFields::of(ProgressKind::Text).with_detail(""));
    sink.progress_with(
        "",
        &ProgressFields::of(ProgressKind::Thinking).with_detail("thinking"),
    );
    sink.progress_with(
        "",
        &ProgressFields::of(ProgressKind::Text).with_detail("answer"),
    );
    sink.record_usage(Some(task_core::Usage {
        input_tokens: Some(10),
        output_tokens: Some(5),
        cost_usd: Some(0.01),
        cache_read_tokens: Some(8),
        cache_creation_tokens: Some(3),
        duplicate_reads: Some(1),
        session_resumed: Some(true),
    }));
    let retry = ChatRunSink::new(
        f.store.clone(),
        &f.thread,
        RUN,
        Arc::new(|| at() + time::Duration::seconds(5)),
        vec![],
    );
    retry.inherit_telemetry(&sink);
    retry.record_usage(Some(task_core::Usage {
        input_tokens: Some(20),
        output_tokens: Some(2),
        cost_usd: Some(0.02),
        cache_read_tokens: Some(4),
        cache_creation_tokens: Some(1),
        duplicate_reads: Some(2),
        session_resumed: Some(false),
    }));
    let finish = ChatFinish {
        state: ChatRunState::Completed,
        final_text: None,
        reason: None,
        context_exhausted: false,
    };
    let run = retry
        .finish(&finish, &ParsedActions::default())
        .expect("finish")
        .expect("run");
    assert_eq!(run.skill_reads, Some(3));
    assert_eq!(run.latency_ms, Some(5000));
    assert_eq!(run.time_to_first_output_ms, Some(2000));
    let usage = run.usage.as_deref().expect("usage");
    assert_eq!(usage.input_tokens, Some(30));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.cache_read_tokens, Some(12));
    assert_eq!(usage.cache_creation_tokens, Some(4));
    assert_eq!(usage.cost_usd, Some(0.03));
    assert_eq!(usage.duplicate_reads, Some(3));
    assert_eq!(usage.session_resumed, Some(false));
    assert!(
        f.events()
            .iter()
            .any(|e| matches!(&e.data, ChatEventData::Run(data) if data.run == run))
    );
    retry.progress_with(
        "",
        &ProgressFields::of(ProgressKind::ToolUse).with_tool("Skill"),
    );
    assert!(
        retry
            .finish(&finish, &ParsedActions::default())
            .expect("repeat")
            .is_none()
    );
    assert_eq!(f.store.chat_run_get(&f.thread, RUN).expect("get"), run);
}

#[test]
fn cos_chat_usage_retry_keeps_unreported_fields_unknown() {
    let f = fixture();
    let sink = f.sink(vec![]);
    sink.record_usage(Some(task_core::Usage {
        input_tokens: Some(4),
        output_tokens: Some(1),
        cache_read_tokens: Some(12),
        cost_usd: Some(0.01),
        ..Default::default()
    }));
    sink.record_usage(Some(task_core::Usage {
        input_tokens: Some(5),
        output_tokens: Some(2),
        ..Default::default()
    }));
    let run = sink
        .finish(
            &ChatFinish {
                state: ChatRunState::Interrupted,
                final_text: None,
                reason: None,
                context_exhausted: false,
            },
            &ParsedActions::default(),
        )
        .expect("finish")
        .expect("run");
    let usage = run.usage.as_deref().expect("usage");
    assert_eq!(usage.input_tokens, Some(9));
    assert_eq!(usage.output_tokens, Some(3));
    assert_eq!(usage.cache_read_tokens, None);
    assert_eq!(usage.cost_usd, None);
    assert_eq!(usage.session_resumed, None);
}
