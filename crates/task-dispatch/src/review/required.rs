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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredRepoResult {
    pub binding: Option<RequiredCheckBinding>,
    pub definition_sha256: String,
    pub results: Vec<task_core::PhaseCheckResult>,
}

pub(super) async fn evaluate(
    repo_checks: Vec<RequiredRepoChecks>,
    workspace: &dyn Workspace,
    command_timeout: Duration,
) -> (Vec<RequiredRepoResult>, HashMap<String, (bool, String)>) {
    // Execute each repository's immutable definition in its own workspace. Only the
    // acceptance workspace shares command results with acceptance conditions.
    let mut required_checks = Vec::new();
    let mut acceptance_passes = HashMap::new();
    for checks in repo_checks {
        let digest = checks
            .commands
            .as_ref()
            .map(|cmds| task_core::required_checks::definition_digest(cmds))
            .unwrap_or_default();
        let mut results = Vec::new();
        let mut binding = checks.binding;
        match checks.commands {
            Err(reason) => {
                binding = None;
                results.push(task_core::PhaseCheckResult {
                    cmd: "workspace.toml definition".into(),
                    pass: false,
                    summary: reason,
                });
            }
            Ok(commands) => {
                let check_workspace = checks.workspace.as_deref().unwrap_or(workspace);
                let mut completed = HashMap::<String, (bool, String)>::new();
                for cmd in commands {
                    let (pass, reason) = if let Some(result) = completed.get(&cmd) {
                        result.clone()
                    } else if checks.reusable.contains(&cmd) {
                        (true, "reused".into())
                    } else {
                        exec_check_with_repair_retries(
                            check_workspace,
                            &cmd,
                            0,
                            command_timeout,
                            "workspace.toml check: ",
                        )
                        .await
                    };
                    completed.insert(cmd.clone(), (pass, reason.clone()));
                    if checks.workspace.is_none() {
                        acceptance_passes.insert(cmd.clone(), (pass, reason.clone()));
                    }
                    results.push(task_core::PhaseCheckResult {
                        cmd,
                        pass,
                        summary: tail(&reason, REASON_TAIL).to_string(),
                    });
                }
            }
        }
        required_checks.push(RequiredRepoResult {
            binding,
            definition_sha256: digest,
            results,
        });
    }
    (required_checks, acceptance_passes)
}
