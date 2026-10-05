//! 2026-10-04 多目的 routing ADR §7.1・§10 Phase 4: shadow の設定型・記録型・日次上限の型（純粋）。
//!
//! - `ShadowPolicy` は実行 shadow（候補モデルで実際に生成する）の opt-in-capped 設定。既定は
//!   `execute = false`（対象なし）。有効化には allowlist・sample_rate・日次上限 3 種・並列/queue の上限・
//!   timeout が全て要る（`validate`）。
//! - `ShadowRecord` は `Event::RoutingShadowRecorded` の中身。primary の判断・成功/失敗・attempts を
//!   変えない別欄の記録で、本文・prompt・credential は持たない（出力は SHA-256 と tokens だけ）。
//! - 日次上限の予約は `SqliteStore::routing_shadow_*`（store/routing_shadow.rs、migration 0049）が
//!   UTC 日界で原子的に行う。ここには判定と型だけを置き、時計は呼び手が `now` で渡す。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

/// `ShadowRecord` の wire 版（§6 の version 1）。
pub const SHADOW_RECORD_VERSION: u32 = 1;

/// allowlist の 1 次元で「何でもよい」を明示する値。空の次元は「対象なし」で、`*` とは別。
pub const SHADOW_ALLOW_ANY: &str = "*";

/// 実行 shadow の対象。各次元は完全一致（または `*`）。**どれか 1 次元でも空なら対象なし**。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShadowAllowlist {
    #[serde(default)]
    pub task_kinds: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub lanes: Vec<String>,
    #[serde(default)]
    pub sources: Vec<String>,
}

impl ShadowAllowlist {
    /// 4 次元が全て空（設定が書かれていない）。
    pub fn is_empty(&self) -> bool {
        self.task_kinds.is_empty()
            && self.roles.is_empty()
            && self.lanes.is_empty()
            && self.sources.is_empty()
    }

    /// `target` が 4 次元全てで一致するか。空の次元は何にも一致しない。
    pub fn matches(&self, target: &ShadowTarget) -> bool {
        fn dim(allowed: &[String], value: &str) -> bool {
            allowed
                .iter()
                .any(|a| a == SHADOW_ALLOW_ANY || a.as_str() == value)
        }
        dim(&self.task_kinds, &target.task_kind)
            && dim(&self.roles, &target.role)
            && dim(&self.lanes, &target.lane)
            && dim(&self.sources, &target.source)
    }
}

/// shadow の対象判定に使う、primary 決定 1 件の属性（本文は持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowTarget {
    pub task_kind: String,
    pub role: String,
    pub lane: String,
    /// 候補の供給元（shadow で呼ぶ側）の id。
    pub source: String,
}

/// §7.1 の `[model_routing.shadow]`。既定は off（`execute = false`、上限は未設定）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShadowPolicy {
    /// 実行 shadow を行うか。既定 false。decision shadow（追加呼出しなし）はこれに依らない。
    #[serde(default)]
    pub execute: bool,
    #[serde(default)]
    pub allowlist: ShadowAllowlist,
    /// [0,1]。`decision_id` の安定 hash で標本化する（`sampled_in`）。既定 0。
    #[serde(default)]
    pub sample_rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_max_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_max_effective_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_queue_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// `ShadowPolicy::validate` の失敗（設定の読込エラーにする）。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ShadowPolicyError {
    #[error("model_routing.shadow.sample_rate must be a finite number in [0, 1] (got {0})")]
    SampleRate(f64),
    #[error("model_routing.shadow.execute = true requires `{0}`")]
    Missing(&'static str),
    #[error("model_routing.shadow.{0} must be greater than 0")]
    NotPositive(&'static str),
    #[error("model_routing.shadow.execute = true requires a non-empty allowlist")]
    EmptyAllowlist,
}

/// 検証済みの日次上限（UTC 日あたり）。予約 store に渡す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowDailyCaps {
    pub max_requests: u64,
    pub max_tokens: u64,
    pub max_effective_usd: f64,
}

