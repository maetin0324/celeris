//! ADR-0061（Phase 104）: モデルの静的な単価表から `Usage` の推定コスト（USD）を計算する。
//!
//! **純粋関数のみ**（I/O・ネットワーク呼び出しはしない。ADR-0001 D2）。単価表はこのファイルに
//! 埋め込んだスナップショットであり、プロバイダの実際の請求額とは一致しない可能性がある
//! （取得時点の公開価格からの概算。運用側が実際の請求と突き合わせて `PRICE_TABLE` を更新する前提）。
//! モデル名は `HarnessSpec` または選択済み route の model 名（例: `"claude-sonnet-5"`,
//! `"gpt-5-codex"`）をそのまま渡す想定で、**前方一致**で単価表を引く（バージョン・日付付きの
//! モデル名にも耐えるため）。一致しないモデルは `None`（費用不明。0 円と偽らない）。

use crate::model::Usage;

/// 100 万トークンあたりの USD 単価。
#[derive(Debug, Clone, Copy)]
struct Price {
    input_per_million: Option<f64>,
    output_per_million: Option<f64>,
    cache_read_per_million: Option<f64>,
    cache_write_per_million: Option<f64>,
}

/// `PRICE_TABLE` の版 ID（ADR 2026-10-27-trial-observed-versions D3）。単価・行を変えたら必ず変える。
/// daemon は `GET /health` の `pricing_version` で公開し、campaign は試行の開始時に観測して宣言と照合する。
pub const PRICE_TABLE_VERSION: &str = "2026-09-approx-1";

/// 前方一致で引く単価表（先に書いた行が優先。長い/具体的なプレフィックスを先に置く）。
/// 2026-09 時点の公開価格からの概算スナップショット。
const PRICE_TABLE: &[(&str, Price)] = &[
    (
        "claude-fable-5-1",
        Price {
            input_per_million: Some(10.0),
            output_per_million: Some(50.0),
            cache_read_per_million: Some(0.25),
            cache_write_per_million: Some(12.5),
        },
    ),
    (
        "claude-opus-5-5",
        Price {
            input_per_million: Some(4.0),
            output_per_million: Some(20.0),
            cache_read_per_million: Some(0.20),
            cache_write_per_million: Some(5.0),
        },
    ),
    (
        "claude-sonnet-5",
        Price {
            input_per_million: Some(2.0),
            output_per_million: Some(10.0),
            cache_read_per_million: Some(0.2),
            cache_write_per_million: Some(2.5),
        },
    ),
    (
        "claude-haiku-4-5",
        Price {
            input_per_million: Some(1.0),
            output_per_million: Some(5.0),
            cache_read_per_million: Some(0.1),
            cache_write_per_million: Some(1.25),
        },
    ),
    (
        "gpt-6-astra",
        Price {
            input_per_million: None,
            output_per_million: None,
            cache_read_per_million: None,
            cache_write_per_million: None,
        },
    ),
    (
        "gpt-6-sol",
        Price {
            input_per_million: None,
            output_per_million: None,
            cache_read_per_million: None,
            cache_write_per_million: None,
        },
    ),
    (
        "gpt-6-luna",
        Price {
            input_per_million: None,
            output_per_million: None,
            cache_read_per_million: None,
            cache_write_per_million: None,
        },
    ),
    (
        "claude-opus",
        Price {
            input_per_million: Some(15.0),
            output_per_million: Some(75.0),
            cache_read_per_million: Some(1.5),
            cache_write_per_million: Some(18.75),
        },
    ),
    (
        "claude-sonnet",
        Price {
            input_per_million: Some(3.0),
            output_per_million: Some(15.0),
            cache_read_per_million: Some(0.3),
            cache_write_per_million: Some(3.75),
        },
    ),
    (
        "claude-haiku",
        Price {
            input_per_million: Some(0.8),
            output_per_million: Some(4.0),
            cache_read_per_million: Some(0.08),
            cache_write_per_million: Some(1.0),
        },
    ),
    (
        "gpt-5",
        Price {
            input_per_million: Some(1.25),
            output_per_million: Some(10.0),
            cache_read_per_million: Some(0.125),
            cache_write_per_million: None,
        },
    ),
];

