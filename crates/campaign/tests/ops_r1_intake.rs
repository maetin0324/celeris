//! ops-r1 intake を実 task-api router（in-process、一時 DB）に当てる（ADR 2026-10-30-ops-r2-isolation-principal D5）。
//! 認可済みの operator principal（`operator:ops-r1`）が固定依頼文を `POST …/relays` で案件 CoS thread に入れ、
//! 出典（人の承認の id・依頼文 sha256・承認時刻）付きの受付として記録される。task は 1 件も作られず
//! （`POST /api/v1/tasks` を呼ばない）、人の token は使わない。出典の無い operator の message と、人の token で
//! 送った message は人の依頼として記録しない。記録の判断者・実行者・承認者は human-ops /2 の別の欄に入り、
//! relay の op は人の能動時間に数えない。実 LLM・外部ネットワークは使わない（CoS run は起こさない）。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::sync::{Arc, Mutex};

use api_common::{EnvOptions, TEST_ACTOR, project_cos_thread, send};
use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    human_ops::{ActorKind, read_records},
    human_ops_report::counts_as_human_op,
    ops_r1_intake::{
        IntakeInput, Mode, SourceInput, SourceKind, client_message_id, intake, sha256_hex,
    },
};
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};

const REQUEST: &str = "固定依頼: K2 の case を解いて報告する。\n";
const OPERATOR_TOKEN: &str = "ops-r2-operator-token";
const HUMAN_TOKEN: &str = "ops-r2-human-token";
const SAID_AT: &str = "2026-10-30T01:02:03Z";
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
const H: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct RouterTransport {
    app: axum::Router,
    rt: tokio::runtime::Handle,
    token: &'static str,
    calls: Arc<Mutex<Vec<(String, String)>>>,
}

impl Transport for RouterTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        self.calls
            .lock()
            .map_err(|e| e.to_string())?
            .push((method.into(), path.into()));
        let auth = format!("Bearer {}", self.token);
        let headers = [("authorization", auth.as_str())];
        let req = match (method, body) {
            ("GET", None) => api_common::get_with(path, &headers),
            ("POST", Some(b)) => api_common::post_json_with(path, b, &headers),
            _ => return Err(format!("unexpected {method} {path}")),
        };
        let r = self.rt.block_on(send(&self.app, req));
        let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        Ok(HttpResponse {
            status: r.status.as_u16(),
            body,
        })
    }
}

fn task_count(db: &std::path::Path) -> i64 {
    rusqlite::Connection::open(db)
        .expect("db")
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .expect("count")
}

fn source() -> SourceInput {
    SourceInput {
        kind: SourceKind::ApprovedRequest,
        daemon: "production".into(),
        thread_id: None,
        message_id: None,
        decision_id: Some("01APPROVAL".into()),
        said_at: SAID_AT.into(),
        request_sha256: sha256_hex(REQUEST.as_bytes()),
        decided_by: "human:rmaeda".into(),
        approved_by: "human:rmaeda".into(),
    }
}

