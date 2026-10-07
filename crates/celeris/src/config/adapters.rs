//! `[adapters.*]`: アダプタごとの既定（claude-code・codex・aider・acp・paperqa・local-deep-research・langmem・fake）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptersConfig {
    #[serde(default)]
    pub fake: FakeConfig,
    #[serde(default)]
    pub claude_code: ClaudeCodeAdapterConfig,
    #[serde(default)]
    pub codex: CodexAdapterConfig,
    /// ADR-0061（Phase 104）: `aider` アダプタ（明確で局所的な少数ファイル修正向け）。
    #[serde(default)]
    pub aider: AiderAdapterConfig,
    #[serde(default)]
    pub acp: AcpAdapterConfig,
    #[serde(default)]
    pub paperqa: PaperQaAdapterConfig,
    #[serde(default)]
    pub local_deep_research: LdrAdapterConfig,
    /// ADR-0047 D4（Phase 62）: 知識整理 run を起こす python（venv の python）と無出力タイムアウト。
    /// LLM の接続先（provider/base_url/model/api_key_secret）は `[knowledge.langmem]` の方（`build_adapters`
    /// が両方を合わせて `task_worker::LangMemConfig` を作る）。
    #[serde(default)]
    pub langmem: LangMemAdapterConfig,
}

/// `[adapters.langmem]`（ADR-0047 D4）。フィールドの意味は `task_worker::LangMemConfig` の
/// `command`/`idle_timeout_secs`/`env` と同じ。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LangMemAdapterConfig {
    /// 起動するコマンド（`tools/langmem/.venv/bin/python` のような venv の python）。既定 `"python3"`
    /// （venv を用意していない構成では `langmem` の import に失敗し、`retryable = false` のエラーになる）。
    #[serde(default = "default_langmem_command")]
    pub command: String,
    /// 無出力タイムアウト（秒）。省略時はハーネスの予算（`[[harnesses]] budget.max_wall_secs` 由来）の
    /// まま（このキーは harness の予算を**縮める**方向にしか効かない。`task_worker::LangMemConfig` が
    /// `min` を取る）。
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。`env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
}

impl Default for LangMemAdapterConfig {
    fn default() -> Self {
        Self {
            command: default_langmem_command(),
            idle_timeout_secs: None,
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
        }
    }
}

fn default_langmem_command() -> String {
    "python3".to_string()
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeConfig {
    /// 起動するコマンド（省略時は `FakeAdapter::default_command()`）。
    #[serde(default)]
    pub command: Vec<String>,
    /// 追加の環境変数。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。`env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
}

/// `claude-code` アダプタの設定（ADR-0006 D6）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeCodeAdapterConfig {
    /// 起動するコマンド名／パス。
    #[serde(default = "default_claude_command")]
    pub command: String,
    /// 末尾に追加する引数。
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// `--permission-mode`。celeris は許可プロンプトに応答できないため既定は `bypassPermissions`。
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// `--model`（省略時は claude の既定モデル）。
    #[serde(default)]
    pub model: Option<String>,
    /// 追加の環境変数（例: `CLAUDE_CONFIG_DIR`）。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。`env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D7: subagent の道具を許すか（`"deny"` 既定 | `"allow_cos"` |
    /// `"allow"`）。未知の値・省略は `deny`。`extra_args` では外れない。
    #[serde(default)]
    pub subagents: String,
}

impl Default for ClaudeCodeAdapterConfig {
    fn default() -> Self {
        Self {
            command: default_claude_command(),
            extra_args: Vec::new(),
            permission_mode: default_permission_mode(),
            model: None,
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
            subagents: String::new(),
        }
    }
}

impl ClaudeCodeAdapterConfig {
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D7: `subagents` を決定的に解決する（未知の値は `Deny`）。
    pub fn resolved_subagents(&self) -> task_worker::tool_policy::SubagentPolicy {
        task_worker::tool_policy::SubagentPolicy::parse(&self.subagents)
    }
}

fn default_claude_command() -> String {
    "claude".to_string()
}
fn default_permission_mode() -> String {
    "bypassPermissions".to_string()
}

