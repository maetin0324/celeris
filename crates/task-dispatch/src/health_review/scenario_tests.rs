//! ADR 2026-11-07-health-review-integration (6): 通常変更と高 risk 変更を dispatcher の流れ全体（planner の計画 →
//! risk の判定 →（高 risk だけ）設計 review → 採用 → WU → final review → 判定）で区別する。
//!
//! - 案件の repo（一時 git repo）の main に stub の `repo_health.py`（`--print-review-policy` は高 risk の class
//!   `store` = `^crates/task-store/` と owner 地図を出し、report の mode は固定の report を書く）と owner 地図を置く。
//! - 試験の adapter が planner・設計 review・WU・final review の run を受ける。WU は計画の write path に file を
//!   書いて commit する。外部ネットワークに出ず、LLM を呼ばない。
//! - `health_review_scenario_real_cli` は repo の本物の検査器（`scripts/dev/repo-health/`）を一時 git repo に
//!   写し、`--print-review-policy` の出力を [`load_review_policy`] が読めることを確かめる。
//!
//! `dispatcher/tests/mod.rs` から `#[path]` で読む（`dispatcher::tests` の試験の道具を使う）。

use super::*;

use crate::health_review::policy::{
    CHECKER_DIR, CHECKER_SCRIPT, CHECKER_SIDE_FILES, REVIEW_POLICY_SCHEMA, REVIEW_POLICY_TIMEOUT,
    load_review_policy,
};
use task_core::design_review::{ReviewPolicyState, RiskLevel};
use task_worker::protocol::HealthReviewInput;

const DIGEST: &str = "5ce7a210000000000000000000000000000000000000000000000000000000ab";
const NORMAL_PATH: &str = "crates/foo/src/lib.rs";
const STORE_PATH: &str = "crates/task-store/src/rows.rs";
const OWNER_MAP: &str = "owners/map.md";

/// stub の report（new の finding 1 件・existing 1 件）。
fn report() -> serde_json::Value {
    let finding = |rule: &str, delta: &str, path: &str| {
        serde_json::json!({"rule_id": rule, "fingerprint": format!("{rule}:00000000000000aa"),
            "severity": "error", "delta": delta, "exception": null, "evidence": [{"path": path}]})
    };
    serde_json::json!({
        "schema": "celeris.repo-health.report/1",
        "summary": {"verdict": "fail", "exit_code": 1, "reviewable": true,
            "counts": {"findings": 2, "fail_grade": 1, "new": 1, "worsened": 0, "existing": 1}},
        "findings": [
            finding("size.responsibility-growth", "new", NORMAL_PATH),
            finding("arch-map.missing_invariant", "existing", OWNER_MAP),
        ],
        "policy": {"version": "1", "source": "base", "changed": false, "files": []},
    })
}

/// main に `[review]` を持つ stub の検査器と owner 地図を commit した repo（`with_policy = false` なら README だけ）。
fn project_repo(with_policy: bool) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    if !with_policy {
        return repo;
    }
    let policy = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA, "source": "base", "owner_map": OWNER_MAP,
        "high_risk": [{"class": "store", "paths": ["^crates/task-store/"]}],
        "invariants": {"events-append-only": "CLAUDE.md"}, "definition_sha256": DIGEST,
    });
    let script = format!(
        "import sys\nargs = sys.argv[1:]\nif '--print-review-policy' in args:\n    sys.stdout.write({policy:?})\n    sys.exit(0)\nopen(args[args.index('--out') + 1], 'w').write({report:?})\nsys.exit(1)\n",
        policy = policy.to_string(),
        report = report().to_string(),
    );
    let path = repo.path().join(CHECKER_SCRIPT);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, script).unwrap();
    std::fs::create_dir_all(repo.path().join("owners")).unwrap();
    std::fs::write(
        repo.path().join(OWNER_MAP),
        "| subsystem | entry |\n|---|---|\n| foo | `crates/foo/src/lib.rs` |\n| store | `crates/task-store/` |\n",
    )
    .unwrap();
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-q", "-m", "checker"]);
    repo
}

