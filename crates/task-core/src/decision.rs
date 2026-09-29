//! ADR-0079 D7（Phase R1a）: 人への決定の要求の型と検証（純粋）。
//!
//! - [`DecisionSpec`]: 計画（`celeris.execution-plan/3` の `decisions`、unit の `decisions`）に書く形。
//! - [`DecisionRequest`]: daemon が id と path を付けた完全な形（`Event::DecisionRequested`、
//!   `decisions.json`、`docs/protocol/decision.schema.json`）。
//! - [`DecisionRow`]: 派生の表 `decisions`（migration 0031）の 1 行と、events からの畳み込み
//!   （[`DecisionRow::from_request`] / [`DecisionRow::apply_answer`] / [`DecisionRow::apply_withdrawal`]）。
//!   store の書き込み（`Event` と同じトランザクション）と replay の再構築がこの同じ関数を通る。
//!
//! 回答の流れ（API・MCP・受信箱・通知）は R3a。ここは型と形の検証だけ。

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::TaskId;

/// D7: `needed_before` で段階を指す接頭辞（`stage:<key>`）。
pub const NEEDED_BEFORE_STAGE_PREFIX: &str = "stage:";
/// D7: worker が自分の unit を止める決定の `needed_before`（worker が出すときだけ。R3a）。
pub const NEEDED_BEFORE_SELF: &str = "self";
/// D7: 選択肢の数の下限・上限。
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 5;

/// D7: 後戻りの大きさ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostOfReversal {
    Low,
    Medium,
    High,
}

/// D7: 選択肢 1 つ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionOption {
    /// `[a-z0-9-]{1,32}`。決定の中で一意。
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consequence: Option<String>,
}

/// D2 / D7: 計画に書く決定の形（`id` / `path` / `raised_by` / `status` は daemon が付ける）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionSpec {
    /// `[a-z0-9-]{1,32}`。計画の中で一意。
    pub key: String,
    pub question: String,
    /// 2..=5 件。
    pub options: Vec<DecisionOption>,
    /// `options` のどれかの key。
    pub recommended: String,
    pub cost_of_reversal: CostOfReversal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_note: Option<String>,
    /// 止める unit の key か `stage:<key>`。計画の `decisions` では 1 件以上（何を止めるかを書かせる）。
    /// unit に書いた `decisions` では省略でき、その unit が補われる（D2 の糖衣）。
    #[serde(default)]
    pub needed_before: Vec<String>,
}

/// D7: 決定の要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// 計画・worker が出す選択。
    Choice,
    /// daemon: compound な leaf を子 task にできない深さ（D4 (3)。R2a）。
    LeafTooLarge,
    /// daemon: 木の上限の超過（D3。R2a）。
    Limit,
    /// daemon: plan/3 の計画が 2 回不正（D9。R2b）。
    PlanInvalid,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::Choice => "choice",
            DecisionKind::LeafTooLarge => "leaf_too_large",
            DecisionKind::Limit => "limit",
            DecisionKind::PlanInvalid => "plan_invalid",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "choice" => Some(DecisionKind::Choice),
            "leaf_too_large" => Some(DecisionKind::LeafTooLarge),
            "limit" => Some(DecisionKind::Limit),
            "plan_invalid" => Some(DecisionKind::PlanInvalid),
            _ => None,
        }
    }
}

/// D7: 決定の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Open,
    Answered,
    Withdrawn,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionStatus::Open => "open",
            DecisionStatus::Answered => "answered",
            DecisionStatus::Withdrawn => "withdrawn",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(DecisionStatus::Open),
            "answered" => Some(DecisionStatus::Answered),
            "withdrawn" => Some(DecisionStatus::Withdrawn),
            _ => None,
        }
    }
}

/// D7: 決定を出した者の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOrigin {
    Planner,
    Worker,
    Daemon,
    /// 人が書いた計画（`PUT /tasks/{id}/execution-plan`、origin human）の `decisions`。
    Human,
}

/// D7: 木の中の位置の 1 段（root から出した節点まで）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionPathEntry {
    pub task_id: TaskId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// D7: 決定を出した節点と run。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionRaisedBy {
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub origin: DecisionOrigin,
}

/// D7: 回答（`Event::DecisionAnswered` の写し。R1a の明確化: 表の `json` に回答を残すため
/// `DecisionRequest.answer` に持つ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionAnswer {
    pub option: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub by: String,
}

/// D7: 決定の要求（`docs/protocol/decision.schema.json`）。`id` と `path` は daemon が付ける
/// （LLM に書かせない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    /// daemon が振る ULID（木の中で一意）。
    pub id: String,
    /// 出した者が付けた key（計画・run の中で一意）。
    pub key: String,
    pub kind: DecisionKind,
    pub question: String,
    pub options: Vec<DecisionOption>,
    pub recommended: String,
    pub cost_of_reversal: CostOfReversal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_note: Option<String>,
    /// `<unit key>` | `stage:<key>` | `self`。
    pub needed_before: Vec<String>,
    /// root から出した節点まで。
    pub path: Vec<DecisionPathEntry>,
    pub raised_by: DecisionRaisedBy,
    pub status: DecisionStatus,
    /// 回答（`status = answered` のときだけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<DecisionAnswer>,
    /// 取り下げの理由（`status = withdrawn` のときだけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn_reason: Option<String>,
}