/// `codex` アダプタの設定（ADR-0008 D4）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexAdapterConfig {
    /// 起動するコマンド名／パス。
    #[serde(default = "default_codex_command")]
    pub command: String,
    /// `exec --json` の後、プロンプトの前に追加する引数（ADR-0008 D3）。
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// `--model`（省略時は codex の既定モデル）。
    #[serde(default)]
    pub model: Option<String>,
    /// 追加の環境変数。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。`env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// ADR-0054 D1（Phase 67）: このインストールの `codex` が `exec resume <id>` サブコマンドを
    /// 受け付けるかの**決定的な**判定（`"exec_resume"` | `"experimental_resume"`。実機のバージョンを
    /// 毎回 probe するのではなく設定で固定する。既定 `"exec_resume"`。`codex exec resume --help` が
    /// 無い古い版では `"experimental_resume"` に変えること。運用手順は `docs/SPEC.md）。
    #[serde(default = "default_codex_resume_mode")]
    pub resume_mode: String,
    /// ADR-0054 Phase 112 D1: `exec resume` で `-c key=value` に翻訳しきれない `extra_args` が残った
    /// ときの扱い（`"dangerous"` | 省略）。既定（省略・未知の値）は落として WARN。`"dangerous"` は
    /// `--dangerously-bypass-approvals-and-sandbox` を使う（意味が広いので明示設定が要る）。
    #[serde(default)]
    pub resume_bypass: String,
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D7: subagent の道具を許すか（`"deny"` 既定 | `"allow_cos"` |
    /// `"allow"`）。未知の値・省略は `deny`。`extra_args` では外れない。
    #[serde(default)]
    pub subagents: String,
}

fn default_codex_resume_mode() -> String {
    "exec_resume".to_string()
}

impl Default for CodexAdapterConfig {
    fn default() -> Self {
        Self {
            command: default_codex_command(),
            extra_args: Vec::new(),
            model: None,
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
            resume_mode: default_codex_resume_mode(),
            resume_bypass: String::new(),
            subagents: String::new(),
        }
    }
}

impl CodexAdapterConfig {
    /// ADR-0054 D1（Phase 67）: `resume_mode` を決定的に解決する。未知の値は `ExecResume`（既定）に
    /// 倒し、起動時の warn は呼び出し側（`dispatch_config`）に任せる（このクレートは task-worker の
    /// tracing 依存を増やしたくないため）。
    pub fn resolved_resume_mode(&self) -> task_worker::CodexResumeMode {
        match self.resume_mode.as_str() {
            "experimental_resume" => task_worker::CodexResumeMode::ExperimentalResume,
            _ => task_worker::CodexResumeMode::ExecResume,
        }
    }

    /// ADR-0054 Phase 112 D1: `resume_bypass` を決定的に解決する。未知の値・省略は `Off`（既定、従来
    /// どおり翻訳できない `extra_args` を落とす）に倒す。
    pub fn resolved_resume_bypass(&self) -> task_worker::CodexResumeBypass {
        match self.resume_bypass.as_str() {
            "dangerous" => task_worker::CodexResumeBypass::Dangerous,
            _ => task_worker::CodexResumeBypass::Off,
        }
    }

    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D7: `subagents` を決定的に解決する（未知の値は `Deny`）。
    pub fn resolved_subagents(&self) -> task_worker::tool_policy::SubagentPolicy {
        task_worker::tool_policy::SubagentPolicy::parse(&self.subagents)
    }
}

fn default_codex_command() -> String {
    "codex".to_string()
}

/// `aider` アダプタの設定（ADR-0061）。`codex`（ADR-0008 D4）と同じ作りで、resume 相当の概念は
/// 無い（aider は 1 回の `--message` で終わる設計。ADR-0061「アダプタ層」）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiderAdapterConfig {
    /// 起動するコマンド名／パス。
    #[serde(default = "default_aider_command")]
    pub command: String,
    /// `--message` の前に追加する引数（例: `["--architect"]`）。
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// `--model`（省略時は aider の既定モデル）。
    #[serde(default)]
    pub model: Option<String>,
    /// 追加の環境変数（`OPENAI_API_KEY`/`ANTHROPIC_API_KEY` 等）。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。`env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
}

