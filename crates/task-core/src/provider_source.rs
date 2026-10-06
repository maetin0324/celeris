//! Provider rows describe an execution adapter and, separately, its LLM source.

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Adapter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmSourceRef {
    Celeris,
    ClaudeOauth,
    CodexOauth,
    /// opencode go の subscription（ADR 2026-10-06 D3）。
    OpencodeGo,
    OpenaiCompatible(String),
    None,
    Unknown,
}

impl LlmSourceRef {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Celeris => "celeris",
            Self::ClaudeOauth => "claude_oauth",
            Self::CodexOauth => "codex_oauth",
            Self::OpencodeGo => "opencode_go",
            Self::OpenaiCompatible(_) => "openai_compatible",
            Self::None => "none",
            Self::Unknown => "unknown",
        }
    }
}

impl Serialize for LlmSourceRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::OpenaiCompatible(id) => {
                serializer.serialize_str(&format!("openai_compatible:{id}"))
            }
            _ => serializer.serialize_str(self.as_str()),
        }
    }
}

impl<'de> Deserialize<'de> for LlmSourceRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "celeris" => Ok(Self::Celeris),
            "claude_oauth" => Ok(Self::ClaudeOauth),
            "codex_oauth" => Ok(Self::CodexOauth),
            "opencode_go" => Ok(Self::OpencodeGo),
            "none" => Ok(Self::None),
            "unknown" => Ok(Self::Unknown),
            _ => value
                .strip_prefix("openai_compatible:")
                .filter(|id| !id.is_empty())
                .map(|id| Self::OpenaiCompatible(id.to_owned()))
                .ok_or_else(|| serde::de::Error::custom("invalid llm_source reference")),
        }
    }
}

impl JsonSchema for LlmSourceRef {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "LlmSourceRef".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        String::json_schema(generator)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    Explicit,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedLlmSource {
    pub source: LlmSourceRef,
    pub origin: SourceOrigin,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_source_string_roundtrip_and_schema() {
        for source in [
            LlmSourceRef::Celeris,
            LlmSourceRef::ClaudeOauth,
            LlmSourceRef::CodexOauth,
            LlmSourceRef::OpencodeGo,
            LlmSourceRef::OpenaiCompatible("qwen".into()),
            LlmSourceRef::None,
            LlmSourceRef::Unknown,
        ] {
            let encoded = serde_json::to_string(&source).unwrap();
            assert_eq!(
                serde_json::from_str::<LlmSourceRef>(&encoded).unwrap(),
                source
            );
        }
        assert_eq!(
            serde_json::to_string(&LlmSourceRef::OpenaiCompatible("qwen".into())).unwrap(),
            "\"openai_compatible:qwen\""
        );
        assert!(serde_json::from_str::<LlmSourceRef>("\"openai_compatible:\"").is_err());
        let schema = schemars::schema_for!(LlmSourceRef);
        assert_eq!(
            schema.get("type").and_then(|value| value.as_str()),
            Some("string")
        );
    }
}
