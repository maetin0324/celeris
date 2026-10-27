//! `[llm_proxy]`（ADR-0053 D1/D2。Phase 65）。celeris の `Config` に埋め込まれる設定の型。
//!
//! ここは値の入れ物だけで、判断は持たない（DESIGN 原則 1 と同じ規律をこのクレートにも適用する）。

use std::net::SocketAddr;

use serde::Deserialize;

/// `[llm_proxy]`。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LlmProxyConfig {
    /// 省略時は「どれかの source が設定されていれば有効」（`LlmProxyConfig::effective_enabled` が判定する）。
    #[serde(default)]
    pub enabled: Option<bool>,
    /// loopback のみで bind する（ADR-0053 D1）。既定 `127.0.0.1:18100`。
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// routing shadow の policy で無料の到達可能な source を最優先するか。
    #[serde(default = "default_prefer_free")]
    pub prefer_free: bool,
    /// 到達性 probe（`GET <base_url>/models`）のキャッシュ寿命（秒）。
    #[serde(default = "default_probe_cache_secs")]
    pub probe_cache_secs: u64,
    /// 1 アカウントに同時に流してよいプロキシ要求数（ADR-0024 D3 の `IN_USE_PENALTY` を使うための
    /// `max_runs_per_account` 相当。CLI ワーカーの `[accounts] max_runs_per_account` とは別枠で数える
    /// （`docs/guides/llm-source.md` に明記。値そのものは共有しないが、cooldown・観測値は同じ帳簿を共有する）。
    #[serde(default = "default_max_concurrent_per_account")]
    pub max_concurrent_per_account: usize,
    /// 429/401 で cooldown を付けるときの既定の秒数（観測から更に長い期限が分かればそちらを使う。
    /// `task_dispatch::accounts::cooldown_for_failure` の `fallback_secs`）。
    #[serde(default = "default_cooldown_fallback_secs")]
    pub cooldown_fallback_secs: u64,
    #[serde(default)]
    pub sources: SourcesConfig,
    /// proxy が照合する Bearer を書いた file（ADR 2026-10-27-api-principals D6。管理 API の token とは別）。
    /// 無ければ proxy は認証しない。相対パスは設定ファイル基準。
    #[serde(default)]
    pub token_file: Option<std::path::PathBuf>,
    /// 削除済みの `[llm_proxy.models]`（tier alias）。書かれていれば読み込みエラーにする。
    #[serde(default, rename = "models")]
    pub removed_models: Option<RemovedModels>,
}

impl Default for LlmProxyConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            listen: default_listen(),
            prefer_free: default_prefer_free(),
            probe_cache_secs: default_probe_cache_secs(),
            max_concurrent_per_account: default_max_concurrent_per_account(),
            cooldown_fallback_secs: default_cooldown_fallback_secs(),
            sources: SourcesConfig::default(),
            token_file: None,
            removed_models: None,
        }
    }
}

impl LlmProxyConfig {
    /// `enabled` が明示されていればその値、省略時は「どれかの source がある」かどうか。
    pub fn effective_enabled(&self) -> bool {
        self.enabled.unwrap_or_else(|| {
            self.sources.claude_oauth.is_some()
                || self.sources.codex_oauth.is_some()
                || !self.sources.openai_compatible.is_empty()
        })
    }
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:18100"
        .parse()
        .unwrap_or_else(|e| unreachable!("hardcoded address: {e}"))
}
fn default_prefer_free() -> bool {
    true
}
fn default_probe_cache_secs() -> u64 {
    60
}
fn default_max_concurrent_per_account() -> usize {
    4
}
fn default_cooldown_fallback_secs() -> u64 {
    300
}

/// `[llm_proxy.sources]`。
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourcesConfig {
    /// `[llm_proxy.sources.claude_oauth]`。無ければ `claude-oauth` は無効
    /// （`claude_oauth/<model>` は 422）。
    #[serde(default)]
    pub claude_oauth: Option<ClaudeOauthConfig>,
    /// `[llm_proxy.sources.codex_oauth]`。無ければ `codex-oauth` は無効。
    #[serde(default)]
    pub codex_oauth: Option<CodexOauthConfig>,
    /// `[[llm_proxy.sources.openai_compatible]]`。0 件でもよい（`<id>/<model>` は 422 になる）。
    #[serde(default)]
    pub openai_compatible: Vec<OpenAiCompatibleConfig>,
}

