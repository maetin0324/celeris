//! Pi + explicit Hashline extension worker (ADR 2026-10-07).
//! JSON print mode; Celeris retains planning, retries and review.
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use task_core::Usage;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::claude_code::build_prompt;
use crate::delegate_file::{clear_delegate_file, forward_delegate_file};
use crate::protocol::{Evidence, RunRequest};
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, kill_now, read_line_limited, read_tail, reap_after_terminal,
    write_result_json,
};

#[derive(Debug, Clone)]
pub struct PiConfig {
    pub command: String,
    /// Executable prefix only (e.g. node CLI path); worker flags cannot be overridden.
    pub args: Vec<String>,
    pub model: Option<String>,
    /// Explicit Hashline paths; no automatic extension discovery.
    pub extensions: Vec<std::path::PathBuf>,
    /// Complete allowlist. Hashline tool names are configured by the operator.
    pub tools: Vec<String>,
    pub base_url: Option<String>,
    pub env: Vec<(String, String)>,
    pub container: Option<crate::container::SharedPlan>,
}

impl Default for PiConfig {
    fn default() -> Self {
        Self {
            command: "pi".into(),
            args: Vec::new(),
            model: None,
            extensions: Vec::new(),
            tools: Vec::new(),
            base_url: None,
            env: Vec::new(),
            container: None,
        }
    }
}

impl PiConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.extensions.is_empty() || self.tools.is_empty() {
            return Err("pi requires explicit Hashline extensions and tool allowlist".into());
        }
        if self
            .args
            .iter()
            .any(|a| a.starts_with('-') || a.starts_with('@'))
        {
            return Err("pi args are command prefix paths, not CLI options or prompt files".into());
        }
        if self.tools.iter().any(|t| {
            t.is_empty()
                || !t
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                || t.to_ascii_lowercase().contains("subagent")
                || t.to_ascii_lowercase().contains("planner")
        }) {
            return Err("pi tools must be tool names without subagent/planner tools".into());
        }
        if !self.model.as_deref().is_some_and(|m| {
            m.split_once('/')
                .is_some_and(|(p, id)| !p.is_empty() && !id.is_empty())
        }) {
            return Err("pi model must be provider/model".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PiAdapter {
    config: PiConfig,
}

impl PiAdapter {
    pub const ID: &'static str = "pi";

    pub fn new(config: PiConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for PiAdapter {
    fn id(&self) -> &str {
        Self::ID
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        run_pi(&self.config, &req, run_id, &limits, sink).await
    }

    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.model = Some(model.to_owned());
        Some(Arc::new(Self::new(config)))
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(Self::new(config)))
    }

    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.container = Some(plan);
        Some(Arc::new(Self::new(config)))
    }
}

#[derive(Debug, Deserialize)]
struct ResultFile {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    evidence: serde_json::Value,
    #[serde(default, rename = "yield")]
    r#yield: Option<serde_json::Value>,
}

fn lenient_evidence(value: serde_json::Value) -> Vec<Evidence> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<Evidence>(item).ok())
            .collect(),
        _ => Vec::new(),
    }
}

