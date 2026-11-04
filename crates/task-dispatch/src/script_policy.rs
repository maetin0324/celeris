//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2: dispatcher が TS 制御層の判定を呼ぶための境界。
//!
//! - [`PolicyScript`] は DTO（`serde_json::Value` と task-core の入力 DTO）だけを使い、deno_core に依存しない。
//!   本物の実装は daemon（`crates/celeris`）が `ScriptRuntimeHandle` で作り、dispatcher に注入する。
//! - dispatcher が使う値は常に Rust の参照実装の出力。TS の出力は `script_compare_recorded` に書くだけ（S2-6）。
//! - [`classify_lane_rules`]・[`classify_route_priority`] は TS の結果を Rust の出力と比べ、outcome・error・
//!   安全違反（S2-8）を決める純粋関数。
//! - [`CompareDraft`] は領域（[`DraftDomain`]）ごとの入力と Rust の出力を持ち、記録の共通部分を作る。
//! - [`ScriptDomainStore`] は mode の表と記録の store の口。`TaskStore` に欄を足さずに `SqliteStore` を渡せるよう、
//!   この crate に置く。

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, CosDigestPassOutput, CosDigestPassRead,
    CosDigestPassViolation, validate_cos_digest_commands, validate_cos_digest_pass,
};
use task_core::execution_gate::{EXECUTION_GATE_TS_RULES, ExecutionGateInput, ExecutionGateOutput};
use task_core::llm_route::{ExecutionClass, HarnessId, Registry, route_priority_decide};
use task_core::model_policy::{LaneCeiling, RuleMatch, TaskFeatures};
use task_core::script_behavior_change::{ApprovedBehaviorChange, ExplainedBy};
use task_core::script_compare::{
    CompareCandidate, CompareError, CompareOutcome, CosDigestApplied, CosDigestAppliedBy,
    CosDigestCommandsSummary, CosDigestCompareRecord, CosDigestFallback, CosDigestOpLog,
    LaneRulesInput, RoutePriorityInput, RoutePriorityOutput, ScriptCompareRecord, ScriptDomain,
    ScriptDomainError, ScriptDomainModeRow, ScriptGeneration, ScriptMode, ScriptModeChange,
    recorded_input,
};
use task_core::script_guard::{SCRIPT_GUARD_ACTOR, ScriptGuardOutcome};
use task_core::{SqliteStore, StoreError, TaskId, Tier};

/// 1 呼び出しの deadline（境界 ADR (f)・S2-6）。超えたら待つのをやめ、`ts_error`（`timeout`）を記録する。
pub const SCRIPT_CALL_DEADLINE: Duration = Duration::from_millis(50);

/// `lane_rules` の比較を付けた呼び出し箇所（S2-4 の `call_site`）。
pub const LANE_RULES_CALL_SITE: &str = "dispatch_run.decide_lane";

/// `route_priority` の比較を付けた呼び出し箇所（S2-4 の `call_site`・付記 S2-11）。
pub const ROUTE_PRIORITY_CALL_SITE: &str = "route_select.route_select_one";

/// `execution_gate` の比較を付けた呼び出し箇所（ADR 2026-11-04-ts-execution-gate-domain D4）。
pub const EXECUTION_GATE_CALL_SITE: &str = "work_units.execution_gate_if_needed";

/// `cos_digest` の pass の記録の呼び出し箇所（付記 S4-15 の `call_site`）。
pub const COS_DIGEST_CALL_SITE: &str = "cos_digest.pass";

/// `route_priority` の候補の上限（S2-4 の `invalid_output`）。
pub const ROUTE_PRIORITY_CANDIDATES_MAX: usize = 256;

/// 同時に走らせる compare の上限。超えた判定は比べずに捨てる（記録もしない）。
pub const MAX_IN_FLIGHT_COMPARES: usize = 32;

/// `lane_rules` の出力 DTO（S2-2）。3 欄すべてを byte 一致で比べる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneRulesOutput {
    pub rule_id: String,
    pub proposed: Tier,
    pub condition: String,
}

impl From<RuleMatch> for LaneRulesOutput {
    fn from(m: RuleMatch) -> Self {
        Self {
            rule_id: m.rule_id.to_owned(),
            proposed: m.lane,
            condition: m.condition.to_owned(),
        }
    }
}

/// S2-4 の `invalid_output` の領域の検証の上限。
const RULE_ID_MAX: usize = 128;
const CONDITION_MAX: usize = 512;

/// TS の 1 回の呼び出しの成功結果。
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyReply {
    /// TS の戻り値（生の JSON）。dispatcher が出力 DTO に deserialize して検証する。
    pub output: serde_json::Value,
    /// 応答した isolate の作り直し回数（S2-4 の `isolate_epoch`）。
    pub isolate_epoch: u64,
}

/// TS の呼び出しの失敗（`ScriptError` を DTO にしたもの）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyScriptError {
    /// snake_case の種類（`exception`・`timeout`・`busy`・`terminated`・`generation_gone`・`heap_limit`・
    /// `input_too_large`・`denied`・`invalid_output`）。`invalid_output` は outcome `invalid_output` にする。
    pub kind: String,
    pub message: String,
    /// `ScriptError::Denied` のときの S2-8 の code（`denied_capability`・`denied_scope`）。
    pub safety_violation: Option<String>,
    /// 失敗した isolate の作り直し回数（分からなければ `None`）。
    pub isolate_epoch: Option<u64>,
}

impl PolicyScriptError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            safety_violation: None,
            isolate_epoch: None,
        }
    }
}

/// どの bundle を呼ぶか（S2-4 の `bundle_sha256`・`host_api`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyScriptBundle {
    /// 64 桁の小文字 hex。
    pub bundle_sha256: String,
    pub host_api: String,
}

/// TS の 1 回の呼び出しの future。
pub type PolicyFuture<'a> =
    Pin<Box<dyn Future<Output = Result<PolicyReply, PolicyScriptError>> + Send + 'a>>;

