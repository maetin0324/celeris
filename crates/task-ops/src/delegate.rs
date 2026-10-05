//! 実行中の委譲の検証（ADR-0016 D2 / M6 / M7）。ストアを見る検証（既存 ID の依存、祖先、木の深さ・run 数）は
//! ここに置く。ストアを見ない検証（空欄・配列内インデックスの範囲・自己参照・閉路）は `task_core::delegate`
//! （`validate_each`）にある。I/O は `TaskStore` の読み取りだけ。LLM 呼び出し・挿入は無い
//! （挿入は `TaskStore::delegate_children` が行う）。

use task_core::{
    DelegateDep, DelegateTask, DelegationLimits, GenreSpec, RoleSpec, Status, Task, TaskId,
    TaskStore, WorkspaceSpec,
};
use time::OffsetDateTime;

use crate::OpsError;

/// `plan_delegation` の結果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DelegationOutcome {
    /// 挿入してよい子（`status = Draft`。`TaskStore::delegate_children` が `Accept` する）。提案の順。
    pub accepted: Vec<Task>,
    /// 拒否した提案の理由。書式は `tasks[<i>] "<title>": <reason>`。ディスパッチャが `WorkerProgress` に載せる。
    pub rejected: Vec<String>,
    /// ADR-0062 B2（Phase 107）: 継承した Remote workspace を Local に落とした子（担当が
    /// `cluster:<id>` を持たなかったもの）。`(task_id, reason)`。ディスパッチャが 1 回だけ tracing に残す。
    pub workspace_downgrades: Vec<(TaskId, String)>,
}

fn reject(index: usize, title: &str, reason: impl std::fmt::Display) -> String {
    format!("tasks[{index}] {title:?}: {reason}")
}

/// 祖先の数 + 1（根 = 1）。親を辿る（64 hop で打ち切り）。
pub fn tree_depth(store: &dyn TaskStore, task: &Task) -> Result<u32, OpsError> {
    let mut depth: u32 = 1;
    let mut current = task.clone();
    for _ in 0..64 {
        match current.parent_id {
            Some(parent_id) => match store.get(parent_id)? {
                Some(parent) => {
                    depth += 1;
                    current = parent;
                }
                None => break,
            },
            None => break,
        }
    }
    Ok(depth)
}

/// 木の根の ID。
pub fn tree_root(store: &dyn TaskStore, task: &Task) -> Result<TaskId, OpsError> {
    let mut current = task.clone();
    for _ in 0..64 {
        match current.parent_id {
            Some(parent_id) => match store.get(parent_id)? {
                Some(parent) => current = parent,
                None => break,
            },
            None => break,
        }
    }
    Ok(current.id)
}

/// 根とその全子孫の `Event::WorkerStarted { role: None, .. }`（ワーカー run）の合計。
pub fn tree_worker_runs(store: &dyn TaskStore, root: TaskId) -> Result<u32, OpsError> {
    let mut total: u32 = 0;
    let mut stack: Vec<TaskId> = vec![root];
    while let Some(id) = stack.pop() {
        let events = store.events_for(id)?;
        for (_, event) in events {
            if let task_core::Event::WorkerStarted { role: None, .. } = event {
                total += 1;
            }
        }
        for child in store.children(id)? {
            stack.push(child.id);
        }
    }
    Ok(total)
}

/// `parent` の直接の子のうち終端でないものの数（`Status::is_terminal`）。
pub fn pending_children(store: &dyn TaskStore, parent: TaskId) -> Result<usize, OpsError> {
    Ok(store
        .children(parent)?
        .iter()
        .filter(|c| !c.status.is_terminal())
        .count())
}

