//! The context a CoS chat run reads besides its thread (ADR 2026-11-11-cos-standing-context D1/D2,
//! ADR 2026-10-27-relayed-instructions D4): the standing context (role, delegated grants, user
//! pages, CoS's memory page, KB entry) and the unexpired relays of the thread. Deterministic: it
//! reads the store, the KB files and the org; no LLM.

use std::path::Path;

use task_core::OrgDirectory;
use task_core::authority::grant_denial;
use task_core::chat::ChatRunSessionMode;
use task_core::chat::ChatThreadKind;
use task_core::store::SqliteStore;
use task_core::{MountKind, OrgKind};
use task_worker::protocol::{
    CosChatContext, CosChatRelayedInstruction, CosStandingContext, CosStandingGrant,
    CosStandingPage, CosThreadRole, RunContext, SessionHandle, SkillMount,
};
use time::OffsetDateTime;

/// The KB page of CoS's own (non-authority) memory (D1).
pub const COS_MEMORY_PAGE: &str = "cos/memory.md";
/// The KB directory of the user pages (D1).
const USER_DIR: &str = "user/";

/// D1 identity: the role follows the thread kind. A legacy thread is served like the general one.
pub fn thread_role(kind: Option<ChatThreadKind>) -> CosThreadRole {
    match kind {
        Some(ChatThreadKind::Project) => CosThreadRole::Project,
        Some(ChatThreadKind::Review) => CosThreadRole::Review,
        Some(ChatThreadKind::General | ChatThreadKind::Legacy) | None => CosThreadRole::General,
    }
}

/// D2: the standing context of one CoS chat run. `project` is the thread's project in a project
/// thread (only its grants), `None` elsewhere (every project's grants). An unreadable ledger fails
/// the launch (fail closed); a missing KB only leaves the KB sections empty.
pub fn standing_context(
    store: &SqliteStore,
    kb_root: &Path,
    kind: Option<ChatThreadKind>,
    project: Option<&str>,
    now: OffsetDateTime,
) -> Result<CosStandingContext, String> {
    let role = thread_role(kind);
    let projects: Vec<String> = match (role, project) {
        (CosThreadRole::Project, Some(id)) => vec![id.to_owned()],
        (CosThreadRole::Project, None) => Vec::new(),
        _ => store
            .project_list()
            .map_err(|e| format!("projects unavailable: {e}"))?
            .into_iter()
            .map(|p| p.id.to_string())
            .collect(),
    };
    let mut grants = Vec::new();
    for id in &projects {
        let ledger = store
            .authority_grants(id)
            .map_err(|e| format!("authority ledger unavailable: {e}"))?;
        for grant in ledger
            .into_iter()
            .filter(|g| grant_denial(g, now).is_none())
        {
            let source_excerpt = store
                .authority_source_excerpt(&grant.source)
                .map_err(|e| format!("authority ledger unavailable: {e}"))?;
            grants.push(CosStandingGrant {
                id: grant.id,
                project_id: grant.project_id,
                scope_kind: grant.scope.kind.as_str().to_owned(),
                scope_ref: grant.scope.reference,
                action_class: grant.action_class.as_str().to_owned(),
                expires_at: Some(grant.expires_at),
                budget_remaining: Some(grant.budget - grant.consumed),
                note: grant.note,
                source: format!("{}:{}", grant.source.kind.as_str(), grant.source.reference),
                source_excerpt,
                created_at: grant.created_at,
            });
        }
    }
    // Newest first; the renderer drops from the end.
    grants.sort_by(|a, b| (&b.created_at, &b.id).cmp(&(&a.created_at, &a.id)));
    let org = store
        .org_list()
        .map_err(|e| format!("organization unavailable: {e}"))?;
    let knowledge_scopes = org
        .iter()
        .find(|n| n.kind == OrgKind::Secretary)
        .map(|cos| task_core::resolve_profile(&org, &cos.id).knowledge)
        .unwrap_or_default()
        .into_iter()
        .map(|m| match m.kind {
            MountKind::Kb => m.scope.unwrap_or_default(),
            _ => m.label(),
        })
        .collect();
    let (user_pages, memory) = if task_ops::knowledge::exists(kb_root) {
        let user_pages = task_ops::knowledge::list_pages(kb_root)
            .into_iter()
            .filter(|p| p.starts_with(USER_DIR))
            .filter_map(|p| kb_page(kb_root, &p))
            .collect();
        (user_pages, kb_page(kb_root, COS_MEMORY_PAGE))
    } else {
        (Vec::new(), None)
    };
    Ok(CosStandingContext {
        role,
        grants,
        user_pages,
        memory,
        knowledge_scopes,
    })
}

