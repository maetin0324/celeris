//! ADR-0079 D11 / §7 R4a: 木の閲覧（`GET /tasks/{id}/task-tree`）・roll-up の材料集め・案件の root の合計・
//! 深さ別の指標・段階の途中報告の子の要約。
//!
//! 決定的（store の読み取りだけ。LLM なし。ADR-0079 D16）。数の畳み込みそのものは純粋関数
//! `task_core::tree_metrics::{rollup, node_metrics, by_depth}`。ここは store から [`RollupNodeFacts`] を
//! 集めることと、表示用の導出値（[`TreeNodePhase`]）だけを持つ。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;
use task_core::{
    DecisionStatus, Event, RollupMetrics, RollupNodeFacts, Status, Task, TaskId, TaskStore,
    TreeLimits, WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitStatus,
};

use crate::OpsError;
use crate::view::ExecutionPhase;

/// 木の節点の「今どこか」（ADR-0079 D5 / D10 の名指しの待ちを含む表示用の導出値。状態機械には足さない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TreeNodePhase {
    Planning,
    Executing,
    Repairing,
    Verifying,
    /// 段階の後の途中確認（ADR-0074 D2）。
    AwaitingHuman,
    /// 子 task だけを待っている（D5）。
    AwaitingChildren,
    /// root の計画の承認待ち（D8）。
    AwaitingPlanApproval,
    /// 答えの無い決定で止まっている（節点の `self` の決定、または `blocked(decision)` の unit だけが残る）。
    HeldOnDecision,
    /// 子の基盤の失敗で unit が `blocked(infra)`（D9。人の再試行を待つ）。
    BlockedInfra,
}

impl From<ExecutionPhase> for TreeNodePhase {
    fn from(p: ExecutionPhase) -> Self {
        match p {
            ExecutionPhase::Planning => TreeNodePhase::Planning,
            ExecutionPhase::Executing => TreeNodePhase::Executing,
            ExecutionPhase::Repairing => TreeNodePhase::Repairing,
            ExecutionPhase::Verifying => TreeNodePhase::Verifying,
            ExecutionPhase::AwaitingHuman => TreeNodePhase::AwaitingHuman,
            ExecutionPhase::AwaitingChildren => TreeNodePhase::AwaitingChildren,
            ExecutionPhase::AwaitingPlanApproval => TreeNodePhase::AwaitingPlanApproval,
        }
    }
}

/// 節点の計画の unit 1 件（統合 WU を含む。superseded / cancelled も履歴として出す）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TreeUnitView {
    pub key: String,
    /// 段階（`work_units.phase`）。/1 の計画は無し。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    pub kind: WorkUnitKind,
    pub title: String,
    pub status: WorkUnitStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<WorkUnitBlockedReason>,
    /// kind task の unit の子 task。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_task_id: Option<TaskId>,
}

/// 木の 1 節点。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskTreeNode {
    pub id: TaskId,
    pub title: String,
    pub status: Status,
    /// 表示用の導出値（終端・計画も待ちも無い task は無し）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<TreeNodePhase>,
    /// task の層（root = 1）。
    pub depth: u32,
    /// 木の親（root は無し）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<TaskId>,
    /// この節点を作った親の計画の unit の key と段階。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_unit_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_stage: Option<String>,
    /// 今の計画の版（計画の無い task は無し）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_version: Option<u32>,
    /// この節点が出した未回答の決定の数。
    pub open_decisions: u32,
    /// 子の節点（この view に含まれるもの。作られた順）。
    #[serde(default)]
    pub children: Vec<TaskId>,
    #[serde(default)]
    pub units: Vec<TreeUnitView>,
    /// 自分の分（run・トークン・定価・quota・壁時計・leaf・決定）。
    pub own: RollupMetrics,
    /// 自分と子孫の合計。
    pub subtree: RollupMetrics,
}

/// 1 つの木の上限の使用（D3）。`max_*` は回答の余裕（`raise-once` / `replan`）を当てた値。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TreeLimitsUsage {
    pub leaves: u32,
    pub max_leaves: u32,
    pub runs: u32,
    pub max_runs: u32,
    pub replans: u32,
    pub max_replans: u32,
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    pub open_decisions: u32,
    pub max_open_decisions: u32,
}

