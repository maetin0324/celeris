use super::*;

#[test]
fn message_content_round_trips_a_plain_string() {
    let msg: ChatMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
    assert_eq!(msg.content.unwrap().as_text(), "hi");
}

#[test]
fn tool_call_message_round_trips() {
    let json = serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": "call_1",
            "type": "function",
            "function": {"name": "lookup", "arguments": "{\"q\":\"x\"}"}
        }]
    });
    let msg: ChatMessage = serde_json::from_value(json).unwrap();
    assert_eq!(msg.tool_calls.unwrap()[0].function.name, "lookup");
}
