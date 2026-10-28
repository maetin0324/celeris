//! `[cos]`（ADR 2026-10-05-cos-chat-home D2/D4）: CoS worker とチャット保持設定。
//! 未知の鍵は他の節と同じく `deny_unknown_fields` で弾く。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::chat::attachments::ChatAttachmentLimits;
use task_worker::cos_chat::{HarnessCapabilities, MissingCapability, capability_reason};

use super::{Config, ConfigError};

const MIB: u64 = 1024 * 1024;

/// `[cos]`。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CosConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub route_candidates: Vec<super::RouteCandidateConfig>,
    #[serde(default = "default_worker_reserve")]
    pub worker_reserve_five_hour: f64,
    #[serde(default)]
    pub harness: CosHarness,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_max_wall_secs")]
    pub max_wall_secs: u64,
    #[serde(default)]
    pub triage: CosTriageConfig,
    /// D4: run 終端後の text/tool 詳細の chat_events を消すまでの日数（既定 30）。
    #[serde(default = "default_retention_days")]
    pub stream_retention_days: u32,
    #[serde(default)]
    pub attachments: CosAttachmentsConfig,
}

impl Default for CosConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            route_candidates: Vec::new(),
            worker_reserve_five_hour: default_worker_reserve(),
            harness: CosHarness::default(),
            max_turns: default_max_turns(),
            max_wall_secs: default_max_wall_secs(),
            triage: CosTriageConfig::default(),
            stream_retention_days: default_retention_days(),
            attachments: CosAttachmentsConfig::default(),
        }
    }
}

fn default_worker_reserve() -> f64 {
    0.90
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CosHarness {
    #[default]
    ClaudeCode,
    Codex,
    Opencode,
}

impl CosHarness {
    pub fn adapter(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Opencode => "acp",
        }
    }
}

/// ADR 2026-10-05-cos-chat-home D2: task-worker の能力表（`HarnessCapabilities::for_adapter`）を
/// 読んで、この harness がまだ確認していない能力の reason を返す。足りない能力があっても harness や
/// provider を変えない（呼び出し側は警告として扱う）。
fn harness_capability_warnings(harness: CosHarness) -> Vec<String> {
    let Some(caps) = HarnessCapabilities::for_adapter(harness.adapter()) else {
        return Vec::new();
    };
    let mut reasons = Vec::new();
    if !caps.shell {
        reasons.push(capability_reason(MissingCapability::Shell).to_owned());
    }
    if !caps.filesystem {
        reasons.push(capability_reason(MissingCapability::Filesystem).to_owned());
    }
    if !caps.mcp {
        reasons.push(capability_reason(MissingCapability::Mcp).to_owned());
    }
    if !caps.native_image_input && !caps.image_read_tool {
        reasons.push(capability_reason(MissingCapability::Image).to_owned());
    }
    reasons
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CosTriageConfig {
    #[serde(default = "default_policy_skill")]
    pub policy_skill: String,
    #[serde(default = "default_policy_version")]
    pub policy_version: String,
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    #[serde(default = "default_human_required")]
    pub human_required: Vec<String>,
    #[serde(default = "default_unavailable_after_secs")]
    pub unavailable_after_secs: u64,
}

impl Default for CosTriageConfig {
    fn default() -> Self {
        Self {
            policy_skill: default_policy_skill(),
            policy_version: default_policy_version(),
            min_confidence: default_min_confidence(),
            human_required: default_human_required(),
            unavailable_after_secs: default_unavailable_after_secs(),
        }
    }
}

fn default_enabled() -> bool {
    true
}
fn default_max_turns() -> u32 {
    70
}
fn default_max_wall_secs() -> u64 {
    900
}
fn default_policy_skill() -> String {
    "cos-inbox-triage".into()
}
fn default_policy_version() -> String {
    "6".into()
}
fn default_min_confidence() -> f64 {
    0.85
}
/// ADR 2026-10-28-cos-routine-delegated-answers D3: the reasons CoS escalates instead of answering.
/// Routine judgments inside an approved frame are answered with a basis, not listed here.
pub const HUMAN_REQUIRED_REASONS: [&str; 7] = [
    "objective_change",
    "beyond_frame",
    "external_publish",
    "destructive",
    "secret_or_security",
    "standing_permission",
    "explicit_human",
];

fn default_human_required() -> Vec<String> {
    HUMAN_REQUIRED_REASONS
        .into_iter()
        .map(str::to_owned)
        .collect()
}
fn default_unavailable_after_secs() -> u64 {
    120
}

/// `[cos.attachments]`。既定は 1 ファイル 25 MiB、1 メッセージ 100 MiB/10 件、保存量 10 GiB、
/// 未送信 24 時間、最後の参照が外れてから 30 日。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CosAttachmentsConfig {
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: u64,
    #[serde(default = "default_max_files_per_message")]
    pub max_files_per_message: usize,
    #[serde(default = "default_max_storage_bytes")]
    pub max_storage_bytes: u64,
    #[serde(default = "default_orphan_ttl_hours")]
    pub orphan_ttl_hours: u32,
    #[serde(default = "default_retention_days")]
    pub unreferenced_retention_days: u32,
}

impl Default for CosAttachmentsConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: default_max_file_bytes(),
            max_message_bytes: default_max_message_bytes(),
            max_files_per_message: default_max_files_per_message(),
            max_storage_bytes: default_max_storage_bytes(),
            orphan_ttl_hours: default_orphan_ttl_hours(),
            unreferenced_retention_days: default_retention_days(),
        }
    }
}

