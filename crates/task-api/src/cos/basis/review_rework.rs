//! ADR 2026-10-29-cos-review-rework-reopen: the routine category (e) `review_rework`.
//!
//! CoS judges whether a reviewer's rejection is a technical defect (the fix is written in the
//! rejection, objective and acceptance stay). The API checks what the store can decide: the task
//! failed on a reviewer verdict of its last attempt, the basis names that reviewer run, the task has
//! no human check, CoS has not reopened it on this basis [`REVIEW_REWORK_MAX`] times, and the
//! rejection is not the same as one CoS already reopened for.

use std::collections::BTreeSet;

use task_core::store::SqliteStore;
use task_core::{Check, Event, TaskStore};
use task_ops::human_inbox::InboxKind;

use super::{
    BASIS_INVALID, BASIS_REPEATED_FAILURE, BASIS_REWORK_LIMIT, BasisCategory, BasisRefKind,
    BasisTarget, DelegatedBasis, normalized, problem, reason_prefix,
};
use crate::problem::{ApiProblem, store_problem};

/// The option key of a failed task's inbox item that reopens the same task.
pub(crate) const REOPEN_OPTION: &str = "reopen";
/// D2: how many times CoS may reopen the same task on `review_rework`.
pub(crate) const REVIEW_REWORK_MAX: usize = 2;

/// The failing reviewer verdicts of one attempt.
#[derive(Debug, PartialEq, Eq)]
struct Rejection {
    /// Reviewer run ids (`reviewer(<id>): …`).
    reviewers: BTreeSet<String>,
    /// The verdict texts without the reviewer prefix, whitespace collapsed.
    texts: BTreeSet<String>,
}

/// Split `reviewer(<id>): <text>` (the reviewer verdict form of `task-dispatch` review.rs).
fn reviewer_verdict(reason: &str) -> Option<(&str, &str)> {
    let rest = reason.strip_prefix("reviewer(")?;
    let (id, text) = rest.split_once("): ")?;
    (!id.is_empty()).then_some((id, text))
}

/// The reviewer rejection of the last worker run in `events`. With `fresh`, a reopen after the
/// verdicts means they belong to an older attempt (`None`).
fn rejection(events: &[(u64, Event)], fresh: bool) -> Option<Rejection> {
    let run = task_ops::derive::last_run_id(events)?;
    let mut out = Rejection {
        reviewers: BTreeSet::new(),
        texts: BTreeSet::new(),
    };
    let mut last_verdict = None;
    for (i, (_, e)) in events.iter().enumerate() {
        if let Event::ReviewVerdict {
            run_id,
            pass: false,
            reason,
            ..
        } = e
            && *run_id == run
            && let Some((reviewer, text)) = reviewer_verdict(reason)
        {
            out.reviewers.insert(reviewer.to_owned());
            out.texts.insert(normalized(text));
            last_verdict = Some(i);
        }
    }
    let last_verdict = last_verdict?;
    let reopened_after = events[last_verdict..]
        .iter()
        .any(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "reopen"));
    (!fresh || !reopened_after).then_some(out)
}

