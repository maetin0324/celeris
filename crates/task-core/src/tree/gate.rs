//! Unit classification and conversion.

use super::*;

// ---------------------------------------------------------------------------
// ADR-0079 D4 (3)（Phase R2a）: unit の gate（計画の採用のときの最終判断）
// ---------------------------------------------------------------------------

/// D4 (3): unit の gate の入力のうち、計画と親から決まるもの。
#[derive(Debug, Clone, Copy)]
pub struct UnitGateContext<'a> {
    /// 計画を持つ task（親）。
    pub parent: &'a Task,
    /// 親の深さ（task の層数。unit の深さは `parent_depth + 1`）。
    pub parent_depth: u32,
    /// 親の実効の repos の名前（親が持たなければ案件の primary。`task_ops::tree::parent_repo_names`）。
    pub parent_repo_names: &'a [String],
    pub limits: &'a TreeLimits,
    /// leaf の 1 run の上限（`ExecutionLimits.work_unit_max_turns` / `work_unit_max_wall_secs`）。
    pub work_unit_max_turns: u32,
    pub work_unit_max_wall_secs: u64,
    // R6-2: R5b-fix3 の `human_plan`（人の計画か）は消した。kind task の unit の手掛かりは計画の書き手に
    // よらず明示（`task_unit_execution_hint`）。
}

/// D4 (3): 1 つの unit の gate の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitGate {
    pub unit_key: String,
    pub declared: UnitDeclared,
    /// unit の view にかけた gate（深さ `parent_depth + 1` の閾値。`shadow = false`）。
    pub decision: crate::execution_gate::ExecutionGateDecision,
    /// unit の深さ（`parent_depth + 1`）。
    pub depth: u32,
    pub threshold: u32,
    /// 宣言と食い違ったときの行動（一致なら `None`）。
    pub action: Option<UnitGateAction>,
    /// 判断の理由（人が読む 1 行。`UnitGateOverridden.reason`）。
    pub reason: String,
}

/// D4 (3) / ADR-0072 D21: unit の view（親の Task を複製し、題名・目的・受け入れ・予算・genre を unit の
/// spec に差し替えたもの）。gate の特徴量の推定と規則表にだけ使う（保存しない）。
///
/// - leaf: 受け入れは `done_when`（reviewer）と `checks`（command）、予算は unit の `budget`（無ければ
///   ADR-0072 D18 の既定 `max(親, 30 turns / 1,800 秒)`）、genre は `harness`（無ければ親）。
/// - kind task: 受け入れは unit の `acceptance`、予算は子 task が受け取る予算（[`tree_child_budget`] =
///   `max(親, 30 turns / 1,800 秒)`。R5b-fix3）、genre は unit（無ければ親）。
/// - `routing` は unit の `features` のヒントだけ（親のヒント・人の明示・CoS のヒントは継がない）。ただし
///   kind task の unit は子 task が受け取る `execution_hint`（[`task_unit_execution_hint`]。unit の `gate`、
///   無ければ明示の compound。R6-2）を持つ（[`unit_gate`] はこの値を人の明示として判定に渡す）。
///   印（labels）は持たない（対話・support-task の判定に当たらないように）。
pub fn unit_view(parent: &Task, unit: &crate::execution_plan::PlanUnitSpec) -> Task {
    use crate::model::{Check, Criterion};
    let mut view = parent.clone();
    view.title = unit.title.clone();
    view.objective = unit.objective.clone();
    view.labels = Vec::new();
    view.kind = crate::model::TaskKind::Execute;
    view.tree = None;
    let hints = unit
        .features
        .clone()
        .and_then(|v| serde_json::from_value::<crate::model_policy::TaskFeatureHints>(v).ok())
        .filter(|h| !h.is_empty());
    view.routing = Some(crate::model::TaskRouting {
        features: hints,
        execution_hint: unit.is_task().then(|| task_unit_execution_hint(unit.gate)),
        ..Default::default()
    });
    if unit.is_task() {
        view.acceptance = unit.acceptance.clone();
        view.genre = unit.genre.clone().or_else(|| parent.genre.clone());
        // R5b-fix3: 子 task が実際に受け取る予算（`tree_child_budget`）で見る。
        view.budget = tree_child_budget(&parent.budget);
    } else {
        let mut acceptance: Vec<Criterion> = unit
            .done_when
            .iter()
            .map(|t| Criterion {
                text: t.clone(),
                check: Check::Reviewer,
            })
            .collect();
        acceptance.extend(unit.checks.iter().map(|c| Criterion {
            text: c.cmd.clone(),
            check: Check::Command {
                cmd: c.cmd.clone(),
                expect_exit: c.expect_exit,
            },
        }));
        view.acceptance = acceptance;
        view.genre = unit.harness.clone().or_else(|| parent.genre.clone());
        let budget = unit.budget.unwrap_or_default();
        view.budget.max_turns = budget
            .max_turns
            .unwrap_or_else(|| parent.budget.max_turns.max(30));
        view.budget.max_wall_secs = budget
            .max_wall_secs
            .unwrap_or_else(|| parent.budget.max_wall_secs.max(1800));
    }
    view
}

