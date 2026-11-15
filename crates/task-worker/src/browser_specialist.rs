//! Browser-only identity for an existing harness. The browser supervisor enforces the
//! capability and policy; this adapter preserves the underlying harness configuration.
use std::sync::Arc;

use async_trait::async_trait;

use crate::adapter::{AdapterSettings, Configured};
use crate::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};

pub struct BrowserSpecialistAdapter {
    inner: Arc<dyn WorkerAdapter>,
}

impl BrowserSpecialistAdapter {
    pub const ID: &'static str = "browser-specialist";

    pub fn new(inner: Arc<dyn WorkerAdapter>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl WorkerAdapter for BrowserSpecialistAdapter {
    fn id(&self) -> &str {
        Self::ID
    }

    fn account_id(&self) -> Option<&str> {
        self.inner.account_id()
    }

    /// ADR 2026-11-10-worker-capability-ports D3: 内側の harness の `configure` へ転送し、複製を包み直す
    /// （id は `browser-specialist` のまま）。`unsupported` は内側の報告をそのまま返す。
    fn configure(&self, settings: &AdapterSettings) -> Configured {
        self.inner
            .configure(settings)
            .map_adapter(|inner| Arc::new(Self::new(inner)))
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        if !task_core::browser::requests_browser(&req.task.skills) {
            return Err(AdapterError::Other(
                "browser-specialist requires browser capability".into(),
            ));
        }
        self.inner.run(req, run_id, limits, sink).await
    }
}