async fn run_pi(
    config: &PiConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    config.validate().map_err(AdapterError::Other)?;
    let extensions: Vec<_> = config
        .extensions
        .iter()
        .map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                req.cwd().join(path)
            }
        })
        .collect();
    for path in &extensions {
        if !path.is_file() && !path.is_dir() {
            return Err(AdapterError::Other(format!(
                "pi Hashline extension does not exist: {}",
                path.display()
            )));
        }
    }
    let run_dir = req.workspace.join("runs").join(run_id);
    tokio::fs::create_dir_all(&run_dir).await?;
    let artifacts_rel = req.artifacts_rel();
    let result_path = req.artifact_path("result.json");
    let _ = tokio::fs::remove_file(&result_path).await;
    clear_delegate_file(&req.artifacts_dir).await;
    tokio::fs::create_dir_all(&req.artifacts_dir).await?;

    let mut prompt = build_prompt(&req.task, &req.context, run_id, &artifacts_rel);
    crate::skills::deliver_agent_skills(req.cwd(), &req.context.skills).await?;
    prompt.push_str(&crate::skills::preamble_section(&req.context.skills));
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    crate::subprocess::write_run_prompt(&run_dir, &prompt, run_id).await;

    let agent_dir = run_dir.join("pi-agent");
    tokio::fs::create_dir_all(&agent_dir).await?;
    tokio::fs::write(
        agent_dir.join("settings.json"),
        b"{\"retry\":{\"enabled\":false},\"enableInstallTelemetry\":false}",
    )
    .await?;
    let (selected_model, provider, model) = config
        .model
        .as_deref()
        .and_then(|m| m.split_once('/').map(|(p, id)| (m, p, id)))
        .ok_or_else(|| AdapterError::Other("pi model must be provider/model".into()))?;
    // Legacy Celeris wire names contain the tier, not a Pi provider prefix.
    let model = if matches!(
        selected_model,
        "celeris/frontier" | "celeris/standard" | "celeris/cheap"
    ) {
        selected_model
    } else {
        model
    };
    let cli_model = format!("{provider}/{model}");
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .args([
            "--mode",
            "json",
            "-p",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
        ])
        .arg("--session-dir")
        .arg(run_dir.join("pi-sessions"))
        .arg("--provider")
        .arg(provider)
        .arg("--model")
        .arg(&cli_model)
        .arg("--tools")
        .arg(config.tools.join(","))
        .envs(config.env.iter().cloned())
        .env("PI_CODING_AGENT_DIR", &agent_dir)
        .current_dir(req.cwd());
    if provider == "opencode-go"
        && let Some((_, account_dir)) = config
            .env
            .iter()
            .rev()
            .find(|(name, _)| name == "XDG_DATA_HOME")
    {
        let key = crate::opencode_account::read_go_key(std::path::Path::new(account_dir))
            .ok_or_else(|| {
                AdapterError::AuthFailed("selected opencode-go account has no API key".into())
            })?;
        command.env("OPENCODE_API_KEY", key.0);
    }
    for path in extensions {
        command.arg("-e").arg(path);
    }
    let mut context_transport = false;
    if let Some(base_url) = &config.base_url {
        let mut custom = serde_json::json!({
            "baseUrl": base_url, "api": "openai-completions",
            "apiKey": "CELERIS_PI_API_KEY", "models": [{"id": model}]
        });
        if !config.env.iter().any(|(k, _)| k == "CELERIS_PI_API_KEY") {
            let key = config
                .env
                .iter()
                .rev()
                .find(|(name, _)| name == "OPENAI_API_KEY")
                .map(|(_, key)| key.as_str())
                .unwrap_or("local");
            command.env("CELERIS_PI_API_KEY", key);
        }
        if matches!(
            model,
            "celeris/frontier" | "celeris/standard" | "celeris/cheap"
        ) && let Some(reference) = &req.context.routing_context_ref
        {
            custom["headers"] =
                serde_json::json!({"x-celeris-routing-context": "CELERIS_PI_ROUTING_CONTEXT"});
            command.env("CELERIS_PI_ROUTING_CONTEXT", reference);
            context_transport = true;
        }
        let models = serde_json::json!({"providers": {provider: custom}});
        tokio::fs::write(agent_dir.join("models.json"), serde_json::to_vec(&models)?).await?;
    }
    crate::routing_context_transport::record(&run_dir, req, context_transport).await;
    let mut command = crate::db_guard::launch(command, config.container.as_deref());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::subprocess::check_arg_lengths(PiAdapter::ID, &command)?;

    let mut child = command.spawn().map_err(AdapterError::Spawn)?;
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdin was not piped".into()))?;
    let prompt_task = tokio::spawn(async move {
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await
    });

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdout was not piped".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AdapterError::Other("worker stderr was not piped".into()))?;

    let stdout_log_path = run_dir.join("stdout.log");
    let stderr_log_path = run_dir.join("stderr.log");
    let stderr_log_path_for_task = stderr_log_path.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = stderr;
        match tokio::fs::File::create(&stderr_log_path_for_task).await {
            Ok(mut file) => {
                if let Err(e) = tokio::io::copy(&mut reader, &mut file).await {
                    warn!("failed to write worker stderr.log: {e}");
                }
            }
            Err(e) => warn!("failed to create worker stderr.log: {e}"),
        }
    });

    let mut stdout_file = tokio::fs::File::create(&stdout_log_path).await?;
    let mut reader = BufReader::new(stdout);

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;
    let mut stream = PiStream::default();

    loop {
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            timeout_terminal = Some(Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::WallClock,
                message: "wall clock exceeded".into(),
                usage: None,
            });
            force_kill = true;
            break;
        }
        let idle_elapsed = last_activity.elapsed();
        if idle_elapsed >= limits.idle_timeout {
            timeout_terminal = Some(Terminal::Error {
                message: "idle timeout".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let wait = (limits.wall_clock - wall_elapsed).min(limits.idle_timeout - idle_elapsed);

        let outcome = match tokio::time::timeout(
            wait,
            read_line_limited(&mut reader, MAX_LINE_BYTES),
        )
        .await
        {
            Err(_elapsed) => continue,
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(outcome)) => outcome,
        };

        match outcome {
            LineOutcome::Eof => break,
            LineOutcome::TooLong => {
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from pi stdout");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                if let Ok(event) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    stream.event(&event, sink);
                }
            }
        }
    }

    let exit_status = if force_kill {
        kill_now(&mut child, limits.kill_grace).await?
    } else {
        reap_after_terminal(&mut child, limits.kill_grace).await?
    };

    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;

    if force_kill {
        prompt_task.abort();
    }
    let _ = prompt_task.await;
    forward_delegate_file(&req.artifacts_dir, sink).await;

    let stderr_tail = read_tail(&stderr_log_path, 4096).await;
    let extension_failed = stderr_tail.contains("Failed to load extension")
        || stderr_tail.contains("Extension error (");
    let terminal = if let Some(mut terminal) = timeout_terminal {
        if let Terminal::BudgetExhausted { usage, .. } = &mut terminal {
            *usage = stream.usage;
        }
        terminal
    } else if extension_failed {
        Terminal::Error {
            message: "pi Hashline extension failed".into(),
            retryable: false,
        }
    } else if let Some(message) = &stream.error {
        Terminal::Error {
            message: message.clone(),
            retryable: true,
        }
    } else if !exit_status.success() || !stream.ended {
        Terminal::Error {
            message: "pi exited unsuccessfully or without agent_end".into(),
            retryable: true,
        }
    } else if let Some(summary) = stream.last_reply.clone().filter(|_| {
        // ADR 2026-10-05 cos-chat-home（live-check 不具合 1）: a CoS chat run replies in its body.
        req.context.cos_chat.is_some() && !req.artifacts_dir.join("result.json").exists()
    }) {
        Terminal::Done {
            summary,
            evidence: Vec::new(),
            usage: stream.usage,
        }
    } else {
        terminal_from_result(&req.artifacts_dir, &artifacts_rel, stream.usage).await
    };
    let provider_failure = if matches!(terminal, Terminal::Error { .. }) {
        pi_failure(stream.error.as_deref().unwrap_or("")).or_else(|| pi_failure(&stderr_tail))
    } else {
        None
    };

    write_result_json(&run_dir, &terminal, provider_failure).await?;

    if let (Terminal::Error { message, .. }, Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

async fn terminal_from_result(
    artifacts_dir: &std::path::Path,
    artifacts_rel: &str,
    usage: Option<Usage>,
) -> Terminal {
    let result_path = artifacts_dir.join("result.json");
    let text = match tokio::fs::read_to_string(&result_path).await {
        Ok(t) => t,
        Err(_) => {
            return Terminal::Error {
                message: format!("pi exited without {artifacts_rel}/result.json"),
                retryable: true,
            };
        }
    };
    if let Some(terminal) = crate::adapter::result_file_wait(&text, usage) {
        return terminal;
    }
    match serde_json::from_str::<ResultFile>(&text) {
        Ok(rf) => {
            if let Some(question) = rf.question {
                Terminal::Question { text: question }
            } else if let Some(summary) = rf.summary {
                Terminal::Done {
                    summary,
                    evidence: lenient_evidence(rf.evidence),
                    usage,
                }
            } else if let Some(checkpoint) = rf.r#yield {
                Terminal::Yielded { checkpoint, usage }
            } else {
                Terminal::Error {
                    message: format!(
                        "{artifacts_rel}/result.json has neither 'summary', 'question' nor 'yield'"
                    ),
                    retryable: true,
                }
            }
        }
        Err(e) => Terminal::Error {
            message: format!("{artifacts_rel}/result.json is not valid JSON: {e}"),
            retryable: true,
        },
    }
}

