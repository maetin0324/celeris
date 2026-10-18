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
                    env: declared_env(config, h).unwrap_or_default(),
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
                    env: declared_env(config, h).unwrap_or_default(),
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
                    env: declared_env(config, h).unwrap_or_default(),
                    container: None,
                }))
            }
            PiAdapter::ID => Arc::new(PiAdapter::new(pi_config(config, h))),
            AcpAdapter::ID | BrowserSpecialistAdapter::ID => {
                let base = &config.adapters.acp;
                let inner: Arc<dyn WorkerAdapter> = Arc::new(AcpAdapter::new(AcpConfig {
                    command: command(&base.command),
                    args: args(&base.args),
                    env: declared_env(config, h).unwrap_or_default(),
                    permission: base.permission,
                    model: None,
                    model_namespace: if id == "opencode" {
                        h.model_namespace.clone()
                    } else {
                        None
                    },
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
                // `env_of`: a capability probe that runs under the environment of the real harness it stands for.
                let probe_env = match &h.env_of {
                    None => None,
                    Some(base) => {
                        let declared = config.harness_declarations.get(base).ok_or_else(|| {
                            config::ConfigError::Invalid(format!(
                                "harnesses.{id}: env_of {base:?} is not a declared harness"
                            ))
                        })?;
                        Some(declared_env(config, declared).ok_or_else(|| {
                            config::ConfigError::Invalid(format!(
                                "harnesses.{id}: env_of {base:?} (kind {:?}) has no adapter environment",
                                declared.kind
                            ))
                        })?)
                    }
                };
                fake.set_env(probe_env.unwrap_or_else(|| {
                    env(
                        &config.adapters.fake.env,
                        &config.adapters.fake.env_from_secrets,
                    )
                }));
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

/// The environment a real harness's adapter gives its worker (`[adapters.<kind>].env` plus its secrets, or the Pi
/// declaration's own env). One function for both the real adapter and a capability probe that must see the same
/// environment (`env_of`), so the two cannot drift. `None` for kinds a B/C run does not use as a coding harness.
fn declared_env(
    config: &Config,
    h: &config::HarnessDeclarationConfig,
) -> Option<Vec<(String, String)>> {
    let secrets_dir = config.secrets.as_ref().map(|s| s.dir.as_path());
    let none = HashMap::new();
    let env = |base_env: &HashMap<String, String>, base_secrets: &HashMap<String, String>| {
        merged_env_with_secrets(base_env, base_secrets, &none, &none, secrets_dir)
    };
    let a = &config.adapters;
    match h.kind.replace('_', "-").as_str() {
        ClaudeCodeAdapter::ID => Some(env(&a.claude_code.env, &a.claude_code.env_from_secrets)),
        CodexAdapter::ID => Some(env(&a.codex.env, &a.codex.env_from_secrets)),
        AiderAdapter::ID => Some(env(&a.aider.env, &a.aider.env_from_secrets)),
        PiAdapter::ID => Some(env(&h.env, &h.env_from_secrets)),
        AcpAdapter::ID | BrowserSpecialistAdapter::ID => {
            Some(env(&a.acp.env, &a.acp.env_from_secrets))
        }
        _ => None,
    }
}

fn pi_config(config: &Config, harness: &config::HarnessDeclarationConfig) -> PiConfig {
    let secrets_dir = config.secrets.as_ref().map(|s| s.dir.as_path());
    let none = HashMap::new();
    PiConfig {
        command: harness.command.clone().unwrap_or_else(|| "pi".into()),
        args: harness.args.clone(),
        model: None,
        extensions: harness.extensions.clone(),
        tools: harness.tools.clone(),
        base_url: None,
        env: merged_env_with_secrets(
            &harness.env,
            &harness.env_from_secrets,
            &none,
            &none,
            secrets_dir,
        ),
        container: None,
    }
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
    use super::{build_harness_adapters, pi_config};
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
    fn opencode_harness_passes_model_namespace() {
        let cfg = load("[harnesses.opencode]\nkind='acp'\nmodel_namespace='qwen-local'\n");
        let adapters = build_harness_adapters(&cfg).unwrap();
        assert_eq!(
            adapters[&"opencode".into()].model_namespace(),
            Some("qwen-local")
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

    #[test]
    fn pi_harness_declaration_reaches_pi_config() {
        let cfg = load(
            r#"
[harnesses.pi]
kind = "pi"
extensions = ["/tmp/hashline.ts"]
tools = ["hashline_read", "hashline_edit", "bash"]
env = { OPENAI_BASE_URL = "http://proxy" }
"#,
        );
        let pi = pi_config(&cfg, &cfg.harness_declarations["pi"]);
        assert_eq!(
            pi.extensions,
            [std::path::PathBuf::from("/tmp/hashline.ts")]
        );
        assert_eq!(pi.tools, ["hashline_read", "hashline_edit", "bash"]);
        assert_eq!(pi.env, [("OPENAI_BASE_URL".into(), "http://proxy".into())]);
    }

    /// ADR 2026-10-11 付記 §2: the declaration must reach the process `PiAdapter` starts. A stub `pi`
    /// records its argv (one per line) and its environment; no real pi or network is used.
    /// The same executable writer as `model_discovery/tests.rs` (ETXTBSY-safe).
    fn write_executable(path: &std::path::Path, script: &str) {
        use std::io::Write;
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#"cat > "$1" && chmod 755 "$1""#)
            .arg("sh")
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(format!("#!/bin/sh\n{script}\n").as_bytes())
            .unwrap();
        drop(stdin);
        assert!(child.wait().unwrap().success());
    }

    #[tokio::test]
    async fn pi_harness_declaration_reaches_pi_process() {
        use task_worker::adapter::NullSink;
        use task_worker::protocol::{
            PROTOCOL_VERSION, RunContext, RunRequest, sample_task_for_tests,
        };
        use task_worker::{RunLimits, Terminal};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stub = root.join("pi-stub");
        let extension = root.join("hashline.ts");
        std::fs::write(&extension, "// stub Hashline extension").unwrap();
        let secrets = root.join("secrets");
        std::fs::create_dir_all(&secrets).unwrap();
        std::fs::write(secrets.join("pi-test-key"), "secret-value\n").unwrap();
        let argv_out = root.join("argv.copy");
        let env_out = root.join("env.copy");
        let artifacts = root.join("artifacts");
        write_executable(
            &stub,
            &format!(
                "cat > /dev/null\n\
                 printf '%s\\n' \"$@\" > '{argv}'\n\
                 env > '{env}'\n\
                 mkdir -p '{artifacts}'\n\
                 printf '%s' '{{\"summary\":\"ok\",\"evidence\":[]}}' > '{artifacts}/result.json'\n\
                 printf '%s\\n' '{{\"type\":\"agent_end\",\"messages\":[]}}'",
                argv = argv_out.display(),
                env = env_out.display(),
                artifacts = artifacts.display(),
            ),
        );
        let toml = format!(
            "[secrets]\n\
             dir = '{secrets}'\n\
             [harnesses.pi]\n\
             kind = \"pi\"\n\
             command = '{stub}'\n\
             extensions = ['{extension}']\n\
             tools = [\"hashline_read\", \"hashline_edit\", \"bash\"]\n\
             env = {{ PI_TEST_PLAIN = \"plain-value\" }}\n\
             env_from_secrets = {{ PI_TEST_SECRET = \"pi-test-key\" }}\n",
            secrets = secrets.display(),
            stub = stub.display(),
            extension = extension.display(),
        );
        let cfg = load(&toml);
        let adapters = build_harness_adapters(&cfg).unwrap();
        // The dispatcher applies the selected route's `provider/model` at launch (as here).
        let pi = adapters[&"pi".into()]
            .with_model("opencode-go/deepseek-v4-pro")
            .unwrap();
        let req = RunRequest {
            protocol: PROTOCOL_VERSION,
            task: sample_task_for_tests(),
            workspace: root.to_path_buf(),
            artifacts_dir: artifacts.clone(),
            work_dir: None,
            context: RunContext::default(),
            cargo_target_dir: None,
            llm_route: None,
        };
        let limits = RunLimits {
            wall_clock: std::time::Duration::from_secs(60),
            idle_timeout: std::time::Duration::from_secs(60),
            kill_grace: std::time::Duration::from_millis(200),
        };
        let outcome = pi.run(req, "declared", limits, &NullSink).await.unwrap();
        assert!(matches!(outcome.terminal, Terminal::Done { .. }));

        let argv = std::fs::read_to_string(&argv_out).unwrap();
        let argv: Vec<&str> = argv.lines().collect();
        assert!(
            argv.windows(2)
                .any(|w| w == ["--tools", "hashline_read,hashline_edit,bash"]),
            "{argv:?}"
        );
        let extension = extension.to_string_lossy();
        assert!(
            argv.windows(2).any(|w| w == ["-e", extension.as_ref()]),
            "{argv:?}"
        );
        let env = std::fs::read_to_string(&env_out).unwrap();
        let env: Vec<&str> = env.lines().collect();
        assert!(env.contains(&"PI_TEST_PLAIN=plain-value"), "{env:?}");
        assert!(env.contains(&"PI_TEST_SECRET=secret-value"), "{env:?}");
    }
}
