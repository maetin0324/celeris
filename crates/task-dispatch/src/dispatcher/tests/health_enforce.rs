//! ADR 2026-11-08-health-enforcement-loop D1.2・D2.2・D3.2・D3.4: WU の check・段の統合・統合の修復の後が、同じ入口
//! （`run_integration_checks` → `check_item::run_item` → `gate::health_gate`）で repo health を評価し、
//! `WorkUnitCheckFinished`・`IntegrationCheckFinished` の `health` 欄に証跡を残し、Fail を既存の repair へ、Error・
//! Unavailable を Blocked（`health_check_error:`）に回す流れの試験。
//!
//! 入口の試験（`health_enforce_entry_*`）は分類だけを見て、経路が項目を列に足すか・repair と Blocked のどちらに回すか・
//! 修復の後に評価し直すかを通らない。ここはそれを dispatcher の流れで確かめる。Fail と Error は試験の repo の base に
//! commit した stub の検査器（適用判定は base の検査器を同梱より先に選ぶ）で作り、Unavailable は同梱の検査器を設定
//! しない土台（`dispatcher_without_bundled_checker`）で作る。外部ネットワークに出ず、LLM を呼ばない。
//!
//! Celeris でない repo（`health_enforce_non_celeris_*`。D1.4）: base に検査器が無く、repo の test 台本も check-health を
//! 呼ばない repo では、同梱の検査器が WU の check と direct の review の両方で当たる。土台の合格の stub の代わりに、同じ
//! [`STUB`] を同梱の配置に書いて process に設定し（[`configure_failing_bundled_checker`]。nextest は試験ごとに process
//! を分けるので他の試験の設定とぶつからない）、土台の `new_test_dispatcher`（合格の stub を設定しない）で流す。
use super::*;
use crate::health_review::gate::{CHECKER_TREE_FILE, configure_bundled_checker};
use task_core::required_checks::health::{
    CHECKER_SCRIPT, CheckerId, CheckerSource, HEALTH_CHECK_ID, HealthEvidence, HealthVerdict,
    SOURCE_SIZE_REPORT,
};

/// base の stub の検査器。`--print-review-policy` は `[review]` の無い repo の方針。report の mode は、作業ツリー
/// （`--repo`）に `FIXED` があれば合格、無ければ隣の `stub.json` の `exit` と report（fail 級の `deps.violation`）。
/// finding の fingerprint は作業ツリーの `FINDING` の 1 行。無ければ `deps.violation:` に作業ツリーの `FP` の数（無ければ 1）
/// を 16 桁で続けたもの（既定は `deps.violation:0000000000000001`）。往復の試験〈`health_limits_integration.rs`〉は `FP`、
/// 修復ごとに指摘を変える試験〈`health_limits_review_*`〉は `FINDING` を書く。
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
fp_file = os.path.join(repo, "FP")
number = int(open(fp_file).read()) if os.path.exists(fp_file) else 1
code = 0 if fixed else stub["exit"]
fp = "deps.violation:%016d" % number
if os.path.exists(os.path.join(repo, "FINDING")):
    with open(os.path.join(repo, "FINDING")) as f:
        fp = f.read().strip()
findings = [] if fixed else [{"rule_id": "deps.violation", "fingerprint": fp,
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

/// `root` の下に stub の検査器（`exit` は FIXED が無いときの exit）を repo 相対の配置で書く（base の検査器と同梱の
/// 検査器が同じ file を使う）。
fn write_checker(root: &Path, exit: i32) {
    let dir = root.join("scripts/dev/repo-health");
    std::fs::create_dir_all(dir.join("health")).unwrap();
    std::fs::write(root.join(CHECKER_SCRIPT), STUB).unwrap();
    std::fs::write(dir.join("stub.json"), format!("{{\"exit\": {exit}}}")).unwrap();
    std::fs::write(dir.join("health/engine.py"), ENGINE).unwrap();
    std::fs::write(dir.join("health/__init__.py"), "").unwrap();
    let side = root.join(SOURCE_SIZE_REPORT);
    std::fs::create_dir_all(side.parent().unwrap()).unwrap();
    std::fs::write(side, "# side\n").unwrap();
}

/// 試験の repo に stub の検査器（`exit` は FIXED が無いときの exit）を commit する。
pub(super) fn commit_base_checker(repo: &Path, exit: i32) {
    write_checker(repo, exit);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "checker"]);
}

