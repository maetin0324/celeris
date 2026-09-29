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

// ---------------------------------------------------------------------------
// ADR-0079 D7（Phase R3a）: 回答の検証・回答の効き目（選択肢 → 効き目の表）・注入の固定の書式・
// worker が出す決定の検証と束ね。すべて純粋関数（LLM なし）。
// ---------------------------------------------------------------------------

/// D7: 回答を入力に入れる節の見出し（子 task の `objective` の末尾と leaf の前置きで同じ）。
pub const DECISIONS_HEADING: &str = "## 人の決定（ADR-0079 D7）";

/// R3a: 自由記述の回答（`choice` の決定で `option` を省いたとき）の `DecisionAnswer.option`。
pub const FREE_TEXT_OPTION: &str = "other";
/// R3a: 回答の `note` の上限（文字数。`task_ops::regate::NOTE_MAX_CHARS` と同じ）。
pub const ANSWER_NOTE_MAX_CHARS: usize = 2000;
/// R3a: worker の決定を束ねた決定の key（衝突すれば `bundle-2`, `bundle-3`, …）。
pub const BUNDLE_KEY: &str = "bundle";
/// R3a: 束ねた決定の選択肢。
pub const BUNDLE_OPTION_RECOMMENDED: &str = "all-recommended";
pub const BUNDLE_OPTION_NOTE: &str = "see-note";

/// R3a: 回答（または人の取り下げ）が daemon に対して持つ効き目。**決定の種類と選択肢の key だけから決まる**
/// （[`answer_effect`]。ADR-0079 付記「R3a 実装時の逸脱・明確化」の表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionEffect {
    /// 待っていた unit（`needs_decisions` / `needed_before`）を再評価して進める。`self` の決定なら節点の
    /// 止めが外れる。
    Resume,
    /// 止めた unit を進める。run 時の木の上限（`needed_before: [self]` の `limit:*`）なら、その上限に
    /// 1 回分の余裕を足す（`task_core::tree::limit_allowance_step`）。
    RaiseOnce,
    /// 決定を出した節点の replan を依頼する（止めた unit は replan が置き換えるまで止めたまま）。
    /// `plan_invalid` では planner をもう一度起こし、人の note を planner に渡す。
    Replan,
    /// `plan_invalid`: 分けずに 1 run（atomic）で試す。
    Atomic,
    /// 止めた unit を取り下げる（`cancelled`）。`needed_before: [self]` なら決定を出した節点を中止する。
    Withdraw,
}

impl DecisionEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionEffect::Resume => "resume",
            DecisionEffect::RaiseOnce => "raise_once",
            DecisionEffect::Replan => "replan",
            DecisionEffect::Atomic => "atomic",
            DecisionEffect::Withdraw => "withdraw",
        }
    }
}

/// R3a: 選択肢 → 効き目の表（決定的）。`choice`（計画・worker・束ね）はどの選択肢でも `Resume`
/// （答えが何であれ、待っていた unit に答えを渡して進める）。daemon の決定は選択肢の key で決まる:
///
/// | kind | option | effect |
/// |---|---|---|
/// | choice | （すべて・自由記述） | Resume |
/// | leaf_too_large | run-as-leaf | Resume |
/// | leaf_too_large / limit | replan | Replan |
/// | leaf_too_large / limit | withdraw | Withdraw |
/// | limit | raise-once | RaiseOnce |
/// | plan_invalid | replan（R2b の human-plan も同じ） | Replan |
/// | plan_invalid | atomic | Atomic |
/// | plan_invalid | cancel（withdraw も同じ） | Withdraw |
pub fn answer_effect(kind: DecisionKind, option: &str) -> DecisionEffect {
    match (kind, option) {
        (DecisionKind::Choice, _) => DecisionEffect::Resume,
        (DecisionKind::Limit, "raise-once") => DecisionEffect::RaiseOnce,
        (
            DecisionKind::LeafTooLarge | DecisionKind::Limit | DecisionKind::PlanInvalid,
            "replan",
        )
        | (DecisionKind::PlanInvalid, "human-plan") => DecisionEffect::Replan,
        (DecisionKind::PlanInvalid, "atomic") => DecisionEffect::Atomic,
        (
            DecisionKind::LeafTooLarge | DecisionKind::Limit | DecisionKind::PlanInvalid,
            "withdraw" | "cancel",
        ) => DecisionEffect::Withdraw,
        // run-as-leaf と、表に無い key（検証で入らない）は待っていたものを進めるだけ。
        _ => DecisionEffect::Resume,
    }
}

