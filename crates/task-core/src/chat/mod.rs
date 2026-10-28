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
mod override_op;
pub mod relay_rejudge;
#[cfg(test)]
mod relay_rejudge_tests;
mod run_store;
mod session;
#[cfg(test)]
mod session_tests;
pub(crate) mod store;
#[cfg(test)]
mod store_tests;
mod system_request;
pub mod triage;
#[cfg(test)]
mod triage_tests;
mod window;
#[cfg(test)]
mod window_tests;
pub use credential::{ChatCheckpointSaved, CosRunCredentialError, CosRunIdentity};
pub use operations::{AuditContext, CosOperation};
pub use override_op::{OverrideAction, OverrideResult};
pub use run_store::{
    CHAT_EVENT_PAGE_DEFAULT, CHAT_EVENT_PAGE_MAX, CHAT_EVENT_RETENTION_DAYS,
    CHAT_TOOL_DETAIL_MAX_BYTES, ChatEventQuery, ChatStopOutcome, chat_run_state_is_terminal,
    chat_run_stop_tx,
};
pub use session::{
    COS_CHAT_SESSION_KIND, COS_CHAT_SESSION_NODE, ChatRunSessionMode, ChatRunSessionRecord,
    ChatSession, ChatSessionKey, ChatSessionUsage,
};
pub use store::{
    CHAT_CLIENT_KEY_MAX_BYTES, CHAT_MESSAGE_ATTACHMENTS_MAX, CHAT_MESSAGE_PAGE_DEFAULT,
    CHAT_MESSAGE_PAGE_MAX, CHAT_MESSAGE_TEXT_MAX_BYTES, CHAT_QUEUE_MAX, CHAT_RELAY_TASKS_MAX,
    CHAT_RELAYS_PROMPT_MAX, CHAT_THREAD_PAGE_DEFAULT, CHAT_THREAD_PAGE_MAX, CHAT_TITLE_MAX_CHARS,
    ChatError, ChatMessagePosted, ChatMessageQuery, ChatThreadCreated, ChatThreadQuery,
    chat_fts_literal, chat_message_cancel_tx, chat_message_post_cos_tx, chat_thread_mark_read_tx,
    chat_thread_patch_tx, chat_thread_resume_queue_tx, cos_thread_ensure_tx, cos_thread_label,
    cos_thread_open_tx, relay_expired,
};
pub use window::{COS_ESCALATION_CARD_KEY_PREFIX, chat_relay_forward_tx};

macro_rules! wire_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
    };
}
// ADR 2026-10-22-cos-fixed-threads D1: the general CoS (one), one CoS per project, and the
// read-only threads kept by migration 0092.
wire_enum!(ChatThreadKind {
    General,
    Project,
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
    /// D5: highest seq the person has read (`POST /chat/threads/{t}/read`).
    pub read_through_seq: u64,
    /// D5: assistant/system messages after `read_through_seq`.
    pub unread_count: u32,
    /// D5: inbox items of this thread still waiting (pending/running/escalated/fallback).
    pub attention_count: u32,
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
    /// ADR 2026-10-08-cos-workspace-files-in-chat D2: files of the CoS thread workspace attached
    /// to this reply, with the workspace-relative path the reply may mention. Empty for others.
    #[serde(default)]
    pub workspace_files: Vec<ChatWorkspaceFile>,
    /// ADR 2026-10-27-relayed-instructions D1: the authenticated principal that posted a
    /// `role=user` message (`human:<name>` / `operator:<name>` / `mcp:<client_id>`). Absent for
    /// CoS / system messages and for messages posted before the record existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// ADR 2026-10-27-relayed-instructions D1: present when this `role=user` message is a
    /// person's instruction relayed by an external operator or an MCP client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<ChatRelay>,
    /// ADR 2026-10-28-single-human-window-general-cos D2: present on a card (札) of the general
    /// thread that stands for an item a project CoS routed to the person. `state` / `open` are
    /// read from the item every time (the card is resolved when the item is).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<Box<ChatEscalationRef>>,
    pub created_at: String,
    pub updated_at: String,
}
/// ADR 2026-10-27-relayed-instructions D1: the relay record of a `role=user` message. The text
/// of the message is the person's own words; this says who carried them, when the person said
/// them and where they apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRelay {
    /// The principal that relayed the instruction (same as the message `author`).
    pub relayed_by: String,
    /// RFC 3339: when the person said it (not after the post).
    pub said_at: String,
    pub scope: ChatRelayScope,
    /// ADR 2026-10-28-single-human-window-general-cos D3: the person's message of the general
    /// thread this relay forwards (`chat.relay_forward`). Absent for operator / MCP relays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ChatRelaySource>,
}
/// The original message a forwarded relay copies (D3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRelaySource {
    pub thread_id: String,
    pub message_id: String,
    /// The original message's `author` (`human:<name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}
