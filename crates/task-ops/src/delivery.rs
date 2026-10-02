//! ADR-0051: 既存レビュアーの対象SHAを固定し、同じrunに取り込み条件を追加する。
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use task_core::{
    Check, Criterion, Delivery, DeliverySkipReason, DeliveryState, Event, OrgNode, PlanOrigin,
    RunIndexRole, StoreError, Task, TaskId, TaskStore,
};

#[derive(Debug, Clone, Default)]
pub struct DeliveryPolicy {
    pub projects: Vec<String>,
    pub repo: PathBuf,
    pub default_departments: BTreeMap<String, String>,
}

/// ADR-0119 D1: only an observed owner can supply a department.
fn resolve_department(
    store: &dyn TaskStore,
    task: &Task,
    policy: &DeliveryPolicy,
    org: &[OrgNode],
) -> Result<Option<String>, StoreError> {
    let department = |id: &str| task_core::department_of(org, id);
    if let Some(found) = task.assignee.as_deref().and_then(department) {
        return Ok(Some(found));
    }

    let events = store.events_for(task.id)?;
    let routed = |run_id: &str, work_unit_id: Option<&str>| {
        events.iter().find_map(|(_, event)| match event {
            Event::RoutingDecided { run_id: id, record }
                if id == run_id && record.work_unit_id.as_deref() == work_unit_id =>
            {
                record.org_node.as_deref().and_then(department)
            }
            _ => None,
        })
    };
    if let Some(plan) = store.execution_plan_active(task.id)?
        && plan.origin == PlanOrigin::Planner
        && let Some(found) = plan
            .planner_run_id
            .as_deref()
            .and_then(|id| routed(id, None))
    {
        return Ok(Some(found));
    }

    let mut votes: BTreeMap<String, usize> = BTreeMap::new();
    for child in store.list(None)? {
        if child.id == task.id
            || child
                .tree
                .as_ref()
                .and_then(|t| t.parent_unit.as_ref())
                .is_none()
        {
            continue;
        }
        let mut ancestor = child.clone();
        let mut seen = std::collections::BTreeSet::<TaskId>::new();
        let mut belongs = false;
        for _ in 0..32 {
            let Some(parent) = ancestor.tree.as_ref().and_then(|t| t.parent_unit.as_ref()) else {
                break;
            };
            if !seen.insert(parent.task_id) {
                break;
            }
            if parent.task_id == task.id {
                belongs = true;
                break;
            }
            let Some(next) = store.get(parent.task_id)? else {
                break;
            };
            ancestor = next;
        }
        if belongs && let Some(found) = child.assignee.as_deref().and_then(department) {
            *votes.entry(found).or_default() += 1;
        }
    }
    for unit in store.work_units_for(task.id)? {
        for run in store.runs_for_work_unit(&unit.id)? {
            if run.task_id == task.id.to_string()
                && run.work_unit_id.as_deref() == Some(unit.id.as_str())
                && run.role == RunIndexRole::Worker
                && let Some(found) = routed(&run.run_id, Some(&unit.id))
            {
                *votes.entry(found).or_default() += 1;
            }
        }
    }
    if let Some((found, _)) = votes
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
    {
        return Ok(Some(found));
    }
    Ok(task
        .project_id
        .and_then(|id| policy.default_departments.get(&id.to_string()))
        .and_then(|id| department(id)))
}

/// ADR-0119 D2: `begin` が delivery を作らない理由。`Silent` は従来どおり何も残さない対象外。
enum Skip {
    Silent,
    Report {
        reason: DeliverySkipReason,
        detail: String,
        head: Option<String>,
    },
}

fn report(reason: DeliverySkipReason, detail: String) -> Skip {
    Skip::Report {
        reason,
        detail,
        head: None,
    }
}

/// delivery の材料（`begin` の判定を通ったもの）。
struct Prepared {
    project_id: task_core::ProjectId,
    row: task_core::ProjectRepo,
    path: PathBuf,
    branch: String,
    default_branch: String,
    base: String,
    head: String,
    department: String,
    old: Option<Delivery>,
}

