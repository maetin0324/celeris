//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4・S2-5: TS 制御層の領域の mode の API の結合試験。
//!
//! 見るもの: `GET /script/domains`（初期値・generation の hash）、`PUT /script/domains/{domain}/mode`
//! （rust→compare→rust、同じ mode の 200・`changed: false`、`canary`・`ts` の 422、版の不一致の 409、
//! 未知の領域の 404、generation の不備の 422、未知欄の 400）、どの拒否でも表と `script_domain_events` が
//! 変わらないこと、CoS の `/cos/operations` の `script_domain.mode_set`（直接の PUT は 422、包めば applied、
//! `canary` は rejected の記録）。

mod common;

use common::cos_ops::{OPS, cos_bearer, db, op_body, run_external};
use common::*;
use serde_json::{Value, json};
use task_core::script_compare::{NewScriptGeneration, ScriptGenerationState};

const LIST: &str = "/api/v1/script/domains";

fn mode_path(domain: &str) -> String {
    format!("/api/v1/script/domains/{domain}/mode")
}

/// `loaded` の generation を 1 つ入れて、その `bundle_sha256` を返す。
fn seed_generation(env: &TestEnv, sha_digit: char, domains: &[&str]) -> String {
    let sha: String = std::iter::repeat_n(sha_digit, 64).collect();
    env.store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha.clone(),
                host_api: "celeris.host/1".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: json!({ "domains": domains }).to_string(),
                state: ScriptGenerationState::Loaded,
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("generation");
    sha
}

fn event_count(env: &TestEnv) -> i64 {
    db(env)
        .query_row("SELECT COUNT(*) FROM script_domain_events", [], |row| {
            row.get(0)
        })
        .expect("count")
}

async fn put_mode(app: &axum::Router, domain: &str, body: Value) -> Resp {
    send(
        app,
        put_json_with(&mode_path(domain), &body, &admin_headers()),
    )
    .await
}

async fn domain_view(app: &axum::Router, domain: &str) -> Value {
    let resp = send(app, get_admin(LIST)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    resp.json()["domains"]
        .as_array()
        .expect("domains")
        .iter()
        .find(|d| d["domain"] == domain)
        .cloned()
        .expect("domain row")
}

#[tokio::test]
async fn script_domain_list_starts_rust_without_generation() {
    let env = admin_env();
    let app = env.router();
    let resp = send(&app, get_admin(LIST)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    let domains = body["domains"].as_array().expect("domains");
    let names: Vec<&str> = domains
        .iter()
        .map(|d| d["domain"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["cos_digest", "lane_rules", "route_priority"]);
    for d in domains {
        assert_eq!(d["mode"], "rust", "{d}");
        assert_eq!(d["version"], 0, "{d}");
        assert!(
            d["generation_id"].is_null() && d["bundle_sha256"].is_null(),
            "{d}"
        );
    }
    let resp = send(&app, get_admin(&format!("{LIST}?x=1"))).await;
    assert_problem(&resp, 400, "bad_request");
}

#[tokio::test]
async fn script_domain_mode_rust_compare_rust_round_trip() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'a', &["lane_rules"]);

    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "compare", "expected_version": 0, "generation": sha, "reason": "比較を始める"}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["changed"], true);
    assert!(body["event_id"].is_u64(), "{body}");
    assert_eq!(body["domain"]["mode"], "compare");
    assert_eq!(body["domain"]["version"], 1);
    assert_eq!(body["domain"]["bundle_sha256"], sha.as_str());
    assert_eq!(body["domain"]["updated_by"], "human:test");
    let view = domain_view(&app, "lane_rules").await;
    assert_eq!(view["mode"], "compare");
    assert_eq!(view["bundle_sha256"], sha.as_str());
    assert_eq!(event_count(&env), 1);

    // 同じ mode・同じ generation: 200・changed false、版も記録も変わらない（古い版でも 409 にしない）。
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["changed"], false);
    assert!(resp.json()["event_id"].is_null());
    assert_eq!(resp.json()["domain"]["version"], 1);
    assert_eq!(event_count(&env), 1);

    // rust へ戻す。
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "rust", "expected_version": 1}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["domain"]["mode"], "rust");
    assert_eq!(body["domain"]["version"], 2);
    assert!(body["domain"]["generation_id"].is_null());
    assert_eq!(event_count(&env), 2);
    // 他の領域は変わらない。
    assert_eq!(domain_view(&app, "route_priority").await["version"], 0);
}

#[tokio::test]
async fn script_domain_canary_and_ts_are_422_without_change() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["lane_rules", "route_priority"]);
    for mode in ["canary", "ts"] {
        for body in [
            json!({"mode": mode, "expected_version": 0, "generation": sha}),
            json!({"mode": mode, "expected_version": 0}),
        ] {
            let resp = put_mode(&app, "lane_rules", body).await;
            assert_problem(&resp, 422, "script_mode_not_enabled");
        }
    }
    let view = domain_view(&app, "lane_rules").await;
    assert_eq!(
        (view["mode"].clone(), view["version"].clone()),
        (json!("rust"), json!(0))
    );
    assert_eq!(event_count(&env), 0);

    // compare の後でも canary・ts は拒む。
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "ts", "expected_version": 1, "generation": sha}),
    )
    .await;
    assert_problem(&resp, 422, "script_mode_not_enabled");
    assert_eq!(domain_view(&app, "lane_rules").await["mode"], "compare");
    assert_eq!(event_count(&env), 1);
}

