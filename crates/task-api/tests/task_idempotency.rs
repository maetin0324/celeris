//! ADR 2026-10-27-task-idempotency-keys: `POST /tasks` と `POST /tasks/{id}/retry` の `idempotency_key`。
//! 同じキー・同じ中身の再送は 1 件だけを作り、中身が違えば 409 `idempotency_key_conflict`。
mod common;

use common::cos_ops::{OPS, cos_bearer, op_body};
use common::*;
use serde_json::{Value, json};
use task_core::{Status, TaskKind, TaskStore};

fn body(title: &str, key: &str) -> Value {
    json!({
        "title": title, "objective": "o",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
        "idempotency_key": key,
    })
}

fn task_count(env: &TestEnv) -> usize {
    env.store.list(None).expect("list").len()
}

#[tokio::test]
async fn create_resend_with_the_same_key_makes_one_task_and_other_content_is_409() {
    let env = admin_env();
    let app = env.router();

    let first = send(&app, post_admin("/api/v1/tasks", &body("a", "op-k1"))).await;
    assert_eq!(first.status.as_u16(), 201, "{}", first.text());
    let id = first.json()["id"].clone();

    let again = send(&app, post_admin("/api/v1/tasks", &body("a", "op-k1"))).await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["id"], id);
    assert_eq!(task_count(&env), 1);

    let changed = send(&app, post_admin("/api/v1/tasks", &body("b", "op-k1"))).await;
    assert_problem(&changed, 409, "idempotency_key_conflict");
    assert_eq!(task_count(&env), 1);

    // キー無しは従来どおり毎回作る。別のキーも別 task。
    let mut keyless = body("a", "x");
    keyless.as_object_mut().unwrap().remove("idempotency_key");
    for _ in 0..2 {
        let r = send(&app, post_admin("/api/v1/tasks", &keyless)).await;
        assert_eq!(r.status.as_u16(), 201, "{}", r.text());
    }
    let other = send(&app, post_admin("/api/v1/tasks", &body("a", "op-k2"))).await;
    assert_eq!(other.status.as_u16(), 201, "{}", other.text());
    assert_ne!(other.json()["id"], id);
    assert_eq!(task_count(&env), 4);

    // 空白だけのキーは 400。
    let blank = send(&app, post_admin("/api/v1/tasks", &body("a", "  "))).await;
    assert_eq!(blank.status.as_u16(), 400, "{}", blank.text());
}

#[tokio::test]
async fn retry_resend_with_the_same_key_makes_one_copy_and_other_content_is_409() {
    let env = admin_env();
    let app = env.router();
    let failed = new_task(TaskKind::Execute, Status::Failed);
    env.seed(&failed);
    let other_failed = new_task(TaskKind::Execute, Status::Failed);
    env.seed(&other_failed);
    let path = format!("/api/v1/tasks/{}/retry", failed.id);

    let first = send(
        &app,
        post_admin(&path, &json!({"idempotency_key": "retry-1"})),
    )
    .await;
    assert_eq!(first.status.as_u16(), 201, "{}", first.text());
    let copy = first.json()["task_id"].clone();

    let again = send(
        &app,
        post_admin(&path, &json!({"idempotency_key": "retry-1"})),
    )
    .await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["task_id"], copy);
    assert_eq!(again.json()["rewired"], first.json()["rewired"]);
    assert_eq!(task_count(&env), 3);

    // 中身（accept）が違う、または別の task への同じキーは 409。複製は増えない。
    let changed = send(
        &app,
        post_admin(
            &path,
            &json!({"idempotency_key": "retry-1", "accept": false}),
        ),
    )
    .await;
    assert_problem(&changed, 409, "idempotency_key_conflict");
    let elsewhere = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{}/retry", other_failed.id),
            &json!({"idempotency_key": "retry-1"}),
        ),
    )
    .await;
    assert_problem(&elsewhere, 409, "idempotency_key_conflict");
    // 作成と同じ名前空間: 作成に使ったキーを retry に使っても 409。
    let created = send(&app, post_admin("/api/v1/tasks", &body("a", "shared-k"))).await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let shared = send(
        &app,
        post_admin(&path, &json!({"idempotency_key": "shared-k"})),
    )
    .await;
    assert_problem(&shared, 409, "idempotency_key_conflict");
    assert_eq!(task_count(&env), 4);

    // キー無しの retry は従来どおり毎回複製する。
    let plain = send(&app, post_admin(&path, &json!({}))).await;
    assert_eq!(plain.status.as_u16(), 201, "{}", plain.text());
    assert_eq!(task_count(&env), 5);
}

