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
        f.inbox_runs(),
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
