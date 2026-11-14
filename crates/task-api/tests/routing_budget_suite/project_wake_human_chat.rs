//! ADR 2026-11-04-project-controller D12・付記 A1 の 9: 案件 thread の人の発言・`…/relays`・`chat.relay_forward`・
//! 人の受信箱の回答は、書き込みと同じ transaction で `human_epoch` を増やす。CoS 自身の発言・回答は数えない。
use crate::common;

use common::cos_ops::{OPS, op_body};
use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::chat::{ChatPostMessageRequest, ChatSendMode, ChatThread};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::integration_request::IntegrationRequest;
use task_core::{
    Approval, ApprovalId, ApprovalStore, Event, ProjectId, Status, TaskKind,
    TaskRepoIntegrationState, TaskRepoIntegrationStatus, TaskStore, Trigger, WorkspaceSpec,
};
use task_core::{ProjectRepo, RepoId, RepoKind, RepoRun};
use time::{Duration, OffsetDateTime};

const WEB_TOKEN: &str = "web-human-token";
const OPERATOR_TOKEN: &str = "operator-token";
const WEB: [(&str, &str); 1] = [("authorization", "Bearer web-human-token")];
const OPERATOR: [(&str, &str); 1] = [("authorization", "Bearer operator-token")];

struct Fixture {
    env: TestEnv,
    app: axum::Router,
    thread: ChatThread,
    project: ProjectId,
}

impl Fixture {
    async fn new(mode: &str) -> Self {
        let env = TestEnv::with(EnvOptions {
            token: Some("s3cret-token-value".into()),
            extra_tokens: vec![
                ApiToken::new(Principal::Human { name: "web".into() }, WEB_TOKEN),
                ApiToken::new(
                    Principal::Operator {
                        name: "claude-code".into(),
                    },
                    OPERATOR_TOKEN,
                ),
            ],
            ..EnvOptions::default()
        });
        let thread = project_cos_thread(&env.store, "pilot:human-chat");
        let project: ProjectId = thread
            .project_id
            .as_deref()
            .expect("project thread")
            .parse()
            .expect("id");
        let app = task_api::router(
            env.state
                .clone()
                .with_project_controller(true, vec![project.to_string()]),
        );
        if mode != "none" {
            let resp = send(
                &app,
                put_json_with(
                    &format!("/api/v1/projects/{project}/controller"),
                    &json!({"mode": mode, "expected_revision": 0, "reason": "人が試す"}),
                    &admin_headers(),
                ),
            )
            .await;
            assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        }
        Self {
            env,
            app,
            thread,
            project,
        }
    }

    fn epoch(&self) -> i64 {
        self.env
            .store
            .project_controller_state(&self.project.to_string())
            .expect("state")
            .map_or(0, |state| state.human_epoch)
    }

    fn wake_keys(&self) -> Vec<String> {
        self.env
            .store
            .project_wakes_pending(&self.project.to_string())
            .expect("wakes")
            .into_iter()
            .map(|wake| wake.wake_key)
            .collect()
    }

    fn assert_counted_once(&self, what: &str) {
        assert_eq!(self.epoch(), 1, "{what}");
        assert_eq!(self.wake_keys(), vec!["human:1".to_string()], "{what}");
    }

    fn assert_not_counted(&self, what: &str) {
        assert_eq!(self.epoch(), 0, "{what}");
        assert!(self.wake_keys().is_empty(), "{what}");
    }

    fn sql(&self, sql: &str) {
        rusqlite::Connection::open(&self.env.db_path)
            .expect("open")
            .execute_batch(sql)
            .expect("sql");
    }

