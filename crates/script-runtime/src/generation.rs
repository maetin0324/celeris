//! generation の登録簿 [`GenerationRegistry`]（境界 ADR 2026-11-02 の付記 S5-1・S5-2・S5-5）。
//!
//! - generation 1 つ = bundle 1 つ = 新しい isolate（新しい actor thread、[`ScriptRuntimeHandle`]）1 つ。
//!   登録（[`GenerationRegistry::register`]）は既存の isolate を使わず、新しい actor で bundle を評価し、
//!   manifest の検証・selftest・各領域の入口の確認に通ったものだけを登録簿に入れる。落ちた isolate は捨てる。
//! - 要求（[`GenerationRegistry::call`]）は呼んだ時点（future を作った時点）でその generation の `Arc` と
//!   in-flight の枠を取る。後で別の generation へ切り替わっても、その要求は取った generation で最後まで走る。
//!   Promise・closure を generation の間で移さない（isolate を跨ぐ値は JSON の DTO だけ）。
//! - 登録簿に無い・drain 中の generation への新しい要求は即 [`ScriptError::GenerationGone`]。呼び出し側は
//!   Rust の参照実装で答える。
//! - drain（[`GenerationRegistry::drain`]）の後、in-flight が 0 になった generation は
//!   [`GenerationRegistry::sweep`] で退役する。drain から [`DRAIN_CUTOFF`]（30 秒）を過ぎても残る要求は打ち切り、
//!   その要求には `GenerationGone` を返す（actor は後から来た結果を捨てる。同期の実行は要求の deadline で
//!   watchdog が止める）。
//! - 生きた generation（drain 中を含む）は [`MAX_LIVE_GENERATIONS`]（2）まで。3 つ目の登録は拒む。
//! - 時間は注入した時計 [`GenerationClock`] で数える。試験は [`ManualClock`] を進めて決定的に確かめる。
//!
//! DB の状態（`loaded`・`active`・`draining`・`retired`・`invalid`）と event は daemon と store が書く。
//! この登録簿は isolate と in-flight だけを持つ。

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::dto::Capability;
use crate::error::ScriptError;
use crate::handle::{
    HostSink, QUEUE_CAPACITY, RuntimeLimits, ScriptCall, ScriptReply, ScriptRuntimeHandle,
};
use crate::manifest::{ManifestError, ScriptManifest, VerifiedBundle, load_deployed};
use crate::ops::domain_capabilities;
use crate::selftest::{SELFTEST_DOMAIN, check_output, selftest_call};

/// 生きた generation（drain 中を含む）の上限（付記 S5-1。(f) の RSS の上限が generation 2 個で 200 MiB）。
pub const MAX_LIVE_GENERATIONS: usize = 2;
/// generation ごとの in-flight の上限（付記 S5-2。今の bounded mpsc の容量と同じ 64）。超えたら `Busy`。
pub const MAX_IN_FLIGHT_PER_GENERATION: usize = QUEUE_CAPACITY;
/// drain の打ち切り（付記 S5-2・(d)）。drain を始めてからこの時間を過ぎて残る要求は `GenerationGone`。
pub const DRAIN_CUTOFF: Duration = Duration::from_secs(30);
/// host が引き継げる checkpoint の版（付記 S5-5。S5 では `1` だけ）。
pub const HOST_CHECKPOINT_VERSIONS: &[u32] = &[1];
/// 登録で知っている領域（付記 S5-1 の 3「`domains` が既知の領域だけ」）。
pub const KNOWN_DOMAINS: &[&str] = &[
    "cos_digest",
    "execution_gate",
    "host_selftest",
    "lane_rules",
    "project_controller",
    "route_priority",
];

/// 登録の検証で selftest と入口の確認に使う要求の上限（実時間。CPU を使う待ちではない）。
const REGISTER_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// 登録簿が使う時計。drain の打ち切りはこの時計で数える。
pub trait GenerationClock: Send + Sync {
    fn now(&self) -> Instant;
}

/// `tokio::time::Instant::now()` を返す時計（`tokio::time::pause` に従う）。daemon の既定。
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

