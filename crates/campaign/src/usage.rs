//! 7 日使用率の確認（`celeris-campaign usage`）。ADR 2026-10-17-mlsys-pilot2-fixes D5。
//! A は Celeris 管理の claude_oauth アカウントで動くので、その消費は `GET /api/v1/llm/accounts` の 7 日使用率に出る。
//! ここでは各アカウントの 7 日使用率と、campaign の腕ごとの tokens（`summary.json`、A を含む）を並べ、
//! 上限（claude 既定 45%・codex 既定 21%）を超えるアカウントを違反にする。
use serde_json::{Value, json};

pub const DEFAULT_MAX_7D_CLAUDE: f64 = 0.45;
pub const DEFAULT_MAX_7D_CODEX: f64 = 0.21;

/// (報告, 違反の一覧)。使用率が観測できないアカウントは 0 にせず `null` とし、違反にも数えない
/// （A のアカウントだけは観測が無いこと自体を違反にする。A の消費を確認できないまま進めない）。
pub fn report(
    accounts: &Value,
    summary: &Value,
    a_account: &str,
    max_claude: f64,
    max_codex: f64,
) -> (Value, Vec<String>) {
    let mut violations = Vec::new();
    let mut rows = Vec::new();
    let mut a_seen = false;
    // A が有効計画に無い（B/C のみ）なら A のアカウントは要求も検査もしない。
    let a_active = summary["cells"].get("A").is_some();
    for x in accounts["items"].as_array().into_iter().flatten() {
        let (src, id) = (
            x["source_id"].as_str().unwrap_or(""),
            x["account_id"].as_str().unwrap_or(""),
        );
        let util = x["usage"]["seven_day"]["utilization"].as_f64();
        let limit = match src {
            "claude_oauth" => Some(max_claude),
            "codex_oauth" => Some(max_codex),
            _ => None,
        };
        let is_a = a_active && src == "claude_oauth" && id == a_account;
        a_seen |= is_a;
        if let (Some(u), Some(l)) = (util, limit)
            && u > l
        {
            violations.push(format!("{src}/{id}: 7-day utilization {u} exceeds {l}"));
        }
        if is_a && util.is_none() {
            violations.push(format!(
                "{src}/{id}: 7-day utilization is not observed; A's consumption cannot be confirmed"
            ));
        }
        rows.push(json!({
            "source_id": src, "account_id": id, "seven_day_utilization": util,
            "observed_at": x["usage"]["observed_at"], "limit": limit, "a_arm": is_a,
        }));
    }
    if a_active && !a_seen {
        violations.push(format!(
            "claude_oauth/{a_account}: not in GET /llm/accounts"
        ));
    }
    let mut arms = serde_json::Map::new();
    for (arm, cell) in summary["cells"].as_object().into_iter().flatten() {
        let t = &cell["tokens"];
        let sum: u64 = ["input", "output", "cache_read", "cache_creation"]
            .iter()
            .filter_map(|k| t[*k].as_u64())
            .sum();
        arms.insert(
            arm.clone(),
            json!({"tokens_all": sum, "usage_missing_trials": t["usage_missing_trials"]}),
        );
    }
    (
        json!({
            "schema": "celeris.campaign-usage/1",
            "ok": violations.is_empty(),
            "a_account": a_account,
            "accounts": rows,
            "campaign_tokens_by_arm": arms,
            "violations": violations,
        }),
        violations,
    )
}
