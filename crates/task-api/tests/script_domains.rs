//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4・S2-5・S5-4: TS 制御層の領域の mode の API の結合試験。
//!
//! 見るもの: `GET /script/domains`（初期値・generation の hash と状態・`canary_bps`・guard の停止）、
//! `PUT /script/domains/{domain}/mode`（rust→compare→rust、同じ mode の 200・`changed: false`、全領域の
//! compare→canary→ts（人の主体だけ・同じ generation の前の段から・計数の前提・`canary_bps`）、戻しは誰でも、
//! guard の停止の 409、版の不一致の 409、未知の領域の 404、generation の不備の 422、未知欄の 400）、
//! `POST /script/guard/clear`（人だけ）、どの拒否でも表と `script_domain_events` が変わらないこと、
//! CoS の `/cos/operations` の `script_domain.mode_set`（直接の PUT は 422、包めば applied、`canary`・`ts` は 403 の
//! rejected の記録）。

mod common;

use common::cos_ops::{OPS, cos_bearer, db, op_body, run_external};
use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::script_compare::{
    NewScriptGeneration, ScriptDomain, ScriptDomainError, ScriptGenerationState,
    ScriptGenerationTransition, ScriptMode, ScriptModeChange,
};
use task_core::script_guard::{
    SCRIPT_GUARD_READY_COS, SCRIPT_GUARD_READY_PURE, ScriptGuardCall, ScriptGuardObservation,
};

const LIST: &str = "/api/v1/script/domains";
const GUARD_CLEAR: &str = "/api/v1/script/guard/clear";

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

