//! ADR §4: deployment の pressure の純粋関数と、費用の見積もりの表示型。
//! 未知は None のまま返し、0 で seed しない。
use super::profiles::SourceState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CostEstimate {
    pub cash_usd: Option<f64>,
    pub subscription_shadow_usd: Option<f64>,
    pub self_host_resource_usd: Option<f64>,
    pub effective_usd: Option<f64>,
    pub pressure: Option<f64>,
    pub assumptions: Vec<String>,
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

fn non_negative(v: f64) -> Result<(), &'static str> {
    if v.is_finite() && v >= 0.0 {
        Ok(())
    } else {
        Err("invalid_estimate")
    }
}

#[cfg(test)]
#[path = "cost/tests.rs"]
mod tests;
