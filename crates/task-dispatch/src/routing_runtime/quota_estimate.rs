//! run の終わりの quota 消費の見積りの判断（ADR-0074 D4・ADR-0076、ADR 2026-11-11 の状態の所有 D3）。
//!
//! 純粋関数 [`propose_estimate`] は、run の終わりの結果（free か、重なりの追跡の結果）・観測値・較正・重み付きトークンを
//! [`EstimateInput`] で受け、この run の `Event::QuotaEstimated`、グループの他の member の event、較正へ足す標本を
//! [`EstimateProposal`] で返す。読み書き・時計・lock は持たない。提案の適用（event の追記と較正の更新）は
//! `routing_runtime/quota.rs` の owner の method が行う。

use task_core::{
    AccountAdapter, Event, QuotaCalibration, QuotaMethod, QuotaWindow, QuotaWindowUse,
    RateLimitObservation, TaskId, Usage,
};

use crate::accounts::QuotaEndOutcome;

/// quota の見積りで見る窓（ADR 2026-10-06 D1）。1 か月窓を持つのは opencode go だけで、claude / codex の
/// event には従来どおり 5 時間と 7 日だけを載せる。
pub(crate) fn windows_for(adapter: AccountAdapter) -> Vec<QuotaWindow> {
    let mut windows = vec![QuotaWindow::FiveHour, QuotaWindow::SevenDay];
    if adapter == AccountAdapter::OpencodeGo {
        windows.push(QuotaWindow::OneMonth);
    }
    windows
}

/// run の重み付きトークン。model に出力比が決まっていなければ `default_r_out`（source ごとの既定）を使う。
pub(crate) fn weighted_tokens_of(usage: &Usage, model: &str, source: &str) -> f64 {
    let r_out = task_core::output_input_ratio(model)
        .unwrap_or_else(|| task_core::quota::default_r_out(source));
    task_core::quota::weighted_tokens(usage, r_out)
}

/// run の終わりの形。
pub(crate) enum EndShape {
    /// アカウントプールを使わない run（Qwen などローカル/従量でない供給元）。D4.2 手順 5: free。
    Free,
    /// アカウントプールの run。`QuotaActivity::end` の結果と、その時点の観測値。
    Pooled {
        adapter: AccountAdapter,
        account: String,
        outcome: Box<QuotaEndOutcome>,
        after: Option<RateLimitObservation>,
    },
}

/// 見積りの入力（owner が集めた値の束）。
pub(crate) struct EstimateInput {
    pub(crate) run_id: String,
    pub(crate) work_unit_id: Option<String>,
    pub(crate) source: String,
    pub(crate) weighted_tokens: f64,
    pub(crate) list_price_usd: Option<f64>,
    /// 単独 run のときに使う、窓ごとの較正（無い窓は載せない）。
    pub(crate) calibrations: Vec<(QuotaWindow, QuotaCalibration)>,
    pub(crate) shape: EndShape,
}

/// `measured` になった窓の標本。適用の側が較正の材料に足す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CalibrationSample {
    pub(crate) window: QuotaWindow,
    pub(crate) used_pct: f64,
}

/// 見積りの提案。
pub(crate) struct EstimateProposal {
    /// この run 自身の event（呼び出し側が自分の events に足す）。
    pub(crate) own: Event,
    /// グループがこの run で閉じたときの、他の member の event（その task へ別に追記する）。
    pub(crate) others: Vec<(TaskId, Event)>,
    /// 較正へ足す標本（重み付きトークンは入力の値）。
    pub(crate) samples: Vec<CalibrationSample>,
}

/// D4.2: `window` 1 つを、measured → apportioned → estimated → unknown の優先順位で決める。
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_window(
    window: QuotaWindow,
    before: Option<&RateLimitObservation>,
    after: Option<&RateLimitObservation>,
    before_valid: bool,
    exclusive: bool,
    apportioned: Option<&task_core::quota::ApportionedInputs>,
    weighted_tokens: f64,
    calibration: Option<QuotaCalibration>,
) -> QuotaWindowUse {
    let measured = task_core::quota::MeasuredInputs {
        before: task_core::quota::snapshot(before, window),
        after: task_core::quota::snapshot(after, window),
        before_valid,
        exclusive,
    };
    task_core::quota::decide_window(window, &measured, apportioned, weighted_tokens, calibration)
}

