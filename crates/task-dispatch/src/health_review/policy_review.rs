//! ADR 2026-11-07-health-review-integration D6.4・I8: policy の file を変える candidate は、reviewer が承認
//! （`HealthVerdictOut.policy.approved = true`）した後も人の承認なしには合格しない。
//!
//! `dispatcher::child_tasks` の子 module として `#[path]` で読む（`child_tasks.rs` の `mod policy_review`）。file を
//! `health_review/` に置くのは、`dispatcher/` の module が repo health の `size.responsibility-growth` の閾値を
//! 超えているため。待ちは `Approval` の子 task（SQLite）だけで表し、dispatcher のメモリに持たない（I9 と同じ）。
//!
//! 流れ（`Check::Human` の既存の経路を使う）:
//! - [`Dispatcher::policy_review_gate`]（`on_review_finished` が合否を数える前）: review が判定した candidate の
//!   （`Check::Reviewer` の無い task・reviewer run の無い review も含む。I8）policy の状態（`HealthReviewInputBuilt` の `input.policy`・`HealthReviewInputSkipped` の `policy`）が
//!   `changed` か不明（report を作れない・方針を読めない・diff を取れない。fail closed）で、ほかの判定が全部合格なら、
//!   candidate ごとの `Approval` の子を探す（不明には D6.3 の reviewer の policy verdict を求めない）。無ければ作る（`ApprovalRequested` → 受信箱。本文に policy の file と diff の要約）。子が終端でない間は
//!   既存の「委譲した子の待ち」で reviewing のまま待つ。人の承認（done）は合格の verdict、却下（failed・
//!   cancelled）は substantive の不合格（→ replan）を足す。
//! - [`Dispatcher::policy_approval_pending`]（`resolve_human_approvals` の先頭）: 承認の子が終端でない間は review を
//!   始めない（daemon の再起動の後も待つ）。
//! - [`Dispatcher::policy_rereview`]（`settle_awaiting_children`）: 承認の子が終端になったら直接 `ReviewPass` に
//!   せず、review をやり直して上の判定に通す（reviewer の合格は ADR 2026-10-27 D2 で再利用される）。
//!
//! 判定に使う policy の状態は base 側の検査器で作った event（I3）だけから読む。同じ candidate に既知の状態が
//! あればそれを使い（後の review で report を作り直せた）、無ければ不明の記録を使う。LLM は呼ばない。

use super::*;
use crate::health_review::input::{POLICY_UNKNOWN, reused_from};
use crate::review::RequiredCheckBinding;
use task_worker::protocol::{DiffStat, HealthPolicyState, HealthRepoInput};

/// policy の変更の承認を待つ `Approval` の子の title の接頭辞（`derive::human_approval_title` と重ならない）。
pub(crate) const POLICY_APPROVAL_TITLE: &str = "Policy change approval: ";

/// 人の却下の不合格に付ける repair の class（scope は substantive。局所修復にしない）。
const POLICY_REJECTED_CLASS: &str = "policy_rejected";

/// reviewer が判定した 1 repo の policy の変更（または決められなかった状態）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyChange {
    pub repo_id: String,
    pub target_sha: String,
    /// reviewer が見た candidate の SHA（承認は candidate ごと）。
    pub reviewed_sha: String,
    /// 変わった policy の file（不明なら空）。
    pub files: Vec<String>,
    /// reviewer に渡した材料の diff（材料を渡さなかった repo は `None`）。
    pub diff: Option<DiffStat>,
    /// policy の状態を決められなかった理由（`Some` は不明。I8 の fail closed で変更と同じに扱う）。
    pub unknown: Option<String>,
}

impl PolicyChange {
    /// verdict の reason に載せる対象（policy の file か、不明の理由）。
    fn subject(&self) -> String {
        match &self.unknown {
            Some(why) => format!(
                "policy state unknown ({})",
                why.chars().take(300).collect::<String>()
            ),
            None => self.files.join(", "),
        }
    }
}

/// この review で判定した candidate のうち、policy の file を変えたもの・policy の状態が不明なもの。
/// `reviewer_ran` が false の review は、reviewer の合格を再利用した candidate（`ReviewerPassReused`）なら元の
/// candidate を見て、それ以外（reviewer run の無い review。I8）はその candidate を見る。
pub(crate) fn policy_changes<'a>(
    events: &[(u64, Event)],
    reviewer_ran: bool,
    bindings: impl IntoIterator<Item = &'a RequiredCheckBinding>,
) -> Vec<PolicyChange> {
    let mut out = Vec::new();
    for b in bindings {
        let own = || (b.target_sha.clone(), b.reviewed_sha.clone());
        let (target, reviewed) = if reviewer_ran {
            own()
        } else {
            reused_from(events, b.repo_id, &b.target_sha, &b.reviewed_sha).unwrap_or_else(own)
        };
        out.extend(candidate_policy(
            events,
            &b.repo_id.to_string(),
            &target,
            &reviewed,
        ));
    }
    out
}

