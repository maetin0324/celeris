//! `[[providers]]` と `providers_include`（ADR-0012 / ADR-0017 M1）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::{
    AccountAdapter, LlmSourceRef, ProviderKind, ResolvedLlmSource, SourceOrigin, Tier,
};
use task_dispatch::ProviderSpec;

use super::{AccountsConfig, Config, ConfigError};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Omitted by legacy rows; all current executable rows are adapters.
    #[serde(default)]
    pub kind: Option<ProviderKind>,
    /// The source used by this adapter. Omission preserves legacy inference.
    #[serde(default)]
    pub llm_source: Option<LlmSourceRef>,
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
    ///
    /// ADR 2026-10-06 D2: `"opencode-go"` のように pool の adapter 名を文字列で書ける（ACP 行のように
    /// 行の adapter 名が pool の adapter と一致しない行で使う）。
    #[serde(default)]
    pub account_pool: task_core::AccountPoolSetting,
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
            .filter(|p| p.account_pool.is_on())
            .map(|p| p.id.clone())
            .collect()
    }

    /// `account_pool = "<adapter>"` で pool の adapter を明示した行の provider id → adapter
    /// （ADR 2026-10-06 D3。dispatcher が行の adapter 名では引けない pool の解決に使う）。
    pub fn account_pool_adapters(&self) -> std::collections::HashMap<String, AccountAdapter> {
        self.providers
            .iter()
            .filter_map(|p| match p.account_pool {
                task_core::AccountPoolSetting::Adapter(a) => Some((p.id.clone(), a)),
                _ => None,
            })
            .collect()
    }

    pub fn provider_specs(&self) -> Vec<ProviderSpec> {
        self.providers
            .iter()
            .map(|p| ProviderSpec {
                id: p.id.clone(),
                adapter: p.adapter.clone(),
                tiers: if self.is_qwen_acp(p) {
                    vec![Tier::Cheap]
                } else {
                    p.tiers.clone()
                },
                concurrency: p.concurrency,
                model: p.model.clone(),
            })
            .collect()
    }

    /// The source and whether it was written explicitly or inferred from a legacy row.
    pub fn provider_llm_source(&self, id: &str) -> Option<ResolvedLlmSource> {
        let p = self.providers.iter().find(|p| p.id == id)?;
        Some(ResolvedLlmSource {
            source: p
                .llm_source
                .clone()
                .unwrap_or_else(|| self.derive_llm_source(p)),
            origin: if p.llm_source.is_some() {
                SourceOrigin::Explicit
            } else {
                SourceOrigin::Derived
            },
        })
    }

    /// ADR-0132 付記 L1/L4/L7: cheap lane で順位付けより先に試すローカルの行と probe 先（設定順）。
    /// `account_pool = false` で `tiers` に cheap を含み、実効 `llm_source` が `openai_compatible:<id>`
    /// （probe 先はその有効な source。無ければ空 = 確かめられない）か、`celeris` で proxy に有効な
    /// `openai_compatible` source と `[llm_proxy.models.qwen].cheap` があるもの（probe 先は有効な source
    /// 全部）。専用契約のアダプタ（`paperqa` / `local-deep-research` / `langmem`）の行は含めない。
    /// `[execution] cheap_local_first = false` なら空。
    pub fn local_cheap_providers(&self) -> Vec<task_dispatch::LocalProviderSpec> {
        if !self.execution.cheap_local_first {
            return Vec::new();
        }
        let enabled: Vec<&llm_proxy::config::OpenAiCompatibleConfig> = self
            .llm_proxy
            .sources
            .openai_compatible
            .iter()
            .filter(|s| s.enabled)
            .collect();
        let target =
            |s: &llm_proxy::config::OpenAiCompatibleConfig| task_dispatch::LocalHealthTarget {
                base_url: s.base_url.clone(),
                bearer_token: s.api_key.clone(),
            };
        let proxy_cheap_is_local = !enabled.is_empty()
            && self
                .llm_proxy
                .models
                .qwen
                .get(&Tier::Cheap)
                .is_some_and(|m| !m.trim().is_empty());
        self.providers
            .iter()
            .filter(|p| !p.account_pool.is_on() && p.tiers.contains(&Tier::Cheap))
            // 専用契約のアダプタ（ADR-0049。`StaticPolicy` が hint の無い仕事を渡さない）は、それに固定された
            // 仕事の唯一の行であることが多い。前段で不通として外すと従来の経路（ADR-0052 の倒し方）を
            // 塞ぐので、ローカルの行にしない。
            .filter(|p| {
                !matches!(
                    p.adapter.as_str(),
                    "paperqa" | "local-deep-research" | "langmem"
                )
            })
            .filter_map(|p| {
                let health = match self.provider_llm_source(&p.id)?.source {
                    // 直結の行は proxy の `enabled` に依らず動くので、無効な source でも `base_url` は見る。
                    LlmSourceRef::OpenaiCompatible(id) => self
                        .llm_proxy
                        .sources
                        .openai_compatible
                        .iter()
                        .filter(|s| s.id == id)
                        .map(target)
                        .collect(),
                    LlmSourceRef::Celeris if proxy_cheap_is_local => {
                        enabled.iter().map(|s| target(s)).collect()
                    }
                    _ => return None,
                };
                Some(task_dispatch::LocalProviderSpec {
                    provider: p.id.clone(),
                    health,
                })
            })
            .collect()
    }

    pub fn provider_kind(&self, id: &str) -> Option<ProviderKind> {
        self.providers
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.kind.unwrap_or_default())
    }

    fn effective_model<'a>(&'a self, p: &'a ProviderConfig) -> Option<&'a str> {
        if !p.model.is_empty() {
            return Some(&p.model);
        }
        match p.adapter.as_str() {
            "claude-code" => self.adapters.claude_code.model.as_deref(),
            "codex" => self.adapters.codex.model.as_deref(),
            "aider" => self.adapters.aider.model.as_deref(),
            "local-deep-research" => self
                .adapters
                .local_deep_research
                .settings
                .get("llm.model")
                .map(String::as_str),
            "langmem" => self.knowledge.langmem.model.as_deref(),
            _ => None,
        }
    }

    fn effective_env<'a>(&'a self, p: &'a ProviderConfig, key: &str) -> Option<&'a str> {
        p.env
            .get(key)
            .map(String::as_str)
            .or_else(|| match p.adapter.as_str() {
                "acp" => self.adapters.acp.env.get(key).map(String::as_str),
                "paperqa" => self.adapters.paperqa.env.get(key).map(String::as_str),
                "local-deep-research" => self
                    .adapters
                    .local_deep_research
                    .env
                    .get(key)
                    .map(String::as_str),
                "langmem" => self.adapters.langmem.env.get(key).map(String::as_str),
                "aider" => self.adapters.aider.env.get(key).map(String::as_str),
                _ => None,
            })
    }

    fn derive_llm_source(&self, p: &ProviderConfig) -> LlmSourceRef {
        match p.adapter.as_str() {
            "fake" => return LlmSourceRef::None,
            "claude-code" => return LlmSourceRef::ClaudeOauth,
            "codex" => return LlmSourceRef::CodexOauth,
            _ => {}
        }
        let model = self.effective_model(p).unwrap_or_default();
        if p.adapter == "acp"
            && (p.account_pool
                == task_core::AccountPoolSetting::Adapter(AccountAdapter::OpencodeGo)
                || model.starts_with("opencode-go/"))
        {
            return LlmSourceRef::OpencodeGo;
        }
        if is_celeris_model(model) || self.model_from_env_is_celeris(p) {
            return LlmSourceRef::Celeris;
        }
        if p.adapter == "acp"
            && p.id.ends_with("-qwen")
            && self.effective_env(p, "OPENCODE_CONFIG").is_some()
            && is_qwen_model(model)
        {
            return LlmSourceRef::OpenaiCompatible("qwen".into());
        }
        LlmSourceRef::Unknown
    }

    fn model_from_env_is_celeris(&self, p: &ProviderConfig) -> bool {
        ["OPENAI_MODEL", "LITELLM_MODEL", "MODEL"]
            .into_iter()
            .any(|key| self.effective_env(p, key).is_some_and(is_celeris_model))
    }

    fn is_qwen_acp(&self, p: &ProviderConfig) -> bool {
        if p.adapter != "acp" {
            return false;
        }
        let model = self.effective_model(p).unwrap_or_default();
        // A proxy model takes precedence over a legacy opencode configuration path.
        if matches!(&p.llm_source, Some(LlmSourceRef::Celeris))
            || is_celeris_model(model)
            || self.model_from_env_is_celeris(p)
        {
            return false;
        }
        is_qwen_model(model)
            || matches!(&p.llm_source, Some(LlmSourceRef::OpenaiCompatible(id)) if id.to_ascii_lowercase().starts_with("qwen"))
    }

    /// Diagnostic codes are stable and contain no environment or credential values.
    pub fn provider_kind_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for p in &self.providers {
            if p.id.ends_with("-qwen") {
                warnings.push(
                    "deprecated_qwen_provider_id: preserve the id until a coordinated migration"
                        .into(),
                );
            }
            if is_qwen_model(self.effective_model(p).unwrap_or_default()) {
                warnings.push(
                    "direct_qwen_model: use a celeris/<tier> proxy model when migrating".into(),
                );
            }
            if self
                .provider_llm_source(&p.id)
                .is_some_and(|r| r.source == LlmSourceRef::Unknown)
            {
                warnings.push("unknown_llm_source: set llm_source explicitly".into());
            }
            if self.is_qwen_acp(p) && p.tiers.iter().any(|t| *t != Tier::Cheap) {
                warnings.push("qwen_fixed_acp_noncheap_tier: use a separate proxy-backed ACP row for frontier/standard".into());
            }
        }
        warnings
    }
}

