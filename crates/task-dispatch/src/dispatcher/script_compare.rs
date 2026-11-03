//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2: dispatcher の compare の結線（`lane_rules`・`route_priority`）。
//!
//! - mode は tick ごとに 1 回 `script_domain_modes` を読む（S2-4。その tick で最初に比べようとした時。両領域の行を一度に読む）。
//!   読めなければその tick は両領域とも `rust` と扱う。
//! - 領域の mode が `compare` のときだけ TS を呼ぶ。mode が `rust` なら入力 DTO も作らない。
//!   `lane_rules` は `decide_lane` が Rust の判定の後に、`route_priority` は `dispatch_run` が `select_route_with`
//!   の直後に呼ぶ（付記 S2-11）。dispatcher は TS を待たない。比較は tokio の task で `SCRIPT_CALL_DEADLINE` 付きで走り、
//!   記録（`script_compare_recorded`）だけを書く。lane・候補列・run・既存の event は Rust の結果のまま（S2-6）。
//! - `PolicyScriptSource` が無い・その generation の口を返せない（bundle が違う・isolate が無い・drain 中、付記 S5-2）・
//!   tokio の runtime が無いときは TS を呼ばず `unavailable` を記録する。
//! - generation の確認・`unavailable`・deadline・in-flight の上限・記録は両領域で同じ経路（[`Dispatcher::compare_domain`]）を通る。
//! - 付記 S5: mode `canary`・`ts` の選択・TS を待つ側・fallback・tick の予算は [`script_canary`]。戻しの書き込みに
//!   失敗した領域は process の中で `rust` と扱う（S5-6）。`canary`・`ts` の generation は manifest の
//!   `checkpoint_versions` が保存済みの版を含むときだけ呼ぶ（S5-5）。
//! - 付記 S4-14: 領域 `cos_digest` は digest pass（`cos_chat/digest.rs`）が [`Dispatcher::script_target`] で
//!   `compare`・`ts` の宛先を得て、`compare` は同じ `spawn_compare` を通る。記録は `script_compare_log`（S4-15）。

use std::sync::Arc;

use task_core::model_policy::{LaneCeiling, TaskFeatures};
use task_core::script_compare::{
    ScriptDomain, ScriptDomainModeRow, ScriptGenerationState, ScriptMode, checkpoint_versions_cover,
};
use tokio::task::JoinHandle;

use super::route_select::{RouteInput, RouteTrace};
use super::*;
use crate::script_policy::{
    MAX_IN_FLIGHT_COMPARES, PolicyScript, PolicyScriptSource, ScriptDomainStore, SingleBundleSource,
};

/// 付記 S5: canary・ts の結線。
#[path = "script_canary.rs"]
pub(in crate::dispatcher) mod script_canary;
#[cfg(test)]
pub(in crate::dispatcher) use script_canary::SCRIPT_TICK_WAIT_BUDGET;
pub(in crate::dispatcher) use script_canary::ScriptTickWait;

/// 試験: mode と比率だけを持つ宛先（選択の試験用。store は空の DB）。
#[cfg(test)]
pub(in crate::dispatcher) fn script_canary_target_for_test(
    mode: ScriptMode,
    canary_bps: Option<u32>,
) -> CompareTarget {
    let store: Arc<dyn ScriptDomainStore> = match task_core::SqliteStore::open_in_memory() {
        Ok(s) => Arc::new(s),
        Err(e) => panic!("in-memory store: {e}"),
    };
    CompareTarget {
        store,
        mode,
        row_generation: Some("G".into()),
        version: 1,
        canary_bps,
        generation_id: "G".into(),
        bundle_sha256: String::new(),
        host_api: String::new(),
        call: Err("test".into()),
    }
}

/// dispatcher が持つ compare の状態。既定は何も注入しない（mode を読まず、TS を呼ばない）。
#[derive(Default)]
pub(in crate::dispatcher) struct ScriptCompareState {
    store: Option<Arc<dyn ScriptDomainStore>>,
    script: Option<Arc<dyn PolicyScriptSource>>,
    /// この tick の mode の行（読めない・store が無いときは空 = 両領域とも `rust`）。
    modes: Vec<ScriptDomainModeRow>,
    /// mode を読んだ tick（`Dispatcher::ticks`）。
    read_at_tick: Option<u64>,
    in_flight: Vec<JoinHandle<()>>,
    /// 付記 S5-8: この tick の TS の待ちと直近の集計。
    tick_wait: ScriptTickWait,
    /// 付記 S5-6: 戻しの書き込みに失敗した領域（process の中で `rust`）。
    pending_rollbacks: Vec<script_canary::PendingRollback>,
    /// この tick の `lane_rules` の TS の lane（task ごと）。
    lane_canary: script_canary::LaneCanary,
}

