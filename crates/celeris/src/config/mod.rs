//! `config.toml`（DESIGN §2, ADR-0005 D7）。相対パス（`db`, `workspace_root`）は設定ファイルのある
//! ディレクトリからの相対と解釈する。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use task_core::{AccountAdapter, OrgKind, valid_org_id};

mod accounts;
mod adapters;
mod api;
mod cluster;
mod db;
mod delegation;
mod dispatch;
mod execution;
mod github;
mod harness;
mod knowledge;
mod org;
mod providers;
mod scratch;
mod selfdeploy;
mod workspace;

pub use accounts::*;
pub use adapters::*;
pub use api::*;
pub use cluster::*;
pub use db::*;
pub use delegation::*;
pub use dispatch::*;
pub use execution::*;
pub use github::*;
pub use harness::*;
pub use knowledge::*;
pub use org::*;
pub use providers::*;
pub use scratch::*;
pub use selfdeploy::*;
pub use workspace::*;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// ADR-0064 D1: 従来どおり `db = "<path>"`（文字列）で書けるほか、`[db]` テーブルで
    /// `path` と一緒に `busy_timeout_ms` 等を書ける（`DbConfig` のカスタム `Deserialize` が両方を
    /// 受け付ける）。
    #[serde(default)]
    pub db: DbConfig,
    #[serde(default = "default_workspace_root")]
    pub workspace_root: PathBuf,
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_lease_grace_secs")]
    pub lease_grace_secs: u64,
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_kill_grace_secs")]
    pub kill_grace_secs: u64,
    #[serde(default = "default_review_timeout_secs")]
    pub review_timeout_secs: u64,
    #[serde(default = "default_error_cooldown_secs")]
    pub error_cooldown_secs: u64,
    /// ADR-0010 D6（P-3）: リトライのバックオフ `min(base·2^(attempts-1), max)` 秒。`base = 0` で無効。
    #[serde(default = "default_retry_backoff_base_secs")]
    pub retry_backoff_base_secs: u64,
    #[serde(default = "default_retry_backoff_max_secs")]
    pub retry_backoff_max_secs: u64,
    /// ADR-0011（P-38）: 同じ試行での連続 requeue の上限。達したら供給側失敗を通常の失敗（attempts 消費）として扱う。0 で requeue しない。
    #[serde(default = "default_max_requeues")]
    pub max_requeues: u32,
    #[serde(default)]
    pub adapters: AdaptersConfig,
    #[serde(default)]
    pub plan: PlanConfig,
    #[serde(default)]
    pub reviewer: ReviewerConfig,
    /// ADR-0054 D2（Phase 113）: `[review]`。Reviewer run 自身のインフラ都合の失敗（`is_error`/
    /// プロセス失敗/resume 拒否/分類できないレート制限文言）を、条件不合格にせず reviewing のまま
    /// やり直す回数の上限。`[reviewer]`（判定 run の adapter/tier）とは別のテーブル。
    #[serde(default)]
    pub review: ReviewConfig,
    /// ADR-0070 D3（Phase 116）: `[dispatch]`。ワーカー run 自身のインフラ都合の失敗
    /// （lease 失効・切替中断・result.json 不在・セッション再開拒否・レート制限・DB busy）を
    /// attempts を消費せず再試行できる回数の上限。
    #[serde(default)]
    pub dispatch: DispatchTomlConfig,
    /// ADR-0072 D18（Phase E1）: `[execution]`。continuation（予算切れ・yield の続き）の可否と上限。
    #[serde(default)]
    pub execution: ExecutionTomlConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// ADR-0017 M1: `providers.d/*.toml`（1 ファイル 1 アカウント）を追加で読み込む glob。末尾は必ず `/*.toml`。
    /// 相対パスは設定ファイル基準。マッチしたファイルはファイル名昇順で `providers` に追記してから `validate()` を通す。
    #[serde(default)]
    pub providers_include: Option<String>,
    /// `providers_include` を解決したディレクトリの絶対パス（`Config::load` が計算。TOML には書かない。
    /// `POST /api/v1/providers` 等が書き込む先）。
    #[serde(skip)]
    pub providers_dir: Option<PathBuf>,
    /// ADR-0018: コマンドを実行するクラスタ。`WorkspaceSpec::Remote{cluster, path}` の `cluster` がここの `id` を指す。
    #[serde(default)]
    pub clusters: Vec<ClusterConfig>,
    /// ADR-0016 D1: 役割ごとの既定と指示文。タスクの値 > 役割の既定 > 全体の既定。
    #[serde(default)]
    pub roles: Vec<RoleConfig>,
    /// ADR-0027 D1: 分野ごとの説明と既定の役割。タスクの値 > 役割の既定 > 分野の既定（`default_role` の役割）> 親の値。
    /// **ADR-0046 D3（Phase 59）以降は `[[harnesses]]` が正**で、ここは旧い設定の互換読み込みと、
    /// `[[harnesses]]` からの射影（`Config::load` が埋める）の置き場になる。
    #[serde(default)]
    pub genres: Vec<GenreConfig>,
    /// ADR-0046 D3（Phase 59）: ハーネス = 実行契約（`[[genres]]` + `[[roles]]` の後継）。
    /// 書けばこちらが正で、`Config::load` が `genres` / `roles` に射影する（既存の経路はそのまま動く）。
    /// 書かなければ旧い `[[genres]]` + `[[roles]]` を決定的に写す（warn を 1 行出す）。
    #[serde(default)]
    pub harnesses: Vec<HarnessConfig>,
    /// Phase 30（ADR-0033 D4 追記）: 対話が常に走る分野。実機の事故（関連研究調査課＝検索ハーネスに
    /// 話しかけたら検索ハーネスが会話しようとして落ちた）を受けて、対話は**ノードの `genre` を使わない**。
    /// 省略時は `conversation_genre_id()` が `task_core::CONVERSATION_GENRE`（`"secretary"`）を返す
    /// （このときは `[[genres]]` に無くても検証しない。`[[genres]]` を使わない最小構成のため）。
    /// **明示したのに `[[genres]]` に無ければ設定エラー**（対話用の分野が無い）。
    #[serde(default)]
    pub conversation: Option<ConversationConfig>,
    /// ADR-0033 D1: 組織図の**種**を書いたファイル（`[[org]]` の並び）。相対パスは設定ファイル基準。
    /// 省略したら種を蒔かない。蒔くのは **DB の `org_nodes` が空のときだけ**で、以後は DB が正
    /// （編集は GUI → API → DB。設定は再読込しない。ADR-0024 の accounts と同じ扱い）。
    #[serde(default)]
    pub org_include: Option<String>,
    /// `org_include` を読んだ結果（`Config::load` が埋める。TOML の `[celeris]` には書かない）。
    #[serde(skip)]
    pub org: Vec<OrgSeedConfig>,
    /// ADR-0016 D2: 実行中の委譲の上限。
    #[serde(default)]
    pub delegation: DelegationConfig,
    /// ADR-0033 D3: 報告の圧縮の閾値（`compress_after` / `compress_after_secs`）。
    #[serde(default)]
    pub reports: crate::reports::ReportsConfig,
    /// ADR-0037（Phase 39）: 人の判断が要るときだけ Discord に知らせる。秘密（webhook URL）が
    /// 無ければ判定はするが何も送らない（エラーにしない）。
    #[serde(default)]
    pub notify: crate::notify::NotifyConfig,
    /// ADR-0024 D1: Claude アカウントのプール。無ければ `account_pool = true` のプロバイダは設定エラー。
    #[serde(default)]
    pub accounts: Option<AccountsConfig>,
    /// ADR-0030 D1: GUI から預かる API キー等の置き場所。無ければこの機能は無効（管理 API は 409）。
    #[serde(default)]
    pub secrets: Option<SecretsConfig>,
    /// ADR-0033 D6: 組織のノードごとの長期記憶の置き場所。無ければ記憶を読まないし書かない。
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
    /// ADR-0040 D4（Phase 47）: ライブ引き継ぎ（`draining` の待ち時間）。
    #[serde(default)]
    pub handoff: HandoffConfig,
    /// ADR-0040 D6（Phase 48）: リリースの置き場所（`GET /releases` と昇格が読む）。
    #[serde(default)]
    pub selfdeploy: SelfdeployConfig,
    /// ADR-0041 D1（Phase 49）: ローカルの作業場所を worktree にするときの設定。
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    /// ADR-0075 D1〜D3（Phase G1）: `[scratch]`。ローカルの scratch pool（owner ごとの `CARGO_TARGET_DIR` と semantic GC）。
    #[serde(default)]
    pub scratch: ScratchConfig,
    /// ADR-0043 D5（Phase 54）: 変更の取り込みで GitHub を使うときの設定（`gh` の場所と merge の方法）。
    #[serde(default)]
    pub github: GithubConfig,
    /// ADR-0043 D3（Phase 56）: コンテナ実行（runtime・既定のイメージ・ビルドの置き場）。
    #[serde(default)]
    pub containers: ContainersConfig,
    // ---- ADR-0047（Phase 61）: 知識ベース。ここから ----
    /// ADR-0047 D1: `[knowledge]`。正本の置き場と、実効 profile が何も言わないときの既定のマウント。
    #[serde(default)]
    pub knowledge: KnowledgeConfig,
    #[serde(default)]
    pub docs_maintenance: crate::doc_gardener::GardenerConfig,
    // ---- ADR-0047（Phase 61）: ここまで ----
    // ---- ADR-0053 D1/D2（Phase 65）: LLM source のローカル OpenAI 互換プロキシ。ここから ----
    /// `[llm_proxy]`。`claude_oauth`/`codex_oauth` の `accounts_dir` は省略時 `[accounts]` から埋める
    /// （`Config::load` が行う。`Config::validate` が「必要なのに埋まらない」を弾く）。
    #[serde(default)]
    pub llm_proxy: llm_proxy::config::LlmProxyConfig,
    // ---- ADR-0053（Phase 65）: ここまで ----
    // ---- ADR-0054 D1（Phase 67）: ノードごとの継続セッション。ここから ----
    /// `[sessions]`。CoS の対話・部門長のレビュー run の継続セッション（`node_sessions`）の逼迫判定。
    #[serde(default)]
    pub sessions: SessionsConfig,
    // ---- ADR-0054 D1（Phase 67）: ここまで ----
    // ---- ADR-0056 D1/D5（Phase 78）: MCP サーバー。ここから ----
    /// `[mcp]`（＋ `[[mcp.listeners]]`）。`[llm_proxy]` と同じ形で `celeris` が読んで起動する
    /// （reload 対象外）。
    #[serde(default)]
    pub mcp: celeris_mcp::config::McpConfig,
    // ---- ADR-0056（Phase 78）: ここまで ----
    /// `Config::load` で読んだファイルの絶対パス（`GET /api/v1/config` の `config_path`。TOML には書かない）。
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

