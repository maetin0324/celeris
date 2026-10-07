//! ADR 2026-10-07-cos-inbox-thread-conversation: the inbox thread as the place to talk about
//! inbox items. The digest of a triage run, the hand-off line, the requeue of an interrupted
//! run and the ordering of a human message against triage. Injected clock, joined handles.

use super::*;
use task_core::chat::{
    ChatActor, ChatCardKind, ChatMessage, ChatMessageRole, ChatPostMessageRequest, ChatSendMode,
};

impl Fixture {
    fn single_item(&self) -> String {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row("SELECT id FROM cos_inbox_items", [], |r| r.get(0))
            .expect("one item")
    }

    fn item_state(&self, id: &str) -> String {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row(
            "SELECT state FROM cos_inbox_items WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .expect("state")
    }

    fn inbox_messages(&self) -> Vec<ChatMessage> {
        let inbox = self.inbox_thread().expect("inbox thread");
        self.store
            .chat_message_list(
                &inbox,
                &ChatMessageQuery {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .expect("messages")
            .items
    }

    fn digests(&self) -> Vec<ChatMessage> {
        self.inbox_messages()
            .into_iter()
            .filter(|m| {
                m.role == ChatMessageRole::Assistant
                    && m.client_message_id
                        .as_deref()
                        .is_some_and(|k| k.starts_with("cos-triage-digest:"))
            })
            .collect()
    }

    fn escalate(&self, item: &str, reason: &str) {
        let body = serde_json::json!({
            "summary": "公開してよいか",
            "options": [{"key":"publish","label":"公開する"},{"key":"hold","label":"保留"}],
            "recommended": "hold",
            "recommendation_reason": "公開先が未確認",
            "web_path": "/tasks/t1",
        });
        let now = self.d.now_utc();
        self.store
            .cos_triage_outbox_claim(item, "escalation", &body.to_string(), now)
            .expect("claim")
            .expect("routed");
        assert!(
            self.store
                .cos_triage_resolve(item, "escalated", Some("op-1"), Some(reason), now)
                .expect("resolve")
        );
    }

    fn human_says(&self, text: &str, mode: ChatSendMode) -> String {
        let inbox = self.inbox_thread().expect("inbox thread");
        self.store
            .chat_message_post(
                &inbox,
                &ChatPostMessageRequest {
                    client_message_id: format!("h-{text}"),
                    text: text.into(),
                    attachment_ids: vec![],
                    reply_to_id: None,
                    mode,
                    resume_queue: false,
                },
                self.d.now_utc(),
            )
            .expect("post")
            .response
            .message
            .id
    }

    fn user_input_runs(&self) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id WHERE m.role='user'",
        )
    }
}

#[tokio::test]
async fn cos_chat_triage_digest_records_each_judgment_with_an_answerable_card() {
    let mut f = fixture(true);
    f.tick();
    f.question("deploy", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    let item = f.single_item();
    // CoS (the run) escalates the item through the resolve path.
    f.escalate(&item, "外部公開なので人の判断");
    assert!(f.join_inbox().await);
    assert!(f.digests().is_empty(), "no digest before the tick observes the end");
    f.tick();
    let digests = f.digests();
    assert_eq!(digests.len(), 1, "one digest per run");
    let d = &digests[0];
    assert!(d.text.contains("### 質問: deploy（質問）"), "{}", d.text);
    assert!(d.text.contains("- 判断: 人に回した"), "{}", d.text);
    assert!(d.text.contains("- 理由: 外部公開なので人の判断"), "{}", d.text);
    assert!(d.text.contains("- 人が決めること: 公開してよいか"), "{}", d.text);
    assert!(d.text.contains("- 選択肢: 公開する / 保留（推奨）"), "{}", d.text);
    assert_eq!(d.cards.len(), 1);
    let card = &d.cards[0];
    assert_eq!(card.kind, ChatCardKind::Question);
    assert_eq!(card.state, "escalated");
    assert_eq!(card.title, "質問: deploy");
    assert_eq!(card.href, "/tasks/t1");
    assert_eq!(card.actor, ChatActor::Cos);
    assert_eq!(card.operation_id.as_deref(), Some("op-1"));
    assert!(d.run_id.is_some());
    // Idempotent across ticks and a restart.
    f.tick();
    f.d.cos_chat_launch.as_mut().expect("launch").triage.digest_due = true;
    f.tick();
    assert_eq!(f.digests().len(), 1);
    assert_eq!(f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"), 0);
}

#[tokio::test]
async fn cos_chat_triage_digest_without_reason_names_the_item_and_fallback_names_it_too() {
    let mut f = fixture(true);
    f.tick();
    f.question("deploy", vec![]);
    f.tick();
    // The run completes without resolving anything (no reason recorded at all).
    assert!(f.join_inbox().await);
    f.tick();
    let digests = f.digests();
    assert_eq!(digests.len(), 1);
    let d = &digests[0];
    assert!(d.text.contains("### 質問: deploy（質問）"), "{}", d.text);
    assert!(d.text.contains("- 判断: 未処理（人へ直接通知する）"), "{}", d.text);
    assert!(d.text.contains("- 理由: 理由の記録なし"), "{}", d.text);
    // The fallback of the same pass hands it over, naming the wait and what to decide.
    assert_eq!(f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"), 1);
    let handoff: Vec<ChatMessage> = f
        .inbox_messages()
        .into_iter()
        .filter(|m| m.client_message_id.as_deref().is_some_and(|k| k.starts_with("cos-fallback:")))
        .collect();
    assert_eq!(handoff.len(), 1);
    let h = &handoff[0];
    assert!(h.text.contains("「質問: deploy」（質問）は人へ委ねた"), "{}", h.text);
    assert!(h.text.contains("決めること: "), "{}", h.text);
    assert!(h.text.contains("選択肢: "), "{}", h.text);
    assert!(h.text.contains("回答: /tasks/"), "{}", h.text);
    assert!(!h.text.contains("item_id="), "{}", h.text);
    assert_eq!(h.cards[0].title, "質問: deploy");
    assert_eq!(h.cards[0].state, "pending");
    assert!(h.cards[0].href.starts_with("/tasks/"));
}

#[tokio::test]
async fn cos_chat_triage_human_interrupt_requeues_items_and_runs_the_human_message_first() {
    let mut f = fixture(true);
    f.tick();
    f.question("deploy", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    let item = f.single_item();
    // The person interrupts the triage run with an instruction before it polls.
    f.human_says("(b) にして、ただし host 1 台だけで", ChatSendMode::Interrupt);
    assert!(f.join_inbox().await);
    f.tick();
    // The interrupted run's item went back to the queue instead of the fallback …
    assert_eq!(f.item_state(&item), "pending");
    assert_eq!(f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"), 0);
    let digests = f.digests();
    assert_eq!(digests.len(), 1);
    assert!(digests[0].text.contains("中断されたので次の run で扱う"), "{}", digests[0].text);
    // … and the human message's run started first (one run per thread).
    assert_eq!(f.inbox_runs(), 2);
    assert_eq!(f.user_input_runs(), 1);
    assert!(f.join_inbox().await);
    f.tick();
    // Then the requeued item is claimed by a new triage run: nothing is lost.
    assert_eq!(f.inbox_runs(), 3);
    assert_eq!(f.item_state(&item), "running");
    assert!(f.join_inbox().await);
}

#[tokio::test]
async fn cos_chat_inbox_human_message_while_triage_runs_is_queued_then_served_before_new_items() {
    let mut f = fixture(true);
    f.tick();
    f.question("deploy", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    let first = f.single_item();
    f.escalate(&first, "人の判断");
    // A question (new item) and a human message arrive while the run is live.
    f.human_says("deploy は (b) で", ChatSendMode::Queue);
    f.question("publish", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1, "one active run per thread");
    assert!(f.join_inbox().await);
    f.tick();
    // The human message is served before the new item.
    assert_eq!(f.inbox_runs(), 2);
    assert_eq!(f.user_input_runs(), 1);
    assert_eq!(f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='pending'"), 1);
    assert!(f.join_inbox().await);
    f.tick();
    assert_eq!(f.inbox_runs(), 3);
    assert_eq!(f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='running'"), 1);
    assert!(f.join_inbox().await);
}

#[test]
fn cos_chat_inbox_context_item_maps_the_report_and_answer_path() {
    use crate::dispatcher::cos_chat::launch::inbox_context_item;
    use task_core::chat::triage::{CosTriageDecision, CosTriageItemReport, CosTriageOption};
    let report = CosTriageItemReport {
        item_id: "i1".into(),
        source_kind: "decision".into(),
        source_key: "decision-d1".into(),
        source_revision: "3".into(),
        state: "escalated".into(),
        summary: String::new(),
        reason: Some("人の判断".into()),
        operation_id: Some("op".into()),
        run_id: Some("run".into()),
        created_at: "2026-10-07T00:00:00.000Z".into(),
        route: Some("escalation".into()),
        decision: Some(CosTriageDecision {
            summary: "どちらか".into(),
            options: vec![CosTriageOption {
                key: "a".into(),
                label: "A".into(),
            }],
            recommended: Some("a".into()),
            recommendation_reason: None,
            web_path: "/tasks/t".into(),
        }),
    };
    let ctx = inbox_context_item(report);
    assert_eq!(ctx.summary, "decision:decision-d1", "empty summary falls back to the source");
    assert_eq!(
        ctx.answer_path.as_deref(),
        Some("/api/v1/inbox/items/decision-d1/answer")
    );
    assert_eq!(ctx.decision.as_ref().map(|d| d.options[0].label.as_str()), Some("A"));
    let mut notice = CosTriageItemReport {
        source_kind: "notice".into(),
        decision: None,
        ..ctx_report_seed()
    };
    notice.summary = "知らせ".into();
    assert!(inbox_context_item(notice).answer_path.is_none());
}

fn ctx_report_seed() -> task_core::chat::triage::CosTriageItemReport {
    task_core::chat::triage::CosTriageItemReport {
        item_id: "n".into(),
        source_kind: "notice".into(),
        source_key: "n1".into(),
        source_revision: "1".into(),
        state: "running".into(),
        summary: String::new(),
        reason: None,
        operation_id: None,
        run_id: None,
        created_at: String::new(),
        route: None,
        decision: None,
    }
}