/// R3a: 回答の検証の失敗（API は 422）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerError {
    UnknownOption {
        option: String,
        allowed: Vec<String>,
    },
    OptionRequired {
        allowed: Vec<String>,
    },
    NoteTooLong {
        max: usize,
    },
}

impl std::fmt::Display for AnswerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnswerError::UnknownOption { option, allowed } => write!(
                f,
                "option: {option:?} is not one of the decision's options ({})",
                allowed.join(", ")
            ),
            AnswerError::OptionRequired { allowed } => write!(
                f,
                "option: required (one of {}); a free-text answer (note only) is accepted for kind=choice only",
                allowed.join(", ")
            ),
            AnswerError::NoteTooLong { max } => {
                write!(f, "note: must be at most {max} characters")
            }
        }
    }
}

/// R3a: 回答を検証し、記録する `option` を返す。`option` は決定の `options` のどれか。省いたときは
/// **`kind = choice` で `note` が空でないときだけ**自由記述の回答（[`FREE_TEXT_OPTION`]）として受け付ける
/// （daemon の決定は効き目が選択肢で決まるので省けない）。`note` は 2,000 文字まで。
pub fn validate_answer(
    request: &DecisionRequest,
    option: Option<&str>,
    note: Option<&str>,
) -> Result<String, AnswerError> {
    let allowed: Vec<String> = request.options.iter().map(|o| o.key.clone()).collect();
    if let Some(n) = note
        && n.trim().chars().count() > ANSWER_NOTE_MAX_CHARS
    {
        return Err(AnswerError::NoteTooLong {
            max: ANSWER_NOTE_MAX_CHARS,
        });
    }
    match option.map(str::trim).filter(|o| !o.is_empty()) {
        Some(o) => {
            if allowed.iter().any(|a| a == o) {
                Ok(o.to_string())
            } else {
                Err(AnswerError::UnknownOption {
                    option: o.to_string(),
                    allowed,
                })
            }
        }
        None => {
            let has_note = note.is_some_and(|n| !n.trim().is_empty());
            if request.kind == DecisionKind::Choice && has_note {
                Ok(FREE_TEXT_OPTION.to_string())
            } else {
                Err(AnswerError::OptionRequired { allowed })
            }
        }
    }
}

/// R3a: 回答を依存する仕事の入力に入れる固定の書式（D7）。子 task の `objective` の末尾と leaf の前置きの
/// 「人の決定」節の両方がこの 1 行を使う: `- <key> <question>: <label>（推奨どおり | 推奨と異なる） — <note>`。
/// 回答が無ければ `None`。
pub fn answer_line(request: &DecisionRequest) -> Option<String> {
    let answer = request.answer.as_ref()?;
    let label = if answer.option == FREE_TEXT_OPTION
        && !request.options.iter().any(|o| o.key == FREE_TEXT_OPTION)
    {
        "自由記述".to_string()
    } else {
        request
            .options
            .iter()
            .find(|o| o.key == answer.option)
            .map(|o| o.label.clone())
            .unwrap_or_else(|| answer.option.clone())
    };
    let agreement = if answer.option == request.recommended {
        "推奨どおり"
    } else {
        "推奨と異なる"
    };
    let mut out = format!(
        "- {} {}: {label}（{agreement}）",
        request.key, request.question
    );
    if let Some(note) = answer.note.as_deref().filter(|n| !n.trim().is_empty()) {
        out.push_str(&format!(" — {}", note.trim()));
    }
    Some(out)
}

