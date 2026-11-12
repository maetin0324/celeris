//! ADR 2026-11-11-cos-fallback-timing: the fallback waits for CoS's first response for every
//! kind. The safety net (`fallback_after_secs`, 1200 in the fixture) counts from the start of the
//! live run (the item's own, or the run it waits behind) or from the time the item became
//! pickable, never from its arrival. Time is the injected test clock; runs are awaited by joining
//! the worker handle, never by sleeping.
//!
//! Invariants the older tests could not catch: they only covered the authorization kind (the
//! other kinds used the 120-second deadline from arrival), and no test had a live run of the
//! thread that this process does not own, which left the process-local "maybe pending" hint
//! cleared with items still pending.

use super::*;
use crate::dispatcher::cos_chat::triage::TriageSkip;

impl Fixture {
    fn item_of(&self, key_like: &str) -> String {
        self.count_text(&format!(
            "SELECT id FROM cos_inbox_items WHERE source_key LIKE '{key_like}'"
        ))
    }

    fn launch_skip(&self, thread: &str) -> Option<TriageSkip> {
        self.d
            .cos_chat_launch
            .as_ref()
            .expect("launch")
            .triage
            .launch_skips
            .get(thread)
            .copied()
    }

    /// A live run of the thread written by another daemon (a draining process after a release
    /// switch): SQLite has it, this process has no handle for it.
    fn foreign_live_run(&self, thread: &str) -> String {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let run = "01FOREIGNRUN000000000000000".to_owned();
        let at = self
            .d
            .now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .expect("ts");
        conn.execute(
            "INSERT INTO chat_runs(run_id,thread_id,input_message_id,state,started_at) \
             VALUES(?1,?2,'foreign-input','running',?3)",
            rusqlite::params![run, thread, at],
        )
        .expect("foreign run");
        run
    }

    fn finish_foreign_run(&self, run: &str) {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let at = self
            .d
            .now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .expect("ts");
        conn.execute(
            "UPDATE chat_runs SET state='completed',finished_at=?2 WHERE run_id=?1",
            rusqlite::params![run, at],
        )
        .expect("finish foreign run");
    }
}

/// (a) A non-authorization item whose own run is live from 121 seconds up to the safety net gets
/// no fallback; when the run ends without an outcome it gets exactly one.
#[tokio::test]
async fn cos_fallback_timing_no_fallback_while_its_own_run_is_live() {
    let mut f = fixture(true);
    f.tick();
    f.question("一次対応中", vec![]);
    f.tick();
    assert_eq!(f.state_count("running"), 1);
    for step in [121, 300, 600, 160] {
        f.advance(step);
        f.tick();
        assert_eq!(f.fallback_outbox(), 0, "run live, {step} s more");
    }
    assert_eq!(f.state_count("running"), 1);
    assert!(f.join_inbox().await);
    f.tick();
    f.assert_one_fallback("未処理");
    f.tick();
    assert_eq!(f.fallback_outbox(), 1);
}

/// (a) The safety net catches a run that hangs, counted from the run's start.
#[tokio::test]
async fn cos_fallback_timing_safety_net_for_a_hung_run_of_any_kind() {
    let mut f = fixture(true);
    f.tick();
    f.question("長い run", vec![]);
    f.tick();
    f.advance(1190);
    f.tick();
    assert_eq!(f.fallback_outbox(), 0);
    f.advance(20);
    f.tick();
    f.assert_one_fallback("1200 秒を超えて終わらない");
}

/// (b) An item that arrives while its thread's run is live waits behind it without timing out,
/// and the run's end lets the next tick pick it.
#[tokio::test]
async fn cos_fallback_timing_item_behind_a_live_run_is_picked_when_it_ends() {
    let mut f = fixture(true);
    f.tick();
    f.question("先の件", vec![]);
    f.tick();
    let thread = f.inbox_thread().expect("thread");
    assert_eq!(f.inbox_runs(), 1);
    let notice = f.notice("report:behind", NoticeKind::Report);
    f.tick();
    let behind = f.item_of(&notice.to_string());
    assert_eq!(f.item_state(&behind), "pending");
    assert_eq!(f.launch_skip(&thread), Some(TriageSkip::ThreadRunning));
    f.advance(1100);
    f.tick();
    assert_eq!(
        f.item_state(&behind),
        "pending",
        "not timed out behind the run"
    );
    assert_eq!(f.fallback_outbox(), 0);
    assert!(f.join_inbox().await);
    f.tick();
    assert_eq!(f.item_state(&behind), "running", "picked on the next tick");
    assert_eq!(f.inbox_runs(), 2);
    assert_eq!(f.launch_skip(&thread), None);
}

