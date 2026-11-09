//! ADR 2026-11-08-health-enforcement-loop D1.2・D3.1〜D3.4: atomic/direct の task の review（`Dispatcher::spawn_review` →
//! `review::review_task`。計画なしの直行経路を含む）の health の項目。`spawn_review` が repo ごとに評価する作業ツリー
//! （`RepoHealth::At`）を置き、`HealthInputJob::attach`（review の非同期 task の中。G2 の材料の後）が [`evaluate_all`] で
//! 評価して `RepoHealth::Evaluated` にする。`review/required.rs` の `evaluate` がそれを必須検査の列の 1 項目にする。
//! 評価は経路共通の入口（[`gate::health_gate`]）だけで、検査器の起動をここに書かない。
//!
//! - 候補（review 前の同期の `(target_sha, reviewed_sha)`）がある repo: base=`target_sha`・head=`reviewed_sha` で評価する。
//!   同じ候補で G2 の材料（`health_review::input`）が評価していれば、その評価（`gate::check_with` の `HealthRun`）を
//!   使い、検査器を 2 回起こさない。束縛（repo・base・head）が違う評価は使わない。統合の修復
//!   （`IntegrationRepairScheduled`）の後の review は新しい候補で評価し直す（前の評価は束縛が違う）。
//! - 項目: Applies なら `cmd =` [`HEALTH_CHECK_ID`] の項目を足し、指紋に検査器を入れる。Pass と PolicyOnly（G2 の
//!   `policy_review_gate` が人の承認を求める）は合格、Fail は不合格で repair の材料（`HealthItem::repair_hint`）を持つ。
//!   Unavailable・Error は `health_check_error: <理由>` の不合格で、repair に回さない（`Verdict::implicit_check`）。
//! - NotApplicable（Git 管理でない・remote workspace）と候補の無い repo は項目を足さず、理由を残す（`HealthItem::note`）。
//!
//! - 再利用（D2.5。reviewer の合格の再利用・同期の後の再 review・統合の修復の後を含む）: 束縛の同じ G2 の評価が無ければ、
//!   同じ task の event の証跡を [`task_core::required_checks::reuse_health`] で引く。同じ `(base, head の tree, policy,
//!   検査器)` の Pass だけを今の `(base, head)` に束縛し直して `reused_from` を書き、検査器を起こさない。tree・base・
//!   検査器・policy が違えば、同期（rebase）で head の tree が変わったときも、入口 [`gate::health_gate`] で評価し直す。
//!   reviewer を省いても health は省かない（束縛が合うときだけ写す）。[`reusable_checks`] は必須検査の cmd の再利用に同じ
//!   適用判定の検査器を渡す（定義指紋が検査器を含むため）。
//!
//! LLM は呼ばない。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use task_core::required_checks::health::{
    CheckerId, HEALTH_CHECK_ID, HealthApplicability, HealthEvidence,
};
use task_core::required_checks::{
    HealthCandidate, HealthRequirement, reusable_passes, reuse_health,
};
use task_core::{Event, PhaseCheckResult, ReviewRepairHint};

use super::gate::{
    self, BundledChecker, HealthGateDecision, HealthGateOutcome, HealthGateRequest, HealthRun,
};
use super::input::{HEALTH_INPUT_TIMEOUT, NOT_SYNCED};
use super::policy::git_stdout;
use crate::review::{
    HEALTH_CHECK_ERROR, HealthItem, RepoHealth, RequiredCheckBinding, RequiredRepoChecks,
};

/// PolicyOnly の項目の理由（合格。人の承認は G2 の `policy_review_gate` が求める）。
pub const HEALTH_POLICY_ONLY: &str =
    "health: policy_only（policy の file だけの変更。policy_review_gate が人の承認を求める）";

