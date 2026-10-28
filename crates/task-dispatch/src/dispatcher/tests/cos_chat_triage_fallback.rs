//! CoS unavailable fallback (ADR 2026-10-05 D6). Each condition has its own
//! fixture; time is the injected test clock and runs are awaited by joining
//! the worker handle, never by sleeping. No network: the outbox is only rows.

use super::*;
use crate::dispatcher::cos_chat::fallback::{
    COS_FALLBACK_HEADLINE, COS_FALLBACK_NO_RECOMMENDATION, FallbackCandidate, unavailable_reason,
};

const FAIL: &str = "cat >/dev/null; echo boom >&2; exit 3";
const QUOTA: &str =
    "CoS unavailable: no usable account for provider p1 (quota, cooldown, or login)";

impl Fixture {
    fn fallback_outbox(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'")
    }

    fn outbox_total(&self) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM notifications WHERE kind IN ('cos_fallback','cos_escalation')",
        )
    }

    fn state_count(&self, state: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM cos_inbox_items WHERE state='{state}'"
        ))
    }

    fn only_item(&self) -> String {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row("SELECT id FROM cos_inbox_items", [], |r| r.get(0))
            .expect("one item")
    }

    fn fallback_body(&self) -> serde_json::Value {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let body: String = conn
            .query_row(
                "SELECT body FROM notifications WHERE kind='cos_fallback'",
                [],
                |r| r.get(0),
            )
            .expect("fallback body");
        serde_json::from_str(&body).expect("json body")
    }

    fn set_unavailable(&mut self, reason: &str) {
        self.d
            .cos_chat_launch
            .as_mut()
            .expect("launch")
            .config
            .unavailable_reason = Some(reason.into());
    }

    /// A daemon restart: the process-local launch state is rebuilt from config.
    fn restart(&mut self, enabled: bool) {
        let launch = self.d.cos_chat_launch.take().expect("launch");
        let mut config = launch.config.clone();
        config.enabled = enabled;
        config.unavailable_reason = None;
        self.d.set_cos_chat_launch(self.store.clone(), config);
    }

    /// Each revision has exactly one outbox row and its item is `fallback`.
    fn assert_one_fallback(&self, reason_part: &str) {
        assert_eq!(self.fallback_outbox(), 1, "one outbox for the revision");
        assert_eq!(self.state_count("fallback"), 1);
        let body = self.fallback_body();
        let reason = body["unavailable_reason"].as_str().expect("reason");
        assert!(reason.contains(reason_part), "{reason}");
        assert_eq!(body["headline"], COS_FALLBACK_HEADLINE);
        let text = body["text"].as_str().expect("text");
        assert!(text.starts_with(COS_FALLBACK_HEADLINE), "{text}");
        assert!(text.contains("不在理由: "), "{text}");
        assert!(text.contains(COS_FALLBACK_NO_RECOMMENDATION), "{text}");
        assert!(
            body["web_path"]
                .as_str()
                .expect("path")
                .starts_with("/tasks/")
        );
    }
}

#[tokio::test]
async fn cos_chat_triage_fallback_run_failure_routes_once() {
    let mut f = fixture_script(true, FAIL);
    f.tick();
    f.question("deploy", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    assert_eq!(f.fallback_outbox(), 0, "no fallback while the run is live");
    assert!(f.join_inbox().await);
    f.tick();
    f.assert_one_fallback("CoS run が失敗");
    let body = f.fallback_body();
    assert!(
        body["summary"]
            .as_str()
            .expect("summary")
            .contains("deploy")
    );
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.fallback_outbox(), 1);
    assert_eq!(
        f.triage_runs(),
        1,
        "the fallback item is never claimed again"
    );
}

#[tokio::test]
async fn cos_chat_triage_fallback_quota_or_login_unavailable() {
    let mut f = fixture(true);
    f.set_unavailable(QUOTA);
    f.tick();
    f.question("quota", vec![]);
    f.tick(); // claim + launch failure: the run is failed with the reason
    assert_eq!(f.inbox_runs(), 1);
    f.tick();
    f.assert_one_fallback("quota 切れ・ログイン不可");
}

#[tokio::test]
async fn cos_chat_triage_fallback_disabled_routes_immediately() {
    let mut f = fixture(false);
    f.tick();
    f.question("disabled", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 0);
    f.assert_one_fallback("cos.enabled=false");
}

#[tokio::test]
async fn cos_chat_triage_fallback_deadline_covers_capacity_wait() {
    let mut f = fixture(true);
    // Stopped intake of new work: CoS never starts, so only the deadline fires.
    f.d.accepting_new_work = false;
    f.tick();
    f.question("waiting", vec![]);
    f.tick();
    f.advance(100);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0, "within unavailable_after_secs");
    assert_eq!(f.state_count("pending"), 1);
    f.advance(30);
    f.tick();
    f.assert_one_fallback("120 秒");
    assert_eq!(f.inbox_runs(), 0);
}

