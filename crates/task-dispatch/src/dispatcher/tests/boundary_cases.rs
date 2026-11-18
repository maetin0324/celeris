//! ADR 2026-11-17-boundary-change-risk-review D5（WorkUnit `cases`）: 安全境界の意味を変える改修の 6 ケース
//! （試験名 `boundary_case_<n>_`）。実際の判定経路を通す: 採用前は dispatcher の設計の関門（`design_gate` →
//! `assess_risk` → `DesignReviewRequested`・設計 review の run）、final review は `build_final_review_material`
//! （差分の risk と `ReviewRequest.diff_design`）・`Verdict::from_final_review`・`HealthInputJob::attach`、health の関門は
//! `health_gate`、policy の関門は `Verdict::from_review_json` と `policy_review_gate`。
//!
//! ケース 1・2・3・5 は実 profile を当てる: この worktree の `scripts/dev/repo-health/`（`celeris.toml` を含む）と
//! `docs/architecture-map.md` を一時 git repo の main に commit し、本物の検査器の `--print-review-policy` が base の
//! commit から読んだ方針で判定する。外部ネットワークに出ず、LLM を呼ばない。
//!
//! 守る不変条件と、既存の試験で検出できない理由:
//! - 既存の `review_risk_*`・`health_review_design_gate_*`・`design_review` の試験は stub の方針（class 1〜3 個）を当て、
//!   採用前と final review を別々に見る。実 profile の class（`authz`・`review-gate`）と不変条件の名前で、1 つの task の
//!   採用前の設計 review が final review の要求をどの組だけ外すか、移動・削除・`#[path]` の付け替えで計画の宣言を class
//!   の外にしても差分が class を拾うか、候補が自分の profile を緩めても base 側の方針で判定するかは見ていない。
//! - 判定を読めない・取れない・判定の無い review は合格にしない（fail-closed。J4）。

use super::health_review_design_verdict::{DesignScriptAdapter, Script};
use super::health_review_scenario::copy_checker;
use super::*;

use crate::health_review::gate::{HealthGateDecision, HealthGateRequest, health_gate};
use crate::health_review::input::{
    CandidateRepo, HEALTH_INPUT_TIMEOUT, HealthInputJob, HealthInputWork,
    UNCOVERED_WITHOUT_REVIEWER, build_final_review_material,
};
use crate::health_review::policy::{
    REVIEW_POLICY_SCHEMA, REVIEW_POLICY_TIMEOUT, load_review_policy,
};
use crate::health_review::verdict::DIFF_DESIGN_VERDICT_MISSING;
use crate::review::{
    FinalReviewMaterial, HealthItem, RepoHealth, RequiredCheckBinding, RequiredRepoChecks,
    RequiredRepoResult, ReviewOutcome, Verdict,
};
use task_core::design_review::{RiskAssessment, RiskLevel, assess_risk};
use task_core::required_checks::health::{CHECKER_DIR, HEALTH_CHECK_ID};
use task_worker::change_rules::review::{design_section, health_section};
use task_worker::protocol::{
    DiffStat, HealthPolicyState, HealthRepoInput, HealthReviewInput, PathStat, ReviewRequest,
};

const MIDDLEWARE: &str = "crates/task-api/src/middleware.rs";
const PRINCIPAL: &str = "crates/task-api/src/principal.rs";
/// `dispatcher.rs` が `#[path]` で読む file（論理 module は dispatcher、物理 path は `health_review/`）。
const DESIGN_GATE: &str = "crates/task-dispatch/src/health_review/design_gate.rs";
const DISPATCHER: &str = "crates/task-dispatch/src/dispatcher.rs";
const PROFILE: &str = "scripts/dev/repo-health/profiles/celeris.toml";
const OWNER_MAP: &str = "docs/architecture-map.md";
const AUTHZ_INVARIANTS: [&str; 2] = [
    "invariant:authz:api-principal-from-token",
    "invariant:authz:cos-audit-same-tx",
];
/// 境界でない普通の path（画面・class 外の handler・文書・class に足さなかった WU の gate・worker の adapter）。
const PLAIN: [&str; 5] = [
    "web/src/App.tsx",
    "crates/task-api/src/handlers/org.rs",
    "docs/SPEC.md",
    "crates/task-dispatch/src/dispatcher/work_units.rs",
    "crates/task-worker/src/claude_code.rs",
];
const REQUIRE_ADMIN: &str =
    "pub fn require_admin(p: &P) -> bool {\n    p.is_admin && p.active\n}\n";
