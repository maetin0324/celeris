//! アーキv4 D（contract-tests）: org の書き込みの契約を、refactor（`handlers/org.rs` と `cos/ops/admin.rs` に
//! 混ざった業務ロジックを共有 use-case に集める）の前のコードに当てて固定する。書き込みは公開の経路
//! （HTTP `/api/v1/org…` と CoS `/api/v1/cos/operations`）だけで行い、結果は応答と DB の行（rusqlite で読むだけ）
//! で確かめる。org の内部の関数は呼ばない（refactor の後もこの file を変えずに通すため）。tx の途中の失敗は、
//! 試験が DB に置く SQLite の trigger（`contract_fault`）で決定的に起こす。
//!
//! 試験ごとに守る不変条件と、既存の試験（organization.rs・cos_delivery_suite/cos_ops_admin_config.rs・
//! cos_operations.rs）で検出できない理由:
//!
//! (a) `cos_org_writes_commit_row_browser_event_operation_and_audit_together`
//!     CoS の org.create・org.update・org.browser_settings・org.delete が、1 回の要求で org の行・
//!     org_browser_events の行（actor `cos`。browser の grant を変える書き込みだけ）・cos_operations の applied の行・
//!     監査 event（`cos_operation`、state applied、target は org の node）を書く。grant を変えない org.update と
//!     org.create・org.delete は org_browser_events を書かない。操作の行の `expected_revision` は送った値のまま残る。
//!     既存で検出できない理由: `cos_ops_admin_config_org_and_skills_are_audited` は org.create・org.update・org.delete の
//!     applied の行と監査 event を見るが、org.browser_settings の成功と、CoS の書き込みの org_browser_events の行
//!     （actor `cos`）を見ない。`browser_settings_event_records_actor_and_before_after` は HTTP（actor `admin`）だけを見る。
//!
//! (a) `cos_org_write_failing_inside_the_transaction_leaves_the_org_unchanged`
//!     org の行を書いた後の段（cos_operations の applied の行・監査 event・chat の card のどれか）で失敗すると、
//!     org の行も org_browser_events の行も残らず、その操作は rejected の行 1 件と rejected の監査 event 1 件だけに
//!     なる（= org の書き込み・操作の記録・監査が同じ transaction）。trigger を外すと同じ要求は applied になる。
//!     既存で検出できない理由: 既存の試験は成功した後の行と、書く前の検証での拒否しか見ず、org の行を書いた後の
//!     失敗を起こさない。org の行を別の transaction で先に commit する変更はどの既存の試験でも落ちない。
//!     依頼の例の「expected_revision の不一致」と「既に解決した triage item の mark による conflict」は、今のコード
//!     では org の操作に届かない（org.* は expected_revision を記録するだけで比べない。triage item の mark は
//!     `/cos/inbox/{i}/resolve` だけが付け、その回答の要求は decision・authorization・question などの経路で、org の
//!     経路にならない）。そこで同じ段の失敗を trigger で起こす。
//!
//! (a) `http_browser_write_failing_after_the_row_leaves_the_org_unchanged`
//!     HTTP の `PATCH /org/{id}/browser-settings` と、grant を変える `PATCH /org/{id}` は、org の行と
//!     org_browser_events の行（actor `admin`）を同じ transaction で書く。後者の挿入が落ちると前者も残らない。
//!     grant を変えない PATCH は org_browser_events を書かないので、同じ失敗の下でも通る。
//!     既存で検出できない理由: organization.rs の `assert_rejected_unchanged` 系は書く前の検証での拒否だけを見る。
//!
//! (b) `org_refusals_keep_status_code_and_cos_records_the_reason`
//!     不明な genre・不正な browser の origin・無い node・子を持つ node の削除・親の無い PATCH で、HTTP と CoS が
//!     同じ status・problem の code・detail を返し、org の行は変わらない。CoS の rejected の行と監査 event の reason は、
//!     書く前の検証での拒否なら `rejected: <detail>; requested reason: <要求の reason>`（行の action は `rejected`、
//!     作成の target は `-`）、transaction の中の拒否（子を持つ node の削除・親の無い PATCH）なら要求の reason の
//!     ままで、action は org の操作。どちらも行の result の `error` に detail が入る。
//!     既存で検出できない理由: `org_validation_is_reported_as_422_and_duplicate_ids_as_409`・
//!     `deleting_a_node_that_still_has_work_is_a_conflict`・`browser_settings_rejects_*` は HTTP の status と code だけを、
//!     `cos_ops_admin_config_org_and_skills_are_audited` は CoS の status と rejected の件数だけを見る。genre と origin を
//!     CoS に送らず、reason の文・action・target・HTTP との detail の一致を見ない。
//!
//! (c) `org_writes_need_an_admin_principal_and_cos_writes_need_the_cos_run`
//!     HTTP の 4 経路（POST・PATCH・browser-settings・DELETE）は誤った token で 401 `unauthorized`、CoS の run の
//!     credential で直接呼ぶと 422 `cos_audit_context_required`（`instead` は `/api/v1/cos/operations`）。
//!     `/cos/operations` の org の操作は token 無しで 401 `unauthorized`、人（`human:test`・`human:web`）と外部
//!     operator の token で 403 `cos_credential_required`。どれも org の行を変えず、CoS の run の credential なら通る。
//!     既存で十分なので足さないもの: token 無しの HTTP の 401 は `org_admin_endpoints_require_a_token`・
//!     `org_admin_endpoints_require_a_token_when_token_file_is_not_configured`・
//!     `browser_settings_requires_admin_and_same_origin`、admin token の 403 は
//!     `cos_chat_ops_api_body_shape_and_identity_claims_are_rejected`（task の操作）、本文の identity の主張の拒否も同じ試験。
//!     既存で検出できない理由: 誤った token、`human:web` と operator の主体での org の操作、CoS の credential での
//!     browser-settings の直接の呼び出し（common/cos_ops.rs の `run_domain` は org.create・update・delete だけ）を見ない。
//!
//! (d) `org_http_and_cos_responses_keep_their_shape`
//!     HTTP の POST（201・Location・body の key の集合と値）・PATCH（200）・browser-settings（200）・DELETE（204・空の
//!     body）の body が `GET /org` の node と同じであること。CoS の `/cos/operations` の応答の key の集合、operation view
//!     の key の集合と値、result（upsert は `GET /org` の node と同じ、delete は `{id, deleted: true}`）。
//!     既存で検出できない理由: organization.rs は値の一部（Location・name・genre）だけを見て key の集合を見ない。
//!     cos_ops_admin_config は CoS の result の `target_id` と `brief` しか見ない。
use crate::common;

