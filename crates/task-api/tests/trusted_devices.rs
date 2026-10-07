//! ADR 2026-10-07-browser-trusted-devices: `/api/v1/browser/trusted-devices` の結合試験。
//! 時刻は `ApiState::with_clock` の偽の時計で決める（期限切れは時計を進めて再現する）。

mod common;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use axum::Router;
use common::*;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use task_api::browser::BrowserApiConfig;
use task_core::TaskStore;
use task_core::model::Event;
use task_core::trusted_device::{TRUSTED_DEVICE_TTL_SECS, trusted_device_event_task_id};

const BASE: &str = "/api/v1/browser/trusted-devices";
const T0: i64 = 1_800_000_000;

struct Fixture {
    env: TestEnv,
    app: Router,
    key: Ed25519KeyPair,
    clock: Arc<AtomicI64>,
}

fn fixture() -> Fixture {
    let env = admin_env();
    let key = {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    };
    let clock = Arc::new(AtomicI64::new(T0));
    let c = Arc::clone(&clock);
    let app = task_api::router(
        env.state
            .clone()
            .with_browser(BrowserApiConfig {
                attestation_public_key: Some(key.public_key().as_ref().to_vec()),
                broker: None,
            })
            .with_clock(Arc::new(move || c.load(Ordering::SeqCst))),
    );
    Fixture {
        env,
        app,
        key,
        clock,
    }
}

