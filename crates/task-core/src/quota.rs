//! ADR-0074 D4（Phase F3 quota）: run の quota 消費を推定する純粋関数。
//!
//! I/O・LLM 呼び出しはしない（DESIGN 原則 1 / ADR-0001 D2）。`Event::QuotaEstimated` の材料
//! （`Usage` と、run の前後で観測した `RateLimitObservation`）から、決定的に
//! `measured` / `apportioned` / `estimated` / `unknown` / `free` のどれかを選ぶ。
//!
//! 5 通りの優先順位（D4.2）: measured → apportioned → estimated → unknown。`free`（Qwen などの
//! 定額でも従量でもない供給元）はこの優先順位より前に、呼び出し側が窓を評価する前に決める
//! （そもそも 5 時間/7 日の枠を持たない）。
//!
//! **既知の限界（D4.2 の「既知の限界」節）**: 人が celeris の外で同じアカウントを使うと、
//! `measured`/`apportioned` にその消費が混ざりうる（区別する手段が無い。U-F3）。重み付きトークンの
//! 係数と定額プランの比例仮定は未検証（U-F4）。

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::accounts::RateLimitObservation;
use crate::model::{RunRole, Usage};

/// D4.2: 窓の種別（ADR-0024 と同じ 2 窓）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindow {
    FiveHour,
    SevenDay,
}

impl QuotaWindow {
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaWindow::FiveHour => "five_hour",
            QuotaWindow::SevenDay => "seven_day",
        }
    }
}

/// D4.2: この窓の消費をどうやって決めたか（confidence）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QuotaMethod {
    /// `before`/`after` が両方有効・排他・同じ窓インスタンス。
    Measured,
    /// 重なった run の集合に、重み付きトークンの比で按分。
    Apportioned,
    /// 直近の `measured` から較正した比率 `k_w` にトークン数を掛けた値。
    Estimated,
    /// 上のどれでも決められなかった（`None` であって `0` ではない）。
    Unknown,
    /// Qwen など定額でも従量でもない供給元（`q = 0` だが「消費なし」を意味であって未知ではない）。
    Free,
}

impl QuotaMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaMethod::Measured => "measured",
            QuotaMethod::Apportioned => "apportioned",
            QuotaMethod::Estimated => "estimated",
            QuotaMethod::Unknown => "unknown",
            QuotaMethod::Free => "free",
        }
    }

    /// 5 通りの中で「より確からしい」方を選ぶ（イベントの代表 `method` を複数窓から 1 つに畳み込むため。
    /// D4.3 は windows 配列とは別に 1 つの `method` を持つ）。同点は現在の値を保つ（決定的）。
    fn rank(self) -> u8 {
        match self {
            QuotaMethod::Measured => 0,
            QuotaMethod::Apportioned => 1,
            QuotaMethod::Estimated => 2,
            QuotaMethod::Free => 3,
            QuotaMethod::Unknown => 4,
        }
    }

    fn better(self, other: QuotaMethod) -> QuotaMethod {
        if self.rank() <= other.rank() {
            self
        } else {
            other
        }
    }
}

/// 窓 1 つのスナップショット（`RateWindow` に観測時刻を添えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WindowSnapshot {
    pub utilization: f64,
    pub resets_at: i64,
    pub observed_at: i64,
}

impl WindowSnapshot {
    fn from_observation(obs: &RateLimitObservation, window: QuotaWindow) -> Option<Self> {
        let w = match window {
            QuotaWindow::FiveHour => obs.five_hour?,
            QuotaWindow::SevenDay => obs.seven_day?,
        };
        Some(WindowSnapshot {
            utilization: w.utilization,
            resets_at: w.resets_at,
            observed_at: obs.observed_at,
        })
    }
}

/// D4.3: run 1 件・窓 1 つの消費。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuotaWindowUse {
    pub window: QuotaWindow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// 0〜100（パーセントポイント）。決められなければ `None`（`0` と混同しない。D4.2 4./unknown_is_never_zero）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_pct: Option<f64>,
    /// **ADR からの逸脱**（`agent-docs/adr/0074-...md` の「Phase F3（quota）実装時の逸脱・明確化」参照）:
    /// D4.3 の JSON 例は窓ごとの `method` を書いていないが、5 時間 / 7 日で窓リセットの有無により
    /// 決め方が食い違いうる（例: 7 日枠だけ `resets_at` を跨ぐ）ため、窓ごとにも残す。
    /// `Event::QuotaEstimated.method` はこれらのうち最も確からしいものを 1 つに畳み込んだ値。
    pub method: QuotaMethod,
}