use common::cos_ops::{
    OPS, audit_events, cos_bearer, db, op_body, post_op, run_domain, run_domain_with_revision,
};
use common::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use task_core::GenreSpec;

/// The reason `op_body` sends (and `run_domain` expects on the applied row).
const REASON: &str = "人が依頼した";
const DEPT: &str = "browser-execution";
const DEPT_PATH: &str = "/api/v1/org/browser-execution";
const SETTINGS: &str = "/api/v1/org/browser-execution/browser-settings";
const WEB_TOKEN: &str = "contract-web-token";
const OPERATOR_TOKEN: &str = "contract-operator-token";

/// The admin token env with one genre (`coding`), so `genre` is validated, plus named principals.
fn org_env() -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        extra_tokens: vec![
            task_api::ApiToken::new(task_api::Principal::Human { name: "web".into() }, WEB_TOKEN),
            task_api::ApiToken::new(
                task_api::Principal::Operator {
                    name: "claude-code".into(),
                },
                OPERATOR_TOKEN,
            ),
        ],
        genres: vec![GenreSpec {
            id: "coding".into(),
            description: "コードを書く".into(),
            ..GenreSpec::default()
        }],
        ..Default::default()
    })
}

/// 秘書・browser の grant を持つ部（子 `leaf` を持つ）・子の無い課 `leaf` を HTTP で作る。
async fn seed(app: &axum::Router) {
    for body in [
        json!({"id": "secretary", "name": "秘書", "kind": "secretary"}),
        json!({"id": DEPT, "name": "ブラウザ実行部", "kind": "department", "parent_id": "secretary",
               "profile": {"browser": {"allowed_domains": ["http://localhost:3000"]}}}),
        json!({"id": "leaf", "name": "課", "kind": "section", "parent_id": DEPT, "genre": "coding"}),
    ] {
        let resp = send(app, post_admin("/api/v1/org", &body)).await;
        assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    }
}

/// A mutation request with `headers` (`DELETE` sends no body).
fn write_request(
    method: &str,
    path: &str,
    body: &Value,
    headers: &[(&str, &str)],
) -> axum::http::Request<axum::body::Body> {
    match method {
        "POST" => post_json_with(path, body, headers),
        "PATCH" => patch_json_with(path, body, headers),
        "DELETE" => delete_with(path, headers),
        other => panic!("unsupported org method {other}"),
    }
}

