//! ADR 2026-10-06 D4: 利用可能モデルの catalog の型と、発見結果の純粋な解析（文字列 → `Vec<DiscoveredModel>`）。
//!
//! I/O（コマンド実行・HTTP）は `celeris::model_discovery`、永続化は `store::model_catalog`。ここは何も取りに行かない。
//! 時刻は UNIX 秒（`i64`）。上書き（`CatalogOverride`）は自動更新の対象外で、別表に持つ。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{TaskId, Tier};

pub mod assignments;

/// `Event::ModelCatalogChanged` を追記する疑似 task の id（nil ULID）。events は task ごとの列なので、
/// task に属さない catalog の変化はこの 1 本に集める。実在の task とは衝突しない。
pub fn catalog_event_task_id() -> TaskId {
    TaskId(ulid::Ulid::nil())
}

/// catalog の source 名。`claude-oauth`・`codex-oauth`・`opencode-go`・`openai-compatible:<id>`
/// （llm-proxy / routing の source 名と同じ）。
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct CatalogSource(pub String);

impl CatalogSource {
    pub const CLAUDE_OAUTH: &'static str = "claude-oauth";
    pub const CODEX_OAUTH: &'static str = "codex-oauth";
    pub const OPENCODE_GO: &'static str = "opencode-go";
    pub const OPENAI_COMPATIBLE_PREFIX: &'static str = "openai-compatible:";

    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn openai_compatible(id: &str) -> Self {
        Self(format!("{}{id}", Self::OPENAI_COMPATIBLE_PREFIX))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 既知の形か（空・空白入りは不可）。
    pub fn is_valid(&self) -> bool {
        let s = self.0.as_str();
        s == Self::CLAUDE_OAUTH
            || s == Self::CODEX_OAUTH
            || s == Self::OPENCODE_GO
            || s.strip_prefix(Self::OPENAI_COMPATIBLE_PREFIX)
                .is_some_and(|id| !id.is_empty() && !id.contains(char::is_whitespace))
    }
}

impl std::fmt::Display for CatalogSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 発見で見えた 1 モデル。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveredModel {
    pub model_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// source ごとの付帯情報（そのまま保存する。無ければ `{}`）。
    #[serde(default = "empty_object")]
    pub capabilities: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

impl DiscoveredModel {
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            display_name: None,
            capabilities: empty_object(),
        }
    }
}

/// `model_catalog` の 1 行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogEntry {
    pub source: CatalogSource,
    pub model_id: String,
    pub display_name: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub available: bool,
    pub capabilities: serde_json::Value,
}

/// 人の上書き（`model_catalog_overrides`）。自動更新は触らない。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogOverride {
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub tier: Option<Tier>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// 上書きの 1 行（キー付き）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogOverrideRow {
    pub source: CatalogSource,
    pub model_id: String,
    pub value: CatalogOverride,
    pub updated_at: i64,
}

/// source ごとの最終発見記録。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveryRecord {
    pub source: CatalogSource,
    pub at: i64,
    pub ok: bool,
    pub error: Option<String>,
    pub count: u32,
}

/// 1 回の反映で変わったモデル。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogDelta {
    pub source: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub restored: Vec<String>,
}

impl CatalogDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.restored.is_empty()
    }
}

/// OpenAI 形式 `{"object":"list","data":[{"id":…}]}`（opencode go gateway・self-host `/v1/models`・
/// Anthropic `/v1/models`）。`display_name` があれば拾う。`id` の無い・空の要素は捨て、重複は 1 件にする。
pub fn parse_openai_models_list(body: &str) -> Result<Vec<DiscoveredModel>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("invalid models JSON: {e}"))?;
    let data = value
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "models JSON has no `data` array".to_string())?;
    let mut out: Vec<DiscoveredModel> = Vec::new();
    for item in data {
        let Some(id) = item.get("id").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        if id.is_empty() || out.iter().any(|m| m.model_id == id) {
            continue;
        }
        let mut model = DiscoveredModel::new(id);
        model.display_name = item
            .get("display_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        out.push(model);
    }
    Ok(out)
}

/// `opencode models <provider>` の stdout。`provider/model` を 1 行 1 件で出す。接頭辞を外し、他の行
/// （ログ・他 provider・空行）は無視する。
pub fn parse_opencode_models_stdout(stdout: &str, provider: &str) -> Vec<DiscoveredModel> {
    let prefix = format!("{provider}/");
    let mut out: Vec<DiscoveredModel> = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        let Some(id) = line.strip_prefix(&prefix) else {
            continue;
        };
        // モデル名は空白を含まない 1 語。説明付きの行などは捨てる。
        if id.is_empty() || id.contains(char::is_whitespace) || out.iter().any(|m| m.model_id == id)
        {
            continue;
        }
        out.push(DiscoveredModel::new(id));
    }
    out
}

/// codex app-server の `model/list` の 1 ページ（`{data:[Model], nextCursor}`）。`id`、無ければ `model` を
/// model_id にする。`displayName` を拾い、`hidden: true` の要素は捨てる（`includeHidden: false` の保険）。
pub fn parse_codex_model_list(
    response: &serde_json::Value,
) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
    let data = response
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "model/list response has no `data` array".to_string())?;
    let mut out = Vec::new();
    for item in data {
        if item.get("hidden").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                item.get("model")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            });
        let Some(id) = id else { continue };
        let mut model = DiscoveredModel::new(id);
        model.display_name = item
            .get("displayName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let mut caps = serde_json::Map::new();
        for key in ["inputModalities", "supportedReasoningEfforts", "isDefault"] {
            if let Some(v) = item.get(key) {
                caps.insert(key.to_string(), v.clone());
            }
        }
        model.capabilities = serde_json::Value::Object(caps);
        out.push(model);
    }
    let next = response
        .get("nextCursor")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok((out, next))
}

#[cfg(test)]
mod tests;
