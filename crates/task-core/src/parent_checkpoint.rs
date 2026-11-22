//! ADR 2026-11-22-parent-checkpoint-evaluator: 親の途中評価（checkpoint）の発火・評価の検査・行動の表の純関数。
//!
//! - D1.2・D1.3: [`triggers`] は strategy root の事実（子の遷移・戦略の文書・使った token・前の判断）から trigger の
//!   一覧と状態（due・requested・evaluated）を導く。状態は保存しない。
//! - D3.2・D4: [`check_evaluation`] は reviewer の出力 `celeris.parent-evaluation/1` の形と上限を確かめ、参照の解決と
//!   Outcome の到達（D6）の事実から有効な判定を導く。参照の実在と到達は呼び出し側が事実として渡す。
//! - D5.2: [`allowed_actions`] は有効な判定から controller が取ってよい行動を返す。
//!
//! I/O も LLM も無い。依存の向き: この module は `strategy`（[`Reference`]・文書の型）と `model`（[`Status`]）を読む。
//! `project_proposal`・`store` はこの module を import しない（task-core の既存の循環に新しい組を足さないため）。
//! controller の検証は [`AllowedAction::kind`] と reason_code・question_key の文字列で照合する。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Status;
use crate::strategy::{Reference, Stance, StrategyDocument};
use crate::strategy_critic::strategy_artifact;

/// D3.2: reviewer の出力の schema。
pub const EVALUATION_SCHEMA: &str = "celeris.parent-evaluation/1";
/// D3.4: dispatcher が root の成果物に書く記録の schema。
pub const RECORD_SCHEMA: &str = "celeris.parent-evaluation-record/1";
/// D3.4: 記録の成果物の name の前置き。
pub const EVALUATION_ARTIFACT_PREFIX: &str = "parent-evaluation/";
/// D3.4: 記録の `ArtifactProduced` の `kind`。
pub const EVALUATION_ARTIFACT_KIND: &str = "parent_evaluation";
/// D3.2: `measured`・`learned`・`refuted`・`uncertainty` の各列の件数の上限。
pub const LIST_MAX: usize = 16;
/// D3.2: 文（claim・statement・hypothesis・resolve_by・reason）の文字数の上限。
pub const TEXT_MAX_CHARS: usize = 2_000;
/// D3.2: `metric`・`before`・`after` の文字数の上限。
pub const SHORT_MAX_CHARS: usize = 200;
/// D3.2: 1 件の参照の件数の上限。
pub const REFS_PER_ITEM_MAX: usize = 16;
/// 評価の全体（JSON に直した byte 数）の上限。形の検査より先に見る。
pub const EVALUATION_MAX_BYTES: usize = 262_144;
/// D2.1: 1 回の `evaluate` に入れる trigger の上限。
pub const TRIGGERS_PER_DECISION_MAX: usize = 8;
/// D5.2: 識別実験の replan の reason_code。
pub const REASON_EXPERIMENT: &str = "checkpoint_experiment";
/// D5.2: pivot の replan の reason_code。
pub const REASON_PIVOT: &str = "checkpoint_pivot";
/// D5.2: checkpoint の escalate の question_key の前置き。
pub const QUESTION_KEY_PREFIX: &str = "checkpoint:";

/// D3.1 の固定の rubric（id と本文）。文書や run は変えられない。
pub const PARENT_EVALUATION_RUBRIC: [(&str, &str); 6] = [
    (
        "E1",
        "実測: 親 Outcome の指標について、子の成果の前後の値を task・artifact・commit の参照つきで挙げる。参照の無い改善は書かない。",
    ),
    (
        "E2",
        "知見と反証: どの仮説が支持され、どれが反証されたかを参照つきで挙げる。子の done を親の進捗と書かない。",
    ),
    (
        "E3",
        "成功の根拠: done の子が Outcome の acceptance のどれにどう効いたか。効いていなければそう書く。",
    ),
    (
        "E4",
        "残る不確実性: 分からないことと、それを確かめる最小の実験か判断。",
    ),
    (
        "E5",
        "次の手: continue・discriminating_experiment・pivot・ask_human から 1 つ。実験と pivot は戦略の新しい版（人の承認）でしか行えないことを前提に選ぶ。",
    ),
    (
        "E6",
        "自己申告の禁止: worker・CoS の報告の文を証拠に数えない。Outcome の到達は goal.outcomes[].reached だけが決める。",
    ),
];