    fn refuse_wakes(&self) {
        self.sql(
            "CREATE TRIGGER refuse_wake BEFORE INSERT ON project_wakes \
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        );
    }

    fn allow_wakes(&self) {
        self.sql("DROP TRIGGER refuse_wake");
    }

    fn count(&self, sql: &str) -> i64 {
        rusqlite::Connection::open(&self.env.db_path)
            .expect("open")
            .query_row(sql, [], |row| row.get(0))
            .expect("count")
    }

    /// 案件・task・events・発言の行の全部（失敗した要求が何も残していないことを比べる）。
    fn domain_rows(&self) -> String {
        rusqlite::Connection::open(&self.env.db_path)
            .expect("open")
            .query_row(
                "SELECT (SELECT json_group_array(json) FROM tasks), \
                 (SELECT COUNT(*) FROM events), (SELECT COUNT(*) FROM chat_messages), \
                 (SELECT COUNT(*) FROM approvals WHERE decision IS NOT NULL), \
                 (SELECT json_group_array(state) FROM task_integrations)",
                [],
                |row| {
                    Ok(format!(
                        "{}|{}|{}|{}|{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?
                    ))
                },
            )
            .expect("rows")
    }

    /// 走っている CoS run の bearer。入力は人の発言（store から直に書くので数えない）。返すのは `(bearer, 入力の id)`。
    fn run(&self, thread: &str, key: &str) -> (String, String) {
        let now = OffsetDateTime::now_utc();
        let store = &self.env.store;
        let said = store
            .chat_message_post_as(
                thread,
                &message_request(&format!("input-{key}")),
                "human:web",
                now,
            )
            .expect("message")
            .response
            .message
            .id;
        let run = format!("run-{key}");
        store
            .chat_run_claim_next(thread, &run, &json!({}), now)
            .expect("claim")
            .expect("run");
        let bearer = task_api::cos::issue_run_bearer(store, thread, &run, Duration::hours(1))
            .expect("issue");
        store
            .chat_run_record_policy(&run, "5", "test")
            .expect("policy");
        (format!("Bearer {bearer}"), said)
    }

    fn task(&self, kind: TaskKind, status: Status) -> task_core::Task {
        let mut task = new_task(kind, status);
        task.project_id = Some(self.project);
        self.env.seed(&task);
        task
    }

    fn general(&self) -> ChatThread {
        self.env
            .store
            .cos_thread_open(
                &task_core::chat::ChatOpenThreadRequest { project_id: None },
                OffsetDateTime::now_utc(),
            )
            .expect("general")
            .thread
    }

    async fn post(&self, path: &str, body: &Value, headers: &[(&str, &str)]) -> Resp {
        send(&self.app, post_json_with(path, body, headers)).await
    }

    /// 受信箱の `kind` の項目（1 件）の id。
    async fn inbox_item(&self, kind: &str) -> String {
        let resp = send(
            &self.app,
            get_admin(&format!("/api/v1/inbox/items?kind={kind}")),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        resp.json()["items"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item["id"].as_str())
            .unwrap_or_else(|| panic!("no {kind} item: {}", resp.text()))
            .to_owned()
    }
}

fn message_request(key: &str) -> ChatPostMessageRequest {
    ChatPostMessageRequest {
        client_message_id: key.into(),
        text: "人の発言".into(),
        attachment_ids: vec![],
        reply_to_id: None,
        mode: ChatSendMode::Queue,
        resume_queue: false,
    }
}

fn message_body(key: &str) -> Value {
    json!({"client_message_id": key, "text": "人の発言", "attachment_ids": [],
           "reply_to_id": null, "mode": "queue", "resume_queue": false})
}

fn relay_body(key: &str) -> Value {
    json!({"client_message_id": key, "text": "人の指示", "said_at": "2026-01-01T00:00:00Z",
           "task_ids": []})
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339")
}

/// 統括 CoS が人の発言を案件 thread へ写す操作の準備: `(bearer, 操作の本文)`。
async fn forward_op(f: &Fixture, key: &str) -> (String, Value) {
    let task = f.task(TaskKind::Execute, Status::Ready);
    let general = f.general();
    let (bearer, said) = f.run(&general.id, key);
    let op = op_body(
        key,
        "POST",
        &format!("/api/v1/chat/threads/{}/forwards", f.thread.id),
        json!({"client_message_id": format!("forward-{key}"), "message_id": said,
               "task_ids": [task.id.to_string()],
               "expires_at": rfc3339(OffsetDateTime::now_utc() + Duration::hours(1))}),
    );
    (bearer, with_policy(op))
}

fn with_policy(mut op: Value) -> Value {
    op["policy_version"] = json!("5");
    op
}

#[tokio::test]
async fn project_wake_api_human_chat_message_bumps_once_and_a_resend_does_not() {
    let f = Fixture::new("shadow").await;
    let path = format!("/api/v1/chat/threads/{}/messages", f.thread.id);
    let resp = f.post(&path, &message_body("h-1"), &WEB).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    f.assert_counted_once("message");
    // 同じ client_message_id の再送は新しい発言でない。
    let resp = f.post(&path, &message_body("h-1"), &WEB).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    f.assert_counted_once("resend");
}

#[tokio::test]
async fn project_wake_api_human_chat_relay_bumps_once_and_a_resend_does_not() {
    let f = Fixture::new("shadow").await;
    let path = format!("/api/v1/chat/threads/{}/relays", f.thread.id);
    let resp = f.post(&path, &relay_body("r-1"), &OPERATOR).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    f.assert_counted_once("relay");
    let resp = f.post(&path, &relay_body("r-1"), &OPERATOR).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    f.assert_counted_once("resend");
}

#[tokio::test]
async fn project_wake_api_human_chat_forward_bumps_once_and_a_replay_does_not() {
    let f = Fixture::new("shadow").await;
    let (bearer, op) = forward_op(&f, "fw").await;
    let token = [("authorization", bearer.as_str())];
    let resp = f.post(OPS, &op, &token).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(
        resp.json()["operation"]["state"],
        "applied",
        "{}",
        resp.text()
    );
    f.assert_counted_once("forward");
    let resp = f.post(OPS, &op, &token).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    f.assert_counted_once("replay");
}

/// 書き込みの検査で断られた要求は数えない。
#[tokio::test]
async fn project_wake_api_human_chat_refused_writes_do_not_bump() {
    let f = Fixture::new("shadow").await;
    let messages = format!("/api/v1/chat/threads/{}/messages", f.thread.id);
    let relays = format!("/api/v1/chat/threads/{}/relays", f.thread.id);
    let mut blank = message_body("blank");
    blank["text"] = json!("  ");
    let resp = f.post(&messages, &blank, &WEB).await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    f.assert_not_counted("blank message");
    // 人の token で relays は 403。
    let resp = f.post(&relays, &relay_body("r"), &WEB).await;
    assert_eq!(resp.status.as_u16(), 403, "{}", resp.text());
    f.assert_not_counted("relay by a person");
    let mut late = relay_body("late");
    late["said_at"] = json!("2999-01-01T00:00:00Z");
    let resp = f.post(&relays, &late, &OPERATOR).await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
    f.assert_not_counted("relay from the future");
}

/// `human_epoch` の書き込みが落ちれば、発言も残らない（同じ transaction）。
#[tokio::test]
async fn project_wake_api_human_chat_bump_failure_rolls_the_message_back() {
    let f = Fixture::new("shadow").await;
    let (bearer, op) = forward_op(&f, "fw").await;
    let before = f.domain_rows();
    f.refuse_wakes();
    let resp = f
        .post(
            &format!("/api/v1/chat/threads/{}/messages", f.thread.id),
            &message_body("h-1"),
            &WEB,
        )
        .await;
    assert!(!resp.status.is_success(), "message: {}", resp.text());
    let resp = f
        .post(
            &format!("/api/v1/chat/threads/{}/relays", f.thread.id),
            &relay_body("r-1"),
            &OPERATOR,
        )
        .await;
    assert!(!resp.status.is_success(), "relay: {}", resp.text());
    assert_eq!(f.domain_rows(), before);
    // 断られた operation は監査の記録（event）を残すので、発言の行だけ比べる。
    let messages = f.count("SELECT COUNT(*) FROM chat_messages");
    let token = [("authorization", bearer.as_str())];
    let resp = f.post(OPS, &op, &token).await;
    assert!(
        !resp.status.is_success() || resp.json()["operation"]["state"] != "applied",
        "forward: {}",
        resp.text()
    );
    f.allow_wakes();
    f.assert_not_counted("bump failure");
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_messages"), messages);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id = 'forward-fw'"),
        0
    );
}

