//! ADR-0131 D5: 定期実行（cron job）の API の結合テスト。
//!
//! 見るもの: `POST /cron-jobs`（201・409 名前重複・422 式/タイムゾーン/雛形・400 未知欄）、`GET /cron-jobs`
//! と `GET /cron-jobs/{id}`（ULID と name の両方で引ける・404）、`PATCH`（schedule 変更で `next_fire_at` を
//! 再計算）、`pause` / `resume`、`POST /run`（task を作る・前回が動いていれば skip）、`GET /runs`（新しい順・
//! `limit`）、`DELETE`（204・作った task は残る）。発火の規則そのものは task-ops の単体試験で見る。

mod common;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::{Value, json};
use task_core::{Status, TaskId, TaskStore};

const BASE: &str = "/api/v1/cron-jobs";

fn job_body(name: &str) -> Value {
    json!({
        "name": name,
        "schedule": "0 4 * * *",
        "timezone": "Asia/Tokyo",
        "template": {
            "title": "知識整理: {date}",
            "objective": "KB と受信箱を整理する",
            "acceptance": [{"type": "reviewer", "text": "整理の記録がある"}],
            "lane": "cheap"
        }
    })
}

fn post_empty(path: &str) -> Request<Body> {
    post_json(path, &json!({}))
}

fn patch(path: &str, body: &Value) -> Request<Body> {
    patch_json_with(path, body, &[])
}

fn delete(path: &str) -> Request<Body> {
    delete_with(path, &[])
}

async fn create(app: &axum::Router, name: &str) -> Value {
    let resp = send(app, post_json(BASE, &job_body(name))).await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    resp.json()
}

#[tokio::test]
async fn create_get_list_and_lookup_by_name() {
    let env = TestEnv::new();
    let app = env.router();

    let created = create(&app, "daily-curation").await;
    let id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["name"], "daily-curation");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["overlap"], "skip");
    assert_eq!(created["catch_up"], "latest");
    assert!(created["next_fire_at"].is_string(), "{created}");
    assert!(created["last_run"].is_null());
    assert_eq!(created["template"]["lane"], "cheap");

    // ULID と name のどちらでも引ける。
    for key in [id.as_str(), "daily-curation"] {
        let resp = send(&app, get(&format!("{BASE}/{key}"))).await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        let body = resp.json();
        assert_eq!(body["id"], id.as_str());
        assert_eq!(body["template"]["title"], "知識整理: {date}");
    }

    create(&app, "another").await;
    let list = send(&app, get(BASE)).await;
    assert_eq!(list.status.as_u16(), 200);
    let names: Vec<String> = list.json()["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|j| j["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(names, vec!["another", "daily-curation"]);

    let missing = send(&app, get(&format!("{BASE}/no-such-job"))).await;
    assert_problem(&missing, 404, "cron_job_not_found");
    let missing_runs = send(&app, get(&format!("{BASE}/no-such-job/runs"))).await;
    assert_problem(&missing_runs, 404, "cron_job_not_found");
}

