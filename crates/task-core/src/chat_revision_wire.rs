//! ADR 2026-11-21-chat-discussion-mode D5.1・D5.2: the wire types of a person's edit / retract of
//! a chat message. [`crate::chat`] re-exports them (`ChatMessage.edit_state`,
//! `ChatEventData::Revised`); the edit itself is [`crate::chat_revision`]. A leaf: it depends on
//! nothing in this crate, so `chat` can use it without a cycle (and without growing `chat`, which
//! sits at repo health's `size.responsibility-growth` module limit).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// D5.2: `metadata_json.revision` / `edited` / `retracted` of one message, flattened into the
/// wire `ChatMessage`. A message never changed carries nothing (`revision` 1 is not sent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageEditState {
    #[serde(default = "first_revision", skip_serializing_if = "is_first_revision")]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited: Option<ChatMessageChange>,
    /// The author retracted the message; its `text` is empty from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted: Option<ChatMessageChange>,
}

impl Default for ChatMessageEditState {
    fn default() -> Self {
        Self {
            revision: 1,
            edited: None,
            retracted: None,
        }
    }
}

impl ChatMessageEditState {
    /// Reads the record from a message's `metadata_json` (absent keys = never changed).
    pub fn of_metadata(meta: &Value) -> Result<Box<Self>, serde_json::Error> {
        let change = |key: &str| match meta.get(key) {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone()).map(Some),
            _ => Ok(None),
        };
        Ok(Box::new(Self {
            revision: meta.get("revision").and_then(Value::as_u64).unwrap_or(1),
            edited: change("edited")?,
            retracted: change("retracted")?,
        }))
    }
}

fn first_revision() -> u64 {
    1
}

fn is_first_revision(r: &u64) -> bool {
    *r == 1
}

/// D5.2: who changed a message and when (`by` is always `human:<name>`); `reason` only on a retract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageChange {
    pub by: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// D5.1 `PATCH /chat/threads/{t}/messages/{m}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMessageEditRequest {
    pub text: String,
    pub expected_revision: u64,
}

/// D5.1 `POST /chat/threads/{t}/messages/{m}/retract` (`reason` up to 500 characters, may be empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMessageRetractRequest {
    pub expected_revision: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageRevisionKind {
    Edit,
    Retract,
}

/// D5.2・D5.4: the data of a `message_revised` chat event, appended in the edit / retract
/// transaction. It keeps the previous body (events are append-only; retention never removes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMessageRevisedData {
    pub message_id: String,
    pub kind: ChatMessageRevisionKind,
    pub revision: u64,
    pub previous_revision: u64,
    pub previous_text: String,
    pub previous_sha256: String,
    pub by: String,
    pub reason: Option<String>,
    pub checkpoint_reset: Option<ChatCheckpointReset>,
}

/// D5.4: the thread summary the change invalidated (the summary covered the changed message).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatCheckpointReset {
    pub previous_through_seq: u64,
    pub previous_summary: String,
}