/// `GET /tasks/{id}/task-tree` の応答。`nodes` は前順（親が子より先。先頭がこの view の根）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskTreeView {
    /// 木の root（`tree::root_id_of`）。
    pub root_id: TaskId,
    /// この view の根（`?root=true` なら `root_id`、そうでなければ問い合わせた task）。
    pub subtree_root: TaskId,
    /// `[execution.tree] enabled`。
    pub tree_enabled: bool,
    pub nodes: Vec<TaskTreeNode>,
    /// view の根の subtree の合計（`nodes[0].subtree` と同じ）。
    pub totals: RollupMetrics,
    /// view が木の root を含むときだけ、木の上限の使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<TreeLimitsUsage>,
}

fn tree_parent(task: &Task) -> Option<TaskId> {
    task.tree
        .as_ref()
        .and_then(|t| t.parent_unit.as_ref())
        .map(|u| u.task_id)
}

/// 未回答の決定の数（出した節点ごと）と、`self` の決定を出して止まっている節点。
fn open_decisions_by_task(
    rows: &[task_core::DecisionRow],
) -> (BTreeMap<TaskId, u32>, BTreeSet<TaskId>) {
    let mut counts: BTreeMap<TaskId, u32> = BTreeMap::new();
    let mut self_held: BTreeSet<TaskId> = BTreeSet::new();
    for r in rows.iter().filter(|r| r.status == DecisionStatus::Open) {
        *counts.entry(r.task_id).or_default() += 1;
        if r.needed_before.iter().any(|n| n == "self") {
            self_held.insert(r.task_id);
        }
    }
    (counts, self_held)
}

/// 1 節点の材料（`quota` は events を読むときだけ。`with_quota = false` なら空）。
pub fn node_facts(
    store: &dyn TaskStore,
    task: &Task,
    open_decisions: u32,
    events: Option<&[Event]>,
) -> Result<RollupNodeFacts, OpsError> {
    let quota = match events {
        Some(events) => task_core::summarize_execution_metrics(task, events).quota,
        None => Vec::new(),
    };
    Ok(RollupNodeFacts {
        task_id: task.id,
        parent_id: tree_parent(task),
        depth: task_core::tree::depth_of(task),
        runs: store.runs_for_task(task.id)?,
        work_units: store.work_units_for(task.id)?,
        quota,
        open_decisions,
    })
}

/// 表示用の導出値（[`TreeNodePhase`] の規則）: 終端なら無し。Execution 節の段階（計画・実行・修復・検証・
/// 途中確認・承認待ち）があればそれ。そうでなければ、節点の `self` の決定 → `blocked(infra)` の unit →
/// 子待ち → `blocked(decision)` の unit の順に名指しの待ちを選ぶ。
fn node_phase(
    task: &Task,
    execution_phase: Option<ExecutionPhase>,
    units: &[WorkUnitRow],
    self_held: bool,
) -> Option<TreeNodePhase> {
    if task.status.is_terminal() {
        return None;
    }
    if let Some(p) = execution_phase
        && p != ExecutionPhase::AwaitingChildren
    {
        return Some(p.into());
    }
    let active = || units.iter().filter(|u| u.status.is_active());
    let blocked = |reason: WorkUnitBlockedReason| {
        active().any(|u| u.status == WorkUnitStatus::Blocked && u.blocked_reason == Some(reason))
    };
    if self_held {
        return Some(TreeNodePhase::HeldOnDecision);
    }
    if blocked(WorkUnitBlockedReason::Infra) {
        return Some(TreeNodePhase::BlockedInfra);
    }
    if execution_phase == Some(ExecutionPhase::AwaitingChildren) {
        return Some(TreeNodePhase::AwaitingChildren);
    }
    if blocked(WorkUnitBlockedReason::Decision) {
        return Some(TreeNodePhase::HeldOnDecision);
    }
    None
}