/// 認可の条件の 1 行の変更（`active` の確認を落とす）。
const REQUIRE_ADMIN_EDITED: &str = "pub fn require_admin(p: &P) -> bool {\n    p.is_admin\n}\n";

fn src_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn class_path(class: &str, path: &str) -> String {
    format!("path_class:{class}:{path}")
}

fn authz(path: &str) -> Vec<String> {
    let mut reasons = vec![class_path("authz", path)];
    reasons.extend(AUTHZ_INVARIANTS.map(String::from));
    reasons
}

fn write(dir: &std::path::Path, path: &str, body: &str) {
    std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
    std::fs::write(dir.join(path), body).unwrap();
}

/// 実 profile の一時 git repo: main に本物の検査器（`scripts/dev/repo-health/` と side file）・owner 地図・`files` を
/// commit する。返すのは repo と main の sha。
fn real_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    init_test_repo(dir);
    copy_checker(&src_root(), dir);
    write(
        dir,
        OWNER_MAP,
        &std::fs::read_to_string(src_root().join(OWNER_MAP)).unwrap(),
    );
    for (path, body) in files {
        write(dir, path, body);
    }
    git_out(dir, &["add", "-A"]);
    git_out(dir, &["commit", "-q", "-m", "real profile"]);
    let base = git_out(dir, &["rev-parse", "HEAD"]);
    (repo, base)
}

/// [`real_repo`] の main から branch `cand` を切り、`change` を当てた 1 commit の候補。
fn real_candidate(
    files: &[(&str, &str)],
    change: impl FnOnce(&std::path::Path),
) -> (tempfile::TempDir, CandidateRepo) {
    let (repo, target_sha) = real_repo(files);
    let dir = repo.path();
    git_out(dir, &["checkout", "-q", "-b", "cand"]);
    change(dir);
    git_out(dir, &["add", "-A"]);
    git_out(dir, &["commit", "-q", "-m", "cand"]);
    let reviewed_sha = git_out(dir, &["rev-parse", "HEAD"]);
    let candidate = CandidateRepo {
        repo_id: RepoId::new().to_string(),
        dir: dir.to_path_buf(),
        target_sha,
        reviewed_sha,
    };
    (repo, candidate)
}

fn work(
    store: &Arc<SqliteStore>,
    task_id: TaskId,
    c: &CandidateRepo,
    policy_only: bool,
    out: &std::path::Path,
) -> HealthInputWork {
    HealthInputWork {
        store: store.clone(),
        task_id,
        review_run: "final-1".into(),
        policy_only,
        candidates: vec![c.clone()],
        skipped: Vec::new(),
        out_dir: out.to_path_buf(),
        timeout: HEALTH_INPUT_TIMEOUT,
        evaluated: Default::default(),
    }
}

/// 本番と同じ入口で final review の材料を作る（`policy_only` は reviewer run の無い review の経路）。
fn material(
    store: &Arc<SqliteStore>,
    task_id: TaskId,
    c: &CandidateRepo,
    policy_only: bool,
    out: &std::path::Path,
) -> FinalReviewMaterial {
    build_final_review_material(&work(store, task_id, c, policy_only, out), None)
}

