mod common;

use common::*;
use serde_json::json;
use task_core::{
    Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun, Status, TaskId,
    TaskKind, TaskRepoIntegrationState, TaskRepoIntegrationStatus, TaskStore, WorkspaceSpec,
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
        auto_integrate: false,
        slug: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
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
    // ADR 2026-10-21: 既定の一覧は付けない。`with=integrations` のときだけ付く。
    let plain = send(&env.router(), get_with("/api/v1/tasks", &admin())).await;
    assert_eq!(plain.status, 200, "{}", plain.text());
    assert!(
        plain.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("integrations").is_none() && item.get("promotion").is_none())
    );
    let list = send(
        &env.router(),
        get_with("/api/v1/tasks?with=integrations", &admin()),
    )
    .await;
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

fn fixture_repo(
    project: &Project,
    name: &str,
    path: std::path::PathBuf,
    now: time::OffsetDateTime,
) -> ProjectRepo {
    ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: name.into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(path),
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::Auto,
        is_primary: false,
        created_at: now,
    }
}

fn unintegrated_items_for(inbox: &serde_json::Value, task: TaskId) -> Vec<serde_json::Value> {
    inbox["items"]
        .as_array()
        .expect("inbox items")
        .iter()
        .filter(|item| item["kind"] == "unintegrated" && item["task"]["id"] == task.to_string())
        .cloned()
        .collect()
}

#[tokio::test]
async fn integration_api_inbox_unintegrated_items_are_per_repo_and_need_note() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let now = time::OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    env.store.project_create(&project).expect("project");
    let repo_a = fixture_repo(&project, "repo-a", env.dir.path().join("repo-a"), now);
    let repo_b = fixture_repo(&project, "repo-b", env.dir.path().join("repo-b"), now);
    env.store.repo_create(&repo_a).expect("repo a");
    env.store.repo_create(&repo_b).expect("repo b");
    let mut task = new_task(TaskKind::Execute, Status::Done);
    task.project_id = Some(project.id);
    env.seed(&task);
    for repo in [&repo_a, &repo_b] {
        env.store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id: task.id,
                repo_id: repo.id,
                branch: format!("task-{}", repo.name),
                head_sha: format!("head-{}", repo.name),
                state: TaskRepoIntegrationStatus::Unintegrated,
                reason: None,
                actor: "system".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("integration state");
    }

    // 1 task × 未取り込み repo 2 つ → 受信箱には repo ごとに ID の異なる item が 2 つ出る。
    let inbox = send(&env.router(), get_with("/api/v1/inbox/items", &admin())).await;
    assert_eq!(inbox.status, 200, "{}", inbox.text());
    let items = unintegrated_items_for(&inbox.json(), task.id);
    assert_eq!(items.len(), 2, "{}", inbox.text());
    assert_ne!(items[0]["id"], items[1]["id"]);
    let item_for = |repo: &str| {
        items
            .iter()
            .find(|item| {
                item["answer"]["native"]["path"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&format!("/integrations/{repo}/"))
            })
            .cloned()
            .unwrap_or_else(|| panic!("no item for {repo}: {items:?}"))
    };
    let item_a = item_for("repo-a");
    let item_b = item_for("repo-b");
    for item in [&item_a, &item_b] {
        let discard = item["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["key"] == "discard");
        assert_eq!(discard.expect("discard option")["needs_note"], true);
    }

    // 理由なし・空白だけの discard/hold は 400 で、状態は変わらない。
    let answer = format!(
        "/api/v1/inbox/items/{}/answer",
        item_b["id"].as_str().unwrap()
    );
    for body in [
        json!({"option": "discard"}),
        json!({"option": "discard", "note": "  "}),
        json!({"option": "hold", "note": ""}),
    ] {
        let refused = send(&env.router(), post_json_with(&answer, &body, &admin())).await;
        assert_eq!(refused.status, 400, "{body}: {}", refused.text());
    }
    let states = send(
        &env.router(),
        get_with(&format!("/api/v1/tasks/{}/integrations", task.id), &admin()),
    )
    .await;
    assert_eq!(states.status, 200, "{}", states.text());
    let states = states.json();
    for item in states["items"].as_array().unwrap() {
        assert_eq!(item["state"], "unintegrated", "{states}");
        assert!(item["reason"].is_null(), "{states}");
    }

    // 2 つ目の item の discard（理由付き）は repo-b だけを discarded にする。
    let discarded = send(
        &env.router(),
        post_json_with(
            &answer,
            &json!({"option": "discard", "note": "重複 branch"}),
            &admin(),
        ),
    )
    .await;
    assert_eq!(discarded.status, 200, "{}", discarded.text());
    let states = send(
        &env.router(),
        get_with(&format!("/api/v1/tasks/{}/integrations", task.id), &admin()),
    )
    .await;
    let states = states.json();
    let state_of = |repo: RepoId| {
        states["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["repo_id"] == repo.to_string())
            .cloned()
            .unwrap()
    };
    assert_eq!(state_of(repo_b.id)["state"], "discarded", "{states}");
    assert_eq!(state_of(repo_b.id)["reason"], "重複 branch", "{states}");
    assert_eq!(state_of(repo_a.id)["state"], "unintegrated", "{states}");
    let events = env.store.events_for(task.id).expect("events");
    assert!(
        events.iter().any(|(_, event)| matches!(
            event,
            task_core::Event::IntegrationDiscarded { repo_id, .. } if *repo_id == repo_b.id
        )),
        "discard event for repo-b missing"
    );
    assert!(!events.iter().any(|(_, event)| matches!(
        event,
        task_core::Event::IntegrationDiscarded { repo_id, .. } if *repo_id == repo_a.id
    )));

    // 残った repo-a の item は別 item のまま。discard 済みの repo-b は受信箱から消える。
    let inbox = send(&env.router(), get_with("/api/v1/inbox/items", &admin())).await;
    let remaining = unintegrated_items_for(&inbox.json(), task.id);
    assert_eq!(remaining.len(), 1, "{}", inbox.text());
    assert_eq!(remaining[0]["id"], item_a["id"]);
}
