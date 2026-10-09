//! ADR-0131 D1〜D3: 汎用の定期実行（cron job）の型と、発火時刻を求める純関数。
//!
//! - [`CronSchedule`] — 5 欄（分 時 日 月 曜日）の cron 式。`*`・数値・範囲・列挙・刻み・月/曜日の英略称・
//!   `@daily` 等の別名だけを受ける（秒欄・`@reboot`・`L`/`W`/`#` は持たない）。
//! - [`CronTz`] — job のタイムゾーン（IANA 名。試験では POSIX TZ 文字列でも作れる）。
//! - [`next_after`] — 与えた時刻より後の最初の発火時刻。
//! - [`due_fires`] — `next_fire_at` から `now` までに過ぎた発火時刻（最新の 1 件と件数）。
//!
//! 時計はすべて引数で受ける（`OffsetDateTime::now_utc()` をこの module では読まない。ADR-0131 D9）。
//! 永続化は `store::cron`（`cron_jobs` / `cron_job_runs`、migration 0039）。

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::model::{TaskId, Tier};

mod store;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

pub use store::{
    CronJobRunUpdate, CronJobStore, cron_job_delete_tx, cron_job_insert_tx, cron_job_run_update_tx,
    cron_job_update_tx,
};

/// `next_after` の探索を打ち切る先（年）。ADR-0131 D1「探索は 5 年先で打ち切り」。
pub const SEARCH_YEARS: i16 = 5;
/// `due_fires` が数える過ぎた発火の上限。ADR-0131 D3「件数は `next_after` を最大 1000 回で打ち切る」。
pub const MISSED_COUNT_CAP: u32 = 1000;

/// cron が作った task に必ず付く label（`task_ops::cron_jobs::CRON_TASK_LABEL` はこの値を
/// re-export する。task-ops は task-core に依存するが逆はできないため、正本をここに置く）。
pub const CRON_TASK_LABEL: &str = "cron";

/// ADR-0131 の日次整理・全体整理 task の harness id（cron の `template.harness` がそのまま
/// task の `genre` になる。`task_ops::knowledge_curation::CURATION_HARNESS` はこの値を
/// re-export する）。ADR-0131 付記（2026-10-04、Complexity Gate 例外）: この harness かつ
/// [`CRON_TASK_LABEL`] を持つ task は `execution_gate::out_of_scope_rule` で常に atomic に
/// する（daemon の検証・適用が task 直下の `artifacts/curation-plan.json` を前提にするため、
/// planner が WorkUnit に分けると成果物が反映されない）。
pub const KNOWLEDGE_CURATION_HARNESS: &str = "knowledge-curation";

// ---- 識別子 ----

/// cron job 1 件の識別子（ULID）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct CronJobId(#[schemars(with = "String")] pub Ulid);

impl CronJobId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for CronJobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CronJobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CronJobId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Ulid::from_string(s)?))
    }
}

/// 実行履歴 1 行の識別子（ULID）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct CronJobRunId(#[schemars(with = "String")] pub Ulid);

impl CronJobRunId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for CronJobRunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CronJobRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CronJobRunId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Ulid::from_string(s)?))
    }
}

// ---- job の規則（D2・D3）----

/// ADR-0131 D2: 前回の task が終わっていないときの規則。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CronOverlap {
    /// 作らずに `skipped_overlap` を記録する（既定）。
    #[default]
    Skip,
    /// `queued` を高々 1 件溜め、前回が終端になった後に作る。
    Queue,
}

impl CronOverlap {
    pub fn as_str(self) -> &'static str {
        match self {
            CronOverlap::Skip => "skip",
            CronOverlap::Queue => "queue",
        }
    }
}

impl FromStr for CronOverlap {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "skip" => Ok(CronOverlap::Skip),
            "queue" => Ok(CronOverlap::Queue),
            other => Err(CronError::Invalid(format!(
                "unknown overlap {other:?} (expected skip|queue)"
            ))),
        }
    }
}

/// ADR-0131 D3: daemon 停止中に過ぎた予定時刻の扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CronCatchUp {
    /// 過ぎた中で最新の 1 件だけ発火する（既定）。
    #[default]
    Latest,
    /// 過ぎた分は何も作らない。
    Skip,
}

impl CronCatchUp {
    pub fn as_str(self) -> &'static str {
        match self {
            CronCatchUp::Latest => "latest",
            CronCatchUp::Skip => "skip",
        }
    }
}

impl FromStr for CronCatchUp {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "latest" => Ok(CronCatchUp::Latest),
            "skip" => Ok(CronCatchUp::Skip),
            other => Err(CronError::Invalid(format!(
                "unknown catch_up {other:?} (expected latest|skip)"
            ))),
        }
    }
}