/// D4.2 の較正値 `k_w`（同じ source の直近の `measured` run から求めた比の和）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuotaCalibration {
    pub k: f64,
    pub samples: u32,
}

pub const WEIGHTS_VERSION: &str = "quota-weights/1";
pub const CACHE_READ_WEIGHT: f64 = 0.1;
pub const CACHE_CREATION_WEIGHT: f64 = 1.25;
/// D4.2: `r_out` の provider 既定値（単価表にモデルが無いときのフォールバック）。
pub const DEFAULT_R_OUT_CLAUDE: f64 = 5.0;
pub const DEFAULT_R_OUT_CODEX: f64 = 4.0;
/// D4.2: `measured` が 3 件未満なら `estimated` を使わない（unknown に落とす）。
pub const MIN_CALIBRATION_SAMPLES: u32 = 3;
/// D4.2 較正の直近件数の上限（呼び出し側がリングバッファをこのサイズに保つ）。
pub const CALIBRATION_MAX_SAMPLES: usize = 20;

/// D4.2: 重み付きトークン `W = in_uncached + 0.1・cache_read + 1.25・cache_creation + r_out・output`。
pub fn weighted_tokens(usage: &Usage, r_out: f64) -> f64 {
    let input = usage.input_tokens.unwrap_or(0) as f64;
    let cache_read = usage.cache_read_tokens.unwrap_or(0) as f64;
    let cache_creation = usage.cache_creation_tokens.unwrap_or(0) as f64;
    let output = usage.output_tokens.unwrap_or(0) as f64;
    input + CACHE_READ_WEIGHT * cache_read + CACHE_CREATION_WEIGHT * cache_creation + r_out * output
}

/// D4.2: `r_out` の provider 既定（`source` は `"claude-oauth"` / `"codex-oauth"` 等の前方一致）。
/// 単価表にモデルがあれば `task_core::pricing::output_input_ratio` を優先する（呼び出し側の役目）。
pub fn default_r_out(source: &str) -> f64 {
    if source.starts_with("codex") || source.starts_with("gpt") {
        DEFAULT_R_OUT_CODEX
    } else {
        DEFAULT_R_OUT_CLAUDE
    }
}

/// D4.1: `before` 観測の有効性。「観測の時刻より後に、そのアカウントを使った消費が無く、枠がリセット
/// されていなければ、300 秒より古くても基準として使う」。`other_consumption_since` は、呼び出し側が
/// 追跡している「この観測より後に起きた、このアカウントの他の消費」の有無（決定的な入力にするため、
/// 判定そのものはここでは行わない）。観測が未来（壁時計のずれ）なら常に無効。
pub fn before_is_valid(observed_at: i64, now: i64, other_consumption_since: bool) -> bool {
    observed_at <= now && !other_consumption_since
}

/// `measured`（D4.2 手順 1）の入力。
#[derive(Debug, Clone, Copy)]
pub struct MeasuredInputs {
    pub before: Option<WindowSnapshot>,
    pub after: Option<WindowSnapshot>,
    /// D4.1 の有効性（`before_is_valid` の結果）。
    pub before_valid: bool,
    /// run の間、同じアカウントを使った他の run が無かった。
    pub exclusive: bool,
}

/// D4.2 手順 1: `measured`。決められなければ `None`（呼び出し側は手順 2 へ）。
pub fn measured_used_pct(input: &MeasuredInputs) -> Option<f64> {
    if !input.exclusive || !input.before_valid {
        return None;
    }
    let before = input.before?;
    let after = input.after?;
    if before.resets_at != after.resets_at {
        // 枠がリセットされた（別の窓インスタンス）: 比較できない。
        return None;
    }
    Some((100.0 * (after.utilization - before.utilization)).max(0.0))
}

/// `apportioned`（D4.2 手順 2）の入力。グループ全体（重なった run の集合）の前後観測と、
/// このグループの重み付きトークンの合計・この run 自身の重み付きトークン。
#[derive(Debug, Clone, Copy)]
pub struct ApportionedInputs {
    pub group_before: Option<WindowSnapshot>,
    pub group_after: Option<WindowSnapshot>,
    pub group_before_valid: bool,
    pub member_weighted_tokens: f64,
    pub group_weighted_tokens_total: f64,
}

