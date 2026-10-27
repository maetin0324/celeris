//! ADR 2026-10-21-task-list-opt-in-integrations: `GET /tasks` は既定で取り込み状態を付けず（git 起動 0）、
//! `?with=integrations` のときも store の全件読み・repo の解決・release の走査が行数に比例しない。
//! 時計ではなく回数で固定する（git 起動は `task_ops::changes::git_spawns_in`、store は `lock_counts`）。
mod common;
use common::*;
use std::path::{Path, PathBuf};
use task_core::{
    Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun, Status, TaskKind,
    TaskRepoIntegrationState, TaskRepoIntegrationStatus, TaskStore, WorkspaceSpec,
};

const REPOS: usize = 2;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=T", "-c", "user.email=t@example.invalid"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `default_branch` 未設定の repo を `REPOS` 個と、それぞれに 1 行ずつ状態を持つ task を `n` 個作る。
fn seed(env: &TestEnv, n: usize) -> Vec<PathBuf> {
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
    let mut paths = Vec::new();
    let mut repo_ids = Vec::new();
    for i in 0..REPOS {
        let path = env.dir.path().join(format!("repo{i}"));
        std::fs::create_dir_all(&path).expect("dir");
        git(&path, &["init", "-b", "main"]);
        git(&path, &["commit", "--allow-empty", "-m", "x"]);
        let repo = ProjectRepo {
            id: RepoId::new(),
            project_id: project.id,
            name: format!("repo{i}"),
            kind: RepoKind::Git,
            location: WorkspaceSpec::local(path.clone()),
            default_branch: None,
            sync: None,
            run: RepoRun::Auto,
            is_primary: i == 0,
            created_at: now,
        };
        env.store.repo_create(&repo).expect("repo");
        repo_ids.push(repo.id);
        paths.push(path);
    }
    for _ in 0..n {
        let mut task = new_task(TaskKind::Execute, Status::Done);
        task.project_id = Some(project.id);
        env.seed(&task);
        for repo_id in &repo_ids {
            env.store
                .task_integration_upsert(&TaskRepoIntegrationState {
                    task_id: task.id,
                    repo_id: *repo_id,
                    branch: format!("celeris/{}", task.id),
                    head_sha: "abc".into(),
                    state: TaskRepoIntegrationStatus::Unintegrated,
                    reason: None,
                    actor: "system".into(),
                    checked_at: now,
                    updated_at: now,
                })
                .expect("state");
        }
    }
    paths
}

fn spawns(paths: &[PathBuf]) -> usize {
    paths
        .iter()
        .map(|p| task_ops::changes::git_spawns_in(p))
        .sum()
}

/// (git 起動回数, 要求が取った reader 接続数)
async fn measure(n: usize, query: &str) -> (usize, u64) {
    let releases = tempfile::tempdir().expect("releases");
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        integration_releases_dir: Some(releases.path().to_path_buf()),
        ..EnvOptions::default()
    });
    let paths = seed(&env, n);
    let (git_before, locks_before) = (spawns(&paths), env.state.store_lock_counts());
    let resp = send(
        &env.router(),
        get_with(
            &format!("/api/v1/tasks?limit=200{query}"),
            &[("authorization", "Bearer s3cret-token-value")],
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let items = resp.json()["items"].as_array().expect("items").len();
    assert_eq!(items, n);
    let locks = env.state.store_lock_counts().since(locks_before);
    (spawns(&paths) - git_before, locks.reader as u64)
}

#[tokio::test]
async fn default_list_spawns_no_git() {
    for n in [1, 20] {
        let (git, _) = measure(n, "").await;
        assert_eq!(git, 0, "n={n}");
    }
}

#[tokio::test]
async fn with_integrations_counts_do_not_scale_with_rows() {
    let (git_1, reader_1) = measure(1, "&with=integrations").await;
    let (git_20, reader_20) = measure(20, "&with=integrations").await;
    // repo ごとに既定 branch の解決（git）を 1 回だけ。行数には依らない。
    assert!(git_1 > 0, "default branch must be resolved");
    assert_eq!(git_1, git_20);
    // store の読みも行数に依らない（全件読み 1 回 + repo ごとの repo_get）。
    let (_, plain_1) = measure(1, "").await;
    let (_, plain_20) = measure(20, "").await;
    assert_eq!(reader_1 - plain_1, reader_20 - plain_20);
}

#[tokio::test]
async fn unknown_with_value_is_400() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let resp = send(
        &env.router(),
        get_with(
            "/api/v1/tasks?with=nope",
            &[("authorization", "Bearer s3cret-token-value")],
        ),
    )
    .await;
    assert_eq!(resp.status, 400, "{}", resp.text());
}