fn price_for(model: &str) -> Option<Price> {
    let model = model.to_ascii_lowercase();
    PRICE_TABLE
        .iter()
        .find(|(prefix, _)| model.starts_with(prefix))
        .map(|(_, price)| *price)
}

/// ADR-0074 D4.2（Phase F3 quota）: `r_out`（重み付きトークンの出力/入力比）を単価表から求める。
/// 単価表にモデルが無い、または入力・出力どちらかの単価が無ければ `None`（呼び出し側は
/// `task_core::quota::default_r_out` の provider 既定にフォールバックする）。
pub fn output_input_ratio(model: &str) -> Option<f64> {
    let price = price_for(model)?;
    let input = price.input_per_million?;
    if input <= 0.0 {
        return None;
    }
    Some(price.output_per_million? / input)
}

/// `model`（前方一致で単価表を引く）と `usage` から USD の推定コストを計算する。
/// 単価表に無いモデル、もしくは `usage` にトークンが 1 件も無ければ `None`。
pub fn estimate_cost_usd(model: &str, usage: &Usage) -> Option<f64> {
    if usage.input_tokens.is_none()
        && usage.output_tokens.is_none()
        && usage.cache_read_tokens.is_none()
        && usage.cache_creation_tokens.is_none()
    {
        return None;
    }
    let price = price_for(model)?;
    fn part(tokens: Option<u64>, rate: Option<f64>) -> Option<f64> {
        let tokens = tokens.unwrap_or(0);
        if tokens == 0 {
            Some(0.0)
        } else {
            Some(tokens as f64 / 1_000_000.0 * rate?)
        }
    }
    let cost = part(usage.input_tokens, price.input_per_million)?
        + part(usage.output_tokens, price.output_per_million)?
        + part(usage.cache_read_tokens, price.cache_read_per_million)?
        + part(usage.cache_creation_tokens, price.cache_write_per_million)?;
    Some(cost)
}

/// ADR 2026-11-06（予算の USD 換算）D2: 人が config `[budget_rates."<model の接頭辞>"]` に書く換算単価
/// （100 万 token あたりの USD。4 欄すべて必須）。単価表に引けない model の予算判定にだけ使い、
/// `usage.cost_usd`・campaign の計上・`PRICE_TABLE_VERSION` には入らない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetRate {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: f64,
    pub cache_write_per_million: f64,
}

/// 換算単価の集合（接頭辞の長い順に引く）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BudgetRates {
    rows: Vec<(String, BudgetRate)>,
}

impl BudgetRates {
    /// 検査して作る。単価が有限で 0 以上でない、接頭辞が空、単価表で 4 種すべての単価がある行に
    /// 一致する接頭辞は `Err`（表を上書きさせない）。
    pub fn new(rows: impl IntoIterator<Item = (String, BudgetRate)>) -> Result<Self, String> {
        let mut out: Vec<(String, BudgetRate)> = Vec::new();
        for (prefix, rate) in rows {
            let prefix = prefix.to_ascii_lowercase();
            if prefix.is_empty() {
                return Err("budget_rates の model 接頭辞が空です".to_string());
            }
            for (name, v) in [
                ("input_per_million", rate.input_per_million),
                ("output_per_million", rate.output_per_million),
                ("cache_read_per_million", rate.cache_read_per_million),
                ("cache_write_per_million", rate.cache_write_per_million),
            ] {
                if !v.is_finite() || v < 0.0 {
                    return Err(format!(
                        "budget_rates.\"{prefix}\".{name} は有限で 0 以上の数にしてください"
                    ));
                }
            }
            if let Some(price) = price_for(&prefix)
                && price.fully_priced()
            {
                return Err(format!(
                    "budget_rates.\"{prefix}\" は単価表（PRICE_TABLE）に 4 種すべての単価がある model です。\
                     表は上書きできません"
                ));
            }
            out.retain(|(p, _)| *p != prefix);
            out.push((prefix, rate));
        }
        out.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        Ok(BudgetRates { rows: out })
    }

