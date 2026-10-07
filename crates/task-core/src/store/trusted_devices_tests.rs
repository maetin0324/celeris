//! ADR 2026-10-07-browser-trusted-devices: 信頼端末の store 試験。時刻は注入した偽の時計（UNIX 秒）。

use sha2::{Digest, Sha256};

use crate::model::Event;
use crate::store::{SqliteStore, TaskStore};
use crate::trusted_device::{
    NewTrustedDevice, TRUSTED_DEVICE_LIMIT, TRUSTED_DEVICE_TTL_SECS, TrustedDeviceMethod,
    TrustedDeviceRejectReason, TrustedDeviceRevokeReason, TrustedDeviceVerdict,
    trusted_device_event_task_id,
};

const T0: i64 = 1_800_000_000;
const DAY: i64 = 24 * 60 * 60;

/// web と同じ形の hash（`sha256("celeris-device\0" + secret)`）。試験では secret は任意の文字列。
fn hash(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"celeris-device\0");
    h.update(secret.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn new_device<'a>(id: &'a str, secret_hash: &'a str) -> NewTrustedDevice<'a> {
    NewTrustedDevice {
        id,
        name: "laptop",
        method: TrustedDeviceMethod::Cookie,
        secret_hash,
        absolute_expires_at: None,
        actor: "owner",
    }
}

fn register(store: &SqliteStore, id: &str, secret: &str, now: i64) -> TrustedDeviceVerdict {
    let h = hash(secret);
    store
        .trusted_device_register(&new_device(id, &h), now)
        .expect("register")
}

fn rotate(
    store: &SqliteStore,
    id: &str,
    presented: &str,
    next: &str,
    now: i64,
) -> TrustedDeviceVerdict {
    store
        .trusted_device_verify_and_rotate(id, &hash(presented), &hash(next), "owner", now)
        .expect("verify_and_rotate")
}

fn device_events(store: &SqliteStore) -> Vec<Event> {
    store
        .events_for(trusted_device_event_task_id())
        .expect("events")
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

fn accepted(v: TrustedDeviceVerdict) -> crate::trusted_device::TrustedDevice {
    match v {
        TrustedDeviceVerdict::Accepted(d) => d,
        other => panic!("expected accepted, got {other:?}"),
    }
}

#[test]
fn trusted_device_register_sets_sliding_expiry_and_lists_without_hash() {
    let store = SqliteStore::open_in_memory().expect("store");
    let d = accepted(register(&store, "dev-1", "s0", T0));
    assert_eq!(d.created_at, T0);
    assert_eq!(d.expires_at, T0 + TRUSTED_DEVICE_TTL_SECS);
    assert_eq!(TRUSTED_DEVICE_TTL_SECS, 90 * DAY);
    assert_eq!(d.absolute_expires_at, None);
    assert_eq!(d.last_used_at, None);
    assert!(d.is_active(T0));
    let list = store.trusted_device_list().expect("list");
    assert_eq!(list, vec![d.clone()]);
    let json = serde_json::to_string(&list).expect("json");
    assert!(
        !json.contains(&hash("s0")),
        "list must not carry hashes: {json}"
    );
    assert!(matches!(
        device_events(&store).as_slice(),
        [Event::TrustedDeviceRegistered { device_id, actor, expires_at, absolute_expires_at: None, .. }]
            if device_id == "dev-1" && actor == "owner" && *expires_at == d.expires_at
    ));
}

#[test]
fn trusted_device_rotate_replaces_hash_extends_and_old_secret_no_longer_current() {
    let store = SqliteStore::open_in_memory().expect("store");
    register(&store, "dev-1", "s0", T0);
    let now = T0 + 10 * DAY;
    let d = accepted(rotate(&store, "dev-1", "s0", "s1", now));
    assert_eq!(d.last_used_at, Some(now));
    assert_eq!(d.expires_at, now + 90 * DAY);
    // 新しい秘密で次も通る（再び回転）。
    let later = now + 80 * DAY;
    let d2 = accepted(rotate(&store, "dev-1", "s1", "s2", later));
    assert_eq!(d2.expires_at, later + 90 * DAY);
    assert!(matches!(
        device_events(&store).as_slice(),
        [
            Event::TrustedDeviceRegistered { .. },
            Event::TrustedDeviceUsed { expires_at: e1, .. },
            Event::TrustedDeviceUsed { expires_at: e2, .. },
        ] if *e1 == now + 90 * DAY && *e2 == later + 90 * DAY
    ));
}

#[test]
fn trusted_device_reuse_of_previous_secret_revokes_device() {
    let store = SqliteStore::open_in_memory().expect("store");
    register(&store, "dev-1", "s0", T0);
    accepted(rotate(&store, "dev-1", "s0", "s1", T0 + 1));
    // 盗まれた旧秘密 s0 の再提示 → 使い回しとして失効。
    assert_eq!(
        rotate(&store, "dev-1", "s0", "x1", T0 + 2),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Reuse)
    );
    let d = store
        .trusted_device_get("dev-1")
        .expect("get")
        .expect("row");
    assert_eq!(d.revoked_at, Some(T0 + 2));
    assert_eq!(d.revoked_reason, Some(TrustedDeviceRevokeReason::Reuse));
    // 本人の現行秘密 s1 も以後は通らない。
    assert_eq!(
        rotate(&store, "dev-1", "s1", "s2", T0 + 3),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Revoked)
    );
    let events = device_events(&store);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRevoked { device_id, reason: TrustedDeviceRevokeReason::Reuse, .. } if device_id == "dev-1"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRejected { device_id: Some(id), reason: TrustedDeviceRejectReason::Reuse, .. } if id == "dev-1"
    )));
}

