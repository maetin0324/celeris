use std::sync::Mutex;

use super::*;
use crate::celeris_exec::HttpResponse;

const THREAD: &str = "cos:project:P1";
const REQUEST: &str = "固定依頼: K4 の case を解いて報告する。\n";

/// 偽 API: 案件 CoS thread 1 本と message を持ち、受けた要求を全部記録する。
struct FakeApi {
    calls: Mutex<Vec<(String, String)>>,
    thread_kind: &'static str,
    messages: Mutex<Vec<Value>>,
    /// `POST …/relays` で付く author（daemon が認証した主体）。
    author: &'static str,
}

impl FakeApi {
    fn new(messages: Vec<Value>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            thread_kind: "project",
            messages: Mutex::new(messages),
            author: "operator:ops-r1",
        }
    }
    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().expect("calls").clone()
    }
}

const SAID_AT: &str = "2026-10-30T01:02:03Z";

/// relay の message（`relay.relayed_by` = author、`said_at` = 出典の時刻）。
fn relayed(id: &str, seq: u64, text: &str, author: &str, said_at: &str) -> Value {
    let mut m = message(id, seq, "user", text, Some(author));
    m["relay"] = json!({"relayed_by": author, "said_at": said_at, "scope": {"project_id": "P1"}});
    m
}

fn source() -> SourceInput {
    SourceInput {
        kind: SourceKind::ApprovedRequest,
        daemon: "production".into(),
        thread_id: Some("cos:project:PROD".into()),
        message_id: None,
        decision_id: Some("01DECISION".into()),
        said_at: SAID_AT.into(),
        request_sha256: sha256_hex(REQUEST.as_bytes()),
        decided_by: "human:rmaeda".into(),
        approved_by: "human:rmaeda".into(),
    }
}

fn message(id: &str, seq: u64, role: &str, text: &str, author: Option<&str>) -> Value {
    let mut m = json!({
        "id": id, "thread_id": THREAD, "seq": seq, "role": role, "text": text,
        "state": "queued", "client_message_id": null, "reply_to_id": null, "run_id": null,
        "attachment_ids": [], "cards": [], "workspace_files": [],
        "created_at": format!("2026-10-29T05:00:{seq:02}Z"), "updated_at": "2026-10-29T05:01:00Z",
    });
    if let Some(a) = author {
        m["author"] = json!(a);
    }
    m
}

impl Transport for FakeApi {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        self.calls
            .lock()
            .expect("calls")
            .push((method.into(), path.into()));
        let ok = |body| Ok(HttpResponse { status: 200, body });
        let base = format!("/api/v1/chat/threads/{THREAD}");
        if method == "GET" && path == base {
            return ok(json!({"thread": {
                "id": THREAD, "kind": self.thread_kind, "title": "ops-r1", "project_id": "P1",
                "status": "open"}, "active_run": null, "last_event_id": "0"}));
        }
        let msgs = format!("{base}/messages");
        if method == "GET" && path.starts_with(&format!("{msgs}?")) {
            // 1 頁 1 件で返し、頁送りも通す（新しい順に before_seq で遡る）。
            let all = self.messages.lock().expect("messages").clone();
            let before = path
                .split("before_seq=")
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(u64::MAX);
            let page: Vec<Value> = all
                .iter()
                .rev()
                .filter(|m| m["seq"].as_u64().is_some_and(|s| s < before))
                .take(1)
                .cloned()
                .collect();
            let next = page
                .first()
                .and_then(|m| m["seq"].as_u64())
                .filter(|s| *s > 1);
            return ok(
                json!({"items": page, "next_before_seq": next, "next_after_seq": null,
                "snapshot_event_id": "0"}),
            );
        }
        if method == "POST" && path == format!("{base}/relays") {
            let b = body.ok_or("no body")?;
            let mut all = self.messages.lock().expect("messages");
            let key = b["client_message_id"].clone();
            if let Some(m) = all.iter().find(|m| m["client_message_id"] == key) {
                return Ok(HttpResponse {
                    status: 202,
                    body: json!({"message": m, "run_id": null, "queue_position": 0}),
                });
            }
            let seq = all.len() as u64 + 1;
            let mut m = relayed(
                &format!("M{seq}"),
                seq,
                b["text"].as_str().unwrap_or(""),
                self.author,
                b["said_at"].as_str().unwrap_or(""),
            );
            m["client_message_id"] = key;
            all.push(m.clone());
            return Ok(HttpResponse {
                status: 202,
                body: json!({"message": m, "run_id": null, "queue_position": 0}),
            });
        }
        Ok(HttpResponse {
            status: 404,
            body: json!({"code": "not_found"}),
        })
    }
}