#[tokio::test]
async fn cos_chat_triage_fallback_completed_run_leaving_item_unhandled() {
    let mut f = fixture(true);
    f.tick();
    f.question("ignored", vec![]);
    f.tick();
    assert!(f.join_inbox().await);
    f.tick();
    f.assert_one_fallback("未処理");
}

#[tokio::test]
async fn cos_chat_triage_fallback_restart_and_recovery_neither_resend_nor_answer() {
    let mut f = fixture(false);
    f.tick();
    f.question("handed", vec![]);
    f.tick();
    f.assert_one_fallback("cos.enabled=false");
    let item = f.only_item();
    let inbox = f.inbox_thread().expect("inbox thread");
    let all = f
        .store
        .chat_message_list(
            &inbox,
            &task_core::chat::ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages")
        .items;
    let dump: Vec<String> = all
        .iter()
        .map(|m| format!("{} {:?}", m.text, m.cards))
        .collect();
    let handoff = all
        .into_iter()
        .filter(|m| {
            m.text.contains("handed」")
                && m.text.contains("人へ委ねた")
                && m.text.contains("決めること: ")
                && m.cards
                    .iter()
                    .any(|c| c.title.contains("handed") && c.state == "pending")
        })
        .count();
    let _ = item;
    assert_eq!(
        handoff, 1,
        "the session is told the item is with the person: {dump:?}"
    );

    // Restart with CoS back: the reconcile pass sees the same revision.
    f.restart(true);
    for _ in 0..3 {
        f.tick();
    }
    assert!(
        !f.join_inbox().await,
        "no CoS run for a handed-over revision"
    );
    assert_eq!(f.inbox_runs(), 0);
    assert_eq!(f.items(), 1);
    assert_eq!(f.fallback_outbox(), 1, "no second notice");
    assert_eq!(f.state_count("fallback"), 1);
    // An escalation attempt by a recovered CoS cannot add a second pending outbox.
    let again = f
        .store
        .cos_triage_outbox_claim(&item, "escalation", "{}", f.d.now_utc())
        .expect("claim");
    assert!(again.is_none());
    assert_eq!(f.outbox_total(), 1);
}

#[tokio::test]
async fn cos_chat_triage_fallback_recovers_claim_whose_mark_was_lost() {
    let mut f = fixture(true);
    f.d.accepting_new_work = false;
    f.tick();
    f.question("crashed", vec![]);
    f.tick();
    let item = f.only_item();
    // A crash between the outbox claim and the state mark.
    f.store
        .cos_triage_outbox_claim(&item, "fallback", "{}", f.d.now_utc())
        .expect("claim")
        .expect("claimed");
    assert_eq!(f.state_count("pending"), 1);
    f.restart(true);
    f.d.accepting_new_work = false;
    f.tick();
    assert_eq!(f.state_count("fallback"), 1);
    assert_eq!(f.fallback_outbox(), 1);
}

#[tokio::test]
async fn cos_chat_triage_fallback_escalation_race_keeps_one_pending() {
    let mut f = fixture(true);
    f.d.accepting_new_work = false;
    f.tick();
    f.question("raced", vec![]);
    f.tick();
    let item = f.only_item();
    // CoS escalated (outbox claimed) just before the deadline.
    f.store
        .cos_triage_outbox_claim(&item, "escalation", "{}", f.d.now_utc())
        .expect("claim")
        .expect("claimed");
    f.advance(300);
    f.tick();
    f.tick();
    assert_eq!(f.fallback_outbox(), 0);
    assert_eq!(f.outbox_total(), 1, "one pending outbox for the revision");
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE ok IS NULL"),
        1
    );
}

#[tokio::test]
async fn cos_chat_triage_fallback_failed_delivery_makes_no_new_wait() {
    let mut f = fixture(false);
    f.tick();
    f.question("undelivered", vec![]);
    f.tick();
    f.assert_one_fallback("cos.enabled=false");
    // The notifier gave up (webhook unset or failing): the row stays failed.
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.execute(
        "UPDATE notifications SET ok=0,error='webhook unset' WHERE kind='cos_fallback'",
        [],
    )
    .expect("fail delivery");
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.items(), 1, "a failed fallback is not a new CoS wait");
    assert_eq!(f.fallback_outbox(), 1);
    assert_eq!(f.inbox_runs(), 0);
}

