//! Real HTTP reload, isolated SQLite and in-process adapters. No LLM processes or external IO.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::{InstanceRole, SqliteStore};
use task_dispatch::{Dispatcher, StaticPolicy};
use task_worker::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};
use time::OffsetDateTime;

use super::{admin::reload_providers, api::api_settings};
use crate::{Config, DaemonMode, SharedRole};

type Seen = (String, RunRequest, String, Vec<(String, String)>);

#[derive(Clone)]
struct HeldFake {
    harness: String,
    model: String,
    env: Vec<(String, String)>,
    seen: tokio::sync::mpsc::UnboundedSender<Seen>,
}

#[async_trait]
impl WorkerAdapter for HeldFake {
    fn id(&self) -> &str {
        &self.harness
    }
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            model: model.into(),
            ..self.clone()
        }))
    }
    fn with_env(&self, env: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            env: env.to_vec(),
            ..self.clone()
        }))
    }
    async fn run(
        &self,
        req: RunRequest,
        id: &str,
        _: RunLimits,
        _: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen
            .send((id.into(), req, self.model.clone(), self.env.clone()))
            .unwrap();
        // Keep the run in flight across reload; the test runtime cancels this future on exit.
        std::future::pending().await
    }
}

fn install_fakes(
    d: &mut Dispatcher,
    config: &Config,
    seen: &tokio::sync::mpsc::UnboundedSender<Seen>,
) {
    let adapters: HashMap<_, Arc<dyn WorkerAdapter>> = config
        .providers
        .iter()
        .map(|p| {
            (
                p.id.clone(),
                Arc::new(HeldFake {
                    harness: p.adapter.clone(),
                    model: String::new(),
                    env: vec![],
                    seen: seen.clone(),
                }) as Arc<dyn WorkerAdapter>,
            )
        })
        .collect();
    d.reload_providers(
        Box::new(StaticPolicy::new(
            config.provider_specs(),
            Duration::from_secs(1),
        )),
        HashMap::new(),
        adapters,
        Default::default(),
    );
}

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
[[providers]]
id = "claude"
adapter = "claude-code"
concurrency = 8
tiers = ["frontier", "standard"]
[providers.tier_models.frontier]
name = "fable"
model_id = "old-fable"
[providers.tier_models.standard]
name = "opus"
model_id = "new-opus"
[[providers]]
id = "codex"
adapter = "codex"
concurrency = 8
tiers = ["frontier", "standard"]
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
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: key.into(),
                project_id: None,
                client_thread_id: key.into(),
            },
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
async fn cos_reload_http_updates_next_run_and_rejects_restart_only_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = write_config(dir.path(), "tier = \"frontier\"");
    for skill in ["cos-operator", "cos-inbox-triage"] {
        let path = config.knowledge.root.join("skills").join(skill);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {skill}\ndescription: test\n---\n# Test\n"),
        )
        .unwrap();
    }
    let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();
    let (seen_tx, mut seen) = tokio::sync::mpsc::unbounded_channel();
    install_fakes(&mut dispatcher, &config, &seen_tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (admin_tx, mut admin_rx) = tokio::sync::mpsc::channel(4);
    let settings = api_settings(
        &config,
        addr,
        Some("reload-test".into()),
        "test".into(),
        "now".into(),
        Some(admin_tx),
        "sha".into(),
        DaemonMode::Normal,
        SharedRole::new(InstanceRole::Active),
        None,
    );
    let (_tx, rx) = tokio::sync::watch::channel(None);
    let state = task_api::ApiState::new(settings, rx).unwrap();
    let (stop, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(task_api::serve_with_listener(listener, state, async {
        let _ = stop_rx.await;
    }));
    let url = format!("http://{addr}/api/v1/reload");

    let first = launch(&mut dispatcher, &store, &mut seen, "before").await;
    let old = resolved(&config, &first.0);
    assert_eq!(old["tier"], "frontier");
    assert_eq!(old["model"], "old-fable");

    // The production failure: changing only tier must resolve the new tier's model.
    let fresh = write_config(dir.path(), "tier = \"standard\"");
    assert_eq!(
        reload(&url, &mut admin_rx, &mut dispatcher, &mut config)
            .await
            .status(),
        200
    );
    install_fakes(&mut dispatcher, &fresh, &seen_tx);
    let second = launch(&mut dispatcher, &store, &mut seen, "tier").await;
    assert_eq!(resolved(&config, &second.0)["tier"], "standard");
    assert_eq!(second.2, "new-opus");
    assert_eq!(resolved(&config, &second.0)["model"], "new-opus");

    let fresh = write_config(
        dir.path(),
        "harness = \"codex\"\nprovider = \"codex\"\nllm_source = \"codex_oauth\"\ntier = \"standard\"\nmodel = \"new-model\"\nmax_turns = 13\nmax_wall_secs = 99\n[[cos.fallbacks]]\nharness = \"claude-code\"\nmodel = \"fallback-model\"\ntier = \"standard\"",
    );
    assert_eq!(
        reload(&url, &mut admin_rx, &mut dispatcher, &mut config)
            .await
            .status(),
        200
    );
    install_fakes(&mut dispatcher, &fresh, &seen_tx);
    let third = launch(&mut dispatcher, &store, &mut seen, "harness").await;
    let changed = resolved(&config, &third.0);
    assert_eq!(changed["harness"], "codex");
    assert_eq!(changed["llm_source"], "codex_oauth");
    assert_eq!(changed["model"], "new-model");
    assert_eq!(third.1.task.budget.max_turns, 13);
    assert_eq!(third.1.task.budget.max_wall_secs, 99);
    assert!(
        third
            .3
            .iter()
            .any(|(k, v)| k == "CELERIS_API_URL" && v == "http://127.0.0.1:7700/api/v1")
    );

    // No opencode provider: the newly configured fallback must actually launch.
    let fresh = write_config(
        dir.path(),
        "harness = \"opencode\"\n[[cos.fallbacks]]\nharness = \"claude-code\"\nmodel = \"new-fallback\"\ntier = \"standard\"",
    );
    assert_eq!(
        reload(&url, &mut admin_rx, &mut dispatcher, &mut config)
            .await
            .status(),
        200
    );
    install_fakes(&mut dispatcher, &fresh, &seen_tx);
    let fourth = launch(&mut dispatcher, &store, &mut seen, "fallback").await;
    let fallback = resolved(&config, &fourth.0);
    assert_eq!(fallback["harness"], "claude-code");
    assert_eq!(fallback["model"], "new-fallback");
    assert_eq!(fallback["tier"], "standard");
    assert_eq!(fourth.2, "new-fallback");
    assert_eq!(
        resolved(&config, &first.0),
        old,
        "in-flight run remains unchanged"
    );

    let first_thread = &first.1.context.cos_chat.as_ref().unwrap().thread_id;
    assert_eq!(
        store.chat_run_get(first_thread, &first.0).unwrap().state,
        task_core::chat::ChatRunState::Running
    );

    for setting in [
        "stream_retention_days = 7",
        "[cos.attachments]\nmax_file_bytes = 1024",
        "[cos.attachments]\nmax_message_bytes = 1048576000",
        "[cos.attachments]\nmax_storage_bytes = 21474836480",
        "[cos.attachments]\nmax_files_per_message = 3",
        "[cos.attachments]\norphan_ttl_hours = 2",
        "[cos.attachments]\nunreferenced_retention_days = 5",
    ] {
        write_config(dir.path(), &format!("tier = \"standard\"\n{setting}"));
        let response = reload(&url, &mut admin_rx, &mut dispatcher, &mut config).await;
        assert_eq!(response.status(), 400, "{setting}");
        let body = response.text().await.unwrap();
        assert!(body.contains("restart celeris"), "{body}");
        assert_eq!(config.cos.harness, crate::config::CosHarness::Opencode);
    }
    let after_rejection = launch(&mut dispatcher, &store, &mut seen, "rejected").await;
    assert_eq!(
        resolved(&config, &after_rejection.0)["model"],
        "new-fallback"
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[test]
fn cos_reload_builder_resolves_accounts_triage_and_running_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = write_config(dir.path(), "");
    // Both route-local and provider-fixed accounts go through the same resolution as startup.
    config.providers[0].account_pool = task_core::AccountPoolSetting::On;
    config.providers[0].account_id = Some("provider-account".into());
    config.cos.worker_reserve_five_hour = 0.85;
    config.cos.triage.policy_skill = "updated-triage".into();
    config.cos.triage.policy_version = "2".into();
    config.cos.triage.min_confidence = 0.91;
    config.cos.triage.human_required = vec!["explicit_human".into()];
    config.cos.triage.unavailable_after_secs = 42;
    config.cos.fallbacks = vec![crate::config::CosRouteConfig {
        harness: crate::config::CosHarness::ClaudeCode,
        llm_source: None,
        provider: Some("claude".into()),
        account_id: Some("provider-account".into()),
        model: Some("fallback".into()),
        tier: task_core::Tier::Standard,
    }];
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
    assert_eq!(built.account_id.as_deref(), Some("provider-account"));
    assert_eq!(
        built.fallbacks[0].account_id.as_deref(),
        Some("provider-account")
    );
    assert_eq!(built.worker_reserve_five_hour, 0.85);
    assert_eq!(built.triage.policy_skill, "updated-triage");
    assert_eq!(built.triage.policy_version, "2");
    assert_eq!(built.triage.min_confidence, 0.91);
    assert_eq!(built.triage.human_required, ["explicit_human"]);
    assert_eq!(built.triage.unavailable_after_secs, 42);
    let no_api = super::cos_launch::build_cos_chat_launch(&config, &original_db, None);
    assert!(no_api.unavailable_reason.unwrap().contains("listen"));
    assert!(
        no_api.fallbacks[0]
            .unavailable_reason
            .as_ref()
            .unwrap()
            .contains("listen")
    );
}