fn hash(tag: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("celeris-device\0{tag}").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Fixture {
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    fn sign_with(&self, key: &Ed25519KeyPair, mut claims: Value) -> Value {
        let obj = claims.as_object_mut().unwrap();
        obj.entry("owner_session_id").or_insert(json!("sess-1"));
        obj.entry("actor_id").or_insert(json!("owner"));
        obj.entry("expires_at").or_insert(json!(self.now() + 20));
        let payload = claims.to_string();
        let signature: String = key
            .sign(payload.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        json!({"payload": payload, "signature": signature})
    }

    fn sign(&self, claims: Value) -> Value {
        self.sign_with(&self.key, claims)
    }

    async fn register(&self, name: &str, secret_hash: &str) -> Resp {
        let assertion = self.sign(json!({"purpose":"device_register","owner_session":true,
            "name":name,"presented_hash":secret_hash}));
        send(
            &self.app,
            post_admin(
                BASE,
                &json!({"name":name,"secret_hash":secret_hash,"assertion":assertion}),
            ),
        )
        .await
    }

    async fn register_ok(&self, name: &str, secret_hash: &str) -> String {
        let resp = self.register(name, secret_hash).await;
        assert_eq!(resp.status, 201, "{}", resp.text());
        resp.json()["device"]["id"].as_str().unwrap().to_string()
    }

    async fn verify(&self, id: &str, presented: &str, next: Option<&str>) -> Resp {
        let readonly = next.is_none();
        let mut claims = json!({"purpose":"device_resume","owner_session":false,
            "device_id":id,"presented_hash":presented,"readonly":readonly});
        let mut body = json!({"device_id":id,"presented_hash":presented,"readonly":readonly});
        if let Some(n) = next {
            claims["next_hash"] = json!(n);
            body["next_hash"] = json!(n);
        }
        body["assertion"] = self.sign(claims);
        send(&self.app, post_admin(&format!("{BASE}/verify"), &body)).await
    }

    fn assertion_headers(&self, claims: Value) -> Vec<(String, String)> {
        let a = self.sign(claims);
        vec![
            (
                "authorization".to_string(),
                "Bearer s3cret-token-value".to_string(),
            ),
            (
                "x-celeris-assertion-payload".to_string(),
                a["payload"].as_str().unwrap().to_string(),
            ),
            (
                "x-celeris-assertion-signature".to_string(),
                a["signature"].as_str().unwrap().to_string(),
            ),
        ]
    }

    async fn list(&self) -> Resp {
        let h = self.assertion_headers(json!({"purpose":"device_list","owner_session":true}));
        let h: Vec<(&str, &str)> = h.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        send(&self.app, get_with(BASE, &h)).await
    }

    async fn revoke(&self, id: &str) -> Resp {
        let h = self.assertion_headers(
            json!({"purpose":"device_revoke","owner_session":true,"device_id":id}),
        );
        let h: Vec<(&str, &str)> = h.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        send(&self.app, delete_with(&format!("{BASE}/{id}"), &h)).await
    }

    fn events(&self) -> Vec<Event> {
        self.env
            .store
            .events_for(trusted_device_event_task_id())
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
}

#[tokio::test]
async fn trusted_device_register_verify_rotates_secret() {
    let f = fixture();
    let id = f.register_ok("laptop", &hash("s1")).await;
    f.advance(60);
    let resp = f.verify(&id, &hash("s1"), Some(&hash("s2"))).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let device = &resp.json()["device"];
    assert_eq!(device["id"], id.as_str());
    assert_eq!(device["last_used_at"], T0 + 60);
    assert_eq!(device["expires_at"], T0 + 60 + TRUSTED_DEVICE_TTL_SECS);
    // 回転後は新しい秘密で通り、さらに回転する。
    f.advance(60);
    let resp = f.verify(&id, &hash("s2"), Some(&hash("s3"))).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert!(
        f.events()
            .iter()
            .any(|e| matches!(e, Event::TrustedDeviceRegistered { actor, .. } if actor == "owner"))
    );
    assert_eq!(
        f.events()
            .iter()
            .filter(|e| matches!(e, Event::TrustedDeviceUsed { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn trusted_device_old_secret_replay_revokes_device() {
    let f = fixture();
    let id = f.register_ok("phone", &hash("a1")).await;
    assert_eq!(
        f.verify(&id, &hash("a1"), Some(&hash("a2"))).await.status,
        200
    );
    // 回転前の秘密の再提示: 拒否し、端末を失効させる。
    let resp = f.verify(&id, &hash("a1"), Some(&hash("a3"))).await;
    assert_problem(&resp, 403, "device_rejected");
    // 現行の秘密でも以後は使えない。
    let resp = f.verify(&id, &hash("a2"), Some(&hash("a4"))).await;
    assert_problem(&resp, 403, "device_rejected");
    let list = f.list().await;
    assert_eq!(list.json()["devices"][0]["revoked_reason"], "reuse");
    assert!(f.events().iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRevoked { device_id, .. } if device_id == &id
    )));
}

#[tokio::test]
async fn trusted_device_expired_with_fake_clock_is_rejected() {
    let f = fixture();
    let id = f.register_ok("desk", &hash("e1")).await;
    // 期限のちょうど 1 秒前は通る（使うと延長される）。
    f.advance(TRUSTED_DEVICE_TTL_SECS - 1);
    assert_eq!(
        f.verify(&id, &hash("e1"), Some(&hash("e2"))).await.status,
        200
    );
    // 最後の使用から 90 日経つと期限切れ。
    f.advance(TRUSTED_DEVICE_TTL_SECS);
    let resp = f.verify(&id, &hash("e2"), Some(&hash("e3"))).await;
    assert_problem(&resp, 403, "device_rejected");
    assert!(f.events().iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRejected { reason, .. }
            if *reason == task_core::trusted_device::TrustedDeviceRejectReason::Expired
    )));
}

#[tokio::test]
async fn trusted_device_revoked_device_is_rejected() {
    let f = fixture();
    let id = f.register_ok("tablet", &hash("r1")).await;
    let resp = f.revoke(&id).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json()["revoked"], true);
    assert_eq!(resp.json()["device"]["revoked_reason"], "owner");
    // 2 回目は冪等（失効済み）。
    let again = f.revoke(&id).await;
    assert_eq!(again.json()["revoked"], false);
    let resp = f.verify(&id, &hash("r1"), Some(&hash("r2"))).await;
    assert_problem(&resp, 403, "device_rejected");
    // readonly でも拒否する。
    assert_problem(
        &f.verify(&id, &hash("r1"), None).await,
        403,
        "device_rejected",
    );
    // 失効の event は actor 付き。
    assert!(f.events().iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRevoked { device_id, actor, .. } if device_id == &id && actor == "owner"
    )));
    // 未知の id は 404。
    assert_problem(
        &f.revoke("01J9ZX5T3K8Q7W6V5R4P3N2M1J").await,
        404,
        "device_not_found",
    );
}

#[tokio::test]
async fn trusted_device_invalid_assertion_is_rejected() {
    let f = fixture();
    let h = hash("x1");
    let other_key = {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    };
    let body = |assertion: Value| json!({"name":"x","secret_hash":h,"assertion":assertion});
    let good =
        json!({"purpose":"device_register","owner_session":true,"name":"x","presented_hash":h});
    // 別の鍵の署名。
    let wrong_key = f.sign_with(&other_key, good.clone());
    // 署名後に payload を書き換えた。
    let mut tampered = f.sign(good.clone());
    tampered["payload"] = json!(
        tampered["payload"]
            .as_str()
            .unwrap()
            .replace("\"x\"", "\"y\"")
    );
    // 用途の取り違え（一覧の assertion で登録）。
    let wrong_purpose = f.sign(json!({"purpose":"device_list","owner_session":true}));
    // owner session ではない。
    let not_owner = f.sign(
        json!({"purpose":"device_register","owner_session":false,"name":"x","presented_hash":h}),
    );
    // 期限切れ・期限が遠すぎる。
    let mut expired = good.clone();
    expired["expires_at"] = json!(f.now() - 1);
    let expired = f.sign(expired);
    let mut too_far = good.clone();
    too_far["expires_at"] = json!(f.now() + 600);
    let too_far = f.sign(too_far);
    // 本文と claims の不一致（別の hash を登録しようとする）。
    let unbound = f.sign(json!({"purpose":"device_register","owner_session":true,
        "name":"x","presented_hash":hash("other")}));
    // Live View の RelayClaims は端末の claims として通らない。
    let relay = f.sign(json!({"task_id":"t","run_id":"r","browser_session_id":"b",
        "owner_session":true,"origin_ok":true}));
    for assertion in [
        wrong_key,
        tampered,
        wrong_purpose,
        not_owner,
        expired,
        too_far,
        unbound,
        relay,
    ] {
        let resp = send(&f.app, post_admin(BASE, &body(assertion))).await;
        assert_problem(&resp, 403, "not_owner_session");
    }
    // daemon token が無ければ 401。
    let resp = send(&f.app, post_json(BASE, &body(f.sign(good)))).await;
    assert_eq!(resp.status, 401);
    // GET・DELETE は header の assertion が無ければ拒否。
    assert_problem(
        &send(&f.app, get_admin(BASE)).await,
        403,
        "not_owner_session",
    );
    assert!(
        f.env.store.trusted_device_list().unwrap().is_empty(),
        "nothing registered"
    );
}

#[tokio::test]
async fn trusted_device_limit_is_five_active_devices() {
    let f = fixture();
    let mut ids = Vec::new();
    for i in 0..5 {
        ids.push(
            f.register_ok(&format!("d{i}"), &hash(&format!("l{i}")))
                .await,
        );
    }
    let resp = f.register("d5", &hash("l5")).await;
    let problem = assert_problem(&resp, 409, "device_limit");
    assert_eq!(problem["limit"], 5);
    // 1 台失効させると登録できる。
    assert_eq!(f.revoke(&ids[0]).await.status, 200);
    assert_eq!(f.register("d5", &hash("l5")).await.status, 201);
    assert!(f.events().iter().any(|e| matches!(
        e,
        Event::TrustedDeviceRejected { reason, device_id: None, .. }
            if *reason == task_core::trusted_device::TrustedDeviceRejectReason::Limit
    )));
}

#[tokio::test]
async fn trusted_device_list_has_no_hashes() {
    let f = fixture();
    let secret = hash("list-secret");
    let id = f.register_ok("workstation", &secret).await;
    let next = hash("list-next");
    assert_eq!(f.verify(&id, &secret, Some(&next)).await.status, 200);
    let resp = f.list().await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["limit"], 5);
    assert_eq!(body["now"], T0);
    let device = &body["devices"][0];
    for key in ["id", "name", "created_at", "last_used_at", "expires_at"] {
        assert!(device.get(key).is_some(), "{key} missing: {device}");
    }
    assert!(device.get("revoked_at").is_some());
    let text = resp.text();
    assert!(!text.contains(&secret), "current-1 hash leaked");
    assert!(!text.contains(&next), "current hash leaked");
    assert!(!text.contains("hash"), "hash field present: {text}");
    // events にも hash は載らない。
    let events = serde_json::to_string(&f.events()).unwrap();
    assert!(!events.contains(&secret) && !events.contains(&next));
}