impl Default for AiderAdapterConfig {
    fn default() -> Self {
        Self {
            command: default_aider_command(),
            extra_args: Vec::new(),
            model: None,
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
        }
    }
}

fn default_aider_command() -> String {
    "aider".to_string()
}

/// `acp` アダプタの設定（ADR-0026 D2）。最初の実装は `opencode acp`。`command`/`args`/`env`/`model` は
/// `[[providers]]` の行ごとに上書きできる（別の ACP エージェントを同居させるため。行の値は `ProviderConfig`
/// の `command`/`args`/`env`/`model` にある）。ここには `model` は無い（ACP はモデルを CLI フラグではなく
/// `session/set_config_option` で渡すので、行の `model` が空なら `None` になるだけで、この節に既定値を置く
/// 意味が無い。ADR-0026 D3）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpAdapterConfig {
    /// 起動する ACP エージェントの実行ファイル。既定 `"opencode"`。
    #[serde(default = "default_acp_command")]
    pub command: String,
    /// コマンドへの引数。既定 `["acp"]`。
    #[serde(default = "default_acp_args")]
    pub args: Vec<String>,
    /// 追加の環境変数（共通分。行の `env` を重ねる。同名キーは行が優先）。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。`env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// `session/request_permission` への即答。`"allow"`（既定）| `"deny"`。
    #[serde(default = "default_acp_permission")]
    pub permission: task_worker::AcpPermission,
    /// `session/set_config_option` の `configId`（`session/new` の `configOptions[].id` と対にする）。
    /// 既定 `"model"`（opencode 1.18.31 で確認済み。ADR-0026 D3）。
    #[serde(default = "default_acp_model_option_id")]
    pub model_option_id: String,
    /// `initialize` の応答を待つ上限（秒）。初回はエージェント側のプロバイダ取得で数分かかりうる。既定 300。
    #[serde(default = "default_acp_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D7: subagent の道具を許すか（`"deny"` 既定 | `"allow_cos"` |
    /// `"allow"`）。未知の値・省略は `deny`。`extra_args` では外れない。
    #[serde(default)]
    pub subagents: String,
}

impl Default for AcpAdapterConfig {
    fn default() -> Self {
        Self {
            command: default_acp_command(),
            args: default_acp_args(),
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
            permission: default_acp_permission(),
            model_option_id: default_acp_model_option_id(),
            startup_timeout_secs: default_acp_startup_timeout_secs(),
            subagents: String::new(),
        }
    }
}

impl AcpAdapterConfig {
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D7: `subagents` を決定的に解決する（未知の値は `Deny`）。
    pub fn resolved_subagents(&self) -> task_worker::tool_policy::SubagentPolicy {
        task_worker::tool_policy::SubagentPolicy::parse(&self.subagents)
    }
}

fn default_acp_command() -> String {
    "opencode".to_string()
}
fn default_acp_args() -> Vec<String> {
    vec!["acp".to_string()]
}
fn default_acp_permission() -> task_worker::AcpPermission {
    task_worker::AcpPermission::Allow
}
fn default_acp_model_option_id() -> String {
    "model".to_string()
}
fn default_acp_startup_timeout_secs() -> u64 {
    300
}

