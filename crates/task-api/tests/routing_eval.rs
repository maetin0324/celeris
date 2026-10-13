//! 評価 run の試行の API（`POST`/`GET /routing-eval/trials`、`POST .../cancel`）の結合テスト。
//!
//! 見るもの: 投入（201・batch_id・store に `check_cmd` が入る）、**冪等な再投入**（同じ trial_id・同じ
//! batch_id・行が増えない）、400（空 trials・未知の source/model/harness・repo 不在・未知欄・timeout 0）、
//! 一覧（`batch_id`/`status` の絞り込み・**応答に `check_cmd` が現れない**）、cancel（200・終端は 409・
//! 無い id は 404）、認証（token を設定した daemon では書き込みに bearer が要る）。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::routing_eval_trial::EvalTrialStatus;

const BASE: &str = "/api/v1/routing-eval/trials";

/// 宣言された harness `claude_code` と LLM source `claude_oauth`（model `claude-sonnet-5`）。
struct DeclaredRoutes;

impl task_api::HarnessesReader for DeclaredRoutes {
    fn harnesses(&self) -> Vec<task_api::types::HarnessView> {
        vec![task_api::types::HarnessView {
            id: "claude_code".into(),
            kind: "cli".into(),
            api_formats: vec!["anthropic".into()],
            source_kinds: vec!["claude_oauth".into()],
            capabilities: vec!["repo".into()],
            rules: vec![],
            execution_class_defaults: vec![],
            route_defaults: vec![],
        }]
    }

    fn source_models(&self) -> Vec<(String, Vec<String>)> {
        vec![("claude_oauth".into(), vec!["claude-sonnet-5".into()])]
    }
}

/// 宣言（`DeclaredRoutes`）を持つ daemon の API。
fn env_with_routes() -> TestEnv {
    TestEnv::with(EnvOptions {
        harnesses: Some(std::sync::Arc::new(DeclaredRoutes)),
        ..Default::default()
    })
}

/// `repo` に実在のディレクトリ（tempdir）を使う 1 試行。`env_with_routes` の daemon は harness
/// `claude_code`・source `claude_oauth`（model `claude-sonnet-5`）を宣言しているので、`-` で書いた
/// この 3 つの id も「daemon が知っている」側に入る。
fn trial(problem: &str, trial_no: u32, repo: &str) -> Value {
    json!({
        "problem_id": problem,
        "trial_no": trial_no,
        "source_id": "claude-oauth",
        "model_id": "claude-sonnet-5",
        "harness_id": "claude-code",
        "repo": repo,
        "base_commit": "0123456789abcdef",
        "objective": "worker に見せる本文（acceptance の command は含まない）",
        "check": {"cmd": "sh notes/hidden-check.sh", "timeout_secs": 600}
    })
}

fn body(manifest_sha: &str, trials: Vec<Value>) -> Value {
    json!({"manifest_sha": manifest_sha, "trials": trials})
}

