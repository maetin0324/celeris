//! TS 制御層の generation（境界 ADR 2026-11-02 (b)(d)、付記 S5-1・S5-2・S4-14）。
//!
//! `[script]` が無ければ isolate を作らない。在れば [`ScriptGenerations`] が `celeris-script` の登録簿
//! （`GenerationRegistry`）を持ち、次を行う。
//! - 登録（`POST /script/generations` → [`ScriptGenerationControl`]）: `<state_dir>/script-bundles/<sha256>/` の bundle を
//!   新しい isolate で検証してから `script_generations` に `loaded` で記録する。検証に落ちたら `invalid` を書く。
//!   配備先の dir が無い hash は `invalid` にせず 409 `script_bundle_not_deployed`（配備の後に登録し直せる）。
//!   store が登録を拒んだら isolate を捨てる（表と isolate を食い違わせない）。
//! - 切替: mode の CAS（store の 1 transaction）が行の `generation_id` と generation の状態を変える。dispatcher は tick
//!   ごとに行を読み、その generation に固定した口（[`PolicyScriptSource`]）で呼ぶ。切替の前に送った要求は元の
//!   generation で最後まで走る。
//! - drain: [`ScriptGenerations::reconcile`]（[`RECONCILE_INTERVAL`] ごと）が `draining` の generation の新しい要求を止め、
//!   in-flight が 0 になるか 30 秒（`DRAIN_CUTOFF`）を過ぎたら isolate を捨てて `retired` を書く
//!   （打ち切った件数を `cancelled_in_flight` に書く。打ち切られた要求は `GenerationGone` で、dispatcher は Rust の参照実装）。
//! - 起動時の復元（[`ScriptGenerations::restore`]）: mode の行が指す generation を新しい isolate で load する。
//!   file が無い・hash が違う・`host_api` が合わない・検証に落ちる・manifest にその領域が無い・その領域の capability が
//!   足りない・checkpoint の版が足りない・generation の状態が `loaded`/`active` でない・script-guard が止まっている、の
//!   どれかなら、その領域を actor `system:script-startup` で `rust` に戻す（受信箱の項目は `script_guard` が event から
//!   作る）。続けて、行から指されない `draining` を drain の終わり（in-flight 0）として、isolate の無い `loaded` を
//!   退役として `retired` にする。
//! - 登録簿の key は generation の `bundle_sha256`（`script_generations` で一意）。検証に通るまで store の id は無いので、
//!   検証してから記録する順序のためにこうする。
//!
//! standby の daemon は isolate を作らず、復元も登録も行わない（503）。active になった後の最初の
//! [`RECONCILE_INTERVAL`] で復元する。
//!
//! 網羅（ADR 2026-11-04-lane-rules-exhaustive-evidence D2・D3）: [`ScriptGenerationControl::exhaustive_start`] が
//! `running` の行を書き、`super::script_exhaustive` の runner を tick の外の tokio task で起こす。runner は `ops` の
//! lock を持たない。復元は他の instance の `running` の行を `aborted`（`daemon_restarted`）にし、停止は runner を
//! 止めて `aborted`（`daemon_stopped`）を書いてから isolate を捨てる。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use celeris_script::dto::HOST_API_VERSION;
use celeris_script::generation::{MAX_LIVE_GENERATIONS, checkpoint_versions_cover};
use celeris_script::manifest::{ScriptManifest, deployed_dir, load_deployed};
use celeris_script::{GenerationError, GenerationRegistry, RetireReason, domain_capabilities};
use task_api::script_generations::{
    ScriptGenerationControl, ScriptGenerationError, SharedScriptGenerationControl,
};
use task_core::script_compare::{
    NewScriptGeneration, ScriptDomain, ScriptDomainError, ScriptDomainModeRow, ScriptGeneration,
    ScriptGenerationState, ScriptGenerationTransition, ScriptMode, ScriptModeChange,
};
use task_core::script_exhaustive::{
    NewScriptExhaustiveRun, ScriptExhaustiveRun, ScriptExhaustiveState, ScriptExhaustiveStopCode,
};
use task_core::store::{SCRIPT_RUNTIME_ACTOR, current_reference_sha256};
use task_core::{InstanceRole, SharedRole, SqliteStore, StoreOptions};
use task_dispatch::script_policy::{PolicyScript, PolicyScriptSource};
use time::OffsetDateTime;

use super::script_exhaustive::{self, ExhaustiveTuning, TargetProbe, TargetState};
use super::script_policy::ScriptPolicy;
use crate::config::Config;