/// final review の `review.json`（criterion 0 の合格と、`design` の欄の文字列）を reviewer の読みで読む。
fn final_review(request: &ReviewRequest, design: &str) -> Result<Vec<Verdict>, String> {
    let text =
        format!(r#"{{"verdicts": [{{"criterion": 0, "pass": true, "reason": "ok"}}]{design}}}"#);
    Verdict::from_final_review(
        &text,
        Some(request),
        &[0],
        "reviewer(r)",
        "artifacts/review.json",
    )
}

/// /3 の計画（leaf 1 つ、write path は `paths`）。`brief` は `new_*` を 1 つも宣言しない設計 brief。
fn plan(paths: &[&str], brief: bool) -> String {
    let mut unit = super::tree::leaf("a", "s1", &[]);
    unit["context"] = serde_json::json!({ "paths": paths });
    let mut plan: serde_json::Value = serde_json::from_str(&super::tree::v3_plan(
        vec![super::tree::stage("s1", false)],
        vec![unit],
    ))
    .unwrap();
    if brief {
        plan["design_brief"] = serde_json::json!({
            "root_cause": "require_admin lets an inactive admin token through",
            "owner": "task-api middleware",
            "options": [
                {"name": "fix the condition in require_admin", "chosen": true, "reason": "the owner decides"},
                {"name": "check in every handler", "chosen": false, "reason": "duplicates the decision"}
            ],
        });
    }
    plan.to_string()
}

/// compound task を `repo` で回した結果（planner は `plans` を順に出し、設計 review は `designs` の台本どおり）。
struct Flow {
    store: Arc<SqliteStore>,
    task: Task,
    adapter: Arc<DesignScriptAdapter>,
    repo: tempfile::TempDir,
    _root: tempfile::TempDir,
}

async fn flow(repo: tempfile::TempDir, plans: Vec<String>, designs: Vec<Script>) -> Flow {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.limits = super::tree::tree_limits();
    run_until_idle(&mut d, 400).await;
    Flow {
        store,
        task,
        adapter,
        repo,
        _root: root,
    }
}

impl Flow {
    fn requested(&self) -> Vec<RiskAssessment> {
        self.store
            .events_for(self.task.id)
            .unwrap()
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::DesignReviewRequested { risk, .. } => Some(risk),
                _ => None,
            })
            .collect()
    }
    fn design_reviews(&self) -> Vec<ReviewRequest> {
        self.adapter
            .inner
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| c.review.clone().filter(|r| r.design.is_some()))
            .collect()
    }
    fn adopted(&self) -> bool {
        self.store
            .execution_plan_active(self.task.id)
            .unwrap()
            .is_some()
    }
}