/// ADR-0131 D1: 履歴 1 行の発火のきっかけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CronTrigger {
    Schedule,
    CatchUp,
    Manual,
}

impl CronTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            CronTrigger::Schedule => "schedule",
            CronTrigger::CatchUp => "catch_up",
            CronTrigger::Manual => "manual",
        }
    }
}

impl FromStr for CronTrigger {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "schedule" => Ok(CronTrigger::Schedule),
            "catch_up" => Ok(CronTrigger::CatchUp),
            "manual" => Ok(CronTrigger::Manual),
            other => Err(CronError::Invalid(format!(
                "unknown cron trigger {other:?}"
            ))),
        }
    }
}

/// ADR-0131 D1: 履歴 1 行の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CronRunOutcome {
    Created,
    Queued,
    SkippedOverlap,
    SkippedMissed,
    Error,
}

impl CronRunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            CronRunOutcome::Created => "created",
            CronRunOutcome::Queued => "queued",
            CronRunOutcome::SkippedOverlap => "skipped_overlap",
            CronRunOutcome::SkippedMissed => "skipped_missed",
            CronRunOutcome::Error => "error",
        }
    }
}

impl FromStr for CronRunOutcome {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(CronRunOutcome::Created),
            "queued" => Ok(CronRunOutcome::Queued),
            "skipped_overlap" => Ok(CronRunOutcome::SkippedOverlap),
            "skipped_missed" => Ok(CronRunOutcome::SkippedMissed),
            "error" => Ok(CronRunOutcome::Error),
            other => Err(CronError::Invalid(format!(
                "unknown cron outcome {other:?}"
            ))),
        }
    }
}

// ---- job 雛形と行 ----

/// ADR-0131 D1: job が作る task の雛形（`cron_jobs.template_json`）。`acceptance` は `task_ops::add` の
/// `CriterionSpec` と同じ JSON の形で持ち、検証と `NewTaskSpec` への変換は task-ops 側で行う
/// （task-core は task-ops に依存しない）。`extra` は job 固有の値（日次整理の `mode` 等）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct CronTaskTemplate {
    /// `{date}` は発火時刻の job タイムゾーンでの `YYYY-MM-DD` に置き換わる（[`render_title`]）。
    pub title: String,
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub acceptance: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// `cron_jobs` の 1 行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CronJob {
    pub id: CronJobId,
    pub name: String,
    pub enabled: bool,
    pub schedule: String,
    pub timezone: String,
    pub overlap: CronOverlap,
    pub catch_up: CronCatchUp,
    pub template: CronTaskTemplate,
    /// UTC。`enabled = false` のとき `None`。
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>")]
    pub next_fire_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub updated_at: OffsetDateTime,
}

impl CronJob {
    /// schedule とタイムゾーンを解いて、`after` より後の次回時刻を返す（作成・更新・再開で使う）。
    pub fn compute_next_after(
        &self,
        after: OffsetDateTime,
    ) -> Result<Option<OffsetDateTime>, CronError> {
        let schedule: CronSchedule = self.schedule.parse()?;
        let tz = CronTz::iana(&self.timezone)?;
        next_after(&schedule, &tz, after)
    }
}

/// `cron_job_runs` の 1 行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CronJobRun {
    pub id: CronJobRunId,
    pub job_id: CronJobId,
    /// 発火の予定時刻（UTC）。手動実行は押した時刻。
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub scheduled_for: OffsetDateTime,
    pub trigger: CronTrigger,
    pub outcome: CronRunOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub recorded_at: OffsetDateTime,
}

/// cron の式・タイムゾーン・雛形の誤り（API では 400 に写す）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronError {
    #[error("invalid cron expression {expr:?}: {reason}")]
    Expression { expr: String, reason: String },
    #[error("unknown time zone {name:?}: {reason}")]
    TimeZone { name: String, reason: String },
    #[error("time out of range: {0}")]
    Range(String),
    #[error("{0}")]
    Invalid(String),
}

// ---- タイムゾーン ----

/// job のタイムゾーン。名前（DB の `timezone`）と解いた [`TimeZone`] を持つ。
#[derive(Debug, Clone)]
pub struct CronTz {
    name: String,
    tz: TimeZone,
}