// ---------------------------------------------------------------- 設定（D1.2）

const CHILDREN_PER_CHECKPOINT_RANGE: (u32, u32) = (1, 8);
const THRESHOLD_RANGE: (u8, u8) = (1, 99);
const THRESHOLDS_MAX: usize = 4;
const ATTEMPTS_RANGE: (u32, u32) = (1, 3);

/// `[project_controller.checkpoint]` の値。範囲の検査は [`CheckpointSettings::new`] だけが行い、範囲外の値を持てない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointSettings {
    children_per_checkpoint: u32,
    budget_thresholds_pct: Vec<u8>,
    max_evaluation_attempts: u32,
}

/// 設定の誤り（`field` は設定の欄の名前）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    pub field: &'static str,
    pub detail: String,
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[project_controller.checkpoint] {}: {}",
            self.field, self.detail
        )
    }
}

impl std::error::Error for SettingsError {}

impl Default for CheckpointSettings {
    fn default() -> Self {
        Self {
            children_per_checkpoint: 2,
            budget_thresholds_pct: vec![50, 80],
            max_evaluation_attempts: 2,
        }
    }
}

impl CheckpointSettings {
    pub fn new(
        children_per_checkpoint: u32,
        budget_thresholds_pct: Vec<u8>,
        max_evaluation_attempts: u32,
    ) -> Result<Self, SettingsError> {
        let (lo, hi) = CHILDREN_PER_CHECKPOINT_RANGE;
        if !(lo..=hi).contains(&children_per_checkpoint) {
            return Err(SettingsError {
                field: "children_per_checkpoint",
                detail: format!("{children_per_checkpoint} は {lo}〜{hi} の外"),
            });
        }
        let (plo, phi) = THRESHOLD_RANGE;
        let in_range = budget_thresholds_pct
            .iter()
            .all(|p| (plo..=phi).contains(p));
        let ascending = budget_thresholds_pct.windows(2).all(|w| w[0] < w[1]);
        if budget_thresholds_pct.len() > THRESHOLDS_MAX || !in_range || !ascending {
            return Err(SettingsError {
                field: "budget_thresholds_pct",
                detail: format!(
                    "{budget_thresholds_pct:?} は {plo}〜{phi} の狭義の昇順・{THRESHOLDS_MAX} 件までの外"
                ),
            });
        }
        let (alo, ahi) = ATTEMPTS_RANGE;
        if !(alo..=ahi).contains(&max_evaluation_attempts) {
            return Err(SettingsError {
                field: "max_evaluation_attempts",
                detail: format!("{max_evaluation_attempts} は {alo}〜{ahi} の外"),
            });
        }
        Ok(Self {
            children_per_checkpoint,
            budget_thresholds_pct,
            max_evaluation_attempts,
        })
    }

    pub fn children_per_checkpoint(&self) -> u32 {
        self.children_per_checkpoint
    }

    pub fn budget_thresholds_pct(&self) -> &[u8] {
        &self.budget_thresholds_pct
    }

    pub fn max_evaluation_attempts(&self) -> u32 {
        self.max_evaluation_attempts
    }
}

// ---------------------------------------------------------------- 発火（D1.2・D1.3）

/// 子の `Transitioned` の 1 件（`to` と event の時刻。RFC 3339 の UTC で、文字列の順が時刻の順）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildTransition {
    pub to: Status,
    pub at: String,
}

/// root の直下の子 1 件。`transitions` は event の順。今の状態は最後の遷移で決まる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildFacts {
    pub task_id: String,
    pub transitions: Vec<ChildTransition>,
}

