//! CoS startup wiring. Route candidates are selected by task-dispatch at each launch.
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
    let api_base_url = api_listen.map(|addr| match addr {
        std::net::SocketAddr::V4(_) => format!("http://127.0.0.1:{}/api/v1", addr.port()),
        std::net::SocketAddr::V6(_) => format!("http://[::1]:{}/api/v1", addr.port()),
    });
    CosChatLaunchConfig {
        enabled: config.cos.enabled,
        worker_reserve_five_hour: config.cos.worker_reserve_five_hour,
        harness: "claude-code".to_owned(),
        max_turns: config.cos.max_turns,
        max_wall_secs: config.cos.max_wall_secs,
        unavailable_reason: api_base_url
            .is_none()
            .then(|| "CoS unavailable: [api] listen is not configured".to_owned()),
        data_dir: db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        db_path: db_path.to_path_buf(),
        attachment_limits: config.cos.attachments.limits(),
        api_base_url: api_base_url.unwrap_or_default(),
        web_base_url: config.notify.base_url().map(str::to_owned),
        triage: task_dispatch::dispatcher::cos_chat::triage::CosTriageSettings {
            policy_skill: config.cos.triage.policy_skill.clone(),
            policy_version: config.cos.triage.policy_version.clone(),
            min_confidence: config.cos.triage.min_confidence,
            human_required: config.cos.triage.human_required.clone(),
            unavailable_after_secs: config.cos.triage.unavailable_after_secs,
            authorization_unavailable_after_secs: config.cos.authorization_unavailable_after_secs(),
        },
    }
}
