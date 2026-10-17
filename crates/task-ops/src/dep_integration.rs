//! ADR 2026-10-17-root-dependency-waits-for-integration: root 同士の `depends_on` は、依存先の成果が
//! repo の既定 branch に入るまで待つ。git の祖先判定だけ（LLM・書き込みなし）。

use std::collections::BTreeSet;
use std::path::PathBuf;

use task_core::{
    DeliverySkipReason, Event, IntegrationWaitCause, ProjectRepo, Status, StoreError, Task, TaskId,
    TaskStore, TaskWait, WorkspaceSpec, is_conversation, support_kind, tree::is_tree_child,
};

/// `task` が待つべき依存先の取り込み（無ければ `None`）。`integrated` は祖先と分かった依存先の記憶
/// （呼び出し側が持つ。既定 branch は前にしか進まない）。依存先は task_id 順に見て最初の 1 件を返す。
pub fn integration_wait(
    store: &dyn TaskStore,
    task: &Task,
    integrated: &mut BTreeSet<TaskId>,
) -> Result<Option<TaskWait>, StoreError> {
    if is_conversation(task) || is_tree_child(task) {
        return Ok(None);
    }
    let mut deps = task.depends_on.clone();
    deps.sort();
    deps.dedup();
    for dep_id in deps {
        if integrated.contains(&dep_id) {
            continue;
        }
        let Some(dep) = store.get(dep_id)? else {
            continue;
        };
        if dep.status != Status::Done || is_tree_child(&dep) || support_kind(&dep).is_some() {
            continue;
        }
        match dependency_state(store, &dep)? {
            None => {
                integrated.insert(dep_id);
            }
            Some((cause, detail)) => {
                return Ok(Some(TaskWait::DependsOnIntegration {
                    task_id: dep_id,
                    cause,
                    detail,
                }));
            }
        }
    }
    Ok(None)
}

/// `None` = 取り込み済み（または取り込む物が無い）。`Some` = まだ入っていない理由。
fn dependency_state(
    store: &dyn TaskStore,
    dep: &Task,
) -> Result<Option<(IntegrationWaitCause, String)>, StoreError> {
    let branch = format!("celeris/{}", dep.id);
    let mut pending: Option<(PathBuf, String, String)> = None;
    for (repo_id, path, configured) in local_repos(store, dep)? {
        let Some(head) = git_out(
            &path,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/heads/{branch}"),
            ],
        ) else {
            continue;
        };
        let default_branch =
            match crate::changes::resolve_default_branch(&path, configured.as_deref()) {
                Ok(b) => b,
                Err(e) => {
                    return Ok(Some((
                        IntegrationWaitCause::Unmerged,
                        format!("repo {repo_id}: {e}"),
                    )));
                }
            };
        if !crate::stranded::ancestor(&path, &head, &default_branch) {
            pending = Some((path, head, default_branch));
            break;
        }
    }
    let Some((_, head, default_branch)) = pending else {
        return Ok(None);
    };
    let integration = store
        .task_integration_list(None)?
        .into_iter()
        .find(|s| s.task_id == dep.id && s.head_sha == head);
    if let Some(s) = integration {
        use task_core::TaskRepoIntegrationStatus as S;
        let cause = match s.state {
            S::Held => Some(IntegrationWaitCause::Held),
            S::Discarded => Some(IntegrationWaitCause::Discarded),
            _ => None,
        };
        if let Some(cause) = cause {
            return Ok(Some((cause, s.reason.unwrap_or_default())));
        }
    }
    let skipped = store
        .events_for(dep.id)?
        .into_iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::DeliverySkipped {
                reason,
                detail,
                head: h,
            } if reason != DeliverySkipReason::NoCommits
                && h.as_ref().is_none_or(|h| *h == head) =>
            {
                Some((reason, detail))
            }
            _ => None,
        });
    let delivered = store.delivery_get(dep.id)?.is_some_and(|d| d.head == head);
    Ok(Some(match skipped {
        Some((reason, detail)) if !delivered => (
            IntegrationWaitCause::DeliverySkipped,
            format!("{}: {detail}", reason.code()),
        ),
        _ => (
            IntegrationWaitCause::Unmerged,
            format!("{branch} は {default_branch} に未取り込み"),
        ),
    }))
}

/// 依存先の Local repo（`repos` の参照、無ければ案件の登録 repo）。Remote は取り込み先が違うので対象外。
fn local_repos(
    store: &dyn TaskStore,
    dep: &Task,
) -> Result<Vec<(task_core::RepoId, PathBuf, Option<String>)>, StoreError> {
    let mut rows: Vec<ProjectRepo> = Vec::new();
    for reference in &dep.repos {
        rows.extend(store.repo_get(reference.repo_id)?);
    }
    if rows.is_empty()
        && let Some(project) = dep.project_id
    {
        rows = store.repo_list(project)?;
    }
    Ok(rows
        .into_iter()
        .filter_map(|r| match r.location {
            WorkspaceSpec::Local { path, .. } => Some((r.id, path, r.default_branch)),
            WorkspaceSpec::Remote { .. } => None,
        })
        .collect())
}

fn git_out(repo: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}
