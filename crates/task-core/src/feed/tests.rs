//! ADR-0133 D3: 通知の store（冪等な追加・束ね・既読化・未読数・一覧）の試験。
//! 試験名に `notification_feed` を含める（葉の check の絞り込み用）。

use time::OffsetDateTime;

use super::*;
use crate::store::SqliteStore;

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().unwrap()
}

fn ev(source: &str, kind: NoticeKind, group: &str, title: &str, at: OffsetDateTime) -> NoticeEvent {
    NoticeEvent {
        source_key: source.to_string(),
        kind,
        group_key: group.to_string(),
        title: title.to_string(),
        summary: title.to_string(),
        project_id: Some("p1".to_string()),
        task_id: None,
        target: Some(NoticeTarget {
            kind: "task".to_string(),
            id: source.to_string(),
        }),
        links: vec![NoticeLink {
            label: "task".to_string(),
            href: format!("/tasks/{source}"),
        }],
        at,
    }
}

// 2026-10-02 10:00 / 11:00 / 12:00 UTC。
const T0: OffsetDateTime = at(1_790_935_200);
const T1: OffsetDateTime = at(1_790_938_800);
const T2: OffsetDateTime = at(1_790_942_400);

const fn at(unix: i64) -> OffsetDateTime {
    match OffsetDateTime::from_unix_timestamp(unix) {
        Ok(t) => t,
        Err(_) => panic!("timestamp"),
    }
}

#[test]
fn notification_feed_record_is_idempotent_per_source_key() {
    let s = store();
    let e = ev(
        "event:1",
        NoticeKind::TaskDone,
        "task_done:project:p1",
        "A",
        T0,
    );
    let first = s.notice_record(&e).unwrap();
    let NoticeRecordOutcome::Created(id) = first else {
        panic!("expected Created, got {first:?}");
    };
    // 同じ出来事を何度足しても 1 件・件数 1 のまま。
    for _ in 0..3 {
        assert_eq!(
            s.notice_record(&e).unwrap(),
            NoticeRecordOutcome::Duplicate(id)
        );
    }
    let page = s.notice_list(&NoticeQuery::default()).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].count, 1);
    assert_eq!(
        page.items[0].target.as_ref().map(|t| t.id.as_str()),
        Some("event:1")
    );
    assert_eq!(page.items[0].links.len(), 1);
}

#[test]
fn notification_feed_bundles_same_group_key_and_counts() {
    let s = store();
    let g = "report:project:p1";
    let a = s
        .notice_record(&ev("report:a", NoticeKind::Report, g, "A", T0))
        .unwrap();
    let b = s
        .notice_record(&ev("report:b", NoticeKind::Report, g, "B", T2))
        .unwrap();
    // 時刻の古い出来事が後から届いても last_at は戻らない。
    let c = s
        .notice_record(&ev("report:c", NoticeKind::Report, g, "C", T1))
        .unwrap();
    let id = a.notice_id();
    assert_eq!(b, NoticeRecordOutcome::Bundled(id));
    assert_eq!(c, NoticeRecordOutcome::Bundled(id));
    let n = s.notice_get(id).unwrap().unwrap();
    assert_eq!(n.count, 3);
    assert_eq!(n.title, "C");
    assert_eq!(n.summary, "C（ほか 2 件）");
    assert_eq!(n.first_at, T0);
    assert_eq!(n.last_at, T2);
    // 別の group_key は別の束。
    let other = s
        .notice_record(&ev(
            "report:x",
            NoticeKind::Report,
            "report:project:p2",
            "X",
            T0,
        ))
        .unwrap();
    assert!(matches!(other, NoticeRecordOutcome::Created(o) if o != id));
    assert_eq!(s.notice_unread_count().unwrap().total, 2);
}

