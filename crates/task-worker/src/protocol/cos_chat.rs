//! `context.cos_chat`: the input of a CoS chat run (ADR 2026-10-05 cos-chat-home D2/D3/D4,
//! ADR 2026-10-06 cos-chat-run-dispatch).
//!
//! A CoS chat run is not a stored task. The dispatcher builds a transient [`Task`] with
//! [`CosChatContext::transient_task`] only because [`super::RunRequest::task`] is required, and the
//! worker side tells the two kinds of run apart by [`is_cos_chat_run`] (the presence of
//! `context.cos_chat`), never by a title or label a task could imitate.
//!
//! The run credential's **value** is never part of this type: only the name of the environment
//! variable that carries it ([`CosChatContext::credential_env`]) is, so `request.json`, the prompt
//! and the run log cannot contain it.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{Budget, Status, Task, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec};
use time::OffsetDateTime;

use super::RunRequest;

/// The environment variable the daemon sets to the run credential (ADR 2026-10-05 D3).
/// `celerisctl` reads the same name (`commands::cos_ops::CREDENTIAL_ENV`).
pub const COS_RUN_CREDENTIAL_ENV: &str = "CELERIS_COS_RUN_CREDENTIAL";

/// Media types the harness may receive as a native image (D4: the raster types with a safe
/// decoder). Everything else is delivered as a file path.
pub const COS_CHAT_IMAGE_MEDIA_TYPES: &[&str] =
    &["image/jpeg", "image/png", "image/webp", "image/gif"];

/// `context.cos_chat`. Present only on CoS chat runs started by the dispatcher.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatContext {
    /// `chat_threads.id`.
    pub thread_id: String,
    /// `chat_runs.id` (distinct from the worker run id in the prompt header).
    pub run_id: String,
    /// The input messages this run must handle, in delivery order (an interrupt first).
    /// Delivery is recorded by `chat_runs.input_message_id` and the message state, not here.
    pub inputs: Vec<CosChatInput>,
    /// The thread summary written by an earlier CoS run, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The last seq the summary covers (0 = no summary yet). Independent of the delivery cursor.
    #[serde(default)]
    pub summary_through_seq: i64,
    /// History after `summary_through_seq` that the summary does not cover yet.
    #[serde(default)]
    pub unsummarized: CosChatHistory,
    /// Delivery cursor of a resumed session (ADR 2026-10-05 D2 付記): the harness session already
    /// holds the thread through this seq. Then `summary` is omitted (it is in the session) and
    /// `unsummarized` holds only the messages after this seq. Absent = full delivery (new, fresh,
    /// fresh retry, restart recovery, or an unknown cursor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_through_seq: Option<i64>,
    /// Attachments staged read-only under `<workspace>/attachments` (D4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<CosChatAttachment>,
    /// Abilities confirmed by the active adapter for this run. Absent means unconfirmed, not
    /// that a configured harness can already deliver an image or use a tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_capabilities: Option<super::super::cos_chat::HarnessCapabilities>,
    /// Names of the skills mounted for this run (`cos-operator`, `cos-inbox-triage`, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Name of the environment variable holding the run credential. Never the value.
    pub credential_env: String,
    /// Base URL of the Celeris API ending in `/api/v1` (for checkpoint and history requests).
    pub api_base_url: String,
    /// ADR 2026-10-07-cos-inbox-thread-conversation D3: the unresolved inbox items (newest first)
    /// when this run is in the inbox thread. Empty in every other thread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inbox_items: Vec<CosChatInboxItem>,
}

/// One unresolved item of the CoS inbox (`cos_inbox_items`), as context for a run in the inbox
/// thread (ADR 2026-10-07-cos-inbox-thread-conversation D3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatInboxItem {
    /// `cos_inbox_items.id` (the triage item; `/cos/inbox/{i}/resolve` while `pending`/`running`).
    pub item_id: String,
    /// `decision` / `question` / `authorization` / `plan_gate` / `phase_gate` / `notice` / …
    pub source_kind: String,
    /// The id of the original wait (the human inbox item id for inbox sources).
    pub source_key: String,
    pub source_revision: String,
    /// `escalated` (waiting on the person), `fallback` (handed over without CoS), `pending` /
    /// `running` (not judged yet).
    pub state: String,
    /// The title of the wait as ingested.
    pub summary: String,
    /// CoS's recorded reason for the outcome, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_at: String,
    /// What the person must decide (from the escalation / fallback packet), if routed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<CosChatInboxDecision>,
    /// Same-app API path a human instruction is relayed to through `/cos/operations`
    /// (`POST /api/v1/inbox/items/{source_key}/answer`); absent for notices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_path: Option<String>,
}

