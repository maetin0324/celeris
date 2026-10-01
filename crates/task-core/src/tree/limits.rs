//! Tree limits, counters, and limit decisions.

use super::*;

// ---------------------------------------------------------------------------
// ADR-0079 D3（Phase R2a）: 木の上限の決定的な数え上げと、超過の決定の要求
// ---------------------------------------------------------------------------

/// D3: 上限の種類（`kind: limit` の決定の key は [`TreeLimitKind::decision_key`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TreeLimitKind {
    /// 1 計画の段階の数（`max_stages`）。
    Stages,
    /// 1 段階の unit の数（`max_units_per_stage`）。
    UnitsPerStage,
    /// 1 計画の kind task の unit の数（`max_child_tasks_per_plan`）。
    ChildTasks,
    /// 子 task を持てない深さの kind task の unit（`max_depth`）。
    MaxDepth,
    /// 木の生涯の leaf の数（`max_tree_leaves`）。
    TreeLeaves,
    /// 木全体の run（reviewer を除く。`max_tree_runs`）。
    TreeRuns,
    /// 木全体で採用した replan の版（`max_tree_replans`）。
    TreeReplans,
    /// 木全体の input + output トークン（`max_tree_tokens`）。
    TreeTokens,
    /// ADR-0079 D9（Phase R2b）: 節点の replan の版（`[execution] max_replans`。節点ごとの上限）。子の失敗を
    /// 吸収する replan が節点の上限に達したら、黙って止まらずに決定の要求にする。
    NodeReplans,
}

impl TreeLimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TreeLimitKind::Stages => "max_stages",
            TreeLimitKind::UnitsPerStage => "max_units_per_stage",
            TreeLimitKind::ChildTasks => "max_child_tasks_per_plan",
            TreeLimitKind::MaxDepth => "max_depth",
            TreeLimitKind::TreeLeaves => "max_tree_leaves",
            TreeLimitKind::TreeRuns => "max_tree_runs",
            TreeLimitKind::TreeReplans => "max_tree_replans",
            TreeLimitKind::TreeTokens => "max_tree_tokens",
            TreeLimitKind::NodeReplans => "max_replans",
        }
    }

    /// `kind: limit` の決定の key（daemon が振る。`limit:<設定名>`、段階ごとなら `:<段階>` を足す）。
    pub fn decision_key(self, scope: Option<&str>) -> String {
        match scope {
            Some(s) => format!("limit:{}:{s}", self.as_str()),
            None => format!("limit:{}", self.as_str()),
        }
    }

    /// 木全体の上限（決定は木に 1 件だけ開く）か。
    pub fn is_tree_wide(self) -> bool {
        matches!(
            self,
            TreeLimitKind::TreeLeaves
                | TreeLimitKind::TreeRuns
                | TreeLimitKind::TreeReplans
                | TreeLimitKind::TreeTokens
        )
    }

    /// ADR-0079 D7（Phase R3a）: `kind: limit` の決定の key（`limit:<設定名>[:<段階>]`）から種類を引く。
    pub fn from_decision_key(key: &str) -> Option<Self> {
        let name = key.strip_prefix("limit:")?;
        let name = name.split(':').next().unwrap_or(name);
        [
            TreeLimitKind::Stages,
            TreeLimitKind::UnitsPerStage,
            TreeLimitKind::ChildTasks,
            TreeLimitKind::MaxDepth,
            TreeLimitKind::TreeLeaves,
            TreeLimitKind::TreeRuns,
            TreeLimitKind::TreeReplans,
            TreeLimitKind::TreeTokens,
            TreeLimitKind::NodeReplans,
        ]
        .into_iter()
        .find(|k| k.as_str() == name)
    }

    /// ADR-0079 D7（Phase R3a）: run を起こす時に数える上限（`tree_run_limit_hold` / 節点の replan）。
    /// これらの `raise-once` / `replan` の回答は、上限に余裕を足す（[`limit_allowance_step`]）。計画の採用時の
    /// 上限（段階・段階あたり・子 task・深さ・木の leaf）は止めた unit を進めるだけ。
    pub fn is_run_time(self) -> bool {
        matches!(
            self,
            TreeLimitKind::TreeRuns
                | TreeLimitKind::TreeReplans
                | TreeLimitKind::TreeTokens
                | TreeLimitKind::NodeReplans
        )
    }
}

