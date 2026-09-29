//! ADR-0079 §7 R1b: 計画（plan/3）の kind task の unit から子 task を作る・子の状態を unit に写す、の
//! 決定的な部品（LLM なし。ADR-0079 D16）。
//!
//! - [`build_child_task`]: unit の spec と親から子 task を組み立てる（D4 (4)。挿入は
//!   `TaskStore::tree_child_create` が 1 トランザクションで行う）。
//! - [`check_task_unit_repos`]: kind task の unit の `repos` が親の repos の部分集合か（R1a からの持ち越し）。
//! - [`unit_mirror`]: 子の状態 → unit の状態（D4 (5)）。
//! - [`answered_decisions`] / [`child_objective`]: 子の `objective` の末尾に足す固定の書式（D4 (4)・D7）。
//!
//! - Phase R2a: [`tree_counters`]（木の上限に照らす数を store から集める。D3）、[`decision_path`]
//!   （決定の要求の path。D7）、[`open_decision`]（同じ key の未回答の決定が木にあるか）。
//!
//! I/O は `TaskStore` の読み取りだけ。

use task_core::{
    DecisionRow, DecisionStatus, DelegateTask, ExecutionPlanSpec, GenreSpec, ParentUnit,
    PlanUnitSpec, RoleSpec, Status, Task, TaskId, TaskStore, TreeInfo, WorkUnitStatus,
};
use time::OffsetDateTime;

use crate::OpsError;

/// 子の `objective` の末尾に足す「木の中の位置」の見出し（固定）。
pub const TREE_PATH_HEADING: &str = "## 木の中の位置（ADR-0079 D4）";
/// 子の `objective` の末尾に足す「人の決定」の見出し（固定。ADR-0079 D7）。
pub const DECISIONS_HEADING: &str = "## 人の決定（ADR-0079 D7）";

/// D4 (5): 子 task の状態から決まる unit の状態と `WorkUnitTransitioned.reason`。子が非終端（走っている・
/// 待っている・人の入力を待つ `blocked` を含む）なら `None`（unit は `running` のまま。子の質問・決定は
/// 子の側で受信箱に出る）。
///
/// 子の `cancelled` も unit を **`failed`** にする（ADR-0079 付記「R1b 実装時の逸脱・明確化」2.:
/// `cancelled` の unit は計画の実行から外れる〈`is_active` でない〉ので、段階が黙って完了してしまう。
/// 失敗として残せば段階は完了せず、親は既存の失敗の経路〈replan〉に入る）。
pub fn unit_mirror(child_status: Status) -> Option<(WorkUnitStatus, &'static str)> {
    match child_status {
        Status::Done => Some((WorkUnitStatus::Done, "child_done")),
        Status::Failed => Some((WorkUnitStatus::Failed, "child_failed")),
        Status::Cancelled => Some((WorkUnitStatus::Failed, "child_cancelled")),
        Status::Draft | Status::Ready | Status::Running | Status::Blocked | Status::Reviewing => {
            None
        }
    }
}

/// 親の「実効の」repos（親が持てばそれ、無ければ案件の primary。子の既定と同じ規則。ADR-0043 D2）の名前。
pub fn parent_repo_names(store: &dyn TaskStore, parent: &Task) -> Result<Vec<String>, OpsError> {
    if !parent.repos.is_empty() {
        return Ok(parent.repos.iter().map(|r| r.name.clone()).collect());
    }
    let project_repos = crate::delegate::project_repos(store, parent)?;
    Ok(project_repos
        .iter()
        .find(|r| r.is_primary)
        .map(|r| vec![r.name.clone()])
        .unwrap_or_default())
}

/// ADR-0079 D2 / D4 (4)（R1a からの持ち越し）: 計画のすべての kind task の unit について、`repos` が
/// 親の repos の部分集合か。外れていれば `Err`（計画の採用を不正な試行として扱う文言）。
pub fn check_task_unit_repos(
    store: &dyn TaskStore,
    parent: &Task,
    spec: &ExecutionPlanSpec,
) -> Result<(), String> {
    let names = parent_repo_names(store, parent).map_err(|e| e.to_string())?;
    for unit in spec.units.iter().filter(|u| u.is_task()) {
        if let Some(bad) = unit.repos.iter().find(|r| !names.iter().any(|n| n == *r)) {
            return Err(format!(
                "unit {}: repos {bad:?} is not one of the parent's repos [{}] (a child task's repos must be a subset of its parent's; ADR-0079 D2)",
                unit.key,
                names.join(", ")
            ));
        }
    }
    Ok(())
}