#[allow(clippy::too_many_arguments)]
fn event(
    run_id: String,
    work_unit_id: Option<String>,
    source: String,
    account: Option<String>,
    windows: Vec<QuotaWindowUse>,
    weighted_tokens: f64,
    method: QuotaMethod,
    calibration: Option<QuotaCalibration>,
    list_price_usd: Option<f64>,
) -> Event {
    Event::QuotaEstimated {
        run_id,
        work_unit_id,
        source,
        account,
        windows,
        weighted_tokens,
        method,
        calibration,
        weights_version: task_core::quota::WEIGHTS_VERSION.to_string(),
        list_price_usd,
    }
}

/// 窓を何も測れない暫定の使用量（`unknown`）。
fn unknown_window(window: QuotaWindow, weighted_tokens: f64) -> QuotaWindowUse {
    decide_window(
        window,
        None,
        None,
        false,
        false,
        None,
        weighted_tokens,
        None,
    )
}

/// 見積りの判断。決定的で、入力だけから event と標本を作る。
pub(crate) fn propose_estimate(input: EstimateInput) -> EstimateProposal {
    let EstimateInput {
        run_id,
        work_unit_id,
        source,
        weighted_tokens,
        list_price_usd,
        calibrations,
        shape,
    } = input;
    let none = |own: Event| EstimateProposal {
        own,
        others: Vec::new(),
        samples: Vec::new(),
    };
    let (adapter, account, outcome, after) = match shape {
        EndShape::Free => {
            let windows = vec![
                task_core::quota::free_window(QuotaWindow::FiveHour),
                task_core::quota::free_window(QuotaWindow::SevenDay),
            ];
            let method = task_core::quota::representative_method(&windows);
            return none(event(
                run_id,
                work_unit_id,
                source,
                None,
                windows,
                weighted_tokens,
                method,
                None,
                list_price_usd,
            ));
        }
        EndShape::Pooled {
            adapter,
            account,
            outcome,
            after,
        } => (adapter, account, *outcome, after),
    };

    match outcome {
        QuotaEndOutcome::Exclusive {
            before,
            before_valid,
        } => {
            let mut calibration_used = None;
            let windows: Vec<_> = windows_for(adapter)
                .into_iter()
                .map(|w| {
                    let calibration = calibrations
                        .iter()
                        .find(|(window, _)| *window == w)
                        .map(|(_, c)| *c);
                    let qw = decide_window(
                        w,
                        before.as_ref(),
                        after.as_ref(),
                        before_valid,
                        true,
                        None,
                        weighted_tokens,
                        calibration,
                    );
                    if qw.method == QuotaMethod::Estimated {
                        calibration_used = calibration;
                    }
                    qw
                })
                .collect();
            let samples = windows
                .iter()
                .filter(|qw| qw.method == QuotaMethod::Measured)
                .filter_map(|qw| {
                    qw.used_pct.map(|used_pct| CalibrationSample {
                        window: qw.window,
                        used_pct,
                    })
                })
                .collect();
            let method = task_core::quota::representative_method(&windows);
            EstimateProposal {
                own: event(
                    run_id,
                    work_unit_id,
                    source,
                    Some(account),
                    windows,
                    weighted_tokens,
                    method,
                    calibration_used,
                    list_price_usd,
                ),
                others: Vec::new(),
                samples,
            }
        }
        // D4.2 手順 2: 「集合の最後の run が終わった時点で決まる。それまでは pending」。暫定の unknown を出す
        // （グループが閉じたら、閉じた run の提案の `others` が別に追記して上書きする）。
        QuotaEndOutcome::Pending => none(event(
            run_id,
            work_unit_id,
            source,
            Some(account),
            vec![
                unknown_window(QuotaWindow::FiveHour, weighted_tokens),
                unknown_window(QuotaWindow::SevenDay, weighted_tokens),
            ],
            weighted_tokens,
            QuotaMethod::Unknown,
            None,
            list_price_usd,
        )),
        QuotaEndOutcome::Closed {
            before,
            before_valid,
            members,
        } => {
            let total_weighted: f64 = members.iter().map(|m| m.weighted_tokens).sum();
            let mut own_event = None;
            let mut others = Vec::new();
            for m in &members {
                let windows: Vec<_> = windows_for(adapter)
                    .into_iter()
                    .map(|w| {
                        let apportion = task_core::quota::ApportionedInputs {
                            group_before: task_core::quota::snapshot(before.as_ref(), w),
                            group_after: task_core::quota::snapshot(after.as_ref(), w),
                            group_before_valid: before_valid,
                            member_weighted_tokens: m.weighted_tokens,
                            group_weighted_tokens_total: total_weighted,
                        };
                        decide_window(
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
                let ev = event(
                    m.run_id.clone(),
                    m.work_unit_id.clone(),
                    m.source.clone(),
                    Some(account.clone()),
                    windows,
                    m.weighted_tokens,
                    method,
                    None,
                    m.list_price_usd,
                );
                if m.run_id == run_id {
                    own_event = Some(ev);
                } else {
                    others.push((m.task_id, ev));
                }
            }
            let own = own_event.unwrap_or_else(|| {
                event(
                    run_id,
                    work_unit_id,
                    source,
                    Some(account),
                    vec![],
                    weighted_tokens,
                    QuotaMethod::Unknown,
                    None,
                    list_price_usd,
                )
            });
            EstimateProposal {
                own,
                others,
                samples: Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::QuotaGroupMember;

    fn obs(utilization: f64) -> RateLimitObservation {
        let window = |u: f64, resets_at: i64| task_core::RateWindow {
            utilization: u,
            resets_at,
        };
        RateLimitObservation {
            five_hour: Some(window(utilization, 20_000)),
            seven_day: Some(window(utilization / 2.0, 600_000)),
            one_month: None,
            status: None,
            resets_at: None,
            observed_at: 1_000,
        }
    }

    fn member(task: TaskId, run_id: &str, tokens: f64) -> QuotaGroupMember {
        QuotaGroupMember {
            task_id: task,
            run_id: run_id.to_string(),
            work_unit_id: None,
            source: "claude-oauth".to_string(),
            weighted_tokens: tokens,
            list_price_usd: None,
        }
    }

    fn input(shape: EndShape) -> EstimateInput {
        EstimateInput {
            run_id: "r1".to_string(),
            work_unit_id: None,
            source: "claude-oauth".to_string(),
            weighted_tokens: 1_000.0,
            list_price_usd: Some(0.5),
            calibrations: Vec::new(),
            shape,
        }
    }

    fn methods(ev: &Event) -> Vec<QuotaMethod> {
        match ev {
            Event::QuotaEstimated { windows, .. } => windows.iter().map(|w| w.method).collect(),
            other => panic!("not a quota event: {other:?}"),
        }
    }

    fn pooled(adapter: AccountAdapter, outcome: QuotaEndOutcome) -> EndShape {
        EndShape::Pooled {
            adapter,
            account: "a1".to_string(),
            outcome: Box::new(outcome),
            after: Some(obs(0.14)),
        }
    }

    /// 不変条件: 見積りの判断は、終わりの形ごとに決まった窓の数・method・追記先・標本を返す。表で縛る。
    #[test]
    fn quota_estimate_table() {
        let other = TaskId::new();
        struct Case {
            name: &'static str,
            shape: EndShape,
            windows: usize,
            method: QuotaMethod,
            others: usize,
            samples: usize,
        }
        let cases = [
            Case {
                name: "free run (no account pool)",
                shape: EndShape::Free,
                windows: 2,
                method: QuotaMethod::Free,
                others: 0,
                samples: 0,
            },
            Case {
                name: "pending (the group is still open)",
                shape: pooled(AccountAdapter::ClaudeCode, QuotaEndOutcome::Pending),
                windows: 2,
                method: QuotaMethod::Unknown,
                others: 0,
                samples: 0,
            },
            Case {
                name: "exclusive with a valid before: measured",
                shape: pooled(
                    AccountAdapter::ClaudeCode,
                    QuotaEndOutcome::Exclusive {
                        before: Some(obs(0.10)),
                        before_valid: true,
                    },
                ),
                windows: 2,
                method: QuotaMethod::Measured,
                others: 0,
                samples: 2,
            },
            Case {
                name: "exclusive with no before and no calibration: unknown",
                shape: pooled(
                    AccountAdapter::ClaudeCode,
                    QuotaEndOutcome::Exclusive {
                        before: None,
                        before_valid: false,
                    },
                ),
                windows: 2,
                method: QuotaMethod::Unknown,
                others: 0,
                samples: 0,
            },
            Case {
                name: "opencode go adds the one-month window",
                shape: pooled(
                    AccountAdapter::OpencodeGo,
                    QuotaEndOutcome::Exclusive {
                        before: None,
                        before_valid: false,
                    },
                ),
                windows: 3,
                method: QuotaMethod::Unknown,
                others: 0,
                samples: 0,
            },
            Case {
                name: "closed group: own event plus one for the other member",
                shape: pooled(
                    AccountAdapter::ClaudeCode,
                    QuotaEndOutcome::Closed {
                        before: Some(obs(0.10)),
                        before_valid: true,
                        members: vec![
                            member(TaskId::new(), "r1", 1_000.0),
                            member(other, "r2", 3_000.0),
                        ],
                    },
                ),
                windows: 2,
                method: QuotaMethod::Apportioned,
                others: 1,
                samples: 0,
            },
        ];
        for case in cases {
            let proposal = propose_estimate(input(case.shape));
            assert_eq!(methods(&proposal.own).len(), case.windows, "{}", case.name);
            let Event::QuotaEstimated { method, .. } = &proposal.own else {
                panic!("{}: not a quota event", case.name);
            };
            assert_eq!(*method, case.method, "{}", case.name);
            assert_eq!(proposal.others.len(), case.others, "{}", case.name);
            assert_eq!(proposal.samples.len(), case.samples, "{}", case.name);
        }
    }

    /// 不変条件: 閉じたグループの他の member の event は、その member の task と run に宛てる。
    #[test]
    fn quota_estimate_closed_group_addresses_other_members() {
        let other = TaskId::new();
        let proposal = propose_estimate(input(pooled(
            AccountAdapter::ClaudeCode,
            QuotaEndOutcome::Closed {
                before: Some(obs(0.10)),
                before_valid: true,
                members: vec![
                    member(TaskId::new(), "r1", 1_000.0),
                    member(other, "r2", 3_000.0),
                ],
            },
        )));
        let (task, ev) = &proposal.others[0];
        assert_eq!(*task, other);
        let Event::QuotaEstimated { run_id, .. } = ev else {
            panic!("not a quota event");
        };
        assert_eq!(run_id, "r2");
    }

    /// 不変条件: 閉じたグループに自分の run が無ければ、窓の無い unknown を返す（値を作らない）。
    #[test]
    fn quota_estimate_closed_group_without_own_run_is_unknown() {
        let proposal = propose_estimate(input(pooled(
            AccountAdapter::ClaudeCode,
            QuotaEndOutcome::Closed {
                before: None,
                before_valid: false,
                members: vec![member(TaskId::new(), "r9", 1_000.0)],
            },
        )));
        assert!(methods(&proposal.own).is_empty());
        let Event::QuotaEstimated { method, .. } = &proposal.own else {
            panic!("not a quota event");
        };
        assert_eq!(*method, QuotaMethod::Unknown);
        assert_eq!(proposal.others.len(), 1);
    }

    /// 不変条件: 窓は adapter で決まる（1 か月窓は opencode go だけ）。
    #[test]
    fn quota_estimate_windows_per_adapter() {
        assert_eq!(windows_for(AccountAdapter::Codex).len(), 2);
        assert_eq!(windows_for(AccountAdapter::OpencodeGo).len(), 3);
    }
}
