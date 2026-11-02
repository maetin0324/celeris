//! 付記 S4-5: 領域 `cos_digest` の compare の記録（表 `script_compare_log` の `json`）。
//!
//! S2-4 の `script_compare_recorded` と同じ欄に、次の違いを持つ。
//! - `input` は常に `null`。`input_digest`・`thread_id`・`run_id` だけを残す（items の本文は残さない）。
//! - `rust`・`ts` は command の本文ではなく `commands_digest` と `command_kinds`。`mismatch` のときだけ
//!   `ts_commands_head`（TS の `post_digest.text` の先頭 512 byte）を足す。
//! - `applied` は Rust が実際に apply した結果。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CompareError, CompareOutcome, ScriptDomain, sha256_digest, truncate_error_message};
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

/// Rust が apply した結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestApplied {
    /// `post_digest` が書いた（または冪等 key で既にあった）message の id。
    pub message_id: String,
    /// `requeue_run` が `pending` に戻した item の数（requeue が無ければ 0）。
    pub requeued: u64,
}

/// `script_compare_log.json` の 1 行（付記 S4-5）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestCompareRecord {
    /// 常に `cos_digest`。
    pub domain: ScriptDomain,
    pub generation: String,
    pub isolate_epoch: u64,
    pub bundle_sha256: String,
    pub host_api: String,
    /// `cos_digest.digest_one`。
    pub call_site: String,
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
}
