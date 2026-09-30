use super::*;
use task_core::{ProgressFields, ProgressKind};

fn row(id: u64, seq: u64, task_id: TaskId, ts: &str, event: Event) -> EventRow {
    EventRow {
        id,
        task_id,
        seq,
        ts: ts.to_string(),
        event,
    }
}

/// カーソルは文字列に往復でき、並びは（時刻, tie）で決まる。
#[test]
fn cursors_round_trip_and_order_by_time_then_tie() {
    let c = ConsoleCursor::new(1_700_000_000_000_000_000, "e12", 12);
    let text = c.encode();
    assert_eq!(ConsoleCursor::decode(&text), Some(c.clone()));
    assert!(ConsoleCursor::decode("nope").is_none());
    assert!(ConsoleCursor::decode("1.2").is_none());
    // 同時刻は tie の順、時刻が違えば時刻の順（`event_id` は並びに効かない）。
    let older = ConsoleCursor::new(1, "z", 9999);
    let newer = ConsoleCursor::new(2, "a", 0);
    assert!(older.order_key() < newer.order_key());
    let a = ConsoleCursor::new(2, "a", 0);
    let b = ConsoleCursor::new(2, "b", 0);
    assert!(a.order_key() < b.order_key());
    // 小数秒のある RFC 3339 も時刻として比べる（文字列比較では逆になる）。
    assert!(at_nanos("2026-09-20T01:00:00Z") < at_nanos("2026-09-20T01:00:00.5Z"));
    assert_eq!(at_nanos("読めない"), 0);
}

/// run ごとに 1 件へ束ね、件数・道具の回数・最後の `status`・始めと終わりを持つ。
#[test]
fn progress_is_grouped_per_run_with_counts_and_head_and_tail() {
    let t1 = TaskId::new();
    let t2 = TaskId::new();
    let mut rows = Vec::new();
    let mut id = 0;
    let mut push =
        |rows: &mut Vec<EventRow>, task: TaskId, run: &str, secs: u32, fields: ProgressFields| {
            id += 1;
            let ts = format!("2026-09-20T01:00:{secs:02}Z");
            rows.push(row(
                id,
                id,
                task,
                &ts,
                Event::worker_progress_with(run, format!("msg {id}"), fields),
            ));
        };
    push(
        &mut rows,
        t1,
        "r1",
        0,
        ProgressFields::of(ProgressKind::Status).with_summary("starting"),
    );
    for i in 1..=8u32 {
        push(
            &mut rows,
            t1,
            "r1",
            i,
            ProgressFields::of(ProgressKind::ToolUse)
                .with_tool("Bash")
                .with_summary(format!("cmd {i}")),
        );
    }
    // 別のタスク・別の run は別の束。間に挟まっても 1 件にまとまる。
    push(&mut rows, t2, "r2", 3, ProgressFields::default());
    push(
        &mut rows,
        t1,
        "r1",
        9,
        ProgressFields::of(ProgressKind::Status).with_summary("finishing"),
    );

    let groups = group_progress(&rows);
    assert_eq!(groups.len(), 2);
    let g = &groups[0];
    assert_eq!(g.task_id, t1);
    assert_eq!(g.run_id, "r1");
    assert_eq!(g.count, 10);
    assert_eq!(g.tool_count, 8);
    assert_eq!(g.last_status.as_deref(), Some("finishing"));
    assert_eq!(g.started_at, "2026-09-20T01:00:00Z");
    assert_eq!(g.updated_at, "2026-09-20T01:00:09Z");
    assert_eq!(g.first.len(), PROGRESS_HEAD_LINES);
    assert_eq!(g.last.len(), PROGRESS_TAIL_LINES);
    assert!(g.truncated, "10 件は頭 3 + 尻 3 に収まらない");
    assert_eq!(g.first[0].text, "starting");
    assert_eq!(g.first[0].kind, Some(ProgressKind::Status));
    assert_eq!(g.last[2].text, "finishing");
    // 構造化されていない進行は `msg` がそのまま 1 行になる。
    let g2 = &groups[1];
    assert_eq!(g2.count, 1);
    assert_eq!(g2.tool_count, 0);
    assert!(!g2.truncated);
    assert!(g2.first[0].text.starts_with("msg "));
    assert!(g2.last.is_empty());
    // `WorkerProgress` 以外は無視する。
    let other = vec![row(
        99,
        0,
        t1,
        "2026-09-20T02:00:00Z",
        Event::ApprovalRequested,
    )];
    assert!(group_progress(&other).is_empty());
}