impl ShadowPolicy {
    /// 設定の検証。`execute = false` でも sample_rate の値域と、書かれた上限の正値は確かめる。
    /// `execute = true` は §7.1 の全項目が必要。
    pub fn validate(&self) -> Result<(), ShadowPolicyError> {
        if !self.sample_rate.is_finite() || !(0.0..=1.0).contains(&self.sample_rate) {
            return Err(ShadowPolicyError::SampleRate(self.sample_rate));
        }
        let ints: [(&'static str, Option<u64>); 5] = [
            ("daily_max_requests", self.daily_max_requests),
            ("daily_max_tokens", self.daily_max_tokens),
            ("max_concurrency", self.max_concurrency.map(u64::from)),
            ("max_queue_depth", self.max_queue_depth.map(u64::from)),
            ("timeout_ms", self.timeout_ms),
        ];
        for (name, value) in ints {
            match value {
                Some(0) => return Err(ShadowPolicyError::NotPositive(name)),
                None if self.execute => return Err(ShadowPolicyError::Missing(name)),
                _ => {}
            }
        }
        match self.daily_max_effective_usd {
            Some(usd) if !usd.is_finite() || usd <= 0.0 => {
                return Err(ShadowPolicyError::NotPositive("daily_max_effective_usd"));
            }
            None if self.execute => {
                return Err(ShadowPolicyError::Missing("daily_max_effective_usd"));
            }
            _ => {}
        }
        if self.execute && self.allowlist.is_empty() {
            return Err(ShadowPolicyError::EmptyAllowlist);
        }
        Ok(())
    }

    /// 日次上限。`execute = true` で全て書かれ正のときだけ `Some`（それ以外は実行しない）。
    pub fn daily_caps(&self) -> Option<ShadowDailyCaps> {
        if !self.execute || self.validate().is_err() {
            return None;
        }
        Some(ShadowDailyCaps {
            max_requests: self.daily_max_requests?,
            max_tokens: self.daily_max_tokens?,
            max_effective_usd: self.daily_max_effective_usd?,
        })
    }

    /// 実行 shadow の入口の判定（予約より前）。off・不正設定は `Off`、allowlist 外は `NotAllowlisted`、
    /// 標本外は `SampledOut`。`Ok` のときだけ予約へ進む。
    pub fn admit(&self, target: &ShadowTarget, decision_id: &str) -> Result<(), ShadowReason> {
        if self.daily_caps().is_none() {
            return Err(ShadowReason::Off);
        }
        if !self.allowlist.matches(target) {
            return Err(ShadowReason::NotAllowlisted);
        }
        if !sampled_in(decision_id, self.sample_rate) {
            return Err(ShadowReason::SampledOut);
        }
        Ok(())
    }
}

/// `decision_id` の安定 hash（SHA-256 の先頭 8 byte を [0,1) に写す）が `sample_rate` 未満なら標本内。
/// 同じ id は process・host・再起動をまたいで同じ結果になる。0 は常に外、1 は常に内。
pub fn sampled_in(decision_id: &str, sample_rate: f64) -> bool {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return false;
    }
    if sample_rate >= 1.0 {
        return true;
    }
    let digest = Sha256::digest(decision_id.as_bytes());
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    // 53 bit に落として f64 で正確に表す。
    let unit = (u64::from_be_bytes(head) >> 11) as f64 / (1u64 << 53) as f64;
    unit < sample_rate
}

/// shadow の種類。`decision` は追加呼出しなしの判断比較、`execution` は候補で実際に生成した。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowKind {
    Decision,
    Execution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowStatus {
    Completed,
    Failed,
    Dropped,
}

/// failed/dropped の安定 reason code（説明文は別欄 `detail`。secret を入れない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowReason {
    /// shadow 実行が設定で off（既定）または設定が不完全。
    Off,
    NotAllowlisted,
    SampledOut,
    QueueFull,
    ConcurrencyLimit,
    /// 日次の request/token/effective 上限を超える（予約できない）。
    CapExceeded,
    /// 最悪消費の費用が分からない（実行対象にしない）。
    UnknownCost,
    Timeout,
    Privacy,
    /// primary と同じ非分離の resource group を使う。
    ResourceGroupShared,
    /// primary の予約待ちが発生したので未開始の shadow を落とした。
    PrimaryPressure,
    UpstreamError,
}

/// `Event::RoutingShadowRecorded` の中身（wire では event の欄に平たく並ぶ）。primary の結果とは別欄。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShadowRecord {
    pub shadow_id: String,
    /// shadow が比べた primary の決定（`RoutingTraceV1.decision_id`）。
    pub primary_decision_id: String,
    pub kind: ShadowKind,
    pub status: ShadowStatus,
    /// completed は `None`。failed/dropped は必須。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ShadowReason>,
    /// 秘密を含まない短い説明（任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 判断した policy/estimator の版。
    pub policy_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// 出力本文の SHA-256（小文字 16 進 64 桁）。本文は保存しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cash_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// 実行 shadow の日次予約（store の reservation id）。decision shadow・予約前の drop は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<String>,
}

