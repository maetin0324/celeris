//! ADR 2026-11-04-project-controller A6: 案件の controller の予算（`[project_controller.budget]`）と、その使った量の
//! 数え方。判定は Rust（D7「予算の判定」）。どの guard が効くかの順は `task_dispatch` の `project_guard` が持つ。
//!
//! - 1 日は UTC の日付（`snapshot` の `cos_inputs_remaining_today` と同じ）。
//! - CoS 入力・起票は判断の記録（`project_controller_decisions`）の `applied` の効果だけを数える
//!   （`suppressed`・`failed` は LLM も root も起こしていない）。
//! - token は案件の task run と案件 CoS chat run の和で、`tree_metrics::RollupMetrics.tokens` と同じ規則（input + output、cache を
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
    /// 1 日の CoS 入力（`queue_cos_input`）と案件 CoS chat の起動数、それぞれの上限。
    pub max_cos_inputs_per_day: u32,
    /// 1 日の起票（`create_root`）の数。
    pub max_roots_per_day: u32,
    /// 進捗の増えない効果つきの判断の連続の上限（TS の判定の後の `runaway_guard`）。
    pub max_no_progress_decisions: u32,
    /// 同じ失敗（失敗の理由の署名が同じ root の失敗）の回数の上限。この回数目の失敗で止まる。
    pub max_same_failure_repeats: u32,
    /// TS の proposal が検証に落ちた判断（`pc/invalid:*`）の連続の上限（LLM・TS の品質の guard）。
    pub max_invalid_decisions: u32,
    /// 1 日の token（案件の task run と CoS chat run の和）。
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
    /// 実際に起動した案件 CoS の chat run（controller 以外の入力も含む）。
    pub chat_starts: u32,
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
    usage_on_day(store, project_id, &day_of(now), now)
}

/// `day`（`YYYY-MM-DD`、UTC）の使った量を数える。過ぎた日も同じ規則で数え直せる（止めた後に流れ込んだ分を含む）。
pub fn usage_on_day(
    store: &SqliteStore,
    project_id: &str,
    day: &str,
    now: OffsetDateTime,
) -> Result<BudgetUsage, StoreError> {
    let mut u = BudgetUsage {
        day: day.to_owned(),
        ..BudgetUsage::default()
    };
    for d in store.project_controller_decisions(project_id, None, DECISIONS_SCANNED)? {
        if d.effect_state != EffectState::Applied {
            continue;
        }
        let at = d.applied_at.as_deref().unwrap_or(&d.created_at);
        if !at.starts_with(day) {
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
    let end = next_day_start(day, now);
    u.chat_starts = store.project_chat_starts_between(project_id, &start, &end)?;
    u.tokens = store
        .project_run_usage_between(project_id, &start, &end)?
        .iter()
        .map(|x| x.input_tokens.unwrap_or(0) + x.output_tokens.unwrap_or(0))
        .sum();
    Ok(u)
}

/// `day` の次の日の 00:00:00Z。`day` が読めなければ `now` の次の日（読めない値で範囲を広げない）。
fn next_day_start(day: &str, now: OffsetDateTime) -> String {
    let date = OffsetDateTime::parse(&format!("{day}T00:00:00Z"), &Rfc3339)
        .map(OffsetDateTime::date)
        .unwrap_or_else(|_| now.to_offset(time::UtcOffset::UTC).date());
    format!("{}T00:00:00Z", date.next_day().unwrap_or(date))
}

/// 次の UTC の 00:00 までの秒（最小 1）。日次予算の止まりの期限。
pub fn secs_until_next_day(now: OffsetDateTime) -> u64 {
    let utc = now.to_offset(time::UtcOffset::UTC);
    let next = utc
        .date()
        .next_day()
        .unwrap_or(utc.date())
        .midnight()
        .assume_utc();
    u64::try_from((next - utc).whole_seconds())
        .unwrap_or(0)
        .max(1)
}

/// 日次予算の超過の分類。`Daily` は翌日に再評価してよく、`Dangerous` は人が見るまで止まったままにする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetSeverity {
    Daily,
    Dangerous,
}

/// 危険とみなす token の超過の倍率（上限の 2 倍以上を 1 日で使った）。
pub const DANGEROUS_TOKEN_FACTOR: u64 = 2;
/// 人の操作が無いまま、翌日の自動の再評価を行える回数（直近 7 日）。
pub const MAX_AUTO_DAILY_RESUMES: usize = 3;

/// 超過の分類。controller は上限で止めるので、回数の上限を超えた値・token の上限の 2 倍以上は「上限が効かなかった」
/// 出来事で、日付が変わっただけでは戻さない。CoS chat の起動は人の発言でも増えるので 2 倍で見る。
pub fn severity(limits: &BudgetLimits, u: &BudgetUsage) -> BudgetSeverity {
    let chat_cap = u64::from(limits.max_cos_inputs_per_day) * DANGEROUS_TOKEN_FACTOR;
    if u.tokens
        >= limits
            .max_tokens_per_day
            .saturating_mul(DANGEROUS_TOKEN_FACTOR)
        || u.cos_inputs > limits.max_cos_inputs_per_day
        || u.roots > limits.max_roots_per_day
        || u64::from(u.chat_starts) >= chat_cap
    {
        BudgetSeverity::Dangerous
    } else {
        BudgetSeverity::Daily
    }
}

/// 使い切った予算（最初に当たったもの）。上限ちょうどで止まる（`>=`）。
pub fn exceeded(limits: &BudgetLimits, u: &BudgetUsage) -> Option<BudgetKind> {
    if u.cos_inputs >= limits.max_cos_inputs_per_day
        || u.chat_starts >= limits.max_cos_inputs_per_day
    {
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
    fn p1g_chat_tokens_and_starts_are_project_scoped_and_daily() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget.db");
        let store = SqliteStore::open(&path).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO chat_threads (id,kind,title,project_id,status,created_at,updated_at)
             VALUES ('pc','project','project','p','open','2026-11-05T00:00:00Z','2026-11-05T00:00:00Z'),
                    ('other','project','other','q','open','2026-11-05T00:00:00Z','2026-11-05T00:00:00Z');"
        ).unwrap();
        for (id, thread, at, tokens) in [
            ("today", "pc", "2026-11-05T01:00:00Z", 1500),
            ("yesterday", "pc", "2026-11-04T01:00:00Z", 50000),
            ("other", "other", "2026-11-05T01:00:00Z", 50000),
        ] {
            conn.execute("INSERT INTO chat_runs (run_id,thread_id,input_message_id,state,started_at,usage_json) VALUES (?1,?2,'input','completed',?3,?4)", rusqlite::params![id, thread, at, format!("{{\"input_tokens\":{tokens},\"output_tokens\":100}}")]).unwrap();
        }
        let now = OffsetDateTime::parse("2026-11-05T12:00:00Z", &Rfc3339).unwrap();
        let u = usage(&store, "p", now).unwrap();
        assert_eq!(u.tokens, 1600);
        assert_eq!(u.chat_starts, 1);
        assert_eq!(u.cos_inputs, 0);
        let start_limit = BudgetLimits {
            max_cos_inputs_per_day: 1,
            ..BudgetLimits::default()
        };
        assert_eq!(exceeded(&start_limit, &u), Some(BudgetKind::CosInputs));
        let limits = BudgetLimits {
            max_tokens_per_day: 1000,
            ..BudgetLimits::default()
        };
        assert_eq!(exceeded(&limits, &u), Some(BudgetKind::Tokens));
        assert_eq!(
            usage(&store, "p", now + time::Duration::days(1))
                .unwrap()
                .tokens,
            0
        );
    }

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
