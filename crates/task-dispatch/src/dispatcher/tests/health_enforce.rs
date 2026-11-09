//! ADR 2026-11-08-health-enforcement-loop D1.2・D2.2・D3.2・D3.4: WU の check・段の統合・統合の修復の後が、同じ入口
//! （`run_integration_checks` → `check_item::run_item` → `gate::health_gate`）で repo health を評価し、
//! `WorkUnitCheckFinished`・`IntegrationCheckFinished` の `health` 欄に証跡を残し、Fail を既存の repair へ、Error・
//! Unavailable を Blocked（`health_check_error:`）に回す流れの試験。
//!
//! 入口の試験（`health_enforce_entry_*`）は分類だけを見て、経路が項目を列に足すか・repair と Blocked のどちらに回すか・
//! 修復の後に評価し直すかを通らない。ここはそれを dispatcher の流れで確かめる。Fail と Error は試験の repo の base に
//! commit した stub の検査器（適用判定は base の検査器を同梱より先に選ぶ）で作り、Unavailable は同梱の検査器を設定
//! しない土台（`dispatcher_without_bundled_checker`）で作る。外部ネットワークに出ず、LLM を呼ばない。
use super::*;
use task_core::required_checks::health::{
    CHECKER_SCRIPT, HEALTH_CHECK_ID, HealthEvidence, HealthVerdict, SOURCE_SIZE_REPORT,
};

/// base の stub の検査器。`--print-review-policy` は `[review]` の無い repo の方針。report の mode は、作業ツリー
/// （`--repo`）に `FIXED` があれば合格、無ければ隣の `stub.json` の `exit` と report（fail 級の `deps.violation`）。
const STUB: &str = r#"import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[1:]
if "--print-review-policy" in argv:
    sys.stdout.write(json.dumps({"schema": "celeris.repo-health.review-policy/1", "source": "absent",
        "owner_map": None, "high_risk": [], "invariants": {}, "definition_sha256": "0" * 64}))
    sys.exit(0)
with open(os.path.join(here, "stub.json")) as f:
    stub = json.load(f)
repo = argv[argv.index("--repo") + 1]
fixed = os.path.exists(os.path.join(repo, "FIXED"))
code = 0 if fixed else stub["exit"]
findings = [] if fixed else [{"rule_id": "deps.violation", "fingerprint": "deps.violation:0000000000000001",
    "severity": "error", "delta": "new", "exception": None, "evidence": [{"path": "src/lib.rs"}]}]
report = {"schema": "celeris.repo-health.report/1", "tool": {"name": "repo_health.py", "version": "stub"},
    "inputs": {"profile": {"resolved": "builtin", "source": "builtin"}},
    "policy": {"version": "1", "source": "builtin", "changed": False, "files": []},
    "rules": [{"id": "deps.violation", "required": True, "status": "pass" if fixed else "fail", "reason": ""}],
    "findings": findings,
    "summary": {"verdict": "pass" if code == 0 else ("fail" if code == 1 else "error"), "exit_code": code,
        "reviewable": True, "counts": {"findings": len(findings), "fail_grade": len(findings), "new": len(findings),
        "worsened": 0, "existing": 0}}}
if code != 2:
    with open(argv[argv.index("--out") + 1], "w") as f:
        json.dump(report, f)
sys.exit(code)
"#;

/// 本物の `health/engine.py` と同じ判定。
const ENGINE: &str = r#"def is_fail_grade(f):
    return f["severity"] == "error" and f["delta"] in ("new", "worsened") and f["exception"] is None


def policy_review_files(report, changed):
    return None
"#;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 試験の repo に stub の検査器（`exit` は FIXED が無いときの exit）を commit する。
fn commit_base_checker(repo: &Path, exit: i32) {
    let dir = repo.join("scripts/dev/repo-health");
    std::fs::create_dir_all(dir.join("health")).unwrap();
    std::fs::write(repo.join(CHECKER_SCRIPT), STUB).unwrap();
    std::fs::write(dir.join("stub.json"), format!("{{\"exit\": {exit}}}")).unwrap();
    std::fs::write(dir.join("health/engine.py"), ENGINE).unwrap();
    std::fs::write(dir.join("health/__init__.py"), "").unwrap();
    let side = repo.join(SOURCE_SIZE_REPORT);
    std::fs::create_dir_all(side.parent().unwrap()).unwrap();
    std::fs::write(side, "# side\n").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "checker"]);
}

/// `(work_unit_id, cmd, pass, health)` の health の項目の終了 event（WU の check と段の統合の両方）。
fn health_items(events: &[Event]) -> Vec<(String, bool, HealthEvidence)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkUnitCheckFinished {
                work_unit_id,
                cmd,
                pass,
                health: Some(h),
                ..
            }
            | Event::IntegrationCheckFinished {
                work_unit_id,
                cmd,
                pass,
                health: Some(h),
                ..
            } if cmd == HEALTH_CHECK_ID => Some((work_unit_id.clone(), *pass, (**h).clone())),
            _ => None,
        })
        .collect()
}