/// `paperqa` アダプタの設定（ADR-0027 D3）。フィールドの意味は `task_worker::PaperQaConfig`
/// （`crates/task-worker/src/paperqa.rs`）と同じ。`[[providers]] adapter = "paperqa"` の行ごとの
/// 上書きは `model`/`env` だけ（`ProviderConfig` の既存フィールドを再利用。ADR-0026 D2 と同じ作り）。
/// `settings`/`paper_directory`/`index_directory` は行では上書きしない
/// （調査タスクごとの `[[genres]]`/`[[roles]]` で使い分ける前提。必要になれば別 ADR で足す）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaperQaAdapterConfig {
    /// ADR-0063 Phase 109d C2: 起動するコマンド。**`pqa` CLI ではなく python インタプリタ**
    /// （`paperqa` パッケージが入った venv の `bin/python`）。既定 `"python"`。旧 `pqa` を指していると
    /// 自動で同じディレクトリの `python` に置き換えて 1 回警告する。
    #[serde(default = "default_paperqa_command")]
    pub command: String,
    /// 設定ファイルへのフルパス（拡張子は付けても付けなくてもよい。実機の仕様。ADR-0027 D3）。
    /// `paperqa_ask.py` にはこのファイル自体（`settings_path`）を渡して**直接読ませる**
    /// （`Settings.from_name` は `PQA_SETTINGS_DIR` を見ないため使えない。本番で観測、
    /// ADR-0063 Phase 109e）。名前だけ（ディレクトリ無し）を渡した場合だけ
    /// `Settings.from_name` にフォールバックする。
    #[serde(default)]
    pub settings: Option<String>,
    /// `--agent.index.paper_directory`。相対パスは設定ファイルのディレクトリ基準で絶対化する。
    #[serde(default)]
    pub paper_directory: Option<PathBuf>,
    /// `--agent.index.index_directory` の親ディレクトリ（タスクごとのサブディレクトリはアダプタが足す）。
    /// 相対パスは設定ファイルのディレクトリ基準で絶対化する。
    #[serde(default)]
    pub index_directory: Option<PathBuf>,
    /// `--agent.index.name`。未指定ならタスク ID を使う。
    #[serde(default)]
    pub index_name: Option<String>,
    /// 末尾に追加する引数（`ask` の前に挿入する）。
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// 追加の環境変数（例: `OPENAI_API_KEY` / `OPENAI_BASE_URL`。LiteLLM 経由の OpenAI 互換エンドポイント向け）。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。`env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// `[adapters.paperqa.acquire]`（ADR-0035 D1）: 文献の取得（arXiv / OpenAlex、鍵無し）。
    /// 意味は `task_worker::AcquireConfig` と同じ。`max_candidates = 0` で取得の段を行わない。
    #[serde(default)]
    pub acquire: task_worker::AcquireConfig,
    /// `[adapters.paperqa.evidence]`（ADR-0035 D3）: 決定的な証拠ゲートの閾値。
    /// 意味は `task_worker::PaperQaEvidence` と同じ。
    #[serde(default)]
    pub evidence: task_worker::PaperQaEvidence,
    /// ADR-0063 Phase 109d C3: 対象ごとの問い + 総括の問いの上限（`ask()` を呼ぶ回数の上限）。既定
    /// 10（Phase 109h: 8 から引き上げ）。比較先があるときは総括の問いを必ず残し、対象側を後ろから
    /// 詰める（`task_worker::paperqa::build_questions_for_targets` 相当、`paperqa_ask.py`）。
    #[serde(default = "default_paperqa_max_asks")]
    pub max_asks: u32,
}

impl Default for PaperQaAdapterConfig {
    fn default() -> Self {
        Self {
            command: default_paperqa_command(),
            settings: None,
            paper_directory: None,
            index_directory: None,
            index_name: None,
            extra_args: Vec::new(),
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
            acquire: task_worker::AcquireConfig::default(),
            evidence: task_worker::PaperQaEvidence::default(),
            max_asks: default_paperqa_max_asks(),
        }
    }
}

fn default_paperqa_command() -> String {
    "python".to_string()
}

fn default_paperqa_max_asks() -> u32 {
    10
}

