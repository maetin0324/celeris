//! `[llm_proxy]`（ADR-0053 D1/D2、Phase 65）の読み込み後の補完と検証。型は `llm_proxy::config` にある。

use std::path::Path;

use llm_proxy::config::LlmProxyConfig;

use super::{AccountsConfig, ConfigError};

/// ADR-0053 D1（Phase 65）: `[llm_proxy.sources.claude_oauth/codex_oauth] accounts_dir` は
/// 省略時（空パス）に `[accounts] claude_dir` / `codex_dir`（すでに絶対化済み）を写す。
/// 明示されていれば（相対なら設定ファイル基準で絶対化して）そちらを使う。
pub(super) fn resolve_accounts_dirs(
    proxy: &mut LlmProxyConfig,
    accounts: Option<&AccountsConfig>,
    base: &Path,
) {
    let home = task_core::home_dir();
    if let Some(claude) = &mut proxy.sources.claude_oauth {
        if claude.accounts_dir.as_os_str().is_empty() {
            if let Some(accounts) = accounts {
                claude.accounts_dir = accounts.claude_dir.clone().unwrap_or_default();
            }
        } else {
            let expanded = task_core::expand_home(&claude.accounts_dir, home.as_deref());
            claude.accounts_dir = if expanded.is_relative() {
                base.join(expanded)
            } else {
                expanded
            };
        }
    }
    if let Some(codex) = &mut proxy.sources.codex_oauth {
        if codex.accounts_dir.as_os_str().is_empty() {
            if let Some(accounts) = accounts {
                codex.accounts_dir = accounts.codex_dir.clone().unwrap_or_default();
            }
        } else {
            let expanded = task_core::expand_home(&codex.accounts_dir, home.as_deref());
            codex.accounts_dir = if expanded.is_relative() {
                base.join(expanded)
            } else {
                expanded
            };
        }
    }
}

pub(super) fn validate(proxy: &LlmProxyConfig) -> Result<(), ConfigError> {
    // ADR-0053 D1（Phase 65）: `claude_oauth`/`codex_oauth` を有効にしたのに `accounts_dir` が埋まらない
    // （`[accounts] claude_dir`/`codex_dir` が無い）のは設定エラー（黙って空のプールにしない）。
    if let Some(claude) = &proxy.sources.claude_oauth
        && claude.enabled
        && claude.accounts_dir.as_os_str().is_empty()
    {
        return Err(ConfigError::Invalid(
            "[llm_proxy.sources.claude_oauth] needs [accounts] claude_dir (or an explicit accounts_dir)".into(),
        ));
    }
    if let Some(codex) = &proxy.sources.codex_oauth
        && codex.enabled
        && codex.accounts_dir.as_os_str().is_empty()
    {
        return Err(ConfigError::Invalid(
            "[llm_proxy.sources.codex_oauth] needs [accounts] codex_dir (or an explicit accounts_dir)".into(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for s in &proxy.sources.openai_compatible {
        if !seen.insert(s.id.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "[llm_proxy.sources.openai_compatible]: duplicate id {:?}",
                s.id
            )));
        }
    }
    Ok(())
}
