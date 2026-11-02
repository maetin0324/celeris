//! TS 制御層の起動と停止（境界 ADR 2026-11-02 (b)(d)）。
//!
//! `[script]` が無ければ何も作らない。在れば `<state_dir>/script-bundles/<sha256>/` を検証して actor を起こす。
//! 失敗したら TS 層を使わずに daemon の起動を続け、理由を log と通知に残す（fail-closed）。
//! S1 では業務領域を TS に切り替えない。handle を持つのは `run` だけで、判定には使わない。

use celeris_script::ScriptRuntimeHandle;
use celeris_script::manifest::load_deployed;
use task_core::{NoticeEvent, NoticeKind, TaskStore};
use time::OffsetDateTime;

use crate::config::ScriptConfig;

/// 動いている TS 層。[`Self::stop`] で actor thread を join する。
pub(crate) struct ScriptLayer {
    handle: ScriptRuntimeHandle,
}

impl ScriptLayer {
    /// 起動時に 1 回呼ぶ。`config` が `None` なら thread を作らない。失敗は `None` にして通知する。
    pub(crate) async fn start(
        config: Option<&ScriptConfig>,
        store: &dyn TaskStore,
        instance_id: &str,
    ) -> Option<Self> {
        let config = config?.clone();
        let sha256 = config.active_bundle_sha256.clone();
        // actor の起動は bundle の評価が終わるまで呼び出し元の thread を塞ぐ。
        let started = tokio::task::spawn_blocking(move || start_blocking(&config))
            .await
            .unwrap_or_else(|e| Err(format!("the startup task failed: {e}")));
        match started {
            Ok(handle) => {
                tracing::info!(bundle_sha256 = %sha256, "script runtime: bundle loaded");
                Some(Self { handle })
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

    /// actor を止めて thread を join する。
    pub(crate) async fn stop(self) {
        let handle = self.handle;
        if let Err(e) = tokio::task::spawn_blocking(move || handle.shutdown()).await {
            tracing::error!(error = %e, "script runtime: shutdown task failed");
        }
    }
}

fn start_blocking(config: &ScriptConfig) -> Result<ScriptRuntimeHandle, String> {
    let bundle = load_deployed(&config.state_dir, &config.active_bundle_sha256)
        .map_err(|e| e.to_string())?;
    ScriptRuntimeHandle::start_with_limits(bundle.source(), config.limits())
        .map_err(|e| e.to_string())
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
