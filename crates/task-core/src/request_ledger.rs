//! 論理要求の台帳（ADR 2026-10-27-eval-request-ledger）の出力の形。
//!
//! 判断要求（`cos_inbox_items` の source triple）を論理要求 `(source_kind, source_key)` の 1 行にまとめる。
//! 組み立ては `SqliteStore::request_ledger`（読み取りだけ）。不明な値は `null` で出し、0 や空文字で埋めない。
use std::collections::BTreeMap;

use serde::Serialize;

/// 出力の schema 名。欄を変えたら版を上げる。
pub const REQUEST_LEDGER_SCHEMA: &str = "celeris.eval-requests/1";

/// `celerisctl eval requests` の出力全体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestLedger {
    pub schema: &'static str,
    pub since: String,
    pub until: String,
    pub summary: RequestLedgerSummary,
    pub requests: Vec<RequestRow>,
}

/// 件数の要約（元表との照合用）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RequestLedgerSummary {
    /// 論理要求の数（= `requests` の行数）。
    pub requests: usize,
    /// 範囲の要求が持つ `cos_inbox_items` の行数（revision の総数）。
    pub items: usize,
    /// 差戻し（人が CoS の operation を覆した）の総数。
    pub returns: usize,
    /// 解決した主体の種類ごとの要求の数。解決が無い要求は `unresolved`。
    pub resolved_by_actor_kind: BTreeMap<String, usize>,
    /// 初回の AI 応答が見つからない要求の数。
    pub first_ai_response_missing: usize,
    /// 終わっていない人の待ち区間の数。
    pub open_human_waits: usize,
}

/// 論理要求 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestRow {
    /// `<source_kind>:<source_key>`（最初の item の key。human-ops /2 D6 と同じ形）。
    pub request_ref: String,
    pub source_kind: String,
    pub source_key: String,
    /// この要求に寄せた全ての key（差戻しで decision・approval の key が変わったものを含む）。
    pub keys: Vec<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub revisions: Vec<RequestRevision>,
    pub returns: Vec<RequestReturn>,
    pub first_ai_response: Option<ActorAt>,
    pub human_waits: Vec<HumanWaitSpan>,
    /// 最後の revision の解決。無ければ `null`。
    pub resolved: Option<ActorAt>,
    /// 解決の後に関係 task（と子孫）で最初に始まった run の開始時刻。
    pub resumed_at: Option<String>,
    pub resume_run_id: Option<String>,
    pub policy_refs: Vec<String>,
    pub tasks: Vec<String>,
    pub cos_runs: Vec<String>,
    pub operations: Vec<String>,
    pub campaign: Option<RequestCampaign>,
}

/// revision（`cos_inbox_items` の 1 行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestRevision {
    pub item_id: String,
    pub source_key: String,
    pub revision: String,
    /// `initial` / `revision` / `returned`（差戻しで作られた item）。
    pub event: &'static str,
    pub created_at: String,
    pub state: String,
    pub reason: Option<String>,
    pub policy_version: String,
    pub thread_id: String,
    pub thread_kind: Option<String>,
    /// この item を取った CoS の run（担当の再起動前の run も含む。取った順）。
    pub run_ids: Vec<String>,
    pub operation_id: Option<String>,
    pub operation_state: Option<String>,
    /// 人への経路（`escalation` / `fallback`）。無ければ `null`。
    pub route: Option<String>,
    pub routed_at: Option<String>,
    pub resolved: Option<ActorAt>,
}

/// 差戻し 1 件（人が CoS の operation を覆した）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestReturn {
    pub at: String,
    pub operation_id: String,
    pub operation_state: String,
    pub from_item_id: String,
    pub to_item_id: Option<String>,
}

/// 時刻と主体（A3 の `InterventionActorKind` の語彙）。`source` は元の記録の参照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActorAt {
    pub at: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub source: String,
}

/// 人を待った区間（人への経路の作成から、その revision の解決まで）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HumanWaitSpan {
    pub item_id: String,
    pub revision: String,
    pub route: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub end_actor_kind: Option<String>,
    pub end_actor_id: Option<String>,
}

/// 関係 task の campaign 識別。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestCampaign {
    pub task_id: String,
    pub campaign_id: String,
    pub case_id: Option<String>,
    pub arm: Option<String>,
    pub repetition: Option<u32>,
}