/// /3 の計画（leaf 1 つ、write path は `path`）。`new_state` を宣言した設計 brief を付けるかを選ぶ。
fn plan(path: &str, new_state: bool) -> String {
    let mut unit = super::tree::leaf("a", "s1", &[]);
    unit["context"] = serde_json::json!({"paths": [path]});
    let mut plan: serde_json::Value = serde_json::from_str(&super::tree::v3_plan(
        vec![super::tree::stage("s1", false)],
        vec![unit],
    ))
    .unwrap();
    if new_state {
        plan["design_brief"] = serde_json::json!({
            "root_cause": "rows are rewritten in place",
            "owner": "task-store",
            "options": [
                {"name": "append a column", "chosen": true, "reason": "keeps history"},
                {"name": "rewrite rows", "chosen": false, "reason": "loses history"}
            ],
            "new_state": ["rows.revision column"],
        });
    }
    plan.to_string()
}

/// planner は `plans` を順に出し、設計 review は合格を返し、WU は `change`（計画の write path）に file を書いて
/// commit し、final review は `ReviewRequest.health` を記録して `health_findings` を `ReviewOutput.health` に返す。
struct ScenarioAdapter {
    plans: StdMutex<std::collections::VecDeque<String>>,
    change: &'static str,
    health_findings: Vec<serde_json::Value>,
    seen: StdMutex<Vec<task_worker::RunContext>>,
}

impl ScenarioAdapter {
    fn new(
        plans: Vec<String>,
        change: &'static str,
        health_findings: Vec<serde_json::Value>,
    ) -> Arc<Self> {
        Arc::new(ScenarioAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
            change,
            health_findings,
            seen: StdMutex::new(Vec::new()),
        })
    }
}

fn done(summary: &str) -> RunOutcome {
    RunOutcome {
        terminal: Terminal::Done {
            summary: summary.into(),
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    }
}

/// run の cwd の中の git の作業ツリー（案件の repo は `repos/<name>/` の下にあることがある）。
fn git_tree(req: &RunRequest) -> PathBuf {
    let cwd = req
        .work_dir
        .clone()
        .unwrap_or_else(|| req.workspace.clone());
    let nested = cwd.join("repos").join("code");
    if nested.join(".git").exists() {
        nested
    } else {
        cwd
    }
}

#[async_trait]
impl WorkerAdapter for ScenarioAdapter {
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
        self.seen.lock().unwrap().push(req.context.clone());
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        if req.context.execution_planner.is_some() {
            // 計画の無い試行は前の試行の file を残さない（task の artifacts dir は run をまたいで同じ）。
            let file = req.artifacts_dir.join("execution-plan.json");
            match self.plans.lock().unwrap().pop_front() {
                Some(json) => std::fs::write(&file, json).unwrap(),
                None => {
                    let _ = std::fs::remove_file(&file);
                }
            }
            return Ok(done("planned"));
        }
        if let Some(review) = &req.context.review {
            let json = if review.design.is_some() {
                serde_json::json!({"verdicts": [], "design": {"pass": true, "findings": []}})
            } else {
                let verdicts: Vec<_> = review
                    .criteria
                    .iter()
                    .map(|c| serde_json::json!({"criterion": c, "pass": true, "reason": "fine"}))
                    .collect();
                serde_json::json!({"verdicts": verdicts, "health": {"findings": self.health_findings}})
            };
            std::fs::write(
                req.artifacts_dir.join(crate::review::REVIEW_FILE_NAME),
                json.to_string(),
            )
            .unwrap();
            return Ok(done("reviewed"));
        }
        if req.context.work_unit.is_some() {
            let tree = git_tree(&req);
            let file = tree.join(self.change);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "pub fn changed() {}\n").unwrap();
            git_out(&tree, &["add", "-A"]);
            git_out(&tree, &["commit", "-q", "-m", self.change]);
        }
        Ok(done("ok"))
    }
}

struct Outcome {
    store: Arc<dyn TaskStore>,
    task: Task,
    seen: Vec<task_worker::RunContext>,
    _repo: tempfile::TempDir,
    _root: tempfile::TempDir,
}