/// TS 制御層の判定を呼ぶ口（S2）。DTO だけを受け渡し、deno_core に依存しない。
///
/// 実装は `deadline` を過ぎたら待つのをやめて `timeout` を返すべきだが、dispatcher も同じ deadline で
/// 打ち切る（実装が守らなくても dispatcher は `deadline` より長く待たない）。
pub trait PolicyScript: Send + Sync {
    /// 読み込んである bundle。表の generation と `bundle_sha256` が違えば dispatcher は呼ばない（`unavailable`）。
    fn bundle(&self) -> PolicyScriptBundle;

    /// `lane_rules` を評価する。入力は機密を除いた DTO（9 軸の `Level` だけ）。
    fn lane_rules(&self, input: LaneRulesInput, deadline: Duration) -> PolicyFuture<'_>;

    /// `route_priority` を評価する。入力は 1 つの harness の候補列の `source`・`model` の id だけ（S2-3）。
    fn route_priority(&self, input: RoutePriorityInput, deadline: Duration) -> PolicyFuture<'_>;

    /// `execution_gate` を評価する。入力は task の本文を含む（ADR 2026-11-04-ts-execution-gate-domain D5）。
    /// 既定は失敗（領域を持たない実装は呼ばれても Rust の判定に戻る）。
    fn execution_gate(&self, _input: ExecutionGateInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async {
            Err(PolicyScriptError::new(
                "unavailable",
                "this script does not implement execution_gate",
            ))
        })
    }

    /// `cos_digest` の digest pass を 1 回走らせる（付記 S4-12）。TS は `call.sink` へ op を送って読み、
    /// 出力 `CosDigestPassOutput`（`{ "runs": [{ run_id, commands }] }`）を返す。
    fn cos_digest_pass(&self, call: CosDigestPassCall, deadline: Duration) -> PolicyFuture<'_>;
}

/// generation ごとの判定の口を返す（付記 S5-2）。dispatcher は mode の行が指す generation を 1 回読み、
/// その generation に固定した口で TS を呼ぶ。`Err` は呼べない理由（isolate が無い・drain 中など）で、
/// dispatcher は TS を呼ばず `unavailable` を記録し、判定は Rust の参照実装のまま。
pub trait PolicyScriptSource: Send + Sync {
    fn pinned(&self, generation: &ScriptGeneration) -> Result<Arc<dyn PolicyScript>, String>;
}

/// bundle 1 つだけを持つ口（試験・単体の runtime）。generation の `bundle_sha256` が同じときだけ返す。
pub struct SingleBundleSource(pub Arc<dyn PolicyScript>);

impl PolicyScriptSource for SingleBundleSource {
    fn pinned(&self, generation: &ScriptGeneration) -> Result<Arc<dyn PolicyScript>, String> {
        let loaded = self.0.bundle().bundle_sha256;
        if loaded == generation.bundle_sha256 {
            Ok(Arc::clone(&self.0))
        } else {
            Err(format!(
                "loaded bundle {loaded} differs from generation bundle {}",
                generation.bundle_sha256
            ))
        }
    }
}

/// mode の表と compare の記録の store（`SqliteStore` が実装する）。
pub trait ScriptDomainStore: Send + Sync {
    fn script_domain_modes(&self) -> Result<Vec<ScriptDomainModeRow>, StoreError>;
    fn script_generation_get(&self, id: &str) -> Result<Option<ScriptGeneration>, StoreError>;
    /// 重複の抑え（S2-4）の後に追記し、同じ transaction で script-guard の計数を更新する（付記 S5-7）。
    /// `ts_side` は canary・ts で TS の側に選ばれた単位か。戻り値は追記したかと計数の結果。
    fn script_compare_append_observed(
        &self,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        ts_side: bool,
        now: OffsetDateTime,
    ) -> Result<(bool, Option<ScriptGuardOutcome>), ScriptDomainError>;
    /// 付記 S5-6: 領域を actor `system:script-guard` で `rust` に戻す（`reason` の先頭は code）。
    /// 版違い（他の更新が先に入った）は `VersionConflict` のまま返す。
    fn script_domain_rollback(
        &self,
        domain: ScriptDomain,
        expected_version: u64,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<(), ScriptDomainError>;
    /// 付記 S5-5: 領域に保存済みの checkpoint の版の集合。
    fn script_checkpoint_versions_stored(&self, domain: ScriptDomain) -> Vec<u32>;
    /// 領域の承認済みの改善仕様（ADR 2026-11-03-ts-intended-behavior-change D4）。既定は無し（何も説明しない）。
    fn script_behavior_changes_approved(
        &self,
        _domain: ScriptDomain,
    ) -> Result<Vec<ApprovedBehaviorChange>, StoreError> {
        Ok(Vec::new())
    }
}

impl ScriptDomainStore for SqliteStore {
    fn script_domain_modes(&self) -> Result<Vec<ScriptDomainModeRow>, StoreError> {
        SqliteStore::script_domain_modes(self)
    }

    fn script_generation_get(&self, id: &str) -> Result<Option<ScriptGeneration>, StoreError> {
        SqliteStore::script_generation_get(self, id)
    }

    fn script_compare_append_observed(
        &self,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        ts_side: bool,
        now: OffsetDateTime,
    ) -> Result<(bool, Option<ScriptGuardOutcome>), ScriptDomainError> {
        SqliteStore::script_compare_append_observed(self, task_id, record, ts_side, now)
    }