/// 2 試行を投入して応答を返す（201 を確かめる）。
async fn post_batch(app: &axum::Router, manifest_sha: &str, repo: &str) -> Value {
    let resp = send(
        app,
        post_json(
            BASE,
            &body(
                manifest_sha,
                vec![trial("p1", 1, repo), trial("p1", 2, repo)],
            ),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    resp.json()
}

fn cancel(path_id: &str) -> axum::http::Request<axum::body::Body> {
    post_json(&format!("{BASE}/{path_id}/cancel"), &json!({}))
}

#[tokio::test]
async fn post_creates_a_batch_and_keeps_the_check_command_server_side() {
    let env = env_with_routes();
    let app = env.router();
    let repo = env.dir.path().to_str().expect("utf-8 path").to_string();

    let created = post_batch(&app, "sha-manifest-1", &repo).await;
    let batch_id = created["batch_id"].as_str().expect("batch_id").to_string();
    assert_eq!(batch_id.len(), 26, "ULID: {batch_id}");
    let trials = created["trials"].as_array().expect("trials");
    assert_eq!(trials.len(), 2);
    assert_eq!(trials[0]["problem_id"], "p1");
    assert_eq!(trials[0]["trial_no"], 1);
    assert_eq!(trials[0]["status"], "queued");
    // 応答は trial_id・problem_id・trial_no・status だけ（check も objective も返さない）。
    let mut keys: Vec<&str> = trials[0]
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["problem_id", "status", "trial_id", "trial_no"]);
    assert!(!created.to_string().contains("hidden-check"), "{created}");

    // store には採点用の check_cmd と worker 用の objective が入っている。
    let rows = env
        .store
        .eval_trials_list(Some(&batch_id), None)
        .expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].check_cmd, "sh notes/hidden-check.sh");
    assert_eq!(rows[0].check_timeout_secs, 600);
    assert_eq!(rows[0].status, EvalTrialStatus::Queued);
    assert_eq!(rows[0].repo, repo);
    assert!(rows[0].objective.contains("worker に見せる本文"));

    // 一覧にも check_cmd は出ない。
    let list = send(&app, get(&format!("{BASE}?batch_id={batch_id}"))).await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let text = list.text();
    assert!(!text.contains("hidden-check"), "{text}");
    assert!(!text.contains("check_cmd"), "{text}");
    assert!(!text.contains("objective"), "{text}");
    let json = list.json();
    assert_eq!(json["trials"].as_array().expect("trials").len(), 2);
    assert_eq!(json["trials"][0]["status"], "queued");
    assert_eq!(json["trials"][0]["source_id"], "claude-oauth");
    assert_eq!(json["trials"][0]["harness_version"], Value::Null);
    assert_eq!(json["trials"][0]["run_id"], Value::Null);
    assert_eq!(json["trials"][0]["check_exit"], Value::Null);
    assert_eq!(json["trials"][0]["timed_out"], false);
    assert!(json["trials"][0]["created_at"].is_string(), "{json}");

    // query を付けられるのは batch_id と status だけ。
    let no_query = send(&app, get(BASE)).await;
    assert_eq!(no_query.json()["trials"].as_array().expect("t").len(), 2);
}

#[tokio::test]
async fn reposting_the_same_trials_is_idempotent() {
    let env = env_with_routes();
    let app = env.router();
    let repo = env.dir.path().to_str().expect("utf-8 path").to_string();

    let first = post_batch(&app, "sha-manifest-2", &repo).await;
    let again = post_batch(&app, "sha-manifest-2", &repo).await;
    assert_eq!(again["batch_id"], first["batch_id"], "{again}");
    assert_eq!(again["trials"], first["trials"], "{again}");

    // 行は増えていない（batch も 1 つ）。
    let rows = env.store.eval_trials_list(None, None).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].batch_id, rows[1].batch_id);

    // 1 試行だけの再投入も同じ行を返す（batch は既存のまま）。
    let partial = send(
        &app,
        post_json(BASE, &body("sha-manifest-2", vec![trial("p1", 1, &repo)])),
    )
    .await;
    assert_eq!(partial.status.as_u16(), 201, "{}", partial.text());
    assert_eq!(partial.json()["batch_id"], first["batch_id"]);
    assert_eq!(
        partial.json()["trials"][0]["trial_id"],
        first["trials"][0]["trial_id"]
    );
    assert_eq!(
        env.store.eval_trials_list(None, None).expect("list").len(),
        2
    );

    // manifest が違えば別の試行（新しい batch に入る）。
    let other = post_batch(&app, "sha-manifest-3", &repo).await;
    assert_ne!(other["batch_id"], first["batch_id"]);
    assert_eq!(
        env.store.eval_trials_list(None, None).expect("list").len(),
        4,
        "manifest sha が違うと同じ問題でも別の試行"
    );
}

