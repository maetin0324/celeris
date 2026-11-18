//! ADR 2026-11-17-boundary-change-risk-review D3・付記 (ii)・(iii)・(v)（WorkUnit `review-gate`）: final review の差分の
//! risk と finding の `symbol`・`rationale`。
//!
//! 守る不変条件:
//! - 差分の path は材料の `--no-renames` の全 path（rename の元と先・消した path・`#[path]` で読む file の物理 path）で、
//!   base 側の方針の class に当たる。採用した計画の合格した設計 review の理由が覆う (class, path) の組だけが要求から外れ、
//!   同じ class の別の path は外れない。
//! - 覆われない組があれば `diff_design` を求め、`design` の合格が無い review・reviewer run の無い review は不合格。
//! - 方針・正規表現・差分を読めなければ `Unknown` で全差分を覆われないものとし、方針が `Absent` の repo は何もしない。
//!
//! 既存の `health_review_input_*` は差分の path を owner 地図の行にしか使わず、`design_review` の試験は計画の write path
//! しか見ないので、差分の rename・削除・覆った組・reviewer の `design` の欠落を検出できない。

use super::*;
use crate::health_review::verdict::DIFF_DESIGN_VERDICT_MISSING;
use crate::review::{HealthItem, RepoHealth, Verdict};
use task_core::PhaseCheckResult;
use task_worker::protocol::ReviewRequest;

const PRINCIPAL: &str = "crates/task-api/src/principal.rs";
const MIDDLEWARE: &str = "crates/task-api/src/middleware.rs";
/// `dispatcher.rs` が `#[path]` で読む file（論理 module は dispatcher、物理 path は `health_review/`）。
const GATE: &str = "crates/task-dispatch/src/health_review/design_gate.rs";
const PLAIN: &str = "web/src/App.tsx";
const INVARIANT: &str = "invariant:authz:api-principal-from-token";

fn classes() -> serde_json::Value {
    serde_json::json!([
        {"class": "authz", "paths": ["^crates/task-api/src/principal\\.rs$", "^crates/task-api/src/middleware\\.rs$"],
         "invariants": ["api-principal-from-token"]},
        {"class": "review-gate", "paths": ["^crates/task-dispatch/src/health_review/(design|design_gate)\\.rs$"]},
        {"class": "store", "paths": ["^crates/task-core/src/store/"]},
    ])
}

fn policy(source: &str, high_risk: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA, "source": source, "owner_map": "owners/map.md", "high_risk": high_risk,
        "invariants": {"api-principal-from-token": "agent-docs/adr/2026-10-27-api-principals.md"},
        "definition_sha256": DIGEST,
    })
}

/// 一時 git repo: main に stub の検査器（`--print-review-policy` は `printed`、`None` なら JSON でない文字列。report の
/// mode は合格の report を書く）と境界の file、branch `cand` に `change` を当てた 1 commit。
fn boundary_repo(
    printed: Option<serde_json::Value>,
    change: impl FnOnce(&Path),
) -> (tempfile::TempDir, CandidateRepo) {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    let git = |args: &[&str]| git_in(dir, args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    let printed = printed.map_or_else(|| "not json".to_string(), |p| p.to_string());
    let mut rep = report(true, Vec::new());
    rep["policy"]["changed"] = false.into();
    let script = format!(
        "import sys\nargs = sys.argv[1:]\nif '--print-review-policy' in args:\n    sys.stdout.write({printed:?})\n    sys.exit(0)\nopen(args[args.index('--out') + 1], 'w').write({rep:?})\n",
        rep = rep.to_string(),
    );
    for (path, body) in [
        (CHECKER_SCRIPT, script.as_str()),
        (PRINCIPAL, "pub fn principal() {}\n"),
        (MIDDLEWARE, "pub fn require_admin() {}\n"),
        (GATE, "fn gate() {}\n"),
        (PLAIN, "export const App = 1;\n"),
    ] {
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), body).unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "base"]);
    let target_sha = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "-b", "cand"]);
    change(dir);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "cand"]);
    let reviewed_sha = git(&["rev-parse", "HEAD"]);
    let candidate = CandidateRepo {
        repo_id: task_core::RepoId::new().to_string(),
        dir: dir.to_path_buf(),
        target_sha,
        reviewed_sha,
    };
    (repo, candidate)
}