/// 戦略の文書の `ArtifactProduced` 1 件（root の subtree と root が参照する文書の task のもの）。
/// `bytes` は記録が引けなければ `None`。`at` は event の時刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyDocumentFacts {
    pub name: String,
    pub bytes: Option<Vec<u8>>,
    pub at: String,
}

/// root が参照する戦略（D1.1 の `StrategyRef` の id と版）と、集めた文書。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyFacts {
    pub strategy_id: String,
    pub version: u32,
    pub documents: Vec<StrategyDocumentFacts>,
}

/// 1 つの root の発火の事実。`strategy` が `None` の root（strategy root でない）は trigger を持たない。
/// `evaluated` は記録の `triggers`、`requested` は `evaluate` の判断（`pending`・`applied`・`suppressed`）の `triggers`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerFacts {
    pub strategy: Option<StrategyFacts>,
    pub children: Vec<ChildFacts>,
    pub tokens_used: u64,
    pub max_tokens: Option<u64>,
    pub evaluated: BTreeSet<String>,
    pub requested: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    Children,
    ChildFailed,
    Refute,
    Budget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerState {
    Due,
    Requested,
    Evaluated,
}

/// 条件が今真の trigger 1 件。`at` は条件が真になった event の時刻（予算は時刻を持たないので `None`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub key: String,
    pub kind: TriggerKind,
    pub state: TriggerState,
    pub at: Option<String>,
}

/// D1.2・D1.3: 条件が今真の trigger を状態つきで返す。順は `at` の昇順（`None` は後ろ）、同じなら key の順。
pub fn triggers(facts: &TriggerFacts, settings: &CheckpointSettings) -> Vec<Trigger> {
    let Some(strategy) = &facts.strategy else {
        return Vec::new();
    };
    let mut found: Vec<(TriggerKind, String, Option<String>)> = Vec::new();

    let mut terminal_at: Vec<&str> = Vec::new();
    for child in &facts.children {
        if let Some(last) = child.transitions.last()
            && last.to.is_terminal()
        {
            terminal_at.push(&last.at);
        }
        if let Some(failed) = child.transitions.iter().find(|t| t.to == Status::Failed) {
            found.push((
                TriggerKind::ChildFailed,
                format!("child_failed:{}", child.task_id),
                Some(failed.at.clone()),
            ));
        }
    }
    terminal_at.sort_unstable();
    let n = settings.children_per_checkpoint as usize;
    let k = terminal_at.len() / n * n;
    if k > 0 {
        found.push((
            TriggerKind::Children,
            format!("children:{k}"),
            terminal_at.get(k - 1).map(|at| (*at).to_string()),
        ));
    }

    found.extend(refute_triggers(strategy));

    if let Some(max) = facts.max_tokens.filter(|m| *m > 0) {
        let used = u128::from(facts.tokens_used) * 100;
        for p in &settings.budget_thresholds_pct {
            if used >= u128::from(*p) * u128::from(max) {
                found.push((TriggerKind::Budget, format!("budget:{p}"), None));
            }
        }
    }

    let mut out: Vec<Trigger> = found
        .into_iter()
        .map(|(kind, key, at)| {
            let state = if facts.evaluated.contains(&key) {
                TriggerState::Evaluated
            } else if facts.requested.contains(&key) {
                TriggerState::Requested
            } else {
                TriggerState::Due
            };
            Trigger {
                key,
                kind,
                state,
                at,
            }
        })
        .collect();
    out.sort_by(|a, b| (a.at.is_none(), &a.at, &a.key).cmp(&(b.at.is_none(), &b.at, &b.key)));
    out
}

/// 文書を読む。name の版・id と中身が一致しないもの、読めないものは `None`（D1.2「読めない文書は数えない」）。
fn read_document(d: &StrategyDocumentFacts, strategy_id: &str) -> Option<StrategyDocument> {
    let (id, version) = strategy_artifact(&d.name)?;
    let doc: StrategyDocument = serde_json::from_slice(d.bytes.as_deref()?).ok()?;
    (id == strategy_id && doc.strategy_id == id && doc.version == version).then_some(doc)
}

