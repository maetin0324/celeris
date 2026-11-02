//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2: TS 制御層の領域（`domain`）・mode・compare の記録の型と、
//! 比較入力の機密除去（S2-3）。
//!
//! - mode の表と記録の store は `store::script_domains`（migration 0103）。
//! - 比較入力は許可列挙（allowlist）で作る。記録するのは入力 DTO そのもので、DTO に無い値は記録しない。
//!   `lane_rules` は 9 軸の `Level` だけ、`route_priority` は `harness`・`lane`・`cos`・`execution_class` と、
//!   その harness の候補列の `source` id・`model` id だけ。credential・account・`SourceSpec`・`llm_routes`・
//!   他の harness の行・残量・task の本文は DTO の型に欄が無いので入らない。
//! - 旧 `routing_shadow` の表・event・config 欄は使わない。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `route_priority` の出力の `basis`（S2-2・S2-10）。正本は `llm_route` の enum（snake_case）。
pub use crate::llm_route::RoutePriorityBasis;
use crate::llm_route::{CandidateKey, ExecutionClass, HarnessId, Registry};

mod cos_digest_record;
mod route_priority_fixture;
use crate::model_policy::TaskFeatures;
use crate::store::StoreError;
pub use cos_digest_record::{
    CosDigestApplied, CosDigestCommandsSummary, CosDigestCompareRecord, cos_digest_command_kind,
    cos_digest_text_head,
};
pub use route_priority_fixture::route_priority_fixture_jsonl;

/// TS 制御層の領域（S2-1）。表 `script_domain_modes` の `domain`。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ScriptDomain {
    LaneRules,
    RoutePriority,
    /// 付記 S4: CoS の受信箱 digest pass（migration 0104）。記録は `script_compare_log`。
    CosDigest,
}

impl ScriptDomain {
    pub const ALL: [ScriptDomain; 3] = [
        ScriptDomain::LaneRules,
        ScriptDomain::RoutePriority,
        ScriptDomain::CosDigest,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ScriptDomain::LaneRules => "lane_rules",
            ScriptDomain::RoutePriority => "route_priority",
            ScriptDomain::CosDigest => "cos_digest",
        }
    }

    /// 既知の領域名だけを受ける（未知は `None`）。
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }
}

/// 領域の mode（(d)）。`rust` → `compare` → `canary` → `ts`。S2 で受け付けるのは `rust` と `compare` だけ（S2-5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptMode {
    Rust,
    Compare,
    Canary,
    Ts,
}

impl ScriptMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScriptMode::Rust => "rust",
            ScriptMode::Compare => "compare",
            ScriptMode::Canary => "canary",
            ScriptMode::Ts => "ts",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Rust, Self::Compare, Self::Canary, Self::Ts]
            .into_iter()
            .find(|m| m.as_str() == name)
    }

    /// S2-5: S2 で変更先として受け付ける mode か（`canary`・`ts` は偽）。
    pub fn enabled_in_s2(self) -> bool {
        matches!(self, ScriptMode::Rust | ScriptMode::Compare)
    }
}

/// 表 `script_generations.state`（S2-4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGenerationState {
    Loaded,
    Rejected,
}

impl ScriptGenerationState {
    pub fn as_str(self) -> &'static str {
        match self {
            ScriptGenerationState::Loaded => "loaded",
            ScriptGenerationState::Rejected => "rejected",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "loaded" => Some(Self::Loaded),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// `script_compare_recorded.outcome`（S2-4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompareOutcome {
    Match,
    Mismatch,
    TsError,
    InvalidOutput,
    Unavailable,
}

/// `script_compare_recorded.error`（`ts_error` の `ScriptError` の種類と文、`invalid_output` の理由）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompareError {
    /// snake_case の種類（`exception`・`timeout`・…・`invalid_output`）。
    pub kind: String,
    /// `ERROR_MESSAGE_MAX` byte で切った文（[`truncate_error_message`]）。
    pub message: String,
}

impl CompareError {
    /// 文を `ERROR_MESSAGE_MAX` byte で切って作る。
    pub fn new(kind: impl Into<String>, message: &str) -> Self {
        Self {
            kind: kind.into(),
            message: truncate_error_message(message),
        }
    }
}

/// event `script_compare_recorded` の欄（S2-4 の表）。task の id は `events.task_id` が持つので持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptCompareRecord {
    pub domain: ScriptDomain,
    /// 使った `script_generations.id`（ULID）。
    pub generation: String,
    /// actor が isolate を作り直した回数。
    pub isolate_epoch: u64,
    pub bundle_sha256: String,
    pub host_api: String,
    /// `dispatch_run.decide_lane`・`route_select.route_select_one`。
    pub call_site: String,
    pub run_id: Option<String>,
    /// `sha256:` + 入力 DTO の直列化の sha256。
    pub input_digest: String,
    /// 機密を除いた入力 DTO（64 KiB 超えは `None`）。
    pub input: Option<serde_json::Value>,
    pub input_truncated: bool,
    /// Rust の出力 DTO。
    pub rust: serde_json::Value,
    /// TS の出力 DTO（`ts_error`・`unavailable` は `None`）。
    pub ts: Option<serde_json::Value>,
    pub outcome: CompareOutcome,
    pub error: Option<CompareError>,
    /// 当てた正規化の規則名（S2 では常に空）。
    pub normalized: Vec<String>,
    /// S2-8 の code。
    pub safety_violations: Vec<String>,
    pub elapsed_us: u64,
}