impl DecisionRequest {
    /// 木の root（`path` の先頭。空なら出した節点）。
    pub fn root_id(&self) -> TaskId {
        self.path
            .first()
            .map(|p| p.task_id)
            .unwrap_or(self.raised_by.task_id)
    }
}

/// 生成したスキーマ（`docs/protocol/decision.schema.json`。`UPDATE_SCHEMA=1` で再生成）。
pub fn schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(DecisionRequest);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

/// D7: 決定の形の検査の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionShapeError {
    InvalidKey { key: String },
    EmptyQuestion { key: String },
    OptionCount { key: String, count: usize },
    InvalidOptionKey { key: String, option: String },
    DuplicateOptionKey { key: String, option: String },
    UnknownRecommended { key: String, recommended: String },
    NoNeededBefore { key: String },
}

impl std::fmt::Display for DecisionShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionShapeError::InvalidKey { key } => {
                write!(
                    f,
                    "invalid decision key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            DecisionShapeError::EmptyQuestion { key } => {
                write!(f, "decision {key}: question must not be empty")
            }
            DecisionShapeError::OptionCount { key, count } => write!(
                f,
                "decision {key}: options must have {MIN_OPTIONS}..={MAX_OPTIONS} entries, found {count}"
            ),
            DecisionShapeError::InvalidOptionKey { key, option } => write!(
                f,
                "decision {key}: invalid option key {option:?} (must match [a-z0-9-]{{1,32}})"
            ),
            DecisionShapeError::DuplicateOptionKey { key, option } => {
                write!(f, "decision {key}: duplicate option key {option}")
            }
            DecisionShapeError::UnknownRecommended { key, recommended } => write!(
                f,
                "decision {key}: recommended {recommended:?} is not one of the options"
            ),
            DecisionShapeError::NoNeededBefore { key } => write!(
                f,
                "decision {key}: needed_before must not be empty (name the unit or stage:<key> it blocks)"
            ),
        }
    }
}

/// `[a-z0-9-]{1,32}`（計画の key と同じ規則）。
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// D7: 決定の形の検査（key・問い・選択肢 2..=5・選択肢の key・推奨・`needed_before` が空でない）。
/// `needed_before` の指す先が計画にあるかは呼び出し側（計画の検証）が見る。
pub fn validate_shape(spec: &DecisionSpec) -> Vec<DecisionShapeError> {
    let mut errors = Vec::new();
    let key = spec.key.clone();
    if !valid_key(&spec.key) {
        errors.push(DecisionShapeError::InvalidKey { key: key.clone() });
    }
    if spec.question.trim().is_empty() {
        errors.push(DecisionShapeError::EmptyQuestion { key: key.clone() });
    }
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&spec.options.len()) {
        errors.push(DecisionShapeError::OptionCount {
            key: key.clone(),
            count: spec.options.len(),
        });
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for o in &spec.options {
        if !valid_key(&o.key) {
            errors.push(DecisionShapeError::InvalidOptionKey {
                key: key.clone(),
                option: o.key.clone(),
            });
        } else if !seen.insert(o.key.as_str()) {
            errors.push(DecisionShapeError::DuplicateOptionKey {
                key: key.clone(),
                option: o.key.clone(),
            });
        }
    }
    if !spec.options.iter().any(|o| o.key == spec.recommended) {
        errors.push(DecisionShapeError::UnknownRecommended {
            key: key.clone(),
            recommended: spec.recommended.clone(),
        });
    }
    if spec.needed_before.is_empty() {
        errors.push(DecisionShapeError::NoNeededBefore { key });
    }
    errors
}

/// D15: 派生の表 `decisions` の 1 行（`json` は [`DecisionRequest`] 全体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    pub id: String,
    pub root_id: TaskId,
    /// 決定を出した節点（`Event::DecisionRequested` を積んだ task）。
    pub task_id: TaskId,
    pub key: String,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub needed_before: Vec<String>,
    pub request: DecisionRequest,
    /// `DecisionRequested` の event の ts。
    pub created_at: String,
    /// `DecisionAnswered` の event の ts（最後の回答。revise〈R3a〉は上書き）。
    pub answered_at: Option<String>,
}

impl DecisionRow {
    /// `Event::DecisionRequested` から行を作る（`task_id` = その event を積んだ task、`ts` = event の ts）。
    pub fn from_request(task_id: TaskId, request: &DecisionRequest, ts: &str) -> Self {
        DecisionRow {
            id: request.id.clone(),
            root_id: request.root_id(),
            task_id,
            key: request.key.clone(),
            kind: request.kind,
            status: request.status,
            needed_before: request.needed_before.clone(),
            request: request.clone(),
            created_at: ts.to_string(),
            answered_at: None,
        }
    }