/// ケース 1: 認可の条件（`require_admin`）を 1 行変える計画は、brief が `new_*` を宣言しなくても実 profile の `authz` で
/// `High` になり、`DesignReviewRequested` と設計 review の run が起きる（prompt に path・owner 地図の行・不変条件の根拠）。
/// final review では、採用した計画の合格した設計 review が覆った組（authz, middleware.rs）の差分には設計の判定を求めず、
/// 計画に無い同じ class の別の path（principal.rs）には `ReviewRequest.diff_design` で `design` を求める。
#[tokio::test]
async fn boundary_case_1_authz_one_line_is_high() {
    let files = [
        (MIDDLEWARE, REQUIRE_ADMIN),
        (PRINCIPAL, "pub fn principal() {}\n"),
    ];
    let (repo, _) = real_repo(&files);
    let f = flow(
        repo,
        vec![plan(&[MIDDLEWARE], true)],
        vec![Script::Verdict(true, Vec::new())],
    )
    .await;
    let requested = f.requested();
    assert_eq!(requested.len(), 1, "one design review is requested");
    assert_eq!(requested[0].level, RiskLevel::High);
    assert_eq!(requested[0].reasons, authz(MIDDLEWARE));
    let reviews = f.design_reviews();
    assert_eq!(reviews.len(), 1, "one design review run");
    let design = reviews[0].design.as_deref().unwrap();
    // owner 地図の行は base の版の地図から同じ規則（`owner_rows`）で引いたもの（今の地図に middleware.rs の行は無い）。
    let map = std::fs::read_to_string(src_root().join(OWNER_MAP)).unwrap();
    let rows = crate::health_review::input::owner_rows(&map, &[MIDDLEWARE.to_string()]);
    assert_eq!(design.owner_rows, rows);
    let prompt = design_section(Some(&reviews[0]));
    for row in &rows {
        assert!(prompt.contains(&row.row), "{}:\n{prompt}", row.row);
    }
    for needle in [
        MIDDLEWARE,
        "api-principal-from-token",
        "agent-docs/adr/2026-10-27-api-principals.md",
        "cos-audit-same-tx",
    ] {
        assert!(prompt.contains(needle), "{needle}:\n{prompt}");
    }
    assert!(
        f.adopted(),
        "the plan is adopted after the passing design review"
    );

    let scratch = tempfile::tempdir().unwrap();
    let (_same, covered) =
        real_candidate(&files, |dir| write(dir, MIDDLEWARE, REQUIRE_ADMIN_EDITED));
    let m = material(&f.store, f.task.id, &covered, true, scratch.path());
    assert!(
        m.diff_design.is_none(),
        "the covered pair needs no second design verdict: {:?}",
        m.diff_design
    );
    let (_other, other) = real_candidate(&files, |dir| {
        write(dir, MIDDLEWARE, REQUIRE_ADMIN_EDITED);
        write(
            dir,
            PRINCIPAL,
            "pub fn principal() { let _admin = true; }\n",
        );
    });
    let m = material(&f.store, f.task.id, &other, true, scratch.path());
    let diff = m
        .diff_design
        .expect("the uncovered pair needs a design verdict");
    assert_eq!(diff.risk.level, RiskLevel::High);
    assert_eq!(diff.risk.reasons, authz(PRINCIPAL));
    assert!(diff.brief.is_some(), "the adopted plan's brief is passed");
    let request = ReviewRequest {
        diff_design: Some(diff),
        ..Default::default()
    };
    assert!(design_section(Some(&request)).contains(PRINCIPAL));
    let missing = final_review(&request, "").unwrap_err();
    assert!(missing.contains(DIFF_DESIGN_VERDICT_MISSING), "{missing}");
    assert!(final_review(&request, r#", "design": {"pass": true}"#).is_ok());
}

/// ケース 2: 同じ大きさの普通の変更（画面の 1 file と class 外の handler の 1 file）は `Low` で、設計 review も final
/// review の設計の節も起きない。実 profile で普通の path は 5 件中 0 件が設計 review の対象（過剰に止めない）。
#[tokio::test]
async fn boundary_case_2_equivalent_plain_change_is_low() {
    let files: Vec<(&str, &str)> = PLAIN.iter().map(|p| (*p, "a\n")).collect();
    let (repo, base) = real_repo(&files);
    let f = flow(repo, vec![plan(&PLAIN[..2], false)], Vec::new()).await;
    assert!(f.requested().is_empty(), "{:?}", f.requested());
    assert!(f.design_reviews().is_empty());
    assert!(
        f.adopted(),
        "the low-risk plan is adopted without a design review"
    );
    let state = load_review_policy(f.repo.path(), &base, None, REVIEW_POLICY_TIMEOUT);
    assert!(
        state.policy().is_some(),
        "the real profile is read: {state:?}"
    );
    let high: Vec<&str> = PLAIN
        .iter()
        .copied()
        .filter(|p| assess_risk(&state, &[p.to_string()], None).level != RiskLevel::Low)
        .collect();
    assert!(high.is_empty(), "plain paths judged high: {high:?}");

    let scratch = tempfile::tempdir().unwrap();
    let (_c, c) = real_candidate(&files, |dir| {
        for path in &PLAIN[..2] {
            write(dir, path, "b\n");
        }
    });
    let m = material(&f.store, f.task.id, &c, true, scratch.path());
    assert!(m.diff_design.is_none(), "{:?}", m.diff_design);
}

/// ケース 3: class の file を別 path へ rename（`git mv`）・削除して別名で追加・`#[path]` の先ごと移す差分は、計画が
/// class の外の path だけを宣言して設計 review なしに採用されても、final review の差分の risk が元の path で class を
/// 拾い、設計の判定が無ければ不合格。
#[tokio::test]
async fn boundary_case_3_move_cannot_escape() {
    let files = [
        (PRINCIPAL, "pub fn principal() {}\n"),
        (MIDDLEWARE, REQUIRE_ADMIN),
        (DESIGN_GATE, "fn gate() {}\n"),
        (
            DISPATCHER,
            "#[path = \"health_review/design_gate.rs\"]\nmod design_gate;\n",
        ),
    ];
    let (repo, _) = real_repo(&files);
    let f = flow(
        repo,
        vec![plan(&["crates/task-api/src/auth/principal.rs"], false)],
        Vec::new(),
    )
    .await;
    assert!(f.requested().is_empty(), "{:?}", f.requested());
    assert!(f.adopted());

    let scratch = tempfile::tempdir().unwrap();
    let (_c, c) = real_candidate(&files, |dir| {
        std::fs::create_dir_all(dir.join("crates/task-api/src/auth")).unwrap();
        git_out(
            dir,
            &["mv", PRINCIPAL, "crates/task-api/src/auth/principal.rs"],
        );
        git_out(dir, &["rm", "-q", MIDDLEWARE]);
        write(
            dir,
            "crates/task-api/src/auth/guard.rs",
            REQUIRE_ADMIN_EDITED,
        );
        std::fs::create_dir_all(dir.join("crates/task-dispatch/src/dispatcher")).unwrap();
        git_out(
            dir,
            &[
                "mv",
                DESIGN_GATE,
                "crates/task-dispatch/src/dispatcher/gate_wiring.rs",
            ],
        );
        write(
            dir,
            DISPATCHER,
            "#[path = \"dispatcher/gate_wiring.rs\"]\nmod design_gate;\n",
        );
    });
    let m = material(&f.store, f.task.id, &c, true, scratch.path());
    let diff = m
        .diff_design
        .expect("the moved class files need a design verdict");
    assert_eq!(diff.risk.level, RiskLevel::High);
    for (class, path) in [
        ("authz", PRINCIPAL),
        ("authz", MIDDLEWARE),
        ("review-gate", DESIGN_GATE),
    ] {
        assert!(
            diff.risk.reasons.contains(&class_path(class, path)),
            "{class} {path}: {:?}",
            diff.risk
        );
    }
    let request = ReviewRequest {
        diff_design: Some(diff),
        ..Default::default()
    };
    let missing = final_review(&request, "").unwrap_err();
    assert!(missing.contains(DIFF_DESIGN_VERDICT_MISSING), "{missing}");
}

/// ケース 4: 既存の循環（a→b→c→a）の内部に新しい組（a→c）を足す差分は、本物の検査器で health の関門が `deps.cycle`
/// の repair（不合格）で止まり、final review の reviewer の prompt（health の節）に finding の symbol
/// `./src/a -> ./src/c` が載る。fixture は B の汎用 Rust git fixture（`test_repo_health_scc_edges.py`・
/// `review_risk_finding_symbol_from_real_checker` と同じ形）で、profile は `[review]` の無い汎用の `default.toml`。
#[test]
fn boundary_case_4_worsening_dependency_blocks() {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    init_test_repo(dir);
    copy_checker(&src_root(), dir);
    let files = [
        (
            format!("{CHECKER_DIR}/profiles/default.toml"),
            "version = \"1\"\n",
        ),
        (
            "Cargo.toml".to_string(),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs".to_string(),
            "pub mod a;\npub mod b;\npub mod c;\n",
        ),
        (
            "src/a.rs".to_string(),
            "use crate::b::B;\n\npub struct A;\n\npub fn a(_: B) {}\n",
        ),
        (
            "src/b.rs".to_string(),
            "use crate::c::C;\n\npub struct B;\n\npub fn b(_: C) {}\n",
        ),
        (
            "src/c.rs".to_string(),
            "use crate::a::A;\n\npub struct C;\n\npub fn c(_: A) {}\n",
        ),
    ];
    for (path, body) in &files {
        write(dir, path, body);
    }
    git_out(dir, &["add", "-A"]);
    git_out(dir, &["commit", "-q", "-m", "base"]);
    let base = git_out(dir, &["rev-parse", "HEAD"]);
    write(
        dir,
        "src/a.rs",
        "use crate::b::B;\nuse crate::c::C;\n\npub struct A;\n\npub fn a(_: B, _: C) {}\n",
    );
    git_out(dir, &["commit", "-q", "-am", "head"]);
    let head = git_out(dir, &["rev-parse", "HEAD"]);

    let outcome = health_gate(&HealthGateRequest {
        repo_id: RepoId::new(),
        worktree: dir,
        base: &base,
        head: &head,
        integrated_by_celeris: true,
        bundled: None,
        timeout: HEALTH_INPUT_TIMEOUT,
    });
    let HealthGateDecision::Repair(repairs) = outcome.decision() else {
        panic!("the new pair inside the cycle must fail the gate: {outcome:?}");
    };
    let cycle = repairs
        .iter()
        .find(|r| r.rule_id == "deps.cycle")
        .unwrap_or_else(|| panic!("no deps.cycle repair: {repairs:?}"));
    assert!(cycle.reason.contains("./src/a -> ./src/c"), "{cycle:?}");

    let scratch = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let c = CandidateRepo {
        repo_id: RepoId::new().to_string(),
        dir: dir.to_path_buf(),
        target_sha: base,
        reviewed_sha: head,
    };
    let m = material(&store, TaskId::new(), &c, false, scratch.path());
    let request = ReviewRequest {
        health: m.health,
        ..Default::default()
    };
    let prompt = health_section(&request);
    assert!(
        prompt.contains("`deps.cycle`") && prompt.contains("symbol=`./src/a -> ./src/c`"),
        "{prompt}"
    );
}

/// reviewer の条件（index 0）を持つ reviewing の task と、`input` の health の材料の event。
fn reviewing(store: &Arc<SqliteStore>, dir: &std::path::Path, input: &HealthRepoInput) -> Task {
    let mut task = new_task(dir, Check::Reviewer, 2);
    task.status = Status::Reviewing;
    store.insert(&task).unwrap();
    store
        .append_event(
            task.id,
            &Event::HealthReviewInputBuilt {
                review_run: "r1".into(),
                repo_id: input.repo_id.clone(),
                target_sha: input.target_sha.clone(),
                reviewed_sha: input.reviewed_sha.clone(),
                definition_sha256: "def".into(),
                input: serde_json::to_value(input).unwrap(),
            },
        )
        .unwrap();
    task
}

/// policy の file を変えた候補の health の材料（差分の path は `paths`）。
fn policy_input(c: &CandidateRepo, paths: &[&str]) -> HealthRepoInput {
    HealthRepoInput {
        repo_id: c.repo_id.clone(),
        target_sha: c.target_sha.clone(),
        reviewed_sha: c.reviewed_sha.clone(),
        diff: DiffStat {
            files: paths.len() as u64,
            insertions: 1,
            deletions: 1,
            paths: paths
                .iter()
                .map(|p| PathStat {
                    path: p.to_string(),
                    insertions: 1,
                    deletions: 1,
                })
                .collect(),
        },
        policy: HealthPolicyState {
            source: "base".into(),
            changed: true,
            files: vec![PROFILE.to_string()],
        },
        ..Default::default()
    }
}

/// reviewer が policy の変更を承認した（`approved = true`）`review.json` を reviewer の読みで読む。
fn approved_by_reviewer(input: &HealthRepoInput) -> Vec<Verdict> {
    let text = r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}],"health":{"findings":[],"policy":{"approved":true,"reason":"the candidate approves its own profile"}}}"#;
    let health = HealthReviewInput {
        repos: vec![input.clone()],
        brief: None,
    };
    Verdict::from_review_json(
        text,
        Some(&health),
        &[0],
        "reviewer(r1)",
        "artifacts/review.json",
    )
    .unwrap()
}

