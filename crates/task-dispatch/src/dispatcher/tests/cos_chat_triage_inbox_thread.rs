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
        conn.query_row("SELECT state FROM cos_inbox_items WHERE id=?1", [id], |r| {
            r.get(0)
        })
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
    assert!(
        f.digests().is_empty(),
        "no digest before the tick observes the end"
    );
    f.tick();
    let digests = f.digests();
    assert_eq!(digests.len(), 1, "one digest per run");
    let d = &digests[0];
    assert!(d.text.contains("### 質問: deploy（質問）"), "{}", d.text);
    assert!(d.text.contains("- 判断: 人に回した"), "{}", d.text);
    assert!(
        d.text.contains("- 理由: 外部公開なので人の判断"),
        "{}",
        d.text
    );
    assert!(
        d.text.contains("- 人が決めること: 公開してよいか"),
        "{}",
        d.text
    );
    assert!(
        d.text.contains("- 選択肢: 公開する / 保留（推奨）"),
        "{}",
        d.text
    );
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
    f.d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .triage
        .digest_due = true;
    f.tick();
    assert_eq!(f.digests().len(), 1);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"),
        0
    );
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
    assert!(
        d.text.contains("- 判断: 未処理（人へ直接通知する）"),
        "{}",
        d.text
    );
    assert!(d.text.contains("- 理由: 理由の記録なし"), "{}", d.text);
    // The fallback of the same pass hands it over, naming the wait and what to decide.
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"),
        1
    );
    let handoff: Vec<ChatMessage> = f
        .inbox_messages()
        .into_iter()
        .filter(|m| {
            m.client_message_id
                .as_deref()
                .is_some_and(|k| k.starts_with("cos-fallback:"))
        })
        .collect();
    assert_eq!(handoff.len(), 1);
    let h = &handoff[0];
    assert!(
        h.text.contains("「質問: deploy」（質問）は人へ委ねた"),
        "{}",
        h.text
    );
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
    f.human_says(
        "(b) にして、ただし host 1 台だけで",
        ChatSendMode::Interrupt,
    );
    assert!(f.join_inbox().await);
    f.tick();
    // The interrupted run's item went back to the queue instead of the fallback …
    assert_eq!(f.item_state(&item), "pending");
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"),
        0
    );
    let digests = f.digests();
    assert_eq!(digests.len(), 1);
    assert!(
        digests[0].text.contains("中断されたので次の run で扱う"),
        "{}",
        digests[0].text
    );
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
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='pending'"),
        1
    );
    assert!(f.join_inbox().await);
    f.tick();
    assert_eq!(f.inbox_runs(), 3);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='running'"),
        1
    );
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
    assert_eq!(
        ctx.summary, "decision:decision-d1",
        "empty summary falls back to the source"
    );
    assert_eq!(
        ctx.answer_path.as_deref(),
        Some("/api/v1/inbox/items/decision-d1/answer")
    );
    assert_eq!(
        ctx.decision.as_ref().map(|d| d.options[0].label.as_str()),
        Some("A")
    );
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

impl Fixture {
    /// Durable terminal runs from a previous process, without spawning a worker.
    fn seed_digest_run(&self, id: &str, state: task_core::chat::ChatRunState) {
        let now = self.d.now_utc();
        self.store
            .cos_triage_ingest_batch(
                "digest-test",
                id,
                &[task_core::chat::triage::CosTriageSource {
                    source_kind: "notice".into(),
                    source_key: id.into(),
                    source_revision: "1".into(),
                    source_event_id: None,
                    operation_id: None,
                    summary: format!("知らせ {id}"),
                    policy_version: "1".into(),
                }],
                now,
            )
            .expect("ingest");
        let claim = self
            .store
            .cos_triage_claim(id, now)
            .expect("claim")
            .expect("run");
        if state == task_core::chat::ChatRunState::Completed {
            self.store
                .cos_triage_resolve(&claim.item_ids[0], "observed", None, Some("確認済み"), now)
                .expect("resolve");
        }
        self.store
            .chat_run_finish(id, state, None, None, now)
            .expect("finish");
    }

    fn digest_due(&self) -> bool {
        self.d
            .cos_chat_launch
            .as_ref()
            .expect("launch")
            .triage
            .digest_due
    }
}

#[tokio::test]
async fn cos_chat_triage_digest_launch_failure_after_startup_is_recovered_on_normal_tick() {
    let mut f = fixture(true);
    f.tick(); // Startup reconciliation has already consumed its digest reservation.
    assert!(!f.digest_due());
    f.d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .config
        .unavailable_reason = Some("CoS unavailable: quota or login".into());
    f.question("deploy", vec![]);
    f.tick();
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_runs WHERE state='failed'"),
        1
    );
    assert!(!f.join_inbox().await, "failure occurred before spawn");
    f.tick(); // No reconcile request, new input, or restart.
    let digests = f.digests();
    assert_eq!(digests.len(), 1);
    assert!(digests[0].text.contains("質問: deploy"));
    assert!(digests[0].text.contains("quota or login"));
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.digests().len(), 1);
    assert_eq!(f.inbox_runs(), 1);
    assert!(!f.digest_due());
}