fn edit(path: &'static str) -> impl FnOnce(&Path) {
    move |dir| std::fs::write(dir.join(path), "pub fn changed() {}\n").unwrap()
}

/// 本番と同じ入口（[`build_final_review_material`]）で材料を作る。計画の無い task（覆った組は無い）。
fn material(
    c: &CandidateRepo,
    policy_only: bool,
    scratch: &Path,
) -> (FinalReviewMaterial, Arc<SqliteStore>, TaskId) {
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut w = work(&store, "review-1", c, scratch);
    w.skipped.clear();
    w.policy_only = policy_only;
    let task = w.task_id;
    (build_final_review_material(&w, None), store, task)
}

fn high(reasons: &[&str]) -> RiskAssessment {
    RiskAssessment {
        level: RiskLevel::High,
        reasons: reasons.iter().map(|r| r.to_string()).collect(),
    }
}

fn repo_risk(risk: RiskAssessment) -> RepoRisk {
    RepoRisk {
        repo_id: "r".into(),
        risk,
        policy: None,
        stat: "repo r: a..b\n".into(),
    }
}

/// 採用した計画（`plan-1`）の設計 review の要求（理由 `reasons`）と判定（`pass`）。
fn design_events(sha: &str, reasons: &[&str], pass: bool) -> Vec<(u64, Event)> {
    let spec = serde_json::from_value(serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA_V3, "rationale": "r",
        "stages": [{"key": "s", "kind": "implement", "title": "S"}],
        "units": [{"key": "u", "stage": "s", "kind": "implement", "title": "u", "objective": "o",
            "done_when": ["d"], "context": {"paths": [MIDDLEWARE]}}],
    }))
    .unwrap();
    vec![
        (
            1,
            Event::DesignReviewRequested {
                plan_sha256: sha.into(),
                plan: Box::new(spec),
                risk: high(reasons),
                policy_sha256: None,
            },
        ),
        (
            2,
            Event::DesignReviewed {
                plan_sha256: sha.into(),
                review_run: "design-run".into(),
                pass,
                findings: Vec::new(),
            },
        ),
    ]
}

fn class_path(class: &str, path: &str) -> String {
    format!("path_class:{class}:{path}")
}

/// rename（`git mv`）の元・消した path・`#[path]` で読む file の物理 path が、材料の差分の risk（記録の
/// `HealthReviewInputBuilt.input.risk`）に出る。計画の無い task では全部が `diff_design` で求められる。
#[test]
fn review_risk_rename_delete_and_path_target_are_assessed() {
    let scratch = tempfile::tempdir().unwrap();
    let (_repo, c) = boundary_repo(Some(policy("base", classes())), |dir| {
        git_in(
            dir,
            &["mv", PRINCIPAL, "crates/task-api/src/principal_v2.rs"],
        );
        git_in(dir, &["rm", "-q", MIDDLEWARE]);
        std::fs::write(dir.join(GATE), "fn gate() { let _changed = 1; }\n").unwrap();
    });
    let (material, store, task) = material(&c, false, scratch.path());
    let health = material.health.expect("material");
    let risk = health.repos[0]
        .risk
        .clone()
        .expect("the diff risk is in the material");
    assert_eq!(risk.level, RiskLevel::High);
    for (class, path) in [
        ("authz", PRINCIPAL),
        ("authz", MIDDLEWARE),
        ("review-gate", GATE),
    ] {
        assert!(
            risk.reasons.contains(&class_path(class, path)),
            "{class} {path}: {risk:?}"
        );
    }
    assert!(risk.reasons.contains(&INVARIANT.to_string()), "{risk:?}");
    assert!(
        !risk.reasons.iter().any(|r| r == "brief_absent_human_plan"),
        "{risk:?}"
    );
    let recorded = store
        .events_for(task)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::HealthReviewInputBuilt { input, .. } => Some(input),
            _ => None,
        })
        .expect("HealthReviewInputBuilt");
    assert_eq!(recorded["risk"], serde_json::to_value(&risk).unwrap());
    let design = material
        .diff_design
        .expect("nothing is covered without a plan");
    assert_eq!(design.risk, risk);
    assert!(
        design.plan_summary.contains("4 file"),
        "{}",
        design.plan_summary
    );
    assert_eq!(design.review_policy.map(|p| p.high_risk.len()), Some(3));
}

