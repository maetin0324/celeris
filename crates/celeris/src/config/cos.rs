//! `[cos]`（ADR 2026-10-05-cos-chat-home D2/D4）: CoS worker とチャット保持設定。
//! 未知の鍵は他の節と同じく `deny_unknown_fields` で弾く。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::{LlmSourceRef, Tier};
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
    pub harness: CosHarness,
    #[serde(default)]
    pub llm_source: Option<LlmSourceRef>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_tier")]
    pub tier: Tier,
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
            harness: CosHarness::default(),
            llm_source: None,
            provider: None,
            account_id: None,
            model: None,
            tier: default_tier(),
            max_turns: default_max_turns(),
            max_wall_secs: default_max_wall_secs(),
            triage: CosTriageConfig::default(),
            stream_retention_days: default_retention_days(),
            attachments: CosAttachmentsConfig::default(),
        }
    }
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

/// A candidate is selected deterministically here; runtime quota and account availability remain
/// the worker's responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCosProvider {
    pub provider: String,
    pub harness: CosHarness,
    pub llm_source: LlmSourceRef,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub tier: Tier,
    /// D2 harness capability table (非致命): この harness の adapter がまだ確認していない能力の
    /// 理由。CoS の harness/provider 選択には使わない（警告のみ）。
    pub capability_warnings: Vec<String>,
}

