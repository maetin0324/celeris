//! `[memory]`（ADR-0033 D6）と `[knowledge]`（ADR-0047）: 長期記憶と知識ベースの置き場。

use std::path::{Path, PathBuf};

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
/// ADR-0132 D4: LangMem は Qwen 専用ではない普通の道具。接続先は celeris の llm-proxy、モデルは
/// 抽象名 `celeris/cheap` を既定の書き方とする（Qwen が生きていれば proxy が Qwen を選び、落ちていれば
/// Claude / GPT の cheap に倒す。ADR-0132 D3）。dispatch 前の到達性 probe（ADR-0052 D1）はこの
/// `base_url`（proxy）を検査し、proxy 自体に届かないときだけ cheap の汎用ハーネスへ倒す（ADR-0052 D2）。
///
/// ```toml
/// [knowledge.langmem]
/// enabled = true
/// provider = "openai-compatible"   # "openai-compatible" | "anthropic"
/// base_url = "http://127.0.0.1:18100/v1"   # celeris の llm-proxy
/// model = "celeris/cheap"
/// api_key_secret = "llm-proxy-token"   # [secrets] の下の id。proxy は bearer を要求する
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

impl MemoryConfig {
    /// ADR-0033 D6 / ADR-0045 D2: `[memory] dir` は `~` を展開し、相対なら設定ファイルのディレクトリ基準。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        self.dir = task_core::expand_home(&self.dir, task_core::home_dir().as_deref());
        if self.dir.is_relative() {
            self.dir = base.join(&self.dir);
        }
    }
}

impl KnowledgeConfig {
    /// ADR-0047 D1（Phase 61）: `[knowledge] root` も同じ扱い（既定の `~/.local/share/celeris/knowledge` もここで絶対パスになる）。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        self.root = task_core::expand_home(&self.root, task_core::home_dir().as_deref());
        if self.root.is_relative() {
            self.root = base.join(&self.root);
        }
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        // ADR-0047 D2（Phase 61）: `[knowledge] default_mounts` の綴り（間違いで黙って無視しない）。
        if let Err(why) = self.mounts() {
            return Err(ConfigError::Invalid(format!(
                "[knowledge] default_mounts: {why}"
            )));
        }
        Ok(())
    }
}

/// ADR-0139 D2: `base_url`（`http://host:port/...`）が同じ celeris の llm-proxy を指すか。
/// `[llm_proxy]` が有効で、ポートが `listen` と同じ、かつ host が loopback か `listen` の IP のとき。
fn targets_llm_proxy(base_url: &str, proxy: &llm_proxy::config::LlmProxyConfig) -> bool {
    if !proxy.effective_enabled() {
        return false;
    }
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    let Ok(port) = port.parse::<u16>() else {
        return false;
    };
    if port != proxy.listen.port() {
        return false;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback() || ip == proxy.listen.ip(),
        Err(_) => false,
    }
}

impl Config {
    /// ADR-0139 D2: `[knowledge.langmem].base_url` が同じ celeris の llm-proxy を指すか。
    pub fn langmem_targets_llm_proxy(&self) -> bool {
        self.knowledge
            .langmem
            .base_url
            .as_deref()
            .is_some_and(|url| targets_llm_proxy(url, &self.llm_proxy))
    }

    /// ADR-0139 D2: langmem（とその dispatch 前 probe）に渡す鍵。proxy を指すなら proxy が照合する
    /// `[api] token_file` の値（読めなければ `api_key_secret` に倒す）。それ以外は `api_key_secret` を
    /// `[secrets] dir` から解決した値。**値はログに出さない**。
    pub fn langmem_api_key(&self) -> Option<String> {
        if self.langmem_targets_llm_proxy()
            && let Ok(Some(token)) = self.api.read_token()
        {
            return Some(token);
        }
        self.langmem_secret_key()
    }

    fn langmem_secret_key(&self) -> Option<String> {
        let id = self.knowledge.langmem.api_key_secret.as_deref()?;
        crate::resolve_secret(self.secrets.as_ref().map(|s| s.dir.as_path()), id)
    }

    /// ADR-0139 D3: 知識整理 run が bearer を持たずに起きる・食い違う設定を、run を起こす前に知らせる
    /// （`Config::load` が `warn!` で出す）。`[knowledge.langmem]` が無効なら空。値は含めない。
    pub fn langmem_auth_warnings(&self) -> Vec<String> {
        let lm = &self.knowledge.langmem;
        if !lm.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.langmem_targets_llm_proxy() {
            match self.api.read_token() {
                Ok(Some(token)) => {
                    if lm.api_key_secret.is_some()
                        && self.langmem_secret_key().is_some_and(|key| key != token)
                    {
                        out.push(
                            "[knowledge.langmem].api_key_secret differs from the [api] token that \
                             llm-proxy checks; langmem uses the [api] token (ADR-0139 D2)"
                                .to_string(),
                        );
                    }
                }
                Ok(None) | Err(_) => {
                    if self.langmem_secret_key().is_none() {
                        out.push(
                            "[knowledge.langmem].base_url points at llm-proxy but neither [api] \
                             token_file nor api_key_secret is readable; langmem runs will get 401 \
                             (ADR-0139 D3)"
                                .to_string(),
                        );
                    } else {
                        out.push(
                            "[knowledge.langmem].base_url points at llm-proxy but [api] token_file \
                             is not readable; falling back to api_key_secret (ADR-0139 D3)"
                                .to_string(),
                        );
                    }
                }
            }
        } else if let Some(id) = &lm.api_key_secret
            && self.langmem_secret_key().is_none()
        {
            out.push(format!(
                "[knowledge.langmem].api_key_secret {id:?} cannot be resolved from [secrets] dir; \
                 langmem runs start without an API key (ADR-0139 D3)"
            ));
        }
        out
    }
}
