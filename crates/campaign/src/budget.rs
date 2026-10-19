//! 残予算 =（上限）−（確定消費）−（in-flight の予約）。新規投入の可否と想定最大超過を決める。
//! 設計は ADR 2026-10-16-campaign-budget-reservation。台帳（ledger）だけを材料にする純関数で、再起動後も同じ値になる。
use std::collections::BTreeMap;

use crate::{
    ledger::{Ledger, LedgerRow},
    manifest::{Budget, Cap, ConfigId},
    record::{REASON_EXECUTOR_ERROR, TrialRecord},
};

/// 確定消費と in-flight の予約を合わせた使用済み枠。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Spend {
    /// campaign 全体（確定消費 + 予約）。
    pub tokens: u64,
    pub wall_secs: u64,
    /// arm ごとの（tokens, wall）。確定消費 + 予約。
    pub arm: BTreeMap<ConfigId, (u64, u64)>,
    /// 予約だけの内訳（in-flight の合計）。
    pub held_tokens: u64,
    pub held_wall_secs: u64,
    /// 自分以外の未完了（終端未確認を含む）の台帳行の数。
    pub in_flight: usize,
}

/// 1 行の予約額。予約（無ければ per_case）と既観測の大きい方。欠測の usage は 0 でなく予約のまま。
pub fn hold_of(row: &LedgerRow, b: &Budget) -> (u64, u64) {
    let reserved = row.reserved.unwrap_or(b.per_case);
    let observed = row.observed_usage;
    let tokens = observed
        .and_then(|o| o.tokens)
        .and_then(|u| u.count(b.token_basis))
        .map_or(reserved.max_tokens, |t| t.max(reserved.max_tokens));
    let wall = observed.map_or(reserved.max_wall_secs, |o| {
        o.wall_secs.max(reserved.max_wall_secs)
    });
    (tokens, wall)
}

/// 記録（確定消費）と台帳（予約）から使用済み枠を作る。`exclude` はこれから投入する自分の行。
/// 台帳が terminal でない行（cancel の停止未確認を含む）は予約が残るので、記録済みでも確定消費として
/// 二重に数えず予約側で数える。未投入の拒否（executor_error）は何も消費していないので数えない。
pub fn spend(
    records: &[TrialRecord],
    ledger: &Ledger,
    b: &Budget,
    hash: &str,
    exclude: Option<&crate::ledger::TrialKey>,
) -> Spend {
    let mut s = Spend::default();
    let open: Vec<&LedgerRow> = ledger
        .unfinished()
        .into_iter()
        .filter(|r| r.key.manifest_sha256 == hash && Some(&r.key) != exclude)
        .collect();
    for r in records.iter().filter(|r| r.ran()) {
        if r.reason.as_deref() == Some(REASON_EXECUTOR_ERROR)
            || open.iter().any(|o| {
                (o.key.arm, &o.key.problem_id, o.key.rep) == (r.config, &r.problem_id, r.rep)
            })
        {
            continue;
        }
        let t = r.charged_tokens(b);
        s.tokens += t;
        s.wall_secs += r.wall_secs;
        let e = s.arm.entry(r.config).or_default();
        e.0 += t;
        e.1 += r.wall_secs;
    }
    for row in &open {
        let (t, w) = hold_of(row, b);
        s.tokens += t;
        s.wall_secs += w;
        s.held_tokens += t;
        s.held_wall_secs += w;
        let e = s.arm.entry(row.key.arm).or_default();
        e.0 += t;
        e.1 += w;
    }
    s.in_flight = open.len();
    s
}

/// B1/C1（同一測定系列 = 同じ `campaign_id` の side manifest）で、hash の違う兄弟 out dir の既消費。
/// 台帳 key に manifest hash が入るため、hash を変えた再 manifest では自分の台帳から既消費が見えない。
/// `out` の親 dir の兄弟のうち、run-meta の campaign_id が同じで hash が違うものの確定消費 + 予約を合算する。
/// 主比較（side でない manifest）は常に 0（既存の計上は不変）。
pub fn series_prior_spend(
    m: &crate::manifest::CampaignManifest,
    hash: &str,
    out: &std::path::Path,
) -> Result<Spend, String> {
    let mut total = Spend::default();
    if !m.is_side() {
        return Ok(total);
    }
    let Some(parent) = out.parent() else {
        return Ok(total);
    };
    let Ok(rd) = std::fs::read_dir(parent) else {
        return Ok(total);
    };
    let mut dirs: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        if dir == out || !dir.is_dir() {
            continue;
        }
        let Ok(text) = std::fs::read(dir.join(crate::record::META_FILE)) else {
            continue;
        };
        let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&text) else {
            continue;
        };
        let other = meta["campaign_sha256"].as_str().unwrap_or_default();
        if meta["campaign_id"].as_str() != Some(m.campaign_id.as_str()) || other == hash {
            continue;
        }
        let records = read_records(&dir.join(crate::record::TRIALS_FILE))?;
        let ledger = Ledger::open_readonly(&dir)?;
        let s = spend(&records, &ledger, &m.budget, other, None);
        total.tokens += s.tokens;
        total.wall_secs += s.wall_secs;
        total.held_tokens += s.held_tokens;
        total.held_wall_secs += s.held_wall_secs;
        total.in_flight += s.in_flight;
        for (k, (t, w)) in s.arm {
            let e = total.arm.entry(k).or_default();
            e.0 += t;
            e.1 += w;
        }
    }
    Ok(total)
}

