//! ADR 2026-11-07-health-review-integration D3.5（3〜5）・I9: 設計 review の結果で計画を採用・replan・人に回す。
//! base の検査器は一時 git repo に commit した stub（高 risk の class `store` = `^crates/task-store/`）。設計 review の
//! reviewer run は試験の adapter が受け、台本どおりに `review.json` を書く（合格・不合格・`design` 無し・終わらない）。
//! 外部ネットワークに出ない。

use super::*;

use crate::health_review::policy::{CHECKER_SCRIPT, REVIEW_POLICY_SCHEMA};
use crate::orphan::OrphanTakeover;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const STORE_PATH: &str = "crates/task-store/src/rows.rs";
const NEW_DAEMON: &str = "01NEWDAEMON0000000000000001";

fn stub() -> String {
    let json = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA,
        "source": "base",
        "high_risk": [{"class": "store", "paths": ["^crates/task-store/"]}],
        "invariants": {},
        "definition_sha256": DIGEST,
    });
    format!("import sys\nsys.stdout.write({:?})\n", json.to_string())
}

fn checker_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let script = repo.path().join(CHECKER_SCRIPT);
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, stub()).unwrap();
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-q", "-m", "checker"]);
    repo
}

/// /3 の計画（leaf 1 つ、write path は `path`、`new_state` を宣言した brief 付き）。`title` で計画の sha を変える。
fn v3_plan(path: &str, title: &str) -> String {
    let mut unit = super::tree::leaf("a", "s1", &[]);
    unit["context"] = serde_json::json!({"paths": [path]});
    unit["title"] = serde_json::json!(title);
    let mut plan: serde_json::Value = serde_json::from_str(&super::tree::v3_plan(
        vec![super::tree::stage("s1", false)],
        vec![unit],
    ))
    .unwrap();
    plan["design_brief"] = serde_json::json!({
        "root_cause": "rows are rewritten in place",
        "owner": "task-store",
        "options": [
            {"name": "append a column", "chosen": true, "reason": "keeps history"},
            {"name": "rewrite rows", "chosen": false, "reason": "loses history"}
        ],
        "new_state": ["rows.revision column"],
    });
    plan.to_string()
}

fn finding(kind: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "severity": "blocking",
        "path": STORE_PATH,
        "reason": reason,
    })
}

/// 設計 review の run の台本。
#[derive(Clone)]
enum Script {
    /// `review.json` に `design { pass, findings }` を書いて `done`。
    Verdict(bool, Vec<serde_json::Value>),
    /// `review.json` を `design` 無しで書いて `done`。
    Missing,
    /// `review.json` を書かず、長く終わらない（デーモンの再起動で失われる run）。
    Hang,
}

/// planner の計画を順に出し、設計 review の run を台本どおりに終える adapter。
struct DesignScriptAdapter {
    inner: PlannerScriptAdapter,
    designs: StdMutex<std::collections::VecDeque<Script>>,
}

#[async_trait]
impl WorkerAdapter for DesignScriptAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let is_design = req
            .context
            .review
            .as_ref()
            .is_some_and(|r| r.design.is_some());
        if !is_design {
            return self.inner.run(req, run_id, limits, sink).await;
        }
        self.inner.seen.lock().unwrap().push(req.context.clone());
        let next = self.designs.lock().unwrap().pop_front();
        let json = match next.unwrap_or(Script::Verdict(true, Vec::new())) {
            Script::Verdict(pass, findings) => {
                serde_json::json!({"verdicts": [], "design": {"pass": pass, "findings": findings}})
            }
            Script::Missing => serde_json::json!({"verdicts": []}),
            Script::Hang => {
                tokio::time::sleep(Duration::from_secs(60)).await;
                serde_json::json!({"verdicts": []})
            }
        };
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        std::fs::write(
            req.artifacts_dir.join(crate::review::REVIEW_FILE_NAME),
            json.to_string(),
        )
        .unwrap();
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "reviewed".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

