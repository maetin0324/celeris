//! ADR 2026-10-04 multi-objective model routing §7.1・§10 Phase 4（daemon-wire）: `[model_routing.shadow]`
//! を task-dispatch の decision shadow と llm-proxy の shadow へ配線する。
//!
//! - 既定（`mode = legacy`・`execute = false`）では llm-proxy に shadow を差し込まない（候補列も作らず、
//!   shadow queue・予約 store も作らない。upstream への追加送信 0）。
//! - `mode = shadow` なら proxy の decision shadow（追加呼出しなし）を差し込む。`execute = true` で
//!   上限が揃っていれば実行 shadow の queue を作り、日次上限は共有 DB の予約
//!   （[`llm_proxy::shadow_budget::StoreShadowBudget`]、migration 0049）で数える。handoff 先の新 instance
//!   も同じ DB の予約を読むので、新旧の合計で上限を越えない。
//! - reload: dispatcher が `set_routing_shadow` で新しい policy を受け取り、[`ProxyShadowControl`]
//!   （[`RoutingShadowListener`]）へ渡す。queue は policy・上限・予約先を 1 回で差し替える
//!   （[`ShadowQueue::reconfigure`]）。decision shadow の記録は mode に合わせて開け閉めする。
//!   起動時に proxy へ差し込まなかった shadow（既定 off で起動）を reload で新しく足すことはしない
//!   （`ProxyState` は起動時に 1 度だけ組む。`[model_routing.retry]` と同じく再起動で効く）。その場合は
//!   warn を出し、dispatcher 側の decision shadow だけが効く。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use llm_proxy::reservation::Clock;
use llm_proxy::shadow::{
    PreferSelfHosted, ProxyShadow, ShadowBudget, ShadowCostFn, ShadowEvent, ShadowExecutor,
    ShadowQueue, ShadowSink,
};
use llm_proxy::shadow_budget::StoreShadowBudget;
use task_core::model_router::policy::RoutingMode;
use task_core::model_router::shadow::{ShadowKind, ShadowPolicy};
use task_core::store::{SqliteStore, StoreOptions};
use task_core::{Event, TaskId, TaskStore};
use task_dispatch::{Dispatcher, RoutingShadowListener};

use crate::config::RoutingCatalog;

/// 要求に出力上限が無いときに予約する最悪 output tokens。
pub(crate) const DEFAULT_OUTPUT_RESERVE: u64 = 4096;

/// shadow の配線に要るもの（試験は偽の実行器・時計を渡す）。
pub(crate) struct ShadowDeps {
    pub db_path: PathBuf,
    pub busy_timeout: Duration,
    pub task_store: Arc<dyn TaskStore>,
    pub executor: Arc<dyn ShadowExecutor>,
    pub clock: Arc<dyn Clock>,
    /// 予約の持ち主（監査用）。
    pub owner: String,
    /// 最悪費用の見積もり。`None` なら全て未知（`unknown_cost` で dropped）。
    pub cost: Option<Arc<ShadowCostFn>>,
}

/// 配線の結果。`shadow` は `ProxyState::with_shadow` に渡す（`None` なら proxy は何もしない）。
pub(crate) struct ProxyShadowWiring {
    pub shadow: Option<ProxyShadow>,
    pub control: Arc<ProxyShadowControl>,
}

/// proxy 側の shadow の開閉と差し替え（dispatcher の listener）。
pub(crate) struct ProxyShadowControl {
    /// proxy に shadow を差し込んだか（false なら reload で有効にしても再起動まで proxy は何もしない）。
    installed: bool,
    decision_on: Arc<AtomicBool>,
    queue: Option<Arc<ShadowQueue>>,
    /// 予約 store（実行 shadow を作ったときだけ開く。reload で上限を変えても同じ接続を使う）。
    budget_store: Option<Arc<SqliteStore>>,
    clock: Arc<dyn Clock>,
}