/// ADR-0079 D7（Phase R3a）: run 時の上限に対する `raise-once`（と `replan`）の回答 1 件が足す余裕。
/// 設定の上限の半分（切り上げ、最低 1）。節点の replan（`max_replans`）は 1 回ずつ。
pub fn limit_allowance_step(limit: TreeLimitKind, configured_max: u64) -> u64 {
    if limit == TreeLimitKind::NodeReplans {
        return 1;
    }
    configured_max.div_ceil(2).max(1)
}

/// ADR-0079 D7（Phase R3a）: 回答で足した余裕を当てた木の上限（run 時の上限だけ。`allowances` は種類ごとの
/// 回答の件数）。
pub fn limits_with_allowances(
    limits: &TreeLimits,
    allowances: &std::collections::BTreeMap<TreeLimitKind, u32>,
) -> TreeLimits {
    let mut out = *limits;
    let grant = |kind: TreeLimitKind, max: u64| -> u64 {
        let n = u64::from(allowances.get(&kind).copied().unwrap_or(0));
        max.saturating_add(n.saturating_mul(limit_allowance_step(kind, max)))
    };
    out.max_tree_runs = u32::try_from(grant(
        TreeLimitKind::TreeRuns,
        u64::from(limits.max_tree_runs),
    ))
    .unwrap_or(u32::MAX);
    out.max_tree_replans = u32::try_from(grant(
        TreeLimitKind::TreeReplans,
        u64::from(limits.max_tree_replans),
    ))
    .unwrap_or(u32::MAX);
    out.max_tree_tokens = limits
        .max_tree_tokens
        .map(|max| grant(TreeLimitKind::TreeTokens, max));
    out
}

/// D3: 計画の採用のときに止める unit の束（1 束 = 1 件の `kind: limit` の決定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitHold {
    pub limit: TreeLimitKind,
    /// 段階ごとの上限なら段階の key。
    pub scope: Option<String>,
    /// 止める unit の key（計画の順）。決定の `needed_before`。
    pub units: Vec<String>,
    /// 上限に照らした数（計画・木の値）と上限。
    pub count: u64,
    pub max: u64,
}