/// class に合わない普通の変更（画面の file）は `Low` で、`diff_design` を求めない（過剰に止めない）。
#[test]
fn review_risk_plain_change_needs_no_design() {
    let scratch = tempfile::tempdir().unwrap();
    let (_repo, c) = boundary_repo(Some(policy("base", classes())), edit(PLAIN));
    let (material, _, _) = material(&c, false, scratch.path());
    let risk = material.health.expect("material").repos[0].risk.clone();
    assert_eq!(risk.map(|r| r.level), Some(RiskLevel::Low));
    assert!(material.diff_design.is_none());
}

/// 採用した計画の合格した設計 review の理由が覆う組（同じ path・`/` で終わる dir の接頭辞）は要求しない。
#[test]
fn review_risk_covered_pair_is_not_required() {
    let events = design_events(
        "plan-1",
        &[
            &class_path("authz", MIDDLEWARE),
            "path_class:store:crates/task-core/src/store/",
            INVARIANT,
        ],
        true,
    );
    let covered = covered_pairs(&events, Some("plan-1"));
    assert_eq!(covered.len(), 2, "{covered:?}");
    let diff = high(&[
        &class_path("authz", MIDDLEWARE),
        "path_class:store:crates/task-core/src/store/tasks.rs",
        INVARIANT,
    ]);
    assert!(uncovered(&diff, &covered).is_empty());
    assert!(diff_design(vec![repo_risk(diff)], &covered, None).is_none());
}

/// 同じ class の別の path は覆わない。不合格・採用していない計画・複数 repo の計画の理由は何も覆わない。
#[test]
fn review_risk_same_class_other_path_is_required() {
    let covered = covered_pairs(
        &design_events("plan-1", &[&class_path("authz", MIDDLEWARE)], true),
        Some("plan-1"),
    );
    let diff = high(&[
        &class_path("authz", MIDDLEWARE),
        &class_path("authz", PRINCIPAL),
        INVARIANT,
    ]);
    assert_eq!(
        uncovered(&diff, &covered),
        vec![class_path("authz", PRINCIPAL), INVARIANT.to_string()]
    );
    let design =
        diff_design(vec![repo_risk(diff.clone())], &covered, None).expect("design is required");
    assert_eq!(design.risk.level, RiskLevel::High);
    assert_eq!(
        design.risk.reasons,
        vec![class_path("authz", PRINCIPAL), INVARIANT.to_string()]
    );
    let pair = class_path("authz", MIDDLEWARE);
    for (events, adopted) in [
        (design_events("plan-1", &[&pair], false), Some("plan-1")),
        (design_events("plan-0", &[&pair], true), Some("plan-1")),
        (design_events("plan-1", &[&pair], true), None),
        (
            design_events("plan-1", &[&format!("code:{pair}")], true),
            Some("plan-1"),
        ),
    ] {
        let covered = covered_pairs(&events, adopted);
        assert!(covered.is_empty(), "{covered:?}");
        assert_eq!(uncovered(&diff, &covered), diff.reasons);
    }
}