/// 1 candidate の policy の状態の記録のうち、最後の既知の状態（変更があれば `Some`）。既知の記録が無ければ最後の
/// 不明の記録（`unknown` 付き）。記録が無い（base に検査器が無い・材料を作っていない）なら `None`。
fn candidate_policy(
    events: &[(u64, Event)],
    repo: &str,
    target: &str,
    reviewed: &str,
) -> Option<PolicyChange> {
    let mut unknown = None;
    for (_, e) in events.iter().rev() {
        let recorded: Result<(HealthPolicyState, Option<DiffStat>, String), String> = match e {
            Event::HealthReviewInputBuilt {
                repo_id,
                target_sha,
                reviewed_sha,
                input,
                ..
            } if repo_id == repo && target_sha == target && reviewed_sha == reviewed => {
                serde_json::from_value::<HealthRepoInput>(input.clone())
                    .map(|i| {
                        (
                            i.policy,
                            Some(i.diff),
                            i.findings.reason.unwrap_or_default(),
                        )
                    })
                    .map_err(|e| format!("HealthReviewInputBuilt の input を読めない: {e}"))
            }
            Event::HealthReviewInputSkipped {
                repo_id,
                target_sha,
                reviewed_sha,
                policy: Some(policy),
                reason,
                ..
            } if repo_id == repo && target_sha == target && reviewed_sha == reviewed => {
                serde_json::from_value::<HealthPolicyState>(policy.clone())
                    .map(|p| (p, None, reason.clone()))
                    .map_err(|e| format!("HealthReviewInputSkipped の policy を読めない: {e}"))
            }
            _ => continue,
        };
        let change = |files, diff, why| PolicyChange {
            repo_id: repo.to_string(),
            target_sha: target.to_string(),
            reviewed_sha: reviewed.to_string(),
            files,
            diff,
            unknown: why,
        };
        match recorded {
            Ok((policy, diff, _)) if policy.source != POLICY_UNKNOWN => {
                return policy.changed.then(|| change(policy.files, diff, None));
            }
            Ok((_, diff, why)) => {
                unknown.get_or_insert_with(|| change(Vec::new(), diff, Some(why)));
            }
            Err(why) => {
                unknown.get_or_insert_with(|| change(Vec::new(), None, Some(why)));
            }
        }
    }
    unknown
}

/// candidate ごとの承認の子の title（照合のキー）。
pub(crate) fn approval_title(change: &PolicyChange) -> String {
    format!(
        "{POLICY_APPROVAL_TITLE}{} @ {}",
        change.repo_id,
        &change.reviewed_sha[..change.reviewed_sha.len().min(12)]
    )
}

/// 受信箱に出す本文: policy の file（不明ならその理由）・diff の要約・reviewer の承認の理由。
pub(crate) fn approval_objective(change: &PolicyChange, reviewer_reason: Option<&str>) -> String {
    let c = change;
    let mut lines = vec![
        match &c.unknown {
            None if reviewer_reason.is_some() => "The reviewer approved a change to the repo-health policy files. It passes only with a human approval (ADR 2026-11-07-health-review-integration D6.4). Reject to send it back for a replan.",
            None => "This candidate changes the repo-health policy files. It passes only with a human approval (ADR 2026-11-07-health-review-integration I8・D6.4). Reject to send it back for a replan.",
            Some(_) => "The repo-health policy state of this candidate could not be determined, so it is treated as a policy change (fail closed, ADR 2026-11-07-health-review-integration I8・D6.4). It passes only with a human approval. Reject to send it back for a replan.",
        }
        .to_string(),
        format!(
            "repo {}: base {} → candidate {}",
            c.repo_id, c.target_sha, c.reviewed_sha
        ),
        match &c.unknown {
            None => format!("policy files: {}", c.files.join(", ")),
            Some(why) => format!("policy state unknown: {why}"),
        },
    ];
    if let Some(diff) = &c.diff {
        lines.push(format!(
            "diff: {} files, +{} -{}",
            diff.files, diff.insertions, diff.deletions
        ));
        lines.extend(
            diff.paths
                .iter()
                .map(|p| format!("- {} (+{} -{})", p.path, p.insertions, p.deletions)),
        );
    }
    if let Some(reason) = reviewer_reason {
        lines.push(format!("reviewer: {reason}"));
    }
    lines.join("\n")
}