/// 起動時の戻しの actor（付記 S2-5・S5-2）。
pub const STARTUP_ACTOR: &str = "system:script-startup";

/// drain の見回りの間隔。drain の打ち切り（30 秒）はこの間隔ぶん遅れうる。
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 起動時に 1 つの領域を `rust` に戻す理由（付記 S5-2）。先頭は `reason_code` が読む code。
/// `manifest` はその generation を新しい isolate で load できたときの manifest、`load_error` は load の誤り。
pub fn restore_reason(
    row: &ScriptDomainModeRow,
    generation: Option<&ScriptGeneration>,
    loaded: Option<&Result<ScriptManifest, String>>,
    halted: Option<&str>,
    stored_checkpoints: &BTreeSet<u32>,
) -> Option<String> {
    if !row.mode.needs_generation() {
        return None;
    }
    let domain = row.domain.as_str();
    if let Some(reason) = halted {
        return Some(format!(
            "startup_guard_halted: the script guard is halted: {reason}"
        ));
    }
    let Some(generation) = generation else {
        return Some(format!(
            "startup_generation_missing: generation {} is not in script_generations",
            row.generation_id.as_deref().unwrap_or("(none)")
        ));
    };
    if !matches!(
        generation.state,
        ScriptGenerationState::Loaded | ScriptGenerationState::Active
    ) {
        return Some(format!(
            "startup_generation_{}: generation {} is {}",
            generation.state.as_str(),
            generation.id,
            generation.state.as_str()
        ));
    }
    let manifest = match loaded {
        None => {
            return Some(format!(
                "startup_bundle_unusable: bundle {} has no script runtime ([script] is not configured)",
                generation.bundle_sha256
            ));
        }
        Some(Err(e)) => {
            return Some(format!(
                "startup_bundle_unusable: bundle {}: {e}",
                generation.bundle_sha256
            ));
        }
        Some(Ok(m)) => m,
    };
    if !manifest.domains.iter().any(|d| d == domain) {
        return Some(format!(
            "startup_domain_not_declared: bundle {} does not declare {domain}",
            generation.bundle_sha256
        ));
    }
    let missing: Vec<&str> = domain_capabilities(domain)
        .iter()
        .filter(|c| !manifest.capabilities.contains(c))
        .map(|c| c.as_str())
        .collect();
    if !missing.is_empty() {
        return Some(format!(
            "startup_capability_missing: bundle {} lacks {} for {domain}",
            generation.bundle_sha256,
            missing.join(", ")
        ));
    }
    if let Err(e) = checkpoint_versions_cover(stored_checkpoints, &manifest.checkpoint_versions) {
        return Some(format!(
            "startup_checkpoint_incompatible: bundle {}: {e}",
            generation.bundle_sha256
        ));
    }
    None
}

/// 保存済みの checkpoint の版（付記 S5-5。S5 の 3 領域には長期 workflow が無く、表も無いので常に空）。
fn stored_checkpoint_versions() -> BTreeSet<u32> {
    BTreeSet::new()
}

/// 実行中の generation の登録簿と DB の整合を持つ（付記 S5-1・S5-2）。`Clone` せず `Arc` で共有する。
pub struct ScriptGenerations {
    registry: GenerationRegistry,
    state_dir: PathBuf,
    db: Arc<SqliteStore>,
    runtime: tokio::runtime::Handle,
    /// 起動時の復元が終わったか。終わるまで登録・退役は 503、dispatcher の口は `unavailable`。
    ready: AtomicBool,
    /// 登録・退役・見回り・復元を 1 つずつ行う（登録簿と表を食い違わせない）。
    ops: tokio::sync::Mutex<()>,
    /// generation（`bundle_sha256`）ごとの口。失敗の記録の `isolate_epoch` を保つため使い回す。
    policies: Mutex<BTreeMap<String, Arc<ScriptPolicy>>>,
    /// この daemon の起動ごとの id（網羅の行の `daemon_instance_id`。`GET /health` の `instance_id`）。
    instance_id: String,
    /// 網羅の runner を止める合図（停止で立てる）。
    exhaustive_stop: tokio::sync::watch::Sender<bool>,
    /// 走っている網羅の runner。
    exhaustive_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// 網羅の時間の設定（本番は ADR の値。試験だけが変える）。
    exhaustive_tuning: Mutex<ExhaustiveTuning>,
}

impl ScriptGenerations {
    /// 呼び出し元の tokio runtime で作る。`registry` の時計は試験で差し替えられる。
    /// instance id は新しい ULID（試験の入口）。daemon は [`Self::for_instance`] で起動の id を渡す。
    pub fn new(
        registry: GenerationRegistry,
        state_dir: PathBuf,
        db: Arc<SqliteStore>,
    ) -> Arc<Self> {
        Self::for_instance(registry, state_dir, db, ulid::Ulid::new().to_string())
    }

