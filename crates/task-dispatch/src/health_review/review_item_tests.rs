//! ADR 2026-11-08-health-enforcement-loop D1.2・D2.4・D3.1〜D3.4: atomic/direct の task の review（`review::review_task` の
//! 必須検査の列）の health の項目の試験（試験名は `health_enforce_direct_review_` と `health_enforce_integration_repair_review_`）。
//! `gate_tests` の子 module で、同じ stub の検査器（base に commit した `repo_health.py` と `health/engine.py`）を使う。
//! 同梱の検査器は `evaluate_all` の `bundled` に直接渡す（process の設定を読まない）。外部ネットワークに出ず、LLM を呼ばない。
//!
//! 既存の `health_enforce_entry_*` は入口の分類だけを見る。これらは review の必須検査の列に項目が入ること・定義指紋に
//! 検査器が入ること・Fail が repair の材料を持つこと・Error が合格にならないこと・G2 の評価を同じ候補で使い回すことを
//! `review_task` を通して見る（入口の試験は review の列と指紋を通らないので、配線の抜けを検出できない）。

use std::path::Path;

use task_core::required_checks::definition_digest;
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthVerdict};
use task_core::{Check, RepoId};

use super::*;
use crate::health_review::review_item::{self, HEALTH_POLICY_ONLY};
use crate::review::{
    HEALTH_CHECK_ERROR, RepoHealth, RequiredCheckBinding, RequiredRepoChecks, ReviewExtras,
    ReviewOutcome, review_task,
};
use task_worker::workspace::LocalWorkspace;

fn bound(repo_id: RepoId, base: &str, head: &str) -> Option<RequiredCheckBinding> {
    Some(RequiredCheckBinding {
        repo_id,
        target_sha: base.to_string(),
        reviewed_sha: head.to_string(),
    })
}

/// `spawn_review` が置く未評価の health（`RepoHealth::At(dir)`）を持つ必須検査（コマンドは無し）。
fn site(binding: Option<RequiredCheckBinding>, dir: &Path) -> RequiredRepoChecks {
    RequiredRepoChecks {
        binding,
        commands: Ok(Vec::new()),
        reusable: Default::default(),
        workspace: None,
        health: Some(RepoHealth::At(dir.to_path_buf())),
    }
}

/// `HealthInputJob::attach` と同じ `evaluate_all` で評価した必須検査（`runs` は G2 の材料の評価）。
async fn checks_at(
    binding: Option<RequiredCheckBinding>,
    dir: &Path,
    remote: bool,
    runs: Vec<HealthRun>,
) -> RequiredRepoChecks {
    let mut repo = vec![site(binding, dir)];
    review_item::evaluate_all(&mut repo, runs, remote, None).await;
    repo.remove(0)
}

async fn checks(binding: Option<RequiredCheckBinding>, dir: &Path) -> RequiredRepoChecks {
    checks_at(binding, dir, false, Vec::new()).await
}

/// 計画なしの直行経路の review（acceptance は空、必須検査は health の項目だけ）。
async fn review(dir: &Path, repo: RequiredRepoChecks) -> ReviewOutcome {
    let task = crate::review::tests::task_with(Vec::new(), dir);
    let ws = LocalWorkspace::new(dir);
    review_task(
        &task,
        &ws,
        dir,
        &dir.join("artifacts"),
        &[],
        TIMEOUT,
        ReviewExtras {
            repo_checks: vec![repo],
            ..ReviewExtras::default()
        },
    )
    .await
}

fn health_result(outcome: &ReviewOutcome) -> &task_core::PhaseCheckResult {
    let results = &outcome.required_checks[0].results;
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0].cmd, HEALTH_CHECK_ID);
    &results[0]
}