/// D4 (3) の表の 5 行目: kind task の unit を task のまま残す構造上の理由（人の acceptance・決定を待つ・
/// 既存の task の採用・親と違う genre / repos / 部署〈親の skills に無い skill〉）。空なら理由なし。
/// `needs_decisions` は実効の値（`effective_needs_decisions`）。
pub fn structural_reasons(
    parent: &Task,
    parent_repo_names: &[String],
    unit: &crate::execution_plan::PlanUnitSpec,
    needs_decisions: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    if unit
        .acceptance
        .iter()
        .any(|c| c.check == crate::model::Check::Human)
    {
        out.push("human acceptance".to_string());
    }
    if !needs_decisions.is_empty() {
        out.push(format!("needs decisions [{}]", needs_decisions.join(", ")));
    }
    if let Some(adopt) = unit.adopt {
        out.push(format!("adopts task {adopt}"));
    }
    if let Some(genre) = unit.genre.as_deref()
        && parent.genre.as_deref() != Some(genre)
    {
        out.push(format!(
            "genre {genre} differs from the parent's {}",
            parent.genre.as_deref().unwrap_or("(none)")
        ));
    }
    if !unit.repos.is_empty() {
        let unit_set: std::collections::BTreeSet<&str> =
            unit.repos.iter().map(String::as_str).collect();
        let parent_set: std::collections::BTreeSet<&str> =
            parent_repo_names.iter().map(String::as_str).collect();
        if unit_set != parent_set {
            out.push(format!(
                "repos [{}] differ from the parent's [{}]",
                unit.repos.join(", "),
                parent_repo_names.join(", ")
            ));
        }
    }
    let foreign: Vec<&str> = unit
        .skills
        .iter()
        .filter(|s| !parent.skills.iter().any(|p| p == *s))
        .map(String::as_str)
        .collect();
    if !foreign.is_empty() {
        out.push(format!(
            "skills [{}] beyond the parent's (another department)",
            foreign.join(", ")
        ));
    }
    out
}

/// D4 (2) の leaf の基準のうち、kind task の unit が満たさないもの（空なら leaf に下げられる）。
/// (a) 1 run: 子が継ぐ予算（親の予算）が leaf の上限以内、(b) リポジトリは高々 1、(c) 機械的な検査
/// （`Check::Command` の acceptance）が 1 本以上。gate の atomic は呼び出し側が見る。
pub fn task_unit_leaf_shortfalls(
    parent: &Task,
    unit: &crate::execution_plan::PlanUnitSpec,
    work_unit_max_turns: u32,
    work_unit_max_wall_secs: u64,
) -> Vec<String> {
    let mut out = Vec::new();
    let inherited = tree_child_budget(&parent.budget);
    if inherited.max_turns > work_unit_max_turns
        || inherited.max_wall_secs > work_unit_max_wall_secs
    {
        out.push(format!(
            "the inherited budget ({} turns / {} s) exceeds one run ({work_unit_max_turns} / {work_unit_max_wall_secs})",
            inherited.max_turns, inherited.max_wall_secs
        ));
    }
    if unit.repos.len() > 1 {
        out.push(format!("{} repos", unit.repos.len()));
    }
    if !unit
        .acceptance
        .iter()
        .any(|c| matches!(c.check, crate::model::Check::Command { .. }))
    {
        out.push("no command check".to_string());
    }
    out
}

