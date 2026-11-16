//! 試験用フック（ADR 2026-11-11 dispatcher-state-ownership D6）。`Dispatcher` の `#[cfg(test)]` の field は
//! `test_hooks: TestHooks` の 1 個だけで、時計・遅延・停止点・CoS 予約の再現と、研究 ablation
//! （phase_effect_ab の `EffectAblation`）はすべてここに置く。新しい試験用の切替を `Dispatcher` や owner の
//! struct に足さず、ここか `EffectAblation` に足す（`structure_guard.rs` が struct の属性を数えて止める）。

use super::phase_effect_ab::EffectAblation;
use crate::dispatcher::Dispatcher;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use task_core::TaskId;
use task_core::llm_route::SourceId;
use time::OffsetDateTime;

#[derive(Default)]
pub(in crate::dispatcher) struct TestHooks {
    /// 注入した壁時計（`now_utc`）。`None` なら実時計。
    pub(in crate::dispatcher) now: Option<Arc<Mutex<OffsetDateTime>>>,
    /// 注入した単調時計（`monotonic_now`・`monotonic_clock`）。`None` なら実時計。
    pub(in crate::dispatcher) policy_clock: Option<Arc<Mutex<Instant>>>,
    /// spawn した worker task が `run_worker` に入る前に待つ時間（worker の終了が遅れる状況の再現）。
    pub(in crate::dispatcher) worker_start_delay: Option<Duration>,
    /// WU の遷移を書いた直後に run の終了処理を打ち切る（その後の event が書かれる前の kill を再現）。
    pub(in crate::dispatcher) halt_after_wu_transition: bool,
    /// この source の全 account を Claude 予約（`CosReservation`）として扱う（CoS の経路でも再現する）。
    pub(in crate::dispatcher) cos_reserved_sources: std::collections::BTreeSet<SourceId>,
    /// 統合の状態確認直前に別の ready 戻しを差し込む一回フック（`start_integration` が `take` する）。
    pub(in crate::dispatcher) before_integration_start: Option<fn(&mut Dispatcher, TaskId)>,
    /// lease の回収を決めた直後（`reclaim_expired_leases` が snapshot の lease で回収の理由を得た後、回収の書き込みの前）
    /// に 1 回だけ呼ぶ一回フック（`take` する）。別の接続（保持者の instance の store）の `renew_lease` の割り込みを再現する。
    pub(in crate::dispatcher) after_reclaim_decision: Option<Box<dyn FnOnce(TaskId) + Send>>,
    /// 研究 ablation（phase_effect_ab の off の variant）。本体はこの 1 つの読み口だけを見る。
    pub(in crate::dispatcher) ablation: EffectAblation,
}
