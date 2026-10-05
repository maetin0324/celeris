//! ADR 2026-10-04-multi-objective-model-routing Phase 4（p4-shadow-eval）: 実行 shadow の日次上限の共有予約（0049）。

use std::path::Path;
use std::sync::{Arc, Barrier};

use time::OffsetDateTime;

use super::tests::sample_task;
use super::*;
use crate::model::{Event, Status};
use crate::model_router::shadow::{
    SHADOW_ALLOW_ANY, ShadowAllowlist, ShadowDailyCaps, ShadowKind, ShadowPolicy, ShadowReason,
    ShadowRecord, ShadowReservation, ShadowReservationRequest, ShadowSettlement, ShadowStatus,
    ShadowTarget,
};

fn at(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
}

/// 注入する時計（試験の中で進める）。
struct FakeClock(std::sync::Mutex<OffsetDateTime>);

impl FakeClock {
    fn new(at: OffsetDateTime) -> Self {
        Self(std::sync::Mutex::new(at))
    }
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().unwrap()
    }
    fn set(&self, at: OffsetDateTime) {
        *self.0.lock().unwrap() = at;
    }
}

fn policy() -> ShadowPolicy {
    ShadowPolicy {
        execute: true,
        allowlist: ShadowAllowlist {
            task_kinds: vec!["coding".into()],
            roles: vec![SHADOW_ALLOW_ANY.into()],
            lanes: vec!["cheap".into()],
            sources: vec!["qwen-local".into()],
        },
        sample_rate: 1.0,
        daily_max_requests: Some(6),
        daily_max_tokens: Some(5_000),
        daily_max_effective_usd: Some(1.0),
        max_concurrency: Some(2),
        max_queue_depth: Some(8),
        timeout_ms: Some(5_000),
    }
}

fn target() -> ShadowTarget {
    ShadowTarget {
        task_kind: "coding".into(),
        role: "implementer".into(),
        lane: "cheap".into(),
        source: "qwen-local".into(),
    }
}

fn req(shadow_id: &str, owner: &str, tokens: u64, usd: Option<f64>) -> ShadowReservationRequest {
    ShadowReservationRequest {
        shadow_id: shadow_id.into(),
        owner: owner.into(),
        worst_tokens: tokens,
        worst_effective_usd: usd,
    }
}

fn reserved_id(r: ShadowReservation) -> String {
    match r {
        ShadowReservation::Reserved { reservation_id, .. } => reservation_id,
        other => panic!("expected reserved, got {other:?}"),
    }
}

fn row_count(path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM routing_shadow_reservations",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn assert_within(caps: &ShadowDailyCaps, store: &SqliteStore, now: OffsetDateTime) {
    let u = store.routing_shadow_usage(now).unwrap();
    assert!(u.requests <= caps.max_requests, "{u:?}");
    assert!(u.tokens <= caps.max_tokens, "{u:?}");
    assert!(u.effective_usd <= caps.max_effective_usd + 1e-9, "{u:?}");
}

