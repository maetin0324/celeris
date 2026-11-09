//! ADR 2026-11-08-health-enforcement-loop D1.2・D3.1〜D3.4: atomic/direct の task の review（`Dispatcher::spawn_review` →
//! `review::review_task`。計画なしの直行経路を含む）の health の項目。`review/required.rs` の `evaluate` が必須検査の列の
//! 1 項目として呼ぶ。評価は経路共通の入口（[`gate::health_gate`]）だけで、検査器の起動をここに書かない。
//!
//! - 候補（review 前の同期の `(target_sha, reviewed_sha)`）がある repo: base=`target_sha`・head=`reviewed_sha` で評価する。
//!   同じ候補で G2 の材料（`health_review::input`）が評価していれば、その評価（`gate::check_with` の `HealthRun`）を
//!   使い、検査器を 2 回起こさない（[`adopt`]）。束縛（repo・base・head）が違う評価は使わない。統合の修復
//!   （`IntegrationRepairScheduled`）の後の review は新しい候補で評価し直す（前の評価は束縛が違う）。
//! - 項目: Applies なら `cmd =` [`HEALTH_CHECK_ID`] の項目を足し、指紋に検査器を入れる。Pass と PolicyOnly（G2 の
//!   `policy_review_gate` が人の承認を求める）は合格、Fail は不合格で repair の材料（[`repair_hint`]）を持つ。
//!   Unavailable・Error は `health_check_error: <理由>` の不合格で、repair に回さない（[`implicit_check`]）。
//! - NotApplicable（Git 管理でない・remote workspace）と候補の無い repo は項目を足さず、理由を残す（[`HealthItem::note`]）。
//!
//! LLM は呼ばない。

use std::path::PathBuf;
use std::time::Instant;

use task_core::required_checks::health::{CheckerId, HEALTH_CHECK_ID, HealthApplicability};
use task_core::{Check, PhaseCheckResult, ReviewRepairHint};

use super::gate::{
    self, BundledChecker, HealthGateDecision, HealthGateOutcome, HealthGateRequest, HealthRun,
};
use super::input::{HEALTH_INPUT_TIMEOUT, NOT_SYNCED};
use crate::review::{RequiredCheckBinding, RequiredRepoChecks, Verdict};

/// Error・Unavailable の項目の理由の接頭辞（D3.4）。
pub const HEALTH_CHECK_ERROR: &str = "health_check_error";
/// PolicyOnly の項目の理由（合格。人の承認は G2 の `policy_review_gate` が求める）。
pub const HEALTH_POLICY_ONLY: &str =
    "health: policy_only（policy の file だけの変更。policy_review_gate が人の承認を求める）";

/// review の repo の health の評価の場所。`spawn_review` が tick の中で作る（git も検査器も起こさない）。
#[derive(Debug)]
pub struct RequiredHealth {
    /// 評価する作業ツリー（候補があれば同期した worktree）。
    pub dir: PathBuf,
    /// ssh の remote workspace（Celeris が統合しない。NotApplicable）。
    pub remote: bool,
    /// release に同梱した検査器（daemon が起動で設定したもの。経路の外側で読んで渡す）。
    pub bundled: Option<&'static BundledChecker>,
    /// 同じ候補で G2 の材料が評価した結果（[`adopt`] が入れる）。
    pub prior: Option<HealthRun>,
}

impl RequiredHealth {
    /// `spawn_review` の repo の評価の場所（同梱の検査器は process に設定されたもの）。
    pub fn new(dir: PathBuf, remote: bool) -> Self {
        RequiredHealth {
            dir,
            remote,
            bundled: gate::bundled_checker(),
            prior: None,
        }
    }
}

/// 1 repo の health の結果。
#[derive(Debug, Default)]
pub struct HealthItem {
    /// 必須検査の列に足す項目（NotApplicable・候補の無い repo は `None`）。
    pub result: Option<PhaseCheckResult>,
    /// 定義指紋に入れる検査器（適用判定が Applies のときだけ）。
    pub checker: Option<CheckerId>,
    /// Fail の repair の材料（`Verdict.repair_hint`）。
    pub repair_hint: Option<ReviewRepairHint>,
    /// 項目を足さない理由（NotApplicable の `HealthApplicability::reason()`・候補が無い）。
    pub note: Option<String>,
}