/// 記録する入力の直列化の上限（S2-3）。超えたら `input` を捨て `input_digest` だけを残す。
pub const RECORDED_INPUT_MAX: usize = 64 * 1024;
/// TS の例外の `message` の上限（S2-3）。
pub const ERROR_MESSAGE_MAX: usize = 512;

/// `lane_rules` の入力 DTO（S2-2）。`TaskFeatures::infer_with_hints` の後の 9 軸だけ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneRulesInput {
    pub features: TaskFeatures,
}

impl LaneRulesInput {
    /// 比較入力を作る（S2-3）。9 軸の `Level` のほかに持つ欄は無い。
    pub fn redacted(features: &TaskFeatures) -> Self {
        Self {
            features: *features,
        }
    }
}

/// `route_priority` の候補 1 件（S2-2 の `CandidateKey`）。`source` id と `model` id だけ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompareCandidate {
    pub source: String,
    pub model: String,
}

impl From<&CandidateKey> for CompareCandidate {
    fn from(key: &CandidateKey) -> Self {
        Self {
            source: key.source.0.clone(),
            model: key.model.0.clone(),
        }
    }
}

/// `RouteTable.lane_route_defaults[]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneCandidates {
    pub lane: String,
    pub candidates: Vec<CompareCandidate>,
}

/// `RouteTable.class_defaults[]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClassCandidates {
    pub execution_class: String,
    pub candidates: Vec<CompareCandidate>,
}

/// `Registry` のうち 1 つの harness の候補列だけの写し（S2-2）。配列は `BTreeMap` の key 順。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteTable {
    pub route_defaults: Vec<CompareCandidate>,
    pub lane_route_defaults: Vec<LaneCandidates>,
    pub class_defaults: Vec<ClassCandidates>,
    pub cos_route_candidates: Vec<CompareCandidate>,
}

/// `route_priority` の入力 DTO（S2-2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutePriorityInput {
    pub harness: String,
    pub lane: Option<String>,
    pub cos: bool,
    pub execution_class: Option<String>,
    pub table: RouteTable,
}

