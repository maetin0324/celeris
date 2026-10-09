//! ADR 2026-10-09-cos-operations-all-mutations D3 (WU ops-projects-cron): the cron-job and
//! project routes run through `/cos/operations`. Each is refused when called directly with the CoS
//! credential (422 + rejected audit row) and applied through the envelope with its row, audit event
//! and chat card. `cron_job.run` is an external-effect operation (C): pending then applied, and a
//! resent request returns the recorded operation without firing the job again.
mod common;

use common::cos_ops::{OPS, audit_events, cos_bearer, db, op_body, run_domain};
use common::*;
use serde_json::{Value, json};
use task_core::CronJobStore;

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

fn job(env: &TestEnv, name: &str) -> Option<task_core::CronJob> {
    env.store.cron_job_get_by_name(name).expect("get")
}

#[tokio::test]
async fn cos_ops_projects_cron_job_lifecycle_is_audited() {
    let env = admin_env();

    let op = run_domain(
        &env,
        "cron-create",
        "POST",
        "/api/v1/cron-jobs",
        job_body("daily"),
        "cron_job.create",
    )
    .await;
    let created = job(&env, "daily").expect("created by cos");
    assert_eq!(op["target_kind"], "cron_job");
    assert_eq!(op["target_id"], created.id.to_string());
    assert_eq!(op["result"]["job"]["name"], "daily");

    run_domain(
        &env,
        "cron-update",
        "PATCH",
        "/api/v1/cron-jobs/daily",
        json!({"schedule": "30 5 * * *"}),
        "cron_job.update",
    )
    .await;
    assert_eq!(job(&env, "daily").expect("job").schedule, "30 5 * * *");

    run_domain(
        &env,
        "cron-pause",
        "POST",
        "/api/v1/cron-jobs/daily/pause",
        json!({}),
        "cron_job.pause",
    )
    .await;
    let paused = job(&env, "daily").expect("job");
    assert!(!paused.enabled && paused.next_fire_at.is_none());

    run_domain(
        &env,
        "cron-resume",
        "POST",
        "/api/v1/cron-jobs/daily/resume",
        json!(null),
        "cron_job.resume",
    )
    .await;
    let resumed = job(&env, "daily").expect("job");
    assert!(resumed.enabled && resumed.next_fire_at.is_some());

    run_domain(
        &env,
        "cron-delete",
        "DELETE",
        &format!("/api/v1/cron-jobs/{}", created.id),
        json!(null),
        "cron_job.delete",
    )
    .await;
    assert!(job(&env, "daily").is_none());
}

#[tokio::test]
async fn cos_ops_projects_cron_domain_errors_are_recorded_as_rejected() {
    let env = admin_env();
    let app = env.router();
    let created = send(&app, post_admin("/api/v1/cron-jobs", &job_body("dup"))).await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());

    let (thread, _, bearer) = cos_bearer(&env, "cron-dup");
    let headers = [("authorization", bearer.as_str())];
    for (key, method, path, body, status) in [
        (
            "dup",
            "POST",
            "/api/v1/cron-jobs",
            job_body("dup"),
            409_u16,
        ),
        (
            "missing",
            "POST",
            "/api/v1/cron-jobs/nope/pause",
            json!({}),
            404,
        ),
        (
            "body",
            "POST",
            "/api/v1/cron-jobs/dup/run",
            json!({"x": 1}),
            422,
        ),
    ] {
        let resp = send(
            &app,
            post_json_with(OPS, &op_body(key, method, path, body), &headers),
        )
        .await;
        assert_eq!(resp.status.as_u16(), status, "{key}: {}", resp.text());
    }
    let rejected: i64 = db(&env)
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='rejected'",
            [&thread],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(rejected, 3);
    let runs = env
        .store
        .cron_job_runs(job(&env, "dup").expect("job").id, None)
        .expect("runs");
    assert!(runs.is_empty(), "a rejected run must not fire: {runs:?}");
}

#[tokio::test]
async fn cos_ops_projects_cron_run_is_external_once() {
    let env = admin_env();
    let app = env.router();
    let created = send(&app, post_admin("/api/v1/cron-jobs", &job_body("manual"))).await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let job_id = job(&env, "manual").expect("job").id;

    let (thread, run, bearer) = cos_bearer(&env, "cron-run");
    let headers = [("authorization", bearer.as_str())];
    let direct = send(
        &app,
        post_json_with("/api/v1/cron-jobs/manual/run", &json!({}), &headers),
    )
    .await;
    assert_problem(&direct, 422, "cos_audit_context_required");
    assert!(env.store.cron_job_runs(job_id, None).expect("runs").is_empty());

    let envelope = op_body("run-once", "POST", "/api/v1/cron-jobs/manual/run", json!({}));
    let first = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(first.status.as_u16(), 200, "{}", first.text());
    let op = first.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "cron_job.run");
    assert_eq!(op["run_id"], run.as_str());
    assert_eq!(op["thread_id"], thread.as_str());
    assert!(op["result"]["task_id"].is_string(), "{op}");
    let op_id = op["id"].as_str().expect("id").to_string();
    let states: Vec<Value> = audit_events(&env, &op_id)
        .into_iter()
        .map(|event| event["state"].clone())
        .collect();
    assert_eq!(states, vec![json!("pending"), json!("applied")]);
    assert_eq!(env.store.cron_job_runs(job_id, None).expect("runs").len(), 1);

    // Resending the same request returns the recorded operation and does not fire again.
    let again = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["operation"]["id"], op_id.as_str());
    assert_eq!(env.store.cron_job_runs(job_id, None).expect("runs").len(), 1);
    assert_eq!(audit_events(&env, &op_id).len(), 2);
}
