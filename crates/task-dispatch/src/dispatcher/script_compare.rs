//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2: dispatcher の compare の結線（`lane_rules`・`route_priority`）。
//!
//! - mode は tick ごとに 1 回 `script_domain_modes` を読む（S2-4。その tick で最初に比べようとした時。両領域の行を一度に読む）。
//!   読めなければその tick は両領域とも `rust` と扱う。
//! - 領域の mode が `compare` のときだけ TS を呼ぶ。mode が `rust` なら入力 DTO も作らない。
//!   `lane_rules` は `decide_lane` が Rust の判定の後に、`route_priority` は `dispatch_run` が `select_route_with`
//!   の直後に呼ぶ（付記 S2-11）。dispatcher は TS を待たない。比較は tokio の task で `SCRIPT_CALL_DEADLINE` 付きで走り、
//!   記録（`script_compare_recorded`）だけを書く。lane・候補列・run・既存の event は Rust の結果のまま（S2-6）。
//! - `PolicyScript` が無い・bundle が generation と違う・tokio の runtime が無いときは TS を呼ばず `unavailable` を記録する。
//! - generation の確認・`unavailable`・deadline・in-flight の上限・記録は両領域で同じ経路（[`Dispatcher::compare_domain`]）を通る。

use std::sync::Arc;

use task_core::model_policy::{LaneCeiling, TaskFeatures};
use task_core::script_compare::{
    ScriptDomain, ScriptDomainModeRow, ScriptGenerationState, ScriptMode,
};
use tokio::task::JoinHandle;

use super::route_select::{RouteInput, RouteTrace};
use super::*;
use crate::script_policy::{
    CompareDraft, MAX_IN_FLIGHT_COMPARES, PolicyScript, SCRIPT_CALL_DEADLINE, ScriptDomainStore,
};

/// dispatcher が持つ compare の状態。既定は何も注入しない（mode を読まず、TS を呼ばない）。
#[derive(Default)]
pub(in crate::dispatcher) struct ScriptCompareState {
    store: Option<Arc<dyn ScriptDomainStore>>,
    script: Option<Arc<dyn PolicyScript>>,
    /// この tick の mode の行（読めない・store が無いときは空 = 両領域とも `rust`）。
    modes: Vec<ScriptDomainModeRow>,
    /// mode を読んだ tick（`Dispatcher::ticks`）。
    read_at_tick: Option<u64>,
    in_flight: Vec<JoinHandle<()>>,
}

impl Dispatcher {
    /// mode の表と記録の store を渡す（daemon は `[script]` の有無によらず渡す。渡さなければ compare は働かない）。
    pub fn set_script_domain_store(&mut self, store: Arc<dyn ScriptDomainStore>) {
        self.script_compare.store = Some(store);
        self.script_compare.read_at_tick = None;
    }

    /// TS 制御層の判定の口を渡す（`None` で外す）。無いまま mode が `compare` なら `unavailable` を記録する。
    pub fn set_policy_script(&mut self, script: Option<Arc<dyn PolicyScript>>) {
        self.script_compare.script = script;
    }

    /// 走っている compare がすべて終わるまで待つ（試験と停止の前の片付け用）。
    pub async fn wait_script_compares(&mut self) {
        for handle in std::mem::take(&mut self.script_compare.in_flight) {
            if let Err(e) = handle.await {
                tracing::warn!(error = %e, "script compare task failed");
            }
        }
    }

