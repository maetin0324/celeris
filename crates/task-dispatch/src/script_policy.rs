//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2: dispatcher が TS 制御層の判定を呼ぶための境界。
//!
//! - [`PolicyScript`] は DTO（`serde_json::Value` と task-core の入力 DTO）だけを使い、deno_core に依存しない。
//!   本物の実装は daemon（`crates/celeris`）が `ScriptRuntimeHandle` で作り、dispatcher に注入する。
//! - dispatcher が使う値は常に Rust の参照実装の出力。TS の出力は `script_compare_recorded` に書くだけ（S2-6）。
//! - [`classify_lane_rules`] は TS の結果を Rust の出力と比べ、outcome・error・安全違反（S2-8）を決める純粋関数。
//! - [`ScriptDomainStore`] は mode の表と記録の store の口。`TaskStore` に欄を足さずに `SqliteStore` を渡せるよう、
//!   この crate に置く。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use task_core::model_policy::{LaneCeiling, RuleMatch, TaskFeatures};
use task_core::script_compare::{
    CompareError, CompareOutcome, LaneRulesInput, ScriptCompareRecord, ScriptDomain,
    ScriptDomainModeRow, ScriptGeneration, recorded_input,
};
use task_core::{SqliteStore, StoreError, TaskId, Tier};

/// 1 呼び出しの deadline（境界 ADR (f)・S2-6）。超えたら待つのをやめ、`ts_error`（`timeout`）を記録する。
pub const SCRIPT_CALL_DEADLINE: Duration = Duration::from_millis(50);

/// `lane_rules` の比較を付けた呼び出し箇所（S2-4 の `call_site`）。
pub const LANE_RULES_CALL_SITE: &str = "dispatch_run.decide_lane";

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
}

/// mode の表と compare の記録の store（`SqliteStore` が実装する）。
pub trait ScriptDomainStore: Send + Sync {
    fn script_domain_modes(&self) -> Result<Vec<ScriptDomainModeRow>, StoreError>;
    fn script_generation_get(&self, id: &str) -> Result<Option<ScriptGeneration>, StoreError>;
    /// 重複の抑え（S2-4）の後に追記したら `true`。
    fn script_compare_append(
        &self,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError>;
}

impl ScriptDomainStore for SqliteStore {
    fn script_domain_modes(&self) -> Result<Vec<ScriptDomainModeRow>, StoreError> {
        SqliteStore::script_domain_modes(self)
    }

    fn script_generation_get(&self, id: &str) -> Result<Option<ScriptGeneration>, StoreError> {
        SqliteStore::script_generation_get(self, id)
    }

    fn script_compare_append(
        &self,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        SqliteStore::script_compare_append(self, task_id, record, now)
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
        TsResult::DeadlineExceeded(d) => Classified {
            ts: None,
            outcome: CompareOutcome::TsError,
            error: Some(CompareError::new(
                "timeout",
                &format!("deadline {} ms exceeded", d.as_millis()),
            )),
            safety_violations: Vec::new(),
        },
        TsResult::Error(e) if e.kind == "invalid_output" => Classified {
            ts: None,
            outcome: CompareOutcome::InvalidOutput,
            error: Some(CompareError::new("invalid_output", &e.message)),
            safety_violations: Vec::new(),
        },
        TsResult::Error(e) => Classified {
            ts: None,
            outcome: CompareOutcome::TsError,
            error: Some(CompareError::new(e.kind.clone(), &e.message)),
            safety_violations: e.safety_violation.iter().cloned().collect(),
        },
        TsResult::Reply(reply) => classify_output(rust, rust_lane, ceiling, &reply.output),
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
    let same = match (serde_json::to_vec(&ts), serde_json::to_vec(rust)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
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

/// 記録の共通部分（generation・bundle・入力・Rust の出力）。TS の結果を受けて [`CompareDraft::finish`] で記録にする。
#[derive(Debug, Clone)]
pub struct CompareDraft {
    pub generation: String,
    pub bundle_sha256: String,
    pub host_api: String,
    pub input: LaneRulesInput,
    pub rust: LaneRulesOutput,
    pub rust_lane: Tier,
    pub ceiling: LaneCeiling,
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
            input: LaneRulesInput::redacted(features),
            rust: task_core::model_policy::match_rule(features).into(),
            rust_lane,
            ceiling: ceiling.clone(),
        }
    }

    fn record(
        &self,
        isolate_epoch: u64,
        classified: Classified,
        elapsed: Duration,
    ) -> Result<ScriptCompareRecord, serde_json::Error> {
        let recorded = recorded_input(&self.input)?;
        Ok(ScriptCompareRecord {
            domain: ScriptDomain::LaneRules,
            generation: self.generation.clone(),
            isolate_epoch,
            bundle_sha256: self.bundle_sha256.clone(),
            host_api: self.host_api.clone(),
            call_site: LANE_RULES_CALL_SITE.to_owned(),
            run_id: None,
            input_digest: recorded.input_digest,
            input: recorded.input,
            input_truncated: recorded.input_truncated,
            rust: serde_json::to_value(&self.rust)?,
            ts: classified.ts,
            outcome: classified.outcome,
            error: classified.error,
            normalized: Vec::new(),
            safety_violations: classified.safety_violations,
            elapsed_us: u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
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
        let classified = classify_lane_rules(&self.rust, self.rust_lane, &self.ceiling, result);
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
}

/// TS を `deadline` 付きで呼ぶ（dispatcher 側の打ち切り。tokio の時計を使うので `tokio::time::pause` で試せる）。
pub async fn call_lane_rules(
    script: &dyn PolicyScript,
    input: LaneRulesInput,
    deadline: Duration,
) -> (TsResult, Duration) {
    let started = tokio::time::Instant::now();
    let result = match tokio::time::timeout(deadline, script.lane_rules(input, deadline)).await {
        Ok(Ok(reply)) => TsResult::Reply(reply),
        Ok(Err(e)) => TsResult::Error(e),
        Err(_) => TsResult::DeadlineExceeded(deadline),
    };
    (result, started.elapsed())
}

#[cfg(test)]
#[path = "script_policy_tests.rs"]
mod tests;