/// `(work_unit_id, cmd, pass, health)` の health の項目の終了 event（WU の check と段の統合の両方）。
pub(super) fn health_items(events: &[Event]) -> Vec<(String, bool, HealthEvidence)> {
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

pub(super) fn unit(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
    key: &str,
) -> task_core::WorkUnitRow {
    store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == key)
        .unwrap_or_else(|| panic!("unit {key}"))
}

pub(super) fn with_check(mut spec: task_core::WorkUnitSpec) -> task_core::WorkUnitSpec {
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

/// Celeris でない repo の test 台本（check-health を呼ばない）。
const NON_CELERIS_TEST: &str = "scripts/test.sh";

/// Celeris でない repo（base に `scripts/dev/repo-health` が無く、test 台本 [`NON_CELERIS_TEST`] が check-health を
/// 呼ばない）を `repo` に作る。
fn init_non_celeris_repo(repo: &Path) {
    init_test_repo(repo);
    let script = repo.join(NON_CELERIS_TEST);
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(
        &script,
        "#!/bin/sh\n# the repo's own tests (no repo health)\nexit 0\n",
    )
    .unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "tests"]);
    assert!(!repo.join("scripts/dev/repo-health").exists());
    assert!(
        !std::fs::read_to_string(&script)
            .unwrap()
            .contains("check-health")
    );
}

/// [`STUB`]（FIXED が無ければ exit 1 と new の error の `deps.violation`）を release の同梱の配置
/// （`<dir>/scripts/dev/repo-health/`・`<dir>/CHECKER_TREE`）に書き、process の同梱の検査器に設定する。返す dir は
/// 試験の間持ち続ける（process の設定がこの dir を指す）。
fn configure_failing_bundled_checker() -> (tempfile::TempDir, CheckerId) {
    let bundle = tempfile::tempdir().unwrap();
    write_checker(bundle.path(), 1);
    std::fs::write(
        bundle.path().join(CHECKER_TREE_FILE),
        format!("repo-health {}\n", "b".repeat(40)),
    )
    .unwrap();
    let configured = configure_bundled_checker(bundle.path())
        .unwrap_or_else(|why| panic!("同梱の検査器を設定できない: {why}"));
    assert_eq!(configured.id.source, CheckerSource::Bundled);
    let id = configured.id.clone();
    (bundle, id)
}

/// 合格の stub を設定しない土台の dispatcher（同梱の検査器は [`configure_failing_bundled_checker`] が設定したもの）。
pub(super) fn non_celeris_dispatcher(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    root: &Path,
    limit: usize,
) -> Dispatcher {
    let mut d = new_test_dispatcher(store, adapter, limit, false);
    d.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(root.to_path_buf(), None);
    d.config.execution.max_parallel_work_units = limit;
    d.set_account_limit("implicit", "p1", Some(limit)).unwrap();
    d
}