impl Dispatcher {
    /// mode の表と記録の store を渡す（daemon は `[script]` の有無によらず渡す。渡さなければ compare は働かない）。
    pub fn set_script_domain_store(&mut self, store: Arc<dyn ScriptDomainStore>) {
        self.script_compare.store = Some(store);
        self.script_compare.read_at_tick = None;
    }

    /// TS 制御層の判定の口を渡す（`None` で外す）。無いまま mode が `compare` なら `unavailable` を記録する。
    /// bundle 1 つの口は [`SingleBundleSource`] で包む（generation の `bundle_sha256` と同じときだけ呼ぶ）。
    pub fn set_policy_script(&mut self, script: Option<Arc<dyn PolicyScript>>) {
        self.script_compare.script =
            script.map(|s| Arc::new(SingleBundleSource(s)) as Arc<dyn PolicyScriptSource>);
    }

    /// generation ごとの口を渡す（付記 S5-2。daemon の generation の登録簿）。mode の CAS の後に読んだ行の
    /// generation へ送り、切替の前に送った要求は元の generation で終わる。
    pub fn set_policy_script_source(&mut self, source: Option<Arc<dyn PolicyScriptSource>>) {
        self.script_compare.script = source;
    }

    /// 走っている compare がすべて終わるまで待つ（試験と停止の前の片付け用）。
    pub async fn wait_script_compares(&mut self) {
        for handle in std::mem::take(&mut self.script_compare.in_flight) {
            if let Err(e) = handle.await {
                tracing::warn!(error = %e, "script compare task failed");
            }
        }
    }

