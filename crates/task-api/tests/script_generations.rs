//! ADR 2026-11-02-ts-control-layer-boundary 付記 S5-1: generation の一覧・登録・退役の API の結合試験。
//!
//! 見るもの: daemon の登録簿（`ScriptGenerationControl`）が未接続なら 503 で表を変えない、検証の失敗の 422
//! （`invalid` の記録が残る・同じ hash の再登録は `script_generation_invalidated`）、生きた generation 3 つ目の 409、
//! 同じ hash の再登録は今の行、`active` の退役の 409、hash の形の 422、CoS の `script_generation.register`・
//! `script_generation.retire`（applied）。偽の登録簿は store に書くだけで isolate を持たない。

mod common;

use std::sync::Arc;

use common::cos_ops::run_external;
use common::*;
use serde_json::json;
use task_api::script_generations::{ScriptGenerationControl, ScriptGenerationError};
use task_core::script_compare::{
    NewScriptGeneration, ScriptGeneration, ScriptGenerationState, ScriptGenerationTransition,
};
use task_core::store::SqliteStore;

const LIST: &str = "/api/v1/script/generations";

fn retire_path(id: &str) -> String {
    format!("/api/v1/script/generations/{id}/retire")
}

fn sha(digit: char) -> String {
    std::iter::repeat_n(digit, 64).collect()
}

/// 偽の登録簿: hash `d…d` は未配備（409）、`e…e` は検証に落ちる（`invalid` を記録して 422）。他は `lane_rules` の manifest で登録する。
struct FakeControl {
    store: SqliteStore,
}

impl ScriptGenerationControl for FakeControl {
    fn register(&self, bundle_sha256: &str) -> Result<ScriptGeneration, ScriptGenerationError> {
        let new = NewScriptGeneration {
            bundle_sha256: bundle_sha256.to_owned(),
            host_api: "celeris.host/1".into(),
            schema_sha256: "0".repeat(64),
            manifest_json: json!({"domains": ["lane_rules"], "checkpoint_versions": []})
                .to_string(),
            state: ScriptGenerationState::Loaded,
        };
        let now = time::OffsetDateTime::now_utc();
        // hash `d…d` は配備先が無い（P1-C）: 表に書かずに `NotDeployed`。
        if bundle_sha256 == sha('d') {
            return Err(ScriptGenerationError::NotDeployed {
                bundle_sha256: bundle_sha256.to_owned(),
                reason: "cannot access script-bundles/d…d/manifest.json".into(),
            });
        }
        if bundle_sha256 == sha('e') {
            self.store
                .script_generation_record_invalid(&new, "selftest failed", now)?;
            return Err(ScriptGenerationError::Invalid {
                bundle_sha256: bundle_sha256.to_owned(),
                reason: "selftest failed".into(),
            });
        }
        Ok(self.store.script_generation_record(&new, now)?)
    }

