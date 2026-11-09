//! ADR 2026-11-08-health-enforcement-loop (6): cron の種類 `health_inventory`（任意・低頻度の全体棚卸し）。
//!
//! task も chat も作らず、発火ごとに [`CronInputSink::post_health_inventory`] で案件（`extra.project_id`）の
//! 棚卸しを 1 回分 dispatcher に積む（D6.1）。検査器の実行・記録・起票は dispatcher 側の仕事で、この module は
//! 雛形の検証と sink への受け渡しだけを行う（LLM・外部 process・network は呼ばない）。既定では job を作らない
//! （D6.2。config の例・seed に置かない。有効にするのは人）。

use task_core::CronJob;
use time::OffsetDateTime;

use super::input_kind::{require_project_id, template_project_id, validate_input_kind};
use super::{CronInputPost, CronInputSink};
use crate::error::OpsError;

/// 雛形の `extra["kind"]` の値。
pub const CRON_KIND_HEALTH_INVENTORY: &str = "health_inventory";

/// 雛形の検証: task の欄は書けず、重ね掛けは `skip` だけ（[`validate_input_kind`]）、`extra.project_id` は
/// 必須（文字列）。
pub(super) fn validate(job: &CronJob) -> Result<(), OpsError> {
    validate_input_kind(job, CRON_KIND_HEALTH_INVENTORY)?;
    require_project_id(job, CRON_KIND_HEALTH_INVENTORY).map(|_| ())
}

/// 1 回の発火: 対象の案件を読み、sink に棚卸しを積ませる。`Err` は履歴の `error`。
pub(super) fn post(
    sink: &mut dyn CronInputSink,
    job: &CronJob,
    scheduled_for: OffsetDateTime,
) -> Result<CronInputPost, String> {
    match template_project_id(job) {
        Some(project) => sink.post_health_inventory(job, scheduled_for, &project),
        None => Err("template.extra.project_id is missing".into()),
    }
}

/// [`CronInputSink::post_health_inventory`] の既定（daemon の tick 以外の呼び出し元）の答え。
pub(super) fn no_sink() -> String {
    format!("{CRON_KIND_HEALTH_INVENTORY} runs are posted only by the daemon tick")
}

/// 手動実行の拒否（`project_reconcile` と同じく、積む先が daemon の tick にしか無い）。
pub(super) fn manual_run_refused() -> OpsError {
    OpsError::Validation(format!(
        "a {CRON_KIND_HEALTH_INVENTORY} job fires only on its schedule (the daemon tick posts the run)"
    ))
}