#[tokio::test]
async fn trusted_device_readonly_verify_does_not_change_db() {
    let f = fixture();
    let secret = hash("ro1");
    let id = f.register_ok("probe-target", &secret).await;
    let before_list = f.env.store.trusted_device_list().unwrap();
    let before_events = f.events().len();
    f.advance(3600);
    for _ in 0..3 {
        let resp = f.verify(&id, &secret, None).await;
        assert_eq!(resp.status, 200, "{}", resp.text());
        assert_eq!(resp.json()["device"]["last_used_at"], Value::Null);
    }
    // 誤った秘密の readonly 検証も何も書かない（拒否の event も無い）。
    assert_problem(
        &f.verify(&id, &hash("wrong"), None).await,
        403,
        "device_rejected",
    );
    // readonly と next_hash の組み合わせは不正。
    let claims = json!({"purpose":"device_resume","owner_session":false,"device_id":id,
        "presented_hash":secret,"next_hash":hash("n"),"readonly":true});
    let body = json!({"device_id":id,"presented_hash":secret,"next_hash":hash("n"),
        "readonly":true,"assertion":f.sign(claims)});
    assert_problem(
        &send(&f.app, post_admin(&format!("{BASE}/verify"), &body)).await,
        422,
        "device_invalid",
    );
    assert_eq!(f.env.store.trusted_device_list().unwrap(), before_list);
    assert_eq!(f.events().len(), before_events);
    // 回転していないので、同じ秘密で通常の検証が通る。
    assert_eq!(f.verify(&id, &secret, Some(&hash("ro2"))).await.status, 200);
}

#[tokio::test]
async fn trusted_device_wrong_secret_is_rejected() {
    let f = fixture();
    let id = f.register_ok("laptop", &hash("w1")).await;
    assert_problem(
        &f.verify(&id, &hash("nope"), Some(&hash("w2"))).await,
        403,
        "device_rejected",
    );
    // 未知の id も同じ応答（区別しない）。
    assert_problem(
        &f.verify("01J9ZX5T3K8Q7W6V5R4P3N2M1J", &hash("w1"), Some(&hash("w2")))
            .await,
        403,
        "device_rejected",
    );
    // 不一致では失効しない（正しい秘密は通る）。
    assert_eq!(
        f.verify(&id, &hash("w1"), Some(&hash("w2"))).await.status,
        200
    );
}
