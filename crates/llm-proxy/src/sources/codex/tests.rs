use super::*;
use crate::openai::{ChatCompletionRequest, ChatMessage};

fn test_config() -> CodexOauthConfig {
    CodexOauthConfig {
        accounts_dir: std::path::PathBuf::new(),
        responses_url: "http://localhost/responses".to_string(),
        token_url: "http://localhost/token".to_string(),
        client_id: "test-client".to_string(),
        enabled: true,
        user_agent: "codex_cli_rs/0.45.0".to_string(),
        send_sampling_params: false,
        reasoning_effort: None,
    }
}

fn user_message(text: &str) -> ChatMessage {
    ChatMessage {
        role: "user".to_string(),
        content: Some(MessageContent::Text(text.to_string())),
        name: None,
        tool_calls: None,
        tool_call_id: None,
    }
}

fn base_request() -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: "gpt-5".to_string(),
        messages: vec![user_message("hi")],
        temperature: Some(0.7),
        top_p: None,
        max_tokens: Some(512),
        stop: None,
        stream: false,
        tools: None,
        tool_choice: None,
    }
}

/// Phase 65b 受け入れ条件 (a): 既定では `store:false` / `stream:true` / `instructions` あり /
/// `temperature`・`max_output_tokens` は無し（Codex CLI が送る形。ADR-0053 Phase 65b 追記）。
#[test]
fn to_responses_body_matches_the_codex_cli_shape_by_default() {
    let req = base_request();
    let cfg = test_config();
    let body = to_responses_body(&req, "gpt-5", &cfg);
    assert_eq!(body["store"], false);
    assert_eq!(
        body["stream"], true,
        "クライアントの要求(stream:false)に関わらず上流へはいつも stream:true"
    );
    assert!(
        body["instructions"].as_str().is_some_and(|s| !s.is_empty()),
        "system が無くても既定の instructions が入る: {body}"
    );
    assert!(
        body.get("temperature").is_none(),
        "既定では temperature を送らない: {body}"
    );
    assert!(
        body.get("max_output_tokens").is_none(),
        "既定では max_output_tokens を送らない: {body}"
    );
}

#[test]
fn to_responses_body_uses_the_client_system_message_as_instructions() {
    let mut req = base_request();
    req.messages.insert(
        0,
        ChatMessage {
            role: "system".to_string(),
            content: Some(MessageContent::Text("You are terse.".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        },
    );
    let body = to_responses_body(&req, "gpt-5", &test_config());
    assert_eq!(body["instructions"], "You are terse.");
}

#[test]
fn to_responses_body_sends_sampling_params_only_when_opted_in() {
    let req = base_request();
    let mut cfg = test_config();
    cfg.send_sampling_params = true;
    let body = to_responses_body(&req, "gpt-5", &cfg);
    assert_eq!(body["temperature"], 0.7);
    assert_eq!(body["max_output_tokens"], 512);
}

#[test]
fn to_responses_body_adds_reasoning_and_include_only_when_configured() {
    let req = base_request();
    let mut cfg = test_config();
    cfg.reasoning_effort = Some("medium".to_string());
    let body = to_responses_body(&req, "gpt-5", &cfg);
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert_eq!(body["include"][0], "reasoning.encrypted_content");

    let without = to_responses_body(&req, "gpt-5", &test_config());
    assert!(without.get("reasoning").is_none());
    assert!(without.get("include").is_none());
}

/// Phase 65b 受け入れ条件 (c): ChatGPT backend がよく返す `{"detail": ...}` を要約に反映する。
#[test]
fn extract_error_summary_prefers_detail_and_message_fields() {
    assert_eq!(
        extract_error_summary(r#"{"detail":"Store must be set to false"}"#),
        "Store must be set to false"
    );
    assert_eq!(
        extract_error_summary(r#"{"error":{"message":"bad request"}}"#),
        "bad request"
    );
    assert_eq!(
        extract_error_summary(r#"{"message":"plain message field"}"#),
        "plain message field"
    );
    assert_eq!(extract_error_summary(""), "upstream error");
}

#[test]
fn extract_error_summary_is_truncated_to_300_chars() {
    let long = "x".repeat(500);
    let body = format!(r#"{{"detail":"{long}"}}"#);
    let summary = extract_error_summary(&body);
    assert_eq!(summary.chars().count(), 300);
}
