//! Host API `celeris:host` v1 の DTO（境界 ADR 2026-11-02 (c) の値の写像）。
//!
//! Rust の型が正本で、`script-schema` がここから JSON Schema（`docs/api/script-host/host-v1.schema.json`）を出す。
//! 規則:
//! - 全 struct は `deny_unknown_fields`。TS が知らない欄を足したら拒否する。
//! - `Option<T>` は欄を常に出し、値が無ければ `null`。TS が欄を省いたら拒否する（`Option::deserialize` で必須にする）。
//! - 2^53−1 を超えうる整数は [`U64String`]（10 進の string）。
//! - unit の enum は snake_case の文字列。未知の値は拒否する。データ付きの enum は `tag = "kind"`。
//! - 時刻は [`Timestamp`]（RFC 3339、UTC、末尾 `Z`）。
//! - TS の `HostError { code, message }` は [`HostError`] で、[`crate::ScriptError`] へ写す。

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ScriptError;

/// Host API の版（semver）。bundle の manifest は `host_api: "^1"` の形で要求する。
pub const HOST_API_VERSION: &str = "1.0.0";
/// Host API の module 名。bundle が import してよいのはこれと bundle 自身だけ。
pub const HOST_MODULE: &str = "celeris:host";

/// bundle の manifest が宣言する capability。Rust は「宣言 ∩ 領域ごとの許可表」だけを有効にする。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum Capability {
    /// 試験用 async op `op_host_selftest_echo`（要求の scope の task を写し返すだけ）。
    #[serde(rename = "selftest:echo_task")]
    SelftestEchoTask,
}

/// 2^53−1 を超えうる符号なし整数。JSON と TS では 10 進の string（先頭 0 なし）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct U64String(pub u64);

impl Serialize for U64String {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for U64String {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // number は受けない（2^53 を超えると TS 側で値が丸まるため）。
        let s = String::deserialize(d)?;
        let canonical = !s.is_empty()
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s == "0" || !s.starts_with('0'));
        if !canonical {
            return Err(serde::de::Error::custom(format!(
                "expected a decimal string without leading zeros, got {s:?}"
            )));
        }
        s.parse::<u64>()
            .map(U64String)
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for U64String {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "U64String".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^(0|[1-9][0-9]{0,19})$",
            "description": "0 以上 2^64−1 以下の整数の 10 進表記（2^53−1 を超えうる値は number にしない）"
        })
    }
}

/// RFC 3339 の UTC 時刻（末尾 `Z`）。TS に時計を持たせないので、現在時刻はこの型で入力に入れる。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(String);

impl Timestamp {
    /// RFC 3339 で UTC（`Z`）の文字列だけを受ける。
    pub fn parse(s: &str) -> Result<Self, String> {
        let t = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|e| format!("invalid RFC 3339 time {s:?}: {e}"))?;
        if !s.ends_with('Z') || t.offset() != time::UtcOffset::UTC {
            return Err(format!("time must be UTC with a trailing Z, got {s:?}"));
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Timestamp::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Timestamp {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Timestamp".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "format": "date-time",
            "pattern": "Z$",
            "description": "RFC 3339 の UTC 時刻（末尾 Z）"
        })
    }
}

/// `HostError.code`。[`ScriptError`] の種類と 1 対 1。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostErrorCode {
    Exception,
    Timeout,
    Busy,
    InvalidOutput,
    Denied,
    Terminated,
    GenerationGone,
}

/// Host API の誤り。op は `{ kind: "error", error }` で返し、`celeris:host` が `HostError` として throw する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostError {
    pub code: HostErrorCode,
    pub message: String,
}

impl HostError {
    pub fn new(code: HostErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<HostError> for ScriptError {
    fn from(e: HostError) -> Self {
        let HostError { code, message } = e;
        match code {
            HostErrorCode::Exception => ScriptError::Exception { message },
            HostErrorCode::Timeout => ScriptError::Timeout,
            HostErrorCode::Busy => ScriptError::Busy,
            HostErrorCode::InvalidOutput => ScriptError::InvalidOutput { reason: message },
            HostErrorCode::Denied => ScriptError::Denied { reason: message },
            HostErrorCode::Terminated => ScriptError::Terminated,
            HostErrorCode::GenerationGone => ScriptError::GenerationGone,
        }
    }
}

/// op の戻り値。`celeris:host` は `ok` なら `value` を返し、`error` なら `HostError` を throw する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostResult<T> {
    Ok { value: T },
    Error { error: HostError },
}

impl<T> From<Result<T, HostError>> for HostResult<T> {
    fn from(r: Result<T, HostError>) -> Self {
        match r {
            Ok(value) => HostResult::Ok { value },
            Err(error) => HostResult::Error { error },
        }
    }
}

/// 試験用の優先度（unit の enum の写像を確かめる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelftestPriority {
    Low,
    Normal,
    High,
}

/// `op_host_selftest_echo` の引数（TS → Rust）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelftestEchoRequest {
    /// 対象の task の id（ULID の文字列）。要求の scope に無ければ `denied`。
    pub task_id: String,
    /// 無ければ `null`（欄は省かない）。
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required)]
    pub note: Option<String>,
    /// token 数など 2^53−1 を超えうる値。
    pub tokens: U64String,
    pub priority: SelftestPriority,
    pub at: Timestamp,
}

/// `op_host_selftest_echo` が見たもの（データ付きの enum の写像を確かめる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelftestObservation {
    /// note があった。
    Noted { length: u32 },
    /// note が null だった。
    Empty,
}

/// `op_host_selftest_echo` の戻り値（Rust → TS）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelftestEchoReply {
    pub task_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required)]
    pub note: Option<String>,
    pub tokens: U64String,
    pub priority: SelftestPriority,
    pub at: Timestamp,
    pub observation: SelftestObservation,
}

/// schema の根。Host API v1 の DTO を 1 つの文書に集める（値として使わない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "celeris:host v1")]
pub struct HostApiV1 {
    pub version: String,
    pub capability: Capability,
    pub host_error: HostError,
    pub selftest_echo_request: SelftestEchoRequest,
    pub selftest_echo_result: HostResult<SelftestEchoReply>,
}

/// Host API v1 の JSON Schema（整形済み、末尾改行付き）。`script-schema` と試験が使う。
pub fn host_api_schema_json() -> String {
    let schema = schemars::schema_for!(HostApiV1);
    let mut out = serde_json::to_string_pretty(&schema).unwrap_or_default();
    out.push('\n');
    out
}
