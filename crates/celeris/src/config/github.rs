//! `[github]`（ADR-0043 D5）: PR での取り込みに使う `gh` と merge の方法。

use serde::Deserialize;

use super::ConfigError;

/// `[github]`（ADR-0043 D5。Phase 54）: 変更の取り込みを PR でやるときの設定。
///
/// - `gh` — CLI の場所（PATH にあれば `"gh"` のまま）。無ければ PR の経路は 409 になる。
/// - `merge_method` — 「Celeris で merge」が使う方法（`merge` / `squash` / `rebase`）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GithubConfig {
    #[serde(default = "default_gh")]
    pub gh: String,
    #[serde(default = "default_merge_method")]
    pub merge_method: String,
}

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            gh: default_gh(),
            merge_method: default_merge_method(),
        }
    }
}

fn default_gh() -> String {
    "gh".to_string()
}

fn default_merge_method() -> String {
    "merge".to_string()
}

/// `gh pr merge` に渡してよい方法（それ以外は設定エラー）。
pub const MERGE_METHODS: [&str; 3] = ["merge", "squash", "rebase"];

impl GithubConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        // ADR-0043 D5: `gh pr merge` に渡す方法は 3 つだけ。
        if !MERGE_METHODS.contains(&self.merge_method.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "[github] merge_method must be one of {MERGE_METHODS:?} (got {:?})",
                self.merge_method
            )));
        }
        if self.gh.trim().is_empty() {
            return Err(ConfigError::Invalid("[github] gh must not be blank".into()));
        }
        Ok(())
    }
}