/// G2 の材料が評価した `runs` のうち、候補の束縛（repo・base=`target_sha`・head=`reviewed_sha`）が同じものを
/// `checks` の [`RequiredHealth::prior`] に入れる。束縛の違う評価（修復の前の head など）は入れない。
pub fn adopt(checks: &mut [RequiredRepoChecks], runs: Vec<HealthRun>) {
    for run in runs {
        let found = checks
            .iter_mut()
            .find(|c| c.health.is_some() && c.binding.as_ref().is_some_and(|b| binds(&run, b)));
        if let Some(health) = found.and_then(|c| c.health.as_mut()) {
            health.prior = Some(run);
        }
    }
}

fn binds(run: &HealthRun, binding: &RequiredCheckBinding) -> bool {
    run.evidence.repo_id == binding.repo_id
        && run.evidence.base_sha == binding.target_sha
        && run.evidence.head_sha == binding.reviewed_sha
}

/// D1.2: review の 1 repo の health を評価する（blocking な git・検査器は `spawn_blocking` で走らせる）。
pub async fn evaluate(
    health: RequiredHealth,
    binding: Option<&RequiredCheckBinding>,
) -> HealthItem {
    let Some(binding) = binding.cloned() else {
        let note = tokio::task::spawn_blocking(move || unbound_note(&health))
            .await
            .unwrap_or_else(|e| format!("health の適用判定を起こせない: {e}"));
        return HealthItem {
            note: Some(note),
            ..HealthItem::default()
        };
    };
    let outcome = match health.prior {
        Some(run) if binds(&run, &binding) => Ok(HealthGateOutcome::Evaluated(run)),
        _ => {
            tokio::task::spawn_blocking(move || {
                gate::health_gate(&HealthGateRequest {
                    repo_id: binding.repo_id,
                    worktree: &health.dir,
                    base: &binding.target_sha,
                    head: &binding.reviewed_sha,
                    integrated_by_celeris: !health.remote,
                    bundled: health.bundled,
                    timeout: HEALTH_INPUT_TIMEOUT,
                })
            })
            .await
        }
    };
    match outcome {
        Ok(outcome) => item(&outcome),
        Err(e) => HealthItem {
            result: Some(PhaseCheckResult::new(
                HEALTH_CHECK_ID.into(),
                false,
                format!("{HEALTH_CHECK_ERROR}: 検査を起こせない: {e}"),
            )),
            ..HealthItem::default()
        },
    }
}

/// 候補の無い repo: 適用判定（同じ `repo_facts` → `applicability`）が NotApplicable ならその理由、それ以外は
/// 候補の SHA が無いので評価しない理由（delivery の照合も候補の無い review を通さない）。
fn unbound_note(health: &RequiredHealth) -> String {
    let deadline = Instant::now() + HEALTH_INPUT_TIMEOUT;
    match gate::applicable(
        &health.dir,
        "HEAD",
        !health.remote,
        health.bundled,
        deadline,
    ) {
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

/// `try_review_repair` が acceptance の外の verdict に当てる検査（D3.2）。health の Fail（[`HealthItem::repair_hint`]
/// を持つ verdict）は `Check::Command { cmd: HEALTH_CHECK_ID }` として `classify_review_failure` に渡し、ほかの暗黙の条件
/// （Plan・aggregate・必須検査・research・health の Error）は従来どおり `Check::Human`（修復しない）。
pub fn implicit_check(verdict: &Verdict) -> Check {
    match &verdict.repair_hint {
        Some(hint) if hint.scope == HEALTH_CHECK_ID => Check::Command {
            cmd: HEALTH_CHECK_ID.to_string(),
            expect_exit: 0,
        },
        _ => Check::Human,
    }
}