/// ADR-0040 D3（Phase 47）: CLI からの上書き。`verify.sh` が本番の設定をそのまま読ませたまま、
/// DB・待ち受け・作業場所・トークンだけを staging のものに差し替えるために使う。
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub db: Option<PathBuf>,
    pub listen: Option<std::net::SocketAddr>,
    pub workspace_root: Option<PathBuf>,
    pub token_file: Option<PathBuf>,
}

fn default_retry_backoff_base_secs() -> u64 {
    10
}
fn default_retry_backoff_max_secs() -> u64 {
    300
}
fn default_max_requeues() -> u32 {
    5
}

/// ADR-0042 D3: タスクの足回り（worktree・成果物・run のログ）の既定の置き場。
/// Phase 51 までは設定ファイル基準の `workspaces` だった。明示してあればそのまま使う。
fn default_workspace_root() -> PathBuf {
    PathBuf::from("~/.local/celeris/workspaces")
}
fn default_tick_ms() -> u64 {
    2000
}
fn default_max_concurrency() -> usize {
    4
}
fn default_lease_grace_secs() -> u64 {
    60
}
fn default_idle_timeout_secs() -> u64 {
    300
}
fn default_kill_grace_secs() -> u64 {
    10
}
fn default_review_timeout_secs() -> u64 {
    600
}
fn default_error_cooldown_secs() -> u64 {
    300
}