/// Every column of `org_nodes` and every `org_browser_events` row, in order: "the org did not change".
fn org_snapshot(env: &TestEnv) -> (Vec<String>, Vec<String>) {
    let conn = db(env);
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).expect("prepare");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    };
    (
        rows(
            "SELECT json_object('id',id,'parent_id',parent_id,'name',name,'kind',kind,'genre',genre,\
             'brief',brief,'position',position,'profile',profile_json,'created_at',created_at,\
             'updated_at',updated_at) FROM org_nodes ORDER BY id",
        ),
        rows(
            "SELECT json_object('node_id',node_id,'ts',ts,'actor',actor,'before',json(before_json),\
             'after',json(after_json)) FROM org_browser_events ORDER BY rowid",
        ),
    )
}

/// The parsed `org_browser_events` rows.
fn browser_events(env: &TestEnv) -> Vec<Value> {
    org_snapshot(env)
        .1
        .iter()
        .map(|row| serde_json::from_str(row).expect("browser event json"))
        .collect()
}

/// The `cos_operations` rows of `thread`, oldest first, as JSON.
fn thread_ops(env: &TestEnv, thread: &str) -> Vec<Value> {
    let conn = db(env);
    let mut stmt = conn
        .prepare(
            "SELECT json_object('id',id,'idempotency_key',idempotency_key,'state',state,'action',action,\
             'target_kind',target_kind,'target_id',target_id,'reason',reason,\
             'expected_revision',expected_revision,'result',json(result_json)) \
             FROM cos_operations WHERE thread_id=?1 ORDER BY created_at, id",
        )
        .expect("prepare");
    stmt.query_map([thread], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|row| serde_json::from_str(&row.expect("row")).expect("operation json"))
        .collect()
}

/// The `cos_operation` audit events of `thread`, oldest first.
fn thread_audits(env: &TestEnv, thread: &str) -> Vec<Value> {
    let conn = db(env);
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY id")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|raw| serde_json::from_str::<Value>(&raw.expect("row")).expect("event json"))
        .filter(|e| e["type"] == "cos_operation" && e["thread_id"] == thread)
        .collect()
}

fn thread_cards(env: &TestEnv, thread: &str) -> i64 {
    db(env)
        .query_row(
            "SELECT COUNT(*) FROM chat_events WHERE thread_id=?1 AND type='card'",
            [thread],
            |row| row.get(0),
        )
        .expect("cards")
}

fn all_ops(env: &TestEnv) -> i64 {
    count(env, "SELECT COUNT(*) FROM cos_operations")
}