/// `parent` の祖先の ID 列（親、その親、…）。
pub fn ancestors(store: &dyn TaskStore, task: &Task) -> Result<Vec<TaskId>, OpsError> {
    let mut out = Vec::new();
    let mut current = task.clone();
    for _ in 0..64 {
        match current.parent_id {
            Some(parent_id) => {
                out.push(parent_id);
                match store.get(parent_id)? {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
            None => break,
        }
    }
    Ok(out)
}

/// ADR-0039 D2: そのタスクが属する案件の作業場所（`projects.workspace`）。案件に属さない・案件が
/// 作業場所を決めていないなら `None`（従来どおり、子は親の workspace を継ぐ）。ストアの読み取りだけ。
pub fn project_workspace(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Option<WorkspaceSpec>, OpsError> {
    let Some(project_id) = task.project_id else {
        return Ok(None);
    };
    Ok(store.project_get(project_id)?.and_then(|p| p.workspace))
}

/// ADR-0043 D1 / D2: そのタスクが属する案件のリポジトリ（primary が先頭）。案件に属さない・
/// リポジトリを 1 つも登録していない案件では空。ストアの読み取りだけ。
pub fn project_repos(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Vec<task_core::ProjectRepo>, OpsError> {
    let Some(project_id) = task.project_id else {
        return Ok(Vec::new());
    };
    Ok(store.repo_list(project_id)?)
}

/// ADR-0016 D2 / M6 / M7, ADR-0027 D1: 実行中の委譲の検証。`already_delegated_this_run` は同じ run で
/// 既に受け入れた件数。`genres` は `genre`（分野）の解決・検証に使う（未知の `genre`、`role` と組み合わせた
/// ときの不整合は `task_core::validate_each` が拒否する）。
#[allow(clippy::too_many_arguments)]
pub fn plan_delegation(
    store: &dyn TaskStore,
    parent: &Task,
    proposals: &[DelegateTask],
    already_delegated_this_run: usize,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    limits: &DelegationLimits,
    now: OffsetDateTime,
) -> Result<DelegationOutcome, OpsError> {
    if proposals.is_empty() {
        return Ok(DelegationOutcome::default());
    }

    // 1. 木全体の上限。1 件でも当たれば全件拒否。
    let would_be_depth = tree_depth(store, parent)? + 1;
    if would_be_depth > limits.max_tree_depth {
        let reason = format!(
            "tree depth would become {would_be_depth} (max {})",
            limits.max_tree_depth
        );
        let rejected = proposals
            .iter()
            .enumerate()
            .map(|(i, t)| reject(i, &t.title, &reason))
            .collect();
        return Ok(DelegationOutcome {
            accepted: vec![],
            rejected,
            ..Default::default()
        });
    }
    let root = tree_root(store, parent)?;
    let runs = tree_worker_runs(store, root)?;
    if runs >= limits.max_tree_runs {
        let reason = format!(
            "tree already has {runs} worker runs (max {})",
            limits.max_tree_runs
        );
        let rejected = proposals
            .iter()
            .enumerate()
            .map(|(i, t)| reject(i, &t.title, &reason))
            .collect();
        return Ok(DelegationOutcome {
            accepted: vec![],
            rejected,
            ..Default::default()
        });
    }

    // 2. ストアを見ない検証。
    let each = task_core::validate_each(proposals, genres);

    // 3. ID 依存の検証（ストアを見る）。
    let ancestor_ids = ancestors(store, parent)?;
    let mut per_item: Vec<Result<(), String>> = Vec::with_capacity(proposals.len());
    for (i, result) in each.into_iter().enumerate() {
        if let Err(e) = result {
            per_item.push(Err(e.to_string()));
            continue;
        }
        let mut item_err: Option<String> = None;
        for dep in &proposals[i].depends_on {
            if let DelegateDep::Id(s) = dep {
                let Ok(dep_id) = s.parse::<TaskId>() else {
                    // `validate_each` already rejects malformed ids; unreachable in practice.
                    item_err = Some(format!("dependency {s} is not a task id"));
                    break;
                };
                if dep_id == parent.id {
                    item_err = Some(format!("dependency {dep_id} is the delegating task itself"));
                    break;
                }
                if ancestor_ids.contains(&dep_id) {
                    item_err = Some(format!(
                        "dependency {dep_id} is an ancestor of the delegating task"
                    ));
                    break;
                }
                match store.get(dep_id)? {
                    None => {
                        item_err = Some(format!("dependency {dep_id} does not exist"));
                        break;
                    }
                    Some(dep) if matches!(dep.status, Status::Failed | Status::Cancelled) => {
                        item_err = Some(format!(
                            "dependency {dep_id} has status {:?} and cannot be depended on",
                            dep.status
                        ));
                        break;
                    }
                    Some(_) => {}
                }
            }
        }
        per_item.push(match item_err {
            Some(e) => Err(e),
            None => Ok(()),
        });
    }

    // 4. 1 run の件数の上限。通った提案を順に数える。
    let mut accepted_indices: Vec<usize> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    let mut count = already_delegated_this_run;
    for (i, result) in per_item.into_iter().enumerate() {
        match result {
            Err(reason) => rejected.push(reject(i, &proposals[i].title, reason)),
            Ok(()) => {
                if count >= limits.max_delegate_per_run {
                    rejected.push(reject(
                        i,
                        &proposals[i].title,
                        format!(
                            "per-run delegation limit ({}) reached",
                            limits.max_delegate_per_run
                        ),
                    ));
                    continue;
                }
                count += 1;
                accepted_indices.push(i);
            }
        }
    }

    // 5. 組み立て。ADR-0033 D4: `assignee` の解決に組織図が要る（ストアの読み取りだけ。LLM は使わない）。
    let org = store.org_list()?;
    // ADR-0039 D2: 子の作業場所は 明示 > 案件の workspace > 親（案件を引くのもストアの読み取りだけ）。
    let project = project_workspace(store, parent)?;
    // ADR-0043 D2: 子のリポジトリは 明示 > 親 > 案件の primary。
    let project_repos = project_repos(store, parent)?;
    let home = task_core::home_dir();
    let workspace = task_core::WorkspaceContext {
        repos: &project_repos,
        project: project.as_ref(),
        home: home.as_deref(),
    };
    let mut workspace_downgrades: Vec<(TaskId, String)> = Vec::new();
    let accepted = task_core::materialize_delegated_logging(
        parent,
        proposals,
        &accepted_indices,
        &org,
        roles,
        genres,
        workspace,
        now,
        &mut |task_id, reason| workspace_downgrades.push((task_id, reason.to_string())),
    );

    Ok(DelegationOutcome {
        accepted,
        rejected,
        workspace_downgrades,
    })
}

/// ADR-0074 D3.7（Phase F4b (f)）: planner の `children` をどう扱うか。
#[derive(Debug, Clone, PartialEq)]
pub enum ChildrenPlan {
    /// 子を作ってよい（`plan_delegation` を通った draft の子。`child-<key>` の印・`skills`・`features`
    /// 付き。挿入時に `Accept` される）。`children` の順。
    Ready(Vec<Task>),
    /// 部をまたぐ子があり、まだ認可されていない。秘書への質問文（`CrossDepartment::question()`）。
    NeedsAuthorization(Vec<String>),
}

/// ADR-0074 D3.7（Phase F4b (f)）: execution-plan/2 の `children` を、既存の委譲の検証（`plan_delegation`:
/// 木の深さ・件数・木の run 数の上限、`validate_each`、担当の解決〈`assignee` は持たないので matching に
/// 任せる。ADR-0069 D1〉）に通す。1 件でも拒否されれば `Err(理由)`（計画の採用は不正な試行として扱う）。
/// 部をまたぐ判定は、子の担当になるはずのノード（`matching::decide` の決定的な結果）の部と、親の担当の
/// 部を比べる（SPEC §3.1 / ADR-0033 D4・D5）。認可されていないものがあれば `NeedsAuthorization`、
/// 人が認めなかったものがあれば `Err`。
#[allow(clippy::too_many_arguments)]
pub fn plan_children(
    store: &dyn TaskStore,
    parent: &Task,
    children: &[task_core::ExecutionChildSpec],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    limits: &DelegationLimits,
    now: OffsetDateTime,
) -> Result<ChildrenPlan, String> {
    if children.is_empty() {
        return Ok(ChildrenPlan::Ready(Vec::new()));
    }
    let index_of = |key: &str| children.iter().position(|c| c.key == key);
    let proposals: Vec<DelegateTask> = children
        .iter()
        .map(|c| DelegateTask {
            requirements: c.requirements.clone(),
            title: c.title.clone(),
            objective: c.objective.clone(),
            acceptance: c.acceptance.clone(),
            role: None,
            genre: c.genre.clone(),
            depends_on: c
                .depends_on
                .iter()
                .filter_map(|d| index_of(d).map(DelegateDep::Index))
                .collect(),
            tier: None,
            assignee: None,
            workspace: None,
        })
        .collect();
    let outcome = plan_delegation(store, parent, &proposals, 0, roles, genres, limits, now)
        .map_err(|e| format!("children: {e}"))?;
    if !outcome.rejected.is_empty() {
        return Err(format!(
            "children rejected: {}",
            outcome.rejected.join("; ")
        ));
    }
    if outcome.accepted.len() != children.len() {
        return Err("children: not every child was accepted".to_string());
    }
    let mut tasks = outcome.accepted;
    for (task, spec) in tasks.iter_mut().zip(children) {
        task.labels.push(task_core::child_label(&spec.key));
        if !spec.skills.is_empty() {
            task.skills = spec.skills.clone();
        }
        if let Some(features) = spec.features.filter(|f| !f.is_empty()) {
            task.routing.get_or_insert_with(Default::default).features = Some(features);
        }
        task_core::browser::validate_task_requirements(
            &task.skills,
            &task.requirements,
            Some(parent),
        )
        .map_err(|e| format!("child {}: {e}", spec.key))?;
    }

    // 部をまたぐか（担当は matching が決めるので、決まるはずのノードで判定する）。
    let pending = cross_department_questions(store, parent, &tasks)?;
    if pending.is_empty() {
        Ok(ChildrenPlan::Ready(tasks))
    } else {
        Ok(ChildrenPlan::NeedsAuthorization(pending))
    }
}

/// ADR-0074 F4b / ADR-0079 D4 (4): 子になる task（担当は matching が決める）のうち、親の担当と別の部署に
/// 当たるものの認可を見る（SPEC §3.1 / ADR-0033 D4・D5）。まだ認可されていないものの秘書への質問文
/// （重複なし）を返す。人が認めなかったものがあれば `Err`。親に担当が無い・部署が引けないなら空。
/// ストアの読み取りだけ（LLM なし）。
pub fn cross_department_questions(
    store: &dyn TaskStore,
    parent: &Task,
    tasks: &[Task],
) -> Result<Vec<String>, String> {
    let org = store.org_list().map_err(|e| e.to_string())?;
    let Some(from) = parent.assignee.as_deref() else {
        return Ok(Vec::new());
    };
    let Some(from_dept) = task_core::department_of(&org, from) else {
        return Ok(Vec::new());
    };
    let rules = store
        .standing_rule_list(Some(from))
        .map_err(|e| e.to_string())?;
    let approvals = store
        .approval_list(None, None, Some(from))
        .map_err(|e| e.to_string())?;
    let mut pending: Vec<String> = Vec::new();
    for task in tasks {
        let crate::matching::Assignment::Assigned { node, .. } =
            crate::matching::decide(&org, task)
        else {
            continue;
        };
        if task_core::department_of(&org, &node).is_none_or(|d| d == from_dept) {
            continue;
        }
        let crossing = crate::conversation::CrossDepartment {
            from: from.to_string(),
            to: node,
            title: task.title.clone(),
        };
        match crate::conversation::cross_authorization(
            &crossing.key(),
            parent.id,
            &approvals,
            &rules,
        ) {
            crate::conversation::CrossAuthorization::Allowed => {}
            crate::conversation::CrossAuthorization::Pending => {
                let q = crossing.question();
                if !pending.contains(&q) {
                    pending.push(q);
                }
            }
            crate::conversation::CrossAuthorization::Denied => {
                return Err(format!(
                    "child {:?} crosses departments ({}) and the delegation was denied",
                    task.title,
                    crossing.key()
                ));
            }
        }
    }
    Ok(pending)
}

#[cfg(test)]
mod tests;
