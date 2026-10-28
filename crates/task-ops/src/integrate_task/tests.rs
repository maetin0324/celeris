//! ADR 2026-10-28-cos-integrate-task D1・D3・D4: 起票内容・決着・人に回す理由（実 git の一時 repo、LLM なし）。
use std::path::Path;
use std::process::Command;

use task_core::integration_request::{
    DELIVERY_ORIGIN, INTEGRATE_TASK_ESCALATED, IntegrateTaskLink, IntegrationRequest,
};
use task_core::{
    ArtifactRef, Budget, Check, Criterion, Event, Project, ProjectId, ProjectStatus, RepoId,
    RepoKind, RepoRef, RepoRun, SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, Tier,
    Trigger, WorkerHint, WorkspaceSpec,
};

use super::*;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// main と feature が同じ行を変えた repo。戻り値は (target_sha, source_sha)。
fn conflicting_repo(dir: &Path) -> (String, String) {
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("lib.rs"), "fn a() {}\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "base"]);
    git(dir, &["checkout", "-qb", "feature"]);
    std::fs::write(dir.join("lib.rs"), "fn a() { feature() }\n").unwrap();
    git(dir, &["commit", "-qam", "feature"]);
    let source = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("lib.rs"), "fn a() { main() }\n").unwrap();
    git(dir, &["commit", "-qam", "main"]);
    (git(dir, &["rev-parse", "HEAD"]), source)
}

fn request(target: &str, source: &str) -> IntegrationRequest {
    IntegrationRequest {
        target_branch: "main".into(),
        target_sha: target.into(),
        source_branch: "feature".into(),
        source_sha: source.into(),
        merge_base: None,
        conflict_files: vec!["lib.rs".into()],
        intent: vec![],
        reason: "conflict".into(),
        recommendation: "resolve".into(),
        actions: vec![],
        candidate_sha: None,
    }
}

fn task(status: Status, repo: &task_core::ProjectRepo) -> Task {
    let now = time::OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: vec!["rust".into()],
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "元の task".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Reviewer,
        }],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::local("/nonexistent/ws"),
        repos: vec![RepoRef::of(repo)],
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 0,
            max_tokens: None,
        },
        attempts: 1,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: Some(repo.project_id),
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn setup() -> (
    tempfile::TempDir,
    SqliteStore,
    task_core::ProjectRepo,
    String,
    String,
) {
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    let (target, source) = conflicting_repo(&repo_dir);
    let store = SqliteStore::open_in_memory().unwrap();
    let now = time::OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        auto_integrate: true,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let repo = task_core::ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "app".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::Local {
            path: repo_dir,
            mode: None,
        },
        default_branch: Some("main".into()),
        sync: None,
        run: RepoRun::default(),
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&repo).unwrap();
    (dir, store, repo, target, source)
}

fn link(store: &SqliteStore, original: TaskId, request_id: &str, integrate: TaskId) {
    store
        .append_event(
            original,
            &Event::IntegrateTaskRequested {
                request_id: request_id.into(),
                integrate_task_id: integrate,
                by: "cos:thread-1".into(),
            },
        )
        .unwrap();
}

#[test]
fn spec_requires_both_heads_the_repo_checks_and_a_reviewer() {
    let (_dir, _store, repo, target, source) = setup();
    let original = task(Status::Done, &repo);
    let req = request(&target, &source);
    let spec = spec(
        &original,
        &req,
        &req.id_for(original.id),
        &["cargo test".into()],
        Some("片側を消さない"),
    )
    .unwrap();
    assert_eq!(spec.repos, vec!["app".to_string()]);
    assert_eq!(spec.project_id, original.project_id);
    assert_eq!(spec.labels, vec![LABEL.to_string()]);
    assert!(
        spec.objective
            .contains(&format!("git merge --no-ff {source}"))
    );
    assert!(spec.objective.contains("片側を消さない"));
    let json = serde_json::to_value(&spec.acceptance).unwrap();
    assert_eq!(json[0]["type"], "command");
    assert_eq!(
        json[0]["cmd"],
        format!(
            "git merge-base --is-ancestor {source} HEAD && git merge-base --is-ancestor {target} HEAD"
        )
    );
    assert_eq!(json[1]["cmd"], "cargo test");
    assert_eq!(json[2]["type"], "reviewer");
}

