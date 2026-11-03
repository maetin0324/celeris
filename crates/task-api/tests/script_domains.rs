//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4・S2-5: TS 制御層の領域の mode の API の結合試験。
//!
//! 見るもの: `GET /script/domains`（初期値・generation の hash）、`PUT /script/domains/{domain}/mode`
//! （rust→compare→rust、同じ mode の 200・`changed: false`、`canary` と `cos_digest` 以外の `ts` の 422、
//! 付記 S4-14 の `cos_digest` の compare→ts（人の主体だけ・同じ generation の compare から・戻しは誰でも）、版の不一致の 409、
//! 未知の領域の 404、generation の不備の 422、未知欄の 400）、どの拒否でも表と `script_domain_events` が
//! 変わらないこと、CoS の `/cos/operations` の `script_domain.mode_set`（直接の PUT は 422、包めば applied、
//! `canary`・`ts` は rejected の記録）。

mod common;

use common::cos_ops::{OPS, cos_bearer, db, op_body, run_external};
use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::script_compare::{
    NewScriptGeneration, ScriptDomain, ScriptDomainError, ScriptGenerationState, ScriptMode,
    ScriptModeChange,
};

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
async fn script_domain_mode_canary_still_rejected() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["lane_rules", "route_priority", "cos_digest"]);
    for domain in ["lane_rules", "route_priority", "cos_digest"] {
        for body in [
            json!({"mode": "canary", "expected_version": 0, "generation": sha}),
            json!({"mode": "canary", "expected_version": 0}),
        ] {
            let resp = put_mode(&app, domain, body).await;
            assert_problem(&resp, 422, "script_mode_not_enabled");
        }
        // compare の後でも canary は拒む。
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "compare", "expected_version": 0, "generation": sha}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 1, "generation": sha}),
        )
        .await;
        assert_problem(&resp, 422, "script_mode_not_enabled");
        let view = domain_view(&app, domain).await;
        assert_eq!(
            (view["mode"].clone(), view["version"].clone()),
            (json!("compare"), json!(1))
        );
    }
    assert_eq!(event_count(&env), 3);
}

/// 付記 S4-14: `ts` を受けるのは `cos_digest` だけ。`lane_rules`・`route_priority` は compare の後でも 422。
#[tokio::test]
async fn script_domain_mode_ts_only_for_cos_digest() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["lane_rules", "route_priority", "cos_digest"]);
    for domain in ["lane_rules", "route_priority"] {
        for body in [
            json!({"mode": "ts", "expected_version": 0, "generation": sha}),
            json!({"mode": "ts", "expected_version": 0}),
        ] {
            let resp = put_mode(&app, domain, body).await;
            assert_problem(&resp, 422, "script_mode_not_enabled");
        }
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "compare", "expected_version": 0, "generation": sha}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "ts", "expected_version": 1, "generation": sha}),
        )
        .await;
        assert_problem(&resp, 422, "script_mode_not_enabled");
        assert_eq!(domain_view(&app, domain).await["mode"], "compare");
    }
    assert_eq!(event_count(&env), 2);

    // cos_digest は人の主体で compare → ts に切り替えられる。
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "ts", "expected_version": 1, "generation": sha, "reason": "TS の pass を試す"}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["changed"], true);
    assert_eq!(body["domain"]["mode"], "ts");
    assert_eq!(body["domain"]["version"], 2);
    assert_eq!(body["domain"]["bundle_sha256"], sha.as_str());
    assert_eq!(body["domain"]["updated_by"], "human:test");
    // 同じ ts・同じ generation は changed false。
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "ts", "expected_version": 2, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["changed"], false);
    assert_eq!(event_count(&env), 4);
    let events = env
        .store
        .script_domain_events(Some(ScriptDomain::CosDigest), 1)
        .expect("events");
    let recorded = serde_json::to_value(&events[0].event).expect("event json");
    assert_eq!(recorded["from_mode"], "compare", "{recorded}");
    assert_eq!(recorded["to_mode"], "ts", "{recorded}");
    assert_eq!(recorded["actor"], "human:test", "{recorded}");
}

const OPERATOR_TOKEN: &str = "operator-token-value-2";

fn env_with_operator() -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        extra_tokens: vec![ApiToken::new(
            Principal::Operator {
                name: "claude-code".into(),
            },
            OPERATOR_TOKEN,
        )],
        ..EnvOptions::default()
    })
}

