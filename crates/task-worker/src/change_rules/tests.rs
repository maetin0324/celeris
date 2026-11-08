//! ADR 2026-11-07-health-review-integration D1.1〜D1.5・D3.4 の試験（prefix `change_rules_generic_`・
//! `change_rules_planner_`）。規約の本文は `GENERIC_RULES` から読み、ここには写さない。
//! harness ごとの配布（Claude Code・Codex・Pi・ACP の実際の入力）は各 adapter の試験 file にある
//! （`change_rules_generic_reaches_*`）。

use std::collections::BTreeMap;

use task_core::TaskKind;
use task_core::design_review::{PathClass, ReviewPolicy};

use super::*;
use crate::claude_code::{build_cos_chat_parts, build_prompt};
use crate::protocol::{
    CosChatContext, ExecutionPlannerContext, ReviewRequest, RunContext, RunWorkspaceContext,
    TreePlannerContext, WorkUnitPromptContext,
};
use crate::test_support::{assert_generic_rules_once, repo_claude_md};

pub(super) const CLASS: &str = "dispatch-state";
const CLASS_PATH: &str = "^crates/task-dispatch/src/dispatcher/";
const INVARIANT: &str = "events-append-only";

pub(super) fn policy() -> ReviewPolicy {
    ReviewPolicy {
        owner_map: Some("docs/architecture-map.md".to_string()),
        high_risk: vec![PathClass {
            class: CLASS.to_string(),
            paths: vec![CLASS_PATH.to_string(), "^crates/task-store/".to_string()],
        }],
        invariants: BTreeMap::from([(INVARIANT.to_string(), "CLAUDE.md".to_string())]),
        definition_sha256: "d".to_string(),
    }
}

pub(super) fn tree_planner() -> ExecutionPlannerContext {
    ExecutionPlannerContext {
        max_work_units: 8,
        tree: Some(TreePlannerContext {
            depth: 1,
            max_depth: 3,
            remaining_depth: 2,
            ..TreePlannerContext::default()
        }),
        ..ExecutionPlannerContext::default()
    }
}

/// 付記 B1: dispatcher が run の作業場所の情報に入れる review 方針。
pub(super) fn workspace_with(review_policy: Option<ReviewPolicy>) -> Option<RunWorkspaceContext> {
    Some(RunWorkspaceContext {
        note: None,
        review_policy,
    })
}

pub(super) fn planner_context(
    planner: ExecutionPlannerContext,
    policy: Option<ReviewPolicy>,
) -> RunContext {
    RunContext {
        execution_planner: Some(planner),
        workspace: workspace_with(policy),
        ..RunContext::default()
    }
}

pub(super) fn prompt_for(kind: TaskKind, labels: &[&str], context: &RunContext) -> String {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = kind;
    task.labels = labels.iter().map(|l| l.to_string()).collect();
    build_prompt(&task, context, "run-change-rules", "artifacts")
}

fn planner_prompt(planner: ExecutionPlannerContext, policy: Option<ReviewPolicy>) -> String {
    prompt_for(TaskKind::Execute, &[], &planner_context(planner, policy))
}

/// D1.2・I1: execute（task・WU）・plan・案件計画・案件 replan・execution plan（/1・/3）・その replan・review の
/// どの run kind の prompt にも、汎用規約がちょうど 1 回、前置きの後・Objective の前に入る。
#[test]
fn change_rules_generic_once_in_every_run_kind() {
    let milestones = task_core::MILESTONES_PLAN_LABEL;
    let replan = task_core::MILESTONES_REPLAN_LABEL;
    let flat = ExecutionPlannerContext {
        max_work_units: 8,
        ..ExecutionPlannerContext::default()
    };
    let planner = |p: ExecutionPlannerContext| RunContext {
        execution_planner: Some(p),
        ..RunContext::default()
    };
    let cases: Vec<(&str, TaskKind, Vec<&str>, RunContext)> = vec![
        ("execute", TaskKind::Execute, vec![], RunContext::default()),
        (
            "work unit",
            TaskKind::Execute,
            vec![],
            RunContext {
                work_unit: Some(WorkUnitPromptContext {
                    objective: "wu objective".to_string(),
                    ..WorkUnitPromptContext::default()
                }),
                ..RunContext::default()
            },
        ),
        ("plan", TaskKind::Plan, vec![], RunContext::default()),
        (
            "project plan",
            TaskKind::Plan,
            vec![milestones],
            RunContext::default(),
        ),
        (
            "project replan",
            TaskKind::Plan,
            vec![milestones, replan],
            RunContext::default(),
        ),
        (
            "execution plan",
            TaskKind::Execute,
            vec![],
            planner(flat.clone()),
        ),
        (
            "execution replan",
            TaskKind::Execute,
            vec![],
            planner(ExecutionPlannerContext {
                replan: true,
                ..flat
            }),
        ),
        (
            "tree plan",
            TaskKind::Execute,
            vec![],
            planner(tree_planner()),
        ),
        (
            "tree replan",
            TaskKind::Execute,
            vec![],
            planner_context(
                ExecutionPlannerContext {
                    replan: true,
                    ..tree_planner()
                },
                Some(policy()),
            ),
        ),
        (
            "review",
            TaskKind::Review,
            vec![],
            RunContext {
                review: Some(ReviewRequest::default()),
                ..RunContext::default()
            },
        ),
    ];
    for (name, kind, labels, context) in cases {
        let prompt = prompt_for(kind, &labels, &context);
        assert_generic_rules_once(&prompt);
        let at = |needle: &str| {
            prompt
                .find(needle)
                .unwrap_or_else(|| panic!("{name}: no `{needle}`: {prompt}"))
        };
        assert!(
            at(GENERIC_RULES.trim_end()) < at("## Objective\n"),
            "{name}: {prompt}"
        );
    }
}