/// CoS 自身の発言、operator の自分の発言、統括 thread の人の発言は数えない。
#[tokio::test]
async fn project_wake_api_human_chat_cos_and_operator_messages_are_not_counted() {
    let f = Fixture::new("shadow").await;
    let (bearer, _) = f.run(&f.thread.id, "cos");
    let token = [("authorization", bearer.as_str())];
    let path = format!("/api/v1/chat/threads/{}/messages", f.thread.id);
    let resp = f
        .post(
            OPS,
            &with_policy(op_body("cos-say", "POST", &path, message_body("cos-1"))),
            &token,
        )
        .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(
        resp.json()["operation"]["state"],
        "applied",
        "{}",
        resp.text()
    );
    f.assert_not_counted("CoS message");

    let resp = f.post(&path, &message_body("op-1"), &OPERATOR).await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    f.assert_not_counted("operator message");

    let general = f.general();
    let resp = f
        .post(
            &format!("/api/v1/chat/threads/{}/messages", general.id),
            &message_body("g-1"),
            &WEB,
        )
        .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    f.assert_not_counted("general thread");
}

/// 案件の状態の行が無い・`off` の案件では何も書かない（I13）。
#[tokio::test]
async fn project_wake_api_human_chat_off_project_is_inert() {
    let f = Fixture::new("none").await;
    let resp = f
        .post(
            &format!("/api/v1/chat/threads/{}/messages", f.thread.id),
            &message_body("h-1"),
            &WEB,
        )
        .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    assert!(
        f.env
            .store
            .project_controller_state(&f.project.to_string())
            .expect("state")
            .is_none()
    );
}

