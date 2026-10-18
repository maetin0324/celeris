//! 測定 daemon の `GET /config` 応答（正式 DTO `task_api::ConfigView`）を測定 config から作る共有部品。
#![allow(dead_code)]
#[path = "../../../task-api/tests/common/mod.rs"]
pub mod api_common;
use std::{fs, path::Path};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::preflight_setup::{MEASURE_RELEASE, current_config};
/// 接続先として一致する daemon の `GET /config` 本文（`write_configs` が最後に書いた測定 config 由来）。
pub fn matching_daemon_body() -> Value {
    daemon_config_body(&current_config(), MEASURE_RELEASE)
}

/// 測定 config で起動した daemon が返す `GET /config` の本文（正式 DTO `task_api::ConfigView` を serialize した値）。
/// 値は daemon の `identity_view` と同じ規則（config の bytes の sha256・path）で作る。
pub fn daemon_config_view(config: &Path, release: &str) -> task_api::ConfigView {
    let t: toml::Value = toml::from_str(&fs::read_to_string(config).unwrap()).unwrap();
    let s = |keys: &[&str]| {
        keys.iter()
            .try_fold(&t, |c, k| c.get(k))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let mut v = api_common::config_view();
    v.config_path = config.display().to_string();
    v.identity.release = release.into();
    v.identity.pid = super::preflight_setup::DAEMON_PID;
    v.identity.config_sha256 = Some(format!("{:x}", Sha256::digest(fs::read(config).unwrap())));
    v.identity.instance_id = "01MEASUREDAEMON00000000000".into();
    let e = &mut v.identity.effective;
    e.db = s(&["db"]);
    e.workspace_root = s(&["workspace_root"]);
    e.knowledge_root = s(&["knowledge", "root"]);
    e.knowledge_default_mounts = vec!["kb:trial".into()];
    e.memory_dir = None;
    let home = std::env::var("HOME").unwrap_or_default();
    e.selfdeploy_releases_dir = format!("{home}/.local/celeris/releases");
    e.selfdeploy_repo = format!("{home}/workspace/agent-platform");
    e.selfdeploy_push = true;
    e.api_listen = s(&["api", "listen"]);
    v
}

pub fn daemon_config_body(config: &Path, release: &str) -> Value {
    serde_json::to_value(daemon_config_view(config, release)).unwrap()
}
