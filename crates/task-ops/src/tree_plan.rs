//! ADR-0079 D3 / D4 (3) / D7（Phase R2a〜R3a、R5b-prep で dispatcher から切り出し）: /3 の計画を採用するときの
//! unit の gate（上げる・下げる）と、採用の直後の止め（`UnitGateOverridden`・`leaf_too_large` / `limit` の決定の
//! 要求・計画の決定の要求・答えの無い決定を待つ leaf の `blocked(decision)`）。
//!
//! planner の計画（dispatcher）と人の計画（`PUT /tasks/{id}/execution-plan`、`celerisctl execution plan set`）が
//! **同じ関数**を通る（R5b-prep）。判断は `task_core::tree` の純粋関数、ここは store の読み取りと書く行・event の
//! 組み立てだけ（LLM なし）。

use std::collections::BTreeSet;

use task_core::execution_plan::{PlanContext, PlanValidationError, ValidatedPlan, validate_with};
use task_core::{
    DecisionOrigin, DecisionRaisedBy, Event, ExecutionLimits, ExecutionPlanRow, PlanOrigin, Task,
    TaskStore, WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkUnitStatus,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::OpsError;

/// ADR-0079 D3（Phase R2a）: 計画の上限（段階の数・段階あたりの unit・子 task・`max_depth`）だけの違反か。
pub fn is_tree_plan_limit_error(e: &PlanValidationError) -> bool {
    use PlanValidationError as E;
    matches!(
        e,
        E::TooManyStages { .. }
            | E::TooManyUnitsInStage { .. }
            | E::TooManyChildTasks { .. }
            | E::ChildTaskTooDeep { .. }
    )
}

/// ADR-0079 D3（Phase R2a）: 計画の上限（[`is_tree_plan_limit_error`] の 4 つ）だけを外した上限。採用に
/// 使い、超えた分は `task_core::tree::plan_limit_holds` が元の上限で選んで止める。
pub fn relaxed_tree_plan_limits(limits: ExecutionLimits) -> ExecutionLimits {
    let mut relaxed = limits;
    relaxed.tree.max_stages = usize::MAX;
    relaxed.tree.max_units_per_stage = usize::MAX;
    relaxed.tree.max_child_tasks_per_plan = usize::MAX;
    relaxed.tree.max_depth = u32::MAX;
    relaxed
}

/// ADR-0079 D4 (3) / D3（Phase R2a）: 採用する /3 の計画の unit の gate の結果と、採用の直後に止める
/// unit の束（[`plan_hold_writes`] が行と event にする）。
#[derive(Debug, Clone)]
pub struct TreePlanOutcome {
    pub report: task_core::UnitGateReport,
    pub holds: Vec<task_core::LimitHold>,
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

/// ADR-0079 D4 (3) / D3（Phase R2a）: 採用する /3 の計画に unit の gate をかけ（上げる・下げるを spec に
/// 当てる）、採用の直後に止める unit（子 task にできない compound な leaf、上限を超える unit）を決める。
/// /3 でない・木が無効なら何もしない（`None`）。上げる・下げるで計画の上限を超えうるので、spec を変えた
/// ときは `adopt_limits` を計画の上限を外したものにする（超えた分は `holds` が止める）。変えた spec が
/// 他の理由で検証に落ちれば（通常起きない）、元の spec のまま採用し、上げる・下げるは当てない（警告）。
/// `limits` は daemon の実効の上限（`[execution.tree]` を含む）、`origin` は再検証の `PlanContext`。
pub fn unit_gate_plan(
    store: &dyn TaskStore,
    task: &Task,
    validated: ValidatedPlan,
    done_work_units: &[(String, WorkUnitSpec)],
    limits: ExecutionLimits,
    adopt_limits: &mut ExecutionLimits,
    origin: PlanOrigin,
) -> (ValidatedPlan, Option<TreePlanOutcome>) {
    let tree = limits.tree;
    if validated.spec.schema != task_core::EXECUTION_PLAN_SCHEMA_V3 || !tree.enabled {
        return (validated, None);
    }
    let names = crate::tree::parent_repo_names(store, task).unwrap_or_default();
    let depth = task_core::tree::depth_of(task);
    let ctx = task_core::UnitGateContext {
        parent: task,
        parent_depth: depth,
        parent_repo_names: &names,
        limits: &tree,
        work_unit_max_turns: limits.work_unit_max_turns,
        work_unit_max_wall_secs: limits.work_unit_max_wall_secs,
    };
    // replan で持ち越す done の unit は gate をかけ直さない（spec を変えない。D17 の不変条件）。
    let done_keys: BTreeSet<String> = done_work_units.iter().map(|(k, _)| k.clone()).collect();
    let mut report = task_core::tree::apply_unit_gates(&ctx, &validated.spec, &done_keys);
    let changed = report.gates.iter().any(|g| {
        matches!(
            g.action,
            Some(task_core::UnitGateAction::Promoted | task_core::UnitGateAction::Demoted)
        )
    });
    let validated = if changed {
        let relaxed = relaxed_tree_plan_limits(*adopt_limits);
        match validate_with(
            &report.spec,
            relaxed,
            done_work_units,
            PlanContext { origin, depth },
        ) {
            Ok(v) => {
                *adopt_limits = relaxed;
                v
            }
            Err(_) => {
                // 上げる・下げるを当てた spec が上限以外の理由で検証に落ちた（通常起きない）: 宣言どおりに採用する。
                for g in &mut report.gates {
                    if matches!(
                        g.action,
                        Some(
                            task_core::UnitGateAction::Promoted
                                | task_core::UnitGateAction::Demoted
                        )
                    ) {
                        g.action = None;
                    }
                }
                report.spec = validated.spec.clone();
                validated
            }
        }
    } else {
        validated
    };
    let tree_leaves = crate::tree::tree_counters(store, task_core::tree::root_id_of(task))
        .map(|c| c.leaves)
        .unwrap_or(0);
    let existing_keys: BTreeSet<String> = store
        .work_units_for(task.id)
        .map(|rows| rows.into_iter().map(|r| r.key).collect())
        .unwrap_or_default();
    let extra_held: BTreeSet<String> = report.leaf_too_large.iter().cloned().collect();
    let holds = task_core::tree::plan_limit_holds(
        &validated.spec,
        &tree,
        depth,
        tree_leaves,
        &existing_keys,
        &extra_held,
        &done_keys,
    );
    (validated, Some(TreePlanOutcome { report, holds }))
}

/// [`plan_hold_writes`] の結果: 書き換える `work_units` の行（`blocked(decision)`）と、計画を持つ task に積む event。
#[derive(Debug, Clone, Default)]
pub struct PlanHoldWrites {
    pub rows: Vec<WorkUnitRow>,
    pub events: Vec<Event>,
    /// 新しく出した計画の決定（`DecisionRequested`）の数。
    pub raised: usize,
}

/// ADR-0079 D4 (3) / D3 / D7（Phase R2a / R3a）: 採用した /3 の計画について、unit の gate の不一致
/// （`UnitGateOverridden`）と、決定の要求（`leaf_too_large` / `limit`。D7 の形、path 付き）と、計画の決定
/// （計画の `decisions` と unit の `decisions`。同じ key の行がこの節点に既にあれば出さない = replan で持ち越した
/// 決定は開き直さない。replan で計画から消えた planner / human の未回答の決定は取り下げる）と、止める unit の
/// `blocked(decision)`（答えの無い決定を待つ leaf を含む。kind task の unit は `ready` のまま子を作らずに待つ）を
/// 組み立てる。止めるのは `units` のうち `pending` / `ready` の行だけ（同じ段階の他の unit・兄弟は止めない）。
///
/// `units` は採用した計画の行（planner の経路は採用の後に store から読んだもの、人の経路は採用の前に組み立てた
/// もの。どちらも同じ規則で止める）。`run_id` は計画を書いた planner run（人の計画は `None`）、
/// `decision_origin` は計画の決定の出どころ（`Planner` / `Human`）。
#[allow(clippy::too_many_arguments)]
pub fn plan_hold_writes(
    store: &dyn TaskStore,
    task: &Task,
    plan: &ExecutionPlanRow,
    units: &[WorkUnitRow],
    run_id: Option<&str>,
    decision_origin: DecisionOrigin,
    outcome: TreePlanOutcome,
    now: OffsetDateTime,
) -> Result<PlanHoldWrites, OpsError> {
    let TreePlanOutcome { report, holds } = outcome;
    let path = crate::tree::decision_path(store, task)?;
    let raised_by = DecisionRaisedBy {
        task_id: task.id,
        run_id: run_id.map(str::to_string),
        origin: DecisionOrigin::Daemon,
    };
    let mut events: Vec<Event> = Vec::new();
    let mut rows: Vec<WorkUnitRow> = Vec::new();
    for g in report.overridden() {
        let Some(action) = g.action else { continue };
        events.push(Event::UnitGateOverridden {
            plan_id: plan.id.clone(),
            unit_key: g.unit_key.clone(),
            declared: g.declared,
            gate: g.decision.mode,
            action,
            depth: g.depth,
            threshold: g.threshold,
            score: g.decision.score,
            reason: g.reason.clone(),
        });
    }
    // 止められる行（`pending` / `ready`）だけを選ぶ。
    let holdable = |key: &str| -> Option<WorkUnitRow> {
        units
            .iter()
            .find(|u| {
                u.key == key && matches!(u.status, WorkUnitStatus::Pending | WorkUnitStatus::Ready)
            })
            .cloned()
    };
    let hold = |row: WorkUnitRow, rows: &mut Vec<WorkUnitRow>, events: &mut Vec<Event>| {
        let mut updated = row.clone();
        updated.status = WorkUnitStatus::Blocked;
        updated.blocked_reason = Some(WorkUnitBlockedReason::Decision);
        updated.updated_at = rfc3339(now);
        events.push(Event::WorkUnitTransitioned {
            work_unit_id: row.id.clone(),
            key: row.key.clone(),
            from: row.status,
            to: WorkUnitStatus::Blocked,
            reason: "decision".to_string(),
            run_id: None,
        });
        rows.push(updated);
    };
    for key in &report.leaf_too_large {
        let (Some(g), Some(row)) = (
            report.gates.iter().find(|g| &g.unit_key == key),
            holdable(key),
        ) else {
            continue;
        };
        let request = task_core::tree::leaf_too_large_decision(
            g,
            &row.spec.title,
            path.clone(),
            raised_by.clone(),
        );
        events.push(Event::DecisionRequested {
            decision: Box::new(request),
        });
        hold(row, &mut rows, &mut events);
    }
    for h in holds {
        let held: Vec<WorkUnitRow> = h.units.iter().filter_map(|k| holdable(k)).collect();
        if held.is_empty() {
            continue;
        }
        let request = task_core::tree::limit_decision(
            h.limit,
            h.scope.as_deref(),
            h.count,
            h.max,
            held.iter().map(|r| r.key.clone()).collect(),
            path.clone(),
            raised_by.clone(),
        );
        events.push(Event::DecisionRequested {
            decision: Box::new(request),
        });
        for row in held {
            hold(row, &mut rows, &mut events);
        }
    }
    let root_id = task_core::tree::root_id_of(task);
    let mine: Vec<task_core::DecisionRow> = store
        .decisions_list(Some(root_id))?
        .into_iter()
        .filter(|r| r.task_id == task.id)
        .collect();
    let plan_decisions = task_core::normalized_decisions(&plan.spec);
    let plan_keys: BTreeSet<&str> = plan_decisions.iter().map(|d| d.key.as_str()).collect();
    for r in mine.iter().filter(|r| {
        r.status == task_core::DecisionStatus::Open
            && r.kind == task_core::DecisionKind::Choice
            && matches!(
                r.request.raised_by.origin,
                DecisionOrigin::Planner | DecisionOrigin::Human
            )
            && !plan_keys.contains(r.key.as_str())
    }) {
        events.push(Event::DecisionWithdrawn {
            id: r.id.clone(),
            reason: format!(
                "replan v{}: the plan no longer asks this decision",
                plan.version
            ),
        });
    }
    let existing: BTreeSet<&str> = mine
        .iter()
        .filter(|r| r.status != task_core::DecisionStatus::Withdrawn)
        .map(|r| r.key.as_str())
        .collect();
    let answered: BTreeSet<&str> = mine
        .iter()
        .filter(|r| r.status == task_core::DecisionStatus::Answered)
        .map(|r| r.key.as_str())
        .collect();
    let mut raised = 0usize;
    for d in &plan_decisions {
        if existing.contains(d.key.as_str()) {
            continue;
        }
        let request = task_core::decision::request_from_spec(
            d,
            path.clone(),
            DecisionRaisedBy {
                task_id: task.id,
                run_id: run_id.map(str::to_string),
                origin: decision_origin,
            },
        );
        events.push(Event::DecisionRequested {
            decision: Box::new(request),
        });
        raised += 1;
    }
    let held_keys: BTreeSet<String> = rows.iter().map(|r| r.key.clone()).collect();
    let waiting: Vec<WorkUnitRow> = units
        .iter()
        .filter(|u| {
            !matches!(u.kind, WorkUnitKind::Task | WorkUnitKind::Integrate)
                && matches!(u.status, WorkUnitStatus::Pending | WorkUnitStatus::Ready)
                && !held_keys.contains(&u.key)
                && u.needs_decisions
                    .iter()
                    .any(|k| !answered.contains(k.as_str()))
        })
        .cloned()
        .collect();
    for row in waiting {
        hold(row, &mut rows, &mut events);
    }
    Ok(PlanHoldWrites {
        rows,
        events,
        raised,
    })
}
