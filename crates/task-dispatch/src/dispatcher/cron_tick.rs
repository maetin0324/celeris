//! ADR-0131 D4: the tick stage that evaluates cron jobs before dispatch. Task jobs create tasks;
//! `cos_review` jobs post one input to the general CoS thread (ADR 2026-10-27-cron-cos-review,
//! [`super::cos_chat::review::CosReviewSink`]). No LLM call here.

use super::Dispatcher;
use super::cos_chat::review::CosReviewSink;

/// A6: 判定の前の guard（`project_wake.rs` の行を増やさない）。
#[path = "project_guard.rs"]
pub(crate) mod project_guard;
/// ADR 2026-11-04-project-controller A1: 案件の durable wake の段（`dispatcher.rs` に行を足さない）。
#[path = "project_wake.rs"]
pub(crate) mod project_wake;

impl Dispatcher {
    /// A draining instance or one short of disk creates nothing. Uses the injectable clock.
    pub(super) fn tick_cron_jobs(&mut self) {
        // ADR-0041 D5: verify は cron job を発火しない（staging DB に task・CoS 入力を作らない）。
        if self.verify_smoke_only() {
            return;
        }
        if !(self.accepting_new_work && self.disk_ready) {
            return;
        }
        let ctx = task_ops::cron_jobs::CronFireContext {
            roles: &self.config.roles,
            genres: &self.config.genres,
            ..Default::default()
        };
        let now = self.now_utc();
        let mut sink = CosReviewSink {
            launch: self.cos_chat_launch.as_ref(),
            findings: &self.stagnation.findings,
            now,
        };
        let mut fired = Vec::new();
        for result in task_ops::cron_jobs::fire_due_with(self.store.as_ref(), &ctx, &mut sink, now)
        {
            match result {
                Ok(outcome) => {
                    fired.extend(outcome.task_id);
                    tracing::info!(
                        job_id = %outcome.job_id,
                        job_name = %outcome.job_name,
                        task_id = ?outcome.task_id,
                        runs = ?outcome.runs,
                        "cron job evaluated"
                    )
                }
                Err(error) => tracing::warn!(error = %error, "cron job evaluation failed"),
            }
        }
        // ADR 2026-10-07-build-tmp-hygiene D1.4: `action` 付きの雛形の task は worker に渡さず、
        // ここで決定的な保守 executor が走らせる（LLM なし）。
        self.run_maintenance_tasks(&fired);
        // ADR 2026-11-04-project-controller A1: cron の後に案件の wake を評価する（同じ gate の内側）。
        self.tick_project_controller();
    }
}
