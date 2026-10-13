//! 要求のモデル名（ADR 2026-10-11 llm-source-harness-selection「llm-proxy の位置」）。
//! 要求は具体的な `<source_id>/<model_id>` だけを受ける。source ID は `claude_oauth`・`codex_oauth`・
//! `[[llm_proxy.sources.openai_compatible]]` の `id`。model ID はその source の model をそのまま上流へ渡す
//! （`/` を含んでよい。最初の `/` までが source ID）。判断（選択）は `crate::selection` にある。

/// 要求されたモデル名がどの供給元の種類を指すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// `claude-oauth`。
    Claude,
    /// `codex-oauth`。
    Gpt,
    /// `openai-compatible`（既定の実装は Qwen）。
    Qwen,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Claude => "claude",
            SourceKind::Gpt => "gpt",
            SourceKind::Qwen => "qwen",
        }
    }
}

/// `[llm_proxy.sources.claude_oauth]` の source ID。
pub const CLAUDE_OAUTH: &str = "claude_oauth";
/// `[llm_proxy.sources.codex_oauth]` の source ID。
pub const CODEX_OAUTH: &str = "codex_oauth";

/// 要求が指す source。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRef {
    ClaudeOauth,
    CodexOauth,
    /// `[[llm_proxy.sources.openai_compatible]]` の `id`（設定にあるかは server が確かめる）。
    OpenAiCompatible(String),
}

impl SourceRef {
    pub fn kind(&self) -> SourceKind {
        match self {
            SourceRef::ClaudeOauth => SourceKind::Claude,
            SourceRef::CodexOauth => SourceKind::Gpt,
            SourceRef::OpenAiCompatible(_) => SourceKind::Qwen,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            SourceRef::ClaudeOauth => CLAUDE_OAUTH,
            SourceRef::CodexOauth => CODEX_OAUTH,
            SourceRef::OpenAiCompatible(id) => id,
        }
    }
}

/// 解析されたモデル要求: 具体的な source と、その source の model。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub source: SourceRef,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelNameError {
    #[error("empty model name")]
    Empty,
    #[error(
        "model name {0:?} is not <source_id>/<model_id> (e.g. qwen/qwen3.8-27b, claude_oauth/claude-sonnet-5)"
    )]
    NotConcrete(String),
}

/// `name` を `<source_id>/<model_id>` として解析する。source ID が設定に在るかは見ない
/// （`claude_oauth`・`codex_oauth` 以外は openai-compatible の id として扱い、server が照合する）。
pub fn parse_model(name: &str) -> Result<ModelRequest, ModelNameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ModelNameError::Empty);
    }
    let Some((source, model)) = name.split_once('/') else {
        return Err(ModelNameError::NotConcrete(name.to_string()));
    };
    if source.is_empty() || model.is_empty() {
        return Err(ModelNameError::NotConcrete(name.to_string()));
    }
    let source = match source {
        CLAUDE_OAUTH => SourceRef::ClaudeOauth,
        CODEX_OAUTH => SourceRef::CodexOauth,
        other => SourceRef::OpenAiCompatible(other.to_string()),
    };
    Ok(ModelRequest {
        source,
        model: model.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_route_concrete_names_name_the_source_and_model() {
        assert_eq!(
            parse_model("claude_oauth/claude-sonnet-5").unwrap(),
            ModelRequest {
                source: SourceRef::ClaudeOauth,
                model: "claude-sonnet-5".into()
            }
        );
        assert_eq!(
            parse_model("codex_oauth/gpt-6-sol").unwrap().source,
            SourceRef::CodexOauth
        );
        assert_eq!(
            parse_model("qwen/org/qwen3.8-27b").unwrap(),
            ModelRequest {
                source: SourceRef::OpenAiCompatible("qwen".into()),
                model: "org/qwen3.8-27b".into()
            }
        );
    }

    #[test]
    fn proxy_route_names_without_a_source_and_model_are_rejected() {
        for name in ["", "gpt-4", "/m", "qwen/", "claude:claude-sonnet-5"] {
            assert!(parse_model(name).is_err(), "{name}");
        }
    }
}