/// D11 / §7 R4a: `GET /tasks/{id}/task-tree`。`from_root` なら木の root から、そうでなければ `task_id` の
/// subtree。木の無い task（`[execution.tree] enabled = false` の旧い task を含む）は 1 節点の木（深さ 1）。
/// 見つからなければ `OpsError::NotFound`。
pub fn task_tree(
    store: &dyn TaskStore,
    task_id: TaskId,
    from_root: bool,
    limits: &TreeLimits,
) -> Result<TaskTreeView, OpsError> {
    let Some(task) = store.get(task_id)? else {
        return Err(OpsError::NotFound(task_id));
    };
    let tree_enabled = limits.enabled;
    let root_id = task_core::tree::root_id_of(&task);
    let subtree_root = if from_root { root_id } else { task_id };
    let all = store.tree_tasks(root_id)?;
    // 子の一覧（作られた順 = `tree_tasks` の順）。
    let mut children_of: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for t in &all {
        if let Some(p) = tree_parent(t) {
            children_of.entry(p).or_default().push(t.id);
        }
    }
    let by_id: BTreeMap<TaskId, &Task> = all.iter().map(|t| (t.id, t)).collect();
    // 前順（親が先）。64 段・既に見た節点で打ち切る。
    let mut order: Vec<TaskId> = Vec::new();
    let mut seen: BTreeSet<TaskId> = BTreeSet::new();
    let mut stack: Vec<(TaskId, u32)> = vec![(subtree_root, 0)];
    while let Some((id, hops)) = stack.pop() {
        if hops > 64 || !seen.insert(id) || !by_id.contains_key(&id) {
            continue;
        }
        order.push(id);
        if let Some(kids) = children_of.get(&id) {
            for k in kids.iter().rev() {
                stack.push((*k, hops + 1));
            }
        }
    }
    let decisions = store.decisions_list(Some(root_id))?;
    let (open_counts, self_held) = open_decisions_by_task(&decisions);

    let mut facts: Vec<RollupNodeFacts> = Vec::with_capacity(order.len());
    let mut shells: Vec<TaskTreeNode> = Vec::with_capacity(order.len());
    for id in &order {
        let Some(t) = by_id.get(id).copied() else {
            continue;
        };
        let events = store.events_for(t.id)?;
        let plain: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
        let open = open_counts.get(&t.id).copied().unwrap_or(0);
        let f = node_facts(store, t, open, Some(&plain))?;
        let execution_phase = crate::view::execution_phase_of(store, t, &events)?;
        let plan_version = store
            .execution_plan_list(t.id)?
            .iter()
            .map(|p| p.version)
            .max();
        let parent_unit = t.tree.as_ref().and_then(|tr| tr.parent_unit.as_ref());
        let units = f
            .work_units
            .iter()
            .map(|u| TreeUnitView {
                key: u.key.clone(),
                stage: u.phase.clone(),
                kind: u.kind,
                title: u.spec.title.clone(),
                status: u.status,
                blocked_reason: u.blocked_reason,
                child_task_id: u.child_task_id.as_deref().and_then(|s| s.parse().ok()),
            })
            .collect();
        shells.push(TaskTreeNode {
            id: t.id,
            title: t.title.clone(),
            status: t.status,
            phase: node_phase(t, execution_phase, &f.work_units, self_held.contains(&t.id)),
            depth: f.depth,
            parent_id: f
                .parent_id
                .filter(|p| seen.contains(p) && *id != subtree_root),
            parent_unit_key: parent_unit.map(|u| u.unit_key.clone()),
            parent_stage: parent_unit.map(|u| u.stage.clone()),
            plan_version,
            open_decisions: open,
            children: children_of
                .get(&t.id)
                .map(|k| k.iter().filter(|c| seen.contains(c)).copied().collect())
                .unwrap_or_default(),
            units,
            own: RollupMetrics::default(),
            subtree: RollupMetrics::default(),
        });
        facts.push(f);
    }
    // view の根の親は view の外なので、roll-up では根として扱う。
    if let Some(first) = facts.first_mut() {
        first.parent_id = None;
    }
    let rolled = task_core::tree_metrics::rollup(&facts);
    for (node, r) in shells.iter_mut().zip(rolled) {
        node.own = r.own;
        node.subtree = r.subtree;
    }
    let totals = shells
        .first()
        .map(|n| n.subtree.clone())
        .unwrap_or_default();
    let usage = if subtree_root == root_id {
        let counters = crate::tree::tree_counters(store, root_id)?;
        let allowances = crate::decision::limit_allowances(store, root_id, None)?;
        let effective = task_core::tree::limits_with_allowances(limits, &allowances);
        Some(TreeLimitsUsage {
            leaves: counters.leaves,
            max_leaves: effective.max_tree_leaves,
            runs: counters.runs,
            max_runs: effective.max_tree_runs,
            replans: counters.replans,
            max_replans: effective.max_tree_replans,
            tokens: counters.tokens,
            max_tokens: effective.max_tree_tokens,
            open_decisions: open_counts.values().sum(),
            max_open_decisions: u32::try_from(effective.max_open_decisions_per_tree)
                .unwrap_or(u32::MAX),
        })
    } else {
        None
    };
    Ok(TaskTreeView {
        root_id,
        subtree_root,
        tree_enabled,
        nodes: shells,
        totals,
        limits: usage,
    })
}

