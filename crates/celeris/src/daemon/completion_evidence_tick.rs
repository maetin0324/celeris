//! ADR 2026-10-22-harness-balance-and-lane-first-routing D2/D7: daemon の定期処理。
//!
//! - 切替の guardrail（短い間隔）: 有効な実行クラスについて、切替した完了単位が切替しなかった単位より
//!   明らかに悪化していれば、そのクラスを止める（`harness_balance_trips`。人の reset だけが戻す）。
//! - 根拠表の更新（長い間隔）: 完了単位の本番結果から lane ごとの根拠表の新しい版を書く。
//!
//! 判定は決定的で LLM を呼ばない。時計は引数で注入する。

use std::time::Duration;

use task_core::completion_outcome::{evaluate_harness_balance, refresh_routing_evidence};
use task_core::{NoticeEvent, NoticeKind, NoticeRecordOutcome, TaskStore};
use task_dispatch::Dispatcher;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// guardrail の判定間隔。
pub(crate) const GUARD_INTERVAL: Duration = Duration::from_secs(900);
/// 根拠表の更新間隔（ADR D7: 6 時間）。
pub(crate) const EVIDENCE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub(crate) enum TickError {
    #[error("store: {0}")]
    Store(#[from] task_core::StoreError),
    #[error("config: {0}")]
    Config(String),
    #[error("time: {0}")]
    Time(#[from] time::error::Format),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GuardReport {
    pub tripped: Vec<String>,
    pub notices: usize,
}

/// 切替の guardrail を 1 回判定する。止めたクラスは受信箱に知らせる（同じクラスは 1 回）。
pub(crate) fn guard_tick(
    store: &dyn TaskStore,
    registry: &task_core::llm_route::Registry,
    now: OffsetDateTime,
) -> Result<GuardReport, TickError> {
    let settings = &registry.route_selection;
    if settings.harness_balance_classes.is_empty() {
        return Ok(GuardReport::default());
    }
    let stamp = now.format(&Rfc3339)?;
    let tripped = evaluate_harness_balance(
        store,
        &settings.harness_balance_classes,
        settings.guardrail_window,
        settings.harness_balance_guard_min,
        settings.harness_balance_guard_margin,
        &stamp,
    )?;
    let mut report = GuardReport::default();
    for (class, trip) in tripped {
        let outcome = store.notice_record(&NoticeEvent {
            source_key: format!("harness_balance:{class}:{}", now.unix_timestamp()),
            kind: NoticeKind::BadNews,
            group_key: format!("harness_balance:{class}"),
            title: format!("routing: {class} の harness 残量切替を止めた"),
            summary: format!(
                "切替した {} 件の悪い率 {:.2} が、しなかった {} 件の {:.2} より悪化した。宣言順に戻した。\
                 再開は `celerisctl routing balance-reset {class}`。",
                trip.switched_n, trip.switched_bad_rate, trip.other_n, trip.other_bad_rate
            ),
            project_id: None,
            task_id: None,
            target: None,
            links: Vec::new(),
            at: now,
        })?;
        report.notices += usize::from(!matches!(outcome, NoticeRecordOutcome::Duplicate(_)));
        report.tripped.push(class);
    }
    Ok(report)
}

/// 根拠表の更新を 1 回行う。書いた行数。
pub(crate) fn evidence_tick(
    store: &dyn TaskStore,
    now: OffsetDateTime,
) -> Result<usize, TickError> {
    Ok(refresh_routing_evidence(store, &now.format(&Rfc3339)?)?)
}

pub(crate) fn guard(
    dispatcher: &Dispatcher,
    config: &crate::Config,
    now: OffsetDateTime,
) -> Result<GuardReport, TickError> {
    let registry = config
        .llm_route_registry()
        .map_err(|e| TickError::Config(e.to_string()))?;
    guard_tick(dispatcher.store().as_ref(), &registry, now)
}
