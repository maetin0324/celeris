//! ADR §4: effective cost・pressure・score の純粋関数。
//! 時計（now）と観測の鮮度は引数で注入する。未知は None のまま返し、0 で seed しない。
//! 課金の実測値（tokens・wall_ms など）はここでは書き換えない。
use super::profiles::{Billing, QuotaWindow, SourceState, TokenPricing};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshnessPolicy {
    pub observation_ttl_seconds: u64,
}
impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            observation_ttl_seconds: 900,
        }
    }
}
impl FreshnessPolicy {
    fn validate(&self) -> Result<(), &'static str> {
        if self.observation_ttl_seconds == 0 {
            Err("invalid_freshness")
        } else {
            Ok(())
        }
    }
}
fn default_window_duration_seconds(id: &str) -> Option<f64> {
    match id {
        "five_hour" | "5h" => Some(18_000.0),
        "seven_day" | "7d" => Some(604_800.0),
        _ => None,
    }
}

/// 1 呼出しの見積もり使用量。欠測は None。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RequestUsage {
    pub input_tokens: Option<f64>,
    pub cached_input_tokens: Option<f64>,
    pub output_tokens: Option<f64>,
    pub gpu_seconds: Option<f64>,
    pub queue_wait_seconds: Option<f64>,
}

/// self-host の予約機会費用の係数（設定由来）。未設定は resource unknown。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SelfHostRates {
    pub usd_per_gpu_second: Option<f64>,
    pub usd_per_wait_second: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct CostInputs<'a> {
    pub billing: Billing,
    /// 実効の単価（deployment の price_override、無ければ model の pricing）。未登録は None。
    pub pricing: Option<&'a TokenPricing>,
    pub usage: RequestUsage,
    pub self_host: SelfHostRates,
    pub state: &'a SourceState,
    pub concurrency_limit: Option<u32>,
    pub now: OffsetDateTime,
    pub freshness: FreshnessPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CostEstimate {
    pub cash_usd: Option<f64>,
    pub subscription_shadow_usd: Option<f64>,
    pub self_host_resource_usd: Option<f64>,
    pub effective_usd: Option<f64>,
    pub pressure: Option<f64>,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreBreakdown {
    pub quality: f64,
    pub cost_term: f64,
    pub latency_term: f64,
    pub pressure_term: f64,
    /// 順位用に 1 とした項（C/L/P）と品質以外の unknown flag。
    pub unknown: Vec<&'static str>,
    pub score: f64,
}

/// effective cost（cash・shadow・resource）と pressure を推定する。不正な数値は Err。
pub fn estimate_cost(input: &CostInputs<'_>) -> Result<CostEstimate, &'static str> {
    input.freshness.validate()?;
    validate_usage(&input.usage)?;
    let mut assumptions = Vec::new();
    let cash = cash_usd(input, &mut assumptions)?;
    let shadow = match input.billing {
        Billing::Subscription => subscription_shadow_usd(input, &mut assumptions)?,
        Billing::MeteredApi | Billing::SelfHosted => Some(0.0),
    };
    let resource = self_host_resource_usd(input)?;
    Ok(CostEstimate {
        cash_usd: cash,
        subscription_shadow_usd: shadow,
        self_host_resource_usd: resource,
        effective_usd: cash.zip(shadow).zip(resource).map(|((c, s), r)| c + s + r),
        pressure: pressure(input.state, input.concurrency_limit)?,
        assumptions,
    })
}

/// 時刻・数量に基づく除外（score より前）。cooldown・rate limit の終了前、新鮮な観測での枯渇。
/// 古い観測の 0 は枯渇とみなさない（unknown）。
pub fn exclusion_reasons(
    state: &SourceState,
    now: OffsetDateTime,
    freshness: FreshnessPolicy,
) -> Result<Vec<&'static str>, &'static str> {
    freshness.validate()?;
    let mut reasons = Vec::new();
    if parse_opt(state.cooldown_until.as_deref())?.is_some_and(|t| t > now) {
        reasons.push("cooldown");
    }
    if parse_opt(state.rate_limited_until.as_deref())?.is_some_and(|t| t > now) {
        reasons.push("rate_limit");
    }
    for w in &state.quota_windows {
        if fresh_remaining(w, state, now, freshness)? == Some(0.0) {
            reasons.push("quota_exhausted");
            break;
        }
    }
    Ok(reasons)
}

fn cash_usd(
    input: &CostInputs<'_>,
    assumptions: &mut Vec<String>,
) -> Result<Option<f64>, &'static str> {
    if input.billing != Billing::MeteredApi {
        return Ok(Some(0.0));
    }
    let Some(price) = input.pricing else {
        assumptions.push("price_unknown".into());
        return Ok(None);
    };
    let u = &input.usage;
    let (Some(input_tokens), Some(output_tokens), Some(in_price), Some(out_price)) = (
        u.input_tokens,
        u.output_tokens,
        price.input_usd_per_million,
        price.output_usd_per_million,
    ) else {
        return Ok(None);
    };
    let (uncached, cached_cost) = match u.cached_input_tokens {
        Some(cached) if cached > 0.0 => {
            let Some(cached_price) = price.cached_input_usd_per_million else {
                assumptions.push("cached_price_unknown".into());
                return Ok(None);
            };
            (input_tokens - cached, cached * cached_price)
        }
        Some(_) => (input_tokens, 0.0),
        None => {
            assumptions.push("cache_unknown_all_uncached".into());
            (input_tokens, 0.0)
        }
    };
    Ok(Some(
        (uncached * in_price + cached_cost + output_tokens * out_price) / 1_000_000.0,
    ))
}

/// shadow = max_j(reserve_j × u_j × h_j)。一窓でも unknown なら全体 unknown（max を過小評価しない）。
fn subscription_shadow_usd(
    input: &CostInputs<'_>,
    assumptions: &mut Vec<String>,
) -> Result<Option<f64>, &'static str> {
    let windows = &input.state.quota_windows;
    if windows.is_empty() {
        assumptions.push("no_quota_window".into());
        return Ok(None);
    }
    let mut max: f64 = 0.0;
    for w in windows {
        match window_shadow_usd(w, input)? {
            Some(term) => max = max.max(term),
            None => {
                assumptions.push(format!("shadow_unknown:{}", w.window_id));
                return Ok(None);
            }
        }
    }
    Ok(Some(max))
}