struct Fixture {
    store: Arc<dyn TaskStore>,
    task: Task,
    adapter: Arc<DesignScriptAdapter>,
    repo: tempfile::TempDir,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(plans: Vec<String>, designs: Vec<Script>) -> Self {
        let repo = checker_repo();
        let root = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let mut task = compound_task(repo.path());
        task.workspace = WorkspaceSpec::Local {
            path: repo.path().to_path_buf(),
            mode: None,
        };
        store.insert(&task).unwrap();
        let adapter = Arc::new(DesignScriptAdapter {
            inner: PlannerScriptAdapter::new(plans.into_iter().map(Some).collect(), HashMap::new()),
            designs: StdMutex::new(designs.into_iter().collect()),
        });
        Fixture {
            store,
            task,
            adapter,
            repo,
            root,
        }
    }
    fn dispatcher(&self) -> Dispatcher {
        let mut d = worktree_dispatcher(
            self.store.clone(),
            self.adapter.clone(),
            self.root.path(),
            None,
        );
        d.config.execution.gate = task_core::GateMode::On;
        d.config.execution.limits = super::tree::tree_limits();
        d
    }
    fn seen(&self) -> Vec<task_worker::RunContext> {
        self.adapter.inner.seen.lock().unwrap().clone()
    }
    fn planner_contexts(&self) -> Vec<task_worker::protocol::ExecutionPlannerContext> {
        self.seen()
            .into_iter()
            .filter_map(|c| c.execution_planner)
            .collect()
    }
    fn design_reviews(&self) -> usize {
        self.seen()
            .iter()
            .filter(|c| c.review.as_ref().is_some_and(|r| r.design.is_some()))
            .count()
    }
    fn work_unit_runs(&self) -> usize {
        self.seen()
            .iter()
            .filter(|c| c.execution_planner.is_none() && c.review.is_none())
            .count()
    }
    fn events(&self) -> Vec<Event> {
        self.store
            .events_for(self.task.id)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
    fn reviewed(&self) -> Vec<(bool, Vec<serde_json::Value>)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::DesignReviewed { pass, findings, .. } => Some((pass, findings)),
                _ => None,
            })
            .collect()
    }
    fn active_plan(&self) -> Option<task_core::ExecutionPlanRow> {
        self.store.execution_plan_active(self.task.id).unwrap()
    }
    fn status(&self) -> Status {
        self.store.get(self.task.id).unwrap().unwrap().status
    }
}

/// D3.5 4.（合格）: 設計 review が `pass = true` を書くと `DesignReviewed { pass: true }` が追記され、
/// `DesignReviewRequested` の計画が採用されて（`ExecutionPlanned`）その unit の run が起きる。reviewer run は 1 本だけ。
#[tokio::test]
async fn health_review_design_verdict_pass_adopts_the_reviewed_plan() {
    let f = Fixture::new(
        vec![v3_plan(STORE_PATH, "first")],
        vec![Script::Verdict(true, Vec::new())],
    );
    let mut d = f.dispatcher();
    run_until_idle(&mut d, 400).await;
    assert_eq!(f.planner_contexts().len(), 1, "the planner ran once");
    assert_eq!(f.design_reviews(), 1, "one design review run");
    assert_eq!(f.reviewed(), vec![(true, Vec::new())]);
    let plan = f.active_plan().expect("the reviewed plan is adopted");
    let requested = f
        .events()
        .into_iter()
        .find_map(|e| match e {
            Event::DesignReviewRequested { plan, .. } => Some(*plan),
            _ => None,
        })
        .unwrap();
    assert_eq!(plan.spec, requested, "the adopted plan is the reviewed one");
    assert!(
        f.events()
            .iter()
            .any(|e| matches!(e, Event::ExecutionPlanned { .. })),
        "{:?}",
        f.events()
    );
    assert!(
        f.work_unit_runs() >= 1,
        "the unit of the adopted plan runs: {:?}",
        f.seen().len()
    );
    assert!(
        !f.events()
            .iter()
            .any(|e| matches!(e, Event::PlanApprovalRequested { .. })),
        "no plan gate for a passing design review of a non-root plan"
    );
}