impl GenerationClock for TokioClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// 試験用の手で進める時計。[`ManualClock::advance`] でだけ進む。
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<Instant>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            now: Mutex::new(Instant::now()),
        }
    }

    pub fn advance(&self, by: Duration) {
        let mut now = lock(&self.now);
        *now += by;
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl GenerationClock for ManualClock {
    fn now(&self) -> Instant {
        *lock(&self.now)
    }
}

/// 登録・退役の誤り。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenerationError {
    /// 配備先の読み出し・manifest の検証（名前と hash の照合、`host_api`、`schema_sha256`）に落ちた。
    #[error("bundle verification failed: {0}")]
    Manifest(#[from] ManifestError),
    #[error("manifest declares an unknown domain {domain:?}")]
    UnknownDomain { domain: String },
    #[error("capability {capability} is not allowed for any declared domain")]
    CapabilityNotAllowed { capability: String },
    #[error("invalid checkpoint_versions: {reason}")]
    CheckpointVersions { reason: String },
    /// 保存済みの checkpoint の版を新しい manifest が引き継げない（付記 S5-5）。
    #[error("checkpoint versions {missing:?} are stored but not accepted by the manifest")]
    CheckpointIncompatible { missing: Vec<u32> },
    #[error("bundle could not start: {0}")]
    Start(ScriptError),
    #[error("selftest failed: {reason}")]
    Selftest { reason: String },
    /// manifest が宣言した領域を bundle の入口が知らない。
    #[error("bundle has no entry for declared domain {domain:?}")]
    EntryMissing { domain: String },
    /// 生きた generation が上限に達している（付記 S5-1。API は 409 `script_generation_limit`）。
    #[error("live generation limit {MAX_LIVE_GENERATIONS} reached")]
    Limit,
    #[error("generation {id} is already registered")]
    Duplicate { id: String },
    #[error("generation {id} is not registered")]
    NotFound { id: String },
    /// in-flight が残るので今は退役できない（drain を使う）。
    #[error("generation {id} still has {in_flight} request(s) in flight")]
    InFlight { id: String, in_flight: usize },
}

impl GenerationError {
    /// bundle そのものが不正（DB の `invalid` に当たる。API は 422）。上限・重複・未登録・in-flight は false。
    pub fn is_invalid_bundle(&self) -> bool {
        !matches!(
            self,
            Self::Limit | Self::Duplicate { .. } | Self::NotFound { .. } | Self::InFlight { .. }
        )
    }
}

/// manifest の検証のうち isolate を使わない部分（付記 S5-1 の 3）。
///
/// 既知の領域だけ・capability ⊆ 宣言した領域の許可表の和・`checkpoint_versions` の形・selftest の領域の宣言。
pub fn validate_manifest(manifest: &ScriptManifest) -> Result<(), GenerationError> {
    let mut seen = BTreeSet::new();
    for domain in &manifest.domains {
        if !KNOWN_DOMAINS.contains(&domain.as_str()) || !seen.insert(domain.as_str()) {
            return Err(GenerationError::UnknownDomain {
                domain: domain.clone(),
            });
        }
    }
    let allowed: BTreeSet<Capability> = manifest
        .domains
        .iter()
        .flat_map(|d| domain_capabilities(d).iter().copied())
        .collect();
    if let Some(cap) = manifest.capabilities.iter().find(|c| !allowed.contains(c)) {
        return Err(GenerationError::CapabilityNotAllowed {
            capability: cap.as_str().to_owned(),
        });
    }
    check_checkpoint_versions(&manifest.checkpoint_versions)?;
    if !seen.contains(SELFTEST_DOMAIN) {
        return Err(GenerationError::Selftest {
            reason: format!("manifest does not declare domain {SELFTEST_DOMAIN}"),
        });
    }
    Ok(())
}

/// `checkpoint_versions` が重複なしの昇順で、host が知る版（[`HOST_CHECKPOINT_VERSIONS`]）に限ること（付記 S5-5）。
pub fn check_checkpoint_versions(versions: &[u32]) -> Result<(), GenerationError> {
    if versions.windows(2).any(|w| w[0] >= w[1]) {
        return Err(GenerationError::CheckpointVersions {
            reason: format!("{versions:?} is not strictly ascending"),
        });
    }
    if let Some(v) = versions
        .iter()
        .find(|v| !HOST_CHECKPOINT_VERSIONS.contains(v))
    {
        return Err(GenerationError::CheckpointVersions {
            reason: format!("version {v} is not known to this host {HOST_CHECKPOINT_VERSIONS:?}"),
        });
    }
    Ok(())
}

/// 領域に保存済みの checkpoint の版の集合 `stored` を、新しい manifest の `checkpoint_versions` が全部含むか
/// （付記 S5-5）。含まなければ mode の CAS も起動時の復元も通さない。
pub fn checkpoint_versions_cover(
    stored: &BTreeSet<u32>,
    manifest_versions: &[u32],
) -> Result<(), GenerationError> {
    let missing: Vec<u32> = stored
        .iter()
        .copied()
        .filter(|v| !manifest_versions.contains(v))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(GenerationError::CheckpointIncompatible { missing })
    }
}