#[test]
fn routing_shadow_opt_in_caps_survive_restart_and_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.sqlite3");
    let clock = FakeClock::new(at("2026-10-05T23:00:00Z"));
    let policy = policy();
    let caps = policy.daily_caps().unwrap();

    // ---- off・対象外・標本外・未知の費用は予約 0（行を書かない）。
    {
        let store = SqliteStore::open(&path).unwrap();
        let off = ShadowPolicy::default();
        let r = store
            .routing_shadow_admit_and_reserve(
                &off,
                &target(),
                "d0",
                &req("s0", "a", 10, Some(0.001)),
                clock.now(),
            )
            .unwrap();
        assert_eq!(r, ShadowReservation::Denied(ShadowReason::Off));
        let mut other = target();
        other.lane = "frontier".into();
        let r = store
            .routing_shadow_admit_and_reserve(
                &policy,
                &other,
                "d0",
                &req("s0", "a", 10, Some(0.001)),
                clock.now(),
            )
            .unwrap();
        assert_eq!(r, ShadowReservation::Denied(ShadowReason::NotAllowlisted));
        let mut sampled = policy.clone();
        sampled.sample_rate = 0.0;
        let r = store
            .routing_shadow_admit_and_reserve(
                &sampled,
                &target(),
                "d0",
                &req("s0", "a", 10, Some(0.001)),
                clock.now(),
            )
            .unwrap();
        assert_eq!(r, ShadowReservation::Denied(ShadowReason::SampledOut));
        for unknown in [None, Some(f64::NAN), Some(-0.1)] {
            let r = store
                .routing_shadow_admit_and_reserve(
                    &policy,
                    &target(),
                    "d0",
                    &req("s0", "a", 10, unknown),
                    clock.now(),
                )
                .unwrap();
            assert_eq!(r, ShadowReservation::Denied(ShadowReason::UnknownCost));
        }
        assert_eq!(row_count(&path), 0);
        assert_eq!(store.routing_shadow_usage(clock.now()).unwrap().requests, 0);

        // unknown cost は dropped として event に残る（primary とは別欄。本文は無い）。
        let task = sample_task(Status::Running);
        store.insert(&task).unwrap();
        let record = ShadowRecord {
            shadow_id: "s0".into(),
            primary_decision_id: "d0".into(),
            kind: ShadowKind::Execution,
            status: ShadowStatus::Dropped,
            reason: Some(ShadowReason::UnknownCost),
            detail: None,
            policy_version: "shadow-v1".into(),
            run_id: Some("run-1".into()),
            request_id: None,
            candidate_model: Some("qwen3".into()),
            candidate_source: Some("qwen-local".into()),
            input_tokens: None,
            output_tokens: None,
            output_sha256: None,
            cash_usd: None,
            effective_usd: None,
            latency_ms: None,
            reservation_id: None,
        };
        record.validate().unwrap();
        let event = Event::RoutingShadowRecorded {
            record: Box::new(record),
        };
        store.append_event(task.id, &event).unwrap();
        let events = store.events_for(task.id).unwrap();
        assert_eq!(events.last().map(|(_, e)| e), Some(&event));
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(wire["type"], "routing_shadow_recorded");
        assert_eq!(wire["status"], "dropped");
        assert_eq!(wire["reason"], "unknown_cost");
    }

    // ---- 予約・確定・上限（usd）。
    let r2 = {
        let store = SqliteStore::open(&path).unwrap();
        let r1 = reserved_id(
            store
                .routing_shadow_admit_and_reserve(
                    &policy,
                    &target(),
                    "d1",
                    &req("s1", "a", 1_000, Some(0.01)),
                    clock.now(),
                )
                .unwrap(),
        );
        assert!(
            store
                .routing_shadow_settle(
                    &r1,
                    ShadowSettlement::Completed {
                        tokens: 400,
                        effective_usd: 0.004
                    },
                    clock.now()
                )
                .unwrap()
        );
        // 確定は 1 回だけ（2 回目は何もしない）。
        assert!(
            !store
                .routing_shadow_settle(&r1, ShadowSettlement::TimedOut, clock.now())
                .unwrap()
        );
        let r = store
            .routing_shadow_reserve(&caps, &req("s-big", "a", 100, Some(2.0)), clock.now())
            .unwrap();
        assert_eq!(r, ShadowReservation::Denied(ShadowReason::CapExceeded));
        // 2 件目は予約したまま process が落ちる（確定しない）。
        reserved_id(
            store
                .routing_shadow_reserve(&caps, &req("s2", "a", 1_000, Some(0.01)), clock.now())
                .unwrap(),
        )
    };

    // ---- 再起動: store を開き直しても予約中の分を数える。timeout は予約額を計上する。
    {
        let store = SqliteStore::open(&path).unwrap();
        let u = store.routing_shadow_usage(clock.now()).unwrap();
        assert_eq!((u.requests, u.tokens, u.open_reservations), (2, 1_400, 1));
        assert!((u.effective_usd - 0.014).abs() < 1e-9, "{u:?}");
        assert!(
            store
                .routing_shadow_settle(&r2, ShadowSettlement::TimedOut, clock.now())
                .unwrap()
        );
        let u = store.routing_shadow_usage(clock.now()).unwrap();
        assert_eq!((u.requests, u.tokens, u.open_reservations), (2, 1_400, 0));
        assert_within(&caps, &store, clock.now());
    }

    // ---- UTC 日界: 偽時計を翌日へ進めると新しい日の枠になる（前日は残る）。
    let day1 = clock.now();
    clock.set(at("2026-10-06T00:00:01Z"));
    // +09:00 で同じ瞬間を渡しても同じ UTC 日に数える。
    let day2_jst = at("2026-10-06T09:00:01+09:00");
    {
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.routing_shadow_usage(clock.now()).unwrap().requests, 0);
        assert_eq!(store.routing_shadow_usage(day1).unwrap().requests, 2);
    }

    // ---- handoff / 2 instance: 2 接続が同時に予約しても上限を越えない（tokens が先に尽きる）。
    let a = Arc::new(SqliteStore::open(&path).unwrap());
    let b = Arc::new(SqliteStore::open(&path).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let now = clock.now();
    let handles: Vec<_> = [("instance-a", a.clone()), ("instance-b", b.clone())]
        .into_iter()
        .map(|(owner, store)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let mut got = Vec::new();
                for i in 0..8 {
                    let r = store
                        .routing_shadow_reserve(
                            &caps,
                            &req(&format!("{owner}-{i}"), owner, 1_000, Some(0.01)),
                            now,
                        )
                        .unwrap();
                    if let ShadowReservation::Reserved {
                        reservation_id,
                        day,
                    } = r
                    {
                        assert_eq!(day, "2026-10-06");
                        got.push(reservation_id);
                    } else {
                        assert_eq!(r, ShadowReservation::Denied(ShadowReason::CapExceeded));
                    }
                }
                got
            })
        })
        .collect();
    let reserved: Vec<String> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    assert_eq!(reserved.len(), 5, "token cap 5000 / 1000 per request");
    assert_within(&caps, &a, now);
    assert_within(&caps, &b, day2_jst);
    let u = b.routing_shadow_usage(day2_jst).unwrap();
    assert_eq!((u.requests, u.tokens, u.open_reservations), (5, 5_000, 5));

    // 失敗は実測が小さくても予約額で数える（送信済みの推論は取り消せない）。
    assert!(
        b.routing_shadow_settle(
            &reserved[0],
            ShadowSettlement::Failed {
                tokens: Some(10),
                effective_usd: Some(0.0001)
            },
            now
        )
        .unwrap()
    );
    // 別 instance が確定した予約は、もう一方からは確定できない（二重計上しない）。
    assert!(
        !a.routing_shadow_settle(&reserved[0], ShadowSettlement::TimedOut, now)
            .unwrap()
    );
    assert_eq!(a.routing_shadow_usage(now).unwrap().tokens, 5_000);

    // completed で実測が小さければ枠が戻り、request 上限（6）まで入る。
    assert!(
        a.routing_shadow_settle(
            &reserved[1],
            ShadowSettlement::Completed {
                tokens: 0,
                effective_usd: 0.0
            },
            now
        )
        .unwrap()
    );
    reserved_id(
        a.routing_shadow_reserve(&caps, &req("tail", "instance-a", 500, Some(0.01)), now)
            .unwrap(),
    );
    let r = b
        .routing_shadow_reserve(&caps, &req("over", "instance-b", 0, Some(0.0)), now)
        .unwrap();
    assert_eq!(r, ShadowReservation::Denied(ShadowReason::CapExceeded));
    assert_within(&caps, &a, now);
    assert_eq!(a.routing_shadow_usage(now).unwrap().requests, 6);
}

#[test]
fn routing_shadow_budget_migration_is_additive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.sqlite3");
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in (1..=48).filter(|v| !super::migrations::RESERVED_VERSIONS.contains(v)) {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
    assert_eq!(row_count(&path), 0);
}
