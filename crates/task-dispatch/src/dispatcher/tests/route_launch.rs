//! ADR 2026-10-11 llm-source-harness-selection: launch inputs for an admitted route — the
//! harness adapter with the picked model and account, and the worker-facing route description.

use super::*;
use crate::dispatcher::RouteLaunchError;
use crate::dispatcher::route_select::RoutePick;
use std::collections::BTreeMap;
use task_core::llm_route::{Candidate, HarnessId, Registry, SourceSpec};
use task_worker::protocol::{CredentialReference, LlmEndpointReference, SelectedLlmRoute};

/// Records what the launch helper applied; `id()` exposes it so the test can read it back
/// through `Arc<dyn WorkerAdapter>`.
struct RecordingAdapter {
    harness: String,
    model: Option<String>,
    env: Vec<(String, String)>,
    label: String,
}

impl RecordingAdapter {
    fn boxed(
        harness: &str,
        model: Option<String>,
        env: Vec<(String, String)>,
    ) -> Arc<dyn WorkerAdapter> {
        let env_text: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let label = format!(
            "{harness}|model={}|env={}",
            model.as_deref().unwrap_or("-"),
            env_text.join(",")
        );
        Arc::new(RecordingAdapter {
            harness: harness.into(),
            model,
            env,
            label,
        })
    }
}

#[async_trait]
impl WorkerAdapter for RecordingAdapter {
    fn id(&self) -> &str {
        &self.label
    }
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(RecordingAdapter::boxed(
            &self.harness,
            Some(model.into()),
            self.env.clone(),
        ))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut env = self.env.clone();
        env.extend_from_slice(extra);
        Some(RecordingAdapter::boxed(
            &self.harness,
            self.model.clone(),
            env,
        ))
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        Err(AdapterError::Io(std::io::Error::other(
            "not run in these tests",
        )))
    }
}

fn source(
    kind: &str,
    api: &str,
    endpoint: Option<&str>,
    credential_ref: Option<&str>,
) -> SourceSpec {
    SourceSpec {
        kind: kind.into(),
        api_format: api.into(),
        accounts: vec![],
        models: vec!["m".into()],
        max_runs: None,
        account_max_runs: BTreeMap::new(),
        quota: None,
        endpoint: endpoint.map(Into::into),
        credential_ref: credential_ref.map(Into::into),
    }
}

fn registry() -> Registry {
    let mut r = Registry::default();
    r.sources.insert(
        "claude_main".into(),
        source("claude_oauth", "claude_cli", None, None),
    );
    r.sources.insert(
        "qwen".into(),
        source(
            "openai_compatible",
            "openai_compatible",
            Some("http://127.0.0.1:17942/v1"),
            Some("qwen-api-key"),
        ),
    );
    r
}

fn pick(harness: &str, source: &str, api: &str, model: &str, account: &str) -> RoutePick {
    RoutePick {
        harness: harness.into(),
        candidate: Candidate {
            source: source.into(),
            model: model.into(),
            api_format: api.into(),
        },
        account: account.into(),
    }
}

fn launch_dispatcher(accounts_root: PathBuf) -> Dispatcher {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> =
        Arc::new(SqliteStore::open(&dir.path().join("db.sqlite")).unwrap());
    let mut d = pool_dispatcher(store, done_adapter(), None, accounts_root, 1, 1);
    d.set_llm_route_registry(registry());
    d.set_harness_adapters(HashMap::from([
        (
            HarnessId::from("claude_code"),
            RecordingAdapter::boxed("claude_code", None, vec![]),
        ),
        (
            HarnessId::from("opencode"),
            RecordingAdapter::boxed("opencode", None, vec![]),
        ),
    ]));
    d
}

#[test]
fn route_select_dispatch_adapter_uses_the_selected_harness_with_the_model() {
    let d = launch_dispatcher(PathBuf::from("/accounts/claude"));
    let adapter = d
        .route_adapter(&pick(
            "opencode",
            "qwen",
            "openai_compatible",
            "qwen3",
            "shared",
        ))
        .unwrap();
    // The opencode adapter, not claude_code's; the picked model; no account env for an API source.
    assert_eq!(adapter.id(), "opencode|model=qwen3|env=");
}

#[test]
fn route_select_dispatch_adapter_applies_the_oauth_account_env() {
    let d = launch_dispatcher(PathBuf::from("/accounts/claude"));
    let adapter = d
        .route_adapter(&pick(
            "claude_code",
            "claude_main",
            "claude_cli",
            "sonnet",
            "ca",
        ))
        .unwrap();
    assert_eq!(
        adapter.id(),
        format!(
            "claude_code|model=sonnet|env={}=/accounts/claude/ca",
            AccountAdapter::ClaudeCode.env_var()
        )
    );
}

#[test]
fn route_select_dispatch_adapter_unknown_harness_or_source_is_err() {
    let d = launch_dispatcher(PathBuf::from("/accounts/claude"));
    assert_eq!(
        d.route_adapter(&pick(
            "aider",
            "qwen",
            "openai_compatible",
            "qwen3",
            "shared"
        ))
        .err(),
        Some(RouteLaunchError::UnknownHarness("aider".into()))
    );
    assert_eq!(
        d.route_adapter(&pick("opencode", "nowhere", "openai_compatible", "m", "a"))
            .err(),
        Some(RouteLaunchError::UnknownSource("nowhere".into()))
    );
    // An OAuth source whose pool has no `[accounts]` root cannot carry its account: fail closed.
    let mut d = launch_dispatcher(PathBuf::from("/accounts/claude"));
    let mut r = registry();
    r.sources.insert(
        "codex_main".into(),
        source("codex_oauth", "codex_cli", None, None),
    );
    d.set_llm_route_registry(r);
    assert_eq!(
        d.route_adapter(&pick("claude_code", "codex_main", "codex_cli", "gpt", "xa"))
            .err(),
        Some(RouteLaunchError::AccountUnavailable {
            source: "codex_main".into(),
            account: "xa".into(),
        })
    );
}

#[test]
fn route_select_dispatch_adapter_selected_llm_route_carries_references_only() {
    let d = launch_dispatcher(PathBuf::from("/accounts/claude"));
    let api = d
        .route_launch_target(&pick(
            "opencode",
            "qwen",
            "openai_compatible",
            "qwen3",
            "shared",
        ))
        .unwrap();
    assert_eq!(
        api,
        SelectedLlmRoute {
            harness_id: "opencode".into(),
            source_id: "qwen".into(),
            model_id: "qwen3".into(),
            account_id: "shared".into(),
            endpoint: Some(LlmEndpointReference {
                base_url: Some("http://127.0.0.1:17942/v1".into()),
                api_format: "openai_compatible".into(),
            }),
            credential: Some(CredentialReference::Environment {
                name: "qwen-api-key".into(),
            }),
        }
    );
    let oauth = d
        .route_launch_target(&pick(
            "claude_code",
            "claude_main",
            "claude_cli",
            "sonnet",
            "ca",
        ))
        .unwrap();
    assert_eq!(oauth.endpoint, None);
    assert_eq!(
        oauth.credential,
        Some(CredentialReference::AccountDirectory {
            path: PathBuf::from("/accounts/claude/ca"),
            env: task_core::AccountAdapter::ClaudeCode.env_var().into(),
        })
    );
    assert_eq!(
        crate::dispatcher::selected_llm_route(
            &registry(),
            &pick("opencode", "nowhere", "openai_compatible", "m", "a"),
            None
        ),
        Err(RouteLaunchError::UnknownSource("nowhere".into()))
    );
}
