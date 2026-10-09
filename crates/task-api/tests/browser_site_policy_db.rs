//! ADR 2026-10-08-browser-prod-enablement D3: site policy の DB 正本と API、grant の credential 設定。
//!
//! 見るもの: API で追加・編集・削除した site policy が、daemon の broker control（同じ DB を別接続で読む
//! `StoreSitePolicies`。daemon と同じ組み方）の手動登録の判定に**作り直しなしで**次の呼び出しから効くこと。
//! 形式検証（422）、参照中の削除（409）、config の種（DB が勝つ）、grant の `credential_use` と
//! `credential_policy_ids` の実在検査。

mod common;
use std::sync::Arc;

use axum::Router;
use common::*;
use serde_json::{Value, json};
use task_api::browser::{SitePolicies, StoreSitePolicies, UnixCredentialBrokerControl};
use task_core::{BrowserSitePolicy, BrowserSitePolicySource, SqliteStore, TaskStore};

const BASE: &str = "/api/v1/browser/site-policies";
const ORIGIN: &str = "https://manaba.example";

fn auth() -> String {
    format!("Bearer {TOKEN}")
}

fn put(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    put_json_with(path, body, &[("authorization", auth().as_str())])
}

fn del(path: &str) -> axum::http::Request<axum::body::Body> {
    delete_with(path, &[("authorization", auth().as_str())])
}

fn get(path: &str) -> axum::http::Request<axum::body::Body> {
    get_with(path, &[("authorization", auth().as_str())])
}

fn post(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    post_json_with(path, body, &[("authorization", auth().as_str())])
}

fn patch(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    patch_json_with(path, body, &[("authorization", auth().as_str())])
}

fn policy_body(login_path: &str, selector: &str) -> Value {
    json!({
        "exact_origin": ORIGIN,
        "login_url": format!("{ORIGIN}{login_path}"),
        "password_selector": selector,
        "submit_selector": "#submit",
    })
}

/// daemon と同じ組み方の broker control（API とは別の接続で同じ DB を読む）。socket には繋がない。
fn broker(env: &TestEnv) -> UnixCredentialBrokerControl {
    let store = SqliteStore::open(&env.db_path).expect("open store");
    UnixCredentialBrokerControl {
        socket: env.dir.path().join("no-credentiald.sock"),
        site_policies: SitePolicies(Arc::new(StoreSitePolicies(Arc::new(store)))),
    }
}

fn fixture() -> (TestEnv, Router) {
    let env = admin_env();
    let app = task_api::router(env.state.clone());
    (env, app)
}