fn unit(store: &Arc<dyn TaskStore>, task_id: TaskId, key: &str) -> task_core::WorkUnitRow {
    store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == key)
        .unwrap_or_else(|| panic!("unit {key}"))
}

fn with_check(mut spec: task_core::WorkUnitSpec) -> task_core::WorkUnitSpec {
    spec.checks = vec![task_core::WorkUnitCheck {
        cmd: "true".into(),
        expect_exit: 0,
        scope: false,
    }];
    spec
}

/// 不変条件: WU の check は health を同じ列の最後の項目として評価し、`WorkUnitCheckFinished.health` に base =
/// `wu.base_commit` の証跡（合格の stub で Pass）を残す。
#[tokio::test]
async fn health_enforce_work_unit_check_records_evidence() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![with_check(v2_wu("a", "build", &[]))],
    );
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(5)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    run_until_idle(&mut d, 800).await;
    let a = unit(&store, task.id, "a");
    let items = health_items(&events_of(&store, task.id));
    let (_, pass, evidence) = items
        .iter()
        .find(|(id, ..)| *id == a.id)
        .unwrap_or_else(|| panic!("no health item for the work unit: {items:?}"));
    assert!(pass);
    assert_eq!(evidence.verdict, HealthVerdict::Pass, "{evidence:?}");
    assert_eq!(Some(&evidence.base_sha), a.base_commit.as_ref());
    assert!(!evidence.head_sha.is_empty() && !evidence.head_tree.is_empty());
}

/// 不変条件: WU の check の health の決定的な Fail は WU の repair（checks の不合格としての retry）に回り、不合格の記録に
/// cmd `HEALTH_CHECK_ID` と reason `health:<rule_id>:<hash>（evidence: …）` が残る。合格にしない。
#[tokio::test]
async fn health_enforce_work_unit_check_fail_routes_to_work_unit_repair() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    commit_base_checker(repo.path(), 1);
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![with_check(v2_wu("a", "build", &[]))],
    );
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(5)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    run_until_idle(&mut d, 800).await;
    let events = events_of(&store, task.id);
    let a = unit(&store, task.id, "a");
    let items = health_items(&events);
    assert!(
        items
            .iter()
            .any(|(id, pass, h)| *id == a.id && !pass && h.verdict == HealthVerdict::Fail),
        "{items:?}"
    );
    let failed: Vec<&task_core::FailedWorkUnitCheck> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorkUnitChecksFailed {
                work_unit_id,
                failed,
                ..
            } if *work_unit_id == a.id => Some(failed),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        !failed.is_empty(),
        "the health Fail did not fail the checks"
    );
    for f in &failed {
        assert_eq!(f.cmd, HEALTH_CHECK_ID);
        assert!(
            f.detail
                .starts_with("health:deps.violation:0000000000000001（evidence: src/lib.rs）"),
            "{}",
            f.detail
        );
    }
    assert_ne!(a.status, task_core::WorkUnitStatus::Done, "{a:?}");
    assert_ne!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

/// 不変条件: WU の check の health の Error（検査器の exit 2）は合格にも WU の repair にもならず、task を Blocked にして
/// `health_check_error: <理由>` を人に聞く。
#[tokio::test]
async fn health_enforce_work_unit_check_error_blocks_task() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    commit_base_checker(repo.path(), 2);
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![with_check(v2_wu("a", "build", &[]))],
    );
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(5)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    run_until_idle(&mut d, 800).await;
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let events = events_of(&store, task.id);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::WorkUnitChecksFailed { .. })),
        "a health Error must not be retried as a check failure"
    );
    let asked = serde_json::to_string(&events).unwrap();
    assert!(asked.contains("health_check_error: "), "{asked}");
    let items = health_items(&events);
    assert!(
        items
            .iter()
            .any(|(_, pass, h)| !pass && h.verdict == HealthVerdict::Error),
        "{items:?}"
    );
}

/// 段の統合の試験の土台: 葉の WU 2 件（checks 無し。WU の check は走らない）を 1 段で統合する。
fn integration_scenario(repo: &Path, store: &Arc<dyn TaskStore>) -> (Task, Arc<ParallelWuAdapter>) {
    init_test_repo(repo);
    let task = parallel_task(repo, "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5))
            .with_file("a", "a.txt", "a\n")
            .with_file("b", "b.txt", "b\n")
            .with_file("repair-build-1", "FIXED", "fixed\n"),
    );
    (task, adapter)
}