#[derive(Default)]
struct PiStream {
    usage: Option<Usage>,
    error: Option<String>,
    ended: bool,
    /// Text of the last assistant message (a CoS chat run's reply when `result.json` is absent).
    last_reply: Option<String>,
}

impl PiStream {
    fn event(&mut self, event: &serde_json::Value, sink: &dyn EventSink) {
        match event["type"].as_str() {
            Some("message_end") if event["message"]["role"] == "assistant" => {
                let message = &event["message"];
                if let Some(usage) = message.get("usage") {
                    let total = self.usage.get_or_insert_with(Usage::default);
                    add_tokens(&mut total.input_tokens, usage["input"].as_u64());
                    add_tokens(&mut total.output_tokens, usage["output"].as_u64());
                    add_tokens(&mut total.cache_read_tokens, usage["cacheRead"].as_u64());
                    add_tokens(
                        &mut total.cache_creation_tokens,
                        usage["cacheWrite"].as_u64(),
                    );
                    let context: u64 = ["input", "cacheRead", "cacheWrite"]
                        .iter()
                        .filter_map(|k| usage[*k].as_u64())
                        .sum();
                    if context > 0 {
                        total.context_tokens = Some(context);
                    }
                    if let Some(cost) = usage["cost"]["total"]
                        .as_f64()
                        .filter(|n| n.is_finite() && *n >= 0.0)
                    {
                        *total.cost_usd.get_or_insert(0.0) += cost;
                    }
                }
                if matches!(message["stopReason"].as_str(), Some("error" | "aborted")) {
                    self.error = Some(
                        message["errorMessage"]
                            .as_str()
                            .unwrap_or("pi request failed")
                            .into(),
                    );
                } else {
                    // A successful retry's final message supersedes an earlier failed attempt.
                    self.error = None;
                }
                if let Some(content) = message["content"].as_array() {
                    let mut reply = Vec::new();
                    for block in content {
                        if block["type"] == "text"
                            && let Some(text) = block["text"].as_str()
                        {
                            sink.progress(text);
                            if !text.trim().is_empty() {
                                reply.push(text.trim());
                            }
                        }
                    }
                    if !reply.is_empty() {
                        self.last_reply = Some(reply.join("\n\n"));
                    }
                }
            }
            Some("agent_end") => self.ended = true,
            Some("compaction_end") if event["aborted"] == false && event["result"].is_object() => {
                sink.context_compacted()
            }
            Some("tool_execution_start") => {
                if let Some(name) = event["toolName"].as_str() {
                    sink.progress_with(
                        name,
                        &task_core::ProgressFields::of(task_core::ProgressKind::ToolUse)
                            .with_tool(name),
                    );
                }
            }
            _ => {}
        }
    }
}

fn add_tokens(total: &mut Option<u64>, count: Option<u64>) {
    if let Some(count) = count {
        *total = Some(total.unwrap_or(0).saturating_add(count));
    }
}

fn pi_failure(text: &str) -> Option<crate::protocol::ProviderFailure> {
    if text.contains("GoUsageLimitError") {
        Some(crate::protocol::ProviderFailure::Throttled {
            retry_after_secs: 60,
        })
    } else {
        classify_provider_failure(text)
    }
}

#[cfg(test)]
mod tests;
