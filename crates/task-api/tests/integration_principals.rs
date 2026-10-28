mod common;

use common::cos_ops::cos_bearer;
use common::*;
use serde_json::json;
use task_api::{ApiToken, Principal};
use task_core::{Event, RepoId, Status, TaskKind, TaskStore};

fn env() -> TestEnv {
    TestEnv::with(EnvOptions {
        extra_tokens: vec![
            ApiToken::new(Principal::Human { name: "web".into() }, "web-token"),
            ApiToken::new(
                Principal::Operator {
                    name: "claude-code".into(),
                },
                "operator-token",
            ),
        ],
        mcp_token: Some("celeris-mcp.test".into()),
        ..Default::default()
    })
}

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn fixture(env: &TestEnv) -> (task_core::Task, RepoId) {
    use task_core::*;
    let now = time::OffsetDateTime::now_utc();
    let project = Project {
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    env.store.project_create(&project).unwrap();
    let path = env.dir.path().join(project.id.to_string());
    std::fs::create_dir(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["commit", "--allow-empty", "-m", "base"]);
    let base = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["checkout", "-b", "task-branch"]);
    std::fs::write(path.join("feature"), "feature").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "feature"]);
    let head = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["checkout", "main"]);
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "repo".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(path.clone()),
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    env.store.repo_create(&repo).unwrap();
    let mut task = new_task(TaskKind::Execute, Status::Done);
    task.project_id = Some(project.id);
    env.seed(&task);
    let source = path.to_string_lossy().into_owned();
    task_ops::workspace::write_marker(
        &env.workspace(&task),
        &task_ops::workspace::WorktreeMarker {
            repo: source.clone(),
            dir: source.clone(),
            branch: "task-branch".into(),
            base: base.clone(),
            base_kind: "main".into(),
            repos: vec![task_ops::workspace::WorktreeMarkerRepo {
                name: "repo".into(),
                kind: "git".into(),
                source: source.clone(),
                dir: source,
                branch: Some("task-branch".into()),
                base: Some(base),
                base_kind: Some("main".into()),
            }],
        },
    )
    .unwrap();
    env.store
        .task_integration_upsert(&TaskRepoIntegrationState {
            task_id: task.id,
            repo_id: repo.id,
            branch: "task-branch".into(),
            head_sha: head,
            state: TaskRepoIntegrationStatus::Unintegrated,
            reason: None,
            actor: "sweep".into(),
            checked_at: now,
            updated_at: now,
        })
        .unwrap();
    (task, repo.id)
}

#[tokio::test]
async fn all_dispositions_and_inbox_forwarding_preserve_token_principals() {
    let env = env();
    let app = env.router();
    for (token, actor) in [
        ("web-token", "human:web"),
        ("operator-token", "operator:claude-code"),
    ] {
        for via_inbox in [false, true] {
            let (task, repo) = fixture(&env);
            let auth = format!("Bearer {token}");
            for (action, expected) in [
                ("hold", "held"),
                ("reopen", "unintegrated"),
                ("discard", "discarded"),
                ("reopen", "unintegrated"),
                ("merge", "integrated"),
            ] {
                // Reopen is not an inbox action; the other dispositions can be delegated by answer.
                let (path, body) = if via_inbox && action != "reopen" {
                    let response = send(
                        &app,
                        get_with("/api/v1/inbox/items", &[("authorization", &auth)]),
                    )
                    .await;
                    let items = response.json();
                    let item = items["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|i| {
                            i["kind"] == "unintegrated" && i["task"]["id"] == task.id.to_string()
                        })
                        .unwrap();
                    (
                        format!(
                            "/api/v1/inbox/items/{}/answer",
                            item["id"].as_str().unwrap()
                        ),
                        json!({"option": action, "note": "理由"}),
                    )
                } else {
                    (
                        format!("/api/v1/tasks/{}/integrations/repo/{action}", task.id),
                        json!({"reason": "理由"}),
                    )
                };
                let response = send(
                    &app,
                    post_json_with(&path, &body, &[("authorization", &auth)]),
                )
                .await;
                assert_eq!(response.status, 200, "{action}: {}", response.text());
                if !via_inbox || action == "reopen" {
                    assert_eq!(response.json()["state"]["actor"], actor);
                }
                let state = env
                    .store
                    .task_integration_get(task.id, repo)
                    .unwrap()
                    .unwrap();
                assert_eq!(state.actor, actor);
                assert_eq!(serde_json::to_value(state).unwrap()["state"], expected);
            }
            let actors: Vec<_> = env
                .store
                .events_for(task.id)
                .unwrap()
                .into_iter()
                .filter_map(|(_, e)| match e {
                    Event::IntegrationMerged { actor, .. }
                    | Event::IntegrationHeld { actor, .. }
                    | Event::IntegrationDiscarded { actor, .. }
                    | Event::IntegrationReopened { actor, .. } => Some(actor),
                    _ => None,
                })
                .collect();
            assert_eq!(actors, vec![actor; 5]);
        }
    }
}