/// D1.2: `checks` の未評価の health（`RepoHealth::At`）を評価する。`runs` は G2 の材料が同じ review で評価した結果で、
/// 束縛（repo・base=`target_sha`・head=`reviewed_sha`）が同じものだけを使う。`remote` は ssh の remote workspace
/// （Celeris が統合しない。NotApplicable）。`bundled` は base に検査器の無い repo に当てる同梱の検査器。
pub async fn evaluate_all(
    checks: &mut [RequiredRepoChecks],
    mut runs: Vec<HealthRun>,
    remote: bool,
    bundled: Option<&'static BundledChecker>,
    events: Vec<Event>,
) {
    let events = Arc::new(events);
    for check in checks.iter_mut() {
        let Some(RepoHealth::At(dir)) = check.health.take() else {
            continue;
        };
        let binding = check.binding.clone();
        let prior = binding
            .as_ref()
            .and_then(|b| runs.iter().position(|run| binds(run, b)))
            .map(|i| runs.swap_remove(i));
        let recorded = Arc::clone(&events);
        let item = tokio::task::spawn_blocking(move || {
            evaluate(&dir, binding.as_ref(), remote, bundled, prior, &recorded)
        })
        .await
        .unwrap_or_else(|e| error_item(&format!("検査を起こせない: {e}")));
        check.health = Some(RepoHealth::Evaluated(item));
    }
}

fn binds(run: &HealthRun, binding: &RequiredCheckBinding) -> bool {
    run.evidence.repo_id == binding.repo_id
        && run.evidence.base_sha == binding.target_sha
        && run.evidence.head_sha == binding.reviewed_sha
}

/// 1 repo の評価（blocking。git と検査器を起こす）。`prior` は束縛の同じ G2 の評価（あれば検査器を起こさない）。
fn evaluate(
    dir: &Path,
    binding: Option<&RequiredCheckBinding>,
    remote: bool,
    bundled: Option<&'static BundledChecker>,
    prior: Option<HealthRun>,
    events: &[Event],
) -> HealthItem {
    let Some(binding) = binding else {
        return HealthItem {
            note: Some(unbound_note(dir, remote, bundled)),
            ..HealthItem::default()
        };
    };
    let reused = match prior {
        Some(_) => None,
        None => reuse(dir, binding, !remote, bundled, events),
    };
    let outcome = match (prior, reused) {
        (Some(run), _) => HealthGateOutcome::Evaluated(run),
        (None, Some(evidence)) => HealthGateOutcome::Evaluated(HealthRun {
            evidence,
            failures: Vec::new(),
        }),
        (None, None) => gate::health_gate(&HealthGateRequest {
            repo_id: binding.repo_id,
            worktree: dir,
            base: &binding.target_sha,
            head: &binding.reviewed_sha,
            integrated_by_celeris: !remote,
            bundled,
            timeout: HEALTH_INPUT_TIMEOUT,
        }),
    };
    item(&outcome)
}

/// 今の候補に束縛し直して写せる health の証跡（D2.5）。適用判定が `Applies` で head の tree を読め、同じ task の event に
/// 同じ `(base, tree, policy, 検査器)` の Pass があるときだけ。読めない・合わないときは `None`（入口で評価し直す）。
fn reuse(
    dir: &Path,
    binding: &RequiredCheckBinding,
    integrated_by_celeris: bool,
    bundled: Option<&BundledChecker>,
    events: &[Event],
) -> Option<HealthEvidence> {
    let (checker, tree) = requirement_of(dir, binding, integrated_by_celeris, bundled)?;
    reuse_health(
        events,
        &HealthCandidate {
            repo_id: binding.repo_id,
            base_sha: &binding.target_sha,
            head_sha: &binding.reviewed_sha,
            head_tree: &tree,
            checker_sha256: &checker.sha256,
        },
    )
}

/// 候補 `binding` の適用判定の検査器と、`reviewed_sha` の tree（適用判定が `Applies` で tree を読めたときだけ）。
fn requirement_of(
    dir: &Path,
    binding: &RequiredCheckBinding,
    integrated_by_celeris: bool,
    bundled: Option<&BundledChecker>,
) -> Option<(CheckerId, String)> {
    let deadline = Instant::now() + HEALTH_INPUT_TIMEOUT;
    let HealthApplicability::Applies { checker } = gate::applicable(
        dir,
        &binding.target_sha,
        integrated_by_celeris,
        bundled,
        deadline,
    ) else {
        return None;
    };
    let tree = git_stdout(
        dir,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{tree}}", binding.reviewed_sha),
        ],
        deadline,
    )
    .ok()?;
    Some((checker, tree.trim().to_string()))
}