/// D11 / D13: 案件の root task（`parent_id` が無く、対話でも裏方でもない task）の合計。`GET /projects/{id}`。
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, JsonSchema)]
pub struct ProjectRootTotals {
    /// root task の数。
    pub root_tasks: u32,
    /// root task の状態ごとの数（`draft` / `ready` / … / `done` / `failed` / `cancelled`。0 件の状態は出ない）。
    #[serde(default)]
    pub by_status: BTreeMap<String, u32>,
    /// root task の subtree の合計（run・reviewer の run・トークン・定価・leaf・決定・壁時計）。**quota は数えない**
    /// （events を読まない。quota は `GET /tasks/{id}/task-tree` と `GET /metrics/execution`）。
    pub totals: RollupMetrics,
}

/// 案件の root task か（D13 の `is_root_task` の読み: 案件直下〈`parent_id` なし〉・木の子でない・対話でも
/// 裏方〈`support_kind`〉でもない）。
pub fn is_project_root_task(task: &Task) -> bool {
    task.parent_id.is_none()
        && tree_parent(task).is_none()
        && !task_core::is_conversation(task)
        && task_core::report::support_kind(task).is_none()
}

/// D11: `GET /projects/{id}` の root task の合計（`tasks` は案件の task。root 以外は無視する）。
pub fn project_root_totals(
    store: &dyn TaskStore,
    tasks: &[Task],
) -> Result<ProjectRootTotals, OpsError> {
    let decisions = store.decisions_list(None)?;
    let (open_counts, _) = open_decisions_by_task(&decisions);
    let mut out = ProjectRootTotals {
        root_tasks: 0,
        by_status: BTreeMap::new(),
        totals: RollupMetrics::default(),
    };
    for root in tasks.iter().filter(|t| is_project_root_task(t)) {
        out.root_tasks += 1;
        *out.by_status
            .entry(crate::view::status_key(root.status).to_string())
            .or_default() += 1;
        let mut facts = Vec::new();
        for t in store.tree_tasks(root.id)? {
            let open = open_counts.get(&t.id).copied().unwrap_or(0);
            facts.push(node_facts(store, &t, open, None)?);
        }
        if facts.is_empty() {
            facts.push(node_facts(
                store,
                root,
                open_counts.get(&root.id).copied().unwrap_or(0),
                None,
            )?);
        }
        let subtree = RollupMetrics::sum(
            facts
                .iter()
                .map(task_core::tree_metrics::node_metrics)
                .collect::<Vec<_>>()
                .iter(),
        );
        out.totals.absorb(&subtree);
    }
    Ok(out)
}