/// 登録簿の中の generation の今の様子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenerationInfo {
    pub generation_id: String,
    pub bundle_sha256: String,
    pub domains: Vec<String>,
    /// drain 中（新しい要求を受けない）か。
    pub draining: bool,
    pub in_flight: usize,
}

/// 要求を固定した generation とその結果。
#[derive(Debug, Clone, PartialEq)]
pub struct PinnedReply {
    /// 要求を受けた時点で固定した generation。
    pub generation_id: String,
    pub reply: ScriptReply,
}

/// 退役の理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetireReason {
    /// drain の後、in-flight が 0 になった。
    Drained,
    /// drain から [`DRAIN_CUTOFF`] を過ぎ、残った要求を打ち切った。
    Cutoff,
    /// in-flight の無い generation を直接退役した（`loaded` の退役）。
    Retired,
    /// 安全違反などで無効にした。残った要求はすぐ打ち切った。
    Invalidated,
}

/// 退役した generation の記録（daemon が `script_generation_retired` に写す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetiredGeneration {
    pub generation_id: String,
    pub bundle_sha256: String,
    pub reason: RetireReason,
    /// 打ち切った in-flight の数（`cancelled_in_flight`）。
    pub cancelled_in_flight: usize,
}

/// 1 つの generation。isolate（actor）と in-flight の数と打ち切りの合図を持つ。
struct Generation {
    id: String,
    manifest: ScriptManifest,
    handle: ScriptRuntimeHandle,
    in_flight: AtomicUsize,
    /// true にすると in-flight の要求は `GenerationGone` で返る。
    cancel: watch::Sender<bool>,
}

struct Slot {
    generation: Arc<Generation>,
    /// drain を始めた時刻（[`GenerationClock`] の値）。
    draining_since: Option<Instant>,
}

#[derive(Default)]
struct State {
    slots: BTreeMap<String, Slot>,
    /// 登録の検証中で、まだ slots に入っていない数（上限の数に入れる）。
    loading: usize,
}

struct RegistryInner {
    clock: Arc<dyn GenerationClock>,
    limits: RuntimeLimits,
    state: Mutex<State>,
}

/// generation の登録簿。`Clone + Send + Sync`。
#[derive(Clone)]
pub struct GenerationRegistry {
    inner: Arc<RegistryInner>,
}

/// in-flight の枠。落ちると数を 1 減らす。
struct InFlightGuard(Arc<Generation>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 登録の検証中の枠。落ちると `loading` を 1 減らす。
struct Reservation<'a>(&'a RegistryInner);

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut st = lock(&self.0.state);
        st.loading = st.loading.saturating_sub(1);
    }
}

impl GenerationRegistry {
    /// `tokio::time` の時計で作る（daemon の既定）。
    pub fn new(limits: RuntimeLimits) -> Self {
        Self::with_clock(limits, Arc::new(TokioClock))
    }