/// One KB page without its front matter; the title is the front matter's, else the file name.
fn kb_page(root: &Path, path: &str) -> Option<CosStandingPage> {
    let raw = task_ops::knowledge::read_page(root, path)?;
    let (front, body) = task_core::knowledge::front_matter(&raw);
    let title = front
        .title
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned());
    Some(CosStandingPage {
        path: path.to_owned(),
        title,
        body: body.trim().to_owned(),
    })
}

/// ADR 2026-10-27-relayed-instructions D4: the unexpired relays of the thread.
pub fn relayed_instructions(
    store: &SqliteStore,
    thread_id: &str,
    now: OffsetDateTime,
) -> Result<Vec<CosChatRelayedInstruction>, String> {
    Ok(store
        .chat_relayed_instructions(thread_id, now)
        .map_err(|e| format!("relayed instructions unavailable: {e}"))?
        .into_iter()
        .filter_map(|message| {
            let relay = message.relay?;
            Some(CosChatRelayedInstruction {
                message_id: message.id,
                seq: i64::try_from(message.seq).unwrap_or(i64::MAX),
                text: message.text,
                relayed_by: relay.relayed_by,
                said_at: relay.said_at,
                task_ids: relay.scope.task_ids,
                expires_at: relay.scope.expires_at,
            })
        })
        .collect())
}

/// D2: the `RunContext` of a CoS chat run. Every field is written out: a CoS run carries its thread
/// context, the standing context and the mounted skills in `cos_chat`/`skills`/`session`, and none
/// of the task-run sections (node memory, standing rules, organization, …), whose CoS counterparts
/// are in `cos_chat.standing`.
pub fn cos_run_context(
    chat: CosChatContext,
    skills: Vec<SkillMount>,
    session_id: &str,
    mode: ChatRunSessionMode,
) -> RunContext {
    RunContext {
        browser: None,
        browser_policy: None,
        prior_review: Vec::new(),
        inputs: Vec::new(),
        answers: Vec::new(),
        review: None,
        role: None,
        children: Vec::new(),
        available_genres: Vec::new(),
        node: None,
        memory: None,
        conversation: Vec::new(),
        standing_rules: Vec::new(),
        organization: Vec::new(),
        clusters: Vec::new(),
        conversation_addressee: None,
        work_genre: None,
        recent_work: Vec::new(),
        milestone_review: None,
        subject_genre: None,
        workspace: None,
        comments: Vec::new(),
        interrupt: None,
        comments_enabled: false,
        profile: None,
        mode: None,
        knowledge: None,
        active_projects: Vec::new(),
        session: Some(SessionHandle {
            adapter: "claude-code".to_owned(),
            session_id: session_id.to_owned(),
            resume: mode == ChatRunSessionMode::Resumed,
        }),
        session_diff: Vec::new(),
        skills,
        continuation: None,
        work_unit: None,
        execution_planner: None,
        decision_requests: false,
        direct_route: None,
        cos_chat: Some(chat),
        routing_context_ref: None,
        input_attachments: Vec::new(),
        campaign: None,
    }
}

#[cfg(test)]
mod tests;
