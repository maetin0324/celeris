//! ADR 2026-11-04-project-controller D12・I7: 人の操作と `human_epoch` は同じ transaction で書く。数えるのは
//! `Principal::Human` の要求だけ（接頭辞 `project_wake_api_human_`）。
mod common;

use common::cos_ops::{OPS, op_body};
use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::chat::{ChatPostMessageRequest, ChatSendMode};
use task_core::{ProjectId, ProjectStatus, Status, TaskKind};
use time::{Duration, OffsetDateTime};

const WEB_TOKEN: &str = "web-human-token";
const OPERATOR_TOKEN: &str = "operator-token";
const MCP_TOKEN: &str = "mcp-daemon-token";

/// 経路（D12 の案件・task の操作）。
#[derive(Debug, Clone, Copy)]
enum Route {
    Patch,
    Pause,
    Resume,
    ProjectCancel,
    Archive,
    Unarchive,
    Create,
    TaskCancel,
    Retry,
    Reopen,
}

const ROUTES: [Route; 10] = [
    Route::Patch,
    Route::Pause,
    Route::Resume,
    Route::ProjectCancel,
    Route::Archive,
    Route::Unarchive,
    Route::Create,
    Route::TaskCancel,
    Route::Retry,
    Route::Reopen,
];

struct Fixture {
    env: TestEnv,
    app: axum::Router,
    thread: task_core::chat::ChatThread,
    project: ProjectId,
}

