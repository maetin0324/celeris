//! ADR-0133 付記: 本番 DB の写しで通知フィードの同期 1 回の所要時間を測る手動の計測。
//! `CELERIS_FEED_MEASURE_DB=<写しの path>` を与えたときだけ動く（先に写しの feed_* 表を空にしておく）。
use std::time::Instant;

use task_core::{NoticeStore, SqliteStore};
use time::OffsetDateTime;

#[test]
#[ignore = "manual measurement against a copy of a production database"]
fn measure_sync_on_database_copy() {
    let Ok(path) = std::env::var("CELERIS_FEED_MEASURE_DB") else {
        eprintln!("CELERIS_FEED_MEASURE_DB is not set; skipping");
        return;
    };
    let store = SqliteStore::open(std::path::Path::new(&path)).expect("open copy");
    let now = OffsetDateTime::now_utc();
    let latest = task_core::TaskStore::latest_event_id(&store).expect("latest event id");
    let mut caught_up_round = None;
    for round in 0..200 {
        let before = store.lock_counts();
        let started = Instant::now();
        let stats = task_ops::notify_feed::sync_notifications_counted(&store, now).expect("sync");
        let elapsed = started.elapsed().as_millis();
        let locks = store.lock_counts().since(before);
        let cursor = store.feed_cursor_get("events").expect("cursor");
        if round < 3 || cursor.as_deref() == Some(latest.to_string().as_str()) {
            eprintln!(
                "round {round}: {stats:?} locks={locks:?} cursor={cursor:?} elapsed_ms={elapsed}"
            );
        }
        if cursor.as_deref() == Some(latest.to_string().as_str()) {
            match caught_up_round {
                None => caught_up_round = Some(round),
                Some(first) if round >= first + 2 => break,
                Some(_) => {}
            }
        }
    }
}