/// 付記 S5-4: `rust` に戻した直後の generation は `draining` で、mode の行の行き先にできない。
/// `retired` にしてから登録し直し、`loaded` に戻す（同じ bundle なら同じ行）。
fn rearm_generation(env: &TestEnv, sha: &str) {
    let generation = env
        .store
        .script_generation_by_sha256(sha)
        .expect("read")
        .expect("generation");
    env.store
        .script_generation_transition(
            &ScriptGenerationTransition {
                id: generation.id.clone(),
                to: ScriptGenerationState::Retired,
                actor: "human:test".into(),
                reason: "drained".into(),
                cancelled_in_flight: None,
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("retire");
    let again = env
        .store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha.to_owned(),
                host_api: generation.host_api.clone(),
                schema_sha256: generation.schema_sha256.clone(),
                manifest_json: generation.manifest_json.clone(),
                state: ScriptGenerationState::Loaded,
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("record again");
    assert_eq!(again.state, ScriptGenerationState::Loaded);
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
            d["generation_id"].is_null()
                && d["bundle_sha256"].is_null()
                && d["generation_state"].is_null()
                && d["canary_bps"].is_null(),
            "{d}"
        );
    }
    assert_eq!(body["guard_halted"], Value::Null, "{body}");
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

/// 付記 S5-4 の昇格の前提: script-guard の計数を (f) の「進む条件」まで積む（`ts_side` は canary の TS 側の単位）。
fn observe_clean(env: &TestEnv, domain: ScriptDomain, sha: &str, ts_side: bool) {
    let generation = env
        .store
        .script_generation_by_sha256(sha)
        .expect("read")
        .expect("generation");
    let required = match domain {
        ScriptDomain::CosDigest => SCRIPT_GUARD_READY_COS,
        _ => SCRIPT_GUARD_READY_PURE,
    };
    for _ in 0..required {
        env.store
            .script_guard_observe(
                domain,
                &generation.id,
                ScriptGuardObservation {
                    call: ScriptGuardCall::Ok,
                    safety_violation: false,
                    ts_side,
                },
                time::OffsetDateTime::now_utc(),
            )
            .expect("observe");
    }
}

fn domain_of(name: &str) -> ScriptDomain {
    ScriptDomain::parse(name).expect("domain")
}

/// 人の主体で `expected_version = v` の `compare` から canary（`canary_bps` 500）へ進める（版は v + 1 になる）。
async fn compare_to_canary(env: &TestEnv, app: &axum::Router, domain: &str, sha: &str, v: u64) {
    observe_clean(env, domain_of(domain), sha, false);
    let resp = put_mode(
        app,
        domain,
        json!({"mode": "canary", "expected_version": v, "generation": sha, "canary_bps": 500}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["domain"]["mode"], "canary");
}

/// 人の主体で `expected_version = v` の `canary` から ts へ進める（版は v + 1 になる）。
async fn canary_to_ts(env: &TestEnv, app: &axum::Router, domain: &str, sha: &str, v: u64) {
    observe_clean(env, domain_of(domain), sha, true);
    let resp = put_mode(
        app,
        domain,
        json!({"mode": "ts", "expected_version": v, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["domain"]["mode"], "ts");
}

/// 人の主体で `expected_version = v` から compare → canary → ts にする（版は v + 3 になる）。
async fn promote_to_ts(env: &TestEnv, app: &axum::Router, domain: &str, sha: &str, v: u64) {
    let resp = put_mode(
        app,
        domain,
        json!({"mode": "compare", "expected_version": v, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    compare_to_canary(env, app, domain, sha, v + 1).await;
    canary_to_ts(env, app, domain, sha, v + 2).await;
}

/// 付記 S5-4: 全領域で compare → canary → ts を受ける。canary は計数が足りなければ 422 `script_canary_not_ready`
/// （本文に計数）、`canary_bps` の欠け・範囲外・余分は 422 で、どの拒否でも表と記録は変わらない。
#[tokio::test]
async fn script_domain_mode_canary_requires_guard_ready() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["lane_rules", "route_priority", "cos_digest"]);
    let mut events = 0;
    for domain in ["lane_rules", "route_priority", "cos_digest"] {
        // rust から canary へは直行できない。
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 0, "generation": sha, "canary_bps": 500}),
        )
        .await;
        assert_problem(&resp, 422, "script_mode_requires_previous_stage");
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "compare", "expected_version": 0, "generation": sha}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        events += 1;
        // 計数が無い canary は 422（本文に計数）。
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 1, "generation": sha, "canary_bps": 500}),
        )
        .await;
        let problem = assert_problem(&resp, 422, "script_canary_not_ready");
        assert_eq!(problem["readiness"]["ready"], false, "{problem}");
        assert_eq!(problem["readiness"]["clean_streak"], 0, "{problem}");
        // canary_bps の欠け・範囲外、compare への余分な canary_bps。
        for body in [
            json!({"mode": "canary", "expected_version": 1, "generation": sha}),
            json!({"mode": "canary", "expected_version": 1, "generation": sha, "canary_bps": 0}),
            json!({"mode": "canary", "expected_version": 1, "generation": sha, "canary_bps": 10000}),
            json!({"mode": "compare", "expected_version": 1, "generation": sha, "canary_bps": 10}),
        ] {
            let resp = put_mode(&app, domain, body.clone()).await;
            assert_problem(&resp, 422, "script_generation_invalid");
        }
        let view = domain_view(&app, domain).await;
        assert_eq!(
            (view["mode"].clone(), view["version"].clone()),
            (json!("compare"), json!(1))
        );
        assert_eq!(event_count(&env), events);

        compare_to_canary(&env, &app, domain, &sha, 1).await;
        events += 1;
        let view = domain_view(&app, domain).await;
        assert_eq!(view["canary_bps"], 500, "{view}");
        assert_eq!(view["generation_state"], "active", "{view}");
        assert_eq!(view["bundle_sha256"], sha.as_str(), "{view}");
        // canary_bps の変更は人の主体で、同じ generation のまま。同じ値は changed false。
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 2, "generation": sha, "canary_bps": 2500}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        assert_eq!(resp.json()["domain"]["canary_bps"], 2500);
        events += 1;
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 3, "generation": sha, "canary_bps": 2500}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        assert_eq!(resp.json()["changed"], false);
        assert_eq!(event_count(&env), events);
        let recorded = env
            .store
            .script_domain_events(Some(domain_of(domain)), 1)
            .expect("events")
            .remove(0);
        assert_eq!(recorded.canary_bps, Some(2500));
        let recorded = serde_json::to_value(&recorded.event).expect("event json");
        assert_eq!(recorded["to_mode"], "canary", "{recorded}");
        assert_eq!(recorded["actor"], "human:test", "{recorded}");
    }
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

