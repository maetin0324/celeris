//! Registry foundation: ADR exclusions are rejected with audit and no secret payload persistence.
mod common;
use common::cos_ops::{OPS, audit_events, cos_bearer, db, op_body};
use common::*;
use serde_json::json;

#[tokio::test]
async fn cos_ops_registry_exclusions_record_reasoned_rejections_without_body_or_side_effects() {
    let env = admin_env();
    let app = env.router();
    let (thread, run, bearer) = cos_bearer(&env, "excluded");
    let cases = [
        (
            "PUT",
            "/api/v1/secrets/example",
            "secret_operations",
            "人の決定 secrets=exclude",
        ),
        (
            "DELETE",
            "/api/v1/accounts/example/login",
            "secret_operations",
            "取消・削除も含む",
        ),
        (
            "POST",
            "/api/v1/tasks/example/browser/live/run/session/grant",
            "browser_credential_attestation",
            "owner attestation",
        ),
        (
            "POST",
            "/api/v1/browser/trusted-devices/verify",
            "browser_credential_attestation",
            "失効・削除も除外",
        ),
        (
            "POST",
            "/api/v1/console/instruct",
            "console_instruction_chain",
            "監査の鎖が二重になる",
        ),
        (
            "POST",
            "/api/v1/cos/operations/example/override",
            "recursive_cos",
            "override は人の取消・差し戻し専用",
        ),
        (
            "POST",
            "/api/v1/plans",
            "removed_by_adr_0079",
            "410 の互換入口を復活させない",
        ),
        (
            "PATCH",
            "/api/v1/milestones/example",
            "removed_by_adr_0079",
            "410 の互換入口を復活させない",
        ),
    ];
    for (index, (method, path, code, detail)) in cases.iter().enumerate() {
        let key = format!("excluded-{index}");
        let body = json!({"secret": "DO-NOT-PERSIST-secret", "assertion": "DO-NOT-PERSIST-assertion", "cookie": "DO-NOT-PERSIST-cookie"});
        let envelope = op_body(&key, method, path, body);
        let resp = send(
            &app,
            post_json_with(OPS, &envelope, &[("authorization", &bearer)]),
        )
        .await;
        let problem = assert_problem(&resp, 422, "cos_operation_not_allowed");
        let why = problem["detail"].as_str().expect("detail");
        assert!(why.contains(code), "{problem}");
        assert!(why.contains(detail), "{problem}");
        let op = env
            .store
            .cos_operation_find(&thread, &key)
            .expect("lookup")
            .expect("rejected row");
        assert_eq!(op.state, "rejected");
        assert_eq!(op.target_id, *path);
        assert_eq!(op.run_id, run);
        assert!(op.reason.contains(why));
        assert_eq!(op.payload["body"], json!({"redacted": true}));
        let events = audit_events(&env, &op.id);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["state"], "rejected");
        assert_eq!(events[0]["actor"], "cos");
        assert!(events[0]["reason"].as_str().expect("reason").contains(why));
        // Existing rejected operations have no applied card. Check all chat payloads anyway.
        assert!(op.event_id.is_none());
        let conn = db(&env);
        let mut stmt = conn
            .prepare("SELECT payload_json FROM chat_events WHERE thread_id=?1")
            .expect("chat payloads");
        let mut serialized: Vec<String> = stmt
            .query_map([&thread], |row| row.get(0))
            .expect("chat query")
            .map(|row| row.expect("chat payload"))
            .collect();
        serialized.extend([
            serde_json::to_string(&op).expect("operation JSON"),
            events[0].to_string(),
            resp.text(),
        ]);
        for value in serialized {
            assert!(
                !value.contains("DO-NOT-PERSIST"),
                "secret was persisted: {value}"
            );
        }
        // Rejection remains idempotent; a changed secret body still conflicts by request hash.
        let again = send(
            &app,
            post_json_with(OPS, &envelope, &[("authorization", &bearer)]),
        )
        .await;
        assert_eq!(again.status.as_u16(), 200, "{}", again.text());
        assert_eq!(again.json()["operation"]["id"], op.id);
        assert_eq!(again.json()["operation"]["state"], "rejected");
        let mut changed = envelope;
        changed["request"]["body"] = json!({"secret": "different-secret"});
        let conflict = send(
            &app,
            post_json_with(OPS, &changed, &[("authorization", &bearer)]),
        )
        .await;
        assert_problem(&conflict, 409, "chat_conflict");
        assert_eq!(audit_events(&env, &op.id).len(), 1);
    }
    let conn = db(&env);
    let tasks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
        .expect("tasks");
    assert_eq!(tasks, 0);
    let applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE state='applied'",
            [],
            |row| row.get(0),
        )
        .expect("applied");
    assert_eq!(applied, 0);
    let rejected: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE state='rejected'",
            [],
            |row| row.get(0),
        )
        .expect("rejected");
    assert_eq!(rejected, cases.len() as i64);
}

#[tokio::test]
async fn cos_ops_registry_unregistered_and_recursive_subtree_bodies_are_redacted() {
    let env = admin_env();
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(&env, "unregistered");
    for (key, method, path) in [
        ("unknown", "POST", "/api/v1/unknown"),
        ("cos-unknown", "PUT", "/api/v1/cos/unknown"),
        ("wrong-method", "PATCH", "/api/v1/secrets/example"),
    ] {
        let resp = send(
            &app,
            post_json_with(
                OPS,
                &op_body(key, method, path, json!({"secret": "DO-NOT-PERSIST"})),
                &[("authorization", &bearer)],
            ),
        )
        .await;
        assert_problem(&resp, 422, "cos_operation_not_allowed");
        let op = env
            .store
            .cos_operation_find(&thread, key)
            .expect("lookup")
            .expect("row");
        assert_eq!(op.payload["body"], json!({"redacted": true}));
        assert!(
            !serde_json::to_string(&op)
                .expect("JSON")
                .contains("DO-NOT-PERSIST")
        );
    }
}

#[tokio::test]
async fn cos_ops_registry_exclusion_precedes_body_and_instruction_validation() {
    let env = admin_env();
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(&env, "excluded-instruction");
    let mut envelope = op_body(
        "excluded",
        "PUT",
        "/api/v1/secrets/example",
        json!({
            "actor": "human", "secret": "DO-NOT-PERSIST"
        }),
    );
    envelope["instructed_by"] = json!("not-an-input-message");
    let resp = send(
        &app,
        post_json_with(OPS, &envelope, &[("authorization", &bearer)]),
    )
    .await;
    let problem = assert_problem(&resp, 422, "cos_operation_not_allowed");
    assert!(
        problem["detail"]
            .as_str()
            .expect("detail")
            .contains("secret_operations")
    );
    let op = env
        .store
        .cos_operation_find(&thread, "excluded")
        .expect("lookup")
        .expect("row");
    assert_eq!(op.state, "rejected");
    assert_eq!(op.payload["body"], json!({"redacted": true}));
    assert_eq!(audit_events(&env, &op.id).len(), 1);
}
