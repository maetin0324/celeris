//! ADR 2026-11-11-cos-standing-context: the standing context is on every kind of CoS chat run and
//! each section stays within its limit. The existing CoS prompt tests check the Core and the
//! per-run sections; none of them looked for a standing context (CoS runs had none).

use super::*;
use crate::protocol::tests::sample_task;
use crate::protocol::{CosChatInboxItem, CosStandingGrant, RunContext};

fn grant(i: usize, note: &str) -> CosStandingGrant {
    CosStandingGrant {
        id: format!("01GRANT{i:03}"),
        project_id: "01PROJ".into(),
        scope_kind: "project".into(),
        scope_ref: "01PROJ".into(),
        action_class: "job_wait_extend".into(),
        expires_at: "2036-11-01T00:00:00Z".into(),
        budget_remaining: 999,
        note: note.into(),
        source: format!("human_message:msg{i}"),
        source_excerpt: Some(
            "job待ちに関しては基本認可するしかないので案件CoSで自動対応してください".into(),
        ),
        created_at: format!("2026-11-{:02}T00:00:00Z", 1 + i % 28),
    }
}

fn page(path: &str, body: &str) -> CosStandingPage {
    CosStandingPage {
        path: path.into(),
        title: path.rsplit('/').next().unwrap_or(path).into(),
        body: body.into(),
    }
}

fn standing(role: CosThreadRole) -> CosStandingContext {
    CosStandingContext {
        role,
        grants: vec![grant(1, "job 待ちの上限は延長してよい")],
        user_pages: vec![
            page("user/preferences.md", "結論を先に書く。"),
            page("user/profile.md", "筑波大の HPC の研究者。"),
        ],
        memory: Some(page(COS_MEMORY_PAGE, "- Sirius の qstat の時刻は JST。")),
        knowledge_scopes: vec!["user".into(), "projects".into(), "memory".into()],
    }
}

fn chat(role: CosThreadRole, inbox: bool) -> CosChatContext {
    CosChatContext {
        thread_id: "thr1".into(),
        run_id: "crun1".into(),
        policy_version: "10".into(),
        credential_env: "CELERIS_COS_RUN_CREDENTIAL".into(),
        api_base_url: "http://127.0.0.1:7720/api/v1".into(),
        inbox_items: if inbox {
            vec![CosChatInboxItem {
                triage_match: None,
                item_id: "i1".into(),
                source_kind: "question".into(),
                source_key: "q1".into(),
                source_revision: "r1".into(),
                state: "pending".into(),
                summary: "job 待ちの上限".into(),
                detail: String::new(),
                reason: None,
                created_at: "2026-11-11T00:00:00Z".into(),
                decision: None,
                answer_path: None,
                thread_id: None,
            }]
        } else {
            Vec::new()
        },
        standing: standing(role),
        ..CosChatContext::default()
    }
}

fn prompt_of(chat: &CosChatContext) -> crate::cos_chat::CosChatPrompt {
    let context = RunContext {
        cos_chat: Some(chat.clone()),
        ..RunContext::default()
    };
    crate::cos_chat::build_parts(&sample_task(), &context, chat, "wrun1", "artifacts")
}

/// The section between the standing heading and the next `## ` heading.
fn standing_of(variable: &str) -> &str {
    let start = variable
        .find("## 常設の文脈")
        .expect("standing context in the prompt");
    let rest = &variable[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |i| i + 4);
    &rest[..end]
}