// ---- 受信箱の回答 ----

/// 受信箱の項目か、受信箱が委ねる route。
#[derive(Debug, Clone, Copy)]
enum Answer {
    Question,
    Decision,
    Draft,
    Failed,
    IntegrationRequest,
    PlanGate,
    PlanGateWithdraw,
    PhaseGate,
    PhaseGateWithdraw,
    Approve,
    Reject,
    AuthorizationDecide,
    Rereview,
    Unroutable,
    IntegrationHold,
    IntegrationDiscard,
    IntegrationReopen,
}

const ANSWERS: [Answer; 17] = [
    Answer::Question,
    Answer::Decision,
    Answer::Draft,
    Answer::Failed,
    Answer::IntegrationRequest,
    Answer::PlanGate,
    Answer::PlanGateWithdraw,
    Answer::PhaseGate,
    Answer::PhaseGateWithdraw,
    Answer::Approve,
    Answer::Reject,
    Answer::AuthorizationDecide,
    Answer::Rereview,
    Answer::Unroutable,
    Answer::IntegrationHold,
    Answer::IntegrationDiscard,
    Answer::IntegrationReopen,
];

fn raise_decision(env: &TestEnv, task: &task_core::Task, id: &str) {
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    env.store
        .append_event(
            task.id,
            &Event::DecisionRequested {
                decision: Box::new(DecisionRequest {
                    id: id.into(),
                    key: "h1".into(),
                    kind: DecisionKind::Choice,
                    question: "which?".into(),
                    options: vec![opt("vault"), opt("manual")],
                    recommended: "vault".into(),
                    cost_of_reversal: CostOfReversal::Low,
                    cost_note: None,
                    needed_before: vec!["self".into()],
                    path: vec![DecisionPathEntry {
                        task_id: task.id,
                        title: "root".into(),
                        stage: None,
                        unit: None,
                    }],
                    raised_by: DecisionRaisedBy {
                        task_id: task.id,
                        run_id: None,
                        origin: DecisionOrigin::Planner,
                    },
                    status: DecisionStatus::Open,
                    answer: None,
                    withdrawn_reason: None,
                }),
            },
        )
        .expect("raise");
}