/// D7: `keys` の決定のうち、`task_id` の節点が出して回答済みのもの（`keys` の順）。1 つでも回答が
/// 無ければ `None`（その unit はまだ待つ）。
pub fn answered_decisions(
    store: &dyn TaskStore,
    task_id: TaskId,
    keys: &[String],
) -> Result<Option<Vec<DecisionRow>>, OpsError> {
    if keys.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let rows = store.decisions_list(None)?;
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        match rows
            .iter()
            .filter(|r| r.task_id == task_id && &r.key == key)
            .find(|r| r.status == DecisionStatus::Answered)
        {
            Some(r) => out.push(r.clone()),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// 親から root までを辿った祖先（root が先頭、`parent` が末尾）。`Task.tree.parent_unit` を辿る
/// （`tree` の無い task は root）。64 hop で打ち切る。
pub fn ancestors_with_self(store: &dyn TaskStore, parent: &Task) -> Result<Vec<Task>, OpsError> {
    let mut chain = vec![parent.clone()];
    let mut current = parent.clone();
    for _ in 0..64 {
        let Some(up) = current
            .tree
            .as_ref()
            .and_then(|t| t.parent_unit.as_ref())
            .map(|u| u.task_id)
        else {
            break;
        };
        match store.get(up)? {
            Some(t) => {
                chain.push(t.clone());
                current = t;
            }
            None => break,
        }
    }
    chain.reverse();
    Ok(chain)
}

/// ADR-0079 D3（Phase R2a）: 木（`root_id` とその子孫）の上限に照らす数を store の読み取りだけで
/// 集める（`task_core::tree::tree_counters`。run・トークン・定価は `runs` の索引、leaf は `work_units`、
/// replan は `execution_plans` の版の数）。
pub fn tree_counters(
    store: &dyn TaskStore,
    root_id: TaskId,
) -> Result<task_core::TreeCounters, OpsError> {
    let mut nodes = Vec::new();
    for task in store.tree_tasks(root_id)? {
        nodes.push(task_core::TreeNodeFacts {
            task_id: task.id,
            depth: task_core::tree::depth_of(&task),
            runs: store.runs_for_task(task.id)?,
            work_units: store.work_units_for(task.id)?,
            plan_versions: u32::try_from(store.execution_plan_list(task.id)?.len())
                .unwrap_or(u32::MAX),
        });
    }
    Ok(task_core::tree::tree_counters(Some(root_id), &nodes))
}

/// ADR-0079 D7（Phase R2a）: 決定の要求の `path`（root から `node` まで）。各段は task の id と題名、
/// 次の節点が属する段階（`stage`）、その節点を作った親の unit（`unit`。root は無し）。
pub fn decision_path(
    store: &dyn TaskStore,
    node: &Task,
) -> Result<Vec<task_core::DecisionPathEntry>, OpsError> {
    let chain = ancestors_with_self(store, node)?;
    let mut out = Vec::with_capacity(chain.len());
    for (i, t) in chain.iter().enumerate() {
        let stage = chain
            .get(i + 1)
            .and_then(|next| next.tree.as_ref())
            .and_then(|tr| tr.parent_unit.as_ref())
            .map(|u| u.stage.clone());
        let unit = t
            .tree
            .as_ref()
            .and_then(|tr| tr.parent_unit.as_ref())
            .map(|u| u.unit_key.clone());
        out.push(task_core::DecisionPathEntry {
            task_id: t.id,
            title: t.title.clone(),
            stage,
            unit,
        });
    }
    Ok(out)
}

/// ADR-0079 D3（Phase R2a）: 木（`root_id`）に同じ `key` の未回答の決定があるか（木の上限の決定は木に
/// 1 件だけ開く。同じ超過で tick ごとに増やさない）。
pub fn open_decision(
    store: &dyn TaskStore,
    root_id: TaskId,
    key: &str,
) -> Result<Option<DecisionRow>, OpsError> {
    Ok(store
        .decisions_list(Some(root_id))?
        .into_iter()
        .find(|r| r.key == key && r.status == DecisionStatus::Open))
}

/// D4 (4) / D7: 子の `objective`（unit の `objective` の後に、祖先の path と回答済みの決定を固定の書式で）。
/// `ancestors` は root が先頭・親が末尾。
pub fn child_objective(
    unit: &PlanUnitSpec,
    ancestors: &[Task],
    decisions: &[DecisionRow],
) -> String {
    let mut out = unit.objective.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(TREE_PATH_HEADING);
    for (i, node) in ancestors.iter().enumerate() {
        // この祖先の中で、次の節点（無ければこの unit）が属する段階。
        let stage = ancestors
            .get(i + 1)
            .and_then(|next| next.tree.as_ref())
            .and_then(|t| t.parent_unit.as_ref())
            .map(|u| u.stage.clone())
            .unwrap_or_else(|| unit.stage.clone());
        out.push_str(&format!(
            "\n- 深さ {}: 「{}」（段階 {stage}）",
            i + 1,
            node.title
        ));
    }
    out.push_str(&format!(
        "\n- 深さ {}: この task「{}」（unit {}）",
        ancestors.len() + 1,
        unit.title,
        unit.key
    ));
    if !decisions.is_empty() {
        out.push_str("\n\n");
        out.push_str(DECISIONS_HEADING);
        for d in decisions {
            let Some(answer) = d.request.answer.as_ref() else {
                continue;
            };
            let label = d
                .request
                .options
                .iter()
                .find(|o| o.key == answer.option)
                .map(|o| o.label.clone())
                .unwrap_or_else(|| answer.option.clone());
            let agreement = if answer.option == d.request.recommended {
                "推奨どおり"
            } else {
                "推奨と異なる"
            };
            out.push_str(&format!(
                "\n- {} {}: {label}（{agreement}）",
                d.key, d.request.question
            ));
            if let Some(note) = answer.note.as_deref().filter(|n| !n.trim().is_empty()) {
                out.push_str(&format!(" — {note}"));
            }
        }
    }
    out
}

/// D4 (4): kind task の unit から子 task を組み立てる（挿入はしない）。
///
/// - `parent_id` = 親、`project_id` / `milestone_id` = 親、`kind = execute`、`status = ready`（親の計画が
///   採用済みなので draft を挟まない）、`labels = [child-<key>]`。
/// - `title` / `acceptance` = unit、`objective` = [`child_objective`]。
/// - `genre` / `skills` = unit（無ければ親）、`repos` = unit（親の部分集合。無ければ親と同じ）、
///   `budget` = 親、`workspace` = 親（ADR-0062 B2 / D5: 担当が `cluster:<id>` を持たなければ継いだ remote を
///   local に落とす。`task_core::materialize_delegated_logging` の規則をそのまま通す）。
/// - 担当は書かない（matching が決める。ADR-0069 D1）。unit の `features` は `routing.features`（ヒント）。
/// - `tree = {root_id, depth + 1, parent_unit}`（`base_commit` は R1c）。`execution_hint` は持たない
///   （子は最初の dispatch で自分の Complexity Gate を通る。ADR-0079 D4 (1)）。
///
/// `Err` はこの unit を作れない理由（unit を `failed` にする文言）。
#[allow(clippy::too_many_arguments)]
pub fn build_child_task(
    store: &dyn TaskStore,
    parent: &Task,
    plan_id: &str,
    unit: &PlanUnitSpec,
    decisions: &[DecisionRow],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<(Task, Vec<String>), String> {
    if !unit.is_task() {
        return Err(format!("unit {} is not a task unit", unit.key));
    }
    let ops = |e: OpsError| e.to_string();
    // repos ⊆ 親（採用時にも見ているが、親の repos が後から変わった場合の保険）。
    let parent_names = parent_repo_names(store, parent).map_err(ops)?;
    if let Some(bad) = unit
        .repos
        .iter()
        .find(|r| !parent_names.iter().any(|n| n == *r))
    {
        return Err(format!(
            "unit {}: repos {bad:?} is not one of the parent's repos [{}] (ADR-0079 D2)",
            unit.key,
            parent_names.join(", ")
        ));
    }
    let ancestors = ancestors_with_self(store, parent).map_err(ops)?;
    let proposal = DelegateTask {
        title: unit.title.clone(),
        objective: child_objective(unit, &ancestors, decisions),
        acceptance: unit.acceptance.clone(),
        role: None,
        genre: unit.genre.clone().or_else(|| parent.genre.clone()),
        depends_on: Vec::new(),
        tier: None,
        assignee: None,
        workspace: None,
    };
    let proposals = [proposal];
    if let Some(Err(e)) = task_core::validate_each(&proposals, genres)
        .into_iter()
        .next()
    {
        return Err(format!("unit {}: {e}", unit.key));
    }
    let org = store.org_list().map_err(|e| e.to_string())?;
    let project_repos = crate::delegate::project_repos(store, parent).map_err(ops)?;
    let home = task_core::home_dir();
    // D4 (4): 子の workspace は親（案件の workspace で上書きしない）。
    let workspace = task_core::WorkspaceContext {
        repos: &project_repos,
        project: None,
        home: home.as_deref(),
    };
    let mut downgrades: Vec<String> = Vec::new();
    let mut built = task_core::materialize_delegated_logging(
        parent,
        &proposals,
        &[0],
        &org,
        roles,
        genres,
        workspace,
        now,
        &mut |_, reason| downgrades.push(reason.to_string()),
    );
    let Some(mut child) = built.pop() else {
        return Err(format!(
            "unit {}: the child task could not be built",
            unit.key
        ));
    };
    child.status = Status::Ready;
    child.budget = parent.budget;
    child.labels = vec![task_core::child_label(&unit.key)];
    child.skills = if unit.skills.is_empty() {
        parent.skills.clone()
    } else {
        unit.skills.clone()
    };
    if !unit.repos.is_empty() {
        let inherited = workspace.child_repos(parent, &[]);
        child.repos = unit
            .repos
            .iter()
            .filter_map(|name| inherited.iter().find(|r| &r.name == name).cloned())
            .collect();
    }
    if let Some(features) = unit
        .features
        .clone()
        .and_then(|v| serde_json::from_value::<task_core::TaskFeatureHints>(v).ok())
        .filter(|f| !f.is_empty())
    {
        child.routing.get_or_insert_with(Default::default).features = Some(features);
    }
    child.tree = Some(TreeInfo::child_of(
        parent,
        ParentUnit {
            task_id: parent.id,
            plan_id: plan_id.to_string(),
            unit_key: unit.key.clone(),
            stage: unit.stage.clone(),
        },
        None,
    ));
    Ok((child, downgrades))
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{
        Budget, Check, Criterion, SqliteStore, TaskKind, Tier, WorkerHint, WorkspaceSpec,
    };

    fn parent(repos: &[&str]) -> Task {
        let now = OffsetDateTime::now_utc();
        Task {
            tree: None,
            routing: None,
            mode: Default::default(),
            skills: vec!["rust".into()],
            repos: repos
                .iter()
                .map(|n| task_core::RepoRef {
                    repo_id: task_core::RepoId::new(),
                    name: (*n).to_string(),
                })
                .collect(),
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "browser capability".into(),
            objective: "o".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Ready,
            priority: 2,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: "/tmp/ws".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 30,
                max_wall_secs: 900,
                max_retries: 2,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
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

    fn v3(units: serde_json::Value) -> ExecutionPlanSpec {
        serde_json::from_value(serde_json::json!({
            "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": "r",
            "stages": [{"key": "phase-2", "kind": "implement", "title": "Phase 2"}],
            "units": units,
        }))
        .unwrap()
    }

    fn task_unit(repos: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "key": "p2-b", "stage": "phase-2", "kind": "task", "title": "broker",
            "objective": "build the broker",
            "acceptance": [serde_json::to_value(Criterion {
                text: "ok".into(),
                check: Check::Command { cmd: "true".into(), expect_exit: 0 },
            }).unwrap()],
            "repos": repos,
            "skills": ["go"],
        })
    }

    /// R1a からの持ち越し: kind task の unit の `repos` は親の repos の部分集合。
    #[test]
    fn task_unit_repos_must_be_a_subset_of_the_parent_repos() {
        let store = SqliteStore::open_in_memory().unwrap();
        let p = parent(&["agent-platform", "docs"]);
        assert!(
            check_task_unit_repos(&store, &p, &v3(serde_json::json!([task_unit(&["docs"])])))
                .is_ok()
        );
        assert!(
            check_task_unit_repos(&store, &p, &v3(serde_json::json!([task_unit(&[])]))).is_ok()
        );
        let err = check_task_unit_repos(
            &store,
            &p,
            &v3(serde_json::json!([task_unit(&["docs", "benchfs"])])),
        )
        .unwrap_err();
        assert!(err.contains("\"benchfs\""), "{err}");
        assert!(err.contains("agent-platform, docs"), "{err}");
        // 親が repos を持たず案件も無ければ、repos を書いた unit はすべて外れる。
        assert!(
            check_task_unit_repos(
                &store,
                &parent(&[]),
                &v3(serde_json::json!([task_unit(&["docs"])]))
            )
            .is_err()
        );
    }

    /// D4 (4): 子は unit の repos（親の部分集合）・skills、親の workspace・budget・案件を継ぎ、ready・
    /// `child-<key>`・`tree`（深さ 2、root = 親）を持ち、objective の末尾に木の位置が固定の書式で入る。
    #[test]
    fn build_child_task_inherits_from_the_parent_and_the_unit() {
        let store = SqliteStore::open_in_memory().unwrap();
        let p = parent(&["agent-platform", "docs"]);
        let spec = v3(serde_json::json!([task_unit(&["docs"])]));
        let (child, downgrades) = build_child_task(
            &store,
            &p,
            "plan-1",
            &spec.units[0],
            &[],
            &[],
            &[],
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert!(downgrades.is_empty());
        assert_eq!(child.parent_id, Some(p.id));
        assert_eq!(child.status, Status::Ready);
        assert_eq!(child.kind, TaskKind::Execute);
        assert_eq!(child.labels, vec!["child-p2-b".to_string()]);
        assert_eq!(child.skills, vec!["go".to_string()]);
        assert_eq!(
            child
                .repos
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["docs"]
        );
        assert_eq!(child.repos[0].repo_id, p.repos[1].repo_id);
        assert_eq!(child.workspace, p.workspace);
        assert_eq!(child.budget, p.budget);
        assert_eq!(child.assignee, None);
        let tree = child.tree.as_ref().unwrap();
        assert_eq!(tree.root_id, p.id);
        assert_eq!(tree.depth, 2);
        assert_eq!(
            tree.parent_unit,
            Some(ParentUnit {
                task_id: p.id,
                plan_id: "plan-1".into(),
                unit_key: "p2-b".into(),
                stage: "phase-2".into(),
            })
        );
        assert_eq!(
            child.objective,
            format!(
                "build the broker\n\n{TREE_PATH_HEADING}\n- 深さ 1: 「browser capability」（段階 phase-2）\n- 深さ 2: この task「broker」（unit p2-b）"
            )
        );
        // unit が repos を書かなければ親と同じ。
        let spec = v3(serde_json::json!([task_unit(&[])]));
        let (child, _) = build_child_task(
            &store,
            &p,
            "plan-1",
            &spec.units[0],
            &[],
            &[],
            &[],
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(child.repos, p.repos);
    }

    #[test]
    fn unit_mirror_follows_the_child_status() {
        assert_eq!(
            unit_mirror(Status::Done),
            Some((WorkUnitStatus::Done, "child_done"))
        );
        assert_eq!(
            unit_mirror(Status::Failed),
            Some((WorkUnitStatus::Failed, "child_failed"))
        );
        assert_eq!(
            unit_mirror(Status::Cancelled),
            Some((WorkUnitStatus::Failed, "child_cancelled"))
        );
        for s in [
            Status::Draft,
            Status::Ready,
            Status::Running,
            Status::Blocked,
            Status::Reviewing,
        ] {
            assert_eq!(unit_mirror(s), None, "{s:?} keeps the unit running");
        }
    }
}
