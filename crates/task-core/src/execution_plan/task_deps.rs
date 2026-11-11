//! ADR 2026-11-11-unit-task-dependencies D1・D4: /3 の計画の unit の `task:<task id>` 依存の書き方と、採用前の検証（純粋関数）。
//!
//! 事実（依存先・自分の木・待つ辺）を読むのは `task_ops::unit_task_deps::plan_errors`。満たされたかの判断は
//! `wait::task_deps`。`validate_v3` は `task:` の依存を段の規則に数えず、ここの [`task_dependency_errors`] が見る。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{EXECUTION_PLAN_SCHEMA_V3, ExecutionPlanSpec, WorkUnitRow};
use crate::model::TaskId;
use crate::org::ProjectId;

/// D1: unit の `depends_on` で木の外の task を指す接頭辞（`task:<task id>`）。
pub const TASK_DEP_PREFIX: &str = "task:";

/// `task:` で始まる依存か（id の形は問わない。形は [`task_dependency_id`]）。`validate_v3` はこの依存を段の規則に数えない。
pub fn is_task_dependency(dep: &str) -> bool {
    dep.starts_with(TASK_DEP_PREFIX)
}

/// `task:<id>` の id。接頭辞が無い・id が ULID でなければ `None`。
pub fn task_dependency_id(dep: &str) -> Option<TaskId> {
    dep.strip_prefix(TASK_DEP_PREFIX)?.parse().ok()
}

/// `id` を指す依存の文字列（`task:<id>`。unit の `depends_on` と解決の `met` の要素）。
pub fn task_dependency_key(id: TaskId) -> String {
    format!("{TASK_DEP_PREFIX}{id}")
}

/// unit の行が待つ `task:` 依存の id（形の悪いものは飛ばす）。
pub fn unit_task_dependencies(unit: &WorkUnitRow) -> impl Iterator<Item = TaskId> + '_ {
    unit.depends_on.iter().filter_map(|d| task_dependency_id(d))
}

/// /3 の計画の unit が書いた `task:` 依存（unit key, 依存の文字列, id）。id の形が違えば `None`。/1・/2 は空
/// （`task:` は未知の依存として `validate` が拒む）。
pub fn plan_task_dependencies(spec: &ExecutionPlanSpec) -> Vec<(String, String, Option<TaskId>)> {
    if spec.schema != EXECUTION_PLAN_SCHEMA_V3 {
        return Vec::new();
    }
    spec.units
        .iter()
        .flat_map(|u| {
            u.depends_on
                .iter()
                .filter(|d| is_task_dependency(d))
                .map(|d| (u.key.clone(), d.clone(), task_dependency_id(d)))
        })
        .collect()
}

/// D4 の検証に使う DB の事実（`task_ops::unit_task_deps::plan_errors` が集める）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskDependencyContext {
    /// 計画を持つ task の案件。
    pub project: Option<ProjectId>,
    /// 計画を持つ task の木（自分・祖先〈`parent_id` と木の親を辿る。委譲の親を含む〉・子孫〈委譲の子と木の子〉・
    /// 自分と祖先が属する木の節点）。委譲の兄弟は入らない。
    pub own_tree: BTreeSet<TaskId>,
    /// 見つかった依存先 → その案件。見つからない id は入らない。
    pub targets: BTreeMap<TaskId, Option<ProjectId>>,
    /// 依存先から辿れる「待つ」辺（task → その task が待つ task。`depends_on`、委譲の子と木の子〈親は子を待つ〉、
    /// 有効な計画の unit の `task:` 依存と kind task の unit の子）。
    pub waits_on: BTreeMap<TaskId, Vec<TaskId>>,
}

/// D1・D4: `task:` 依存を拒む理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskDependencyRejection {
    /// `task:` の後が task id（26 文字の ULID）でない。
    Malformed,
    /// その task が無い。
    NotFound,
    /// 自分の木（自分・祖先〈委譲の親を含む〉・子孫・同じ木の節点）の task。祖先は子孫を待つので必ず循環し、
    /// 木の中の順序は unit の key で書く。
    InOwnTree,
    /// 別の案件の task（計画を持つ task が案件に属さないときも）。
    OtherProject,
    /// その task が（`depends_on`・子・計画の `task:` 依存を辿って）この task の木を待つ。
    Cycle,
}

/// D4: 拒んだ `task:` 依存 1 件（`PlanValidationError` と同じく planner・人に理由として返す文を持つ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependencyError {
    pub key: String,
    pub depends_on: String,
    pub reason: TaskDependencyRejection,
}

impl std::fmt::Display for TaskDependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.reason {
            TaskDependencyRejection::Malformed => {
                "must be \"task:<task id>\" with a 26-character task id（task:<task id> の形でない）"
            }
            TaskDependencyRejection::NotFound => "no such task（依存先の task が無い）",
            TaskDependencyRejection::InOwnTree => {
                "points at this task, an ancestor (including the delegating parent), a descendant or a node of its tree; ancestors wait for this task, and order inside a tree is written with unit keys（自分・祖先・子孫・同じ木の task は待てない）"
            }
            TaskDependencyRejection::OtherProject => {
                "is not a task of this task's project（別の案件の task は待てない）"
            }
            TaskDependencyRejection::Cycle => {
                "would wait in a cycle: that task waits for this task's tree（循環する）"
            }
        };
        write!(
            f,
            "unit {}: depends_on {:?} {why}. ADR 2026-11-11-unit-task-dependencies",
            self.key, self.depends_on
        )
    }
}

/// D1・D4: 計画の `task:` 依存の検証（形・存在・自分の木の外・同じ案件・循環なし）。依存ごとに最初に当たった理由を 1 件返す。
pub fn task_dependency_errors(
    spec: &ExecutionPlanSpec,
    ctx: &TaskDependencyContext,
) -> Vec<TaskDependencyError> {
    let mut errors = Vec::new();
    for (key, depends_on, id) in plan_task_dependencies(spec) {
        let reason = match id {
            None => Some(TaskDependencyRejection::Malformed),
            Some(id) => rejection(id, ctx),
        };
        if let Some(reason) = reason {
            errors.push(TaskDependencyError {
                key,
                depends_on,
                reason,
            });
        }
    }
    errors
}

fn rejection(id: TaskId, ctx: &TaskDependencyContext) -> Option<TaskDependencyRejection> {
    let Some(project) = ctx.targets.get(&id) else {
        return Some(TaskDependencyRejection::NotFound);
    };
    if ctx.own_tree.contains(&id) {
        return Some(TaskDependencyRejection::InOwnTree);
    }
    if ctx.project.is_none() || *project != ctx.project {
        return Some(TaskDependencyRejection::OtherProject);
    }
    reaches(&ctx.waits_on, id, &ctx.own_tree).then_some(TaskDependencyRejection::Cycle)
}

/// `start` から待つ辺を辿って `targets` のどれかに着くか（訪問済みは辿らない）。
fn reaches(
    edges: &BTreeMap<TaskId, Vec<TaskId>>,
    start: TaskId,
    targets: &BTreeSet<TaskId>,
) -> bool {
    let mut seen = BTreeSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        for next in edges.get(&id).into_iter().flatten() {
            if targets.contains(next) {
                return true;
            }
            if seen.insert(*next) {
                queue.push_back(*next);
            }
        }
    }
    false
}