    fn script_domain_rollback(
        &self,
        domain: ScriptDomain,
        expected_version: u64,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<(), ScriptDomainError> {
        let change = ScriptModeChange {
            domain: domain.as_str().to_owned(),
            to_mode: ScriptMode::Rust,
            to_generation: None,
            expected_version,
            actor: SCRIPT_GUARD_ACTOR.to_owned(),
            reason: reason.to_owned(),
        };
        SqliteStore::script_domain_set_mode(self, &change, now).map(|_| ())
    }

    fn script_checkpoint_versions_stored(&self, domain: ScriptDomain) -> Vec<u32> {
        SqliteStore::script_checkpoint_versions_stored(self, domain)
    }

    fn script_behavior_changes_approved(
        &self,
        domain: ScriptDomain,
    ) -> Result<Vec<ApprovedBehaviorChange>, StoreError> {
        SqliteStore::script_behavior_changes_approved(self, domain)
    }
}

/// TS の呼び出しの結果（deadline 超えを含む）。
#[derive(Debug, Clone, PartialEq)]
pub enum TsResult {
    Reply(PolicyReply),
    Error(PolicyScriptError),
    /// dispatcher の deadline を過ぎた。
    DeadlineExceeded(Duration),
}

/// [`classify_lane_rules`] の結果（記録の `ts`・`outcome`・`error`・`safety_violations`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    pub ts: Option<serde_json::Value>,
    pub outcome: CompareOutcome,
    pub error: Option<CompareError>,
    pub safety_violations: Vec<String>,
}

/// TS の結果を Rust の出力と比べる（S2-2・S2-4・S2-8）。`rust_lane` は Rust が天井で丸めた後の lane。
/// 正規化はしない（S2-2）。TS の戻り値を出力 DTO に deserialize し直し、Rust と同じ直列化の byte 列で比べる。
pub fn classify_lane_rules(
    rust: &LaneRulesOutput,
    rust_lane: Tier,
    ceiling: &LaneCeiling,
    result: &TsResult,
) -> Classified {
    match result {
        TsResult::Reply(reply) => classify_output(rust, rust_lane, ceiling, &reply.output),
        TsResult::DeadlineExceeded(d) => deadline_exceeded(*d),
        TsResult::Error(e) => script_error(e),
    }
}

/// TS の結果を Rust の `route_priority` の出力と比べる（S2-2・S2-4・S2-8）。`input` は TS に渡した入力で、
/// TS の候補が `input.table` のどの列にも無ければ `undeclared_candidate` を数える。
/// 正規化はしない（S2-2）。`basis` と `candidates`（順序・重複を含む）を同じ直列化の byte 列で比べる。
pub fn classify_route_priority(
    rust: &RoutePriorityOutput,
    input: &RoutePriorityInput,
    result: &TsResult,
) -> Classified {
    let raw = match result {
        TsResult::Reply(reply) => &reply.output,
        TsResult::DeadlineExceeded(d) => return deadline_exceeded(*d),
        TsResult::Error(e) => return script_error(e),
    };
    let invalid = |reason: String| Classified {
        ts: Some(bounded_raw(raw)),
        outcome: CompareOutcome::InvalidOutput,
        error: Some(CompareError::new("invalid_output", &reason)),
        safety_violations: Vec::new(),
    };
    let ts: RoutePriorityOutput = match serde_json::from_value(raw.clone()) {
        Ok(ts) => ts,
        Err(e) => return invalid(format!("route_priority output: {e}")),
    };
    if ts.candidates.len() > ROUTE_PRIORITY_CANDIDATES_MAX {
        return invalid(format!(
            "route_priority output: at most {ROUTE_PRIORITY_CANDIDATES_MAX} candidates (got {})",
            ts.candidates.len()
        ));
    }
    let declared = |c: &CompareCandidate| {
        let table = &input.table;
        table.route_defaults.contains(c)
            || table.cos_route_candidates.contains(c)
            || table
                .lane_route_defaults
                .iter()
                .any(|l| l.candidates.contains(c))
            || table
                .class_defaults
                .iter()
                .any(|l| l.candidates.contains(c))
    };
    let mut safety_violations = Vec::new();
    if !ts.candidates.iter().all(declared) {
        safety_violations.push("undeclared_candidate".to_owned());
    }
    Classified {
        ts: serde_json::to_value(&ts).ok(),
        outcome: if same_bytes(&ts, rust) {
            CompareOutcome::Match
        } else {
            CompareOutcome::Mismatch
        },
        error: None,
        safety_violations,
    }
}

/// `execution_gate` の入力の本文（`title`・`objective`）の上限。超えたら TS を呼ばない（ADR D9）。
pub const EXECUTION_GATE_TEXT_MAX: usize = 256 * 1024;
const EXECUTION_GATE_SIGNALS_MAX: usize = 16;
const EXECUTION_GATE_SIGNAL_NAME_MAX: usize = 8;
const EXECUTION_GATE_SIGNAL_DETAIL_MAX: usize = 128;
/// 例外文の代わりに残す文（ADR D6。TS の例外文は本文を写しうる）。
const EXECUTION_GATE_MESSAGE_WITHHELD: &str = "message withheld: the input carries task text";

/// TS の結果を Rust の `execution_gate` の出力と比べ、Rust の検証を当てる（ADR D6・D8）。例外文と不正な出力は
/// 本文を写しうるので記録に残さない。正規化はしない。`mode`・`score`・`rule_id`・`signals` を同じ直列化の byte 列で比べる。
pub fn classify_execution_gate(
    rust: &ExecutionGateOutput,
    input: &ExecutionGateInput,
    result: &TsResult,
) -> Classified {
    let raw = match result {
        TsResult::Reply(reply) => &reply.output,
        TsResult::DeadlineExceeded(d) => return deadline_exceeded(*d),
        TsResult::Error(e) => {
            let bytes = e.message.len();
            let mut classified = script_error(e);
            if let Some(error) = &mut classified.error {
                error.message = format!("{EXECUTION_GATE_MESSAGE_WITHHELD} ({bytes} bytes)");
            }
            return classified;
        }
    };
    let invalid = |reason: &str, violation: Option<&str>| Classified {
        ts: None,
        outcome: CompareOutcome::InvalidOutput,
        error: Some(CompareError::new(
            "invalid_output",
            &format!("execution_gate output: {reason}"),
        )),
        safety_violations: violation.map(str::to_owned).into_iter().collect(),
    };
    let Ok(ts) = serde_json::from_value::<ExecutionGateOutput>(raw.clone()) else {
        return invalid("does not match the output DTO", None);
    };
    if ts.signals.len() > EXECUTION_GATE_SIGNALS_MAX {
        return invalid("too many signals", None);
    }
    let mut names = BTreeSet::new();
    let bad_signal = ts.signals.iter().any(|s| {
        s.name.is_empty()
            || s.name.len() > EXECUTION_GATE_SIGNAL_NAME_MAX
            || s.detail.len() > EXECUTION_GATE_SIGNAL_DETAIL_MAX
            || !names.insert(s.name.as_str())
    });
    if bad_signal {
        return invalid("a signal is too long or duplicated", None);
    }
    if !EXECUTION_GATE_TS_RULES.contains(&ts.rule_id.as_str()) {
        return invalid("rule_id is not allowed", Some("gate_rule_not_allowed"));
    }
    let sum = ts
        .signals
        .iter()
        .fold(0i64, |acc, s| acc.saturating_add(i64::from(s.weight)));
    let mode_ok = ts.rule_id.starts_with(ts.mode.as_str());
    let threshold_ok = match ts.rule_id.as_str() {
        "compound/score" => ts.score >= input.threshold,
        "atomic/score" => ts.score < input.threshold,
        _ => true,
    };
    if !mode_ok || !threshold_ok || i64::from(ts.score) != sum {
        return invalid(
            "mode, score and threshold contradict",
            Some("gate_threshold_violated"),
        );
    }
    Classified {
        ts: serde_json::to_value(&ts).ok(),
        outcome: if same_bytes(&ts, rust) {
            CompareOutcome::Match
        } else {
            CompareOutcome::Mismatch
        },
        error: None,
        safety_violations: Vec::new(),
    }
}

/// 出力 DTO どうしを同じ serde_json の直列化の byte 列で比べる（S2-2）。
fn same_bytes<T: Serialize>(a: &T, b: &T) -> bool {
    match (serde_json::to_vec(a), serde_json::to_vec(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// dispatcher の deadline を過ぎた結果の分類（両領域で同じ）。
fn deadline_exceeded(d: Duration) -> Classified {
    Classified {
        ts: None,
        outcome: CompareOutcome::TsError,
        error: Some(CompareError::new(
            "timeout",
            &format!("deadline {} ms exceeded", d.as_millis()),
        )),
        safety_violations: Vec::new(),
    }
}

/// `PolicyScriptError` の分類（両領域で同じ）。`invalid_output` は outcome `invalid_output` にする。
fn script_error(e: &PolicyScriptError) -> Classified {
    if e.kind == "invalid_output" {
        return Classified {
            ts: None,
            outcome: CompareOutcome::InvalidOutput,
            error: Some(CompareError::new("invalid_output", &e.message)),
            safety_violations: Vec::new(),
        };
    }
    Classified {
        ts: None,
        outcome: CompareOutcome::TsError,
        error: Some(CompareError::new(e.kind.clone(), &e.message)),
        safety_violations: e.safety_violation.iter().cloned().collect(),
    }
}

fn classify_output(
    rust: &LaneRulesOutput,
    rust_lane: Tier,
    ceiling: &LaneCeiling,
    raw: &serde_json::Value,
) -> Classified {
    let invalid = |reason: String| Classified {
        ts: Some(bounded_raw(raw)),
        outcome: CompareOutcome::InvalidOutput,
        error: Some(CompareError::new("invalid_output", &reason)),
        safety_violations: Vec::new(),
    };
    let ts: LaneRulesOutput = match serde_json::from_value(raw.clone()) {
        Ok(ts) => ts,
        Err(e) => return invalid(format!("lane_rules output: {e}")),
    };
    if ts.rule_id.is_empty() || ts.rule_id.len() > RULE_ID_MAX {
        return invalid(format!(
            "lane_rules output: rule_id must be 1..={RULE_ID_MAX} bytes (got {})",
            ts.rule_id.len()
        ));
    }
    if ts.condition.len() > CONDITION_MAX {
        return invalid(format!(
            "lane_rules output: condition must be at most {CONDITION_MAX} bytes (got {})",
            ts.condition.len()
        ));
    }
    let same = same_bytes(&ts, rust);
    let mut safety_violations = Vec::new();
    let (ts_lane, _) = ceiling.clamp(ts.proposed);
    if !ceiling.permits(ts_lane) && ceiling.permits(rust_lane) {
        safety_violations.push("ceiling_exceeded".to_owned());
    }
    Classified {
        ts: serde_json::to_value(&ts).ok(),
        outcome: if same {
            CompareOutcome::Match
        } else {
            CompareOutcome::Mismatch
        },
        error: None,
        safety_violations,
    }
}

/// `invalid_output` の生の値は 64 KiB まで（S2-4）。超えたら値の代わりに大きさだけを残す。
fn bounded_raw(raw: &serde_json::Value) -> serde_json::Value {
    let len = serde_json::to_vec(raw)
        .map(|b| b.len())
        .unwrap_or(usize::MAX);
    if len <= task_core::script_compare::RECORDED_INPUT_MAX {
        raw.clone()
    } else {
        serde_json::json!({ "truncated_bytes": len })
    }
}

/// 領域ごとの入力と Rust の出力（記録の `input`・`rust` と分類に使う）。
#[derive(Debug, Clone)]
pub enum DraftDomain {
    LaneRules {
        input: LaneRulesInput,
        rust: LaneRulesOutput,
        /// Rust が天井で丸めた後の lane（`ceiling_exceeded` の判定に使う）。
        rust_lane: Tier,
        ceiling: LaneCeiling,
    },
    RoutePriority {
        input: RoutePriorityInput,
        rust: RoutePriorityOutput,
    },
    ExecutionGate {
        input: Box<ExecutionGateInput>,
        rust: ExecutionGateOutput,
    },
}

/// 記録の共通部分（generation・bundle・run・領域の入力と Rust の出力）。TS の結果を受けて
/// [`CompareDraft::finish`] で記録にする。
#[derive(Debug, Clone)]
pub struct CompareDraft {
    pub generation: String,
    pub bundle_sha256: String,
    pub host_api: String,
    /// 判定した run（作る前の判定は `None`）。
    pub run_id: Option<String>,
    pub domain: DraftDomain,
    /// 付記 S5-3・S5-6: canary・ts の記録の欄（compare では `None`・`false`）。
    pub applied_by: Option<CosDigestAppliedBy>,
    pub fallback: Option<CosDigestFallback>,
    pub canary_selected: bool,
    pub canary_bps: Option<u32>,
}

impl CompareDraft {
    /// `lane_rules` の下書きを作る。Rust の出力は `match_rule` を同じ features に当て直したもの（純粋関数）。
    pub fn lane_rules(
        generation: String,
        bundle_sha256: String,
        host_api: String,
        features: &TaskFeatures,
        rust_lane: Tier,
        ceiling: &LaneCeiling,
    ) -> Self {
        Self {
            generation,
            bundle_sha256,
            host_api,
            run_id: None,
            applied_by: None,
            fallback: None,
            canary_selected: false,
            canary_bps: None,
            domain: DraftDomain::LaneRules {
                input: LaneRulesInput::redacted(features),
                rust: task_core::model_policy::match_rule(features).into(),
                rust_lane,
                ceiling: ceiling.clone(),
            },
        }
    }

    /// `route_priority` の下書きを作る。Rust の出力は `route_priority_decide` を同じ引数に当て直したもの
    /// （純粋関数。`route_select_one` の最初の `route_priority` と同じ引数）。
    #[allow(clippy::too_many_arguments)]
    pub fn route_priority(
        generation: String,
        bundle_sha256: String,
        host_api: String,
        run_id: Option<String>,
        registry: &Registry,
        harness: &HarnessId,
        lane: Option<&str>,
        cos: bool,
        execution_class: Option<&ExecutionClass>,
    ) -> Self {
        Self {
            generation,
            bundle_sha256,
            host_api,
            run_id,
            applied_by: None,
            fallback: None,
            canary_selected: false,
            canary_bps: None,
            domain: DraftDomain::RoutePriority {
                input: RoutePriorityInput::redacted(registry, harness, lane, cos, execution_class),
                rust: RoutePriorityOutput::from_decision(route_priority_decide(
                    registry,
                    harness,
                    lane,
                    cos,
                    execution_class,
                )),
            },
        }
    }

    /// `execution_gate` の下書きを作る。Rust の出力は `execution_gate::score` を同じ入力に当てたもの（純粋関数）。
    pub fn execution_gate(
        generation: String,
        bundle_sha256: String,
        host_api: String,
        input: ExecutionGateInput,
    ) -> Self {
        let rust = task_core::execution_gate::score(&input);
        Self {
            generation,
            bundle_sha256,
            host_api,
            run_id: None,
            applied_by: None,
            fallback: None,
            canary_selected: false,
            canary_bps: None,
            domain: DraftDomain::ExecutionGate {
                input: Box::new(input),
                rust,
            },
        }
    }

    pub fn script_domain(&self) -> ScriptDomain {
        match self.domain {
            DraftDomain::LaneRules { .. } => ScriptDomain::LaneRules,
            DraftDomain::RoutePriority { .. } => ScriptDomain::RoutePriority,
            DraftDomain::ExecutionGate { .. } => ScriptDomain::ExecutionGate,
        }
    }

    fn call_site(&self) -> &'static str {
        match self.domain {
            DraftDomain::LaneRules { .. } => LANE_RULES_CALL_SITE,
            DraftDomain::RoutePriority { .. } => ROUTE_PRIORITY_CALL_SITE,
            DraftDomain::ExecutionGate { .. } => EXECUTION_GATE_CALL_SITE,
        }
    }

    fn record(
        &self,
        isolate_epoch: u64,
        classified: Classified,
        elapsed: Duration,
    ) -> Result<ScriptCompareRecord, serde_json::Error> {
        let (recorded, rust) = match &self.domain {
            DraftDomain::LaneRules { input, rust, .. } => {
                (recorded_input(input)?, serde_json::to_value(rust)?)
            }
            DraftDomain::RoutePriority { input, rust } => {
                (recorded_input(input)?, serde_json::to_value(rust)?)
            }
            DraftDomain::ExecutionGate { input, rust } => {
                // ADR D6: 記録は本文を除いた値で、`input_digest` もこの値から作る。
                (
                    recorded_input(&input.recorded())?,
                    serde_json::to_value(rust)?,
                )
            }
        };
        Ok(ScriptCompareRecord {
            domain: self.script_domain(),
            generation: self.generation.clone(),
            isolate_epoch,
            bundle_sha256: self.bundle_sha256.clone(),
            host_api: self.host_api.clone(),
            call_site: self.call_site().to_owned(),
            run_id: self.run_id.clone(),
            input_digest: recorded.input_digest,
            input: recorded.input,
            input_truncated: recorded.input_truncated,
            rust,
            ts: classified.ts,
            outcome: classified.outcome,
            error: classified.error,
            normalized: Vec::new(),
            safety_violations: classified.safety_violations,
            elapsed_us: u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
            applied_by: self.applied_by,
            fallback: self.fallback.clone(),
            canary_selected: self.canary_selected,
            canary_bps: self.canary_bps,
        })
    }

    /// TS の結果から記録を作る。
    pub fn finish(
        &self,
        result: &TsResult,
        elapsed: Duration,
    ) -> Result<ScriptCompareRecord, serde_json::Error> {
        let epoch = match result {
            TsResult::Reply(r) => r.isolate_epoch,
            TsResult::Error(e) => e.isolate_epoch.unwrap_or(0),
            TsResult::DeadlineExceeded(_) => 0,
        };
        let classified = match &self.domain {
            DraftDomain::LaneRules {
                rust,
                rust_lane,
                ceiling,
                ..
            } => classify_lane_rules(rust, *rust_lane, ceiling, result),
            DraftDomain::RoutePriority { input, rust } => {
                classify_route_priority(rust, input, result)
            }
            DraftDomain::ExecutionGate { input, rust } => {
                classify_execution_gate(rust, input, result)
            }
        };
        self.record(epoch, classified, elapsed)
    }

    /// TS を呼ばなかった記録（`unavailable`）。`reason` は `error.message` に残す。
    pub fn unavailable(&self, reason: &str) -> Result<ScriptCompareRecord, serde_json::Error> {
        self.record(
            0,
            Classified {
                ts: None,
                outcome: CompareOutcome::Unavailable,
                error: Some(CompareError::new("unavailable", reason)),
                safety_violations: Vec::new(),
            },
            Duration::ZERO,
        )
    }

    /// この下書きの領域の TS を `deadline` 付きで呼ぶ。
    pub async fn call(
        &self,
        script: &dyn PolicyScript,
        deadline: Duration,
    ) -> (TsResult, Duration) {
        match &self.domain {
            DraftDomain::LaneRules { input, .. } => {
                call_lane_rules(script, input.clone(), deadline).await
            }
            DraftDomain::RoutePriority { input, .. } => {
                call_route_priority(script, input.clone(), deadline).await
            }
            DraftDomain::ExecutionGate { input, .. } => {
                let input = input.as_ref().clone();
                with_deadline(script.execution_gate(input, deadline), deadline).await
            }
        }
    }
}

/// [`classify_cos_digest_pass`] の run ごとの結果。
#[derive(Debug, Clone, PartialEq)]
pub struct CosDigestClassified {
    pub ts: Option<CosDigestCommandsSummary>,
    pub ts_commands_head: Option<String>,
    pub outcome: CompareOutcome,
    pub error: Option<CompareError>,
    pub safety_violations: Vec<String>,
    /// outcome `explained` の仕様（ADR 2026-11-03-ts-intended-behavior-change D2）。
    pub explained_by: Option<ExplainedBy>,
}

impl CosDigestClassified {
    fn from_classified(c: Classified) -> Self {
        Self {
            ts: None,
            ts_commands_head: None,
            outcome: c.outcome,
            error: c.error,
            safety_violations: c.safety_violations,
            explained_by: None,
        }
    }

    fn invalid(ts: Option<CosDigestCommandsSummary>, message: &str, code: Option<&str>) -> Self {
        Self {
            ts,
            ts_commands_head: None,
            outcome: CompareOutcome::InvalidOutput,
            error: Some(CompareError::new("invalid_output", message)),
            safety_violations: code.map(str::to_owned).into_iter().collect(),
            explained_by: None,
        }
    }

    /// TS を呼ばなかった（`unavailable`）。
    pub fn unavailable(reason: &str) -> Self {
        Self {
            ts: None,
            ts_commands_head: None,
            outcome: CompareOutcome::Unavailable,
            error: Some(CompareError::new("unavailable", reason)),
            safety_violations: Vec::new(),
            explained_by: None,
        }
    }
}

/// [`classify_cos_digest_pass`] の結果（付記 S4-14・S4-15）。
#[derive(Debug, Clone, PartialEq)]
pub struct CosDigestPassClassified {
    /// 規則 1〜9 に通った TS の出力（mode `ts` で apply してよい）。通らなければ `None`。
    pub output: Option<CosDigestPassOutput>,
    /// TS の出力を使えない理由（mode `ts` の `fallback`）。`output` が `Some` なら `None`。
    pub fallback: Option<CosDigestFallback>,
    /// Rust の参照実装の出力の run ごと（並びも同じ）の比較。
    pub runs: Vec<(String, CosDigestClassified)>,
}

impl CosDigestPassClassified {
    /// TS を呼ばなかった pass（`unavailable`）。
    pub fn unavailable(rust: &CosDigestPassOutput, reason: &str) -> Self {
        Self {
            output: None,
            fallback: Some(CosDigestFallback::new("unavailable", reason)),
            runs: rust
                .runs
                .iter()
                .map(|r| (r.run_id.clone(), CosDigestClassified::unavailable(reason)))
                .collect(),
        }
    }

    pub fn run(&self, run_id: &str) -> Option<&CosDigestClassified> {
        self.runs
            .iter()
            .find(|(id, _)| id == run_id)
            .map(|(_, c)| c)
    }
}

/// pass の TS の結果を Rust の参照実装と比べ、規則 1〜9（付記 S4-14）で検証する。
/// 正規化はしない。run ごとに command 列（順序込み）を同じ直列化の byte 列で比べる。
/// 違った run は、承認済みの改善仕様 `approved` の期待差分（Rust の出力に編集を当てたもの）と byte 一致するときだけ
/// `explained` にする（ADR 2026-11-03-ts-intended-behavior-change D1・D2）。検証に落ちた出力は説明しない。
pub fn classify_cos_digest_pass(
    reads: &[CosDigestPassRead],
    rust: &CosDigestPassOutput,
    result: &TsResult,
    approved: &[ApprovedBehaviorChange],
) -> CosDigestPassClassified {
    let every = |c: &dyn Fn() -> CosDigestClassified| -> Vec<(String, CosDigestClassified)> {
        rust.runs.iter().map(|r| (r.run_id.clone(), c())).collect()
    };
    let raw = match result {
        TsResult::Reply(reply) => &reply.output,
        TsResult::DeadlineExceeded(d) => {
            let c = deadline_exceeded(*d);
            return CosDigestPassClassified {
                output: None,
                fallback: Some(CosDigestFallback::new(
                    "ts_error",
                    &format!("timeout: no reply within {} ms", d.as_millis()),
                )),
                runs: every(&|| CosDigestClassified::from_classified(c.clone())),
            };
        }
        TsResult::Error(e) => {
            let c = script_error(e);
            return CosDigestPassClassified {
                output: None,
                fallback: Some(CosDigestFallback::new(
                    "ts_error",
                    &format!("{}: {}", e.kind, e.message),
                )),
                runs: every(&|| CosDigestClassified::from_classified(c.clone())),
            };
        }
    };
    let ts: CosDigestPassOutput = match serde_json::from_value(raw.clone()) {
        Ok(ts) => ts,
        Err(e) => {
            let message = format!("cos_digest pass output: {e}");
            return CosDigestPassClassified {
                output: None,
                fallback: Some(CosDigestFallback::new("invalid_output", &message)),
                runs: every(&|| CosDigestClassified::invalid(None, &message, None)),
            };
        }
    };
    let violation = validate_cos_digest_pass(reads, &ts).err();
    // 規則 1〜6 は run ごとに当てる。規則 7〜9 の違反（run を名指さない・Rust の出力に無い run）は全行に付ける。
    let whole_pass = violation.as_ref().filter(|v| {
        !matches!(v, CosDigestPassViolation::Run { .. })
            && v.run_id()
                .is_none_or(|id| !rust.runs.iter().any(|r| r.run_id == id))
    });
    let runs = rust
        .runs
        .iter()
        .map(|r| {
            let ts_run = ts.runs.iter().find(|t| t.run_id == r.run_id);
            let summary = ts_run.and_then(|t| CosDigestCommandsSummary::of(&t.commands).ok());
            let input = reads
                .iter()
                .find(|x| x.run.run_id == r.run_id)
                .and_then(|x| x.input.as_ref());
            let per_run = match (ts_run, input) {
                (Some(t), Some(input)) => validate_cos_digest_commands(input, &t.commands).err(),
                _ => None,
            };
            let named = violation
                .as_ref()
                .filter(|v| v.run_id() == Some(r.run_id.as_str()));
            let c = match (whole_pass.or(named), per_run, ts_run) {
                (_, Some(v), _) => CosDigestClassified::invalid(
                    summary,
                    &format!("cos_digest pass output: run {}: {v}", r.run_id),
                    v.code(),
                ),
                (Some(v), None, _) => CosDigestClassified::invalid(
                    summary,
                    &format!("cos_digest pass output: {v}"),
                    v.safety_code(),
                ),
                (None, None, None) => CosDigestClassified::invalid(
                    None,
                    &format!("cos_digest pass output: run {} is missing", r.run_id),
                    None,
                ),
                (None, None, Some(t)) => {
                    let same = same_bytes(&t.commands.as_slice(), &r.commands.as_slice());
                    let explained_by = (!same)
                        .then(|| explain_cos_digest_run(approved, input, &r.commands, &t.commands))
                        .flatten();
                    CosDigestClassified {
                        ts: summary,
                        ts_commands_head: if same {
                            None
                        } else {
                            task_core::script_compare::cos_digest_text_head(&t.commands)
                        },
                        outcome: match (same, &explained_by) {
                            (true, _) => CompareOutcome::Match,
                            (false, Some(_)) => CompareOutcome::Explained,
                            (false, None) => CompareOutcome::Mismatch,
                        },
                        error: None,
                        safety_violations: Vec::new(),
                        explained_by,
                    }
                }
            };
            (r.run_id.clone(), c)
        })
        .collect();
    match violation {
        Some(v) => CosDigestPassClassified {
            output: None,
            fallback: Some(CosDigestFallback::new(v.fallback_code(), &v.to_string())),
            runs,
        },
        None => CosDigestPassClassified {
            output: Some(ts),
            fallback: None,
            runs,
        },
    }
}

/// 違った run 1 つを承認済みの仕様で説明する（D1）。入力が読めていない run は説明しない。
fn explain_cos_digest_run(
    approved: &[ApprovedBehaviorChange],
    input: Option<&CosDigestInput>,
    rust: &[CosDigestCommand],
    ts: &[CosDigestCommand],
) -> Option<ExplainedBy> {
    if approved.is_empty() {
        return None;
    }
    let input = serde_json::to_value(input?).ok()?;
    let rust = serde_json::to_value(rust).ok()?;
    let ts = serde_json::to_value(ts).ok()?;
    task_core::script_behavior_change::explain(
        approved,
        ScriptDomain::CosDigest,
        &input,
        &rust,
        &ts,
    )
}

/// `cos_digest` の pass の op（付記 S4-12）。名前は capability と同じ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CosDigestOp {
    ListRuns,
    ReadRun,
}

impl CosDigestOp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ListRuns => "cos_digest:list_runs",
            Self::ReadRun => "cos_digest:read_run",
        }
    }
}