/// ADR-0054 D2（Phase 68）: 対話 run の「育つ返事」は thinking を置き換え、text をつなげ、
/// tool_use/tool_result を順番どおり積む（先頭・末尾で切らない）。
#[test]
fn conversation_progress_replaces_thinking_appends_text_and_orders_steps() {
    let t1 = TaskId::new();
    let mut rows = Vec::new();
    let mut id = 0u64;
    let mut push = |rows: &mut Vec<EventRow>, secs: u32, fields: ProgressFields, msg: &str| {
        id += 1;
        let ts = format!("2026-09-21T01:00:{secs:02}Z");
        rows.push(row(
            id,
            id,
            t1,
            &ts,
            Event::worker_progress_with("run-1", msg.to_string(), fields),
        ));
    };
    push(
        &mut rows,
        0,
        ProgressFields::of(ProgressKind::Thinking).with_summary("考え中…"),
        "thinking",
    );
    push(
        &mut rows,
        1,
        ProgressFields::of(ProgressKind::ToolUse)
            .with_tool("celerisctl")
            .with_summary("knowledge search rust"),
        "tool",
    );
    push(
        &mut rows,
        2,
        ProgressFields::of(ProgressKind::ToolResult).with_summary("3 件"),
        "tool result",
    );
    push(
        &mut rows,
        3,
        ProgressFields::of(ProgressKind::Thinking).with_summary("まとめ中…"),
        "thinking2",
    );
    push(
        &mut rows,
        4,
        ProgressFields::of(ProgressKind::Text).with_summary("承知しま"),
        "text1",
    );
    push(
        &mut rows,
        5,
        ProgressFields::of(ProgressKind::Text).with_summary("した。"),
        "text2",
    );
    push(
        &mut rows,
        6,
        ProgressFields::of(ProgressKind::Status).with_summary("節目"),
        "status",
    );

    let groups = group_conversation_progress(&rows);
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert_eq!(g.task_id, t1);
    assert_eq!(g.run_id, "run-1");
    // thinking は最後の 1 行に置き換わる（積み上げない）。
    assert_eq!(g.thinking.as_deref(), Some("まとめ中…"));
    // text はそのまま連結される。
    assert_eq!(g.text, "承知しました。");
    // tool_use / tool_result は順番どおり積まれる（status は積まれない）。
    assert_eq!(g.steps.len(), 2);
    assert_eq!(g.steps[0].kind, ProgressKind::ToolUse);
    assert_eq!(g.steps[0].tool.as_deref(), Some("celerisctl"));
    assert_eq!(g.steps[0].text, "knowledge search rust");
    assert_eq!(g.steps[1].kind, ProgressKind::ToolResult);
    assert_eq!(g.steps[1].text, "3 件");
    assert_eq!(g.started_at, "2026-09-21T01:00:00Z");
    assert_eq!(g.updated_at, "2026-09-21T01:00:06Z");
}

/// SSE の積み上げ（`merge_conversation_reply`）は取りこぼしが無い（`merge_progress` と違い、
/// 先頭・末尾で切らない）。
#[test]
fn merge_conversation_reply_accumulates_without_truncation() {
    let t1 = TaskId::new();
    let mut acc = ConsoleReplyAccum {
        task_id: t1,
        run_id: "run-1".into(),
        started_at: "2026-09-21T01:00:00Z".into(),
        updated_at: "2026-09-21T01:00:00Z".into(),
        thinking: Some("考え中…".into()),
        steps: vec![ConsoleReplyStep {
            kind: ProgressKind::ToolUse,
            tool: Some("celerisctl".into()),
            text: "knowledge search rust".into(),
            error: false,
        }],
        text: "承知しま".into(),
    };
    let next = ConsoleReplyAccum {
        task_id: t1,
        run_id: "run-1".into(),
        started_at: "2026-09-21T01:00:05Z".into(),
        updated_at: "2026-09-21T01:00:05Z".into(),
        thinking: None,
        steps: vec![ConsoleReplyStep {
            kind: ProgressKind::ToolResult,
            tool: None,
            text: "3 件".into(),
            error: false,
        }],
        text: "した。".into(),
    };
    merge_conversation_reply(&mut acc, &next);
    assert_eq!(
        acc.thinking.as_deref(),
        Some("考え中…"),
        "空なら置き換えない"
    );
    assert_eq!(acc.text, "承知しました。");
    assert_eq!(acc.steps.len(), 2);
    assert_eq!(acc.updated_at, "2026-09-21T01:00:05Z");
}
