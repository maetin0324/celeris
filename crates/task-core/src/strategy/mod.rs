//! ADR 2026-11-21-strategy-evidence-contract: 戦略・仮説・証拠の文書 `celeris.strategy/1` の型と検証の純関数。
//!
//! 文書は task の成果物（`strategy/<strategy_id>/v<version>.json`）として置き、承認は既存の決定（ADR-0079 D7）で行う。
//! この module は store を読まない。照合に要る事実（今の Project.request の sha256・承認済み Roadmap の版・成果物の記録・
//! 前の版の file・承認の決定・参照の解決）は呼び出し側（CoS の `task.create` の検査）が [`CheckContext`] に入れて渡す。
//! 検査は最初の違反で止まらず、違反を規則（[`Rule`]）つきで全部返す。

pub mod envelope;
pub mod review;
#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::chat_citation::{
    ChatCitation, ChatCitationState, CitedMessage, citation_shape_ok, citation_state,
};
use crate::decision::DecisionStatus;
use crate::project_proposal::sha256_hex;
use crate::roadmap::ROADMAP_TEXT_MAX;
pub use review::{
    CandidateState, ExecutionFacts, Experiment, ExperimentBudget, ExperimentTask, HeldAssumption,
    check_execution,
};

pub const SCHEMA: &str = "celeris.strategy/1";
/// D1.3: 文書の file の上限（byte、UTF-8）。
pub const DOCUMENT_MAX_BYTES: usize = 262_144;
pub const QUESTION_MAX_CHARS: usize = 4_000;
/// 題・方針・判定方法・主張・反証の条件・変更の理由・費用の注（Roadmap と同じ上限）。
pub const TEXT_MAX_CHARS: usize = ROADMAP_TEXT_MAX;
pub const LIST_ITEM_MAX_CHARS: usize = 500;
pub const LABEL_MAX_CHARS: usize = 200;
pub const ID_MAX_CHARS: usize = 40;
pub const CANDIDATES_MAX: usize = 5;
pub const HYPOTHESES_MAX: usize = 32;
pub const EVIDENCE_MAX: usize = 256;
pub const RISKS_MAX: usize = 16;
pub const REEVALUATE_MAX: usize = 8;
pub const REFS_PER_ITEM_MAX: usize = 16;
pub const REFS_TOTAL_MAX: usize = 1_024;
pub const URL_MAX_CHARS: usize = 2_048;
/// 版の上限（gate が鎖を辿る回数の上限も同じ）。
pub const VERSION_MAX: u32 = 64;
/// D2.2: 承認の決定の選択肢。
pub const APPROVE_OPTION: &str = "approve";
pub const REJECT_OPTION: &str = "reject";

/// D1.1: 1 つの戦略の 1 版。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyDocument {
    pub schema: String,
    pub project_id: String,
    pub strategy_id: String,
    pub version: u32,
    pub supersedes: Option<ArtifactPointer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_reason: Option<String>,
    pub idempotency_key: String,
    /// 欠けは型の誤りでなく出典の欠損（[`Rule::SourceMissing`]）として返す。
    #[serde(default)]
    pub source: Option<Source>,
    pub actor: Actor,
    pub source_revision: SourceRevision,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<Consent>,
    pub acl: Acl,
    pub decision_state: DecisionState,
    pub research_question: ResearchQuestion,
    pub candidates: Vec<Candidate>,
    /// ADR 2026-11-22-strategy-review-critic D1.2: 保留する前提。
    pub held_assumptions: Vec<HeldAssumption>,
    pub hypotheses: Vec<Hypothesis>,
    pub evidence: Vec<Evidence>,
}

/// 成果物の参照（`supersedes` と `artifact` の参照の本体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPointer {
    pub task_id: String,
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    CelerisCosThread,
    HumanPaste,
    McpClient,
    MarkdownImport,
    /// この案件の task の run が成果物として作った（文書だけ。envelope では使えない）。
    CelerisTaskRun,
}

