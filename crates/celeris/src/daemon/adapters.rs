//! harness 宣言から run adapter を組み立て、秘密の利用先を公開する。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use task_worker::{
    AcpAdapter, AcpConfig, AiderAdapter, AiderConfig, BrowserSpecialistAdapter, ClaudeCodeAdapter,
    ClaudeCodeConfig, CodexAdapter, CodexConfig, FakeAdapter, LangMemAdapter, LangMemConfig,
    LdrAdapter, LdrConfig, PaperQaAdapter, PaperQaConfig, PiAdapter, PiConfig, WorkerAdapter,
};

use super::secrets::merged_env_with_secrets;
use crate::{Config, config};

/// ADR 2026-10-11 llm-source-harness-selection: one base adapter per `[harnesses.<id>]`.
/// `kind` names the adapter (`claude_code` and `claude-code` are the same kind); `[adapters.<kind>]`
/// is the base and the declaration's `command`/`args` override it. The model and the account are
/// not set here: the dispatcher applies the selected route's model and account at launch.
/// An unknown kind is a configuration error (no harness without an adapter).
pub fn build_harness_adapters(
    config: &Config,
) -> Result<HashMap<task_core::llm_route::HarnessId, Arc<dyn WorkerAdapter>>, config::ConfigError> {
    let secrets_dir = config.secrets.as_ref().map(|s| s.dir.as_path());
    let none = HashMap::new();
    let env = |base_env: &HashMap<String, String>, base_secrets: &HashMap<String, String>| {
        merged_env_with_secrets(base_env, base_secrets, &none, &none, secrets_dir)
    };
    let mut out: HashMap<task_core::llm_route::HarnessId, Arc<dyn WorkerAdapter>> = HashMap::new();
    for (id, h) in &config.harness_declarations {
        let kind = h.kind.replace('_', "-");
        let command = |default: &str| h.command.clone().unwrap_or_else(|| default.to_string());
        let args = |default: &[String]| {
            if h.args.is_empty() {
                default.to_vec()
            } else {
                h.args.clone()
            }
        };
        let adapter: Arc<dyn WorkerAdapter> = match kind.as_str() {
            ClaudeCodeAdapter::ID => {
                let base = &config.adapters.claude_code;
                Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
                    command: command(&base.command),
                    extra_args: args(&base.extra_args),
                    permission_mode: base.permission_mode.clone(),
                    model: None,
                    env: env(&base.env, &base.env_from_secrets),
                    container: None,
                    subagents: base.resolved_subagents(),
                }))
            }
            CodexAdapter::ID => {
                let base = &config.adapters.codex;
                Arc::new(CodexAdapter::new(CodexConfig {
                    command: command(&base.command),
                    extra_args: args(&base.extra_args),
                    model: None,
                    env: env(&base.env, &base.env_from_secrets),
                    container: None,
                    resume_mode: base.resolved_resume_mode(),
                    resume_bypass: base.resolved_resume_bypass(),
                    subagents: base.resolved_subagents(),
                }))
            }
            AiderAdapter::ID => {
                let base = &config.adapters.aider;
                Arc::new(AiderAdapter::new(AiderConfig {
                    command: command(&base.command),
                    extra_args: args(&base.extra_args),
                    model: None,
                    env: env(&base.env, &base.env_from_secrets),
                    container: None,
                }))
            }
            PiAdapter::ID => Arc::new(PiAdapter::new(PiConfig {
                command: command("pi"),
                args: h.args.clone(),
                model: None,
                extensions: Vec::new(),
                tools: Vec::new(),
                base_url: None,
                env: Vec::new(),
                container: None,
            })),
            AcpAdapter::ID | BrowserSpecialistAdapter::ID => {
                let base = &config.adapters.acp;
                let inner: Arc<dyn WorkerAdapter> = Arc::new(AcpAdapter::new(AcpConfig {
                    command: command(&base.command),
                    args: args(&base.args),
                    env: env(&base.env, &base.env_from_secrets),
                    permission: base.permission,
                    model: None,
                    model_option_id: base.model_option_id.clone(),
                    startup_timeout: Duration::from_secs(base.startup_timeout_secs),
                    container: None,
                    subagents: base.resolved_subagents(),
                }));
                if kind == BrowserSpecialistAdapter::ID {
                    Arc::new(BrowserSpecialistAdapter::new(inner))
                } else {
                    inner
                }
            }
            PaperQaAdapter::ID => {
                let base = &config.adapters.paperqa;
                Arc::new(PaperQaAdapter::new(PaperQaConfig {
                    command: command(&base.command),
                    settings: base.settings.clone(),
                    paper_directory: base.paper_directory.clone(),
                    index_directory: base.index_directory.clone(),
                    index_name: base.index_name.clone(),
                    model: None,
                    env: env(&base.env, &base.env_from_secrets),
                    extra_args: args(&base.extra_args),
                    acquire: base.acquire.clone(),
                    evidence: base.evidence,
                    max_asks: base.max_asks,
                    knowledge_root: Some(config.knowledge.root.clone()),
                }))
            }
            LdrAdapter::ID => {
                let base = &config.adapters.local_deep_research;
                let mut settings: Vec<(String, String)> = base
                    .settings
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                settings.sort();
                Arc::new(LdrAdapter::new(LdrConfig {
                    command: command(&base.command),
                    mode: base.mode,
                    iterations: base.iterations,
                    questions_per_iteration: base.questions_per_iteration,
                    settings,
                    model: None,
                    env: env(&base.env, &base.env_from_secrets),
                    evidence: base.evidence,
                    retry_mode: base.retry_mode,
                    retry_iterations: base.retry_iterations,
                    structured_synthesis: base.structured_synthesis,
                }))
            }
            LangMemAdapter::ID => {
                let base = &config.adapters.langmem;
                let llm = &config.knowledge.langmem;
                Arc::new(LangMemAdapter::new(LangMemConfig {
                    command: command(&base.command),
                    idle_timeout_secs: base.idle_timeout_secs,
                    provider: llm.provider,
                    base_url: llm.base_url.clone(),
                    model: llm.model.clone(),
                    api_key: config.langmem_api_key(),
                    env: env(&base.env, &base.env_from_secrets),
                }))
            }
            FakeAdapter::ID => {
                let argv = match &h.command {
                    Some(c) => std::iter::once(c.clone())
                        .chain(h.args.iter().cloned())
                        .collect(),
                    None => config.adapters.fake.command.clone(),
                };
                let mut fake = FakeAdapter::new(argv);
                fake.set_env(env(
                    &config.adapters.fake.env,
                    &config.adapters.fake.env_from_secrets,
                ));
                Arc::new(fake)
            }
            _ => {
                return Err(config::ConfigError::Invalid(format!(
                    "harnesses.{id}: unknown kind {:?}; expected one of claude_code, codex, aider, \
                     pi, acp, browser_specialist, paperqa, local_deep_research, langmem, fake",
                    h.kind
                )));
            }
        };
        out.insert(id.as_str().into(), adapter);
    }
    Ok(out)
}

