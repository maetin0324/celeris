//! ADR 2026-11-21-chat-discussion-mode D1.1: the discussion mode a person picks for one message
//! (`discuss` / `propose` / `execute`), recorded only in `chat_messages.metadata_json`. The mode of
//! a CoS run is derived from these records by [`crate::discussion_mode`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// D1.1: what a person allows the CoS run that answers this message to do. Distinct from
/// [`super::ChatSendMode`] (the queue handling of a send).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatDiscussionMode {
    /// Talk and read only: every `/cos/operations` action is refused.
    Discuss,
    /// Talk and leave unapproved drafts as reply files: every `/cos/operations` action is refused.
    Propose,
    /// Today's behaviour: the existing checks of `/cos/operations` decide.
    Execute,
}

/// The `metadata_json` key of [`ChatDiscussionMode`].
pub(crate) const METADATA_KEY: &str = "discussion_mode";

impl ChatDiscussionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discuss => "discuss",
            Self::Propose => "propose",
            Self::Execute => "execute",
        }
    }

    /// D1.1: the value written to `metadata_json` — only discuss and propose; an absent field and
    /// `execute` write nothing, so such a post is byte-identical to one made before the field existed.
    pub(crate) fn recorded(mode: Option<Self>) -> Option<Self> {
        mode.filter(|m| *m != Self::Execute)
    }

    /// The recorded mode of a message's `metadata_json` (`None` when absent). A value outside the
    /// three words is read as `discuss`: an unreadable record must not widen what CoS may do.
    pub(crate) fn of_metadata(meta: &Value) -> Option<Self> {
        meta.get(METADATA_KEY).filter(|v| !v.is_null()).map(|v| {
            serde_json::from_value(v.clone())
                .ok()
                .filter(|m| *m != Self::Execute)
                .unwrap_or(Self::Discuss)
        })
    }
}
