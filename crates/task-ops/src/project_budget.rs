//! ADR 2026-11-04-project-controller A6: 案件の controller の予算（`[project_controller.budget]`）と、その使った量の
//! 数え方。判定は Rust（D7「予算の判定」）。どの guard が効くかの順は `task_dispatch` の `project_guard` が持つ。
//!
//! - 1 日は UTC の日付（`snapshot` の `cos_inputs_remaining_today` と同じ）。
//! - CoS 入力・起票は判断の記録（`project_controller_decisions`）の `applied` の効果だけを数える
//!   （`suppressed`・`failed` は LLM も root も起こしていない）。
//! - 上限は案件の task run と案件 CoS chat run の費用の和（ADR 2026-11-06-budget-usd-caps D1。run ごとに model と usage から
//!   `task_core::pricing::budget_cost` で micro-USD に換算し、cache read・cache write はそれぞれの単価で数える）。単価が引けない
//!   model と usage の無い終わった run は費用不明で、1 件でもあれば `BudgetKind::UsdUnknown` で止める（D2、fail-closed）。
//!   `tokens` は表示用（input + output）で判定に使わない。

use serde::Serialize;
use sha2::{Digest, Sha256};
use task_core::StoreError;
use task_core::pricing::{BudgetRates, budget_cost};
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
    /// 1 日の費用の上限（micro-USD。案件の task run と CoS chat run の換算費用の和）。
    pub max_usd_per_day_micros: u64,
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
            max_usd_per_day_micros: 100_000_000,
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
    /// 換算費用の和が上限に達した。
    Usd,
    /// 費用不明の run（単価が引けない model・usage の無い run）がある。
    UsdUnknown,
}

impl BudgetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetKind::CosInputs => "cos_inputs",
            BudgetKind::Roots => "roots",
            BudgetKind::Usd => "usd",
            BudgetKind::UsdUnknown => "usd_unknown",
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
    /// 表示用の token（input + output。cache を含めない）。判定には使わない。
    pub tokens: u64,
    /// 換算費用の和（micro-USD）。
    pub usd_micros: u64,
    /// 費用不明の run の数（1 以上なら `UsdUnknown` で止める）。
    pub usd_unknown_runs: u32,
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
    rates: &BudgetRates,
) -> Result<BudgetUsage, StoreError> {
    usage_on_day(store, project_id, &day_of(now), now, rates)
}

/// `day`（`YYYY-MM-DD`、UTC）の使った量を数える。過ぎた日も同じ規則で数え直せる（止めた後に流れ込んだ分を含む）。
pub fn usage_on_day(
    store: &SqliteStore,
    project_id: &str,
    day: &str,
    now: OffsetDateTime,
    rates: &BudgetRates,
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
    let runs = store.project_run_usage_between(project_id, &start, &end)?;
    u.tokens = runs
        .iter()
        .filter_map(|x| x.usage.as_ref())
        .map(|x| x.input_tokens.unwrap_or(0) + x.output_tokens.unwrap_or(0))
        .sum();
    let cost = budget_cost(
        runs.iter().map(|x| (x.model.as_deref(), x.usage.as_ref())),
        rates,
    );
    u.usd_micros = cost.usd_micros;
    u.usd_unknown_runs = cost.unknown_runs;
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

/// 危険とみなす費用の超過の倍率（上限の 2 倍以上を 1 日で使った）。
pub const DANGEROUS_USD_FACTOR: u64 = 2;
/// 人の操作が無いまま、翌日の自動の再評価を行える回数（直近 7 日）。
pub const MAX_AUTO_DAILY_RESUMES: usize = 3;

/// 超過の分類。controller は上限で止めるので、回数の上限を超えた値・費用の上限の 2 倍以上・費用不明の run は「上限が効かなかった」
/// 出来事で、日付が変わっただけでは戻さない。CoS chat の起動は人の発言でも増えるので 2 倍で見る。
pub fn severity(limits: &BudgetLimits, u: &BudgetUsage) -> BudgetSeverity {
    let chat_cap = u64::from(limits.max_cos_inputs_per_day) * DANGEROUS_USD_FACTOR;
    if u.usd_unknown_runs > 0
        || u.usd_micros
            >= limits
                .max_usd_per_day_micros
                .saturating_mul(DANGEROUS_USD_FACTOR)
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
    } else if u.usd_micros >= limits.max_usd_per_day_micros {
        Some(BudgetKind::Usd)
    } else if u.usd_unknown_runs > 0 {
        Some(BudgetKind::UsdUnknown)
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
#[path = "project_budget_tests.rs"]
mod tests;
