//! ADR 2026-11-07-health-review-integration D4・I5: final review の `ReviewRequest.health` を candidate SHA ごとに 1 回
//! 作る。本物の repo health の代わりに一時 git repo の main に stub の `repo_health.py` を commit する（report の
//! mode では log に 1 行足して固定の report を書く）。外部ネットワークに出ない。

use super::target_sync::{bump_main_writing, commit_file, events_of, reviewing_task, set_check};
use super::*;

use crate::health_review::policy::{CHECKER_SCRIPT, REVIEW_POLICY_SCHEMA};
use task_worker::protocol::HealthReviewInput;

const DIGEST: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn report() -> serde_json::Value {
    let finding = |rule: &str, delta: &str, path: &str| {
        serde_json::json!({"rule_id": rule, "fingerprint": format!("{rule}:0123456789abcdef"),
            "severity": "error", "delta": delta, "exception": null, "evidence": [{"path": path}]})
    };
    serde_json::json!({
        "schema": "celeris.repo-health.report/1",
        "summary": {"verdict": "fail", "exit_code": 1, "reviewable": true,
            "counts": {"findings": 3, "fail_grade": 2, "new": 1, "worsened": 1, "existing": 1}},
        "findings": [
            finding("size.responsibility-growth", "new", "crates/foo/src/lib.rs"),
            finding("deps.violation", "worsened", "crates/foo/Cargo.toml"),
            finding("arch-map.missing_invariant", "existing", "owners/map.md"),
        ],
        "policy": {"version": "1", "source": "base", "changed": false, "files": []},
    })
}

/// main に stub の検査器（方針の `source`）と owner 地図を commit した repo と、report の実行を数える log。
fn health_repo(source: &str) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    health_repo_with(source, Some(report()))
}

/// [`health_repo`] の report を選べる形（`None` は report を書かずに exit 2）。
fn health_repo_with(
    source: &str,
    report: Option<serde_json::Value>,
) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("report-runs.log");
    let policy = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA, "source": source, "owner_map": "owners/map.md", "high_risk": [],
        "invariants": {"events-append-only": "CLAUDE.md"}, "definition_sha256": DIGEST,
    });
    let script = format!(
        "import sys\nargs = sys.argv[1:]\nif '--print-review-policy' in args:\n    sys.stdout.write({policy:?})\n    sys.exit(0)\nopen({log:?}, 'a').write(' '.join(args) + '\\n')\nreport = {report:?}\nif not report:\n    sys.exit(2)\nopen(args[args.index('--out') + 1], 'w').write(report)\nsys.exit(1)\n",
        policy = policy.to_string(),
        log = log.display().to_string(),
        report = report.map(|r| r.to_string()).unwrap_or_default(),
    );
    let path = repo.path().join(CHECKER_SCRIPT);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, script).unwrap();
    std::fs::create_dir_all(repo.path().join("owners")).unwrap();
    std::fs::write(
        repo.path().join("owners/map.md"),
        "| subsystem | entry |\n|---|---|\n| foo | `crates/foo/src/{lib.rs,bar.rs}` |\n| other | `crates/other/` |\n",
    )
    .unwrap();
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-q", "-m", "checker"]);
    (repo, scratch, log)
}

fn log_lines(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// reviewer の run の `ReviewRequest.health` を記録し、合格の `review.json` を書く。`fail_first` なら 1 回目は
/// adapter の失敗（reviewer run のやり直しになる）。
struct HealthReviewer {
    seen: Arc<StdMutex<Vec<Option<HealthReviewInput>>>>,
    fail_first: bool,
}

#[async_trait]
impl WorkerAdapter for HealthReviewer {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        if req.task.kind == TaskKind::Review {
            let health = req.context.review.as_ref().and_then(|r| r.health.clone());
            let n = {
                let mut seen = self.seen.lock().unwrap();
                seen.push(health);
                seen.len()
            };
            if self.fail_first && n == 1 {
                return Err(AdapterError::Io(std::io::Error::other("transient")));
            }
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"fine"}],"health":{"findings":[]}}"#,
            )
            .unwrap();
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