fn prepare(
    store: &dyn TaskStore,
    task: &Task,
    workspace_root: &Path,
    policy: &DeliveryPolicy,
) -> Result<Result<Prepared, Skip>, StoreError> {
    let Some(project_id) = task.project_id else {
        return Ok(Err(Skip::Silent));
    };
    // ADR-0079 D6（Phase R1c）: 木の子 task は main に取り込まない（成果は親の段階末尾の統合で親の
    // ブランチに入る）。部署の取り込み判定・`deliveries` の行・release は root だけ。
    if !policy.projects.contains(&project_id.to_string())
        || task_core::support_kind(task).is_some()
        || task_core::tree::is_tree_child(task)
    {
        return Ok(Err(Skip::Silent));
    }
    if task.repos.len() != 1 {
        return Ok(Err(report(
            DeliverySkipReason::MultipleRepos,
            format!("task の repo が {} 件", task.repos.len()),
        )));
    }
    let task_dir = workspace_root.join(task.id.to_string());
    let Some(marker) = crate::workspace::read_marker(&task_dir) else {
        return Ok(Err(report(
            DeliverySkipReason::NoMarker,
            format!("{} が読めない", task_dir.join("worktree.json").display()),
        )));
    };
    if marker.repos.len() != 1 {
        return Ok(Err(report(
            DeliverySkipReason::MultipleRepos,
            format!("worktree.json の repo が {} 件", marker.repos.len()),
        )));
    }
    let mr = &marker.repos[0];
    let repo_id = task.repos[0].repo_id;
    let Some(row) = store
        .repo_list(project_id)?
        .into_iter()
        .find(|r| r.id == repo_id)
    else {
        return Ok(Err(report(
            DeliverySkipReason::RepoRowMissing,
            format!("repo {repo_id} が案件に無い"),
        )));
    };
    if mr.name != row.name {
        return Ok(Err(report(
            DeliverySkipReason::MarkerRepoMismatch,
            format!(
                "worktree.json の repo {} / task の repo {}",
                mr.name, row.name
            ),
        )));
    }
    let task_core::WorkspaceSpec::Local { path, .. } = &row.location else {
        return Ok(Err(report(
            DeliverySkipReason::RepoNotLocal,
            format!("repo {} はローカルではない", row.name),
        )));
    };
    let canonical = path.canonicalize().ok();
    if !path.is_dir() || canonical != policy.repo.canonicalize().ok() {
        return Ok(Err(report(
            DeliverySkipReason::RepoPathMismatch,
            format!(
                "repo {} の path {} / 取り込み先 {}",
                row.name,
                path.display(),
                policy.repo.display()
            ),
        )));
    }
    if Path::new(&mr.source).canonicalize().ok() != canonical {
        return Ok(Err(report(
            DeliverySkipReason::MarkerRepoMismatch,
            format!(
                "worktree.json の source {} / 登録 path {}",
                mr.source,
                path.display()
            ),
        )));
    }
    if mr.kind != "git" {
        return Ok(Err(report(
            DeliverySkipReason::NotGit,
            format!("worktree.json の kind {}", mr.kind),
        )));
    }
    let Some(branch) = mr.branch.clone() else {
        return Ok(Err(report(
            DeliverySkipReason::NoBranch,
            format!("repo {} の worktree にブランチが無い", row.name),
        )));
    };
    if !branch.ends_with(&task.id.to_string()) {
        return Ok(Err(report(
            DeliverySkipReason::BranchNameMismatch,
            format!("branch {branch} が {} で終わらない", task.id),
        )));
    }
    let default_branch = crate::changes::default_branch(path, row.default_branch.as_deref());
    let resolve = |r: &str| {
        crate::changes::git(path, &["rev-parse", "--verify", r], Duration::from_secs(10))
            .filter(|o| o.ok)
            .map(|o| o.stdout.trim().to_string())
    };
    let (base, head) = (
        resolve(&format!("refs/heads/{default_branch}")),
        resolve(&format!("refs/heads/{branch}")),
    );
    let (Some(base), Some(head)) = (base, head.clone()) else {
        return Ok(Err(Skip::Report {
            reason: DeliverySkipReason::RefsUnresolvable,
            detail: format!(
                "{}: refs/heads/{default_branch} または refs/heads/{branch} を解決できない",
                path.display()
            ),
            head,
        }));
    };
    let old = store.delivery_get(task.id)?;
    if old.as_ref().is_some_and(|d| {
        matches!(d.state, DeliveryState::Merging | DeliveryState::Preparing)
            || (d.state == DeliveryState::Ready && d.head == head)
    }) {
        return Ok(Err(Skip::Silent));
    }
    let org = store.org_list()?;
    let Some(department) = resolve_department(store, task, policy, &org)? else {
        return Ok(Err(Skip::Report {
            reason: DeliverySkipReason::DepartmentUnresolved,
            detail: format!(
                "assignee {}・計画の担当・子と WU の担当・案件の既定部署のどれからも部が決まらない",
                task.assignee.as_deref().unwrap_or("なし")
            ),
            head: Some(head),
        }));
    };
    Ok(Ok(Prepared {
        project_id,
        path: path.clone(),
        row,
        branch,
        default_branch,
        base,
        head,
        department,
        old,
    }))
}

