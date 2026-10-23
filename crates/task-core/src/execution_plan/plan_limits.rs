//! Planner-size projection for /3 replans. Full plans remain authoritative for execution.

use super::validation::is_done_carry_over;
use super::*;

/// Stages consuming the per-plan budget: empty stages and stages with unfinished units.
/// Use the same rule in validation, adoption holds, and approval facts.
pub fn counted_plan_stages<'a>(
    spec: &'a ExecutionPlanSpec,
    done_keys: &BTreeSet<&str>,
) -> Vec<&'a StageSpec> {
    spec.stages
        .iter()
        .filter(|stage| {
            !spec.units.iter().any(|u| u.stage == stage.key)
                || spec
                    .units
                    .iter()
                    .any(|u| u.stage == stage.key && !done_keys.contains(u.key.as_str()))
        })
        .collect()
}

// Size-only projection. Never use this to validate references or materialize a plan.
pub(super) fn planner_portion_v3(
    spec: &ExecutionPlanSpec,
    done: &[(String, WorkUnitSpec)],
) -> ExecutionPlanSpec {
    let mut portion = spec.clone();
    portion.units.retain(|u| !is_done_carry_over(u, done));
    let carried_keys = spec
        .units
        .iter()
        .filter(|u| is_done_carry_over(u, done))
        .map(|u| u.key.as_str())
        .collect();
    portion.stages = counted_plan_stages(spec, &carried_keys)
        .into_iter()
        .cloned()
        .collect();
    // Root decisions required solely by carried units are historical too. A decision used by
    // live work or with no historical reference remains part of the planner budget.
    portion.decisions.retain(|d| {
        !spec
            .units
            .iter()
            .any(|u| is_done_carry_over(u, done) && u.needs_decisions.contains(&d.key))
            || portion
                .units
                .iter()
                .any(|u| u.needs_decisions.contains(&d.key))
            || d.needed_before.iter().any(|target| {
                portion.units.iter().any(|u| &u.key == target)
                    || target
                        .strip_prefix(crate::decision::NEEDED_BEFORE_STAGE_PREFIX)
                        .is_some_and(|key| portion.stages.iter().any(|s| s.key == key))
            })
    });
    portion
}