#[tokio::test]
async fn post_rejects_empty_unknown_route_and_missing_repo_with_400() {
    let env = env_with_routes();
    let app = env.router();
    let repo = env.dir.path().to_str().expect("utf-8 path").to_string();

    let empty = send(&app, post_json(BASE, &body("sha-1", vec![]))).await;
    let problem = assert_problem(&empty, 400, "eval_trial_invalid");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("trials must not be empty")),
        "{problem}"
    );

    for (field, kind, value) in [
        ("source_id", "source", "no-such-source"),
        ("model_id", "model", "no-such-model"),
        ("harness_id", "harness", "no-such-harness"),
    ] {
        let mut item = trial("p1", 1, &repo);
        item[field] = json!(value);
        let resp = send(&app, post_json(BASE, &body("sha-1", vec![item]))).await;
        let problem = assert_problem(&resp, 400, "eval_trial_invalid");
        assert!(
            problem["detail"]
                .as_str()
                .is_some_and(|d| d.contains(&format!("unknown {kind}"))),
            "{field}: {problem}"
        );
    }

    let missing = send(
        &app,
        post_json(
            BASE,
            &body(
                "sha-1",
                vec![trial("p1", 1, "/nonexistent/eval-trial-repo")],
            ),
        ),
    )
    .await;
    let problem = assert_problem(&missing, 400, "eval_trial_invalid");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("no such repository directory")),
        "{problem}"
    );

    // 未知欄は 400 `bad_request`（serde の deny_unknown_fields）。
    let mut unknown_field = trial("p1", 1, &repo);
    unknown_field["nope"] = json!(1);
    let resp = send(&app, post_json(BASE, &body("sha-1", vec![unknown_field]))).await;
    assert_problem(&resp, 400, "bad_request");

    // check.timeout_secs = 0 は採点できないので 400。
    let mut zero_timeout = trial("p1", 1, &repo);
    zero_timeout["check"]["timeout_secs"] = json!(0);
    let resp = send(&app, post_json(BASE, &body("sha-1", vec![zero_timeout]))).await;
    assert_problem(&resp, 400, "eval_trial_invalid");

    // 同じ batch の中の重複も 400（store の冪等で 1 行に潰れて応答と合わなくなるのを防ぐ）。
    let dup = send(
        &app,
        post_json(
            BASE,
            &body("sha-1", vec![trial("p1", 1, &repo), trial("p1", 1, &repo)]),
        ),
    )
    .await;
    let problem = assert_problem(&dup, 400, "eval_trial_invalid");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("duplicate trial")),
        "{problem}"
    );

    // どれも行を作っていない。
    assert_eq!(
        env.store.eval_trials_list(None, None).expect("list").len(),
        0,
        "400 の投入は行を残さない"
    );
}

#[tokio::test]
async fn list_filters_by_batch_and_status() {
    let env = env_with_routes();
    let app = env.router();
    let repo = env.dir.path().to_str().expect("utf-8 path").to_string();
    let first = post_batch(&app, "sha-a", &repo).await;
    let second = post_batch(&app, "sha-b", &repo).await;
    let batch_a = first["batch_id"].as_str().expect("batch").to_string();
    let batch_b = second["batch_id"].as_str().expect("batch").to_string();

    // 1 本だけ running にする（dispatcher の claim と同じ store 経路）。
    let running_id = env
        .store
        .eval_trial_claim_next()
        .expect("claim")
        .expect("a queued trial")
        .trial_id;

    let all = send(&app, get(BASE)).await;
    assert_eq!(all.json()["trials"].as_array().expect("t").len(), 4);

    let only_b = send(&app, get(&format!("{BASE}?batch_id={batch_b}"))).await;
    assert_eq!(only_b.json()["trials"].as_array().expect("t").len(), 2);

    let running = send(&app, get(&format!("{BASE}?status=running"))).await;
    let rows = running.json()["trials"].as_array().expect("t").clone();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["trial_id"], running_id.as_str());

    let both = send(
        &app,
        get(&format!("{BASE}?batch_id={batch_a}&status=queued")),
    )
    .await;
    assert_eq!(both.json()["trials"].as_array().expect("t").len(), 1);

    let none = send(
        &app,
        get(&format!("{BASE}?batch_id={batch_a}&status=passed")),
    )
    .await;
    assert_eq!(none.json()["trials"].as_array().expect("t").len(), 0);

    let bad_status = send(&app, get(&format!("{BASE}?status=bogus"))).await;
    assert_problem(&bad_status, 400, "eval_trial_invalid");
    let bad_param = send(&app, get(&format!("{BASE}?nope=1"))).await;
    assert_problem(&bad_param, 400, "bad_request");
}