/// ADR-0119 D3: `DeliverySkipped` を (task, reason, head) ごとに高々 1 件積む。積んだら `true`。
fn record_skip(
    store: &dyn TaskStore,
    task: &Task,
    reason: DeliverySkipReason,
    detail: String,
    head: Option<String>,
) -> Result<bool, StoreError> {
    let seen = store.events_for(task.id)?.into_iter().any(|(_, e)| {
        matches!(e, Event::DeliverySkipped { reason: r, head: h, .. } if r == reason && h == head)
    });
    if seen {
        return Ok(false);
    }
    store.append_event(
        task.id,
        &Event::DeliverySkipped {
            reason,
            detail,
            head,
        },
    )?;
    Ok(true)
}

pub fn begin(
    store: &dyn TaskStore,
    task: &mut Task,
    workspace_root: &Path,
    policy: &DeliveryPolicy,
    worker_run: &str,
    review_run: &str,
) -> Result<Option<Delivery>, StoreError> {
    let Prepared {
        project_id,
        row,
        path,
        branch,
        default_branch,
        base,
        head,
        department,
        old,
    } = match prepare(store, task, workspace_root, policy)? {
        Ok(p) => p,
        Err(Skip::Silent) => return Ok(None),
        Err(Skip::Report {
            reason,
            detail,
            head,
        }) => {
            record_skip(store, task, reason, detail, head)?;
            return Ok(None);
        }
    };
    let criterion_idx = task.acceptance.len();
    task.acceptance.push(Criterion {check:Check::Reviewer,text:format!("【部署内のマージ可否判定】リポジトリ {} の {} ({}) に、コミット {} (ブランチ {}) を取り込めること。指定SHAの実際の差分、依頼範囲、テスト、移行・設定の互換性、秘密の混入を確認し、対象の実装worktreeに未コミット変更や未解決の欠陥があれば不合格にする。登録元リポジトリにはユーザーの未コミット変更があり得る。対象差分と重ならずfast-forwardで保持できる既存変更は拒否理由にせず、復元・コミット・stashしない。対象差分と衝突する変更は停止理由にする。レビューは部署をまとめる担当 {} がこの独立runで行い、上司やCoSで重複レビューしない。合格すると制御プレーンがマージ・release・verifyまで実行し、最終デプロイだけ人が押す。要件や権限などユーザーに確認しないと判断できない場合は、理由の先頭に [needs-human] と具体的な質問を含める。技術的な不備だけなら部署内で差し戻す。自分でmergeやdeploy、実装変更はしない。",path.display(),default_branch,base,head,branch,department) });
    let d = Delivery {
        task_id: task.id,
        project_id,
        repo_id: row.id,
        repo: row.name,
        branch,
        base,
        head,
        default_branch,
        department,
        review_run: review_run.into(),
        worker_run: worker_run.into(),
        criterion_idx,
        decision: None,
        state: DeliveryState::Reviewing,
        detail: "部署のレビュアーが実装とマージ可否を確認中です".into(),
        release: None,
        prepare_pid: None,
        notification: None,
        pushed_at: None,
        push_error: None,
    };
    if !store.delivery_save(old.as_ref(), &d)? {
        return Err(StoreError::Invalid(
            "delivery review changed concurrently".into(),
        ));
    }
    Ok(Some(d))
}

#[cfg(test)]
mod tests;
