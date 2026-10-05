//! ADR §4: effective cost・pressure・score の純粋関数。
//! 時計（now）と観測の鮮度は引数で注入する。未知は None のまま返し、0 で seed しない。
//! 課金の実測値（tokens・wall_ms など）はここでは書き換えない。
use super::{
    policy::{FreshnessPolicy, RoutingPolicy, default_window_duration_seconds},
    profiles::{Billing, QuotaWindow, SourceState, TokenPricing},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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

/// 同じ入力は同じ出力。C = min(effective/ref,1)、L = min(latency/ref,1)、P = pressure。
/// C/L/P の未知は順位用に 1 とし unknown flag を残す。品質の未知・不合格は Err。
pub fn score(
    policy: &RoutingPolicy,
    quality: Option<f64>,
    estimate: &CostEstimate,
    latency_ms: Option<f64>,
) -> Result<ScoreBreakdown, &'static str> {
    policy.validate()?;
    let q = quality.ok_or("quality_unknown")?;
    if !q.is_finite() || !(0.0..=1.0).contains(&q) {
        return Err("quality_invalid");
    }
    if q < policy.min_quality {
        return Err("quality_below_min");
    }
    let mut unknown = Vec::new();
    let cost_term = match estimate.effective_usd {
        Some(v) => {
            non_negative(v)?;
            (v / policy.normalization.cost_reference_usd).min(1.0)
        }
        None => {
            unknown.push("cost_unknown");
            1.0
        }
    };
    let latency_term = match latency_ms {
        Some(v) => {
            non_negative(v)?;
            (v / policy.normalization.latency_reference_ms).min(1.0)
        }
        None => {
            unknown.push("latency_unknown");
            1.0
        }
    };
    let pressure_term = match estimate.pressure {
        Some(p) if p.is_finite() && (0.0..=1.0).contains(&p) => p,
        Some(_) => return Err("invalid_estimate"),
        None => {
            unknown.push("pressure_unknown");
            1.0
        }
    };
    let w = &policy.weights;
    let score =
        w.quality * q - w.cost * cost_term - w.latency * latency_term - w.pressure * pressure_term;
    Ok(ScoreBreakdown {
        quality: q,
        cost_term,
        latency_term,
        pressure_term,
        unknown,
        score,
    })
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
    if !(0.0..=freshness.observation_ttl_seconds).contains(&age) {
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
mod tests {
    use super::*;
    use crate::Tier;
    use crate::model_router::policy::{RoutingMode, WINDOW_5H_SECONDS};

    const EPS: f64 = 1e-9;

    fn at(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap()
    }

    fn rfc(t: OffsetDateTime) -> String {
        t.format(&Rfc3339).unwrap()
    }

    fn window(
        id: &str,
        remaining: f64,
        reset: Option<OffsetDateTime>,
        observed: OffsetDateTime,
    ) -> QuotaWindow {
        QuotaWindow {
            account_id: "acc-1".into(),
            window_id: id.into(),
            unit: "requests".into(),
            remaining: Some(remaining),
            limit: Some(1.0),
            reset_at: reset.map(rfc),
            measured: true,
            observed_at: Some(rfc(observed)),
            window_duration_s: None,
            estimated_consumption: Some(1.0),
            reserve_value_usd: Some(10.0),
        }
    }

    fn state(windows: Vec<QuotaWindow>, observed: OffsetDateTime) -> SourceState {
        let mut s = SourceState::unobserved("dep-1");
        s.observed_at = Some(rfc(observed));
        s.quota_windows = windows;
        s
    }

    fn sub<'a>(state: &'a SourceState, now: OffsetDateTime) -> CostInputs<'a> {
        CostInputs {
            billing: Billing::Subscription,
            pricing: None,
            usage: RequestUsage::default(),
            self_host: SelfHostRates::default(),
            state,
            concurrency_limit: None,
            now,
            freshness: FreshnessPolicy::default(),
        }
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|v| (v - b).abs() < EPS)
    }

    #[test]
    fn routing_effective_cost_distinguishes_cash_shadow_and_resource() {
        let now = at(0);
        let five_h = WINDOW_5H_SECONDS as i64;
        // 残量 0.5 → 0.1 で shadow が 5 倍（reset までは同じ）。
        let s_half = state(
            vec![window(
                "five_hour",
                0.5,
                Some(now + time::Duration::seconds(five_h / 2)),
                now,
            )],
            now,
        );
        let s_tenth = state(
            vec![window(
                "five_hour",
                0.1,
                Some(now + time::Duration::seconds(five_h / 2)),
                now,
            )],
            now,
        );
        let a = estimate_cost(&sub(&s_half, now)).unwrap();
        let b = estimate_cost(&sub(&s_tenth, now)).unwrap();
        let ratio = b.subscription_shadow_usd.unwrap() / a.subscription_shadow_usd.unwrap();
        assert!(
            (ratio - 5.0).abs() < EPS,
            "remaining 0.5->0.1 must multiply shadow by 5, got {ratio}"
        );
        // reset までの時間が半分なら shadow も半分（残量は同じ）。
        let full = state(
            vec![window(
                "five_hour",
                0.5,
                Some(now + time::Duration::seconds(five_h)),
                now,
            )],
            now,
        );
        let c = estimate_cost(&sub(&full, now)).unwrap();
        let half_ratio = a.subscription_shadow_usd.unwrap() / c.subscription_shadow_usd.unwrap();
        assert!(
            (half_ratio - 0.5).abs() < EPS,
            "half time to reset must halve shadow, got {half_ratio}"
        );
        // 同じ入力は同じ出力。
        assert_eq!(estimate_cost(&sub(&s_half, now)).unwrap(), a);

        // 実測 cash は窓・queue の状態に依らず不変。未知価格は 0 にしない。
        let priced = TokenPricing {
            input_usd_per_million: Some(1.0),
            cached_input_usd_per_million: None,
            output_usd_per_million: Some(2.0),
            as_of: None,
            provenance: "test".into(),
        };
        let usage = RequestUsage {
            input_tokens: Some(1_000_000.0),
            output_tokens: Some(500_000.0),
            ..Default::default()
        };
        let api = |s: &SourceState, pricing: Option<&TokenPricing>| {
            estimate_cost(&CostInputs {
                billing: Billing::MeteredApi,
                pricing,
                usage,
                self_host: SelfHostRates::default(),
                state: s,
                concurrency_limit: None,
                now,
                freshness: FreshnessPolicy::default(),
            })
            .unwrap()
        };
        let p1 = api(&s_half, Some(&priced));
        let p2 = api(&s_tenth, Some(&priced));
        assert!(close(p1.cash_usd, 2.0), "cash = 1.0 + 0.5*2.0");
        assert_eq!(
            p1.cash_usd, p2.cash_usd,
            "cash must not depend on quota state"
        );
        assert_eq!(
            api(&s_half, None).cash_usd,
            None,
            "unknown price is None, not 0"
        );
        assert!(
            api(&s_half, None)
                .assumptions
                .contains(&"price_unknown".to_string())
        );

        // self-host: queue 待ちが増えると resource と pressure が増える。
        let rates = SelfHostRates {
            usd_per_gpu_second: Some(0.001),
            usd_per_wait_second: Some(0.0001),
        };
        let mut busy = SourceState::unobserved("gpu-1");
        busy.observed_at = Some(rfc(now));
        busy.queue_limit = Some(4);
        let sh = |depth: u32, wait: f64| {
            let mut s = busy.clone();
            s.queue_depth = Some(depth);
            estimate_cost(&CostInputs {
                billing: Billing::SelfHosted,
                pricing: None,
                usage: RequestUsage {
                    gpu_seconds: Some(100.0),
                    queue_wait_seconds: Some(wait),
                    ..Default::default()
                },
                self_host: rates,
                state: &s,
                concurrency_limit: None,
                now,
                freshness: FreshnessPolicy::default(),
            })
            .unwrap()
        };
        let low = sh(1, 10.0);
        let high = sh(3, 30.0);
        assert!(close(low.self_host_resource_usd, 0.1 + 0.001));
        assert!(high.self_host_resource_usd > low.self_host_resource_usd);
        assert!(close(low.pressure, 0.25) && close(high.pressure, 0.75));
        assert_eq!(low.cash_usd, Some(0.0), "self-host has no API cash");
        // 測定・設定が無ければ resource は unknown。
        let no_rate = estimate_cost(&CostInputs {
            billing: Billing::SelfHosted,
            pricing: None,
            usage: RequestUsage {
                gpu_seconds: Some(1.0),
                queue_wait_seconds: Some(1.0),
                ..Default::default()
            },
            self_host: SelfHostRates::default(),
            state: &busy,
            concurrency_limit: None,
            now,
            freshness: FreshnessPolicy::default(),
        })
        .unwrap();
        assert_eq!(no_rate.self_host_resource_usd, None);
        assert_eq!(no_rate.effective_usd, None);
    }

    #[test]
    fn routing_source_state_stale_quota_is_unknown() {
        let now = at(0);
        let ttl = FreshnessPolicy::default().observation_ttl_seconds as i64;
        // 偽時計: TTL を 1 秒超えた観測。残量 0.5 を満タン扱いせず shadow は unknown。
        let old = state(
            vec![window(
                "five_hour",
                0.5,
                Some(now + time::Duration::hours(1)),
                now - time::Duration::seconds(ttl + 1),
            )],
            now - time::Duration::seconds(ttl + 1),
        );
        let stale = estimate_cost(&sub(&old, now)).unwrap();
        assert_eq!(
            stale.subscription_shadow_usd, None,
            "stale remaining must not become zero shadow"
        );
        assert!(
            exclusion_reasons(&old, now, FreshnessPolicy::default())
                .unwrap()
                .is_empty()
        );

        // reset 境界を越えた窓（観測は新しくても）も満タンではなく unknown。
        let past_reset = state(
            vec![window(
                "five_hour",
                0.5,
                Some(now - time::Duration::seconds(1)),
                now,
            )],
            now,
        );
        assert_eq!(
            estimate_cost(&sub(&past_reset, now))
                .unwrap()
                .subscription_shadow_usd,
            None
        );

        // 古い観測の 0 は枯渇として除外しない（reset 前でも TTL 外なら unknown）。
        let stale_zero = state(
            vec![window(
                "five_hour",
                0.0,
                Some(now + time::Duration::hours(1)),
                now - time::Duration::seconds(ttl + 1),
            )],
            now,
        );
        assert!(
            exclusion_reasons(&stale_zero, now, FreshnessPolicy::default())
                .unwrap()
                .is_empty()
        );

        // 新鮮な枯渇が 1 窓でもあれば除外（他の窓が十分でも）。
        let exhausted = state(
            vec![
                window("five_hour", 0.0, Some(now + time::Duration::hours(1)), now),
                window("seven_day", 0.9, Some(now + time::Duration::days(3)), now),
            ],
            now,
        );
        assert_eq!(
            exclusion_reasons(&exhausted, now, FreshnessPolicy::default()).unwrap(),
            vec!["quota_exhausted"]
        );

        // 観測時刻が無い・未来の観測は既知にしない。
        let mut no_time = state(vec![window("monthly", 0.5, None, now)], now);
        no_time.observed_at = None;
        no_time.quota_windows[0].observed_at = None;
        assert_eq!(
            estimate_cost(&sub(&no_time, now))
                .unwrap()
                .subscription_shadow_usd,
            None
        );
        let future = state(
            vec![window(
                "monthly",
                0.5,
                None,
                now + time::Duration::seconds(5),
            )],
            now,
        );
        assert_eq!(
            estimate_cost(&sub(&future, now))
                .unwrap()
                .subscription_shadow_usd,
            None
        );

        // 数値の検証: NaN・負・分母 0 は拒否。
        let mut nan = window("five_hour", 0.5, Some(now + time::Duration::hours(1)), now);
        nan.estimated_consumption = Some(f64::NAN);
        assert_eq!(
            estimate_cost(&sub(&state(vec![nan], now), now)).unwrap_err(),
            "invalid_estimate"
        );
        let mut zero_duration = window("custom", 0.5, Some(now + time::Duration::hours(1)), now);
        zero_duration.window_duration_s = Some(0.0);
        assert_eq!(
            estimate_cost(&sub(&state(vec![zero_duration], now), now)).unwrap_err(),
            "zero_denominator"
        );
        let mut zero_queue = SourceState::unobserved("gpu-1");
        zero_queue.queue_depth = Some(1);
        zero_queue.queue_limit = Some(0);
        assert_eq!(pressure(&zero_queue, None).unwrap_err(), "zero_denominator");
    }

    #[test]
    fn routing_score_unknown_terms_rank_as_worst_and_keep_flags() {
        let policy = RoutingPolicy::defaults(Tier::Standard, RoutingMode::Shadow);
        let known = CostEstimate {
            cash_usd: Some(0.2),
            subscription_shadow_usd: Some(0.0),
            self_host_resource_usd: Some(0.0),
            effective_usd: Some(0.2),
            pressure: Some(0.5),
            assumptions: vec![],
        };
        let unknown = CostEstimate {
            effective_usd: None,
            pressure: None,
            ..known.clone()
        };
        let a = score(&policy, Some(0.9), &known, Some(1000.0)).unwrap();
        let b = score(&policy, Some(0.9), &unknown, None).unwrap();
        assert!(a.unknown.is_empty());
        assert_eq!(
            b.unknown,
            vec!["cost_unknown", "latency_unknown", "pressure_unknown"]
        );
        assert!(
            a.score > b.score,
            "unknown terms must not look better than known ones"
        );
        assert_eq!(
            score(&policy, None, &known, None).unwrap_err(),
            "quality_unknown"
        );
        assert_eq!(
            score(&policy, Some(0.1), &known, None).unwrap_err(),
            "quality_below_min"
        );
    }
}