/// D5 / D11 / §7 R4a: 段階 `phase` の子 task（kind task の unit）ごとの要約の行（途中報告の `child_units`）。
/// 書式（固定・決定的）: `<key> <子の題名>: <子の状態> / <n> run（reviewer <m>）/ $<定価>[（不完全）] — <見出し>`。
/// run と定価は子の subtree（孫を含む）の合計、見出しは子の最新の報告（ADR-0034 の報告の流れ）の `headline`、
/// 無ければ「報告なし」。子がまだ作られていない unit は `<key> <unit の題名>: 未作成（<unit の状態>）`。
pub fn stage_child_summaries(
    store: &dyn TaskStore,
    units: &[WorkUnitRow],
    phase: &str,
) -> Result<Vec<String>, OpsError> {
    let mut out = Vec::new();
    let mut in_stage: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| {
            u.kind == WorkUnitKind::Task
                && u.status.is_active()
                && u.phase.as_deref() == Some(phase)
        })
        .collect();
    in_stage.sort_by_key(|u| u.seq);
    for u in in_stage {
        let child = match u
            .child_task_id
            .as_deref()
            .and_then(|s| s.parse::<TaskId>().ok())
        {
            Some(id) => store.get(id)?,
            None => None,
        };
        let Some(child) = child else {
            out.push(format!(
                "{} {}: 未作成（{}）",
                u.key,
                u.spec.title,
                u.status.as_str()
            ));
            continue;
        };
        let mut facts = Vec::new();
        for t in store.tree_tasks(task_core::tree::root_id_of(&child))? {
            if t.id == child.id || is_descendant_of(&t, child.id, store)? {
                facts.push(node_facts(store, &t, 0, None)?);
            }
        }
        let sum = RollupMetrics::sum(
            facts
                .iter()
                .map(task_core::tree_metrics::node_metrics)
                .collect::<Vec<_>>()
                .iter(),
        );
        let headline = store
            .report_list(&task_core::report::ReportFilter {
                task_id: Some(child.id),
                limit: 1,
                ..task_core::report::ReportFilter::default()
            })?
            .into_iter()
            .next()
            .map(|r| r.headline)
            .unwrap_or_else(|| "報告なし".to_string());
        out.push(format!(
            "{} {}: {} / {} run（reviewer {}）/ ${:.2}{} — {}",
            u.key,
            child.title,
            crate::view::status_key(child.status),
            sum.runs,
            sum.reviewer_runs,
            sum.cost_usd,
            if sum.cost_usd_complete {
                ""
            } else {
                "（不完全）"
            },
            headline
        ));
    }
    Ok(out)
}

/// `t` が `ancestor` の子孫か（木の親を辿る。64 段で打ち切る）。
fn is_descendant_of(t: &Task, ancestor: TaskId, store: &dyn TaskStore) -> Result<bool, OpsError> {
    let mut parent = tree_parent(t);
    for _ in 0..64 {
        let Some(p) = parent else {
            return Ok(false);
        };
        if p == ancestor {
            return Ok(true);
        }
        parent = match store.get(p)? {
            Some(pt) => tree_parent(&pt),
            None => None,
        };
    }
    Ok(false)
}

/// U-R7（`GET /metrics/execution?group_by=depth`）: 1 task の自分の分（quota は `with_quota` のときだけ events から）。
/// `open_decisions` は呼び出し側が [`open_decision_counts`] から渡す。
pub fn own_metrics(
    store: &dyn TaskStore,
    task: &Task,
    open_decisions: u32,
    with_quota: bool,
) -> Result<RollupMetrics, OpsError> {
    let events: Option<Vec<Event>> = if with_quota {
        Some(
            store
                .events_for(task.id)?
                .into_iter()
                .map(|(_, e)| e)
                .collect(),
        )
    } else {
        None
    };
    let facts = node_facts(store, task, open_decisions, events.as_deref())?;
    Ok(task_core::tree_metrics::node_metrics(&facts))
}

