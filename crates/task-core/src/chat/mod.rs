//! CoS chat wire contract (ADR 2026-10-05 D2).
//! Store operations are separate; these types define the stable REST/SSE JSON shape.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod attachments;
mod credential;
#[cfg(test)]
mod credential_tests;
mod operations;
#[cfg(test)]
mod operations_tests;
mod run_store;
pub(crate) mod store;
#[cfg(test)]
mod store_tests;
pub use credential::{ChatCheckpointSaved, CosRunCredentialError, CosRunIdentity};
pub use operations::{AuditContext, CosOperation};
pub use run_store::{
    CHAT_EVENT_PAGE_DEFAULT, CHAT_EVENT_PAGE_MAX, CHAT_EVENT_RETENTION_DAYS,
    CHAT_TOOL_DETAIL_MAX_BYTES, ChatEventQuery, ChatStopOutcome, chat_run_state_is_terminal,
};
pub use store::{
    CHAT_CLIENT_KEY_MAX_BYTES, CHAT_MESSAGE_ATTACHMENTS_MAX, CHAT_MESSAGE_PAGE_DEFAULT,
    CHAT_MESSAGE_PAGE_MAX, CHAT_MESSAGE_TEXT_MAX_BYTES, CHAT_QUEUE_MAX, CHAT_THREAD_PAGE_DEFAULT,
    CHAT_THREAD_PAGE_MAX, CHAT_TITLE_MAX_CHARS, ChatError, ChatMessagePosted, ChatMessageQuery,
    ChatThreadCreated, ChatThreadQuery, chat_fts_literal,
};

macro_rules! wire_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
    };
}
wire_enum!(ChatThreadKind {
    Human,
    Inbox,
    Legacy
});
wire_enum!(ChatThreadStatus { Open, Archived });
wire_enum!(ChatMessageRole {
    User,
    Assistant,
    System
});
wire_enum!(ChatMessageState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Interrupted,
    Failed
});
wire_enum!(ChatRunState {
    Queued,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
    Interrupted
});
wire_enum!(ChatAttachmentState { Ready, Deleted });
wire_enum!(ChatCardKind {
    Task,
    Decision,
    Question,
    Approval,
    PlanGate,
    Notice,
    Operation
});
wire_enum!(ChatActor { Human, Cos, System });
wire_enum!(ChatSendMode { Queue, Interrupt });
wire_enum!(ChatEventType {
    Message,
    TextDelta,
    Status,
    Tool,
    Run,
    Queue,
    Card,
    Thread
});
wire_enum!(ChatStatusPhase {
    Queued,
    Starting,
    Thinking,
    Working,
    Waiting
});
wire_enum!(ChatToolState {
    Running,
    Completed,
    Failed
});
wire_enum!(ChatSessionMode {
    New,
    Resumed,
    Fresh
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatThread {
    pub id: String,
    pub kind: ChatThreadKind,
    pub title: String,
    pub project_id: Option<String>,
    pub status: ChatThreadStatus,
    pub queue_paused: bool,
    pub active_run_id: Option<String>,
    pub queued_count: u32,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatCard {
    pub kind: ChatCardKind,
    pub id: String,
    pub title: String,
    pub state: String,
    pub href: String,
    pub actor: ChatActor,
    pub reason: Option<String>,
    pub operation_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessage {
    pub id: String,
    pub thread_id: String,
    pub seq: u64,
    pub role: ChatMessageRole,
    pub text: String,
    pub state: ChatMessageState,
    pub client_message_id: Option<String>,
    pub reply_to_id: Option<String>,
    pub run_id: Option<String>,
    pub attachment_ids: Vec<String>,
    pub cards: Vec<ChatCard>,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRun {
    pub id: String,
    pub thread_id: String,
    pub input_message_id: String,
    pub output_message_id: Option<String>,
    pub state: ChatRunState,
    pub reason: Option<String>,
    pub harness: Option<String>,
    pub llm_source: Option<String>,
    pub provider: Option<String>,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub tier: Option<String>,
    pub session_mode: Option<ChatSessionMode>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatAttachment {
    pub id: String,
    pub thread_id: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub state: ChatAttachmentState,
    pub preview_url: Option<String>,
    pub download_url: String,
    pub expires_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: ChatEventType,
    pub thread_id: String,
    pub run_id: Option<String>,
    pub message_id: Option<String>,
    pub at: String,
    pub data: ChatEventData,
}
// Each variant has a distinct field set. `untagged` keeps data as the D2 object,
// without inserting another discriminator into the SSE envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ChatEventData {
    Message(ChatMessageData),
    TextDelta(ChatTextDeltaData),
    Status(ChatStatusData),
    Tool(ChatToolData),
    Run(ChatRunData),
    Queue(ChatQueueData),
    Card(ChatCardData),
    Thread(ChatThreadData),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMessageData {
    pub message: ChatMessage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatTextDeltaData {
    pub offset: u64,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatStatusData {
    pub phase: ChatStatusPhase,
    pub summary: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatToolData {
    pub call_id: String,
    pub name: String,
    pub state: ChatToolState,
    pub summary: String,
    pub detail: Option<String>,
    pub error: bool,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatRunData {
    pub run: ChatRun,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatQueueData {
    pub message_ids: Vec<String>,
    pub paused: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatCardData {
    pub card: ChatCard,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatThreadData {
    pub thread: ChatThread,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatCreateThreadRequest {
    pub title: String,
    pub project_id: Option<String>,
    pub client_thread_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatPatchThreadRequest {
    pub title: Option<String>,
    pub status: Option<ChatThreadStatus>,
    pub expected_revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatPostMessageRequest {
    pub client_message_id: String,
    pub text: String,
    pub attachment_ids: Vec<String>,
    pub reply_to_id: Option<String>,
    pub mode: ChatSendMode,
    pub resume_queue: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatStopRequest {
    pub run_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatResumeQueueRequest {
    pub expected_revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatReferenceRequest {
    pub owner_kind: ChatReferenceOwnerKind,
    pub owner_id: String,
    pub idempotency_key: String,
}
wire_enum!(ChatReferenceOwnerKind {
    Task,
    KnowledgeInbox
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatThreadListResponse {
    pub items: Vec<ChatThread>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatThreadResponse {
    pub thread: ChatThread,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatThreadDetailResponse {
    pub thread: ChatThread,
    pub active_run: Option<ChatRun>,
    pub last_event_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageListResponse {
    pub items: Vec<ChatMessage>,
    pub next_before_seq: Option<u64>,
    pub next_after_seq: Option<u64>,
    pub snapshot_event_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageResponse {
    pub message: ChatMessage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatPostMessageResponse {
    pub message: ChatMessage,
    pub run_id: Option<String>,
    pub queue_position: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRunResponse {
    pub run: ChatRun,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatStopResponse {
    pub run: ChatRun,
    pub queue_paused: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatEventListResponse {
    pub items: Vec<ChatEvent>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatAttachmentResponse {
    pub attachment: ChatAttachment,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatReferenceResponse {
    pub attachment_id: String,
    pub owner_kind: ChatReferenceOwnerKind,
    pub owner_id: String,
}

#[cfg(test)]
mod tests;