fn refute_ids(doc: &StrategyDocument) -> impl Iterator<Item = &str> {
    doc.evidence
        .iter()
        .filter(|e| e.stance == Stance::Refute)
        .map(|e| e.id.as_str())
}

/// D1.2 の反証: 参照した版より新しい版の、参照した版に無い refute の証拠。同じ証拠 id は最初に現れた版の key 1 つだけ。
fn refute_triggers(strategy: &StrategyFacts) -> Vec<(TriggerKind, String, Option<String>)> {
    let mut docs: Vec<(StrategyDocument, &str)> = strategy
        .documents
        .iter()
        .filter_map(|d| Some((read_document(d, &strategy.strategy_id)?, d.at.as_str())))
        .collect();
    docs.sort_by(|a, b| (a.0.version, a.1).cmp(&(b.0.version, b.1)));
    let known: BTreeSet<&str> = docs
        .iter()
        .filter(|(d, _)| d.version == strategy.version)
        .flat_map(|(d, _)| refute_ids(d))
        .collect();
    let mut first: BTreeMap<&str, (u32, &str)> = BTreeMap::new();
    for (doc, at) in docs.iter().filter(|(d, _)| d.version > strategy.version) {
        for id in refute_ids(doc).filter(|id| !known.contains(id)) {
            first.entry(id).or_insert((doc.version, at));
        }
    }
    first
        .into_iter()
        .map(|(id, (version, at))| {
            (
                TriggerKind::Refute,
                format!("refute:{}:v{version}:{id}", strategy.strategy_id),
                Some(at.to_string()),
            )
        })
        .collect()
}