pub(super) fn validate(
    store: &SqliteStore,
    basis: &DelegatedBasis,
    target: &BasisTarget,
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| problem(BASIS_INVALID, detail);
    if target.kind != Some(InboxKind::Failed) || target.option.as_deref() != Some(REOPEN_OPTION) {
        return Err(invalid(format!(
            "review_rework answers {REOPEN_OPTION} to a failed task"
        )));
    }
    let Some(task_id) = target.task_id else {
        return Err(invalid("review_rework needs the failed task".into()));
    };
    let Some(task) = store.get(task_id).map_err(store_problem)? else {
        return Err(invalid(format!("task {task_id} does not exist")));
    };
    // ADR D3: a human check is the person's judgment of the result; CoS does not start another
    // attempt over it.
    if task
        .acceptance
        .iter()
        .any(|c| matches!(c.check, Check::Human))
    {
        return Err(crate::cos::inbox::human_required_problem(
            "the task has a human check",
        ));
    }
    let events = store.events_for(task_id).map_err(store_problem)?;
    let Some(current) = rejection(&events, true) else {
        return Err(invalid(format!(
            "task {task_id} did not fail on a reviewer verdict of its last attempt; escalate it"
        )));
    };
    let cited: Vec<&str> = basis
        .refs
        .iter()
        .filter(|r| r.kind == BasisRefKind::Review)
        .map(|r| r.id.trim())
        .collect();
    if cited.is_empty() || cited.iter().any(|id| !current.reviewers.contains(*id)) {
        return Err(invalid(format!(
            "review_rework needs review refs naming the rejecting reviewer run ({})",
            current
                .reviewers
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    // The rejections CoS already reopened for, read at each applied reopen of this category.
    let prefix = reason_prefix(&DelegatedBasis {
        category: BasisCategory::ReviewRework,
        refs: Vec::new(),
        frame: None,
        failures: None,
    });
    let previous: Vec<Rejection> = events
        .iter()
        .enumerate()
        .filter(|(_, (_, e))| {
            matches!(e, Event::CosOperation { state, target_kind, reason, .. }
                if state == "applied" && target_kind == "task" && reason.starts_with(&prefix))
        })
        .filter_map(|(i, _)| rejection(&events[..i], false))
        .collect();
    if previous.len() >= REVIEW_REWORK_MAX {
        return Err(problem(
            BASIS_REWORK_LIMIT,
            format!(
                "CoS already reopened task {task_id} {} times on review_rework (limit \
                 {REVIEW_REWORK_MAX}); escalate it",
                previous.len()
            ),
        ));
    }
    if previous.iter().any(|p| p.texts == current.texts) {
        return Err(problem(
            BASIS_REPEATED_FAILURE,
            "the reviewer repeats a rejection CoS already reopened for; escalate it",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{RunRole, Status};

    fn started(run: &str, role: Option<RunRole>) -> Event {
        Event::WorkerStarted {
            run_id: run.into(),
            adapter: "fake".into(),
            model: "m".into(),
            provider: None,
            account: None,
            role,
            task_role: None,
        }
    }

    fn verdict(run: &str, pass: bool, reason: &str) -> Event {
        Event::ReviewVerdict {
            run_id: run.into(),
            criterion_idx: 0,
            pass,
            reason: reason.into(),
        }
    }

    fn seq(events: Vec<Event>) -> Vec<(u64, Event)> {
        events
            .into_iter()
            .enumerate()
            .map(|(i, e)| (i as u64, e))
            .collect()
    }

    #[test]
    fn rejection_reads_the_failing_reviewer_verdicts_of_the_last_run() {
        let events = seq(vec![
            started("run-1", None),
            verdict("run-1", false, "reviewer(rev-1): old"),
            started("run-2", None),
            started("rev-2", Some(RunRole::Reviewer)),
            verdict("run-2", true, "reviewer(rev-2): ok"),
            verdict("run-2", false, "reviewer(rev-2):  index.json   が混入"),
            verdict("run-2", false, "command `cargo test` exit 1"),
        ]);
        let got = rejection(&events, true).expect("rejection");
        assert_eq!(got.reviewers, BTreeSet::from(["rev-2".to_string()]));
        assert_eq!(got.texts, BTreeSet::from(["index.json が混入".to_string()]));
    }

    #[test]
    fn rejection_is_none_without_a_reviewer_verdict_or_after_a_reopen() {
        let command_only = seq(vec![
            started("run-1", None),
            verdict("run-1", false, "command `cargo test` exit 1"),
            verdict("run-1", false, "reviewer infra failure ×3: x"),
        ]);
        assert!(rejection(&command_only, true).is_none());
        let reopened = seq(vec![
            started("run-1", None),
            verdict("run-1", false, "reviewer(rev-1): fix it"),
            Event::Transitioned {
                from: Status::Failed,
                to: Status::Ready,
                reason: "reopen".into(),
            },
        ]);
        assert!(rejection(&reopened, true).is_none());
        assert!(rejection(&reopened, false).is_some());
    }
}