fn window_shadow_usd(w: &QuotaWindow, input: &CostInputs<'_>) -> Result<Option<f64>, &'static str> {
    let (Some(reserve), Some(consumption)) = (w.reserve_value_usd, w.estimated_consumption) else {
        return Ok(None);
    };
    if w.unit.is_empty() || w.unit == "unknown" {
        return Ok(None);
    }
    // 残量が無い・古い・reset を越えた窓は unknown。満タンとは扱わない。
    let Some(remaining) = fresh_remaining(w, input.state, input.now, input.freshness)? else {
        return Ok(None);
    };
    if remaining <= 0.0 {
        return Ok(None);
    }
    let Some(reset) = parse_opt(w.reset_at.as_deref())? else {
        return Ok(None);
    };
    let Some(duration) = w
        .window_duration_s
        .or_else(|| default_window_duration_seconds(&w.window_id))
    else {
        return Ok(None);
    };
    non_negative(reserve)?;
    non_negative(consumption)?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err("zero_denominator");
    }
    let u = consumption / remaining;
    let h = ((reset - input.now).as_seconds_f64() / duration).clamp(0.0, 1.0);
    Ok(Some(reserve * u * h))
}

fn self_host_resource_usd(input: &CostInputs<'_>) -> Result<Option<f64>, &'static str> {
    if input.billing != Billing::SelfHosted {
        return Ok(Some(0.0));
    }
    let (Some(gpu), Some(gpu_rate)) = (input.usage.gpu_seconds, input.self_host.usd_per_gpu_second)
    else {
        return Ok(None);
    };
    let (Some(wait), Some(wait_rate)) = (
        input.usage.queue_wait_seconds,
        input.self_host.usd_per_wait_second,
    ) else {
        return Ok(None);
    };
    non_negative(gpu_rate)?;
    non_negative(wait_rate)?;
    Ok(Some(gpu * gpu_rate + wait * wait_rate))
}

/// pressure = max(既知成分)。各成分は [0,1] に clamp。既知成分が無ければ None。
pub fn pressure(
    state: &SourceState,
    concurrency_limit: Option<u32>,
) -> Result<Option<f64>, &'static str> {
    let mut parts = Vec::new();
    if let (Some(used), Some(limit)) = (state.in_use, concurrency_limit) {
        parts.push(ratio(f64::from(used), f64::from(limit))?);
    }
    if let (Some(depth), Some(limit)) = (state.queue_depth, state.queue_limit) {
        parts.push(ratio(f64::from(depth), f64::from(limit))?);
    }
    if let Some(util) = state.gpu_utilization {
        parts.push(ratio(util, 1.0)?);
    }
    if let (Some(used), Some(total)) = (state.vram_used, state.vram_total) {
        parts.push(ratio(used as f64, total as f64)?);
    }
    Ok(parts.into_iter().reduce(f64::max))
}

