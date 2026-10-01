//! ADR-0051: 既存レビュアーの対象SHAを固定し、同じrunに取り込み条件を追加する。
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use task_core::{
    Check, Criterion, Delivery, DeliveryState, Event, OrgNode, PlanOrigin, RunIndexRole,
    StoreError, Task, TaskId, TaskStore,
};

#[derive(Debug, Clone, Default)]
pub struct DeliveryPolicy {
    pub projects: Vec<String>,
    pub repo: PathBuf,
    pub default_departments: BTreeMap<String, String>,
}

/// ADR-0099 D1: only an observed owner can supply a department.
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

pub fn begin(
    store: &dyn TaskStore,
    task: &mut Task,
    workspace_root: &Path,
    policy: &DeliveryPolicy,
    worker_run: &str,
    review_run: &str,
) -> Result<Option<Delivery>, StoreError> {
    let Some(project_id) = task.project_id else {
        return Ok(None);
    };
    // ADR-0079 D6（Phase R1c）: 木の子 task は main に取り込まない（成果は親の段階末尾の統合で親の
    // ブランチに入る）。部署の取り込み判定・`deliveries` の行・release は root だけ。
    if !policy.projects.contains(&project_id.to_string())
        || task_core::support_kind(task).is_some()
        || task_core::tree::is_tree_child(task)
        || task.repos.len() != 1
    {
        return Ok(None);
    }
    let org = store.org_list()?;
    let Some(department) = resolve_department(store, task, policy, &org)? else {
        return Ok(None);
    };
    let Some(marker) = crate::workspace::read_marker(&workspace_root.join(task.id.to_string()))
    else {
        return Ok(None);
    };
    if marker.repos.len() != 1 {
        return Ok(None);
    }
    let mr = &marker.repos[0];
    let Some(row) = store
        .repo_list(project_id)?
        .into_iter()
        .find(|r| r.id == task.repos[0].repo_id)
    else {
        return Ok(None);
    };
    let task_core::WorkspaceSpec::Local { path, .. } = &row.location else {
        return Ok(None);
    };
    if !path.is_dir()
        || path.canonicalize().ok() != policy.repo.canonicalize().ok()
        || Path::new(&mr.source).canonicalize().ok() != path.canonicalize().ok()
        || mr.kind != "git"
    {
        return Ok(None);
    }
    let Some(branch) = &mr.branch else {
        return Ok(None);
    };
    if !branch.ends_with(&task.id.to_string()) {
        return Ok(None);
    }
    let default_branch = crate::changes::default_branch(path, row.default_branch.as_deref());
    let resolve = |r: &str| {
        crate::changes::git(path, &["rev-parse", "--verify", r], Duration::from_secs(10))
            .filter(|o| o.ok)
            .map(|o| o.stdout.trim().to_string())
    };
    let (Some(base), Some(head)) = (
        resolve(&format!("refs/heads/{default_branch}")),
        resolve(&format!("refs/heads/{branch}")),
    ) else {
        return Ok(None);
    };
    let old = store.delivery_get(task.id)?;
    if old.as_ref().is_some_and(|d| {
        matches!(d.state, DeliveryState::Merging | DeliveryState::Preparing)
            || (d.state == DeliveryState::Ready && d.head == head)
    }) {
        return Ok(None);
    }
    let criterion_idx = task.acceptance.len();
    task.acceptance.push(Criterion {check:Check::Reviewer,text:format!("【部署内のマージ可否判定】リポジトリ {} の {} ({}) に、コミット {} (ブランチ {}) を取り込めること。指定SHAの実際の差分、依頼範囲、テスト、移行・設定の互換性、秘密の混入を確認し、対象の実装worktreeに未コミット変更や未解決の欠陥があれば不合格にする。登録元リポジトリにはユーザーの未コミット変更があり得る。対象差分と重ならずfast-forwardで保持できる既存変更は拒否理由にせず、復元・コミット・stashしない。対象差分と衝突する変更は停止理由にする。レビューは部署をまとめる担当 {} がこの独立runで行い、上司やCoSで重複レビューしない。合格すると制御プレーンがマージ・release・verifyまで実行し、最終デプロイだけ人が押す。要件や権限などユーザーに確認しないと判断できない場合は、理由の先頭に [needs-human] と具体的な質問を含める。技術的な不備だけなら部署内で差し戻す。自分でmergeやdeploy、実装変更はしない。",path.display(),default_branch,base,head,branch,department) });
    let d = Delivery {
        task_id: task.id,
        project_id,
        repo_id: row.id,
        repo: row.name,
        branch: branch.clone(),
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