/// ケース 5: 候補が自分の profile の `authz` から middleware の path を外し（方針を緩め）、同じ候補で middleware を
/// 変えても、risk は base 側の方針で `High`。policy の file の変更は reviewer が `approved = true` を書いても合格に
/// ならない（混ぜた候補は不合格、policy だけの候補は人の `Approval` の子を待つ）。
#[tokio::test]
async fn boundary_case_5_policy_self_approval_impossible() {
    let files = [(MIDDLEWARE, REQUIRE_ADMIN)];
    let loosen = |dir: &std::path::Path| {
        let text = std::fs::read_to_string(dir.join(PROFILE)).unwrap();
        let loosened = text.replace("src/middleware", "src/middleware_never");
        assert_ne!(loosened, text, "the real profile has the middleware path");
        write(dir, PROFILE, &loosened);
    };
    let (_c, c) = real_candidate(&files, |dir| {
        loosen(dir);
        write(dir, MIDDLEWARE, REQUIRE_ADMIN_EDITED);
    });
    let paths = [MIDDLEWARE.to_string()];
    let own = load_review_policy(&c.dir, &c.reviewed_sha, None, REVIEW_POLICY_TIMEOUT);
    assert_eq!(
        assess_risk(&own, &paths, None).level,
        RiskLevel::Low,
        "the candidate's own profile no longer classifies middleware"
    );
    let base = load_review_policy(&c.dir, &c.target_sha, None, REVIEW_POLICY_TIMEOUT);
    assert_eq!(
        assess_risk(&base, &paths, None).reasons[0],
        class_path("authz", MIDDLEWARE)
    );

    let scratch = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let m = material(&store, TaskId::new(), &c, true, scratch.path());
    let diff = m.diff_design.expect("the base policy judges the candidate");
    assert_eq!(diff.risk.level, RiskLevel::High);
    assert!(
        diff.risk.reasons.contains(&class_path("authz", MIDDLEWARE)),
        "{:?}",
        diff.risk
    );
    assert_eq!(
        diff.review_policy.map(|p| p.definition_sha256),
        base.policy().map(|p| p.definition_sha256.clone()),
        "the policy of the design section is the base one"
    );

    // 混ぜた候補（policy の file と middleware）: reviewer の承認があっても不合格。
    let mixed = approved_by_reviewer(&policy_input(&c, &[PROFILE, MIDDLEWARE]));
    assert!(mixed.iter().any(|v| !v.pass), "{mixed:?}");

    // policy だけの候補: reviewer の承認の後も人の承認の子を待つ。
    let (_p, p) = real_candidate(&files, loosen);
    let input = policy_input(&p, &[PROFILE]);
    let verdicts = approved_by_reviewer(&input);
    assert!(
        verdicts
            .iter()
            .any(|v| v.reason.contains("human approval required")),
        "{verdicts:?}"
    );
    let task = reviewing(&store, scratch.path(), &input);
    let d = dispatcher(store.clone(), done_adapter(), 1);
    let repo_id: RepoId = input.repo_id.parse().unwrap();
    let mut outcome = ReviewOutcome {
        verdicts,
        plan: None,
        provider_failure: None,
        reviewer_run: None,
        target_stale: None,
        required_checks: vec![RequiredRepoResult {
            binding: Some(RequiredCheckBinding {
                repo_id,
                target_sha: input.target_sha.clone(),
                reviewed_sha: input.reviewed_sha.clone(),
            }),
            definition_sha256: "def".into(),
            results: vec![],
            health_repair_hint: None,
            health_note: None,
        }],
        command_checks: vec![],
    };
    d.policy_review_gate(&task, Some("r1"), &mut outcome)
        .unwrap();
    let approvals: Vec<Task> = store
        .children(task.id)
        .unwrap()
        .into_iter()
        .filter(|c| c.kind == TaskKind::Approval)
        .collect();
    assert_eq!(approvals.len(), 1, "a human approval child is created");
    assert_eq!(approvals[0].status, Status::Ready);
    assert_eq!(
        d.resolve_human_approvals(&task).unwrap(),
        None,
        "the review waits for the human"
    );
}

