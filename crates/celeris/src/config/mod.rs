//! `config.toml`（DESIGN §2, ADR-0005 D7）。相対パス（`db`, `workspace_root`）は設定ファイルのある
//! ディレクトリからの相対と解釈する。
//!
//! 流れはこのファイルで読める: `Config::load` が TOML を読み、subsystem ごとに相対パスを解決し
//! （`org_include`・`providers_include` の取り込みを含む）、`[[harnesses]]` を合流し、`Config::validate`
//! を通す。`validate` は subsystem ごとの検証を決まった順に呼ぶ（最初に見つかった誤りを返す）。
//! CLI の上書き（`apply_overrides`）と煙試験（`apply_verify_smoke`）は `load` の後に呼ぶ。
//!
//! 型・serde の既定・検証・パスの解決は subsystem ごとのファイルに同居する:
//! `db` / `api` / `providers` / `adapters` / `accounts`（＋`[secrets]`）/ `proxy`（`[llm_proxy]`）/
//! `harness`（`[[harnesses]]`・`[[roles]]`・`[[genres]]`）/ `org` / `delegation` / `dispatch`
//! （`[reviewer]`・`[review]`・`[dispatch]`・`[plan]`・`[sessions]`）/ `execution` / `cluster` /
//! `workspace`（＋`[containers]`）/ `scratch` / `github` / `selfdeploy`（＋`[handoff]`）/
//! `knowledge`（＋`[memory]`）。公開型はすべて `crate::config::*` から従来どおり引ける。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

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
mod proxy;
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
    /// Isolated browser runtime is disabled unless its API and provider are configured.
    #[serde(default)]
    pub browser: BrowserRuntimeConfig,
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

/// ADR-0116 D5: `[browser]`。`runtime` で isolated browser の runtime を誰が持つかを切り替える。
/// - `"daemon"`（既定）— 従来どおり daemon が bwrap / sandboxd / Chrome を持つ（same-uid / subuid）。
/// - `"launcher"` — 専用 host user の launcher（ADR-0115）に `launcher_socket` 経由で頼む。
///   `launcher_socket` が無ければ設定読込みで error（fail closed。daemon 経路への黙った fallback はしない）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRuntimeConfig {
    #[serde(default)]
    pub egress: BrowserEgressConfig,
    #[serde(default = "default_browser_runtime")]
    pub runtime: String,
    #[serde(default)]
    pub launcher_socket: Option<PathBuf>,
}

impl Default for BrowserRuntimeConfig {
    fn default() -> Self {
        Self {
            egress: BrowserEgressConfig::default(),
            runtime: default_browser_runtime(),
            launcher_socket: None,
        }
    }
}

fn default_browser_runtime() -> String {
    "daemon".to_string()
}