/// op の応答（Host API の `HostResult` の形）。`code` は `HostError.code`（`exception`・`denied`・`busy`・
/// `invalid_output`・`timeout`・`terminated`）。
#[derive(Debug, Clone, PartialEq)]
pub enum CosDigestOpReply {
    Ok(serde_json::Value),
    Error { code: String, message: String },
}

impl CosDigestOpReply {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    /// `{ "kind": "ok", "value" }`・`{ "kind": "error", "error": { code, message } }`。
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Ok(value) => serde_json::json!({ "kind": "ok", "value": value }),
            Self::Error { code, message } => serde_json::json!({
                "kind": "error",
                "error": { "code": code, "message": message },
            }),
        }
    }
}

/// TS が呼んだ op 1 回。dispatcher の op server が `reply` で答える。
#[derive(Debug)]
pub struct CosDigestOpCall {
    pub op: CosDigestOp,
    pub request: serde_json::Value,
    pub reply: tokio::sync::oneshot::Sender<CosDigestOpReply>,
}

/// op の送り口（容量 [`COS_DIGEST_OP_SINK_CAPACITY`] の bounded channel）。
pub type CosDigestOpSink = tokio::sync::mpsc::Sender<CosDigestOpCall>;

/// op の送り口の容量（付記 S4-12）。
pub const COS_DIGEST_OP_SINK_CAPACITY: usize = 64;

