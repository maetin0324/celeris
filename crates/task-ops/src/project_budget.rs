//! ADR 2026-11-04-project-controller A6: 案件の controller の予算（`[project_controller.budget]`）と、その使った量の
//! 数え方。判定は Rust（D7「予算の判定」）。どの guard が効くかの順は `task_dispatch` の `project_guard` が持つ。
//!
//! - 1 日は UTC の日付（`snapshot` の `cos_inputs_remaining_today` と同じ）。
//! - CoS 入力・起票は判断の記録（`project_controller_decisions`）の `applied` の効果だけを数える
//!   （`suppressed`・`failed` は LLM も root も起こしていない）。
//! - token は案件の task の run の和で、`tree_metrics::RollupMetrics.tokens` と同じ規則（input + output、cache を
//!   含めない）。

use serde::Serialize;
use sha2::{Digest, Sha256};
use task_core::StoreError;
use task_core::project_wake::EffectState;
use task_core::store::SqliteStore;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 判断の履歴を数える件数（新しい順）。1 日の上限より十分に大きい。
const DECISIONS_SCANNED: usize = 512;

/// `[project_controller.budget]` の実効値。どれも 0 は「上限なし」ではなく設定の誤り（config が拒否する）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetLimits {
    /// 1 日の CoS 入力（案件 CoS の run を起こす `queue_cos_input`）の数。
    pub max_cos_inputs_per_day: u32,
    /// 1 日の起票（`create_root`）の数。
    pub max_roots_per_day: u32,
    /// 進捗の増えない効果つきの判断の連続の上限（TS の判定の後の `runaway_guard`）。
    pub max_no_progress_decisions: u32,
    /// 同じ失敗（失敗の理由の署名が同じ root の失敗）の回数の上限。この回数目の失敗で止まる。
    pub max_same_failure_repeats: u32,
    /// TS の proposal が検証に落ちた判断（`pc/invalid:*`）の連続の上限（LLM・TS の品質の guard）。
    pub max_invalid_decisions: u32,
    /// 1 日の token（案件の task の run の和）。
    pub max_tokens_per_day: u64,
    /// 外からの入力（cron 以外の wake）が無いまま `waiting` でいられる秒。過ぎたら人に 1 回聞いて止まる。
    pub ttl_secs: u64,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        let snapshot = task_core::project_proposal::SnapshotLimits::default();
        Self {
            max_cos_inputs_per_day: snapshot.cos_inputs_remaining_today,
            max_roots_per_day: snapshot.roots_remaining_today,
            max_no_progress_decisions: snapshot.max_no_progress_decisions,
            max_same_failure_repeats: 3,
            max_invalid_decisions: 3,
            max_tokens_per_day: 20_000_000,
            ttl_secs: 3 * 24 * 3600,
        }
    }
}

/// 予算の種類（guard の名と問いの key に使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    CosInputs,
    Roots,
    Tokens,
}

impl BudgetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetKind::CosInputs => "cos_inputs",
            BudgetKind::Roots => "roots",
            BudgetKind::Tokens => "tokens",
        }
    }
}

/// 今日（UTC）使った量。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BudgetUsage {
    /// `YYYY-MM-DD`（UTC）。
    pub day: String,
    pub cos_inputs: u32,
    pub roots: u32,
    pub tokens: u64,
}

/// `YYYY-MM-DD`（UTC）。
pub fn day_of(now: OffsetDateTime) -> String {
    now.to_offset(time::UtcOffset::UTC).date().to_string()
}

/// 今日（UTC）の使った量を数える。
pub fn usage(
    store: &SqliteStore,
    project_id: &str,
    now: OffsetDateTime,
) -> Result<BudgetUsage, StoreError> {
    let day = day_of(now);
    let mut u = BudgetUsage {
        day: day.clone(),
        ..BudgetUsage::default()
    };
    for d in store.project_controller_decisions(project_id, None, DECISIONS_SCANNED)? {
        if d.effect_state != EffectState::Applied {
            continue;
        }
        let at = d.applied_at.as_deref().unwrap_or(&d.created_at);
        if !at.starts_with(&day) {
            continue;
        }
        let kind = serde_json::from_str::<serde_json::Value>(&d.effect_json)
            .ok()
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned));
        match kind.as_deref() {
            Some("queue_cos_input") => u.cos_inputs += 1,
            Some("create_root") => u.roots += 1,
            _ => {}
        }
    }
    let start = format!("{day}T00:00:00Z");
    u.tokens = store
        .project_run_usage_since(project_id, &start)?
        .iter()
        .map(|x| x.input_tokens.unwrap_or(0) + x.output_tokens.unwrap_or(0))
        .sum();
    Ok(u)
}

/// 使い切った予算（最初に当たったもの）。上限ちょうどで止まる（`>=`）。
pub fn exceeded(limits: &BudgetLimits, u: &BudgetUsage) -> Option<BudgetKind> {
    if u.cos_inputs >= limits.max_cos_inputs_per_day {
        Some(BudgetKind::CosInputs)
    } else if u.roots >= limits.max_roots_per_day {
        Some(BudgetKind::Roots)
    } else if u.tokens >= limits.max_tokens_per_day {
        Some(BudgetKind::Tokens)
    } else {
        None
    }
}

/// 失敗の署名: wake の種類と、理由の文の空白を畳み連続する数字を 1 個の `#` にした先頭 200 文字の sha256 の先頭 16 桁。
/// 理由の文は判定の値に使わない（I11）: 同じかどうかだけを見る。
pub fn failure_signature(kind: &str, reason: &str) -> String {
    let mut norm = String::new();
    for word in reason.split_whitespace() {
        if !norm.is_empty() {
            norm.push(' ');
        }
        let mut prev_digit = false;
        for c in word.chars() {
            let digit = c.is_ascii_digit();
            if !(digit && prev_digit) {
                norm.push(if digit { '#' } else { c });
            }
            prev_digit = digit;
        }
    }
    let norm: String = norm.chars().take(200).collect();
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"\n");
    h.update(norm.as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_owned()
}

/// `at` から `ttl_secs` 以上たったか（`at` が読めなければ `false`）。
pub fn ttl_expired(at: &str, ttl_secs: u64, now: OffsetDateTime) -> bool {
    OffsetDateTime::parse(at, &Rfc3339)
        .map(|t| (now - t).whole_seconds() >= ttl_secs as i64)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_budget_signature_ignores_numbers_and_spacing() {
        let a = failure_signature("root_failed", "check failed: exit 1 after  12s");
        let b = failure_signature("root_failed", "check failed: exit 2 after 9s");
        let c = failure_signature("root_failed", "build failed");
        let d = failure_signature("review_failed", "check failed: exit 1 after 12s");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn project_budget_exceeded_at_the_limit() {
        let l = BudgetLimits {
            max_cos_inputs_per_day: 2,
            ..BudgetLimits::default()
        };
        let mut u = BudgetUsage::default();
        assert_eq!(exceeded(&l, &u), None);
        u.cos_inputs = 2;
        assert_eq!(exceeded(&l, &u), Some(BudgetKind::CosInputs));
        u.cos_inputs = 0;
        u.tokens = l.max_tokens_per_day;
        assert_eq!(exceeded(&l, &u), Some(BudgetKind::Tokens));
    }
}