#[test]
fn trusted_device_expired_is_rejected_and_unused_device_expires_after_90_days() {
    let store = SqliteStore::open_in_memory().expect("store");
    register(&store, "dev-1", "s0", T0);
    let expiry = T0 + 90 * DAY;
    assert_eq!(
        store
            .trusted_device_verify_readonly("dev-1", &hash("s0"), expiry - 1)
            .expect("ro"),
        TrustedDeviceVerdict::Accepted(
            store
                .trusted_device_get("dev-1")
                .expect("get")
                .expect("row")
        )
    );
    assert_eq!(
        rotate(&store, "dev-1", "s0", "s1", expiry),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Expired)
    );
    // 期限切れの拒否では回転しない。
    let d = store
        .trusted_device_get("dev-1")
        .expect("get")
        .expect("row");
    assert_eq!(d.last_used_at, None);
    assert!(!d.is_active(expiry));
}

#[test]
fn trusted_device_revoked_is_rejected_immediately() {
    let store = SqliteStore::open_in_memory().expect("store");
    register(&store, "dev-1", "s0", T0);
    assert!(
        store
            .trusted_device_revoke("dev-1", "owner", T0 + 5)
            .expect("revoke")
    );
    assert!(
        !store
            .trusted_device_revoke("dev-1", "owner", T0 + 6)
            .expect("revoke again")
    );
    assert!(
        !store
            .trusted_device_revoke("nope", "owner", T0 + 6)
            .expect("revoke unknown")
    );
    assert_eq!(
        rotate(&store, "dev-1", "s0", "s1", T0 + 7),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Revoked)
    );
    assert_eq!(
        store
            .trusted_device_verify_readonly("dev-1", &hash("s0"), T0 + 7)
            .expect("ro"),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Revoked)
    );
    let d = store
        .trusted_device_get("dev-1")
        .expect("get")
        .expect("row");
    assert_eq!(d.revoked_reason, Some(TrustedDeviceRevokeReason::Owner));
    let revoked = device_events(&store)
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Event::TrustedDeviceRevoked {
                    reason: TrustedDeviceRevokeReason::Owner,
                    ..
                }
            )
        })
        .count();
    assert_eq!(revoked, 1);
}