    fn lookup(&self, model: &str) -> Option<BudgetRate> {
        self.rows
            .iter()
            .find(|(prefix, _)| model.starts_with(prefix.as_str()))
            .map(|(_, r)| *r)
    }
}

impl Price {
    fn fully_priced(&self) -> bool {
        self.input_per_million.is_some()
            && self.output_per_million.is_some()
            && self.cache_read_per_million.is_some()
            && self.cache_write_per_million.is_some()
    }

    /// ADR 2026-11-06-phase5-n0-usd-accounting-contract D3: 予算判定の cache creation の単価。表の
    /// `cache_write_per_million` は 5 分 TTL の値だが、Claude Code は 1 時間 TTL（input の 2 倍）で書き、`Usage` は
    /// TTL を分けないので、大きい方に倒す（小さく数えない）。write の単価が無い行は `None`（費用不明）のまま。
    fn budget_cache_write_per_million(&self) -> Option<f64> {
        let write = self.cache_write_per_million?;
        Some(match self.input_per_million {
            Some(input) => write.max(2.0 * input),
            None => write,
        })
    }
}

/// 1 run の予算判定用の費用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunBudgetCost {
    /// micro-USD（1e-6 USD。小さく数えないよう切り上げ）。
    Known(u64),
    /// 単価が引けない・usage が無い（上限に達したものとして止める。0 として数えない）。
    Unknown,
}

/// 予算判定の費用の和。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BudgetCost {
    pub usd_micros: u64,
    /// 費用不明の run の数（1 件以上なら fail-closed で止める）。
    pub unknown_runs: u32,
}

impl BudgetCost {
    pub fn add(&mut self, cost: RunBudgetCost) {
        match cost {
            RunBudgetCost::Known(m) => self.usd_micros = self.usd_micros.saturating_add(m),
            RunBudgetCost::Unknown => self.unknown_runs = self.unknown_runs.saturating_add(1),
        }
    }

    pub fn usd(&self) -> f64 {
        self.usd_micros as f64 / 1_000_000.0
    }
}

/// USD を micro-USD に切り上げる（浮動小数の誤差 1e-7 は無視）。
pub fn usd_to_micros_ceil(usd: f64) -> u64 {
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    ((usd * 1_000_000.0) - 1e-7).ceil().max(0.0) as u64
}

/// route の `<source>/<model>` 接頭辞を除いた model 名（最後の `/` より後）。
fn bare_model(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// 1 run の予算判定用の費用。`usage` が無い、token が 1 件も無い、model が無い、単価が引けない
/// （単価表に無い・使った token 種の単価が `None`・`budget_rates` にも無い）run は `Unknown`。
/// 保存済みの `usage.cost_usd` は使わず、model と usage から今の単価表で再計算する。cache read は
/// cache read の単価、cache creation は単価表の行なら 1 時間 TTL の書き込み単価（input の 2 倍）と
/// cache write の単価の大きい方、`budget_rates` の行なら人が書いた cache write の単価で数える
/// （ADR 2026-11-06-phase5-n0-usd-accounting-contract D3）。
pub fn run_budget_cost(
    model: Option<&str>,
    usage: Option<&Usage>,
    rates: &BudgetRates,
) -> RunBudgetCost {
    let (Some(model), Some(usage)) = (model, usage) else {
        return RunBudgetCost::Unknown;
    };
    if usage.input_tokens.is_none()
        && usage.output_tokens.is_none()
        && usage.cache_read_tokens.is_none()
        && usage.cache_creation_tokens.is_none()
    {
        return RunBudgetCost::Unknown;
    }
    let model = bare_model(model).to_ascii_lowercase();
    let kinds = [
        usage.input_tokens.unwrap_or(0),
        usage.output_tokens.unwrap_or(0),
        usage.cache_read_tokens.unwrap_or(0),
        usage.cache_creation_tokens.unwrap_or(0),
    ];
    let from_table = price_for(&model).and_then(|p| {
        let r = [
            p.input_per_million,
            p.output_per_million,
            p.cache_read_per_million,
            p.budget_cache_write_per_million(),
        ];
        let mut sum = 0.0;
        for (tokens, rate) in kinds.iter().zip(r) {
            if *tokens == 0 {
                continue;
            }
            sum += *tokens as f64 * rate?;
        }
        Some(sum)
    });
    let usd_micro_f = from_table.or_else(|| {
        rates.lookup(&model).map(|r| {
            kinds[0] as f64 * r.input_per_million
                + kinds[1] as f64 * r.output_per_million
                + kinds[2] as f64 * r.cache_read_per_million
                + kinds[3] as f64 * r.cache_write_per_million
        })
    });
    match usd_micro_f {
        // tokens × (USD / 100 万 token) = micro-USD。
        Some(m) => RunBudgetCost::Known(usd_to_micros_ceil(m / 1_000_000.0)),
        None => RunBudgetCost::Unknown,
    }
}

/// run 群（`(model, usage)`）の費用の和。単価なし・usage なしは `unknown_runs` に数える。
pub fn budget_cost<'a>(
    runs: impl IntoIterator<Item = (Option<&'a str>, Option<&'a Usage>)>,
    rates: &BudgetRates,
) -> BudgetCost {
    let mut total = BudgetCost::default();
    for (model, usage) in runs {
        total.add(run_budget_cost(model, usage, rates));
    }
    total
}

