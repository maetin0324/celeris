//! quota の窓と較正の状態の下位 owner（ADR-0074 D4・ADR-0076、ADR 2026-11-11 dispatcher-state-ownership D1・D3・D8）。
//!
//! [`QuotaBook`] は run の重なりの追跡（`QuotaActivity`）と `estimated` の較正材料（`QuotaCalibrationBook`）を持つ。
//! どちらも揮発で、再起動で失われても次の run が `estimated`/`unknown` に倒れるだけ（値を捏造しない）。
//! 見積りの判断は純粋関数 [`super::quota_estimate::propose_estimate`]。この module の `RoutingRuntime` の method は、
//! 観測値を帳簿から読み、提案を受けて較正の更新と他の member の event の追記を行う（適用）。

use task_core::{AccountAdapter, Event, SqliteStore, TaskId, Usage};

use super::RoutingRuntime;
use super::quota_estimate::{
    EndShape, EstimateInput, propose_estimate, weighted_tokens_of, windows_for,
};
use crate::accounts::{QuotaActivity, QuotaCalibrationBook, QuotaGroupMember};

/// quota の窓と較正の状態。
pub struct QuotaBook {
    /// ADR-0074 D4（Phase F3 quota）: run の重なりを追跡し、measured/apportioned/pending を決定する
    /// （プロセス内メモリのみ。`AccountBook` とは別軸で、replay の対象外）。
    quota_activity: QuotaActivity,
    /// ADR-0074 D4.2（Phase F3 quota）: `estimated` の較正材料（source × 窓ごとの直近 `measured`）。
    quota_calibration: QuotaCalibrationBook,
}

impl QuotaBook {
    pub(crate) fn new() -> Self {
        Self {
            quota_activity: QuotaActivity::new(),
            quota_calibration: QuotaCalibrationBook::new(),
        }
    }
}

impl RoutingRuntime {
    /// run の開始。そのアカウントの現在の観測値をグループの `before` として登録する。
    /// プールを使わない run（`pool` が `None`）は何もしない。`now` は Unix 秒。
    pub(crate) fn quota_begin(
        &mut self,
        pool: Option<(&str, AccountAdapter)>,
        run_id: &str,
        now: i64,
    ) {
        let Some((account_id, adapter)) = pool else {
            return;
        };
        let before = self.observed_usage(adapter, account_id);
        let before_valid = before
            .as_ref()
            .is_some_and(|obs| task_core::quota::before_is_valid(obs.observed_at, now, false));
        self.quota
            .quota_activity
            .begin(adapter, account_id, run_id, before, before_valid);
    }

    /// `quota_begin` を呼んで、まだ閉じていない run か。
    pub(crate) fn quota_is_tracked(
        &self,
        adapter: AccountAdapter,
        account_id: &str,
        run_id: &str,
    ) -> bool {
        self.quota
            .quota_activity
            .is_tracked(adapter, account_id, run_id)
    }

    fn observed_usage(
        &self,
        adapter: AccountAdapter,
        account_id: &str,
    ) -> Option<task_core::RateLimitObservation> {
        self.account_book(adapter).and_then(|book| {
            book.lock()
                .ok()
                .and_then(|guard| guard.state(account_id).and_then(|s| s.usage.clone()))
        })
    }

    /// run の終わりで quota 消費を見積もり、この run 自身の `Event::QuotaEstimated` を返す。
    /// 見積りの判断は純粋関数に任せ、ここでは提案を適用する: 重なりの追跡を閉じ、`measured` の標本を較正へ足し、
    /// グループがこの run で閉じたときの他の member の event をその task へ直接追記する
    /// （「同じ run_id は最後の event が有効」なので、以前の暫定 event を上書きする）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn settle_quota(
        &mut self,
        store: &SqliteStore,
        task_id: TaskId,
        run_id: &str,
        work_unit_id: Option<String>,
        pool: Option<(&str, AccountAdapter)>,
        provider_source: String,
        model: &str,
        usage: Option<&Usage>,
    ) -> Event {
        let usage = usage.copied().unwrap_or_default();
        let list_price_usd = usage.cost_usd;
        let input = match pool {
            None => EstimateInput {
                run_id: run_id.to_string(),
                work_unit_id,
                weighted_tokens: weighted_tokens_of(&usage, model, &provider_source),
                source: provider_source,
                list_price_usd,
                calibrations: Vec::new(),
                shape: EndShape::Free,
            },
            Some((account_id, adapter)) => {
                let source = crate::accounts::quota_source_label(adapter).to_string();
                let weighted_tokens = weighted_tokens_of(&usage, model, &source);
                let after = self.observed_usage(adapter, account_id);
                let calibrations = windows_for(adapter)
                    .into_iter()
                    .filter_map(|w| {
                        self.quota
                            .quota_calibration
                            .calibration(&source, w)
                            .map(|c| (w, c))
                    })
                    .collect();
                let member = QuotaGroupMember {
                    task_id,
                    run_id: run_id.to_string(),
                    work_unit_id: work_unit_id.clone(),
                    source: source.clone(),
                    weighted_tokens,
                    list_price_usd,
                };
                let outcome = self.quota.quota_activity.end(adapter, account_id, member);
                EstimateInput {
                    run_id: run_id.to_string(),
                    work_unit_id,
                    source,
                    weighted_tokens,
                    list_price_usd,
                    calibrations,
                    shape: EndShape::Pooled {
                        adapter,
                        account: account_id.to_string(),
                        outcome: Box::new(outcome),
                        after,
                    },
                }
            }
        };
        let source = input.source.clone();
        let weighted_tokens = input.weighted_tokens;
        let proposal = propose_estimate(input);
        for sample in &proposal.samples {
            self.quota.quota_calibration.record(
                &source,
                sample.window,
                sample.used_pct,
                weighted_tokens,
            );
        }
        for (task_id, ev) in &proposal.others {
            if let Err(e) = store.append_event(*task_id, ev) {
                tracing::warn!(%task_id, error = %e, "failed to record the apportioned quota event for a group member");
            }
        }
        proposal.own
    }
}
