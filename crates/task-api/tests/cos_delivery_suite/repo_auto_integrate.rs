//! ADR 2026-11-14-repo-auto-integrate D1/D2/D8: `auto_integrate` は案件のリポジトリの欄。
//! `POST /projects/{id}/repos`・`PATCH /repos/{id}` で読み書きでき、remote・`dir` の行で有効にする要求は
//! 422。`PATCH /projects/{id}` の `auto_integrate` は未知の欄（422）。
//! 条件の正本は task-core の検証。この試験は API がその拒否を 422 に写し、何も書かないことを守る。

use crate::common;

use common::*;
use serde_json::{Value, json};
use task_core::TaskStore;

async fn make_project(app: &axum::Router) -> String {
    let resp = send(
        app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "asmr", "request": "取り込む"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    resp.json()["id"].as_str().expect("id").to_string()
}

async fn create(app: &axum::Router, project: &str, body: Value) -> Resp {
    send(
        app,
        post_admin(&format!("/api/v1/projects/{project}/repos"), &body),
    )
    .await
}

#[tokio::test]
async fn a_local_git_repo_reads_and_writes_auto_integrate() {
    let env = admin_env();
    let app = env.router();
    let project = make_project(&app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let code = dir.path().join("code");
    std::fs::create_dir_all(code.join(".git")).expect("mkdir");
    let local = json!({"kind": "local", "path": code.to_string_lossy()});

    let created = create(&app, &project, json!({"name": "code", "location": local})).await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    assert_eq!(created.json()["auto_integrate"], false, "既定は無効");
    let id = created.json()["id"].as_str().expect("id").to_string();
    let url = format!("/api/v1/repos/{id}");

    let resp = send(&app, patch_admin(&url, &json!({"auto_integrate": true}))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["auto_integrate"], true);
    assert_eq!(resp.json()["name"], "code", "他の欄は変えない");
    let stored = env.store.repo_get(id.parse().unwrap()).unwrap().unwrap();
    assert!(stored.auto_integrate);

    // 一覧と GET /projects/{id} の repos[] に出る。
    let list = send(
        &app,
        get_admin(&format!("/api/v1/projects/{project}/repos")),
    )
    .await;
    assert_eq!(list.json()["items"][0]["auto_integrate"], true);
    let detail = send(&app, get_admin(&format!("/api/v1/projects/{project}"))).await;
    assert_eq!(detail.json()["repos"][0]["auto_integrate"], true);

    // 別の欄だけを直しても有効のまま。
    let resp = send(&app, patch_admin(&url, &json!({"run": "host"}))).await;
    assert_eq!(resp.json()["auto_integrate"], true);

    let resp = send(&app, patch_admin(&url, &json!({"auto_integrate": false}))).await;
    assert_eq!(resp.json()["auto_integrate"], false);

    // 作成時に有効にできる。型違いは 400、トークン無しは 401。
    let other = dir.path().join("other");
    std::fs::create_dir_all(other.join(".git")).expect("mkdir");
    let created = create(
        &app,
        &project,
        json!({"name": "other", "location": {"kind": "local", "path": other.to_string_lossy()}, "auto_integrate": true}),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    assert_eq!(created.json()["auto_integrate"], true);
    let resp = send(&app, patch_admin(&url, &json!({"auto_integrate": "yes"}))).await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    let resp = send(
        &app,
        patch_json_with(&url, &json!({"auto_integrate": true}), &[]),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401);
}

#[tokio::test]
async fn enabling_a_remote_or_dir_repo_is_422_and_writes_nothing() {
    let env = admin_env();
    let app = env.router();
    let project = make_project(&app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).expect("mkdir");
    let remote = json!({"kind": "remote", "cluster": "pegasus", "path": "/work/x"});
    let plain_loc = json!({"kind": "local", "path": plain.to_string_lossy()});

    // 作成: remote と dir は 422、行は残らない。
    let resp = create(
        &app,
        &project,
        json!({"name": "far", "location": remote, "auto_integrate": true}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    let resp = create(
        &app,
        &project,
        json!({"name": "plain", "kind": "dir", "location": plain_loc, "auto_integrate": true}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    let list = send(
        &app,
        get_admin(&format!("/api/v1/projects/{project}/repos")),
    )
    .await;
    assert_eq!(list.json()["items"].as_array().unwrap().len(), 0);

    // 更新: remote の行を有効にする要求は 422。無効のままは保たれる。
    let far = create(&app, &project, json!({"name": "far", "location": remote})).await;
    assert_eq!(far.status.as_u16(), 201, "{}", far.text());
    let far_id = far.json()["id"].as_str().expect("id").to_string();
    let resp = send(
        &app,
        patch_admin(
            &format!("/api/v1/repos/{far_id}"),
            &json!({"auto_integrate": true}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    let stored = env
        .store
        .repo_get(far_id.parse().unwrap())
        .unwrap()
        .unwrap();
    assert!(!stored.auto_integrate);

    // 有効な local の行を remote に変える・dir に変える要求も 422（黙って false に落とさない）。
    let code = dir.path().join("code");
    std::fs::create_dir_all(code.join(".git")).expect("mkdir");
    let on = create(
        &app,
        &project,
        json!({"name": "code", "location": {"kind": "local", "path": code.to_string_lossy()}, "auto_integrate": true}),
    )
    .await;
    assert_eq!(on.status.as_u16(), 201, "{}", on.text());
    let on_id = on.json()["id"].as_str().expect("id").to_string();
    let url = format!("/api/v1/repos/{on_id}");
    let resp = send(&app, patch_admin(&url, &json!({"location": remote}))).await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    let resp = send(&app, patch_admin(&url, &json!({"kind": "dir"}))).await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    let stored = env.store.repo_get(on_id.parse().unwrap()).unwrap().unwrap();
    assert!(stored.auto_integrate, "拒否は何も書かない");
    assert!(matches!(
        stored.location,
        task_core::WorkspaceSpec::Local { .. }
    ));

    // 同じ要求で false を書けば変えられる。
    let resp = send(
        &app,
        patch_admin(&url, &json!({"location": remote, "auto_integrate": false})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["auto_integrate"], false);
}

#[tokio::test]
async fn patching_a_project_auto_integrate_is_422() {
    let env = admin_env();
    let app = env.router();
    let project = make_project(&app).await;
    let url = format!("/api/v1/projects/{project}");
    for value in [json!(true), json!(false)] {
        let resp = send(&app, patch_admin(&url, &json!({"auto_integrate": value}))).await;
        assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    }
    // 案件の応答にこの欄は無い。
    let resp = send(&app, get_admin(&url)).await;
    assert!(resp.json()["project"].get("auto_integrate").is_none());
}