#[tokio::test]
async fn create_rejects_duplicates_and_invalid_input() {
    let env = TestEnv::new();
    let app = env.router();
    create(&app, "daily").await;

    let dup = send(&app, post_json(BASE, &job_body("daily"))).await;
    assert_problem(&dup, 409, "cron_job_name_in_use");

    let mut bad_schedule = job_body("bad-schedule");
    bad_schedule["schedule"] = json!("61 * * * *");
    assert_problem(
        &send(&app, post_json(BASE, &bad_schedule)).await,
        422,
        "validation",
    );

    let mut bad_tz = job_body("bad-tz");
    bad_tz["timezone"] = json!("Mars/Olympus");
    assert_problem(
        &send(&app, post_json(BASE, &bad_tz)).await,
        422,
        "validation",
    );

    let mut no_acceptance = job_body("no-acceptance");
    no_acceptance["template"]["acceptance"] = json!([]);
    assert_problem(
        &send(&app, post_json(BASE, &no_acceptance)).await,
        422,
        "validation",
    );

    let mut unknown = job_body("unknown");
    unknown["surprise"] = json!(1);
    assert_problem(
        &send(&app, post_json(BASE, &unknown)).await,
        400,
        "bad_request",
    );

    // 失敗した作成は何も残さない。
    let list = send(&app, get(BASE)).await.json();
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn patch_updates_fields_and_recomputes_next_fire() {
    let env = TestEnv::new();
    let app = env.router();
    let created = create(&app, "daily").await;
    let before = created["next_fire_at"].clone();

    let resp = send(
        &app,
        patch(
            &format!("{BASE}/daily"),
            &json!({"schedule": "30 23 * * *", "overlap": "queue"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["schedule"], "30 23 * * *");
    assert_eq!(body["overlap"], "queue");
    assert_ne!(body["next_fire_at"], before, "{body}");
    assert_eq!(body["template"]["lane"], "cheap", "template untouched");

    let bad = send(
        &app,
        patch(
            &format!("{BASE}/daily"),
            &json!({"timezone": "Nowhere/City"}),
        ),
    )
    .await;
    assert_problem(&bad, 422, "validation");
    let unknown = send(
        &app,
        patch(&format!("{BASE}/daily"), &json!({"enabled": false})),
    )
    .await;
    assert_problem(&unknown, 400, "bad_request");
    let missing = send(&app, patch(&format!("{BASE}/nope"), &json!({}))).await;
    assert_problem(&missing, 404, "cron_job_not_found");
}

#[tokio::test]
async fn pause_and_resume() {
    let env = TestEnv::new();
    let app = env.router();
    create(&app, "daily").await;

    let paused = send(&app, post_empty(&format!("{BASE}/daily/pause"))).await;
    assert_eq!(paused.status.as_u16(), 200, "{}", paused.text());
    let body = paused.json();
    assert_eq!(body["enabled"], false);
    assert!(body["next_fire_at"].is_null(), "{body}");

    let resumed = send(&app, post_empty(&format!("{BASE}/daily/resume"))).await;
    assert_eq!(resumed.status.as_u16(), 200, "{}", resumed.text());
    let body = resumed.json();
    assert_eq!(body["enabled"], true);
    assert!(body["next_fire_at"].is_string(), "{body}");

    let missing = send(&app, post_empty(&format!("{BASE}/nope/pause"))).await;
    assert_problem(&missing, 404, "cron_job_not_found");
}

#[tokio::test]
async fn manual_run_creates_a_task_and_skips_while_previous_is_running() {
    let env = TestEnv::new();
    let app = env.router();
    create(&app, "daily").await;

    let resp = send(&app, post_empty(&format!("{BASE}/daily/run"))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["job_name"], "daily");
    let runs = body["runs"].as_array().expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["trigger"], "manual");
    assert_eq!(runs[0]["outcome"], "created");
    let task_id: TaskId = body["task_id"]
        .as_str()
        .expect("task_id")
        .parse()
        .expect("ulid");
    assert_eq!(runs[0]["task_id"], task_id.to_string());

    // 作った task は通常の task として見える（雛形の題名の {date} が置き換わり、ラベル cron 付き）。
    let task = env.store.get(task_id).expect("get").expect("task exists");
    assert!(task.title.starts_with("知識整理: 20"), "{}", task.title);
    assert!(!task.title.contains("{date}"));
    assert_eq!(task.status, Status::Ready);
    let detail = send(&app, get(&format!("/api/v1/tasks/{task_id}"))).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());

    // 前回の task がまだ終端でない: overlap = skip なので task を作らず `skipped_overlap` を残す。
    let skipped = send(&app, post_empty(&format!("{BASE}/daily/run"))).await;
    assert_eq!(skipped.status.as_u16(), 200, "{}", skipped.text());
    let body = skipped.json();
    assert!(body["task_id"].is_null(), "{body}");
    assert_eq!(body["runs"][0]["outcome"], "skipped_overlap");

    // 履歴は新しい順。job の last_run も最新。
    let hist = send(&app, get(&format!("{BASE}/daily/runs"))).await;
    assert_eq!(hist.status.as_u16(), 200, "{}", hist.text());
    let hist = hist.json();
    let items = hist["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["outcome"], "skipped_overlap");
    assert_eq!(items[1]["outcome"], "created");
    let limited = send(&app, get(&format!("{BASE}/daily/runs?limit=1")))
        .await
        .json();
    assert_eq!(limited["items"].as_array().map(Vec::len), Some(1));
    let bad_limit = send(&app, get(&format!("{BASE}/daily/runs?limit=0"))).await;
    assert_problem(&bad_limit, 400, "bad_request");
    let job = send(&app, get(&format!("{BASE}/daily"))).await.json();
    assert_eq!(job["last_run"]["outcome"], "skipped_overlap");

    let missing = send(&app, post_empty(&format!("{BASE}/nope/run"))).await;
    assert_problem(&missing, 404, "cron_job_not_found");
}

#[tokio::test]
async fn delete_removes_job_and_history_but_keeps_tasks() {
    let env = TestEnv::new();
    let app = env.router();
    create(&app, "daily").await;
    let run = send(&app, post_empty(&format!("{BASE}/daily/run")))
        .await
        .json();
    let task_id: TaskId = run["task_id"]
        .as_str()
        .expect("task")
        .parse()
        .expect("ulid");

    let resp = send(&app, delete(&format!("{BASE}/daily"))).await;
    assert_eq!(resp.status.as_u16(), 204, "{}", resp.text());
    assert_problem(
        &send(&app, get(&format!("{BASE}/daily"))).await,
        404,
        "cron_job_not_found",
    );
    assert_problem(
        &send(&app, delete(&format!("{BASE}/daily"))).await,
        404,
        "cron_job_not_found",
    );
    assert!(
        env.store.get(task_id).expect("get").is_some(),
        "task survives"
    );
}

#[tokio::test]
async fn token_is_required_when_configured() {
    let env = admin_env();
    let app = env.router();
    let anon = send(&app, post_json(BASE, &job_body("daily"))).await;
    assert_eq!(anon.status.as_u16(), 401, "{}", anon.text());
    let authed = send(&app, post_admin(BASE, &job_body("daily"))).await;
    assert_eq!(authed.status.as_u16(), 201, "{}", authed.text());
    let run = send(&app, post_admin(&format!("{BASE}/daily/run"), &json!({}))).await;
    assert_eq!(run.status.as_u16(), 200, "{}", run.text());
}
