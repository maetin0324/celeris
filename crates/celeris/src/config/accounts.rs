use std::collections::HashMap;
use std::path::PathBuf;

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
        roots
    }

    pub fn root_for(&self, adapter: AccountAdapter) -> Option<&PathBuf> {
        match adapter {
            AccountAdapter::ClaudeCode => self.claude_dir.as_ref(),
            AccountAdapter::Codex => self.codex_dir.as_ref(),
        }
    }
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
