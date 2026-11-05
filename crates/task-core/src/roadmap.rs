//! ADR 2026-11-04-project-controller 付記 A5: 案件の Roadmap（レビューできる計画の版）。
//!
//! - Roadmap は Project に紐づく計画の版で、独立の Goal ではない（D1）。旧 milestones（ADR-0079 で凍結）とは別の
//!   名詞で、`milestones` 表・`Task.milestone_id`・ExecutionPlan の stage・WorkUnit の phase を読まない（D2）。
//! - `Project.request` が正本の依頼、[`crate::project_brief::ProjectBrief`] は動的な要約（全置換）、Roadmap は
//!   人が認可した途中目標の版。brief の書き込みは版を変えない。
//! - 版は提案（`proposed`）として積まれ、人の決定（`approved`）だけが今の版を進める（前の版は `superseded`）。
//! - Outcome の `id` は版をまたいで不変: approved の版にある id は次の版から消せない（`dropped` にするだけ）。
//! - Outcome の状態（`open`・`in_progress`・`reached`）と root との対応は追記専用の記録で、版を進めない。
//!   `reached` は root の evidence を Rust が [`crate::project_proposal::root_satisfies`] で確かめたときだけ。
//!
//! この module は I/O を持たない純粋な型と関数だけ。書き込みは `store/roadmap.rs`、組み立ては
//! `task_ops::roadmap`。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project_proposal::{OUTCOMES_FENCE, Outcome, OutcomeCheck, parse_outcomes};

/// 1 版の Outcome の上限（件）。
pub const ROADMAP_OUTCOMES_MAX: usize = 64;
/// `change_reason`・決定の `note`・状態の `reason` の上限（文字）。
pub const ROADMAP_TEXT_MAX: usize = 2_000;
/// 1 つの acceptance の文の上限（文字）。
pub const ROADMAP_ACCEPTANCE_MAX: usize = 2_000;
/// snapshot の Outcome の `source`。
pub const ROADMAP_SOURCE: &str = "roadmap";

/// 版の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapStatus {
    Proposed,
    Approved,
    Rejected,
    Superseded,
}

impl RoadmapStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "proposed" => Self::Proposed,
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }
}

/// Outcome の状態。`dropped` は版（人の承認）でだけ付き、状態の記録では付けられない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Open,
    InProgress,
    Reached,
    Dropped,
}

impl OutcomeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Reached => "reached",
            Self::Dropped => "dropped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => Self::Open,
            "in_progress" => Self::InProgress,
            "reached" => Self::Reached,
            "dropped" => Self::Dropped,
            _ => return None,
        })
    }
}

/// 版の中の Outcome の定義（提案の本文と版の行の `outcomes_json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoadmapOutcomeSpec {
    /// 版をまたいで不変の id（`[a-z0-9_-]{1,32}`）。
    pub id: String,
    pub title: String,
    /// 達成の条件。controller の root はこの文を文字どおり含む（I8）。
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// この版で取り下げる（id は残す）。
    #[serde(default)]
    pub dropped: bool,
    /// 決定的な検査（第 4 フェーズ P4-B）。`Project.request` の Outcome block の `checks` と同じ契約で、controller の
    /// root の `Check::Command` になる（P0-E）。版の一部なので、変えるには提案と人の承認が要る。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<OutcomeCheck>,
}

/// `POST …/roadmap/proposals` の本文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoadmapProposalInput {
    /// 提案の土台にした approved の版（無ければ `null`）。決定のときに今の approved と違えば 409。
    pub base_version: Option<i64>,
    pub change_reason: String,
    pub outcomes: Vec<RoadmapOutcomeSpec>,
}

/// 版の行（提案・決定の記録そのまま）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoadmapVersion {
    pub project_id: String,
    pub version: i64,
    pub status: RoadmapStatus,
    pub base_version: Option<i64>,
    /// 提案の時点の `Project.request` の SHA-256。
    pub request_sha256: String,
    pub change_reason: String,
    pub outcomes: Vec<RoadmapOutcomeSpec>,
    pub proposed_by: String,
    pub proposed_at: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub decision_note: Option<String>,
    pub superseded_at: Option<String>,
}

