//! ADR 2026-10-27-unblocks-reopen-on-promotion D2: 修正 task を含む release の昇格を周期的に見つけ、
//! `unblocks` で結んだ待っている task を 1 度だけ reopen する（`failed`）かコメントを残す（`blocked`）。
//! 判定は `task_ops::unblocks`（決定的。JSON と DB だけ）。間隔と単発実行は `stranded_sweep` と同じ。

use std::path::PathBuf;
use std::sync::Arc;

use task_core::{InstanceRole, SharedRole, TaskStore};
use time::OffsetDateTime;

use super::stranded_sweep::{INTERVAL, SingleFlight, periodic};

pub(crate) fn run(
    store: &dyn TaskStore,
    releases_dir: &std::path::Path,
    now: OffsetDateTime,
) -> Result<Vec<task_ops::unblocks::UnblockOutcome>, String> {
    // 未発火の結びが無ければ releases_dir を読まない。
    if store
        .unblock_pending()
        .map_err(|e| e.to_string())?
        .is_empty()
    {
        return Ok(Vec::new());
    }
    let releases = task_ops::unblocks::read_promoted_releases(releases_dir);
    task_ops::unblocks::sweep(store, &releases, now).map_err(|e| e.to_string())
}

pub(crate) fn spawn(
    store: Arc<dyn TaskStore>,
    role: SharedRole,
    releases_dir: PathBuf,
) -> tokio::task::JoinHandle<()> {
    let flight = Arc::new(SingleFlight::default());
    tokio::spawn(periodic(INTERVAL, move || {
        if role.get() != InstanceRole::Active {
            return;
        }
        let Ok(guard) = flight.try_begin() else {
            return;
        };
        let store = Arc::clone(&store);
        let releases_dir = releases_dir.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            match run(store.as_ref(), &releases_dir, OffsetDateTime::now_utc()) {
                Ok(fired) => {
                    for o in fired {
                        tracing::info!(
                            fix = %o.fix_task_id,
                            waiting = %o.waiting_task_id,
                            release = %o.release,
                            action = o.action.as_str(),
                            "unblocks: a release with the fix was promoted"
                        );
                    }
                }
                Err(error) => tracing::warn!(%error, "unblocks sweep failed"),
            }
        });
    }))
}