fn ratio(num: f64, den: f64) -> Result<f64, &'static str> {
    non_negative(num)?;
    non_negative(den)?;
    if den == 0.0 {
        return Err("zero_denominator");
    }
    Ok((num / den).clamp(0.0, 1.0))
}

/// 残量が「新鮮」なときだけ既知。観測が TTL 内で、reset 境界を越えていないこと。
fn fresh_remaining(
    w: &QuotaWindow,
    state: &SourceState,
    now: OffsetDateTime,
    freshness: FreshnessPolicy,
) -> Result<Option<f64>, &'static str> {
    let Some(remaining) = w.remaining else {
        return Ok(None);
    };
    non_negative(remaining)?;
    if !is_fresh(w, state, now, freshness)? {
        return Ok(None);
    }
    Ok(Some(remaining))
}

fn is_fresh(
    w: &QuotaWindow,
    state: &SourceState,
    now: OffsetDateTime,
    freshness: FreshnessPolicy,
) -> Result<bool, &'static str> {
    let Some(observed) = parse_opt(w.observed_at.as_deref().or(state.observed_at.as_deref()))?
    else {
        return Ok(false);
    };
    let age = (now - observed).as_seconds_f64();
    if !(0.0..=freshness.observation_ttl_seconds as f64).contains(&age) {
        return Ok(false);
    }
    Ok(parse_opt(w.reset_at.as_deref())?.is_none_or(|reset| reset > now))
}

fn validate_usage(u: &RequestUsage) -> Result<(), &'static str> {
    for v in [
        u.input_tokens,
        u.cached_input_tokens,
        u.output_tokens,
        u.gpu_seconds,
        u.queue_wait_seconds,
    ]
    .into_iter()
    .flatten()
    {
        non_negative(v)?;
    }
    if let (Some(cached), Some(input)) = (u.cached_input_tokens, u.input_tokens)
        && cached > input
    {
        return Err("invalid_estimate");
    }
    Ok(())
}

fn non_negative(v: f64) -> Result<(), &'static str> {
    if v.is_finite() && v >= 0.0 {
        Ok(())
    } else {
        Err("invalid_estimate")
    }
}

fn parse_opt(raw: Option<&str>) -> Result<Option<OffsetDateTime>, &'static str> {
    raw.map(|s| OffsetDateTime::parse(s, &Rfc3339).map_err(|_| "invalid_timestamp"))
        .transpose()
}

#[cfg(test)]
#[path = "cost/tests.rs"]
mod tests;