impl SourceKind {
    /// 外部の文（untrusted）。consent が要り、状態を進められない（brief R8・R11）。
    pub fn is_external(self) -> bool {
        matches!(
            self,
            Self::HumanPaste | Self::McpClient | Self::MarkdownImport
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub kind: SourceKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Cos,
    Mcp,
    Worker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub kind: ActorKind,
    pub id: String,
}

/// source と actor の組（brief R3。`celeris_task_run` は worker だけ）。envelope と文書が同じ表を使う。
pub fn actor_allowed(source: SourceKind, actor: ActorKind) -> bool {
    match source {
        SourceKind::CelerisCosThread => matches!(actor, ActorKind::Human | ActorKind::Cos),
        SourceKind::HumanPaste | SourceKind::MarkdownImport => actor == ActorKind::Human,
        SourceKind::McpClient => actor == ActorKind::Mcp,
        SourceKind::CelerisTaskRun => actor == ActorKind::Worker,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRevision {
    pub request_sha256: String,
    pub roadmap_version: Option<i64>,
}

/// 外部の文に人が与えた許可（brief R8）。欄の欠けは [`consent_granted`] が拒否する。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    #[serde(default)]
    pub granted_by: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub granted_at: String,
}

/// 人（`human:<名前>`）が、選んだ抜粋か要約だけを許したか。
pub fn consent_granted(consent: &Consent) -> bool {
    crate::is_human_principal(&consent.granted_by)
        && matches!(consent.scope.as_str(), "selected_excerpt" | "summary")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Internal,
    Confidential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acl {
    pub classification: Classification,
    #[serde(default)]
    pub outbound: bool,
}

/// 作った側の状態。`approved`・`rejected` は読めるが書けない（[`Rule::StateNotWritable`]。承認の正本は決定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionState {
    Discussion,
    Proposed,
    Withdrawn,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchQuestion {
    pub text: String,
    #[serde(default)]
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub approach: String,
    pub decision_method: String,
    pub cost: Cost,
    #[serde(default)]
    pub risks: Vec<String>,
    pub reevaluate_when: Vec<String>,
    /// 候補の根拠（仮説の id）。空は出典の欠損。
    #[serde(default)]
    pub hypotheses: Vec<String>,
    #[serde(default)]
    pub selected: bool,
    /// ADR 2026-11-22-strategy-review-critic D1.1: 候補の採否（文書の承認の状態とは別）。
    pub state: CandidateState,
    /// `rejected` の根拠（同じ文書の、候補の仮説の `refute` の証拠の id）。
    #[serde(default)]
    pub rejected_by: Vec<String>,
    /// 最小の識別実験。
    pub experiment: Experiment,
    pub expected_information_value: String,
    /// 停止条件。
    pub stop_when: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub usd_max: f64,
    pub wall_hours_max: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Confirmed,
    Estimated,
    Contradicted,
    NeedsDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    pub confidence: Confidence,
    pub falsified_by: String,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Support,
    Refute,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub id: String,
    pub stance: Stance,
    pub hypothesis: String,
    pub statement: String,
    pub observed_at: String,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub references: Vec<Reference>,
}

/// D1.2: 参照の 6 種。`chat` は chat の引用（ADR 2026-11-21-chat-discussion-mode D3・D6。revision と本文の sha256 が要る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reference {
    Commit {
        repo: String,
        sha: String,
    },
    Url {
        url: String,
    },
    Task {
        task_id: String,
    },
    Knowledge {
        path: String,
        sha256: String,
    },
    Chat(ChatCitation),
    Artifact {
        task_id: String,
        name: String,
        sha256: String,
    },
}

impl Reference {
    /// 外部でない参照（`confirmed` の仮説の根拠になれる。D2.3）。
    fn is_internal(&self) -> bool {
        matches!(
            self,
            Self::Commit { .. } | Self::Artifact { .. } | Self::Task { .. }
        )
    }
}

/// 検査の目的。discussion note の記録は承認を求めず、実行（CoS の `task.create`）は人の承認を求める（D2.1・D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Record,
    Execute,
}

/// 文書の成果物の記録（`ArtifactProduced`）。`sha256` は dispatcher が計算した値（D1.1 の content_hash）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFacts {
    pub name: String,
    pub sha256: String,
    pub run_id: String,
}

/// 参照を store で引いた結果。`project_id` が `None` は案件の無いもの（統括の thread・案件でない scope の page）。
/// `chat` の参照は引いた message の今の事実（[`RefStatus::Cited`]）で、状態は検査のたびに導く（保存しない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefStatus {
    Missing,
    Found { project_id: Option<String> },
    Cited(CitedMessage),
}

impl RefStatus {
    fn project_id(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Found { project_id } => project_id.as_deref(),
            Self::Cited(m) => m.project_id.as_deref(),
        }
    }
}

/// 前の版の 1 つ。`bytes` は記録が引けなければ `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousVersion {
    pub pointer: ArtifactPointer,
    pub bytes: Option<Vec<u8>>,
    /// 前の版を作った task の案件。
    pub task_project_id: Option<String>,
}

/// 承認に使う決定（文書を作った task の `decisions` の行と、最後の `DecisionAnswered`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRecord {
    pub key: String,
    pub question: String,
    pub status: DecisionStatus,
    /// 最後の回答の `(option, by)`。
    pub last_answer: Option<(String, String)>,
}

/// 照合の文脈（呼び出し側が store から集める）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckContext {
    pub intent: Intent,
    /// 文書を作った task の案件。
    pub project_id: String,
    /// 今の `Project.request` の [`sha256_hex`]。
    pub request_sha256: String,
    /// 今の承認済み Roadmap の版（無ければ `None`）。
    pub roadmap_version: Option<i64>,
    pub artifact: ArtifactFacts,
    /// 同じ idempotency の key（同じ name の成果物・同じ版の承認の決定）に記録された sha256 の全件。
    pub key_sha256s: Vec<String>,
    /// 前の版の鎖。`chain[0]` が `version - 1`。
    pub chain: Vec<PreviousVersion>,
    pub approval: Option<ApprovalRecord>,
    /// 同じ `strategy_id` で人が承認した最大の版。
    pub latest_approved_version: Option<u32>,
    /// 文書の参照（`url` を除く）を引いた結果。載っていない参照は引けなかったものとして扱う。
    pub references: Vec<(Reference, RefStatus)>,
}

/// 検査の規則。`code()` は CoS の `task.create` が返す 422 の code（D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Required,
    Shape,
    Limit,
    Secret,
    StateNotWritable,
    Actor,
    Consent,
    UnsupportedClaim,
    SourceMissing,
    ContentHash,
    Stale,
    OtherProject,
    Unapproved,
    Outbound,
    Supersedes,
    RefuteRemoved,
    Idempotency,
    /// ADR 2026-11-22-strategy-review-critic D5 の 4 つ。
    ExperimentPhase,
    Uncritiqued,
    RevisitRejected,
    FailureUnread,
}

