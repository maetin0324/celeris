//! ADR 2026-10-04-multi-objective-model-routing Phase 2: routing の相関欄。
//! `llm_proxy_requests`（0022）の行に、決定・snapshot・run・task・供給元・モデルを結ぶ欄（0048）の型。
//! 行と相関欄は llm-proxy（`log.rs` の `insert_routed`）が 1 文で書く。

/// 要求 1 件の相関欄。`None` は「未記録」（0048 より前の行は全部 `None`）。
/// `request_id` は `llm_proxy_requests.id`、`account` は既存欄（0022）をそのまま使う。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingCorrelation {
    pub decision_id: Option<String>,
    pub snapshot_id: Option<String>,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub source_id: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
}
