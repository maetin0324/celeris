//! ADR 2026-10-29-cos-authorization-fallback-after-triage: an authorization item reaches the
//! person (Discord outbox) only after CoS's first response leaves it with the person. Time is the
//! injected test clock; runs are awaited by joining the worker handle, never by sleeping.

use super::*;
use task_core::approval::{Approval, ApprovalId, ApprovalStore};

impl Fixture {
    /// A task blocked on a question with a pending approval: one `authorization` item.
    fn authorization(&self, title: &str) -> TaskId {
        let task = self.question(title, vec![]);
        self.store
            .approval_append(&Approval {
                id: ApprovalId::new(),
                project_id: None,
                node_id: "cluster-hpc".into(),
                task_id: Some(task),
                question: format!("{title}?"),
                decision: None,
                answer: None,
                created_at: self.d.now_utc(),
                decided_at: None,
            })
            .expect("approval");
        task
    }

    fn only_kind(&self) -> String {
        self.count_text("SELECT source_kind FROM cos_inbox_items")
    }

    fn live_runs(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM chat_runs WHERE state IN ('queued','running','stopping')")
    }
}

/// 受け入れ条件 0: the CoS run that claimed the authorization item is still running past
/// `unavailable_after_secs` (120): no fallback outbox. The run is never awaited, so it stays live.
#[tokio::test]
async fn cos_chat_authorization_no_fallback_while_its_run_is_live() {
    let mut f = fixture(true);
    f.tick();
    f.authorization("委譲");
    f.tick();
    assert_eq!(f.only_kind(), "authorization");
    assert_eq!(f.inbox_runs(), 1);
    assert_eq!(f.live_runs(), 1);
    assert_eq!(f.state_count("running"), 1);
    f.advance(130);
    f.tick();
    f.advance(600);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0, "CoS is still working on it");
    assert_eq!(f.outbox_total(), 0);
    assert_eq!(f.state_count("running"), 1);
}

/// 受け入れ条件 2: a run that hangs past `authorization_unavailable_after_secs` (counted from
/// the run's start) reaches the person once.
#[tokio::test]
async fn cos_chat_authorization_safety_net_for_a_hung_run() {
    let mut f = fixture(true);
    f.tick();
    f.authorization("長い run");
    f.tick();
    assert_eq!(f.live_runs(), 1);
    f.advance(1190);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0, "within the safety net");
    f.advance(20);
    f.tick();
    f.assert_one_fallback("1200 秒を超えて終わらない");
    f.tick();
    assert_eq!(f.fallback_outbox(), 1);
}

/// 受け入れ条件 2: CoS never starts (stopped intake): not at 120 seconds, but once the safety
/// net from the item's arrival has passed.
#[tokio::test]
async fn cos_chat_authorization_safety_net_when_no_run_starts() {
    let mut f = fixture(true);
    f.d.accepting_new_work = false;
    f.tick();
    f.authorization("容量待ち");
    f.tick();
    f.advance(130);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0, "120 seconds do not apply");
    assert_eq!(f.state_count("pending"), 1);
    f.advance(1100);
    f.tick();
    f.assert_one_fallback("1200 秒を超えても始まらない");
    assert_eq!(f.inbox_runs(), 0);
}

/// The safety net is the configured value.
#[tokio::test]
async fn cos_chat_authorization_safety_net_is_configurable() {
    let mut f = fixture(true);
    f.d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .config
        .triage
        .authorization_unavailable_after_secs = 300;
    f.d.accepting_new_work = false;
    f.tick();
    f.authorization("短い上限");
    f.tick();
    f.advance(290);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0);
    f.advance(20);
    f.tick();
    f.assert_one_fallback("300 秒を超えても始まらない");
}

/// 受け入れ条件 1: the run failed: one outbox.
#[tokio::test]
async fn cos_chat_authorization_failed_run_routes_once() {
    let mut f = fixture_script(true, FAIL);
    f.tick();
    f.authorization("失敗");
    f.tick();
    assert_eq!(f.fallback_outbox(), 0);
    assert!(f.join_inbox().await);
    f.tick();
    f.assert_one_fallback("CoS run が失敗");
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.fallback_outbox(), 1);
}

/// 受け入れ条件 1: the run completed but left the item unhandled: one outbox.
#[tokio::test]
async fn cos_chat_authorization_completed_run_leaving_item_unhandled() {
    let mut f = fixture(true);
    f.tick();
    f.authorization("未処理");
    f.tick();
    assert!(f.join_inbox().await);
    f.tick();
    f.assert_one_fallback("未処理");
    f.tick();
    assert_eq!(f.fallback_outbox(), 1);
}

/// 受け入れ条件 1: the run was stopped. ADR 2026-10-07-cos-inbox-thread-conversation D1 gives
/// the item back to CoS once (a new run); that run ends without handling it: one outbox, and none
/// while the second run is live.
#[tokio::test]
async fn cos_chat_authorization_stopped_run_routes_once() {
    let mut f = fixture(true);
    f.tick();
    f.authorization("停止");
    f.tick();
    let run: String = f.count_text("SELECT run_id FROM chat_runs");
    // The person stopped the run (the worker handle is dropped: no outcome is written).
    f.d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .running
        .clear();
    f.store
        .chat_run_finish(
            &run,
            task_core::chat::ChatRunState::Stopped,
            Some("stopped by the person"),
            None,
            f.d.now_utc(),
        )
        .expect("finish");
    f.advance(130);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0, "CoS works on it again");
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_runs"), 2);
    assert_eq!(f.state_count("running"), 1);
    assert!(f.join_inbox().await);
    f.tick();
    f.assert_one_fallback("未処理");
    f.tick();
    assert_eq!(f.fallback_outbox(), 1);
}