fn input(mode: Mode) -> IntakeInput {
    IntakeInput {
        thread_id: THREAD.into(),
        trial_id: "ops-r2-B-k2-1".into(),
        condition_id: "ops-r2-B".into(),
        request_text: REQUEST.into(),
        mode,
        source: source(),
        operator_session: Some("11111111-2222-4333-8444-555555555555".into()),
    }
}

fn no_task_api(api: &FakeApi) {
    let calls = api.calls();
    assert!(
        calls
            .iter()
            .all(|(_, p)| p.starts_with("/api/v1/chat/threads/")),
        "only chat thread endpoints are called: {calls:?}"
    );
    assert!(
        !calls.iter().any(|(_, p)| p.starts_with("/api/v1/tasks")),
        "{calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|(m, p)| m == "POST" && p.ends_with("/messages")),
        "a person's message endpoint is never used: {calls:?}"
    );
}

#[test]
fn ops_r1_intake_relay_records_source_and_posts_once() {
    let api = FakeApi::new(vec![]);
    let rec = intake(&api, &input(Mode::Relay)).expect("relay");
    assert_eq!(rec.schema, SCHEMA);
    assert_eq!(rec.intake_mode, "relay");
    assert_eq!(
        rec.client_message_id.as_deref(),
        Some("ops-r1-intake:ops-r2-B-k2-1")
    );
    assert_eq!(rec.project_id, "P1");
    assert_eq!(rec.request_sha256, sha256_hex(REQUEST.as_bytes()));
    assert_eq!(rec.relayed_by, "operator:ops-r1");
    assert_eq!(
        rec.source,
        Source {
            kind: SourceKind::ApprovedRequest,
            daemon: "production".into(),
            thread_id: Some("cos:project:PROD".into()),
            message_id: None,
            decision_id: Some("01DECISION".into()),
            said_at: SAID_AT.into(),
            request_sha256: sha256_hex(REQUEST.as_bytes()),
        }
    );
    // relay の said_at は出典の時刻（人が承認した時刻）。
    assert_eq!(
        api.messages.lock().expect("m")[0]["relay"]["said_at"],
        SAID_AT
    );
    // 再実行は同じ client_message_id なので同じ message（記録も同じ）。
    let again = intake(&api, &input(Mode::Relay)).expect("relay again");
    assert_eq!(again, rec);
    assert_eq!(api.messages.lock().expect("m").len(), 1);
    let posts: Vec<_> = api
        .calls()
        .into_iter()
        .filter(|(m, _)| m == "POST")
        .collect();
    assert_eq!(
        posts,
        vec![
            (
                "POST".to_string(),
                format!("/api/v1/chat/threads/{THREAD}/relays")
            );
            2
        ]
    );
    no_task_api(&api);
}

#[test]
fn ops_r1_intake_keeps_decider_executor_approver_apart() {
    let api = FakeApi::new(vec![]);
    let rec = intake(&api, &input(Mode::Relay)).expect("relay");
    let human = Actor {
        kind: ActorKind::Human,
        id: "human:rmaeda".into(),
    };
    let operator = Actor {
        kind: ActorKind::ExternalOperator,
        id: "operator:ops-r1".into(),
    };
    assert_eq!(rec.decided_by, human);
    assert_eq!(rec.approved_by, human);
    assert_eq!(rec.executed_by, operator);
    assert_eq!(rec.actor, operator);
    // human-ops の op 行: actor = executed_by = 実験専用 session、判断と承認は人、credential は operator principal。
    assert_eq!(
        rec.human_op,
        HumanOpRoles {
            actor_kind: "external_operator".into(),
            actor: "11111111-2222-4333-8444-555555555555".into(),
            decided_by: "human:human:rmaeda".into(),
            executed_by: "external_operator:11111111-2222-4333-8444-555555555555".into(),
            approved_by: "human:human:rmaeda".into(),
            credential: "operator:ops-r1".into(),
            request_ref: "chat-message:M1".into(),
            request_revision: "1".into(),
            policy_ref: "decision:01DECISION".into(),
            celeris_event_ref: "M1".into(),
        }
    );
    let v = serde_json::to_value(&rec).expect("json");
    for f in ["decided_by", "executed_by", "approved_by"] {
        assert!(v[f]["kind"].is_string() && v[f]["id"].is_string(), "{f}");
    }
    assert_eq!(v["source"]["kind"], "approved_request");
    // 人の発言の原文を出典にすれば policy_ref は人の message。
    let mut i = input(Mode::Relay);
    i.trial_id = "ops-r2-B-k2-2".into();
    i.source.kind = SourceKind::ChatMessage;
    i.source.decision_id = None;
    i.source.message_id = Some("01HUMANMSG".into());
    i.operator_session = None;
    let rec = intake(&api, &i).expect("chat message source");
    assert_eq!(rec.human_op.policy_ref, "chat-message:01HUMANMSG");
    assert_eq!(rec.human_op.actor, "operator:ops-r1");
}