/// 1 pass の op 呼び出しの上限。超えた op は `busy`（付記 S4-12）。
pub const COS_DIGEST_PASS_MAX_OPS: usize = 64;

/// 1 pass の deadline（付記 S4-12。op の待ちも含む）。
pub const COS_DIGEST_PASS_DEADLINE: Duration = Duration::from_millis(200);

/// [`PolicyScript::cos_digest_pass`] の引数（付記 S4-12）。
#[derive(Debug, Clone)]
pub struct CosDigestPassCall {
    /// ULID。記録の `pass_id`。
    pub pass_id: String,
    pub sink: CosDigestOpSink,
}

/// pass の TS を `deadline` 付きで呼ぶ。
pub async fn call_cos_digest_pass(
    script: &dyn PolicyScript,
    call: CosDigestPassCall,
    deadline: Duration,
) -> (TsResult, Duration) {
    with_deadline(script.cos_digest_pass(call, deadline), deadline).await
}

/// `cos_digest` の pass の記録の共通部分（付記 S4-15）。
#[derive(Debug, Clone)]
pub struct CosDigestPassDraft {
    pub generation: String,
    pub bundle_sha256: String,
    pub host_api: String,
    pub mode: ScriptMode,
    pub pass_id: String,
    /// 付記 S5-3: この pass が TS の側に選ばれたか（`ts` は常に真、`compare` は偽）。
    pub canary_selected: bool,
    pub canary_bps: Option<u32>,
}

