//! account/source の利用可否（ADR 2026-10-15-mlsys-min-routing-policy D6）。
//! Unknown の quota/cost を 0 扱いしない。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Remaining {
    Known(f64),
    Unknown,
}

impl Remaining {
    /// cost の順位付け用。Unknown は最悪値で、0（無料）扱いにしない。
    pub fn pessimistic_cost(self) -> f64 {
        match self {
            Remaining::Known(v) => v,
            Remaining::Unknown => f64::INFINITY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    Incompatible,
    CoolingDown,
    ConcurrencyFull,
    QuotaExhausted,
    BudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable(UnavailableReason),
    /// 未確認。不可でも可でもない。
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AvailabilityInput {
    pub compatible: bool,
    pub cooling_down: bool,
    pub in_use: u32,
    /// `None` は同時実行の上限なし。
    pub max_concurrent: Option<u32>,
    pub quota_remaining: Remaining,
    pub estimated_cost: Remaining,
    pub budget_remaining: Remaining,
}

pub fn availability(i: &AvailabilityInput) -> Availability {
    use Availability::*;
    use UnavailableReason::*;
    if !i.compatible {
        return Unavailable(Incompatible);
    }
    if i.cooling_down {
        return Unavailable(CoolingDown);
    }
    if i.max_concurrent.is_some_and(|m| i.in_use >= m) {
        return Unavailable(ConcurrencyFull);
    }
    let cost = match i.estimated_cost {
        Remaining::Known(c) => Some(c),
        Remaining::Unknown => None,
    };
    if let Remaining::Known(q) = i.quota_remaining
        && (q <= 0.0 || cost.is_some_and(|c| c > q))
    {
        return Unavailable(QuotaExhausted);
    }
    if let Remaining::Known(b) = i.budget_remaining
        && (b <= 0.0 || cost.is_some_and(|c| c > b))
    {
        return Unavailable(BudgetExhausted);
    }
    let unknown =
        [i.quota_remaining, i.estimated_cost, i.budget_remaining].contains(&Remaining::Unknown);
    if unknown { Unknown } else { Available }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AvailabilitySplit<T> {
    pub available: Vec<T>,
    /// 未確認。`available` が空でも可として使わない（呼び出し側が確認してから）。
    pub unconfirmed: Vec<T>,
}

/// Unavailable を落とし、Available を先、Unknown を後ろに分ける。
pub fn split_by_availability<T>(items: Vec<(T, Availability)>) -> AvailabilitySplit<T> {
    let mut out = AvailabilitySplit {
        available: vec![],
        unconfirmed: vec![],
    };
    for (t, a) in items {
        match a {
            Availability::Available => out.available.push(t),
            Availability::Unknown => out.unconfirmed.push(t),
            Availability::Unavailable(_) => {}
        }
    }
    out
}