struct Reviewed {
    store: Arc<dyn TaskStore>,
    seen: Vec<Option<HealthReviewInput>>,
    events: Vec<Event>,
    task: Task,
    target: String,
    head: String,
    status: Status,
}

/// reviewing の task（repo は `repo`、worktree で `crates/foo/src/lib.rs` を変える）を review まで流す。
async fn review_flow(
    repo: &std::path::Path,
    reviewer: bool,
    fail_first: bool,
    check: Option<String>,
) -> Reviewed {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = reviewing_task(repo, &store);
    if let Some(cmd) = check {
        set_check(&mut task, cmd);
    }
    if reviewer {
        task.acceptance.push(task_core::Criterion {
            text: "reviewer".into(),
            check: Check::Reviewer,
        });
    }
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(HealthReviewer {
        seen: seen.clone(),
        fail_first,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    std::fs::create_dir_all(wt.dir.join("crates/foo/src")).unwrap();
    let head = commit_file(&wt.dir, "crates/foo/src/lib.rs", "pub fn foo() {}\n");
    let target = git_out(repo, &["rev-parse", "refs/heads/main"]);
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 200).await.idle);
    let status = store.get(task.id).unwrap().unwrap().status;
    let events = events_of(&store, &task);
    let seen = seen.lock().unwrap().clone();
    Reviewed {
        store,
        seen,
        events,
        task,
        target,
        head,
        status,
    }
}

fn built(events: &[Event]) -> Vec<(String, String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::HealthReviewInputBuilt {
                review_run,
                target_sha,
                reviewed_sha,
                definition_sha256,
                ..
            } => {
                assert_eq!(definition_sha256, DIGEST);
                Some((review_run.clone(), target_sha.clone(), reviewed_sha.clone()))
            }
            _ => None,
        })
        .collect()
}

fn reviewer_runs(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkerStarted {
                run_id,
                role: Some(task_core::RunRole::Reviewer),
                ..
            } => Some(run_id.clone()),
            _ => None,
        })
        .collect()
}

/// `ReviewRequest.health` に diff 統計・owner 行・findings の要約・policy の状態・不変条件が入り、repo health は
/// `--base <target> --head <candidate> --policy-version 1 --out <task dir の runs/health/>` で 1 回だけ走る。
#[tokio::test]
async fn health_review_input_reaches_the_reviewer_request() {
    let (repo, _scratch, log) = health_repo("base");
    let r = review_flow(repo.path(), true, false, None).await;
    assert_eq!(r.status, Status::Done, "{:?}", r.events);
    assert_eq!(r.seen.len(), 1);
    let health = r.seen[0].clone().expect("final review gets health");
    assert_eq!(health.repos.len(), 1);
    assert_eq!(health.brief, None, "no adopted plan");
    let input = &health.repos[0];
    assert_eq!(input.repo_id, r.task.repos[0].repo_id.to_string());
    assert_eq!(input.target_sha, r.target);
    assert_eq!(
        input.reviewed_sha, r.head,
        "main did not move, so the rebase keeps the SHA"
    );
    assert_eq!(
        (
            input.diff.files,
            input.diff.insertions,
            input.diff.deletions
        ),
        (1, 1, 0)
    );
    assert_eq!(input.diff.paths[0].path, "crates/foo/src/lib.rs");
    assert_eq!(input.owner_rows.len(), 1);
    assert_eq!(input.owner_rows[0].path, "crates/foo/src/lib.rs");
    assert!(input.owner_rows[0].row.contains("{lib.rs,bar.rs}"));
    assert_eq!(input.findings.verdict, "fail");
    assert!(input.findings.reviewable);
    let rules: Vec<_> = input
        .findings
        .new
        .iter()
        .map(|f| f.rule_id.as_str())
        .collect();
    assert_eq!(rules, ["size.responsibility-growth", "deps.violation"]);
    assert_eq!(input.findings.truncated, 0);
    assert_eq!(input.findings.counts.existing, 1);
    assert_eq!(
        (input.policy.source.as_str(), input.policy.changed),
        ("base", false)
    );
    assert_eq!(
        input
            .invariants
            .get("events-append-only")
            .map(String::as_str),
        Some("CLAUDE.md")
    );

    let runs = reviewer_runs(&r.events);
    assert_eq!(
        built(&r.events),
        vec![(runs[0].clone(), r.target.clone(), r.head.clone())]
    );
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 1, "repo health ran once: {lines:?}");
    let want = format!(
        "--base {} --head {} --policy-version 1 --out ",
        r.target, r.head
    );
    assert!(lines[0].contains(&want), "{}", lines[0]);
    assert!(lines[0].contains("/runs/health/"), "{}", lines[0]);
}