#[tokio::test]
async fn cos_chat_triage_fallback_new_revision_gets_its_own_outbox() {
    let mut f = fixture(false);
    f.tick();
    f.question("first", vec![]);
    f.question("second", vec![]);
    f.tick();
    assert_eq!(f.fallback_outbox(), 2, "one outbox per revision");
    assert_eq!(
        f.count(
            "SELECT COUNT(*) FROM (SELECT key FROM notifications WHERE kind='cos_fallback' \
             GROUP BY key HAVING COUNT(*)>1)"
        ),
        0
    );
}

#[test]
fn cos_chat_triage_fallback_reason_is_deterministic() {
    let now = OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("now");
    let base = FallbackCandidate {
        item_id: "i".into(),
        source_kind: "question".into(),
        source_key: "question-1".into(),
        source_revision: "r".into(),
        created_at: "2026-09-21T13:46:40.000Z".into(),
        run_id: None,
        run_state: None,
        run_reason: None,
        route: None,
        summary: String::new(),
        detail: String::new(),
    };
    let fresh = FallbackCandidate {
        created_at: (now - time::Duration::seconds(10))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("ts"),
        ..base.clone()
    };
    assert!(unavailable_reason(&fresh, true, 120, now).is_none());
    assert!(unavailable_reason(&fresh, false, 120, now).is_some());
    let running = FallbackCandidate {
        run_id: Some("r".into()),
        run_state: Some("running".into()),
        ..fresh.clone()
    };
    assert!(unavailable_reason(&running, true, 120, now).is_none());
    for (state, part) in [
        ("failed", "失敗"),
        ("interrupted", "止まった"),
        ("stopped", "止まった"),
        ("completed", "未処理"),
    ] {
        let c = FallbackCandidate {
            run_state: Some(state.into()),
            ..running.clone()
        };
        let reason = unavailable_reason(&c, true, 120, now).expect(state);
        assert!(reason.contains(part), "{state}: {reason}");
    }
    let quota = FallbackCandidate {
        run_state: Some("failed".into()),
        run_reason: Some(QUOTA.into()),
        ..running
    };
    assert!(
        unavailable_reason(&quota, true, 120, now)
            .expect("quota")
            .contains("quota")
    );
}

impl Fixture {
    /// Triage runs (input is the claim line), as opposed to runs on queued requests.
    fn triage_runs(&self) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id \
             WHERE m.text LIKE '{}%'",
            task_core::chat::triage::COS_TRIAGE_CLAIM_PREFIX
        ))
    }

    /// Messages of the general CoS thread: `(role, state, client key, text)`.
    fn general_messages(&self) -> Vec<(String, String, Option<String>, String)> {
        let thread = self.inbox_thread().expect("general thread");
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT role,state,client_message_id,text FROM chat_messages \
                 WHERE thread_id=?1 ORDER BY seq",
            )
            .expect("prepare");
        stmt.query_map([thread], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
    }

    /// A blocked task whose question is `text` (the whole body of the wait).
    fn long_question(&self, title: &str, text: &str) -> TaskId {
        let mut task = new_task(self.dir.path(), Check::Human, 0);
        task.title = title.into();
        task.status = Status::Blocked;
        self.store
            .create_task(
                &task,
                vec![Event::QuestionRaised {
                    run_id: "r0".into(),
                    text: text.into(),
                }],
            )
            .expect("task");
        task.id
    }
}

