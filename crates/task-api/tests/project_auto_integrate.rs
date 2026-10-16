//! ADR 2026-10-16-project-auto-integrate D1/D4: `PATCH /projects/{id} {auto_integrate}`。
//! 既定は無効、切り替えは `GET /projects/{id}` に出て DB（store）に残り、他の欄は変えない。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::TaskStore;

fn auth() -> [(&'static str, String); 1] {
    [("authorization", format!("Bearer {TOKEN}"))]
}

fn patch(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    let a = auth();
    patch_json_with(path, body, &[(a[0].0, a[0].1.as_str())])
}

#[tokio::test]
async fn auto_integrate_is_a_project_attribute_switched_by_patch() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    });
    let app = env.router();
    let a = auth();
    let resp = send(
        &app,
        post_json_with(
            "/api/v1/projects",
            &json!({"title": "asmr", "request": "取り込む"}),
            &[(a[0].0, a[0].1.as_str())],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let id = resp.json()["id"].as_str().expect("id").to_string();
    assert_eq!(resp.json()["auto_integrate"], false, "既定は無効");
    let pid: task_core::ProjectId = id.parse().unwrap();

    let url = format!("/api/v1/projects/{id}");
    let resp = send(&app, patch(&url, &json!({"auto_integrate": true}))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["auto_integrate"], true);
    assert_eq!(resp.json()["title"], "asmr");
    assert!(env.store.project_get(pid).unwrap().unwrap().auto_integrate);

    let resp = send(&app, get_with(&url, &[(a[0].0, a[0].1.as_str())])).await;
    assert_eq!(resp.json()["project"]["auto_integrate"], true);

    let resp = send(&app, patch(&url, &json!({"auto_integrate": false}))).await;
    assert_eq!(resp.json()["auto_integrate"], false);
    assert!(!env.store.project_get(pid).unwrap().unwrap().auto_integrate);

    // 無い案件は 404、型違いは 400、トークン無しは 401。
    let missing = "/api/v1/projects/01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let resp = send(&app, patch(missing, &json!({"auto_integrate": true}))).await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());
    let resp = send(&app, patch(&url, &json!({"auto_integrate": "yes"}))).await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    let resp = send(
        &app,
        patch_json_with(&url, &json!({"auto_integrate": true}), &[]),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401);
}