/// ADR-0079「R6-2」（R5b-fix3 を改める）: kind task の unit から作る子 task の `routing.execution_hint`（unit の
/// gate も同じ値を使う）。**計画の書き手（人・planner）によらず明示**（`explicit = true`、gate は
/// `human/explicit`）で、`mode` は unit の `gate`（無ければ `compound`: kind task を選んだこと自体が「自分の計画が
/// 要る」の意味）。1 run で済む子を望む書き手は `gate: atomic` を書く。
pub fn task_unit_execution_hint(
    gate: Option<crate::execution_gate::ExecutionMode>,
) -> crate::execution_gate::ExecutionHintSpec {
    crate::execution_gate::ExecutionHintSpec {
        mode: gate.unwrap_or(crate::execution_gate::ExecutionMode::Compound),
        explicit: true,
    }
}

/// D4 (3): unit の gate（純粋関数）。view に深さ `parent_depth + 1` の閾値で gate をかけ、planner の
/// 宣言と照らして D4 (3) の表の行を決める（表の 7 行目〈子 task を持てない深さの task〉は計画の検証の
/// `ChildTaskTooDeep` が先に拾う）。
pub fn unit_gate(
    ctx: &UnitGateContext<'_>,
    unit: &crate::execution_plan::PlanUnitSpec,
    // R6-2: kind task の unit を task のまま残す構造上の理由（`structural_reasons`）は、kind task の gate が明示に
    // なったので見なくなった（引数は呼び出し側の形のために残す）。
    _needs_decisions: &[String],
) -> UnitGate {
    use crate::execution_gate::{ExecutionMode, GateThreshold};
    let depth = ctx.parent_depth.saturating_add(1);
    let at = GateThreshold::at_depth(depth, ctx.limits.gate_depth_step);
    let threshold = gate_threshold(depth, ctx.limits.gate_depth_step);
    let view = unit_view(ctx.parent, unit);
    let hints = view.routing.as_ref().and_then(|r| r.features);
    let (features, _) = crate::model_policy::TaskFeatures::infer_with_hints(&view, hints.as_ref());
    // R6-2: kind task の unit は計画の書き手の明示（unit の `gate`、無ければ compound。子 task の
    // `routing.execution_hint` と同じ値で、`unit_view` が持つ）。leaf の unit には何も足さない。
    let hint = view.routing.as_ref().and_then(|r| r.execution_hint);
    let decision = crate::execution_gate::decide_at(
        &view,
        &features,
        hint.filter(|h| h.explicit).map(|h| h.mode),
        hint.is_some_and(|h| !h.explicit && h.mode == ExecutionMode::Compound),
        crate::execution_gate::ExecutionGateInputs::default(),
        false,
        at,
    );
    let gate_note = format!(
        "{} (score {} / threshold {threshold})",
        decision.rule_id, decision.score
    );
    let (declared, action, reason) = if unit.is_task() {
        // R6-2: kind task の unit の gate は書き手の明示（`gate`、無ければ compound）なので上書きしない
        // （`gate: atomic` の子は計画を持たない 1 つの節点として走る。task のまま残す）。下げる〈`Demoted`〉・
        // task のまま残す〈`KeptTask`〉の記録は出ない（`UnitGateOverridden` は leaf の暗黙の判定だけ）。
        // 固定パイプラインの genre（`out_of_scope_rule`）の atomic も同じく子 task のまま（子の dispatch の
        // gate が同じ規則で atomic にする）。
        (UnitDeclared::Task, None, gate_note)
    } else {
        match decision.mode {
            ExecutionMode::Atomic => (UnitDeclared::Leaf, None, gate_note),
            ExecutionMode::Compound => {
                if can_have_child_tasks(ctx.parent_depth, ctx.limits.max_depth) {
                    (
                        UnitDeclared::Leaf,
                        Some(UnitGateAction::Promoted),
                        format!("{gate_note}; promoted to a child task"),
                    )
                } else if ctx.limits.auto_leaf {
                    (
                        UnitDeclared::Leaf,
                        Some(UnitGateAction::AutoLeaf),
                        format!(
                            "{gate_note}; gate is compound but depth {} reached max_depth {}: executing as leaf automatically",
                            ctx.parent_depth, ctx.limits.max_depth
                        ),
                    )
                } else {
                    (
                        UnitDeclared::Leaf,
                        Some(UnitGateAction::Decision),
                        format!(
                            "{gate_note}; a task at depth {} cannot have child tasks (max_depth {}): asking a human",
                            ctx.parent_depth, ctx.limits.max_depth
                        ),
                    )
                }
            }
        }
    };
    UnitGate {
        unit_key: unit.key.clone(),
        declared,
        decision,
        depth,
        threshold,
        action,
        reason,
    }
}

