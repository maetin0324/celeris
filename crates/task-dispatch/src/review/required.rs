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
    /// ADR 2026-11-08-health-enforcement-loop D1.2: the repo health item (`None` adds no item).
    pub health: Option<RepoHealth>,
}

impl From<&crate::dispatcher::review_spawn::ReviewSync> for RequiredCheckBinding {
    /// The candidate of the pre-review sync (ADR-0118): base = `target_sha`, head = `reviewed_sha`.
    fn from(sync: &crate::dispatcher::review_spawn::ReviewSync) -> Self {
        RequiredCheckBinding {
            repo_id: sync.repo_id,
            target_sha: sync.target_sha.clone(),
            reviewed_sha: sync.reviewed_sha.clone(),
        }
    }
}

/// Prefix of the reason of a health item that is Error or Unavailable (never a pass, never repaired. D3.4).
pub const HEALTH_CHECK_ERROR: &str = "health_check_error";

/// D1.2: the health item of one repository. `spawn_review` sets where to evaluate; `HealthInputJob::attach`
/// (`health_review::review_item::evaluate_all`) evaluates it through the shared entry before the review runs.
#[derive(Debug)]
pub enum RepoHealth {
    /// Not evaluated yet: the worktree to evaluate (the synced worktree when there is a candidate).
    At(std::path::PathBuf),
    Evaluated(HealthItem),
}

/// The evaluated health of one repository.
#[derive(Debug, Default)]
pub struct HealthItem {
    /// The item added to the required checks (`None` for NotApplicable and for a repo without a candidate).
    pub result: Option<task_core::PhaseCheckResult>,
    /// The checker put in the definition digest (only when the applicability is Applies. D2.4).
    pub checker: Option<task_core::required_checks::health::CheckerId>,
    /// Repair material of a Fail (`Verdict.repair_hint`. D3.2).
    pub repair_hint: Option<task_core::ReviewRepairHint>,
    /// Why no item was added (`HealthApplicability::reason()` of NotApplicable, or no candidate).
    pub note: Option<String>,
}

impl HealthItem {
    /// The required checks of `health` (fail closed: a site that was never evaluated is not a pass).
    fn of(health: Option<RepoHealth>) -> Self {
        match health {
            None => HealthItem::default(),
            Some(RepoHealth::Evaluated(item)) => item,
            Some(RepoHealth::At(dir)) => HealthItem {
                result: Some(task_core::PhaseCheckResult::new(
                    task_core::required_checks::health::HEALTH_CHECK_ID.into(),
                    false,
                    format!("{HEALTH_CHECK_ERROR}: not evaluated ({})", dir.display()),
                )),
                ..HealthItem::default()
            },
        }
    }
}

impl Verdict {
    /// D3.2: the check `try_review_repair` gives `classify_review_failure` for a verdict outside the acceptance. A
    /// health Fail (it carries the [`HealthItem::repair_hint`]) is `Check::Command { cmd: HEALTH_CHECK_ID }`; other
    /// implicit conditions (plan, aggregate, required checks, research, health Error) are `Check::Human` (no repair).
    pub fn implicit_check(&self) -> task_core::Check {
        let id = task_core::required_checks::health::HEALTH_CHECK_ID;
        match &self.repair_hint {
            Some(hint) if hint.scope == id => task_core::Check::Command {
                cmd: id.to_string(),
                expect_exit: 0,
            },
            _ => task_core::Check::Human,
        }
    }
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
        let mut health = HealthItem::of(checks.health);
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
