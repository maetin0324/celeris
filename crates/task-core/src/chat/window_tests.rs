//! ADR 2026-10-28-single-human-window-general-cos D2/D3 at the store.
use super::triage::CosTriageSource;
use super::{
    ChatForwardRequest, ChatMessage, ChatMessageQuery, ChatOpenThreadRequest,
    ChatPostMessageRequest, ChatSendMode,
};
use crate::store::SqliteStore;
use crate::store::tests::sample_task;
use crate::{Status, TaskStore};
use time::OffsetDateTime;

fn at(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + n).expect("time")
}

fn source(key: &str, task: Option<String>) -> CosTriageSource {
    CosTriageSource {
        source_kind: "decision".into(),
        source_key: key.into(),
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: "公開先の確認".into(),
        detail: String::new(),
        policy_version: "5".into(),
        task_id: task,
    }
}

fn general(s: &SqliteStore) -> String {
    s.cos_thread_open(&ChatOpenThreadRequest { project_id: None }, at(0))
        .expect("general")
        .thread
        .id
}

fn messages(s: &SqliteStore, thread: &str) -> Vec<ChatMessage> {
    s.chat_message_list(thread, &ChatMessageQuery::default())
        .expect("list")
        .items
}

fn cards(s: &SqliteStore, thread: &str) -> Vec<ChatMessage> {
    messages(s, thread)
        .into_iter()
        .filter(|m| m.escalation.is_some())
        .collect()
}

fn item_id(s: &SqliteStore, key: &str) -> String {
    s.lock()
        .expect("lock")
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1",
            [key],
            |r| r.get(0),
        )
        .expect("item")
}

/// A project thread with one task; returns `(project thread id, task id)`.
fn project_task(s: &SqliteStore) -> (String, String) {
    let project = crate::ProjectId::new();
    let thread = crate::chat::test_project_thread(s, &project.to_string(), at(0));
    let mut task = sample_task(Status::Running);
    task.project_id = Some(project);
    s.create_task(&task, vec![]).expect("task");
    (thread.id, task.id.to_string())
}

