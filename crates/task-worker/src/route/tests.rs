use super::*;
use crate::adapter::{AdapterError, EventSink, NullSink, RunLimits, RunOutcome, Terminal};
use crate::protocol::{LlmEndpointReference, PROTOCOL_VERSION, RunContext};
use crate::{PiAdapter, PiConfig};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// What a launched adapter was configured with when its `run` was called.
#[derive(Debug, Clone, Default, PartialEq)]
struct Seen {
    harness: String,
    model: Option<String>,
    env: Vec<(String, String)>,
    endpoint: Option<(String, Option<String>)>,
}

#[derive(Clone)]
struct Recorder {
    seen: Seen,
    takes_endpoint: bool,
    namespace: Option<String>,
    log: Arc<Mutex<Vec<Seen>>>,
}

impl Recorder {
    fn new(harness: &str, takes_endpoint: bool, log: &Arc<Mutex<Vec<Seen>>>) -> Self {
        Self {
            seen: Seen {
                harness: harness.into(),
                ..Seen::default()
            },
            takes_endpoint,
            namespace: (harness == "opencode").then(|| "qwen-local".to_string()),
            log: Arc::clone(log),
        }
    }
}

#[async_trait]
impl WorkerAdapter for Recorder {
    fn id(&self) -> &str {
        if self.seen.harness == "opencode" {
            "acp"
        } else {
            "recorder"
        }
    }
    fn model_namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.seen.model = Some(model.into());
        Some(Arc::new(next))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.seen.env.extend(extra.iter().cloned());
        Some(Arc::new(next))
    }
    fn with_llm_endpoint(
        &self,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Option<Arc<dyn WorkerAdapter>> {
        if !self.takes_endpoint {
            return None;
        }
        let mut next = self.clone();
        next.seen.endpoint = Some((base_url.into(), api_key.map(str::to_string)));
        Some(Arc::new(next))
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.log.lock().unwrap().push(self.seen.clone());
        Err(AdapterError::Other("recorded".into()))
    }
}

fn adapters(log: &Arc<Mutex<Vec<Seen>>>) -> HashMap<HarnessId, Arc<dyn WorkerAdapter>> {
    let mut out: HashMap<HarnessId, Arc<dyn WorkerAdapter>> = HashMap::new();
    out.insert(
        "claude".into(),
        Arc::new(Recorder::new("claude", false, log)),
    );
    out.insert("pi".into(), Arc::new(Recorder::new("pi", true, log)));
    out.insert(
        "opencode".into(),
        Arc::new(Recorder::new("opencode", true, log)),
    );
    out
}

fn route(harness: &str, model: &str) -> SelectedLlmRoute {
    SelectedLlmRoute {
        harness_id: harness.into(),
        source_id: "src".into(),
        model_id: model.into(),
        account_id: "acct".into(),
        endpoint: None,
        credential: None,
    }
}

fn request(dir: &Path, route: Option<SelectedLlmRoute>) -> RunRequest {
    RunRequest {
        protocol: PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        workspace: dir.to_path_buf(),
        artifacts_dir: dir.join("artifacts"),
        work_dir: None,
        context: RunContext::default(),
        cargo_target_dir: None,
        llm_route: route,
    }
}

fn limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(60),
        idle_timeout: Duration::from_secs(60),
        kill_grace: Duration::from_millis(200),
    }
}

fn no_secret(_: &str) -> Option<String> {
    None
}

/// The run is started by the named harness's adapter with exactly the route's model.
#[tokio::test]
async fn worker_route_runs_the_given_harness_with_the_given_model() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), Some(route("pi", "qwen/qwen3-coder")));
    let adapter = request_adapter(&adapters(&log), &req, &no_secret).unwrap();
    let _ = adapter.run(req, "r1", limits(), &NullSink).await;
    assert_eq!(
        log.lock().unwrap().as_slice(),
        &[Seen {
            harness: "pi".into(),
            model: Some("qwen/qwen3-coder".into()),
            ..Seen::default()
        }]
    );
}

#[tokio::test]
async fn opencode_model_id_uses_declared_namespace() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), Some(route("opencode", "qwen3.8-27b")));
    let adapter = request_adapter(&adapters(&log), &req, &no_secret).unwrap();
    let _ = adapter.run(req, "r-opencode", limits(), &NullSink).await;
    assert_eq!(
        log.lock().unwrap()[0].model.as_deref(),
        Some("qwen-local/qwen3.8-27b")
    );
}

#[tokio::test]
async fn opencode_without_namespace_keeps_route_model() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut map = adapters(&log);
    map.insert(
        "opencode".into(),
        Arc::new(Recorder {
            seen: Seen {
                harness: "opencode".into(),
                ..Seen::default()
            },
            takes_endpoint: true,
            namespace: None,
            log: Arc::clone(&log),
        }),
    );
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), Some(route("opencode", "qwen3.8-27b")));
    let adapter = request_adapter(&map, &req, &no_secret).unwrap();
    let _ = adapter
        .run(req, "r-opencode-unqualified", limits(), &NullSink)
        .await;
    assert_eq!(log.lock().unwrap()[0].model.as_deref(), Some("qwen3.8-27b"));
}