#[test]
fn settle_closes_the_request_once_main_contains_the_source_head() {
    let (_dir, store, repo, target, source) = setup();
    let original = task(Status::Done, &repo);
    store.insert(&original).unwrap();
    let req = request(&target, &source);
    let request_id = req.id_for(original.id);
    store
        .integration_request_record(original.id, &req, DELIVERY_ORIGIN)
        .unwrap();
    let integrate = task(Status::Reviewing, &repo);
    store.insert(&integrate).unwrap();
    link(&store, original.id, &request_id, integrate.id);

    // integrate task が review 中の間・reviewer の合格で done になっても main に入る前は何もしない。
    assert!(settle(&store).unwrap().is_empty());
    store
        .apply_transition(integrate.id, Trigger::ReviewPass, None)
        .unwrap();
    assert_eq!(
        store.get(integrate.id).unwrap().unwrap().status,
        Status::Done
    );
    assert!(settle(&store).unwrap().is_empty());
    assert_eq!(open_requests(&store).unwrap().len(), 1);

    // worker が main を基点に元の head を merge して解き、配送が main を進めた。
    let WorkspaceSpec::Local { path, .. } = &repo.location else {
        unreachable!()
    };
    git(path, &["checkout", "-qb", "celeris/integrate"]);
    let merge = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["merge", "--no-ff", "-q", &source])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the fixture must conflict");
    std::fs::write(path.join("lib.rs"), "fn a() { main(); feature() }\n").unwrap();
    git(path, &["add", "lib.rs"]);
    git(path, &["commit", "-qm", "resolve"]);
    git(path, &["checkout", "-q", "main"]);
    git(path, &["merge", "--ff-only", "-q", "celeris/integrate"]);

    let out = settle(&store).unwrap();
    assert_eq!(
        out,
        vec![Outcome::Integrated {
            task_id: original.id,
            request_id: request_id.clone(),
            integrate_task_id: integrate.id,
        }]
    );
    assert!(open_requests(&store).unwrap().is_empty());
    let events = store.events_for(original.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e,
        Event::IntegrationAnswered { request_id: id, answer, by, .. }
            if *id == request_id && answer == "integrated" && by.as_deref() == Some(task_core::DAEMON_ACTOR))));
    assert_eq!(
        link_for(&store, original.id, &request_id).unwrap(),
        Some(IntegrateTaskLink::Integrated {
            task_id: integrate.id
        })
    );
    // 2 回目は何もしない。
    assert!(settle(&store).unwrap().is_empty());
}

#[test]
fn settle_escalates_a_failed_integrate_task_and_keeps_the_request_open() {
    let (_dir, store, repo, target, source) = setup();
    let original = task(Status::Done, &repo);
    store.insert(&original).unwrap();
    let req = request(&target, &source);
    let request_id = req.id_for(original.id);
    store
        .integration_request_record(original.id, &req, DELIVERY_ORIGIN)
        .unwrap();
    let integrate = task(Status::Running, &repo);
    store.insert(&integrate).unwrap();
    link(&store, original.id, &request_id, integrate.id);
    assert!(escalated_reason(&store, original.id).unwrap().is_none());
    store
        .apply_transition(
            integrate.id,
            Trigger::WorkerError { retryable: false },
            None,
        )
        .unwrap();
    assert_eq!(
        store.get(integrate.id).unwrap().unwrap().status,
        Status::Failed
    );

    let out = settle(&store).unwrap();
    assert_eq!(
        out,
        vec![Outcome::Escalated {
            task_id: original.id,
            request_id: request_id.clone(),
            integrate_task_id: integrate.id,
        }]
    );
    assert_eq!(
        open_requests(&store).unwrap().len(),
        1,
        "the request stays for a person"
    );
    assert!(matches!(
        link_for(&store, original.id, &request_id).unwrap(),
        Some(IntegrateTaskLink::Escalated { task_id, .. }) if task_id == integrate.id
    ));
    let events = store.events_for(original.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e,
        Event::IntegrateTaskSettled { outcome, .. } if outcome == INTEGRATE_TASK_ESCALATED)));
    let why = escalated_reason(&store, original.id)
        .unwrap()
        .expect("human required");
    assert!(why.contains(&integrate.id.to_string()), "{why}");
    assert!(!offerable(
        DELIVERY_ORIGIN,
        link_for(&store, original.id, &request_id).unwrap().as_ref()
    ));
    // 決着は 1 回だけ記録する。
    assert!(settle(&store).unwrap().is_empty());
}
