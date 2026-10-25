//! PBS recovery の別 process controller（子）と、親の独立した保存の読み直し（ADR 2026-10-25-recovery-scheduler-backend
//! 付記 I14 (3)）。Celeris arm は dev-dependency の task-api router が要るので test 側に置く。
//! 使う側の crate root に `RouterTransport` と `router_at` があること（`dagbench_harness.rs`）。
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use celeris_campaign::dagbench::{
    celeris_side::{CelerisSide, Paths, RunSpec},
    pbs::{
        backend::Arm,
        entry::{Controller, GatedExt, SavedReader},
        proc::{SavedWait, read_saved_state_json},
    },
};
use serde_json::{Value, json};
use task_core::{TaskStore, cluster_job::ClusterJobWaitStore};

/// Celeris arm の測定専用 controller（子 process の中だけで走る。停止は親の SIGKILL）。
pub struct CelerisChildController {
    pub spec: RunSpec,
    pub dir: PathBuf,
}

impl Controller for CelerisChildController {
    fn run(&mut self, ext: GatedExt) -> Result<bool, String> {
        let paths = Paths::under(&self.dir.join("celeris.db"), &self.dir);
        let store: Arc<dyn TaskStore> =
            Arc::new(task_core::SqliteStore::open(&paths.db).map_err(|e| e.to_string())?);
        let transport = crate::RouterTransport(crate::router_at(&self.dir));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let spec = self.spec.clone();
        rt.block_on(async move {
            let mut side = CelerisSide::with_backend(spec, paths, store, Box::new(ext))?;
            let r = side.drive(&transport).await?;
            Ok(r.campaigns.iter().all(|c| c.status == "done"))
        })
    }

    fn saved_reader(&self) -> SavedReader {
        let db = self.dir.join("celeris.db");
        Box::new(move |physical| {
            let store =
                task_core::SqliteStore::open(&db).map_err(|e| io::Error::other(e.to_string()))?;
            let waits = store
                .cluster_job_waits_waiting()
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(waits
                .into_iter()
                .find(|w| w.jobs.iter().any(|j| j == physical))
                .map(|w| json!({"wait_id": w.wait_id, "job_id": physical, "source": "TaskStore"})))
        })
    }
}

/// Celeris の保存を子と別の read-only 接続で読む（`SqliteStore::open` の書込み接続・migration を使わない）。
pub fn read_saved_sqlite_ro(trial_dir: &Path, job_id: &str) -> io::Result<Option<SavedWait>> {
    let db = trial_dir.join("celeris.db");
    if !db.exists() {
        return Ok(None);
    }
    let conn =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(io::Error::other)?;
    let mut stmt = conn
        .prepare("SELECT wait_id, jobs_json, state FROM cluster_job_waits WHERE state = 'waiting'")
        .map_err(io::Error::other)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(io::Error::other)?;
    for row in rows {
        let (wait_id, jobs_json, state) = row.map_err(io::Error::other)?;
        let jobs: Value = serde_json::from_str(&jobs_json).map_err(io::Error::other)?;
        if jobs
            .as_array()
            .is_some_and(|a| a.iter().any(|j| j == job_id))
        {
            return Ok(Some(SavedWait {
                source: "sqlite_ro".into(),
                wait_id: Some(wait_id.clone()),
                job_id: job_id.into(),
                snapshot: json!({"wait_id": wait_id, "jobs": jobs, "state": state}),
            }));
        }
    }
    Ok(None)
}

/// arm ごとの親の読み直し。
pub fn read_saved(arm: Arm, trial_dir: &Path, job_id: &str) -> io::Result<Option<SavedWait>> {
    match arm {
        Arm::Baseline => read_saved_state_json(trial_dir, job_id),
        Arm::Celeris => read_saved_sqlite_ro(trial_dir, job_id),
    }
}