#[tokio::test]
async fn cancel_moves_a_queued_trial_and_conflicts_afterwards() {
    let env = env_with_routes();
    let app = env.router();
    let repo = env.dir.path().to_str().expect("utf-8 path").to_string();
    let created = post_batch(&app, "sha-cancel", &repo).await;
    let trials = created["trials"].as_array().expect("trials").clone();
    let queued = trials[0]["trial_id"].as_str().expect("id").to_string();
    let other = trials[1]["trial_id"].as_str().expect("id").to_string();

    let resp = send(&app, cancel(&queued)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let view = resp.json();
    assert_eq!(view["trial_id"], queued.as_str());
    assert_eq!(view["status"], "cancelled");
    assert!(view["finished_at"].is_string(), "{view}");
    assert!(!resp.text().contains("hidden-check"), "{}", resp.text());

    // 取り消し済みは 409。
    let again = send(&app, cancel(&queued)).await;
    assert_problem(&again, 409, "eval_trial_not_cancellable");

    // 無い id は 404。
    let missing = send(&app, cancel("01JNOPE00000000000000000")).await;
    assert_problem(&missing, 404, "eval_trial_not_found");

    // running は取り消せる（run を止めるのは dispatcher 側）。
    let claimed = env.store.eval_trial_claim_next().expect("claim");
    assert_eq!(claimed.expect("queued trial").trial_id, other);
    let running = send(&app, cancel(&other)).await;
    assert_eq!(running.status.as_u16(), 200, "{}", running.text());
    assert_eq!(running.json()["status"], "cancelled");

    // 終端（passed）は 409。
    let done = post_batch(&app, "sha-terminal", &repo).await;
    let done_id = done["trials"][0]["trial_id"]
        .as_str()
        .expect("id")
        .to_string();
    let claimed = env.store.eval_trial_claim_next().expect("claim");
    assert_eq!(claimed.expect("queued trial").trial_id, done_id);
    env.store
        .eval_trial_mark_started(
            &done_id,
            "01JRUN000000000000000000",
            "01JDEC000000000000000000",
            "claude 2.0.0",
            "/nonexistent/eval-worktree",
        )
        .expect("mark started");
    env.store
        .eval_trial_mark_scoring(&done_id)
        .expect("scoring");
    env.store
        .eval_trial_finish(&done_id, EvalTrialStatus::Passed, Some(0), false, None)
        .expect("finish");
    let terminal = send(&app, cancel(&done_id)).await;
    assert_problem(&terminal, 409, "eval_trial_not_cancellable");

    // 終端した行は一覧に残る（採点結果を読める）。
    let passed = send(&app, get(&format!("{BASE}?status=passed"))).await;
    let rows = passed.json()["trials"].as_array().expect("t").clone();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["check_exit"], 0);
    assert_eq!(rows[0]["harness_version"], "claude 2.0.0");
    assert_eq!(rows[0]["run_id"], "01JRUN000000000000000000");
    assert!(rows[0]["started_at"].is_string(), "{rows:?}");

    // query は付けない（付いたら 400）。
    let with_query = send(
        &app,
        post_json(&format!("{BASE}/{done_id}/cancel?x=1"), &json!({})),
    )
    .await;
    assert_problem(&with_query, 400, "bad_request");
}

#[tokio::test]
async fn writes_need_the_bearer_token_when_the_daemon_has_one() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        harnesses: Some(std::sync::Arc::new(DeclaredRoutes)),
        ..Default::default()
    });
    let app = env.router();
    let repo = env.dir.path().to_str().expect("utf-8 path").to_string();

    let no_token = send(
        &app,
        post_json(BASE, &body("sha-auth", vec![trial("p1", 1, &repo)])),
    )
    .await;
    assert_problem(&no_token, 401, "unauthorized");
    let cancel_no_token = send(&app, cancel("01JNOPE00000000000000000")).await;
    assert_problem(&cancel_no_token, 401, "unauthorized");

    // bearer があれば通る（既存の書き込み API と同じ規律）。
    let with_token = send(
        &app,
        post_admin(BASE, &body("sha-auth", vec![trial("p1", 1, &repo)])),
    )
    .await;
    assert_eq!(with_token.status.as_u16(), 201, "{}", with_token.text());
    let list = send(&app, get_admin(BASE)).await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    assert_eq!(list.json()["trials"].as_array().expect("t").len(), 1);
}
