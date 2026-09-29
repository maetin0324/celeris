use std::path::PathBuf;

use serde::Deserialize;

use super::{Config, ConfigError};

/// `[memory]`（ADR-0033 D6）: 組織のノードごとの長期記憶。`<dir>/<node_id>/notes.md` と
/// `<dir>/<node_id>/projects/<project_id>.md`。中身は run の前に前置きされ、結果ファイルの `memory` が
/// 日付付きの箇条書きで追記される。人の好みや相談の中身が入るので `dir` は 0700 で作る。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfig {
    /// ADR-0045 D2: 省略時は `~/.local/celeris/memory`。相対なら設定ファイル基準。`Config::load` が絶対化する。
    #[serde(default = "default_memory_dir")]
    pub dir: PathBuf,
}

/// ADR-0045 D2: `~/.local/celeris/memory`。
fn default_memory_dir() -> PathBuf {
    PathBuf::from("~/.local/celeris/memory")
}

/// `[knowledge]`（ADR-0047 D1 / D2）: 知識ベースの正本の置き場と、既定のマウント。
///
/// ```toml
/// [knowledge]
/// root = "~/.local/share/celeris/knowledge"
/// default_mounts = ["kb:user", "kb:environment"]
/// ```
///
/// **celeris はこのディレクトリを勝手に作らない**（用意するのは `celerisctl knowledge init` だけ）。
/// 無ければ `GET /knowledge/tree` は `initialized: false` を返し、前置きの索引は空になる。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeConfig {
    /// ADR-0047 D1: 既定は `~/.local/share/celeris/knowledge`。相対なら設定ファイル基準。`Config::load` が絶対化する。
    #[serde(default = "default_knowledge_root")]
    pub root: PathBuf,
    /// ADR-0047 D2: 実効 profile（ADR-0046 D1）の `knowledge` に何も無いときに全ノードが継ぐマウント。
    /// 書き方は `kb:<scope>` / `repo:<name>[:<docs>]` / `dir:<path>` / `memory[:<node>]`。
    #[serde(default = "default_knowledge_mounts")]
    pub default_mounts: Vec<String>,
    /// ADR-0047 D4（Phase 62）: LangMem による自動メンテナンス（既定は無効）。
    #[serde(default)]
    pub langmem: LangMemKnowledgeConfig,
    #[serde(default)]
    pub gc: crate::knowledge_gc::GcConfig,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            root: default_knowledge_root(),
            default_mounts: default_knowledge_mounts(),
            langmem: LangMemKnowledgeConfig::default(),
            gc: crate::knowledge_gc::GcConfig::default(),
        }
    }
}

/// `[knowledge.langmem]`（ADR-0047 D4）: 知識整理 run のトリガと LLM の接続先。
///
/// ```toml
/// [knowledge.langmem]
/// enabled = true
/// provider = "openai-compatible"   # "openai-compatible" | "anthropic"
/// base_url = "http://bnode150:18000/v1"
/// model = "qwen3.8-27b"
/// api_key_secret = "langmem-openai-key"   # [secrets] の下の id。無ければ渡さない
/// max_related_pages = 10
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LangMemKnowledgeConfig {
    /// 既定 `false`（ADR-0047 D4 / §6: 明示的に有効化するまで知識整理 run は起きない）。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: task_worker::LangMemProvider,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// `[secrets] dir` の下の id（LLM の API キー）。省略すると渡さない
    /// （ローカルの OpenAI 互換エンドポイントは鍵を確認しないことが多い）。
    #[serde(default)]
    pub api_key_secret: Option<String>,
    /// ADR-0047 D3: 関連する既存の KB ページを検索の上位何件まで依頼文に入れるか。
    #[serde(default = "default_max_related_pages")]
    pub max_related_pages: usize,
}

impl Default for LangMemKnowledgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: task_worker::LangMemProvider::default(),
            base_url: None,
            model: None,
            api_key_secret: None,
            max_related_pages: default_max_related_pages(),
        }
    }
}

fn default_max_related_pages() -> usize {
    10
}

impl KnowledgeConfig {
    /// `default_mounts` を [`task_core::KnowledgeMount`] にする（綴り間違いは `validate()` が弾く）。
    pub fn mounts(&self) -> Result<Vec<task_core::KnowledgeMount>, String> {
        self.default_mounts
            .iter()
            .map(|m| m.parse::<task_core::KnowledgeMount>())
            .collect()
    }
}

fn default_knowledge_root() -> PathBuf {
    task_core::knowledge::default_root()
}

/// ADR-0046 D7 の木が `knowledge` を書くまでの既定（人のことと環境は誰でも読む）。
fn default_knowledge_mounts() -> Vec<String> {
    vec!["kb:user".to_string(), "kb:environment".to_string()]
}

impl Config {
    /// ADR-0033 D6: `[memory] dir` を 0700 で作る（無ければ）。`[memory]` が無ければ何もしない。
    pub fn ensure_memory_dir(&self) -> Result<(), ConfigError> {
        let Some(memory) = &self.memory else {
            return Ok(());
        };
        task_worker::memory::create_dir_all_0700(&memory.dir).map_err(|source| ConfigError::Read {
            path: memory.dir.clone(),
            source,
        })
    }
}