    pub fn with_clock(limits: RuntimeLimits, clock: Arc<dyn GenerationClock>) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                clock,
                limits,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// `<state_dir>/script-bundles/<bundle_sha256>/` を読み（配備先の名前と hash の照合を含む）、[`Self::register`] する。
    pub async fn register_deployed(
        &self,
        generation_id: &str,
        state_dir: &std::path::Path,
        bundle_sha256: &str,
    ) -> Result<GenerationInfo, GenerationError> {
        let state_dir = state_dir.to_owned();
        let sha = bundle_sha256.to_owned();
        let bundle = tokio::task::spawn_blocking(move || load_deployed(&state_dir, &sha))
            .await
            .map_err(|_| GenerationError::Start(ScriptError::Terminated))??;
        self.register(generation_id, bundle).await
    }

    /// 新しい isolate で bundle を load・検証し、通ったものだけを登録する（付記 S5-1 の 3）。
    ///
    /// 検証: [`validate_manifest`]、module の評価、selftest、manifest の各領域の入口。落ちたら isolate を捨てる。
    pub async fn register(
        &self,
        generation_id: &str,
        bundle: VerifiedBundle,
    ) -> Result<GenerationInfo, GenerationError> {
        validate_manifest(&bundle.manifest)?;
        let _reservation = self.reserve(generation_id)?;
        let source = bundle.source();
        let limits = self.inner.limits;
        let handle = tokio::task::spawn_blocking(move || {
            ScriptRuntimeHandle::start_with_limits(source, limits)
        })
        .await
        .map_err(|_| GenerationError::Start(ScriptError::Terminated))?
        .map_err(GenerationError::Start)?;
        if let Err(err) = verify_loaded(&handle, &bundle.manifest).await {
            shutdown_all(vec![handle]).await;
            return Err(err);
        }
        let (cancel, _) = watch::channel(false);
        let generation = Arc::new(Generation {
            id: generation_id.to_owned(),
            manifest: bundle.manifest,
            handle,
            in_flight: AtomicUsize::new(0),
            cancel,
        });
        let info = info_of(&generation, false);
        let mut st = lock(&self.inner.state);
        st.slots.insert(
            generation_id.to_owned(),
            Slot {
                generation,
                draining_since: None,
            },
        );
        // `_reservation` は lock を取るので、ここで先に st を離してから落とす。
        drop(st);
        Ok(info)
    }