impl Outcome {
    fn events(&self) -> Vec<Event> {
        self.store
            .events_for(self.task.id)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
    fn planner_runs(&self) -> usize {
        self.seen
            .iter()
            .filter(|c| c.execution_planner.is_some())
            .count()
    }
    fn design_review_runs(&self) -> usize {
        self.seen
            .iter()
            .filter(|c| c.review.as_ref().is_some_and(|r| r.design.is_some()))
            .count()
    }
    fn final_reviews(&self) -> Vec<Option<HealthReviewInput>> {
        self.seen
            .iter()
            .filter_map(|c| c.review.as_ref())
            .filter(|r| r.design.is_none())
            .map(|r| r.health.clone())
            .collect()
    }
    fn reviewer_runs(&self) -> usize {
        self.seen.iter().filter(|c| c.review.is_some()).count()
    }
    fn work_unit_runs(&self) -> usize {
        self.seen.iter().filter(|c| c.work_unit.is_some()).count()
    }
    /// 最初に `pred` に当たる event の位置。
    fn position(&self, pred: impl Fn(&Event) -> bool) -> Option<usize> {
        self.events().iter().position(pred)
    }
    fn status(&self) -> Status {
        self.store.get(self.task.id).unwrap().unwrap().status
    }
}

/// 案件の repo（`with_policy` なら `[review]` のある stub の検査器付き）を持ち reviewer の受け入れ条件のある compound
/// task を、planner が `change` を書く計画を出し final review が `health_findings` を返す adapter で止まるまで回す。
async fn run_flow(
    with_policy: bool,
    change: &'static str,
    new_state: bool,
    health_findings: Vec<serde_json::Value>,
) -> Outcome {
    let repo = project_repo(with_policy);
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project, repos) =
        project_with_repos(&store, &[("code", repo.path(), task_core::RepoKind::Git)]);
    let mut task = compound_task(repo.path());
    task.workspace = WorkspaceSpec::Local {
        path: repo.path().to_path_buf(),
        mode: None,
    };
    task.project_id = Some(project);
    task.repos = repos.iter().map(RepoRef::of).collect();
    task.acceptance.push(task_core::Criterion {
        text: "the change keeps one source of truth".into(),
        check: Check::Reviewer,
    });
    store.insert(&task).unwrap();
    let adapter = ScenarioAdapter::new(vec![plan(change, new_state)], change, health_findings);
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.limits = super::tree::tree_limits();
    run_until_idle(&mut d, 400).await;
    let seen = adapter.seen.lock().unwrap().clone();
    Outcome {
        store,
        task,
        seen,
        _repo: repo,
        _root: root,
    }
}

/// (6) 通常変更: `[review]` のある repo で高 risk の class に触れない計画は、設計 review の run なしに採用され、
/// final review の `ReviewRequest.health` に diff 統計・owner 行・findings の要約が入る。reviewer run の数は
/// `[review]` の無い repo の同じ流れ（従来）と同じ 1 本で、task は合格で終わる。
#[tokio::test]
async fn health_review_scenario_normal_change() {
    let out = run_flow(true, NORMAL_PATH, false, vec![]).await;
    let baseline = run_flow(false, NORMAL_PATH, false, vec![]).await;

    assert_eq!(out.planner_runs(), 1);
    assert_eq!(out.design_review_runs(), 0, "no design review run");
    assert!(
        out.position(|e| matches!(e, Event::DesignReviewRequested { .. }))
            .is_none(),
        "no design review is requested for a low-risk plan"
    );
    assert!(
        out.position(|e| matches!(e, Event::ExecutionPlanned { .. }))
            .is_some()
    );
    assert_eq!(out.work_unit_runs(), 1);
    assert_eq!(
        out.reviewer_runs(),
        baseline.reviewer_runs(),
        "the review policy adds no reviewer run"
    );
    assert_eq!(out.reviewer_runs(), 1, "only the final review runs");
    assert_eq!(
        out.seen.len(),
        baseline.seen.len(),
        "no run is added at all"
    );
    assert_eq!(
        baseline.final_reviews(),
        vec![None],
        "no health without [review]"
    );

    let reviews = out.final_reviews();
    assert_eq!(reviews.len(), 1);
    let health = reviews[0]
        .as_ref()
        .expect("the final review carries health");
    assert_eq!(health.repos.len(), 1);
    let repo = &health.repos[0];
    assert!(repo.diff.files >= 1, "diff stats: {:?}", repo.diff);
    assert!(
        repo.diff.paths.iter().any(|p| p.path == NORMAL_PATH),
        "the changed path is in the diff stats: {:?}",
        repo.diff.paths
    );
    assert!(
        repo.owner_rows
            .iter()
            .any(|r| r.path == NORMAL_PATH && r.row.contains("| foo |")),
        "the owner row of the changed path: {:?}",
        repo.owner_rows
    );
    assert_eq!(repo.findings.counts.new, 1);
    assert_eq!(
        repo.findings
            .new
            .iter()
            .map(|f| f.rule_id.as_str())
            .collect::<Vec<_>>(),
        vec!["size.responsibility-growth"]
    );
    assert!(health.brief.is_none(), "the plan has no design brief");
    assert_eq!(out.status(), Status::Done);
    assert!(
        out.events()
            .iter()
            .all(|e| !matches!(e, Event::ReviewVerdict { pass: false, .. }))
    );
}

