//! 合格の stub の同梱の検査器の試験（prefix `health_enforce_g2_passing_stub_`）。
//!
//! 守る不変条件: base に検査器の無い一時 repo でも、stub を同梱の検査器にすると経路共通の入口（[`gate::health_gate`]）は
//! 同梱の検査器で評価した Pass になり、review 方針（[`load_review_policy`]）は `Absent`（`[review]` の無い repo）になる。
//! dispatcher 試験の土台と dagbench の driver はこの性質に頼る。stub の report・方針の形が gate の読む形とずれると、
//! それらの試験は「設計 review に回った」「承認待ちで止まった」としか見えず原因が分からないので、ここで直接確かめる。

use std::path::Path;
use std::time::Duration;

use task_core::RepoId;
use task_core::design_review::ReviewPolicyState;
use task_core::required_checks::health::{CheckerSource, HealthVerdict, UnavailableReason};

use super::*;
use crate::health_review::gate::{
    HealthGateDecision, HealthGateOutcome, HealthGateRequest, health_gate, read_bundled_checker,
};
use crate::health_review::policy::load_review_policy;

const TIMEOUT: Duration = Duration::from_secs(60);

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 検査器を持たない一時 repo（main に 2 commit）と base・head の sha。
fn repo_without_checker() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    std::fs::write(p.join("README.md"), "hello\n").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "base"]);
    let base = git(p, &["rev-parse", "HEAD"]);
    std::fs::write(p.join("a.rs"), "fn a() {}\n").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "change"]);
    let head = git(p, &["rev-parse", "HEAD"]);
    (dir, base, head)
}

/// 同梱の検査器として渡すと、検査器の無い repo の入口は Pass（同梱の版の証跡）、review 方針は `Absent`。渡さなければ
/// 入口は `Unavailable(checker_missing)` のまま（stub は適用判定を変えない）。
#[test]
fn health_enforce_g2_passing_stub_passes_a_repo_without_checker() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_passing_stub_checker(root.path()).expect("write stub");
    let bundled = read_bundled_checker(&dir).expect("read stub");
    assert_eq!(bundled.id.source, CheckerSource::Bundled);
    assert_eq!(
        write_passing_stub_checker(root.path()).expect("write again"),
        dir,
        "同じ本文は同じ dir に書く"
    );
    assert_eq!(read_bundled_checker(&dir).expect("read again"), bundled);

    let (repo, base, head) = repo_without_checker();
    let mut request = HealthGateRequest {
        repo_id: RepoId::new(),
        worktree: repo.path(),
        base: &base,
        head: &head,
        integrated_by_celeris: true,
        bundled: None,
        timeout: TIMEOUT,
    };
    let without = health_gate(&request);
    assert!(
        matches!(
            without,
            HealthGateOutcome::Unavailable {
                reason: UnavailableReason::CheckerMissing,
                ..
            }
        ),
        "{without:?}"
    );

    request.bundled = Some(&bundled);
    let outcome = health_gate(&request);
    let HealthGateOutcome::Evaluated(run) = &outcome else {
        panic!("not evaluated: {outcome:?}");
    };
    assert_eq!(run.evidence.verdict, HealthVerdict::Pass, "{outcome:?}");
    assert_eq!(run.evidence.checker, Some(bundled.id.clone()));
    assert!(run.evidence.reviewable);
    assert_eq!(outcome.decision(), HealthGateDecision::Pass);

    assert_eq!(
        load_review_policy(repo.path(), &base, Some(&bundled), TIMEOUT),
        ReviewPolicyState::Absent
    );
    assert!(matches!(
        load_review_policy(repo.path(), &base, None, TIMEOUT),
        ReviewPolicyState::Unavailable(_)
    ));
}

/// process への設定は何度呼んでも同じ値で、[`gate::bundled_checker`] がそれを返す。
#[test]
fn health_enforce_g2_passing_stub_configures_once_per_process() {
    let first = configure_passing_stub_checker().expect("configure");
    let second = configure_passing_stub_checker().expect("configure again");
    assert!(std::ptr::eq(first, second));
    assert_eq!(gate::bundled_checker(), Some(first));
    assert_eq!(first.id.source, CheckerSource::Bundled);
}
