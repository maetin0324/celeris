use super::*;
use crate::daemon::SnapshotTaskWait;

fn snapshot_at(last_tick_at: &str, tick_ms: u64, task_id: TaskId) -> DaemonSnapshot {
    serde_json::from_value(serde_json::json!({
        "instance_id": "i", "pid": 1, "hostname": "h", "started_at": last_tick_at,
        "last_tick_at": last_tick_at, "ticks": 1, "tick_ms": tick_ms,
        "in_flight": [], "cooldowns": [], "approvals_pending": 0, "decisions_open": 0, "providers": [],
        "task_waits": [SnapshotTaskWait { task_id, wait: TaskWait::ReviewAwaitingHuman }],
    }))
    .expect("snapshot")
}

#[test]
fn task_wait_api_memory_waits_only_from_a_fresh_snapshot() {
    let id = TaskId::new();
    let now = OffsetDateTime::parse("2026-10-11T11:05:00Z", &Rfc3339).expect("now");
    // tick 2 秒 × 10 = 20 秒以内なら使う。
    let fresh = snapshot_at("2026-10-11T11:04:45Z", 2000, id);
    assert_eq!(
        MemoryWaits::from_snapshot(Some(&fresh), now).get(id),
        Some(&TaskWait::ReviewAwaitingHuman)
    );
    let stale = snapshot_at("2026-10-11T11:04:30Z", 2000, id);
    assert_eq!(MemoryWaits::from_snapshot(Some(&stale), now).get(id), None);
    assert_eq!(MemoryWaits::from_snapshot(None, now).get(id), None);
}
