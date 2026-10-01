//! ADR-0098（Phase R7-10）: worker の run が宣言した後続 task（`<artifacts_dir>/followups.json`）。
//!
//! - run の前: 古い宣言を消し（`delegate.json` / `plan.json` と同じ）、`celerisctl add` が書き先を知るための env を渡す。
//! - run の後（終わり方に依らず）: run がまだ lease を持っていれば `task_ops::followup::absorb_followups_file`。
//!
//! 出自（task / run）はこの run を起こした daemon が持つ値で、ファイルや env の自己申告は使わない（ADR-0098 D2）。
//! 業務の規則（案件・repos の継承、落とす欄、重複）は `task_ops::followup` にある。

use super::*;

use task_ops::followup::{
    ENV_FOLLOWUPS_FILE, ENV_RUN_DB, ENV_RUN_ID, ENV_TASK_ID, FOLLOWUPS_FILE_NAME,
};

/// ADR-0098 D6: worker の run に渡す env（`CELERIS_TASK_ID` / `CELERIS_RUN_ID` / `CELERIS_FOLLOWUPS_FILE`、
/// daemon の DB が分かれば `CELERIS_RUN_DB`）。`run_db` は ADR-0095 の `db_guard` が守る DB（入っていなければ `None`。
/// そのときは run の中の `celerisctl add` は従来どおり DB に向かう）。
pub(super) fn followups_env(
    task_id: TaskId,
    run_id: &str,
    artifacts_dir: &Path,
    run_db: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = vec![
        (ENV_TASK_ID.to_string(), task_id.to_string()),
        (ENV_RUN_ID.to_string(), run_id.to_string()),
        (
            ENV_FOLLOWUPS_FILE.to_string(),
            artifacts_dir
                .join(FOLLOWUPS_FILE_NAME)
                .display()
                .to_string(),
        ),
    ];
    if let Some(db) = run_db {
        env.push((ENV_RUN_DB.to_string(), db.display().to_string()));
    }
    env
}

/// ADR-0098 D1: 前の run（lease を失った・daemon ごと落ちた）が残した宣言を今回のものと誤読しない。
pub(super) async fn clear_stale_followups(artifacts_dir: &Path) {
    let _ = tokio::fs::remove_file(artifacts_dir.join(FOLLOWUPS_FILE_NAME)).await;
}

/// `Dispatcher::run_holds_lease` と同じ規則（v1・atomic は Task の lease、v2 は running の WU の `lease_run_id`）。
pub(super) fn store_run_holds_lease(
    store: &dyn TaskStore,
    task: &Task,
    run_id: &str,
) -> Result<bool, task_core::StoreError> {
    if task.status != Status::Running {
        return Ok(false);
    }
    let Some(lease) = task.lease.as_ref() else {
        return Ok(false);
    };
    if lease.worker_run_id == run_id {
        return Ok(true);
    }
    if !is_phase_lease_holder(&lease.worker_run_id) {
        return Ok(false);
    }
    Ok(store.work_units_for(task.id)?.iter().any(|u| {
        u.status == task_core::WorkUnitStatus::Running && u.lease_run_id.as_deref() == Some(run_id)
    }))
}

/// ADR-0098 D1: run `run_id`（task `task_id`）が返った直後に、その run の成果物ディレクトリの宣言を取り込む。
/// lease を失った run の宣言は作らない（ファイルは残し、次の run の開始時に消える）。失敗しても run は壊さない。
pub(super) fn absorb_run_followups(
    store: &dyn TaskStore,
    task_id: TaskId,
    run_id: &str,
    artifacts_dir: &Path,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
) -> Option<task_ops::followup::AbsorbOutcome> {
    if !artifacts_dir.join(FOLLOWUPS_FILE_NAME).exists() {
        return None;
    }
    let task = match store.get(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(%task_id, %run_id, error = %e, "follow-ups: cannot read the task (ADR-0098)");
            return None;
        }
    };
    match store_run_holds_lease(store, &task, run_id) {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(%task_id, %run_id, status = ?task.status, "follow-ups: the run no longer holds the lease; not creating its follow-ups (ADR-0098 D1)");
            return None;
        }
        Err(e) => {
            tracing::warn!(%task_id, %run_id, error = %e, "follow-ups: cannot check the lease (ADR-0098)");
            return None;
        }
    }
    match task_ops::followup::absorb_followups_file(
        store,
        task_id,
        run_id,
        artifacts_dir,
        roles,
        genres,
        OffsetDateTime::now_utc(),
    ) {
        Ok(outcome) => {
            if let Some(o) = &outcome {
                tracing::info!(%task_id, %run_id, created = o.created.len(), notes = o.notes.len(), "follow-ups absorbed (ADR-0098)");
            }
            outcome
        }
        Err(e) => {
            tracing::warn!(%task_id, %run_id, error = %e, "follow-ups: failed to absorb (ADR-0098)");
            None
        }
    }
}
