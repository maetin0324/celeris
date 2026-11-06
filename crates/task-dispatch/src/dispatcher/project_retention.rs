//! 第 4 フェーズ P4-D（ADR 2026-11-05-phase4-p4d-retention）: wake・判断・compare の記録の GC の段。
//!
//! `[project_controller.retention]` があるときだけ、`interval_secs` ごとに 1 回 `SqliteStore::retention_gc`
//! を呼ぶ（表ごとに最大 `max_rows_per_pass` 行、1 transaction に `batch_rows` 行）。消してよい行の条件は
//! store と DB の trigger が持ち、ここは間隔と記録だけ。LLM 呼び出しは無い。

use super::Dispatcher;

/// GC の記録（`project_retention_runs.actor`）の主体。
pub const RETENTION_ACTOR: &str = "system:project-retention";

impl Dispatcher {
    pub(crate) fn tick_project_retention(&mut self) {
        let now = self.now_utc();
        let Some(launch) = self.cos_chat_launch.as_mut() else {
            return;
        };
        let store = launch.store.clone();
        let Some(rt) = launch.project_controller.as_mut() else {
            return;
        };
        let Some(r) = rt.settings.retention.clone() else {
            return;
        };
        if rt.retention_next.is_some_and(|next| now < next) {
            return;
        }
        let secs = i64::try_from(r.interval_secs).unwrap_or(i64::MAX / 2);
        rt.retention_next = Some(now + time::Duration::seconds(secs));
        match store.retention_gc(&r.policy, now, &r.archive_dir, RETENTION_ACTOR) {
            Ok(report) => {
                if report.deleted() > 0 {
                    tracing::info!(
                        deleted = report.deleted(),
                        "project retention: rows archived and deleted"
                    );
                }
                rt.retention_last = Some(report);
            }
            Err(error) => {
                tracing::warn!(%error, "project retention: GC failed; rows are kept");
            }
        }
    }
}
