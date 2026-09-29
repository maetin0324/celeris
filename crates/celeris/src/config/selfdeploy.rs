//! `[handoff]`（ADR-0040 D4）と `[selfdeploy]`（ADR-0040 D6 / ADR-0045 D2）: 昇格とリリースの置き場。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::ConfigError;

/// `[handoff]`（ADR-0040 D4）: 昇格のライブ引き継ぎ。`active` が `draining` になったあと、手元の run が
/// 終わるのをここまで待つ。超えたら残りを abort し（リースが切れて新しい active が従来の「リース切れ」の
/// 経路で拾う）、exit 0 する。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffConfig {
    #[serde(default = "default_drain_timeout_secs")]
    pub drain_timeout_secs: u64,
    /// ADR-0070 D4（Phase 116）: 既定 `false`。`drain_timeout_secs` を過ぎても、`draining` の
    /// インスタンスは手元の run が生きている限り待ち続け、`abort_all_runs` を呼ばない（1 回だけ
    /// WARN を出す）。待つ上限は各 run 自身の `max_wall_secs`（+ `lease_grace`）で、それを超えれば
    /// 通常のリース失効の経路（D5）に乗る。`true` にすると従来どおり `drain_timeout_secs` で
    /// 強制的に abort する（人が明示的に強い昇格を選んだときだけ設定する想定。`promote.sh --force`
    /// 相当）。
    #[serde(default)]
    pub drain_force_abort: bool,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self {
            drain_timeout_secs: default_drain_timeout_secs(),
            drain_force_abort: false,
        }
    }
}

fn default_drain_timeout_secs() -> u64 {
    3600
}

/// `[selfdeploy]`（ADR-0040 D6）: `release.sh` が作るリリースの置き場所。`GET /releases` はここの
/// `manifest.json` / `gate.json` / `verify.json` を読むだけで、`POST /releases/{sha12}/promote` は
/// `<releases_dir>/<sha12>/scripts/promote.sh` を起こす。`current` / `previous` の symlink は
/// **`releases_dir` の親**（本番では `~/.local/celeris/current`）にある。
///
/// ADR-0045 D2: 既定は **`~/.local/celeris/releases`**（`~` は `$HOME` で展開する。Phase 57 までは
/// 設定ファイル基準の `releases` だった）。書いてあれば従来どおり、相対なら設定ファイルのディレクトリ基準。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfdeployConfig {
    /// ADR-0051: 自動取り込みを許可する自己改善案件。空なら無効。
    #[serde(default)]
    pub delivery_projects: Vec<String>,
    #[serde(default = "default_releases_dir")]
    pub releases_dir: PathBuf,
    /// ADR-0041 D3: **作業チェックアウト**の場所（`~/workspace/agent-platform`）。
    /// `GET /releases` の `on_main` 判定と、delivery_projects有効時のレビュー済みSHAの取り込みに使う。
    /// `~` は celeris の `$HOME` で展開する。無くても構わない（その場合 `on_main` は `null`）。
    #[serde(default = "default_selfdeploy_repo")]
    pub repo: PathBuf,
    /// ADR-0051 Phase 106追記: `merge_reviewed` が成功した直後、release.sh を起こす前に
    /// `origin` へ push する。push の失敗は release 準備を止めない（本番反映の妨げにしない）。
    #[serde(default = "default_selfdeploy_push")]
    pub push: bool,
    /// ADR-0051 Phase 106追記: push先のリモート名。
    #[serde(default = "default_selfdeploy_push_remote")]
    pub push_remote: String,
}

impl Default for SelfdeployConfig {
    fn default() -> Self {
        Self {
            releases_dir: default_releases_dir(),
            delivery_projects: Vec::new(),
            repo: default_selfdeploy_repo(),
            push: default_selfdeploy_push(),
            push_remote: default_selfdeploy_push_remote(),
        }
    }
}

fn default_selfdeploy_push() -> bool {
    true
}

fn default_selfdeploy_push_remote() -> String {
    "origin".to_string()
}

/// ADR-0045 D2: `~/.local/celeris/releases`。
fn default_releases_dir() -> PathBuf {
    PathBuf::from("~/.local/celeris/releases")
}

fn default_selfdeploy_repo() -> PathBuf {
    PathBuf::from("~/workspace/agent-platform")
}

impl SelfdeployConfig {
    /// ADR-0040 D6 / ADR-0045 D2: `[selfdeploy] releases_dir` は `~` を展開し、相対なら設定ファイルの
    /// ディレクトリ基準（既定の `~/.local/celeris/releases` もここで絶対パスになる）。
    /// ADR-0041 D3: `[selfdeploy] repo` は**人のチェックアウト**なので `~` を展開する
    /// （既定の `~/workspace/agent-platform` もここで絶対パスになる）。`$HOME` が無い環境や
    /// 相対で書かれたときは、他のパス設定と同じく設定ファイルのディレクトリ基準。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        self.releases_dir =
            task_core::expand_home(&self.releases_dir, task_core::home_dir().as_deref());
        if self.releases_dir.is_relative() {
            self.releases_dir = base.join(&self.releases_dir);
        }
        self.repo = task_core::expand_home(&self.repo, task_core::home_dir().as_deref());
        if self.repo.is_relative() {
            self.repo = base.join(&self.repo);
        }
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if !self.delivery_projects.is_empty()
            && (self
                .delivery_projects
                .iter()
                .any(|p| p.parse::<task_core::ProjectId>().is_err())
                || self.releases_dir.file_name().and_then(|v| v.to_str()) != Some("releases"))
        {
            return Err(ConfigError::Invalid("delivery_projects requires project IDs and the standard <state>/releases directory".into()));
        }
        // ADR-0051 Phase 106追記: 空のリモート名でpushしようとして分かりにくいgitエラーになるのを防ぐ。
        if self.push_remote.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[selfdeploy] push_remote must not be blank".into(),
            ));
        }
        Ok(())
    }
}
