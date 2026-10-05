use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// ADR 2026-10-04 §3.4: task と要求の入力 snapshot。
///
/// 本文・credential・prompt は持たない（feature event に入れない）。欄は後から増えても旧 JSON を
/// decode できるよう、`#[serde(default)]` で欠けた欄を `Default` にし、未知の欄は無視する
/// （`deny_unknown_fields` は付けない）。欠測の欄は `None` と `missing_fields` で示し、0 や空で埋めない
/// （ただし `required_*` の bool と `safety_margin` は Phase 2 以前からの欄なので既定値を持つ）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RoutingContext {
    pub version: String,
    /// 由来の種別。task に属さない要求は `standalone`。
    pub origin: String,
    pub task_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub run_id: Option<String>,
    /// 組織の node（課）。org の seed ではなく DB 上の node id。
    pub org_node: Option<String>,
    /// worker / reviewer / planner / cos など。
    pub role: Option<String>,
    pub harness: Option<String>,
    pub task_kind: Option<String>,
    pub phase: Option<RoutingPhase>,
    /// 受け入れ条件の安定 ID 列（command/artifact/reviewer/human）。生の command は入れない。
    pub acceptance_criteria: Vec<String>,
    /// harness が要求するプロトコル（tool calling）が必要か。
    pub required_tools: bool,
    /// 必要な tool の capability ID 列。
    pub required_tool_ids: Vec<String>,
    pub required_structured_output: bool,
    pub required_vision: bool,
    pub required_streaming: bool,
    pub environment: RoutingEnvironment,
    pub input_tokens: Option<u64>,
    pub output_reserve: Option<u64>,
    pub safety_margin: u64,
    /// run / WU ごとの試行数。
    pub attempts: Option<u32>,
    /// reviewer が不合格とした回数。
    pub review_failures: Option<u32>,
    /// 決定的検査（acceptance の check）が落ちた回数。
    pub check_failures: Option<u32>,
    /// スケジューラの出自付きの値。品質制約を緩める理由にはしない。
    pub priority: Option<i32>,
    /// 文脈全体の出自（例: `llm-proxy:request-fields`）。欄ごとの出自は `field_provenance`。
    pub provenance: String,
    /// 欄ごとの出自（欄名 → 出自）。
    pub field_provenance: BTreeMap<String, String>,
    /// 取得できなかった欄の名前。
    pub missing_fields: Vec<String>,
}

/// task の計画段（ADR §3.4 の phase）。知らない値は `Unknown` に読む（失敗させない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingPhase {
    Planning,
    Implementation,
    Review,
    #[serde(other)]
    Unknown,
}

/// 実行環境（ADR §3.4 の environment）。欄が無ければ `None`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RoutingEnvironment {
    /// 実行場所の種別（例: `local` / `remote` / `cluster`）。
    pub locality: Option<String>,
    pub host: Option<String>,
    /// 外部ネットワークへの接続が要るか。
    pub external_network: Option<bool>,
}

/// task と無関係な要求（proxy 直叩き・standalone）の最小 context。
/// task 由来の欄は全部 `None` で、`missing_fields` にその名前を入れる。
pub fn standalone_context(provenance: &str) -> RoutingContext {
    RoutingContext {
        version: "1".into(),
        origin: "standalone".into(),
        provenance: provenance.into(),
        missing_fields: [
            "task_id",
            "work_unit_id",
            "run_id",
            "org_node",
            "role",
            "harness",
            "task_kind",
            "phase",
            "acceptance_criteria",
            "attempts",
            "review_failures",
            "check_failures",
            "priority",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        ..RoutingContext::default()
    }
}