/// 作業ツリーに FIXED を書いて commit する（health の修復の代わり）。
pub(super) fn commit_fixed(dir: &Path) {
    std::fs::write(dir.join("FIXED"), "fixed\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "fix the health finding"]);
}

/// 不変条件: Celeris でない repo の WU の check は同梱の検査器で health を評価し（証跡の検査器は同梱の版）、決定的な
/// new の error（`deps.violation`）を checks の不合格として WU の repair（retry）に回す。retry が直した HEAD は評価し
/// 直して合格にする。base の検査器の試験（`health_enforce_work_unit_check_*`）は同梱の検査器を通らず、入口の試験
/// （`health_enforce_entry_non_celeris_*`）は WU の check の流れを通らないので、ここで確かめる。
#[tokio::test]
async fn health_enforce_non_celeris_work_unit_check_routes_to_repair() {
    let (_bundle, bundled) = configure_failing_bundled_checker();
    let repo = tempfile::tempdir().unwrap();
    init_non_celeris_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut spec = v2_wu("a", "build", &[]);
    spec.checks = vec![task_core::WorkUnitCheck {
        cmd: format!("sh {NON_CELERIS_TEST}"),
        expect_exit: 0,
        scope: false,
    }];
    adopt_v2_plan(&store, task.id, &["build"], vec![spec]);
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = runs.clone();
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5)).with_action("a", move |dir| {
            if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                commit_fixed(dir);
            }
        }),
    );
    let mut d = non_celeris_dispatcher(store.clone(), adapter, root.path(), 2);
    run_until_idle(&mut d, 800).await;
    let events = events_of(&store, task.id);
    let a = unit(&store, task.id, "a");
    let items: Vec<(bool, HealthEvidence)> = health_items(&events)
        .into_iter()
        .filter(|(id, ..)| *id == a.id)
        .map(|(_, pass, h)| (pass, h))
        .collect();
    let (failed, passed) = match items.as_slice() {
        [first, .., last] => (first, last),
        _ => panic!("the work unit check did not evaluate health twice: {items:?}"),
    };
    assert!(
        !failed.0 && failed.1.verdict == HealthVerdict::Fail,
        "{failed:?}"
    );
    assert_eq!(failed.1.checker.as_ref(), Some(&bundled));
    assert_eq!(failed.1.blocking, vec!["deps.violation:0000000000000001"]);
    assert!(
        passed.0 && passed.1.verdict == HealthVerdict::Pass,
        "{passed:?}"
    );
    assert_eq!(passed.1.checker.as_ref(), Some(&bundled));
    assert_ne!(passed.1.head_sha, failed.1.head_sha);
    let failed_checks: Vec<&task_core::FailedWorkUnitCheck> = events
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
    assert_eq!(failed_checks.len(), 1, "{failed_checks:?}");
    assert_eq!(failed_checks[0].cmd, HEALTH_CHECK_ID);
    assert!(
        failed_checks[0]
            .detail
            .starts_with("health:deps.violation:0000000000000001（evidence: src/lib.rs）"),
        "{}",
        failed_checks[0].detail
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::AutoReplanRequested { .. } | Event::QuestionRaised { .. }
        )),
        "a deterministic health Fail must go to the work unit repair"
    );
    assert_eq!(a.status, task_core::WorkUnitStatus::Done, "{a:?}");
}