fn integration_request() -> IntegrationRequest {
    IntegrationRequest {
        target_branch: "main".into(),
        target_sha: "target-head".into(),
        source_branch: "feature".into(),
        source_sha: "source-head".into(),
        merge_base: None,
        conflict_files: vec!["src/lib.rs".into()],
        intent: vec![],
        reason: "conflict".into(),
        recommendation: "resolve both sides".into(),
        actions: vec![],
        candidate_sha: None,
    }
}

fn plan_gate_task(f: &Fixture) -> task_core::Task {
    let task = f.task(TaskKind::Execute, Status::Ready);
    f.env
        .store
        .apply_transition(task.id, Trigger::Dispatch, None)
        .expect("dispatch");
    f.env
        .store
        .apply_transition_with_events(
            task.id,
            Trigger::PlanGate {
                plan_id: "plan-1".into(),
            },
            vec![Event::PlanApprovalRequested {
                plan_id: "plan-1".into(),
                reasons: vec!["review_human:phase-2".into()],
            }],
        )
        .expect("plan gate");
    task
}

fn phase_gate_task(f: &Fixture) -> task_core::Task {
    let task = f.task(TaskKind::Execute, Status::Ready);
    f.env
        .store
        .apply_transition(task.id, Trigger::Dispatch, None)
        .expect("dispatch");
    f.env
        .store
        .apply_transition_with_events(
            task.id,
            Trigger::PhaseGate {
                phase: "design".into(),
            },
            vec![Event::PhaseReported {
                phase: "design".into(),
                report: Box::new(task_core::PhaseReport {
                    phase: "design".into(),
                    phase_title: "設計".into(),
                    next_phase: Some("build".into()),
                    ..Default::default()
                }),
            }],
        )
        .expect("phase gate");
    task
}

/// 統合の状態の行がある終わった task と、その repo の名前。
fn integrated_task(f: &Fixture, state: TaskRepoIntegrationStatus) -> (task_core::Task, String) {
    let now = OffsetDateTime::now_utc();
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: f.project,
        name: "main".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(f.env.dir.path().join("repo")),
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        auto_integrate: false,
        created_at: now,
    };
    f.env.store.repo_create(&repo).expect("repo");
    let task = f.task(TaskKind::Execute, Status::Done);
    f.env
        .store
        .task_integration_upsert(&TaskRepoIntegrationState {
            task_id: task.id,
            repo_id: repo.id,
            branch: "task-branch".into(),
            head_sha: "abc".into(),
            state,
            reason: None,
            actor: "system".into(),
            checked_at: now,
            updated_at: now,
        })
        .expect("integration state");
    (task, repo.name)
}

