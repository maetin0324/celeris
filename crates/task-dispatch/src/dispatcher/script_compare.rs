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
//! - 付記 S4: 領域 `cos_digest` は digest pass が Rust の command を apply した後に [`Dispatcher::compare_cos_digest`] で
//!   同じ経路（`compare_target`・`spawn_compare`）を通る。記録は task の events でなく `script_compare_log`（S4-5）。

use std::sync::Arc;

use task_core::model_policy::{LaneCeiling, TaskFeatures};
use task_core::script_compare::{
    ScriptDomain, ScriptDomainModeRow, ScriptGenerationState, ScriptMode,
};
use tokio::task::JoinHandle;

use super::route_select::{RouteInput, RouteTrace};
use super::*;
use crate::script_policy::{
    CompareDraft, CosDigestDraft, MAX_IN_FLIGHT_COMPARES, PolicyScript, SCRIPT_CALL_DEADLINE,
    ScriptDomainStore,
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

    /// 全領域の共通の経路: mode の確認 → generation の確認 → 下書き → `unavailable` か TS の呼び出し。
    /// `draft` は mode が `compare` で generation の id があるときだけ呼ぶ（mode `rust` では DTO を作らない）。
    fn compare_domain(
        &mut self,
        task_id: TaskId,
        domain: ScriptDomain,
        draft: impl FnOnce(&Self, String, String, String) -> CompareDraft,
    ) {
        let Some(target) = self.compare_target(domain) else {
            return;
        };
        let draft = draft(
            self,
            target.generation_id,
            target.bundle_sha256,
            target.host_api,
        );
        let store = target.store;
        let (script, runtime) = match target.call {
            Err(reason) => {
                append_compare(store.as_ref(), task_id, draft.unavailable(&reason));
                return;
            }
            Ok(call) => call,
        };
        self.spawn_compare(domain, &runtime, async move {
            let (result, elapsed) = draft.call(script.as_ref(), SCRIPT_CALL_DEADLINE).await;
            append_compare(store.as_ref(), task_id, draft.finish(&result, elapsed));
        });
    }

    /// 付記 S4-4: digest pass が Rust の command を apply した後に呼ぶ。mode が `compare` の領域 `cos_digest` だけ、
    /// 同じ `CosDigestInput` を別の tokio task で TS に渡し、Rust の command と比べて `script_compare_log` に追記する。
    /// mode `rust`・store 無しでは何もしない（TS を呼ばない）。message・requeue は変えない（戻り値も無い）。
    pub(in crate::dispatcher) fn compare_cos_digest(&mut self, digests: Vec<CosDigestApplication>) {
        if digests.is_empty() {
            return;
        }
        for digest in digests {
            let Some(target) = self.compare_target(ScriptDomain::CosDigest) else {
                return;
            };
            let draft = CosDigestDraft {
                generation: target.generation_id,
                bundle_sha256: target.bundle_sha256,
                host_api: target.host_api,
                input: digest.input,
                rust: digest.commands,
                applied: digest.applied,
            };
            let store = target.store;
            let (script, runtime) = match target.call {
                Err(reason) => {
                    append_compare_log(store.as_ref(), draft.unavailable(&reason));
                    continue;
                }
                Ok(call) => call,
            };
            self.spawn_compare(ScriptDomain::CosDigest, &runtime, async move {
                let (result, elapsed) = draft.call(script.as_ref(), SCRIPT_CALL_DEADLINE).await;
                append_compare_log(store.as_ref(), draft.finish(&result, elapsed));
            });
        }
    }

    /// mode が `compare` で generation の id があれば比較の宛先を返す（無ければ `None` = 何もしない）。
    /// `call` が `Err` なら TS を呼ばず、その理由で `unavailable` を記録する。
    fn compare_target(&mut self, domain: ScriptDomain) -> Option<CompareTarget> {
        self.refresh_script_modes();
        let row = self
            .script_compare
            .modes
            .iter()
            .find(|r| r.domain == domain)?;
        if row.mode != ScriptMode::Compare {
            return None;
        }
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
        let call = match (unavailable, &self.script_compare.script) {
            (Some(reason), _) => Err(reason),
            (None, Some(script)) => match tokio::runtime::Handle::try_current() {
                Ok(runtime) => Ok((script.clone(), runtime)),
                Err(_) => Err("no tokio runtime for the script call".to_owned()),
            },
            (None, None) => Err("no policy script runtime".to_owned()),
        };
        Some(CompareTarget {
            store,
            generation_id,
            bundle_sha256,
            host_api,
            call,
        })
    }

    /// in-flight の上限（全領域の合計 `MAX_IN_FLIGHT_COMPARES`）の内で比較の task を走らせる。超えたら捨てる。
    fn spawn_compare(
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

/// [`Dispatcher::compare_target`] の結果。
struct CompareTarget {
    store: Arc<dyn ScriptDomainStore>,
    generation_id: String,
    bundle_sha256: String,
    host_api: String,
    call: Result<(Arc<dyn PolicyScript>, tokio::runtime::Handle), String>,
}

/// digest pass が apply した 1 run（付記 S4-4 の compare の入力）。
pub(crate) struct CosDigestApplication {
    pub input: task_core::chat::digest::CosDigestInput,
    pub commands: Vec<task_core::chat::digest::CosDigestCommand>,
    pub applied: task_core::script_compare::CosDigestApplied,
}

/// `cos_digest` の記録を追記する。失敗しても digest pass は続ける（warn の log だけ。S4-4）。
fn append_compare_log(
    store: &dyn ScriptDomainStore,
    record: Result<task_core::script_compare::CosDigestCompareRecord, serde_json::Error>,
) {
    let result = record
        .map_err(StoreError::from)
        .and_then(|r| store.script_compare_log_append(&r, time::OffsetDateTime::now_utc()));
    if let Err(e) = result {
        tracing::warn!(error = %e, "script_compare_log append failed");
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