#[test]
fn ops_r1_intake_bind_takes_only_sourced_operator_or_mcp_relays() {
    let api = FakeApi::new(vec![
        relayed("M1", 1, REQUEST, "operator:ops-r1", SAID_AT),
        relayed("M2", 2, REQUEST, "mcp:console", SAID_AT),
        message("M3", 3, "user", REQUEST, Some("operator:ops-r1")),
        message("M4", 4, "user", REQUEST, Some("human:rmaeda")),
        relayed("M5", 5, REQUEST, "operator:ops-r1", "2026-10-30T09:00:00Z"),
        {
            let mut m = relayed("M6", 6, REQUEST, "operator:ops-r1", SAID_AT);
            m["relay"]["relayed_by"] = json!("operator:other");
            m
        },
        relayed("M7", 7, "違う依頼", "operator:ops-r1", SAID_AT),
        message("M8", 8, "system", REQUEST, None),
        message("M9", 9, "user", REQUEST, None),
    ]);
    let rec = intake(&api, &input(Mode::Bind("M1".into()))).expect("operator relay");
    assert_eq!(rec.intake_mode, "bind");
    assert_eq!(rec.message_id, "M1");
    assert_eq!(rec.executed_by.kind, ActorKind::ExternalOperator);
    assert_eq!(rec.decided_by.kind, ActorKind::Human);
    let rec = intake(&api, &input(Mode::Bind("M2".into()))).expect("mcp relay");
    assert_eq!(rec.executed_by.kind, ActorKind::McpClient);
    assert_eq!(rec.human_op.credential, "mcp:console");
    // 出典（relay）の無い operator の message は人の依頼として数えない。
    let e = intake(&api, &input(Mode::Bind("M3".into()))).expect_err("no relay");
    assert!(e.contains("no relay"), "{e}");
    // 人の token の message は受けない（誰が token を使ったか区別できない）。
    let e = intake(&api, &input(Mode::Bind("M4".into()))).expect_err("human token");
    assert!(e.contains("person's token"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M5".into()))).expect_err("said_at");
    assert!(e.contains("said_at"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M6".into()))).expect_err("relayed_by");
    assert!(e.contains("relayed_by"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M7".into()))).expect_err("text");
    assert!(e.contains("sha256"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M8".into()))).expect_err("role");
    assert!(e.contains("role system"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M9".into()))).expect_err("no author");
    assert!(e.contains("unknown"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M10".into()))).expect_err("missing");
    assert!(e.contains("not in thread"), "{e}");
    assert!(api.calls().iter().all(|(m, _)| m == "GET"));
    // relay した結果の author が人なら（人の token を渡した）記録しない。
    let mut h = FakeApi::new(vec![]);
    h.author = "human:rmaeda";
    let e = intake(&h, &input(Mode::Relay)).expect_err("human relay");
    assert!(e.contains("person's token"), "{e}");
    // 統括 thread には入れない。
    let mut general = FakeApi::new(vec![]);
    general.thread_kind = "general";
    let e = intake(&general, &input(Mode::Relay)).expect_err("general");
    assert!(e.contains("project CoS thread"), "{e}");
    assert!(general.calls().iter().all(|(m, _)| m == "GET"));
    no_task_api(&api);
}

#[test]
fn ops_r1_intake_refuses_unsourced_requests_before_posting() {
    let cases: Vec<(&str, Box<dyn Fn(&mut SourceInput)>)> = vec![
        (
            "differs from --request-file",
            Box::new(|s| s.request_sha256 = sha256_hex(b"other")),
        ),
        ("needs decision_id", Box::new(|s| s.decision_id = None)),
        (
            "needs thread_id and message_id",
            Box::new(|s| s.kind = SourceKind::ChatMessage),
        ),
        ("production", Box::new(|s| s.daemon = "measure".into())),
        (
            "decided_by must be human",
            Box::new(|s| s.decided_by = "operator:ops-r1".into()),
        ),
        (
            "approved_by must be human",
            Box::new(|s| s.approved_by = "human:".into()),
        ),
        (
            "RFC 3339 UTC",
            Box::new(|s| s.said_at = "2026-10-30T10:02:03+09:00".into()),
        ),
    ];
    for (want, edit) in cases {
        let api = FakeApi::new(vec![]);
        let mut i = input(Mode::Relay);
        edit(&mut i.source);
        let e = intake(&api, &i).expect_err(want);
        assert!(e.contains(want), "{want}: {e}");
        assert!(api.calls().is_empty(), "{want}: nothing is sent");
        assert!(api.messages.lock().expect("m").is_empty());
    }
}

#[test]
fn ops_r1_intake_cli_writes_out_and_refuses_overwrite() {
    let dir = tempfile::tempdir().expect("dir");
    let req = dir.path().join("request.md");
    std::fs::write(&req, REQUEST).expect("req");
    let src = dir.path().join("source.json");
    std::fs::write(
        &src,
        json!({
            "kind": "approved_request", "daemon": "production", "decision_id": "01DECISION",
            "said_at": SAID_AT, "request_sha256": sha256_hex(REQUEST.as_bytes()),
            "decided_by": "human:rmaeda", "approved_by": "human:rmaeda",
        })
        .to_string(),
    )
    .expect("source");
    let out = dir.path().join("raw/intake.json");
    let args = |trial: &str| -> Vec<String> {
        [
            "ops-r1-intake",
            "--api-url",
            "http://127.0.0.1:1",
            "--token-env",
            "T",
            "--thread",
            THREAD,
            "--trial-id",
            trial,
            "--condition-id",
            "ops-r2-B",
            "--request-file",
            req.to_str().expect("p"),
            "--source",
            src.to_str().expect("p"),
            "--relay",
            "--out",
            out.to_str().expect("p"),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    let api = FakeApi::new(vec![]);
    let s = cli_with(&args("t1"), &api).expect("cli");
    assert_eq!(std::fs::read_to_string(&out).expect("out"), s);
    let v: Value = serde_json::from_str(&s).expect("json");
    assert_eq!(v["schema"], "ops-r1-intake/2");
    assert_eq!(v["message_id"], "M1");
    assert_eq!(
        v["actor"],
        json!({"kind": "external_operator", "id": "operator:ops-r1"})
    );
    assert_eq!(v["source"]["decision_id"], "01DECISION");
    assert_eq!(v["source"]["message_id"], Value::Null);
    // 同じ受付の再実行は byte 一致で受け、別の trial では上書きしない。
    assert_eq!(cli_with(&args("t1"), &api).expect("again"), s);
    let e = cli_with(&args("t2"), &api).expect_err("overwrite");
    assert!(e.contains("not overwritten"), "{e}");
    // 出典の file に知らない欄があれば拒否する。
    std::fs::write(&src, r#"{"kind":"approved_request","extra":1}"#).expect("bad");
    assert!(cli_with(&args("t1"), &api).is_err());
    no_task_api(&api);
}

#[test]
fn ops_r1_intake_options_need_exactly_one_mode_and_a_source() {
    let base: Vec<String> = [
        "ops-r1-intake",
        "--api-url",
        "http://x",
        "--token-env",
        "T",
        "--thread",
        "t",
        "--trial-id",
        "a",
        "--condition-id",
        "c",
        "--request-file",
        "f",
        "--source",
        "s",
        "--out",
        "o",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert!(Options::parse(&base).is_err());
    let mut both = base.clone();
    both.extend(["--relay", "--bind-message", "m"].map(String::from));
    assert!(Options::parse(&both).is_err());
    let mut bind = base.clone();
    bind.extend(["--bind-message", "m"].map(String::from));
    assert_eq!(
        Options::parse(&bind).expect("bind").mode,
        Mode::Bind("m".into())
    );
    let mut relay = base.clone();
    relay.extend(["--relay", "--operator-session", "u1"].map(String::from));
    let o = Options::parse(&relay).expect("relay");
    assert_eq!(o.mode, Mode::Relay);
    assert_eq!(o.operator_session.as_deref(), Some("u1"));
    // 旧い --send（人の token で送る）は無い。
    let mut send = base.clone();
    send.push("--send".into());
    assert!(
        Options::parse(&send)
            .expect_err("send")
            .contains("unknown option --send")
    );
    // 出典は必須。
    let no_source: Vec<String> = bind
        .iter()
        .enumerate()
        .filter(|(i, a)| *a != "--source" && !(*i > 0 && bind[i - 1] == "--source"))
        .map(|(_, a)| a.clone())
        .collect();
    assert!(
        Options::parse(&no_source)
            .expect_err("source")
            .contains("--source is required")
    );
    assert!(client_message_id(&"x".repeat(200)).is_err());
}