/// ADR-0030 D3: `GET /secrets` の `used_by` を設定から導く（秘密 id → それを使っている adapter の
/// env_from_secrets・LangMem の api_key_secret・LLM source の credential_ref の一覧）。設定順・キー順で決定的に並べる。
pub fn secret_usage(config: &Config) -> HashMap<String, Vec<task_api::types::SecretUse>> {
    fn push(
        map: &mut HashMap<String, Vec<task_api::types::SecretUse>>,
        secret_id: &str,
        scope: &str,
        name: &str,
        env: &str,
    ) {
        map.entry(secret_id.to_string())
            .or_default()
            .push(task_api::types::SecretUse {
                scope: scope.to_string(),
                name: name.to_string(),
                env: env.to_string(),
            });
    }

    let mut map: HashMap<String, Vec<task_api::types::SecretUse>> = HashMap::new();
    let adapters: [(&str, &HashMap<String, String>); 8] = [
        (
            ClaudeCodeAdapter::ID,
            &config.adapters.claude_code.env_from_secrets,
        ),
        (CodexAdapter::ID, &config.adapters.codex.env_from_secrets),
        (AiderAdapter::ID, &config.adapters.aider.env_from_secrets),
        (FakeAdapter::ID, &config.adapters.fake.env_from_secrets),
        (AcpAdapter::ID, &config.adapters.acp.env_from_secrets),
        (
            PaperQaAdapter::ID,
            &config.adapters.paperqa.env_from_secrets,
        ),
        (
            LdrAdapter::ID,
            &config.adapters.local_deep_research.env_from_secrets,
        ),
        (
            LangMemAdapter::ID,
            &config.adapters.langmem.env_from_secrets,
        ),
    ];
    for (name, from_secrets) in adapters {
        let mut env_keys: Vec<&String> = from_secrets.keys().collect();
        env_keys.sort();
        for env_key in env_keys {
            push(&mut map, &from_secrets[env_key], "adapter", name, env_key);
        }
    }
    // ADR-0047 D4: `[knowledge.langmem].api_key_secret`（環境変数の写像ではなく 1 つの LLM 鍵）。
    if let Some(id) = &config.knowledge.langmem.api_key_secret {
        push(&mut map, id, "adapter", LangMemAdapter::ID, "api_key");
    }
    for (id, source) in &config.llm_sources {
        if let Some(secret_id) = &source.credential_ref {
            push(&mut map, secret_id, "source", id, "credential");
        }
    }
    map
}

#[cfg(test)]
mod harness_adapter_tests {
    use super::build_harness_adapters;
    use crate::Config;

    fn load(text: &str) -> Config {
        Config::parse_with_delivery(text).unwrap()
    }

    #[test]
    fn route_select_dispatch_adapter_daemon_builds_one_adapter_per_declared_harness() {
        let cfg = load(
            r#"
[llm_sources.main]
kind = "openai_compatible"
models = ["m1"]
[harnesses.opencode]
kind = "acp"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible"]
command = "opencode"
args = ["acp"]
[harnesses.claude]
kind = "claude_code"
source_kinds = ["claude_oauth"]
[harnesses.stub]
kind = "fake"
"#,
        );
        let adapters = build_harness_adapters(&cfg).unwrap();
        let mut ids: Vec<(String, String)> = adapters
            .iter()
            .map(|(h, a)| (h.to_string(), a.id().to_string()))
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                ("claude".into(), "claude-code".into()),
                ("opencode".into(), "acp".into()),
                ("stub".into(), "fake".into()),
            ]
        );
    }

    #[test]
    fn route_select_dispatch_adapter_daemon_rejects_unknown_harness_kind() {
        let cfg = load(
            r#"
[harnesses.odd]
kind = "no_such_runner"
"#,
        );
        let err = build_harness_adapters(&cfg).err().map(|e| e.to_string());
        assert!(
            err.as_deref()
                .is_some_and(|e| e.contains("harnesses.odd") && e.contains("no_such_runner")),
            "{err:?}"
        );
    }
}