async fn put_mode_as_operator(app: &axum::Router, domain: &str, body: Value) -> Resp {
    let bearer = format!("Bearer {OPERATOR_TOKEN}");
    send(
        app,
        put_json_with(
            &mode_path(domain),
            &body,
            &[("authorization", bearer.as_str())],
        ),
    )
    .await
}

/// 付記 S5-4: `compare→canary`・`canary` の比率の変更・`canary→ts` は人の API token の主体だけ。外部 operator は 403、
/// daemon（`system:*`）は store が拒む（CoS は `script_domain_mode_through_cos_operations`）。計数が足りていても、
/// どの拒否でも表と記録は変わらない。
#[tokio::test]
async fn script_domain_mode_canary_requires_human() {
    let env = env_with_operator();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["cos_digest", "lane_rules"]);
    for domain in ["cos_digest", "lane_rules"] {
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "compare", "expected_version": 0, "generation": sha}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        observe_clean(&env, domain_of(domain), &sha, false);
        let resp = put_mode_as_operator(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 1, "generation": sha, "canary_bps": 500}),
        )
        .await;
        assert_problem(&resp, 403, "script_mode_requires_human");
        // generation の不備より主体の検査が先（未登録の hash でも 403）。
        let resp = put_mode_as_operator(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 1, "generation": "f".repeat(64), "canary_bps": 500}),
        )
        .await;
        assert_problem(&resp, 403, "script_mode_requires_human");
        let generation = env
            .store
            .script_domain_mode(domain)
            .expect("mode")
            .generation_id;
        let err = env
            .store
            .script_domain_set_canary(
                &ScriptModeChange {
                    domain: domain.into(),
                    to_mode: ScriptMode::Canary,
                    to_generation: generation,
                    expected_version: 1,
                    actor: "system:script-guard".into(),
                    reason: "test".into(),
                },
                500,
                time::OffsetDateTime::now_utc(),
            )
            .expect_err("system actor");
        assert!(
            matches!(err, ScriptDomainError::CanaryRequiresHuman(ref a) if a == "system:script-guard"),
            "{err}"
        );
        let view = domain_view(&app, domain).await;
        assert_eq!(
            (view["mode"].clone(), view["version"].clone()),
            (json!("compare"), json!(1))
        );

        // 人が canary にした後も、operator は比率の変更と ts への昇格ができない。
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "canary", "expected_version": 1, "generation": sha, "canary_bps": 500}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        observe_clean(&env, domain_of(domain), &sha, true);
        for body in [
            json!({"mode": "canary", "expected_version": 2, "generation": sha, "canary_bps": 9000}),
            json!({"mode": "ts", "expected_version": 2, "generation": sha}),
        ] {
            let resp = put_mode_as_operator(&app, domain, body.clone()).await;
            assert_problem(&resp, 403, "script_mode_requires_human");
        }
        let view = domain_view(&app, domain).await;
        assert_eq!(
            (
                view["mode"].clone(),
                view["version"].clone(),
                view["canary_bps"].clone()
            ),
            (json!("canary"), json!(2), json!(500))
        );
    }
    assert_eq!(event_count(&env), 4);
    let generations: i64 = db(&env)
        .query_row("SELECT COUNT(*) FROM script_generation_events", [], |row| {
            row.get(0)
        })
        .expect("count");
    // 登録 1 件と activated 1 件だけ（拒否は generation の記録も増やさない）。
    assert_eq!(generations, 2);
}

