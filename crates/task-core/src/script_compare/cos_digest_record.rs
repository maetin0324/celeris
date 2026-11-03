//! 付記 S4-5: 領域 `cos_digest` の compare の記録（表 `script_compare_log` の `json`）。
//!
//! S2-4 の `script_compare_recorded` と同じ欄に、次の違いを持つ。
//! - `input` は常に `null`。`input_digest`・`thread_id`・`run_id` だけを残す（items の本文は残さない）。
//! - `rust`・`ts` は command の本文ではなく `commands_digest` と `command_kinds`。`mismatch` のときだけ
//!   `ts_commands_head`（TS の `post_digest.text` の先頭 512 byte）を足す。
//! - `applied` は Rust が実際に apply した結果。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    CompareError, CompareOutcome, ScriptDomain, ScriptMode, sha256_digest, truncate_error_message,
};
use crate::chat::digest::CosDigestCommand;

/// command 列の要約（本文を持たない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestCommandsSummary {
    /// `sha256:` + command 列の直列化の sha256。
    pub commands_digest: String,
    /// `post_digest`・`requeue_run` の並び。
    pub command_kinds: Vec<String>,
}

impl CosDigestCommandsSummary {
    pub fn of(commands: &[CosDigestCommand]) -> Result<Self, serde_json::Error> {
        Ok(Self {
            commands_digest: sha256_digest(&serde_json::to_vec(commands)?),
            command_kinds: commands
                .iter()
                .map(|c| cos_digest_command_kind(c).to_owned())
                .collect(),
        })
    }
}

/// command の `kind`（直列化の tag と同じ名前）。
pub fn cos_digest_command_kind(command: &CosDigestCommand) -> &'static str {
    match command {
        CosDigestCommand::PostDigest { .. } => "post_digest",
        CosDigestCommand::RequeueRun { .. } => "requeue_run",
    }
}

/// `mismatch` のときに残す TS の `post_digest.text` の先頭（512 byte、UTF-8 の文字の途中では切らない）。
pub fn cos_digest_text_head(commands: &[CosDigestCommand]) -> Option<String> {
    commands.iter().find_map(|c| match c {
        CosDigestCommand::PostDigest { text, .. } => Some(truncate_error_message(text)),
        CosDigestCommand::RequeueRun { .. } => None,
    })
}

/// apply した結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestApplied {
    /// `post_digest` が書いた（または冪等 key で既にあった）message の id。
    pub message_id: String,
    /// `requeue_run` が `pending` に戻した item の数（requeue が無ければ 0）。
    pub requeued: u64,
}

/// その run に apply した command を出した側（付記 S4-15）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CosDigestAppliedBy {
    Rust,
    Ts,
}

impl CosDigestAppliedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Ts => "ts",
        }
    }
}

/// mode `ts` で Rust の command を apply した理由（付記 S4-15）。`message` は 512 byte で切る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestFallback {
    /// `ts_error`・`invalid_output`・`run_outside_list`・`run_missing`・`run_unreadable`・`run_order`・
    /// `mode_changed`・`unavailable`。
    pub code: String,
    pub message: String,
}

impl CosDigestFallback {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: truncate_error_message(message),
        }
    }
}

/// その run に関わる op 呼び出し 1 回（付記 S4-15）。本文は残さない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestOpLog {
    /// `cos_digest:list_runs`・`cos_digest:read_run`。
    pub op: String,
    /// `sha256:` + 要求の直列化の sha256。
    pub request_digest: String,
    /// `sha256:` + 応答（`{kind:"ok",value}`・`{kind:"error",error}`）の直列化の sha256。
    pub reply_digest: String,
    /// `ok`・`error`・`denied`・`memo`（memo から答えた 2 回目以降）。
    pub outcome: String,
    pub elapsed_us: u64,
}

/// `script_compare_log.json` の 1 行（付記 S4-5・S4-15）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestCompareRecord {
    /// 常に `cos_digest`。
    pub domain: ScriptDomain,
    pub generation: String,
    pub isolate_epoch: u64,
    pub bundle_sha256: String,
    pub host_api: String,
    /// `cos_digest.pass`。
    pub call_site: String,
    /// pass の始めに読んだ mode（`compare`・`ts`）。
    pub mode: ScriptMode,
    /// 同じ pass の行どうしで同じ値（ULID）。
    pub pass_id: String,
    /// その run に apply した command を出した側。`compare` では常に `rust`。
    pub applied_by: CosDigestAppliedBy,
    /// mode `ts` で Rust を apply した理由（`compare` と TS を apply した行では `null`）。
    pub fallback: Option<CosDigestFallback>,
    /// その run に関わる op 呼び出し（その pass の `list_runs` と、その run の `read_run`）。
    pub ops: Vec<CosDigestOpLog>,
    pub thread_id: String,
    pub run_id: String,
    /// `sha256:` + `CosDigestInput` の直列化の sha256。
    pub input_digest: String,
    /// 常に `null`（本文を残さない）。
    pub input: Option<serde_json::Value>,
    pub rust: CosDigestCommandsSummary,
    /// TS の出力の要約（`ts_error`・`unavailable`・deserialize できない出力は `null`）。
    pub ts: Option<CosDigestCommandsSummary>,
    /// `mismatch` のときだけ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_commands_head: Option<String>,
    pub outcome: CompareOutcome,
    pub error: Option<CompareError>,
    /// 当てた正規化の規則名（S4 では常に空）。
    pub normalized: Vec<String>,
    /// S4-3 の code（`key_mismatch`・`scope_mismatch`・`card_outside_items`・`requeue_state_invalid`・
    /// `duplicate_command`）と S2-8 の `denied_*`。
    pub safety_violations: Vec<String>,
    pub elapsed_us: u64,
    pub applied: CosDigestApplied,
    /// 付記 S5-3: mode `canary`・`ts` でこの pass が TS の側に選ばれたか。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub canary_selected: bool,
    /// 付記 S5-3: mode `canary` の比率（それ以外は `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canary_bps: Option<u32>,
}