// ---------------------------------------------------------------- 評価の形（D3.2）

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    OnTrack,
    AtRisk,
    OffTrack,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Next {
    Continue,
    DiscriminatingExperiment,
    Pivot,
    AskHuman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAxis {
    Succeeded,
    Partial,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicAxis {
    Advanced,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalAxis {
    Met,
    NotMet,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Axes {
    pub execution: ExecutionAxis,
    pub epistemic: EpistemicAxis,
    pub goal: GoalAxis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    pub claim: String,
    pub metric: String,
    pub before: String,
    pub after: String,
    #[serde(default)]
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Learned {
    pub statement: String,
    #[serde(default)]
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refuted {
    pub hypothesis: String,
    pub statement: String,
    #[serde(default)]
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uncertainty {
    pub statement: String,
    pub resolve_by: String,
}

/// D3.2: `ReviewOutput.parent_evaluation` の中身。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentEvaluation {
    pub schema: String,
    pub verdict: Verdict,
    pub next: Next,
    pub axes: Axes,
    #[serde(default)]
    pub measured: Vec<Measured>,
    #[serde(default)]
    pub learned: Vec<Learned>,
    #[serde(default)]
    pub refuted: Vec<Refuted>,
    #[serde(default)]
    pub uncertainty: Vec<Uncertainty>,
    pub reason: String,
}

impl ParentEvaluation {
    /// 支持の根拠になりうる主張（measured・learned・refuted）を `(path, 参照)` で返す。
    fn claims(&self) -> impl Iterator<Item = (String, &[Reference])> {
        let m = self.measured.iter().enumerate();
        let l = self.learned.iter().enumerate();
        let r = self.refuted.iter().enumerate();
        m.map(|(i, x)| (format!("measured[{i}]"), x.references.as_slice()))
            .chain(l.map(|(i, x)| (format!("learned[{i}]"), x.references.as_slice())))
            .chain(r.map(|(i, x)| (format!("refuted[{i}]"), x.references.as_slice())))
    }
}

// ---------------------------------------------------------------- 決定的な規則（D4）

/// root の終端の子の数（D4 R5）。`done_passed` は reviewer の合格つきの done。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildOutcomes {
    pub done_passed: u32,
    pub done_unpassed: u32,
    pub failed: u32,
    pub cancelled: u32,
}

/// D4 の事実（dispatcher が store から集める）。`resolved` は同じ案件で実在を確かめた参照（`task`・`artifact`・`commit`）。
/// `goal_reached` は Rust の D6（`recorded_outcomes`・`root_satisfies`）の値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationFacts {
    pub resolved: Vec<Reference>,
    pub goal_reached: bool,
    pub children: ChildOutcomes,
}

/// 違反の規則。`Shape`・`Limit` だけが記録を `invalid` にし、他は有効な判定を事実の側に直したことの記録。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// R1 形。
    Shape,
    /// R1 上限。
    Limit,
    /// R2 参照の無い・解決しない主張。
    Unsupported,
    /// R3 支持の無い on_track。
    OnTrackUnsupported,
    /// R4 goal の申告が D6 と合わない。
    GoalNotD6,
    /// R5 execution の申告が事実と合わない。
    ExecutionMismatch,
    /// R6 epistemic の申告が支持と合わない。
    EpistemicMismatch,
    /// R7 表に無い verdict と next の組。
    NextNotAllowed,
}

impl Rule {
    pub fn code(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            Self::Limit => "limit",
            Self::Unsupported => "unsupported",
            Self::OnTrackUnsupported => "on_track_unsupported",
            Self::GoalNotD6 => "goal_not_d6",
            Self::ExecutionMismatch => "execution_mismatch",
            Self::EpistemicMismatch => "epistemic_mismatch",
            Self::NextNotAllowed => "next_not_allowed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub rule: Rule,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Checked,
    Invalid,
}

/// D3.4 の `effective`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effective {
    pub verdict: Verdict,
    pub next: Next,
    pub axes: Axes,
    pub unsupported: u32,
    pub negative_result: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedEvaluation {
    pub status: RecordStatus,
    pub effective: Effective,
    pub violations: Vec<Violation>,
}

fn violation(rule: Rule, path: impl Into<String>, detail: impl Into<String>) -> Violation {
    Violation {
        rule,
        path: path.into(),
        detail: detail.into(),
    }
}

/// 支持の根拠になれる参照（外部の文・knowledge・chat は untrusted で数えない。子 2 D2.3 と同じ考え）。
fn is_internal(r: &Reference) -> bool {
    matches!(
        r,
        Reference::Task { .. } | Reference::Artifact { .. } | Reference::Commit { .. }
    )
}

/// D4 R5: 終端の子の事実から execution を決める。
fn execution_of(c: &ChildOutcomes) -> ExecutionAxis {
    let terminal = c.done_passed + c.done_unpassed + c.failed + c.cancelled;
    if terminal == 0 {
        ExecutionAxis::Unknown
    } else if c.failed > 0 {
        ExecutionAxis::Failed
    } else if c.done_passed == terminal {
        ExecutionAxis::Succeeded
    } else {
        ExecutionAxis::Partial
    }
}

fn parse(raw: &Value) -> Result<ParentEvaluation, Violation> {
    let bytes = serde_json::to_vec(raw).map_or(usize::MAX, |b| b.len());
    if bytes > EVALUATION_MAX_BYTES {
        return Err(violation(
            Rule::Limit,
            "parent_evaluation",
            format!("{bytes} byte は上限 {EVALUATION_MAX_BYTES} を超える"),
        ));
    }
    serde_json::from_value(raw.clone())
        .map_err(|e| violation(Rule::Shape, "parent_evaluation", e.to_string()))
}

fn text(out: &mut Vec<Violation>, path: String, value: &str, max: usize) {
    let chars = value.chars().count();
    if value.trim().is_empty() {
        out.push(violation(Rule::Shape, path, "空"));
    } else if chars > max {
        out.push(violation(
            Rule::Limit,
            path,
            format!("{chars} 文字は上限 {max} を超える"),
        ));
    }
}

fn list<T>(out: &mut Vec<Violation>, path: &str, items: &[T]) {
    if items.len() > LIST_MAX {
        out.push(violation(
            Rule::Limit,
            path,
            format!("{} 件は上限 {LIST_MAX} を超える", items.len()),
        ));
    }
}

fn refs(out: &mut Vec<Violation>, path: String, refs: &[Reference]) {
    if refs.len() > REFS_PER_ITEM_MAX {
        out.push(violation(
            Rule::Limit,
            path,
            format!("参照 {} 件は上限 {REFS_PER_ITEM_MAX} を超える", refs.len()),
        ));
    }
}

/// D4 R1: 形と上限。
fn shape_and_limits(e: &ParentEvaluation) -> Vec<Violation> {
    let mut out = Vec::new();
    if e.schema != EVALUATION_SCHEMA {
        out.push(violation(
            Rule::Shape,
            "schema",
            format!("{} は {EVALUATION_SCHEMA} でない", e.schema),
        ));
    }
    text(&mut out, "reason".into(), &e.reason, TEXT_MAX_CHARS);
    list(&mut out, "measured", &e.measured);
    list(&mut out, "learned", &e.learned);
    list(&mut out, "refuted", &e.refuted);
    list(&mut out, "uncertainty", &e.uncertainty);
    for (i, m) in e.measured.iter().enumerate() {
        let p = format!("measured[{i}]");
        text(&mut out, format!("{p}.claim"), &m.claim, TEXT_MAX_CHARS);
        text(&mut out, format!("{p}.metric"), &m.metric, SHORT_MAX_CHARS);
        text(&mut out, format!("{p}.before"), &m.before, SHORT_MAX_CHARS);
        text(&mut out, format!("{p}.after"), &m.after, SHORT_MAX_CHARS);
        refs(&mut out, format!("{p}.references"), &m.references);
    }
    for (i, l) in e.learned.iter().enumerate() {
        let p = format!("learned[{i}]");
        text(
            &mut out,
            format!("{p}.statement"),
            &l.statement,
            TEXT_MAX_CHARS,
        );
        refs(&mut out, format!("{p}.references"), &l.references);
    }
    for (i, r) in e.refuted.iter().enumerate() {
        let p = format!("refuted[{i}]");
        text(
            &mut out,
            format!("{p}.hypothesis"),
            &r.hypothesis,
            TEXT_MAX_CHARS,
        );
        text(
            &mut out,
            format!("{p}.statement"),
            &r.statement,
            TEXT_MAX_CHARS,
        );
        refs(&mut out, format!("{p}.references"), &r.references);
    }
    for (i, u) in e.uncertainty.iter().enumerate() {
        let p = format!("uncertainty[{i}]");
        text(
            &mut out,
            format!("{p}.statement"),
            &u.statement,
            TEXT_MAX_CHARS,
        );
        text(
            &mut out,
            format!("{p}.resolve_by"),
            &u.resolve_by,
            TEXT_MAX_CHARS,
        );
    }
    out
}

/// 呼び出し側が実在を引くべき参照（`task`・`artifact`・`commit`、重複なし、最初に現れた順）。形の崩れた評価は空。
pub fn references_to_resolve(raw: &Value) -> Vec<Reference> {
    let Ok(e) = serde_json::from_value::<ParentEvaluation>(raw.clone()) else {
        return Vec::new();
    };
    let mut out: Vec<Reference> = Vec::new();
    for (_, rs) in e.claims() {
        for r in rs.iter().filter(|r| is_internal(r)) {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
    }
    out
}

/// 形の崩れた・節の無い評価の結果（D3.4 の `status: invalid`）。execution と goal は事実のまま出す。
fn invalid(violations: Vec<Violation>, facts: &EvaluationFacts) -> CheckedEvaluation {
    CheckedEvaluation {
        status: RecordStatus::Invalid,
        effective: Effective {
            verdict: Verdict::Unknown,
            next: Next::AskHuman,
            axes: Axes {
                execution: execution_of(&facts.children),
                epistemic: EpistemicAxis::Unknown,
                goal: if facts.goal_reached {
                    GoalAxis::Met
                } else {
                    GoalAxis::Unknown
                },
            },
            unsupported: 0,
            negative_result: false,
        },
        violations,
    }
}

/// D4: reviewer の出力（`ReviewOutput.parent_evaluation`。`None` は節が無い）を事実と照らし、有効な判定を返す。
pub fn check_evaluation(raw: Option<&Value>, facts: &EvaluationFacts) -> CheckedEvaluation {
    let Some(raw) = raw else {
        return invalid(
            vec![violation(Rule::Shape, "parent_evaluation", "節が無い")],
            facts,
        );
    };
    let e = match parse(raw) {
        Ok(e) => e,
        Err(v) => return invalid(vec![v], facts),
    };
    let r1 = shape_and_limits(&e);
    if !r1.is_empty() {
        return invalid(r1, facts);
    }

    let mut violations = Vec::new();
    // R2
    let supported = |rs: &[Reference]| {
        rs.iter()
            .any(|r| is_internal(r) && facts.resolved.contains(r))
    };
    let mut unsupported = 0u32;
    for (path, rs) in e.claims() {
        if !supported(rs) {
            unsupported += 1;
            violations.push(violation(
                Rule::Unsupported,
                path,
                "解決した task・artifact・commit の参照が無い",
            ));
        }
    }
    let supported_measured = e
        .measured
        .iter()
        .filter(|m| supported(&m.references))
        .count();
    let supported_learned = e
        .learned
        .iter()
        .filter(|l| supported(&l.references))
        .count();
    let supported_refuted = e
        .refuted
        .iter()
        .filter(|r| supported(&r.references))
        .count();

    // R4
    let goal = match (facts.goal_reached, e.axes.goal) {
        (true, GoalAxis::NotMet) => {
            violations.push(violation(
                Rule::GoalNotD6,
                "axes.goal",
                "D6 で到達済みなのに not_met",
            ));
            GoalAxis::Met
        }
        (true, _) => GoalAxis::Met,
        (false, GoalAxis::Met) => {
            unsupported += 1;
            violations.push(violation(
                Rule::GoalNotD6,
                "axes.goal",
                "met は D6 の到達だけが決める（自己申告）",
            ));
            GoalAxis::Unknown
        }
        (false, claimed) => claimed,
    };

    // R3
    let verdict = if e.verdict == Verdict::OnTrack && (supported_measured == 0 || unsupported > 0) {
        violations.push(violation(
            Rule::OnTrackUnsupported,
            "verdict",
            format!(
                "on_track には支持された measured が 1 件以上・unsupported 0 が要る（measured {supported_measured}・unsupported {unsupported}）"
            ),
        ));
        Verdict::Unknown
    } else {
        e.verdict
    };

    // R5
    let execution = execution_of(&facts.children);
    if e.axes.execution != execution {
        violations.push(violation(
            Rule::ExecutionMismatch,
            "axes.execution",
            format!("申告 {:?}・事実 {execution:?}", e.axes.execution),
        ));
    }

    // R6
    let epistemic = if supported_learned + supported_refuted > 0 {
        EpistemicAxis::Advanced
    } else if e.axes.epistemic == EpistemicAxis::Advanced {
        EpistemicAxis::Unknown
    } else {
        e.axes.epistemic
    };
    if epistemic != e.axes.epistemic {
        violations.push(violation(
            Rule::EpistemicMismatch,
            "axes.epistemic",
            format!("申告 {:?}・支持から {epistemic:?}", e.axes.epistemic),
        ));
    }

    // R7
    let next =
        if e.next == Next::Continue && matches!(verdict, Verdict::OffTrack | Verdict::Unknown) {
            violations.push(violation(
                Rule::NextNotAllowed,
                "next",
                format!("{verdict:?} と continue の組は D5.2 の表に無い"),
            ));
            Next::AskHuman
        } else {
            e.next
        };

    let negative_result = execution == ExecutionAxis::Succeeded
        && epistemic == EpistemicAxis::Advanced
        && goal == GoalAxis::NotMet
        && supported_refuted > 0;
    CheckedEvaluation {
        status: RecordStatus::Checked,
        effective: Effective {
            verdict,
            next,
            axes: Axes {
                execution,
                epistemic,
                goal,
            },
            unsupported,
            negative_result,
        },
        violations,
    }
}

// ---------------------------------------------------------------- 記録（D3.4）

/// 記録の成果物の name（`parent-evaluation/<判断 id>.json`）。
pub fn evaluation_artifact_name(decision_id: &str) -> String {
    format!("{EVALUATION_ARTIFACT_PREFIX}{decision_id}.json")
}

/// name が記録の成果物なら判断 id。[`evaluation_artifact_name`] で作り直して一致するものだけ。
pub fn evaluation_decision_id(name: &str) -> Option<&str> {
    let id = name
        .strip_prefix(EVALUATION_ARTIFACT_PREFIX)?
        .strip_suffix(".json")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// D3.4: root の成果物 `parent-evaluation/<判断 id>.json` の中身。`evaluation` は `checked` のときだけ写す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentEvaluationRecord {
    pub schema: String,
    pub decision_id: String,
    pub root_id: String,
    pub triggers: Vec<String>,
    pub status: RecordStatus,
    pub evaluation: Option<Value>,
    pub effective: Effective,
    pub violations: Vec<String>,
}

impl ParentEvaluationRecord {
    pub fn new(
        decision_id: &str,
        root_id: &str,
        triggers: Vec<String>,
        evaluation: Option<Value>,
        checked: &CheckedEvaluation,
    ) -> Self {
        Self {
            schema: RECORD_SCHEMA.to_string(),
            decision_id: decision_id.to_string(),
            root_id: root_id.to_string(),
            triggers,
            status: checked.status,
            evaluation: evaluation.filter(|_| checked.status == RecordStatus::Checked),
            effective: checked.effective.clone(),
            violations: checked
                .violations
                .iter()
                .map(|v| format!("{} {}: {}", v.rule.code(), v.path, v.detail))
                .collect(),
        }
    }
}

// ---------------------------------------------------------------- 写しの表（D5.2）

/// D5.2 の表の行動。kind は `ProjectAction::kind()` と同じ文字列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedAction {
    Continue,
    Wait,
    Replan {
        reason_code: &'static str,
    },
    /// question_key は [`checkpoint_question_key`] の値。
    Escalate,
}

impl AllowedAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Wait => "wait",
            Self::Replan { .. } => "replan",
            Self::Escalate => "escalate",
        }
    }
}

/// D5.2: まだ写していない評価に対して許す行動。表に無い組（off_track・unknown と continue）は ask_human の行。
pub fn allowed_actions(status: RecordStatus, effective: &Effective) -> Vec<AllowedAction> {
    use AllowedAction::*;
    if status == RecordStatus::Invalid {
        return vec![Escalate];
    }
    match (effective.verdict, effective.next) {
        (Verdict::OnTrack | Verdict::AtRisk, Next::Continue) => vec![Continue, Wait],
        (_, Next::DiscriminatingExperiment) => vec![
            Replan {
                reason_code: REASON_EXPERIMENT,
            },
            Escalate,
        ],
        (_, Next::Pivot) => vec![
            Replan {
                reason_code: REASON_PIVOT,
            },
            Escalate,
        ],
        (_, Next::AskHuman) | (Verdict::OffTrack | Verdict::Unknown, Next::Continue) => {
            vec![Escalate]
        }
    }
}

/// D5.2: checkpoint の escalate の question_key（`checkpoint:<判断 id>`）。
pub fn checkpoint_question_key(decision_id: &str) -> String {
    format!("{QUESTION_KEY_PREFIX}{decision_id}")
}

/// 行動（kind・replan の reason_code・escalate の question_key）が表の行動のどれかに当たるか。
pub fn action_allowed(
    allowed: &[AllowedAction],
    decision_id: &str,
    kind: &str,
    reason_code: Option<&str>,
    question_key: Option<&str>,
) -> bool {
    allowed.iter().any(|a| {
        a.kind() == kind
            && match a {
                AllowedAction::Continue | AllowedAction::Wait => true,
                AllowedAction::Replan { reason_code: rc } => reason_code == Some(*rc),
                AllowedAction::Escalate => {
                    question_key == Some(checkpoint_question_key(decision_id).as_str())
                }
            }
    })
}

#[cfg(test)]
mod tests;