/// 付記 S4-14: `→ts` は人の API token の主体だけ。外部 operator は 403、daemon（`system:*`）は store が拒む。
/// （CoS の操作は `script_domain_mode_through_cos_operations` で見る。）どの拒否でも表と記録は変わらない。
#[tokio::test]
async fn script_domain_mode_ts_requires_human_actor() {
    let env = env_with_operator();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["cos_digest"]);
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());

    let bearer = format!("Bearer {OPERATOR_TOKEN}");
    let resp = send(
        &app,
        put_json_with(
            &mode_path("cos_digest"),
            &json!({"mode": "ts", "expected_version": 1, "generation": sha}),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&resp, 403, "script_mode_requires_human");

    let generation = env
        .store
        .script_domain_mode("cos_digest")
        .expect("mode")
        .generation_id;
    let err = env
        .store
        .script_domain_set_mode(
            &ScriptModeChange {
                domain: "cos_digest".into(),
                to_mode: ScriptMode::Ts,
                to_generation: generation,
                expected_version: 1,
                actor: "system:script-startup".into(),
                reason: "test".into(),
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect_err("system actor");
    assert!(
        matches!(err, ScriptDomainError::TsRequiresHuman(ref a) if a == "system:script-startup"),
        "{err}"
    );
    let view = domain_view(&app, "cos_digest").await;
    assert_eq!(
        (view["mode"].clone(), view["version"].clone()),
        (json!("compare"), json!(1))
    );
    assert_eq!(event_count(&env), 1);

    // operator の token でも compare・rust への切り替えは今までどおり行える。
    let resp = send(
        &app,
        put_json_with(
            &mode_path("cos_digest"),
            &json!({"mode": "rust", "expected_version": 1}),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["domain"]["updated_by"], "operator:claude-code");
}

/// 付記 S4-14: `→ts` の元は同じ generation の `compare` だけ。`rust` からの直行と generation の同時変更は 422。
#[tokio::test]
async fn script_domain_mode_ts_requires_compare_same_generation() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["cos_digest"]);
    let sha2 = seed_generation(&env, 'c', &["cos_digest"]);
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "ts", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_problem(&resp, 422, "script_mode_ts_requires_compare");
    // generation の無い ts は generation の不備。
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "ts", "expected_version": 0}),
    )
    .await;
    assert_problem(&resp, 422, "script_generation_invalid");
    assert_eq!(event_count(&env), 0);

    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "ts", "expected_version": 1, "generation": sha2}),
    )
    .await;
    assert_problem(&resp, 422, "script_mode_ts_requires_compare");
    // 古い版は 409（版の検査が先）。
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "ts", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_problem(&resp, 409, "script_domain_version_conflict");
    let view = domain_view(&app, "cos_digest").await;
    assert_eq!(
        (view["mode"].clone(), view["version"].clone()),
        (json!("compare"), json!(1))
    );
    assert_eq!(view["bundle_sha256"], sha.as_str());
    assert_eq!(event_count(&env), 1);
}

/// `cos_digest` を人の主体で `expected_version = v` から compare → ts にする（版は v + 2 になる）。
async fn compare_then_ts(app: &axum::Router, sha: &str, v: u64) {
    let resp = put_mode(
        app,
        "cos_digest",
        json!({"mode": "compare", "expected_version": v, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let resp = put_mode(
        app,
        "cos_digest",
        json!({"mode": "ts", "expected_version": v + 1, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["domain"]["mode"], "ts");
}

/// 付記 S4-14: 戻し（ts→compare・ts→rust）は誰でも行える（外部 operator・CoS・daemon）。
#[tokio::test]
async fn script_domain_mode_ts_rollback_by_anyone() {
    let env = env_with_operator();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["cos_digest"]);

    // 外部 operator: ts → compare。
    compare_then_ts(&app, &sha, 0).await;
    let bearer = format!("Bearer {OPERATOR_TOKEN}");
    let resp = send(
        &env.router(),
        put_json_with(
            &mode_path("cos_digest"),
            &json!({"mode": "rust", "expected_version": 2}),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["domain"]["mode"], "rust");

    // CoS: ts → compare（/cos/operations で applied）。
    compare_then_ts(&app, &sha, 3).await;
    run_external(
        &env,
        "script-ts-back",
        "PUT",
        &mode_path("cos_digest"),
        json!({"mode": "compare", "expected_version": 5, "generation": sha}),
        "script_domain.mode_set",
    )
    .await;
    let row = env.store.script_domain_mode("cos_digest").expect("mode");
    assert_eq!((row.mode, row.version), (ScriptMode::Compare, 6));
    assert!(row.updated_by.starts_with("cos:"), "{}", row.updated_by);

    // daemon（system:*）: ts → rust。
    let resp = put_mode(
        &env.router(),
        "cos_digest",
        json!({"mode": "ts", "expected_version": 6, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    env.store
        .script_domain_set_mode(
            &ScriptModeChange {
                domain: "cos_digest".into(),
                to_mode: ScriptMode::Rust,
                to_generation: None,
                expected_version: 7,
                actor: "system:script-startup".into(),
                reason: "bundle missing".into(),
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("system rollback");
    let row = env.store.script_domain_mode("cos_digest").expect("mode");
    assert_eq!((row.mode, row.version), (ScriptMode::Rust, 8));
    assert_eq!(row.updated_by, "system:script-startup");
    assert_eq!(event_count(&env), 8);
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

    // 付記 S4-14: cos_digest の compare→ts は CoS では 403（human_only）で、rejected の記録が残り表は変わらない。
    let sha_digest = seed_generation(&env, 'f', &["cos_digest"]);
    run_external(
        &env,
        "script-digest-compare",
        "PUT",
        &mode_path("cos_digest"),
        json!({"mode": "compare", "expected_version": 0, "generation": sha_digest}),
        "script_domain.mode_set",
    )
    .await;
    let (thread, _, bearer) = cos_bearer(&env, "script-ts");
    let headers = [("authorization", bearer.as_str())];
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "script-ts",
                "PUT",
                &mode_path("cos_digest"),
                json!({"mode": "ts", "expected_version": 1, "generation": sha_digest}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 403, "script_mode_requires_human");
    let (rejected, reason): (i64, String) = db(&env)
        .query_row(
            "SELECT COUNT(*), MAX(reason) FROM cos_operations WHERE thread_id=?1 AND state='rejected'",
            [&thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("count");
    assert_eq!(rejected, 1);
    assert!(reason.contains("human_only"), "{reason}");
    let row = env.store.script_domain_mode("cos_digest").expect("mode");
    assert_eq!((row.mode.as_str(), row.version), ("compare", 1));
    assert_eq!(event_count(&env), 3);
}