/// 不変条件: 候補のある direct の review は health の項目（証跡つき）を必須検査の列に足し、定義指紋に適用判定の
/// 検査器を入れる（D2.4。検査器が変われば証跡が失効する）。Pass は合格。
#[tokio::test]
async fn health_enforce_direct_review_pass_records_item_and_checker_digest() {
    let text = report("pass", 0, vec![], base_policy(false), vec![]);
    let (dir, base) = repo_with_checker(&stub_json(0, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let repo_id = RepoId::new();

    let outcome = review(
        dir.path(),
        checks(bound(repo_id, &base, &head), dir.path()).await,
    )
    .await;

    let item = health_result(&outcome);
    assert!(item.pass, "{item:?}");
    let evidence = item.health.as_deref().expect("health evidence");
    assert_eq!(evidence.verdict, HealthVerdict::Pass);
    assert_eq!(
        (evidence.base_sha.as_str(), evidence.head_sha.as_str()),
        (base.as_str(), head.as_str())
    );
    let checker = applied(dir.path(), &base, None);
    let repo = &outcome.required_checks[0];
    assert_eq!(
        repo.definition_sha256,
        definition_digest(&[], Some(&checker))
    );
    assert_ne!(repo.definition_sha256, definition_digest(&[], None));
    assert!(outcome.all_pass(), "{:?}", outcome.verdicts);
}

/// 不変条件: policy の file だけの変更（PolicyOnly）は review の項目では合格で、人の承認は G2 の `policy_review_gate`
/// が求める（ここで repair にも Error にもしない）。
#[tokio::test]
async fn health_enforce_direct_review_policy_only_goes_to_policy_gate() {
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&policy_report())), true);
    let head = commit(dir.path(), &[(PROFILE, "[review]\n# changed\n")], "policy");

    let outcome = review(
        dir.path(),
        checks(bound(RepoId::new(), &base, &head), dir.path()).await,
    )
    .await;

    let item = health_result(&outcome);
    assert!(item.pass, "{item:?}");
    assert_eq!(item.summary, HEALTH_POLICY_ONLY);
    assert_eq!(
        item.health.as_deref().map(|h| h.verdict),
        Some(HealthVerdict::PolicyOnly)
    );
    assert!(outcome.verdicts[0].repair_hint.is_none());
}

/// 不変条件: 決定的な Fail（new の error）は不合格で、verdict は repair の材料（scope `HEALTH_CHECK_ID`・class
/// `health:<rule_id>`）を持ち、`try_review_repair` は `Check::Command { cmd: HEALTH_CHECK_ID }` として
/// `classify_review_failure` に渡す（D3.2）。reason は fingerprint と evidence の path。
#[tokio::test]
async fn health_enforce_direct_review_fail_routes_to_repair() {
    let findings = vec![finding("deps.violation", 1, "error", "new", false)];
    let rules = vec![rule("deps.violation", "fail", "")];
    let text = report("fail", 1, findings, base_policy(false), rules);
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");

    let outcome = review(
        dir.path(),
        checks(bound(RepoId::new(), &base, &head), dir.path()).await,
    )
    .await;

    let item = health_result(&outcome);
    assert!(!item.pass);
    assert_eq!(
        item.health.as_deref().map(|h| h.verdict),
        Some(HealthVerdict::Fail)
    );
    let fp = format!("deps.violation:{:016x}", 1);
    assert_eq!(item.summary, format!("health:{fp}（evidence: {SRC}）"));
    assert!(!outcome.all_pass());
    let verdict = &outcome.verdicts[0];
    let hint = verdict.repair_hint.as_ref().expect("repair hint");
    assert_eq!(hint.scope, HEALTH_CHECK_ID);
    assert_eq!(hint.class, "health:deps.violation");
    assert_eq!(
        verdict.implicit_check(),
        Check::Command {
            cmd: HEALTH_CHECK_ID.to_string(),
            expect_exit: 0
        }
    );
}

/// 不変条件: Unavailable（base にも同梱にも検査器が無い）と Error（exit 2）は `health_check_error: <理由>` の不合格で、
/// repair の材料を持たず、`try_review_repair` は修復しない（`Check::Human`。D3.4）。
#[tokio::test]
async fn health_enforce_direct_review_error_is_not_pass() {
    let dir = init_repo();
    let base = sh_git(dir.path(), &["rev-parse", "HEAD"]);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let missing = review(
        dir.path(),
        checks(bound(RepoId::new(), &base, &head), dir.path()).await,
    )
    .await;

    let (errored, ebase) = repo_with_checker(&stub_json(2, None), true);
    let ehead = commit(errored.path(), &[(SRC, "fn b() {}\n")], "change");
    let exit2 = review(
        errored.path(),
        checks(bound(RepoId::new(), &ebase, &ehead), errored.path()).await,
    )
    .await;

    for (outcome, reason) in [(&missing, "checker_missing"), (&exit2, "")] {
        let item = health_result(outcome);
        assert!(!item.pass, "{item:?}");
        assert!(
            item.summary
                .starts_with(&format!("{HEALTH_CHECK_ERROR}: {reason}")),
            "{item:?}"
        );
        assert_eq!(
            item.health.as_deref().map(|h| h.verdict),
            Some(HealthVerdict::Error)
        );
        assert!(!outcome.all_pass());
        assert!(outcome.verdicts[0].repair_hint.is_none());
        assert_eq!(outcome.verdicts[0].implicit_check(), Check::Human);
    }
    // Unavailable は検査器が無いので、定義指紋に検査器を入れない。
    assert_eq!(
        missing.required_checks[0].definition_sha256,
        definition_digest(&[], None)
    );
    // 評価されなかった health（`RepoHealth::At` のまま。attach を通らない）も合格にしない（fail closed）。
    let unevaluated = review(
        dir.path(),
        site(bound(RepoId::new(), &base, &head), dir.path()),
    )
    .await;
    let item = health_result(&unevaluated);
    assert!(!item.pass);
    assert!(
        item.summary
            .starts_with(&format!("{HEALTH_CHECK_ERROR}: not evaluated"))
    );
}

