//! `[cos]`（ADR 2026-10-05-cos-chat-home D4）: CoS チャットの添付上限と chat_events の保持。
//!
//! ここは store-api が足す鍵だけを持つ。harness 等の `[cos]` の他の鍵は cos-run が同じ構造体に欄を足す。
//! 未知の鍵は他の節と同じく `deny_unknown_fields` で弾く。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::chat::attachments::ChatAttachmentLimits;

use super::{Config, ConfigError};

const MIB: u64 = 1024 * 1024;

/// `[cos]`。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CosConfig {
    /// D4: run 終端後の text/tool 詳細の chat_events を消すまでの日数（既定 30）。
    #[serde(default = "default_retention_days")]
    pub stream_retention_days: u32,
    #[serde(default)]
    pub attachments: CosAttachmentsConfig,
}

impl Default for CosConfig {
    fn default() -> Self {
        Self {
            stream_retention_days: default_retention_days(),
            attachments: CosAttachmentsConfig::default(),
        }
    }
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
    /// 添付上限の `[cos.attachments]` を task-core の型で返す。
    pub fn chat_attachment_limits(&self) -> ChatAttachmentLimits {
        self.cos.attachments.limits()
    }
}

#[cfg(test)]
#[path = "cos_tests.rs"]
mod tests;
