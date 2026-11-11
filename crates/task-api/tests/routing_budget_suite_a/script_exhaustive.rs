//! ADR 2026-11-04-lane-rules-exhaustive-evidence D7: 網羅の起動（`POST /script/domains/{domain}/exhaustive-runs`）と
//! 証拠の一覧（`GET`）の API の結合試験。
//!
//! 偽の登録簿（`FakeControl`）は store の `script_exhaustive_begin` を呼んで `running` の行を書くだけで、isolate も runner も
//! 持たない（実際に流す試験は `crates/celeris/tests/script_exhaustive.rs`）。ここで見るのは主体の検査（human・operator だけ）、
//! 各拒否の code と表が変わらないこと、一覧の立場、CoS の operations が EXCLUDED で拒むこと。

use crate::common;

use std::sync::Arc;

use common::cos_ops::{OPS, cos_bearer, db, op_body};
use common::*;
use serde_json::{Value, json};
use task_api::script_generations::{ScriptGenerationControl, ScriptGenerationError};
use task_api::{ApiToken, Principal};
use task_core::script_compare::{
    NewScriptGeneration, ScriptDomain, ScriptGeneration, ScriptGenerationState,
    ScriptGenerationTransition,
};
use task_core::script_exhaustive::{
    LANE_RULES_EXHAUSTIVE_CALLS, NewScriptExhaustiveRun, ScriptExhaustiveCounts,
    ScriptExhaustiveRun, ScriptExhaustiveState,
};
use task_core::store::{SqliteStore, current_reference_sha256};

const OPERATOR_TOKEN: &str = "operator-token-value-2";
const MCP_TOKEN: &str = "mcp-token-value-3";

fn runs_path(domain: &str) -> String {
    format!("/api/v1/script/domains/{domain}/exhaustive-runs")
}

fn mode_path(domain: &str) -> String {
    format!("/api/v1/script/domains/{domain}/mode")
}

struct FakeControl {
    store: SqliteStore,
}

impl ScriptGenerationControl for FakeControl {
    fn register(&self, _: &str) -> Result<ScriptGeneration, ScriptGenerationError> {
        Err(ScriptGenerationError::Unavailable("fake".into()))
    }

    fn retire(&self, _: &str, _: &str, _: &str) -> Result<ScriptGeneration, ScriptGenerationError> {
        Err(ScriptGenerationError::Unavailable("fake".into()))
    }

    fn exhaustive_start(
        &self,
        domain: ScriptDomain,
        bundle_sha256: &str,
        actor: &str,
        reason: &str,
    ) -> Result<ScriptExhaustiveRun, ScriptGenerationError> {
        Ok(self.store.script_exhaustive_begin(
            &NewScriptExhaustiveRun {
                domain,
                bundle_sha256: bundle_sha256.to_owned(),
                daemon_instance_id: "01FAKEINSTANCE".into(),
                requested_by: actor.to_owned(),
                reason: reason.to_owned(),
                reference_sha256: current_reference_sha256()
                    .map_err(|e| ScriptGenerationError::Runtime(e.to_string()))?
                    .to_owned(),
            },
            time::OffsetDateTime::now_utc(),
        )?)
    }
}

fn env_with(wired: bool) -> TestEnv {
    let mut env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        extra_tokens: vec![
            ApiToken::new(
                Principal::Operator {
                    name: "claude-code".into(),
                },
                OPERATOR_TOKEN,
            ),
            ApiToken::new(
                Principal::Mcp {
                    client_id: "mcp-test".into(),
                },
                MCP_TOKEN,
            ),
        ],
        ..EnvOptions::default()
    });
    if wired {
        let store = SqliteStore::open(&env.db_path).expect("store");
        env.state = env
            .state
            .clone()
            .with_script_generations(Arc::new(FakeControl { store }));
    }
    env
}

fn sha_of(digit: char) -> String {
    std::iter::repeat_n(digit, 64).collect()
}

fn seed_generation(env: &TestEnv, digit: char) -> String {
    let sha = sha_of(digit);
    env.store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha.clone(),
                host_api: "celeris.host/1".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: json!({"domains": ["lane_rules", "route_priority"]}).to_string(),
                state: ScriptGenerationState::Loaded,
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("generation");
    sha
}

