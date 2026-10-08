//! Shared startup/reload resolution of CoS launch settings.
use std::net::SocketAddr;
use std::path::Path;

use crate::Config;
use task_dispatch::dispatcher::cos_chat::launch::CosChatLaunchConfig;

/// The DB and API listener belong to the running daemon and do not change on reload.
pub(super) fn build_cos_chat_launch(
    config: &Config,
    db_path: &Path,
    api_listen: Option<SocketAddr>,
) -> CosChatLaunchConfig {
    let resolved = config.resolve_cos_provider();
    let (provider, llm_source, account_id, model, mut unavailable_reason) = match resolved {
        Ok(provider) => (
            Some(provider.provider),
            Some(cos_source_name(&provider.llm_source)),
            provider.account_id,
            provider.model,
            None,
        ),
        Err(reason) => (None, None, None, None, Some(reason)),
    };
    let api_base_url = api_listen.map(|addr| match addr {
        std::net::SocketAddr::V4(_) => format!("http://127.0.0.1:{}/api/v1", addr.port()),
        std::net::SocketAddr::V6(_) => format!("http://[::1]:{}/api/v1", addr.port()),
    });
    if api_base_url.is_none() && unavailable_reason.is_none() {
        unavailable_reason = Some("CoS unavailable: [api] listen is not configured".into());
    }
    CosChatLaunchConfig {
        enabled: config.cos.enabled,
        fallbacks: config
            .resolve_cos_fallbacks()
            .into_iter()
            .zip(&config.cos.fallbacks)
            .map(|(resolved, route)| {
                let (provider, source, account, model, unavailable_reason) = match resolved {
                    Ok(r) => (
                        Some(r.provider),
                        Some(cos_source_name(&r.llm_source)),
                        r.account_id,
                        r.model,
                        None,
                    ),
                    Err(reason) => (None, None, None, None, Some(reason)),
                };
                task_dispatch::dispatcher::cos_chat::launch::CosChatRoute {
                    harness: route.harness.adapter().to_owned(),
                    provider,
                    llm_source: source,
                    account_id: account,
                    model,
                    tier: route.tier,
                    unavailable_reason: if api_base_url.is_none() {
                        Some("CoS unavailable: [api] listen is not configured".into())
                    } else {
                        unavailable_reason
                    },
                }
            })
            .collect(),
        worker_reserve_five_hour: config.cos.worker_reserve_five_hour,
        harness: config.cos.harness.adapter().to_owned(),
        llm_source,
        provider,
        account_id,
        model,
        tier: config.cos.tier,
        max_turns: config.cos.max_turns,
        max_wall_secs: config.cos.max_wall_secs,
        unavailable_reason,
        data_dir: db_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf(),
        db_path: db_path.to_path_buf(),
        attachment_limits: config.cos.attachments.limits(),
        api_base_url: api_base_url.unwrap_or_default(),
        triage: task_dispatch::dispatcher::cos_chat::triage::CosTriageSettings {
            policy_skill: config.cos.triage.policy_skill.clone(),
            policy_version: config.cos.triage.policy_version.clone(),
            min_confidence: config.cos.triage.min_confidence,
            human_required: config.cos.triage.human_required.clone(),
            unavailable_after_secs: config.cos.triage.unavailable_after_secs,
        },
    }
}

// Preserve the named endpoint in both the run record and the session identity.
fn cos_source_name(source: &task_core::LlmSourceRef) -> String {
    match source {
        task_core::LlmSourceRef::OpenaiCompatible(id) => format!("openai_compatible:{id}"),
        other => other.as_str().to_owned(),
    }
}
