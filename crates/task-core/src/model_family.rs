//! ADR 2026-10-07（coding harness の既定）: model の family 判定。
//!
//! 自由文字列の `ModelProfile.family` を enum に変えるのは [`ModelFamily::parse`] だけにする。
//! 他の crate で `family == "claude"`・`contains("claude")` のような直書きをしない。
//! family は閉じた型（`LlmSourceRef`・`AccountAdapter`）から先に導き、決まらなければ
//! routing catalog の `ModelProfile.family`、それでも決まらなければ `Unknown`（非 Claude）にする。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::accounts::AccountAdapter;
use crate::model_router::profiles::ModelProfile;
use crate::provider_source::LlmSourceRef;

/// model の系統。未知（`Other`・`Unknown`）は非 Claude として扱う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelFamily {
    Claude,
    Gpt,
    Qwen,
    /// family は書かれているが既知の系統ではない（DeepSeek 等）。
    Other,
    /// family を決める材料が無い。
    Unknown,
}

impl ModelFamily {
    /// `ModelProfile.family` の文字列から enum へ変える唯一の場所。大文字小文字は区別しない。
    /// 空（空白だけを含む）は `Unknown`、既知でない名前は `Other`。
    pub fn parse(s: &str) -> ModelFamily {
        let s = s.trim();
        if s.is_empty() {
            ModelFamily::Unknown
        } else if s.eq_ignore_ascii_case("claude") {
            ModelFamily::Claude
        } else if s.eq_ignore_ascii_case("gpt") {
            ModelFamily::Gpt
        } else if s.eq_ignore_ascii_case("qwen") {
            ModelFamily::Qwen
        } else {
            ModelFamily::Other
        }
    }

    /// Claude 系か。`Claude` のときだけ true（未知は false）。
    pub fn is_claude(self) -> bool {
        matches!(self, ModelFamily::Claude)
    }
}

/// family をどの材料で決めたか（routing audit に残す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FamilyBasis {
    LlmSource,
    AccountPool,
    ModelProfile,
    Unknown,
}

/// [`derive_family`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FamilyDecision {
    pub family: ModelFamily,
    pub basis: FamilyBasis,
}

/// provider 行の family を導く。上から順に最初に決まったものを採る:
/// 1. `LlmSourceRef`（`ClaudeOauth` → Claude、`CodexOauth` → Gpt）
/// 2. 行の pool の `AccountAdapter`（`ClaudeCode` → Claude、`Codex` → Gpt。`OpencodeGo` は model 次第）
/// 3. `ModelProfile.family`（routing catalog。[`ModelFamily::parse`]）
/// 4. `Unknown`
///
/// `DeploymentProfile.source_ref` のような文字列は判定に使わない。
/// deployment は family の材料を持たないため引数に含めない。
/// catalog profile が無い行は `model = None` を渡せる。
pub fn derive_family(
    source: &LlmSourceRef,
    pool: Option<AccountAdapter>,
    model: Option<&ModelProfile>,
) -> FamilyDecision {
    let by_source = match source {
        LlmSourceRef::ClaudeOauth => Some(ModelFamily::Claude),
        LlmSourceRef::CodexOauth => Some(ModelFamily::Gpt),
        LlmSourceRef::Celeris
        | LlmSourceRef::OpencodeGo
        | LlmSourceRef::OpenaiCompatible(_)
        | LlmSourceRef::None
        | LlmSourceRef::Unknown => None,
    };
    if let Some(family) = by_source {
        return FamilyDecision {
            family,
            basis: FamilyBasis::LlmSource,
        };
    }
    let by_pool = match pool {
        Some(AccountAdapter::ClaudeCode) => Some(ModelFamily::Claude),
        Some(AccountAdapter::Codex) => Some(ModelFamily::Gpt),
        Some(AccountAdapter::OpencodeGo) | None => None,
    };
    if let Some(family) = by_pool {
        return FamilyDecision {
            family,
            basis: FamilyBasis::AccountPool,
        };
    }
    match model.map(|m| ModelFamily::parse(&m.family)) {
        Some(family) if family != ModelFamily::Unknown => FamilyDecision {
            family,
            basis: FamilyBasis::ModelProfile,
        },
        _ => FamilyDecision {
            family: ModelFamily::Unknown,
            basis: FamilyBasis::Unknown,
        },
    }
}