/// D4.2 手順 2: `apportioned`。グループがまだ閉じていない（`group_after` が無い）間は `None`
/// （呼び出し側は「集合の最後の run が終わった時点で決まる」を守り、closed になってから呼ぶ）。
pub fn apportioned_used_pct(input: &ApportionedInputs) -> Option<f64> {
    if !input.group_before_valid || input.group_weighted_tokens_total <= 0.0 {
        return None;
    }
    let before = input.group_before?;
    let after = input.group_after?;
    if before.resets_at != after.resets_at {
        return None;
    }
    let delta = (100.0 * (after.utilization - before.utilization)).max(0.0);
    Some(delta * (input.member_weighted_tokens / input.group_weighted_tokens_total))
}

/// D4.2 手順 3: `estimated`。`measured` が 3 件未満なら `None`（呼び出し側は手順 4 = unknown へ）。
pub fn estimated_used_pct(
    weighted_tokens: f64,
    calibration: Option<QuotaCalibration>,
) -> Option<f64> {
    let cal = calibration?;
    if cal.samples < MIN_CALIBRATION_SAMPLES || !cal.k.is_finite() || cal.k < 0.0 {
        return None;
    }
    Some(cal.k * weighted_tokens)
}

/// D4.2: `k_w = Σq / ΣW`（比の和。外れ値に強い）。`samples` は使った `measured` run の件数。
/// 入力が空、または重みの合計が 0 以下なら `None`。
pub fn calibrate(measured: &[(f64, f64)]) -> Option<QuotaCalibration> {
    if measured.is_empty() {
        return None;
    }
    let sum_w: f64 = measured.iter().map(|(_, w)| w).sum();
    if sum_w <= 0.0 {
        return None;
    }
    let sum_q: f64 = measured.iter().map(|(q, _)| q).sum();
    Some(QuotaCalibration {
        k: sum_q / sum_w,
        samples: measured.len() as u32,
    })
}

/// 1 窓分を、D4.2 の優先順位（measured → apportioned → estimated → unknown）で決定的に決める。
#[allow(clippy::too_many_arguments)]
pub fn decide_window(
    window: QuotaWindow,
    measured: &MeasuredInputs,
    apportioned: Option<&ApportionedInputs>,
    weighted_tokens: f64,
    calibration: Option<QuotaCalibration>,
) -> QuotaWindowUse {
    if let Some(pct) = measured_used_pct(measured) {
        return QuotaWindowUse {
            window,
            before: measured.before.map(|w| w.utilization),
            after: measured.after.map(|w| w.utilization),
            resets_at: measured.after.map(|w| w.resets_at),
            used_pct: Some(pct),
            method: QuotaMethod::Measured,
        };
    }
    if let Some(ap) = apportioned
        && let Some(pct) = apportioned_used_pct(ap)
    {
        return QuotaWindowUse {
            window,
            before: ap.group_before.map(|w| w.utilization),
            after: ap.group_after.map(|w| w.utilization),
            resets_at: ap.group_after.map(|w| w.resets_at),
            used_pct: Some(pct),
            method: QuotaMethod::Apportioned,
        };
    }
    if let Some(pct) = estimated_used_pct(weighted_tokens, calibration) {
        return QuotaWindowUse {
            window,
            before: None,
            after: None,
            resets_at: None,
            used_pct: Some(pct),
            method: QuotaMethod::Estimated,
        };
    }
    QuotaWindowUse {
        window,
        before: None,
        after: None,
        resets_at: None,
        used_pct: None,
        method: QuotaMethod::Unknown,
    }
}

/// `free`（D4.2 手順 5）の窓（Qwen 等。窓自体を持たない供給元）。
pub fn free_window(window: QuotaWindow) -> QuotaWindowUse {
    QuotaWindowUse {
        window,
        before: None,
        after: None,
        resets_at: None,
        used_pct: Some(0.0),
        method: QuotaMethod::Free,
    }
}

/// `windows` から `WindowSnapshot` を作る（観測が無ければ `None`）。
pub fn snapshot(obs: Option<&RateLimitObservation>, window: QuotaWindow) -> Option<WindowSnapshot> {
    WindowSnapshot::from_observation(obs?, window)
}

/// `windows` を 1 つの代表 `method` に畳み込む（D4.3 の `Event::QuotaEstimated.method`）。
/// 空なら `Unknown`。
pub fn representative_method(windows: &[QuotaWindowUse]) -> QuotaMethod {
    windows
        .iter()
        .map(|w| w.method)
        .fold(QuotaMethod::Unknown, |acc, m| acc.better(m))
}