#[tokio::test]
async fn integration_routes_apply_common_guard_without_changing_authorization() {
    let env = env();
    let (task, repo) = fixture(&env);
    let app = env.router();
    let (_, _, cos) = cos_bearer(&env, "integration-denied");
    for action in ["merge", "hold", "discard", "reopen"] {
        let path = format!("/api/v1/tasks/{}/integrations/repo/{action}", task.id);
        for (auth, status) in [
            ("Bearer unknown", 401),
            ("Bearer celeris-mcp.test", 403),
            (cos.as_str(), 422),
        ] {
            let response = send(
                &app,
                post_json_with(
                    &path,
                    &json!({"reason":"x"}),
                    &[("authorization", auth), ("x-celeris-mcp-client", "client")],
                ),
            )
            .await;
            assert_eq!(
                response.status.as_u16(),
                status,
                "{action}: {}",
                response.text()
            );
        }
        let response = send(
            &app,
            post_json_with(
                &path,
                &json!({"reason":"x"}),
                &[
                    ("authorization", "Bearer operator-token"),
                    ("origin", "http://localhost"),
                ],
            ),
        )
        .await;
        assert_eq!(response.status, 403);
    }
    assert_eq!(
        env.store
            .task_integration_get(task.id, repo)
            .unwrap()
            .unwrap()
            .actor,
        "sweep"
    );
    for path in [
        "/api/v1/integrations".into(),
        format!("/api/v1/tasks/{}/integrations", task.id),
    ] {
        assert_eq!(send(&app, get_with(&path, &[])).await.status, 401);
    }
}

#[tokio::test]
async fn integration_request_answers_record_web_and_operator() {
    let env = env();
    let app = env.router();
    for (auth, actor) in [
        ("Bearer web-token".to_string(), "human:web".to_string()),
        (
            "Bearer operator-token".to_string(),
            "operator:claude-code".to_string(),
        ),
    ] {
        let task = new_task(TaskKind::Execute, Status::Done);
        env.seed(&task);
        let request = task_core::integration_request::IntegrationRequest {
            target_branch: "main".into(),
            target_sha: "a".repeat(40),
            source_branch: "task".into(),
            source_sha: "b".repeat(40),
            merge_base: None,
            conflict_files: vec![],
            intent: vec![],
            reason: "conflict".into(),
            recommendation: "integrated".into(),
            actions: vec![],
            candidate_sha: None,
        };
        env.store
            .integration_request_record(task.id, &request, "delivery")
            .unwrap();
        let response = send(
            &app,
            get_with(
                "/api/v1/inbox/items",
                &[("authorization", "Bearer web-token")],
            ),
        )
        .await;
        let value = response.json();
        let item = value["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["kind"] == "integration_request" && i["task"]["id"] == task.id.to_string())
            .unwrap();
        let path = format!(
            "/api/v1/inbox/items/{}/answer",
            item["id"].as_str().unwrap()
        );
        let body = json!({"option":"integrated", "note":"confirmed"});
        let response = send(
            &app,
            post_json_with(&path, &body, &[("authorization", &auth)]),
        )
        .await;
        assert_eq!(response.status, 200, "{}", response.text());
        assert!(env.store.events_for(task.id).unwrap().iter().any(|(_, e)| matches!(e, Event::IntegrationAnswered { by, .. } if by.as_deref() == Some(&actor))));
    }
}