impl Fixture {
    /// `answer` の前提を作り、人の要求 `(path, 本文)` を返す。受信箱に出る種類は受信箱から答える。
    async fn prepare(&self, answer: Answer) -> (String, Value) {
        let inbox = |id: String| format!("/api/v1/inbox/items/{id}/answer");
        match answer {
            Answer::Question => {
                let task = new_task_in(self, TaskKind::Execute, Status::Blocked);
                self.env.seed_with(
                    &task,
                    vec![Event::WorkerFinished {
                        run_id: ulid::Ulid::new().to_string(),
                        outcome: "question: which database?".into(),
                        usage: None,
                        role: None,
                        metrics: None,
                        end: None,
                    }],
                );
                (
                    inbox(self.inbox_item("question").await),
                    json!({"option": "answer", "note": "SQLite"}),
                )
            }
            Answer::Decision => {
                let task = self.task(TaskKind::Execute, Status::Ready);
                raise_decision(&self.env, &task, "dec-human");
                (
                    inbox(self.inbox_item("decision").await),
                    json!({"option": "manual"}),
                )
            }
            Answer::Draft => {
                self.task(TaskKind::Execute, Status::Draft);
                (
                    inbox(self.inbox_item("draft_accept").await),
                    json!({"option": "accept"}),
                )
            }
            Answer::Failed => {
                self.task(TaskKind::Execute, Status::Failed);
                (
                    inbox(self.inbox_item("failed").await),
                    json!({"option": "retry"}),
                )
            }
            Answer::IntegrationRequest => {
                let task = self.task(TaskKind::Execute, Status::Running);
                assert!(
                    self.env
                        .store
                        .integration_request_record(task.id, &integration_request(), "delivery")
                        .expect("record")
                );
                (
                    inbox(self.inbox_item("integration_request").await),
                    json!({"option": "integrated", "note": "手で merge した"}),
                )
            }
            Answer::PlanGate => {
                plan_gate_task(self);
                (
                    inbox(self.inbox_item("plan_gate").await),
                    json!({"option": "approve"}),
                )
            }
            Answer::PlanGateWithdraw => {
                plan_gate_task(self);
                (
                    inbox(self.inbox_item("plan_gate").await),
                    json!({"option": "withdraw"}),
                )
            }
            Answer::PhaseGate => {
                phase_gate_task(self);
                (
                    inbox(self.inbox_item("phase_gate").await),
                    json!({"option": "continue"}),
                )
            }
            Answer::PhaseGateWithdraw => {
                phase_gate_task(self);
                (
                    inbox(self.inbox_item("phase_gate").await),
                    json!({"option": "withdraw"}),
                )
            }
            // 受け入れ確認・認可・rereview・担当の指定・統合は、受信箱の項目が委ねる先の route を直に呼ぶ。
            Answer::Approve => {
                let task = self.task(TaskKind::Approval, Status::Ready);
                (format!("/api/v1/tasks/{}/approve", task.id), json!({}))
            }
            Answer::Reject => {
                let task = self.task(TaskKind::Approval, Status::Ready);
                (format!("/api/v1/tasks/{}/reject", task.id), json!({}))
            }
            Answer::AuthorizationDecide => {
                let task = self.task(TaskKind::Execute, Status::Blocked);
                let approval = Approval {
                    id: ApprovalId::new(),
                    project_id: Some(self.project),
                    node_id: "cos".into(),
                    task_id: Some(task.id),
                    question: "使ってよいですか".into(),
                    decision: None,
                    answer: None,
                    created_at: OffsetDateTime::now_utc(),
                    decided_at: None,
                };
                self.env.store.approval_append(&approval).expect("approval");
                (
                    format!("/api/v1/approvals/{}/decide", approval.id),
                    json!({"decision": "once", "answer": "使ってよい"}),
                )
            }
            Answer::Rereview => {
                let task = self.task(TaskKind::Execute, Status::Reviewing);
                self.env
                    .store
                    .append_event(
                        task.id,
                        &Event::ReviewResyncHalted {
                            review_run: "run-review".into(),
                            repo_id: RepoId::new(),
                            reviewed_sha: "a".repeat(40),
                            target_sha: "b".repeat(40),
                            resyncs: 3,
                        },
                    )
                    .expect("halted");
                (format!("/api/v1/tasks/{}/rereview", task.id), json!({}))
            }
            Answer::Unroutable => {
                let task = self.task(TaskKind::Execute, Status::Ready);
                (
                    format!("/api/v1/tasks/{}", task.id),
                    json!({"title": "担当を直す"}),
                )
            }
            Answer::IntegrationHold => {
                let (task, repo) = integrated_task(self, TaskRepoIntegrationStatus::Unintegrated);
                (
                    format!("/api/v1/tasks/{}/integrations/{repo}/hold", task.id),
                    json!({"reason": "待つ"}),
                )
            }
            Answer::IntegrationDiscard => {
                let (task, repo) = integrated_task(self, TaskRepoIntegrationStatus::Unintegrated);
                (
                    format!("/api/v1/tasks/{}/integrations/{repo}/discard", task.id),
                    json!({"reason": "要らない"}),
                )
            }
            Answer::IntegrationReopen => {
                let (task, repo) = integrated_task(self, TaskRepoIntegrationStatus::Held);
                (
                    format!("/api/v1/tasks/{}/integrations/{repo}/reopen", task.id),
                    Value::Null,
                )
            }
        }
    }

