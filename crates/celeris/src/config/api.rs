//! `[api]`（ADR-0013 D3 / D11）: HTTP API の待ち受けとトークン。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::ConfigError;

/// `[api]`（ADR-0013 D3 / D11）: HTTP API 層。`listen` が無ければ API を起動しない（既定）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConfig {
    /// 例: `"127.0.0.1:7700"`。
    #[serde(default)]
    pub listen: Option<std::net::SocketAddr>,
    /// Bearer トークンを書いたファイル（前後の空白は除く）。loopback 以外で `listen` するときは必須。相対パスは設定ファイル基準。
    #[serde(default)]
    pub token_file: Option<PathBuf>,
    /// 追加で許可する `Host` ヘッダの値（`localhost` / `127.0.0.1` / `[::1]` とポート付きの形は常に許可）。
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Browser human attestation verifier (Ed25519 public key, raw or hex).
    #[serde(default)]
    pub browser_attestation_public_key_file: Option<PathBuf>,
    /// Private local credentiald control socket. The daemon PID must be admitted by credentiald.
    #[serde(default)]
    pub browser_credentiald_control_socket: Option<PathBuf>,
    /// Administrator supplied login policies; absent by default.
    #[serde(default)]
    pub browser_site_policies: Vec<task_api::browser::TrustedSitePolicy>,
}

impl ApiConfig {
    /// `token_file` の内容（前後の空白を除く）。読めない・空なら設定エラー。トークンの値はエラー文にもログにも出さない（`docs/gui/api.md` §1.1）。
    pub fn read_token(&self) -> Result<Option<String>, ConfigError> {
        let Some(path) = &self.token_file else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::Invalid(format!(
                "[api] token_file {} cannot be read: {e}",
                path.display()
            ))
        })?;
        let token = text.trim();
        if token.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "[api] token_file {} is empty",
                path.display()
            )));
        }
        Ok(Some(token.to_string()))
    }
}

impl ApiConfig {
    /// `token_file` と browser 系のパスは、相対なら設定ファイルのディレクトリ基準（`~` は展開しない）。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        if let Some(token_file) = &self.token_file
            && token_file.is_relative()
        {
            self.token_file = Some(base.join(token_file));
        }
        for slot in [
            &mut self.browser_attestation_public_key_file,
            &mut self.browser_credentiald_control_socket,
        ] {
            if let Some(path) = slot.as_ref()
                && path.is_relative()
            {
                *slot = Some(base.join(path));
            }
        }
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        // ADR-0013 D11: loopback 以外で API をリッスンするならトークンを必須にする。
        if let Some(listen) = self.listen
            && !listen.ip().is_loopback()
            && self.token_file.is_none()
        {
            return Err(ConfigError::Invalid(format!(
                "[api] listen = {listen} is not a loopback address; token_file is required"
            )));
        }
        Ok(())
    }
}
