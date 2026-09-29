use std::path::PathBuf;

use serde::Deserialize;

/// `[workspace]`（ADR-0041 D1 / ADR-0043 D2）: 案件のリポジトリが `kind = local` の git リポジトリで
/// `mode = "worktree"`（既定）のとき、celeris はタスクごと・リポジトリごとに `git worktree` を切る。
/// そのブランチ名の接頭辞の既定は **`celeris/`**（ADR-0042 D3 で旧名から改めた。ADR-0045 D1 の全面改名で、
/// クラスタ側〈ADR-0019 の `WorktreeSettings::branch_prefix`〉も `celeris/` に揃えた）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    #[serde(default = "default_worktree_branch_prefix")]
    pub worktree_branch_prefix: String,
    /// ADR-0066 D1（Phase 110b）: ローカルの git worktree のホスト実行（コンテナ・Remote は対象外）に
    /// `CARGO_TARGET_DIR=<build_cache_dir>/cargo/<repo-key>` を与え、同じリポジトリの worktree 間で
    /// cargo のビルドキャッシュを共有する。既定 `true`。
    #[serde(default = "default_shared_build_cache")]
    pub shared_build_cache: bool,
    /// ADR-0066 D1: ビルドキャッシュの置き場所。既定 `~/.local/celeris/build-cache`（ADR-0042 D3 の層）。
    #[serde(default = "default_build_cache_dir")]
    pub build_cache_dir: PathBuf,
    /// ADR-0066 D2（Phase 110b）: 終端（done / failed / cancelled）になってからこの秒数経った作業場所
    /// から、ビルド生成物（`target/` 等）だけを刈る。既定 86400 秒（24 時間）。`0` で無効。
    #[serde(default = "default_prune_after_secs")]
    pub prune_after_secs: u64,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            worktree_branch_prefix: default_worktree_branch_prefix(),
            shared_build_cache: default_shared_build_cache(),
            build_cache_dir: default_build_cache_dir(),
            prune_after_secs: default_prune_after_secs(),
        }
    }
}

fn default_worktree_branch_prefix() -> String {
    task_worker::DEFAULT_BRANCH_PREFIX.to_string()
}

fn default_shared_build_cache() -> bool {
    true
}

/// ADR-0042 D3 / ADR-0066 D1: `~/.local/celeris/build-cache`。
fn default_build_cache_dir() -> PathBuf {
    PathBuf::from("~/.local/celeris/build-cache")
}

fn default_prune_after_secs() -> u64 {
    86400
}

/// `[containers]`（ADR-0043 D3。Phase 56）: リポジトリの `run` が `container` のタスクを
/// どのコンテナ runtime で、どのイメージで走らせるか。
///
/// - `runtime` — `"auto"`（既定。podman を先に試し、駄目なら docker）/ `"podman"` / `"docker"`。
///   起動時に `<runtime> info` を 1 度だけ起こして能力を確かめ、結果を `GET /daemon` とログに出す。
///   どれも使えなければ `run = container` のタスクは dispatch されず `blocked` になる。
/// - `image_default` — `workspace.toml` に `[container] image` も `dockerfile` も無いときのイメージ。
///   既定は `celeris-worker:latest`（`scripts/containers/build-worker.sh` で作る）。
/// - `build_dir` — `[container] dockerfile` からビルドしたイメージの作業場所。既定は
///   `~/.local/celeris/containers`（ADR-0042 D3）。`~` は展開し、相対ならこの設定ファイル基準。
/// - `build_timeout_secs` — 1 回のビルドの上限（既定 1800）。超えたらタスクを `blocked` にして人に聞く。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContainersConfig {
    #[serde(default = "default_container_runtime")]
    pub runtime: String,
    #[serde(default = "default_container_image")]
    pub image_default: String,
    #[serde(default = "default_container_build_dir")]
    pub build_dir: PathBuf,
    #[serde(default = "default_container_build_timeout_secs")]
    pub build_timeout_secs: u64,
}

impl Default for ContainersConfig {
    fn default() -> Self {
        Self {
            runtime: default_container_runtime(),
            image_default: default_container_image(),
            build_dir: default_container_build_dir(),
            build_timeout_secs: default_container_build_timeout_secs(),
        }
    }
}

fn default_container_runtime() -> String {
    "auto".to_string()
}

fn default_container_image() -> String {
    task_worker::container::DEFAULT_IMAGE.to_string()
}

/// ADR-0042 D3: `~/.local/celeris/containers`。
fn default_container_build_dir() -> PathBuf {
    PathBuf::from("~/.local/celeris/containers")
}

fn default_container_build_timeout_secs() -> u64 {
    task_worker::container::DEFAULT_BUILD_TIMEOUT_SECS
}
