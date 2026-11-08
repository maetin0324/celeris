//! ADR 2026-11-07-health-review-integration 付記 (b) B3・B4 の試験（prefix `change_rules_worker_profile_`）: worker の
//! run（atomic task の execute・WU の run）と planner の run の実際の prompt（`build_prompt`。全 harness が同じ
//! `prompt_header` を通る）に、`RunContext.workspace.review_policy` から repo 固有の規約の節が 1 回出る。どの run に方針を
//! 入れるかは dispatcher が決める（task-dispatch の `health_review_worker_profile_`）。

use task_core::TaskKind;

use super::tests::{CLASS, policy, prompt_for, tree_planner, workspace_with};
use super::{REPO_RULES_HEADING, repo_section, section};
use crate::claude_code::build_cos_chat_parts;
use crate::protocol::{
    ConversationAddressee, CosChatContext, ReviewRequest, RunContext, RunWorkspaceContext,
    WorkUnitPromptContext,
};

const NOTE: &str = "この案件のコードは `/work/repo` にある。";

/// atomic task の execute・WU の run・/3 の planner の run の文脈（作業場所の情報は `workspace`）。
fn worker_and_planner_runs(
    workspace: Option<RunWorkspaceContext>,
) -> Vec<(&'static str, RunContext)> {
    vec![
        (
            "atomic execute",
            RunContext {
                workspace: workspace.clone(),
                ..RunContext::default()
            },
        ),
        (
            "work unit",
            RunContext {
                work_unit: Some(WorkUnitPromptContext {
                    key: "impl".to_string(),
                    objective: "wu objective".to_string(),
                    ..WorkUnitPromptContext::default()
                }),
                workspace: workspace.clone(),
                ..RunContext::default()
            },
        ),
        (
            "tree planner",
            RunContext {
                execution_planner: Some(tree_planner()),
                workspace,
                ..RunContext::default()
            },
        ),
    ]
}

/// B4: 方針のある worker・planner の run の prompt には、汎用規約の直後に repo 固有の節（高 risk の class・不変条件）が
/// ちょうど 1 回、Objective の前に出る。作業場所の説明（`workspace.note`）も同じ欄から前置きに出る。
#[test]
fn change_rules_worker_profile_once_in_worker_and_planner_prompts() {
    let workspace = Some(RunWorkspaceContext {
        note: Some(NOTE.to_string()),
        review_policy: Some(policy()),
    });
    for (name, context) in worker_and_planner_runs(workspace) {
        let prompt = prompt_for(TaskKind::Execute, &[], &context);
        let repo = repo_section(&context);
        assert!(repo.starts_with(REPO_RULES_HEADING), "{name}: {repo}");
        assert_eq!(
            prompt.matches(&format!("{}{repo}", section())).count(),
            1,
            "{name}: {prompt}"
        );
        let heading_line = format!("\n{REPO_RULES_HEADING}\n");
        assert_eq!(prompt.matches(&heading_line).count(), 1, "{name}: {prompt}");
        assert_eq!(
            prompt.matches(&format!("`{CLASS}`")).count(),
            1,
            "{name}: {prompt}"
        );
        assert!(prompt.contains(NOTE), "{name}: {prompt}");
        let at = |needle: &str| {
            prompt
                .find(needle)
                .unwrap_or_else(|| panic!("{name}: no `{needle}`: {prompt}"))
        };
        assert!(at(&heading_line) < at("## Objective\n"), "{name}: {prompt}");
    }
}

/// B3・B5: 方針の無い run（`Absent`・`Unavailable` で `review_policy = None`、作業場所の情報自体が無い run）には repo
/// 固有の節が出ない。reviewer の run（dispatcher は `workspace` を入れない）にも出ない。CoS chat は `prompt_header` を
/// 通らないので、文脈に方針があっても節は出ない（D1.3）。
#[test]
fn change_rules_worker_profile_absent_without_a_policy() {
    let note_only = Some(RunWorkspaceContext {
        note: Some(NOTE.to_string()),
        review_policy: None,
    });
    let runs = worker_and_planner_runs(note_only)
        .into_iter()
        .chain(worker_and_planner_runs(None));
    for (name, context) in runs {
        let prompt = prompt_for(TaskKind::Execute, &[], &context);
        assert!(!prompt.contains(REPO_RULES_HEADING), "{name}: {prompt}");
        assert!(!prompt.contains(CLASS), "{name}: {prompt}");
        assert_eq!(
            prompt.contains(NOTE),
            context.workspace.is_some(),
            "{name}: {prompt}"
        );
    }

    let review = RunContext {
        review: Some(ReviewRequest::default()),
        ..RunContext::default()
    };
    let prompt = prompt_for(TaskKind::Review, &[], &review);
    assert!(!prompt.contains(REPO_RULES_HEADING), "{prompt}");

    let chat = RunContext {
        cos_chat: Some(CosChatContext {
            thread_id: "thread-1".to_string(),
            run_id: "chat-run-1".to_string(),
            ..CosChatContext::default()
        }),
        conversation_addressee: Some(ConversationAddressee::Secretary),
        workspace: workspace_with(Some(policy())),
        ..RunContext::default()
    };
    let task = crate::protocol::tests::sample_task();
    let parts = build_cos_chat_parts(&task, &chat, "chat-run-1", "artifacts")
        .unwrap_or_else(|| panic!("a CoS chat run"));
    assert!(!parts.core.contains(REPO_RULES_HEADING), "{}", parts.core);
    assert!(
        !parts.variable.contains(REPO_RULES_HEADING),
        "{}",
        parts.variable
    );
}
