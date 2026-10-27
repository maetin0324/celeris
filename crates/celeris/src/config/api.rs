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
    /// 名前付きの Bearer token（ADR 2026-10-27-api-principals D1）。loopback 以外で `listen` するときは 1 本以上
    /// 必須。削除済みの `[api] token_file` は `deny_unknown_fields` で読み込みエラーになる。
    #[serde(default)]
    pub tokens: Vec<ApiTokenConfig>,
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

/// `[[api.tokens]]` の 1 本: 主体（`human:<name>` / `operator:<name>`）と token の file。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiTokenConfig {
    pub principal: String,
    /// token を書いた file（前後の空白は除く）。相対パスは設定ファイル基準。
    pub token_file: PathBuf,
}

/// CLI（celerisctl 等）が使う token の主体を選ぶ環境変数（例 `operator:claude-code`）。
pub const CLI_PRINCIPAL_ENV: &str = "CELERIS_API_PRINCIPAL";

impl ApiConfig {
    /// `[[api.tokens]]` を読む。読めない・空・主体の形が違う・主体か token の重複は設定エラー。token の値は
    /// エラー文にもログにも出さない（`docs/api/v1/api.md` §1.1）。
    pub fn read_tokens(&self) -> Result<Vec<task_api::ApiToken>, ConfigError> {
        let mut out: Vec<task_api::ApiToken> = Vec::with_capacity(self.tokens.len());
        for entry in &self.tokens {
            let principal = task_api::Principal::parse_token_principal(&entry.principal)
                .map_err(|e| ConfigError::Invalid(format!("[[api.tokens]] {e}")))?;
            if out.iter().any(|t| t.principal == principal) {
                return Err(ConfigError::Invalid(format!(
                    "[[api.tokens]] principal {} appears twice",
                    entry.principal
                )));
            }
            let token = read_token_file(&entry.token_file, "[[api.tokens]] token_file")?;
            if token.starts_with(task_api::cos::COS_BEARER_PREFIX) {
                return Err(ConfigError::Invalid(format!(
                    "[[api.tokens]] token_file {} uses the reserved CoS run credential prefix",
                    entry.token_file.display()
                )));
            }
            if let Some(other) = out.iter().find(|t| t.token == token) {
                return Err(ConfigError::Invalid(format!(
                    "[[api.tokens]] {} and {} share the same token",
                    other.principal.actor(),
                    entry.principal
                )));
            }
            out.push(task_api::ApiToken::new(principal, token));
        }
        Ok(out)
    }

    /// CLI が API を呼ぶときの token（ADR 2026-10-27-api-principals D6）。`$CELERIS_API_PRINCIPAL` があれば
    /// その主体の token、無ければ最初の `operator:` の token、それも無ければ最初の token。1 本も無ければ `None`。
    pub fn client_token(&self) -> Result<Option<String>, ConfigError> {
        let tokens = self.read_tokens()?;
        if let Ok(wanted) = std::env::var(CLI_PRINCIPAL_ENV) {
            return tokens
                .into_iter()
                .find(|t| t.principal.actor() == wanted)
                .map(|t| Some(t.token))
                .ok_or_else(|| {
                    ConfigError::Invalid(format!(
                        "${CLI_PRINCIPAL_ENV} = {wanted:?} is not in [[api.tokens]]"
                    ))
                });
        }
        let operator = tokens
            .iter()
            .position(|t| matches!(t.principal, task_api::Principal::Operator { .. }));
        Ok(match operator {
            Some(i) => tokens.into_iter().nth(i).map(|t| t.token),
            None => tokens.into_iter().next().map(|t| t.token),
        })
    }
}

/// token の file を読む（前後の空白を除く）。読めない・空なら設定エラー（値は出さない）。
pub(crate) fn read_token_file(path: &Path, what: &str) -> Result<String, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        ConfigError::Invalid(format!("{what} {} cannot be read: {e}", path.display()))
    })?;
    let token = text.trim();
    if token.is_empty() {
        return Err(ConfigError::Invalid(format!(
            "{what} {} is empty",
            path.display()
        )));
    }
    Ok(token.to_string())
}

impl ApiConfig {
    /// token の file と browser 系のパスは、相対なら設定ファイルのディレクトリ基準（`~` は展開しない）。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        for token in &mut self.tokens {
            if token.token_file.is_relative() {
                token.token_file = base.join(&token.token_file);
            }
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
            && self.tokens.is_empty()
        {
            return Err(ConfigError::Invalid(format!(
                "[api] listen = {listen} is not a loopback address; [[api.tokens]] is required"
            )));
        }
        Ok(())
    }
}