/// The node `id` as `GET /api/v1/org` returns it.
async fn listed_node(app: &axum::Router, id: &str) -> Option<Value> {
    let resp = send(app, get_admin("/api/v1/org")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    resp.json()["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|node| node["id"] == id)
        .cloned()
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"))
        .keys()
        .cloned()
        .collect()
}

fn key_set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// Install (`Some`) or remove (`None`) the test's failure trigger `contract_fault`.
fn set_fault(env: &TestEnv, trigger: Option<&str>) {
    let conn = db(env);
    conn.execute_batch("DROP TRIGGER IF EXISTS contract_fault;")
        .expect("drop trigger");
    if let Some(sql) = trigger {
        conn.execute_batch(sql).expect("create trigger");
    }
}

/// Failure points after the org write inside the CoS operation transaction: the applied operation
/// row, its applied audit event, and its chat card (the rejected record written after the rollback
/// matches none of them).
const CO_FAULTS: &[(&str, &str)] = &[
    (
        "operation-row",
        "CREATE TRIGGER contract_fault BEFORE INSERT ON cos_operations WHEN NEW.state = 'applied' \
         BEGIN SELECT RAISE(ABORT, 'contract fault: operation row'); END;",
    ),
    (
        "audit-event",
        "CREATE TRIGGER contract_fault BEFORE INSERT ON events \
         WHEN json_extract(NEW.json, '$.type') = 'cos_operation' \
         AND json_extract(NEW.json, '$.state') = 'applied' \
         BEGIN SELECT RAISE(ABORT, 'contract fault: audit event'); END;",
    ),
    (
        "chat-card",
        "CREATE TRIGGER contract_fault BEFORE INSERT ON chat_events WHEN NEW.type = 'card' \
         BEGIN SELECT RAISE(ABORT, 'contract fault: chat card'); END;",
    ),
];

#[tokio::test]
async fn cos_org_writes_commit_row_browser_event_operation_and_audit_together() {
    let env = org_env();
    let app = env.router();
    seed(&app).await;
    // Creating a node with a grant writes no browser event (only the PATCH routes do).
    assert!(browser_events(&env).is_empty());

    // org.browser_settings: the row, a `cos` browser event with before/after, the applied row and
    // its audit event (run_domain checks the operation row, the audit event and the chat card).
    let settings = run_domain(
        &env,
        "contract-settings",
        "PATCH",
        SETTINGS,
        json!({"allowed_domains": ["https://billing.example.com"]}),
        "org.browser_settings",
    )
    .await;
    let events = browser_events(&env);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["node_id"], DEPT);
    assert_eq!(events[0]["actor"], "cos");
    let (before, after) = (
        events[0]["before"].to_string(),
        events[0]["after"].to_string(),
    );
    assert!(before.contains("http://localhost:3000"), "{before}");
    assert!(after.contains("https://billing.example.com"), "{after}");
    assert!(!after.contains("http://localhost:3000"), "{after}");
    let node = listed_node(&app, DEPT).await.expect("node");
    assert_eq!(
        node["profile"]["browser"]["allowed_domains"],
        json!(["https://billing.example.com"])
    );

    // org.update that replaces the grant: one more `cos` browser event; the envelope revision is
    // recorded as sent (org has no revision to compare it with).
    let update = run_domain_with_revision(
        &env,
        "contract-update-grant",
        "PATCH",
        DEPT_PATH,
        json!({"profile": {"browser": {"allowed_domains": ["https://docs.example.com"]}}}),
        "org.update",
        Some("rev-7"),
    )
    .await;
    assert_eq!(update["expected_revision"], "rev-7");
    let row: String = db(&env)
        .query_row(
            "SELECT expected_revision FROM cos_operations WHERE id=?1",
            [update["id"].as_str().expect("id")],
            |r| r.get(0),
        )
        .expect("revision");
    assert_eq!(row, "rev-7");
    let events = browser_events(&env);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[1]["actor"], "cos");
    assert!(
        events[1]["after"]
            .to_string()
            .contains("https://docs.example.com")
    );

    // org.update that leaves the grant alone, org.create and org.delete write no browser event.
    let brief = run_domain(
        &env,
        "contract-update-brief",
        "PATCH",
        DEPT_PATH,
        json!({"brief": "ブラウザで作業する"}),
        "org.update",
    )
    .await;
    let create = run_domain(
        &env,
        "contract-create",
        "POST",
        "/api/v1/org",
        json!({"id": "cos-leaf", "name": "CoS の課", "kind": "section", "parent_id": DEPT}),
        "org.create",
    )
    .await;
    assert!(listed_node(&app, "cos-leaf").await.is_some());
    let delete = run_domain(
        &env,
        "contract-delete",
        "DELETE",
        "/api/v1/org/cos-leaf",
        json!(null),
        "org.delete",
    )
    .await;
    assert!(listed_node(&app, "cos-leaf").await.is_none());
    assert_eq!(browser_events(&env).len(), 2);
    assert_eq!(
        listed_node(&app, DEPT).await.expect("node")["brief"],
        "ブラウザで作業する"
    );

    // Each applied operation has exactly one audit event, and it targets the org node it wrote.
    for (op, target) in [
        (&settings, DEPT),
        (&update, DEPT),
        (&brief, DEPT),
        (&create, "cos-leaf"),
        (&delete, "cos-leaf"),
    ] {
        assert_eq!(op["target_kind"], "org", "{op}");
        assert_eq!(op["target_id"], target, "{op}");
        let audits = audit_events(&env, op["id"].as_str().expect("id"));
        assert_eq!(audits.len(), 1, "{audits:?}");
        assert_eq!(audits[0]["target_kind"], "org");
        assert_eq!(audits[0]["target_id"], target);
        assert_eq!(audits[0]["state"], "applied");
        assert_eq!(audits[0]["reason"], REASON);
    }
}