/// D3（Phase R2a）: 計画の上限（段階の数・段階あたりの unit・計画あたりの子 task・`max_depth`）と木の
/// leaf の上限に照らし、超える分の unit を決定的に選ぶ（純粋関数）。上限の内に収まる unit は止めない
/// （兄弟は進む）。選び方はどれも「計画の順で上限の内に入るものを残し、残りを止める」:
///
/// - 段階の数: `max_stages` 番目より後の段階の unit すべて。
/// - 段階あたり: 段階ごとに、先頭から `max_units_per_stage` 個より後の unit。
/// - 子 task: 子を作る kind task の unit（`adopt` と `done_keys` の unit を除く。R7-2）の先頭から
///   `max_child_tasks_per_plan` 個より後。
/// - `max_depth`: `depth` の task が子 task を持てなければ、kind task の unit すべて。
/// - 木の leaf: この計画で新しく作る leaf（`existing_keys` に無い leaf）のうち、木の残り
///   （`max_tree_leaves − tree_leaves`）より後。
///
/// 前の束で止めた unit は後の束に入れない（1 つの unit は 1 件の決定で止まる）。`extra_held` は unit の
/// gate が既に止めた unit（`leaf_too_large`。数えるが止める束には入れない）。`done_keys` は replan で持ち越す
/// done の unit（子 task の数に入れない。検証と同じ。ADR-0079 R7-2。段階あたりの unit の数にも入れない。R7-3）。
pub fn plan_limit_holds(
    spec: &crate::execution_plan::ExecutionPlanSpec,
    limits: &TreeLimits,
    depth: u32,
    tree_leaves: u32,
    existing_keys: &std::collections::BTreeSet<String>,
    extra_held: &std::collections::BTreeSet<String>,
    done_keys: &std::collections::BTreeSet<String>,
) -> Vec<LimitHold> {
    let mut holds: Vec<LimitHold> = Vec::new();
    if spec.schema != crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3 {
        return holds;
    }
    let mut held: std::collections::BTreeSet<String> = extra_held.clone();
    let push = |holds: &mut Vec<LimitHold>,
                held: &mut std::collections::BTreeSet<String>,
                limit: TreeLimitKind,
                scope: Option<String>,
                candidates: Vec<String>,
                count: u64,
                max: u64| {
        let units: Vec<String> = candidates
            .into_iter()
            .filter(|k| !held.contains(k))
            .collect();
        if units.is_empty() {
            return;
        }
        held.extend(units.iter().cloned());
        holds.push(LimitHold {
            limit,
            scope,
            units,
            count,
            max,
        });
    };
    // 段階の数。
    if spec.stages.len() > limits.max_stages {
        let late: std::collections::BTreeSet<&str> = spec
            .stages
            .iter()
            .skip(limits.max_stages)
            .map(|s| s.key.as_str())
            .collect();
        let units = spec
            .units
            .iter()
            .filter(|u| late.contains(u.stage.as_str()))
            .map(|u| u.key.clone())
            .collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::Stages,
            None,
            units,
            spec.stages.len() as u64,
            limits.max_stages as u64,
        );
    }
    // 段階あたりの unit。ADR-0079 付記「R7-3」D5: 生きた unit だけ（持ち越す done の unit と `adopt` の unit を除く。
    // 検証の `TooManyUnitsInStage` と同じ）。
    for stage in &spec.stages {
        let in_stage: Vec<&crate::execution_plan::PlanUnitSpec> = spec
            .units
            .iter()
            .filter(|u| u.stage == stage.key && u.adopt.is_none() && !done_keys.contains(&u.key))
            .collect();
        if in_stage.len() > limits.max_units_per_stage {
            let units = in_stage
                .iter()
                .skip(limits.max_units_per_stage)
                .map(|u| u.key.clone())
                .collect();
            push(
                &mut holds,
                &mut held,
                TreeLimitKind::UnitsPerStage,
                Some(stage.key.clone()),
                units,
                in_stage.len() as u64,
                limits.max_units_per_stage as u64,
            );
        }
    }
    // 子 task の数と深さ。
    let task_units: Vec<&crate::execution_plan::PlanUnitSpec> =
        spec.units.iter().filter(|u| u.is_task()).collect();
    // ADR-0079 D15（Phase R5b-prep）: `adopt` の unit は子を作らないので子 task の上限に数えない（検証と同じ）。
    // ADR-0079 R7-2: 持ち越す done の unit も数えない（検証の `TooManyChildTasks` と同じ）。
    let new_children: Vec<&&crate::execution_plan::PlanUnitSpec> = task_units
        .iter()
        .filter(|u| u.creates_child() && !done_keys.contains(&u.key))
        .collect();
    if new_children.len() > limits.max_child_tasks_per_plan {
        let units = new_children
            .iter()
            .skip(limits.max_child_tasks_per_plan)
            .map(|u| u.key.clone())
            .collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::ChildTasks,
            None,
            units,
            new_children.len() as u64,
            limits.max_child_tasks_per_plan as u64,
        );
    }
    if !task_units.is_empty() && !can_have_child_tasks(depth, limits.max_depth) {
        let units = task_units.iter().map(|u| u.key.clone()).collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::MaxDepth,
            None,
            units,
            u64::from(depth) + 1,
            u64::from(limits.max_depth),
        );
    }
    // 木の leaf（この計画で新しく作るもの。既に止めた unit は数えない）。
    let new_leaves: Vec<String> = spec
        .units
        .iter()
        .filter(|u| !u.is_task() && !existing_keys.contains(&u.key) && !held.contains(&u.key))
        .map(|u| u.key.clone())
        .collect();
    let room = limits.max_tree_leaves.saturating_sub(tree_leaves) as usize;
    if new_leaves.len() > room {
        let total = u64::from(tree_leaves) + new_leaves.len() as u64;
        let units = new_leaves.into_iter().skip(room).collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::TreeLeaves,
            None,
            units,
            total,
            u64::from(limits.max_tree_leaves),
        );
    }
    holds
}

/// D3 / U-R7（Phase R2a）: 深さごとの数（木の節点の run・トークン・定価と、reviewer の run と定価）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DepthCounters {
    pub depth: u32,
    /// この深さの task の数。
    pub nodes: u32,
    /// reviewer を除く run（worker・planner・wrap-up。repair の run は worker に数える）。
    pub runs: u32,
    /// role ごとの run（reviewer を含む。`RunIndexRole::as_str`）。
    pub runs_by_role: std::collections::BTreeMap<String, u32>,
    /// U-R7: reviewer の run と、その定価（USD）。
    pub reviewer_runs: u32,
    pub reviewer_cost_usd: f64,
    /// input + output トークン（reviewer を含むすべての run）。
    pub tokens: u64,
    /// 定価（USD。reviewer を含む）。`cost_usd_complete` が `false` なら下限。
    pub cost_usd: f64,
    pub cost_usd_complete: bool,
}

