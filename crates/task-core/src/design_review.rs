//! ADR 2026-11-07-health-review-integration D2.4: repo 固有の review 方針（profile の `[review]` 節）の
//! Rust 側の型。
//!
//! 値の正本は repo の `scripts/dev/repo-health/profiles/<profile>.toml` の `[review]` 節で、dispatcher は
//! `repo_health.py --print-review-policy` が base（target）側の版から出した JSON
//! （`celeris.repo-health.review-policy/1`）を読んでこの型にする。Rust では TOML を読まない。
//! 型は worker protocol の `ExecutionPlannerContext.review_policy` にそのまま載る（planner の prompt への写し）。
//!
//! D3.3 の risk の判定（[`assess_risk`]）もここに置く。入力（review 方針の状態・計画の write path・設計 brief）
//! だけで決まる純粋関数で、LLM も I/O も使わない（I4）。

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::execution_plan::{DesignBrief, ExecutionPlanSpec};

/// D2.4: 解決済みの review 方針。`source = absent`（base に `[review]` が無い）の repo では作らない
/// （`ExecutionPlannerContext.review_policy = None`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewPolicy {
    /// 変更 path の owner 行を引く file（repo 相対。例 `docs/architecture-map.md`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_map: Option<String>,
    /// 高 risk の path class。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub high_risk: Vec<PathClass>,
    /// 保護する不変条件の名前 → 根拠の file（repo 相対）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub invariants: BTreeMap<String, String>,
    /// 解決した profile の blob の内容から作った digest（D2.3）。health の材料の再利用の鍵に入る。
    pub definition_sha256: String,
}

/// D2.2: 高 risk の path class 1 件。`paths` は repo 相対 path に `re.search` で当てる正規表現。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PathClass {
    pub class: String,
    pub paths: Vec<String>,
}

/// D3.3: dispatcher が base（target）側から review 方針を読んだ結果。`Absent` と `Unavailable` を分け、
/// どちらも黙って省かない（ADR「fallback」の節）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewPolicyState {
    /// base に `[review]` があり、読めた。
    Present(ReviewPolicy),
    /// base に `[review]` が無い（`--print-review-policy` の `source = absent`）。その repo は設計 review を使わない。
    Absent,
    /// 読めなかった（script の error・remote workspace など）。値は理由の文（risk は `Unknown` になる）。
    Unavailable(String),
}

impl ReviewPolicyState {
    /// 読めた review 方針（`Present` のときだけ）。
    pub fn policy(&self) -> Option<&ReviewPolicy> {
        match self {
            ReviewPolicyState::Present(policy) => Some(policy),
            ReviewPolicyState::Absent | ReviewPolicyState::Unavailable(_) => None,
        }
    }
}

/// D3.3: 計画の risk。`High` と `Unknown` は採用の前に設計 review に回す（D3.5）。`Low` と `None` は
/// 今までどおり採用する（D3.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// review 方針の無い repo（`review_policy_absent`）。
    None,
    Low,
    High,
    /// review 方針を読めなかった（fail closed。`High` と同じく設計 review に回す）。
    Unknown,
}

impl RiskLevel {
    /// D3.5: 採用の前に設計 review が要るか。
    pub fn needs_design_review(self) -> bool {
        matches!(self, RiskLevel::High | RiskLevel::Unknown)
    }
}

/// D3.3: risk の判定の結果。`reasons` は判定の根拠の機械向けの語（`review_policy_absent`・
/// `review_policy_unavailable:<要約>`・`path_class:<class>+<欄>`・`new_fallback`・`brief_absent_human_plan`）。
/// 高 risk の path に触れない `Low` は空。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

impl RiskAssessment {
    fn new(level: RiskLevel, reasons: Vec<String>) -> Self {
        RiskAssessment { level, reasons }
    }

    fn unavailable(why: &str) -> Self {
        Self::new(
            RiskLevel::Unknown,
            vec![format!("review_policy_unavailable:{}", summarize(why))],
        )
    }
}

/// `review_policy_unavailable:<要約>` の要約（最初の空でない行、200 文字まで）。
fn summarize(why: &str) -> String {
    let line = why
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown");
    line.chars().take(200).collect()
}