/// 記録 1 行に要る run の値。
pub struct CosDigestRunRecord<'a> {
    pub input: &'a CosDigestInput,
    pub rust: &'a [CosDigestCommand],
    pub classified: &'a CosDigestClassified,
    pub applied_by: CosDigestAppliedBy,
    pub fallback: Option<CosDigestFallback>,
    pub applied: CosDigestApplied,
    pub ops: Vec<CosDigestOpLog>,
}

impl CosDigestPassDraft {
    /// run 1 つの記録を作る。
    pub fn record(
        &self,
        run: CosDigestRunRecord<'_>,
        isolate_epoch: u64,
        elapsed: Duration,
    ) -> Result<CosDigestCompareRecord, serde_json::Error> {
        let c = run.classified.clone();
        Ok(CosDigestCompareRecord {
            domain: ScriptDomain::CosDigest,
            generation: self.generation.clone(),
            isolate_epoch,
            bundle_sha256: self.bundle_sha256.clone(),
            host_api: self.host_api.clone(),
            call_site: COS_DIGEST_CALL_SITE.to_owned(),
            mode: self.mode,
            pass_id: self.pass_id.clone(),
            applied_by: run.applied_by,
            fallback: run.fallback,
            ops: run.ops,
            thread_id: run.input.thread.clone(),
            run_id: run.input.run_id.clone(),
            input_digest: task_core::script_compare::sha256_digest(&serde_json::to_vec(run.input)?),
            input: None,
            rust: CosDigestCommandsSummary::of(run.rust)?,
            ts: c.ts,
            ts_commands_head: c.ts_commands_head,
            outcome: c.outcome,
            error: c.error,
            normalized: Vec::new(),
            safety_violations: c.safety_violations,
            elapsed_us: u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
            applied: run.applied,
            canary_selected: self.canary_selected,
            canary_bps: self.canary_bps,
            explained_by: c.explained_by,
        })
    }
}