    /// `instance_id` は daemon の起動ごとの id（網羅の行の `daemon_instance_id`）。
    pub fn for_instance(
        registry: GenerationRegistry,
        state_dir: PathBuf,
        db: Arc<SqliteStore>,
        instance_id: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            state_dir,
            db,
            runtime: tokio::runtime::Handle::current(),
            ready: AtomicBool::new(false),
            ops: tokio::sync::Mutex::new(()),
            policies: Mutex::new(BTreeMap::new()),
            instance_id,
            exhaustive_stop: tokio::sync::watch::channel(false).0,
            exhaustive_tasks: Mutex::new(Vec::new()),
            exhaustive_tuning: Mutex::new(ExhaustiveTuning::default()),
        })
    }

    /// この daemon の起動ごとの id。
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// 網羅の時間の設定を変える（試験だけ。次に始める実行から効く）。
    pub fn set_exhaustive_tuning(&self, tuning: ExhaustiveTuning) {
        *lock(&self.exhaustive_tuning) = tuning;
    }

    /// 走っている網羅の runner が終わるのを待つ（試験の出来事待ち）。
    pub async fn wait_exhaustive(&self) {
        let tasks = std::mem::take(&mut *lock(&self.exhaustive_tasks));
        for t in tasks {
            if let Err(e) = t.await {
                tracing::error!(error = %e, "script exhaustive: runner task failed");
            }
        }
    }

    /// 登録簿（試験が in-flight と generation を見る）。
    pub fn registry(&self) -> &GenerationRegistry {
        &self.registry
    }

    /// 起動時の復元が終わったか。
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// dispatcher に渡す口。
    pub fn source(self: &Arc<Self>) -> Arc<dyn PolicyScriptSource> {
        Arc::clone(self) as Arc<dyn PolicyScriptSource>
    }

    /// API に渡す口。
    pub fn control(self: &Arc<Self>) -> SharedScriptGenerationControl {
        Arc::clone(self) as SharedScriptGenerationControl
    }

    /// 起動時の復元（付記 S5-2）。mode の行が指す generation を新しい isolate で load し、使えない領域を `rust` に戻し、
    /// 指されない `draining`・isolate の無い `loaded` を `retired` にする。DB が読めなければ何もせず `Err`
    /// （次の見回りでやり直す）。
    pub async fn restore(&self) -> Result<(), String> {
        let _ops = self.ops.lock().await;
        let db = Arc::clone(&self.db);
        let instance = self.instance_id.clone();
        let (rows, halted) = blocking(move || {
            // D3: 他の instance の `running` の網羅は途中から再開しない（`aborted`、`daemon_restarted`）。
            let aborted = db
                .script_exhaustive_abort_stale(&instance, OffsetDateTime::now_utc())
                .map_err(|e| e.to_string())?;
            if aborted > 0 {
                tracing::warn!(
                    aborted,
                    "script exhaustive: runs left running by another daemon marked aborted"
                );
            }
            let rows = db
                .script_domain_active_generations()
                .map_err(|e| e.to_string())?;
            let halted = db.script_guard_halted().map_err(|e| e.to_string())?;
            Ok((rows, halted))
        })
        .await?;
        let mut loaded: BTreeMap<String, Result<ScriptManifest, String>> = BTreeMap::new();
        if halted.is_none() {
            for (row, generation) in &rows {
                let Some(g) = generation.as_ref().filter(|_| row.mode.needs_generation()) else {
                    continue;
                };
                if loaded.contains_key(&g.bundle_sha256)
                    || !matches!(
                        g.state,
                        ScriptGenerationState::Loaded | ScriptGenerationState::Active
                    )
                {
                    continue;
                }
                let result = self.load(&g.bundle_sha256).await;
                loaded.insert(g.bundle_sha256.clone(), result);
            }
        }
        let stored = stored_checkpoint_versions();
        let mut rollbacks = Vec::new();
        for (row, generation) in &rows {
            let manifest = generation
                .as_ref()
                .and_then(|g| loaded.get(&g.bundle_sha256));
            if let Some(reason) = restore_reason(
                row,
                generation.as_ref(),
                manifest,
                halted.as_deref(),
                &stored,
            ) {
                rollbacks.push((row.clone(), reason));
            }
        }
        let db = Arc::clone(&self.db);
        let keep: BTreeSet<String> = self
            .registry
            .list()
            .into_iter()
            .map(|i| i.generation_id)
            .collect();
        blocking(move || {
            rollback_rows(&db, &rollbacks);
            retire_unpointed(&db, &keep);
            Ok(())
        })
        .await?;
        // 全領域が戻った generation の isolate はここで捨てる（DB は上で `retired` になっている）。
        self.reconcile_locked().await;
        self.ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// bundle を新しい isolate で load する（付記 S5-1 の 3 の検証）。
    async fn load(&self, sha256: &str) -> Result<ScriptManifest, String> {
        if let Some(m) = self.registry.manifest(sha256) {
            return Ok(m);
        }
        self.registry
            .register_deployed(sha256, &self.state_dir, sha256)
            .await
            .map_err(|e| e.to_string())?;
        self.registry
            .manifest(sha256)
            .ok_or_else(|| format!("generation {sha256} vanished after the load"))
    }

    /// 登録簿と表を合わせる（付記 S5-2 の drain）。[`RECONCILE_INTERVAL`] ごとに呼ぶ。
    pub async fn reconcile(&self) {
        let _ops = self.ops.lock().await;
        self.reconcile_locked().await;
    }

    async fn reconcile_locked(&self) {
        let db = Arc::clone(&self.db);
        let rows = match blocking(move || db.script_generations().map_err(|e| e.to_string())).await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "script runtime: generations unreadable; drain check skipped");
                return;
            }
        };
        let by_sha: BTreeMap<&str, &ScriptGeneration> =
            rows.iter().map(|g| (g.bundle_sha256.as_str(), g)).collect();
        for info in self.registry.list() {
            match by_sha.get(info.generation_id.as_str()).map(|g| g.state) {
                Some(ScriptGenerationState::Draining) => {
                    let _ = self.registry.drain(&info.generation_id);
                }
                // 表で isolate を持たない状態（他の経路の退役・無効化、行が無い）なら残った要求も打ち切る。
                Some(ScriptGenerationState::Retired | ScriptGenerationState::Invalid) | None => {
                    let _ = self.registry.invalidate(&info.generation_id).await;
                    self.forget(&info.generation_id);
                }
                Some(ScriptGenerationState::Loaded | ScriptGenerationState::Active) => {}
            }
        }
        let retired = self.registry.sweep().await;
        let mut done = Vec::new();
        for r in &retired {
            self.forget(&r.generation_id);
            if let Some(g) = by_sha.get(r.generation_id.as_str()) {
                let reason = match r.reason {
                    RetireReason::Cutoff => format!(
                        "drain_cutoff: {} request(s) still in flight at the 30 s cutoff",
                        r.cancelled_in_flight
                    ),
                    _ => "drained: no request in flight".to_owned(),
                };
                done.push((g.id.clone(), reason, r.cancelled_in_flight));
            }
        }
        // isolate を持たない `draining`（記録の失敗の後など）も drain の終わりとして退役させる。
        for g in &rows {
            if g.state == ScriptGenerationState::Draining
                && self.registry.get(&g.bundle_sha256).is_none()
                && !done.iter().any(|(id, _, _)| id == &g.id)
            {
                done.push((g.id.clone(), "drained: no isolate holds it".to_owned(), 0));
            }
        }
        if done.is_empty() {
            return;
        }
        let db = Arc::clone(&self.db);
        let _ = blocking(move || {
            for (id, reason, cancelled) in done {
                retire_row(&db, &id, SCRIPT_RUNTIME_ACTOR, &reason, cancelled);
            }
            Ok(())
        })
        .await;
    }

    fn forget(&self, sha256: &str) {
        lock(&self.policies).remove(sha256);
    }

    /// 全 isolate を捨てる（daemon の停止）。表は変えない（次の起動の復元が合わせる）。
    pub async fn shutdown(&self) {
        // 網羅の runner を先に止め、`aborted`（`daemon_stopped`）を書き終えてから isolate を捨てる。
        let _ = self.exhaustive_stop.send(true);
        self.wait_exhaustive().await;
        let _ops = self.ops.lock().await;
        self.ready.store(false, Ordering::SeqCst);
        self.registry.shutdown().await;
        lock(&self.policies).clear();
    }

    fn ensure_ready(&self) -> Result<(), ScriptGenerationError> {
        if self.is_ready() {
            Ok(())
        } else {
            Err(ScriptGenerationError::Unavailable(
                "this daemon has not restored its generations (standby or starting)".into(),
            ))
        }
    }

    fn register_locked(&self, sha256: &str) -> Result<ScriptGeneration, ScriptGenerationError> {
        if let Some(existing) = self
            .db
            .script_generation_by_sha256(sha256)
            .map_err(store_err)?
        {
            if existing.state.is_live() {
                return Ok(existing);
            }
            if existing.state == ScriptGenerationState::Invalid {
                return Err(ScriptDomainError::GenerationInvalidated(existing.id).into());
            }
        }
        let loaded = self.runtime.block_on(self.registry.register_deployed(
            sha256,
            &self.state_dir,
            sha256,
        ));
        match loaded {
            Ok(_) => {}
            Err(GenerationError::Limit) => {
                return Err(ScriptDomainError::GenerationLimit {
                    live: MAX_LIVE_GENERATIONS,
                    max: MAX_LIVE_GENERATIONS,
                }
                .into());
            }
            // 付記 S5-1 の 6 の例外（P1-C）: 配備先の dir が無いのは「まだ配備していない」で、bundle の不正ではない。
            // `invalid` を書かずに 409 `script_bundle_not_deployed` を返し、`script-deploy` の後に同じ hash で登録し直せる。
            Err(e) if e.is_invalid_bundle() && !deployed_dir(&self.state_dir, sha256).is_dir() => {
                return Err(ScriptGenerationError::NotDeployed {
                    bundle_sha256: sha256.to_owned(),
                    reason: e.to_string(),
                });
            }
            Err(e) if e.is_invalid_bundle() => {
                let reason = e.to_string();
                let new = invalid_row(&self.state_dir, sha256);
                if let Err(err) = self.db.script_generation_record_invalid(
                    &new,
                    &reason,
                    OffsetDateTime::now_utc(),
                ) {
                    tracing::error!(error = %err, bundle_sha256 = sha256, "script runtime: could not record the invalid generation");
                }
                return Err(ScriptGenerationError::Invalid {
                    bundle_sha256: sha256.to_owned(),
                    reason,
                });
            }
            Err(e) => return Err(ScriptGenerationError::Runtime(e.to_string())),
        }
        let recorded = self
            .registry
            .manifest(sha256)
            .ok_or_else(|| ScriptGenerationError::Runtime("the loaded generation vanished".into()))
            .and_then(|m| new_row(&m).map_err(ScriptGenerationError::Runtime))
            .and_then(|new| {
                self.db
                    .script_generation_record(&new, OffsetDateTime::now_utc())
                    .map_err(ScriptGenerationError::from)
            });
        if recorded.is_err() {
            // store が拒んだ: isolate を捨てる（表に無い isolate を残さない）。
            let _ = self.runtime.block_on(self.registry.retire(sha256));
        }
        recorded
    }

    fn retire_locked(
        &self,
        id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<ScriptGeneration, ScriptGenerationError> {
        let current = self
            .db
            .script_generation_get(id)
            .map_err(store_err)?
            .ok_or_else(|| ScriptDomainError::GenerationNotFound(id.to_owned()))?;
        let in_flight = self
            .registry
            .get(&current.bundle_sha256)
            .map_or(0, |i| i.in_flight);
        let row = self.db.script_generation_transition(
            &ScriptGenerationTransition {
                id: id.to_owned(),
                to: ScriptGenerationState::Retired,
                actor: actor.to_owned(),
                reason: reason.to_owned(),
                cancelled_in_flight: Some(in_flight as u64),
            },
            OffsetDateTime::now_utc(),
        )?;
        // 表が `retired` になってから isolate を捨てる（残った要求は `GenerationGone`）。
        let _ = self
            .runtime
            .block_on(self.registry.invalidate(&current.bundle_sha256));
        self.forget(&current.bundle_sha256);
        Ok(row)
    }

    /// 網羅の行を書き、runner を起こす（D2・D3・D7）。表の拒否は store の `script_exhaustive_begin` が決める。
    fn exhaustive_start_inner(
        &self,
        domain: ScriptDomain,
        bundle_sha256: &str,
        actor: &str,
        reason: &str,
    ) -> Result<ScriptExhaustiveRun, ScriptGenerationError> {
        let live = |info: Option<celeris_script::GenerationInfo>| info.is_some_and(|i| !i.draining);
        if domain == ScriptDomain::LaneRules && !live(self.registry.get(bundle_sha256)) {
            // 行の generation は在るが、この daemon に isolate が無い（復元に落ちた等）。表を変えない。
            if self
                .db
                .script_generation_by_sha256(bundle_sha256)
                .map_err(store_err)?
                .is_some_and(|g| g.state.is_live())
            {
                return Err(ScriptGenerationError::Unavailable(format!(
                    "generation `{bundle_sha256}` has no isolate in this daemon"
                )));
            }
        }
        let reference = current_reference_sha256().map_err(store_err)?;
        let run = self.db.script_exhaustive_begin(
            &NewScriptExhaustiveRun {
                domain,
                bundle_sha256: bundle_sha256.to_owned(),
                daemon_instance_id: self.instance_id.clone(),
                requested_by: actor.to_owned(),
                reason: reason.to_owned(),
                reference_sha256: reference.to_owned(),
            },
            OffsetDateTime::now_utc(),
        )?;
        let pinned = self
            .db
            .script_generation_get(&run.generation_id)
            .map_err(store_err)?
            .ok_or_else(|| ScriptDomainError::GenerationNotFound(run.generation_id.clone()))
            .map_err(ScriptGenerationError::from)
            .and_then(|g| self.pinned(&g).map_err(ScriptGenerationError::Unavailable));
        let policy = match pinned {
            Ok(p) => p,
            Err(e) => {
                // 行を残したまま走らせない: 書いた行を `aborted`（`generation_gone`）で閉じる。
                let _ = self.db.script_exhaustive_finish(
                    &run.id,
                    ScriptExhaustiveState::Aborted,
                    Some(ScriptExhaustiveStopCode::GenerationGone),
                    &run.counts,
                    None,
                    OffsetDateTime::now_utc(),
                );
                return Err(e);
            }
        };
        let probe: TargetProbe = {
            let registry = self.registry.clone();
            let sha = run.bundle_sha256.clone();
            Arc::new(move || match registry.get(&sha) {
                None => TargetState::Gone,
                Some(i) if i.draining => TargetState::Gone,
                Some(i) if i.in_flight > 0 => TargetState::LiveInFlight,
                Some(_) => TargetState::Ready,
            })
        };
        let tuning = lock(&self.exhaustive_tuning).clone();
        let handle = self.runtime.spawn(script_exhaustive::run(
            policy,
            probe,
            tuning,
            self.exhaustive_stop.subscribe(),
            Arc::clone(&self.db),
            run.id.clone(),
        ));
        let mut tasks = lock(&self.exhaustive_tasks);
        tasks.retain(|t| !t.is_finished());
        tasks.push(handle);
        tracing::info!(
            run = %run.id,
            generation = %run.generation_id,
            mode = %run.mode_at_start,
            actor,
            "script exhaustive: run started"
        );
        Ok(run)
    }
}