/// 付記 S5-4: `ts` の元は同じ generation の `canary` だけ。`compare` からの直行（S4 で許した `cos_digest` も）・
/// generation の同時変更は 422、古い版は 409。
#[tokio::test]
async fn script_domain_mode_ts_requires_same_generation_canary() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["cos_digest", "route_priority"]);
    let sha2 = seed_generation(&env, 'c', &["cos_digest", "route_priority"]);
    for domain in ["cos_digest", "route_priority"] {
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "ts", "expected_version": 0, "generation": sha}),
        )
        .await;
        assert_problem(&resp, 422, "script_mode_requires_previous_stage");
        let resp = put_mode(&app, domain, json!({"mode": "ts", "expected_version": 0})).await;
        assert_problem(&resp, 422, "script_generation_invalid");
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "compare", "expected_version": 0, "generation": sha}),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        // compare からの ts は計数が足りていても拒む（canary を経る）。
        observe_clean(&env, domain_of(domain), &sha, true);
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "ts", "expected_version": 1, "generation": sha}),
        )
        .await;
        assert_problem(&resp, 422, "script_mode_requires_previous_stage");
        compare_to_canary(&env, &app, domain, &sha, 1).await;
        // 別の generation の ts、canary の TS 側の計数が無い ts、古い版。
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "ts", "expected_version": 2, "generation": sha2}),
        )
        .await;
        assert_problem(&resp, 422, "script_mode_requires_previous_stage");
        let resp = put_mode(
            &app,
            domain,
            json!({"mode": "ts", "expected_version": 1, "generation": sha}),
        )
        .await;
        assert_problem(&resp, 409, "script_domain_version_conflict");
        let view = domain_view(&app, domain).await;
        assert_eq!(
            (view["mode"].clone(), view["version"].clone()),
            (json!("canary"), json!(2))
        );
        canary_to_ts(&env, &app, domain, &sha, 2).await;
        let view = domain_view(&app, domain).await;
        assert_eq!(view["canary_bps"], Value::Null, "{view}");
        assert_eq!(view["generation_state"], "active", "{view}");
    }
    assert_eq!(event_count(&env), 6);
}

