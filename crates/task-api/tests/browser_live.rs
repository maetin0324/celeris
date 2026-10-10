mod common;
use common::*;
use futures_util::StreamExt;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use task_api::browser::BrowserApiConfig;
use task_core::{RunIndexRole, RunIndexStatus, RunRow, Status, TaskKind, TaskStore};
use time::OffsetDateTime;
use tower::ServiceExt;

struct FrameEntry {
    key: (String, String),
    slot: std::sync::Arc<task_core::browser_live_frame::LatestFrameSlot>,
}
impl task_core::browser_isolation::LiveIsolation for FrameEntry {
    fn current_attestation(
        &self,
    ) -> Result<
        task_core::browser_isolation::IsolationAttestation,
        Vec<task_core::browser_isolation::IsolationViolation>,
    > {
        Err(vec![
            task_core::browser_isolation::IsolationViolation::SameUid,
        ])
    }
}
impl task_core::browser_isolation::LiveSessionEntry for FrameEntry {
    fn kind(&self) -> task_core::browser_isolation::RuntimeKind {
        task_core::browser_isolation::RuntimeKind::NotIsolated
    }
    fn accepts_state(&self) -> bool {
        false
    }
    fn deliver_state(&self, _: &[u8]) -> Result<(), task_core::browser_isolation::StateRejected> {
        Err(task_core::browser_isolation::StateRejected)
    }
    fn live_key(&self) -> Option<(String, String)> {
        Some(self.key.clone())
    }
    fn live_frames(
        &self,
    ) -> Option<std::sync::Arc<task_core::browser_live_frame::LatestFrameSlot>> {
        Some(self.slot.clone())
    }
}