/// Outcome の状態・root との対応の記録 1 件（追記専用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutcomeEvent {
    pub id: i64,
    pub outcome_id: String,
    /// `status` か `root_linked`。
    pub kind: String,
    pub status: Option<OutcomeStatus>,
    pub root_task_id: Option<String>,
    /// 記録した時点の approved の版。
    pub roadmap_version: i64,
    /// `task:<root id>` の形。`reached` では Rust が確かめた root だけ。
    pub evidence: Vec<String>,
    pub reason: String,
    pub actor: String,
    pub created_at: String,
}

/// 画面・API・snapshot に出す Outcome（定義＋今の状態＋対応する root）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoadmapOutcome {
    pub id: String,
    pub title: String,
    pub acceptance: Vec<String>,
    /// `reached` の根拠（Rust が確かめた `task:<root id>`）。
    pub evidence: Vec<String>,
    pub priority: i32,
    pub depends_on: Vec<String>,
    pub status: OutcomeStatus,
    /// この Outcome を担った root（古い順。controller の起票と状態の記録から）。
    pub root_task_ids: Vec<String>,
}

/// `GET …/roadmap` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Roadmap {
    pub project_id: String,
    pub version: i64,
    pub status: RoadmapStatus,
    pub request_sha256: String,
    pub change_reason: String,
    pub proposed_by: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub outcomes: Vec<RoadmapOutcome>,
}

/// `GET …/roadmap/history` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoadmapHistory {
    /// 版の行（版の番号の順）。
    pub versions: Vec<RoadmapVersion>,
    /// Outcome の記録（古い順）。
    pub outcome_events: Vec<OutcomeEvent>,
}

/// 検証の誤り（`code` は API の problem の code）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoadmapInvalid {
    pub code: &'static str,
    pub detail: String,
}