    /// `Event::DecisionAnswered` を当てる。
    pub fn apply_answer(&mut self, option: &str, note: Option<&str>, by: &str, ts: &str) {
        self.status = DecisionStatus::Answered;
        self.request.status = DecisionStatus::Answered;
        self.request.answer = Some(DecisionAnswer {
            option: option.to_string(),
            note: note.map(str::to_string),
            by: by.to_string(),
        });
        self.request.withdrawn_reason = None;
        self.answered_at = Some(ts.to_string());
    }

    /// `Event::DecisionWithdrawn` を当てる。
    pub fn apply_withdrawal(&mut self, reason: &str) {
        self.status = DecisionStatus::Withdrawn;
        self.request.status = DecisionStatus::Withdrawn;
        self.request.withdrawn_reason = Some(reason.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(key: &str) -> DecisionOption {
        DecisionOption {
            key: key.into(),
            label: format!("label {key}"),
            consequence: None,
        }
    }

    pub(crate) fn spec(key: &str, needed_before: &[&str]) -> DecisionSpec {
        DecisionSpec {
            key: key.into(),
            question: format!("question {key}?"),
            options: vec![opt("a"), opt("b")],
            recommended: "a".into(),
            cost_of_reversal: CostOfReversal::Medium,
            cost_note: None,
            needed_before: needed_before.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn shape_accepts_a_well_formed_decision() {
        assert!(validate_shape(&spec("h1", &["p2"])).is_empty());
    }

    #[test]
    fn shape_rejects_bad_options_recommended_and_missing_needed_before() {
        let mut s = spec("H1", &[]);
        s.options = vec![opt("a")];
        s.recommended = "zzz".into();
        let errs = validate_shape(&s);
        assert!(
            errs.iter()
                .any(|e| matches!(e, DecisionShapeError::InvalidKey { .. }))
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, DecisionShapeError::OptionCount { count: 1, .. }))
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, DecisionShapeError::UnknownRecommended { .. }))
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, DecisionShapeError::NoNeededBefore { .. }))
        );

        let mut six = spec("h2", &["x"]);
        six.options = (0..6).map(|i| opt(&format!("o{i}"))).collect();
        six.recommended = "o0".into();
        assert!(matches!(
            validate_shape(&six).as_slice(),
            [DecisionShapeError::OptionCount { count: 6, .. }]
        ));

        let mut dup = spec("h3", &["x"]);
        dup.options = vec![opt("a"), opt("a")];
        assert!(matches!(
            validate_shape(&dup).as_slice(),
            [DecisionShapeError::DuplicateOptionKey { .. }]
        ));
    }

    /// 回答・取り下げの畳み込み（store と replay が同じ関数を通る）。
    #[test]
    fn row_folds_answer_and_withdrawal() {
        let root = TaskId::new();
        let req = DecisionRequest {
            id: "01D".into(),
            key: "h1".into(),
            kind: DecisionKind::Choice,
            question: "q".into(),
            options: vec![opt("a"), opt("b")],
            recommended: "a".into(),
            cost_of_reversal: CostOfReversal::Low,
            cost_note: None,
            needed_before: vec!["p2".into()],
            path: vec![DecisionPathEntry {
                task_id: root,
                title: "root".into(),
                stage: Some("phase-2".into()),
                unit: None,
            }],
            raised_by: DecisionRaisedBy {
                task_id: root,
                run_id: None,
                origin: DecisionOrigin::Planner,
            },
            status: DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        };
        let mut row = DecisionRow::from_request(root, &req, "t0");
        assert_eq!(row.root_id, root);
        assert_eq!(row.status, DecisionStatus::Open);
        row.apply_answer("b", Some("n"), "human", "t1");
        assert_eq!(row.status, DecisionStatus::Answered);
        assert_eq!(row.answered_at.as_deref(), Some("t1"));
        assert_eq!(
            row.request.answer.as_ref().map(|a| a.option.as_str()),
            Some("b")
        );
        row.apply_withdrawal("superseded");
        assert_eq!(row.status, DecisionStatus::Withdrawn);
        assert_eq!(row.request.withdrawn_reason.as_deref(), Some("superseded"));
    }

    /// D7 / R1a (e): `decision.schema.json` の生成スキーマとコミット済みファイルの一致。
    #[test]
    fn decision_committed_schema_matches_generated() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/protocol/decision.schema.json"
        );
        let generated = serde_json::to_string_pretty(&schema_value()).unwrap() + "\n";
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::write(path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e} (run with UPDATE_SCHEMA=1 to generate)"));
        assert_eq!(
            committed, generated,
            "schema drift: run `UPDATE_SCHEMA=1 cargo test -p task-core`"
        );
    }
}
