use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::Tier;
use task_dispatch::ProviderSpec;

use super::{Config, ConfigError};

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
