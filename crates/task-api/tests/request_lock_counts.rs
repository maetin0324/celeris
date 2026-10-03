//! ADR-0133 付記: 主要な GET（受信箱・通知・組織・task 詳細）と通知フィードの同期 1 回が、
//! events の量に比例した書き込み接続・SQL を使わないことを、時計ではなく
//! `SqliteStore::lock_counts`（接続を取った回数）と `FeedSyncStats`（読んだ行・書き込み回数）で固定する。
mod common;
use common::*;
use task_core::{Event, LockCounts, Status, Task, TaskKind};
use time::OffsetDateTime;

const PATHS: &[&str] = &[
    "/api/v1/inbox",
    "/api/v1/inbox/items",
    "/api/v1/notifications",
    "/api/v1/notifications/unread-count",
    "/api/v1/org",
    "/api/v1/browser/waits",
];

fn history(n: usize) -> Vec<Event> {
    (0..n)
        .map(|i| Event::Transitioned {
            from: if i % 2 == 0 {
                Status::Ready
            } else {
                Status::Running
            },
            to: if i % 2 == 0 {
                Status::Running
            } else {
                Status::Ready
            },
            reason: "history".into(),
        })
        .collect()
}

fn seed(env: &TestEnv, tasks: usize, events_per_task: usize) -> Vec<Task> {
    let statuses = [Status::Done, Status::Failed, Status::Ready, Status::Blocked];
    (0..tasks)
        .map(|i| {
            let task = new_task(TaskKind::Execute, statuses[i % statuses.len()]);
            env.seed_with(&task, history(events_per_task));
            task
        })
        .collect()
}

async fn counts(env: &TestEnv, path: &str) -> LockCounts {
    let app = env.router();
    let before = env.state.store_lock_counts();
    let resp = send(&app, get_admin(path)).await;
    assert_eq!(resp.status, 200, "{path}: {}", resp.text());
    env.state.store_lock_counts().since(before)
}

/// 1 つの DB で、通知フィードを追いつかせてから各 GET の接続回数を測る。
async fn measure(tasks: usize, events_per_task: usize) -> Vec<(String, LockCounts)> {
    let env = admin_env();
    let seeded = seed(&env, tasks, events_per_task);
    let now = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 100, "feed sync never caught up");
        let before = env.store.lock_counts();
        let stats = task_ops::notify_feed::sync_notifications_counted(&env.store, now).unwrap();
        let locks = env.store.lock_counts().since(before);
        // tick 1 回分の同期は events を予算までしか読まず、書き込みは記録と位置の更新だけ。
        assert!(
            stats.events_scanned <= task_ops::notify_feed::EVENT_BUDGET as u64,
            "{stats:?}"
        );
        assert!(
            locks.writer <= stats.record_calls + 3,
            "{stats:?} {locks:?}"
        );
        if stats.events_scanned == 0 && stats.record_calls == 0 {
            // 追いついた後の同期は書き込み接続を取らない。
            assert_eq!(locks.writer, 0, "{stats:?} {locks:?}");
            assert!(locks.reader <= 10, "{stats:?} {locks:?}");
            break;
        }
    }
    let mut out = Vec::new();
    for path in PATHS {
        out.push((path.to_string(), counts(&env, path).await));
    }
    let detail = format!("/api/v1/tasks/{}", seeded[1].id);
    out.push((
        "/api/v1/tasks/{id}".to_string(),
        counts(&env, &detail).await,
    ));
    out
}

#[tokio::test]
async fn main_reads_and_feed_sync_do_not_scale_with_event_volume() {
    const TASKS: usize = 300;
    // 同じ task 数で events を 1,800 行と 21,300 行にする。
    let small = measure(TASKS, 5).await;
    let large = measure(TASKS, 70).await;
    assert_eq!(
        small, large,
        "lock counts must not depend on the number of events"
    );
    for (path, locks) in &large {
        let max_writer = match path.as_str() {
            "/api/v1/notifications" | "/api/v1/notifications/unread-count" | "/api/v1/org" => 0,
            "/api/v1/browser/waits" | "/api/v1/tasks/{id}" => 1,
            // 受信箱: 書き込み接続は task 数によらない定数回（読むだけの照会は読み取り接続）。
            _ => 4,
        };
        assert!(locks.writer <= max_writer, "{path}: {locks:?}");
        // 読み取りは task 1 件あたり高々 2 回（events を task ごとに読む既存の受信箱の形）。
        assert!(locks.reader <= 2 * TASKS as u64, "{path}: {locks:?}");
    }
}