/// 付記 S5-4: 戻し（canary・ts → compare、→ rust）は認証された誰でも行える（外部 operator・CoS・daemon）。
#[tokio::test]
async fn script_domain_mode_rollback_by_anyone() {
    let env = env_with_operator();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["cos_digest"]);

    // 外部 operator: ts → rust。
    promote_to_ts(&env, &app, "cos_digest", &sha, 0).await;
    let resp = put_mode_as_operator(
        &app,
        "cos_digest",
        json!({"mode": "rust", "expected_version": 3}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["domain"]["mode"], "rust");
    assert_eq!(resp.json()["domain"]["updated_by"], "operator:claude-code");

    // 外部 operator: canary → compare。
    rearm_generation(&env, &sha);
    let resp = put_mode(
        &app,
        "cos_digest",
        json!({"mode": "compare", "expected_version": 4, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    compare_to_canary(&env, &app, "cos_digest", &sha, 5).await;
    let resp = put_mode_as_operator(
        &app,
        "cos_digest",
        json!({"mode": "compare", "expected_version": 6, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["domain"]["canary_bps"], Value::Null);

    // CoS: ts → compare（/cos/operations で applied）。
    compare_to_canary(&env, &app, "cos_digest", &sha, 7).await;
    canary_to_ts(&env, &app, "cos_digest", &sha, 8).await;
    run_external(
        &env,
        "script-ts-back",
        "PUT",
        &mode_path("cos_digest"),
        json!({"mode": "compare", "expected_version": 9, "generation": sha}),
        "script_domain.mode_set",
    )
    .await;
    let row = env.store.script_domain_mode("cos_digest").expect("mode");
    assert_eq!((row.mode, row.version), (ScriptMode::Compare, 10));
    assert!(row.updated_by.starts_with("cos:"), "{}", row.updated_by);

    // daemon（system:*）: canary → rust。
    compare_to_canary(&env, &app, "cos_digest", &sha, 10).await;
    env.store
        .script_domain_set_mode(
            &ScriptModeChange {
                domain: "cos_digest".into(),
                to_mode: ScriptMode::Rust,
                to_generation: None,
                expected_version: 11,
                actor: "system:script-guard".into(),
                reason: "mismatch".into(),
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("system rollback");
    let row = env.store.script_domain_mode("cos_digest").expect("mode");
    assert_eq!((row.mode, row.version), (ScriptMode::Rust, 12));
    assert_eq!(row.updated_by, "system:script-guard");
    assert_eq!(event_count(&env), 12);
}

/// 付記 S5-4: script-guard の全体の停止の間は `rust` 以外を 409 で拒み（戻しは通る）、停止を解くのは人だけ。
#[tokio::test]
async fn script_domain_mode_refused_while_halted() {
    let env = env_with_operator();
    let app = env.router();
    let sha = seed_generation(&env, 'b', &["lane_rules"]);
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "compare", "expected_version": 0, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let rolled = env
        .store
        .script_guard_halt("unvalidated effect", time::OffsetDateTime::now_utc())
        .expect("halt");
    assert_eq!(rolled, vec![ScriptDomain::LaneRules]);
    let list = send(&app, get_admin(LIST)).await.json();
    assert_eq!(list["guard_halted"], "unvalidated effect", "{list}");
    let before = event_count(&env);
    rearm_generation(&env, &sha);
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "compare", "expected_version": 2, "generation": sha}),
    )
    .await;
    let problem = assert_problem(&resp, 409, "script_guard_halted");
    assert_eq!(problem["halted"], "unvalidated effect", "{problem}");
    // rust への（同じ mode の）変更は止まっていても通る。
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "rust", "expected_version": 2}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(event_count(&env), before);

    script_domain_guard_clear_checks(&env, &app).await;
    let resp = put_mode(
        &app,
        "lane_rules",
        json!({"mode": "compare", "expected_version": 2, "generation": sha}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
}

/// 付記 S5-4: `POST /script/guard/clear` は人の主体だけ。operator は 403 で表も記録も変えない。
async fn script_domain_guard_clear_checks(env: &TestEnv, app: &axum::Router) {
    let guard_events = || -> i64 {
        db(env)
            .query_row(
                "SELECT COUNT(*) FROM script_generation_events WHERE json LIKE '%script_guard_cleared%'",
                [],
                |row| row.get(0),
            )
            .expect("count")
    };
    let bearer = format!("Bearer {OPERATOR_TOKEN}");
    let resp = send(
        app,
        post_json_with(
            GUARD_CLEAR,
            &json!({"reason": "operator"}),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&resp, 403, "script_mode_requires_human");
    assert!(env.store.script_guard_halted().expect("halted").is_some());
    assert_eq!(guard_events(), 0);
    let resp = send(
        app,
        post_json_with(
            GUARD_CLEAR,
            &json!({"reason": "原因を直した"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["cleared"], true);
    assert_eq!(guard_events(), 1);
    let resp = send(
        app,
        post_json_with(GUARD_CLEAR, &json!({"reason": "again"}), &admin_headers()),
    )
    .await;
    assert_eq!(resp.json()["cleared"], false);
    assert_eq!(guard_events(), 1);
    let list = send(app, get_admin(LIST)).await.json();
    assert_eq!(list["guard_halted"], Value::Null, "{list}");
}

/// 付記 S5-4: guard の解除だけを見る（停止していない間の解除は `cleared: false`、人以外は 403）。
#[tokio::test]
async fn script_domain_guard_clear_human_only() {
    let env = env_with_operator();
    let app = env.router();
    let resp = send(
        &app,
        post_json_with(GUARD_CLEAR, &json!({"reason": "none"}), &admin_headers()),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["cleared"], false);
    env.store
        .script_guard_halt("test", time::OffsetDateTime::now_utc())
        .expect("halt");
    script_domain_guard_clear_checks(&env, &app).await;
    let resp = send(
        &app,
        post_json_with(
            GUARD_CLEAR,
            &json!({"reason": "x", "extra": 1}),
            &admin_headers(),
        ),
    )
    .await;
    assert_problem(&resp, 400, "bad_request");
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

    // 付記 S5-4: canary は CoS では計数が足りていても 403（human_only）で、rejected の記録が残り、表は変わらない。
    rearm_generation(&env, &sha);
    run_external(
        &env,
        "script-compare-again",
        "PUT",
        &mode_path("lane_rules"),
        json!({"mode": "compare", "expected_version": 2, "generation": sha}),
        "script_domain.mode_set",
    )
    .await;
    observe_clean(&env, ScriptDomain::LaneRules, &sha, false);
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
                json!({"mode": "canary", "expected_version": 3, "generation": sha, "canary_bps": 500}),
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
    let row = env.store.script_domain_mode("lane_rules").expect("mode");
    assert_eq!((row.mode.as_str(), row.version), ("compare", 3));
    assert_eq!(event_count(&env), 3);

    // cos_digest の canary→ts も CoS では 403（human_only）で、rejected の記録が残り表は変わらない。
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
    compare_to_canary(&env, &app, "cos_digest", &sha_digest, 1).await;
    observe_clean(&env, ScriptDomain::CosDigest, &sha_digest, true);
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
                json!({"mode": "ts", "expected_version": 2, "generation": sha_digest}),
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
    assert_eq!((row.mode.as_str(), row.version), ("canary", 2));
    assert_eq!(event_count(&env), 5);
}

/// P1-C: CoS の run credential では script-guard の停止を解けない。直接の POST は CoS の監査の文脈が無いので通らず、
/// `/cos/operations` に包むと registry の EXCLUDED（human_only）で拒まれる（rejected の記録が残る）。どちらでも停止は
/// 残り、`script_guard_cleared` は書かれない。人の token だけが解ける（権限を広げない）。
#[tokio::test]
async fn script_domain_guard_clear_refused_for_cos() {
    let env = admin_env();
    let app = env.router();
    env.store
        .script_guard_halt("guard_mismatch: test", time::OffsetDateTime::now_utc())
        .expect("halt");
    let cleared_events = || -> i64 {
        db(&env)
            .query_row(
                "SELECT COUNT(*) FROM script_generation_events WHERE json LIKE '%script_guard_cleared%'",
                [],
                |row| row.get(0),
            )
            .expect("count")
    };
    let (thread, _, bearer) = cos_bearer(&env, "script-guard-clear");
    let headers = [("authorization", bearer.as_str())];
    let direct = send(
        &app,
        post_json_with(GUARD_CLEAR, &json!({"reason": "cos"}), &headers),
    )
    .await;
    assert_ne!(direct.status.as_u16(), 200, "{}", direct.text());
    let wrapped = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "script-guard-clear",
                "POST",
                GUARD_CLEAR,
                json!({"reason": "cos"}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&wrapped, 422, "cos_operation_not_allowed");
    assert!(wrapped.text().contains("human_only"), "{}", wrapped.text());
    let rejected: i64 = db(&env)
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='rejected'",
            [&thread],
            |row| row.get(0),
        )
        .expect("rejected");
    // 直接の POST も包んだ操作も rejected として残る。
    assert!(rejected >= 1, "{}", wrapped.text());
    assert!(env.store.script_guard_halted().expect("halted").is_some());
    assert_eq!(cleared_events(), 0);
    // 人の token では解ける。
    let resp = send(
        &app,
        post_json_with(
            GUARD_CLEAR,
            &json!({"reason": "人が確かめた"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["cleared"], true);
    assert_eq!(cleared_events(), 1);
}

/// P1-C: CoS の run credential は script の読み取り（`GET /script/domains`・`GET /script/generations`）を直接通せる。
/// `celerisctl script domains`・`generations list`・`guard status` は CoS の run でこの 2 つを読む（`cos_read_only`）。
/// 読み取りは表を変えない。
#[tokio::test]
async fn script_cos_read_only_gets_pass_with_run_credential() {
    let env = admin_env();
    let app = env.router();
    let sha = seed_generation(&env, 'a', &["lane_rules"]);
    let (_, _, bearer) = cos_bearer(&env, "script-read");
    let headers = [("authorization", bearer.as_str())];
    let domains = send(&app, get_with(LIST, &headers)).await;
    assert_eq!(domains.status.as_u16(), 200, "{}", domains.text());
    assert!(
        domains.json()["guard_halted"].is_null(),
        "{}",
        domains.text()
    );
    let generations = send(&app, get_with("/api/v1/script/generations", &headers)).await;
    assert_eq!(generations.status.as_u16(), 200, "{}", generations.text());
    assert!(generations.text().contains(&sha), "{}", generations.text());
    let ops: i64 = db(&env)
        .query_row("SELECT COUNT(*) FROM cos_operations", [], |row| row.get(0))
        .expect("ops");
    assert_eq!(ops, 0);
}
