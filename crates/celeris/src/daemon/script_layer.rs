//! TS 制御層の起動と停止（境界 ADR 2026-11-02 (b)(d)）。
//!
//! `[script]` が無ければ何も作らない。在れば `<state_dir>/script-bundles/<sha256>/` を検証して actor を起こす。
//! 失敗したら TS 層を使わずに daemon の起動を続け、理由を log と通知に残す（fail-closed）。
//! TS の結果は compare の記録にだけ使う（付記 S2-6）。判定は Rust のまま。
//!
//! 起動時の DB の整合（付記 S2-4・S2-5）:
//! - 検証に通った bundle の `loaded` の行を `script_generations` に作る（同じ hash の行が在れば再利用）。
//! - `compare` の領域の generation が読み込んだ bundle と違う（`[script]` が無い・bundle を読めなかったを含む）なら、
//!   actor `system:script-startup` で `rust` へ戻す。

use std::sync::Arc;

use celeris_script::ScriptRuntimeHandle;
use celeris_script::dto::HOST_API_VERSION;
use celeris_script::manifest::{VerifiedBundle, load_deployed};
use task_core::script_compare::{
    NewScriptGeneration, ScriptGenerationState, ScriptMode, ScriptModeChange,
};
use task_core::{NoticeEvent, NoticeKind, SqliteStore, StoreOptions, TaskStore};
use task_dispatch::script_policy::PolicyScript;
use time::OffsetDateTime;

use super::script_policy::ScriptPolicy;
use crate::config::{Config, ScriptConfig};

/// 起動時の戻しの actor（付記 S2-5）。
const STARTUP_ACTOR: &str = "system:script-startup";

/// 動いている TS 層。[`Self::stop`] で actor thread を join する。
pub(crate) struct ScriptLayer {
    handle: ScriptRuntimeHandle,
    bundle_sha256: String,
}

impl ScriptLayer {
    /// 起動時に 1 回呼ぶ。`[script]` が無ければ thread を作らない。失敗は `None` にして通知する。
    /// どの場合も、読み込めた bundle に合わせて DB の generation と mode を直す。
    pub(crate) async fn start(
        config: &Config,
        store: &dyn TaskStore,
        instance_id: &str,
    ) -> Option<Self> {
        let started = match config.script.as_ref() {
            Some(script) => Self::load(script, store, instance_id).await,
            None => None,
        };
        let db = SqliteStore::open_with(
            &config.db.path,
            StoreOptions {
                busy_timeout: config.db.busy_timeout(),
                ..StoreOptions::default()
            },
        );
        match db {
            Ok(db) => {
                if let Some((_, bundle)) = &started {
                    record_generation(&db, bundle);
                }
                restore_modes(
                    &db,
                    started
                        .as_ref()
                        .map(|(layer, _)| layer.bundle_sha256.as_str()),
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "script runtime: could not open the store for the startup checks");
            }
        }
        started.map(|(layer, _)| layer)
    }

    /// bundle を検証して actor を起こす。
    async fn load(
        config: &ScriptConfig,
        store: &dyn TaskStore,
        instance_id: &str,
    ) -> Option<(Self, VerifiedBundle)> {
        let config = config.clone();
        let sha256 = config.active_bundle_sha256.clone();
        // actor の起動は bundle の評価が終わるまで呼び出し元の thread を塞ぐ。
        let started = tokio::task::spawn_blocking(move || start_blocking(&config))
            .await
            .unwrap_or_else(|e| Err(format!("the startup task failed: {e}")));
        match started {
            Ok((handle, bundle)) => {
                tracing::info!(bundle_sha256 = %sha256, "script runtime: bundle loaded");
                Some((
                    Self {
                        handle,
                        bundle_sha256: sha256,
                    },
                    bundle,
                ))
            }
            Err(reason) => {
                tracing::error!(
                    bundle_sha256 = %sha256, %reason,
                    "script runtime: bundle rejected; continuing without the TS layer (fail-closed)"
                );
                record_failure(store, instance_id, &sha256, &reason);
                None
            }
        }
    }