/// D4 (3) の「task に上げる」: leaf の spec を kind task の spec にする。`checks` は command の acceptance
/// （leaf は `checks` を 1 本以上持つ）、`done_when` と `context.paths` は目的の末尾に固定の書式で移し
/// （仕事を落とさない。reviewer の acceptance にはしない — 子の最終レビューに reviewer run を足さない。U-R7）、
/// `context.repo` は `repos`。leaf 専用の欄（`checks` / `budget` / `harness` / `context.paths`）は消す。
pub fn promote_to_task(
    unit: &crate::execution_plan::PlanUnitSpec,
) -> crate::execution_plan::PlanUnitSpec {
    use crate::execution_plan::RepoSelector;
    use crate::model::{Check, Criterion};
    let mut out = unit.clone();
    out.kind = crate::execution_plan::WorkUnitKind::Task;
    // ADR-0074 付記 2026-10-05: 範囲 check（`scope: true`）は WU の作業時だけで意味を持つので acceptance に写さない
    // （子 task の final review で他の変更を拾って必ず落ちる）。子 task 自身の leaf が範囲 check を持つ。
    let mut acceptance: Vec<Criterion> = unit
        .checks
        .iter()
        .filter(|c| !c.scope)
        .map(|c| Criterion {
            text: format!("`{}` exits {}", c.cmd, c.expect_exit),
            check: Check::Command {
                cmd: c.cmd.clone(),
                expect_exit: c.expect_exit,
            },
        })
        .collect();
    if acceptance.is_empty() {
        acceptance = unit
            .done_when
            .iter()
            .map(|t| Criterion {
                text: t.clone(),
                check: Check::Reviewer,
            })
            .collect();
    }
    if acceptance.is_empty() {
        acceptance.push(Criterion {
            text: unit.title.clone(),
            check: Check::Reviewer,
        });
    }
    out.acceptance = acceptance;
    let mut objective = unit.objective.trim_end().to_string();
    if !unit.done_when.is_empty() {
        objective.push_str(&format!(
            "\n\n{PROMOTED_DONE_WHEN_HEADING}\n{}",
            unit.done_when
                .iter()
                .map(|d| format!("- {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !unit.context.paths.is_empty() {
        objective.push_str(&format!(
            "\n\n{PROMOTED_PATHS_HEADING} {}",
            unit.context.paths.join(", ")
        ));
    }
    out.objective = objective;
    out.done_when = Vec::new();
    out.checks = Vec::new();
    out.budget = None;
    out.harness = None;
    out.repos = match &unit.context.repo {
        Some(RepoSelector::One(r)) => vec![r.clone()],
        Some(RepoSelector::Many(v)) => v.clone(),
        None => Vec::new(),
    };
    out.context.paths = Vec::new();
    out.context.repo = None;
    out
}

/// [`promote_to_task`] が目的の末尾に足す見出し（固定）。
pub const PROMOTED_DONE_WHEN_HEADING: &str =
    "## 完了の条件（計画の leaf から引き継ぎ。ADR-0079 D4 (3)）";
/// [`promote_to_task`] が目的の末尾に足す対象のパスの前置き（固定）。
pub const PROMOTED_PATHS_HEADING: &str = "対象のパス（計画の leaf から引き継ぎ）:";

/// D4 (3) の「leaf に下げる」: kind task の spec を leaf の spec（kind = 段階の kind）にする。acceptance の
/// 文は `done_when`、command の検査は `checks`、`repos`（高々 1）は `context.repo`。kind task 専用の欄
/// （`acceptance` / `genre` / `skills` / `repos` / `adopt` / `gate`）は消す（構造上の理由が無いことを確かめてから
/// 呼ぶので、genre は親と同じか無く、skills は親の部分集合）。
pub fn demote_to_leaf(
    unit: &crate::execution_plan::PlanUnitSpec,
    stage_kind: crate::execution_plan::WorkUnitKind,
) -> crate::execution_plan::PlanUnitSpec {
    use crate::execution_plan::{RepoSelector, WorkUnitCheck};
    let mut out = unit.clone();
    out.kind = stage_kind;
    let mut done_when = unit.done_when.clone();
    done_when.extend(unit.acceptance.iter().map(|c| c.text.clone()));
    out.done_when = done_when;
    out.checks = unit
        .acceptance
        .iter()
        .filter_map(|c| match &c.check {
            crate::model::Check::Command { cmd, expect_exit } => Some(WorkUnitCheck {
                cmd: cmd.clone(),
                expect_exit: *expect_exit,
                scope: false,
            }),
            _ => None,
        })
        .collect();
    out.context.repo = unit.repos.first().map(|r| RepoSelector::One(r.clone()));
    out.acceptance = Vec::new();
    out.genre = None;
    out.skills = Vec::new();
    out.repos = Vec::new();
    out.adopt = None;
    out.gate = None;
    out
}

/// D4 (3): 計画のすべての unit に gate をかけた結果（純粋関数）。`spec` は上げる・下げるを当てた計画、
/// `gates` は unit ごとの結果（計画の順）、`leaf_too_large` は決定の要求にする unit（`spec` では leaf の
/// まま）。/3 以外は何もしない（`gates` は空）。
#[derive(Debug, Clone)]
pub struct UnitGateReport {
    pub spec: crate::execution_plan::ExecutionPlanSpec,
    pub gates: Vec<UnitGate>,
    pub leaf_too_large: Vec<String>,
}

impl UnitGateReport {
    /// 宣言と食い違った unit（`UnitGateOverridden` を出すもの）。
    pub fn overridden(&self) -> impl Iterator<Item = &UnitGate> {
        self.gates.iter().filter(|g| g.action.is_some())
    }
}

/// D4 (3)（Phase R2a）: 計画の各 unit に [`unit_gate`] をかけ、上げる・下げるを spec に当てる（純粋関数）。
/// `skip` の unit（replan で持ち越す done の unit）は gate をかけず spec も変えない。
pub fn apply_unit_gates(
    ctx: &UnitGateContext<'_>,
    spec: &crate::execution_plan::ExecutionPlanSpec,
    skip: &std::collections::BTreeSet<String>,
) -> UnitGateReport {
    let mut out = spec.clone();
    let mut gates = Vec::new();
    let mut leaf_too_large = Vec::new();
    if spec.schema != crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3 {
        return UnitGateReport {
            spec: out,
            gates,
            leaf_too_large,
        };
    }
    for (i, unit) in spec.units.iter().enumerate() {
        if skip.contains(&unit.key) {
            continue;
        }
        let needs = crate::execution_plan::effective_needs_decisions(spec, &unit.key);
        let gate = unit_gate(ctx, unit, &needs);
        match gate.action {
            Some(UnitGateAction::Promoted) => out.units[i] = promote_to_task(unit),
            Some(UnitGateAction::Demoted) => {
                let stage_kind = spec
                    .stages
                    .iter()
                    .find(|s| s.key == unit.stage)
                    .map(|s| s.kind)
                    .unwrap_or(crate::execution_plan::WorkUnitKind::Implement);
                out.units[i] = demote_to_leaf(unit, stage_kind);
            }
            Some(UnitGateAction::Decision) => leaf_too_large.push(unit.key.clone()),
            Some(UnitGateAction::KeptTask | UnitGateAction::AutoLeaf) | None => {}
        }
        gates.push(gate);
    }
    UnitGateReport {
        spec: out,
        gates,
        leaf_too_large,
    }
}
