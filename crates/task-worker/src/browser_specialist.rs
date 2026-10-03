//! Browser-only identity for an existing harness. The browser supervisor enforces the
//! capability and policy; this adapter preserves the underlying harness configuration.
use std::sync::Arc;

use async_trait::async_trait;

use crate::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};

pub struct BrowserSpecialistAdapter {
    inner: Arc<dyn WorkerAdapter>,
}

impl BrowserSpecialistAdapter {
    pub const ID: &'static str = "browser-specialist";

    pub fn new(inner: Arc<dyn WorkerAdapter>) -> Self {
        Self { inner }
    }

    fn wrap(inner: Option<Arc<dyn WorkerAdapter>>) -> Option<Arc<dyn WorkerAdapter>> {
        inner.map(|inner| Arc::new(Self::new(inner)) as Arc<dyn WorkerAdapter>)
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

    fn model_for_tier(&self, tier: task_core::Tier) -> Result<Option<String>, String> {
        self.inner.model_for_tier(tier)
    }

    fn reasoning_effort_for_tier(&self, tier: task_core::Tier) -> Option<String> {
        self.inner.reasoning_effort_for_tier(tier)
    }

    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Self::wrap(self.inner.with_model(model))
    }

    fn with_permission_mode(&self, mode: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Self::wrap(self.inner.with_permission_mode(mode))
    }

    fn supports_reasoning_effort(&self) -> bool {
        self.inner.supports_reasoning_effort()
    }

    fn with_reasoning_effort(&self, effort: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Self::wrap(self.inner.with_reasoning_effort(effort))
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        Self::wrap(self.inner.with_env(extra))
    }

    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        Self::wrap(self.inner.with_container(plan))
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