/// D3.5 4.（不合格）: `pass = false` の findings は `DesignReviewed` に写り、planner が replan で起き、その
/// `previous_attempt_errors` に findings（種別・path・理由）が載る。2 回目の計画（低 risk）はそのまま採用される。
#[tokio::test]
async fn health_review_design_verdict_fail_replans_with_the_findings_as_prior() {
    let f = Fixture::new(
        vec![
            v3_plan(STORE_PATH, "first"),
            v3_plan("notes/plan.md", "second"),
        ],
        vec![Script::Verdict(
            false,
            vec![finding(
                "dual_source_of_truth",
                "rows and events both hold the revision",
            )],
        )],
    );
    let mut d = f.dispatcher();
    run_until_idle(&mut d, 400).await;
    let planners = f.planner_contexts();
    assert_eq!(
        planners.len(),
        2,
        "the planner is run again after the failed design review"
    );
    let prior = planners[1].previous_attempt_errors.join("\n");
    assert!(
        prior.contains("design review failed")
            && prior.contains("dual_source_of_truth(blocking)")
            && prior.contains(STORE_PATH)
            && prior.contains("rows and events both hold the revision"),
        "the findings reach the planner's prior: {prior}"
    );
    assert_eq!(
        f.design_reviews(),
        1,
        "the low-risk second plan needs no design review"
    );
    let reviewed = f.reviewed();
    assert_eq!(reviewed.len(), 1);
    assert!(!reviewed[0].0);
    assert_eq!(reviewed[0].1[0]["kind"], "dual_source_of_truth");
    let plan = f.active_plan().expect("the second plan is adopted");
    assert_eq!(plan.spec.units[0].title, "second");
    assert!(
        f.events()
            .iter()
            .any(|e| matches!(e, Event::WorkerProgress { msg, .. } if msg.contains("design review failed") && msg.contains("もう一度だけ試します"))),
        "the replan reason is recorded as a planner retry"
    );
}

/// D3.5 5.: 設計 review の不合格が 2 回続いたら、2 回目の計画を採用したうえで `PlanGate`（`awaiting_plan_approval`、
/// `PlanApprovalRequested.reasons` に `design_review_failed:2`）で人に回す。unit の run は起きない。
#[tokio::test]
async fn health_review_design_verdict_two_failures_in_a_row_ask_a_human_plan_gate() {
    let f = Fixture::new(
        vec![v3_plan(STORE_PATH, "first"), v3_plan(STORE_PATH, "second")],
        vec![
            Script::Verdict(false, vec![finding("duplicate_implementation", "dup")]),
            Script::Verdict(
                false,
                vec![finding("local_hook_in_shared_interface", "hook")],
            ),
        ],
    );
    let mut d = f.dispatcher();
    run_until_idle(&mut d, 400).await;
    assert_eq!(f.planner_contexts().len(), 2);
    assert_eq!(f.design_reviews(), 2, "each plan got its own design review");
    assert_eq!(
        f.reviewed().iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        vec![false, false]
    );
    assert_eq!(f.status(), Status::Blocked, "{:?}", f.events());
    let plan = f
        .active_plan()
        .expect("the twice-failed plan is adopted for the human gate");
    assert_eq!(plan.spec.units[0].title, "second");
    let reasons = f
        .events()
        .into_iter()
        .find_map(|e| match e {
            Event::PlanApprovalRequested { plan_id, reasons } if plan_id == plan.id => {
                Some(reasons)
            }
            _ => None,
        })
        .expect("PlanApprovalRequested for the adopted plan");
    assert!(
        reasons.iter().any(|r| r == "design_review_failed:2"),
        "{reasons:?}"
    );
    assert_eq!(
        task_ops::phase_gate::last_transition_reason(&f.store.events_for(f.task.id).unwrap()),
        Some("awaiting_plan_approval")
    );
    assert_eq!(
        f.work_unit_runs(),
        0,
        "no unit runs while the plan waits for the human"
    );
}