/// D3.3: 計画の write path。全 unit（/3 の `units`）の `expected_write_paths` と leaf の `context.paths`、
/// /1・/2 の `work_units` の `context.paths` の和（重複を除き昇順）。
pub fn plan_write_paths(spec: &ExecutionPlanSpec) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for unit in &spec.units {
        paths.extend(unit.expected_write_paths.iter().flatten().cloned());
        if !unit.is_task() {
            paths.extend(unit.context.paths.iter().cloned());
        }
    }
    for wu in &spec.work_units {
        paths.extend(wu.context.paths.iter().cloned());
    }
    paths.into_iter().collect()
}

/// D3.3: write path のどれかに合う高 risk の class の名前（review 方針の順）。当て方は `profile.py` と同じ
/// `re.search`（path のどこかに合えばよい。`^` で先頭に固定する）。Rust の `regex` で読めない正規表現が
/// あれば、その class と式を返す（呼び出し側は方針を読めないものとして扱う）。
pub fn high_risk_classes(
    policy: &ReviewPolicy,
    write_paths: &[String],
) -> Result<Vec<String>, String> {
    let mut classes = Vec::new();
    for class in &policy.high_risk {
        let mut matched = false;
        for pattern in &class.paths {
            let re = Regex::new(pattern).map_err(|e| {
                format!(
                    "invalid path pattern {pattern:?} in class {}: {e}",
                    class.class
                )
            })?;
            matched = matched || write_paths.iter().any(|p| re.is_match(p));
        }
        if matched {
            classes.push(class.class.clone());
        }
    }
    Ok(classes)
}

/// D3.3: risk の判定。規則（上から順に）:
/// - 方針が `Absent` → `None`（`review_policy_absent`）。`Unavailable`・正規表現を読めない → `Unknown`
///   （`review_policy_unavailable:<要約>`）。
/// - write path が高 risk の class に合い、brief が `new_state`・`new_branch`・`new_fallback`・`new_port` の
///   どれかを宣言 → `High`（class と欄の組ごとに `path_class:<class>+<欄>`）。
/// - brief が `new_fallback` を宣言 → path に依らず `High`（`new_fallback`）。
/// - 高 risk の class に合い brief が無い → `Low`（`brief_absent_human_plan`）。planner の計画は採用前の検証
///   （`execution_plan::require_design_brief`）で差し戻されるので、ここに来るのは人の計画だけ。
/// - それ以外 → `Low`（理由なし）。
pub fn assess_risk(
    policy: &ReviewPolicyState,
    write_paths: &[String],
    brief: Option<&DesignBrief>,
) -> RiskAssessment {
    let policy = match policy {
        ReviewPolicyState::Present(policy) => policy,
        ReviewPolicyState::Absent => {
            return RiskAssessment::new(RiskLevel::None, vec!["review_policy_absent".to_string()]);
        }
        ReviewPolicyState::Unavailable(why) => return RiskAssessment::unavailable(why),
    };
    let classes = match high_risk_classes(policy, write_paths) {
        Ok(classes) => classes,
        Err(why) => return RiskAssessment::unavailable(&why),
    };
    let declared = brief.map(DesignBrief::declared_new).unwrap_or_default();
    let mut reasons: Vec<String> = classes
        .iter()
        .flat_map(|class| {
            declared
                .iter()
                .map(move |field| format!("path_class:{class}+{field}"))
        })
        .collect();
    if declared.contains(&"new_fallback") {
        reasons.push("new_fallback".to_string());
    }
    if !reasons.is_empty() {
        return RiskAssessment::new(RiskLevel::High, reasons);
    }
    if brief.is_none() && !classes.is_empty() {
        return RiskAssessment::new(RiskLevel::Low, vec!["brief_absent_human_plan".to_string()]);
    }
    RiskAssessment::new(RiskLevel::Low, Vec::new())
}

/// D3.3・D3.5: 計画 1 つの risk（[`plan_write_paths`] と計画の `design_brief` で [`assess_risk`] を呼ぶ）。
pub fn assess_plan_risk(policy: &ReviewPolicyState, spec: &ExecutionPlanSpec) -> RiskAssessment {
    assess_risk(policy, &plan_write_paths(spec), spec.design_brief.as_ref())
}

/// D3.5: `Event::DesignReviewRequested.plan_sha256`・`Event::DesignReviewed.plan_sha256` の値（計画の JSON の
/// SHA-256 の 16 進）。設計 review の待ちは同じ値の 2 つの event の有無だけで表す（I9）。
pub fn plan_sha256(spec: &ExecutionPlanSpec) -> String {
    crate::project_proposal::sha256_hex(&serde_json::to_string(spec).unwrap_or_default())
}

#[cfg(test)]
#[path = "design_review/tests.rs"]
mod tests;