fn default_retention_days() -> u32 {
    30
}
fn default_max_file_bytes() -> u64 {
    25 * MIB
}
fn default_max_message_bytes() -> u64 {
    100 * MIB
}
fn default_max_files_per_message() -> usize {
    10
}
fn default_max_storage_bytes() -> u64 {
    10 * 1024 * MIB
}
fn default_orphan_ttl_hours() -> u32 {
    24
}

impl CosAttachmentsConfig {
    pub fn limits(&self) -> ChatAttachmentLimits {
        ChatAttachmentLimits {
            max_file_bytes: self.max_file_bytes,
            max_message_bytes: self.max_message_bytes,
            max_files_per_message: self.max_files_per_message,
            max_storage_bytes: self.max_storage_bytes,
            orphan_ttl_hours: i64::from(self.orphan_ttl_hours),
            unreferenced_retention_days: i64::from(self.unreferenced_retention_days),
        }
    }
}

impl CosConfig {
    /// D4: 添付 blob の置き場の親（`<data_dir>/chat/attachments`）。data dir は DB と同じ場所
    /// （DB と一緒に backup する）。
    pub fn attachment_data_dir(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn stream_retention(&self) -> time::Duration {
        time::Duration::days(i64::from(self.stream_retention_days))
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if !self.worker_reserve_five_hour.is_finite()
            || self.worker_reserve_five_hour <= 0.0
            || self.worker_reserve_five_hour > 0.97
        {
            return Err(ConfigError::Invalid(
                "[cos] worker_reserve_five_hour must be > 0 and <= 0.97".into(),
            ));
        }
        if self.max_turns == 0 || self.max_wall_secs == 0 {
            return Err(ConfigError::Invalid(
                "[cos] max_turns and max_wall_secs must be >= 1".into(),
            ));
        }
        if self.triage.policy_skill.trim().is_empty()
            || self.triage.policy_version.trim().is_empty()
        {
            return Err(ConfigError::Invalid(
                "[cos.triage] policy_skill and policy_version must not be empty".into(),
            ));
        }
        if !self.triage.min_confidence.is_finite()
            || !(0.0..=1.0).contains(&self.triage.min_confidence)
        {
            return Err(ConfigError::Invalid(
                "[cos.triage] min_confidence must be between 0 and 1".into(),
            ));
        }
        if self.triage.unavailable_after_secs == 0 {
            return Err(ConfigError::Invalid(
                "[cos.triage] unavailable_after_secs must be >= 1".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for reason in &self.triage.human_required {
            if reason.trim().is_empty() || !seen.insert(reason) {
                return Err(ConfigError::Invalid(
                    "[cos.triage] human_required must contain unique nonempty reasons".into(),
                ));
            }
            if !HUMAN_REQUIRED_REASONS.contains(&reason.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "[cos.triage] human_required: unknown reason {reason:?} (known: {}; \
                     fundamental_change, resource_overrun and security were replaced by \
                     objective_change, beyond_frame and secret_or_security)",
                    HUMAN_REQUIRED_REASONS.join(", ")
                )));
            }
        }
        let a = &self.attachments;
        let invalid = |msg: &str| Err(ConfigError::Invalid(format!("[cos.attachments] {msg}")));
        if self.stream_retention_days == 0 {
            return Err(ConfigError::Invalid(
                "[cos] stream_retention_days must be >= 1".into(),
            ));
        }
        if a.max_file_bytes == 0 || a.max_file_bytes > i64::MAX as u64 {
            return invalid("max_file_bytes must be between 1 and i64::MAX");
        }
        if a.max_message_bytes < a.max_file_bytes || a.max_message_bytes > i64::MAX as u64 {
            return invalid("max_message_bytes must be >= max_file_bytes and <= i64::MAX");
        }
        if a.max_storage_bytes < a.max_message_bytes || a.max_storage_bytes > i64::MAX as u64 {
            return invalid("max_storage_bytes must be >= max_message_bytes and <= i64::MAX");
        }
        if a.max_files_per_message == 0 {
            return invalid("max_files_per_message must be >= 1");
        }
        if a.orphan_ttl_hours == 0 {
            return invalid("orphan_ttl_hours must be >= 1");
        }
        if a.unreferenced_retention_days == 0 {
            return invalid("unreferenced_retention_days must be >= 1");
        }
        Ok(())
    }
}

impl Config {
    /// CoS always uses the declared Claude Code harness; source/model/account are selected at dispatch.
    pub(super) fn validate_cos_mapping(&self) -> Result<(), ConfigError> {
        if self.cos.harness != CosHarness::ClaudeCode {
            return Err(ConfigError::Invalid(
                "[cos] harness is fixed to claude_code".into(),
            ));
        }
        Ok(())
    }

    /// D2 harness capability table: non-fatal reasons for abilities the fixed adapter does not confirm.
    pub fn cos_harness_capability_warnings(&self) -> Vec<String> {
        harness_capability_warnings(CosHarness::ClaudeCode)
    }

    /// 添付上限の `[cos.attachments]` を task-core の型で返す。
    pub fn chat_attachment_limits(&self) -> ChatAttachmentLimits {
        self.cos.attachments.limits()
    }
}

#[cfg(test)]
#[path = "cos_tests.rs"]
mod tests;
