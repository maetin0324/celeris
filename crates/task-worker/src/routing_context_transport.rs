//! Adapter-specific proxy header transport. The reference is an opaque, daemon-issued value.
//! Only the transport status remains in run evidence; never log the reference.

use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};
use tokio::process::Command;

use crate::protocol::{ContextTransport, RunRequest, RunTransportEvidence};

pub(crate) const HEADER: &str = "x-celeris-routing-context";

pub(crate) async fn record(run_dir: &Path, req: &RunRequest, supported: bool) {
    if req.context.routing_context_ref.is_none() {
        return;
    }
    let transport = if supported {
        ContextTransport::Header
    } else {
        ContextTransport::Unsupported
    };
    let evidence = RunTransportEvidence {
        context_transport: Some(transport),
    };
    if let Ok(mut data) = serde_json::to_vec(&evidence) {
        data.push(b'\n');
        let _ = tokio::fs::write(run_dir.join("context-transport.json"), data).await;
    }
}

fn valid_ref(req: &RunRequest) -> Option<&str> {
    req.context
        .routing_context_ref
        .as_deref()
        .filter(|reference| {
            !reference.is_empty()
                && reference.len() <= 256
                && reference.bytes().all(|byte| byte.is_ascii_graphic())
        })
}

fn env_value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// OpenCode loads `OPENCODE_CONFIG_CONTENT` after `OPENCODE_CONFIG` and deep-merges it.
/// Limit the overlay to the configured Celeris provider so other endpoints cannot receive the ref.
pub(crate) fn configure_acp(
    command: &mut Command,
    env: &[(String, String)],
    model: Option<&str>,
    req: &RunRequest,
) -> bool {
    let Some(reference) = valid_ref(req) else {
        return false;
    };
    let inline = env_value(env, "OPENCODE_CONFIG_CONTENT");
    let file_config = env_value(env, "OPENCODE_CONFIG")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|body| serde_json::from_str::<Value>(&body).ok());
    let mut overlay = match inline {
        Some(body) => match serde_json::from_str::<Value>(body) {
            Ok(value) if value.is_object() => value,
            _ => return false,
        },
        None => json!({}),
    };
    let selected = model
        .or_else(|| overlay.get("model").and_then(Value::as_str))
        .or_else(|| file_config.as_ref()?.get("model")?.as_str());
    let Some((provider_id, _model_id)) = selected.and_then(|name| name.split_once('/')) else {
        return false;
    };
    let provider_id = provider_id.to_owned();
    let base_url = overlay
        .get("provider")
        .and_then(|providers| providers.get(&provider_id))
        .and_then(|provider| provider.get("options"))
        .and_then(|options| options.get("baseURL"))
        .and_then(Value::as_str)
        .or_else(|| {
            file_config
                .as_ref()?
                .get("provider")?
                .get(&provider_id)?
                .get("options")?
                .get("baseURL")?
                .as_str()
        });
    match base_url {
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => {}
        _ => return false,
    }
    let inherited_header_names: Vec<String> = file_config
        .as_ref()
        .and_then(|config| config.get("provider"))
        .and_then(|providers| providers.get(&provider_id))
        .and_then(|provider| provider.get("options"))
        .and_then(|options| options.get("headers"))
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|headers| headers.keys())
        .filter(|name| name.eq_ignore_ascii_case(HEADER))
        .cloned()
        .collect();
    let Some(root) = overlay.as_object_mut() else {
        return false;
    };
    let providers = root.entry("provider").or_insert_with(|| json!({}));
    let Some(providers) = providers.as_object_mut() else {
        return false;
    };
    let provider = providers.entry(provider_id).or_insert_with(|| json!({}));
    let Some(provider) = provider.as_object_mut() else {
        return false;
    };
    let options = provider.entry("options").or_insert_with(|| json!({}));
    let Some(options) = options.as_object_mut() else {
        return false;
    };
    let headers = options.entry("headers").or_insert_with(|| json!({}));
    let Some(headers) = headers.as_object_mut() else {
        return false;
    };
    headers.retain(|key, _| !key.eq_ignore_ascii_case(HEADER));
    for name in inherited_header_names {
        headers.insert(name, json!(reference));
    }
    headers.insert(HEADER.into(), json!(reference));
    command.env("OPENCODE_CONFIG_CONTENT", overlay.to_string());
    true
}