/// 不変条件: 段の統合は health を評価し、`IntegrationCheckFinished.health` に base = 段の基点（段の最初の unit の
/// `base_commit`）、head = 統合の HEAD（`PhaseIntegrated.head`）の証跡を残し、`PhaseIntegrated.checks` の列にも足す。
#[tokio::test]
async fn health_enforce_phase_integration_records_evidence() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task, adapter) = integration_scenario(repo.path(), &store);
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let events = events_of(&store, task.id);
    let integ = unit(&store, task.id, "integrate-build");
    let first = unit(&store, task.id, "a");
    let (head, checks) = events
        .iter()
        .find_map(|e| match e {
            Event::PhaseIntegrated { head, checks, .. } => Some((head.clone(), checks.clone())),
            _ => None,
        })
        .expect("PhaseIntegrated");
    let items = health_items(&events);
    let (_, pass, evidence) = items
        .iter()
        .find(|(id, ..)| *id == integ.id)
        .unwrap_or_else(|| panic!("no health item for the integration: {items:?}"));
    assert!(pass);
    assert_eq!(evidence.verdict, HealthVerdict::Pass);
    assert_eq!(evidence.head_sha, head);
    assert_eq!(Some(&evidence.base_sha), first.base_commit.as_ref());
    assert!(
        checks.iter().any(|c| c.cmd == HEALTH_CHECK_ID && c.pass),
        "{checks:?}"
    );
}

/// 不変条件: 段の統合の health の決定的な Fail は段の統合の自動修復（`schedule_integration_check_repair`）に回り、
/// repair WU の class は rule ごとの bucket `health:<rule_id>` になる。replan や合格にしない。
#[tokio::test]
async fn health_enforce_phase_integration_fail_schedules_repair() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task, adapter) = integration_scenario(repo.path(), &store);
    commit_base_checker(repo.path(), 1);
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let events = events_of(&store, task.id);
    let repair = unit(&store, task.id, "repair-build-1");
    assert_eq!(repair.kind, task_core::WorkUnitKind::Repair);
    assert!(
        repair
            .spec
            .title
            .starts_with("repair (health:deps.violation): "),
        "{}",
        repair.spec.title
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::RepairScheduled { class, .. } if class == "health:deps.violation"
        )),
        "no health repair was scheduled"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::AutoReplanRequested { .. })),
        "a repairable health Fail must not replan"
    );
}

/// 不変条件: 統合の修復の後は、修復の commit を含む新しい統合の HEAD で health を評価し直し、前の証跡（Fail）を使わない。
#[tokio::test]
async fn health_enforce_integration_repair_reevaluates_new_head() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task, adapter) = integration_scenario(repo.path(), &store);
    commit_base_checker(repo.path(), 1);
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let events = events_of(&store, task.id);
    let integ = unit(&store, task.id, "integrate-build");
    let runs: Vec<(bool, HealthEvidence)> = health_items(&events)
        .into_iter()
        .filter(|(id, ..)| *id == integ.id)
        .map(|(_, pass, h)| (pass, h))
        .collect();
    assert!(runs.len() >= 2, "{runs:?}");
    let (failed, passed) = (&runs[0], &runs[runs.len() - 1]);
    assert!(
        !failed.0 && failed.1.verdict == HealthVerdict::Fail,
        "{failed:?}"
    );
    assert!(
        passed.0 && passed.1.verdict == HealthVerdict::Pass,
        "{passed:?}"
    );
    assert_ne!(passed.1.head_sha, failed.1.head_sha);
    assert_eq!(passed.1.base_sha, failed.1.base_sha);
    let head = events
        .iter()
        .find_map(|e| match e {
            Event::PhaseIntegrated { head, .. } => Some(head.clone()),
            _ => None,
        })
        .expect("PhaseIntegrated after the repair");
    assert_eq!(passed.1.head_sha, head);
}

/// 不変条件: 段の統合の health が Unavailable（base にも同梱にも検査器が無い）なら合格にも repair・replan にもせず、
/// 統合を止めて task を Blocked にし、`health_check_error: checker_missing` を人に聞く。
#[tokio::test]
async fn health_enforce_phase_integration_unavailable_blocks_task() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task, adapter) = integration_scenario(repo.path(), &store);
    let mut d = dispatcher_without_bundled_checker(store.clone(), adapter, 3);
    d.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(root.path().to_path_buf(), None);
    d.config.execution.max_parallel_work_units = 3;
    d.set_account_limit("implicit", "p1", Some(3)).unwrap();
    run_until_idle(&mut d, 800).await;
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let events = events_of(&store, task.id);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::PhaseIntegrated { .. }
                | Event::AutoReplanRequested { .. }
                | Event::RepairScheduled { .. }
        )),
        "an unavailable health check must not pass, repair or replan"
    );
    let asked = events.iter().any(|e| {
        matches!(e, Event::QuestionRaised { text, .. } if text.starts_with("health_check_error: checker_missing"))
    });
    assert!(asked, "{events:?}");
    let integ = unit(&store, task.id, "integrate-build");
    assert_eq!(integ.status, task_core::WorkUnitStatus::Blocked);
}
