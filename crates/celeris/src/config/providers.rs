//! `[[providers]]` と `providers_include`（ADR-0012 / ADR-0017 M1）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::{AccountAdapter, Tier};
use task_dispatch::ProviderSpec;

use super::{AccountsConfig, Config, ConfigError};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    #[serde(default)]
    pub tier_models: task_core::model_routing::TierModels,
    #[serde(default)]
    pub account_id: Option<String>,
    pub id: String,
    pub adapter: String,
    #[serde(default = "default_tiers")]
    pub tiers: Vec<Tier>,
    #[serde(default = "default_provider_concurrency")]
    pub concurrency: usize,
    /// 空でなければ、このプロバイダの run の `--model` に使う（空なら `[adapters.<種別>].model`。ADR-0012 D1）。
    #[serde(default)]
    pub model: String,
    /// このプロバイダの run にだけ渡す環境変数（旧認証設定も互換性のため保持）。`[adapters.<種別>].env` に重ね、同名キーはこちらが優先
    /// （例: `CLAUDE_CONFIG_DIR`、`CODEX_HOME`。ADR-0012 D1）。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。この行の `env` より優先（優先順は
    /// celeris の環境 < `[adapters.*].env` < `[adapters.*].env_from_secrets` < 行の `env` < 行の `env_from_secrets`）。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// ADR-0024 D2: `true` なら `[accounts]` のプールから残量に基づいてアカウントを選ぶ。`adapter = "claude-code"`
    /// かつ `[accounts]` があるときだけ有効（既定 `false`）。
    #[serde(default)]
    pub account_pool: bool,
    /// ADR-0026 D2: `adapter = "acp"` のときだけ意味を持つ、この行の ACP エージェント実行ファイルの上書き
    /// （省略時は `[adapters.acp].command`）。他のアダプタで指定すると `Config::validate` が設定エラーにする。
    #[serde(default)]
    pub command: Option<String>,
    /// ADR-0026 D2: 上と同じ（引数）。省略時は `[adapters.acp].args`。
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// ADR-0027 D3: `adapter = "paperqa"` のときだけ意味を持つ、この行の PaperQA 設定ファイルの上書き
    /// （`-s <name>`、拡張子は付けない。省略時は `[adapters.paperqa].settings`）。他のアダプタで指定すると
    /// `Config::validate` が設定エラーにする。相対パスは設定ファイルのディレクトリ基準で絶対化する。
    #[serde(default)]
    pub settings: Option<String>,
}

fn default_tiers() -> Vec<Tier> {
    vec![Tier::Frontier, Tier::Standard, Tier::Cheap]
}
fn default_provider_concurrency() -> usize {
    1
}

/// `providers_include` の glob（`<dir>/*.toml` の形だけを受け付ける）からディレクトリを取り出し、
/// 相対なら `base` 基準で絶対化する（ADR-0017 M1）。
pub(super) fn providers_include_dir(pattern: &str, base: &Path) -> Result<PathBuf, ConfigError> {
    let dir_part = pattern.strip_suffix("*.toml").ok_or_else(|| {
        ConfigError::Invalid(format!(
            "providers_include must end with \"*.toml\" (got {pattern:?})"
        ))
    })?;
    let dir_part = dir_part.strip_suffix('/').unwrap_or(dir_part);
    let dir = PathBuf::from(dir_part);
    Ok(if dir.is_relative() {
        base.join(dir)
    } else {
        dir
    })
}

/// `dir` 配下の `*.toml` をファイル名昇順で読み、それぞれを 1 件の `ProviderConfig`（`[[providers]]` の 1 行と同じ形）として
/// 解析する（ADR-0017 M1）。`dir` が無ければ空のまま（アカウントをまだ 1 つも追加していない状態）。
pub fn load_provider_files(dir: &Path) -> Result<Vec<ProviderConfig>, ConfigError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| ConfigError::Read {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    paths.sort();
    let mut providers = Vec::with_capacity(paths.len());
    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let provider: ProviderConfig = toml::from_str(&text)?;
        providers.push(provider);
    }
    Ok(providers)
}

impl Config {
    /// ADR-0024 D1 / ADR-0025 D1: `[accounts]` の下の `account_pool = true` のプロバイダ id（重複なし）。
    pub fn account_pool_providers(&self) -> std::collections::HashSet<String> {
        self.providers
            .iter()
            .filter(|p| p.account_pool)
            .map(|p| p.id.clone())
            .collect()
    }

    pub fn provider_specs(&self) -> Vec<ProviderSpec> {
        self.providers
            .iter()
            .map(|p| ProviderSpec {
                id: p.id.clone(),
                adapter: p.adapter.clone(),
                tiers: p.tiers.clone(),
                concurrency: p.concurrency,
                model: p.model.clone(),
            })
            .collect()
    }
}