/// Aider passes `extra_params.extra_headers` to LiteLLM. The run-local model settings
/// contain only the opaque reference and no credentials or prompt text.
pub(crate) fn configure_aider(
    command: &mut Command,
    env: &[(String, String)],
    _model: Option<&str>,
    extra_args: &[String],
    run_dir: &Path,
    req: &RunRequest,
) -> Option<tempfile::NamedTempFile> {
    let reference = valid_ref(req)?;
    if (env_value(env, "OPENAI_API_BASE").is_none() && env_value(env, "OPENAI_BASE_URL").is_none())
        || extra_args
            .iter()
            .any(|arg| arg == "--model-settings-file" || arg.starts_with("--model-settings-file="))
    {
        return None;
    }
    let body = format!(
        "- name: aider/extra_params\n  extra_params:\n    extra_headers:\n      {HEADER}: {}\n",
        json!(reference)
    );
    let mut file = tempfile::NamedTempFile::new_in(run_dir).ok()?;
    file.write_all(body.as_bytes()).ok()?;
    command.arg("--model-settings-file").arg(file.path());
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PROTOCOL_VERSION, RunContext};

    fn request(with_ref: bool) -> RunRequest {
        RunRequest {
            protocol: PROTOCOL_VERSION,
            task: crate::protocol::tests::sample_task(),
            workspace: std::env::temp_dir(),
            artifacts_dir: std::env::temp_dir().join("artifacts"),
            work_dir: None,
            context: RunContext {
                routing_context_ref: with_ref.then(|| "opaque-ref".into()),
                ..Default::default()
            },
            cargo_target_dir: None,
            llm_route: None,
        }
    }

    #[test]
    fn opencode_transport_requires_routing_ref_and_provider_base_url() {
        let mut command = Command::new("opencode");
        let config = r#"{"model":"opencode-go/grok-4.6","provider":{"opencode-go":{"options":{"baseURL":"http://127.0.0.1:18100/v1"}}}}"#;
        assert!(configure_acp(
            &mut command,
            &[("OPENCODE_CONFIG_CONTENT".into(), config.into())],
            Some("opencode-go/grok-4.6"),
            &request(true),
        ));
        assert!(!configure_acp(
            &mut Command::new("opencode"),
            &[("OPENCODE_CONFIG_CONTENT".into(), config.into())],
            Some("opencode-go/grok-4.6"),
            &request(false),
        ));
        let missing_url = r#"{"model":"opencode-go/grok-4.6"}"#;
        assert!(!configure_acp(
            &mut Command::new("opencode"),
            &[("OPENCODE_CONFIG_CONTENT".into(), missing_url.into())],
            Some("opencode-go/grok-4.6"),
            &request(true),
        ));
        let configured_url = r#"{"model":"opencode-go/grok-4.6","provider":{"opencode-go":{"options":{"baseURL":"https://api.example/v1"}}}}"#;
        assert!(configure_acp(
            &mut Command::new("opencode"),
            &[("OPENCODE_CONFIG_CONTENT".into(), configured_url.into())],
            Some("opencode-go/grok-4.6"),
            &request(true),
        ));
    }

    #[test]
    fn aider_transport_requires_routing_ref_and_api_base() {
        let dir = tempfile::tempdir().unwrap();
        let env = vec![("OPENAI_API_BASE".into(), "http://127.0.0.1:18100/v1".into())];
        let mut command = Command::new("aider");
        let settings = configure_aider(
            &mut command,
            &env,
            Some("opencode-go/grok-4.6"),
            &[],
            dir.path(),
            &request(true),
        );
        let contents = std::fs::read_to_string(settings.unwrap().path()).unwrap();
        assert!(contents.contains(HEADER));
        assert!(
            configure_aider(
                &mut Command::new("aider"),
                &env,
                Some("qwen/qwen3.8-27b"),
                &[],
                dir.path(),
                &request(true),
            )
            .is_some()
        );
        assert!(
            configure_aider(
                &mut Command::new("aider"),
                &[],
                Some("opencode-go/grok-4.6"),
                &[],
                dir.path(),
                &request(true),
            )
            .is_none()
        );
        assert!(
            configure_aider(
                &mut Command::new("aider"),
                &env,
                Some("opencode-go/grok-4.6"),
                &[],
                dir.path(),
                &request(false),
            )
            .is_none()
        );
    }
}
