//! `POST /tasks` の variant 付き campaign 識別: org 照合と保存が同じ transaction で行われ、
//! 拒否時に task / events / 冪等キー / 識別が何も残らない。fake なし・LLM なし（実 store）。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::org::{OrgKind, OrgNode};
use task_core::{TaskStore, profile};

const H1: &str = "b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44";

fn p(body: &Value) -> axum::http::Request<axum::body::Body> {
    post_json_with(
        "/api/v1/tasks",
        body,
        &[("authorization", format!("Bearer {TOKEN}").as_str())],
    )
}

fn node(id: &str, parent: Option<&str>, skills: &[&str]) -> OrgNode {
    OrgNode {
        id: id.into(),
        parent_id: parent.map(Into::into),
        name: id.into(),
        kind: if parent.is_none() {
            OrgKind::Secretary
        } else {
            OrgKind::Section
        },
        genre: None,
        brief: String::new(),
        profile: profile::Profile {
            skills: skills.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
        position: 0,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

fn org(skill: &str) -> Vec<OrgNode> {
    vec![
        node("secretary", None, &[]),
        node("coding", Some("secretary"), &[skill]),
    ]
}

fn pin(nodes: &[OrgNode]) -> Value {
    json!({
        "nodes": nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
        "coding_node": "coding",
        "effective_profile_sha256":
            task_core::variant_wire::effective_profile_sha256(&profile::resolve(nodes, "coding"))
                .unwrap(),
    })
}

fn identity(content: &str, pin: Value) -> Value {
    json!({"campaign_id": "c1", "case_id": "case-a", "arm": "B", "repetition": 1,
           "manifest_hash": "m1", "routing_setting": "fixed",
           "variant": {"comparison_id": "M1", "id": "v1",
                       "content_sha256": content, "selection_sha256": H1},
           "variant_org": pin})
}

fn body(campaign: Value) -> Value {
    json!({"title": "trial", "objective": "o",
           "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
           "campaign": campaign, "campaign_idempotency_key": "campaign-k1"})
}

fn counts(env: &TestEnv) -> (usize, i64, i64) {
    let tasks = env.store.list(None).unwrap().len();
    let conn = rusqlite::Connection::open(&env.db_path).unwrap();
    let q = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    (
        tasks,
        q("SELECT COUNT(*) FROM events"),
        q("SELECT COUNT(*) FROM task_campaigns"),
    )
}

fn env_with(nodes: &[OrgNode]) -> TestEnv {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    });
    env.store.replace_org_nodes(nodes).unwrap();
    env
}

#[tokio::test]
async fn variant_post_resend_and_org_drift() {
    let nodes = org("x");
    let env = env_with(&nodes);
    let app = env.router();
    let good = body(identity(H1, pin(&nodes)));

    let first = send(&app, p(&good)).await;
    assert_eq!(first.status.as_u16(), 201, "{}", first.text());
    let id = first.json()["id"].as_str().unwrap().to_string();
    let own = env.store.task_campaign_own(&id).unwrap().unwrap();
    assert!(own.variant.is_some() && own.variant_org.is_some());
    let after_first = counts(&env);

    // GET は保存した識別（variant・構成 pin 込み）を TaskDetail.campaign で返す。
    let detail = send(
        &app,
        get_with(
            &format!("/api/v1/tasks/{id}"),
            &[("authorization", format!("Bearer {TOKEN}").as_str())],
        ),
    )
    .await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    assert_eq!(detail.json()["campaign"]["variant"]["id"], json!("v1"));
    assert_eq!(
        detail.json()["campaign"]["variant_org"],
        good["campaign"]["variant_org"]
    );

    // 同一再送は 200 で新規作成なし。
    let again = send(&app, p(&good)).await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["id"], json!(id));
    assert_eq!(counts(&env), after_first);

    // org が drift したら、同一の冪等照会でも成功にしない。
    env.store.replace_org_nodes(&org("y")).unwrap();
    let drifted = send(&app, p(&good)).await;
    assert_eq!(drifted.status.as_u16(), 409, "{}", drifted.text());
    assert_eq!(drifted.json()["code"], json!("campaign_org_drift"));
    assert_eq!(counts(&env), after_first);
}

#[tokio::test]
async fn org_drift_on_new_post_leaves_nothing() {
    let nodes = org("x");
    let env = env_with(&nodes);
    let app = env.router();
    let before = counts(&env);
    // pin は skill x、現 org は y。
    env.store.replace_org_nodes(&org("y")).unwrap();
    let before_drift = counts(&env);
    let resp = send(&app, p(&body(identity(H1, pin(&nodes))))).await;
    assert_eq!(resp.status.as_u16(), 409, "{}", resp.text());
    assert_eq!(counts(&env), before_drift);
    let _ = before;
    // 脱落 / 余分 node も拒否。
    let missing = json!({"nodes": ["secretary"], "coding_node": "secretary",
        "effective_profile_sha256": H1});
    let resp = send(&app, p(&body(identity(H1, missing)))).await;
    assert!(
        (400..500).contains(&resp.status.as_u16()),
        "{}",
        resp.text()
    );
    assert_eq!(counts(&env), before_drift);
}

#[tokio::test]
async fn half_variant_and_changed_content_are_rejected_without_residue() {
    let nodes = org("x");
    let env = env_with(&nodes);
    let app = env.router();
    let base = counts(&env);

    // 片欠け（variant だけ / variant_org だけ）は 400。
    for drop in ["variant", "variant_org"] {
        let mut c = identity(H1, pin(&nodes));
        c.as_object_mut().unwrap().remove(drop);
        let resp = send(&app, p(&body(c))).await;
        assert_eq!(resp.status.as_u16(), 400, "{drop}: {}", resp.text());
        assert_eq!(counts(&env), base);
    }

    let first = send(&app, p(&body(identity(H1, pin(&nodes))))).await;
    assert_eq!(first.status.as_u16(), 201, "{}", first.text());
    let stored = counts(&env);

    // 別 content hash / variant 脱落 / 構成 pin 変更は 409、何も増えない。
    let other = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let mut stripped = identity(H1, pin(&nodes));
    stripped.as_object_mut().unwrap().remove("variant");
    stripped.as_object_mut().unwrap().remove("variant_org");
    let mut other_pin = pin(&nodes);
    other_pin["effective_profile_sha256"] = json!(other);
    for c in [
        identity(other, pin(&nodes)),
        stripped,
        identity(H1, other_pin),
    ] {
        let resp = send(&app, p(&body(c))).await;
        assert_eq!(resp.status.as_u16(), 409, "{}", resp.text());
        assert_eq!(counts(&env), stored);
    }
}