    fn reserve(&self, generation_id: &str) -> Result<Reservation<'_>, GenerationError> {
        let mut st = lock(&self.inner.state);
        if st.slots.contains_key(generation_id) {
            return Err(GenerationError::Duplicate {
                id: generation_id.to_owned(),
            });
        }
        if st.slots.len() + st.loading >= MAX_LIVE_GENERATIONS {
            return Err(GenerationError::Limit);
        }
        st.loading += 1;
        Ok(Reservation(&self.inner))
    }

    /// generation `generation_id` の入口を呼ぶ。
    ///
    /// この関数を呼んだ時点で generation と in-flight の枠を取る（future を後で poll しても固定は変わらない）。
    /// 未登録・drain 中なら `GenerationGone`、in-flight が [`MAX_IN_FLIGHT_PER_GENERATION`] なら `Busy`。
    pub fn call(
        &self,
        generation_id: &str,
        call: ScriptCall,
        host_sink: Option<HostSink>,
    ) -> impl Future<Output = Result<PinnedReply, ScriptError>> + Send + 'static {
        let acquired = self.acquire(generation_id);
        async move {
            let guard = acquired?;
            let generation = Arc::clone(&guard.0);
            let result = call_until_cancelled(
                generation.cancel.subscribe(),
                generation.handle.call_with_host_sink(call, host_sink),
            )
            .await;
            drop(guard);
            result.map(|reply| PinnedReply {
                generation_id: generation.id.clone(),
                reply,
            })
        }
    }

    fn acquire(&self, generation_id: &str) -> Result<InFlightGuard, ScriptError> {
        let st = lock(&self.inner.state);
        let slot = st
            .slots
            .get(generation_id)
            .filter(|s| s.draining_since.is_none())
            .ok_or(ScriptError::GenerationGone)?;
        let generation = &slot.generation;
        // 増やすのも lock の中なので、drain・sweep と食い違わない。
        if generation.in_flight.load(Ordering::SeqCst) >= MAX_IN_FLIGHT_PER_GENERATION {
            return Err(ScriptError::Busy);
        }
        generation.in_flight.fetch_add(1, Ordering::SeqCst);
        Ok(InFlightGuard(Arc::clone(generation)))
    }

    /// drain を始める。以後の新しい要求は `GenerationGone`。既に drain 中なら何もしない（始めた時刻は変えない）。
    pub fn drain(&self, generation_id: &str) -> Result<(), GenerationError> {
        let now = self.inner.clock.now();
        let mut st = lock(&self.inner.state);
        let slot = st
            .slots
            .get_mut(generation_id)
            .ok_or_else(|| GenerationError::NotFound {
                id: generation_id.to_owned(),
            })?;
        slot.draining_since.get_or_insert(now);
        Ok(())
    }

    /// drain 中の generation のうち、in-flight が 0 のものと [`DRAIN_CUTOFF`] を過ぎたものを退役させる。
    ///
    /// daemon は tick ごとに呼ぶ。isolate の後始末（actor thread の join）は blocking の thread で行う。
    pub async fn sweep(&self) -> Vec<RetiredGeneration> {
        let now = self.inner.clock.now();
        let mut retired = Vec::new();
        let mut handles = Vec::new();
        {
            let mut st = lock(&self.inner.state);
            let ids: Vec<String> = st
                .slots
                .iter()
                .filter_map(|(id, s)| s.draining_since.map(|_| id.clone()))
                .collect();
            for id in ids {
                let Some(slot) = st.slots.get(&id) else {
                    continue;
                };
                let Some(since) = slot.draining_since else {
                    continue;
                };
                let in_flight = slot.generation.in_flight.load(Ordering::SeqCst);
                let reason = if in_flight == 0 {
                    RetireReason::Drained
                } else if now.saturating_duration_since(since) >= DRAIN_CUTOFF {
                    RetireReason::Cutoff
                } else {
                    continue;
                };
                if let Some(slot) = st.slots.remove(&id) {
                    retired.push(take_out(&slot.generation, reason, in_flight));
                    handles.push(slot.generation.handle.clone());
                }
            }
        }
        shutdown_all(handles).await;
        retired
    }

    /// in-flight の無い generation をすぐ退役させる（DB の `loaded` の退役）。in-flight があれば
    /// [`GenerationError::InFlight`]（[`Self::drain`] を使う）。
    pub async fn retire(&self, generation_id: &str) -> Result<RetiredGeneration, GenerationError> {
        let slot = {
            let mut st = lock(&self.inner.state);
            let slot = st
                .slots
                .get(generation_id)
                .ok_or_else(|| GenerationError::NotFound {
                    id: generation_id.to_owned(),
                })?;
            let in_flight = slot.generation.in_flight.load(Ordering::SeqCst);
            if in_flight > 0 {
                return Err(GenerationError::InFlight {
                    id: generation_id.to_owned(),
                    in_flight,
                });
            }
            st.slots.remove(generation_id)
        };
        let Some(slot) = slot else {
            return Err(GenerationError::NotFound {
                id: generation_id.to_owned(),
            });
        };
        let retired = take_out(&slot.generation, RetireReason::Retired, 0);
        shutdown_all(vec![slot.generation.handle.clone()]).await;
        Ok(retired)
    }

    /// generation をすぐ捨てる（安全違反など。付記 S5-7）。残った要求は `GenerationGone` で返る。
    pub async fn invalidate(
        &self,
        generation_id: &str,
    ) -> Result<RetiredGeneration, GenerationError> {
        let slot = lock(&self.inner.state).slots.remove(generation_id);
        let Some(slot) = slot else {
            return Err(GenerationError::NotFound {
                id: generation_id.to_owned(),
            });
        };
        let in_flight = slot.generation.in_flight.load(Ordering::SeqCst);
        let retired = take_out(&slot.generation, RetireReason::Invalidated, in_flight);
        shutdown_all(vec![slot.generation.handle.clone()]).await;
        Ok(retired)
    }

    /// 登録簿の generation の一覧（id の順）。
    pub fn list(&self) -> Vec<GenerationInfo> {
        let st = lock(&self.inner.state);
        st.slots
            .values()
            .map(|s| info_of(&s.generation, s.draining_since.is_some()))
            .collect()
    }

    /// 1 つの generation の様子。
    pub fn get(&self, generation_id: &str) -> Option<GenerationInfo> {
        let st = lock(&self.inner.state);
        st.slots
            .get(generation_id)
            .map(|s| info_of(&s.generation, s.draining_since.is_some()))
    }

    /// 登録した generation の manifest（領域・capability・`checkpoint_versions` の照合に使う）。
    pub fn manifest(&self, generation_id: &str) -> Option<ScriptManifest> {
        let st = lock(&self.inner.state);
        st.slots
            .get(generation_id)
            .map(|s| s.generation.manifest.clone())
    }

    /// 生きた generation（drain 中を含む）の数。
    pub fn live_count(&self) -> usize {
        lock(&self.inner.state).slots.len()
    }

    /// 全 generation を捨てる（daemon の停止）。残った要求は `GenerationGone`。
    pub async fn shutdown(&self) -> Vec<RetiredGeneration> {
        let slots: Vec<Slot> = {
            let mut st = lock(&self.inner.state);
            std::mem::take(&mut st.slots).into_values().collect()
        };
        let mut retired = Vec::new();
        let mut handles = Vec::new();
        for slot in slots {
            let in_flight = slot.generation.in_flight.load(Ordering::SeqCst);
            retired.push(take_out(
                &slot.generation,
                RetireReason::Invalidated,
                in_flight,
            ));
            handles.push(slot.generation.handle.clone());
        }
        shutdown_all(handles).await;
        retired
    }
}