#[cfg(test)]
#[path = "pricing/budget_ttl_tests.rs"]
mod budget_ttl_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_computes_from_all_four_token_kinds() {
        let usage = Usage {
            input_tokens: Some(1_000_000),
            output_tokens: Some(500_000),
            cache_read_tokens: Some(2_000_000),
            cache_creation_tokens: Some(1_000_000),
            cost_usd: None,
            duplicate_reads: None,
            session_resumed: None,
            context_tokens: None,
        };
        let cost = estimate_cost_usd("claude-sonnet-5", &usage).expect("known model");
        // 2.0*1 + 10.0*0.5 + 0.2*2 + 2.5*1 = 9.9
        assert!((cost - 9.9).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn version_suffixes_still_match_by_prefix() {
        let usage = Usage {
            input_tokens: Some(1_000_000),
            ..Usage::default()
        };
        assert!(estimate_cost_usd("claude-opus-4-20260101", &usage).is_some());
        assert!(estimate_cost_usd("gpt-5-codex", &usage).is_some());
    }

    #[test]
    fn unknown_model_is_none() {
        let usage = Usage {
            input_tokens: Some(1000),
            ..Usage::default()
        };
        assert_eq!(estimate_cost_usd("some-local-llm", &usage), None);
    }

    #[test]
    fn default_claude_frontier_has_a_price() {
        let usage = Usage {
            input_tokens: Some(1_000_000),
            ..Usage::default()
        };
        assert_eq!(estimate_cost_usd("claude-fable-5-1", &usage), Some(10.0));
    }

    #[test]
    fn default_gpt_frontier_has_no_offline_price_source() {
        // docs/ と config/ には gpt-6-astra の単価根拠がない。
        let usage = Usage {
            input_tokens: Some(1_000_000),
            ..Usage::default()
        };
        assert_eq!(estimate_cost_usd("gpt-6-astra", &usage), None);
    }

    #[test]
    fn nonzero_tokens_with_missing_price_are_unknown() {
        let usage = Usage {
            cache_creation_tokens: Some(1),
            ..Usage::default()
        };
        assert_eq!(estimate_cost_usd("gpt-5-codex", &usage), None);
        let zero = Usage {
            cache_creation_tokens: Some(0),
            input_tokens: Some(1_000_000),
            ..Usage::default()
        };
        assert_eq!(estimate_cost_usd("gpt-5-codex", &zero), Some(1.25));
    }

    #[test]
    fn no_tokens_at_all_is_none_even_for_a_known_model() {
        assert_eq!(
            estimate_cost_usd("claude-sonnet-5", &Usage::default()),
            None
        );
    }

    #[test]
    fn output_input_ratio_of_a_known_model() {
        // claude-sonnet-5: output 10.0 / input 2.0 = 5.0
        let ratio = output_input_ratio("claude-sonnet-5").expect("known model");
        assert!((ratio - 5.0).abs() < 1e-9, "{ratio}");
    }

    #[test]
    fn output_input_ratio_of_an_unpriced_model_is_none() {
        assert_eq!(output_input_ratio("gpt-6-sol"), None);
        assert_eq!(output_input_ratio("some-local-llm"), None);
    }

    #[test]
    fn missing_fields_are_treated_as_zero_not_as_unknown() {
        let usage = Usage {
            output_tokens: Some(1_000_000),
            ..Usage::default()
        };
        let cost = estimate_cost_usd("claude-haiku-4", &usage).expect("known model");
        assert!((cost - 4.0).abs() < 1e-9, "{cost}");
    }

    fn usage(i: u64, o: u64, cr: u64, cw: u64) -> Usage {
        Usage {
            input_tokens: Some(i),
            output_tokens: Some(o),
            cache_read_tokens: Some(cr),
            cache_creation_tokens: Some(cw),
            ..Usage::default()
        }
    }

    fn local_rate() -> BudgetRate {
        BudgetRate {
            input_per_million: 1.0,
            output_per_million: 2.0,
            cache_read_per_million: 0.5,
            cache_write_per_million: 0.0,
        }
    }

    /// 現行の model（2026-11）が単価表を前方一致で引ける。単価を変えたら PRICE_TABLE_VERSION も変える。
    #[test]
    fn current_claude_models_resolve_by_prefix() {
        assert_eq!(PRICE_TABLE_VERSION, "2026-09-approx-1");
        let u = usage(1_000_000, 1_000_000, 1_000_000, 1_000_000);
        // (model, input + output + cache read + cache write の USD)
        let expect = [
            ("claude-sonnet-5-5", 2.0 + 10.0 + 0.2 + 2.5),
            ("claude-sonnet-5-5-20261001", 2.0 + 10.0 + 0.2 + 2.5),
            ("claude-opus-5-5", 4.0 + 20.0 + 0.20 + 5.0),
            ("claude-haiku-5-5", 0.8 + 4.0 + 0.08 + 1.0),
            ("claude-fable-5-1", 10.0 + 50.0 + 0.25 + 12.5),
        ];
        for (model, usd) in expect {
            let c = estimate_cost_usd(model, &u).unwrap_or_else(|| panic!("{model} が引けない"));
            assert!((c - usd).abs() < 1e-9, "{model}: {c} != {usd}");
        }
    }

    #[test]
    fn budget_cost_counts_cache_at_cache_prices() {
        let rates = BudgetRates::default();
        // 同じ総 token 数 1,000,000 でも、cache read 主体は input 主体よりずっと安い。
        let heavy_cache = run_budget_cost(
            Some("claude-sonnet-5-5"),
            Some(&usage(0, 0, 1_000_000, 0)),
            &rates,
        );
        let plain = run_budget_cost(
            Some("claude-sonnet-5-5"),
            Some(&usage(1_000_000, 0, 0, 0)),
            &rates,
        );
        assert_eq!(heavy_cache, RunBudgetCost::Known(200_000));
        assert_eq!(plain, RunBudgetCost::Known(2_000_000));
        let write = run_budget_cost(
            Some("claude-sonnet-5-5"),
            Some(&usage(0, 0, 0, 1_000_000)),
            &rates,
        );
        // cache creation は 1 時間 TTL の書き込み単価（input 2.0 の 2 倍 = 4.0）で数える（N0 ADR D3）。
        assert_eq!(write, RunBudgetCost::Known(4_000_000));
    }

    #[test]
    fn budget_cost_ignores_stored_cost_and_strips_route_source() {
        let mut u = usage(1_000_000, 0, 0, 0);
        u.cost_usd = Some(99.0);
        assert_eq!(
            run_budget_cost(
                Some("anthropic/claude-sonnet-5-5"),
                Some(&u),
                &BudgetRates::default()
            ),
            RunBudgetCost::Known(2_000_000)
        );
    }

    #[test]
    fn budget_cost_rounds_up_to_micro_usd() {
        // 1 token × 2.0 USD/100 万 token = 2 micro-USD。0.5 micro-USD は 1 に切り上げる。
        let c = run_budget_cost(
            Some("claude-haiku-5-5"),
            Some(&usage(0, 0, 1, 0)),
            &BudgetRates::default(),
        );
        assert_eq!(c, RunBudgetCost::Known(1));
    }

    #[test]
    fn unpriced_model_and_null_usage_are_unknown_not_zero() {
        let rates = BudgetRates::default();
        let u = usage(1000, 1000, 0, 0);
        assert_eq!(
            run_budget_cost(Some("qwen3-local"), Some(&u), &rates),
            RunBudgetCost::Unknown
        );
        assert_eq!(
            run_budget_cost(Some("gpt-6-sol"), Some(&u), &rates),
            RunBudgetCost::Unknown
        );
        // gpt-5 は cache write の単価が無い: cache write を使った run だけ不明。
        assert_eq!(
            run_budget_cost(Some("gpt-5"), Some(&usage(10, 0, 0, 5)), &rates),
            RunBudgetCost::Unknown
        );
        assert!(matches!(
            run_budget_cost(Some("gpt-5"), Some(&usage(10, 0, 5, 0)), &rates),
            RunBudgetCost::Known(_)
        ));
        assert_eq!(
            run_budget_cost(Some("claude-sonnet-5-5"), None, &rates),
            RunBudgetCost::Unknown
        );
        assert_eq!(
            run_budget_cost(Some("claude-sonnet-5-5"), Some(&Usage::default()), &rates),
            RunBudgetCost::Unknown
        );
        assert_eq!(
            run_budget_cost(None, Some(&u), &rates),
            RunBudgetCost::Unknown
        );
        let total = budget_cost(
            [
                (Some("claude-sonnet-5-5"), Some(&usage(1_000_000, 0, 0, 0))),
                (Some("qwen3-local"), Some(&u)),
                (Some("claude-sonnet-5-5"), None),
            ],
            &rates,
        );
        assert_eq!(
            total,
            BudgetCost {
                usd_micros: 2_000_000,
                unknown_runs: 2
            }
        );
    }

    #[test]
    fn budget_rates_price_local_models_and_longest_prefix_wins() {
        let rates = BudgetRates::new([
            ("qwen".to_string(), local_rate()),
            (
                "qwen3-free".to_string(),
                BudgetRate {
                    input_per_million: 0.0,
                    output_per_million: 0.0,
                    cache_read_per_million: 0.0,
                    cache_write_per_million: 0.0,
                },
            ),
        ])
        .expect("valid");
        let u = usage(1_000_000, 1_000_000, 2_000_000, 0);
        // 1 + 2 + 0.5×2 = 4 USD
        assert_eq!(
            run_budget_cost(Some("opencode-qwen/qwen3.8-flash"), Some(&u), &rates),
            RunBudgetCost::Known(4_000_000)
        );
        // 明示した 0 は 0（人の宣言）。
        assert_eq!(
            run_budget_cost(Some("qwen3-free-x"), Some(&u), &rates),
            RunBudgetCost::Known(0)
        );
    }

    #[test]
    fn budget_rates_reject_table_override_and_bad_values() {
        assert!(BudgetRates::new([("claude-sonnet-5-5".to_string(), local_rate())]).is_err());
        // gpt-5 は cache write 単価が無いので、人が足せる。
        assert!(BudgetRates::new([("gpt-5".to_string(), local_rate())]).is_ok());
        for bad in [f64::NAN, -1.0, f64::INFINITY] {
            let mut r = local_rate();
            r.output_per_million = bad;
            assert!(BudgetRates::new([("qwen".to_string(), r)]).is_err());
        }
        assert!(BudgetRates::new([(String::new(), local_rate())]).is_err());
    }
}