/// D3（Phase R2a）: 木（root とその子孫）の上限に照らす数。store から集めた [`TreeNodeFacts`] から
/// [`tree_counters`] が決定的に作る。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TreeCounters {
    pub root_id: Option<TaskId>,
    pub nodes: u32,
    /// 木の生涯で作った leaf（repair・統合・kind task の行を除く work_units の行。決定を待って止めている
    /// 行〈`blocked(decision)`〉は作ったうちに数えない）。
    pub leaves: u32,
    /// reviewer を除く run（`max_tree_runs` に照らす値）。
    pub runs: u32,
    pub reviewer_runs: u32,
    /// 木全体で採用した replan の版（節点ごとの `計画の版の数 − 1` の和）。
    pub replans: u32,
    /// input + output トークン（`max_tree_tokens` に照らす値。reviewer を含む）。
    pub tokens: u64,
    pub cost_usd: f64,
    pub cost_usd_complete: bool,
    /// 深さの昇順。
    pub by_depth: Vec<DepthCounters>,
}

/// [`tree_counters`] の入力: 木の 1 節点の事実（store の読み取りだけで集める。`task_ops::tree::tree_counters`）。
#[derive(Debug, Clone)]
pub struct TreeNodeFacts {
    pub task_id: TaskId,
    pub depth: u32,
    pub runs: Vec<crate::execution_plan::RunRow>,
    pub work_units: Vec<crate::execution_plan::WorkUnitRow>,
    /// 計画の版の数（`execution_plans` の行数。replan は `版 − 1`）。
    pub plan_versions: u32,
}

/// D3 / U-R7（Phase R2a）: 木の数（純粋関数）。
pub fn tree_counters(root_id: Option<TaskId>, nodes: &[TreeNodeFacts]) -> TreeCounters {
    use crate::execution_plan::{
        RunIndexRole, WorkUnitBlockedReason, WorkUnitKind, WorkUnitStatus,
    };
    let mut out = TreeCounters {
        root_id,
        cost_usd_complete: true,
        ..Default::default()
    };
    let mut by_depth: std::collections::BTreeMap<u32, DepthCounters> =
        std::collections::BTreeMap::new();
    for node in nodes {
        out.nodes += 1;
        out.replans += node.plan_versions.saturating_sub(1);
        out.leaves += node
            .work_units
            .iter()
            .filter(|u| {
                !matches!(
                    u.kind,
                    WorkUnitKind::Task | WorkUnitKind::Integrate | WorkUnitKind::Repair
                ) && !(u.status == WorkUnitStatus::Blocked
                    && u.blocked_reason == Some(WorkUnitBlockedReason::Decision))
            })
            .count() as u32;
        let d = by_depth.entry(node.depth).or_insert_with(|| DepthCounters {
            depth: node.depth,
            cost_usd_complete: true,
            ..Default::default()
        });
        d.nodes += 1;
        for run in &node.runs {
            *d.runs_by_role
                .entry(run.role.as_str().to_string())
                .or_default() += 1;
            let reviewer = run.role == RunIndexRole::Reviewer;
            if reviewer {
                d.reviewer_runs += 1;
                out.reviewer_runs += 1;
            } else {
                d.runs += 1;
                out.runs += 1;
            }
            let Some(usage) = run.usage.as_ref() else {
                continue;
            };
            let tokens = usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0);
            d.tokens += tokens;
            out.tokens += tokens;
            match usage.cost_usd {
                Some(c) => {
                    d.cost_usd += c;
                    out.cost_usd += c;
                    if reviewer {
                        d.reviewer_cost_usd += c;
                    }
                }
                None if tokens > 0 => {
                    d.cost_usd_complete = false;
                    out.cost_usd_complete = false;
                }
                None => {}
            }
        }
    }
    out.by_depth = by_depth.into_values().collect();
    out
}

/// D3（Phase R2a）: 次に起こそうとしている run の種類（`max_tree_replans` は replan の planner run だけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextRun {
    Worker,
    Planner { replan: bool },
}

/// D3: run の上限の超過（どれか 1 つ。runs → tokens → replans の順で先に当たったもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimitBreach {
    pub limit: TreeLimitKind,
    pub count: u64,
    pub max: u64,
}