/// ADR 2026-10-28-cos-inbox-chat-summary D1/D4 (受け入れ条件 0・1): a wait whose options are
/// spelled out past the 600-char inbox detail reaches the chat whole, and CoS is asked to add its
/// summary once it can run. The request is a queued system message: CoS runs on it, and it is
/// never a person's instruction.
#[tokio::test]
async fn cos_chat_triage_fallback_hands_over_the_whole_body_and_queues_a_summary() {
    let mut f = fixture(true);
    // Stopped intake of new work: CoS never starts, so only the deadline fires.
    f.d.accepting_new_work = false;
    f.tick();
    let body = format!(
        "{}\nA: cluster-hpc へ今回だけ委譲する\nB: software-engineering が自分で実行する\nC: 委譲をやめて人が手で行う",
        "委譲の背景。".repeat(120)
    );
    f.long_question("長い質問", &body);
    f.tick();
    f.advance(130);
    f.tick();
    f.assert_one_fallback("120 秒");
    assert_eq!(f.inbox_runs(), 0);
    let messages = f.general_messages();
    let handoff = messages
        .iter()
        .find(|(_, _, key, _)| {
            key.as_deref()
                .is_some_and(|k| k.starts_with("cos-fallback:"))
        })
        .expect("hand-off line");
    assert!(
        handoff.3.contains("C: 委譲をやめて人が手で行う"),
        "{}",
        handoff.3
    );
    assert!(handoff.3.contains("CoS が戻り次第"), "{}", handoff.3);
    assert!(!handoff.3.contains('…'), "not clipped: {}", handoff.3);
    let requests: Vec<_> = messages
        .iter()
        .filter(|(_, _, key, _)| {
            key.as_deref().is_some_and(|k| {
                k.starts_with(task_core::chat::triage::COS_FALLBACK_SUMMARY_KEY_PREFIX)
            })
        })
        .collect();
    assert_eq!(requests.len(), 1, "{messages:?}");
    let (role, state, _, text) = requests[0];
    assert_eq!((role.as_str(), state.as_str()), ("system", "queued"));
    assert!(
        text.starts_with(task_core::chat::triage::COS_FALLBACK_SUMMARY_PREFIX),
        "{text}"
    );
    assert!(text.contains("人の発言ではない"), "{text}");
    assert!(text.contains("GET /api/v1/tasks/"), "{text}");
    assert!(text.contains("長い質問"), "{text}");

    // Ticks while CoS is still away neither duplicate the line nor the request.
    f.tick();
    f.tick();
    assert_eq!(
        f.count(
            "SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'cos-fallback-summary:%'"
        ),
        1
    );

    // CoS is back: the normal queue claim runs CoS on the request.
    f.d.accepting_new_work = true;
    f.tick();
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_runs"),
        1,
        "a run on the queued request"
    );
    assert_eq!(
        f.triage_runs(),
        0,
        "the handed-over item is not triaged again"
    );
    let input_text = f.count_text(
        "SELECT m.text FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id \
         ORDER BY r.started_at DESC, r.rowid DESC LIMIT 1",
    );
    assert!(
        input_text.starts_with(task_core::chat::triage::COS_FALLBACK_SUMMARY_PREFIX),
        "{input_text}"
    );
    assert!(f.join_inbox().await);
}

/// CoS disabled: the hand-off line does not promise a summary and nothing is queued.
#[tokio::test]
async fn cos_chat_triage_fallback_disabled_queues_no_summary() {
    let mut f = fixture(false);
    f.tick();
    f.question("disabled", vec![]);
    f.tick();
    f.assert_one_fallback("cos.enabled=false");
    let messages = f.general_messages();
    assert!(
        messages
            .iter()
            .all(|(_, _, _, t)| !t.contains("CoS が戻り次第"))
    );
    assert_eq!(
        f.count(
            "SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'cos-fallback-summary:%'"
        ),
        0
    );
}

/// ADR 2026-10-28-cos-inbox-chat-summary D2 (受け入れ条件 2): a notice bundle that grows after
/// ingestion keeps its title and body together. The fallback of the first revision quotes that
/// revision's body, never the later event's body under the earlier title.
#[tokio::test]
async fn cos_chat_triage_fallback_notice_title_and_body_stay_together() {
    let mut f = fixture(false);
    f.tick();
    let record = |f: &Fixture, key: &str, title: &str, summary: &str| {
        f.store
            .notice_record(&NoticeEvent {
                source_key: key.into(),
                kind: NoticeKind::Report,
                group_key: "report:bundle".into(),
                title: title.into(),
                summary: summary.into(),
                project_id: None,
                task_id: None,
                target: None,
                links: Vec::new(),
                at: f.d.now_utc(),
            })
            .expect("notice")
            .notice_id()
    };
    record(
        &f,
        "report:1",
        "日次整理が完了",
        "KB の受信箱を 3 件整理した",
    );
    f.tick();
    // The bundle grows before the fallback of revision 1 is written.
    record(&f, "report:2", "task X が完了", "task X の成果を統合した");
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    let rows: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT source_revision,summary,detail FROM cos_inbox_items ORDER BY source_revision")
            .expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    };
    assert_eq!(
        rows[0],
        (
            "1".to_string(),
            "日次整理が完了".to_string(),
            "KB の受信箱を 3 件整理した".to_string()
        )
    );
    f.tick();
    let bodies: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT n.body FROM notifications n JOIN cos_notification_routes r ON r.notification_id=n.id \
                 JOIN cos_inbox_items i ON i.id=r.item_id WHERE i.source_revision='1'",
            )
            .expect("prepare");
        stmt.query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    };
    assert_eq!(bodies.len(), 1, "revision 1 handed over once");
    let body: serde_json::Value = serde_json::from_str(&bodies[0]).expect("json");
    let summary = body["summary"].as_str().expect("summary");
    assert_eq!(summary, "日次整理が完了 — KB の受信箱を 3 件整理した");
    assert!(!summary.contains("task X"), "{summary}");
}