/// (b) The run the item waits behind hangs: the safety net counts from that run's start.
#[tokio::test]
async fn cos_fallback_timing_behind_a_hung_run_uses_the_run_start() {
    let mut f = fixture(true);
    f.tick();
    f.question("先の件", vec![]);
    f.tick();
    f.advance(600);
    let notice = f.notice("report:hung", NoticeKind::Report);
    f.tick();
    let behind = f.item_of(&notice.to_string());
    f.advance(590);
    f.tick();
    assert_eq!(f.item_state(&behind), "pending");
    f.advance(20);
    f.tick();
    assert_eq!(f.item_state(&behind), "fallback");
    let reason: String = f.count_text(&format!(
        "SELECT reason FROM cos_inbox_items WHERE id='{behind}'"
    ));
    assert!(
        reason.contains("前の run が 1200 秒を超えて終わらず"),
        "{reason}"
    );
}

/// (c) Standby → active: an item ingested while the daemon took no new work is picked on the
/// first tick after the switch, and the skip reason is recorded meanwhile.
#[tokio::test]
async fn cos_fallback_timing_standby_to_active_picks_on_the_next_tick() {
    let mut f = fixture(true);
    f.d.accepting_new_work = false;
    f.tick();
    f.question("切り替え", vec![]);
    f.tick();
    let thread = f.inbox_thread().expect("thread");
    assert_eq!(f.launch_skip(&thread), Some(TriageSkip::NotAccepting));
    assert_eq!(f.state_count("pending"), 1);
    f.advance(130);
    f.tick();
    assert_eq!(
        f.fallback_outbox(),
        0,
        "the old 120-second deadline is gone"
    );
    f.d.accepting_new_work = true;
    f.tick();
    assert_eq!(f.state_count("running"), 1);
    assert_eq!(f.inbox_runs(), 1);
}

/// (c) A live run of the thread that this process does not own (SQLite only): the claim is
/// refused and recorded as such; once that run ends, the next tick picks the item. Before this
/// change the refused claim cleared the in-memory hint and the item stayed pending until the
/// fallback deadline.
#[tokio::test]
async fn cos_fallback_timing_idle_thread_after_a_foreign_run_is_picked() {
    let mut f = fixture(true);
    f.d.accepting_new_work = false;
    f.tick();
    f.question("引き継ぎ", vec![]);
    f.tick();
    let thread = f.inbox_thread().expect("thread");
    let foreign = f.foreign_live_run(&thread);
    f.d.accepting_new_work = true;
    f.tick();
    assert_eq!(f.state_count("pending"), 1);
    assert_eq!(f.launch_skip(&thread), Some(TriageSkip::RunInDb));
    f.advance(1100);
    f.tick();
    assert_eq!(
        f.state_count("pending"),
        1,
        "behind the live run: no fallback"
    );
    assert_eq!(f.fallback_outbox(), 0);
    f.finish_foreign_run(&foreign);
    f.tick();
    assert_eq!(f.state_count("running"), 1, "picked on the next tick");
    assert_eq!(f.fallback_outbox(), 0);
}

/// (c) A pickable item that CoS still cannot start (no slot, stopped intake) is handed over once
/// the safety net has passed since it became pickable — the end of the thread's last run, not
/// its arrival.
#[tokio::test]
async fn cos_fallback_timing_pickable_clock_starts_at_the_end_of_the_last_run() {
    let mut f = fixture(true);
    f.tick();
    f.question("先の件", vec![]);
    f.tick();
    let notice = f.notice("report:pickable", NoticeKind::Report);
    f.tick();
    let behind = f.item_of(&notice.to_string());
    f.advance(1000);
    f.tick();
    // The run ends; the daemon stops taking new work at the same moment.
    f.d.accepting_new_work = false;
    assert!(f.join_inbox().await);
    f.tick();
    assert_eq!(f.item_state(&behind), "pending");
    f.advance(1000);
    f.tick();
    assert_eq!(
        f.item_state(&behind),
        "pending",
        "2000 s after arrival, 1000 s pickable"
    );
    f.advance(210);
    f.tick();
    assert_eq!(f.item_state(&behind), "fallback");
}