#[test]
fn cos_standing_every_thread_kind_gets_all_sections_within_limits() {
    let cases = [
        (CosThreadRole::General, false, "role=general", "統括 CoS"),
        (
            CosThreadRole::Project,
            false,
            "role=project",
            "この案件の CoS",
        ),
        (
            CosThreadRole::Review,
            false,
            "role=review",
            "定期見直しの CoS",
        ),
        (
            CosThreadRole::Project,
            true,
            "role=project",
            "受信箱の一次対応",
        ),
    ];
    let mut cores = Vec::new();
    for (role, inbox, marker, identity) in cases {
        let parts = prompt_of(&chat(role, inbox));
        let section = standing_of(&parts.variable);
        assert!(section.contains(marker), "{section}");
        assert!(section.contains(identity), "{section}");
        assert!(
            section.contains("grant `01GRANT001`") && section.contains("human_message:msg1"),
            "grant id and the person's basis: {section}"
        );
        assert!(section.contains("「job待ちに関しては基本認可"), "{section}");
        assert!(
            section.contains("期限 2036-11-01T00:00:00Z、残り 999 回"),
            "{section}"
        );
        assert!(section.contains("user/preferences.md") && section.contains("結論を先に書く"));
        assert!(section.contains("Sirius の qstat の時刻は JST"));
        assert!(section.contains("`GET /api/v1/knowledge/tree?q=<語>&scope=<scope>`"));
        assert!(section.contains("`user`・`projects`・`memory`"));
        assert!(
            section.contains("grants=1/") && section.contains("user=2/"),
            "{section}"
        );
        assert!(section.contains("memory=1/") && section.contains("kb=3/"));
        assert!(chars(section) <= TOTAL_MAX + 200, "{}", chars(section));
        // The Core stays byte-identical whatever the thread (ADR 2026-10-08 D6).
        cores.push(parts.core);
    }
    assert!(cores.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn cos_standing_empty_sources_still_render_every_section() {
    let mut chat = chat(CosThreadRole::General, false);
    chat.standing = CosStandingContext::default();
    let section = standing_section(&chat);
    assert!(section.contains("<!-- celeris:cos-standing v1 role=general grants=0/"));
    for heading in [
        "### 役割",
        "### 任された範囲",
        "### 利用者",
        "### CoS の覚え書き",
        "### KB の入口",
    ] {
        assert!(section.contains(heading), "{heading}: {section}");
    }
    assert!(section.contains("生きている grant は無い"));
    assert!(section.contains("（まだ無い）"));
}

#[test]
fn cos_standing_overflow_drops_the_oldest_and_says_how_many() {
    let mut chat = chat(CosThreadRole::General, true);
    let long = "長い説明".repeat(200);
    chat.standing.grants = (0..60).map(|i| grant(i, &long)).collect();
    chat.standing.user_pages = (0..30)
        .map(|i| page(&format!("user/p{i:02}.md"), &"好み".repeat(800)))
        .collect();
    let memory = format!("{}- 最新の覚え書き\n", "- 古い覚え書き\n".repeat(400));
    chat.standing.memory = Some(page(COS_MEMORY_PAGE, &memory));
    let section = standing_section(&chat);
    let grants = grants_section(&chat.standing);
    assert!(chars(&grants.text) <= GRANTS_MAX, "{}", chars(&grants.text));
    assert!(grants.omitted > 0 && grants.items + grants.omitted == 60);
    // Newest first: the first grant is kept, the last ones are dropped.
    assert!(grants.text.contains("01GRANT000") && !grants.text.contains("01GRANT059"));
    assert!(grants.text.contains(&format!("ほか {} 件", grants.omitted)));
    let user = user_section(&chat.standing);
    assert!(chars(&user.text) <= USER_MAX && user.omitted > 0);
    let memory = memory_section(chat.standing.memory.as_ref());
    assert!(chars(&memory.text) <= MEMORY_MAX, "{}", chars(&memory.text));
    assert!(memory.text.contains("最新の覚え書き") && memory.text.contains("字を省略"));
    let role = role_section(&chat.standing, true);
    assert!(chars(&role.text) <= ROLE_MAX);
    let marker_len = section.lines().nth(1).map_or(0, chars);
    assert!(
        chars(&section) <= TOTAL_MAX + marker_len + 40,
        "{}",
        chars(&section)
    );
    assert!(section.contains(&format!("omitted={}", grants.omitted + user.omitted + 1)));
}

/// D1: the way the memory section tells CoS to add a note is one the KB accepts. Every writer of the
/// KB (`knowledge.record`, `celerisctl knowledge record`, the curation run's `apply_candidates`)
/// goes through `task_core::knowledge::layout::place`, so the scope and path named in the prompt
/// must pass it and land on the page the dispatcher reads. The earlier page `cos/memory.md` with
/// scope `cos` was refused there, and no prompt test looked at the guard.
#[test]
fn cos_standing_memory_note_route_passes_the_kb_placement_guard() {
    use task_core::knowledge::layout::{Layout, PlacementRequest, place};
    let section = memory_section(None);
    assert!(
        section
            .text
            .contains(&format!("\"scope\":\"{COS_MEMORY_SCOPE}\""))
    );
    assert!(
        section
            .text
            .contains(&format!("\"path\":\"{COS_MEMORY_PAGE}\""))
    );
    // The API knows the projects; `celerisctl knowledge record` does not.
    for projects in [Some(Vec::new()), None] {
        let layout = Layout::new(projects, Vec::<String>::new(), Vec::new(), "2026-11-12");
        let req = PlacementRequest {
            op: None,
            path: Some(COS_MEMORY_PAGE),
            scope: Some(COS_MEMORY_SCOPE),
            title: "CoS の覚え書き",
            tags: &[],
        };
        let placed = place(&req, &layout).expect("the KB guard accepts the memory note route");
        assert_eq!(placed.path, COS_MEMORY_PAGE);
        assert_eq!(placed.scope.as_deref(), Some("environment"));
    }
}

/// ADR 2026-11-12-cos-triage-scope D5: a run with inbox items reads what a person keeps and how CoS
/// answers the rest in the standing context, without opening the skill. The other tests check the
/// sections' limits but not that the policy summary is there (and only for inbox runs).
#[test]
fn cos_standing_inbox_run_carries_the_triage_policy_summary() {
    assert!(
        chars(TRIAGE_POLICY) <= POLICY_MAX,
        "{}",
        chars(TRIAGE_POLICY)
    );
    for needle in [
        "人に残す件",
        "to=operator",
        "task_scope",
        "credential の入力",
        "本番 config",
    ] {
        assert!(TRIAGE_POLICY.contains(needle), "{needle}");
    }
    let with_inbox = prompt_of(&chat(CosThreadRole::Project, true));
    assert!(standing_of(&with_inbox.variable).contains(TRIAGE_POLICY));
    let without = prompt_of(&chat(CosThreadRole::Project, false));
    assert!(!standing_of(&without.variable).contains("受信箱の一次対応の方針"));
}