#[tokio::test]
async fn cos_org_write_failing_inside_the_transaction_leaves_the_org_unchanged() {
    let env = org_env();
    let app = env.router();
    seed(&app).await;
    let grant = json!({"allowed_domains": ["https://billing.example.com"]});
    let writes = [
        (
            "POST",
            "/api/v1/org",
            json!({"id": "newcomer", "name": "新しい課", "kind": "section", "parent_id": DEPT}),
            "org.create",
        ),
        (
            "PATCH",
            DEPT_PATH,
            json!({"brief": "変える", "profile": {"browser": grant.clone()}}),
            "org.update",
        ),
        ("PATCH", SETTINGS, grant.clone(), "org.browser_settings"),
        ("DELETE", "/api/v1/org/leaf", json!(null), "org.delete"),
    ];
    for (fault, trigger) in CO_FAULTS {
        for (method, path, body, action) in &writes {
            let label = format!("{fault} {action}");
            let (thread, _, bearer) = cos_bearer(&env, &format!("fault-{fault}-{action}"));
            set_fault(&env, Some(trigger));
            let before = org_snapshot(&env);
            let resp = post_op(&app, &bearer, &op_body("fault", method, path, body.clone())).await;
            assert_problem(&resp, 500, "chat_store_error");
            set_fault(&env, None);

            assert_eq!(org_snapshot(&env), before, "{label}: org changed");
            let ops = thread_ops(&env, &thread);
            assert_eq!(ops.len(), 1, "{label}: {ops:?}");
            assert_eq!(ops[0]["state"], "rejected", "{label}");
            assert_eq!(ops[0]["action"], *action, "{label}");
            assert_eq!(ops[0]["reason"], REASON, "{label}");
            let error = ops[0]["result"]["error"].as_str().unwrap_or_default();
            assert!(error.contains("contract fault"), "{label}: {error}");
            let audits = thread_audits(&env, &thread);
            assert_eq!(audits.len(), 1, "{label}: {audits:?}");
            assert_eq!(audits[0]["state"], "rejected", "{label}");
            assert_eq!(audits[0]["operation_id"], ops[0]["id"], "{label}");
            assert_eq!(thread_cards(&env, &thread), 0, "{label}");
        }
    }

    // Without the fault the same writes apply: the fault refused them, not the request.
    for (method, path, body, action) in &writes {
        let (thread, _, bearer) = cos_bearer(&env, &format!("unfaulted-{action}"));
        let resp = post_op(&app, &bearer, &op_body("ok", method, path, body.clone())).await;
        assert_eq!(resp.status.as_u16(), 200, "{action}: {}", resp.text());
        assert_eq!(resp.json()["operation"]["state"], "applied", "{action}");
        assert_eq!(thread_cards(&env, &thread), 1, "{action}");
    }
    assert!(listed_node(&app, "newcomer").await.is_some());
    assert!(listed_node(&app, "leaf").await.is_none());
    let events = browser_events(&env);
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(events.iter().all(|e| e["actor"] == "cos"), "{events:?}");
}

#[tokio::test]
async fn http_browser_write_failing_after_the_row_leaves_the_org_unchanged() {
    let env = org_env();
    let app = env.router();
    seed(&app).await;
    // Two different grants, so each write changes the grant even after the other one applied.
    let writes = [
        (
            SETTINGS,
            json!({"allowed_domains": ["https://billing.example.com"]}),
        ),
        (
            DEPT_PATH,
            json!({"profile": {"browser": {"allowed_domains": ["https://docs.example.com"]}}}),
        ),
    ];
    set_fault(
        &env,
        Some(
            "CREATE TRIGGER contract_fault BEFORE INSERT ON org_browser_events \
             BEGIN SELECT RAISE(ABORT, 'contract fault: browser event'); END;",
        ),
    );
    let before = org_snapshot(&env);
    for (path, body) in &writes {
        let resp = send(&app, patch_admin(path, body)).await;
        assert_problem(&resp, 500, "internal");
        assert_eq!(org_snapshot(&env), before, "{path}: org changed");
    }
    // A patch that keeps the grant writes no browser event, so the same fault does not stop it.
    let resp = send(&app, patch_admin(DEPT_PATH, &json!({"brief": "変える"}))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["brief"], "変える");
    set_fault(&env, None);

    for (path, body) in &writes {
        let resp = send(&app, patch_admin(path, body)).await;
        assert_eq!(resp.status.as_u16(), 200, "{path}: {}", resp.text());
    }
    let events = browser_events(&env);
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(events.iter().all(|e| e["actor"] == "admin"), "{events:?}");
}

/// One refused org write, sent once over HTTP (admin) and once through `/cos/operations`.
struct Refusal {
    key: &'static str,
    method: &'static str,
    path: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
    /// Refused inside the operation transaction (else before it, by the request's validation).
    in_tx: bool,
    /// The `cos_operations` row's `(action, target_id)`.
    recorded: (&'static str, &'static str),
}