#[tokio::test]
async fn script_domain_stale_version_is_409_without_change() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'c', &["route_priority"]);
    let resp = put_mode(
        &app,
        "route_priority",
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());

    // 他の更新が先に入った後の古い版（0）での戻しは 409。
    let resp = put_mode(
        &app,
        "route_priority",
        json!({"mode": "rust", "expected_version": 0}),
    )
    .await;
    let problem = assert_problem(&resp, 409, "script_domain_version_conflict");
    assert_eq!(problem["expected_version"], 0, "{problem}");
    assert_eq!(problem["version"], 1, "{problem}");
    // 未来の版も 409。
    let resp = put_mode(
        &app,
        "route_priority",
        json!({"mode": "rust", "expected_version": 7}),
    )
    .await;
    assert_problem(&resp, 409, "script_domain_version_conflict");
    let view = domain_view(&app, "route_priority").await;
    assert_eq!(
        (view["mode"].clone(), view["version"].clone()),
        (json!("compare"), json!(1))
    );
    assert_eq!(event_count(&env), 1);
}

#[tokio::test]
async fn script_domain_rejects_unknown_domain_bad_generation_and_bad_body() {
    let env = admin_env();
    let app = env.router();
    let sha_other = seed_generation(&env, 'd', &["route_priority"]);

    let resp = put_mode(&app, "nope", json!({"mode": "rust", "expected_version": 0})).await;
    assert_problem(&resp, 404, "script_domain_not_found");

    for body in [
        // compare に generation が無い。
        json!({"mode": "compare", "expected_version": 0}),
        // rust に generation がある。
        json!({"mode": "rust", "expected_version": 0, "generation": sha_other}),
        // 未登録の hash。
        json!({"mode": "compare", "expected_version": 0, "generation": "f".repeat(64)}),
        // manifest の domains に lane_rules が無い。
        json!({"mode": "compare", "expected_version": 0, "generation": sha_other}),
    ] {
        let resp = put_mode(&app, "lane_rules", body.clone()).await;
        assert_problem(&resp, 422, "script_generation_invalid");
    }
    for body in [
        json!({"mode": "compare", "expected_version": 0, "extra": 1}),
        json!({"mode": "shadow", "expected_version": 0}),
        json!({"mode": "rust"}),
    ] {
        let resp = put_mode(&app, "lane_rules", body.clone()).await;
        assert_problem(&resp, 400, "bad_request");
    }
    assert_eq!(domain_view(&app, "lane_rules").await["version"], 0);
    assert_eq!(event_count(&env), 0);
}

#[tokio::test]
async fn script_domain_mode_through_cos_operations() {
    let env = admin_env();
    let sha = seed_generation(&env, 'e', &["lane_rules"]);

    // 直接の PUT は 422 cos_audit_context_required、包めば applied（rust→compare）。
    let op = run_external(
        &env,
        "script-compare",
        "PUT",
        &mode_path("lane_rules"),
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
        "script_domain.mode_set",
    )
    .await;
    assert_eq!(op["target_kind"], "script_domain", "{op}");
    assert_eq!(op["target_id"], "lane_rules", "{op}");
    let row = env.store.script_domain_mode("lane_rules").expect("mode");
    assert_eq!(row.mode.as_str(), "compare");
    assert!(row.updated_by.starts_with("cos:"), "{}", row.updated_by);
    let events = env.store.script_domain_events(None, 10).expect("events");
    assert_eq!(events.len(), 1);
    let recorded = serde_json::to_value(&events[0].event).expect("event json");
    assert!(
        recorded["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "CoS の操作の理由が残る: {recorded}"
    );

    // rust への戻しも CoS に許可する。
    run_external(
        &env,
        "script-rust",
        "PUT",
        &mode_path("lane_rules"),
        json!({"mode": "rust", "expected_version": 1}),
        "script_domain.mode_set",
    )
    .await;
    assert_eq!(
        env.store
            .script_domain_mode("lane_rules")
            .expect("mode")
            .version,
        2
    );

    // canary は CoS でも 422 で、rejected の記録が残り、表は変わらない。
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(&env, "script-canary");
    let headers = [("authorization", bearer.as_str())];
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "script-canary",
                "PUT",
                &mode_path("lane_rules"),
                json!({"mode": "canary", "expected_version": 2, "generation": sha}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    let rejected: i64 = db(&env)
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='rejected'",
            [&thread],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(rejected, 1);
    let row = env.store.script_domain_mode("lane_rules").expect("mode");
    assert_eq!((row.mode.as_str(), row.version), ("rust", 2));
    assert_eq!(event_count(&env), 2);
}