impl CronTz {
    /// IANA 名（例 `Asia/Tokyo`・`UTC`）。host の `/usr/share/zoneinfo` を読む。
    pub fn iana(name: &str) -> Result<Self, CronError> {
        let tz = TimeZone::get(name).map_err(|e| CronError::TimeZone {
            name: name.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self {
            name: name.to_string(),
            tz,
        })
    }

    /// POSIX TZ 文字列（例 `EST5EDT,M3.2.0,M11.1.0`）。host の tzdata に依存しない試験用（ADR-0131 D9）。
    pub fn posix(spec: &str) -> Result<Self, CronError> {
        let tz = TimeZone::posix(spec).map_err(|e| CronError::TimeZone {
            name: spec.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self {
            name: spec.to_string(),
            tz,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// `t` の job タイムゾーンでの日付 `YYYY-MM-DD`（雛形の `{date}`）。
    pub fn local_date(&self, t: OffsetDateTime) -> Result<String, CronError> {
        let d = to_jiff(t)?.to_zoned(self.tz.clone()).date();
        Ok(format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day()))
    }
}

/// 雛形の title の `{date}` を発火時刻の job タイムゾーンの日付に置き換える。
pub fn render_title(
    title: &str,
    tz: &CronTz,
    fired_at: OffsetDateTime,
) -> Result<String, CronError> {
    if !title.contains("{date}") {
        return Ok(title.to_string());
    }
    Ok(title.replace("{date}", &tz.local_date(fired_at)?))
}

// ---- cron 式 ----

/// 1 欄の値の集合（bit i = 値 i）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Field {
    bits: u64,
    /// 欄の文字列が `*` で始まる（Vixie cron の DOM_STAR/DOW_STAR。日と曜日の OR 判定に使う）。
    star: bool,
}

impl Field {
    fn has(&self, v: i8) -> bool {
        v >= 0 && (self.bits >> v) & 1 == 1
    }

    /// `from` 以上で最初に含む値（`max` まで）。
    fn next_from(&self, from: i8, max: i8) -> Option<i8> {
        (from.max(0)..=max).find(|v| self.has(*v))
    }
}

/// 5 欄の cron 式（ADR-0131 D1）。`FromStr` で parse する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    expr: String,
    minute: Field,
    hour: Field,
    dom: Field,
    month: Field,
    /// 0 = 日曜（7 は 0 に寄せる）。
    dow: Field,
}

const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DOW_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

impl CronSchedule {
    /// parse したときの元の式。
    pub fn expr(&self) -> &str {
        &self.expr
    }

    fn day_matches(&self, date: Date) -> bool {
        let dom = self.dom.has(date.day());
        let dow = self.dow.has(date.weekday().to_sunday_zero_offset());
        // Vixie cron: 日と曜日が両方とも制限されていれば OR、どちらかが `*` で始まれば AND。
        if self.dom.star || self.dow.star {
            dom && dow
        } else {
            dom || dow
        }
    }
}

impl fmt::Display for CronSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.expr)
    }
}

impl FromStr for CronSchedule {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let expr = s.trim();
        let err = |reason: String| CronError::Expression {
            expr: expr.to_string(),
            reason,
        };
        let expanded = match expr {
            "@yearly" | "@annually" => "0 0 1 1 *",
            "@monthly" => "0 0 1 * *",
            "@weekly" => "0 0 * * 0",
            "@daily" | "@midnight" => "0 0 * * *",
            "@hourly" => "0 * * * *",
            other if other.starts_with('@') => {
                return Err(err(format!("unsupported alias {other}")));
            }
            other => other,
        };
        let parts: Vec<&str> = expanded.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(err(format!(
                "expected 5 fields (minute hour day month weekday), got {}",
                parts.len()
            )));
        }
        let minute = parse_field(parts[0], 0, 59, None).map_err(|r| err(format!("minute: {r}")))?;
        let hour = parse_field(parts[1], 0, 23, None).map_err(|r| err(format!("hour: {r}")))?;
        let dom = parse_field(parts[2], 1, 31, None).map_err(|r| err(format!("day: {r}")))?;
        let month = parse_field(parts[3], 1, 12, Some((&MONTH_NAMES, 1)))
            .map_err(|r| err(format!("month: {r}")))?;
        let mut dow = parse_field(parts[4], 0, 7, Some((&DOW_NAMES, 0)))
            .map_err(|r| err(format!("weekday: {r}")))?;
        if dow.has(7) {
            dow.bits = (dow.bits & !(1 << 7)) | 1;
        }
        let schedule = CronSchedule {
            expr: expr.to_string(),
            minute,
            hour,
            dom,
            month,
            dow,
        };
        if !schedule.can_fire() {
            return Err(err("the day/month combination never occurs".to_string()));
        }
        Ok(schedule)
    }
}