    /// 付記 S2-4: mode を tick ごとに 1 回だけ読む。
    fn refresh_script_modes(&mut self) {
        if self.script_compare.read_at_tick == Some(self.ticks) {
            return;
        }
        self.script_compare.read_at_tick = Some(self.ticks);
        let Some(store) = &self.script_compare.store else {
            self.script_compare.modes.clear();
            return;
        };
        self.script_compare.modes = match store.script_domain_modes() {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "script domain modes unreadable; treating as rust");
                Vec::new()
            }
        };
    }

    /// `decide_lane` の Rust の判定の後に呼ぶ。`features` は `infer_with_hints` の後の 9 軸、`rust_lane` は天井で
    /// 丸めた後の lane。mode が `compare` でなければ何もしない。Rust の判定は変えない（戻り値も無い）。
    pub(super) fn compare_lane_rules(
        &mut self,
        task_id: TaskId,
        features: &TaskFeatures,
        rust_lane: Tier,
        ceiling: &LaneCeiling,
    ) {
        self.compare_domain(task_id, ScriptDomain::LaneRules, |_, g, sha, api| {
            CompareDraft::lane_rules(g, sha, api, features, rust_lane, ceiling)
        });
    }

    /// 付記 S2-11: `dispatch_run` が task の run の `select_route_with` の直後に 1 回だけ呼ぶ。
    /// 比べる harness は `trace.harness`（選んだ harness か、全部が断ったときは最後に試した harness）で、
    /// 無ければ記録しない。CoS の判定（`input.cos`）は比べない。入力と Rust の出力は `route_select_one` の
    /// 最初の `route_priority` と同じ引数で作る。候補列・選んだ route・event は変えない。
    pub(in crate::dispatcher) fn compare_route_priority(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        input: &RouteInput,
        trace: &RouteTrace,
    ) {
        if input.cos {
            return;
        }
        let Some(harness) = trace.harness.clone() else {
            return;
        };
        self.compare_domain(task_id, ScriptDomain::RoutePriority, |d, g, sha, api| {
            CompareDraft::route_priority(
                g,
                sha,
                api,
                Some(run_id.to_owned()),
                &d.llm_route_registry,
                &harness,
                input.lane.as_deref(),
                false,
                input.harness.execution_class.as_ref(),
            )
        });
    }

    /// 両領域の共通の経路: mode の確認 → generation の確認 → 下書き → `unavailable` か TS の呼び出し。
    /// `draft` は mode が `compare` で generation の id があるときだけ呼ぶ（mode `rust` では DTO を作らない）。
    fn compare_domain(
        &mut self,
        task_id: TaskId,
        domain: ScriptDomain,
        draft: impl FnOnce(&Self, String, String, String) -> CompareDraft,
    ) {
        self.refresh_script_modes();
        let Some(row) = self
            .script_compare
            .modes
            .iter()
            .find(|r| r.domain == domain)
        else {
            return;
        };
        if row.mode != ScriptMode::Compare {
            return;
        }
        let Some(store) = self.script_compare.store.clone() else {
            return;
        };
        let Some(generation_id) = row.generation_id.clone() else {
            return;
        };
        let generation = match store.script_generation_get(&generation_id) {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, generation = %generation_id, "script generation unreadable; compare skipped");
                return;
            }
        };
        let (bundle_sha256, host_api) = generation
            .as_ref()
            .map(|g| (g.bundle_sha256.clone(), g.host_api.clone()))
            .unwrap_or_default();
        let draft = draft(self, generation_id.clone(), bundle_sha256, host_api);
        let unavailable = match (&generation, &self.script_compare.script) {
            (None, _) => Some(format!("script generation `{generation_id}` not found")),
            (Some(g), _) if g.state != ScriptGenerationState::Loaded => {
                Some(format!("script generation `{generation_id}` is not loaded"))
            }
            (Some(g), _) if !g.declares_domain(domain) => Some(format!(
                "script generation `{generation_id}` does not declare {}",
                domain.as_str()
            )),
            (Some(_), None) => Some("no policy script runtime".to_owned()),
            (Some(g), Some(script)) if script.bundle().bundle_sha256 != g.bundle_sha256 => {
                Some(format!(
                    "loaded bundle {} differs from generation bundle {}",
                    script.bundle().bundle_sha256,
                    g.bundle_sha256
                ))
            }
            _ => None,
        };
        let runtime = tokio::runtime::Handle::try_current();
        let unavailable = unavailable.or_else(|| {
            runtime
                .is_err()
                .then(|| "no tokio runtime for the script call".to_owned())
        });
        if let Some(reason) = unavailable {
            append_compare(store.as_ref(), task_id, draft.unavailable(&reason));
            return;
        }
        let (Some(script), Ok(runtime)) = (self.script_compare.script.clone(), runtime) else {
            return;
        };
        self.script_compare.in_flight.retain(|h| !h.is_finished());
        if self.script_compare.in_flight.len() >= MAX_IN_FLIGHT_COMPARES {
            tracing::warn!(%task_id, domain = domain.as_str(), "too many script compares in flight; compare skipped");
            return;
        }
        let handle = runtime.spawn(async move {
            let (result, elapsed) = draft.call(script.as_ref(), SCRIPT_CALL_DEADLINE).await;
            append_compare(store.as_ref(), task_id, draft.finish(&result, elapsed));
        });
        self.script_compare.in_flight.push(handle);
    }
}

/// 記録を追記する。失敗しても dispatch は続ける（warn の log だけ。S2-6）。
fn append_compare(
    store: &dyn ScriptDomainStore,
    task_id: TaskId,
    record: Result<task_core::script_compare::ScriptCompareRecord, serde_json::Error>,
) {
    let result = record
        .map_err(StoreError::from)
        .and_then(|r| store.script_compare_append(task_id, &r, time::OffsetDateTime::now_utc()));
    if let Err(e) = result {
        tracing::warn!(%task_id, error = %e, "script_compare_recorded append failed");
    }
}