#[tokio::test]
async fn browser_site_policy_db_api_changes_reach_broker_without_restart() {
    let (env, app) = fixture();
    let broker = broker(&env);
    // 未登録: selector 無しの policy（broker は注入を拒否する）。
    let before = broker
        .credential_policy("task-1", "manaba", ORIGIN)
        .expect("lookup");
    assert_eq!(before.login_url, None);
    assert_eq!(before.password_selector, None);

    // 追加: 次の判定から login_url・selector が入る。
    let created = send(
        &app,
        put(
            &format!("{BASE}/manaba"),
            &policy_body("/login", "#password"),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    assert_eq!(created.json()["created"], true);
    assert_eq!(created.json()["policy"]["source"], "api");
    let added = broker
        .credential_policy("task-1", "manaba", ORIGIN)
        .expect("lookup");
    assert_eq!(
        added.login_url.as_deref(),
        Some("https://manaba.example/login")
    );
    assert_eq!(added.password_selector.as_deref(), Some("#password"));
    assert_eq!(added.submit_selector.as_deref(), Some("#submit"));
    assert!(
        added.require_approval,
        "credential use stays approved per use"
    );
    // origin が違えば当たらない（1 policy = 1 origin）。
    let other = broker
        .credential_policy("task-1", "manaba", "https://other.example")
        .expect("lookup");
    assert_eq!(other.login_url, None);

    // 編集: 同じ broker（作り直しなし）が新しい値を返す。
    let replaced = send(
        &app,
        put(
            &format!("{BASE}/manaba"),
            &policy_body("/ct/login", "input[name=pw]"),
        ),
    )
    .await;
    assert_eq!(replaced.status.as_u16(), 200, "{}", replaced.text());
    assert_eq!(replaced.json()["created"], false);
    let edited = broker
        .credential_policy("task-1", "manaba", ORIGIN)
        .expect("lookup");
    assert_eq!(
        edited.login_url.as_deref(),
        Some("https://manaba.example/ct/login")
    );
    assert_eq!(edited.password_selector.as_deref(), Some("input[name=pw]"));

    // 削除: 次の判定から selector 無しに戻る。
    let deleted = send(&app, del(&format!("{BASE}/manaba"))).await;
    assert_eq!(deleted.status.as_u16(), 204, "{}", deleted.text());
    let gone = broker
        .credential_policy("task-1", "manaba", ORIGIN)
        .expect("lookup");
    assert_eq!(gone.login_url, None);
    assert_eq!(gone.password_selector, None);

    // 監査は追記専用の流れに op・source・actor だけ（URL・selector は載せない）。
    let events = env.store.browser_site_policy_events().expect("events");
    let ops: Vec<_> = events
        .iter()
        .map(|(id, op, src, actor)| (id.as_str(), op.as_str(), src.as_str(), actor.as_str()))
        .collect();
    assert_eq!(
        ops,
        vec![
            ("manaba", "upsert", "api", "admin"),
            ("manaba", "upsert", "api", "admin"),
            ("manaba", "delete", "api", "admin"),
        ]
    );
}

#[tokio::test]
async fn browser_site_policy_db_put_validates_and_lists() {
    let (_env, app) = fixture();
    // ADR-0110 D2 の形式検証: login URL が origin の外。
    let bad = send(
        &app,
        put(
            &format!("{BASE}/manaba"),
            &json!({"exact_origin": ORIGIN, "login_url": "https://evil.example/login", "password_selector": "#p"}),
        ),
    )
    .await;
    let problem = assert_problem(&bad, 422, "site_policy_invalid");
    assert!(problem["reason"].is_string(), "{problem}");
    // policy_id の形。
    let bad_id = send(
        &app,
        put(&format!("{BASE}/bad%20id"), &policy_body("/login", "#p")),
    )
    .await;
    assert_problem(&bad_id, 422, "site_policy_invalid");
    // 知らない欄。
    let unknown = send(
        &app,
        put(
            &format!("{BASE}/manaba"),
            &json!({"exact_origin": ORIGIN, "login_url": format!("{ORIGIN}/login"), "password_selector": "#p", "bogus": 1}),
        ),
    )
    .await;
    assert_problem(&unknown, 422, "site_policy_invalid");
    // admin token が要る。
    let anon = send(
        &app,
        put_json_with(&format!("{BASE}/manaba"), &policy_body("/login", "#p"), &[]),
    )
    .await;
    assert_eq!(anon.status.as_u16(), 401);

    for id in ["zeta", "alpha"] {
        let r = send(
            &app,
            put(&format!("{BASE}/{id}"), &policy_body("/login", "#p")),
        )
        .await;
        assert_eq!(r.status.as_u16(), 201, "{}", r.text());
    }
    let list = send(&app, get(BASE)).await;
    assert_eq!(list.status.as_u16(), 200);
    let ids: Vec<_> = list.json()["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["policy_id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ids, vec!["alpha", "zeta"]);
    assert_eq!(
        list.json()["items"][0]["login_url"],
        "https://manaba.example/login"
    );
    // 無い policy の削除は 404。
    let missing = send(&app, del(&format!("{BASE}/nope"))).await;
    assert_problem(&missing, 404, "site_policy_not_found");
}

async fn seed_browser_node(app: &Router, policy_ids: Value) {
    let r = send(
        app,
        post(
            "/api/v1/org",
            &json!({"id":"secretary", "name":"Secretary", "kind":"secretary"}),
        ),
    )
    .await;
    assert_eq!(r.status.as_u16(), 201, "{}", r.text());
    let r = send(
        app,
        post(
            "/api/v1/org",
            &json!({"id":"browser-execution", "name":"Browser", "kind":"department", "parent_id":"secretary",
                    "profile":{"browser":{"allowed_domains":[ORIGIN], "credential_policy_ids": policy_ids}}}),
        ),
    )
    .await;
    assert_eq!(r.status.as_u16(), 201, "{}", r.text());
}

const SETTINGS: &str = "/api/v1/org/browser-execution/browser-settings";

#[tokio::test]
async fn browser_site_policy_db_delete_in_use_by_grant_is_conflict() {
    let (env, app) = fixture();
    let r = send(
        &app,
        put(&format!("{BASE}/manaba"), &policy_body("/login", "#p")),
    )
    .await;
    assert_eq!(r.status.as_u16(), 201);
    seed_browser_node(&app, json!(["manaba"])).await;
    let conflict = send(&app, del(&format!("{BASE}/manaba"))).await;
    let problem = assert_problem(&conflict, 409, "site_policy_in_use");
    assert_eq!(problem["node_ids"], json!(["browser-execution"]));
    assert_eq!(problem["wait_ids"], json!([]));
    assert!(
        env.store
            .browser_site_policy_get("manaba")
            .expect("get")
            .is_some()
    );
    // grant から外せば消せる。
    let r = send(&app, patch(SETTINGS, &json!({"credential_policy_ids": []}))).await;
    assert_eq!(r.status.as_u16(), 200, "{}", r.text());
    let r = send(&app, del(&format!("{BASE}/manaba"))).await;
    assert_eq!(r.status.as_u16(), 204, "{}", r.text());
}

#[tokio::test]
async fn browser_site_policy_db_grant_credential_settings() {
    let (env, app) = fixture();
    seed_browser_node(&app, json!([])).await;
    // 実在しない site policy は grant に入れない。
    let unknown = send(
        &app,
        patch(SETTINGS, &json!({"credential_policy_ids": ["manaba"]})),
    )
    .await;
    let problem = assert_problem(&unknown, 422, "unknown_site_policy");
    assert_eq!(problem["policy_ids"], json!(["manaba"]));
    let r = send(
        &app,
        put(&format!("{BASE}/manaba"), &policy_body("/login", "#p")),
    )
    .await;
    assert_eq!(r.status.as_u16(), 201);

    // credential_use: true は Phase 1 の集合を実体化して credential_use を加える。
    let on = send(
        &app,
        patch(
            SETTINGS,
            &json!({"credential_policy_ids": ["manaba"], "credential_use": true}),
        ),
    )
    .await;
    assert_eq!(on.status.as_u16(), 200, "{}", on.text());
    let browser = &on.json()["profile"]["browser"];
    assert_eq!(browser["credential_policy_ids"], json!(["manaba"]));
    let actions: Vec<String> =
        serde_json::from_value(browser["allowed_actions"].clone()).expect("actions");
    let mut expected: Vec<String> = task_core::BrowserAction::PHASE1
        .iter()
        .map(|a| {
            serde_json::to_value(a)
                .expect("action")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    expected.push("credential_use".into());
    assert_eq!(actions, expected);
    // 2 回目の true は重複させない。
    let again = send(&app, patch(SETTINGS, &json!({"credential_use": true}))).await;
    assert_eq!(
        again.json()["profile"]["browser"]["allowed_actions"],
        browser["allowed_actions"]
    );

    // false は credential_use だけを外す。
    let off = send(&app, patch(SETTINGS, &json!({"credential_use": false}))).await;
    assert_eq!(off.status.as_u16(), 200, "{}", off.text());
    let actions: Vec<String> =
        serde_json::from_value(off.json()["profile"]["browser"]["allowed_actions"].clone())
            .expect("actions");
    assert!(!actions.iter().any(|a| a == "credential_use"));
    assert_eq!(actions.len(), task_core::BrowserAction::PHASE1.len());

    // 保存された grant も同じ（DB が正本）。
    let node = env
        .store
        .org_get("browser-execution")
        .expect("org")
        .expect("node");
    let stored = node
        .profile
        .browser
        .expect("grant")
        .allowed_actions
        .expect("actions");
    assert!(!stored.contains(&task_core::BrowserAction::CredentialUse));
}

#[tokio::test]
async fn browser_site_policy_db_config_seed_only_fills_missing() {
    let (env, app) = fixture();
    let r = send(
        &app,
        put(
            &format!("{BASE}/manaba"),
            &policy_body("/api-login", "#api"),
        ),
    )
    .await;
    assert_eq!(r.status.as_u16(), 201);
    let seeds = vec![
        BrowserSitePolicy {
            policy_id: "manaba".into(),
            exact_origin: ORIGIN.into(),
            login_url: format!("{ORIGIN}/config-login"),
            password_selector: "#config".into(),
            submit_selector: None,
            username_selector: None,
            post_login: None,
        },
        BrowserSitePolicy {
            policy_id: "portal".into(),
            exact_origin: "https://portal.example".into(),
            login_url: "https://portal.example/login".into(),
            password_selector: "#pw".into(),
            submit_selector: None,
            username_selector: None,
            post_login: None,
        },
    ];
    let inserted = env
        .store
        .browser_site_policy_seed(&seeds, time::OffsetDateTime::now_utc())
        .expect("seed");
    assert_eq!(inserted, vec!["portal".to_string()]);
    // DB が勝つ（API で書いた manaba は書き換えない）。
    let manaba = env
        .store
        .browser_site_policy_get("manaba")
        .expect("get")
        .expect("row");
    assert_eq!(manaba.policy.login_url, "https://manaba.example/api-login");
    assert_eq!(manaba.source, BrowserSitePolicySource::Api);
    let portal = env
        .store
        .browser_site_policy_get("portal")
        .expect("get")
        .expect("row");
    assert_eq!(portal.source, BrowserSitePolicySource::Config);
    // 2 回目の起動では何も入れない。
    let again = env
        .store
        .browser_site_policy_seed(&seeds, time::OffsetDateTime::now_utc())
        .expect("seed");
    assert!(again.is_empty());
    // 種で入れた policy も API で一覧・判定に出る。
    let list = send(&app, get(BASE)).await;
    assert_eq!(list.json()["items"][1]["source"], "config");
    let decided = broker(&env)
        .credential_policy("task-1", "portal", "https://portal.example")
        .expect("lookup");
    assert_eq!(
        decided.login_url.as_deref(),
        Some("https://portal.example/login")
    );
}