/// D3（Phase R2a）: 木の数に照らし、次の run を起こすと上限を超えるか（純粋関数）。超えるなら
/// その上限。`runs >= max_tree_runs`（もう 1 本起こすと超える）、`tokens >= max_tree_tokens`（設定した
/// ときだけ）、replan の planner run で `replans >= max_tree_replans`。
pub fn run_limit_breach(
    limits: &TreeLimits,
    counters: &TreeCounters,
    next: NextRun,
) -> Option<RunLimitBreach> {
    if counters.runs >= limits.max_tree_runs {
        return Some(RunLimitBreach {
            limit: TreeLimitKind::TreeRuns,
            count: u64::from(counters.runs),
            max: u64::from(limits.max_tree_runs),
        });
    }
    if let Some(max) = limits.max_tree_tokens
        && counters.tokens >= max
    {
        return Some(RunLimitBreach {
            limit: TreeLimitKind::TreeTokens,
            count: counters.tokens,
            max,
        });
    }
    if next == (NextRun::Planner { replan: true }) && counters.replans >= limits.max_tree_replans {
        return Some(RunLimitBreach {
            limit: TreeLimitKind::TreeReplans,
            count: u64::from(counters.replans),
            max: u64::from(limits.max_tree_replans),
        });
    }
    None
}

