//! Pi + explicit Hashline extension worker (ADR 2026-10-07).
//! JSON print mode; Celeris retains planning, retries and review.
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use task_core::Usage;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{
    AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter, deadline_terminal,
    spawn_capture_stderr,
};
use crate::claude_code::build_prompt_with_skill_list;
use crate::delegate_file::forward_delegate_file;
use crate::protocol::RunRequest;
use crate::provider::classify_provider_failure;
use crate::result_report::{ResultFileRead, interpret_result_text, read_result_file};
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, finish_run, kill_now, prepare_run, read_line_limited, read_tail,
    reap_after_terminal,
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

    fn with_llm_endpoint(
        &self,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.base_url = Some(base_url.to_owned());
        if let Some(key) = api_key {
            config.env.retain(|(k, _)| k != "CELERIS_PI_API_KEY");
            config
                .env
                .push(("CELERIS_PI_API_KEY".into(), key.to_owned()));
        }
        Some(Arc::new(Self::new(config)))
    }

    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.container = Some(plan);
        Some(Arc::new(Self::new(config)))
    }
}

/// ADR 2026-10-08-cos-chat-prompt-cache D6.2: the stdin of a CoS chat pi run — the skill list
/// (pi runs with `--no-skills`, so the list stays in the input) followed by the run specific part.
pub(crate) fn cos_chat_pi_input(skills: &[crate::protocol::SkillMount], variable: &str) -> String {
    format!(
        "{}{variable}",
        crate::skills::skills_block(skills)
            .map(|block| format!("{block}\n"))
            .unwrap_or_default()
    )
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
    let paths = prepare_run(req, run_id, "stdout.log").await?;
    let (run_dir, artifacts_rel) = (&paths.run_dir, &paths.artifacts_rel);
    tokio::fs::create_dir_all(&req.artifacts_dir).await?;

    // ADR 2026-10-08-cos-chat-prompt-cache D6.2: a CoS chat run gives pi the fixed Core with
    // `--append-system-prompt` (pi appends it right after its own base prompt, ahead of the project
    // context files and the cwd line) and only the skill list and the run specific part on stdin.
    let cos_parts =
        crate::claude_code::build_cos_chat_parts(&req.task, &req.context, run_id, artifacts_rel);
    // 必須規則（作業場所の CLAUDE.md）は prompt の先頭に写す。pi は AGENTS.md があると CLAUDE.md を
    // 読まないので、作業ツリーの file に頼らない。読めなければ run を落とす。
    let rules = crate::required_rules::preamble_for_run(req.cwd()).await?;
    let prompt = match &cos_parts {
        Some(parts) => cos_chat_pi_input(&req.context.skills, &parts.variable),
        None => build_prompt_with_skill_list(&req.task, &req.context, run_id, artifacts_rel),
    };
    let prompt = format!("{rules}{prompt}");
    crate::skills::deliver_agent_skills(req.cwd(), &req.context.skills).await?;
    let recorded = match &cos_parts {
        Some(parts) => {
            crate::cos_chat::prompt_record("--append-system-prompt", &parts.core, &prompt)
        }
        None => prompt.clone(),
    };
    crate::subprocess::write_run_prompt(run_dir, &recorded, run_id).await;

    let agent_dir = run_dir.join("pi-agent");
    tokio::fs::create_dir_all(&agent_dir).await?;
    tokio::fs::write(
        agent_dir.join("settings.json"),
        b"{\"retry\":{\"enabled\":false},\"enableInstallTelemetry\":false}",
    )
    .await?;
    let (provider, model) = config
        .model
        .as_deref()
        .and_then(|m| m.split_once('/'))
        .ok_or_else(|| AdapterError::Other("pi model must be provider/model".into()))?;
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
        .arg(config.tools.join(","));
    if let Some(parts) = &cos_parts {
        command.arg("--append-system-prompt").arg(&parts.core);
    }
    command
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
        if let Some(reference) = &req.context.routing_context_ref {
            custom["headers"] =
                serde_json::json!({"x-celeris-routing-context": "CELERIS_PI_ROUTING_CONTEXT"});
            command.env("CELERIS_PI_ROUTING_CONTEXT", reference);
            context_transport = true;
        }
        let models = serde_json::json!({"providers": {provider: custom}});
        tokio::fs::write(agent_dir.join("models.json"), serde_json::to_vec(&models)?).await?;
    }
    crate::routing_context_transport::record(run_dir, req, context_transport).await;
    let mut command = crate::db_guard::launch(command, config.container.as_deref());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::subprocess::check_arg_lengths(PiAdapter::ID, &command)?;

    let mut child = crate::subprocess::spawn_after_notice(sink, &mut command)?;
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
    let stderr_task = spawn_capture_stderr(&mut child, &paths.stderr_log, run_id)?;

    let mut stdout_file = tokio::fs::File::create(&paths.stdout_log).await?;
    let mut reader = BufReader::new(stdout);

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;
    let mut stream = PiStream::default();

    loop {
        let wait =
            match deadline_terminal(start, last_activity, limits.wall_clock, limits.idle_timeout) {
                Ok(wait) => wait,
                Err(terminal) => {
                    timeout_terminal = Some(*terminal);
                    force_kill = true;
                    break;
                }
            };

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

    let stderr_tail = read_tail(&paths.stderr_log, 4096).await;
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
            usage: None,
        }
    } else if let Some(message) = &stream.error {
        Terminal::Error {
            message: message.clone(),
            retryable: true,
            usage: None,
        }
    } else if !exit_status.success() || !stream.ended {
        Terminal::Error {
            message: "pi exited unsuccessfully or without agent_end".into(),
            retryable: true,
            usage: None,
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
        terminal_from_result(req, start, stream.usage).await
    };
    let provider_failure = if matches!(terminal, Terminal::Error { .. }) {
        pi_failure(stream.error.as_deref().unwrap_or("")).or_else(|| pi_failure(&stderr_tail))
    } else {
        None
    };

    finish_run(run_dir, terminal, provider_failure, exit_status).await
}

/// 結果ファイルの読み取り結果から終端を合成する。本文の解釈は結果契約の正本
/// （`result_report::interpret_result_text`。ADR 2026-11-10-adapter-result-contract D1）に任せ、ここは
/// 結果ファイルが無いときの pi の文言だけを持つ。
/// `start` は監視 loop の開始（spawn の直後）。誤配置の候補は、それを壁時計に直した時刻より後に書かれた file に絞る
/// （`result_location::misplaced_report` は 2 秒の余裕を取る）。
async fn terminal_from_result(req: &RunRequest, start: Instant, usage: Option<Usage>) -> Terminal {
    let artifacts_rel = req.artifacts_rel();
    match read_result_file(req, std::time::SystemTime::now() - start.elapsed()).await {
        ResultFileRead::Misplaced(error) => *error,
        ResultFileRead::Missing => Terminal::Error {
            message: format!("pi exited without {artifacts_rel}/result.json"),
            retryable: true,
            usage: None,
        },
        ResultFileRead::Text(text) => interpret_result_text(&text, &artifacts_rel, usage),
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