/// `diff_design` を渡した final review は `design` の合格が要る: 無い・`pass = false`・壊れた `design` は全 criterion の
/// 不合格（`fail_all`）。`diff_design` の無い review は今までどおり `design` を見ない。
#[test]
fn review_risk_design_missing_or_failed_is_not_pass() {
    let asked = ReviewRequest {
        diff_design: diff_design(
            vec![repo_risk(high(&[&class_path("authz", PRINCIPAL)]))],
            &[],
            None,
        ),
        ..Default::default()
    };
    let verdicts = r#""verdicts": [{"criterion": 0, "pass": true, "reason": "ok"}]"#;
    let read = |design: &str, review: &ReviewRequest| {
        let text = format!("{{{verdicts}{design}}}");
        Verdict::from_final_review(
            &text,
            Some(review),
            &[0],
            "reviewer(r)",
            "artifacts/review.json",
        )
    };
    let passed = read(r#", "design": {"pass": true}"#, &asked).expect("design pass");
    assert!(passed.iter().all(|v| v.pass));
    let missing = read("", &asked).unwrap_err();
    assert!(missing.contains(DIFF_DESIGN_VERDICT_MISSING), "{missing}");
    assert!(
        missing.contains(&class_path("authz", PRINCIPAL)),
        "{missing}"
    );
    let failed = read(r#", "design": {"pass": false}"#, &asked).unwrap_err();
    assert!(failed.contains("design verdict failed"), "{failed}");
    let broken = read(r#", "design": {"pass": "yes"}"#, &asked).unwrap_err();
    assert!(broken.contains("not a valid ReviewOutput"), "{broken}");
    assert!(read("", &ReviewRequest::default()).is_ok());
}

/// fail-closed: 方針を読めない・正規表現を読めない・差分を取れない repo は `Unknown` で設計の判定を求める。方針が
/// `Absent` の repo は高 risk の path を変えても何もしない。
#[test]
fn review_risk_unknown_requires_design() {
    let scratch = tempfile::tempdir().unwrap();
    let (_repo, c) = boundary_repo(None, edit(PLAIN));
    for policy_only in [false, true] {
        let design = material(&c, policy_only, scratch.path())
            .0
            .diff_design
            .expect("unknown policy");
        assert_eq!(design.risk.level, RiskLevel::Unknown);
        assert!(
            design.risk.reasons[0].starts_with("review_policy_unavailable:"),
            "{design:?}"
        );
    }
    let bad: ReviewPolicy = serde_json::from_value(policy(
        "base",
        serde_json::json!([{"class": "x", "paths": ["("]}]),
    ))
    .unwrap();
    let risk = diff_risk(&ReviewPolicyState::Present(bad), &[PLAIN.to_string()]);
    assert_eq!(risk.level, RiskLevel::Unknown);
    assert_eq!(uncovered(&risk, &[]), risk.reasons);

    let (_repo, mut c) = boundary_repo(Some(policy("base", classes())), edit(PLAIN));
    c.reviewed_sha = "0".repeat(40);
    let design = material(&c, true, scratch.path())
        .0
        .diff_design
        .expect("no diff");
    assert_eq!(design.risk.level, RiskLevel::Unknown);

    let (_repo, c) = boundary_repo(Some(policy("absent", classes())), edit(PRINCIPAL));
    for policy_only in [false, true] {
        assert!(
            material(&c, policy_only, scratch.path())
                .0
                .diff_design
                .is_none()
        );
    }
}

/// 付記 (v): reviewer run の無い review（policy_only）で覆われない組が残れば、合格の health の項目を理由（class・path）
/// 付きの不合格にし、局所修復にしない。もともと不合格の項目は元の理由のまま。
#[test]
fn review_risk_policy_only_fails_with_class_and_path() {
    let scratch = tempfile::tempdir().unwrap();
    let (_repo, c) = boundary_repo(Some(policy("base", classes())), edit(MIDDLEWARE));
    let (material, _, _) = material(&c, true, scratch.path());
    assert!(material.health.is_none());
    let item = |pass: bool, summary: &str| RequiredRepoChecks {
        binding: None,
        commands: Ok(Vec::new()),
        reusable: Default::default(),
        workspace: None,
        health: Some(RepoHealth::Evaluated(HealthItem {
            result: Some(PhaseCheckResult::new(
                HEALTH_CHECK_ID.into(),
                pass,
                summary.into(),
            )),
            ..Default::default()
        })),
    };
    let mut checks = vec![
        item(true, "health: pass"),
        item(false, "health_check_error: x"),
    ];
    fail_uncovered(&mut checks, material.diff_design.as_deref());
    let Some(RepoHealth::Evaluated(failed)) = &checks[0].health else {
        panic!("evaluated");
    };
    let result = failed.result.as_ref().unwrap();
    assert!(!result.pass);
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
    assert_eq!(
        failed.repair_hint.as_ref().map(|h| h.scope.as_str()),
        Some(SUBSTANTIVE_SCOPE)
    );
    let Some(RepoHealth::Evaluated(kept)) = &checks[1].health else {
        panic!("evaluated");
    };
    assert_eq!(
        kept.result.as_ref().unwrap().summary,
        "health_check_error: x"
    );
    assert!(kept.repair_hint.is_none());
}

/// 付記 (ii): 本物の検査器（`scripts/dev/repo-health/repo_health.py`。`health_review_scenario_real_cli` と同じ
/// `python3 -I -B` の起動）を一時 git repo の base（a→b→c→a の循環）と head（循環の内部に a→c を足す）に当てた report
/// から、`summarize_report` が `symbol` に組の実際の値、`rationale` に「新しい組」を写す。
#[test]
fn review_risk_finding_symbol_from_real_checker() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    let git = |args: &[&str]| git_in(dir, args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    let files = [
        (
            "scripts/dev/repo-health/profiles/default.toml",
            "version = \"1\"\n",
        ),
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\n"),
        (
            "src/a.rs",
            "use crate::b::B;\n\npub struct A;\n\npub fn a(_: B) {}\n",
        ),
        (
            "src/b.rs",
            "use crate::c::C;\n\npub struct B;\n\npub fn b(_: C) {}\n",
        ),
        (
            "src/c.rs",
            "use crate::a::A;\n\npub struct C;\n\npub fn c(_: A) {}\n",
        ),
    ];
    for (path, body) in files {
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), body).unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    let head_a = "use crate::b::B;\nuse crate::c::C;\n\npub struct A;\n\npub fn a(_: B, _: C) {}\n";
    std::fs::write(dir.join("src/a.rs"), head_a).unwrap();
    git(&["commit", "-q", "-am", "head"]);
    let head = git(&["rev-parse", "HEAD"]);
    let out = repo.path().join("report.json");
    let ran = std::process::Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(src_root.join(CHECKER_SCRIPT))
        .arg("--repo")
        .arg(dir)
        .args([
            "--base",
            &base,
            "--head",
            &head,
            "--policy-version",
            "1",
            "--out",
        ])
        .arg(&out)
        .output()
        .unwrap();
    let text = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&ran.stderr)));
    let report: RepoHealthReport = serde_json::from_str(&text).unwrap();
    let (summary, _) = summarize_report(report);
    let edge = summary
        .new
        .iter()
        .find(|f| {
            f.rule_id == "deps.cycle"
                && f.rationale
                    .as_deref()
                    .is_some_and(|r| r.contains("新しい組"))
        })
        .unwrap_or_else(|| panic!("no new-pair finding: {:?}", summary.new));
    assert_eq!(edge.symbol.as_deref(), Some("./src/a -> ./src/c"));
    assert!(
        edge.rationale
            .as_deref()
            .unwrap()
            .contains("./src/a -> ./src/c"),
        "{edge:?}"
    );
    assert_eq!(
        (edge.severity.as_str(), edge.delta.as_str()),
        ("error", "new")
    );
}