    /// 走っている compare を打ち切って待つ（試験で「dispatcher が落ちた」を作る）。
    #[cfg(test)]
    pub(in crate::dispatcher) async fn abort_script_compares(&mut self) {
        for handle in std::mem::take(&mut self.script_compare.in_flight) {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// 付記 S2-4: mode を tick ごとに 1 回だけ読む。
    fn refresh_script_modes(&mut self) {
        if self.script_compare.read_at_tick == Some(self.ticks) {
            return;
        }
        if self
            .script_compare
            .read_at_tick
            .is_none_or(|t| t != self.ticks)
        {
            self.script_compare.lane_canary.clear();
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
    /// 丸めた後の lane。mode `compare` と canary の Rust の側は記録だけ（`None`）。canary の TS の側と `ts` は
    /// Rust の検証を通った TS の lane を返す（付記 S5-3・S5-6）。
    pub(super) fn compare_lane_rules(
        &mut self,
        task_id: TaskId,
        features: &TaskFeatures,
        rust_lane: Tier,
        ceiling: &LaneCeiling,
    ) -> Option<Tier> {
        self.canary_lane_rules(task_id, features, rust_lane, ceiling)
    }

    /// 付記 S2-11・S5-3: `dispatch_run` が task の run の `select_route_with` の直後に 1 回だけ呼ぶ。
    /// 比べる harness は `trace.harness`（選んだ harness か、全部が断ったときは最後に試した harness）で、
    /// 無ければ記録しない。CoS の判定（`input.cos`）は比べない。canary の TS の側と `ts` で、検証を通った TS の
    /// 候補列が Rust と違えば、その列を返す（呼び出し側が選び直す）。
    pub(in crate::dispatcher) fn compare_route_priority(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        input: &RouteInput,
        trace: &RouteTrace,
    ) -> Option<(
        task_core::llm_route::HarnessId,
        Vec<task_core::llm_route::CandidateKey>,
    )> {
        self.canary_route_priority(task_id, run_id, input, trace)
    }

    /// 付記 S4-15: `script_compare_log` の store（無ければ記録しない）。
    pub(in crate::dispatcher) fn script_compare_log_store(
        &self,
    ) -> Option<Arc<dyn ScriptDomainStore>> {
        self.script_compare.store.clone()
    }

    /// mode が `modes` のどれかで generation の id があれば宛先を返す（付記 S4-14: `cos_digest` は `ts` も）。
    pub(in crate::dispatcher) fn script_target(
        &mut self,
        domain: ScriptDomain,
        modes: &[ScriptMode],
    ) -> Option<CompareTarget> {
        self.refresh_script_modes();
        let row = self
            .script_compare
            .modes
            .iter()
            .find(|r| r.domain == domain)?;
        if !modes.contains(&row.mode) {
            return None;
        }
        // 付記 S5-6: 戻しを書けなかった領域は `rust`。
        if self
            .script_compare
            .pending_rollbacks
            .iter()
            .any(|p| p.domain == domain)
        {
            return None;
        }
        let (mode, row_generation, version, canary_bps) = (
            row.mode,
            row.generation_id.clone(),
            row.version,
            row.canary_bps,
        );
        let store = self.script_compare.store.clone()?;
        let generation_id = row.generation_id.clone()?;
        let generation = match store.script_generation_get(&generation_id) {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, generation = %generation_id, "script generation unreadable; compare skipped");
                return None;
            }
        };
        let (bundle_sha256, host_api) = generation
            .as_ref()
            .map(|g| (g.bundle_sha256.clone(), g.host_api.clone()))
            .unwrap_or_default();
        let pinned = match (&generation, &self.script_compare.script) {
            (None, _) => Err(format!("script generation `{generation_id}` not found")),
            (Some(g), _) if g.state != ScriptGenerationState::Active => {
                Err(format!("script generation `{generation_id}` is not active"))
            }
            (Some(g), _) if !g.declares_domain(domain) => Err(format!(
                "script generation `{generation_id}` does not declare {}",
                domain.as_str()
            )),
            // 付記 S5-5: TS の結果を使う mode は、保存済みの checkpoint の版を manifest が含む generation だけ。
            (Some(g), _)
                if matches!(mode, ScriptMode::Canary | ScriptMode::Ts)
                    && !checkpoint_versions_cover(
                        &g.checkpoint_versions(),
                        &store.script_checkpoint_versions_stored(domain),
                    ) =>
            {
                Err(format!(
                    "checkpoint_incompatible: script generation `{generation_id}` does not cover the stored checkpoint versions of {}",
                    domain.as_str()
                ))
            }
            (Some(_), None) => Err("no policy script runtime".to_owned()),
            (Some(g), Some(source)) => source.pinned(g),
        };
        let call = pinned.and_then(|script| match tokio::runtime::Handle::try_current() {
            Ok(runtime) => Ok((script, runtime)),
            Err(_) => Err("no tokio runtime for the script call".to_owned()),
        });
        Some(CompareTarget {
            store,
            mode,
            row_generation,
            version,
            canary_bps,
            generation_id,
            bundle_sha256,
            host_api,
            call,
        })
    }

    /// in-flight の上限（全領域の合計 `MAX_IN_FLIGHT_COMPARES`）の内で比較の task を走らせる。超えたら捨てる。
    pub(in crate::dispatcher) fn spawn_compare(
        &mut self,
        domain: ScriptDomain,
        runtime: &tokio::runtime::Handle,
        compare: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        self.script_compare.in_flight.retain(|h| !h.is_finished());
        if self.script_compare.in_flight.len() >= MAX_IN_FLIGHT_COMPARES {
            tracing::warn!(
                domain = domain.as_str(),
                "too many script compares in flight; compare skipped"
            );
            return;
        }
        let handle = runtime.spawn(compare);
        self.script_compare.in_flight.push(handle);
    }
}

/// [`Dispatcher::script_target`] の結果。
pub(in crate::dispatcher) struct CompareTarget {
    pub store: Arc<dyn ScriptDomainStore>,
    /// pass・tick の始めの mode の行（mode・generation・version。付記 S4-14 の `mode_changed`）。
    pub mode: ScriptMode,
    pub row_generation: Option<String>,
    pub version: u64,
    /// mode `canary` の比率（S5-3）。
    pub canary_bps: Option<u32>,
    pub generation_id: String,
    pub bundle_sha256: String,
    pub host_api: String,
    pub call: Result<(Arc<dyn PolicyScript>, tokio::runtime::Handle), String>,
}