/// 未回答の決定の数（出した節点ごと。全木）。
pub fn open_decision_counts(store: &dyn TaskStore) -> Result<BTreeMap<TaskId, u32>, OpsError> {
    Ok(open_decisions_by_task(&store.decisions_list(None)?).0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::report::ReportStore;
    use task_core::{
        Budget, ParentUnit, RunIndexRole, RunIndexStatus, RunRow, SqliteStore, TaskKind, Tier,
        TreeInfo, Usage, WorkUnitContext, WorkUnitSpec, WorkerHint, WorkspaceSpec,
    };
    use time::OffsetDateTime;

    fn task(title: &str, status: Status, parent: Option<(&Task, &str)>) -> Task {
        let now = OffsetDateTime::now_utc();
        Task {
            tree: parent.map(|(p, unit)| TreeInfo {
                root_id: task_core::tree::root_id_of(p),
                depth: task_core::tree::depth_of(p) + 1,
                parent_unit: Some(ParentUnit {
                    task_id: p.id,
                    plan_id: "p".into(),
                    unit_key: unit.into(),
                    stage: "s1".into(),
                    attempt: 1,
                }),
                base_commit: None,
            }),
            routing: None,
            mode: Default::default(),
            skills: vec![],
            repos: vec![],
            id: TaskId::new(),
            parent_id: parent.map(|(p, _)| p.id),
            kind: TaskKind::Execute,
            title: title.into(),
            objective: "o".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status,
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

    fn task_row(key: &str, status: WorkUnitStatus, child: Option<TaskId>, seq: u32) -> WorkUnitRow {
        let mut row = WorkUnitRow::new(
            format!("id-{key}"),
            "t".into(),
            "p".into(),
            seq,
            WorkUnitSpec {
                key: key.into(),
                kind: WorkUnitKind::Task,
                title: format!("unit {key}"),
                objective: "o".into(),
                depends_on: vec![],
                done_when: vec![],
                checks: vec![],
                context: WorkUnitContext::default(),
                harness: None,
                features: None,
                budget: None,
                outputs: vec![],
                phase: Some("s1".into()),
            },
            status,
            "2026-09-29T00:00:00Z".into(),
        );
        row.phase = Some("s1".into());
        row.child_task_id = child.map(|c| c.to_string());
        row
    }

    fn run(store: &SqliteStore, t: &Task, id: &str, role: RunIndexRole, cost: Option<f64>) {
        store
            .run_index_start(RunRow {
                run_id: id.into(),
                task_id: t.id.to_string(),
                work_unit_id: None,
                role,
                seq: 1,
                status: RunIndexStatus::Completed,
                adapter: None,
                model: None,
                account: None,
                session_id: None,
                checkpoint: None,
                usage: Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(1),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: cost,
                }),
                metrics: None,
                started_at: "2026-09-29T10:00:00Z".into(),
                finished_at: Some("2026-09-29T10:05:00Z".into()),
            })
            .unwrap();
    }

    /// D11 / §7 R4a (c): 段階の子の要約は、子の subtree（孫を含む）の run・reviewer の run・定価（不完全の印）と、
    /// 子の最新の報告の見出しを 1 行にする。まだ作られていない unit は「未作成」。別の段階・superseded の unit は出さない。
    #[test]
    fn stage_child_summaries_sum_the_child_subtree_and_quote_its_latest_report() {
        let store = SqliteStore::open_in_memory().unwrap();
        let root = task("root", Status::Ready, None);
        store.insert(&root).unwrap();
        let child = task("Phase 2", Status::Done, Some((&root, "p2")));
        store.insert(&child).unwrap();
        let grandchild = task("P2-A", Status::Done, Some((&child, "p2a")));
        store.insert(&grandchild).unwrap();
        run(&store, &child, "c1", RunIndexRole::Worker, Some(0.40));
        run(&store, &grandchild, "g1", RunIndexRole::Worker, None);
        run(
            &store,
            &grandchild,
            "g2",
            RunIndexRole::Reviewer,
            Some(0.10),
        );
        for (headline, at) in [
            ("古い見出し", "2026-09-29T10:00:00Z"),
            ("Phase 2 を終えました", "2026-09-29T11:00:00Z"),
        ] {
            store
                .report_append(&task_core::report::Report {
                    id: task_core::report::ReportId::new(),
                    project_id: None,
                    node_id: "dev".into(),
                    task_id: Some(child.id),
                    kind: task_core::report::ReportKind::Result,
                    level: 1,
                    headline: headline.into(),
                    body: String::new(),
                    sources: vec![],
                    read_at: None,
                    created_at: OffsetDateTime::parse(
                        at,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        let mut other_stage = task_row("x", WorkUnitStatus::Done, None, 2);
        other_stage.phase = Some("s2".into());
        let units = vec![
            task_row("p2", WorkUnitStatus::Done, Some(child.id), 0),
            task_row("p3", WorkUnitStatus::Pending, None, 1),
            other_stage,
            task_row("gone", WorkUnitStatus::Superseded, None, 3),
        ];
        let lines = stage_child_summaries(&store, &units, "s1").unwrap();
        assert_eq!(
            lines,
            vec![
                "p2 Phase 2: done / 2 run（reviewer 1）/ $0.50（不完全） — Phase 2 を終えました"
                    .to_string(),
                "p3 unit p3: 未作成（pending）".to_string(),
            ]
        );
    }
}
