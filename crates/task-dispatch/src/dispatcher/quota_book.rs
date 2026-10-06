//! quota の見積りと release（ADR-0074 D4、ADR-0076）。ADR-0082 の L1。

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
        let (Some(account_id), Some(adapter)) = (account, account_adapter) else {
            return;
        };
        let before = self.account_book(adapter).and_then(|book| {
            book.lock()
                .ok()
                .and_then(|guard| guard.state(account_id).and_then(|s| s.usage.clone()))
        });
        let now = (self.now_unix_fn)();
        let before_valid = before
            .as_ref()
            .is_some_and(|obs| task_core::quota::before_is_valid(obs.observed_at, now, false));
        self.quota_activity
            .begin(adapter, account_id, run_id, before, before_valid);
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
        if !self.quota_activity.is_tracked(adapter, acct, run_id) {
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

    /// D4.2: `window` 1 つを、measured → apportioned → estimated → unknown の優先順位で決める
    /// （較正の取得はここでは行わない。呼び出し側が `calibration` を渡す）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn decide_quota_window(
        window: task_core::QuotaWindow,
        before: Option<&RateLimitObservation>,
        after: Option<&RateLimitObservation>,
        before_valid: bool,
        exclusive: bool,
        apportioned: Option<&task_core::quota::ApportionedInputs>,
        weighted_tokens: f64,
        calibration: Option<task_core::QuotaCalibration>,
    ) -> task_core::QuotaWindowUse {
        let measured = task_core::quota::MeasuredInputs {
            before: task_core::quota::snapshot(before, window),
            after: task_core::quota::snapshot(after, window),
            before_valid,
            exclusive,
        };
        task_core::quota::decide_window(
            window,
            &measured,
            apportioned,
            weighted_tokens,
            calibration,
        )
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
        let usage_owned = usage.copied().unwrap_or_default();
        let list_price_usd = usage_owned.cost_usd;
        let weighted_tokens_with_default_r_out = |default_r_out: f64| {
            let r_out = task_core::output_input_ratio(model).unwrap_or(default_r_out);
            task_core::quota::weighted_tokens(&usage_owned, r_out)
        };

        let (Some(account_id), Some(adapter)) = (account, account_adapter) else {
            // D4.2 手順 5: アカウントプールを使わない run は free（Qwen などローカル/従量でない供給元）。
            let source = provider.to_string();
            let weighted_tokens =
                weighted_tokens_with_default_r_out(task_core::quota::default_r_out(&source));
            let windows = vec![
                task_core::quota::free_window(task_core::QuotaWindow::FiveHour),
                task_core::quota::free_window(task_core::QuotaWindow::SevenDay),
            ];
            let method = task_core::quota::representative_method(&windows);
            return Event::QuotaEstimated {
                run_id: run_id.to_string(),
                work_unit_id,
                source,
                account: None,
                windows,
                weighted_tokens,
                method,
                calibration: None,
                weights_version: task_core::quota::WEIGHTS_VERSION.to_string(),
                list_price_usd,
            };
        };

        let source = crate::accounts::quota_source_label(adapter).to_string();
        let weighted_tokens =
            weighted_tokens_with_default_r_out(task_core::quota::default_r_out(&source));
        let after = self.account_book(adapter).and_then(|book| {
            book.lock()
                .ok()
                .and_then(|guard| guard.state(account_id).and_then(|s| s.usage.clone()))
        });
        let member = crate::accounts::QuotaGroupMember {
            task_id,
            run_id: run_id.to_string(),
            work_unit_id: work_unit_id.clone(),
            source: source.clone(),
            weighted_tokens,
            list_price_usd,
        };
        let outcome = self.quota_activity.end(adapter, account_id, member);

        match outcome {
            crate::accounts::QuotaEndOutcome::Exclusive {
                before,
                before_valid,
            } => {
                let mut calibration_used = None;
                let windows: Vec<_> = quota_windows_for(adapter)
                    .into_iter()
                    .map(|w| {
                        let calibration = self.quota_calibration.calibration(&source, w);
                        let qw = Self::decide_quota_window(
                            w,
                            before.as_ref(),
                            after.as_ref(),
                            before_valid,
                            true,
                            None,
                            weighted_tokens,
                            calibration,
                        );
                        if qw.method == task_core::QuotaMethod::Estimated {
                            calibration_used = calibration;
                        }
                        qw
                    })
                    .collect();
                for qw in &windows {
                    if qw.method == task_core::QuotaMethod::Measured
                        && let Some(pct) = qw.used_pct
                    {
                        self.quota_calibration
                            .record(&source, qw.window, pct, weighted_tokens);
                    }
                }
                let method = task_core::quota::representative_method(&windows);
                Event::QuotaEstimated {
                    run_id: run_id.to_string(),
                    work_unit_id,
                    source,
                    account: Some(account_id.to_string()),
                    windows,
                    weighted_tokens,
                    method,
                    calibration: calibration_used,
                    weights_version: task_core::quota::WEIGHTS_VERSION.to_string(),
                    list_price_usd,
                }
            }
            crate::accounts::QuotaEndOutcome::Pending => {
                // D4.2 手順 2: 「集合の最後の run が終わった時点で決まる。それまでは pending」。
                // 暫定の unknown を出す（グループが閉じたら `store.append_event` で上書きされる）。
                let windows = vec![
                    task_core::quota::decide_window(
                        task_core::QuotaWindow::FiveHour,
                        &task_core::quota::MeasuredInputs {
                            before: None,
                            after: None,
                            before_valid: false,
                            exclusive: false,
                        },
                        None,
                        weighted_tokens,
                        None,
                    ),
                    task_core::quota::decide_window(
                        task_core::QuotaWindow::SevenDay,
                        &task_core::quota::MeasuredInputs {
                            before: None,
                            after: None,
                            before_valid: false,
                            exclusive: false,
                        },
                        None,
                        weighted_tokens,
                        None,
                    ),
                ];
                Event::QuotaEstimated {
                    run_id: run_id.to_string(),
                    work_unit_id,
                    source,
                    account: Some(account_id.to_string()),
                    windows,
                    weighted_tokens,
                    method: task_core::QuotaMethod::Unknown,
                    calibration: None,
                    weights_version: task_core::quota::WEIGHTS_VERSION.to_string(),
                    list_price_usd,
                }
            }
            crate::accounts::QuotaEndOutcome::Closed {
                before,
                before_valid,
                members,
            } => {
                let total_weighted: f64 = members.iter().map(|m| m.weighted_tokens).sum();
                let mut own_event = None;
                for m in &members {
                    let windows: Vec<_> = quota_windows_for(adapter)
                        .into_iter()
                        .map(|w| {
                            let apportion = task_core::quota::ApportionedInputs {
                                group_before: task_core::quota::snapshot(before.as_ref(), w),
                                group_after: task_core::quota::snapshot(after.as_ref(), w),
                                group_before_valid: before_valid,
                                member_weighted_tokens: m.weighted_tokens,
                                group_weighted_tokens_total: total_weighted,
                            };
                            Self::decide_quota_window(
                                w,
                                None,
                                None,
                                false,
                                false,
                                Some(&apportion),
                                m.weighted_tokens,
                                None,
                            )
                        })
                        .collect();
                    let method = task_core::quota::representative_method(&windows);
                    let ev = Event::QuotaEstimated {
                        run_id: m.run_id.clone(),
                        work_unit_id: m.work_unit_id.clone(),
                        source: m.source.clone(),
                        account: Some(account_id.to_string()),
                        windows,
                        weighted_tokens: m.weighted_tokens,
                        method,
                        calibration: None,
                        weights_version: task_core::quota::WEIGHTS_VERSION.to_string(),
                        list_price_usd: m.list_price_usd,
                    };
                    if m.run_id == run_id {
                        own_event = Some(ev);
                    } else if let Err(e) = self.store.append_event(m.task_id, &ev) {
                        tracing::warn!(task_id = %m.task_id, run_id = %m.run_id, error = %e, "failed to record the apportioned quota event for a group member");
                    }
                }
                own_event.unwrap_or_else(|| Event::QuotaEstimated {
                    run_id: run_id.to_string(),
                    work_unit_id,
                    source,
                    account: Some(account_id.to_string()),
                    windows: vec![],
                    weighted_tokens,
                    method: task_core::QuotaMethod::Unknown,
                    calibration: None,
                    weights_version: task_core::quota::WEIGHTS_VERSION.to_string(),
                    list_price_usd,
                })
            }
        }
    }
}

/// quota の見積りで見る窓（ADR 2026-10-06 D1）。1 か月窓を持つのは opencode go だけで、claude / codex の
/// event には従来どおり 5 時間と 7 日だけを載せる。
fn quota_windows_for(adapter: AccountAdapter) -> Vec<task_core::QuotaWindow> {
    let mut windows = vec![
        task_core::QuotaWindow::FiveHour,
        task_core::QuotaWindow::SevenDay,
    ];
    if adapter == AccountAdapter::OpencodeGo {
        windows.push(task_core::QuotaWindow::OneMonth);
    }
    windows
}