/// 終端の承認の子を verdict にする（終端でなければ `None` = 待つ）。`note` は `approval_decision_note` の値。
pub(crate) fn approval_verdict(
    child: &Task,
    note: &str,
    criterion_idx: usize,
    files: &str,
) -> Option<Verdict> {
    let rejected = |reason: String| Verdict {
        criterion_idx,
        pass: false,
        reason,
        repair_hint: Some(task_core::execution::ReviewRepairHint {
            scope: crate::health_review::verdict::SUBSTANTIVE_SCOPE.to_string(),
            class: POLICY_REJECTED_CLASS.to_string(),
        }),
    };
    let tag = format!("human(approval task {})", child.id);
    match child.status {
        Status::Done => Some(Verdict {
            criterion_idx,
            pass: true,
            reason: format!("{tag}: health:policy approved by a human: {files}"),
            repair_hint: None,
        }),
        Status::Failed => Some(rejected(format!(
            "{tag}: health:policy: rejected by a human{note}: {files}"
        ))),
        Status::Cancelled => Some(rejected(format!(
            "{tag}: health:policy: approval cancelled: {files}"
        ))),
        _ => None,
    }
}

/// 人の承認・却下の verdict を付ける criterion: `Check::Reviewer` の条件があればその条件（従来どおり）。無ければ
/// この review の verdict の最大の `criterion_idx` の次（acceptance の外の暗黙の条件。delivery の reviewer の条件・
/// plan・必須検査の暗黙の条件と重ならない）。acceptance の外の不合格は `try_review_repair` が `Check::Human` と同じに
/// 扱う（修復しない = substantive → replan）ので、却下が `Check::Command` の局所修復に化けない。
pub(crate) fn policy_criterion(task: &Task, outcome: &ReviewOutcome) -> usize {
    let reviewer = task
        .acceptance
        .iter()
        .position(|c| matches!(c.check, Check::Reviewer));
    reviewer.unwrap_or_else(|| {
        let next = outcome.verdicts.iter().map(|v| v.criterion_idx + 1).max();
        next.unwrap_or(0).max(task.acceptance.len())
    })
}

fn is_policy_approval(task: &Task) -> bool {
    task.kind == TaskKind::Approval && task.title.starts_with(POLICY_APPROVAL_TITLE)
}

impl Dispatcher {
    /// `on_review_finished` が `all_pass` を数える前に呼ぶ。policy の変更が無い review・不合格を含む review では
    /// 何もしない（不合格は従来どおり repair/replan）。`Check::Reviewer` の無い task（reviewer run の無い review。
    /// I8）にも掛ける。verdict を付ける criterion は [`policy_criterion`]。
    pub(crate) fn policy_review_gate(
        &self,
        task: &Task,
        review_run: Option<&str>,
        outcome: &mut ReviewOutcome,
    ) -> Result<(), DispatchError> {
        let bindings: Vec<&RequiredCheckBinding> = outcome
            .required_checks
            .iter()
            .filter_map(|r| r.binding.as_ref())
            .collect();
        if bindings.is_empty() {
            return Ok(());
        }
        let criterion = policy_criterion(task, outcome);
        let changes = policy_changes(
            &self.store.events_for(task.id)?,
            review_run.is_some(),
            bindings,
        );
        if changes.is_empty() || !outcome.all_pass() {
            return Ok(());
        }
        let reviewer_reason = outcome
            .verdicts
            .iter()
            .find(|v| v.reason.contains("health:policy (approved by reviewer"))
            .map(|v| v.reason.clone());
        let children = self.store.children(task.id)?;
        for change in &changes {
            let title = approval_title(change);
            let files = change.subject();
            match children
                .iter()
                .find(|c| is_policy_approval(c) && c.title == title)
            {
                None => {
                    let objective = approval_objective(change, reviewer_reason.as_deref());
                    self.create_human_approval_child(task, &title, &objective)?;
                }
                Some(child) => {
                    let note = approval_decision_note(&self.store.events_for(child.id)?);
                    outcome
                        .verdicts
                        .extend(approval_verdict(child, &note, criterion, &files));
                }
            }
        }
        Ok(())
    }

    /// policy の変更の承認の子が終端でない間は review を始めない（`resolve_human_approvals` の待ちと同じ）。
    pub(crate) fn policy_approval_pending(&self, task: &Task) -> Result<bool, DispatchError> {
        Ok(self
            .store
            .children(task.id)?
            .iter()
            .any(|c| is_policy_approval(c) && !c.status.is_terminal()))
    }

    /// 子待ちが終わった task に policy の変更の承認の子があれば、`ReviewPass` にせず review をやり直す
    /// （`true`）。やり直した review が [`Self::policy_review_gate`] で人の判断を verdict にする。
    pub(crate) fn policy_rereview(&self, task: &Task) -> Result<bool, DispatchError> {
        Ok(self.store.children(task.id)?.iter().any(is_policy_approval))
    }
}

#[cfg(test)]
#[path = "policy_review/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "policy_review/unknown_tests.rs"]
mod unknown_tests;

#[cfg(test)]
#[path = "policy_review/no_reviewer_tests.rs"]
mod no_reviewer_tests;