fn sign(
    key: &ring::signature::Ed25519KeyPair,
    task: &str,
    run: &str,
    browser: &str,
    owner: &str,
    is_owner: bool,
) -> Value {
    sign_with_claims(
        key,
        Claims {
            task,
            run,
            browser,
            owner,
            is_owner,
            origin_ok: true,
            ttl: 30,
        },
    )
}
struct Claims<'a> {
    task: &'a str,
    run: &'a str,
    browser: &'a str,
    owner: &'a str,
    is_owner: bool,
    origin_ok: bool,
    ttl: i64,
}
fn sign_with_claims(key: &ring::signature::Ed25519KeyPair, claims: Claims<'_>) -> Value {
    let Claims {
        task,
        run,
        browser,
        owner,
        is_owner,
        origin_ok,
        ttl,
    } = claims;
    let payload = json!({"task_id":task,"run_id":run,"browser_session_id":browser,
        "owner_session_id":owner,"owner_session":is_owner,"origin_ok":origin_ok,
        "expires_at":OffsetDateTime::now_utc().unix_timestamp()+ttl})
    .to_string();
    let signature: String = key
        .sign(payload.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({"payload":payload,"signature":signature})
}
fn path(task: &str, run: &str, session: &str, method: &str) -> String {
    format!("/api/v1/tasks/{task}/browser/live/{run}/{session}/{method}")
}
fn start(env: &TestEnv, task: &str, run: &str) {
    env.store
        .run_index_start(RunRow {
            run_id: run.into(),
            task_id: task.into(),
            work_unit_id: None,
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Running,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: "2026-09-29T00:00:00Z".into(),
            finished_at: None,
        })
        .unwrap();
}
#[tokio::test]
async fn browser_live_frame_authorization_is_bound_and_scrubbed() {
    let env = admin_env();
    let key = {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    };
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    env.store
        .acquire_lease(task.id, "r1", std::time::Duration::from_secs(60))
        .unwrap();
    let id = task.id.to_string();
    start(&env, &id, "r1");
    let sessions = std::sync::Arc::new(task_core::browser_isolation::LiveSessions::default());
    let slot = std::sync::Arc::new(task_core::browser_live_frame::LatestFrameSlot::default());
    slot.publish(
        task_core::browser_live_frame::LiveFrame::new(
            1,
            1,
            1,
            task_core::browser_live_frame::LiveFrameEncoding::Jpeg,
            vec![0xff, 0xd8, 0xff],
        )
        .unwrap(),
    );
    sessions.insert(
        "browser-1",
        std::sync::Arc::new(FrameEntry {
            key: (id.clone(), "r1".into()),
            slot,
        }),
    );
    let app = task_api::router(
        env.state
            .clone()
            .with_browser(BrowserApiConfig {
                attestation_public_key: Some(key.public_key().as_ref().to_vec()),
                broker: None,
            })
            .with_live_sessions(sessions),
    );
    let other = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&other);
    let other_id = other.id.to_string();
    start(&env, &other_id, "r-other");
    let proof = sign(&key, &id, "r1", "browser-1", "owner", true);
    let grant = send(
        &app,
        post_admin(
            &path(&id, "r1", "browser-1", "grant"),
            &json!({"assertion":proof}),
        ),
    )
    .await;
    assert_eq!(grant.status, 200, "{}", grant.text());
    assert_eq!(grant.header("cache-control"), Some("no-store"));
    assert_eq!(grant.json()["frames_available"], true);
    assert_eq!(grant.json()["live_reason"], Value::Null);
    let grant_id = grant.json()["grant_id"].as_str().unwrap().to_string();
    let relay = json!({"assertion":proof,"grant_id":grant_id});
    assert_eq!(
        send(
            &app,
            post_admin(&path(&id, "r1", "browser-1", "check"), &relay)
        )
        .await
        .status,
        200
    );
    let frame_path = path(&id, "r1", "browser-1", "frames");
    let owner_frame = app
        .clone()
        .oneshot(post_admin(&frame_path, &relay))
        .await
        .unwrap();
    assert_eq!(owner_frame.status(), 200);
    assert_eq!(
        owner_frame.headers().get("cache-control").unwrap(),
        "no-store"
    );
    let mut body = owner_frame.into_body().into_data_stream();
    assert_eq!(
        body.next().await.unwrap().unwrap().as_ref(),
        &[0, 0, 0, 3, 0xff, 0xd8, 0xff]
    );
    let wrong_task = sign(&key, &other_id, "r-other", "browser-1", "owner", true);
    assert_problem(
        &send(
            &app,
            post_admin(
                &path(&other_id, "r-other", "browser-1", "frames"),
                &json!({"assertion":wrong_task,"grant_id":grant_id}),
            ),
        )
        .await,
        403,
        "other_task",
    );
    let wrong_session = sign(&key, &id, "r1", "browser-2", "owner", true);
    assert_problem(
        &send(
            &app,
            post_admin(
                &path(&id, "r1", "browser-2", "frames"),
                &json!({"assertion":wrong_session,"grant_id":grant_id}),
            ),
        )
        .await,
        403,
        "other_task",
    );
    let wrong_run = sign(&key, &id, "r2", "browser-1", "owner", true);
    assert_problem(
        &send(
            &app,
            post_admin(
                &path(&id, "r2", "browser-1", "frames"),
                &json!({"assertion":wrong_run,"grant_id":grant_id}),
            ),
        )
        .await,
        403,
        "other_task",
    );
    let wrong_owner = sign(&key, &id, "r1", "browser-1", "someone-else", true);
    assert_problem(
        &send(
            &app,
            post_admin(
                &frame_path,
                &json!({"assertion":wrong_owner,"grant_id":grant_id}),
            ),
        )
        .await,
        403,
        "not_owner_session",
    );
    let not_owner = sign(&key, &id, "r1", "browser-1", "owner", false);
    assert_problem(
        &send(
            &app,
            post_admin(
                &frame_path,
                &json!({"assertion":not_owner,"grant_id":grant_id}),
            ),
        )
        .await,
        403,
        "not_owner_session",
    );
    let bad_origin = sign_with_claims(
        &key,
        Claims {
            task: &id,
            run: "r1",
            browser: "browser-1",
            owner: "owner",
            is_owner: true,
            origin_ok: false,
            ttl: 30,
        },
    );
    assert_problem(
        &send(
            &app,
            post_admin(
                &frame_path,
                &json!({"assertion":bad_origin,"grant_id":grant_id}),
            ),
        )
        .await,
        403,
        "origin_mismatch",
    );
    let expired = sign_with_claims(
        &key,
        Claims {
            task: &id,
            run: "r1",
            browser: "browser-1",
            owner: "owner",
            is_owner: true,
            origin_ok: true,
            ttl: -1,
        },
    );
    assert_problem(
        &send(
            &app,
            post_admin(
                &frame_path,
                &json!({"assertion":expired,"grant_id":grant_id}),
            ),
        )
        .await,
        403,
        "not_owner_session",
    );
    assert_problem(
        &send(
            &app,
            post_admin(
                &frame_path,
                &json!({"assertion":proof,"grant_id":"expired-grant"}),
            ),
        )
        .await,
        410,
        "grant_expired",
    );
    let other_proof = sign(&key, &other_id, "r1", "browser-1", "owner", true);
    let resp = send(
        &app,
        post_admin(
            &path(&other_id, "r1", "browser-1", "check"),
            &json!({"assertion":other_proof,"grant_id":grant_id}),
        ),
    )
    .await;
    assert_problem(&resp, 403, "other_task");
    let other_session = sign(&key, &id, "r1", "browser-2", "owner", true);
    let resp = send(
        &app,
        post_admin(
            &path(&id, "r1", "browser-2", "check"),
            &json!({"assertion":other_session,"grant_id":grant_id}),
        ),
    )
    .await;
    assert_problem(&resp, 403, "other_task");
    let other_run = sign(&key, &id, "r2", "browser-1", "owner", true);
    let resp = send(
        &app,
        post_admin(
            &path(&id, "r2", "browser-1", "check"),
            &json!({"assertion":other_run,"grant_id":grant_id}),
        ),
    )
    .await;
    assert_problem(&resp, 403, "other_task");
    let wrong_owner = sign(&key, &id, "r1", "browser-1", "someone-else", true);
    let resp = send(
        &app,
        post_admin(
            &path(&id, "r1", "browser-1", "check"),
            &json!({"assertion":wrong_owner,"grant_id":grant_id}),
        ),
    )
    .await;
    assert_problem(&resp, 403, "not_owner_session");
    let not_owner = sign(&key, &id, "r1", "browser-1", "owner", false);
    assert_problem(
        &send(
            &app,
            post_admin(
                &path(&id, "r1", "browser-1", "grant"),
                &json!({"assertion":not_owner}),
            ),
        )
        .await,
        403,
        "not_owner_session",
    );
    for body in [
        json!({"kind":"url","url":"https://example.com/p?token=SENTINEL#SENTINEL"}),
        json!({"kind":"console","level":"error","text":"Set-Cookie: token=SENTINEL"}),
    ] {
        assert_eq!(
            send(
                &app,
                post_admin(&path(&id, "r1", "browser-1", "events"), &body)
            )
            .await
            .status,
            200
        );
    }
    let read = send(
        &app,
        post_admin(
            &format!("{}?after=1", path(&id, "r1", "browser-1", "read")),
            &relay,
        ),
    )
    .await;
    assert_eq!(read.status, 200, "{}", read.text());
    assert_eq!(read.json()["events"].as_array().unwrap().len(), 1);
    assert!(!read.text().contains("SENTINEL"));
    for suffix in ["", "?after=999"] {
        let reset = send(
            &app,
            post_admin(
                &format!("{}{}", path(&id, "r1", "browser-1", "read"), suffix),
                &relay,
            ),
        )
        .await;
        assert_eq!(reset.json()["plan"]["kind"], "reset");
    }
    let stop = send(
        &app,
        post_admin(
            &path(&id, "r1", "browser-1", "events"),
            &json!({"kind":"status","state":"observation_stopped"}),
        ),
    )
    .await;
    assert_eq!(stop.status, 200);
    assert_problem(
        &send(
            &app,
            post_admin(&path(&id, "r1", "browser-1", "check"), &relay),
        )
        .await,
        403,
        "observation_stopped",
    );
    assert_problem(
        &send(
            &app,
            post_admin(
                &path(&id, "r1", "browser-1", "grant"),
                &json!({"assertion":proof}),
            ),
        )
        .await,
        403,
        "observation_stopped",
    );
    env.store
        .run_index_finish(
            "r1",
            RunIndexStatus::Completed,
            None,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    assert_problem(
        &send(
            &app,
            post_admin(&path(&id, "r1", "browser-1", "check"), &relay),
        )
        .await,
        410,
        "run_ended",
    );
}