#[tokio::test]
async fn cos_chat_triage_digest_restart_drains_more_than_twenty_runs_on_normal_ticks() {
    let mut f = fixture(true);
    for i in 0..41 {
        f.seed_digest_run(
            &format!("old-{i:02}"),
            task_core::chat::ChatRunState::Completed,
        );
    }
    // Fresh launch state sees durable history once, then ordinary ticks drain it.
    f.tick();
    assert_eq!(f.digests().len(), 20, "bounded first pass");
    f.tick();
    assert_eq!(f.digests().len(), 40, "bounded second pass");
    f.tick();
    assert_eq!(f.digests().len(), 41, "last run is not stranded");
    f.tick();
    assert!(!f.digest_due(), "stop scanning after the backlog is empty");
    let ids: std::collections::HashSet<_> =
        f.digests().into_iter().map(|m| m.run_id.unwrap()).collect();
    assert_eq!(ids.len(), 41);
    assert_eq!(f.inbox_runs(), 41, "no new run needed to drive recovery");
    assert!(!f.join_inbox().await);
}

#[tokio::test]
async fn cos_chat_triage_digest_write_failure_retries_on_normal_tick() {
    let mut f = fixture(true);
    f.seed_digest_run("old", task_core::chat::ChatRunState::Completed);
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.execute_batch(
        "CREATE TRIGGER fail_digest BEFORE INSERT ON chat_messages
        WHEN NEW.client_message_id LIKE 'cos-triage-digest:%'
        BEGIN SELECT RAISE(FAIL, 'injected digest failure'); END;",
    )
    .expect("trigger");
    f.tick();
    assert!(f.digests().is_empty());
    conn.execute_batch("DROP TRIGGER fail_digest")
        .expect("restore writes");
    f.tick();
    assert_eq!(f.digests().len(), 1, "failed write remains scheduled");
    f.tick();
    assert!(!f.digest_due());
}

#[tokio::test]
async fn cos_chat_triage_digest_scan_failure_retries_on_normal_tick() {
    let mut f = fixture(true);
    f.tick();
    f.seed_digest_run("old", task_core::chat::ChatRunState::Completed);
    f.d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .triage
        .digest_due = true;
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.execute_batch("ALTER TABLE cos_inbox_items RENAME TO hidden_items")
        .expect("hide table");
    f.tick();
    assert!(f.digests().is_empty());
    conn.execute_batch("ALTER TABLE hidden_items RENAME TO cos_inbox_items")
        .expect("restore table");
    f.tick();
    assert_eq!(f.digests().len(), 1, "failed scan remains scheduled");
    f.tick();
    assert!(!f.digest_due());
}

#[tokio::test]
async fn cos_chat_triage_digest_orphan_recovery_after_startup_is_scheduled() {
    let mut f = fixture(true);
    f.tick();
    assert!(!f.digest_due());
    // Intake is allowed while launches are paused. The previous owner left an
    // escalated item and a stopped run whose terminal transition is still due.
    f.d.accepting_new_work = false;
    f.question("deploy", vec![]);
    f.tick();
    let claim = f
        .store
        .cos_triage_claim("orphan", f.d.now_utc())
        .expect("claim")
        .expect("run");
    f.escalate(&claim.item_ids[0], "人の判断");
    f.store
        .chat_run_stop(&claim.thread_id, &claim.run_id, f.d.now_utc())
        .expect("stop");
    f.d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "restarted-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| false),
    });
    f.d.accepting_new_work = true;
    f.tick();
    assert_eq!(
        f.store
            .chat_run_get(&claim.thread_id, &claim.run_id)
            .expect("run")
            .state,
        task_core::chat::ChatRunState::Stopped
    );
    let digests = f.digests();
    assert_eq!(digests.len(), 1);
    assert_eq!(digests[0].run_id.as_deref(), Some("orphan"));
    assert!(digests[0].text.contains("質問: deploy"));
    assert!(digests[0].text.contains("人に回した"));
    assert!(!f.join_inbox().await);
    f.tick();
    assert!(!f.digest_due());
    assert_eq!(f.digests().len(), 1);
}

/// ADR 2026-10-08-cos-chat-prompt-cache T5: the inbox thread's runs (triage and a human message)
/// mount `cos-inbox-triage` next to `cos-operator`; ordinary threads do not (cos_chat_launch).
#[tokio::test]
async fn cos_chat_skill_inbox_thread_runs_mount_inbox_triage() {
    let mut f = fixture(true);
    f.tick();
    f.question("deploy", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    assert!(f.join_inbox().await);
    f.human_says("deploy はどうなった?", ChatSendMode::Queue);
    f.tick();
    assert_eq!(f.user_input_runs(), 1);
    assert!(f.join_inbox().await);
    let inbox = f.inbox_thread().expect("inbox thread");
    let runs = f
        .dir
        .path()
        .join("cos/threads")
        .join(&inbox)
        .join("workspace/runs");
    let mut seen = 0;
    for entry in std::fs::read_dir(&runs).expect("runs dir") {
        let request = entry.expect("entry").path().join("request.json");
        let Ok(text) = std::fs::read_to_string(&request) else {
            continue;
        };
        let json: serde_json::Value = serde_json::from_str(&text).expect("request json");
        assert_eq!(
            json["context"]["cos_chat"]["skills"],
            serde_json::json!(["cos-operator", "cos-inbox-triage"]),
            "{}",
            request.display()
        );
        assert_eq!(
            json["context"]["skills"].as_array().map(Vec::len),
            Some(2)
        );
        seen += 1;
    }
    assert_eq!(seen, 2, "the triage run and the human message's run");
}