    /// dispatcher に渡す判定の口。
    pub(crate) fn policy_script(&self) -> Arc<dyn PolicyScript> {
        Arc::new(ScriptPolicy::new(
            self.handle.clone(),
            self.bundle_sha256.clone(),
        ))
    }

    /// actor を止めて thread を join する。
    pub(crate) async fn stop(self) {
        let handle = self.handle;
        if let Err(e) = tokio::task::spawn_blocking(move || handle.shutdown()).await {
            tracing::error!(error = %e, "script runtime: shutdown task failed");
        }
    }
}

fn start_blocking(config: &ScriptConfig) -> Result<(ScriptRuntimeHandle, VerifiedBundle), String> {
    let bundle = load_deployed(&config.state_dir, &config.active_bundle_sha256)
        .map_err(|e| e.to_string())?;
    let handle = ScriptRuntimeHandle::start_with_limits(bundle.source(), config.limits())
        .map_err(|e| e.to_string())?;
    Ok((handle, bundle))
}

/// `loaded` の行を作る。失敗しても起動は続ける（compare へ切り替えられないだけで、判定は Rust のまま）。
fn record_generation(db: &SqliteStore, bundle: &VerifiedBundle) {
    let manifest = &bundle.manifest;
    let manifest_json = match serde_json::to_string(manifest) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!(error = %e, "script runtime: could not serialize the manifest");
            return;
        }
    };
    let new = NewScriptGeneration {
        bundle_sha256: manifest.bundle_sha256.clone(),
        host_api: HOST_API_VERSION.to_owned(),
        schema_sha256: manifest.schema_sha256.clone(),
        manifest_json,
        state: ScriptGenerationState::Loaded,
    };
    if let Err(e) = db.script_generation_record(&new, OffsetDateTime::now_utc()) {
        tracing::error!(error = %e, "script runtime: could not record the generation");
    }
}

/// `compare` の領域のうち、`active` の bundle で動かせないものを `rust` へ戻す（付記 S2-5）。
fn restore_modes(db: &SqliteStore, active: Option<&str>) {
    let rows = match db.script_domain_modes() {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "script runtime: could not read the domain modes");
            return;
        }
    };
    for row in rows.into_iter().filter(|r| r.mode == ScriptMode::Compare) {
        let generation = match row.generation_id.as_deref() {
            Some(id) => db.script_generation_get(id),
            None => Ok(None),
        };
        let generation = match generation {
            Ok(generation) => generation,
            Err(e) => {
                tracing::error!(error = %e, domain = row.domain.as_str(), "script runtime: could not read the generation");
                continue;
            }
        };
        let wanted = generation.as_ref().map(|g| g.bundle_sha256.as_str());
        if wanted.is_some() && wanted == active {
            continue;
        }
        let change = ScriptModeChange {
            domain: row.domain.as_str().to_owned(),
            to_mode: ScriptMode::Rust,
            to_generation: None,
            expected_version: row.version,
            actor: STARTUP_ACTOR.to_owned(),
            reason: format!(
                "the generation's bundle {} is not the loaded bundle {}",
                wanted.unwrap_or("(unknown)"),
                active.unwrap_or("(none)")
            ),
        };
        match db.script_domain_set_mode(&change, OffsetDateTime::now_utc()) {
            Ok(_) => tracing::warn!(
                domain = row.domain.as_str(),
                reason = %change.reason,
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

/// 通知の記録に失敗しても起動は続ける（log には理由が残っている）。
fn record_failure(store: &dyn TaskStore, instance_id: &str, sha256: &str, reason: &str) {
    let recorded = store.notice_record(&NoticeEvent {
        source_key: format!("script_bundle_rejected:{instance_id}"),
        kind: NoticeKind::BadNews,
        group_key: "script_bundle_rejected".to_owned(),
        title: "TS 制御層の bundle を読めなかった".to_owned(),
        summary: format!(
            "daemon は TS 層なしで起動した。全領域は Rust のまま動く。bundle {sha256}: {reason}"
        ),
        project_id: None,
        task_id: None,
        target: None,
        links: Vec::new(),
        at: OffsetDateTime::now_utc(),
    });
    if let Err(e) = recorded {
        tracing::error!(error = %e, "script runtime: could not record the notice");
    }
}