/// D4.3: アカウント × 窓の合計（`ExecutionMetrics.quota` / WU ごとの quota に使う集計行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuotaUse {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub window: QuotaWindow,
    /// 決められた run だけの合計（`unknown` の run は寄与しない）。全部 `unknown`/`free` なら
    /// `free` は `0.0` を寄与するので `Some(0.0)`、`unknown` だけなら `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_pct: Option<f64>,
    /// この (source, account, window) にこの窓の値を持つ run の数（`unknown` も含む）。
    pub runs: u32,
    pub method_counts: BTreeMap<String, u32>,
    /// ADR-0076: `runs` の役割別の内訳（`"worker"` / `"planner"` / `"reviewer"`。値の合計は `runs`）。
    /// 欄の無い旧 JSON は空 map として読む。
    #[serde(default)]
    pub runs_by_role: BTreeMap<String, u32>,
}

/// `QuotaEstimated` 相当の 1 run 分の情報（`execution_metrics::summarize` と WU ごとの集計が
/// 共有する入力の形）。
#[derive(Debug, Clone)]
pub struct QuotaRunRecord<'a> {
    pub source: &'a str,
    pub account: Option<&'a str>,
    pub windows: &'a [QuotaWindowUse],
    /// ADR-0076: この run の役割（`WorkerStarted.role` から。無ければ worker）。
    pub role: RunRole,
}

/// D4.3: 複数 run の `QuotaRunRecord` を (source, account, window) ごとに合計する純粋関数。
/// 呼び出し側は「同じ run_id は最後の Event が有効」という重複排除を先に済ませておくこと
/// （`execution_metrics::summarize` はイベント順で上書きしてから渡す）。
pub fn aggregate_quota_use<'a>(
    records: impl IntoIterator<Item = QuotaRunRecord<'a>>,
) -> Vec<QuotaUse> {
    #[derive(Default)]
    struct Acc {
        used_pct: Option<f64>,
        runs: u32,
        method_counts: BTreeMap<String, u32>,
        runs_by_role: BTreeMap<String, u32>,
    }
    let mut acc: BTreeMap<(String, Option<String>, QuotaWindow), Acc> = BTreeMap::new();
    for record in records {
        for w in record.windows {
            let key = (
                record.source.to_string(),
                record.account.map(str::to_string),
                w.window,
            );
            let entry = acc.entry(key).or_default();
            entry.runs += 1;
            *entry
                .runs_by_role
                .entry(record.role.as_str().to_string())
                .or_insert(0) += 1;
            *entry
                .method_counts
                .entry(w.method.as_str().to_string())
                .or_insert(0) += 1;
            if let Some(pct) = w.used_pct {
                entry.used_pct = Some(entry.used_pct.unwrap_or(0.0) + pct);
            }
        }
    }
    acc.into_iter()
        .map(|((source, account, window), a)| QuotaUse {
            source,
            account,
            window,
            used_pct: a.used_pct,
            runs: a.runs,
            method_counts: a.method_counts,
            runs_by_role: a.runs_by_role,
        })
        .collect()
}

/// D4.3: 複数の `QuotaUse`（例えば別々の Task から求めたもの）を、さらに (source, account, window)
/// ごとに合計する純粋関数（`GET /metrics/execution` の group ごとの集計に使う）。
pub fn merge_quota_use(rows: impl IntoIterator<Item = QuotaUse>) -> Vec<QuotaUse> {
    let mut acc: BTreeMap<(String, Option<String>, QuotaWindow), QuotaUse> = BTreeMap::new();
    for row in rows {
        let key = (row.source.clone(), row.account.clone(), row.window);
        match acc.entry(key) {
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(row);
            }
            std::collections::btree_map::Entry::Occupied(mut o) => {
                let existing = o.get_mut();
                existing.runs += row.runs;
                existing.used_pct = match (existing.used_pct, row.used_pct) {
                    (Some(a), Some(b)) => Some(a + b),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                };
                for (method, count) in row.method_counts {
                    *existing.method_counts.entry(method).or_insert(0) += count;
                }
                for (role, count) in row.runs_by_role {
                    *existing.runs_by_role.entry(role).or_insert(0) += count;
                }
            }
        }
    }
    acc.into_values().collect()
}

#[cfg(test)]
#[path = "quota/tests.rs"]
mod tests;
