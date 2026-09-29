mod common;
use common::*;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use task_api::browser::BrowserApiConfig;
use task_core::{RunIndexRole, RunIndexStatus, RunRow, Status, TaskKind, TaskStore};
use time::OffsetDateTime;

fn sign(
    key: &ring::signature::Ed25519KeyPair,
    task: &str,
    run: &str,
    browser: &str,
    owner: &str,
    is_owner: bool,
) -> Value {
    let payload = json!({"task_id":task,"run_id":run,"browser_session_id":browser,
        "owner_session_id":owner,"owner_session":is_owner,"origin_ok":true,
        "expires_at":OffsetDateTime::now_utc().unix_timestamp()+30})
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
async fn grant_check_and_replay_are_bound_and_scrubbed() {
    let env = admin_env();
    let key = {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    };
    let app = task_api::router(env.state.clone().with_browser(BrowserApiConfig {
        attestation_public_key: Some(key.public_key().as_ref().to_vec()),
        broker: None,
    }));
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    env.store
        .acquire_lease(task.id, "r1", std::time::Duration::from_secs(60))
        .unwrap();
    let id = task.id.to_string();
    start(&env, &id, "r1");
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
    let other = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&other);
    let other_id = other.id.to_string();
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
