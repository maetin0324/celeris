//! ADR 2026-11-07-health-review-integration D2.4: repo 固有の review 方針（profile の `[review]` 節）の
//! Rust 側の型。
//!
//! 値の正本は repo の `scripts/dev/repo-health/profiles/<profile>.toml` の `[review]` 節で、dispatcher は
//! `repo_health.py --print-review-policy` が base（target）側の版から出した JSON
//! （`celeris.repo-health.review-policy/1`）を読んでこの型にする。Rust では TOML を読まない。
//! 型は worker protocol の `RunContext.review_policy` にそのまま載る（planner・worker・reviewer の
//! prompt への写し）。risk の判定（`assess_risk`）は D3.3 の後続 unit がこの module に足す。

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// D2.4: 解決済みの review 方針。`source = absent`（base に `[review]` が無い）の repo では作らない
/// （`RunContext.review_policy = None`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewPolicy {
    /// 変更 path の owner 行を引く file（repo 相対。例 `docs/architecture-map.md`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_map: Option<String>,
    /// 高 risk の path class。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub high_risk: Vec<PathClass>,
    /// 保護する不変条件の名前 → 根拠の file（repo 相対）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub invariants: BTreeMap<String, String>,
    /// 解決した profile の blob の内容から作った digest（D2.3）。health の材料の再利用の鍵に入る。
    pub definition_sha256: String,
}

/// D2.2: 高 risk の path class 1 件。`paths` は repo 相対 path に `re.search` で当てる正規表現。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PathClass {
    pub class: String,
    pub paths: Vec<String>,
}