/// `local-deep-research` アダプタの設定（ADR-0029 D1）。フィールドの意味は
/// `task_worker::LdrConfig`（`crates/task-worker/src/local_deep_research.rs`）と同じ。
/// `[[providers]] adapter = "local-deep-research"` の行ごとの上書きは `model`/`env` だけ
/// （`ProviderConfig` の既存フィールドを再利用。`paperqa`/`acp` と同じ作り）。`settings` は行では
/// 上書きしない（`ProviderConfig.settings` は `paperqa` 専用のフィールドで、LDR では再利用しない。
/// 必要になれば別 ADR で行ごとの上書きを足す）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LdrAdapterConfig {
    /// 起動するコマンド（LDR を入れた venv の python）。既定 `"python3"`。
    #[serde(default = "default_ldr_command")]
    pub command: String,
    /// `quick`（既定）| `detailed` | `report`。
    #[serde(default)]
    pub mode: task_worker::LdrMode,
    #[serde(default)]
    pub iterations: Option<u32>,
    #[serde(default)]
    pub questions_per_iteration: Option<u32>,
    /// `settings_override` に渡すキー。値は文字列で書き、数値・真偽値・JSON 配列/オブジェクトに見える
    /// ものはランナー（Python）側で変換する（ADR-0029 D1/D3: TOML の型を混ぜない）。
    #[serde(default)]
    pub settings: HashMap<String, String>,
    /// 追加の環境変数（例: `search.tool` に対応する SearXNG の URL 等は `settings` 側。ここは
    /// LiteLLM/OpenAI 互換エンドポイントの鍵など、プロセス環境変数として渡すもの）。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id（例: `LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY = "tavily"`）。
    /// `env` より優先。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// `[adapters.local_deep_research.evidence]`（ADR-0031 D2）: 決定的な証拠ゲートの閾値。
    /// 意味は `task_worker::EvidenceThresholds` と同じ。
    #[serde(default)]
    pub evidence: task_worker::EvidenceThresholds,
    /// ADR-0063 D2（Phase 109）: `attempts >= 1`（前回が reviewer 不合格）の run で使う `mode`。
    /// 既定 `detailed`。
    #[serde(default = "default_ldr_retry_mode")]
    pub retry_mode: task_worker::LdrMode,
    /// ADR-0063 D2: 同じく再挑戦の run で使う `iterations`。既定 `Some(5)`。`null` を書けば
    /// 通常の `iterations` のまま（再挑戦でも上げない）。
    #[serde(default = "default_ldr_retry_iterations")]
    pub retry_iterations: Option<u32>,
    /// ADR-0063 Phase 109c B3: 目的文から対象が取れたとき、LDR の答えと必読の一次情報の抜粋を材料に
    /// プロキシの LLM で対象×観点の表と対象ごとの節を合成し `report.md` の先頭に置くか。既定 `true`。
    #[serde(default = "default_ldr_structured_synthesis")]
    pub structured_synthesis: bool,
}

fn default_ldr_retry_mode() -> task_worker::LdrMode {
    task_worker::LdrMode::Detailed
}

fn default_ldr_retry_iterations() -> Option<u32> {
    Some(5)
}

fn default_ldr_structured_synthesis() -> bool {
    true
}

impl Default for LdrAdapterConfig {
    fn default() -> Self {
        Self {
            command: default_ldr_command(),
            mode: task_worker::LdrMode::default(),
            iterations: None,
            questions_per_iteration: None,
            settings: HashMap::new(),
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
            evidence: task_worker::EvidenceThresholds::default(),
            retry_mode: default_ldr_retry_mode(),
            retry_iterations: default_ldr_retry_iterations(),
            structured_synthesis: default_ldr_structured_synthesis(),
        }
    }
}

fn default_ldr_command() -> String {
    "python3".to_string()
}

impl PaperQaAdapterConfig {
    /// ADR-0027 D3: `[adapters.paperqa]` のパス設定は、他のパス設定と同じく設定ファイルのディレクトリ基準で
    /// 絶対化する。`settings` は `pqa -s` に渡す文字列（拡張子無し）だが、パスの形をしているので同様に扱う。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        if let Some(dir) = &self.paper_directory
            && dir.is_relative()
        {
            self.paper_directory = Some(base.join(dir));
        }
        if let Some(dir) = &self.index_directory
            && dir.is_relative()
        {
            self.index_directory = Some(base.join(dir));
        }
        if let Some(settings) = &self.settings
            && Path::new(settings).is_relative()
        {
            self.settings = Some(base.join(settings).to_string_lossy().into_owned());
        }
    }
}