    fn retire(
        &self,
        generation_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<ScriptGeneration, ScriptGenerationError> {
        Ok(self.store.script_generation_transition(
            &ScriptGenerationTransition {
                id: generation_id.to_owned(),
                to: ScriptGenerationState::Retired,
                actor: actor.to_owned(),
                reason: reason.to_owned(),
                cancelled_in_flight: None,
            },
            time::OffsetDateTime::now_utc(),
        )?)
    }
}

fn wired_env() -> TestEnv {
    let mut env = admin_env();
    let store = SqliteStore::open(&env.db_path).expect("store");
    env.state = env
        .state
        .clone()
        .with_script_generations(Arc::new(FakeControl { store }));
    env
}

fn generation_rows(env: &TestEnv) -> (i64, i64) {
    let conn = rusqlite::Connection::open(&env.db_path).expect("db");
    let rows = conn
        .query_row("SELECT COUNT(*) FROM script_generations", [], |r| r.get(0))
        .expect("rows");
    let events = conn
        .query_row("SELECT COUNT(*) FROM script_generation_events", [], |r| {
            r.get(0)
        })
        .expect("events");
    (rows, events)
}

async fn register(app: &axum::Router, bundle: &str) -> Resp {
    send(
        app,
        post_json_with(LIST, &json!({"bundle_sha256": bundle}), &admin_headers()),
    )
    .await
}

#[tokio::test]
async fn script_domain_generation_register_503_when_unwired() {
    let env = admin_env();
    let app = env.router();
    let resp = register(&app, &sha('a')).await;
    assert_problem(&resp, 503, "script_runtime_unavailable");
    let resp = send(
        &app,
        post_json_with(&retire_path("01X"), &json!({}), &admin_headers()),
    )
    .await;
    assert_problem(&resp, 503, "script_runtime_unavailable");
    assert_eq!(generation_rows(&env), (0, 0));
    // 一覧は store を読むので未接続でも 200。
    let resp = send(&app, get_admin(LIST)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["generations"], json!([]));
}

#[tokio::test]
async fn script_domain_generation_register_invalid_422() {
    let env = wired_env();
    let app = env.router();
    for bad in [
        "abc".to_owned(),
        "A".repeat(64),
        format!("{}g", "a".repeat(63)),
    ] {
        let resp = register(&app, &bad).await;
        assert_problem(&resp, 422, "script_generation_invalid");
    }
    assert_eq!(generation_rows(&env), (0, 0));
    let resp = send(
        &app,
        post_json_with(
            LIST,
            &json!({"bundle_sha256": sha('a'), "extra": 1}),
            &admin_headers(),
        ),
    )
    .await;
    assert_problem(&resp, 400, "bad_request");

    // 検証の失敗は 422 で、invalid の行が残る。同じ hash の再登録は invalidated。
    let resp = register(&app, &sha('e')).await;
    assert_problem(&resp, 422, "script_generation_invalid");
    let resp = register(&app, &sha('e')).await;
    assert_problem(&resp, 422, "script_generation_invalidated");
    let list = send(&app, get_admin(LIST)).await.json();
    assert_eq!(list["generations"][0]["state"], "invalid", "{list}");
    assert_eq!(generation_rows(&env), (1, 1));
}

#[tokio::test]
async fn script_domain_generation_limit_409() {
    let env = wired_env();
    let app = env.router();
    let resp = register(&app, &sha('a')).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let first = resp.json();
    assert_eq!(first["state"], "loaded");
    assert_eq!(first["domains"], json!(["lane_rules"]));
    assert_eq!(first["checkpoint_versions"], json!([]));
    // 同じ hash は今の行（記録も増えない）。
    let again = register(&app, &sha('a')).await;
    assert_eq!(again.json()["id"], first["id"]);
    assert_eq!(generation_rows(&env), (1, 1));
    let resp = register(&app, &sha('b')).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let resp = register(&app, &sha('c')).await;
    assert_problem(&resp, 409, "script_generation_limit");
    assert_eq!(generation_rows(&env), (2, 2));

    // 退役させれば 3 つ目を登録できる。
    let id = first["id"].as_str().expect("id");
    let resp = send(
        &app,
        post_json_with(
            &retire_path(id),
            &json!({"reason": "old"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["state"], "retired");
    let resp = register(&app, &sha('c')).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let list = send(&app, get_admin(LIST)).await.json();
    let states: Vec<&str> = list["generations"]
        .as_array()
        .expect("list")
        .iter()
        .map(|g| g["state"].as_str().expect("state"))
        .collect();
    assert_eq!(states, vec!["retired", "loaded", "loaded"]);
}

#[tokio::test]
async fn script_domain_generation_retire_active_409_and_unknown_404() {
    let env = wired_env();
    let app = env.router();
    let view = register(&app, &sha('a')).await.json();
    let resp = send(
        &app,
        put_json_with(
            "/api/v1/script/domains/lane_rules/mode",
            &json!({"mode": "compare", "expected_version": 0, "generation": sha('a')}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let before = generation_rows(&env);
    let id = view["id"].as_str().expect("id");
    let resp = send(
        &app,
        post_json_with(&retire_path(id), &json!({}), &admin_headers()),
    )
    .await;
    assert_problem(&resp, 409, "script_generation_active");
    let resp = send(
        &app,
        post_json_with(&retire_path("01NOPE"), &json!({}), &admin_headers()),
    )
    .await;
    assert_problem(&resp, 404, "script_generation_not_found");
    assert_eq!(generation_rows(&env), before);
}

#[tokio::test]
async fn script_domain_generation_through_cos_operations() {
    let env = wired_env();
    let op = run_external(
        &env,
        "script-register",
        "POST",
        LIST,
        json!({"bundle_sha256": sha('a')}),
        "script_generation.register",
    )
    .await;
    assert_eq!(op["target_kind"], "script_generation", "{op}");
    let generation = env
        .store
        .script_generation_by_sha256(&sha('a'))
        .expect("read")
        .expect("generation");
    assert_eq!(generation.state, ScriptGenerationState::Loaded);
    run_external(
        &env,
        "script-retire",
        "POST",
        &retire_path(&generation.id),
        json!({}),
        "script_generation.retire",
    )
    .await;
    let generation = env
        .store
        .script_generation_get(&generation.id)
        .expect("read")
        .expect("generation");
    assert_eq!(generation.state, ScriptGenerationState::Retired);
    let events = env
        .store
        .script_generation_events_after(0, 10)
        .expect("events");
    let retired = events.last().expect("retired event");
    let json = serde_json::to_value(&retired.event).expect("json");
    assert!(
        json.to_string().contains("cos:"),
        "the CoS actor is recorded: {json}"
    );
}

/// P1-C（S5 未解決 2）: 未配備の hash は 409 `script_bundle_not_deployed` で、表と記録を変えない。同じ hash の再試行も
/// 409 のまま（`script_generation_invalidated` に固定されない）。
#[tokio::test]
async fn script_domain_generation_register_not_deployed_409_is_retryable() {
    let env = wired_env();
    let app = env.router();
    for _ in 0..2 {
        let resp = register(&app, &sha('d')).await;
        assert_problem(&resp, 409, "script_bundle_not_deployed");
        assert_eq!(generation_rows(&env), (0, 0));
    }
    // 他の hash の登録は通る（未配備の拒否が登録簿を塞がない）。
    let resp = register(&app, &sha('a')).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
}