/// 他の out の trials.jsonl を読み取り専用で読む（末尾の欠けた行は無視し、直さない）。
fn read_records(path: &std::path::Path) -> Result<Vec<TrialRecord>, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let keep = text.rfind('\n').map_or(0, |i| i + 1);
    text[..keep]
        .lines()
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", path.display())))
        .collect()
}

impl Spend {
    /// 同一系列の既消費を加えた使用済み枠（総枠 `Budget::max_*` との比較用）。
    pub fn plus_prior(mut self, prior: &Spend) -> Spend {
        self.tokens += prior.tokens;
        self.wall_secs += prior.wall_secs;
        self.held_tokens += prior.held_tokens;
        self.held_wall_secs += prior.held_wall_secs;
        self.in_flight += prior.in_flight;
        self
    }
}

/// 試行の実効上限 = min(per_case, per_arm の残り)。
pub fn effective_cap(b: &Budget, arm_tokens: u64, arm_wall: u64) -> Cap {
    Cap {
        max_tokens: b
            .per_case
            .max_tokens
            .min(b.per_arm.max_tokens.saturating_sub(arm_tokens)),
        max_wall_secs: b
            .per_case
            .max_wall_secs
            .min(b.per_arm.max_wall_secs.saturating_sub(arm_wall)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// 予約して投入してよい。値が予約額。
    Admit(Cap),
    /// 先行の予約が解けるまで投入しない（次の run が回収してから再判定する）。
    Defer,
    /// 何も in-flight でないのに残予算が予約を下回る: これ以上は投入できない（打ち切り）。
    Exhausted,
    /// この arm の枠だけが尽きた（既存の arm_exhausted 経路に任せる）。
    ArmExhausted,
}

/// 新規投入の可否。global の残り < 予約なら投入しない。arm の残りは他の in-flight の予約を差し引く。
pub fn admit(b: &Budget, s: &Spend, arm: ConfigId) -> Gate {
    let (at, aw) = s.arm.get(&arm).copied().unwrap_or_default();
    let cap = effective_cap(b, at, aw);
    if cap.max_tokens == 0 || cap.max_wall_secs == 0 {
        return if s.in_flight > 0 {
            Gate::Defer
        } else {
            Gate::ArmExhausted
        };
    }
    let g_tokens = b.max_tokens.saturating_sub(s.tokens);
    let g_wall = b.max_wall_secs.saturating_sub(s.wall_secs);
    if g_tokens < cap.max_tokens || g_wall < cap.max_wall_secs {
        return if s.in_flight > 0 {
            Gate::Defer
        } else {
            Gate::Exhausted
        };
    }
    Gate::Admit(cap)
}

/// 想定最大超過の式。入力はすべて明示値（実時間を測らない）。
/// - 上限保証: 確定済みの試行は campaign 上限を超えない（投入時に `残 >= 予約`）。超過は in-flight の
///   試行が予約を超えて消費した分だけで、`parallel × (poll_interval × 消費速度 + 終端確認までの時間 × 消費速度)` で抑える。
/// - 実行側の硬い上限（task の wall 上限）がある wall は `wall_cap` 以内。tokens は実行側上限が無いので poll 依存。
pub fn max_overshoot_tokens(
    max_parallel: u32,
    poll_interval_secs: u64,
    stop_confirm_secs: u64,
    burn_tokens_per_sec: u64,
) -> u64 {
    u64::from(max_parallel)
        .saturating_mul(poll_interval_secs.saturating_add(stop_confirm_secs))
        .saturating_mul(burn_tokens_per_sec)
}

pub const OVERSHOOT_FORMULA: &str = "global tokens never exceed max_tokens + overshoot; \
overshoot <= max_parallel * (poll_interval_secs + stop_confirm_secs) * burn_tokens_per_sec \
(tokens have no executor-side hard cap; wall is capped by the task wall limit min(trial_timeout, reserved wall)); \
reservation is released only when the task is terminal and, if cancelled, stop is confirmed";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overshoot_formula_is_linear_in_parallel_and_interval() {
        assert_eq!(max_overshoot_tokens(1, 60, 120, 1_000), 180_000);
        assert_eq!(max_overshoot_tokens(2, 60, 120, 1_000), 360_000);
        assert_eq!(max_overshoot_tokens(1, 0, 0, 1_000), 0);
    }
}