impl ScriptGenerationControl for ScriptGenerations {
    fn register(&self, bundle_sha256: &str) -> Result<ScriptGeneration, ScriptGenerationError> {
        self.ensure_ready()?;
        let _ops = self.ops.blocking_lock();
        self.register_locked(bundle_sha256)
    }

    fn retire(
        &self,
        generation_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<ScriptGeneration, ScriptGenerationError> {
        self.ensure_ready()?;
        let _ops = self.ops.blocking_lock();
        self.retire_locked(generation_id, actor, reason)
    }

    fn exhaustive_start(
        &self,
        domain: ScriptDomain,
        bundle_sha256: &str,
        actor: &str,
        reason: &str,
    ) -> Result<ScriptExhaustiveRun, ScriptGenerationError> {
        self.ensure_ready()?;
        // 登録・退役・見回りと行の読み書きを食い違わせない（runner 自身は lock を持たない。D3）。
        let _ops = self.ops.blocking_lock();
        self.exhaustive_start_inner(domain, bundle_sha256, actor, reason)
    }
}

impl PolicyScriptSource for ScriptGenerations {
    fn pinned(&self, generation: &ScriptGeneration) -> Result<Arc<dyn PolicyScript>, String> {
        let sha = &generation.bundle_sha256;
        match self.registry.get(sha) {
            None => Err(format!(
                "script generation `{}` has no isolate in this daemon",
                generation.id
            )),
            Some(info) if info.draining => {
                Err(format!("script generation `{}` is draining", generation.id))
            }
            Some(_) => {
                let mut policies = lock(&self.policies);
                let policy = policies.entry(sha.clone()).or_insert_with(|| {
                    Arc::new(ScriptPolicy::pinned(self.registry.clone(), sha.clone()))
                });
                Ok(Arc::clone(policy) as Arc<dyn PolicyScript>)
            }
        }
    }
}

fn store_err(e: task_core::StoreError) -> ScriptGenerationError {
    ScriptGenerationError::Domain(ScriptDomainError::from(e))
}

fn new_row(manifest: &ScriptManifest) -> Result<NewScriptGeneration, String> {
    Ok(NewScriptGeneration {
        bundle_sha256: manifest.bundle_sha256.clone(),
        host_api: HOST_API_VERSION.to_owned(),
        schema_sha256: manifest.schema_sha256.clone(),
        manifest_json: serde_json::to_string(manifest).map_err(|e| e.to_string())?,
        state: ScriptGenerationState::Loaded,
    })
}

/// 検証に落ちた bundle の行。manifest が読めればその値を、読めなければ空の manifest を書く。
fn invalid_row(state_dir: &std::path::Path, sha256: &str) -> NewScriptGeneration {
    let manifest = load_deployed(state_dir, sha256).ok().map(|b| b.manifest);
    NewScriptGeneration {
        bundle_sha256: sha256.to_owned(),
        host_api: HOST_API_VERSION.to_owned(),
        schema_sha256: manifest
            .as_ref()
            .map(|m| m.schema_sha256.clone())
            .unwrap_or_default(),
        manifest_json: manifest
            .as_ref()
            .and_then(|m| serde_json::to_string(m).ok())
            .unwrap_or_else(|| "{}".to_owned()),
        state: ScriptGenerationState::Invalid,
    }
}

/// 起動時の戻しを書く。失敗は log に残して続ける（判定は Rust のまま、次の起動でやり直す）。
fn rollback_rows(db: &SqliteStore, rollbacks: &[(ScriptDomainModeRow, String)]) {
    for (row, reason) in rollbacks {
        let change = ScriptModeChange {
            domain: row.domain.as_str().to_owned(),
            to_mode: ScriptMode::Rust,
            to_generation: None,
            expected_version: row.version,
            actor: STARTUP_ACTOR.to_owned(),
            reason: reason.clone(),
        };
        match db.script_domain_set_mode(&change, OffsetDateTime::now_utc()) {
            Ok(_) => tracing::warn!(
                domain = row.domain.as_str(),
                %reason,
                "script runtime: domain mode restored to rust"
            ),
            Err(e) => tracing::error!(
                error = %e,
                domain = row.domain.as_str(),
                "script runtime: could not restore the domain mode to rust"
            ),
        }
    }
}

/// 起動時に、mode の行から指されない `draining`（drain の終わり、in-flight 0）と、isolate を持たない `loaded`
/// （前の process の isolate は消えた）を `retired` にする。`keep` はこの process の isolate の key（`bundle_sha256`）。
fn retire_unpointed(db: &SqliteStore, keep: &BTreeSet<String>) {
    let pointed: BTreeSet<String> = match db.script_domain_modes() {
        Ok(rows) => rows.into_iter().filter_map(|r| r.generation_id).collect(),
        Err(e) => {
            tracing::error!(error = %e, "script runtime: could not read the domain modes");
            return;
        }
    };
    let generations = match db.script_generations() {
        Ok(g) => g,
        Err(e) => {
            tracing::error!(error = %e, "script runtime: could not read the generations");
            return;
        }
    };
    for g in generations {
        if pointed.contains(&g.id) || keep.contains(&g.bundle_sha256) {
            continue;
        }
        let reason = match g.state {
            ScriptGenerationState::Draining => {
                "drained: the daemon restarted with no request in flight"
            }
            ScriptGenerationState::Loaded => {
                "startup_no_isolate: the daemon restarted; register the bundle again to load it"
            }
            _ => continue,
        };
        retire_row(db, &g.id, STARTUP_ACTOR, reason, 0);
    }
}

fn retire_row(db: &SqliteStore, id: &str, actor: &str, reason: &str, cancelled: usize) {
    let change = ScriptGenerationTransition {
        id: id.to_owned(),
        to: ScriptGenerationState::Retired,
        actor: actor.to_owned(),
        reason: reason.to_owned(),
        cancelled_in_flight: Some(cancelled as u64),
    };
    match db.script_generation_transition(&change, OffsetDateTime::now_utc()) {
        Ok(_) => tracing::info!(generation = id, %reason, "script runtime: generation retired"),
        Err(e) => {
            tracing::error!(error = %e, generation = id, "script runtime: could not retire the generation")
        }
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(format!("the blocking task failed: {e}")))
}

/// 復元・見回りをしてよい役割か（verify は DB の写しを相手にするので active と同じ）。
fn may_restore(role: &SharedRole) -> bool {
    matches!(role.get(), InstanceRole::Active | InstanceRole::Verify)
}

/// daemon の TS 層。[`Self::stop`] で見回りを止め、isolate を捨てる。
pub(crate) struct ScriptLayer {
    generations: Option<Arc<ScriptGenerations>>,
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl ScriptLayer {
    /// API より前に呼ぶ。`[script]` が無い・DB を開けなければ `None`（API の登録は 503）。isolate はまだ作らない。
    /// `registry` が在ればそれを使う（試験の入口 `run_with_test_hooks`）。
    pub(crate) fn prepare(
        config: &Config,
        registry: Option<GenerationRegistry>,
        instance_id: &str,
    ) -> Option<Arc<ScriptGenerations>> {
        let script = config.script.as_ref()?;
        let db = open_db(config)?;
        Some(ScriptGenerations::for_instance(
            registry.unwrap_or_else(|| GenerationRegistry::new(script.limits())),
            script.state_dir.clone(),
            Arc::new(db),
            instance_id.to_owned(),
        ))
    }

    /// 起動時の復元を行い（active・verify のとき。standby は active になってから）、見回りを始める。
    /// `[script]` が無ければ generation の要る mode を全部 `rust` に戻す（付記 S4-14）。
    pub(crate) async fn start(
        config: &Config,
        generations: Option<Arc<ScriptGenerations>>,
        role: SharedRole,
    ) -> Self {
        let (stop, mut stop_rx) = tokio::sync::watch::channel(false);
        if may_restore(&role) {
            restore_once(config, generations.as_ref()).await;
        }
        let task = {
            let generations = generations.clone();
            let config = config.clone();
            tokio::spawn(async move {
                let mut restored = may_restore(&role);
                loop {
                    tokio::select! {
                        _ = stop_rx.changed() => break,
                        _ = tokio::time::sleep(RECONCILE_INTERVAL) => {}
                    }
                    if !may_restore(&role) {
                        continue;
                    }
                    if !restored {
                        restored = restore_once(&config, generations.as_ref()).await;
                        continue;
                    }
                    match &generations {
                        Some(g) if !g.is_ready() => {
                            restored = restore_once(&config, Some(g)).await;
                        }
                        Some(g) => g.reconcile().await,
                        None => break,
                    }
                }
            })
        };
        Self {
            generations,
            stop,
            task,
        }
    }

    pub(crate) fn source(&self) -> Option<Arc<dyn PolicyScriptSource>> {
        self.generations.as_ref().map(|g| g.source())
    }

    /// 見回りを止めて isolate を捨てる。
    pub(crate) async fn stop(self) {
        let _ = self.stop.send(true);
        if let Err(e) = self.task.await {
            tracing::error!(error = %e, "script runtime: maintenance task failed");
        }
        if let Some(g) = self.generations {
            g.shutdown().await;
        }
    }
}

/// 1 回の復元。成功したら true。
async fn restore_once(config: &Config, generations: Option<&Arc<ScriptGenerations>>) -> bool {
    match generations {
        Some(g) => match g.restore().await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(%error, "script runtime: startup restore failed; retrying");
                false
            }
        },
        None => {
            let config = config.clone();
            let done = blocking(move || {
                let db = open_db(&config).ok_or_else(|| "store unavailable".to_owned())?;
                restore_without_runtime(&db)
            })
            .await;
            if let Err(error) = &done {
                tracing::error!(%error, "script runtime: startup restore failed");
            }
            // DB を開けなければ次の起動まで待つ（`[script]` が無いので dispatcher は TS を呼ばない）。
            true
        }
    }
}

/// `[script]` の無い daemon の起動時の復元: generation の要る mode を全部 `rust` に戻す。
fn restore_without_runtime(db: &SqliteStore) -> Result<(), String> {
    let rows = db
        .script_domain_active_generations()
        .map_err(|e| e.to_string())?;
    let halted = db.script_guard_halted().map_err(|e| e.to_string())?;
    let stored = stored_checkpoint_versions();
    let rollbacks: Vec<_> = rows
        .iter()
        .filter_map(|(row, g)| {
            restore_reason(row, g.as_ref(), None, halted.as_deref(), &stored)
                .map(|reason| (row.clone(), reason))
        })
        .collect();
    rollback_rows(db, &rollbacks);
    retire_unpointed(db, &BTreeSet::new());
    Ok(())
}

fn open_db(config: &Config) -> Option<SqliteStore> {
    match SqliteStore::open_with(
        &config.db.path,
        StoreOptions {
            busy_timeout: config.db.busy_timeout(),
            ..StoreOptions::default()
        },
    ) {
        Ok(db) => Some(db),
        Err(e) => {
            tracing::error!(error = %e, "script runtime: could not open the store for the generations");
            None
        }
    }
}

#[cfg(test)]
#[path = "script_layer_tests.rs"]
mod tests;