impl RoadmapInvalid {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

fn text_ok(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= max
}

/// 提案を検証する。`prev` は今の approved の版の Outcome（無ければ `None`）。
///
/// - 形（id・重複・依存の未知と循環・題・空の acceptance）は request の block と同じ規則（[`parse_outcomes`]）。
/// - `prev` にある id は消せない（stable id）。取り下げは `dropped: true`。
/// - 取り下げていない Outcome が取り下げた Outcome に依存してはいけない。
/// - 中身が `prev` と同じなら `roadmap_no_change`（版を増やさない）。
pub fn validate_proposal(
    prev: Option<&[RoadmapOutcomeSpec]>,
    input: &RoadmapProposalInput,
) -> Result<(), RoadmapInvalid> {
    if !text_ok(&input.change_reason, ROADMAP_TEXT_MAX) {
        return Err(RoadmapInvalid::new(
            "roadmap_invalid",
            format!("change_reason must be 1 to {ROADMAP_TEXT_MAX} characters and not blank"),
        ));
    }
    if input.outcomes.is_empty() || input.outcomes.len() > ROADMAP_OUTCOMES_MAX {
        return Err(RoadmapInvalid::new(
            "roadmap_invalid",
            format!("outcomes must have 1 to {ROADMAP_OUTCOMES_MAX} items"),
        ));
    }
    if let Some(o) = input.outcomes.iter().find(|o| {
        o.acceptance
            .iter()
            .any(|a| a.chars().count() > ROADMAP_ACCEPTANCE_MAX)
    }) {
        return Err(RoadmapInvalid::new(
            "roadmap_invalid",
            format!(
                "outcome {:?} has an acceptance over {ROADMAP_ACCEPTANCE_MAX} characters",
                o.id
            ),
        ));
    }
    // 形の検査は request の block と同じ（重複・依存・循環・題・acceptance）。
    let block: Vec<serde_json::Value> = input
        .outcomes
        .iter()
        .map(|o| {
            serde_json::json!({
                "id": o.id, "title": o.title, "acceptance": o.acceptance,
                "depends_on": o.depends_on, "priority": o.priority, "checks": o.checks,
            })
        })
        .collect();
    let text = format!("{OUTCOMES_FENCE}\n{}\n```", serde_json::Value::Array(block));
    parse_outcomes(&text).map_err(|e| RoadmapInvalid::new("roadmap_invalid", e))?;
    let by_id: BTreeMap<&str, &RoadmapOutcomeSpec> =
        input.outcomes.iter().map(|o| (o.id.as_str(), o)).collect();
    for o in input.outcomes.iter().filter(|o| !o.dropped) {
        if let Some(d) = o.depends_on.iter().find(|d| by_id[d.as_str()].dropped) {
            return Err(RoadmapInvalid::new(
                "roadmap_invalid",
                format!("outcome {:?} depends on the dropped outcome {d:?}", o.id),
            ));
        }
    }
    if input.outcomes.iter().all(|o| o.dropped) {
        return Err(RoadmapInvalid::new(
            "roadmap_invalid",
            "at least one outcome must not be dropped",
        ));
    }
    if let Some(prev) = prev {
        let now: BTreeSet<&str> = by_id.keys().copied().collect();
        if let Some(gone) = prev.iter().find(|p| !now.contains(p.id.as_str())) {
            return Err(RoadmapInvalid::new(
                "roadmap_outcome_id_removed",
                format!(
                    "outcome {:?} of the approved version must stay (set dropped: true to retire it)",
                    gone.id
                ),
            ));
        }
        if prev == input.outcomes.as_slice() {
            return Err(RoadmapInvalid::new(
                "roadmap_no_change",
                "the proposal has the same outcomes as the approved version",
            ));
        }
    }
    Ok(())
}

/// approved の版の Outcome を snapshot の形にする（取り下げは除く。`source = roadmap`）。
pub fn snapshot_outcomes(version: &RoadmapVersion) -> Vec<Outcome> {
    version
        .outcomes
        .iter()
        .filter(|o| !o.dropped)
        .map(|o| Outcome {
            id: o.id.clone(),
            title: o.title.clone(),
            acceptance: o.acceptance.clone(),
            depends_on: o.depends_on.clone(),
            priority: o.priority,
            source: ROADMAP_SOURCE.to_owned(),
            // 第 4 フェーズ P4-B: 依頼文の block と同じく root の acceptance に `Check::Command` として足す。
            checks: o.checks.clone(),
        })
        .collect()
}

/// 版の行と Outcome の記録から画面の Roadmap を作る。`launched` は controller が起票した root と、その root の
/// `outcome_ids`（起票の冪等キーの記録。古い順）。
pub fn build_view(
    version: &RoadmapVersion,
    events: &[OutcomeEvent],
    launched: &[(String, Vec<String>)],
) -> Roadmap {
    let outcomes = version
        .outcomes
        .iter()
        .map(|spec| {
            let mine = events.iter().filter(|e| e.outcome_id == spec.id);
            let last = mine
                .clone()
                .rfind(|e| e.kind == "status")
                .and_then(|e| e.status.map(|s| (s, e.evidence.clone())));
            let (status, evidence) = if spec.dropped {
                (OutcomeStatus::Dropped, Vec::new())
            } else {
                match last {
                    Some((OutcomeStatus::Reached, ev)) => (OutcomeStatus::Reached, ev),
                    Some((s, _)) => (s, Vec::new()),
                    None => (OutcomeStatus::Open, Vec::new()),
                }
            };
            let mut roots: Vec<String> = Vec::new();
            let mut push = |id: &str| {
                if !roots.iter().any(|r| r == id) {
                    roots.push(id.to_owned());
                }
            };
            for (root, ids) in launched {
                if ids.iter().any(|i| i == &spec.id) {
                    push(root);
                }
            }
            for e in mine.filter(|e| e.kind == "root_linked") {
                if let Some(r) = &e.root_task_id {
                    push(r);
                }
            }
            RoadmapOutcome {
                id: spec.id.clone(),
                title: spec.title.clone(),
                acceptance: spec.acceptance.clone(),
                evidence,
                priority: spec.priority,
                depends_on: spec.depends_on.clone(),
                status,
                root_task_ids: roots,
            }
        })
        .collect();
    Roadmap {
        project_id: version.project_id.clone(),
        version: version.version,
        status: version.status,
        request_sha256: version.request_sha256.clone(),
        change_reason: version.change_reason.clone(),
        proposed_by: version.proposed_by.clone(),
        decided_by: version.decided_by.clone(),
        decided_at: version.decided_at.clone(),
        outcomes,
    }
}

#[cfg(test)]
#[path = "roadmap_tests.rs"]
mod tests;