#[test]
fn trusted_device_wrong_secret_and_unknown_id_are_rejected() {
    let store = SqliteStore::open_in_memory().expect("store");
    register(&store, "dev-1", "s0", T0);
    assert_eq!(
        rotate(&store, "dev-1", "wrong", "s1", T0 + 1),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Mismatch)
    );
    // 不一致では失効しない（本人の秘密は生きている）。
    accepted(rotate(&store, "dev-1", "s0", "s1", T0 + 2));
    assert_eq!(
        rotate(&store, "attacker-chosen-id", "s0", "s1", T0 + 3),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Unknown)
    );
    // 実在しない id は events に書かない。
    let events = device_events(&store);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRejected {
            device_id: None,
            reason: TrustedDeviceRejectReason::Unknown,
            ..
        }
    )));
    let json = serde_json::to_string(&events).expect("json");
    assert!(!json.contains("attacker-chosen-id"));
}

#[test]
fn trusted_device_limit_rejects_sixth_active_device() {
    let store = SqliteStore::open_in_memory().expect("store");
    assert_eq!(TRUSTED_DEVICE_LIMIT, 5);
    for i in 0..TRUSTED_DEVICE_LIMIT {
        accepted(register(&store, &format!("dev-{i}"), &format!("s{i}"), T0));
    }
    assert_eq!(
        register(&store, "dev-6", "s6", T0 + 1),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Limit)
    );
    assert_eq!(
        store.trusted_device_list().expect("list").len(),
        TRUSTED_DEVICE_LIMIT
    );
    assert!(device_events(&store).iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRejected {
            device_id: None,
            reason: TrustedDeviceRejectReason::Limit,
            ..
        }
    )));
    // 1 台失効させると空きができる（失効・期限切れは数えない）。
    assert!(
        store
            .trusted_device_revoke("dev-0", "owner", T0 + 2)
            .expect("revoke")
    );
    accepted(register(&store, "dev-6", "s6", T0 + 3));
    // 期限切れの端末も数えない。
    let after_expiry = T0 + 90 * DAY + 10;
    accepted(register(&store, "dev-7", "s7", after_expiry));
}

#[test]
fn trusted_device_absolute_cap_bounds_extension_and_none_means_unbounded() {
    let store = SqliteStore::open_in_memory().expect("store");
    // 人の決定は「絶対上限なし」（None）。使い続ければ 90 日を何度でも越える。
    register(&store, "free", "f0", T0);
    let mut now = T0;
    let mut cur = String::from("f0");
    for i in 1..=4 {
        now += 80 * DAY;
        let next = format!("f{i}");
        let d = accepted(rotate(&store, "free", &cur, &next, now));
        assert_eq!(d.expires_at, now + 90 * DAY);
        cur = next;
    }
    assert!(now - T0 > 300 * DAY);
    // 絶対上限を持つ行は、延長がそれを越えず、上限で拒否される。
    let abs = T0 + 100 * DAY;
    let h = hash("c0");
    let capped = NewTrustedDevice {
        absolute_expires_at: Some(abs),
        ..new_device("capped", &h)
    };
    let d = accepted(
        store
            .trusted_device_register(&capped, T0)
            .expect("register"),
    );
    assert_eq!(d.expires_at, T0 + 90 * DAY);
    let d = accepted(rotate(&store, "capped", "c0", "c1", T0 + 50 * DAY));
    assert_eq!(d.expires_at, abs);
    assert_eq!(
        rotate(&store, "capped", "c1", "c2", abs),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Expired)
    );
}