/// `call` を待ち、generation が打ち切られたら `GenerationGone` を返す。
///
/// 打ち切りの合図の確認と `call` の poll の間に打ち切りと actor の停止が終わると、`call` は `Terminated` を
/// 返す（queue が閉じている・返事の口が落ちた）。[`take_out`] は actor を止める前に合図を立てるので、ここで
/// 合図を読み直せば打ち切りと actor の自滅を区別できる。actor が自分で止まった場合は `Terminated` のまま。
async fn call_until_cancelled<T>(
    mut cancelled: watch::Receiver<bool>,
    call: impl Future<Output = Result<T, ScriptError>>,
) -> Result<T, ScriptError> {
    let result = tokio::select! {
        biased;
        _ = cancelled.wait_for(|c| *c) => Err(ScriptError::GenerationGone),
        r = call => r,
    };
    match result {
        Err(ScriptError::Terminated) if *cancelled.borrow() => Err(ScriptError::GenerationGone),
        other => other,
    }
}

/// 登録簿から外す generation の in-flight を打ち切り、記録を作る。
fn take_out(generation: &Generation, reason: RetireReason, in_flight: usize) -> RetiredGeneration {
    if in_flight > 0 {
        generation.cancel.send_replace(true);
    }
    RetiredGeneration {
        generation_id: generation.id.clone(),
        bundle_sha256: generation.manifest.bundle_sha256.clone(),
        reason,
        cancelled_in_flight: in_flight,
    }
}

fn info_of(generation: &Generation, draining: bool) -> GenerationInfo {
    GenerationInfo {
        generation_id: generation.id.clone(),
        bundle_sha256: generation.manifest.bundle_sha256.clone(),
        domains: generation.manifest.domains.clone(),
        draining,
        in_flight: generation.in_flight.load(Ordering::SeqCst),
    }
}