/// `[llm_proxy.sources.claude_oauth]`（ADR-0053 D1-1）。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClaudeOauthConfig {
    /// `[accounts] claude_dir` を再利用する（celeris が `Config::load` でここへ絶対パスを埋める。
    /// 空のままなら `[accounts] claude_dir` が無いということで、`effective_enabled` な状態では
    /// 設定エラーになる。`docs/guides/llm-source.md` 参照）。
    #[serde(default)]
    pub accounts_dir: std::path::PathBuf,
    #[serde(default = "default_claude_base_url")]
    pub base_url: String,
    /// OAuth のトークン更新エンドポイント（テストは偽の上流に向ける）。
    #[serde(default = "default_claude_token_url")]
    pub token_url: String,
    /// Claude Code CLI と同じ client_id（未確認。`docs/guides/llm-source.md` に注記）。
    #[serde(default = "default_claude_client_id")]
    pub client_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_claude_base_url() -> String {
    "https://api.anthropic.com".to_string()
}
fn default_claude_token_url() -> String {
    "https://console.anthropic.com/v1/oauth/token".to_string()
}
fn default_claude_client_id() -> String {
    "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string()
}

/// `[llm_proxy.sources.codex_oauth]`（ADR-0053 D1-2。Phase 65b で D1-2 の要求形を Codex CLI に合わせた）。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CodexOauthConfig {
    /// `[accounts] codex_dir` を再利用する（celeris が `Config::load` で埋める。上と同じ規律）。
    #[serde(default)]
    pub accounts_dir: std::path::PathBuf,
    #[serde(default = "default_codex_responses_url")]
    pub responses_url: String,
    #[serde(default = "default_codex_token_url")]
    pub token_url: String,
    #[serde(default = "default_codex_client_id")]
    pub client_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `User-Agent`（Codex CLI と同じ形 `codex_cli_rs/<version>`）。**バージョン値は未確認**
    /// （`docs/guides/llm-source.md` §2 参照。実機で `codex --version` 等から確認して上書きすること）。
    #[serde(default = "default_codex_user_agent")]
    pub user_agent: String,
    /// 既定では `temperature` / `max_output_tokens` を上流へ送らない（ChatGPT の Codex backend は
    /// Codex CLI が送らないフィールドを拒否することがあるため。ADR-0053 Phase 65b 追記）。
    /// 明示的に `true` にしたときだけ、クライアントの値をそのまま転送する（opt-in）。
    #[serde(default)]
    pub send_sampling_params: bool,
    /// 設定したときだけ `"reasoning": {"effort": <値>, "summary": "auto"}` と
    /// `"include": ["reasoning.encrypted_content"]` を付ける（Codex CLI 相当。既定は付けない）。
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

fn default_codex_responses_url() -> String {
    "https://chatgpt.com/backend-api/codex/responses".to_string()
}
fn default_codex_token_url() -> String {
    "https://auth.openai.com/oauth/token".to_string()
}
fn default_codex_client_id() -> String {
    "app_EMoamEEZ73f0CkXaXp7hrann".to_string()
}
/// **未確認**（`docs/guides/llm-source.md` §2 参照）。Codex CLI (`codex-rs`) が送る形に沿わせた既定値。
fn default_codex_user_agent() -> String {
    "codex_cli_rs/0.45.0".to_string()
}

/// `[[llm_proxy.sources.openai_compatible]]`（ADR-0053 D1-3）。既存の Qwen 等をそのまま中継する。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpenAiCompatibleConfig {
    /// 要求の `<id>/<model>` の source ID（`claude_oauth`・`codex_oauth` は予約）。`x-celeris-source` には
    /// `openai-compatible:<id>` で出る。
    pub id: String,
    pub base_url: String,
    /// `[secrets]` の解決は celeris 側で行い、平文の値をここに渡す（無ければ `Authorization` を付けない）。
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// ADR 2026-10-11 llm-source-harness-selection: `[llm_proxy.models]` の tier alias は削除した。
/// 欄が残っている設定は、新しい書き方を示して読み込みエラーにする（読み替えも警告だけの無視もしない）。
pub const MODELS_REMOVED_MESSAGE: &str = "[llm_proxy.models] (tier alias such as celeris/cheap, claude/<tier>, qwen/cheap) was removed: \
request a concrete <source_id>/<model_id> instead (e.g. model = \"qwen/qwen3.8-27b\", \"claude_oauth/claude-sonnet-5\", \
\"codex_oauth/gpt-6-luna\"); source IDs are claude_oauth, codex_oauth and the id of each [[llm_proxy.sources.openai_compatible]]";

/// `[llm_proxy.models]` が書かれていたことを表す（値は持たない。deserialize は必ず失敗する）。
#[derive(Debug, Clone, PartialEq)]
pub enum RemovedModels {}

impl<'de> Deserialize<'de> for RemovedModels {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(MODELS_REMOVED_MESSAGE))
    }
}

#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;