impl CronSchedule {
    /// 日と月の組み合わせが暦の上で存在するか（例 `0 0 31 2 *` は存在しない）。曜日で発火しうるなら真。
    fn can_fire(&self) -> bool {
        if !self.dow.star {
            // 曜日が制限されていれば、日が `*` なら AND（その曜日は毎月ある）、日も制限なら OR。
            return true;
        }
        // 2 月は閏年の 29 日まで数える。
        const MAX_DAYS: [i8; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        (1..=12i8)
            .any(|m| self.month.has(m) && (1..=MAX_DAYS[(m - 1) as usize]).any(|d| self.dom.has(d)))
    }
}

fn parse_value(s: &str, names: Option<(&[&str], i8)>) -> Result<i8, String> {
    if let Ok(v) = s.parse::<i8>() {
        return Ok(v);
    }
    if let Some((names, offset)) = names {
        let lower = s.to_ascii_lowercase();
        if let Some(i) = names.iter().position(|n| *n == lower) {
            return Ok(i as i8 + offset);
        }
    }
    Err(format!("bad value {s:?}"))
}

fn parse_field(
    text: &str,
    min: i8,
    max: i8,
    names: Option<(&[&str], i8)>,
) -> Result<Field, String> {
    let mut bits = 0u64;
    for item in text.split(',') {
        if item.is_empty() {
            return Err(format!("empty list item in {text:?}"));
        }
        let (range, step) = match item.split_once('/') {
            Some((r, s)) => {
                let step: i8 = s.parse().map_err(|_| format!("bad step {s:?}"))?;
                if step <= 0 {
                    return Err(format!("step must be positive: {s:?}"));
                }
                (r, Some(step))
            }
            None => (item, None),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (parse_value(a, names)?, parse_value(b, names)?)
        } else {
            if step.is_some() {
                return Err(format!("step needs a range or *: {item:?}"));
            }
            let v = parse_value(range, names)?;
            (v, v)
        };
        if lo < min || hi > max {
            return Err(format!("{item:?} is out of range {min}-{max}"));
        }
        if lo > hi {
            return Err(format!("reversed range {item:?}"));
        }
        let step = step.unwrap_or(1);
        let mut v = lo;
        while v <= hi {
            bits |= 1 << v;
            v = match v.checked_add(step) {
                Some(n) => n,
                None => break,
            };
        }
    }
    Ok(Field {
        bits,
        star: text.starts_with('*'),
    })
}

// ---- 時刻の計算 ----

fn to_jiff(t: OffsetDateTime) -> Result<Timestamp, CronError> {
    Timestamp::from_nanosecond(t.unix_timestamp_nanos())
        .map_err(|e| CronError::Range(e.to_string()))
}

fn from_jiff(t: Timestamp) -> Result<OffsetDateTime, CronError> {
    OffsetDateTime::from_unix_timestamp_nanos(t.as_nanosecond())
        .map_err(|e| CronError::Range(e.to_string()))
}

fn jerr(e: jiff::Error) -> CronError {
    CronError::Range(e.to_string())
}

/// `after` より後（厳密に大きい）の最初の発火時刻（UTC）。5 年先までに無ければ `None`。
///
/// job のタイムゾーンの壁時計で `after` の次の分から、月→日→時→分の順に欄ごとに桁上げして探す。
/// DST（ADR-0131 D1）:
/// - 存在しない壁時計時刻（春の飛び）は飛びの後へずらして 1 回発火する（jiff の `compatible`）。
/// - 重複する壁時計時刻（秋の戻り）は早い方の offset で 1 回だけ。壁時計は単調に進めて探すので、
///   戻った後の同じ壁時計時刻は 2 回目として拾わない（解いた時刻が `after` 以下なら飛ばす）。
pub fn next_after(
    schedule: &CronSchedule,
    tz: &CronTz,
    after: OffsetDateTime,
) -> Result<Option<OffsetDateTime>, CronError> {
    let after_ts = to_jiff(after)?;
    let local = after_ts.to_zoned(tz.tz.clone()).datetime();
    let start = DateTime::from_parts(
        local.date(),
        Time::new(local.hour(), local.minute(), 0, 0).map_err(jerr)?,
    );
    let mut dt = start
        .checked_add(jiff::Span::new().minutes(1))
        .map_err(jerr)?;
    let limit_year = start.year().saturating_add(SEARCH_YEARS);
    loop {
        if dt.year() > limit_year {
            return Ok(None);
        }
        let date = dt.date();
        if !schedule.month.has(date.month()) {
            dt = first_of_next_month(date)?;
            continue;
        }
        if !schedule.day_matches(date) {
            dt = start_of_day(date.tomorrow().map_err(jerr)?);
            continue;
        }
        let Some(hour) = schedule.hour.next_from(dt.hour(), 23) else {
            dt = start_of_day(date.tomorrow().map_err(jerr)?);
            continue;
        };
        if hour != dt.hour() {
            dt = DateTime::from_parts(date, Time::new(hour, 0, 0, 0).map_err(jerr)?);
            continue;
        }
        let Some(minute) = schedule.minute.next_from(dt.minute(), 59) else {
            dt = next_hour(dt)?;
            continue;
        };
        let candidate = DateTime::from_parts(date, Time::new(hour, minute, 0, 0).map_err(jerr)?);
        let ts = tz
            .tz
            .to_ambiguous_timestamp(candidate)
            .compatible()
            .map_err(jerr)?;
        if ts > after_ts {
            return Ok(Some(from_jiff(ts)?));
        }
        // 秋の戻りの後半: この壁時計時刻は早い方の offset で既に過ぎている。
        dt = candidate
            .checked_add(jiff::Span::new().minutes(1))
            .map_err(jerr)?;
    }
}

fn start_of_day(date: Date) -> DateTime {
    DateTime::from_parts(date, Time::midnight())
}

fn first_of_next_month(date: Date) -> Result<DateTime, CronError> {
    let first = date.first_of_month();
    let next = first
        .checked_add(jiff::Span::new().months(1))
        .map_err(jerr)?;
    Ok(start_of_day(next))
}

fn next_hour(dt: DateTime) -> Result<DateTime, CronError> {
    let top = DateTime::from_parts(dt.date(), Time::new(dt.hour(), 0, 0, 0).map_err(jerr)?);
    top.checked_add(jiff::Span::new().hours(1)).map_err(jerr)
}

/// `next_fire_at` から `now` までに過ぎた発火（ADR-0131 D3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueFires {
    /// 過ぎた中で最も古い予定時刻（= 渡した `next_fire_at`）。
    pub earliest: OffsetDateTime,
    /// 過ぎた中で最新の予定時刻（`catch_up = latest` で発火するもの）。
    pub latest: OffsetDateTime,
    /// 過ぎた予定時刻の数（`latest` を含む）。[`MISSED_COUNT_CAP`] で打ち切る。
    pub count: u32,
    /// `count` が上限で打ち切られた。
    pub count_capped: bool,
    /// `now` より後の次回時刻（`next_fire_at` の更新先）。
    pub next: Option<OffsetDateTime>,
}