#[test]
fn notification_feed_read_bundle_starts_a_new_bundle() {
    let s = store();
    let g = "task_done:project:p1";
    let id = s
        .notice_record(&ev("event:1", NoticeKind::TaskDone, g, "A", T0))
        .unwrap()
        .notice_id();
    assert!(s.notice_mark_read(id, T1).unwrap());
    // 既読の束には足さない: 新しい束になる。既読化の 2 回目は false。
    assert!(!s.notice_mark_read(id, T2).unwrap());
    let next = s
        .notice_record(&ev("event:2", NoticeKind::TaskDone, g, "B", T2))
        .unwrap();
    assert!(matches!(next, NoticeRecordOutcome::Created(n) if n != id));
    // 既読の束の出来事を再び足しても重複のまま（数えない）。
    assert_eq!(
        s.notice_record(&ev("event:1", NoticeKind::TaskDone, g, "A", T0))
            .unwrap(),
        NoticeRecordOutcome::Duplicate(id)
    );
    let old = s.notice_get(id).unwrap().unwrap();
    assert_eq!(old.count, 1);
    assert_eq!(old.read_at, Some(T1));
    assert_eq!(s.notice_unread_count().unwrap().total, 1);
}

#[test]
fn notification_feed_mark_all_read_and_unread_count_by_kind() {
    let s = store();
    s.notice_record(&ev(
        "e:1",
        NoticeKind::TaskDone,
        "task_done:project:p1",
        "A",
        T0,
    ))
    .unwrap();
    s.notice_record(&ev(
        "e:2",
        NoticeKind::TaskDone,
        "task_done:project:p2",
        "B",
        T0,
    ))
    .unwrap();
    s.notice_record(&ev("e:3", NoticeKind::Release, "release", "R", T1))
        .unwrap();
    s.notice_record(&ev(
        "e:4",
        NoticeKind::BadNews,
        "bad_news:project:p1",
        "X",
        T1,
    ))
    .unwrap();
    let c = s.notice_unread_count().unwrap();
    assert_eq!(c.total, 4);
    assert_eq!(c.by_kind.get("task_done"), Some(&2));
    assert_eq!(c.by_kind.get("release"), Some(&1));
    assert_eq!(c.by_kind.get("report"), None);

    // 種類を絞った全件既読。
    assert_eq!(
        s.notice_mark_all_read(&[NoticeKind::TaskDone], T2).unwrap(),
        2
    );
    let c = s.notice_unread_count().unwrap();
    assert_eq!(c.total, 2);
    assert_eq!(c.by_kind.get("task_done"), None);
    // 全件既読。2 回目は 0 件。
    assert_eq!(s.notice_mark_all_read(&[], T2).unwrap(), 2);
    assert_eq!(s.notice_mark_all_read(&[], T2).unwrap(), 0);
    assert_eq!(
        s.notice_unread_count().unwrap(),
        NoticeUnreadCount::default()
    );
}