/// D1.3: CoS chat run（`build_cos_chat_parts`）の固定 Core にも run 固有の部分にも汎用規約は入らない。
#[test]
fn change_rules_generic_absent_from_cos_chat() {
    let context = RunContext {
        cos_chat: Some(CosChatContext {
            thread_id: "thread-1".to_string(),
            run_id: "chat-run-1".to_string(),
            ..CosChatContext::default()
        }),
        conversation_addressee: Some(crate::protocol::ConversationAddressee::Secretary),
        execution_planner: Some(tree_planner()),
        workspace: workspace_with(Some(policy())),
        ..RunContext::default()
    };
    let task = crate::protocol::tests::sample_task();
    let heading = GENERIC_RULES.lines().next().unwrap_or_default();
    let prompt = build_prompt(&task, &context, "chat-run-1", "artifacts");
    assert!(!prompt.contains(heading), "{prompt}");
    assert!(!prompt.contains(REPO_RULES_HEADING), "{prompt}");
    let parts = build_cos_chat_parts(&task, &context, "chat-run-1", "artifacts")
        .unwrap_or_else(|| panic!("a CoS chat run"));
    assert!(!parts.core.contains(heading), "{}", parts.core);
    assert!(!parts.variable.contains(heading), "{}", parts.variable);
}

/// I2: generic.md は repo 固有の名前（この workspace の crate 名・必須規則の file・Celeris の語）を含まない。
#[test]
fn change_rules_generic_has_no_repo_specific_names() {
    let crates_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut names: Vec<String> = std::fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", crates_dir.display()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n == "task-worker"), "{names:?}");
    names.extend(
        [
            "celeris",
            "agent-platform",
            "crates/",
            "task_",
            "claude.md",
            "agents.md",
            "sqlite",
            "events",
            "dispatcher",
        ]
        .map(String::from),
    );
    let lower = GENERIC_RULES.to_lowercase();
    for name in &names {
        assert!(
            !lower.contains(&name.to_lowercase()),
            "generic.md names `{name}`"
        );
    }
}

/// D1.5: 汎用規約と repo の必須規則（`CLAUDE.md`）は互いの本文を写さない（短い行・見出しの記号を除く全行）。
#[test]
fn change_rules_generic_and_required_rules_do_not_copy_each_other() {
    let rules = repo_claude_md();
    let body = |text: &str| -> Vec<String> {
        text.lines()
            .map(|l| l.trim_start_matches(['#', '-', ' ']).trim().to_string())
            .filter(|l| l.chars().count() >= 12)
            .collect()
    };
    for line in body(GENERIC_RULES) {
        assert!(!rules.contains(&line), "CLAUDE.md copies `{line}`");
    }
    for line in body(&rules) {
        assert!(!GENERIC_RULES.contains(&line), "generic.md copies `{line}`");
    }
}

/// (b): review 方針があれば repo 固有の規約（owner 地図・高 risk の class と path・不変条件）が汎用規約の
/// 直後に 1 回出る。方針が無い・中身が空なら何も出ない。
#[test]
fn change_rules_generic_repo_section_follows_only_with_a_review_policy() {
    let with = planner_prompt(tree_planner(), Some(policy()));
    let generic = section();
    let repo = repo_section(&planner_context(tree_planner(), Some(policy())));
    assert!(repo.starts_with(REPO_RULES_HEADING), "{repo}");
    for needle in [
        CLASS,
        CLASS_PATH,
        "^crates/task-store/",
        INVARIANT,
        "docs/architecture-map.md",
    ] {
        assert!(repo.contains(needle), "{needle}: {repo}");
    }
    assert_eq!(
        with.matches(&format!("{generic}{repo}")).count(),
        1,
        "{with}"
    );
    // 見出しの行は 1 回だけ（/3 の brief の節は見出しの名前で参照するだけ）。
    let heading_line = format!("\n{REPO_RULES_HEADING}\n");
    assert_eq!(with.matches(&heading_line).count(), 1, "{with}");

    let without = planner_prompt(tree_planner(), None);
    assert!(!without.contains(REPO_RULES_HEADING), "{without}");
    let empty = ReviewPolicy {
        definition_sha256: "d".to_string(),
        ..ReviewPolicy::default()
    };
    let empty = planner_prompt(tree_planner(), Some(empty));
    assert!(!empty.contains(REPO_RULES_HEADING), "{empty}");
    assert!(
        !prompt_for(TaskKind::Execute, &[], &RunContext::default()).contains(REPO_RULES_HEADING)
    );
}

