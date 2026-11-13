//! quota の run 単位の入口（ADR-0074 D4、ADR-0076）。ADR-0082 の L1。状態・見積りの判断・適用は `routing_runtime/quota*.rs`。

use super::*;

impl Dispatcher {
    /// ADR-0074 D4（Phase F3 quota）: run の開始で、そのアカウントの現在の観測値をグループの
    /// `before` として `QuotaActivity` に登録する。アカウントプールを使わない run
    /// （`account`/`account_adapter` が `None`）は何もしない（D4.2 手順 5 の `free` は完了時に
    /// 決める。プールが無いのでそもそも重なりを追う意味が無い）。ADR-0076: worker / planner /
    /// reviewer のどの run でも呼ぶ。呼んだ run は終了のすべての経路で `resolve_quota_estimate`
    /// か `release_quota_if_tracked` を通すこと（開いたまま残ると重なりの判定が狂う）。
    pub(super) fn quota_begin(
        &mut self,
        account: Option<&str>,
        account_adapter: Option<AccountAdapter>,
        run_id: &str,
    ) {
        let now = (self.now_unix_fn)();
        self.routing
            .quota_begin(account.zip(account_adapter), run_id, now);
    }

    /// ADR-0074 D4 / ADR-0076: `run_id` の `Event::WorkerStarted.model`（quota の `r_out` を引く
    /// ため。読めなければ空文字 = 既定の比）。
    pub(super) fn started_model_of(&self, task_id: TaskId, run_id: &str) -> String {
        self.store
            .events_for(task_id)
            .ok()
            .and_then(|events_so_far| {
                events_so_far.iter().rev().find_map(|(_, e)| match e {
                    Event::WorkerStarted {
                        run_id: r, model, ..
                    } if r == run_id => Some(model.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default()
    }

    /// ADR-0074 D4（Phase F3 quota）: 何も永続化しない早期 return（不明なタスク・stale な結果の
    /// 破棄）でも `QuotaActivity` の bookkeeping だけは必ず閉じる（さもないと重なりの判定が永遠に
    /// 狂う）。`is_tracked` で「`quota_begin` を呼んだ run か」を確かめてから呼ぶ（誤って無関係な
    /// 他の run のグループを壊さないための防御）。戻り値（この run 自身の Event）は捨てる（グループが閉じて
    /// 他のメンバー分が確定すれば、それらは `resolve_quota_estimate` の中で別タスクへ直接書かれる）。
    pub(super) fn release_quota_if_tracked(
        &mut self,
        run_id: &str,
        account: Option<&str>,
        account_adapter: Option<AccountAdapter>,
        provider: &ProviderId,
        task_id: TaskId,
    ) {
        let (Some(acct), Some(adapter)) = (account, account_adapter) else {
            return;
        };
        if !self.routing.quota_is_tracked(adapter, acct, run_id) {
            return;
        }
        let _ = self.resolve_quota_estimate(
            task_id,
            run_id,
            None,
            Some(acct),
            Some(adapter),
            provider,
            "",
            None,
        );
    }

    /// ADR-0074 D4（Phase F3 quota）: run の終了で quota 消費を決定的に見積もり、この run 自身の
    /// `Event::QuotaEstimated` を返す。重なった run のグループがこの run で閉じた場合は、他の
    /// メンバーの分の Event を該当タスクへ直接書く（`store.append_event`。「同じ run_id は最後の
    /// Event が有効」なので、それらの run の以前の暫定 Event を上書きする）。呼び出し側は戻り値を
    /// 自分の `events` に足す（`WorkerFinished` と同じトランザクション）。
    ///
    /// `usage` が `None`（stale な結果の破棄など、`WorkerFinished` 自体を残さない経路）でも呼んでよい
    /// （`QuotaActivity` の bookkeeping を必ず閉じるため）。戻り値は使わなくてよい。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve_quota_estimate(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        work_unit_id: Option<String>,
        account: Option<&str>,
        account_adapter: Option<AccountAdapter>,
        provider: &ProviderId,
        model: &str,
        usage: Option<&task_core::Usage>,
    ) -> Event {
        let pool = account.zip(account_adapter);
        self.routing.settle_quota(
            &*self.store,
            crate::routing_runtime::quota::QuotaRunEnd {
                task_id,
                run_id,
                work_unit_id,
                pool,
                provider_source: provider.to_string(),
                model,
                usage,
            },
        )
    }
}
