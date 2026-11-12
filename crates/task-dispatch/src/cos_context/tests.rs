//! ADR 2026-11-11-cos-standing-context D2: the dispatcher fills the standing context of a CoS run
//! from the ledger, the KB and the org. Before this, CoS runs were built with
//! `..RunContext::default()` and no test looked at what they carried besides the thread.

use super::*;
use std::sync::Arc;
use task_core::authority::{self as a, GrantScope, ScopeKind};
use task_core::{Project, ProjectId, ProjectStatus, TaskStore};
use time::format_description::well_known::Rfc3339;

fn at(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).expect("time")
}

fn project(store: &SqliteStore, title: &str) -> String {
    let now = at("2026-11-01T00:00:00Z");
    let p = Project {
        id: ProjectId::new(),
        title: title.into(),
        request: "依頼".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        auto_integrate: false,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&p).expect("project");
    p.id.to_string()
}

/// A grant of the person (the controller operation is the simplest valid basis in the ledger).
fn grant(store: &SqliteStore, project: &str, note: &str, expires: &str, now: OffsetDateTime) {
    store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            SqliteStore::authority_grant_insert_tx(
                tx,
                &a::NewAuthorityGrant {
                    id: ulid::Ulid::new().to_string(),
                    project_id: project.to_owned(),
                    scope: GrantScope {
                        kind: ScopeKind::Project,
                        reference: project.to_owned(),
                    },
                    action_class: a::ActionClass::LaunchRoot,
                    source: a::GrantSource {
                        kind: a::SourceKind::HumanOperation,
                        reference: format!("project.controller_put:{project}:1"),
                    },
                    granted_by: "human:test".into(),
                    question_key: None,
                    note: note.into(),
                    expires_at: expires.into(),
                    budget: 5,
                },
                now,
            )?
            .expect("grant accepted");
            Ok(())
        })
        .expect("grant");
}

fn kb(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("kb");
    task_ops::knowledge::init(&root).expect("kb init");
    std::fs::create_dir_all(root.join("user")).expect("user dir");
    let memory = root.join(COS_MEMORY_PAGE);
    std::fs::create_dir_all(memory.parent().expect("memory dir")).expect("memory dir");
    std::fs::write(
        root.join("user/preferences.md"),
        "---\ntitle: 人の好み\n---\n# 人の好み\n結論を先に書く。\n",
    )
    .expect("page");
    std::fs::write(memory, "- Sirius の時刻は JST。\n").expect("memory");
    root
}

#[test]
fn cos_standing_context_reads_live_grants_kb_pages_and_scopes_per_thread_kind() {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("store"));
    let root = kb(dir.path());
    let now = at("2026-11-11T00:00:00Z");
    let p1 = project(&store, "BenchFS");
    let p2 = project(&store, "Pluvio");
    grant(
        &store,
        &p1,
        "job 待ちは延長してよい",
        "2027-11-01T00:00:00Z",
        at("2026-11-02T00:00:00Z"),
    );
    grant(
        &store,
        &p2,
        "Pluvio の起票",
        "2027-11-01T00:00:00Z",
        at("2026-11-03T00:00:00Z"),
    );
    grant(
        &store,
        &p1,
        "切れた許可",
        "2026-11-05T00:00:00Z",
        at("2026-11-04T00:00:00Z"),
    );

    let general =
        standing_context(&store, &root, Some(ChatThreadKind::General), None, now).expect("general");
    assert_eq!(general.role, CosThreadRole::General);
    // Every project's live grants, newest first; the expired one is not delegated any more.
    let notes: Vec<&str> = general.grants.iter().map(|g| g.note.as_str()).collect();
    assert_eq!(notes, ["Pluvio の起票", "job 待ちは延長してよい"]);
    assert!(
        general.grants[1]
            .source
            .starts_with("human_operation:project.controller_put:")
    );
    assert_eq!(general.grants[1].budget_remaining, Some(5));
    // `kb init` seeds its own `user/` pages; every page is from `user/`, ours among them.
    assert!(
        general
            .user_pages
            .iter()
            .all(|p| p.path.starts_with("user/"))
    );
    let prefs = general
        .user_pages
        .iter()
        .find(|p| p.path == "user/preferences.md")
        .expect("user/preferences.md");
    assert_eq!(prefs.title, "人の好み");
    assert!(prefs.body.contains("結論を先に書く") && !prefs.body.contains("title:"));
    assert_eq!(
        general.memory.as_ref().map(|m| m.path.as_str()),
        Some(COS_MEMORY_PAGE)
    );

    let project_thread =
        standing_context(&store, &root, Some(ChatThreadKind::Project), Some(&p1), now)
            .expect("project");
    assert_eq!(project_thread.role, CosThreadRole::Project);
    let notes: Vec<&str> = project_thread
        .grants
        .iter()
        .map(|g| g.note.as_str())
        .collect();
    assert_eq!(notes, ["job 待ちは延長してよい"]);

    let review =
        standing_context(&store, &root, Some(ChatThreadKind::Review), None, now).expect("review");
    assert_eq!(review.role, CosThreadRole::Review);
    assert_eq!(review.grants.len(), 2);

    // No KB on the host: the KB sections are empty, the run still gets its grants.
    let bare = standing_context(&store, &dir.path().join("none"), None, None, now).expect("bare");
    assert!(bare.user_pages.is_empty() && bare.memory.is_none());
    assert_eq!(bare.grants.len(), 2);
}

#[test]
fn cos_standing_run_context_carries_the_chat_and_session_only() {
    let chat = CosChatContext {
        thread_id: "thr".into(),
        standing: CosStandingContext {
            role: CosThreadRole::Project,
            ..CosStandingContext::default()
        },
        ..CosChatContext::default()
    };
    let ctx = cos_run_context(
        chat.clone(),
        Vec::new(),
        "sess",
        ChatRunSessionMode::Resumed,
    );
    assert_eq!(ctx.cos_chat, Some(chat));
    let session = ctx.session.as_ref().expect("session");
    assert!(session.resume && session.session_id == "sess");
    assert!(ctx.standing_rules.is_empty() && ctx.memory.is_none() && ctx.knowledge.is_none());
    let fresh = cos_run_context(
        CosChatContext::default(),
        Vec::new(),
        "s2",
        ChatRunSessionMode::Fresh,
    );
    assert!(!fresh.session.expect("session").resume);
}
