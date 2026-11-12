//! `context.cos_chat.standing`: the standing context every CoS chat run reads (ADR 2026-11-11
//! cos-standing-context D1/D2). The dispatcher fills it from the authority ledger, the KB pages
//! `user/*.md` and [`COS_MEMORY_PAGE`], and the cos org profile; the worker renders it with the
//! limits of [`crate::cos_chat::standing`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// D1: the KB page of CoS's own (non-authority) memory. It lives in the existing category of
/// Celeris's own operating habits, so a KB candidate with [`COS_MEMORY_SCOPE`] and this path passes
/// the KB placement guard (`task_core::knowledge::layout::place`) that `knowledge.record` and the
/// curation run share. The dispatcher reads this page and the prompt names the same path.
pub const COS_MEMORY_PAGE: &str = "environment/celeris/cos-memory.md";
/// D1: the `scope` of a KB candidate that adds to [`COS_MEMORY_PAGE`].
pub const COS_MEMORY_SCOPE: &str = "environment/celeris";

/// The role of the thread the run serves (D1 identity). Chosen from the thread kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CosThreadRole {
    /// The general CoS thread (the person's single window), and a legacy thread.
    #[default]
    General,
    /// A project CoS thread.
    Project,
    /// The review (back) thread of the periodic review.
    Review,
}

impl CosThreadRole {
    pub fn as_str(self) -> &'static str {
        match self {
            CosThreadRole::General => "general",
            CosThreadRole::Project => "project",
            CosThreadRole::Review => "review",
        }
    }
}

/// One live grant of the authority ledger (D1 memory: what the person delegated).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosStandingGrant {
    pub id: String,
    pub project_id: String,
    pub scope_kind: String,
    pub scope_ref: String,
    pub action_class: String,
    /// RFC3339, or absent when the grant lasts until it is revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Uses left, or absent when the grant has no use limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_remaining: Option<i64>,
    pub note: String,
    /// `<kind>:<ref>` of the person's basis (`human_message:<message id>` …).
    pub source: String,
    /// The head of the person's own words, when readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_excerpt: Option<String>,
    pub created_at: String,
}

/// One KB page quoted into the standing context (`user/*.md`, `environment/celeris/cos-memory.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosStandingPage {
    /// KB-relative path (`user/preferences.md`).
    pub path: String,
    pub title: String,
    /// The body without front matter (the renderer applies the limits).
    pub body: String,
}

/// The standing context. Always present on a CoS chat run built by the dispatcher.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CosStandingContext {
    pub role: CosThreadRole,
    /// Live grants, newest first: the thread's project and the global ones in a project thread,
    /// every project and the global ones elsewhere.
    #[serde(default)]
    pub grants: Vec<CosStandingGrant>,
    /// `user/*.md` in path order. Empty when the KB has none or is not set up.
    #[serde(default)]
    pub user_pages: Vec<CosStandingPage>,
    /// `environment/celeris/cos-memory.md` (CoS's memory page), when it exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<CosStandingPage>,
    /// The KB scopes the cos org profile mounts (`user`, `projects`, …), for the search entry.
    #[serde(default)]
    pub knowledge_scopes: Vec<String>,
}