/// (6) 高 risk 変更: store の class の path を書き `new_state` を宣言した計画は、採用の前に設計 review の run が
/// 起き、合格の後に採用される。final review で reviewer が blocking の `dual_source_of_truth` を返すと review は
/// 不合格になり、局所修復でなく replan（planner の `replan` run）に回る。
#[tokio::test]
async fn health_review_scenario_high_risk_change() {
    let blocking = serde_json::json!({
        "kind": "dual_source_of_truth", "severity": "blocking", "path": STORE_PATH,
        "reason": "rows.revision and the events both hold the revision",
    });
    let out = run_flow(true, STORE_PATH, true, vec![blocking]).await;

    // 設計 review は採用の前。
    let requested = out
        .position(|e| matches!(e, Event::DesignReviewRequested { .. }))
        .expect("a design review is requested");
    let reviewed = out
        .position(|e| matches!(e, Event::DesignReviewed { pass: true, .. }))
        .expect("the design review passes");
    let adopted = out
        .position(|e| matches!(e, Event::ExecutionPlanned { .. }))
        .expect("the plan is adopted");
    assert!(requested < reviewed && reviewed < adopted);
    let events = out.events();
    let Some(Event::DesignReviewRequested { risk, .. }) = events.get(requested) else {
        unreachable!()
    };
    assert_eq!(risk.level, RiskLevel::High);
    assert_eq!(out.design_review_runs(), 1);
    assert_eq!(out.work_unit_runs(), 1, "the adopted plan's unit ran once");

    // final review は health を受け、blocking の finding で不合格になる。
    let reviews = out.final_reviews();
    assert_eq!(reviews.len(), 1, "one final review run");
    let health = reviews[0]
        .as_ref()
        .expect("the final review carries health");
    assert!(
        health.repos[0]
            .diff
            .paths
            .iter()
            .any(|p| p.path == STORE_PATH)
    );
    assert!(
        health.brief.is_some(),
        "the adopted plan's brief reaches the reviewer"
    );
    let failed = out
        .position(|e| {
            matches!(e, Event::ReviewVerdict { pass: false, reason, .. }
                if reason.contains("health:dual_source_of_truth"))
        })
        .expect("the blocking finding fails the review");
    assert!(adopted < failed);

    // 局所修復（repair WU）でなく replan。
    let units = out.store.work_units_for(out.task.id).unwrap();
    assert!(
        units
            .iter()
            .all(|u| u.kind != task_core::WorkUnitKind::Repair),
        "no repair unit: {:?}",
        units.iter().map(|u| (&u.key, u.kind)).collect::<Vec<_>>()
    );
    let replans: Vec<_> = out
        .seen
        .iter()
        .filter_map(|c| c.execution_planner.as_ref())
        .filter(|p| p.replan)
        .collect();
    assert!(!replans.is_empty(), "the planner runs again as a replan");
    assert!(
        replans[0].replan_reason.contains("dual_source_of_truth"),
        "replan reason: {}",
        replans[0].replan_reason
    );
}