/// `next_fire_at <= now` のとき、`now` までに過ぎた予定時刻の最新の 1 件と件数を返す。
/// `next_fire_at > now` なら `None`（まだ時刻ではない）。
///
/// 件数は `next_after` を最大 [`MISSED_COUNT_CAP`] 回で打ち切る。打ち切ったときの最新の 1 件は
/// `now` から遡った窓（1 時間・1 日・32 日・366 日・5 年）の中を前から辿って求める（全件は数えない）。
pub fn due_fires(
    schedule: &CronSchedule,
    tz: &CronTz,
    next_fire_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<Option<DueFires>, CronError> {
    if next_fire_at > now {
        return Ok(None);
    }
    let mut latest = next_fire_at;
    let mut count: u32 = 1;
    let mut capped = false;
    loop {
        match next_after(schedule, tz, latest)? {
            Some(t) if t <= now => {
                if count >= MISSED_COUNT_CAP {
                    capped = true;
                    break;
                }
                latest = t;
                count += 1;
            }
            _ => break,
        }
    }
    if capped {
        latest = latest_at_or_before(schedule, tz, latest, now)?;
    }
    let next = next_after(schedule, tz, now)?;
    Ok(Some(DueFires {
        earliest: next_fire_at,
        latest,
        count,
        count_capped: capped,
        next,
    }))
}

/// `floor` より後で `now` 以下の最新の予定時刻（無ければ `floor`）。遡る窓を広げながら探す。
fn latest_at_or_before(
    schedule: &CronSchedule,
    tz: &CronTz,
    floor: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<OffsetDateTime, CronError> {
    const WINDOWS: [time::Duration; 5] = [
        time::Duration::hours(1),
        time::Duration::days(1),
        time::Duration::days(32),
        time::Duration::days(366),
        time::Duration::days(366 * 5),
    ];
    for window in WINDOWS {
        let from = (now - window).max(floor);
        let mut cursor = from;
        let mut found = None;
        while let Some(t) = next_after(schedule, tz, cursor)? {
            if t > now {
                break;
            }
            found = Some(t);
            cursor = t;
        }
        if let Some(t) = found {
            return Ok(t);
        }
        if from == floor {
            break;
        }
    }
    Ok(floor)
}
