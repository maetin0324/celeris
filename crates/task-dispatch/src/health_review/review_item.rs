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
//! LLM は呼ばない。

use std::path::Path;
use std::time::Instant;

use task_core::PhaseCheckResult;
use task_core::ReviewRepairHint;
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthApplicability};

use super::gate::{
    self, BundledChecker, HealthGateDecision, HealthGateOutcome, HealthGateRequest, HealthRun,
};
use super::input::{HEALTH_INPUT_TIMEOUT, NOT_SYNCED};
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
) {
    for check in checks.iter_mut() {
        let Some(RepoHealth::At(dir)) = check.health.take() else {
            continue;
        };
        let binding = check.binding.clone();
        let prior = binding
            .as_ref()
            .and_then(|b| runs.iter().position(|run| binds(run, b)))
            .map(|i| runs.swap_remove(i));
        let item = tokio::task::spawn_blocking(move || {
            evaluate(&dir, binding.as_ref(), remote, bundled, prior)
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
) -> HealthItem {
    let Some(binding) = binding else {
        return HealthItem {
            note: Some(unbound_note(dir, remote, bundled)),
            ..HealthItem::default()
        };
    };
    let outcome = match prior {
        Some(run) => HealthGateOutcome::Evaluated(run),
        None => gate::health_gate(&HealthGateRequest {
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