/// D2: an item of a project thread routed to the person shows one card in the general thread
/// (keyed by the source triple; a second route of the same item adds nothing). An item of the
/// general thread has no card. When the item resolves, the card reads as resolved.
#[test]
fn escalation_of_a_project_item_posts_one_card_in_the_general_thread_and_resolves_with_it() {
    let s = SqliteStore::open_in_memory().expect("store");
    let (project_thread, task) = project_task(&s);
    let general = general(&s);

    s.cos_triage_ingest_batch(
        "decision",
        "c1",
        &[source("decision-p", Some(task.clone()))],
        at(1),
    )
    .expect("ingest");
    let item = item_id(&s, "decision-p");
    let body = serde_json::json!({
        "summary": "公開先を決める",
        "options": [{"key": "a", "label": "社内だけ"}, {"key": "b", "label": "公開"}],
        "recommended": "a",
        "web_path": format!("/tasks/{task}"),
        "task_id": task,
    })
    .to_string();
    let first = s
        .cos_triage_outbox_claim(&item, "escalation", &body, at(2))
        .expect("claim");
    assert!(first.is_some());
    // The fallback route of the same item is refused by the route table; the card stays one.
    s.cos_triage_outbox_claim(&item, "fallback", &body, at(3))
        .expect("second route");
    s.cos_triage_resolve(&item, "escalated", None, None, at(3))
        .expect("escalated");

    let posted = cards(&s, &general);
    assert_eq!(posted.len(), 1, "one card per source triple");
    let card = &posted[0];
    let escalation = card.escalation.as_ref().expect("escalation");
    assert_eq!(escalation.item_id, item);
    assert_eq!(escalation.source_thread_id, project_thread);
    assert_eq!(escalation.task_id.as_deref(), Some(task.as_str()));
    assert_eq!(escalation.state, "escalated");
    assert!(escalation.open);
    assert!(card.text.contains("公開先を決める"), "{}", card.text);
    assert!(card.text.contains("社内だけ / 公開"), "{}", card.text);
    assert!(
        card.text.contains(&format!("task: {task}")),
        "{}",
        card.text
    );
    assert_eq!(card.cards.len(), 1);
    assert_eq!(card.cards[0].id, "decision-p");
    assert_eq!(card.cards[0].state, "escalated");
    // The project thread keeps its own record (the item), with no card there.
    assert!(cards(&s, &project_thread).is_empty());

    // A direct escalation of a general item posts no card.
    s.cos_triage_direct_escalate(&source("decision-g", None), r#"{"summary":"g"}"#, at(4))
        .expect("direct");
    assert_eq!(cards(&s, &general).len(), 1);
    // A direct escalation of a project item posts its card once.
    let direct = source("decision-d", Some(task.clone()));
    s.cos_triage_direct_escalate(&direct, r#"{"summary":"鍵の登録"}"#, at(5))
        .expect("direct project");
    s.cos_triage_direct_escalate(&direct, r#"{"summary":"鍵の登録"}"#, at(6))
        .expect("direct again");
    assert_eq!(cards(&s, &general).len(), 2);

    // The source wait closes: reconcile resolves the item (2026-10-16), and the card follows.
    s.cos_triage_reconcile("decision", &[], at(7))
        .expect("reconcile");
    for card in cards(&s, &general) {
        let escalation = card.escalation.expect("escalation");
        assert_eq!(escalation.state, "resolved");
        assert!(!escalation.open);
        assert_eq!(card.cards[0].state, "resolved");
    }
}

fn human_post(s: &SqliteStore, thread: &str, key: &str, text: &str, author: &str) -> String {
    s.chat_message_post_as(
        thread,
        &ChatPostMessageRequest {
            client_message_id: key.into(),
            text: text.into(),
            attachment_ids: vec![],
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
        },
        author,
        at(10),
    )
    .expect("post")
    .response
    .message
    .id
}

fn forward(
    s: &SqliteStore,
    from: &str,
    to: &str,
    message: &str,
    tasks: &[String],
    expires: &str,
) -> Result<ChatMessage, super::ChatError> {
    s.chat_relay_forward(
        from,
        to,
        &ChatForwardRequest {
            client_message_id: format!("forward:{message}:{to}"),
            message_id: message.into(),
            task_ids: tasks.to_vec(),
            expires_at: expires.into(),
        },
        &format!("cos:{from}"),
        at(20),
    )
    .map(|posted| posted.response.message)
}

/// D3: the forward copies the person's words and time verbatim into the project thread as a
/// queued relay of `cos:<general>`; it needs tasks of that project and an expiry, and only a
/// person's own message of the general thread can be forwarded.
#[test]
fn forward_copies_the_persons_message_into_the_project_thread_within_scope() {
    let s = SqliteStore::open_in_memory().expect("store");
    let (project_thread, task) = project_task(&s);
    let (_other_thread, other_task) = project_task(&s);
    let general = general(&s);
    let said = human_post(&s, &general, "h1", "社内だけで進めて", "human:web");
    let expires = "2026-10-29T00:00:00Z";
    let tasks = vec![task.clone()];

    let relayed = forward(&s, &general, &project_thread, &said, &tasks, expires).expect("forward");
    assert_eq!(relayed.thread_id, project_thread);
    assert_eq!(relayed.text, "社内だけで進めて");
    assert_eq!(
        relayed.author.as_deref(),
        Some(format!("cos:{general}").as_str())
    );
    let relay = relayed.relay.as_ref().expect("relay");
    assert_eq!(relay.relayed_by, format!("cos:{general}"));
    assert_eq!(
        relay.said_at,
        s.chat_message_get(&general, &said).expect("m").created_at
    );
    assert_eq!(relay.scope.task_ids, tasks);
    assert!(relay.scope.expires_at.is_some());
    let source = relay.source.as_ref().expect("source");
    assert_eq!(source.message_id, said);
    assert_eq!(source.author.as_deref(), Some("human:web"));
    // Same key: same message.
    let again = forward(&s, &general, &project_thread, &said, &tasks, expires).expect("replay");
    assert_eq!(again.id, relayed.id);

    // Out of scope: no tasks, no expiry, an expired time, a task of another project.
    for (tasks, expires) in [
        (vec![], expires),
        (tasks.clone(), ""),
        (tasks.clone(), "2026-10-01T00:00:00Z"),
        (vec![other_task.clone()], expires),
    ] {
        let mut s_tasks = tasks;
        s_tasks.dedup();
        assert!(
            forward(&s, &general, &project_thread, &said, &s_tasks, expires).is_err(),
            "{s_tasks:?} {expires}"
        );
    }
    // Not the person's own message: a CoS-authored (no human author) or a relayed one.
    let operator = human_post(
        &s,
        &general,
        "h2",
        "operator の要約",
        "operator:claude-code",
    );
    assert!(
        forward(
            &s,
            &general,
            &project_thread,
            &operator,
            std::slice::from_ref(&task),
            expires
        )
        .is_err()
    );
    // Only from the general thread, only to a project thread.
    assert!(
        forward(
            &s,
            &project_thread,
            &project_thread,
            &said,
            std::slice::from_ref(&task),
            expires
        )
        .is_err()
    );
    assert!(forward(&s, &general, &general, &said, &[task], expires).is_err());
}
