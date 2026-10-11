mod common;

use common::*;
use serde_json::json;
use task_core::{
    Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun, Status, TaskKind,
    TaskRepoIntegrationState, TaskRepoIntegrationStatus, TaskStore, WorkspaceSpec,
};

fn admin() -> [(&'static str, &'static str); 1] {
    [("authorization", "Bearer s3cret-token-value")]
}

#[tokio::test]
async fn integration_api_lists_and_filters_states() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let response = send(
        &env.router(),
        get_with("/api/v1/integrations?state=unintegrated", &admin()),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.json()["items"], json!([]));
}

#[tokio::test]
async fn integration_api_disposition_requires_human_auth_and_nonempty_reason() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let path = format!(
        "/api/v1/tasks/{}/integrations/repo/discard",
        task_core::TaskId::new()
    );
    let denied = send(&env.router(), post_json(&path, &json!({"reason":"x"}))).await;
    assert_eq!(denied.status, 401);
    let empty = send(
        &env.router(),
        post_json_with(&path, &json!({"reason":"  "}), &admin()),
    )
    .await;
    assert_eq!(empty.status, 400, "{}", empty.text());
}

#[tokio::test]
async fn integration_api_rejects_unknown_state_filter() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let response = send(
        &env.router(),
        get_with("/api/v1/integrations?state=unknown", &admin()),
    )
    .await;
    assert_eq!(response.status, 400, "{}", response.text());
}

#[tokio::test]
async fn integration_api_task_detail_and_list_include_unintegrated_state() {
    let releases = tempfile::tempdir().expect("releases dir");
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        integration_releases_dir: Some(releases.path().to_path_buf()),
        ..EnvOptions::default()
    });
    let now = time::OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    env.store.project_create(&project).expect("project");
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "agent-platform".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(env.dir.path().join("repo")),
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    env.store.repo_create(&repo).expect("repo");
    let repository_path = env.dir.path().join("repo");
    std::fs::create_dir_all(&repository_path).expect("repository dir");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repository_path)
            .args(args)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(repository_path.join("README"), "fixture\n").expect("fixture file");
    for args in [vec!["add", "README"], vec!["commit", "-m", "fixture"]] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repository_path)
            .args(args)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&repository_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse");
    assert!(output.status.success());
    let head = String::from_utf8(output.stdout)
        .expect("sha")
        .trim()
        .to_owned();
    let release = releases.path().join(&head[..12]);
    std::fs::create_dir_all(&release).expect("release dir");
    std::fs::write(
        release.join("manifest.json"),
        json!({"sha": head}).to_string(),
    )
    .expect("manifest");
    std::fs::write(
        release.join("promoted.json"),
        json!({"promoted_at": "2026-10-11T00:00:00Z"}).to_string(),
    )
    .expect("promoted marker");
    let mut task = new_task(TaskKind::Execute, Status::Done);
    task.project_id = Some(project.id);
    env.seed(&task);
    env.store
        .task_integration_upsert(&TaskRepoIntegrationState {
            task_id: task.id,
            repo_id: repo.id,
            branch: "task-branch".into(),
            head_sha: head.clone(),
            state: TaskRepoIntegrationStatus::Unintegrated,
            reason: None,
            actor: "system".into(),
            checked_at: now,
            updated_at: now,
        })
        .expect("integration state");

    let detail = send(
        &env.router(),
        get_with(&format!("/api/v1/tasks/{}", task.id), &admin()),
    )
    .await;
    assert_eq!(detail.status, 200, "{}", detail.text());
    assert_eq!(detail.json()["integrations"][0]["state"], "unintegrated");
    let list = send(&env.router(), get_with("/api/v1/tasks", &admin())).await;
    assert_eq!(list.status, 200, "{}", list.text());
    assert!(list.json()["items"].as_array().unwrap().iter().any(|item| {
        item["id"] == task.id.to_string() && item["integrations"][0]["state"] == "unintegrated"
    }));

    env.store
        .task_integration_upsert(&TaskRepoIntegrationState {
            task_id: task.id,
            repo_id: repo.id,
            branch: "main".into(),
            head_sha: head,
            state: TaskRepoIntegrationStatus::Integrated,
            reason: None,
            actor: "human".into(),
            checked_at: now,
            updated_at: now,
        })
        .expect("integrated state");
    let integrated = send(
        &env.router(),
        get_with(&format!("/api/v1/tasks/{}", task.id), &admin()),
    )
    .await;
    assert_eq!(integrated.status, 200, "{}", integrated.text());
    assert_eq!(integrated.json()["integrations"][0]["state"], "integrated");
    assert_eq!(integrated.json()["promotion"]["status"], "promoted");
}