/// TS の結果の isolate の作り直し回数（分からなければ 0）。
pub fn ts_isolate_epoch(result: &TsResult) -> u64 {
    match result {
        TsResult::Reply(r) => r.isolate_epoch,
        TsResult::Error(e) => e.isolate_epoch.unwrap_or(0),
        TsResult::DeadlineExceeded(_) => 0,
    }
}

/// dispatcher 側の打ち切り（tokio の時計を使うので `tokio::time::pause` で試せる）。
async fn with_deadline(call: PolicyFuture<'_>, deadline: Duration) -> (TsResult, Duration) {
    let started = tokio::time::Instant::now();
    let result = match tokio::time::timeout(deadline, call).await {
        Ok(Ok(reply)) => TsResult::Reply(reply),
        Ok(Err(e)) => TsResult::Error(e),
        Err(_) => TsResult::DeadlineExceeded(deadline),
    };
    (result, started.elapsed())
}

/// `lane_rules` の TS を `deadline` 付きで呼ぶ。
pub async fn call_lane_rules(
    script: &dyn PolicyScript,
    input: LaneRulesInput,
    deadline: Duration,
) -> (TsResult, Duration) {
    with_deadline(script.lane_rules(input, deadline), deadline).await
}

/// `route_priority` の TS を `deadline` 付きで呼ぶ。
pub async fn call_route_priority(
    script: &dyn PolicyScript,
    input: RoutePriorityInput,
    deadline: Duration,
) -> (TsResult, Duration) {
    with_deadline(script.route_priority(input, deadline), deadline).await
}