/// 新しい isolate で selftest と各領域の入口を確かめる。
///
/// 入口の確認は入力 `null`・scope 空・host sink 無しで領域を 1 回呼び、bundle の入口が
/// `unknown domain: <領域>` で拒んだら入口が無いとみなす。host sink が無いので op は副作用を持てない。
async fn verify_loaded(
    handle: &ScriptRuntimeHandle,
    manifest: &ScriptManifest,
) -> Result<(), GenerationError> {
    let reply = handle
        .call(selftest_call(REGISTER_CALL_TIMEOUT))
        .await
        .map_err(|e| GenerationError::Selftest {
            reason: e.to_string(),
        })?;
    check_output(&reply.output).map_err(|reason| GenerationError::Selftest { reason })?;
    for domain in manifest.domains.iter().filter(|d| *d != SELFTEST_DOMAIN) {
        let probe = ScriptCall {
            domain: domain.clone(),
            input: serde_json::Value::Null,
            scope_task_ids: BTreeSet::new(),
            timeout: REGISTER_CALL_TIMEOUT,
        };
        match handle.call(probe).await {
            Err(ScriptError::Exception { message })
                if message.contains(&format!("unknown domain: {domain}")) =>
            {
                return Err(GenerationError::EntryMissing {
                    domain: domain.clone(),
                });
            }
            // 入口が入力 null を拒む・op が host sink 無しで失敗するのは、入口があるということ。
            Ok(_) | Err(ScriptError::Exception { .. }) | Err(ScriptError::InvalidOutput { .. }) => {
            }
            Err(e) => {
                return Err(GenerationError::Selftest {
                    reason: format!("entry probe for {domain} failed: {e}"),
                });
            }
        }
    }
    Ok(())
}

/// actor thread の join は blocking なので blocking の thread で行う。
async fn shutdown_all(handles: Vec<ScriptRuntimeHandle>) {
    if handles.is_empty() {
        return;
    }
    // join の thread が panic しても、actor は最後の handle の Drop で止まる。
    let _ = tokio::task::spawn_blocking(move || {
        for h in handles {
            h.shutdown();
        }
    })
    .await;
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_domains_cover_capability_table() {
        for cap in [
            Capability::SelftestEchoTask,
            Capability::CosDigestListRuns,
            Capability::CosDigestReadRun,
        ] {
            assert!(KNOWN_DOMAINS.contains(&cap.domain()));
        }
    }

    /// 合図の確認が済んだ後、`call` の poll の前に打ち切りが終わる窓を再現する。`call` の中で合図を立てて
    /// `Terminated` を返すと、実時間に頼らず同じ順序になる。
    #[tokio::test]
    async fn cancel_between_signal_check_and_call_poll_is_generation_gone() {
        let (cancel, cancelled) = watch::channel(false);
        let call = async move {
            cancel.send_replace(true);
            Err::<(), _>(ScriptError::Terminated)
        };
        assert_eq!(
            call_until_cancelled(cancelled, call).await,
            Err(ScriptError::GenerationGone)
        );
    }

    /// 打ち切られていないのに actor が止まった場合は `Terminated` のまま（`GenerationGone` と混ぜない）。
    #[tokio::test]
    async fn terminated_without_cancel_stays_terminated() {
        let (_cancel, cancelled) = watch::channel(false);
        let call = async { Err::<(), _>(ScriptError::Terminated) };
        assert_eq!(
            call_until_cancelled(cancelled, call).await,
            Err(ScriptError::Terminated)
        );
    }

    /// 打ち切りと同時に届いた結果は、`Terminated` 以外ならそのまま返す。
    #[tokio::test]
    async fn other_outcomes_pass_through_when_cancelled_during_call() {
        let (cancel, cancelled) = watch::channel(false);
        let call = async move {
            cancel.send_replace(true);
            Ok::<_, ScriptError>(1)
        };
        assert_eq!(call_until_cancelled(cancelled, call).await, Ok(1));

        let (cancel, cancelled) = watch::channel(false);
        let call = async move {
            cancel.send_replace(true);
            Err::<(), _>(ScriptError::Timeout)
        };
        assert_eq!(
            call_until_cancelled(cancelled, call).await,
            Err(ScriptError::Timeout)
        );
    }

    #[test]
    fn invalid_bundle_classification() {
        assert!(!GenerationError::Limit.is_invalid_bundle());
        assert!(GenerationError::EntryMissing { domain: "x".into() }.is_invalid_bundle());
    }
}