/// `spawn_review` が repo ごとに呼ぶ、同じ束縛で合格済みの必須検査の cmd。health の要求（今の適用判定の検査器）を
/// `reusable_passes` に渡す（定義指紋が検査器を含む）。要求を作れない（Applies でない・tree を読めない）ときは
/// `NotRequired` で、health の当たる候補の証跡とは指紋が合わず再利用しない（fail closed）。
pub fn reusable_checks(
    events: &[Event],
    binding: &RequiredCheckBinding,
    dir: &Path,
    remote: bool,
    cmds: &[String],
) -> BTreeSet<String> {
    let required = requirement_of(dir, binding, !remote, gate::bundled_checker());
    let health = match &required {
        Some((checker, tree)) => HealthRequirement::Applies {
            checker,
            reviewed_tree: tree,
        },
        None => HealthRequirement::NotRequired,
    };
    reusable_passes(
        events,
        binding.repo_id,
        &binding.target_sha,
        &binding.reviewed_sha,
        cmds,
        health,
    )
}

fn error_item(reason: &str) -> HealthItem {
    HealthItem {
        result: Some(PhaseCheckResult::new(
            HEALTH_CHECK_ID.into(),
            false,
            format!("{HEALTH_CHECK_ERROR}: {reason}"),
        )),
        ..HealthItem::default()
    }
}

/// 候補の無い repo: 適用判定（同じ `repo_facts` → `applicability`）が NotApplicable ならその理由、それ以外は
/// 候補の SHA が無いので評価しない理由（delivery の照合も候補の無い review を通さない）。
fn unbound_note(dir: &Path, remote: bool, bundled: Option<&BundledChecker>) -> String {
    let deadline = Instant::now() + HEALTH_INPUT_TIMEOUT;
    match gate::applicable(dir, "HEAD", !remote, bundled, deadline) {
        applies @ HealthApplicability::NotApplicable { .. } => applies.reason().unwrap_or_default(),
        _ => NOT_SYNCED.to_string(),
    }
}

/// D3.1〜D3.4: 入口の結果を必須検査の項目にする。
pub fn item(outcome: &HealthGateOutcome) -> HealthItem {
    let checker = match outcome {
        HealthGateOutcome::Evaluated(run) => run.evidence.checker.clone(),
        _ => None,
    };
    let Some(evidence) = outcome.item() else {
        return HealthItem {
            note: outcome.evidence().reason.clone(),
            ..HealthItem::default()
        };
    };
    let mut repair_hint = None;
    let (pass, summary) = match outcome.decision() {
        HealthGateDecision::Pass => (true, "health: pass".to_string()),
        HealthGateDecision::PolicyOnly => (true, HEALTH_POLICY_ONLY.to_string()),
        HealthGateDecision::Repair(repairs) => {
            let buckets: Vec<String> = repairs.iter().map(|r| r.bucket()).collect();
            repair_hint = Some(ReviewRepairHint {
                scope: HEALTH_CHECK_ID.to_string(),
                class: buckets.join(","),
            });
            let reasons: Vec<&str> = repairs.iter().map(|r| r.reason.as_str()).collect();
            (false, reasons.join("; "))
        }
        HealthGateDecision::Error { reason } => (false, format!("{HEALTH_CHECK_ERROR}: {reason}")),
        // `item()` が `Some` なら NotApplicable ではない（合格にしない側に倒す）。
        HealthGateDecision::NotApplicable => {
            (false, format!("{HEALTH_CHECK_ERROR}: not_applicable"))
        }
    };
    HealthItem {
        result: Some(PhaseCheckResult {
            cmd: HEALTH_CHECK_ID.to_string(),
            pass,
            summary,
            health: Some(Box::new(evidence.clone())),
        }),
        checker,
        repair_hint,
        note: None,
    }
}
