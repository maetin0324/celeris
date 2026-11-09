//! Required checks are evaluated per repository, with reuse scoped to the definition and snapshot.
use super::*;

/// Existing review snapshot, together with the checks evaluated in that repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredCheckBinding {
    pub repo_id: task_core::RepoId,
    pub target_sha: String,
    pub reviewed_sha: String,
}

pub struct RequiredRepoChecks {
    pub binding: Option<RequiredCheckBinding>,
    pub commands: Result<Vec<String>, String>,
    pub reusable: std::collections::BTreeSet<String>,
    /// None uses the acceptance workspace; other repositories supply their own workspace.
    pub workspace: Option<Box<dyn Workspace>>,
    /// ADR 2026-11-08-health-enforcement-loop D1.2: where the repo health item is evaluated (`None` adds no item).
    pub health: Option<crate::health_review::review_item::RequiredHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredRepoResult {
    pub binding: Option<RequiredCheckBinding>,
    pub definition_sha256: String,
    pub results: Vec<task_core::PhaseCheckResult>,
    /// Repair material of a failing health item (`Verdict.repair_hint`, D3.2).
    pub health_repair_hint: Option<task_core::ReviewRepairHint>,
    /// Why no health item was added (NotApplicable, or no candidate to bind).
    pub health_note: Option<String>,
}

impl RequiredRepoResult {
    /// `Verdict.repair_hint` of `result` (only the failing health item carries one).
    pub fn repair_hint(
        &self,
        result: &task_core::PhaseCheckResult,
    ) -> Option<task_core::ReviewRepairHint> {
        (result.cmd == task_core::required_checks::health::HEALTH_CHECK_ID)
            .then(|| self.health_repair_hint.clone())
            .flatten()
    }
}

pub(super) async fn evaluate(
    repo_checks: Vec<RequiredRepoChecks>,
    workspace: &dyn Workspace,
    command_timeout: Duration,
) -> (Vec<RequiredRepoResult>, HashMap<String, CheckOutcome>) {
    // Execute each repository's immutable definition in its own workspace. Only the
    // acceptance workspace shares command results with acceptance conditions.
    let mut required_checks = Vec::new();
    let mut acceptance_passes = HashMap::new();
    for checks in repo_checks {
        let mut digest = String::new();
        let mut health = crate::health_review::review_item::HealthItem::default();
        let mut results = Vec::new();
        let mut binding = checks.binding;
        match checks.commands {
            Err(reason) => {
                binding = None;
                results.push(task_core::PhaseCheckResult::new(
                    "workspace.toml definition".into(),
                    false,
                    reason,
                ));
            }
            Ok(commands) => {
                // D1.2・D2.4: the health item joins the required checks; Applies puts the checker in the digest.
                if let Some(site) = checks.health {
                    health =
                        crate::health_review::review_item::evaluate(site, binding.as_ref()).await;
                }
                digest = task_core::required_checks::definition_digest(
                    &commands,
                    health.checker.as_ref(),
                );
                let check_workspace = checks.workspace.as_deref().unwrap_or(workspace);
                let mut completed = HashMap::<String, CheckOutcome>::new();
                for cmd in commands {
                    let outcome = if let Some(result) = completed.get(&cmd) {
                        result.clone()
                    } else if checks.reusable.contains(&cmd) {
                        CheckOutcome {
                            pass: true,
                            reason: "reused".into(),
                            exit: None,
                            timed_out: false,
                        }
                    } else {
                        exec_check_outcome(
                            check_workspace,
                            &cmd,
                            0,
                            command_timeout,
                            "workspace.toml check: ",
                        )
                        .await
                    };
                    completed.insert(cmd.clone(), outcome.clone());
                    if checks.workspace.is_none() {
                        acceptance_passes.insert(cmd.clone(), outcome.clone());
                    }
                    results.push(task_core::PhaseCheckResult::new(
                        cmd,
                        outcome.pass,
                        tail(&outcome.reason, REASON_TAIL).to_string(),
                    ));
                }
                results.extend(health.result.take());
            }
        }
        if let Some(note) = &health.note {
            tracing::info!(note = %note, "repo health item not added to the required checks");
        }
        required_checks.push(RequiredRepoResult {
            binding,
            definition_sha256: digest,
            results,
            health_repair_hint: health.repair_hint,
            health_note: health.note,
        });
    }
    (required_checks, acceptance_passes)
}