#[test]
fn trusted_device_readonly_verify_writes_nothing() {
    let store = SqliteStore::open_in_memory().expect("store");
    let before = accepted(register(&store, "dev-1", "s0", T0));
    let events_before = device_events(&store).len();
    for _ in 0..3 {
        assert_eq!(
            store
                .trusted_device_verify_readonly("dev-1", &hash("s0"), T0 + DAY)
                .expect("ro"),
            TrustedDeviceVerdict::Accepted(before.clone())
        );
    }
    assert_eq!(
        store
            .trusted_device_verify_readonly("dev-1", &hash("bad"), T0 + DAY)
            .expect("ro"),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Mismatch)
    );
    accepted(rotate(&store, "dev-1", "s0", "s1", T0 + 2 * DAY));
    let after_rotate = store
        .trusted_device_get("dev-1")
        .expect("get")
        .expect("row");
    let n = device_events(&store).len();
    // 回転前の秘密を readonly で見ても失効させない。
    assert_eq!(
        store
            .trusted_device_verify_readonly("dev-1", &hash("s0"), T0 + 3 * DAY)
            .expect("ro"),
        TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Reuse)
    );
    assert_eq!(
        store
            .trusted_device_get("dev-1")
            .expect("get")
            .expect("row"),
        after_rotate
    );
    assert_eq!(device_events(&store).len(), n);
    assert_eq!(n, events_before + 1);
}

#[test]
fn trusted_device_events_carry_no_secret_or_hash() {
    let store = SqliteStore::open_in_memory().expect("store");
    let secrets = ["sec-alpha", "sec-beta", "sec-gamma", "sec-delta"];
    register(&store, "dev-1", secrets[0], T0);
    accepted(rotate(&store, "dev-1", secrets[0], secrets[1], T0 + 1));
    rotate(&store, "dev-1", "sec-wrong", secrets[2], T0 + 2);
    rotate(&store, "dev-1", secrets[0], secrets[3], T0 + 3); // reuse → revoke
    rotate(&store, "dev-1", secrets[1], secrets[2], T0 + 4); // revoked
    for i in 0..TRUSTED_DEVICE_LIMIT + 1 {
        register(
            &store,
            &format!("more-{i}"),
            &format!("more-secret-{i}"),
            T0 + 5,
        );
    }
    // events 表の生の json を読み、秘密の値・hash のどちらも含まないことを確かめる。
    let conn = store.lock().expect("lock");
    let mut stmt = conn
        .prepare("SELECT json FROM events WHERE task_id = ?1")
        .expect("prepare");
    let rows: Vec<String> = stmt
        .query_map([trusted_device_event_task_id().to_string()], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert!(rows.len() >= 8, "{rows:?}");
    let mut needles: Vec<String> = secrets.iter().map(|s| s.to_string()).collect();
    needles.push("sec-wrong".into());
    needles.extend((0..=TRUSTED_DEVICE_LIMIT).map(|i| format!("more-secret-{i}")));
    let hashes: Vec<String> = needles.iter().map(|s| hash(s)).collect();
    for json in &rows {
        for n in needles.iter().chain(hashes.iter()) {
            assert!(!json.contains(n.as_str()), "event leaks {n}: {json}");
        }
        assert!(
            !json.contains("hash"),
            "event mentions a hash field: {json}"
        );
    }
}

#[test]
fn trusted_device_rejects_malformed_hashes_as_invalid() {
    let store = SqliteStore::open_in_memory().expect("store");
    assert!(
        store
            .trusted_device_register(&new_device("dev-1", "not-a-hash"), T0)
            .is_err()
    );
    register(&store, "dev-1", "s0", T0);
    assert!(
        store
            .trusted_device_verify_and_rotate("dev-1", &hash("s0"), "ZZ", "owner", T0 + 1)
            .is_err()
    );
    assert!(
        store
            .trusted_device_verify_and_rotate("dev-1", &hash("s0"), &hash("s0"), "owner", T0 + 1)
            .is_err()
    );
    // 失敗した呼び出しは何も変えていない。
    accepted(rotate(&store, "dev-1", "s0", "s1", T0 + 2));
}