/// 試験の偽の TS が使う、op の送り口越しの参照の pass（`cosDigestPass` と同じ手順）。
#[cfg(test)]
pub(crate) mod test_pass {
    use super::*;
    use task_core::chat::digest::{
        CosDigestListRunsReply, CosDigestPassRun, CosDigestReadRunRequest, cos_digest_decide,
    };

    /// op 1 回を送って応答を待つ（送れない・答えが無いときは `terminated`）。
    pub(crate) async fn op(
        sink: &CosDigestOpSink,
        op: CosDigestOp,
        request: serde_json::Value,
    ) -> CosDigestOpReply {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if sink
            .send(CosDigestOpCall {
                op,
                request,
                reply: tx,
            })
            .await
            .is_err()
        {
            return CosDigestOpReply::error("terminated", "sink closed");
        }
        rx.await
            .unwrap_or_else(|_| CosDigestOpReply::error("terminated", "no reply"))
    }

    /// list を読み、list の run を順に read し（失敗した run は飛ばす）、`cos_digest_decide` で決める。
    pub(crate) async fn reference_pass(
        sink: &CosDigestOpSink,
    ) -> Result<CosDigestPassOutput, PolicyScriptError> {
        let list = match op(sink, CosDigestOp::ListRuns, serde_json::json!({})).await {
            CosDigestOpReply::Ok(v) => serde_json::from_value::<CosDigestListRunsReply>(v)
                .map_err(|e| PolicyScriptError::new("exception", e.to_string()))?,
            CosDigestOpReply::Error { code, message } => {
                return Err(PolicyScriptError::new(
                    "exception",
                    format!("{code}: {message}"),
                ));
            }
        };
        let mut runs = Vec::new();
        for run in list.runs {
            let request = serde_json::to_value(CosDigestReadRunRequest {
                thread: run.thread.clone(),
                run_id: run.run_id.clone(),
            })
            .map_err(|e| PolicyScriptError::new("exception", e.to_string()))?;
            if let CosDigestOpReply::Ok(v) = op(sink, CosDigestOp::ReadRun, request).await {
                let input: CosDigestInput = serde_json::from_value(v)
                    .map_err(|e| PolicyScriptError::new("exception", e.to_string()))?;
                runs.push(CosDigestPassRun {
                    run_id: input.run_id.clone(),
                    commands: cos_digest_decide(&input),
                });
            }
        }
        Ok(CosDigestPassOutput { runs })
    }

    /// 参照の pass を `PolicyReply` にする。
    pub(crate) async fn reference_reply(
        call: &CosDigestPassCall,
    ) -> Result<PolicyReply, PolicyScriptError> {
        let output = reference_pass(&call.sink).await?;
        Ok(PolicyReply {
            output: serde_json::to_value(output)
                .map_err(|e| PolicyScriptError::new("exception", e.to_string()))?,
            isolate_epoch: 1,
        })
    }
}

#[cfg(test)]
#[path = "script_policy_tests.rs"]
mod tests;