impl BrowserRuntimeConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        match self.runtime.as_str() {
            "daemon" => Ok(()),
            "launcher" => {
                if self.launcher_socket.is_none() {
                    Err(ConfigError::Invalid(
                        "[browser] launcher_socket is required when runtime = \"launcher\" (ADR-0116 D5)"
                            .into(),
                    ))
                } else {
                    Ok(())
                }
            }
            other => Err(ConfigError::Invalid(format!(
                "[browser] runtime must be one of [\"daemon\", \"launcher\"] (got {other:?})"
            ))),
        }
    }

    /// ADR-0116 D5: `validate` を通した後にだけ呼ぶ想定（`runtime = "launcher"` なら
    /// `launcher_socket` が `Some` であることを前提にする）。
    pub fn runtime_kind(&self) -> task_worker::browser::BrowserRuntimeKind {
        match self.runtime.as_str() {
            "launcher" => task_worker::browser::BrowserRuntimeKind::Launcher {
                socket: self.launcher_socket.clone().unwrap_or_default(),
            },
            _ => task_worker::browser::BrowserRuntimeKind::Daemon,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserEgressConfig {
    #[serde(default)]
    pub resolver: Option<std::net::IpAddr>,
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
        let deprecated = cfg.scratch.deprecated_sections();
        if !deprecated.is_empty() {
            tracing::warn!(sections = %deprecated.join(", "), "deprecated scratch cache settings are ignored");
        }
        let base = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let base = base.canonicalize().unwrap_or(base);
        cfg.source_path = Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));

        // 1. 相対パスの解決と外部ファイルの取り込み（subsystem ごと。順序に意味があるのは
        //    `[llm_proxy]` だけで、絶対化済みの `[accounts]` を写すので `[accounts]` の後に置く）。
        cfg.db.resolve_paths(&base);
        // ADR-0042 D3: `workspace_root` の既定は `~/.local/celeris/workspaces`。`~` を展開してから、
        // それでも相対なら他のパス設定と同じく設定ファイルのディレクトリ基準にする
        // （`$HOME` が無い環境や `workspace_root = "workspaces"` と書いた既存の設定は従来どおり）。
        cfg.workspace_root =
            task_core::expand_home(&cfg.workspace_root, task_core::home_dir().as_deref());
        if cfg.workspace_root.is_relative() {
            cfg.workspace_root = base.join(&cfg.workspace_root);
        }
        cfg.containers.resolve_paths(&base);
        cfg.workspace.resolve_paths(&base);
        cfg.scratch.resolve_paths(&base);
        cfg.api.resolve_paths(&base);
        if let Some(org_include) = &cfg.org_include {
            cfg.org = org::load_org_seed(org_include, &base)?;
        }
        if let Some(pattern) = &cfg.providers_include {
            let dir = providers::providers_include_dir(pattern, &base)?;
            cfg.providers.extend(load_provider_files(&dir)?);
            cfg.providers_dir = Some(dir);
        }
        if let Some(accounts) = &mut cfg.accounts {
            accounts.resolve_paths(&base);
        }
        proxy::resolve_accounts_dirs(&mut cfg.llm_proxy, cfg.accounts.as_ref(), &base);
        if let Some(secrets) = &mut cfg.secrets {
            secrets.resolve_paths(&base);
        }
        if let Some(memory) = &mut cfg.memory {
            memory.resolve_paths(&base);
        }
        cfg.knowledge.resolve_paths(&base);
        cfg.selfdeploy.resolve_paths(&base);
        cfg.adapters.paperqa.resolve_paths(&base);
        providers::resolve_provider_settings(&mut cfg.providers, &base);

        // 2. 旧い `[[genres]]` + `[[roles]]` と `[[harnesses]]` の合流（ADR-0046 D3）。
        cfg.merge_harnesses();

        // 3. 検証。
        cfg.validate()?;
        // API を有効にするなら、トークンが読めることを起動時に確かめる（exit 2）。
        if cfg.api.listen.is_some() {
            cfg.api.read_token()?;
        }
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.selfdeploy.validate()?;
        self.browser.validate()?;
        if self.max_concurrency == 0 {
            return Err(ConfigError::Invalid("max_concurrency must be >= 1".into()));
        }
        if self.tick_ms == 0 {
            return Err(ConfigError::Invalid("tick_ms must be >= 1".into()));
        }
        self.github.validate()?;
        self.knowledge.validate()?;
        self.execution.validate()?;
        self.containers.validate()?;
        providers::validate_providers(&self.providers, self.accounts.as_ref())?;
        if let Some(accounts) = &self.accounts {
            accounts.validate()?;
        }
        self.reviewer.validate(&self.providers)?;
        self.api.validate()?;
        cluster::validate_clusters(&self.clusters)?;
        self.workspace.validate()?;
        harness::validate_harnesses(&self.harnesses)?;
        let role_ids = harness::validate_roles(&self.roles)?;
        let genre_ids = harness::validate_genres(&self.genres, &role_ids)?;
        harness::validate_conversation(self.conversation.as_ref(), &genre_ids)?;
        self.validate_org_seed(&genre_ids)?;
        self.delegation.validate()?;
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
        proxy::validate(&self.llm_proxy)?;
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