#[test]
fn notification_feed_list_filters_unread_kind_and_pages() {
    let s = store();
    for i in 0..5u8 {
        let at = T0 + time::Duration::minutes(i64::from(i));
        s.notice_record(&ev(
            &format!("e:{i}"),
            NoticeKind::Report,
            &format!("report:project:p{i}"),
            &format!("R{i}"),
            at,
        ))
        .unwrap();
    }
    let rel = s
        .notice_record(&ev("e:rel", NoticeKind::Release, "release", "REL", T2))
        .unwrap()
        .notice_id();
    s.notice_mark_read(rel, T2).unwrap();

    let all = s.notice_list(&NoticeQuery::default()).unwrap();
    assert_eq!(all.total, 6);
    assert_eq!(all.items[0].title, "REL"); // 新しい順

    let unread = s
        .notice_list(&NoticeQuery {
            unread_only: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(unread.total, 5);
    assert!(unread.items.iter().all(Notice::is_unread));

    let releases = s
        .notice_list(&NoticeQuery {
            kinds: vec![NoticeKind::Release],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(releases.total, 1);

    let page2 = s
        .notice_list(&NoticeQuery {
            kinds: vec![NoticeKind::Report],
            limit: 2,
            offset: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page2.total, 5);
    let titles: Vec<&str> = page2.items.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, vec!["R2", "R1"]);
}

#[test]
fn notification_feed_prune_and_cursor() {
    let s = store();
    let id = s
        .notice_record(&ev(
            "e:1",
            NoticeKind::TaskDone,
            "task_done:project:p1",
            "A",
            T0,
        ))
        .unwrap()
        .notice_id();
    s.notice_record(&ev("e:2", NoticeKind::Release, "release", "R", T0))
        .unwrap();
    s.notice_mark_read(id, T0).unwrap();
    // 未読は消さない。既読でも before 以降は消さない。
    assert_eq!(s.notice_prune_read(T0).unwrap(), 0);
    assert_eq!(s.notice_prune_read(T1).unwrap(), 1);
    assert!(s.notice_get(id).unwrap().is_none());
    assert_eq!(s.notice_list(&NoticeQuery::default()).unwrap().total, 1);

    assert_eq!(s.feed_cursor_get("events").unwrap(), None);
    s.feed_cursor_set("events", "10").unwrap();
    s.feed_cursor_set("events", "12").unwrap();
    assert_eq!(s.feed_cursor_get("events").unwrap().as_deref(), Some("12"));
}

#[test]
fn notification_feed_kind_round_trips_and_summary_is_deterministic() {
    for k in NoticeKind::ALL {
        assert_eq!(NoticeKind::parse(k.as_str()), Some(k));
        let json = serde_json::to_string(&k).unwrap();
        assert_eq!(json, format!("\"{}\"", k.as_str()));
    }
    assert_eq!(NoticeKind::parse("task_failed"), None);
    assert_eq!(bundled_summary("x", 1), "x");
    assert_eq!(bundled_summary("x", 4), "x（ほか 3 件）");
}

#[test]
fn notification_feed_migration_skips_reserved_versions_and_fills_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.sqlite3");
    drop(SqliteStore::open(&path).unwrap());
    let versions = |p: &std::path::Path| -> Vec<u32> {
        let conn = rusqlite::Connection::open(p).unwrap();
        let mut stmt = conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, u32>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    let v = versions(&path);
    assert_eq!(v.last(), Some(&crate::store::SCHEMA_VERSION));
    for reserved in crate::store::migrations::RESERVED_VERSIONS {
        assert!(!v.contains(reserved), "reserved {reserved} recorded");
    }
    // 0041 の記録が無い DB（38〜40 は予約で飛び、42〜45 は当たり済み）を開くと
    // 飛んだ 0041 だけが当たる。
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DELETE FROM schema_migrations WHERE version = 41; \
             DROP TABLE feed_notices; DROP TABLE feed_sources; DROP TABLE feed_cursor;",
        )
        .unwrap();
    }
    let s = SqliteStore::open(&path).unwrap();
    assert_eq!(s.notice_unread_count().unwrap().total, 0);
    assert_eq!(versions(&path), v);
}

/// ADR-0133 付記: まとめての記録は 1 回の書き込み接続で、重複・束ね・走査位置を `notice_record` と同じに扱う。
#[test]
fn notification_feed_batch_records_in_one_write_and_skips_known_sources() {
    let s = store();
    s.notice_record(&ev("event:1", NoticeKind::TaskDone, "g", "一", T0))
        .unwrap();
    let before = s.lock_counts();
    let outcomes = s
        .notice_record_batch(
            &[
                ev("event:1", NoticeKind::TaskDone, "g", "一", T0),
                ev("event:2", NoticeKind::TaskDone, "g", "二", T1),
                ev("event:3", NoticeKind::Report, "r", "三", T2),
            ],
            &[("events", "3".to_string())],
        )
        .unwrap();
    assert_eq!(s.lock_counts().since(before).writer, 1);
    assert!(matches!(outcomes[0], NoticeRecordOutcome::Duplicate(_)));
    assert!(matches!(outcomes[1], NoticeRecordOutcome::Bundled(_)));
    assert!(matches!(outcomes[2], NoticeRecordOutcome::Created(_)));
    assert_eq!(s.feed_cursor_get("events").unwrap().as_deref(), Some("3"));
    let known = s
        .notice_sources_known(&["event:1".into(), "event:2".into(), "event:9".into()])
        .unwrap();
    assert_eq!(known.len(), 2);
    assert!(!known.contains("event:9"));
    // 何も無ければ書き込み接続を取らない。
    let before = s.lock_counts();
    assert!(s.notice_record_batch(&[], &[]).unwrap().is_empty());
    assert_eq!(s.lock_counts().since(before).writer, 0);
}