    async fn call(
        &self,
        answer: Answer,
        request: &(String, Value),
        token: &[(&str, &str)],
    ) -> Resp {
        let (path, body) = request;
        let built = match answer {
            Answer::Unroutable => patch_json_with(path, body, token),
            _ => post_json_with(path, body, token),
        };
        send(&self.app, built).await
    }
}

fn new_task_in(f: &Fixture, kind: TaskKind, status: Status) -> task_core::Task {
    let mut task = new_task(kind, status);
    task.project_id = Some(f.project);
    task
}

/// 人の回答は経路ごとに epoch +1・wake 1 件（委ね先の後始末で二重に数えない）。
#[tokio::test]
async fn project_wake_api_human_inbox_each_answer_bumps_once() {
    for answer in ANSWERS {
        let f = Fixture::new("shadow").await;
        let request = f.prepare(answer).await;
        let resp = f.call(answer, &request, &WEB).await;
        assert!(resp.status.is_success(), "{answer:?}: {}", resp.text());
        f.assert_counted_once(&format!("{answer:?}"));
    }
}

/// `human_epoch` の書き込みが落ちれば、回答も残らない（同じ transaction）。
#[tokio::test]
async fn project_wake_api_human_inbox_bump_failure_rolls_the_answer_back() {
    for answer in ANSWERS {
        // 取り下げの監査 event は遷移の前に別の transaction で書く（ADR 付記 A1 の 9）。下の試験で epoch だけ見る。
        if matches!(answer, Answer::PlanGateWithdraw) {
            continue;
        }
        let f = Fixture::new("shadow").await;
        let request = f.prepare(answer).await;
        let before = f.domain_rows();
        f.refuse_wakes();
        let resp = f.call(answer, &request, &WEB).await;
        assert!(!resp.status.is_success(), "{answer:?}: {}", resp.text());
        f.allow_wakes();
        f.assert_not_counted(&format!("{answer:?}"));
        assert_eq!(f.domain_rows(), before, "{answer:?}");
    }
}

#[tokio::test]
async fn project_wake_api_human_inbox_plan_withdraw_failure_keeps_the_epoch() {
    let f = Fixture::new("shadow").await;
    let request = f.prepare(Answer::PlanGateWithdraw).await;
    f.refuse_wakes();
    let resp = f.call(Answer::PlanGateWithdraw, &request, &WEB).await;
    assert!(!resp.status.is_success(), "{}", resp.text());
    f.allow_wakes();
    f.assert_not_counted("withdraw");
}

/// 検査で断られた回答は数えない。
#[tokio::test]
async fn project_wake_api_human_inbox_refused_answers_do_not_bump() {
    let f = Fixture::new("shadow").await;
    let task = f.task(TaskKind::Execute, Status::Ready);
    raise_decision(&f.env, &task, "dec-human");
    let id = f.inbox_item("decision").await;
    let resp = f
        .post(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option": "nope"}),
            &WEB,
        )
        .await;
    assert!(!resp.status.is_success(), "{}", resp.text());
    f.assert_not_counted("unknown option");
    let resp = f
        .post(
            "/api/v1/inbox/items/question-gone/answer",
            &json!({"option": "answer", "note": "x"}),
            &WEB,
        )
        .await;
    assert!(!resp.status.is_success(), "{}", resp.text());
    f.assert_not_counted("gone");
}