#[tokio::test]
async fn org_refusals_keep_status_code_and_cos_records_the_reason() {
    let env = org_env();
    let app = env.router();
    seed(&app).await;
    let valid = json!({"allowed_domains": ["https://billing.example.com"]});
    let cases = [
        Refusal {
            key: "genre-create",
            method: "POST",
            path: "/api/v1/org",
            body: json!({"id": "ghost", "name": "?", "kind": "section", "parent_id": DEPT, "genre": "bogus"}),
            status: 422,
            code: "validation",
            in_tx: false,
            recorded: ("rejected", "-"),
        },
        Refusal {
            key: "genre-patch",
            method: "PATCH",
            path: "/api/v1/org/leaf",
            body: json!({"genre": "bogus"}),
            status: 422,
            code: "validation",
            in_tx: false,
            recorded: ("rejected", "leaf"),
        },
        Refusal {
            key: "origin-settings",
            method: "PATCH",
            path: SETTINGS,
            body: json!({"allowed_domains": ["https://billing.example.com/path"]}),
            status: 422,
            code: "validation",
            in_tx: false,
            recorded: ("rejected", DEPT),
        },
        Refusal {
            key: "origin-patch",
            method: "PATCH",
            path: DEPT_PATH,
            body: json!({"profile": {"browser": {"allowed_domains": ["ftp://billing.example.com"]}}}),
            status: 422,
            code: "validation",
            in_tx: false,
            recorded: ("rejected", DEPT),
        },
        Refusal {
            key: "missing-patch",
            method: "PATCH",
            path: "/api/v1/org/nope",
            body: json!({"brief": "x"}),
            status: 404,
            code: "org_node_not_found",
            in_tx: false,
            recorded: ("rejected", "nope"),
        },
        Refusal {
            key: "missing-settings",
            method: "PATCH",
            path: "/api/v1/org/nope/browser-settings",
            body: valid.clone(),
            status: 404,
            code: "org_node_not_found",
            in_tx: false,
            recorded: ("rejected", "nope"),
        },
        Refusal {
            key: "missing-delete",
            method: "DELETE",
            path: "/api/v1/org/nope",
            body: json!(null),
            status: 404,
            code: "org_node_not_found",
            in_tx: false,
            recorded: ("rejected", "nope"),
        },
        Refusal {
            key: "children-delete",
            method: "DELETE",
            path: DEPT_PATH,
            body: json!(null),
            status: 409,
            code: "org_node_in_use",
            in_tx: true,
            recorded: ("org.delete", DEPT),
        },
        Refusal {
            key: "tree-patch",
            method: "PATCH",
            path: "/api/v1/org/leaf",
            body: json!({"parent_id": "nobody"}),
            status: 422,
            code: "validation",
            in_tx: true,
            recorded: ("org.update", "leaf"),
        },
    ];
    let (thread, _, bearer) = cos_bearer(&env, "contract-refusals");
    let before = org_snapshot(&env);
    for case in &cases {
        let key = case.key;
        let http = send(
            &app,
            write_request(case.method, case.path, &case.body, &admin_headers()),
        )
        .await;
        let detail = assert_problem(&http, case.status, case.code)["detail"]
            .as_str()
            .expect("detail")
            .to_string();
        assert_eq!(org_snapshot(&env), before, "{key}: HTTP changed the org");

        let cos = post_op(
            &app,
            &bearer,
            &op_body(key, case.method, case.path, case.body.clone()),
        )
        .await;
        let problem = assert_problem(&cos, case.status, case.code);
        assert_eq!(problem["detail"], detail.as_str(), "{key}: CoS detail");
        assert_eq!(org_snapshot(&env), before, "{key}: CoS changed the org");

        let ops = thread_ops(&env, &thread);
        let row = ops
            .iter()
            .find(|op| op["idempotency_key"] == key)
            .unwrap_or_else(|| panic!("{key}: no operation row in {ops:?}"));
        assert_eq!(row["state"], "rejected", "{key}");
        assert_eq!(row["target_kind"], "org", "{key}");
        assert_eq!(
            (row["action"].as_str(), row["target_id"].as_str()),
            (Some(case.recorded.0), Some(case.recorded.1)),
            "{key}"
        );
        let reason = if case.in_tx {
            REASON.to_string()
        } else {
            format!("rejected: {detail}; requested reason: {REASON}")
        };
        assert_eq!(row["reason"], reason.as_str(), "{key}");
        let error = row["result"]["error"].as_str().unwrap_or_default();
        assert!(error.contains(&detail), "{key}: {error} lacks {detail}");
        let audits = audit_events(&env, row["id"].as_str().expect("id"));
        assert_eq!(audits.len(), 1, "{key}: {audits:?}");
        assert_eq!(audits[0]["state"], "rejected", "{key}");
        assert_eq!(audits[0]["reason"], reason.as_str(), "{key}");
        assert_eq!(audits[0]["actor"], "cos", "{key}");
    }
    assert_eq!(thread_ops(&env, &thread).len(), cases.len());
    assert_eq!(thread_cards(&env, &thread), 0);
}