impl Rule {
    pub fn code(self) -> &'static str {
        match self {
            Self::SourceMissing | Self::ContentHash => "strategy_source_missing",
            Self::Stale => "strategy_stale",
            Self::OtherProject => "strategy_other_project",
            Self::Unapproved | Self::Outbound => "strategy_unapproved",
            Self::RefuteRemoved => "strategy_refute_removed",
            Self::Idempotency => "strategy_idempotency_conflict",
            Self::ExperimentPhase => "strategy_experiment_phase",
            Self::Uncritiqued => "strategy_uncritiqued",
            Self::RevisitRejected => "strategy_revisit_rejected",
            Self::FailureUnread => "strategy_failure_unread",
            Self::Required
            | Self::Shape
            | Self::Limit
            | Self::Secret
            | Self::StateNotWritable
            | Self::Actor
            | Self::Consent
            | Self::UnsupportedClaim
            | Self::Supersedes => "strategy_invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub rule: Rule,
    pub path: String,
    pub detail: String,
}

/// 成果物の name（D3.1）。
pub fn artifact_name(strategy_id: &str, version: u32) -> String {
    format!("strategy/{strategy_id}/v{version}.json")
}

/// D1.4: 文書の idempotency の key。
pub fn idempotency_key(strategy_id: &str, version: u32) -> String {
    format!("strategy:{strategy_id}:v{version}")
}

/// D2.2: 承認の決定の key（決定の key の上限 32 文字に収まる）。
pub fn approval_decision_key(version: u32) -> String {
    format!("strategy-approve-v{version}")
}

/// D2.2: 承認の決定の問い。名前と sha256 を 1 回ずつ書く（[`approval_subject`] が読む形の唯一の定義）。
pub fn approval_question(strategy_id: &str, version: u32, sha256: &str) -> String {
    format!(
        "{}（sha256:{sha256}）を承認しますか。{APPROVE_OPTION} で承認、{REJECT_OPTION} で却下します。",
        artifact_name(strategy_id, version)
    )
}

/// 承認の決定の問いが指す文書。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalSubject {
    pub strategy_id: String,
    pub version: u32,
    pub sha256: String,
}

static SUBJECT_NAME: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"strategy/([a-z0-9-]{1,40})/v([0-9]{1,2})\.json").ok());
static SUBJECT_SHA: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"sha256:([0-9a-f]{64})").ok());
static SECRET: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(bearer\s+[a-z0-9._-]{16,}|password\s*[:=]|cookie\s*[:=]|set-cookie|https://hooks\.|sk-[a-z0-9]{20,}|ghp_[a-z0-9]{20,}|xox[bp]-)",
    )
    .ok()
});

/// 問いから文書の name と sha256 を読む。名前と sha256 がちょうど 1 回ずつ現れるときだけ `Some`。
pub fn approval_subject(question: &str) -> Option<ApprovalSubject> {
    let (name_re, sha_re) = (SUBJECT_NAME.as_ref()?, SUBJECT_SHA.as_ref()?);
    let names: Vec<_> = name_re.captures_iter(question).collect();
    let shas: Vec<_> = sha_re.captures_iter(question).collect();
    let ([name], [sha]) = (names.as_slice(), shas.as_slice()) else {
        return None;
    };
    Some(ApprovalSubject {
        strategy_id: name[1].to_string(),
        version: name[2].parse().ok()?,
        sha256: sha[1].to_string(),
    })
}

/// 秘密に見える文字列（brief R9 の token・cookie・webhook の形）。正規表現が組めなければ fail-closed で `true`。
pub fn contains_secret(text: &str) -> bool {
    SECRET.as_ref().is_none_or(|re| re.is_match(text))
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_id(s: &str) -> bool {
    (1..=ID_MAX_CHARS).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_rfc3339(s: &str) -> bool {
    OffsetDateTime::parse(s, &Rfc3339).is_ok()
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn bare_sha(s: &str) -> &str {
    s.strip_prefix("sha256:").unwrap_or(s)
}

/// 文書の file の byte 列を検査する。違反が無ければ空の `Vec`。
pub fn validate_document(bytes: &[u8], ctx: &CheckContext) -> Vec<Violation> {
    let mut out = Vec::new();
    let Some((text, doc)) = parse_document(bytes, &mut out) else {
        return out;
    };
    let input = Input::new(text, &doc, ctx);
    for (_, check) in CHECKS {
        check(&input, &mut out);
    }
    out
}

fn push(out: &mut Vec<Violation>, rule: Rule, path: impl Into<String>, detail: impl Into<String>) {
    out.push(Violation {
        rule,
        path: path.into(),
        detail: detail.into(),
    });
}

/// 共有フォーマットの必須の欄（`source` の欠けは出典の欠損として別に返す）。
const REQUIRED: [&str; 16] = [
    "schema",
    "project_id",
    "strategy_id",
    "version",
    "supersedes",
    "idempotency_key",
    "actor",
    "source_revision",
    "timestamp",
    "acl",
    "decision_state",
    "research_question",
    "candidates",
    "held_assumptions",
    "hypotheses",
    "evidence",
];

fn parse_document<'b>(
    bytes: &'b [u8],
    out: &mut Vec<Violation>,
) -> Option<(&'b str, StrategyDocument)> {
    if bytes.len() > DOCUMENT_MAX_BYTES {
        push(
            out,
            Rule::Limit,
            "",
            format!(
                "文書が {} byte で上限 {DOCUMENT_MAX_BYTES} を超える",
                bytes.len()
            ),
        );
        return None;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        push(out, Rule::Shape, "", "文書が UTF-8 でない");
        return None;
    };
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            push(out, Rule::Shape, "", format!("JSON でない: {e}"));
            return None;
        }
    };
    let Some(obj) = value.as_object() else {
        push(out, Rule::Shape, "", "文書が JSON の object でない");
        return None;
    };
    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|k| !obj.contains_key(*k))
        .collect();
    for key in &missing {
        push(out, Rule::Required, *key, "必須の欄が無い");
    }
    match serde_json::from_value::<StrategyDocument>(value) {
        Ok(doc) => Some((text, doc)),
        Err(e) => {
            if missing.is_empty() {
                push(out, Rule::Shape, "", format!("欄の型・語彙が違う: {e}"));
            }
            None
        }
    }
}

