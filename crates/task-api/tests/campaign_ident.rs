//! ADR 2026-10-16-campaign-identity-propagation: `POST /tasks` が campaign 識別を受け、task 作成と同じ
//! transaction で保存する。識別は HTTP 経由でだけ入れる（DB への直挿入に頼らない）。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::TaskStore;

fn p(body: &Value) -> axum::http::Request<axum::body::Body> {
    post_json_with(
        "/api/v1/tasks",
        body,
        &[("authorization", format!("Bearer {TOKEN}").as_str())],
    )
}

fn task(extra: Value) -> Value {
    let mut b = json!({
        "title": "trial", "objective": "o",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
    });
    for (k, v) in extra.as_object().expect("object") {
        b[k] = v.clone();
    }
    b
}

fn campaign(rep: u32, manifest: &str) -> Value {
    json!({"campaign_id": "c1", "case_id": "case-a", "arm": "B", "repetition": rep,
           "manifest_hash": manifest, "routing_setting": "fixed"})
}

#[tokio::test]
async fn create_stores_campaign_identity_and_resend_is_idempotent() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    });
    let app = env.router();

    let body = task(json!({"campaign": campaign(1, "m1"), "idempotency_key": "campaign-k1"}));
    let first = send(&app, p(&body)).await;
    assert_eq!(first.status.as_u16(), 201, "{}", first.text());
    let id = first.json()["id"].as_str().expect("id").to_string();
    let own = env
        .store
        .task_campaign_own(&id)
        .expect("read")
        .expect("identity saved");
    assert_eq!(own.campaign_id, "c1");
    assert_eq!(own.case_id.as_deref(), Some("case-a"));
    assert_eq!(own.arm.as_deref(), Some("B"));
    assert_eq!(own.repetition, Some(1));
    assert_eq!(own.manifest_hash.as_deref(), Some("m1"));
    // 冪等キーは label にしない（`task_idempotency_keys` で照合する。ADR 2026-10-27-task-idempotency-keys）。
    assert!(
        !first.json()["labels"]
            .as_array()
            .is_some_and(|labels| labels.iter().any(|l| l == "campaign-k1"))
    );

    // 同一識別の再送は既存 task（200）。新しい task は増えない。
    let again = send(&app, p(&body)).await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["id"], json!(id));
    // キー無しでも同じ試行なら同じ task。
    let keyless = send(&app, p(&task(json!({"campaign": campaign(1, "m1")})))).await;
    assert_eq!(keyless.status.as_u16(), 200, "{}", keyless.text());
    assert_eq!(keyless.json()["id"], json!(id));

    // 別の試行（repetition 違い）は衝突せず別 task。
    let other = send(
        &app,
        p(&task(
            json!({"campaign": campaign(2, "m1"), "idempotency_key": "campaign-k2"}),
        )),
    )
    .await;
    assert_eq!(other.status.as_u16(), 201, "{}", other.text());
    assert_ne!(other.json()["id"], json!(id));

    // 同じキーで識別が違えば 409、同じ試行で manifest が違っても 409。どちらも task を作らない。
    let bad_key = send(
        &app,
        p(&task(
            json!({"campaign": campaign(3, "m1"), "idempotency_key": "campaign-k1"}),
        )),
    )
    .await;
    assert_eq!(bad_key.status.as_u16(), 409, "{}", bad_key.text());
    assert_eq!(bad_key.json()["code"], "idempotency_key_conflict");
    let bad_manifest = send(&app, p(&task(json!({"campaign": campaign(1, "m2")})))).await;
    assert_eq!(bad_manifest.status.as_u16(), 409, "{}", bad_manifest.text());

    // 子 task は自分の行を持たず、親の識別を継ぐ。
    let mut child = new_task(task_core::TaskKind::Execute, task_core::Status::Draft);
    child.parent_id = Some(id.parse().expect("task id"));
    env.store.insert(&child).expect("insert child");
    let cid = child.id.to_string();
    assert!(env.store.task_campaign_own(&cid).expect("read").is_none());
    assert_eq!(
        env.store.task_campaign_resolve(&cid).expect("resolve"),
        Some(own)
    );

    // campaign 無しの task は従来どおり識別 NULL。キーだけでも作れる（汎用の冪等キー）。不正な routing_setting は 400。
    let plain = send(&app, p(&task(json!({})))).await;
    assert_eq!(plain.status.as_u16(), 201);
    let pid = plain.json()["id"].as_str().expect("id").to_string();
    assert!(
        env.store
            .task_campaign_resolve(&pid)
            .expect("resolve")
            .is_none()
    );
    let key_only = send(&app, p(&task(json!({"idempotency_key": "k"})))).await;
    assert_eq!(key_only.status.as_u16(), 201, "{}", key_only.text());
    let mut bad = campaign(9, "m");
    bad["routing_setting"] = json!("auto");
    let resp = send(&app, p(&task(json!({"campaign": bad})))).await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
}