#[tokio::test]
async fn org_writes_need_an_admin_principal_and_cos_writes_need_the_cos_run() {
    let env = org_env();
    let app = env.router();
    seed(&app).await;
    let writes = [
        (
            "POST",
            "/api/v1/org",
            json!({"id": "intruder", "name": "x", "kind": "section", "parent_id": DEPT}),
        ),
        ("PATCH", "/api/v1/org/leaf", json!({"brief": "x"})),
        (
            "PATCH",
            SETTINGS,
            json!({"allowed_domains": ["https://billing.example.com"]}),
        ),
        ("DELETE", "/api/v1/org/leaf", json!(null)),
    ];
    let before = org_snapshot(&env);

    // HTTP: a wrong bearer is unauthenticated.
    for (method, path, body) in &writes {
        let resp = send(
            &app,
            write_request(
                method,
                path,
                body,
                &[("authorization", "Bearer not-the-token")],
            ),
        )
        .await;
        assert_problem(&resp, 401, "unauthorized");
    }
    assert_eq!(org_snapshot(&env), before);
    assert_eq!(all_ops(&env), 0);

    // HTTP with a CoS run credential: refused with the envelope to use instead, recorded rejected.
    let (thread, _, bearer) = cos_bearer(&env, "contract-direct");
    for (method, path, body) in &writes {
        let resp = send(
            &app,
            write_request(method, path, body, &[("authorization", bearer.as_str())]),
        )
        .await;
        let problem = assert_problem(&resp, 422, "cos_audit_context_required");
        assert_eq!(problem["instead"], OPS, "{method} {path}");
    }
    assert_eq!(org_snapshot(&env), before);
    let ops = thread_ops(&env, &thread);
    assert_eq!(ops.len(), writes.len(), "{ops:?}");
    assert!(ops.iter().all(|op| op["state"] == "rejected"), "{ops:?}");

    // `/cos/operations`: no credential is 401, people and the external operator are 403.
    let envelope = op_body(
        "contract-principal",
        "PATCH",
        "/api/v1/org/leaf",
        json!({"brief": "x"}),
    );
    let recorded = all_ops(&env);
    let resp = send(&app, post_json_with(OPS, &envelope, &[])).await;
    assert_problem(&resp, 401, "unauthorized");
    for token in [TOKEN, WEB_TOKEN, OPERATOR_TOKEN] {
        let bearer = format!("Bearer {token}");
        let resp = post_op(&app, &bearer, &envelope).await;
        assert_problem(&resp, 403, "cos_credential_required");
    }
    assert_eq!(org_snapshot(&env), before);
    assert_eq!(all_ops(&env), recorded);

    // The CoS run's own credential applies the same envelope.
    let (_, _, bearer) = cos_bearer(&env, "contract-principal-ok");
    let resp = post_op(&app, &bearer, &envelope).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["operation"]["state"], "applied");
    assert_eq!(listed_node(&app, "leaf").await.expect("leaf")["brief"], "x");
}

