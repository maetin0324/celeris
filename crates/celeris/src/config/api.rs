use std::path::PathBuf;

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