#[tokio::test]
async fn non_opencode_model_id_remains_route_model() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), Some(route("pi", "qwen3.8-27b")));
    let adapter = request_adapter(&adapters(&log), &req, &no_secret).unwrap();
    let _ = adapter.run(req, "r-pi", limits(), &NullSink).await;
    assert_eq!(log.lock().unwrap()[0].model.as_deref(), Some("qwen3.8-27b"));
}

#[test]
fn worker_route_unknown_harness_fails_closed() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let err = route_adapter(&adapters(&log), &route("aider", "m"), &no_secret).err();
    assert_eq!(err, Some(RouteError::UnknownHarness("aider".into())));
}

#[test]
fn worker_route_without_a_route_fails_closed() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let err = request_adapter(&adapters(&log), &request(dir.path(), None), &no_secret).err();
    assert_eq!(err, Some(RouteError::Missing));
}

/// An OAuth account's login directory reaches the harness through the env the route names.
#[tokio::test]
async fn worker_route_account_directory_is_put_in_the_named_env() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let mut r = route("claude", "claude-sonnet-5-5");
    r.credential = Some(CredentialReference::AccountDirectory {
        path: PathBuf::from("/accounts/claude/acct"),
        env: "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
    });
    let adapter = route_adapter(&adapters(&log), &r, &no_secret).unwrap();
    let _ = adapter
        .run(request(dir.path(), Some(r)), "r1", limits(), &NullSink)
        .await;
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(
        seen.env,
        vec![(
            "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_string(),
            "/accounts/claude/acct".to_string()
        )]
    );
}

/// An OpenAI-compatible source's endpoint and resolved credential reach the harness; a harness
/// that cannot take an endpoint, or a credential that cannot be resolved, fails closed.
#[tokio::test]
async fn worker_route_openai_compatible_endpoint_and_credential() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let mut r = route("pi", "qwen/qwen3-coder");
    r.endpoint = Some(LlmEndpointReference {
        base_url: Some("http://127.0.0.1:18000/v1".into()),
        api_format: "openai_compatible".into(),
    });
    r.credential = Some(CredentialReference::Environment {
        name: "qwen-api-key".into(),
    });
    let secret = |name: &str| (name == "qwen-api-key").then(|| "k".to_string());
    let adapter = route_adapter(&adapters(&log), &r, &secret).unwrap();
    let _ = adapter
        .run(
            request(dir.path(), Some(r.clone())),
            "r1",
            limits(),
            &NullSink,
        )
        .await;
    assert_eq!(
        log.lock().unwrap()[0].endpoint,
        Some(("http://127.0.0.1:18000/v1".into(), Some("k".into())))
    );
    assert_eq!(
        route_adapter(&adapters(&log), &r, &no_secret).err(),
        Some(RouteError::CredentialUnresolved {
            source_id: "src".into(),
            name: "qwen-api-key".into()
        })
    );
    r.harness_id = "claude".into();
    assert_eq!(
        route_adapter(&adapters(&log), &r, &secret).err(),
        Some(RouteError::EndpointUnsupported {
            harness: "claude".into(),
            base_url: "http://127.0.0.1:18000/v1".into()
        })
    );
}

/// A real process: the harness declaration's command is started with the route's model.
#[tokio::test]
async fn worker_route_starts_the_declared_command_with_the_route_model() {
    let dir = tempfile::tempdir().unwrap();
    let command = dir.path().join("pi-declared");
    crate::test_support::write_executable(
        &command,
        r#"#!/bin/sh
cat > /dev/null
for arg in "$@"; do printf '%s\n' "$arg" >> args.copy; done
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"agent_end","messages":[]}'
"#,
    );
    let extension = dir.path().join("hashline.ts");
    std::fs::write(&extension, "// stub extension path").unwrap();
    let mut table: HashMap<HarnessId, Arc<dyn WorkerAdapter>> = HashMap::new();
    table.insert(
        "pi_declared".into(),
        Arc::new(PiAdapter::new(PiConfig {
            command: command.to_string_lossy().into(),
            extensions: vec![extension],
            tools: vec!["hashline_read".into(), "hashline_edit".into()],
            ..Default::default()
        })),
    );
    let req = request(
        dir.path(),
        Some(route("pi_declared", "opencode-go/deepseek-v4-pro")),
    );
    let adapter = request_adapter(&table, &req, &no_secret).unwrap();
    let result = adapter
        .run(req, "route", limits(), &NullSink)
        .await
        .unwrap();
    assert!(matches!(result.terminal, Terminal::Done { .. }));
    let args = std::fs::read_to_string(dir.path().join("args.copy")).unwrap();
    assert!(
        args.contains("--model\nopencode-go/deepseek-v4-pro\n"),
        "{args}"
    );
}