#[tokio::test]
async fn cos_operations_reject_a_body_idempotency_key() {
    let env = admin_env();
    let app = env.router();
    let failed = new_task(TaskKind::Execute, Status::Failed);
    env.seed(&failed);
    let (_, _, bearer) = cos_bearer(&env, "cos-body-key");
    let headers = [("authorization", bearer.as_str())];
    let before = task_count(&env);
    for (key, path) in [
        ("cos-create", "/api/v1/tasks".to_string()),
        ("cos-retry", format!("/api/v1/tasks/{}/retry", failed.id)),
    ] {
        let envelope = op_body(key, "POST", &path, body("a", "inner-k"));
        let envelope = if path.ends_with("/retry") {
            op_body(key, "POST", &path, json!({"idempotency_key": "inner-k"}))
        } else {
            envelope
        };
        let resp = send(&app, post_json_with(OPS, &envelope, &headers)).await;
        assert_ne!(resp.status.as_u16(), 200, "{key}: {}", resp.text());
        assert!(
            resp.text().contains("idempotency_key in the request body"),
            "{key}: {}",
            resp.text()
        );
    }
    assert_eq!(task_count(&env), before);
}

#[tokio::test]
async fn simultaneous_create_and_retry_return_the_same_task() {
    let env = admin_env();
    let app = env.router();
    let payload = body("concurrent", "create-concurrent");
    let (a, b) = tokio::join!(
        send(&app, post_admin("/api/v1/tasks", &payload)),
        send(&app, post_admin("/api/v1/tasks", &payload)),
    );
    for response in [&a, &b] {
        assert!(
            matches!(response.status.as_u16(), 200 | 201),
            "{}",
            response.text()
        );
    }
    assert_eq!(a.json()["id"], b.json()["id"]);
    assert_eq!(task_count(&env), 1);

    let failed = new_task(TaskKind::Execute, Status::Failed);
    env.seed(&failed);
    let path = format!("/api/v1/tasks/{}/retry", failed.id);
    let payload = json!({"idempotency_key": "retry-concurrent"});
    let (a, b) = tokio::join!(
        send(&app, post_admin(&path, &payload)),
        send(&app, post_admin(&path, &payload)),
    );
    for response in [&a, &b] {
        assert!(
            matches!(response.status.as_u16(), 200 | 201),
            "{}",
            response.text()
        );
    }
    assert_eq!(a.json(), b.json());
    assert_eq!(task_count(&env), 3);
}

#[tokio::test]
async fn failed_creation_does_not_reserve_key_and_old_field_is_rejected() {
    let env = admin_env();
    let app = env.router();
    let mut payload = body("rollback", "rollback-key");
    payload["attachment_ids"] = json!(["nonexistent-attachment"]);
    let invalid = send(&app, post_admin("/api/v1/tasks", &payload)).await;
    assert_eq!(invalid.status.as_u16(), 422, "{}", invalid.text());
    assert_eq!(task_count(&env), 0);
    payload.as_object_mut().unwrap().remove("attachment_ids");
    let valid = send(&app, post_admin("/api/v1/tasks", &payload)).await;
    assert_eq!(valid.status.as_u16(), 201, "{}", valid.text());
    payload["campaign_idempotency_key"] = json!("old-key");
    let old = send(&app, post_admin("/api/v1/tasks", &payload)).await;
    assert_eq!(old.status.as_u16(), 400, "{}", old.text());
}

#[tokio::test]
async fn campaign_resend_binds_new_key_to_request_content() {
    let env = admin_env();
    let app = env.router();
    let mut payload = body("campaign", "first-key");
    payload["campaign"] = json!({"campaign_id": "c1", "case_id": "case-a", "arm": "B",
        "repetition": 1, "manifest_hash": "m1", "routing_setting": "fixed"});
    let first = send(&app, post_admin("/api/v1/tasks", &payload)).await;
    assert_eq!(first.status.as_u16(), 201, "{}", first.text());
    payload["idempotency_key"] = json!("second-key");
    let again = send(&app, post_admin("/api/v1/tasks", &payload)).await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["id"], first.json()["id"]);
    payload["title"] = json!("changed");
    let changed = send(&app, post_admin("/api/v1/tasks", &payload)).await;
    assert_problem(&changed, 409, "idempotency_key_conflict");
    assert_eq!(task_count(&env), 1);
}