#[tokio::test]
async fn org_http_and_cos_responses_keep_their_shape() {
    let env = org_env();
    let app = env.router();
    let base = [
        "brief",
        "created_at",
        "id",
        "kind",
        "name",
        "parent_id",
        "position",
        "updated_at",
    ];
    let with = |extra: &[&str]| {
        let mut set = key_set(&base);
        set.extend(extra.iter().map(|k| k.to_string()));
        set
    };

    // POST: 201, Location, the node (empty profile and absent genre are omitted, parent_id is null).
    let resp = send(
        &app,
        post_admin(
            "/api/v1/org",
            &json!({"id": "secretary", "name": "秘書", "kind": "secretary"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    assert_eq!(resp.header("location"), Some("/api/v1/org/secretary"));
    assert_eq!(
        resp.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    let root = resp.json();
    assert_eq!(keys(&root), key_set(&base), "{root}");
    assert_eq!(root["parent_id"], Value::Null);
    assert_eq!(root["kind"], "secretary");
    assert_eq!(root["brief"], "");
    assert_eq!(root["position"], 0);
    assert_eq!(root["created_at"], root["updated_at"]);
    assert_eq!(listed_node(&app, "secretary").await, Some(root));

    let resp = send(
        &app,
        post_admin(
            "/api/v1/org",
            &json!({"id": DEPT, "name": "ブラウザ実行部", "kind": "department", "parent_id": "secretary",
                    "brief": "ブラウザで作業する", "position": 2,
                    "profile": {"browser": {"allowed_domains": ["http://localhost:3000"]}}}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let dept = resp.json();
    assert_eq!(keys(&dept), with(&["profile"]), "{dept}");
    assert_eq!(
        (&dept["brief"], &dept["position"], &dept["parent_id"]),
        (&json!("ブラウザで作業する"), &json!(2), &json!("secretary"))
    );
    let resp = send(
        &app,
        post_admin(
            "/api/v1/org",
            &json!({"id": "leaf", "name": "課", "kind": "section", "parent_id": DEPT, "genre": "coding"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let leaf = resp.json();
    assert_eq!(keys(&leaf), with(&["genre"]), "{leaf}");

    // PATCH: 200 with the node; created_at is kept, omitted fields keep their values.
    let resp = send(
        &app,
        patch_admin("/api/v1/org/leaf", &json!({"name": "改名した課"})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let patched = resp.json();
    assert_eq!(keys(&patched), keys(&leaf), "{patched}");
    assert_eq!(patched["name"], "改名した課");
    assert_eq!(patched["genre"], "coding");
    assert_eq!(patched["created_at"], leaf["created_at"]);
    assert_eq!(listed_node(&app, "leaf").await, Some(patched));

    // browser-settings: 200 with the node; only the grant changed.
    let resp = send(
        &app,
        patch_admin(
            SETTINGS,
            &json!({"allowed_domains": ["https://billing.example.com"]}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let settings = resp.json();
    assert_eq!(keys(&settings), keys(&dept), "{settings}");
    assert_eq!(
        settings["profile"]["browser"]["allowed_domains"],
        json!(["https://billing.example.com"])
    );
    for field in ["name", "brief", "position", "parent_id", "created_at"] {
        assert_eq!(settings[field], dept[field], "{field}");
    }
    assert_eq!(listed_node(&app, DEPT).await, Some(settings));

    // DELETE: 204 with an empty body.
    let resp = send(&app, delete_admin("/api/v1/org/leaf")).await;
    assert_eq!(resp.status.as_u16(), 204, "{}", resp.text());
    assert!(resp.body.is_empty(), "{}", resp.text());
    assert!(listed_node(&app, "leaf").await.is_none());

    // CoS: the operation view and its result (the node for upserts, `{id, deleted}` for a delete).
    let view = key_set(&[
        "action",
        "actor",
        "event_id",
        "expected_revision",
        "id",
        "idempotency_key",
        "payload",
        "policy_version",
        "reason",
        "request_hash",
        "result",
        "run_id",
        "state",
        "target_id",
        "target_kind",
        "thread_id",
    ]);
    let create_body =
        json!({"id": "cos-leaf", "name": "CoS の課", "kind": "section", "parent_id": DEPT});
    let created = run_domain(
        &env,
        "shape-create",
        "POST",
        "/api/v1/org",
        create_body.clone(),
        "org.create",
    )
    .await;
    assert_eq!(keys(&created), view, "{created}");
    assert_eq!(
        (&created["target_kind"], &created["target_id"]),
        (&json!("org"), &json!("cos-leaf"))
    );
    assert_eq!(created["expected_revision"], Value::Null);
    assert_eq!(created["idempotency_key"], "shape-create");
    assert_eq!(created["policy_version"], "1");
    assert_eq!(created["reason"], REASON);
    assert!(created["event_id"].is_string(), "{created}");
    assert!(created["request_hash"].is_string(), "{created}");
    assert_eq!(
        created["payload"],
        op_body("shape-create", "POST", "/api/v1/org", create_body)["request"]
    );
    assert_eq!(keys(&created["result"]), key_set(&base), "{created}");
    assert_eq!(
        Some(created["result"].clone()),
        listed_node(&app, "cos-leaf").await
    );

    let updated = run_domain(
        &env,
        "shape-update",
        "PATCH",
        "/api/v1/org/cos-leaf",
        json!({"brief": "CoS が書いた"}),
        "org.update",
    )
    .await;
    assert_eq!(keys(&updated), view);
    assert_eq!(updated["result"]["brief"], "CoS が書いた");
    assert_eq!(
        updated["result"]["created_at"],
        created["result"]["created_at"]
    );
    assert_eq!(
        Some(updated["result"].clone()),
        listed_node(&app, "cos-leaf").await
    );

    let browser = run_domain(
        &env,
        "shape-settings",
        "PATCH",
        SETTINGS,
        json!({"allowed_domains": ["https://docs.example.com"]}),
        "org.browser_settings",
    )
    .await;
    assert_eq!(keys(&browser), view);
    assert_eq!(keys(&browser["result"]), with(&["profile"]));
    assert_eq!(
        Some(browser["result"].clone()),
        listed_node(&app, DEPT).await
    );

    // The delete goes through post_op to see the response envelope too.
    let (_, _, bearer) = cos_bearer(&env, "shape-delete");
    let resp = post_op(
        &app,
        &bearer,
        &op_body(
            "shape-delete",
            "DELETE",
            "/api/v1/org/cos-leaf",
            json!(null),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(
        resp.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    let envelope = resp.json();
    assert_eq!(keys(&envelope), key_set(&["operation"]), "{envelope}");
    let deleted = &envelope["operation"];
    assert_eq!(keys(deleted), view);
    assert_eq!(deleted["state"], "applied");
    assert_eq!(deleted["action"], "org.delete");
    assert_eq!(
        deleted["result"],
        json!({"id": "cos-leaf", "deleted": true})
    );
    assert!(listed_node(&app, "cos-leaf").await.is_none());
}