/// D2: the item a card of the general thread stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatEscalationRef {
    /// `cos_inbox_items.id`.
    pub item_id: String,
    pub source_kind: String,
    /// The inbox item id (the card's id).
    pub source_key: String,
    /// The project CoS thread that holds the item (the card links to it).
    pub source_thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The item's current state.
    pub state: String,
    /// `pending` / `running` / `escalated` / `fallback`: still waiting.
    pub open: bool,
}
/// Where a relayed instruction applies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRelayScope {
    /// The thread's project; absent in the general CoS thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Tasks the instruction applies to; empty = the whole project / thread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_ids: Vec<String>,
    /// RFC 3339: after this the instruction is no longer a ground for `instructed_by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}
/// One workspace file attached to a CoS reply (`path` is relative to the thread workspace).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatWorkspaceFile {
    pub path: String,
    pub attachment_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// Nominal adapter usage; absent when the harness did not report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Box<crate::Usage>>,
    /// Observed Skill calls and reads of .claude/skills/**/SKILL.md (tool_use only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_reads: Option<u64>,
    /// First nonempty assistant text delta, using the dispatch clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_output_at: Option<String>,
    /// Start to finish wall time, in milliseconds; unknown for unfinished runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Start to first assistant text delta, in milliseconds; unknown without a delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_output_ms: Option<u64>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
/// ADR 2026-10-22-cos-fixed-threads D2: open the general CoS (`project_id: null`) or the CoS of
/// one project. Idempotent; nothing else creates a thread.
pub struct ChatOpenThreadRequest {
    pub project_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// D5 `POST /chat/threads/{t}/read`: raise the read watermark (never lowers it).
pub struct ChatReadThreadRequest {
    pub through_seq: u64,
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
/// ADR 2026-10-27-relayed-instructions D2: `POST /chat/threads/{t}/relays`. `text` is the
/// person's words verbatim; the relaying principal comes from the authentication, never from here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatRelayRequest {
    pub client_message_id: String,
    pub text: String,
    /// RFC 3339: when the person said it.
    pub said_at: String,
    /// Tasks the instruction applies to (must exist; at most `CHAT_RELAY_TASKS_MAX`).
    #[serde(default)]
    pub task_ids: Vec<String>,
    /// RFC 3339: optional expiry (must be after `said_at`).
    #[serde(default)]
    pub expires_at: Option<String>,
}
/// ADR 2026-10-28-single-human-window-general-cos D3: `POST /chat/threads/{t}/forwards`
/// (CoS operation `chat.relay_forward` only). No text: the forwarded text is the person's
/// message `message_id` of the general thread, verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatForwardRequest {
    pub client_message_id: String,
    /// The person's message of the general thread.
    pub message_id: String,
    /// Tasks of the target project the instruction applies to (1..=`CHAT_RELAY_TASKS_MAX`).
    pub task_ids: Vec<String>,
    /// RFC 3339 expiry (required, after now).
    pub expires_at: String,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRunListResponse {
    pub items: Vec<ChatRun>,
    pub next_before: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRunResponse {
    pub run: ChatRun,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatStopResponse {
    pub run: ChatRun,
    pub queue_paused: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// Test helper: the CoS thread of a project `key` (the project row is inserted if missing), so
/// tests that need several threads get one per key (ADR 2026-10-22-cos-fixed-threads D1).
#[cfg(test)]
pub(crate) fn test_project_thread(
    s: &crate::store::SqliteStore,
    key: &str,
    now: time::OffsetDateTime,
) -> ChatThread {
    let at = store::chat_ts(now);
    s.lock()
        .expect("lock")
        .execute(
            "INSERT OR IGNORE INTO projects(id,title,request,status,created_at,updated_at) \
             VALUES(?1,?1,'','active',?2,?2)",
            rusqlite::params![key, at],
        )
        .expect("project");
    s.cos_thread_open(
        &ChatOpenThreadRequest {
            project_id: Some(key.to_owned()),
        },
        now,
    )
    .expect("project thread")
    .thread
}