#[cfg(test)]
impl ProxyShadowControl {
    pub(crate) fn queue(&self) -> Option<&Arc<ShadowQueue>> {
        self.queue.as_ref()
    }

    pub(crate) fn decision_on(&self) -> bool {
        self.decision_on.load(Ordering::SeqCst)
    }

    pub(crate) fn installed(&self) -> bool {
        self.installed
    }
}

impl RoutingShadowListener for ProxyShadowControl {
    fn reload(&self, mode: RoutingMode, policy: &ShadowPolicy) {
        self.decision_on
            .store(mode == RoutingMode::Shadow, Ordering::SeqCst);
        match &self.queue {
            Some(queue) => {
                let budget = self.budget_store.as_ref().and_then(|store| {
                    StoreShadowBudget::new(Arc::clone(store), policy, Arc::clone(&self.clock))
                        .map(|b| Arc::new(b) as Arc<dyn ShadowBudget>)
                });
                queue.reconfigure(policy.clone(), budget);
            }
            None if policy.daily_caps().is_some() => tracing::warn!(
                "model_routing.shadow.execute was enabled by reload; \
                 the llm-proxy execution shadow starts after a restart"
            ),
            None => {}
        }
        if !self.installed && mode == RoutingMode::Shadow {
            tracing::warn!(
                "model_routing.mode = shadow was enabled by reload; \
                 the llm-proxy decision shadow starts after a restart (dispatcher shadow is active)"
            );
        }
    }
}

/// 起動時の配線。`mode` と `policy` は検証済みの `RoutingRuntime` のもの（無ければ既定 = off）。
pub(crate) fn wire_proxy_shadow(
    mode: RoutingMode,
    policy: &ShadowPolicy,
    deps: ShadowDeps,
) -> Result<ProxyShadowWiring, String> {
    let decision_on = Arc::new(AtomicBool::new(mode == RoutingMode::Shadow));
    let executes = policy.daily_caps().is_some();
    if mode != RoutingMode::Shadow && !executes {
        return Ok(ProxyShadowWiring {
            shadow: None,
            control: Arc::new(ProxyShadowControl {
                installed: false,
                decision_on,
                queue: None,
                budget_store: None,
                clock: deps.clock,
            }),
        });
    }
    let sink: Arc<dyn ShadowSink> = Arc::new(TaskShadowSink {
        store: Arc::clone(&deps.task_store),
        decision_on: Arc::clone(&decision_on),
        append_lock: Arc::new(std::sync::Mutex::new(())),
    });
    let (queue, budget_store) = if executes {
        let store = SqliteStore::open_with(
            &deps.db_path,
            StoreOptions {
                busy_timeout: deps.busy_timeout,
                read_pool_size: 0,
                background_checkpoint: false,
            },
        )
        .map_err(|e| format!("model_routing.shadow: could not open the reservation store: {e}"))?;
        let store = Arc::new(store);
        let budget = StoreShadowBudget::new(Arc::clone(&store), policy, Arc::clone(&deps.clock))
            .ok_or("model_routing.shadow: execute policy has no daily caps")?;
        let queue = ShadowQueue::new(
            policy.clone(),
            deps.owner,
            deps.executor,
            Arc::new(budget),
            Arc::clone(&sink),
            Arc::clone(&deps.clock),
        )
        .ok_or("model_routing.shadow: execute policy is incomplete")?;
        (Some(queue), Some(store))
    } else {
        (None, None)
    };
    let shadow = ProxyShadow {
        decision: Some(Arc::new(PreferSelfHosted)),
        execution: queue.clone(),
        sink,
        cost: deps.cost,
        default_output_reserve: DEFAULT_OUTPUT_RESERVE,
    };
    Ok(ProxyShadowWiring {
        shadow: Some(shadow),
        control: Arc::new(ProxyShadowControl {
            installed: true,
            decision_on,
            queue,
            budget_store,
            clock: deps.clock,
        }),
    })
}

