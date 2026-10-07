//! Resolve an explicit tier binding before starting a CLI. Never substitute another model.
use crate::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};
use async_trait::async_trait;
use std::sync::Arc;
use task_core::{
    Tier,
    model_routing::{TierModels, resolve},
};

pub struct TieredAdapter {
    pub base: Arc<dyn WorkerAdapter>,
    pub models: TierModels,
    pub account_id: Option<String>,
    pub credential_error: Option<String>,
}
#[async_trait]
impl WorkerAdapter for TieredAdapter {
    fn id(&self) -> &str {
        self.base.id()
    }
    fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }
    fn model_for_tier(&self, tier: Tier) -> Result<Option<String>, String> {
        if let Some(reason) = &self.credential_error {
            return Err(reason.clone());
        }
        resolve(&self.models, tier)
    }
    fn reasoning_effort_for_tier(&self, tier: Tier) -> Option<String> {
        task_core::model_routing::reasoning_effort(&self.models, tier)
    }
    /// ADR-0069 Phase 118 D1: 実際に CLI へ effort を渡せるかは基盤アダプタ次第（codex は対応、
    /// claude-code は既定のまま非対応）。
    fn supports_reasoning_effort(&self) -> bool {
        self.base.supports_reasoning_effort()
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let tier = req.task.worker_hint.tier;
        let model = self.model_for_tier(tier).map_err(AdapterError::Other)?;
        let mut base = match model {
            Some(model) => self
                .base
                .with_model(&model)
                .ok_or_else(|| AdapterError::Other("adapter cannot apply tier model".into()))?,
            None => self.base.clone(),
        };
        // ADR-0069 Phase 118 D1: 対応するアダプタ（codex）にだけ、実際に reasoning effort を渡す。
        // 対応しないアダプタ（claude-code）では黙って素通しする（監査記録は別の層で「渡らなかった」
        // ことを区別する）。
        if let Some(effort) = self.reasoning_effort_for_tier(tier)
            && let Some(wrapped) = base.with_reasoning_effort(&effort)
        {
            base = wrapped;
        }
        base.run(req, run_id, limits, sink).await
    }
    /// ADR 2026-10-05 cos-chat-home D2: CoS run は config の model（または tier から解決した model）を
    /// 明示で固定する。基盤アダプタに model を載せ、tier の束縛は空にする（run で別 model に置き換えない）。
    /// account・credential の状態はそのまま保つ。基盤が model を受けられなければ None。
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            base: self.base.with_model(model)?,
            models: TierModels::new(),
            account_id: self.account_id.clone(),
            credential_error: self.credential_error.clone(),
        }))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            base: self.base.with_env(extra)?,
            models: self.models.clone(),
            account_id: self.account_id.clone(),
            credential_error: self.credential_error.clone(),
        }))
    }
    /// ADR 2026-10-06 model-role-assignments D2: `models` だけを差し替えた複製（基盤アダプタ・account は同じ）。
    fn with_tier_models(&self, models: TierModels) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            base: self.base.clone(),
            models,
            account_id: self.account_id.clone(),
            credential_error: self.credential_error.clone(),
        }))
    }
    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            base: self.base.with_container(plan)?,
            models: self.models.clone(),
            account_id: self.account_id.clone(),
            credential_error: self.credential_error.clone(),
        }))
    }
    /// ADR-0072 D14（Phase E4b 項目3）: `self.adapters` に登録された実体は `TieredAdapter` で
    /// 包まれている（`with_model` と同じ理由）ので、基盤アダプタへそのまま中継する。
    fn with_permission_mode(&self, mode: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            base: self.base.with_permission_mode(mode)?,
            models: self.models.clone(),
            account_id: self.account_id.clone(),
            credential_error: self.credential_error.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeAdapter;
    use task_core::model_routing::ModelBinding;

    fn binding(model: &str) -> ModelBinding {
        ModelBinding {
            name: model.into(),
            model_id: Some(model.into()),
            unavailable_reason: None,
            reasoning_effort: None,
        }
    }

    /// CoS の明示 model: `with_model` は基盤に model を載せ、tier の束縛を空にした複製を返す。
    #[test]
    fn with_model_pins_the_base_model_and_clears_tier_bindings() {
        let mut models = TierModels::new();
        models.insert(Tier::Frontier, binding("claude-fable-5-1"));
        let tiered = TieredAdapter {
            base: Arc::new(FakeAdapter::default()),
            models,
            account_id: Some("acct".into()),
            credential_error: None,
        };
        match FakeAdapter::default().with_model("m") {
            Some(_) => {
                let pinned = tiered.with_model("m").expect("base accepts a model");
                assert_eq!(pinned.account_id(), Some("acct"));
                assert_eq!(pinned.model_for_tier(Tier::Frontier).unwrap(), None);
            }
            None => assert!(tiered.with_model("m").is_none()),
        }
    }

    /// ADR 2026-10-06 model-role-assignments D2: `with_tier_models` は `models` だけを差し替えた複製を返し、
    /// 元のアダプタは変わらない。
    #[test]
    fn with_tier_models_replaces_only_the_bindings() {
        let mut old = TierModels::new();
        old.insert(Tier::Cheap, binding("old"));
        let adapter = TieredAdapter {
            base: Arc::new(FakeAdapter::new(vec![])),
            models: old,
            account_id: Some("acct".into()),
            credential_error: None,
        };
        let mut new = TierModels::new();
        new.insert(Tier::Cheap, binding("new"));
        let replaced = adapter.with_tier_models(new).expect("tiered supports it");
        assert_eq!(replaced.id(), "fake");
        assert_eq!(replaced.account_id(), Some("acct"));
        assert_eq!(
            replaced.model_for_tier(Tier::Cheap),
            Ok(Some("new".to_string()))
        );
        assert_eq!(
            adapter.model_for_tier(Tier::Cheap),
            Ok(Some("old".to_string()))
        );
        // 束縛を持たない基盤アダプタの既定は None。
        assert!(
            FakeAdapter::new(vec![])
                .with_tier_models(TierModels::new())
                .is_none()
        );
    }
}