/// generic.md の「実装前 brief」の番号付きの項目（`brief` の節が対応させる 7 項目）。
fn generic_brief_items() -> usize {
    GENERIC_RULES
        .split("\n## ")
        .find(|part| part.starts_with("実装前 brief"))
        .unwrap_or_else(|| panic!("generic.md has no brief section: {GENERIC_RULES}"))
        .lines()
        .filter(|l| {
            l.split_once(". ")
                .is_some_and(|(n, _)| n.parse::<u32>().is_ok())
        })
        .count()
}

/// D3.4: review 方針のある /3 の planner には設計 brief の節が出て、7 項目の欄（generic.md の「実装前 brief」に
/// 対応）・高 risk の class での必須（`DesignBriefMissing`）・High と Unknown で採用前の設計 review を書く。
/// class の一覧は repo 固有の節にだけあり、brief の節には再掲しない。
#[test]
fn change_rules_planner_brief_section_with_a_review_policy() {
    let context = planner_context(tree_planner(), Some(policy()));
    let brief = design_brief_section(&context);
    let prompt = prompt_for(TaskKind::Execute, &[], &context);
    assert_eq!(prompt.matches(&brief).count(), 1, "{prompt}");
    assert_eq!(generic_brief_items(), 7);
    let fields = [
        "root_cause",
        "owner",
        "reuse",
        "options",
        "new_state",
        "removals",
        "invariants",
    ];
    for (n, field) in (1..=7).zip(fields) {
        let line = brief
            .lines()
            .find(|l| l.starts_with(&format!("{n}. ")))
            .unwrap_or_else(|| panic!("item {n}: {brief}"));
        assert!(line.contains(&format!("`{field}`")), "{line}");
    }
    assert!(brief.contains("「実装前 brief」"), "{brief}");
    assert!(GENERIC_RULES.contains("\n## 実装前 brief\n"));
    for needle in [
        "DesignBriefMissing",
        REPO_RULES_HEADING,
        "High",
        "Unknown",
        "設計 review",
    ] {
        assert!(brief.contains(needle), "{needle}: {brief}");
    }
    assert!(
        !brief.contains(CLASS) && !brief.contains(CLASS_PATH),
        "{brief}"
    );
    assert_eq!(prompt.matches(&format!("`{CLASS}`")).count(), 1, "{prompt}");
    // brief の節は /3 の計画の形の説明の後に出る（`design_brief` は /3 だけの欄）。
    let at = |needle: &str| {
        prompt
            .find(needle)
            .unwrap_or_else(|| panic!("no `{needle}`: {prompt}"))
    };
    assert!(
        at(task_core::EXECUTION_PLAN_SCHEMA_V3) < at(&brief),
        "{prompt}"
    );
}

/// D3.4: review 方針の無い /3 の planner でも brief の節は出るが、必須の文と class への参照は出ず、方針が無い
/// ときの扱い（任意・方針を読めなければ Unknown で設計 review）を書く。/1 の planner には brief の節が無い。
#[test]
fn change_rules_planner_brief_section_without_a_review_policy() {
    let with = design_brief_section(&planner_context(tree_planner(), Some(policy())));
    let context = planner_context(tree_planner(), None);
    let brief = design_brief_section(&context);
    let prompt = prompt_for(TaskKind::Execute, &[], &context);
    assert_eq!(prompt.matches(&brief).count(), 1, "{prompt}");
    assert_ne!(brief, with);
    assert!(!brief.contains("DesignBriefMissing"), "{brief}");
    assert!(!brief.contains(REPO_RULES_HEADING), "{brief}");
    assert!(
        brief.contains("Unknown") && brief.contains("設計 review"),
        "{brief}"
    );
    let heading = brief.lines().next().unwrap_or_default();
    assert!(with.starts_with(heading), "{with}");

    // /1（木でない）の planner は `design_brief` を書けないので節を出さない。repo 固有の節は出る。
    let flat = planner_prompt(
        ExecutionPlannerContext {
            max_work_units: 8,
            ..ExecutionPlannerContext::default()
        },
        Some(policy()),
    );
    assert!(!flat.contains(heading), "{flat}");
    assert!(flat.contains(REPO_RULES_HEADING), "{flat}");
}