/// D3.5 3.: reviewer が `review.json` に `design` を書かなければ不合格（`DesignReviewed { pass: false, findings: [] }`）で、
/// planner がもう一度起きる。
#[tokio::test]
async fn health_review_design_verdict_missing_design_is_a_failure() {
    let f = Fixture::new(
        vec![
            v3_plan(STORE_PATH, "first"),
            v3_plan("notes/plan.md", "second"),
        ],
        vec![Script::Missing],
    );
    let mut d = f.dispatcher();
    run_until_idle(&mut d, 400).await;
    assert_eq!(f.reviewed(), vec![(false, Vec::new())]);
    let planners = f.planner_contexts();
    assert_eq!(planners.len(), 2, "the planner is run again");
    assert!(
        planners[1]
            .previous_attempt_errors
            .join("\n")
            .contains("design（設計 review の判定）が無い"),
        "{:?}",
        planners[1].previous_attempt_errors
    );
    assert!(
        f.active_plan().is_some(),
        "the second (low-risk) plan is adopted"
    );
}

/// I9: 設計 review の待ちは events だけで表す。reviewer run の途中でデーモンが落ちても、新しいデーモンは
/// `DesignReviewRequested`（同じ `plan_sha256` の `DesignReviewed` が無い）から待ちを戻し、孤児の回収が失われた run を
/// `WorkerFinished{interrupted}` で閉じた後に reviewer run を起こし直す。その判定（合格）で計画が採用される。
#[tokio::test]
async fn health_review_design_verdict_pending_review_survives_a_daemon_restart() {
    let f = Fixture::new(
        vec![v3_plan(STORE_PATH, "first")],
        vec![Script::Hang, Script::Verdict(true, Vec::new())],
    );
    let mut old = f.dispatcher();
    // 設計 review の run が起きる（終わらない）まで回す。
    for _ in 0..200 {
        old.tick().unwrap();
        if f.design_reviews() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(f.design_reviews(), 1, "{:?}", f.events());
    assert_eq!(f.status(), Status::Ready);
    assert!(f.active_plan().is_none());
    // クラッシュ: DB には何も書かない。
    old.crash_for_test();
    drop(old);
    let events = f.store.events_for(f.task.id).unwrap();
    assert!(
        crate::health_review::design::pending_design_review(&events).is_some(),
        "the wait is still readable from the events"
    );
    assert!(
        crate::health_review::design::design_review_in_flight(&events, 0).is_some(),
        "the lost run has no WorkerFinished yet: {events:?}"
    );

    // 新しい active（孤児の回収を有効にする。他に生きている instance は無い）。
    f.store
        .instance_register(&super::orphan_takeover::instance(
            NEW_DAEMON,
            InstanceRole::Active,
            std::process::id(),
        ))
        .unwrap();
    let mut new = f.dispatcher();
    new.set_orphan_takeover(OrphanTakeover {
        instance_id: NEW_DAEMON.into(),
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    run_until_idle(&mut new, 400).await;
    let interrupted = f
        .events()
        .into_iter()
        .filter(|e| matches!(e, Event::WorkerFinished { role: Some(RunRole::Reviewer), outcome, .. } if outcome.starts_with("interrupted:")))
        .count();
    assert_eq!(
        interrupted,
        1,
        "the lost run is closed by the ownerless-run reconcile: {:?}",
        f.events()
    );
    assert_eq!(
        f.design_reviews(),
        2,
        "the design review is started again by the new daemon"
    );
    assert_eq!(f.reviewed(), vec![(true, Vec::new())]);
    assert!(
        f.active_plan().is_some(),
        "the reviewed plan is adopted after the restart"
    );
    assert_eq!(
        f.planner_contexts().len(),
        1,
        "the planner is not run again"
    );
    assert!(f.work_unit_runs() >= 1);
    drop(f.repo);
}