fn is_celeris_model(model: &str) -> bool {
    matches!(
        model,
        "celeris/frontier"
            | "celeris/standard"
            | "celeris/cheap"
            | "openai/celeris/frontier"
            | "openai/celeris/standard"
            | "openai/celeris/cheap"
    )
}

fn is_qwen_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.starts_with("qwen") || lower.contains("/qwen")
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
pub(super) fn validate_providers(cfg: &Config) -> Result<(), ConfigError> {
    let providers = &cfg.providers;
    let accounts: Option<&AccountsConfig> = cfg.accounts.as_ref();
    if providers.is_empty() {
        return Err(ConfigError::Invalid(
            "at least one [[providers]] entry is required".into(),
        ));
    }
    let mut seen_ids = std::collections::HashSet::new();
    for p in providers {
        if let Some(source) = &p.llm_source {
            let inferred = cfg.derive_llm_source(p);
            let incompatible = match source {
                LlmSourceRef::ClaudeOauth => {
                    p.adapter != "claude-code"
                        || is_celeris_model(cfg.effective_model(p).unwrap_or_default())
                }
                LlmSourceRef::CodexOauth => {
                    p.adapter != "codex"
                        || is_celeris_model(cfg.effective_model(p).unwrap_or_default())
                }
                // ADR 2026-10-06 D3: opencode_go は ACP（opencode）adapter の行だけ。
                LlmSourceRef::OpencodeGo => p.adapter != "acp",
                LlmSourceRef::None => !matches!(p.adapter.as_str(), "fake" | "browser-specialist"),
                LlmSourceRef::Celeris => inferred != LlmSourceRef::Celeris,
                LlmSourceRef::OpenaiCompatible(_) => {
                    matches!(p.adapter.as_str(), "fake" | "claude-code" | "codex")
                        || (inferred != LlmSourceRef::Unknown && inferred != *source)
                }
                LlmSourceRef::Unknown => true,
            };
            if incompatible {
                return Err(ConfigError::Invalid(format!(
                    "provider {}: llm_source conflicts with adapter or model",
                    p.id
                )));
            }
            if let LlmSourceRef::OpenaiCompatible(id) = source
                && !cfg
                    .llm_proxy
                    .sources
                    .openai_compatible
                    .iter()
                    .any(|s| s.id == *id)
            {
                return Err(ConfigError::Invalid(format!(
                    "provider {}: llm_source references an unknown openai_compatible source",
                    p.id
                )));
            }
        }
        if p.account_id.is_some() && !p.account_pool.is_on() {
            return Err(ConfigError::Invalid(format!(
                "provider {}: account_id requires account_pool",
                p.id
            )));
        }
        // ADR 2026-10-06 D3: tier_models は claude-code / codex と、llm_source が opencode_go の ACP 行で使える。
        let opencode_go_acp = p.adapter == "acp"
            && cfg
                .provider_llm_source(&p.id)
                .is_some_and(|r| r.source == LlmSourceRef::OpencodeGo);
        if !p.tier_models.is_empty()
            && !matches!(p.adapter.as_str(), "claude-code" | "codex")
            && !opencode_go_acp
        {
            return Err(ConfigError::Invalid(format!(
                "provider {}: tier_models supported only for Claude/GPT or opencode_go",
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
            && p.adapter != task_worker::BrowserSpecialistAdapter::ID
            && p.adapter != task_worker::PaperQaAdapter::ID
            && p.adapter != task_worker::LdrAdapter::ID
            && p.adapter != task_worker::LangMemAdapter::ID
        {
            return Err(ConfigError::Invalid(format!(
                "provider {}: adapter {:?} is not available in this build (fake, claude-code, codex, aider, acp, browser-specialist, paperqa, local-deep-research, langmem only)",
                p.id, p.adapter
            )));
        }
        if p.concurrency == 0 {
            return Err(ConfigError::Invalid(format!(
                "provider {}: concurrency must be >= 1",
                p.id
            )));
        }
        // ADR-0026 D2 / ADR-0106: `command`/`args` は ACP と browser-specialist の行で意味を持つ。他のアダプタに書いたら
        // 静かに無視せず設定エラーにする（書いた本人の勘違いを早く見つけるため）。
        if p.adapter != task_worker::AcpAdapter::ID
            && p.adapter != task_worker::BrowserSpecialistAdapter::ID
            && (p.command.is_some() || p.args.is_some())
        {
            return Err(ConfigError::Invalid(format!(
                "provider {}: command/args are only allowed when adapter = \"acp\" or \"browser-specialist\" (ADR-0106)",
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
        if p.account_pool.is_on() {
            let Some(account_adapter) = p.account_pool.pool_adapter(&p.adapter) else {
                return Err(ConfigError::Invalid(format!(
                    "provider {}: account_pool = true requires adapter = \"claude-code\" or \"codex\" (or account_pool = \"opencode-go\" on an acp row)",
                    p.id
                )));
            };
            // opencode-go の pool を使えるのは ACP（opencode）行だけ（ADR 2026-10-06 D3）。
            if account_adapter == AccountAdapter::OpencodeGo && p.adapter != "acp" {
                return Err(ConfigError::Invalid(format!(
                    "provider {}: account_pool = \"opencode-go\" requires adapter = \"acp\"",
                    p.id
                )));
            }
            // 名前付き pool は行の adapter が claude-code / codex なら同名のものだけ。
            if let task_core::AccountPoolSetting::Adapter(named) = p.account_pool
                && let Some(own) = AccountAdapter::parse(&p.adapter)
                && own != named
            {
                return Err(ConfigError::Invalid(format!(
                    "provider {}: account_pool = {:?} conflicts with adapter {:?}",
                    p.id,
                    named.as_str(),
                    p.adapter
                )));
            }
            match accounts {
                Some(accounts) if accounts.root_for(account_adapter).is_some() => {}
                Some(_) => {
                    return Err(ConfigError::Invalid(format!(
                        "provider {}: account_pool = true requires [accounts] {} to be set",
                        p.id,
                        match account_adapter {
                            AccountAdapter::ClaudeCode => "claude_dir",
                            AccountAdapter::Codex => "codex_dir",
                            AccountAdapter::OpencodeGo => "opencode_dir",
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
