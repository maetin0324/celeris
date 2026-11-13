//! ADR 2026-11-12-knowledge-usage-per-run D2・D3: worker run の中で KB の操作の入口が台帳に書いた回数を、
//! run の後に daemon が `WorkerFinished.metrics.knowledge`（`runs.metrics_json`）に入れる。
//!
//! 守る不変条件: 数は daemon が渡した台帳から読み、harness の log を解析しない。既存の metrics の試験は
//! 壁時計と試行回数しか見ないので、台帳の配線（env の受け渡し・run 後の読み取り）を検出できない。

use super::*;

/// `configure` で受け取った env の台帳に、入口と同じ関数で search 2 回・get 1 回を書いて done になる。
struct KnowledgeUsingAdapter {
    env: Vec<(String, String)>,
}

#[async_trait]
impl WorkerAdapter for KnowledgeUsingAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    fn configure(&self, settings: &task_worker::AdapterSettings) -> task_worker::Configured {
        settings.configured(&[task_worker::AdapterSetting::Env], || {
            let mut env = self.env.clone();
            settings.apply_env(&mut env);
            Arc::new(KnowledgeUsingAdapter { env })
        })
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let Some((_, path)) = self
            .env
            .iter()
            .rev()
            .find(|(k, _)| k == task_core::knowledge::usage::ENV_USAGE_FILE)
        else {
            return Err(AdapterError::Other(
                "no CELERIS_KNOWLEDGE_USAGE_FILE".into(),
            ));
        };
        let path = std::path::Path::new(path);
        for op in [
            task_core::KnowledgeOp::Search,
            task_core::KnowledgeOp::Search,
            task_core::KnowledgeOp::Get,
        ] {
            task_core::knowledge::usage::append(path, op, true)
                .map_err(|e| AdapterError::Other(e.to_string()))?;
        }
        Ok(done_outcome())
    }
}

#[tokio::test]
async fn knowledge_ops_counted_at_the_entry_are_stored_on_the_worker_run_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    // 前の run の台帳が残っていても、今回の run の数に混ぜない。
    let artifacts = task_core::artifacts::artifacts_dir_for(&task, dir.path());
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(
        artifacts.join(task_core::knowledge::usage::USAGE_FILE_NAME),
        "{\"op\":\"record\",\"ok\":true}\n",
    )
    .unwrap();
    let adapter = Arc::new(KnowledgeUsingAdapter { env: Vec::new() });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.tick().unwrap();
    await_worker_completion(&mut d, task.id).await;
    assert_eq!(d.tick().unwrap().finished, 1);

    let metrics = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished { metrics, .. } => metrics,
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:?}", store.events_for(task.id).unwrap()));
    assert_eq!(
        metrics.knowledge,
        Some(task_core::KnowledgeUse {
            search: 2,
            get: 1,
            record: 0,
            failed: 0,
        })
    );
}