fn default_enabled() -> bool {
    true
}
fn default_tier() -> Tier {
    Tier::Frontier
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
    "1".into()
}
fn default_min_confidence() -> f64 {
    0.85
}
fn default_human_required() -> Vec<String> {
    [
        "fundamental_change",
        "external_publish",
        "destructive",
        "resource_overrun",
        "security",
        "explicit_human",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn default_unavailable_after_secs() -> u64 {
    120
}

/// `[cos.attachments]`。既定は 1 ファイル 25 MiB、1 メッセージ 100 MiB/10 件、保存量 10 GiB、
/// 未送信 24 時間、最後の参照が外れてから 30 日。
#[derive(Debug, Clone, Deserialize)]
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
        if self.max_turns == 0 || self.max_wall_secs == 0 {
            return Err(ConfigError::Invalid(
                "[cos] max_turns and max_wall_secs must be >= 1".into(),
            ));
        }
        if matches!(
            self.llm_source,
            Some(LlmSourceRef::None | LlmSourceRef::Unknown)
        ) {
            return Err(ConfigError::Invalid(
                "[cos] llm_source cannot be none or unknown".into(),
            ));
        }
        for (name, value) in [
            ("provider", &self.provider),
            ("account_id", &self.account_id),
            ("model", &self.model),
        ] {
            if value.as_ref().is_some_and(|v| v.trim().is_empty()) {
                return Err(ConfigError::Invalid(format!(
                    "[cos] {name} must not be empty"
                )));
            }
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
    /// Reject contradictions written explicitly, while allowing a valid configuration to have
    /// no currently usable provider. Runtime availability is reported by `resolve_cos_provider`.
    pub(super) fn validate_cos_mapping(&self) -> Result<(), ConfigError> {
        let cos = &self.cos;
        if let Some(source) = &cos.llm_source {
            let compatible = match cos.harness {
                CosHarness::ClaudeCode => *source == LlmSourceRef::ClaudeOauth,
                CosHarness::Codex => *source == LlmSourceRef::CodexOauth,
                CosHarness::Opencode => matches!(
                    source,
                    LlmSourceRef::Celeris | LlmSourceRef::OpenaiCompatible(_)
                ),
            };
            if !compatible {
                return Err(ConfigError::Invalid(
                    "[cos] llm_source conflicts with harness".into(),
                ));
            }
        }
        let proxy_model = cos.model.as_ref().is_some_and(|model| {
            model.starts_with("celeris/") || model.starts_with("openai/celeris/")
        });
        if proxy_model && cos.harness != CosHarness::Opencode {
            return Err(ConfigError::Invalid(
                "[cos] model conflicts with harness".into(),
            ));
        }
        if proxy_model
            && cos
                .llm_source
                .as_ref()
                .is_some_and(|s| *s != LlmSourceRef::Celeris)
        {
            return Err(ConfigError::Invalid(
                "[cos] model conflicts with llm_source".into(),
            ));
        }
        if let Some(id) = &cos.provider {
            let p = self.providers.iter().find(|p| &p.id == id).ok_or_else(|| {
                ConfigError::Invalid(format!("[cos] provider {id:?} does not exist"))
            })?;
            if p.adapter != cos.harness.adapter() {
                return Err(ConfigError::Invalid(format!(
                    "[cos] provider {id:?} conflicts with harness"
                )));
            }
            if !p.tiers.contains(&cos.tier) {
                return Err(ConfigError::Invalid(format!(
                    "[cos] provider {id:?} does not support tier"
                )));
            }
            if let Some(source) = &cos.llm_source {
                let actual = self.provider_llm_source(id).expect("provider found").source;
                if &actual != source {
                    return Err(ConfigError::Invalid(format!(
                        "[cos] provider {id:?} conflicts with llm_source"
                    )));
                }
            }
            if proxy_model
                && self.provider_llm_source(id).expect("provider found").source
                    != LlmSourceRef::Celeris
            {
                return Err(ConfigError::Invalid(format!(
                    "[cos] provider {id:?} conflicts with model"
                )));
            }
            if let Some(account) = &cos.account_id
                && (!p.account_pool.is_on()
                    || p.account_id.as_ref().is_some_and(|fixed| fixed != account))
            {
                return Err(ConfigError::Invalid(format!(
                    "[cos] provider {id:?} conflicts with account_id"
                )));
            }
        }
        Ok(())
    }

    /// D2 harness capability table: non-fatal reasons for abilities the configured harness's
    /// adapter does not confirm yet (e.g. no native image input, no image-reading tool). This
    /// never changes `[cos] harness`/`provider`; it is informational only.
    pub fn cos_harness_capability_warnings(&self) -> Vec<String> {
        harness_capability_warnings(self.cos.harness)
    }

    /// Pure candidate selection. A missing candidate is an explicit unavailable reason, never
    /// a silent fallback to another harness or source.
    pub fn resolve_cos_provider(&self) -> Result<ResolvedCosProvider, String> {
        let cos = &self.cos;
        let capability_warnings = harness_capability_warnings(cos.harness);
        let with_capability_notes = |reason: String| -> String {
            if capability_warnings.is_empty() {
                reason
            } else {
                format!(
                    "{reason} (harness capability notes: {})",
                    capability_warnings.join("; ")
                )
            }
        };
        if !cos.enabled {
            return Err("CoS is disabled by [cos] enabled=false".into());
        }
        let candidates: Vec<_> = self
            .providers
            .iter()
            .filter(|p| {
                (cos.provider.as_ref().is_none_or(|id| &p.id == id))
                    && p.adapter == cos.harness.adapter()
                    && p.tiers.contains(&cos.tier)
                    && p.tier_models
                        .get(&cos.tier)
                        .is_none_or(|binding| binding.unavailable_reason.is_none())
                    && cos.account_id.as_ref().is_none_or(|id| {
                        p.account_pool.is_on()
                            && p.account_id.as_ref().is_none_or(|fixed| fixed == id)
                    })
                    && self.provider_llm_source(&p.id).is_some_and(|source| {
                        !matches!(source.source, LlmSourceRef::None | LlmSourceRef::Unknown)
                            && (!cos.model.as_ref().is_some_and(|model| {
                                model.starts_with("celeris/")
                                    || model.starts_with("openai/celeris/")
                            }) || source.source == LlmSourceRef::Celeris)
                            && cos
                                .llm_source
                                .as_ref()
                                .is_none_or(|wanted| *wanted == source.source)
                    })
            })
            .collect();
        let Some(p) = candidates.first() else {
            if let Some(id) = &cos.provider
                && let Some(reason) = self
                    .providers
                    .iter()
                    .find(|p| &p.id == id)
                    .and_then(|p| p.tier_models.get(&cos.tier))
                    .and_then(|binding| binding.unavailable_reason.as_ref())
            {
                return Err(with_capability_notes(format!(
                    "CoS unavailable: provider {id:?} tier {:?}: {reason}",
                    cos.tier
                )));
            }
            return Err(with_capability_notes(format!(
                "CoS unavailable: no provider for harness={}, tier={:?}, source={:?}, provider={:?}, account={:?}",
                cos.harness.adapter(),
                cos.tier,
                cos.llm_source,
                cos.provider,
                cos.account_id
            )));
        };
        Ok(ResolvedCosProvider {
            provider: p.id.clone(),
            harness: cos.harness,
            llm_source: self
                .provider_llm_source(&p.id)
                .expect("provider found")
                .source,
            account_id: cos.account_id.clone().or_else(|| p.account_id.clone()),
            model: cos.model.clone().or_else(|| {
                p.tier_models
                    .get(&cos.tier)
                    .and_then(|binding| binding.model_id.clone())
                    .or_else(|| self.effective_model(p).map(str::to_owned))
            }),
            tier: cos.tier,
            capability_warnings,
        })
    }
    /// 添付上限の `[cos.attachments]` を task-core の型で返す。
    pub fn chat_attachment_limits(&self) -> ChatAttachmentLimits {
        self.cos.attachments.limits()
    }
}

#[cfg(test)]
#[path = "cos_tests.rs"]
mod tests;