/// 受け入れ条件 1: `cos.enabled=false`: one outbox at once.
#[tokio::test]
async fn cos_chat_authorization_disabled_routes_immediately() {
    let mut f = fixture(false);
    f.tick();
    f.authorization("無効");
    f.tick();
    assert_eq!(f.inbox_runs(), 0);
    f.assert_one_fallback("cos.enabled=false");
    f.tick();
    assert_eq!(f.fallback_outbox(), 1);
}

/// 受け入れ条件 1: CoS escalated the item (the resolve API claims the escalation outbox and marks
/// the item): exactly one outbox, the escalation, and no fallback later.
#[tokio::test]
async fn cos_chat_authorization_escalation_is_the_only_outbox() {
    let mut f = fixture(true);
    f.tick();
    f.authorization("人へ");
    f.tick();
    let item = f.only_item();
    f.advance(130);
    f.tick();
    f.store
        .cos_triage_outbox_claim(&item, "escalation", "{}", f.d.now_utc())
        .expect("claim")
        .expect("claimed");
    f.store
        .cos_triage_resolve(
            &item,
            "escalated",
            None,
            Some("standing_permission"),
            f.d.now_utc(),
        )
        .expect("resolve");
    assert!(f.join_inbox().await);
    for _ in 0..3 {
        f.tick();
    }
    f.advance(2000);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_escalation'"),
        1
    );
    assert_eq!(f.outbox_total(), 1);
}

/// 受け入れ条件 1: CoS answered or observed the item: nothing goes to Discord, even after the
/// run ends and the safety net has passed.
#[tokio::test]
async fn cos_chat_authorization_answered_or_observed_sends_nothing() {
    for outcome in ["answered", "observed"] {
        let mut f = fixture(true);
        f.tick();
        f.authorization(outcome);
        f.tick();
        let item = f.only_item();
        f.advance(130);
        f.tick();
        assert!(
            f.store
                .cos_triage_resolve(&item, outcome, None, Some("CoS が閉じた"), f.d.now_utc())
                .expect("resolve")
        );
        assert!(f.join_inbox().await);
        for _ in 0..3 {
            f.tick();
        }
        f.advance(2000);
        f.tick();
        assert_eq!(f.outbox_total(), 0, "{outcome}");
        assert_eq!(f.state_count(outcome), 1, "{outcome}");
    }
}

/// 認可以外の件は従来どおり: a question whose run is still live goes out after 120 seconds.
#[tokio::test]
async fn cos_chat_non_authorization_keeps_the_120_second_deadline() {
    let mut f = fixture(true);
    f.tick();
    f.question("質問", vec![]);
    f.tick();
    assert_eq!(f.live_runs(), 1);
    f.advance(130);
    f.tick();
    f.assert_one_fallback("120 秒");
}

#[test]
fn cos_chat_authorization_reason_is_deterministic() {
    let now = OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("now");
    let ts = |secs: i64| {
        (now - time::Duration::seconds(secs))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("ts")
    };
    let pending = FallbackCandidate {
        item_id: "i".into(),
        source_kind: "authorization".into(),
        source_key: "authorization-1".into(),
        source_revision: "r".into(),
        created_at: ts(1500),
        run_id: None,
        run_state: None,
        run_reason: None,
        run_started_at: None,
        route: None,
        summary: String::new(),
        detail: String::new(),
    };
    let fresh = FallbackCandidate {
        created_at: ts(300),
        ..pending.clone()
    };
    assert!(unavailable_reason(&fresh, true, DEADLINES, now).is_none());
    assert!(
        unavailable_reason(&pending, true, DEADLINES, now)
            .expect("no run")
            .contains("始まらない")
    );
    for state in ["queued", "running", "stopping"] {
        // Arrived long ago, but the run started 600 seconds ago.
        let live = FallbackCandidate {
            run_id: Some("run".into()),
            run_state: Some(state.into()),
            run_started_at: Some(ts(600)),
            ..pending.clone()
        };
        assert!(
            unavailable_reason(&live, true, DEADLINES, now).is_none(),
            "{state}"
        );
        let hung = FallbackCandidate {
            run_started_at: Some(ts(1300)),
            ..live
        };
        assert!(
            unavailable_reason(&hung, true, DEADLINES, now)
                .expect(state)
                .contains("終わらない")
        );
    }
    for (state, part) in [
        ("failed", "失敗"),
        ("stopped", "止まった"),
        ("interrupted", "止まった"),
        ("completed", "未処理"),
    ] {
        // Ended without an outcome: at once, long before the safety net.
        let ended = FallbackCandidate {
            run_id: Some("run".into()),
            run_state: Some(state.into()),
            run_started_at: Some(ts(5)),
            created_at: ts(10),
            ..pending.clone()
        };
        assert!(
            unavailable_reason(&ended, true, DEADLINES, now)
                .expect(state)
                .contains(part),
            "{state}"
        );
    }
    assert!(unavailable_reason(&fresh, false, DEADLINES, now).is_some());
    let missing = FallbackCandidate {
        run_id: Some("run".into()),
        created_at: ts(10),
        ..pending
    };
    assert!(
        unavailable_reason(&missing, true, DEADLINES, now)
            .expect("crash")
            .contains("crash")
    );
}