/// daemon の配線一式: proxy に差し込む shadow を作り、dispatcher に listener を登録する
/// （登録時に現在の mode・policy が 1 度渡る。以後 reload のたびに渡る）。
pub(crate) fn install_proxy_shadow(
    dispatcher: &mut Dispatcher,
    deps: ShadowDeps,
) -> Result<ProxyShadowWiring, String> {
    let mode = dispatcher.dispatch_routing().mode;
    let policy = dispatcher.routing_shadow_policy().clone();
    let wiring = wire_proxy_shadow(mode, &policy, deps)?;
    dispatcher
        .add_routing_shadow_listener(Arc::clone(&wiring.control) as Arc<dyn RoutingShadowListener>);
    Ok(wiring)
}

/// 最悪費用の見積もり: catalog で `source_ref`・`upstream_model` が一致する deployment の単価
/// （`price_override`、無ければ model の pricing）の input/output の大きい方 × 最悪 tokens。
/// 単価が揃わなければ未知（`None`）。catalog は reload で差し替わる共有 snapshot を読む。
pub(crate) fn catalog_cost(
    catalog: Arc<std::sync::RwLock<Arc<RoutingCatalog>>>,
) -> Arc<ShadowCostFn> {
    Arc::new(move |source: &str, model: &str, worst_tokens: u64| {
        let catalog = Arc::clone(&catalog.read().unwrap_or_else(|e| e.into_inner()));
        let deployment = catalog
            .deployments
            .iter()
            .find(|d| d.source_ref == source && d.upstream_model == model)?;
        let pricing = deployment.price_override.as_ref().or_else(|| {
            catalog
                .models
                .iter()
                .find(|m| m.id == deployment.model_profile_id)
                .and_then(|m| m.pricing.as_ref())
        })?;
        let rate = pricing
            .input_usd_per_million?
            .max(pricing.output_usd_per_million?);
        let usd = rate * worst_tokens as f64 / 1_000_000.0;
        (usd.is_finite() && usd >= 0.0).then_some(usd)
    })
}

/// shadow の記録を task event（`routing_shadow_recorded`）として追記する。decision shadow は
/// mode = shadow の間だけ。run の所属を DB で照合できない記録は task に結ばない（proxy log のみ）。
struct TaskShadowSink {
    store: Arc<dyn TaskStore>,
    decision_on: Arc<AtomicBool>,
    append_lock: Arc<std::sync::Mutex<()>>,
}

impl ShadowSink for TaskShadowSink {
    fn record(&self, event: ShadowEvent) {
        if event.record.kind == ShadowKind::Decision && !self.decision_on.load(Ordering::SeqCst) {
            return;
        }
        let store = Arc::clone(&self.store);
        let lock = Arc::clone(&self.append_lock);
        let append = move || {
            if let Err(error) = append_shadow_event(store.as_ref(), &lock, event) {
                tracing::debug!(%error, "shadow record not appended to a task");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(append);
            }
            Err(_) => append(),
        }
    }
}

fn append_shadow_event(
    store: &dyn TaskStore,
    lock: &std::sync::Mutex<()>,
    event: ShadowEvent,
) -> Result<(), String> {
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let task = event.task_id.ok_or("shadow record has no task")?;
    let task_id: TaskId = task.parse().map_err(|e| format!("invalid task id: {e}"))?;
    let run_id = event
        .record
        .run_id
        .as_deref()
        .ok_or("shadow record has no run id")?;
    if !store
        .run_index_get(run_id)
        .map_err(|e| e.to_string())?
        .is_some_and(|r| r.task_id == task)
    {
        return Err("shadow record run does not belong to task".into());
    }
    event.record.validate().map_err(|e| e.to_string())?;
    store
        .append_event(
            task_id,
            &Event::RoutingShadowRecorded {
                record: Box::new(event.record),
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "routing_shadow_tests.rs"]
mod tests;