/// The decision packet of a routed item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatInboxDecision {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<CosChatInboxOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation_reason: Option<String>,
    pub web_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatInboxOption {
    pub key: String,
    pub label: String,
}

/// One message delivered to the run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatInput {
    /// `chat_messages.id`.
    pub id: String,
    pub seq: i64,
    pub text: String,
    /// `true` when the human sent it with `mode = interrupt`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt: bool,
    /// Ids of the attachments sent with this message (details in `cos_chat.attachments`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ids: Vec<String>,
}

/// An explicit range of the thread history. `messages` may hold fewer seqs than the range
/// (a size budget); the missing seqs are named in the prompt, never dropped silently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatHistory {
    /// First seq of the range (inclusive). `from_seq > through_seq` means an empty range.
    pub from_seq: i64,
    /// Last seq of the range (inclusive).
    pub through_seq: i64,
    /// The messages handed over, in seq order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<CosChatHistoryMessage>,
}

/// A message of [`CosChatHistory`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatHistoryMessage {
    pub id: String,
    pub seq: i64,
    /// `user` / `assistant` / `system`.
    pub role: String,
    pub text: String,
}

/// How an attachment reaches the harness (D4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CosChatDelivery {
    /// A native image input of the harness (or the path and an image-reading tool).
    Image,
    /// A path plus the manifest; the CoS picks the tool.
    #[default]
    File,
}

impl CosChatDelivery {
    /// `image` for the safely decodable raster types, `file` for everything else.
    pub fn for_media_type(media_type: &str) -> Self {
        let mt = media_type.trim().to_ascii_lowercase();
        if COS_CHAT_IMAGE_MEDIA_TYPES.contains(&mt.as_str()) {
            Self::Image
        } else {
            Self::File
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
        }
    }
}

/// One entry of the attachment manifest (D4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosChatAttachment {
    pub id: String,
    /// Original name (display only).
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    /// Lowercase hex SHA-256, verified by the daemon when staging.
    pub sha256: String,
    /// Absolute path of the read-only staged copy (`<workspace>/attachments/<id>/<name>`).
    pub path: PathBuf,
    pub delivery: CosChatDelivery,
}

/// `true` when the request is a CoS chat run (decided only by `context.cos_chat`).
pub fn is_cos_chat_run(request: &RunRequest) -> bool {
    request.context.cos_chat.is_some()
}

impl CosChatHistory {
    /// The seq ranges inside `from_seq..=through_seq` that `messages` does not carry.
    pub fn missing_ranges(&self) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        if self.from_seq > self.through_seq {
            return out;
        }
        let mut next = self.from_seq;
        let mut seqs: Vec<i64> = self
            .messages
            .iter()
            .map(|m| m.seq)
            .filter(|s| (self.from_seq..=self.through_seq).contains(s))
            .collect();
        seqs.sort_unstable();
        seqs.dedup();
        for seq in seqs {
            if seq > next {
                out.push((next, seq - 1));
            }
            next = seq + 1;
        }
        if next <= self.through_seq {
            out.push((next, self.through_seq));
        }
        out
    }
}

impl CosChatContext {
    /// The transient task value a CoS chat run carries in `RunRequest.task`. The dispatcher never
    /// stores it; its id is fresh per run and nothing reads its status.
    pub fn transient_task(&self, workspace: &Path, budget: Budget, now: OffsetDateTime) -> Task {
        Task {
            id: TaskId::new(),
            requirements: Default::default(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: format!("CoS chat {}", self.thread_id),
            objective: format!(
                "Handle the CoS chat thread {} (chat run {}).",
                self.thread_id, self.run_id
            ),
            acceptance: Vec::new(),
            inputs: Vec::new(),
            depends_on: Vec::new(),
            status: Status::Running,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Frontier,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: workspace.to_path_buf(),
                mode: None,
            },
            budget,
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
            tree: None,
            paused_at: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
        }
    }
}
