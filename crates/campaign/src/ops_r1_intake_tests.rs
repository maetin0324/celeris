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
    /// `POST …/messages` で付く author（daemon が認証した主体）。
    author: &'static str,
}

impl FakeApi {
    fn new(messages: Vec<Value>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            thread_kind: "project",
            messages: Mutex::new(messages),
            author: "human:rmaeda",
        }
    }
    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().expect("calls").clone()
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
        if method == "POST" && path == msgs {
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
            let mut m = message(
                &format!("M{seq}"),
                seq,
                "user",
                b["text"].as_str().unwrap_or(""),
                Some(self.author),
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
        trial_id: "ops-r1-B-k4-1".into(),
        condition_id: "ops-r1-B".into(),
        request_text: REQUEST.into(),
        mode,
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
}

#[test]
fn ops_r1_intake_bind_records_fields_without_task_api() {
    let api = FakeApi::new(vec![
        message("M1", 1, "user", REQUEST, Some("human:rmaeda")),
        message("M2", 2, "assistant", "了解", None),
        message("M3", 3, "user", "別の話", Some("human:rmaeda")),
    ]);
    let rec = intake(&api, &input(Mode::Bind("M1".into()))).expect("bind");
    assert_eq!(rec.schema, SCHEMA);
    assert_eq!(rec.trial_id, "ops-r1-B-k4-1");
    assert_eq!(rec.condition_id, "ops-r1-B");
    assert_eq!(rec.thread_id, THREAD);
    assert_eq!(rec.project_id, "P1");
    assert_eq!(rec.message_id, "M1");
    assert_eq!(rec.message_seq, 1);
    assert_eq!(rec.accepted_at, "2026-10-29T05:00:01Z");
    assert_eq!(rec.request_sha256, sha256_hex(REQUEST.as_bytes()));
    assert_eq!(rec.request_bytes, REQUEST.len());
    assert_eq!(rec.intake_mode, "bind");
    assert_eq!(
        rec.actor,
        Actor {
            kind: ActorKind::Human,
            id: "human:rmaeda".into()
        }
    );
    // 頁送りで seq 3 → 2 → 1 と遡った。POST は 0 回。
    assert!(api.calls().iter().all(|(m, _)| m == "GET"));
    no_task_api(&api);
    let v = serde_json::to_value(&rec).expect("json");
    for f in [
        "trial_id",
        "condition_id",
        "thread_id",
        "project_id",
        "message_id",
        "accepted_at",
        "request_sha256",
    ] {
        assert!(v[f].is_string(), "{f}");
    }
    assert_eq!(v["actor"]["kind"], "human");
}

#[test]
fn ops_r1_intake_send_posts_to_the_cos_thread_once() {
    let api = FakeApi::new(vec![]);
    let rec = intake(&api, &input(Mode::Send)).expect("send");
    assert_eq!(rec.intake_mode, "send");
    assert_eq!(
        rec.client_message_id.as_deref(),
        Some("ops-r1-intake:ops-r1-B-k4-1")
    );
    assert_eq!(rec.actor.kind, ActorKind::Human);
    // 再実行は同じ client_message_id なので同じ message（記録も同じ）。
    let again = intake(&api, &input(Mode::Send)).expect("send again");
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
                format!("/api/v1/chat/threads/{THREAD}/messages")
            ),
            (
                "POST".to_string(),
                format!("/api/v1/chat/threads/{THREAD}/messages")
            ),
        ]
    );
    no_task_api(&api);
}

#[test]
fn ops_r1_intake_rejects_non_person_wrong_text_and_wrong_thread() {
    let api = FakeApi::new(vec![
        message("M1", 1, "user", REQUEST, Some("operator:claude-code")),
        message("M2", 2, "user", "違う依頼", Some("human:rmaeda")),
        message("M3", 3, "system", REQUEST, None),
        message("M4", 4, "user", REQUEST, None),
    ]);
    let e = intake(&api, &input(Mode::Bind("M1".into()))).expect_err("operator");
    assert!(e.contains("external_operator"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M2".into()))).expect_err("text");
    assert!(e.contains("sha256"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M3".into()))).expect_err("role");
    assert!(e.contains("role system"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M4".into()))).expect_err("no author");
    assert!(e.contains("unknown"), "{e}");
    let e = intake(&api, &input(Mode::Bind("M9".into()))).expect_err("missing");
    assert!(e.contains("not in thread"), "{e}");
    // --send でも人の token でなければ記録しない。
    let mut op = FakeApi::new(vec![]);
    op.author = "operator:claude-code";
    assert!(intake(&op, &input(Mode::Send)).is_err());
    // 統括 thread には入れない。
    let mut general = FakeApi::new(vec![]);
    general.thread_kind = "general";
    let e = intake(&general, &input(Mode::Send)).expect_err("general");
    assert!(e.contains("project CoS thread"), "{e}");
    assert!(general.calls().iter().all(|(m, _)| m == "GET"));
    no_task_api(&api);
}

#[test]
fn ops_r1_intake_cli_writes_out_and_refuses_overwrite() {
    let dir = tempfile::tempdir().expect("dir");
    let req = dir.path().join("request.md");
    std::fs::write(&req, REQUEST).expect("req");
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
            "ops-r1-B",
            "--request-file",
            req.to_str().expect("p"),
            "--send",
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
    assert_eq!(v["message_id"], "M1");
    assert_eq!(v["actor"], json!({"kind": "human", "id": "human:rmaeda"}));
    // 同じ受付の再実行は byte 一致で受け、別の trial では上書きしない。
    assert_eq!(cli_with(&args("t1"), &api).expect("again"), s);
    let e = cli_with(&args("t2"), &api).expect_err("overwrite");
    assert!(e.contains("not overwritten"), "{e}");
    no_task_api(&api);
}

#[test]
fn ops_r1_intake_options_need_exactly_one_mode() {
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
        "--out",
        "o",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert!(Options::parse(&base).is_err());
    let mut both = base.clone();
    both.extend(["--send", "--bind-message", "m"].map(String::from));
    assert!(Options::parse(&both).is_err());
    let mut bind = base.clone();
    bind.extend(["--bind-message", "m"].map(String::from));
    assert_eq!(
        Options::parse(&bind).expect("bind").mode,
        Mode::Bind("m".into())
    );
    let mut unknown = base;
    unknown.extend(["--send", "--task-template", "x"].map(String::from));
    assert!(Options::parse(&unknown).is_err());
    assert!(client_message_id(&"x".repeat(200)).is_err());
}