async fn set_compare(env: &TestEnv, app: &axum::Router, domain: &str, sha: &str) {
    let version = env.store.script_domain_mode(domain).expect("mode").version;
    let resp = send(
        app,
        put_json_with(
            &mode_path(domain),
            &json!({"mode": "compare", "expected_version": version, "generation": sha}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
}

/// 停止で `rust` に戻された generation（`draining`）を `retired` → `loaded` に戻す。
fn seed_rearm(env: &TestEnv, sha: &str) {
    let generation = env
        .store
        .script_generation_by_sha256(sha)
        .expect("read")
        .expect("generation");
    if generation.state == ScriptGenerationState::Loaded {
        return;
    }
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
    env.store
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
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn start_with(app: &axum::Router, domain: &str, body: Value, token: Option<&str>) -> Resp {
    match token {
        None => {
            send(
                app,
                post_json_with(&runs_path(domain), &body, &admin_headers()),
            )
            .await
        }
        Some(t) => {
            let b = bearer(t);
            send(
                app,
                post_json_with(&runs_path(domain), &body, &[("authorization", b.as_str())]),
            )
            .await
        }
    }
}

fn rows(env: &TestEnv) -> i64 {
    db(env)
        .query_row("SELECT COUNT(*) FROM script_exhaustive_runs", [], |r| {
            r.get(0)
        })
        .expect("count")
}

fn finish(env: &TestEnv, id: &str, state: ScriptExhaustiveState, counts: &ScriptExhaustiveCounts) {
    env.store
        .script_exhaustive_finish(
            id,
            state,
            None,
            counts,
            None,
            time::OffsetDateTime::now_utc(),
        )
        .expect("finish");
}

fn passing_counts() -> ScriptExhaustiveCounts {
    ScriptExhaustiveCounts {
        calls: LANE_RULES_EXHAUSTIVE_CALLS,
        matches: LANE_RULES_EXHAUSTIVE_CALLS,
        ..ScriptExhaustiveCounts::default()
    }
}

/// 起こせるのは `human:*`・`operator:*` だけ。MCP は 403 で表を変えない。runtime が無ければ 503。
#[tokio::test]
async fn exhaustive_start_requires_human_or_operator() {
    let env = env_with(true);
    let app = env.router();
    let sha = seed_generation(&env, 'a');
    set_compare(&env, &app, "lane_rules", &sha).await;
    let body = json!({"generation": sha, "reason": "本番で網羅"});

    let resp = start_with(&app, "lane_rules", body.clone(), Some(MCP_TOKEN)).await;
    // MCP の token は読み取り専用で、handler の前（middleware）で 403 になる。handler 自身の主体の検査は
    // `script_exhaustive.rs` の単体試験（`cos:*`・`mcp:*`・`system:*`）で見る。
    assert_eq!(resp.status.as_u16(), 403, "{}", resp.text());
    assert_eq!(rows(&env), 0);

    let resp = start_with(&app, "lane_rules", body.clone(), Some(OPERATOR_TOKEN)).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let run = resp.json();
    assert_eq!(run["state"], "running", "{run}");
    assert_eq!(run["requested_by"], "operator:claude-code", "{run}");
    assert_eq!(run["bundle_sha256"], sha.as_str(), "{run}");
    assert_eq!(rows(&env), 1);
    finish(
        &env,
        run["id"].as_str().expect("id"),
        ScriptExhaustiveState::Passed,
        &passing_counts(),
    );

    let resp = start_with(&app, "lane_rules", body, None).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    assert_eq!(resp.json()["requested_by"], "human:test");
    assert_eq!(rows(&env), 2);

    // runtime が無い daemon は 503 で表を変えない。
    let unwired = env_with(false);
    let app = unwired.router();
    let resp = start_with(
        &app,
        "lane_rules",
        json!({"generation": sha, "reason": "x"}),
        None,
    )
    .await;
    assert_problem(&resp, 503, "script_runtime_unavailable");
    assert_eq!(rows(&unwired), 0);
}

/// 各拒否の code。どの拒否でも表を変えない。
#[tokio::test]
async fn exhaustive_start_rejects_each_conflict() {
    let env = env_with(true);
    let app = env.router();
    let sha = seed_generation(&env, 'a');
    let other = seed_generation(&env, 'b');
    let body = json!({"generation": sha, "reason": "r"});

    let resp = start_with(&app, "nope", body.clone(), None).await;
    assert_problem(&resp, 404, "script_domain_not_found");
    // mode が rust のうちは 409。
    let resp = start_with(&app, "lane_rules", body.clone(), None).await;
    assert_problem(&resp, 409, "script_exhaustive_mode_conflict");
    set_compare(&env, &app, "lane_rules", &sha).await;
    // 領域の行と違う generation も 409。
    let resp = start_with(
        &app,
        "lane_rules",
        json!({"generation": other, "reason": "r"}),
        None,
    )
    .await;
    assert_problem(&resp, 409, "script_exhaustive_mode_conflict");
    // lane_rules 以外は 422。
    set_compare(&env, &app, "route_priority", &sha).await;
    let resp = start_with(&app, "route_priority", body.clone(), None).await;
    assert_problem(&resp, 422, "script_exhaustive_domain_unsupported");
    // 本文の不備: 未知欄・欠け・空の reason。
    for bad in [
        json!({"generation": sha, "reason": "r", "extra": 1}),
        json!({"generation": sha}),
    ] {
        let resp = start_with(&app, "lane_rules", bad, None).await;
        assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    }
    let resp = start_with(
        &app,
        "lane_rules",
        json!({"generation": sha, "reason": "  "}),
        None,
    )
    .await;
    assert_problem(&resp, 422, "script_generation_invalid");
    assert_eq!(rows(&env), 0);
    // 全体の停止の間は 409。
    env.store
        .script_guard_halt("guard_mismatch: test", time::OffsetDateTime::now_utc())
        .expect("halt");
    let resp = start_with(&app, "lane_rules", body.clone(), None).await;
    assert_problem(&resp, 409, "script_guard_halted");
    assert_eq!(rows(&env), 0);
    env.store
        .script_guard_clear("human:test", "ok", time::OffsetDateTime::now_utc())
        .expect("clear");
    // 停止は mode を `rust` へ戻すので、compare に直してから起こす。
    let version = env.store.script_domain_mode("lane_rules").expect("mode");
    assert_eq!(version.mode.as_str(), "rust");
    seed_rearm(&env, &sha);
    set_compare(&env, &app, "lane_rules", &sha).await;
    // 走っている間は 409。
    let resp = start_with(&app, "lane_rules", body.clone(), None).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let running = resp.json()["id"].as_str().expect("id").to_owned();
    let resp = start_with(&app, "lane_rules", body, None).await;
    let problem = assert_problem(&resp, 409, "script_exhaustive_running");
    assert_eq!(problem["running_run_id"], running.as_str(), "{problem}");
    assert_eq!(rows(&env), 1);
}

/// 一覧は誰でも読める（MCP も 200）。証拠が無ければ `missing`、合格の行があれば `passed`、`lane_rules` 以外は
/// `not_applicable`。行は新しい順。
#[tokio::test]
async fn exhaustive_list_shows_standing() {
    let env = env_with(true);
    let app = env.router();
    let sha = seed_generation(&env, 'a');
    let get = |token: &'static str, domain: &'static str| {
        let b = bearer(token);
        let path = runs_path(domain);
        let app = app.clone();
        async move { send(&app, get_with(&path, &[("authorization", b.as_str())])).await }
    };
    let resp = get(MCP_TOKEN, "lane_rules").await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let list = resp.json();
    assert_eq!(list["standing"], "missing", "{list}");
    assert_eq!(list["runs"], json!([]), "{list}");
    assert_eq!(
        list["reference_sha256"],
        current_reference_sha256().expect("reference"),
        "{list}"
    );
    assert_problem(
        &get(MCP_TOKEN, "nope").await,
        404,
        "script_domain_not_found",
    );
    let other = get(MCP_TOKEN, "route_priority").await.json();
    assert_eq!(other["standing"], "not_applicable", "{other}");

    set_compare(&env, &app, "lane_rules", &sha).await;
    let resp = start_with(
        &app,
        "lane_rules",
        json!({"generation": sha, "reason": "r"}),
        None,
    )
    .await;
    let id = resp.json()["id"].as_str().expect("id").to_owned();
    let list = get(MCP_TOKEN, "lane_rules").await.json();
    // running は証拠に数えない。
    assert_eq!(list["standing"], "missing", "{list}");
    assert_eq!(list["runs"][0]["state"], "running", "{list}");
    assert_eq!(list["generation"], sha.as_str(), "{list}");
    finish(&env, &id, ScriptExhaustiveState::Passed, &passing_counts());
    let list = get(OPERATOR_TOKEN, "lane_rules").await.json();
    assert_eq!(list["standing"], "passed", "{list}");
    assert_eq!(list["runs"][0]["state"], "passed", "{list}");
    assert_eq!(
        list["runs"][0]["counts"]["calls"],
        LANE_RULES_EXHAUSTIVE_CALLS
    );
}

/// CoS の run credential では起こせない。直接の POST は通らず、`/cos/operations` に包むと registry の EXCLUDED
/// （`script_api_only`）で 422 `cos_operation_not_allowed`。どちらでも行は増えない。読み取りは CoS の run でも通る。
#[tokio::test]
async fn exhaustive_start_refused_for_cos() {
    let env = env_with(true);
    let app = env.router();
    let sha = seed_generation(&env, 'a');
    set_compare(&env, &app, "lane_rules", &sha).await;
    let (_, _, cos) = cos_bearer(&env, "script-exhaustive");
    let headers = [("authorization", cos.as_str())];
    let path = runs_path("lane_rules");
    let body = json!({"generation": sha, "reason": "cos"});
    let direct = send(&app, post_json_with(&path, &body, &headers)).await;
    assert_ne!(direct.status.as_u16(), 202, "{}", direct.text());
    let wrapped = send(
        &app,
        post_json_with(
            OPS,
            &op_body("script-exhaustive", "POST", &path, body),
            &headers,
        ),
    )
    .await;
    assert_problem(&wrapped, 422, "cos_operation_not_allowed");
    assert!(
        wrapped.text().contains("script_api_only"),
        "{}",
        wrapped.text()
    );
    assert_eq!(rows(&env), 0);
    let read = send(&app, get_with(&path, &headers)).await;
    assert_eq!(read.status.as_u16(), 200, "{}", read.text());
}
