//! `[accounts]`（ADR-0024 / ADR-0025）と `[secrets]`（ADR-0030）: アカウントの根ディレクトリと秘密の置き場。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::AccountAdapter;

use super::{Config, ConfigError};

/// `[secrets]`（ADR-0030 D1）: 1 秘密 = 1 ファイル（ファイル名 = id、中身 = 値 1 行）。`dir` を 0700 で作る。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretsConfig {
    /// ADR-0045 D2: 省略時は `~/.config/celeris/secrets`。相対なら設定ファイル基準。`Config::load` が絶対化する。
    #[serde(default = "default_secrets_dir")]
    pub dir: PathBuf,
}

/// ADR-0045 D2: `~/.config/celeris/secrets`（秘密は設定側に置く）。
fn default_secrets_dir() -> PathBuf {
    PathBuf::from("~/.config/celeris/secrets")
}

/// `[accounts]`（ADR-0024 D1、ADR-0025 D1）: `claude_dir` / `codex_dir` の下の 1 ディレクトリが 1 アカウント。
/// どちらか一方だけでもよい（少なくとも一方は必要。`validate` でチェックする）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountsConfig {
    /// 相対なら設定ファイル基準。`Config::load` が絶対化する。`<claude_dir>/<id>/` = `CLAUDE_SECURESTORAGE_CONFIG_DIR`。
    /// ADR-0045 D2 の置き場は `~/.local/celeris/claude-accounts`（`config/celeris.example.toml` と
    /// 移行スクリプトが書く）。**暗黙の既定は入れない**: `None` は「claude のプールを設定していない」
    /// という意味を持っていて、ADR-0024 D2 の検査がそれを見ているため。
    #[serde(default)]
    pub claude_dir: Option<PathBuf>,
    /// ADR-0025 D1: 相対なら設定ファイル基準。`<codex_dir>/<id>/` = `CODEX_HOME`。
    /// ADR-0045 D2 の置き場は `~/.local/celeris/codex-accounts`。`claude_dir` と同じ理由で
    /// 暗黙の既定は入れない（ADR-0025 D1 の検査が `None` を見る）。
    #[serde(default)]
    pub codex_dir: Option<PathBuf>,
    /// ADR 2026-10-06 D2: opencode go の account pool の根。`<opencode_dir>/<id>/opencode/auth.json` が
    /// opencode の data dir の形そのままで、worker には `XDG_DATA_HOME=<opencode_dir>/<id>` を渡す。
    /// 相対なら設定ファイル基準。暗黙の既定は入れない（`None` は「opencode go の pool を使わない」）。
    #[serde(default)]
    pub opencode_dir: Option<PathBuf>,
    /// ADR 2026-10-06 D2: opencode go の利用量 endpoint（`GET`、`Authorization: Bearer`）。試験は偽 HTTP server に向ける。
    #[serde(default = "default_opencode_go_usage_url")]
    pub opencode_go_usage_url: String,
    /// 1 アカウントで同時に走らせる run の上限。
    #[serde(default = "default_max_runs_per_account")]
    pub max_runs_per_account: usize,
    /// D6 の確認に使うモデル（枠はアカウント単位なので最も安いモデルでよい。claude-code の確認にだけ使う）。
    #[serde(default = "default_check_model")]
    pub check_model: String,
}

impl AccountsConfig {
    /// アダプタ → 根ディレクトリ（設定されているものだけ）。
    pub fn roots(&self) -> HashMap<AccountAdapter, PathBuf> {
        let mut roots = HashMap::new();
        if let Some(dir) = &self.claude_dir {
            roots.insert(AccountAdapter::ClaudeCode, dir.clone());
        }
        if let Some(dir) = &self.codex_dir {
            roots.insert(AccountAdapter::Codex, dir.clone());
        }
        if let Some(dir) = &self.opencode_dir {
            roots.insert(AccountAdapter::OpencodeGo, dir.clone());
        }
        roots
    }

    pub fn root_for(&self, adapter: AccountAdapter) -> Option<&PathBuf> {
        match adapter {
            AccountAdapter::ClaudeCode => self.claude_dir.as_ref(),
            AccountAdapter::Codex => self.codex_dir.as_ref(),
            AccountAdapter::OpencodeGo => self.opencode_dir.as_ref(),
        }
    }
}

fn default_opencode_go_usage_url() -> String {
    "https://opencode.ai/zen/go/v1/usage".to_string()
}
fn default_max_runs_per_account() -> usize {
    2
}
fn default_check_model() -> String {
    "haiku".to_string()
}

impl Config {
    /// ADR-0024 D1 / ADR-0025 D1: `[accounts]` の設定された根ディレクトリ（claude_dir・codex_dir）をそれぞれ
    /// 0700 で作る（無ければ）。`[accounts]` が無ければ何もしない。
    pub fn ensure_accounts_dir(&self) -> Result<(), ConfigError> {
        let Some(accounts) = &self.accounts else {
            return Ok(());
        };
        for dir in accounts.roots().values() {
            if dir.exists() {
                continue;
            }
            std::fs::create_dir_all(dir).map_err(|source| ConfigError::Read {
                path: dir.clone(),
                source,
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o700);
                std::fs::set_permissions(dir, perms).map_err(|source| ConfigError::Read {
                    path: dir.clone(),
                    source,
                })?;
            }
        }
        Ok(())
    }

    /// ADR-0030 D1: `[secrets] dir` を 0700 で作る（無ければ）。`[secrets]` が無ければ何もしない。
    pub fn ensure_secrets_dir(&self) -> Result<(), ConfigError> {
        let Some(secrets) = &self.secrets else {
            return Ok(());
        };
        if secrets.dir.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&secrets.dir).map_err(|source| ConfigError::Read {
            path: secrets.dir.clone(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(&secrets.dir, perms).map_err(|source| ConfigError::Read {
                path: secrets.dir.clone(),
                source,
            })?;
        }
        Ok(())
    }
}

impl SecretsConfig {
    /// ADR-0030 D1 / ADR-0045 D2: `[secrets] dir` は `~` を展開し、相対なら設定ファイルのディレクトリ基準。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        self.dir = task_core::expand_home(&self.dir, task_core::home_dir().as_deref());
        if self.dir.is_relative() {
            self.dir = base.join(&self.dir);
        }
    }
}

impl AccountsConfig {
    /// ADR-0045 D2: `[accounts]` の既定は `~/.local/celeris/{claude,codex}-accounts`。
    /// `~` を展開してから、それでも相対なら従来どおり設定ファイルのディレクトリ基準。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        let home = task_core::home_dir();
        for slot in [
            &mut self.claude_dir,
            &mut self.codex_dir,
            &mut self.opencode_dir,
        ] {
            if let Some(dir) = slot {
                let expanded = task_core::expand_home(dir, home.as_deref());
                *slot = Some(if expanded.is_relative() {
                    base.join(expanded)
                } else {
                    expanded
                });
            }
        }
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.claude_dir.is_none() && self.codex_dir.is_none() && self.opencode_dir.is_none() {
            return Err(ConfigError::Invalid(
                "[accounts] requires at least one of claude_dir / codex_dir / opencode_dir".into(),
            ));
        }
        if self.max_runs_per_account == 0 {
            return Err(ConfigError::Invalid(
                "[accounts] max_runs_per_account must be >= 1".into(),
            ));
        }
        Ok(())
    }
}