/// 不変条件: Git 管理でない作業場所と remote workspace は NotApplicable で、項目を足さず理由を残す（指紋に検査器を
/// 入れず、review は health で落ちない）。
#[tokio::test]
async fn health_enforce_direct_review_non_git_repo_adds_no_item() {
    let plain = tempfile::tempdir().unwrap();
    std::fs::write(plain.path().join("README.md"), "plain\n").unwrap();
    let outcome = review(plain.path(), checks(None, plain.path()).await).await;
    let repo = &outcome.required_checks[0];
    assert!(repo.results.is_empty(), "{repo:?}");
    assert_eq!(repo.health_note.as_deref(), Some("not_a_git_repo"));
    assert_eq!(repo.definition_sha256, definition_digest(&[], None));
    assert!(outcome.all_pass());

    let remote = checks_at(None, plain.path(), true, Vec::new()).await;
    let outcome = review(plain.path(), remote).await;
    assert!(outcome.required_checks[0].results.is_empty());
    assert_eq!(
        outcome.required_checks[0].health_note.as_deref(),
        Some("remote_workspace")
    );
}

/// 同じ repo の検査器で 1 回評価した `HealthRun`（`tool_version` を印にする）。
fn marked_run(dir: &Path, repo_id: RepoId, base: &str, head: &str) -> HealthRun {
    let outcome = health_gate(&HealthGateRequest {
        repo_id,
        worktree: dir,
        base,
        head,
        integrated_by_celeris: true,
        bundled: None,
        timeout: TIMEOUT,
    });
    let HealthGateOutcome::Evaluated(mut run) = outcome else {
        panic!("not evaluated: {outcome:?}");
    };
    run.evidence.tool_version = "from-g2-input".into();
    run
}

/// 不変条件: G2 の材料が同じ候補（repo・base・head）で評価した結果を review の項目に使い、検査器を 2 回起こさない
/// （D4.2。印の `tool_version` がそのまま残る）。
#[tokio::test]
async fn health_enforce_direct_review_uses_the_g2_evaluation_of_the_same_candidate() {
    let text = report("pass", 0, vec![], base_policy(false), vec![]);
    let (dir, base) = repo_with_checker(&stub_json(0, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let repo_id = RepoId::new();
    let marked = marked_run(dir.path(), repo_id, &base, &head);
    let repo = checks_at(
        bound(repo_id, &base, &head),
        dir.path(),
        false,
        vec![marked],
    )
    .await;

    let outcome = review(dir.path(), repo).await;

    let item = health_result(&outcome);
    assert!(item.pass);
    assert_eq!(
        item.health.as_deref().map(|h| h.tool_version.as_str()),
        Some("from-g2-input")
    );
}

/// 不変条件: 統合の修復（`IntegrationRepairScheduled`）の後の review は新しい head で評価し直し、修復の前の head の
/// 評価（束縛が違う）を使わない。修復の前の Fail が残らず、新しい head の report で決まる。
#[tokio::test]
async fn health_enforce_integration_repair_review_reevaluates_the_new_head() {
    let text = report("pass", 0, vec![], base_policy(false), vec![]);
    let (dir, base) = repo_with_checker(&stub_json(0, Some(&text)), true);
    let before = commit(dir.path(), &[(SRC, "fn b() {}\n")], "before repair");
    let repaired = commit(dir.path(), &[(SRC, "fn c() {}\n")], "integration repair");
    let repo_id = RepoId::new();
    let mut stale = marked_run(dir.path(), repo_id, &base, &before);
    stale.evidence.verdict = HealthVerdict::Fail;
    // 修復の前の head の評価（束縛が違う）を G2 の材料の評価として渡しても使わない。
    let repo = checks_at(
        bound(repo_id, &base, &repaired),
        dir.path(),
        false,
        vec![stale],
    )
    .await;

    let outcome = review(dir.path(), repo).await;

    let item = health_result(&outcome);
    let evidence = item.health.as_deref().expect("health evidence");
    assert_eq!(evidence.head_sha, repaired);
    assert_eq!(evidence.tool_version, "0.9.1");
    assert!(item.pass, "{item:?}");
}