/// operator の回答、案件を持たない task・案件の行が無い task への回答は数えない。
#[tokio::test]
async fn project_wake_api_human_inbox_operator_or_projectless_is_not_counted() {
    let f = Fixture::new("shadow").await;
    let task = f.task(TaskKind::Execute, Status::Ready);
    raise_decision(&f.env, &task, "dec-human");
    let id = f.inbox_item("decision").await;
    let resp = f
        .post(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option": "manual"}),
            &OPERATOR,
        )
        .await;
    assert!(resp.status.is_success(), "{}", resp.text());
    f.assert_not_counted("operator");

    let f = Fixture::new("shadow").await;
    let lone = new_task(TaskKind::Execute, Status::Ready);
    f.env.seed(&lone);
    raise_decision(&f.env, &lone, "dec-lone");
    let id = f.inbox_item("decision").await;
    let resp = f
        .post(
            &format!("/api/v1/inbox/items/{id}/answer"),
            &json!({"option": "manual"}),
            &WEB,
        )
        .await;
    assert!(resp.status.is_success(), "{}", resp.text());
    f.assert_not_counted("projectless");
}

/// CoS の `inbox.answer` と `/cos/inbox/{id}/resolve` は数えない。
#[tokio::test]
async fn project_wake_api_human_inbox_cos_answer_and_resolve_are_not_counted() {
    let f = Fixture::new("shadow").await;
    let task = f.task(TaskKind::Execute, Status::Ready);
    raise_decision(&f.env, &task, "dec-cos");
    let id = f.inbox_item("decision").await;
    let (bearer, said) = f.run(&f.thread.id, "cos-answer");
    let token = [("authorization", bearer.as_str())];
    let mut op = with_policy(op_body(
        "cos-answer",
        "POST",
        &format!("/api/v1/inbox/items/{id}/answer"),
        json!({"option": "manual", "note": "人の指示どおり"}),
    ));
    op["instructed_by"] = json!(said);
    let resp = f.post(OPS, &op, &token).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(
        resp.json()["operation"]["state"],
        "applied",
        "{}",
        resp.text()
    );
    f.assert_not_counted("CoS inbox.answer");
    f.env
        .store
        .chat_run_finish(
            "run-cos-answer",
            task_core::chat::ChatRunState::Completed,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .expect("finish");

    let now = OffsetDateTime::now_utc();
    let task = f.task(TaskKind::Execute, Status::Ready);
    raise_decision(&f.env, &task, "dec-other");
    let source = task_core::chat::triage::CosTriageSource {
        source_kind: "decision".into(),
        source_key: "decision-dec-other".into(),
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: "判断".into(),
        detail: String::new(),
        policy_version: "5".into(),
        task_id: Some(task.id.to_string()),
    };
    f.env
        .store
        .cos_triage_ingest_batch("decision", "w-1", &[source], now)
        .expect("ingest");
    let claim = f
        .env
        .store
        .cos_triage_claim(&f.thread.id, "run-triage", now)
        .expect("claim")
        .expect("triage run");
    let bearer = task_api::cos::issue_run_bearer(
        &f.env.store,
        &f.thread.id,
        "run-triage",
        Duration::hours(1),
    )
    .expect("issue");
    f.env
        .store
        .chat_run_record_policy("run-triage", "5", "test")
        .expect("policy");
    let bearer = format!("Bearer {bearer}");
    let resp = f
        .post(
            &format!("/api/v1/cos/inbox/{}/resolve", claim.item_ids[0]),
            &json!({
                "idempotency_key": "esc-1",
                "expected_revision": "r1",
                "outcome": "escalate",
                "reason": "人に聞く",
                "policy_version": "5",
                "escalation": {
                    "to": "human", "summary": "決める",
                    "options": [{"key": "vault", "label": "vault"}, {"key": "manual", "label": "manual"}],
                    "recommended": "vault",
                    "reserved_options": ["vault", "manual"],
                    "recommendation_reason": "既定",
                    "web_path": format!("/tasks/{}", task.id),
                },
            }),
            &[("authorization", bearer.as_str())],
        )
        .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    f.assert_not_counted("cos/inbox resolve");
}