/// 本物の検査器の file（`scripts/dev/repo-health/` と side file）を `dest` に写す（`__pycache__` は除く）。
fn copy_checker(src_root: &std::path::Path, dest: &std::path::Path) {
    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if name == "__pycache__" {
                continue;
            }
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&path, &to.join(&name));
            } else {
                std::fs::copy(&path, to.join(&name)).unwrap();
            }
        }
    }
    copy_dir(&src_root.join(CHECKER_DIR), &dest.join(CHECKER_DIR));
    for side in CHECKER_SIDE_FILES {
        let from = src_root.join(side);
        if from.exists() {
            std::fs::create_dir_all(dest.join(side).parent().unwrap()).unwrap();
            std::fs::copy(&from, dest.join(side)).unwrap();
        }
    }
}

/// (6) 本物の CLI: repo の `scripts/dev/repo-health/repo_health.py --print-review-policy`（review-policy-cli が
/// 作った）の出力を、policy-load の loader（[`load_review_policy`]）が base の commit から読める。profile は repo の
/// `default.toml`（extends `celeris`）で、`[review]` は celeris の値になる。
#[test]
fn health_review_scenario_real_cli() {
    let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    copy_checker(&src_root, repo.path());
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-q", "-m", "real checker"]);
    let base = git_out(repo.path(), &["rev-parse", "HEAD"]);
    // 本物の出力の key の組は stub（この file と他の health_review_ 試験）と同じ。
    let printed = std::process::Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(src_root.join(CHECKER_SCRIPT))
        .args(["--print-review-policy", "--repo"])
        .arg(repo.path())
        .args(["--base", &base])
        .output()
        .unwrap();
    assert!(
        printed.status.success(),
        "{}",
        String::from_utf8_lossy(&printed.stderr)
    );
    let raw: serde_json::Value = serde_json::from_slice(&printed.stdout).unwrap();
    let keys: Vec<&str> = raw
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut stub_keys: Vec<&str> = vec![
        "schema",
        "source",
        "owner_map",
        "high_risk",
        "invariants",
        "definition_sha256",
    ];
    stub_keys.sort_unstable();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, stub_keys);
    assert_eq!(raw["schema"], REVIEW_POLICY_SCHEMA);
    assert_eq!(raw["source"], "base");

    // 候補（作業ツリー）の profile の変更は読まない（I3）: `[review]` の無い profile に書き換えても値は変わらない。
    std::fs::write(
        repo.path()
            .join(CHECKER_DIR)
            .join("profiles")
            .join("default.toml"),
        "version = \"1\"\n",
    )
    .unwrap();
    let state = load_review_policy(repo.path(), &base, REVIEW_POLICY_TIMEOUT);
    let ReviewPolicyState::Present(policy) = state else {
        panic!("the real --print-review-policy output is not readable: {state:?}");
    };
    // loader の値は CLI の出力と同じ（base の commit の検査器と profile を使う）。
    assert_eq!(policy.definition_sha256, raw["definition_sha256"]);
    assert_eq!(
        serde_json::to_value(&policy.high_risk).unwrap(),
        raw["high_risk"]
    );
    assert_eq!(
        serde_json::to_value(&policy.invariants).unwrap(),
        raw["invariants"]
    );
    assert_eq!(
        policy.owner_map.as_deref(),
        Some("docs/architecture-map.md")
    );
    assert_eq!(policy.definition_sha256.len(), 64);
    let store = policy
        .high_risk
        .iter()
        .find(|c| c.class == "store")
        .expect("the celeris profile has the store class");
    assert!(
        store
            .paths
            .iter()
            .any(|p| p == "^crates/task-core/src/store/"),
        "store paths: {:?}",
        store.paths
    );
    for class in ["credential", "sandbox"] {
        assert!(
            policy.high_risk.iter().any(|c| c.class == class),
            "class {class} in {:?}",
            policy.high_risk
        );
    }
}
