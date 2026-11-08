//! ADR 2026-11-07-health-review-integration D6.4・I8: policy の file を変える candidate は、reviewer が承認
//! （`HealthVerdictOut.policy.approved = true`）した後も人の承認なしには合格しない。
//!
//! `dispatcher::child_tasks` の子 module として `#[path]` で読む（`child_tasks.rs` の `mod policy_review`）。file を
//! `health_review/` に置くのは、`dispatcher/` の module が repo health の `size.responsibility-growth` の閾値を
//! 超えているため。待ちは `Approval` の子 task（SQLite）だけで表し、dispatcher のメモリに持たない（I9 と同じ）。
//!
//! 流れ（`Check::Human` の既存の経路を使う）:
//! - [`Dispatcher::policy_review_gate`]（`on_review_finished` が合否を数える前）: reviewer が判定した candidate の
//!   `HealthReviewInputBuilt` が `policy.changed` で、ほかの判定が全部合格なら、candidate ごとの `Approval` の子を
//!   探す。無ければ作る（`ApprovalRequested` → 受信箱。本文に policy の file と diff の要約）。子が終端でない間は
//!   既存の「委譲した子の待ち」で reviewing のまま待つ。人の承認（done）は合格の verdict、却下（failed・
//!   cancelled）は substantive の不合格（→ replan）を足す。
//! - [`Dispatcher::policy_approval_pending`]（`resolve_human_approvals` の先頭）: 承認の子が終端でない間は review を
//!   始めない（daemon の再起動の後も待つ）。
//! - [`Dispatcher::policy_rereview`]（`settle_awaiting_children`）: 承認の子が終端になったら直接 `ReviewPass` に
//!   せず、review をやり直して上の判定に通す（reviewer の合格は ADR 2026-10-27 D2 で再利用される）。
//!
//! 判定に使う policy の状態は `HealthReviewInputBuilt`（base 側の検査器で作った材料。I3）だけから読む。LLM は呼ばない。

use super::*;
use crate::review::RequiredCheckBinding;
use task_worker::protocol::HealthRepoInput;

/// policy の変更の承認を待つ `Approval` の子の title の接頭辞（`derive::human_approval_title` と重ならない）。
pub(crate) const POLICY_APPROVAL_TITLE: &str = "Policy change approval: ";

/// 人の却下の不合格に付ける repair の class（scope は substantive。局所修復にしない）。
const POLICY_REJECTED_CLASS: &str = "policy_rejected";

/// reviewer が判定した 1 repo の policy の変更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyChange {
    /// reviewer が見た candidate の SHA（承認は candidate ごと）。
    pub reviewed_sha: String,
    /// その candidate の health の材料（base 側の検査器で作ったもの）。
    pub input: HealthRepoInput,
}

/// この review で reviewer が判定した candidate のうち、policy の file を変えたもの。`reviewer_ran` が false の
/// review（reviewer の合格を再利用した）は、`ReviewerPassReused` の元の candidate を見る。
pub(crate) fn policy_changes<'a>(
    events: &[(u64, Event)],
    reviewer_ran: bool,
    bindings: impl IntoIterator<Item = &'a RequiredCheckBinding>,
) -> Vec<PolicyChange> {
    let mut out = Vec::new();
    for b in bindings {
        let judged = if reviewer_ran {
            Some((b.target_sha.clone(), b.reviewed_sha.clone()))
        } else {
            events.iter().rev().find_map(|(_, e)| match e {
                Event::ReviewerPassReused {
                    repo_id,
                    target_sha,
                    reviewed_sha,
                    from_target_sha,
                    from_reviewed_sha,
                    ..
                } if *repo_id == b.repo_id
                    && *target_sha == b.target_sha
                    && *reviewed_sha == b.reviewed_sha =>
                {
                    Some((from_target_sha.clone(), from_reviewed_sha.clone()))
                }
                _ => None,
            })
        };
        let Some((target, reviewed)) = judged else {
            continue;
        };
        let repo = b.repo_id.to_string();
        let input = events.iter().rev().find_map(|(_, e)| match e {
            Event::HealthReviewInputBuilt {
                repo_id,
                target_sha,
                reviewed_sha,
                input,
                ..
            } if *repo_id == repo && *target_sha == target && *reviewed_sha == reviewed => {
                serde_json::from_value::<HealthRepoInput>(input.clone()).ok()
            }
            _ => None,
        });
        if let Some(input) = input.filter(|i| i.policy.changed) {
            out.push(PolicyChange {
                reviewed_sha: reviewed,
                input,
            });
        }
    }
    out
}

/// candidate ごとの承認の子の title（照合のキー）。
pub(crate) fn approval_title(change: &PolicyChange) -> String {
    format!(
        "{POLICY_APPROVAL_TITLE}{} @ {}",
        change.input.repo_id,
        &change.reviewed_sha[..change.reviewed_sha.len().min(12)]
    )
}

/// 受信箱に出す本文: policy の file・diff の要約・reviewer の承認の理由。
pub(crate) fn approval_objective(change: &PolicyChange, reviewer_reason: Option<&str>) -> String {
    let i = &change.input;
    let mut lines = vec![
        "The reviewer approved a change to the repo-health policy files. It passes only with a human approval (ADR 2026-11-07-health-review-integration D6.4). Reject to send it back for a replan.".to_string(),
        format!(
            "repo {}: base {} → candidate {}",
            i.repo_id, i.target_sha, i.reviewed_sha
        ),
        format!("policy files: {}", i.policy.files.join(", ")),
        format!(
            "diff: {} files, +{} -{}",
            i.diff.files, i.diff.insertions, i.diff.deletions
        ),
    ];
    lines.extend(
        i.diff
            .paths
            .iter()
            .map(|p| format!("- {} (+{} -{})", p.path, p.insertions, p.deletions)),
    );
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

fn is_policy_approval(task: &Task) -> bool {
    task.kind == TaskKind::Approval && task.title.starts_with(POLICY_APPROVAL_TITLE)
}

impl Dispatcher {
    /// `on_review_finished` が `all_pass` を数える前に呼ぶ。policy の変更が無い review・不合格を含む review では
    /// 何もしない（不合格は従来どおり repair/replan）。
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
        let criterion = task
            .acceptance
            .iter()
            .position(|c| matches!(c.check, Check::Reviewer));
        let (Some(criterion), false) = (criterion, bindings.is_empty()) else {
            return Ok(());
        };
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
            let files = change.input.policy.files.join(", ");
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
