//! task を作らない種類（`cos_review`・`project_reconcile`・`health_inventory`）の雛形の共通の検証と読み取り。
//!
//! これらの種類は発火で task を作らず、[`super::CronInputSink`] に入力を積むだけなので、task の欄は書けず、
//! 重ね掛けは `skip` だけ（溜めた入力は積んだ先の待ち行列にあるので `queue` は持たない）。種類ごとに同じ
//! 一覧と検査を書かないよう、検証はここの [`validate_input_kind`] 1 つに置く（ADR
//! 2026-11-08-health-enforcement-loop D6.1）。

use task_core::{CronJob, CronOverlap, CronTaskTemplate};

use crate::error::OpsError;

/// task を作らない種類の雛形に書けない task の欄の名前（[`set_task_fields`] と同じ順）。
pub const INPUT_KIND_TASK_FIELDS: [&str; 9] = [
    "acceptance",
    "assignee",
    "harness",
    "lane",
    "project",
    "repos",
    "priority",
    "extra.mode",
    "extra.action",
];

/// 雛形の task の欄のうち書かれているもの（[`INPUT_KIND_TASK_FIELDS`] の名前）。
fn set_task_fields(t: &CronTaskTemplate) -> impl Iterator<Item = &'static str> {
    let set = [
        !t.acceptance.is_empty(),
        t.assignee.is_some(),
        t.harness.is_some(),
        t.lane.is_some(),
        t.project.is_some(),
        !t.repos.is_empty(),
        t.priority.is_some(),
        t.extra.contains_key("mode"),
        t.extra.contains_key("action"),
    ];
    INPUT_KIND_TASK_FIELDS
        .into_iter()
        .zip(set)
        .filter_map(|(field, set)| set.then_some(field))
}

/// task を作らない種類 `kind`（`extra.kind` の値）の雛形の検証: task の欄が書かれていれば最初の 1 つを、
/// 重ね掛けが `skip` でなければその旨を `Validation` で返す。種類に固有の欄の検査は呼び出し元が続けて行う。
pub fn validate_input_kind(job: &CronJob, kind: &str) -> Result<(), OpsError> {
    if let Some(field) = set_task_fields(&job.template).next() {
        return Err(OpsError::Validation(format!(
            "template.{field} is a task field; a {kind} job does not create a task"
        )));
    }
    if job.overlap != CronOverlap::Skip {
        return Err(OpsError::Validation(format!(
            "a {kind} job must use overlap \"skip\""
        )));
    }
    Ok(())
}

/// 雛形の対象の案件 id（`extra.project_id`。前後の空白を除き、空なら無し）。
pub fn template_project_id(job: &CronJob) -> Option<String> {
    job.template
        .extra
        .get("project_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `extra.project_id` が必須の種類 `kind` の検証（[`template_project_id`] で読めなければ `Validation`）。
pub fn require_project_id(job: &CronJob, kind: &str) -> Result<String, OpsError> {
    template_project_id(job).ok_or_else(|| {
        OpsError::Validation(format!(
            "a {kind} job needs template.extra.project_id (a string)"
        ))
    })
}