/// D3 / D7（Phase R2a）: `kind: limit` の決定の要求（daemon が出す。選択肢は D3 の「今回だけ上限を上げて
/// 続ける / この subtree を replan で小さくする / この subtree を取り下げる」、推奨は replan）。
#[allow(clippy::too_many_arguments)]
pub fn limit_decision(
    limit: TreeLimitKind,
    scope: Option<&str>,
    count: u64,
    max: u64,
    needed_before: Vec<String>,
    path: Vec<crate::decision::DecisionPathEntry>,
    raised_by: crate::decision::DecisionRaisedBy,
) -> crate::decision::DecisionRequest {
    use crate::decision::{CostOfReversal, DecisionKind, DecisionOption, DecisionRequest};
    let what = match scope {
        Some(s) => format!("{}（段階 {s}）", limit.as_str()),
        None => limit.as_str().to_string(),
    };
    DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: limit.decision_key(scope),
        kind: DecisionKind::Limit,
        question: format!(
            "木の上限 {what} を超えます（{count} > 上限 {max}）。止めた仕事をどうしますか"
        ),
        options: vec![
            DecisionOption {
                key: "raise-once".to_string(),
                label: "今回だけ上限を上げて続ける".to_string(),
                consequence: Some("止めた unit / run をそのまま進める".to_string()),
            },
            DecisionOption {
                key: "replan".to_string(),
                label: "この subtree を replan で小さくする".to_string(),
                consequence: Some("planner が上限に収まる計画を出し直す".to_string()),
            },
            DecisionOption {
                key: "withdraw".to_string(),
                label: "この subtree を取り下げる".to_string(),
                consequence: Some("止めた仕事を行わない".to_string()),
            },
        ],
        recommended: "replan".to_string(),
        cost_of_reversal: CostOfReversal::Medium,
        cost_note: Some(format!("[execution.tree] {} = {max}", limit.as_str())),
        needed_before,
        path,
        raised_by,
        status: crate::decision::DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

/// ADR-0079 付記「R7-3」D4: gate が compound と判定した根拠の文（決定文に使う）。score が閾値以上
/// （`compound/score`）なら「score S ≥ 閾値 T」、強制規則（`compound/long-and-broad` など）なら score が閾値に
/// 届いていなくても規則で compound になったことを書く（以前は常に「score S ≥ 閾値 T」と書き、「score 7 ≥ 閾値 11」の
/// ような誤った文になっていた）。
pub fn gate_basis_text(
    decision: &crate::execution_gate::ExecutionGateDecision,
    threshold: u32,
) -> String {
    let rule = decision.rule_id.as_str();
    let score = decision.score;
    if i64::from(score) >= i64::from(threshold) {
        return format!("gate {rule}、score {score} ≥ 閾値 {threshold}");
    }
    let why = match rule {
        "compound/long-and-broad" => "expected_length=high かつ cross_cutting=high",
        "human/explicit" => "人の明示",
        _ => "score 以外の規則",
    };
    format!(
        "gate {rule}: score {score} は閾値 {threshold} 未満だが、この規則は score によらず compound と判定する（{why}）"
    )
}

/// D4 (3)（Phase R2a）: `kind: leaf_too_large` の決定の要求（compound な leaf を子 task にできない深さ）。
pub fn leaf_too_large_decision(
    gate: &UnitGate,
    unit_title: &str,
    path: Vec<crate::decision::DecisionPathEntry>,
    raised_by: crate::decision::DecisionRaisedBy,
) -> crate::decision::DecisionRequest {
    use crate::decision::{CostOfReversal, DecisionKind, DecisionOption, DecisionRequest};
    DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: format!("leaf_too_large:{}", gate.unit_key),
        kind: DecisionKind::LeafTooLarge,
        question: format!(
            "unit {}「{unit_title}」は 1 run に収まらない見込み（{}）ですが、この深さでは子 task にできません。どうしますか",
            gate.unit_key,
            gate_basis_text(&gate.decision, gate.threshold)
        ),
        options: vec![
            DecisionOption {
                key: "run-as-leaf".to_string(),
                label: "leaf のまま 1 run で試す".to_string(),
                consequence: Some("continuation に頼る可能性がある".to_string()),
            },
            DecisionOption {
                key: "replan".to_string(),
                label: "replan で小さな leaf に分ける".to_string(),
                consequence: None,
            },
            DecisionOption {
                key: "withdraw".to_string(),
                label: "この unit を取り下げる".to_string(),
                consequence: None,
            },
        ],
        recommended: "replan".to_string(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: Some(gate.reason.clone()),
        needed_before: vec![gate.unit_key.clone()],
        path,
        raised_by,
        status: crate::decision::DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

/// ADR-0079 D9（Phase R2b）: `kind: plan_invalid` の決定の key（daemon が振る。節点ごとに 1 件だけ開く）。
pub const PLAN_INVALID_DECISION_KEY: &str = "plan_invalid";

/// ADR-0079 D9（Phase R2b / R3a）: /3 の計画が 2 回不正だったときの決定の要求（atomic に倒さない）。選択肢は
/// 「note を添えて replan / atomic（1 run、初回の計画のときだけ）で試す / 取り下げる（cancel）」、推奨は replan。`errors` は検証の理由（最後の試行まで、古い順）。`needed_before: ["self"]`（この節点の run を止める）。
pub fn plan_invalid_decision(
    task_id: TaskId,
    replan: bool,
    errors: &[String],
    path: Vec<crate::decision::DecisionPathEntry>,
    raised_by: crate::decision::DecisionRaisedBy,
) -> crate::decision::DecisionRequest {
    use crate::decision::{CostOfReversal, DecisionKind, DecisionOption, DecisionRequest};
    let what = if replan {
        "計画の見直し（replan）"
    } else {
        "計画"
    };
    let mut note = errors
        .iter()
        .enumerate()
        .map(|(i, e)| format!("試行 {}: {e}", i + 1))
        .collect::<Vec<_>>()
        .join(" / ");
    if note.chars().count() > 2000 {
        note = note.chars().take(2000).collect::<String>() + "…";
    }
    DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: PLAN_INVALID_DECISION_KEY.to_string(),
        kind: DecisionKind::PlanInvalid,
        question: format!(
            "planner が出した{what}（celeris.execution-plan/3）が 2 回とも検証に通りませんでした。この task をどう進めますか"
        ),
        // Phase R3a: 選択肢は「note を添えて planner にもう一度計画させる（replan）/ atomic / 取り下げる（cancel）」。
        // atomic は計画がまだ無いとき（初回の計画）だけ（採用済みの計画の WU を宙に浮かせない。
        // `task_ops::regate` の「計画を持つ Task を atomic に戻さない」と同じ規則）。
        options: {
            let mut options = vec![DecisionOption {
                key: "replan".to_string(),
                label: "note の指示を添えて planner にもう一度計画させる".to_string(),
                consequence: Some(format!(
                    "人の note が planner に渡る（人が計画を書くなら PUT /tasks/{task_id}/execution-plan）"
                )),
            }];
            if !replan {
                options.push(DecisionOption {
                    key: "atomic".to_string(),
                    label: "分けずに 1 run（atomic）で試す".to_string(),
                    consequence: Some("分けると決めた仕事を 1 run に収める".to_string()),
                });
            }
            options.push(DecisionOption {
                key: "cancel".to_string(),
                label: "この task を取り下げる".to_string(),
                consequence: None,
            });
            options
        },
        recommended: "replan".to_string(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: if note.is_empty() { None } else { Some(note) },
        needed_before: vec![crate::decision::NEEDED_BEFORE_SELF.to_string()],
        path,
        raised_by,
        status: crate::decision::DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}