impl Config {
    /// ファイルから読み、相対パスを設定ファイルのディレクトリ基準で絶対化する。
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut cfg: Config = toml::from_str(&text)?;
        let base = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let base = base.canonicalize().unwrap_or(base);
        cfg.source_path = Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
        // ADR-0045 D2 / ADR-0064 D1: `db` の既定は `~/.local/celeris/celeris.sqlite3`。`~` を展開して
        // から、それでも相対なら従来どおり設定ファイルのディレクトリ基準にする。`[db] backup_dir` も
        // 同じ規則（省略時は触らない）。
        cfg.db.path = task_core::expand_home(&cfg.db.path, task_core::home_dir().as_deref());
        if cfg.db.path.is_relative() {
            cfg.db.path = base.join(&cfg.db.path);
        }
        if let Some(backup_dir) = &cfg.db.backup_dir {
            let expanded = task_core::expand_home(backup_dir, task_core::home_dir().as_deref());
            cfg.db.backup_dir = Some(if expanded.is_relative() {
                base.join(&expanded)
            } else {
                expanded
            });
        }
        // ADR-0042 D3: `workspace_root` の既定は `~/.local/celeris/workspaces`。`~` を展開してから、
        // それでも相対なら他のパス設定と同じく設定ファイルのディレクトリ基準にする
        // （`$HOME` が無い環境や `workspace_root = "workspaces"` と書いた既存の設定は従来どおり）。
        cfg.workspace_root =
            task_core::expand_home(&cfg.workspace_root, task_core::home_dir().as_deref());
        if cfg.workspace_root.is_relative() {
            cfg.workspace_root = base.join(&cfg.workspace_root);
        }
        // ADR-0043 D3 / ADR-0042 D3: `[containers] build_dir` の既定は `~/.local/celeris/containers`。
        cfg.containers.build_dir =
            task_core::expand_home(&cfg.containers.build_dir, task_core::home_dir().as_deref());
        if cfg.containers.build_dir.is_relative() {
            cfg.containers.build_dir = base.join(&cfg.containers.build_dir);
        }
        // ADR-0066 D1: `[workspace] build_cache_dir` の既定は `~/.local/celeris/build-cache`。
        cfg.workspace.build_cache_dir = task_core::expand_home(
            &cfg.workspace.build_cache_dir,
            task_core::home_dir().as_deref(),
        );
        if cfg.workspace.build_cache_dir.is_relative() {
            cfg.workspace.build_cache_dir = base.join(&cfg.workspace.build_cache_dir);
        }
        // ADR-0075 D7: `[scratch] dir`（書いたときだけ。既定は `scratch_dir()` が build_cache_dir の親から組む）。
        if let Some(dir) = &cfg.scratch.dir {
            let expanded = task_core::expand_home(dir, task_core::home_dir().as_deref());
            cfg.scratch.dir = Some(if expanded.is_relative() {
                base.join(&expanded)
            } else {
                expanded
            });
        }
        // ADR-0075 D4（Phase G2）: `[scratch.sccache] binary`（書いたときだけ）。
        if let Some(bin) = &cfg.scratch.sccache.binary {
            let expanded = task_core::expand_home(bin, task_core::home_dir().as_deref());
            cfg.scratch.sccache.binary = Some(if expanded.is_relative() {
                base.join(&expanded)
            } else {
                expanded
            });
        }
        if let Some(token_file) = &cfg.api.token_file
            && token_file.is_relative()
        {
            cfg.api.token_file = Some(base.join(token_file));
        }
        for slot in [
            &mut cfg.api.browser_attestation_public_key_file,
            &mut cfg.api.browser_credentiald_control_socket,
        ] {
            if let Some(path) = slot.as_ref()
                && path.is_relative()
            {
                *slot = Some(base.join(path));
            }
        }
        // ADR-0033 D1: 組織図の種。ファイルが無ければ設定エラー（書いたのに読めないのは事故なので黙らない）。
        if let Some(org_include) = &cfg.org_include {
            let path = {
                let p = PathBuf::from(org_include);
                if p.is_relative() { base.join(p) } else { p }
            };
            let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
                path: path.clone(),
                source,
            })?;
            let file: OrgSeedFile = toml::from_str(&text)?;
            cfg.org = file.org;
        }
        if let Some(pattern) = &cfg.providers_include {
            let dir = providers_include_dir(pattern, &base)?;
            cfg.providers.extend(load_provider_files(&dir)?);
            cfg.providers_dir = Some(dir);
        }
        // ADR-0045 D2: `[accounts]` の既定は `~/.local/celeris/{claude,codex}-accounts`。
        // `~` を展開してから、それでも相対なら従来どおり設定ファイルのディレクトリ基準。
        if let Some(accounts) = &mut cfg.accounts {
            let home = task_core::home_dir();
            for slot in [&mut accounts.claude_dir, &mut accounts.codex_dir] {
                if let Some(dir) = slot {
                    let expanded = task_core::expand_home(dir, home.as_deref());
                    *slot = Some(if expanded.is_relative() {
                        base.join(expanded)
                    } else {
                        expanded
                    });
                }
            }
        }
        // ADR-0053 D1（Phase 65）: `[llm_proxy.sources.claude_oauth/codex_oauth] accounts_dir` は
        // 省略時（空パス）に `[accounts] claude_dir` / `codex_dir`（すでに絶対化済み）を写す。
        // 明示されていれば（相対なら設定ファイル基準で絶対化して）そちらを使う。
        let home = task_core::home_dir();
        if let Some(claude) = &mut cfg.llm_proxy.sources.claude_oauth {
            if claude.accounts_dir.as_os_str().is_empty() {
                if let Some(accounts) = &cfg.accounts {
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
        if let Some(codex) = &mut cfg.llm_proxy.sources.codex_oauth {
            if codex.accounts_dir.as_os_str().is_empty() {
                if let Some(accounts) = &cfg.accounts {
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
        // ADR-0030 D1 / ADR-0045 D2: `[secrets] dir` は `~` を展開し、相対なら設定ファイルのディレクトリ基準。
        if let Some(secrets) = &mut cfg.secrets {
            secrets.dir = task_core::expand_home(&secrets.dir, task_core::home_dir().as_deref());
            if secrets.dir.is_relative() {
                secrets.dir = base.join(&secrets.dir);
            }
        }
        // ADR-0033 D6 / ADR-0045 D2: `[memory] dir` も同じ扱い。
        if let Some(memory) = &mut cfg.memory {
            memory.dir = task_core::expand_home(&memory.dir, task_core::home_dir().as_deref());
            if memory.dir.is_relative() {
                memory.dir = base.join(&memory.dir);
            }
        }
        // ADR-0047 D1（Phase 61）: `[knowledge] root` も同じ扱い（既定の `~/.local/share/celeris/knowledge` もここで絶対パスになる）。
        cfg.knowledge.root =
            task_core::expand_home(&cfg.knowledge.root, task_core::home_dir().as_deref());
        if cfg.knowledge.root.is_relative() {
            cfg.knowledge.root = base.join(&cfg.knowledge.root);
        }
        // ADR-0040 D6 / ADR-0045 D2: `[selfdeploy] releases_dir` も同じ扱い
        // （既定の `~/.local/celeris/releases` もここで絶対パスになる）。
        cfg.selfdeploy.releases_dir = task_core::expand_home(
            &cfg.selfdeploy.releases_dir,
            task_core::home_dir().as_deref(),
        );
        if cfg.selfdeploy.releases_dir.is_relative() {
            cfg.selfdeploy.releases_dir = base.join(&cfg.selfdeploy.releases_dir);
        }
        // ADR-0041 D3: `[selfdeploy] repo` は**人のチェックアウト**なので `~` を展開する
        // （既定の `~/workspace/agent-platform` もここで絶対パスになる）。`$HOME` が無い環境や
        // 相対で書かれたときは、他のパス設定と同じく設定ファイルのディレクトリ基準。
        cfg.selfdeploy.repo =
            task_core::expand_home(&cfg.selfdeploy.repo, task_core::home_dir().as_deref());
        if cfg.selfdeploy.repo.is_relative() {
            cfg.selfdeploy.repo = base.join(&cfg.selfdeploy.repo);
        }
        // ADR-0027 D3: `[adapters.paperqa]` のパス設定は、他のパス設定と同じく設定ファイルのディレクトリ基準で
        // 絶対化する。`settings` は `pqa -s` に渡す文字列（拡張子無し）だが、パスの形をしているので同様に扱う。
        if let Some(dir) = &cfg.adapters.paperqa.paper_directory
            && dir.is_relative()
        {
            cfg.adapters.paperqa.paper_directory = Some(base.join(dir));
        }
        if let Some(dir) = &cfg.adapters.paperqa.index_directory
            && dir.is_relative()
        {
            cfg.adapters.paperqa.index_directory = Some(base.join(dir));
        }
        if let Some(settings) = &cfg.adapters.paperqa.settings
            && Path::new(settings).is_relative()
        {
            cfg.adapters.paperqa.settings =
                Some(base.join(settings).to_string_lossy().into_owned());
        }
        // ADR-0027 D3: 行ごとの `settings` の上書きも同じ基準で絶対化する。
        for p in &mut cfg.providers {
            if let Some(settings) = &p.settings
                && Path::new(settings).is_relative()
            {
                p.settings = Some(base.join(settings).to_string_lossy().into_owned());
            }
        }
        // ADR-0046 D3（Phase 59）: `[[harnesses]]` があれば `genres` / `roles` に射影してから検証する
        // （既存の経路は `genre` / `role` のまま動く）。無ければ旧い形のまま検証し、warn を 1 行出す。
        if cfg.harnesses.is_empty() {
            if !cfg.genres.is_empty() || !cfg.roles.is_empty() {
                tracing::warn!(
                    genres = cfg.genres.len(),
                    roles = cfg.roles.len(),
                    "config: [[genres]] + [[roles]] は ADR-0046 D3 で [[harnesses]] に置き換わった。                     互換で読み込んだ。`celerisctl config to-harnesses --config <this file>` で新しい形を書き出せる"
                );
            }
        } else {
            cfg.project_harnesses();
        }
        cfg.validate()?;
        // API を有効にするなら、トークンが読めることを起動時に確かめる（exit 2）。
        if cfg.api.listen.is_some() {
            cfg.api.read_token()?;
        }
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.selfdeploy.delivery_projects.is_empty()
            && (self
                .selfdeploy
                .delivery_projects
                .iter()
                .any(|p| p.parse::<task_core::ProjectId>().is_err())
                || self
                    .selfdeploy
                    .releases_dir
                    .file_name()
                    .and_then(|v| v.to_str())
                    != Some("releases"))
        {
            return Err(ConfigError::Invalid("delivery_projects requires project IDs and the standard <state>/releases directory".into()));
        }
        // ADR-0051 Phase 106追記: 空のリモート名でpushしようとして分かりにくいgitエラーになるのを防ぐ。
        if self.selfdeploy.push_remote.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[selfdeploy] push_remote must not be blank".into(),
            ));
        }

        if self.max_concurrency == 0 {
            return Err(ConfigError::Invalid("max_concurrency must be >= 1".into()));
        }
        if self.tick_ms == 0 {
            return Err(ConfigError::Invalid("tick_ms must be >= 1".into()));
        }
        // ADR-0043 D5: `gh pr merge` に渡す方法は 3 つだけ。
        if !MERGE_METHODS.contains(&self.github.merge_method.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "[github] merge_method must be one of {MERGE_METHODS:?} (got {:?})",
                self.github.merge_method
            )));
        }
        if self.github.gh.trim().is_empty() {
            return Err(ConfigError::Invalid("[github] gh must not be blank".into()));
        }
        // ADR-0047 D2（Phase 61）: `[knowledge] default_mounts` の綴り（間違いで黙って無視しない）。
        if let Err(why) = self.knowledge.mounts() {
            return Err(ConfigError::Invalid(format!(
                "[knowledge] default_mounts: {why}"
            )));
        }
        // ADR-0072 D13（Phase E3）: gate は 3 つだけ（綴り間違いで黙って shadow/off に倒れないように）。
        if task_core::GateMode::parse(&self.execution.gate).is_none() {
            return Err(ConfigError::Invalid(format!(
                "[execution] gate must be one of [\"off\", \"shadow\", \"on\"] (got {:?})",
                self.execution.gate
            )));
        }
        // ADR-0074 D5.2（Phase F1）: work_unit_lane_cap は 2 つだけ。
        if task_core::WorkUnitLaneCap::parse(&self.execution.work_unit_lane_cap).is_none() {
            return Err(ConfigError::Invalid(format!(
                "[execution] work_unit_lane_cap must be one of [\"task\", \"none\"] (got {:?})",
                self.execution.work_unit_lane_cap
            )));
        }
        // ADR-0074 §4（Phase F2b）: max_parallel_work_units は 1..=6。
        if !(1..=task_dispatch::dispatcher::MAX_PARALLEL_WORK_UNITS_CAP)
            .contains(&self.execution.max_parallel_work_units)
        {
            return Err(ConfigError::Invalid(format!(
                "[execution] max_parallel_work_units must be between 1 and {} (got {})",
                task_dispatch::dispatcher::MAX_PARALLEL_WORK_UNITS_CAP,
                self.execution.max_parallel_work_units
            )));
        }
        // ADR-0079 D3（Phase R1a）: `[execution.tree]` の範囲（max_depth は task の層数で 1..=3）。
        if let Err(why) = self.execution.tree.validate() {
            return Err(ConfigError::Invalid(format!("[execution.tree] {why}")));
        }
        // ADR-0043 D3: runtime は 3 つだけ（綴り間違いで黙ってホスト実行に倒れないように）。
        if task_worker::RuntimePreference::parse(&self.containers.runtime).is_none() {
            return Err(ConfigError::Invalid(format!(
                "[containers] runtime must be one of [\"auto\", \"podman\", \"docker\"] (got {:?})",
                self.containers.runtime
            )));
        }
        if self.containers.image_default.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[containers] image_default must not be blank".into(),
            ));
        }
        if self.containers.build_timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "[containers] build_timeout_secs must be >= 1".into(),
            ));
        }
        if self.providers.is_empty() {
            return Err(ConfigError::Invalid(
                "at least one [[providers]] entry is required".into(),
            ));
        }
        let mut seen_ids = std::collections::HashSet::new();
        for p in &self.providers {
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
            if p.adapter != task_worker::AcpAdapter::ID && (p.command.is_some() || p.args.is_some())
            {
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
                match &self.accounts {
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
        if let Some(accounts) = &self.accounts
            && accounts.claude_dir.is_none()
            && accounts.codex_dir.is_none()
        {
            return Err(ConfigError::Invalid(
                "[accounts] requires at least one of claude_dir / codex_dir".into(),
            ));
        }
        if let Some(accounts) = &self.accounts
            && accounts.max_runs_per_account == 0
        {
            return Err(ConfigError::Invalid(
                "[accounts] max_runs_per_account must be >= 1".into(),
            ));
        }
        // ADR-0010 D9: Reviewer run を満たせるプロバイダが無い設定は、Reviewer 条件のタスクが無音で待ち続ける原因になる。
        // ADR-0069 Phase 118 D4: `[reviewer] tier` が未設定なら lane はタスクごとに動的に決まる
        // （worker run の lane に一致・組織の天井で丸め）ので、特定の 1 tier だけを検査する意味が無い。
        // その場合は「（`adapter` 制約を満たす）プロバイダが 1 つ以上の tier を提供しているか」に緩める。
        let reviewer = &self.reviewer;
        let reviewer_ok = match reviewer.tier {
            Some(explicit) => self.providers.iter().any(|p| {
                p.tiers.contains(&explicit)
                    && reviewer.adapter.as_deref().is_none_or(|a| p.adapter == a)
            }),
            None => self.providers.iter().any(|p| {
                !p.tiers.is_empty() && reviewer.adapter.as_deref().is_none_or(|a| p.adapter == a)
            }),
        };
        if !reviewer_ok {
            let adapter_suffix = reviewer
                .adapter
                .as_deref()
                .map(|a| format!(" with adapter {a:?}"))
                .unwrap_or_default();
            return Err(ConfigError::Invalid(match reviewer.tier {
                Some(t) => format!(
                    "[reviewer] no provider offers tier {t:?}{adapter_suffix} for reviewer runs"
                ),
                None => format!(
                    "[reviewer] no provider offers any tier{adapter_suffix} for reviewer runs"
                ),
            }));
        }
        // ADR-0013 D11: loopback 以外で API をリッスンするならトークンを必須にする。
        if let Some(listen) = self.api.listen
            && !listen.ip().is_loopback()
            && self.api.token_file.is_none()
        {
            return Err(ConfigError::Invalid(format!(
                "[api] listen = {listen} is not a loopback address; token_file is required"
            )));
        }
        // ADR-0018: クラスタの id は重複させない。sync は rsync / none のみ。並列度は 1 以上。
        let mut cluster_ids = std::collections::HashSet::new();
        for c in &self.clusters {
            if c.id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "[[clusters]] id must not be empty".to_string(),
                ));
            }
            if !cluster_ids.insert(&c.id) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate cluster id: {}",
                    c.id
                )));
            }
            if c.host.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: host must not be empty",
                    c.id
                )));
            }
            if !matches!(c.sync.as_str(), "rsync" | "none" | "worktree") {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: sync must be \"worktree\", \"rsync\" or \"none\" (got {:?})",
                    c.id, c.sync
                )));
            }
            // ADR-0032 D1: 認証方式は 3 つだけ。既定は "manual"（celeris は接続を張らない）。
            if !matches!(c.auth.as_str(), "manual" | "publickey" | "totp") {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: auth must be \"manual\", \"publickey\" or \"totp\" (got {:?})",
                    c.id, c.auth
                )));
            }
            // ADR-0060（Phase 103）: master の起こし方も 3 つだけ。既定は "auto"。
            if !matches!(
                c.master_launcher.as_str(),
                "auto" | "systemd-run" | "inline"
            ) {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: master_launcher must be \"auto\", \"systemd-run\" or \"inline\" (got {:?})",
                    c.id, c.master_launcher
                )));
            }
            // ADR-0078 D1: `control_persist` は "yes" か正の秒数だけ（"no"・"0"・空・"10m" は不可）。
            let persist_ok = c.control_persist == "yes"
                || (!c.control_persist.is_empty()
                    && c.control_persist.bytes().all(|b| b.is_ascii_digit())
                    && c.control_persist.parse::<u64>().is_ok_and(|n| n > 0));
            if !persist_ok {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: control_persist must be \"yes\" or a positive number of seconds (got {:?})",
                    c.id, c.control_persist
                )));
            }
            // ADR-0019 D2: 自動削除は実装しない（実行結果を消してしまわないため）。
            if c.remove_worktree_when != "never" {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: remove_worktree_when must be \"never\" (got {:?}); remove the worktree by hand",
                    c.id, c.remove_worktree_when
                )));
            }
            if c.worktree_base.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: worktree_base must not be empty",
                    c.id
                )));
            }
            if c.concurrency == 0 {
                return Err(ConfigError::Invalid(format!(
                    "[[clusters]] {}: concurrency must be >= 1",
                    c.id
                )));
            }
        }
        // ADR-0041 D1: ローカルの worktree のブランチ名は `<接頭辞><task_id>`。接頭辞が空だと
        // タスク id そのものがブランチ名になり、人のブランチと見分けが付かない。
        if self.workspace.worktree_branch_prefix.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[workspace] worktree_branch_prefix must not be empty".to_string(),
            ));
        }
        // ADR-0046 D3（Phase 59）: ハーネスの id は重複させない。adapter は providers と同じ判定。
        let mut harness_ids = std::collections::HashSet::new();
        for h in &self.harnesses {
            if h.id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "[[harnesses]] id must not be empty".to_string(),
                ));
            }
            if !harness_ids.insert(&h.id) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate harness id: {}",
                    h.id
                )));
            }
            if let Some(adapter) = &h.adapter
                && adapter != task_worker::FakeAdapter::ID
                && adapter != task_worker::ClaudeCodeAdapter::ID
                && adapter != task_worker::CodexAdapter::ID
                && adapter != task_worker::AcpAdapter::ID
                && adapter != task_worker::PaperQaAdapter::ID
                && adapter != task_worker::LdrAdapter::ID
                && adapter != task_worker::LangMemAdapter::ID
            {
                return Err(ConfigError::Invalid(format!(
                    "[[harnesses]] {}: adapter {adapter:?} is not available in this build (fake, claude-code, codex, acp, paperqa, local-deep-research, langmem only)",
                    h.id
                )));
            }
            if h.budget.max_turns == Some(0) {
                return Err(ConfigError::Invalid(format!(
                    "[[harnesses]] {}: max_turns must be >= 1",
                    h.id
                )));
            }
            if h.budget.max_wall_secs == Some(0) {
                return Err(ConfigError::Invalid(format!(
                    "[[harnesses]] {}: max_wall_secs must be >= 1",
                    h.id
                )));
            }
        }
        // ADR-0016 D1: 役割の id は重複させない。adapter は providers と同じ判定。上限は 1 以上。
        let mut role_ids = std::collections::HashSet::new();
        for r in &self.roles {
            if r.id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "[[roles]] id must not be empty".to_string(),
                ));
            }
            if !role_ids.insert(&r.id) {
                return Err(ConfigError::Invalid(format!("duplicate role id: {}", r.id)));
            }
            if let Some(adapter) = &r.adapter
                && adapter != task_worker::FakeAdapter::ID
                && adapter != task_worker::ClaudeCodeAdapter::ID
                && adapter != task_worker::CodexAdapter::ID
                && adapter != task_worker::AcpAdapter::ID
                && adapter != task_worker::PaperQaAdapter::ID
                && adapter != task_worker::LdrAdapter::ID
                && adapter != task_worker::LangMemAdapter::ID
            {
                return Err(ConfigError::Invalid(format!(
                    "[[roles]] {}: adapter {adapter:?} is not available in this build (fake, claude-code, codex, acp, paperqa, local-deep-research, langmem only)",
                    r.id
                )));
            }
            if r.max_turns == Some(0) {
                return Err(ConfigError::Invalid(format!(
                    "[[roles]] {}: max_turns must be >= 1",
                    r.id
                )));
            }
            if r.max_wall_secs == Some(0) {
                return Err(ConfigError::Invalid(format!(
                    "[[roles]] {}: max_wall_secs must be >= 1",
                    r.id
                )));
            }
        }
        // ADR-0027 D1: 分野の id は重複させない。`default_role` と `roles` の各要素は `[[roles]]` に存在すること、
        // `default_role`（あれば）は `roles` に含まれること。
        let mut genre_ids = std::collections::HashSet::new();
        for g in &self.genres {
            if g.id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "[[genres]] id must not be empty".to_string(),
                ));
            }
            if !genre_ids.insert(&g.id) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate genre id: {}",
                    g.id
                )));
            }
            for role_id in &g.roles {
                if !role_ids.contains(role_id) {
                    return Err(ConfigError::Invalid(format!(
                        "[[genres]] {}: role {role_id:?} in roles is not defined in [[roles]]",
                        g.id
                    )));
                }
            }
            if let Some(default_role) = &g.default_role {
                if !role_ids.contains(default_role) {
                    return Err(ConfigError::Invalid(format!(
                        "[[genres]] {}: default_role {default_role:?} is not defined in [[roles]]",
                        g.id
                    )));
                }
                if !g.roles.iter().any(|r| r == default_role) {
                    return Err(ConfigError::Invalid(format!(
                        "[[genres]] {}: default_role {default_role:?} must be included in roles",
                        g.id
                    )));
                }
            }
        }
        // Phase 30（ADR-0033 D4 追記）: `[conversation]` を明示したのに、その分野が `[[genres]]` に
        // 無ければ設定エラー（対話用の分野が無い）。省略時の既定（`CONVERSATION_GENRE`）は、
        // `[[genres]]` を使わない最小構成を壊さないよう、ここでは検証しない
        // （`conversation_genre_id()` の呼び出し側が `GenreSpec::find` で見つからなければ既定の
        // 役割で走るだけで、実害は無い）。
        if let Some(conversation) = &self.conversation
            && !genre_ids.contains(&conversation.genre)
        {
            return Err(ConfigError::Invalid(format!(
                "[conversation]: genre {:?} is not defined in [[genres]] (対話用の分野が無い)",
                conversation.genre
            )));
        }
        // ADR-0033 D1: 組織図の種。id は重複させず英小文字ケバブ、`secretary` はちょうど 1 つ、
        // それ以外の親は同じファイル内に居ること、`genre` は `[[genres]]` にあること。
        // 木としての整合（循環・種類の順序）はストアの `org_upsert` が最終的に見る。
        let mut org_ids = std::collections::HashSet::new();
        let mut secretaries = 0usize;
        for node in &self.org {
            if !valid_org_id(&node.id) {
                return Err(ConfigError::Invalid(format!(
                    "[[org]] id {:?} must be lowercase kebab-case",
                    node.id
                )));
            }
            if !org_ids.insert(node.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate org id: {}",
                    node.id
                )));
            }
            if node.name.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "[[org]] {}: name must not be empty",
                    node.id
                )));
            }
            if node.kind == OrgKind::Secretary {
                secretaries += 1;
            }
            if let Some(genre) = &node.genre
                && !genre_ids.contains(genre)
            {
                return Err(ConfigError::Invalid(format!(
                    "[[org]] {}: genre {genre:?} is not defined in [[genres]]",
                    node.id
                )));
            }
        }
        if !self.org.is_empty() && secretaries != 1 {
            return Err(ConfigError::Invalid(format!(
                "[[org]] must contain exactly one node with kind = \"secretary\" (found {secretaries})"
            )));
        }
        for node in &self.org {
            match (&node.parent_id, node.kind) {
                (Some(parent), _) if !org_ids.contains(parent.as_str()) => {
                    return Err(ConfigError::Invalid(format!(
                        "[[org]] {}: parent_id {parent:?} is not one of the [[org]] entries",
                        node.id
                    )));
                }
                (Some(_), OrgKind::Secretary) => {
                    return Err(ConfigError::Invalid(format!(
                        "[[org]] {}: the secretary is the root and must not have a parent_id",
                        node.id
                    )));
                }
                (None, OrgKind::Secretary) => {}
                (None, _) => {
                    return Err(ConfigError::Invalid(format!(
                        "[[org]] {}: parent_id is required (only the secretary is a root)",
                        node.id
                    )));
                }
                _ => {}
            }
            // ADR-0046 D1（Phase 59）: 種の profile も起動時に検証する（知らない道具・知らない
            // ハーネス・skill の綴り）。ハーネスの集合は射影後の `[[genres]]` ＋ 組み込み。
            if let Some(profile) = &node.profile {
                let known = task_core::known_harness_ids(&self.genre_specs());
                task_core::validate_profile(profile, &known).map_err(|e| {
                    ConfigError::Invalid(format!("[[org]] {}: profile: {e}", node.id))
                })?;
            }
        }
        // ADR-0016 D2 / M6: 0 の上限は「委譲を止める」ではなく設定ミス（拒否理由が毎回出るだけ）なので拒否する。
        // ADR-0021 D4: 知らない値は設定エラー（黙って既定に落とさない）。
        if !matches!(
            self.delegation.on_child_failure.as_str(),
            "retry_then_ask" | "ignore"
        ) {
            return Err(ConfigError::Invalid(format!(
                "[delegation] on_child_failure must be \"retry_then_ask\" or \"ignore\" (got {:?})",
                self.delegation.on_child_failure
            )));
        }
        if self.delegation.max_delegate_per_run == 0 {
            return Err(ConfigError::Invalid(
                "[delegation] max_delegate_per_run must be >= 1".to_string(),
            ));
        }
        if self.delegation.max_tree_depth == 0 {
            return Err(ConfigError::Invalid(
                "[delegation] max_tree_depth must be >= 1".to_string(),
            ));
        }
        if self.delegation.max_tree_runs == 0 {
            return Err(ConfigError::Invalid(
                "[delegation] max_tree_runs must be >= 1".to_string(),
            ));
        }
        // Phase 7 監査: cooldown 0 だと供給側失敗の requeue が毎 tick の再 dispatch になる。
        if self.error_cooldown_secs == 0 {
            return Err(ConfigError::Invalid(
                "error_cooldown_secs must be >= 1".into(),
            ));
        }
        // ADR-0010 D7 の前提: 無出力で強制終了された run の結果（SIGKILL までの kill_grace + 次 tick での取り込み）が、
        // 延長後のリース期限より先に処理されること。
        if self.kill_grace_secs * 1000 + self.tick_ms >= self.lease_grace_secs * 1000 / 2 {
            return Err(ConfigError::Invalid(
                "kill_grace_secs + tick_ms must be shorter than lease_grace_secs / 2 (lease renewal safety, ADR-0010 D7)".into(),
            ));
        }
        if self.retry_backoff_max_secs < self.retry_backoff_base_secs {
            return Err(ConfigError::Invalid(
                "retry_backoff_max_secs must be >= retry_backoff_base_secs".into(),
            ));
        }
        // ADR-0053 D1（Phase 65）: `claude_oauth`/`codex_oauth` を有効にしたのに `accounts_dir` が埋まらない
        // （`[accounts] claude_dir`/`codex_dir` が無い）のは設定エラー（黙って空のプールにしない）。
        if let Some(claude) = &self.llm_proxy.sources.claude_oauth
            && claude.enabled
            && claude.accounts_dir.as_os_str().is_empty()
        {
            return Err(ConfigError::Invalid(
                "[llm_proxy.sources.claude_oauth] needs [accounts] claude_dir (or an explicit accounts_dir)".into(),
            ));
        }
        if let Some(codex) = &self.llm_proxy.sources.codex_oauth
            && codex.enabled
            && codex.accounts_dir.as_os_str().is_empty()
        {
            return Err(ConfigError::Invalid(
                "[llm_proxy.sources.codex_oauth] needs [accounts] codex_dir (or an explicit accounts_dir)".into(),
            ));
        }
        {
            let mut seen = std::collections::HashSet::new();
            for s in &self.llm_proxy.sources.openai_compatible {
                if !seen.insert(s.id.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "[llm_proxy.sources.openai_compatible]: duplicate id {:?}",
                        s.id
                    )));
                }
            }
        }
        // ADR-0056 D1（Phase 78）: `auth = "none"` は loopback だけ、`client` は `none` のときだけ。
        self.mcp
            .resolve_listeners()
            .map_err(|e| ConfigError::Invalid(e.to_string()))?;
        Ok(())
    }

    pub fn tick(&self) -> Duration {
        Duration::from_millis(self.tick_ms)
    }

    /// ADR-0040 D4: `[handoff] drain_timeout_secs`。
    pub fn drain_timeout(&self) -> Duration {
        Duration::from_secs(self.handoff.drain_timeout_secs)
    }

    /// ADR-0040 D3: CLI の上書きを設定に重ねる（`Config::load` の**後に**呼ぶ）。相対パスは
    /// `Config::load` と同じく**設定ファイルのディレクトリ基準**で絶対化する（設定に書いた場合と
    /// CLI で渡した場合で同じ場所を指すようにするため）。設定をファイルから読んでいないときは
    /// カレントディレクトリ基準になる。
    pub fn apply_overrides(&mut self, overrides: &Overrides) {
        let base = self
            .source_path
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let resolve = |p: &PathBuf| -> PathBuf {
            if p.is_relative() {
                base.join(p)
            } else {
                p.clone()
            }
        };
        if let Some(db) = &overrides.db {
            self.db.path = resolve(db);
        }
        if let Some(listen) = overrides.listen {
            self.api.listen = Some(listen);
        }
        if let Some(root) = &overrides.workspace_root {
            self.workspace_root = resolve(root);
        }
        if let Some(token_file) = &overrides.token_file {
            self.api.token_file = Some(resolve(token_file));
        }
    }
}

#[cfg(test)]
mod tests;
