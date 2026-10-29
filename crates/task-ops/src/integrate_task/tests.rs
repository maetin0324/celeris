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

/// 段 `build`（葉 `a`）の計画を採り、統合 WU `integrate-build` を依頼で止めた blocked の task。`repaired` なら
/// 同じ段に merge の repair WU（自動の解消の後の依頼）がある。戻り値は (task, request id)。
fn phase_request_task(
    store: &SqliteStore,
    repo: &task_core::ProjectRepo,
    target: &str,
    source: &str,
    repaired: bool,
) -> (Task, String) {
    let original = task(Status::Blocked, repo);
    store.insert(&original).unwrap();
    let unit = |key: &str, kind: task_core::WorkUnitKind| task_core::WorkUnitSpec {
        key: key.into(),
        kind,
        title: key.into(),
        objective: key.into(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: Default::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: Some("build".into()),
    };
    let mut work_units = vec![unit("a", task_core::WorkUnitKind::Implement)];
    if repaired {
        work_units.push(unit("merge-build-a", task_core::WorkUnitKind::Repair));
    }
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
        rationale: "phase".into(),
        work_units,
        phases: vec![task_core::PhaseSpec {
            key: "build".into(),
            kind: task_core::WorkUnitKind::Implement,
            title: "build".into(),
        }],
        children: Vec::new(),
    };
    crate::execution::adopt_plan(
        store,
        original.id,
        spec,
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    let mut integ = store
        .work_units_for(original.id)
        .unwrap()
        .into_iter()
        .find(|u| u.kind == task_core::WorkUnitKind::Integrate)
        .expect("integration unit");
    assert_eq!(integ.key, "integrate-build");
    let from = integ.status;
    integ.status = task_core::WorkUnitStatus::Blocked;
    integ.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
    store
        .work_unit_transition(
            original.id,
            integ.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from,
                to: task_core::WorkUnitStatus::Blocked,
                reason: "integration_request".into(),
                run_id: None,
            },
        )
        .unwrap();
    let mut phase_request = request(target, source);
    phase_request.target_branch = format!("celeris/{}", original.id);
    assert!(
        store
            .integration_request_record(original.id, &phase_request, "phase:integrate-build")
            .unwrap()
    );
    let request_id = phase_request.id_for(original.id);
    (original, request_id)
}

/// ADR 2026-10-29-phase-integration-auto-repair D3: 自動の解消を経ずに人に回した（この ADR の前の）段の統合の
/// 依頼は、sweep が daemon の `retry` で閉じて統合 WU を pending に戻し、task を再開する。1 回だけ行う。
#[test]
fn sweep_hands_unrepaired_phase_requests_back_to_the_automatic_repair() {
    let (_dir, store, repo, target, source) = setup();
    let (original, request_id) = phase_request_task(&store, &repo, &target, &source, false);
    let resumed = phase::retry_unrepaired_requests(&store).unwrap();
    assert_eq!(resumed, vec![(original.id, request_id.clone())]);
    assert!(open_requests(&store).unwrap().is_empty());
    let events = store.events_for(original.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e,
        Event::IntegrationAnswered { request_id: id, answer, by, .. }
            if *id == request_id && answer == phase::RETRY_ANSWER && by.as_deref() == Some(task_core::DAEMON_ACTOR))));
    let integ = store
        .work_units_for(original.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "integrate-build")
        .unwrap();
    assert_eq!(integ.status, task_core::WorkUnitStatus::Pending);
    assert_eq!(integ.blocked_reason, None);
    assert_ne!(
        store.get(original.id).unwrap().unwrap().status,
        Status::Blocked
    );
    assert!(phase::retry_unrepaired_requests(&store).unwrap().is_empty());
}

/// D2・D3: 自動の解消（merge の repair WU）が尽きて人に回した依頼と、delivery の依頼には sweep は触らない。
#[test]
fn sweep_leaves_requests_after_the_automatic_repair_and_delivery_requests() {
    let (_dir, store, repo, target, source) = setup();
    let (original, _) = phase_request_task(&store, &repo, &target, &source, true);
    let delivered = task(Status::Done, &repo);
    store.insert(&delivered).unwrap();
    store
        .integration_request_record(delivered.id, &request(&target, &source), DELIVERY_ORIGIN)
        .unwrap();
    assert!(phase::retry_unrepaired_requests(&store).unwrap().is_empty());
    assert_eq!(open_requests(&store).unwrap().len(), 2);
    assert_eq!(
        store.get(original.id).unwrap().unwrap().status,
        Status::Blocked
    );
}