/// `ShadowRecord::validate` の失敗。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShadowRecordError {
    #[error("shadow record {0}: failed/dropped requires a reason")]
    MissingReason(String),
    #[error("shadow record {0}: completed must not carry a reason")]
    UnexpectedReason(String),
    #[error("shadow record {0}: output_sha256 must be 64 lowercase hex digits")]
    BadHash(String),
    #[error("shadow record {0}: decision shadow must not carry tokens, cost or output")]
    DecisionWithUsage(String),
    #[error("shadow record {0}: cost must be finite and non-negative")]
    BadCost(String),
}

impl ShadowRecord {
    pub fn validate(&self) -> Result<(), ShadowRecordError> {
        let id = || self.shadow_id.clone();
        match (self.status, self.reason) {
            (ShadowStatus::Completed, Some(_)) => {
                return Err(ShadowRecordError::UnexpectedReason(id()));
            }
            (ShadowStatus::Failed | ShadowStatus::Dropped, None) => {
                return Err(ShadowRecordError::MissingReason(id()));
            }
            _ => {}
        }
        if let Some(hash) = &self.output_sha256
            && !is_sha256_hex(hash)
        {
            return Err(ShadowRecordError::BadHash(id()));
        }
        if self.kind == ShadowKind::Decision
            && (self.input_tokens.is_some()
                || self.output_tokens.is_some()
                || self.output_sha256.is_some()
                || self.cash_usd.is_some()
                || self.effective_usd.is_some())
        {
            return Err(ShadowRecordError::DecisionWithUsage(id()));
        }
        for usd in [self.cash_usd, self.effective_usd].into_iter().flatten() {
            if !usd.is_finite() || usd < 0.0 {
                return Err(ShadowRecordError::BadCost(id()));
            }
        }
        Ok(())
    }
}

/// 出力本文から `output_sha256` を作る（本文は呼び手の手元から出さない）。
pub fn output_sha256(output: &[u8]) -> String {
    Sha256::digest(output)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 予約の入力: 開始前に見積もる最悪消費（input + output 上限の tokens と、その effective 費用）。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowReservationRequest {
    pub shadow_id: String,
    /// 予約した instance（daemon handoff の監査用。判定には使わない）。
    pub owner: String,
    /// output 上限込みの最悪 tokens。
    pub worst_tokens: u64,
    /// output 上限込みの最悪 effective 費用。`None`・非有限・負は未知として予約しない。
    pub worst_effective_usd: Option<f64>,
}

/// 予約の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowReservation {
    Reserved {
        reservation_id: String,
        day: String,
    },
    /// 予約しなかった（`UnknownCost` / `CapExceeded`）。shadow は dropped で記録する。
    Denied(ShadowReason),
}

/// 予約の確定。completed は実測を計上する。failed/timeout は実測と予約の大きい方を計上する
/// （送信済みの推論はゼロにできないため、§7.1）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadowSettlement {
    Completed {
        tokens: u64,
        effective_usd: f64,
    },
    Failed {
        tokens: Option<u64>,
        effective_usd: Option<f64>,
    },
    TimedOut,
}

/// UTC 日 1 日分の消費（予約中 + 確定）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShadowDailyUsage {
    pub requests: u64,
    pub tokens: u64,
    pub effective_usd: f64,
    /// まだ確定していない予約の数（再起動で取り残されたものも上限に数え続ける）。
    pub open_reservations: u64,
}

/// UTC 日界の key（`YYYY-MM-DD`）。`now` の offset に依らず UTC に直してから切る。
pub fn utc_day(now: OffsetDateTime) -> String {
    let d = now.to_offset(time::UtcOffset::UTC).date();
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// effective 費用の記録単位（micro USD、切り上げ）。上限との比較は整数で行い、浮動小数の累積誤差で
/// 上限を越えないようにする。
pub fn usd_to_micros_ceil(usd: f64) -> Option<u64> {
    if !usd.is_finite() || usd < 0.0 {
        return None;
    }
    let micros = snap_micros(usd).unwrap_or_else(|| (usd * 1_000_000.0).ceil());
    if micros > u64::MAX as f64 {
        return None;
    }
    Some(micros as u64)
}

/// 上限側は切り捨て（上限を広げない）。
pub fn usd_cap_to_micros_floor(usd: f64) -> u64 {
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    let micros = snap_micros(usd).unwrap_or_else(|| (usd * 1_000_000.0).floor());
    if micros > u64::MAX as f64 {
        u64::MAX
    } else {
        micros as u64
    }
}

/// 10 進の小数（0.01 など）の 2 進誤差で 1 micro ずれないよう、整数にごく近い値は整数に寄せる。
fn snap_micros(usd: f64) -> Option<f64> {
    let micros = usd * 1_000_000.0;
    let rounded = micros.round();
    ((micros - rounded).abs() < 1e-6).then_some(rounded)
}

#[cfg(test)]
mod tests;