struct Input<'a> {
    text: &'a str,
    doc: &'a StrategyDocument,
    ctx: &'a CheckContext,
    /// `ctx.chain` と同じ並びの前の版の文書（引けない・読めないものは `None`）。
    prev: Vec<Option<StrategyDocument>>,
}

impl<'a> Input<'a> {
    fn new(text: &'a str, doc: &'a StrategyDocument, ctx: &'a CheckContext) -> Self {
        let prev = ctx
            .chain
            .iter()
            .map(|link| {
                link.bytes
                    .as_deref()
                    .and_then(|b| serde_json::from_slice(b).ok())
            })
            .collect();
        Self {
            text,
            doc,
            ctx,
            prev,
        }
    }

    /// 鎖に要る前の版の数。
    fn expected_prev(&self) -> usize {
        self.doc.version.clamp(1, VERSION_MAX) as usize - 1
    }

    fn refs(&self) -> impl Iterator<Item = (String, &'a Reference)> {
        let doc = self.doc;
        let rq = doc
            .research_question
            .references
            .iter()
            .enumerate()
            .map(|(j, r)| (format!("research_question.references[{j}]"), r));
        let ev = doc.evidence.iter().enumerate().flat_map(|(i, e)| {
            e.references
                .iter()
                .enumerate()
                .map(move |(j, r)| (format!("evidence[{i}].references[{j}]"), r))
        });
        rq.chain(ev)
    }

    fn lookup(&self, reference: &Reference) -> Option<&'a RefStatus> {
        self.ctx
            .references
            .iter()
            .find(|(r, _)| r == reference)
            .map(|(_, s)| s)
    }

    /// chat の引用の今の状態（ADR 2026-11-21-chat-discussion-mode D6 の導出。要約の引用と同じ関数）。
    fn chat_state(&self, reference: &Reference, c: &ChatCitation) -> ChatCitationState {
        let now = match self.lookup(reference) {
            Some(RefStatus::Cited(m)) => Some(m),
            _ => None,
        };
        citation_state(c, now)
    }

    /// D2.2 の 1〜5。満たさなければ理由。
    fn human_approval(&self) -> Result<(), String> {
        let Some(a) = &self.ctx.approval else {
            return Err("承認の決定が無い".into());
        };
        let doc = self.doc;
        if a.key != approval_decision_key(doc.version) {
            return Err(format!(
                "決定の key {} が {} でない",
                a.key,
                approval_decision_key(doc.version)
            ));
        }
        let want = ApprovalSubject {
            strategy_id: doc.strategy_id.clone(),
            version: doc.version,
            sha256: bare_sha(&self.ctx.artifact.sha256).to_string(),
        };
        if approval_subject(&a.question).as_ref() != Some(&want) {
            return Err("決定の問いの name・sha256 が文書と違う".into());
        }
        if a.status != DecisionStatus::Answered {
            return Err(format!("決定が answered でない（{}）", a.status.as_str()));
        }
        match &a.last_answer {
            Some((option, by)) if option == APPROVE_OPTION && crate::is_human_principal(by) => {
                Ok(())
            }
            Some((option, _)) if option != APPROVE_OPTION => Err(format!("最後の回答が {option}")),
            Some((_, by)) => Err(format!("承認した主体 {by} が人（human:<名前>）でない")),
            None => Err("回答が無い".into()),
        }
    }
}