/// ケース 6（fail-closed）: 方針を読めない → 採用前 `Unknown` で設計 review（低 risk の path でも採用しない）。正規表現を
/// 読めない → `Unknown`。差分を取れない → final review の `diff_design` が `Unknown`。`diff_design` を渡した review で
/// reviewer が `design` を書かない・`pass = false` → 不合格。reviewer run の無い review（policy_only）で覆われない組 →
/// health の合格の項目が class・path 付きの不合格。
#[tokio::test]
async fn boundary_case_6_fail_closed() {
    let failing = super::health_review_design_gate::checker_repo(
        "import sys\nsys.stderr.write('boom\\n')\nsys.exit(3)\n",
    );
    let f = flow(
        failing,
        vec![plan(&["notes/plan.md"], false)],
        vec![Script::Missing],
    )
    .await;
    let requested = f.requested();
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].level, RiskLevel::Unknown);
    assert!(
        requested[0].reasons[0].starts_with("review_policy_unavailable:"),
        "{requested:?}"
    );
    assert!(!f.adopted(), "an unreadable policy does not adopt the plan");

    let broken = serde_json::from_value(serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA, "source": "base", "owner_map": OWNER_MAP,
        "high_risk": [{"class": "authz", "paths": ["("]}], "invariants": {},
        "definition_sha256": "0".repeat(64),
    }))
    .unwrap();
    let risk = assess_risk(
        &task_core::design_review::ReviewPolicyState::Present(broken),
        &[PLAIN[0].to_string()],
        None,
    );
    assert_eq!(risk.level, RiskLevel::Unknown, "{risk:?}");

    let scratch = tempfile::tempdir().unwrap();
    let files = [(MIDDLEWARE, REQUIRE_ADMIN)];
    let (_n, mut no_diff) = real_candidate(&files, |dir| write(dir, PLAIN[0], "b\n"));
    no_diff.reviewed_sha = "0".repeat(40);
    let m = material(&f.store, f.task.id, &no_diff, true, scratch.path());
    let diff = m
        .diff_design
        .expect("a diff that cannot be read needs a design verdict");
    assert_eq!(diff.risk.level, RiskLevel::Unknown, "{:?}", diff.risk);
    let request = ReviewRequest {
        diff_design: Some(diff),
        ..Default::default()
    };
    let missing = final_review(&request, "").unwrap_err();
    assert!(missing.contains(DIFF_DESIGN_VERDICT_MISSING), "{missing}");
    assert!(final_review(&request, r#", "design": {"pass": false}"#).is_err());

    let (_c, c) = real_candidate(&files, |dir| write(dir, MIDDLEWARE, REQUIRE_ADMIN_EDITED));
    let job = HealthInputJob {
        work: Some(work(&f.store, f.task.id, &c, true, scratch.path())),
        remote: false,
    };
    let mut checks = vec![RequiredRepoChecks {
        binding: None,
        commands: Ok(Vec::new()),
        reusable: Default::default(),
        workspace: None,
        health: Some(RepoHealth::Evaluated(HealthItem {
            result: Some(PhaseCheckResult::new(
                HEALTH_CHECK_ID.into(),
                true,
                "health: pass".into(),
            )),
            ..Default::default()
        })),
    }];
    assert!(job.attach(None, &mut checks, Vec::new()).await.is_none());
    let Some(RepoHealth::Evaluated(item)) = &checks[0].health else {
        panic!("evaluated");
    };
    let result = item.result.as_ref().unwrap();
    assert!(!result.pass, "{result:?}");
    assert!(
        result.summary.contains(UNCOVERED_WITHOUT_REVIEWER),
        "{}",
        result.summary
    );
    assert!(
        result.summary.contains(&class_path("authz", MIDDLEWARE)),
        "{}",
        result.summary
    );
}
