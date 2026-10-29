//! Real HTTP reload, isolated SQLite and in-process adapters. No LLM processes or external IO.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use task_core::chat::{ChatOpenThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::{InstanceRole, SqliteStore};
use task_dispatch::Dispatcher;
use task_worker::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};
use time::OffsetDateTime;

use super::{admin::reload_providers, api::api_settings};
use crate::{Config, DaemonMode, SharedRole};

type Seen = (String, RunRequest, String, Vec<(String, String)>);

fn write_config(dir: &std::path::Path, cos: &str) -> Config {
    let path = dir.join("config.toml");
    let db = dir.join("test.db");
    let ws = dir.join("ws");
    let kb = dir.join("kb");
    std::fs::write(
        &path,
        format!(
            r#"
db = {db:?}
workspace_root = {ws:?}
[dispatch]
min_free_disk_mb = 0
[knowledge]
root = {kb:?}
[execution]
max_cos_runs = 8
[api]
listen = "127.0.0.1:7700"
# The default watch reads the host's `/`, `/local` and `/tmp`; a full host disk would raise a
# `disk_full` inbox item and start an extra triage run in the inbox thread.
[maintenance]
disk_watch = []
[cos]
{cos}
"#
        ),
    )
    .unwrap();
    Config::load(&path).unwrap()
}

async fn launch(
    d: &mut Dispatcher,
    store: &SqliteStore,
    seen: &mut tokio::sync::mpsc::UnboundedReceiver<Seen>,
    key: &str,
) -> Seen {
    let now = OffsetDateTime::now_utc();
    let thread = store
        .cos_thread_open(
&ChatOpenThreadRequest { project_id: None },
now,
        )
        .unwrap()
        .thread
        .id;
    store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: key.into(),
                text: key.into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .unwrap();
    d.tick().unwrap();
    // Only this thread's run: another run (e.g. inbox triage) started by the same tick is skipped.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let started = seen.recv().await.unwrap();
            if store
                .chat_run_get(&thread, &started.0)
                .is_ok_and(|run| run.thread_id == thread)
            {
                return started;
            }
        }
    })
    .await
    .expect("fake started")
}

fn resolved(config: &Config, run: &str) -> serde_json::Value {
    let db = rusqlite::Connection::open(&config.db.path).unwrap();
    let raw: String = db
        .query_row(
            "SELECT resolved_config_json FROM chat_runs WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

async fn reload(
    url: &str,
    rx: &mut tokio::sync::mpsc::Receiver<task_api::AdminRequest>,
    d: &mut Dispatcher,
    config: &mut Config,
) -> reqwest::Response {
    let call = reqwest::Client::new()
        .post(url)
        .bearer_auth("reload-test")
        .json(&serde_json::json!({}))
        .send();
    let handle = async {
        let task_api::AdminRequest::Reload { reply } = rx.recv().await.unwrap() else {
            panic!("reload request")
        };
        reply.send(reload_providers(d, config)).unwrap();
    };
    let (response, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(call, handle)
    })
    .await
    .unwrap();
    response.unwrap()
}

#[tokio::test]
async fn cos_reload_keeps_future_launches_on_fixed_harness() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), "max_turns = 13");
    let launch = super::cos_launch::build_cos_chat_launch(
        &config,
        &config.db.path,
        Some("127.0.0.1:7700".parse().unwrap()),
    );
    assert_eq!(launch.harness, "claude-code");
    assert_eq!(launch.max_turns, 13);
}

#[test]
fn cos_reload_builder_resolves_accounts_triage_and_running_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = write_config(dir.path(), "");
    config.cos.worker_reserve_five_hour = 0.85;
    config.cos.triage.policy_skill = "updated-triage".into();
    config.cos.triage.policy_version = "2".into();
    config.cos.triage.min_confidence = 0.91;
    config.cos.triage.human_required = vec!["explicit_human".into()];
    config.cos.triage.unavailable_after_secs = 42;
    config.cos.triage.authorization_unavailable_after_secs = Some(4242);
    let original_db = config.db.path.clone();
    config.db.path = dir.path().join("not-running.db");
    config.api.listen = None;
    let built = super::cos_launch::build_cos_chat_launch(
        &config,
        &original_db,
        Some("[::1]:12345".parse().unwrap()),
    );
    assert_eq!(built.db_path, original_db);
    assert_eq!(built.data_dir, dir.path());
    assert_eq!(built.api_base_url, "http://[::1]:12345/api/v1");
    assert_eq!(built.worker_reserve_five_hour, 0.85);
    assert_eq!(built.triage.policy_skill, "updated-triage");
    assert_eq!(built.triage.policy_version, "2");
    assert_eq!(built.triage.min_confidence, 0.91);
    assert_eq!(built.triage.human_required, ["explicit_human"]);
    assert_eq!(built.triage.unavailable_after_secs, 42);
    assert_eq!(built.triage.authorization_unavailable_after_secs, 4242);
    let no_api = super::cos_launch::build_cos_chat_launch(&config, &original_db, None);
    assert!(no_api.unavailable_reason.unwrap().contains("listen"));
}