impl RoutePriorityInput {
    /// 比較入力を作る（S2-3）。`registry` から `harness` の候補列の `source`・`model` の id だけを写す。
    /// `sources`（`SourceSpec`）・`harnesses`・`llm_routes`・`route_selection`・`harness_selection`・
    /// 他の harness の行は読まない。
    pub fn redacted(
        registry: &Registry,
        harness: &HarnessId,
        lane: Option<&str>,
        cos: bool,
        execution_class: Option<&ExecutionClass>,
    ) -> Self {
        let list = |keys: &[CandidateKey]| keys.iter().map(CompareCandidate::from).collect();
        let route_defaults = registry
            .route_defaults
            .get(harness)
            .map(|keys| list(keys))
            .unwrap_or_default();
        let lane_route_defaults = registry
            .lane_route_defaults
            .iter()
            .filter(|((h, _), _)| h == harness)
            .map(|((_, lane), keys)| LaneCandidates {
                lane: lane.clone(),
                candidates: list(keys),
            })
            .collect();
        let class_defaults = registry
            .route_class_defaults
            .get(harness)
            .map(|classes| {
                classes
                    .iter()
                    .map(|(class, keys)| ClassCandidates {
                        execution_class: class.0.clone(),
                        candidates: list(keys),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            harness: harness.0.clone(),
            lane: lane.map(str::to_owned),
            cos,
            execution_class: execution_class.map(|c| c.0.clone()),
            table: RouteTable {
                route_defaults,
                lane_route_defaults,
                class_defaults,
                cos_route_candidates: list(&registry.cos_route_candidates),
            },
        }
    }
}

/// `route_priority` の出力 DTO（S2-2）。`llm_route::route_priority_decide` の結果をそのまま写す。
/// 欄の順は宣言順（`basis`・`candidates`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutePriorityOutput {
    pub basis: RoutePriorityBasis,
    pub candidates: Vec<CompareCandidate>,
}

impl RoutePriorityOutput {
    /// `route_priority_decide` の `(basis, 列)` から作る（列の順・重複はそのまま）。
    pub fn from_decision((basis, candidates): (RoutePriorityBasis, &[CandidateKey])) -> Self {
        Self {
            basis,
            candidates: candidates.iter().map(CompareCandidate::from).collect(),
        }
    }
}

/// 記録する入力（S2-3・S2-4 の `input_digest`・`input`・`input_truncated`）。
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedInput {
    pub input_digest: String,
    pub input: Option<serde_json::Value>,
    pub input_truncated: bool,
}

/// 入力 DTO を serde_json で直列化し（空白なし、struct は宣言順）、digest と記録する値を作る。
/// 直列化が `RECORDED_INPUT_MAX` byte を超えたら値を捨てて digest だけを残す。
pub fn recorded_input<T: Serialize>(input: &T) -> Result<RecordedInput, serde_json::Error> {
    let bytes = serde_json::to_vec(input)?;
    let input_digest = sha256_digest(&bytes);
    if bytes.len() > RECORDED_INPUT_MAX {
        return Ok(RecordedInput {
            input_digest,
            input: None,
            input_truncated: true,
        });
    }
    Ok(RecordedInput {
        input_digest,
        input: Some(serde_json::from_slice(&bytes)?),
        input_truncated: false,
    })
}

/// `sha256:` + 64 桁の小文字 hex。
pub fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 文を `ERROR_MESSAGE_MAX` byte 以下に切る（UTF-8 の文字の途中では切らない）。
pub fn truncate_error_message(message: &str) -> String {
    if message.len() <= ERROR_MESSAGE_MAX {
        return message.to_owned();
    }
    let mut end = ERROR_MESSAGE_MAX;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

/// mode の変更の要求（S2-4 の CAS 更新）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptModeChange {
    /// 領域名（未知の名前は `ScriptDomainError::UnknownDomain`）。
    pub domain: String,
    pub to_mode: ScriptMode,
    /// `compare` では必須（`script_generations.id`）。`rust` では `None`。
    pub to_generation: Option<String>,
    /// 呼び出し側が読んだ `version`。表の値と違えば `VersionConflict`。
    pub expected_version: u64,
    pub actor: String,
    pub reason: String,
}

/// mode の変更の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptModeChangeOutcome {
    /// 表を更新し、`script_domain_events` に 1 行を追記した（同じ transaction）。
    Changed {
        row: ScriptDomainModeRow,
        event_id: u64,
    },
    /// 同じ mode・同じ generation への変更。表も記録も変えない（S2-5）。
    Unchanged { row: ScriptDomainModeRow },
}

/// 表 `script_domain_modes` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptDomainModeRow {
    pub domain: ScriptDomain,
    pub mode: ScriptMode,
    pub generation_id: Option<String>,
    pub version: u64,
    pub updated_at: String,
    pub updated_by: String,
}

/// 表 `script_generations` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptGeneration {
    pub id: String,
    pub bundle_sha256: String,
    pub host_api: String,
    pub schema_sha256: String,
    /// manifest の全文。
    pub manifest_json: String,
    pub state: ScriptGenerationState,
    pub created_at: String,
    pub updated_at: String,
}

impl ScriptGeneration {
    /// manifest の `domains` にこの領域があるか（manifest が読めなければ偽）。
    pub fn declares_domain(&self, domain: ScriptDomain) -> bool {
        serde_json::from_str::<serde_json::Value>(&self.manifest_json)
            .ok()
            .and_then(|v| v.get("domains").cloned())
            .and_then(|d| d.as_array().cloned())
            .is_some_and(|ds| ds.iter().any(|d| d.as_str() == Some(domain.as_str())))
    }
}

/// 新しい generation の行（daemon が bundle を load して検証した結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewScriptGeneration {
    pub bundle_sha256: String,
    pub host_api: String,
    pub schema_sha256: String,
    pub manifest_json: String,
    pub state: ScriptGenerationState,
}

/// mode の変更・読み出しの失敗。API は `UnknownDomain` を 404、`ModeNotEnabled` を 422
/// （`script_mode_not_enabled`）、`VersionConflict` を 409、generation の不備を 422 に写す。
#[derive(Debug, thiserror::Error)]
pub enum ScriptDomainError {
    #[error("unknown script domain `{0}`")]
    UnknownDomain(String),
    #[error("script mode `{}` is not enabled in S2 (only rust and compare)", .0.as_str())]
    ModeNotEnabled(ScriptMode),
    #[error("script domain `{domain}` version conflict: expected {expected}, found {found}")]
    VersionConflict {
        domain: String,
        expected: u64,
        found: u64,
    },
    #[error("mode `compare` needs a generation")]
    GenerationRequired,
    #[error("mode `rust` takes no generation")]
    GenerationNotAllowed,
    #[error("script generation `{0}` not found")]
    GenerationNotFound(String),
    #[error("script generation `{0}` is not loaded")]
    GenerationNotLoaded(String),
    #[error("script generation `{generation}` does not declare domain `{domain}`")]
    DomainNotDeclared { domain: String, generation: String },
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ScriptDomainError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(StoreError::from(e))
    }
}

impl From<serde_json::Error> for ScriptDomainError {
    fn from(e: serde_json::Error) -> Self {
        Self::Store(StoreError::from(e))
    }
}

#[cfg(test)]
#[path = "script_compare/tests.rs"]
mod tests;