/// R3a: worker（`result.json` の `decisions`）が出した決定を検証した結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerDecisionBatch {
    /// 記録する決定（`needed_before` の `self` は、leaf の run なら leaf の key に書き換え済み）。上限を
    /// 超えた分は最後の 1 件（key [`BUNDLE_KEY`]）に束ねてある。
    pub accepted: Vec<DecisionSpec>,
    /// 捨てた要素と理由（人に見える進行の 1 行にする）。
    pub rejected: Vec<String>,
    /// 束ねた決定の元の key（束ねなければ空）。
    pub bundled: Vec<String>,
}

/// R3a: worker の決定を検証する（形・`needed_before` の指す先・key の重複）。`self_key` は leaf（WU）の run
/// ならその key（`self` をこれに書き換える）、atomic の run なら `None`（`self` のまま = 節点を止める）。
/// `units` / `stages` は計画の unit と段階の key（atomic なら空: `self` 以外は指せない）。`taken` は
/// この節点で既に使われている決定の key。`cap` は新しく開ける決定の数（D3 の `max_open_decisions`
/// の残り）: 超えれば先頭 `cap − 1` 件を残し、残りを 1 件の決定に束ねる（`cap = 0` でも 1 件には束ねる）。
pub fn prepare_worker_decisions(
    raw: &[serde_json::Value],
    self_key: Option<&str>,
    units: &BTreeSet<String>,
    stages: &BTreeSet<String>,
    taken: &BTreeSet<String>,
    cap: usize,
) -> WorkerDecisionBatch {
    let mut out = WorkerDecisionBatch::default();
    let mut used: BTreeSet<String> = taken.clone();
    for (i, item) in raw.iter().enumerate() {
        let mut spec: DecisionSpec = match serde_json::from_value(item.clone()) {
            Ok(s) => s,
            Err(e) => {
                out.rejected.push(format!("decision #{}: {e}", i + 1));
                continue;
            }
        };
        let errors = validate_shape(&spec);
        if !errors.is_empty() {
            out.rejected.push(
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            continue;
        }
        if used.contains(&spec.key) {
            out.rejected.push(format!(
                "decision {}: the key is already used on this node",
                spec.key
            ));
            continue;
        }
        let mut targets: Vec<String> = Vec::new();
        let mut bad: Option<String> = None;
        for n in &spec.needed_before {
            let resolved = if n == NEEDED_BEFORE_SELF {
                self_key.unwrap_or(NEEDED_BEFORE_SELF).to_string()
            } else if let Some(stage) = n.strip_prefix(NEEDED_BEFORE_STAGE_PREFIX) {
                if !stages.contains(stage) {
                    bad = Some(format!("unknown stage {stage:?}"));
                    break;
                }
                n.clone()
            } else {
                if !units.contains(n) {
                    bad = Some(format!("unknown unit {n:?}"));
                    break;
                }
                n.clone()
            };
            if !targets.contains(&resolved) {
                targets.push(resolved);
            }
        }
        if let Some(why) = bad {
            out.rejected.push(format!(
                "decision {}: needed_before names an {why} (use self, a unit key of the plan or stage:<key>)",
                spec.key
            ));
            continue;
        }
        spec.needed_before = targets;
        used.insert(spec.key.clone());
        out.accepted.push(spec);
    }
    if out.accepted.len() > cap {
        let keep = cap.saturating_sub(1);
        let rest: Vec<DecisionSpec> = out.accepted.split_off(keep);
        if rest.len() == 1 {
            out.accepted.extend(rest);
        } else {
            let mut key = BUNDLE_KEY.to_string();
            let mut n = 2;
            while used.contains(&key) {
                key = format!("{BUNDLE_KEY}-{n}");
                n += 1;
            }
            out.bundled = rest.iter().map(|d| d.key.clone()).collect();
            out.accepted.push(bundle_decisions(&key, &rest, cap));
        }
    }
    out
}

/// R3a: 上限を超えた worker の決定を 1 件（`choice`）に束ねる。問いに元の問い・選択肢・推奨を列挙し、
/// `needed_before` は元の和（順を保つ）、後戻りの大きさは最大のもの。
fn bundle_decisions(key: &str, rest: &[DecisionSpec], cap: usize) -> DecisionSpec {
    let mut question = format!(
        "worker が出した決定が多すぎるため（未回答の上限の残り {cap} 件）、次の {} 件を 1 件にまとめました。note に各問いへの答えを書くか、推奨どおりに進めてください:",
        rest.len()
    );
    let mut needed_before: Vec<String> = Vec::new();
    let mut cost = CostOfReversal::Low;
    for (i, d) in rest.iter().enumerate() {
        let recommended = d
            .options
            .iter()
            .find(|o| o.key == d.recommended)
            .map(|o| o.label.as_str())
            .unwrap_or(d.recommended.as_str());
        let options = d
            .options
            .iter()
            .map(|o| format!("{}={}", o.key, o.label))
            .collect::<Vec<_>>()
            .join(" / ");
        question.push_str(&format!(
            "\n{}) {} {}（選択肢: {options}。推奨: {recommended}）",
            i + 1,
            d.key,
            d.question
        ));
        for n in &d.needed_before {
            if !needed_before.contains(n) {
                needed_before.push(n.clone());
            }
        }
        cost = match (cost, d.cost_of_reversal) {
            (_, CostOfReversal::High) | (CostOfReversal::High, _) => CostOfReversal::High,
            (_, CostOfReversal::Medium) | (CostOfReversal::Medium, _) => CostOfReversal::Medium,
            _ => CostOfReversal::Low,
        };
    }
    DecisionSpec {
        key: key.to_string(),
        question,
        options: vec![
            DecisionOption {
                key: BUNDLE_OPTION_RECOMMENDED.to_string(),
                label: "すべて推奨どおりに進める".to_string(),
                consequence: None,
            },
            DecisionOption {
                key: BUNDLE_OPTION_NOTE.to_string(),
                label: "note に書いた答えで進める".to_string(),
                consequence: None,
            },
        ],
        recommended: BUNDLE_OPTION_RECOMMENDED.to_string(),
        cost_of_reversal: cost,
        cost_note: Some(format!(
            "束ねた決定: {}",
            rest.iter()
                .map(|d| d.key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        needed_before,
    }
}

/// R3a: worker の `result.json` の本文から `decisions` の配列を取り出す（無い・配列でない・JSON でない
/// ときは空。形の検証は [`prepare_worker_decisions`]）。
pub fn worker_decisions_from_json(text: &str) -> Vec<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("decisions").and_then(|d| d.as_array()).cloned())
        .unwrap_or_default()
}

/// R3a: 計画・worker の決定の spec から、daemon が id と path を付けた要求を作る。
pub fn request_from_spec(
    spec: &DecisionSpec,
    path: Vec<DecisionPathEntry>,
    raised_by: DecisionRaisedBy,
) -> DecisionRequest {
    DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: spec.key.clone(),
        kind: DecisionKind::Choice,
        question: spec.question.clone(),
        options: spec.options.clone(),
        recommended: spec.recommended.clone(),
        cost_of_reversal: spec.cost_of_reversal,
        cost_note: spec.cost_note.clone(),
        needed_before: spec.needed_before.clone(),
        path,
        raised_by,
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
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

    fn request(kind: DecisionKind, options: &[&str], recommended: &str) -> DecisionRequest {
        DecisionRequest {
            id: "01D".into(),
            key: "h1".into(),
            kind,
            question: "which backend".into(),
            options: options.iter().map(|k| opt(k)).collect(),
            recommended: recommended.into(),
            cost_of_reversal: CostOfReversal::Low,
            cost_note: None,
            needed_before: vec!["p2".into()],
            path: Vec::new(),
            raised_by: DecisionRaisedBy {
                task_id: TaskId::new(),
                run_id: None,
                origin: DecisionOrigin::Planner,
            },
            status: DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        }
    }

    /// R3a: 選択肢 → 効き目の表（ADR-0079 付記 R3a の表と同じ）。
    #[test]
    fn answer_effect_table() {
        use DecisionEffect::*;
        use DecisionKind::*;
        let table = [
            (Choice, "anything", Resume),
            (Choice, FREE_TEXT_OPTION, Resume),
            (Choice, "replan", Resume),
            (LeafTooLarge, "run-as-leaf", Resume),
            (LeafTooLarge, "replan", Replan),
            (LeafTooLarge, "withdraw", Withdraw),
            (Limit, "raise-once", RaiseOnce),
            (Limit, "replan", Replan),
            (Limit, "withdraw", Withdraw),
            (PlanInvalid, "replan", Replan),
            (PlanInvalid, "human-plan", Replan),
            (PlanInvalid, "atomic", Atomic),
            (PlanInvalid, "cancel", Withdraw),
            (PlanInvalid, "withdraw", Withdraw),
        ];
        for (kind, option, effect) in table {
            assert_eq!(answer_effect(kind, option), effect, "{kind:?} {option}");
        }
    }

    /// R3a: 回答の検証（選択肢の中・自由記述は choice で note があるときだけ・note の長さ）。
    #[test]
    fn validate_answer_checks_options_free_text_and_note() {
        let choice = request(DecisionKind::Choice, &["a", "b"], "a");
        assert_eq!(validate_answer(&choice, Some("b"), None).unwrap(), "b");
        assert!(matches!(
            validate_answer(&choice, Some("zzz"), None),
            Err(AnswerError::UnknownOption { .. })
        ));
        assert_eq!(
            validate_answer(&choice, None, Some("use the org vault")).unwrap(),
            FREE_TEXT_OPTION
        );
        assert!(matches!(
            validate_answer(&choice, None, Some("  ")),
            Err(AnswerError::OptionRequired { .. })
        ));
        let limit = request(DecisionKind::Limit, &["raise-once", "replan"], "replan");
        assert!(matches!(
            validate_answer(&limit, None, Some("free text")),
            Err(AnswerError::OptionRequired { .. })
        ));
        let long = "x".repeat(ANSWER_NOTE_MAX_CHARS + 1);
        assert!(matches!(
            validate_answer(&choice, Some("a"), Some(&long)),
            Err(AnswerError::NoteTooLong { .. })
        ));
    }

    /// R3a: 注入の固定の書式（子の objective と leaf の前置きが共有する 1 行）。
    #[test]
    fn answer_line_is_the_fixed_format() {
        let mut r = request(DecisionKind::Choice, &["a", "b"], "a");
        assert_eq!(answer_line(&r), None);
        r.answer = Some(DecisionAnswer {
            option: "b".into(),
            note: Some(" trial first ".into()),
            by: "human".into(),
        });
        assert_eq!(
            answer_line(&r).unwrap(),
            "- h1 which backend: label b（推奨と異なる） — trial first"
        );
        r.answer = Some(DecisionAnswer {
            option: "a".into(),
            note: None,
            by: "human".into(),
        });
        assert_eq!(
            answer_line(&r).unwrap(),
            "- h1 which backend: label a（推奨どおり）"
        );
        r.answer = Some(DecisionAnswer {
            option: FREE_TEXT_OPTION.into(),
            note: Some("neither".into()),
            by: "human".into(),
        });
        assert_eq!(
            answer_line(&r).unwrap(),
            "- h1 which backend: 自由記述（推奨と異なる） — neither"
        );
    }

    fn raw(key: &str, needed_before: &[&str]) -> serde_json::Value {
        serde_json::to_value(spec(key, needed_before)).unwrap()
    }

    /// R3a: worker の決定の検証（形・指す先・key の重複・`self` の書き換え）。
    #[test]
    fn worker_decisions_are_validated_and_self_is_resolved() {
        let units: BTreeSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let stages: BTreeSet<String> = ["s1"].iter().map(|s| s.to_string()).collect();
        let taken: BTreeSet<String> = ["h0"].iter().map(|s| s.to_string()).collect();
        let items = vec![
            raw("w1", &["self"]),
            raw("w2", &["b", "stage:s1"]),
            raw("w3", &["nope"]),
            raw("h0", &["b"]),
            serde_json::json!({"key": "w5", "question": "q"}),
            raw("w1", &["b"]),
        ];
        let batch = prepare_worker_decisions(&items, Some("a"), &units, &stages, &taken, 8);
        assert_eq!(
            batch
                .accepted
                .iter()
                .map(|d| (d.key.as_str(), d.needed_before.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("w1", vec!["a".to_string()]),
                ("w2", vec!["b".to_string(), "stage:s1".to_string()]),
            ]
        );
        assert_eq!(batch.rejected.len(), 4, "{:?}", batch.rejected);
        assert!(batch.bundled.is_empty());
        // atomic の run: `self` のまま。unit は指せない。
        let atomic = prepare_worker_decisions(
            &[raw("w1", &["self"]), raw("w2", &["a"])],
            None,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            8,
        );
        assert_eq!(atomic.accepted.len(), 1);
        assert_eq!(atomic.accepted[0].needed_before, vec!["self".to_string()]);
        assert_eq!(worker_decisions_from_json(r#"{"summary":"s"}"#).len(), 0);
        assert_eq!(
            worker_decisions_from_json(r#"{"decisions":[{"key":"x"}]}"#).len(),
            1
        );
    }

    /// R3a: 上限を超えた worker の決定は 1 件に束ねる（先頭 cap − 1 件は残す）。
    #[test]
    fn worker_decisions_beyond_the_cap_are_bundled() {
        let units: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let taken: BTreeSet<String> = [BUNDLE_KEY].iter().map(|s| s.to_string()).collect();
        let mut high = spec("w3", &["c"]);
        high.cost_of_reversal = CostOfReversal::High;
        let items = vec![
            raw("w1", &["a"]),
            raw("w2", &["b"]),
            serde_json::to_value(high).unwrap(),
            raw("w4", &["a"]),
        ];
        let batch = prepare_worker_decisions(&items, None, &units, &BTreeSet::new(), &taken, 2);
        assert_eq!(batch.accepted.len(), 2);
        assert_eq!(batch.accepted[0].key, "w1");
        let bundle = &batch.accepted[1];
        assert_eq!(bundle.key, "bundle-2", "the plain key is taken");
        assert_eq!(batch.bundled, vec!["w2", "w3", "w4"]);
        assert_eq!(bundle.needed_before, vec!["b", "c", "a"]);
        assert_eq!(bundle.cost_of_reversal, CostOfReversal::High);
        assert!(
            bundle.question.contains("question w3?"),
            "{}",
            bundle.question
        );
        assert!(validate_shape(bundle).is_empty());
        // cap 0 でも 1 件には束ねる。cap 1 なら全部が 1 件の束になる。
        let zero = prepare_worker_decisions(
            &items[..2],
            None,
            &units,
            &BTreeSet::new(),
            &BTreeSet::new(),
            0,
        );
        assert_eq!(zero.accepted.len(), 1);
        assert_eq!(zero.accepted[0].key, BUNDLE_KEY);
        let one_over = prepare_worker_decisions(
            &items[..2],
            None,
            &units,
            &BTreeSet::new(),
            &BTreeSet::new(),
            1,
        );
        assert_eq!(
            one_over
                .accepted
                .iter()
                .map(|d| d.key.as_str())
                .collect::<Vec<_>>(),
            vec![BUNDLE_KEY]
        );
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