/// ADR-0027 D3: 行ごとの `settings` の上書きも `[adapters.paperqa]` と同じ基準（設定ファイルのディレクトリ）で絶対化する。
pub(super) fn resolve_provider_settings(providers: &mut [ProviderConfig], base: &Path) {
    for p in providers {
        if let Some(settings) = &p.settings
            && Path::new(settings).is_relative()
        {
            p.settings = Some(base.join(settings).to_string_lossy().into_owned());
        }
    }
}

/// `[[providers]]` の検証。`account_pool = true` の行は `[accounts]` の根ディレクトリも見る。
pub(super) fn validate_providers(
    providers: &[ProviderConfig],
    accounts: Option<&AccountsConfig>,
) -> Result<(), ConfigError> {
    if providers.is_empty() {
        return Err(ConfigError::Invalid(
            "at least one [[providers]] entry is required".into(),
        ));
    }
    let mut seen_ids = std::collections::HashSet::new();
    for p in providers {
        if p.account_id.is_some() && !p.account_pool {
            return Err(ConfigError::Invalid(format!(
                "provider {}: account_id requires account_pool",
                p.id
            )));
        }
        if !p.tier_models.is_empty() && !matches!(p.adapter.as_str(), "claude-code" | "codex") {
            return Err(ConfigError::Invalid(format!(
                "provider {}: tier_models supported only for Claude/GPT",
                p.id
            )));
        }

        // ADR-0012 D1: アダプタのインスタンスはプロバイダ ID で引くので重複は許さない。
        if !seen_ids.insert(p.id.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "duplicate provider id {:?}",
                p.id
            )));
        }
        if p.adapter != task_worker::FakeAdapter::ID
            && p.adapter != task_worker::ClaudeCodeAdapter::ID
            && p.adapter != task_worker::CodexAdapter::ID
            && p.adapter != task_worker::AiderAdapter::ID
            && p.adapter != task_worker::AcpAdapter::ID
            && p.adapter != task_worker::PaperQaAdapter::ID
            && p.adapter != task_worker::LdrAdapter::ID
            && p.adapter != task_worker::LangMemAdapter::ID
        {
            return Err(ConfigError::Invalid(format!(
                "provider {}: adapter {:?} is not available in this build (fake, claude-code, codex, aider, acp, paperqa, local-deep-research, langmem only)",
                p.id, p.adapter
            )));
        }
        if p.concurrency == 0 {
            return Err(ConfigError::Invalid(format!(
                "provider {}: concurrency must be >= 1",
                p.id
            )));
        }
        // ADR-0026 D2: `command`/`args` は `adapter = "acp"` の行だけで意味を持つ。他のアダプタに書いたら
        // 静かに無視せず設定エラーにする（書いた本人の勘違いを早く見つけるため）。
        if p.adapter != task_worker::AcpAdapter::ID && (p.command.is_some() || p.args.is_some()) {
            return Err(ConfigError::Invalid(format!(
                "provider {}: command/args are only allowed when adapter = \"acp\" (ADR-0026 D2)",
                p.id
            )));
        }
        // ADR-0027 D3: `settings` は `adapter = "paperqa"` の行だけで意味を持つ（acp の `command`/`args` と同じ考え方）。
        if p.adapter != task_worker::PaperQaAdapter::ID && p.settings.is_some() {
            return Err(ConfigError::Invalid(format!(
                "provider {}: settings is only allowed when adapter = \"paperqa\" (ADR-0027 D3)",
                p.id
            )));
        }
        // ADR-0024 D2 / ADR-0025 D1: `account_pool = true` は claude-code か codex だけ、かつ `[accounts]` に
        // そのアダプタの根ディレクトリが設定されている必要がある。
        if p.account_pool {
            let Some(account_adapter) = AccountAdapter::parse(&p.adapter) else {
                return Err(ConfigError::Invalid(format!(
                    "provider {}: account_pool = true requires adapter = \"claude-code\" or \"codex\"",
                    p.id
                )));
            };
            match accounts {
                Some(accounts) if accounts.root_for(account_adapter).is_some() => {}
                Some(_) => {
                    return Err(ConfigError::Invalid(format!(
                        "provider {}: account_pool = true requires [accounts] {} to be set",
                        p.id,
                        match account_adapter {
                            AccountAdapter::ClaudeCode => "claude_dir",
                            AccountAdapter::Codex => "codex_dir",
                        }
                    )));
                }
                None => {
                    return Err(ConfigError::Invalid(format!(
                        "provider {}: account_pool = true requires an [accounts] section",
                        p.id
                    )));
                }
            }
        }
    }
    Ok(())
}