impl Fixture {
    fn off() -> Self {
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
            mcp_token: Some(MCP_TOKEN.into()),
            ..EnvOptions::default()
        });
        let thread = project_cos_thread(&env.store, "pilot:human-epoch");
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
        Self {
            env,
            app,
            thread,
            project,
        }
    }

    /// `shadow` の案件（`human_epoch` を数える状態）。
    async fn shadow() -> Self {
        let fixture = Self::off();
        let resp = send(
            &fixture.app,
            put_json_with(
                &format!("/api/v1/projects/{}/controller", fixture.project),
                &json!({"mode": "shadow", "expected_revision": 0, "reason": "人が試す"}),
                &admin_headers(),
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        fixture
    }

    fn task(&self, status: Status) -> task_core::Task {
        let mut task = new_task(TaskKind::Execute, status);
        task.project_id = Some(self.project);
        self.env.seed(&task);
        task
    }

    fn set_project_status(&self, status: ProjectStatus) {
        use task_core::TaskStore;
        assert!(
            self.env
                .store
                .project_set_status(self.project, status)
                .expect("status")
        );
    }

    /// `route` の前提を作り、`(method, path, body)` を返す。
    fn prepare(&self, route: Route) -> (&'static str, String, Value) {
        use task_core::TaskStore;
        let project = self.project;
        match route {
            Route::Patch => (
                "PATCH",
                format!("/api/v1/projects/{project}"),
                json!({"title": "pilot:renamed"}),
            ),
            Route::Pause => (
                "POST",
                format!("/api/v1/projects/{project}/pause"),
                json!({}),
            ),
            Route::Resume => {
                self.env
                    .store
                    .project_set_lifecycle(
                        project,
                        ProjectStatus::Paused,
                        Some(Some(ProjectStatus::Active)),
                    )
                    .expect("paused");
                (
                    "POST",
                    format!("/api/v1/projects/{project}/resume"),
                    json!({}),
                )
            }
            Route::ProjectCancel => (
                "POST",
                format!("/api/v1/projects/{project}/cancel"),
                json!({}),
            ),
            Route::Archive => {
                self.set_project_status(ProjectStatus::Cancelled);
                (
                    "POST",
                    format!("/api/v1/projects/{project}/archive"),
                    json!({}),
                )
            }
            Route::Unarchive => {
                self.set_project_status(ProjectStatus::Cancelled);
                self.env
                    .store
                    .project_set_archived_at(project, Some(OffsetDateTime::now_utc()))
                    .expect("archived");
                (
                    "POST",
                    format!("/api/v1/projects/{project}/unarchive"),
                    json!({}),
                )
            }
            Route::Create => {
                let mut body = valid_task_body();
                body["project_id"] = json!(project.to_string());
                body["repos"] = json!([]);
                ("POST", "/api/v1/tasks".to_string(), body)
            }
            Route::TaskCancel => (
                "POST",
                format!("/api/v1/tasks/{}/cancel", self.task(Status::Ready).id),
                json!({}),
            ),
            Route::Retry => (
                "POST",
                format!("/api/v1/tasks/{}/retry", self.task(Status::Failed).id),
                json!({}),
            ),
            Route::Reopen => (
                "POST",
                format!("/api/v1/tasks/{}/reopen", self.task(Status::Done).id),
                json!({}),
            ),
        }
    }

    async fn call(&self, request: &(&str, String, Value), token: &[(&str, &str)]) -> Resp {
        let (method, path, body) = request;
        let built = match *method {
            "PATCH" => patch_json_with(path, body, token),
            _ => post_json_with(path, body, token),
        };
        send(&self.app, built).await
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

    fn sql(&self, sql: &str) {
        rusqlite::Connection::open(&self.env.db_path)
            .expect("open")
            .execute_batch(sql)
            .expect("sql");
    }

    /// 案件と task の行の全部（失敗した要求が何も残していないことを比べる）。
    fn domain_rows(&self) -> String {
        rusqlite::Connection::open(&self.env.db_path)
            .expect("open")
            .query_row(
                "SELECT (SELECT json_group_array(json_array(id, title, status, archived_at, paused_from)) \
                 FROM projects), (SELECT json_group_array(json) FROM tasks), \
                 (SELECT COUNT(*) FROM events), (SELECT COUNT(*) FROM task_idempotency_keys)",
                [],
                |row| {
                    Ok(format!(
                        "{}|{}|{}|{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?
                    ))
                },
            )
            .expect("rows")
    }

    /// 走っている CoS run の bearer（案件 thread）。
    fn cos_bearer(&self, key: &str) -> String {
        let now = OffsetDateTime::now_utc();
        let store = &self.env.store;
        store
            .chat_message_post(
                &self.thread.id,
                &ChatPostMessageRequest {
                    client_message_id: format!("message-{key}"),
                    text: "input".into(),
                    attachment_ids: vec![],
                    reply_to_id: None,
                    mode: ChatSendMode::Queue,
                    resume_queue: false,
                },
                now,
            )
            .expect("message");
        let run = format!("run-{key}");
        store
            .chat_run_claim_next(&self.thread.id, &run, &json!({}), now)
            .expect("claim")
            .expect("run");
        let bearer =
            task_api::cos::issue_run_bearer(store, &self.thread.id, &run, Duration::hours(1))
                .expect("issue");
        store
            .chat_run_record_policy(&run, "1", "test")
            .expect("policy");
        format!("Bearer {bearer}")
    }
}

fn valid_task_body() -> Value {
    json!({"title": "t", "objective": "o", "acceptance": [{"type": "command", "cmd": "true"}]})
}

fn web() -> [(&'static str, String); 1] {
    [("authorization", format!("Bearer {WEB_TOKEN}"))]
}

fn headers(owned: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
    owned.to_vec()
}

fn refs<'a>(owned: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    owned.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[tokio::test]
async fn project_wake_api_human_each_route_bumps_once() {
    for route in ROUTES {
        let f = Fixture::shadow().await;
        let request = f.prepare(route);
        let token = headers(&web());
        let resp = f.call(&request, &refs(&token)).await;
        assert!(resp.status.is_success(), "{route:?}: {}", resp.text());
        assert_eq!(f.epoch(), 1, "{route:?}");
        assert_eq!(f.wake_keys(), vec!["human:1".to_string()], "{route:?}");
    }
}

/// 書き込みが transaction の中で失敗したら `human_epoch` も wake も残らない。
#[tokio::test]
async fn project_wake_api_human_write_failure_keeps_the_epoch() {
    for route in ROUTES {
        let f = Fixture::shadow().await;
        let request = f.prepare(route);
        let before = f.domain_rows();
        let event = match route {
            Route::Create | Route::Retry => "INSERT ON tasks",
            Route::TaskCancel | Route::Reopen => "UPDATE ON tasks",
            _ => "UPDATE ON projects",
        };
        f.sql(&format!(
            "CREATE TRIGGER refuse_write BEFORE {event} BEGIN SELECT RAISE(ABORT, 'refused'); END;"
        ));
        let token = headers(&web());
        let resp = f.call(&request, &refs(&token)).await;
        assert!(!resp.status.is_success(), "{route:?}: {}", resp.text());
        f.sql("DROP TRIGGER refuse_write");
        assert_eq!(f.epoch(), 0, "{route:?}");
        assert!(f.wake_keys().is_empty(), "{route:?}");
        assert_eq!(f.domain_rows(), before, "{route:?}");
    }
}

/// 増やす側が失敗したら書き込みも残らない（同じ transaction であることの証拠）。
#[tokio::test]
async fn project_wake_api_human_bump_failure_rolls_the_write_back() {
    for route in ROUTES {
        let f = Fixture::shadow().await;
        let request = f.prepare(route);
        let before = f.domain_rows();
        f.sql(
            "CREATE TRIGGER refuse_wake BEFORE INSERT ON project_wakes \
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        );
        let token = headers(&web());
        let resp = f.call(&request, &refs(&token)).await;
        assert!(!resp.status.is_success(), "{route:?}: {}", resp.text());
        f.sql("DROP TRIGGER refuse_wake");
        assert_eq!(f.epoch(), 0, "{route:?}");
        assert_eq!(f.domain_rows(), before, "{route:?}");
    }
}

/// 状態の検査で断られた要求は数えない。何も変えない archive の繰り返しも数えない。
#[tokio::test]
async fn project_wake_api_human_refused_or_unchanged_requests_do_not_bump() {
    let f = Fixture::shadow().await;
    let token = headers(&web());
    let done = f.task(Status::Done);
    let ready = f.task(Status::Ready);
    for (path, code) in [
        (format!("/api/v1/tasks/{}/cancel", done.id), 409),
        (format!("/api/v1/tasks/{}/reopen", ready.id), 409),
        (format!("/api/v1/tasks/{}/retry", ready.id), 409),
        (format!("/api/v1/projects/{}/resume", f.project), 409),
        (format!("/api/v1/projects/{}/archive", f.project), 409),
    ] {
        let resp = f
            .call(&("POST", path.clone(), json!({})), &refs(&token))
            .await;
        assert_eq!(resp.status.as_u16(), code, "{path}: {}", resp.text());
    }
    let resp = f
        .call(
            &(
                "POST",
                format!("/api/v1/projects/{}/unarchive", f.project),
                json!({}),
            ),
            &refs(&token),
        )
        .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(f.epoch(), 0);
    assert!(f.wake_keys().is_empty());
}

#[tokio::test]
async fn project_wake_api_human_operator_is_not_counted() {
    for route in ROUTES {
        let f = Fixture::shadow().await;
        let request = f.prepare(route);
        let token = [("authorization", format!("Bearer {OPERATOR_TOKEN}"))];
        let resp = f.call(&request, &refs(&token)).await;
        assert!(resp.status.is_success(), "{route:?}: {}", resp.text());
        assert_eq!(f.epoch(), 0, "{route:?}");
        assert!(f.wake_keys().is_empty(), "{route:?}");
    }
}

#[tokio::test]
async fn project_wake_api_human_mcp_is_not_counted() {
    for route in ROUTES {
        let f = Fixture::shadow().await;
        let request = f.prepare(route);
        let token = [
            ("authorization", format!("Bearer {MCP_TOKEN}")),
            ("x-celeris-mcp-client", "01MCPCLIENTA".to_string()),
        ];
        let resp = f.call(&request, &refs(&token)).await;
        assert_eq!(resp.status.as_u16(), 403, "{route:?}: {}", resp.text());
        assert_eq!(f.epoch(), 0, "{route:?}");
        assert!(f.wake_keys().is_empty(), "{route:?}");
    }
}

/// CoS の bearer が同じ route を直接呼んでも、`/cos/operations` を通しても数えない。
#[tokio::test]
async fn project_wake_api_human_cos_is_not_counted() {
    for route in ROUTES {
        let f = Fixture::shadow().await;
        let request = f.prepare(route);
        let bearer = f.cos_bearer("direct");
        let token = [("authorization", bearer.clone())];
        let _ = f.call(&request, &refs(&token)).await;
        assert_eq!(f.epoch(), 0, "{route:?} direct");
        assert!(f.wake_keys().is_empty(), "{route:?} direct");
    }
    for route in ROUTES {
        let f = Fixture::shadow().await;
        let (method, path, body) = f.prepare(route);
        let bearer = f.cos_bearer("ops");
        let token = [("authorization", bearer.as_str())];
        let resp = send(
            &f.app,
            post_json_with(OPS, &op_body("human-epoch", method, &path, body), &token),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{route:?}: {}", resp.text());
        assert_eq!(
            resp.json()["operation"]["state"],
            "applied",
            "{route:?}: {}",
            resp.text()
        );
        assert_eq!(f.epoch(), 0, "{route:?} operations");
        assert!(f.wake_keys().is_empty(), "{route:?} operations");
    }
}

/// `off`・行なしの案件と案件の無い task では何も書かない（I13）。
#[tokio::test]
async fn project_wake_api_human_off_or_projectless_is_inert() {
    let f = Fixture::off();
    let token = headers(&web());
    let request = f.prepare(Route::Pause);
    let resp = f.call(&request, &refs(&token)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert!(
        f.env
            .store
            .project_controller_state(&f.project.to_string())
            .unwrap()
            .is_none()
    );

    let f = Fixture::shadow().await;
    let lone = new_task(TaskKind::Execute, Status::Ready);
    f.env.seed(&lone);
    let resp = f
        .call(
            &(
                "POST",
                format!("/api/v1/tasks/{}/cancel", lone.id),
                json!({}),
            ),
            &refs(&token),
        )
        .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(f.epoch(), 0);
    assert!(f.wake_keys().is_empty());
}