/// 不変条件: Celeris でない repo の atomic/direct の task の review は同梱の検査器で health を評価し
/// （`RequiredChecksEvaluated.results[].health` の検査器は同梱の版）、決定的な new の error を review の repair
/// （`RepairScheduled` の class `health:deps.violation`、origin `Review`）に回す。修復の後の review は新しい候補で評価し
/// 直して合格にし、task を done にする。unit の試験（`health_enforce_direct_review_*`）は review を直接呼び、
/// dispatcher の review の起動・修復・再 review を通らないので、ここで確かめる。
#[tokio::test]
async fn health_enforce_non_celeris_direct_review_routes_to_repair() {
    let (_bundle, bundled) = configure_failing_bundled_checker();
    let repo = tempfile::tempdir().unwrap();
    init_non_celeris_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = super::target_sync::reviewing_task(repo.path(), &store);
    super::target_sync::set_check(&mut task, format!("sh {NON_CELERIS_TEST}"));
    store.insert(&task).unwrap();
    let adapter = Arc::new(RepairActionAdapter::new(|dir, _| commit_fixed(dir)));
    let mut d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    std::fs::create_dir_all(wt.dir.join("src")).unwrap();
    let head = super::target_sync::commit_file(&wt.dir, "src/lib.rs", "pub fn foo() {}\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 800).await.idle);
    let events = events_of(&store, task.id);
    let health: Vec<(String, bool, HealthEvidence)> = events
        .iter()
        .filter_map(|e| match e {
            Event::RequiredChecksEvaluated {
                reviewed_sha,
                results,
                ..
            } => Some((reviewed_sha, results)),
            _ => None,
        })
        .flat_map(|(sha, results)| {
            results
                .iter()
                .filter(|r| r.cmd == HEALTH_CHECK_ID)
                .filter_map(move |r| Some((sha.clone(), r.pass, (**r.health.as_ref()?).clone())))
        })
        .collect();
    let (failed, passed) = match health.as_slice() {
        [first, .., last] => (first, last),
        _ => panic!("the review did not evaluate health twice: {health:?}"),
    };
    assert_eq!(failed.0, head);
    assert!(
        !failed.1 && failed.2.verdict == HealthVerdict::Fail,
        "{failed:?}"
    );
    assert_eq!(failed.2.checker.as_ref(), Some(&bundled));
    assert_eq!(failed.2.head_sha, head);
    assert!(
        passed.1 && passed.2.verdict == HealthVerdict::Pass,
        "{passed:?}"
    );
    assert_eq!(passed.2.checker.as_ref(), Some(&bundled));
    assert_ne!(passed.0, head);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_repair")),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::AutoReplanRequested { .. })),
        "a repairable health Fail must not replan"
    );
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    // D3.2・D4.1: review の修復の class も段の統合（`health_enforce_phase_integration_fail_schedules_repair`）と同じ
    // rule ごとの bucket `health:<rule_id>`（rule ごとに `max_repairs_per_class` を数える）。
    let classes: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::RepairScheduled { class, origin, .. }
                if *origin == task_core::execution::RepairOrigin::Review =>
            {
                Some(class.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(classes, vec!["health:deps.violation"], "{events:?}");
}

/// ADR-0072 D16 付記（G4-S1）。不変条件: `Check::Reviewer` のある task でも、決定的な health の Fail（base の stub の
/// 検査器の `deps.violation`）は review の repair（`RepairScheduled` の class `health:deps.violation`、attempts 不変）に回る。
/// health の Fail で reviewer run を起こさなかった reviewer の項目（`REVIEWER_NOT_EVALUATED_REASON`）は分類に入らない。
/// 修復の後の review は health を Pass にし、reviewer の合格で task を done にする。上の
/// `health_enforce_non_celeris_direct_review_routes_to_repair` は `Check::Reviewer` の無い task なので、未評価の reviewer
/// の項目が health の Fail を `Substantive`（`ReviewFail`）にする欠けを検出できない。
/// 不変条件: 決定的な health Fail は reviewer に妨げられず修復へ進む。
/// 変更理由: ADR R1・R6 により Absent でも材料が届くため、偽 reviewer にも health 判定が要る。
#[tokio::test]
async fn review_repair_not_evaluated_reviewer_does_not_block_a_health_repair() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    commit_base_checker(repo.path(), 1);
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = super::target_sync::reviewing_task(repo.path(), &store);
    super::target_sync::set_check(&mut task, "true".into());
    task.acceptance.push(Criterion {
        text: "the change is complete".into(),
        check: Check::Reviewer,
    });
    task.budget.max_retries = 0;
    store.insert(&task).unwrap();
    // `health` も書く: reviewer に health の材料を渡した review は `health` の判定を求める（I7、ADR
    // 2026-11-07-health-review-integration D5.2）。ADR 2026-11-11-health-generic-review-input の R1 で `[review]` の
    // 無い（absent）repo も材料を受けるので、この偽の reviewer も空の判定を書く（R6）。試験が守る不変条件は変えない。
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_review(
                r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"fine"}],"health":{"findings":[]}}"#,
            )
            .with_action("repair-1", commit_fixed),
    );
    let mut d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    std::fs::create_dir_all(wt.dir.join("src")).unwrap();
    super::target_sync::commit_file(&wt.dir, "src/lib.rs", "pub fn foo() {}\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 800).await.idle);
    let events = events_of(&store, task.id);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::ReviewVerdict { criterion_idx: 1, pass: false, reason, .. }
                if reason == crate::review::REVIEWER_NOT_EVALUATED_REASON
        )),
        "{events:?}"
    );
    let classes: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::RepairScheduled { class, origin, .. }
                if *origin == task_core::execution::RepairOrigin::Review =>
            {
                Some(class.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(classes, vec!["health:deps.violation"], "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_fail")),
        "{events:?}"
    );
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(
        (stored.status, stored.attempts),
        (Status::Done, 0),
        "{stored:?}"
    );
}

/// review の repair の WU（key `repair-*`）の run の後に、その作業ツリーと key で `on_repair` を呼ぶ adapter（修復の代わり。
/// ほかは [`WuScriptAdapter`]）。
pub(super) struct RepairActionAdapter {
    inner: WuScriptAdapter,
    on_repair: Box<dyn Fn(&Path, &str) + Send + Sync>,
}

impl RepairActionAdapter {
    pub(super) fn new(on_repair: impl Fn(&Path, &str) + Send + Sync + 'static) -> Self {
        Self {
            inner: WuScriptAdapter::new(HashMap::new()),
            on_repair: Box::new(on_repair),
        }
    }
}

#[async_trait]
impl WorkerAdapter for RepairActionAdapter {
    fn id(&self) -> &str {
        self.inner.id()
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let repair = req
            .context
            .work_unit
            .as_ref()
            .map(|w| w.key.clone())
            .filter(|key| key.starts_with("repair-"));
        let cwd = req.cwd().to_path_buf();
        let result = self.inner.run(req, run_id, limits, sink).await;
        if let Some(key) = repair {
            (self.on_repair)(&cwd, &key);
        }
        result
    }
}
