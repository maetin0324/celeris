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
    }

    // 部をまたぐか（担当は matching が決めるので、決まるはずのノードで判定する）。
    let org = store.org_list().map_err(|e| e.to_string())?;
    let Some(from) = parent.assignee.as_deref() else {
        return Ok(ChildrenPlan::Ready(tasks));
    };
    let Some(from_dept) = task_core::department_of(&org, from) else {
        return Ok(ChildrenPlan::Ready(tasks));
    };
    let rules = store
        .standing_rule_list(Some(from))
        .map_err(|e| e.to_string())?;
    let approvals = store
        .approval_list(None, None, Some(from))
        .map_err(|e| e.to_string())?;
    let mut pending: Vec<String> = Vec::new();
    for task in &tasks {
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
    if pending.is_empty() {
        Ok(ChildrenPlan::Ready(tasks))
    } else {
        Ok(ChildrenPlan::NeedsAuthorization(pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use task_core::{
        Budget, Check, Criterion, Event, RunRole, SqliteStore, Task, TaskKind, Tier, WorkerHint,
        WorkspaceSpec,
    };

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn dt(title: &str, deps: Vec<DelegateDep>) -> DelegateTask {
        DelegateTask {
            title: title.into(),
            objective: format!("do {title}"),
            acceptance: vec![Criterion {
                text: "c".into(),
                check: Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
            }],
            role: None,
            genre: None,
            depends_on: deps,
            tier: None,
            assignee: None,
            workspace: None,
        }
    }

    fn profile_node(
        id: &str,
        parent: Option<&str>,
        kind: task_core::OrgKind,
        allowed: &[&str],
    ) -> task_core::OrgNode {
        let t = now();
        task_core::OrgNode {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            name: id.into(),
            kind,
            genre: None,
            brief: String::new(),
            profile: task_core::Profile {
                harnesses: task_core::HarnessPrefs {
                    allowed: allowed.iter().map(|s| s.to_string()).collect(),
                    default: allowed.first().map(|s| s.to_string()),
                },
                ..task_core::Profile::default()
            },
            position: 0,
            created_at: t,
            updated_at: t,
        }
    }

    fn child_spec(key: &str, genre: &str, deps: &[&str]) -> task_core::ExecutionChildSpec {
        task_core::ExecutionChildSpec {
            key: key.into(),
            title: format!("child {key}"),
            objective: format!("do {key}"),
            acceptance: vec![Criterion {
                text: "c".into(),
                check: Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
            }],
            genre: Some(genre.into()),
            skills: vec!["survey".into()],
            features: None,
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
        }
    }

    /// ADR-0074 D3.7（Phase F4b (f)）: planner の `children` は既存の委譲の検証（`plan_delegation`）を
    /// 通って `child-<key>` の印つきの子になる。担当になるはずのノードが親と別の部なら秘書への質問
    /// （認可されるまで子は作らない）、認可（`once`）の後は通る、`denied` なら拒否。木の深さの上限など
    /// 委譲の検証に落ちれば拒否。
    #[test]
    fn plan_children_runs_the_delegation_checks_and_asks_before_crossing_departments() {
        use task_core::OrgKind;
        use task_core::approval::{Approval, ApprovalId, ApprovalStore, Decision};
        let store = SqliteStore::open_in_memory().unwrap();
        for n in [
            profile_node("cos", None, OrgKind::Secretary, &["conversation"]),
            profile_node("engineering", Some("cos"), OrgKind::Department, &["coding"]),
            profile_node(
                "research",
                Some("cos"),
                OrgKind::Department,
                &["literature"],
            ),
        ] {
            store.org_upsert(&n).unwrap();
        }
        let genres: Vec<GenreSpec> = ["coding", "literature"]
            .iter()
            .map(|g| GenreSpec {
                id: g.to_string(),
                ..GenreSpec::default()
            })
            .collect();
        let mut parent = make_task(None, Status::Running);
        parent.assignee = Some("engineering".into());
        parent.genre = Some("coding".into());
        store.insert(&parent).unwrap();
        let limits = DelegationLimits::default();

        // 同じ部（coding）の子はそのまま通る。依存は同じ children の中の index になる。
        let same = [
            child_spec("impl", "coding", &[]),
            child_spec("tests", "coding", &["impl"]),
        ];
        match plan_children(&store, &parent, &same, &[], &genres, &limits, now()).unwrap() {
            ChildrenPlan::Ready(tasks) => {
                assert_eq!(tasks.len(), 2);
                assert!(tasks[0].labels.contains(&"child-impl".to_string()));
                assert_eq!(tasks[1].depends_on, vec![tasks[0].id]);
                assert!(tasks.iter().all(|t| t.parent_id == Some(parent.id)));
                assert!(tasks.iter().all(|t| t.assignee.is_none()));
                assert_eq!(tasks[0].skills, vec!["survey".to_string()]);
            }
            other => panic!("{other:?}"),
        }

        // 別の部（literature → research）の子は秘書への質問。
        let cross = [child_spec("lit", "literature", &[])];
        let question =
            match plan_children(&store, &parent, &cross, &[], &genres, &limits, now()).unwrap() {
                ChildrenPlan::NeedsAuthorization(qs) => {
                    assert_eq!(qs.len(), 1);
                    assert!(
                        qs[0].starts_with("cross-department: engineering -> research"),
                        "{qs:?}"
                    );
                    qs[0].clone()
                }
                other => panic!("{other:?}"),
            };
        // 人が「今回だけ」認めれば通る。
        let mut approval = Approval {
            id: ApprovalId::new(),
            project_id: None,
            node_id: "engineering".into(),
            task_id: Some(parent.id),
            question: question.clone(),
            decision: Some(Decision::Once),
            answer: None,
            created_at: now(),
            decided_at: Some(now()),
        };
        store.approval_append(&approval).unwrap();
        assert!(matches!(
            plan_children(&store, &parent, &cross, &[], &genres, &limits, now()).unwrap(),
            ChildrenPlan::Ready(t) if t.len() == 1
        ));
        // 後から「認めない」と答え直せば拒否。
        approval.id = ApprovalId::new();
        approval.decision = Some(Decision::Denied);
        approval.decided_at = Some(now() + time::Duration::seconds(5));
        store.approval_append(&approval).unwrap();
        let err = plan_children(&store, &parent, &cross, &[], &genres, &limits, now())
            .expect_err("denied");
        assert!(err.contains("denied"), "{err}");

        // 委譲の検証（木の深さの上限）に落ちれば拒否。
        let shallow = DelegationLimits {
            max_tree_depth: 1,
            ..DelegationLimits::default()
        };
        let err = plan_children(&store, &parent, &same, &[], &genres, &shallow, now())
            .expect_err("too deep");
        assert!(err.contains("tree depth"), "{err}");
    }

    fn make_task(parent_id: Option<TaskId>, status: Status) -> Task {
        let t = now();
        Task {
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id,
            kind: TaskKind::Execute,
            title: "t".into(),
            objective: "o".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status,
            priority: 3,
            worker_hint: WorkerHint {
                tier: Tier::Frontier,
                adapter: Some("fake".into()),
            },
            workspace: WorkspaceSpec::Local {
                path: PathBuf::from("/tmp/ws"),
                mode: None,
            },
            budget: Budget {
                max_turns: 10,
                max_wall_secs: 600,
                max_retries: 2,
            },
            attempts: 0,
            lease: None,
            created_at: t,
            updated_at: t,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        }
    }

    fn insert(store: &SqliteStore, task: &Task) {
        store.insert(task).expect("insert");
    }

    #[test]
    fn accepts_proposals_maps_index_deps_and_applies_role_defaults() {
        let store = SqliteStore::open_in_memory().expect("open");
        let parent = make_task(None, Status::Running);
        insert(&store, &parent);

        let roles = vec![RoleSpec {
            id: "implementer".into(),
            tier: Some(Tier::Cheap),
            adapter: Some("codex".into()),
            max_turns: None,
            max_wall_secs: None,
            instructions: None,
        }];
        let mut a = dt("a", vec![]);
        a.role = Some("implementer".into());
        let b = dt("b", vec![DelegateDep::Index(0)]);
        let proposals = vec![a, b];

        let out = plan_delegation(
            &store,
            &parent,
            &proposals,
            0,
            &roles,
            &[],
            &DelegationLimits::default(),
            now(),
        )
        .expect("plan_delegation");
        assert_eq!(out.accepted.len(), 2);
        assert!(out.rejected.is_empty());
        assert_eq!(out.accepted[0].worker_hint.tier, Tier::Cheap);
        assert_eq!(out.accepted[1].depends_on, vec![out.accepted[0].id]);
    }

    /// ADR-0027 D1: `plan_delegation` は分野を解決し（役割の分野が一意なら継ぐ）、`genre` を指定した
    /// タスクの `role` がその分野に無ければ拒否する。unrelated_genre はここまでの検証を通っても
    /// role/genre の不整合で落ちる。
    #[test]
    fn plan_delegation_resolves_genre_and_rejects_role_genre_mismatch() {
        let store = SqliteStore::open_in_memory().expect("open");
        let mut parent = make_task(None, Status::Running);
        parent.genre = Some("coding".into());
        insert(&store, &parent);

        let genres = vec![task_core::GenreSpec {
            id: "literature".into(),
            description: "survey".into(),
            default_role: Some("literature-reader".into()),
            roles: vec!["literature-scout".into(), "literature-reader".into()],
            ..task_core::GenreSpec::default()
        }];

        // 1. role だけ指定: 分野が一意に決まるので "literature" を継ぐ。
        let mut by_role = dt("by-role", vec![]);
        by_role.role = Some("literature-scout".into());
        // 2. genre と role が矛盾: 拒否される。
        let mut mismatched = dt("mismatched", vec![]);
        mismatched.genre = Some("literature".into());
        mismatched.role = Some("implementer".into());
        // 3. 何も指定しない: 親の分野 "coding" を継ぐ。
        let inherits = dt("inherits", vec![]);

        let proposals = vec![by_role, mismatched, inherits];
        let out = plan_delegation(
            &store,
            &parent,
            &proposals,
            0,
            &[],
            &genres,
            &DelegationLimits::default(),
            now(),
        )
        .expect("plan_delegation");
        assert_eq!(out.accepted.len(), 2);
        assert_eq!(out.accepted[0].title, "by-role");
        assert_eq!(out.accepted[0].genre.as_deref(), Some("literature"));
        assert_eq!(out.accepted[1].title, "inherits");
        assert_eq!(out.accepted[1].genre.as_deref(), Some("coding"));
        assert_eq!(out.rejected.len(), 1);
        assert!(
            out.rejected[0].contains("mismatched"),
            "{}",
            out.rejected[0]
        );
        assert!(
            out.rejected[0].contains("is not one of genre"),
            "{}",
            out.rejected[0]
        );
    }

    #[test]
    fn per_run_limit_rejects_the_remainder() {
        let store = SqliteStore::open_in_memory().expect("open");
        let parent = make_task(None, Status::Running);
        insert(&store, &parent);

        let limits = DelegationLimits {
            max_delegate_per_run: 2,
            ..DelegationLimits::default()
        };
        let proposals = vec![dt("a", vec![]), dt("b", vec![])];
        let out = plan_delegation(&store, &parent, &proposals, 1, &[], &[], &limits, now())
            .expect("plan_delegation");
        assert_eq!(out.accepted.len(), 1);
        assert_eq!(out.rejected.len(), 1);
        assert!(
            out.rejected[0].contains("per-run delegation limit"),
            "{}",
            out.rejected[0]
        );
    }

    #[test]
    fn tree_depth_limit_rejects_everything() {
        let store = SqliteStore::open_in_memory().expect("open");
        let root = make_task(None, Status::Running);
        insert(&store, &root);
        let mid = make_task(Some(root.id), Status::Running);
        insert(&store, &mid);
        let parent = make_task(Some(mid.id), Status::Running);
        insert(&store, &parent);
        // tree_depth(parent) == 3, +1 == 4 > max_tree_depth (3) -> reject all
        let limits = DelegationLimits {
            max_tree_depth: 3,
            ..DelegationLimits::default()
        };
        let proposals = vec![dt("a", vec![]), dt("b", vec![])];
        let out = plan_delegation(&store, &parent, &proposals, 0, &[], &[], &limits, now())
            .expect("plan_delegation");
        assert!(out.accepted.is_empty());
        assert_eq!(out.rejected.len(), 2);
        assert!(
            out.rejected[0].contains("tree depth would become 4"),
            "{}",
            out.rejected[0]
        );
    }

    #[test]
    fn tree_runs_limit_rejects_everything_and_ignores_reviewer_runs() {
        let store = SqliteStore::open_in_memory().expect("open");
        let root = make_task(None, Status::Running);
        insert(&store, &root);
        let child = make_task(Some(root.id), Status::Running);
        insert(&store, &child);

        // 2 ワーカー run（root と child）+ 1 reviewer run（数えない）。
        store
            .append_event(
                root.id,
                &Event::WorkerStarted {
                    run_id: "r1".into(),
                    adapter: "a".into(),
                    model: "m".into(),
                    provider: None,
                    account: None,
                    role: None,
                    task_role: None,
                },
            )
            .expect("append");
        store
            .append_event(
                child.id,
                &Event::WorkerStarted {
                    run_id: "r2".into(),
                    adapter: "a".into(),
                    model: "m".into(),
                    provider: None,
                    account: None,
                    role: None,
                    task_role: None,
                },
            )
            .expect("append");
        store
            .append_event(
                child.id,
                &Event::WorkerStarted {
                    run_id: "rev1".into(),
                    adapter: "a".into(),
                    model: "m".into(),
                    provider: None,
                    account: None,
                    role: Some(RunRole::Reviewer),
                    task_role: None,
                },
            )
            .expect("append");

        assert_eq!(tree_worker_runs(&store, root.id).expect("count"), 2);

        let limits = DelegationLimits {
            max_tree_runs: 2,
            ..DelegationLimits::default()
        };
        let proposals = vec![dt("a", vec![])];
        let out = plan_delegation(&store, &child, &proposals, 0, &[], &[], &limits, now())
            .expect("plan_delegation");
        assert!(out.accepted.is_empty());
        assert_eq!(out.rejected.len(), 1);
        assert!(
            out.rejected[0].contains("tree already has 2 worker runs"),
            "{}",
            out.rejected[0]
        );
    }

    #[test]
    fn rejects_self_and_ancestor_and_missing_dependencies_but_accepts_others() {
        let store = SqliteStore::open_in_memory().expect("open");
        let grandparent = make_task(None, Status::Running);
        insert(&store, &grandparent);
        let parent = make_task(Some(grandparent.id), Status::Running);
        insert(&store, &parent);
        let missing_id = TaskId::new();

        let proposals = vec![
            dt("self-ref", vec![DelegateDep::Id(parent.id.to_string())]),
            dt(
                "ancestor-ref",
                vec![DelegateDep::Id(grandparent.id.to_string())],
            ),
            dt("missing-ref", vec![DelegateDep::Id(missing_id.to_string())]),
            dt("ok", vec![]),
        ];
        let out = plan_delegation(
            &store,
            &parent,
            &proposals,
            0,
            &[],
            &[],
            &DelegationLimits::default(),
            now(),
        )
        .expect("plan_delegation");
        assert_eq!(out.accepted.len(), 1);
        assert_eq!(out.accepted[0].title, "ok");
        assert_eq!(out.rejected.len(), 3);
        assert!(
            out.rejected[0].contains("is the delegating task itself"),
            "{}",
            out.rejected[0]
        );
        assert!(
            out.rejected[1].contains("is an ancestor of the delegating task"),
            "{}",
            out.rejected[1]
        );
        assert!(
            out.rejected[2].contains("does not exist"),
            "{}",
            out.rejected[2]
        );
    }

    #[test]
    fn rejects_dependency_with_failed_status() {
        let store = SqliteStore::open_in_memory().expect("open");
        let parent = make_task(None, Status::Running);
        insert(&store, &parent);
        let failed_dep = make_task(None, Status::Failed);
        insert(&store, &failed_dep);

        let proposals = vec![dt("a", vec![DelegateDep::Id(failed_dep.id.to_string())])];
        let out = plan_delegation(
            &store,
            &parent,
            &proposals,
            0,
            &[],
            &[],
            &DelegationLimits::default(),
            now(),
        )
        .expect("plan_delegation");
        assert!(out.accepted.is_empty());
        assert_eq!(out.rejected.len(), 1);
        assert!(
            out.rejected[0].contains("has status Failed and cannot be depended on"),
            "{}",
            out.rejected[0]
        );
    }

    /// ADR-0039 D2: 委譲した子は **明示 > 案件の workspace > 親** の順で作業場所を決める。
    /// 案件の作業場所は DB（`projects.workspace`）から引く（ストアの読み取りだけ）。
    #[test]
    fn delegated_children_inherit_the_projects_workspace() {
        let store = SqliteStore::open_in_memory().expect("open");
        let now_ts = now();
        let project = task_core::Project {
            auto_advance: false,
            slug: None,
            archived_at: None,
            paused_from: None,
            id: task_core::ProjectId::new(),
            title: "Pluvio".into(),
            request: "PoC".into(),
            status: task_core::ProjectStatus::Active,
            secretary_summary: None,
            workspace: Some(WorkspaceSpec::Remote {
                cluster: "pegasus".into(),
                path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
                mode: None,
            }),
            created_at: now_ts,
            updated_at: now_ts,
        };
        store.project_create(&project).expect("project");

        let mut parent = make_task(None, Status::Running);
        parent.project_id = Some(project.id);
        insert(&store, &parent);

        // 1. 何も書かなければ案件の作業場所（親の `/tmp/ws` ではない）。
        let out = plan_delegation(
            &store,
            &parent,
            &[dt("a", vec![])],
            0,
            &[],
            &[],
            &DelegationLimits::default(),
            now(),
        )
        .expect("plan_delegation");
        assert_eq!(
            out.accepted[0].workspace,
            project.workspace.clone().expect("some")
        );

        // 2. 子が明示すれば案件より強い。
        let mut explicit = dt("b", vec![]);
        let elsewhere = WorkspaceSpec::Local {
            path: PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
            mode: None,
        };
        explicit.workspace = Some(elsewhere.clone());
        let out = plan_delegation(
            &store,
            &parent,
            &[explicit],
            0,
            &[],
            &[],
            &DelegationLimits::default(),
            now(),
        )
        .expect("plan_delegation");
        assert_eq!(out.accepted[0].workspace, elsewhere);

        // 3. 案件に属さない親では従来どおり親を継ぐ。
        let orphan = make_task(None, Status::Running);
        insert(&store, &orphan);
        let out = plan_delegation(
            &store,
            &orphan,
            &[dt("c", vec![])],
            0,
            &[],
            &[],
            &DelegationLimits::default(),
            now(),
        )
        .expect("plan_delegation");
        assert_eq!(out.accepted[0].workspace, orphan.workspace);
    }

    #[test]
    fn pending_children_does_not_count_terminal_children() {
        let store = SqliteStore::open_in_memory().expect("open");
        let parent = make_task(None, Status::Running);
        insert(&store, &parent);
        let done_child = make_task(Some(parent.id), Status::Done);
        insert(&store, &done_child);
        let running_child = make_task(Some(parent.id), Status::Running);
        insert(&store, &running_child);

        assert_eq!(pending_children(&store, parent.id).expect("pending"), 1);
    }
}
