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
    assert_eq!(general.grants[1].budget_remaining, 5);
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

/// ADR 2026-11-11-cos-standing-context D4: a standing forward is listed with its grant while the
/// grant backs it, and leaves the list once the person revokes the grant, by the rule the forward
/// passed (`forward_gap`). Before, the list read only the relay's expiry (a standing forward has
/// none), so a revoked grant's forward stayed in the prompt as a standing instruction.
#[test]
fn cos_standing_relayed_list_follows_the_grant_of_a_standing_forward() {
    use task_core::chat::{ChatForwardRequest, ChatOpenThreadRequest, ChatPostMessageRequest};
    let store = SqliteStore::open_in_memory().expect("store");
    let now = at("2026-11-11T00:00:00Z");
    let project = project(&store, "BenchFS");
    let open = |project_id: Option<String>| {
        store
            .cos_thread_open(&ChatOpenThreadRequest { project_id }, now)
            .expect("thread")
            .thread
            .id
    };
    let (general, project_thread) = (open(None), open(Some(project)));
    let said = store
        .chat_message_post_as(
            &general,
            &ChatPostMessageRequest {
                client_message_id: "h-standing".into(),
                text: "job待ちは常に延長してください".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: task_core::chat::ChatSendMode::Queue,
                resume_queue: false,
            },
            "human:web",
            now,
        )
        .expect("post")
        .response
        .message
        .id;
    let grant = store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            let new = a::NewAuthorityGrant {
                id: ulid::Ulid::new().to_string(),
                project_id: a::GLOBAL_LEDGER.into(),
                scope: GrantScope {
                    kind: ScopeKind::Global,
                    reference: a::GLOBAL_LEDGER.into(),
                },
                action_class: a::ActionClass::JobWaitExtend,
                source: a::GrantSource {
                    kind: a::SourceKind::HumanMessage,
                    reference: said.clone(),
                },
                granted_by: "human:web".into(),
                question_key: None,
                note: "job 待ちの上限は延長する".into(),
                expires_at: "2036-11-01T00:00:00Z".into(),
                budget: 1000,
            };
            Ok(SqliteStore::authority_grant_insert_tx(tx, &new, now)?.expect("accepted"))
        })
        .expect("grant")
        .id;
    let forward = ChatForwardRequest {
        client_message_id: "forward:standing".into(),
        message_id: said,
        task_ids: vec![],
        expires_at: None,
        grant_id: Some(grant.clone()),
    };
    let by = format!("cos:{general}");
    store
        .chat_relay_forward(&general, &project_thread, &forward, &by, now)
        .expect("standing forward");

    let listed = relayed_instructions(&store, &project_thread, now).expect("list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].grant_id.as_deref(), Some(grant.as_str()));
    assert!(listed[0].task_ids.is_empty() && listed[0].expires_at.is_none());

    store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            SqliteStore::authority_grant_revoke_tx(tx, a::GLOBAL_LEDGER, &grant, "human:web", now)
        })
        .expect("revoke")
        .expect("revoked");
    let listed = relayed_instructions(&store, &project_thread, now).expect("list");
    assert!(
        listed.is_empty(),
        "a revoked grant's forward is listed: {listed:?}"
    );
}

/// ADR 2026-11-12-cos-triage-scope D4: the daemon's match is computed from the store only (no LLM,
/// no clock but the one passed) and is the same for the same items. `task_ops::triage_match` tests
/// the rules; this pins that each item handed to a CoS run gets its line.
#[test]
fn cos_triage_match_lines_are_deterministic_store_reads() {
    use task_worker::protocol::CosChatInboxItem;
    let store = task_core::store::SqliteStore::open_in_memory().expect("store");
    let now = time::OffsetDateTime::from_unix_timestamp(1_794_441_600).expect("time");
    let item = |kind: &str, detail: &str| CosChatInboxItem {
        item_id: format!("i-{kind}"),
        source_kind: kind.into(),
        thread_id: Some("no-such-thread".into()),
        summary: "件".into(),
        detail: detail.into(),
        ..CosChatInboxItem::default()
    };
    let items = vec![
        item(
            "authorization",
            "クラスタ `pegasus` の job の待ちが上限（3600 秒）に達しました: 1 Q",
        ),
        item("credential_wait", ""),
        item("notice", ""),
    ];
    let lines = |items: Vec<CosChatInboxItem>| -> Vec<Option<String>> {
        super::with_triage_match(&store, items, now)
            .expect("match")
            .into_iter()
            .map(|i| i.triage_match)
            .collect()
    };
    let first = lines(items.clone());
    assert_eq!(
        first,
        vec![
            Some("job_wait_extend の候補".to_string()),
            Some("人に残す件（credential_input）。to=human で回す".to_string()),
            None,
        ]
    );
    assert_eq!(lines(items), first);
}