fn cli(args: &[&str]) -> (i32, String) {
    let o = std::process::Command::new(
        std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"),
    )
    .arg("human-op")
    .args(args)
    .env("CELERIS_HUMAN_OPS_NOW", "2026-10-30T02:00:00Z")
    .env("CELERIS_HUMAN_OPS_HOST", "eval-host")
    .output()
    .expect("spawn");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn ops_r1_intake_operator_relay_with_source_on_the_real_router() {
    let env = api_common::TestEnv::with(EnvOptions {
        extra_tokens: vec![
            ApiToken::new(
                Principal::Operator {
                    name: "ops-r1".into(),
                },
                OPERATOR_TOKEN,
            ),
            ApiToken::new(
                Principal::Human {
                    name: "ops-r2".into(),
                },
                HUMAN_TOKEN,
            ),
        ],
        ..EnvOptions::default()
    });
    let thread = project_cos_thread(&env.store, "ops-r2-measure");
    let app = env.router();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let op = RouterTransport {
        app: app.clone(),
        rt: tokio::runtime::Handle::current(),
        token: OPERATOR_TOKEN,
        calls: calls.clone(),
    };
    let tid = thread.id.clone();
    let input = move |trial: &str, mode: Mode| IntakeInput {
        thread_id: tid.clone(),
        trial_id: trial.into(),
        condition_id: "ops-r2-B".into(),
        request_text: REQUEST.into(),
        mode,
        source: source(),
        operator_session: Some(SESSION.into()),
    };
    let input2 = input.clone();
    // --relay: operator principal で固定依頼文を relay する（再実行しても 1 件）。
    let (relayed, again, op) = tokio::task::spawn_blocking(move || {
        let r = intake(&op, &input("ops-r2-B-k2-1", Mode::Relay));
        let a = intake(&op, &input("ops-r2-B-k2-1", Mode::Relay));
        (r, a, op)
    })
    .await
    .expect("join");
    let relayed = relayed.expect("relay");
    assert_eq!(again.expect("relay again"), relayed);
    assert_eq!(relayed.thread_id, thread.id);
    assert_eq!(
        relayed.project_id,
        thread.project_id.clone().expect("project")
    );
    assert_eq!(relayed.intake_mode, "relay");
    assert_eq!(relayed.actor.kind, ActorKind::ExternalOperator);
    assert_eq!(relayed.executed_by.id, "operator:ops-r1");
    assert_eq!(relayed.relayed_by, "operator:ops-r1");
    assert_eq!(relayed.decided_by.kind, ActorKind::Human);
    assert_eq!(relayed.approved_by.id, "human:rmaeda");
    assert_eq!(relayed.source.decision_id.as_deref(), Some("01APPROVAL"));
    assert_eq!(relayed.source.said_at, SAID_AT);
    assert_eq!(relayed.request_sha256, sha256_hex(REQUEST.as_bytes()));
    assert_eq!(
        relayed.client_message_id,
        Some(client_message_id("ops-r2-B-k2-1").expect("key"))
    );
    assert!(
        relayed.accepted_at.ends_with('Z'),
        "{}",
        relayed.accepted_at
    );

    // 同じ relay を --bind-message で結んでも同じ出典・主体になる。
    let mid = relayed.message_id.clone();
    let (bound, op) = tokio::task::spawn_blocking(move || {
        let mut i = input2("ops-r2-B-k2-1", Mode::Bind(mid));
        i.trial_id = "ops-r2-B-k2-1".into();
        (intake(&op, &i), op)
    })
    .await
    .expect("join");
    let bound = bound.expect("bind");
    assert_eq!(bound.intake_mode, "bind");
    assert_eq!(bound.executed_by, relayed.executed_by);
    assert_eq!(bound.source, relayed.source);

    // 出典の無い operator の message（…/messages で送った）は人の依頼として記録しない。
    let plain = send(
        &app,
        api_common::post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &json!({"client_message_id": "plain-1", "text": REQUEST, "attachment_ids": [],
                "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &[("authorization", &format!("Bearer {OPERATOR_TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(plain.status.as_u16(), 202, "{}", plain.text());
    assert_eq!(plain.json()["message"]["author"], "operator:ops-r1");
    let plain_id = plain.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    // 人の token の message も受けない。
    let by_human = send(
        &app,
        api_common::post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &json!({"client_message_id": "human-1", "text": REQUEST, "attachment_ids": [],
                "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &[("authorization", &format!("Bearer {HUMAN_TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(by_human.status.as_u16(), 202, "{}", by_human.text());
    let human_id = by_human.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let tid = thread.id.clone();
    let (no_source, human_msg, op) = tokio::task::spawn_blocking(move || {
        let i = |m: String| IntakeInput {
            thread_id: tid.clone(),
            trial_id: "ops-r2-B-k2-2".into(),
            condition_id: "ops-r2-B".into(),
            request_text: REQUEST.into(),
            mode: Mode::Bind(m),
            source: source(),
            operator_session: None,
        };
        (intake(&op, &i(plain_id)), intake(&op, &i(human_id)), op)
    })
    .await
    .expect("join");
    assert!(
        no_source
            .as_ref()
            .expect_err("no relay")
            .contains("no relay"),
        "{no_source:?}"
    );
    assert!(
        human_msg
            .as_ref()
            .expect_err("human token")
            .contains("person's token"),
        "{human_msg:?}"
    );
    // 人の token では relay できない（daemon が 403）ので、その経路の記録は作られない。
    let human_t = RouterTransport {
        app: app.clone(),
        rt: tokio::runtime::Handle::current(),
        token: HUMAN_TOKEN,
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let tid = thread.id.clone();
    let refused = tokio::task::spawn_blocking(move || {
        intake(
            &human_t,
            &IntakeInput {
                thread_id: tid,
                trial_id: "ops-r2-B-k2-3".into(),
                condition_id: "ops-r2-B".into(),
                request_text: REQUEST.into(),
                mode: Mode::Relay,
                source: source(),
                operator_session: None,
            },
        )
    })
    .await
    .expect("join");
    assert!(
        refused.as_ref().expect_err("403").contains("403"),
        "{refused:?}"
    );
    drop(op);

    // task は作られず、呼んだのは chat thread の endpoint だけ。relay は 1 件。
    assert_eq!(task_count(&env.db_path), 0);
    let calls = calls.lock().expect("calls").clone();
    assert!(
        calls
            .iter()
            .all(|(_, p)| p.starts_with("/api/v1/chat/threads/")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .filter(|(m, _)| m == "POST")
            .all(|(_, p)| p.ends_with("/relays")),
        "{calls:?}"
    );
    let listed = send(
        &app,
        api_common::get_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &[("authorization", &format!("Bearer {OPERATOR_TOKEN}"))],
        ),
    )
    .await;
    let items = listed.json()["items"].as_array().expect("items").clone();
    assert_eq!(
        items.iter().filter(|m| m["relay"].is_object()).count(),
        1,
        "one relay per trial"
    );
    assert_ne!(TEST_ACTOR, relayed.executed_by.id);

    // human-ops /2: 記録の値をそのまま `human-op record` に渡すと、判断者・実行者・承認者が別の欄に残り、
    // relay の op は人の能動時間に数えない（actor が人でない）。人の区間は人が記録したものだけを数える。
    let dir = tempfile::tempdir().expect("dir");
    let f = dir.path().join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    let r = &relayed.human_op;
    let base = [
        "record",
        "--file",
        f,
        "--campaign",
        "c1",
        "--manifest-sha256",
        H,
        "--case",
        "k2",
        "--arm",
        "B0",
        "--rep",
        "1",
    ];
    let mut relay_op: Vec<&str> = base.to_vec();
    relay_op.extend([
        "--actor-kind",
        r.actor_kind.as_str(),
        "--actor",
        r.actor.as_str(),
        "--decided-by",
        r.decided_by.as_str(),
        "--executed-by",
        r.executed_by.as_str(),
        "--approved-by",
        r.approved_by.as_str(),
        "--credential",
        r.credential.as_str(),
        "--request-ref",
        r.request_ref.as_str(),
        "--request-revision",
        r.request_revision.as_str(),
        "--policy-ref",
        r.policy_ref.as_str(),
        "--celeris-event-ref",
        r.celeris_event_ref.as_str(),
        "--mode",
        "manual",
        "--intent",
        "protocol",
        "--op-kind",
        "protocol_step",
        "--logical-op",
        "relay-request",
        "--started-at",
        "2026-10-30T01:10:00Z",
        "--ended-at",
        "2026-10-30T01:11:00Z",
        "--outcome",
        "ok",
    ]);
    let (c, e) = cli(&relay_op);
    assert_eq!(c, 0, "{e}");
    let mut human_op: Vec<&str> = base.to_vec();
    human_op.extend([
        "--actor-kind",
        "human",
        "--actor",
        "rmaeda",
        "--decided-by",
        "human:rmaeda",
        "--executed-by",
        "human:rmaeda",
        "--mode",
        "manual",
        "--intent",
        "safety",
        "--op-kind",
        "safety_approval",
        "--logical-op",
        "approve",
        "--started-at",
        "2026-10-30T01:00:00Z",
        "--ended-at",
        "2026-10-30T01:02:03Z",
        "--outcome",
        "ok",
    ]);
    let (c, e) = cli(&human_op);
    assert_eq!(c, 0, "{e}");
    let recs = read_records(std::path::Path::new(f)).expect("records");
    let relay_rows: Vec<_> = recs
        .iter()
        .filter(|x| x.logical_op_id.as_deref() == Some("relay-request"))
        .collect();
    assert_eq!(relay_rows.len(), 2, "op_start and op_end");
    for x in &relay_rows {
        assert_eq!(
            x.actor.as_ref().map(|a| a.kind),
            Some(ActorKind::ExternalOperator)
        );
        assert_eq!(x.actor.as_ref().map(|a| a.id.as_str()), Some(SESSION));
        assert_eq!(
            x.decided_by.as_ref().map(|a| a.kind),
            Some(ActorKind::Human)
        );
        assert_eq!(
            x.executed_by.as_ref().map(|a| a.kind),
            Some(ActorKind::ExternalOperator)
        );
        assert_eq!(
            x.approved_by.as_ref().map(|a| a.kind),
            Some(ActorKind::Human)
        );
        assert_eq!(x.credential.as_deref(), Some("operator:ops-r1"));
        assert_eq!(x.policy_ref.as_deref(), Some("decision:01APPROVAL"));
        assert_eq!(
            x.request_ref.as_deref(),
            Some(format!("chat-message:{}", relayed.message_id).as_str())
        );
        assert!(!counts_as_human_op(x), "the relay is not the person's time");
    }
    let human_rows: Vec<_> = recs
        .iter()
        .filter(|x| x.logical_op_id.as_deref() == Some("approve"))
        .collect();
    assert_eq!(human_rows.len(), 2);
    assert!(human_rows.iter().all(|x| counts_as_human_op(x)));
}