/// reviewer run が失敗してやり直しても、同じ candidate の材料は event から使い回し、repo health は 1 回だけ。
#[tokio::test]
async fn health_review_input_reused_on_reviewer_retry() {
    let (repo, _scratch, log) = health_repo("base");
    let r = review_flow(repo.path(), true, true, None).await;
    assert_eq!(r.status, Status::Done, "{:?}", r.events);
    assert_eq!(
        r.seen.len(),
        2,
        "the reviewer ran again after the transient failure"
    );
    assert!(r.seen[0].is_some());
    assert_eq!(r.seen[0], r.seen[1]);
    assert_eq!(reviewer_runs(&r.events).len(), 2);
    assert_eq!(built(&r.events).len(), 1);
    assert_eq!(log_lines(&log).len(), 1, "repo health ran once");
}

/// review 方針が absent の repo は作らず、`HealthReviewInputSkipped` に理由を残す（`ReviewRequest.health` は無い）。
/// policy の関門のため repo health は 1 回走り、policy の状態（ここでは変更なし）を同じ event に残す。
#[tokio::test]
async fn health_review_input_policy_absent_is_skipped() {
    let (repo, _scratch, log) = health_repo("absent");
    let r = review_flow(repo.path(), true, false, None).await;
    assert_eq!(r.status, Status::Done, "{:?}", r.events);
    assert_eq!(r.seen, vec![None]);
    assert!(built(&r.events).is_empty());
    assert_eq!(log_lines(&log).len(), 1);
    let skipped: Vec<_> = r
        .events
        .iter()
        .filter_map(|e| match e {
            Event::HealthReviewInputSkipped {
                reason,
                reviewed_sha,
                policy,
                ..
            } => Some((reason.clone(), reviewed_sha.clone(), policy.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].0.contains("absent"), "{skipped:?}");
    assert_eq!(skipped[0].1, r.head);
    assert_eq!(
        skipped[0].2.as_ref().map(|p| p["changed"].clone()),
        Some(false.into())
    );
}

/// I8・D6.4（fail closed）: report を書かない repo health の candidate は、reviewer が合格を書いても done に
/// ならない。policy の状態を不明として candidate の SHA に結び付けて記録し、承認の子を作って reviewing のまま待つ。
#[tokio::test]
async fn health_review_policy_unknown_flow_missing_report_waits_for_a_human() {
    let (repo, _scratch, log) = health_repo_with("base", None);
    let r = review_flow(repo.path(), true, false, None).await;
    assert_eq!(r.status, Status::Reviewing, "{:?}", r.events);
    assert_eq!(log_lines(&log).len(), 1);
    let health = r.seen[0]
        .as_ref()
        .expect("the reviewer gets the health input");
    assert_eq!(health.repos[0].policy.source, "unavailable");
    let run = reviewer_runs(&r.events).remove(0);
    assert_eq!(
        built(&r.events),
        vec![(run, r.target.clone(), r.head.clone())]
    );
    let approvals: Vec<Task> = r
        .store
        .children(r.task.id)
        .unwrap()
        .into_iter()
        .filter(|c| c.kind == TaskKind::Approval)
        .collect();
    assert_eq!(approvals.len(), 1, "{:?}", r.events);
    assert_eq!(
        approvals[0].title,
        format!(
            "Policy change approval: {} @ {}",
            r.task.repos[0].repo_id,
            &r.head[..12]
        )
    );
    assert!(
        approvals[0]
            .objective
            .contains("policy state unknown: repo_health.py が report を書かなかった"),
        "{}",
        approvals[0].objective
    );
}

/// reviewer 条件の無い review（決定的 check だけ）と、その間の tick では作らない。
#[tokio::test]
async fn health_review_input_not_built_by_ticks_or_reviews_without_a_reviewer() {
    let (repo, _scratch, log) = health_repo("base");
    let r = review_flow(repo.path(), false, false, None).await;
    assert_eq!(r.status, Status::Done, "{:?}", r.events);
    assert!(r.seen.is_empty());
    assert!(log_lines(&log).is_empty());
    assert!(!r.events.iter().any(|e| matches!(
        e,
        Event::HealthReviewInputBuilt { .. } | Event::HealthReviewInputSkipped { .. }
    )));
}

/// `reviewer_reuse`（target の進みが候補の差分と重ならない）で reviewer を省いた再 review では作らない。
/// 1 回目の review（reviewer あり）の分だけが残る。
#[tokio::test]
async fn health_review_input_not_built_when_the_reviewer_pass_is_reused() {
    let (repo, _scratch, log) = health_repo("base");
    let flag = tempfile::tempdir().unwrap();
    let check = bump_main_writing(&flag.path().join("advanced"), "other.txt", "printf other");
    let r = review_flow(repo.path(), true, false, Some(check)).await;
    assert_eq!(r.status, Status::Done, "{:?}", r.events);
    assert_eq!(
        r.seen.len(),
        1,
        "the second review reused the reviewer pass"
    );
    assert!(
        r.events
            .iter()
            .any(|e| matches!(e, Event::ReviewerPassReused { .. }))
    );
    let builds = built(&r.events);
    assert_eq!(builds.len(), 1, "{builds:?}");
    assert_eq!(builds[0].1, r.target, "only the first candidate");
    assert_eq!(log_lines(&log).len(), 1);
}

/// WU の check と WU の run（計画の採用から統合まで）では作らない。作るのは final review の reviewer run だけで、
/// 最初の reviewer run の前に材料の event は無い。
#[tokio::test]
async fn health_review_input_not_built_for_work_units() {
    let (repo, _scratch, log) = health_repo("base");
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = compound_task(repo.path());
    task.workspace = WorkspaceSpec::Local {
        path: repo.path().to_path_buf(),
        mode: None,
    };
    let (project, repos) =
        project_with_repos_integrating(&store, &[("code", repo.path(), RepoKind::Git)], false);
    task.project_id = Some(project);
    task.repos = repos.iter().map(RepoRef::of).collect();
    task.acceptance.push(task_core::Criterion {
        text: "reviewer".into(),
        check: Check::Reviewer,
    });
    store.insert(&task).unwrap();
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(plan_json(vec![
            wu_spec("a", &[]),
            wu_spec("b", &["a"]),
        ]))],
        HashMap::new(),
    ));
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    run_until_idle(&mut d, 400).await;

    let units = store.work_units_for(task.id).unwrap();
    assert!(units.len() >= 2, "the plan was adopted: {units:?}");
    let events = events_of(&store, &task);
    let reviewers = reviewer_runs(&events);
    let first_reviewer = events.iter().position(|e| {
        matches!(
            e,
            Event::WorkerStarted {
                role: Some(task_core::RunRole::Reviewer),
                ..
            }
        )
    });
    let first_build = events
        .iter()
        .position(|e| matches!(e, Event::HealthReviewInputBuilt { .. }));
    if let Some(build) = first_build {
        assert!(first_reviewer.is_some_and(|r| r < build), "{events:?}");
    }
    let builds = built(&events);
    assert!(
        builds.iter().all(|(run, ..)| reviewers.contains(run)),
        "{builds:?}"
    );
    assert!(builds.len() <= 1, "{builds:?}");
    assert_eq!(log_lines(&log).len(), builds.len());
    let wu_runs = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.execution_planner.is_none() && c.review.is_none())
        .count();
    assert!(wu_runs >= 2, "the work units ran");
}