type Check = fn(&Input<'_>, &mut Vec<Violation>);

/// 検査の表。各関数は表の規則だけを返す（ablation の試験が 1 規則ずつ外して確かめる）。
const CHECKS: [(Rule, Check); 16] = [
    (Rule::Shape, check_shape),
    (Rule::Limit, check_limits),
    (Rule::ContentHash, check_content_hash),
    (Rule::Secret, check_secret),
    (Rule::StateNotWritable, check_state),
    (Rule::Actor, check_actor),
    (Rule::Consent, check_consent),
    (Rule::UnsupportedClaim, check_claims),
    (Rule::SourceMissing, check_sources),
    (Rule::Stale, check_stale),
    (Rule::OtherProject, check_project),
    (Rule::Unapproved, check_approval),
    (Rule::Outbound, check_outbound),
    (Rule::Supersedes, check_supersedes),
    (Rule::RefuteRemoved, check_refute),
    (Rule::Idempotency, check_idempotency),
];

fn check_shape(input: &Input<'_>, out: &mut Vec<Violation>) {
    let (doc, ctx) = (input.doc, input.ctx);
    let mut bad = |path: String, detail: &str| push(out, Rule::Shape, path, detail);
    if doc.schema != SCHEMA {
        bad("schema".into(), "schema が celeris.strategy/1 でない");
    }
    if !is_id(&doc.strategy_id) {
        bad(
            "strategy_id".into(),
            "strategy_id が [a-z0-9-]{1,40} でない",
        );
    }
    if doc.idempotency_key != idempotency_key(&doc.strategy_id, doc.version) {
        bad(
            "idempotency_key".into(),
            "idempotency_key が strategy:<strategy_id>:v<version> でない",
        );
    }
    if ctx.artifact.name != artifact_name(&doc.strategy_id, doc.version) {
        bad(
            "".into(),
            "成果物の name が strategy/<strategy_id>/v<version>.json でない",
        );
    }
    if !is_rfc3339(&doc.timestamp) {
        bad("timestamp".into(), "timestamp が RFC 3339 でない");
    }
    if !is_hex(&doc.source_revision.request_sha256, 64) {
        bad(
            "source_revision.request_sha256".into(),
            "64 桁の小文字 hex でない",
        );
    }
    let ids = |kind: &str, list: Vec<&str>, bad: &mut dyn FnMut(String, &str)| {
        let mut seen = std::collections::BTreeSet::new();
        for (i, id) in list.into_iter().enumerate() {
            if !is_id(id) || !seen.insert(id) {
                bad(
                    format!("{kind}[{i}].id"),
                    "id が [a-z0-9-]{1,40} でないか重複する",
                );
            }
        }
    };
    ids(
        "candidates",
        doc.candidates.iter().map(|c| c.id.as_str()).collect(),
        &mut bad,
    );
    ids(
        "hypotheses",
        doc.hypotheses.iter().map(|h| h.id.as_str()).collect(),
        &mut bad,
    );
    ids(
        "evidence",
        doc.evidence.iter().map(|e| e.id.as_str()).collect(),
        &mut bad,
    );
    if doc.candidates.iter().filter(|c| c.selected).count() > 1 {
        bad("candidates".into(), "selected の候補が 2 件以上");
    }
    let hyp = |id: &str| doc.hypotheses.iter().find(|h| h.id == id);
    for (i, c) in doc.candidates.iter().enumerate() {
        if c.hypotheses.iter().any(|h| hyp(h).is_none()) {
            bad(format!("candidates[{i}].hypotheses"), "無い仮説を指す");
        }
        if ![c.cost.usd_max, c.cost.wall_hours_max]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
        {
            bad(
                format!("candidates[{i}].cost"),
                "費用が 0 以上の有限の数でない",
            );
        }
    }
    for (i, h) in doc.hypotheses.iter().enumerate() {
        let linked = |id: &String| {
            doc.evidence
                .iter()
                .any(|e| &e.id == id && e.hypothesis == h.id)
        };
        if !h.evidence.iter().all(linked) {
            bad(
                format!("hypotheses[{i}].evidence"),
                "無い証拠か、別の仮説の証拠を指す",
            );
        }
    }
    for (i, e) in doc.evidence.iter().enumerate() {
        if hyp(&e.hypothesis).is_none() {
            bad(format!("evidence[{i}].hypothesis"), "無い仮説を指す");
        }
        if !is_rfc3339(&e.observed_at) {
            bad(
                format!("evidence[{i}].observed_at"),
                "observed_at が RFC 3339 でない",
            );
        }
    }
    for (path, r) in input.refs() {
        let ok = match r {
            Reference::Commit { repo, sha } => !repo.is_empty() && is_hex(sha, 40),
            Reference::Url { url } => url.strip_prefix("https://").is_some_and(|rest| {
                let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
                !host.is_empty() && !host.contains('@')
            }),
            Reference::Task { task_id } => !task_id.is_empty(),
            Reference::Knowledge { path, sha256 } => !path.is_empty() && is_hex(sha256, 64),
            Reference::Chat(c) => citation_shape_ok(c),
            Reference::Artifact {
                task_id,
                name,
                sha256,
            } => !task_id.is_empty() && !name.is_empty() && is_hex(sha256, 64),
        };
        if !ok {
            bad(
                path,
                "参照の形が違う（commit は 40 桁 hex、url は userinfo の無い https、sha256 は 64 桁 hex、chat は revision が 1 以上）",
            );
        }
    }
}

fn check_limits(input: &Input<'_>, out: &mut Vec<Violation>) {
    let doc = input.doc;
    if !(1..=VERSION_MAX).contains(&doc.version) {
        push(
            out,
            Rule::Limit,
            "version",
            format!("version が 1〜{VERSION_MAX} の外"),
        );
    }
    let mut text = |path: String, s: &str, max: usize| {
        if s.trim().is_empty() {
            push(out, Rule::Limit, path, "空の文");
        } else if chars(s) > max {
            push(
                out,
                Rule::Limit,
                path,
                format!("{} 文字で上限 {max} を超える", chars(s)),
            );
        }
    };
    text(
        "research_question.text".into(),
        &doc.research_question.text,
        QUESTION_MAX_CHARS,
    );
    if let Some(reason) = &doc.change_reason {
        text("change_reason".into(), reason, TEXT_MAX_CHARS);
    }
    let sources = doc
        .source
        .iter()
        .map(|s| ("source.label".to_string(), s))
        .chain(doc.evidence.iter().enumerate().filter_map(|(i, e)| {
            e.source
                .as_ref()
                .map(|s| (format!("evidence[{i}].source.label"), s))
        }));
    for (path, s) in sources {
        text(path, &s.label, LABEL_MAX_CHARS);
    }
    for (i, c) in doc.candidates.iter().enumerate() {
        for (k, s) in [
            ("title", c.title.as_str()),
            ("approach", &c.approach),
            ("decision_method", &c.decision_method),
        ] {
            text(format!("candidates[{i}].{k}"), s, TEXT_MAX_CHARS);
        }
        if let Some(note) = &c.cost.note {
            text(format!("candidates[{i}].cost.note"), note, TEXT_MAX_CHARS);
        }
        for (j, r) in c.risks.iter().enumerate() {
            text(
                format!("candidates[{i}].risks[{j}]"),
                r,
                LIST_ITEM_MAX_CHARS,
            );
        }
        for (j, r) in c.reevaluate_when.iter().enumerate() {
            text(
                format!("candidates[{i}].reevaluate_when[{j}]"),
                r,
                LIST_ITEM_MAX_CHARS,
            );
        }
    }
    for (i, h) in doc.hypotheses.iter().enumerate() {
        text(
            format!("hypotheses[{i}].statement"),
            &h.statement,
            TEXT_MAX_CHARS,
        );
        text(
            format!("hypotheses[{i}].falsified_by"),
            &h.falsified_by,
            TEXT_MAX_CHARS,
        );
    }
    for (i, e) in doc.evidence.iter().enumerate() {
        text(
            format!("evidence[{i}].statement"),
            &e.statement,
            TEXT_MAX_CHARS,
        );
    }
    let mut count = |path: String, n: usize, min: usize, max: usize| {
        if !(min..=max).contains(&n) {
            push(
                out,
                Rule::Limit,
                path,
                format!("{n} 件で範囲 {min}〜{max} の外"),
            );
        }
    };
    count("candidates".into(), doc.candidates.len(), 1, CANDIDATES_MAX);
    count("hypotheses".into(), doc.hypotheses.len(), 1, HYPOTHESES_MAX);
    count("evidence".into(), doc.evidence.len(), 0, EVIDENCE_MAX);
    for (i, c) in doc.candidates.iter().enumerate() {
        count(
            format!("candidates[{i}].risks"),
            c.risks.len(),
            0,
            RISKS_MAX,
        );
        count(
            format!("candidates[{i}].reevaluate_when"),
            c.reevaluate_when.len(),
            1,
            REEVALUATE_MAX,
        );
    }
    // 参照の下限（1 件以上）は出典の欠損（check_sources）が見る。
    count(
        "research_question.references".into(),
        doc.research_question.references.len(),
        0,
        REFS_PER_ITEM_MAX,
    );
    for (i, e) in doc.evidence.iter().enumerate() {
        count(
            format!("evidence[{i}].references"),
            e.references.len(),
            0,
            REFS_PER_ITEM_MAX,
        );
    }
    count("".into(), input.refs().count(), 0, REFS_TOTAL_MAX);
    for (path, r) in input.refs() {
        if let Reference::Url { url } = r
            && chars(url) > URL_MAX_CHARS
        {
            push(
                out,
                Rule::Limit,
                path,
                format!("url が上限 {URL_MAX_CHARS} 文字を超える"),
            );
        }
    }
}

fn check_content_hash(input: &Input<'_>, out: &mut Vec<Violation>) {
    if sha256_hex(input.text) != bare_sha(&input.ctx.artifact.sha256) {
        push(
            out,
            Rule::ContentHash,
            "",
            "file の sha256 が成果物の記録（ArtifactProduced）と違う（上書きされた）",
        );
    }
}

fn check_secret(input: &Input<'_>, out: &mut Vec<Violation>) {
    if contains_secret(input.text) {
        push(
            out,
            Rule::Secret,
            "",
            "秘密に見える文字列（token・cookie・webhook）を含む",
        );
    }
}

fn check_state(input: &Input<'_>, out: &mut Vec<Violation>) {
    if matches!(
        input.doc.decision_state,
        DecisionState::Approved | DecisionState::Rejected
    ) {
        push(
            out,
            Rule::StateNotWritable,
            "decision_state",
            "approved・rejected は文書に書けない（承認は人の決定だけが決める）",
        );
    }
}

fn check_actor(input: &Input<'_>, out: &mut Vec<Violation>) {
    let (doc, actor) = (input.doc, &input.doc.actor);
    if let Some(source) = &doc.source {
        if !actor_allowed(source.kind, actor.kind) {
            push(
                out,
                Rule::Actor,
                "actor.kind",
                "source の種類と actor の組が違う",
            );
        }
        if source.kind == SourceKind::CelerisTaskRun && actor.id != input.ctx.artifact.run_id {
            push(
                out,
                Rule::Actor,
                "actor.id",
                "actor.id が成果物を記録した run と違う",
            );
        }
    }
    let id_ok = match actor.kind {
        ActorKind::Human => crate::is_human_principal(&actor.id),
        ActorKind::Cos => crate::is_cos_actor(&actor.id),
        ActorKind::Mcp => actor.id.strip_prefix("mcp:").is_some_and(|r| !r.is_empty()),
        ActorKind::Worker => !actor.id.is_empty(),
    };
    if !id_ok {
        push(
            out,
            Rule::Actor,
            "actor.id",
            "actor.id の主体が actor.kind と合わない",
        );
    }
}

fn check_consent(input: &Input<'_>, out: &mut Vec<Violation>) {
    let doc = input.doc;
    let external = doc
        .source
        .iter()
        .chain(doc.evidence.iter().filter_map(|e| e.source.as_ref()))
        .any(|s| s.kind.is_external());
    if !external {
        return;
    }
    match &doc.consent {
        Some(c) if consent_granted(c) && is_rfc3339(&c.granted_at) => {}
        _ => push(
            out,
            Rule::Consent,
            "consent",
            "外部の文に人（human:）が与えた選んだ抜粋か要約の許可が無い",
        ),
    }
}

fn check_claims(input: &Input<'_>, out: &mut Vec<Violation>) {
    let doc = input.doc;
    for (i, h) in doc.hypotheses.iter().enumerate() {
        if h.confidence != Confidence::Confirmed {
            continue;
        }
        let ev: Vec<&Evidence> = doc
            .evidence
            .iter()
            .filter(|e| e.hypothesis == h.id)
            .collect();
        if ev.iter().any(|e| e.stance == Stance::Refute) {
            push(
                out,
                Rule::UnsupportedClaim,
                format!("hypotheses[{i}].confidence"),
                "反証のある仮説を confirmed にできない（contradicted か needs_decision）",
            );
        }
        let grounded = ev.iter().any(|e| {
            e.stance == Stance::Support && e.references.iter().any(Reference::is_internal)
        });
        if !grounded {
            push(
                out,
                Rule::UnsupportedClaim,
                format!("hypotheses[{i}].confidence"),
                "confirmed に commit・artifact・task を参照する支持の証拠が無い",
            );
        }
    }
}

fn check_sources(input: &Input<'_>, out: &mut Vec<Violation>) {
    let (doc, ctx) = (input.doc, input.ctx);
    let mut missing = |path: String, detail: &str| push(out, Rule::SourceMissing, path, detail);
    if doc.source.is_none() {
        missing("source".into(), "文書の source が無い");
    }
    if doc.research_question.references.is_empty() {
        missing(
            "research_question.references".into(),
            "研究質問の参照が無い",
        );
    }
    for (i, c) in doc.candidates.iter().enumerate() {
        if c.hypotheses.is_empty() {
            missing(
                format!("candidates[{i}].hypotheses"),
                "候補が根拠の仮説を指さない",
            );
        }
    }
    for (i, e) in doc.evidence.iter().enumerate() {
        if e.source.is_none() {
            missing(format!("evidence[{i}].source"), "証拠の source が無い");
        }
        if e.references.is_empty() {
            missing(format!("evidence[{i}].references"), "証拠の参照が無い");
        }
    }
    for (path, r) in input.refs() {
        match r {
            Reference::Url { .. } => {}
            Reference::Chat(c) => match input.chat_state(r, c) {
                ChatCitationState::Retracted => missing(path, "引いた chat の発言が撤回された"),
                ChatCitationState::Missing => missing(path, "引いた chat の発言・その版が無い"),
                ChatCitationState::Current | ChatCitationState::Edited => {}
            },
            _ => {
                if !matches!(input.lookup(r), Some(RefStatus::Found { .. })) {
                    missing(path, "参照が引けない");
                }
            }
        }
    }
    for i in 0..input.expected_prev() {
        if ctx.chain.get(i).is_none_or(|link| link.bytes.is_none()) {
            missing(format!("chain[{i}]"), "前の版の成果物が引けない");
        }
    }
}

fn check_stale(input: &Input<'_>, out: &mut Vec<Violation>) {
    let (doc, ctx) = (input.doc, input.ctx);
    if doc.source_revision.request_sha256 != ctx.request_sha256 {
        push(
            out,
            Rule::Stale,
            "source_revision.request_sha256",
            "Project.request が文書の前提から変わった",
        );
    }
    if doc.source_revision.roadmap_version != ctx.roadmap_version {
        push(
            out,
            Rule::Stale,
            "source_revision.roadmap_version",
            "承認済み Roadmap の版が文書の前提と違う",
        );
    }
    if ctx.latest_approved_version.is_some_and(|v| v > doc.version) {
        push(
            out,
            Rule::Stale,
            "version",
            "より新しい版が人に承認されている",
        );
    }
    for (path, r) in input.refs() {
        if let Reference::Chat(c) = r
            && input.chat_state(r, c) == ChatCitationState::Edited
        {
            push(
                out,
                Rule::Stale,
                path,
                "引いた chat の発言が引いた版の後に編集された",
            );
        }
    }
}

fn check_project(input: &Input<'_>, out: &mut Vec<Violation>) {
    let (doc, ctx) = (input.doc, input.ctx);
    let other = |p: &str| p != ctx.project_id;
    if other(&doc.project_id) {
        push(
            out,
            Rule::OtherProject,
            "project_id",
            "文書の案件が作った task の案件と違う",
        );
    }
    for (path, r) in input.refs() {
        if let Some(p) = input.lookup(r).and_then(RefStatus::project_id)
            && other(p)
        {
            push(out, Rule::OtherProject, path, "参照が別の案件の資料を指す");
        }
    }
    for (i, (link, prev)) in ctx.chain.iter().zip(&input.prev).enumerate() {
        let prev_project = prev.as_ref().map(|d| d.project_id.as_str());
        if link.task_project_id.as_deref().is_some_and(other) || prev_project.is_some_and(other) {
            push(
                out,
                Rule::OtherProject,
                format!("chain[{i}]"),
                "前の版が別の案件のもの",
            );
        }
    }
}

fn check_approval(input: &Input<'_>, out: &mut Vec<Violation>) {
    if input.ctx.intent != Intent::Execute {
        return;
    }
    if input.doc.decision_state != DecisionState::Proposed {
        push(
            out,
            Rule::Unapproved,
            "decision_state",
            "実行できるのは proposed の文書だけ",
        );
    }
    if let Err(why) = input.human_approval() {
        push(out, Rule::Unapproved, "", format!("人の承認が無い: {why}"));
    }
}

fn check_outbound(input: &Input<'_>, out: &mut Vec<Violation>) {
    let acl = &input.doc.acl;
    if acl.classification == Classification::Confidential
        && acl.outbound
        && let Err(why) = input.human_approval()
    {
        push(
            out,
            Rule::Outbound,
            "acl.outbound",
            format!("confidential を外へ出す人の承認が無い: {why}"),
        );
    }
}

fn check_supersedes(input: &Input<'_>, out: &mut Vec<Violation>) {
    let (doc, ctx) = (input.doc, input.ctx);
    let mut bad = |path: String, detail: String| push(out, Rule::Supersedes, path, detail);
    let expected = input.expected_prev();
    match (&doc.supersedes, expected) {
        (None, 0) => {}
        (Some(_), 0) => bad(
            "supersedes".into(),
            "version 1 の supersedes は null".into(),
        ),
        (None, _) => bad("supersedes".into(), "version > 1 は前の版を指す".into()),
        (Some(p), _) => {
            if p.name != artifact_name(&doc.strategy_id, doc.version - 1) || !is_hex(&p.sha256, 64)
            {
                bad(
                    "supersedes".into(),
                    "前の版の name・sha256 の形が違う".into(),
                );
            }
            if ctx.chain.first().map(|l| &l.pointer) != Some(p) {
                bad("supersedes".into(), "鎖の先頭が supersedes と違う".into());
            }
        }
    }
    if expected > 0 && doc.change_reason.is_none() {
        bad(
            "change_reason".into(),
            "version > 1 は change_reason が要る".into(),
        );
    }
    if ctx.chain.len() > expected {
        bad(
            "chain".into(),
            format!("前の版が {} 件で {expected} 件より多い", ctx.chain.len()),
        );
    }
    for (i, (link, prev)) in ctx.chain.iter().zip(&input.prev).enumerate().take(expected) {
        let Some(bytes) = &link.bytes else { continue };
        let path = format!("chain[{i}]");
        let hash_ok = std::str::from_utf8(bytes)
            .is_ok_and(|t| sha256_hex(t) == bare_sha(&link.pointer.sha256));
        if !hash_ok {
            bad(path.clone(), "前の版の file の sha256 が参照と違う".into());
        }
        let Some(prev) = prev else {
            bad(path, "前の版が celeris.strategy/1 として読めない".into());
            continue;
        };
        let want_version = doc.version - 1 - i as u32;
        if prev.version != want_version || prev.strategy_id != doc.strategy_id {
            bad(
                path.clone(),
                format!("前の版が {} の v{want_version} でない", doc.strategy_id),
            );
        }
        let next = ctx
            .chain
            .get(i + 1)
            .filter(|_| i + 1 < expected)
            .map(|l| &l.pointer);
        if prev.supersedes.as_ref() != next {
            bad(path, "前の版の supersedes が鎖の次と違う".into());
        }
    }
}

fn check_refute(input: &Input<'_>, out: &mut Vec<Violation>) {
    let docs: Vec<Option<&StrategyDocument>> = std::iter::once(Some(input.doc))
        .chain(input.prev.iter().map(Option::as_ref))
        .collect();
    for (i, pair) in docs.windows(2).enumerate() {
        let [Some(newer), Some(older)] = pair else {
            continue;
        };
        for e in older.evidence.iter().filter(|e| e.stance == Stance::Refute) {
            if !newer.evidence.contains(e) {
                let at = if i == 0 {
                    "文書".to_string()
                } else {
                    format!("chain[{}]", i - 1)
                };
                push(
                    out,
                    Rule::RefuteRemoved,
                    format!("evidence.{}", e.id),
                    format!(
                        "{at} が前の版 v{} の反証の証拠を消したか書き換えた",
                        older.version
                    ),
                );
            }
        }
    }
}

fn check_idempotency(input: &Input<'_>, out: &mut Vec<Violation>) {
    let own = bare_sha(&input.ctx.artifact.sha256);
    if input.ctx.key_sha256s.iter().any(|s| bare_sha(s) != own) {
        push(
            out,
            Rule::Idempotency,
            "idempotency_key",
            "同じ版（同じ idempotency の key）に違う sha256 の記録がある",
        );
    }
}
