//! ADR 2026-11-08-health-enforcement-loop D1.2・D2.2・D3.2・D3.4: WU の check と段の統合が必須検査の列に足す health の
//! 項目（`cmd` は [`HEALTH_CHECK_ID`]）。
//!
//! 両方の経路は `dispatcher::phase_integration::run_integration_checks` を通り、そこがこの module の [`evaluate`] を
//! 1 回呼ぶ（経路ごとに検査器を起こさない）。[`evaluate`] は経路共通の入口 [`gate::health_gate`] を呼び、扱い
//! （[`HealthGateDecision`]）を検査の列の 1 行（`pass`・`reason`）に写す。写しの規則はここだけに置く:
//!
//! - Pass・PolicyOnly: 合格（PolicyOnly は WU・段の統合では先へ進める。D3.1）。
//! - Repair: 不合格。reason は rule ごとの `health:<rule_id>:<hash>[,…]（evidence: …）` を `; ` で並べた文で、
//!   `task_core::classify_review_failure` が `RepairClass::Health` に分ける（D3.2）。class の数え方は
//!   [`repair_buckets`]（`health:<rule_id>`）。
//! - Error（Unavailable を含む）: 不合格。reason は `health_check_error: <理由>`（[`check_error`] が読む）。repair に回さず、
//!   経路は task を Blocked にする（D3.4）。
//! - NotApplicable: 列に足さない（event には理由を持つ証跡を残す）。
//!
//! 修復の後の再評価は、経路が新しい head で [`evaluate`] を呼び直す（前の証跡を使わない。ここは何も覚えない）。

use std::path::PathBuf;

use task_core::RepoId;
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthEvidence};

use super::gate::{self, BundledChecker, HealthGateDecision, HealthGateRequest};
use super::input::HEALTH_INPUT_TIMEOUT;

/// D3.4: Error・Unavailable の項目の reason の先頭の語（`health_check_error: <理由>`）。
pub const HEALTH_CHECK_ERROR: &str = "health_check_error";

/// health を評価する 1 つの repo の作業ツリーと範囲（経路が組み立てる。D1.2 の表）。
#[derive(Debug, Clone)]
pub struct HealthTarget {
    pub repo_id: RepoId,
    /// git と検査器を実行する作業ツリー（WU は WU の作業ツリー、段の統合は統合後の Task の作業ツリー）。
    pub worktree: PathBuf,
    /// WU は `wu.base_commit`、段の統合は段の基点。
    pub base: String,
    /// WU は WU の HEAD、段の統合は統合の HEAD（証跡には解決した sha が残る）。
    pub head: String,
    /// release に同梱した検査器（経路の外側で読んだ値）。
    pub bundled: Option<BundledChecker>,
}

/// 検査の列に足す health の 1 行と、event に残す証跡。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthItem {
    /// 列に足すか（NotApplicable は `false`。event には `evidence` を残す）。
    pub counted: bool,
    pub pass: bool,
    /// Pass は `Some(0)`、Repair は `Some(1)`、Error と NotApplicable は `None`。
    pub exit: Option<i32>,
    pub reason: String,
    pub evidence: HealthEvidence,
}

/// D1.2: `target` を経路共通の入口で 1 回評価し、列の 1 行に写す（blocking。git と検査器の子 process を起こす）。
pub fn evaluate(target: &HealthTarget) -> HealthItem {
    let outcome = gate::health_gate(&HealthGateRequest {
        repo_id: target.repo_id,
        worktree: &target.worktree,
        base: &target.base,
        head: &target.head,
        integrated_by_celeris: true,
        bundled: target.bundled.as_ref(),
        timeout: HEALTH_INPUT_TIMEOUT,
    });
    let evidence = outcome.evidence().clone();
    let (counted, pass, exit, reason) = match outcome.decision() {
        HealthGateDecision::NotApplicable => (
            false,
            true,
            None,
            format!(
                "health: not applicable: {}",
                evidence.reason.as_deref().unwrap_or_default()
            ),
        ),
        HealthGateDecision::Pass => (true, true, Some(0), "health: pass".to_string()),
        HealthGateDecision::PolicyOnly => (
            true,
            true,
            Some(0),
            format!(
                "health: {}",
                evidence.reason.as_deref().unwrap_or("policy_only")
            ),
        ),
        HealthGateDecision::Repair(repairs) => (
            true,
            false,
            Some(1),
            repairs
                .iter()
                .map(|r| r.reason.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ),
        HealthGateDecision::Error { reason } => {
            (true, false, None, format!("{HEALTH_CHECK_ERROR}: {reason}"))
        }
    };
    HealthItem {
        counted,
        pass,
        exit,
        reason,
        evidence,
    }
}

/// 列の 1 行が health の Error（D3.4）なら、その reason（`health_check_error: <理由>`）。
pub fn check_error<'a>(cmd: &str, pass: bool, reason: &'a str) -> Option<&'a str> {
    let rest = reason.strip_prefix(HEALTH_CHECK_ERROR)?;
    (cmd == HEALTH_CHECK_ID && !pass && rest.starts_with(':')).then_some(reason)
}

/// D3.2・D4.1: health の Repair の reason（[`evaluate`] が作る）から、`max_repairs_per_class` を数える rule ごとの
/// bucket（`health:<rule_id>`、重複なし・順は reason のまま）。health の Repair でなければ空。
pub fn repair_buckets(reason: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in reason.split("; ") {
        let Some(item) = part.strip_prefix(task_core::execution::HEALTH_REPAIR_REASON_PREFIX)
        else {
            continue;
        };
        let first = item.split([',', '（']).next().unwrap_or_default();
        if let Some((rule_id, _hash)) = first.rsplit_once(':') {
            let bucket = format!("health:{rule_id}");
            if !out.contains(&bucket) {
                out.push(bucket);
            }
        }
    }
    out
}

/// WU の check の列に足す health の項目の check（`WorkUnitCheckFinished` の cmd と不合格の記録に使う）。
pub fn work_unit_check() -> task_core::WorkUnitCheck {
    task_core::WorkUnitCheck {
        cmd: HEALTH_CHECK_ID.to_string(),
        expect_exit: 0,
        scope: false,
    }
}

#[cfg(test)]
#[path = "check_item_tests.rs"]
mod tests;
