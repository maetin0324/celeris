//! ADR 2026-11-19-planner-reviewer-prompt-coverage D1〜D3 の試験（prefix `planner_prompt_acceptance_`・
//! `prompt_full_objective_planner`）。fixture は `change_rules::tests` の `planner_context`・`tree_planner`・`prompt_for` と
//! `protocol::tests::sample_task`。design review と final review の `prompt_full_objective_*` は別の試験 file にある。

use task_core::{Check, Criterion, Task, TaskKind};

use crate::change_rules::tests::{planner_context, prompt_for, tree_planner};
use crate::claude_code::{build_prompt, build_prompt_with_skill_list};
use crate::protocol::{ExecutionPlannerContext, ReviewRequest, RunContext, WorkUnitPromptContext};

const HEADING: &str = "## Task acceptance criteria (checked by the final review)\n";

/// 種類の違う check を持つ 3 件の受入条件（text は互いに含み合わない印）。
fn criteria() -> Vec<Criterion> {
    vec![
        Criterion {
            text: "ACC-ZERO test-affected と clippy と fmt が通る".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        },
        Criterion {
            text: "ACC-ONE 進捗の file が status: done".to_string(),
            check: Check::Reviewer,
        },
        Criterion {
            text: "ACC-TWO 人が報告を読んで承認する".to_string(),
            check: Check::Human,
        },
    ]
}

fn expected_section() -> String {
    format!(
        "{HEADING}0. ACC-ZERO test-affected と clippy と fmt が通る\n1. ACC-ONE 進捗の file が status: done\n\
         2. ACC-TWO 人が報告を読んで承認する\n\n"
    )
}

fn task_with(kind: TaskKind, labels: &[&str], acceptance: Vec<Criterion>) -> Task {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = kind;
    task.labels = labels.iter().map(|l| l.to_string()).collect();
    task.acceptance = acceptance;
    task
}

fn flat_planner(parallel: bool) -> ExecutionPlannerContext {
    ExecutionPlannerContext {
        max_work_units: 8,
        parallel,
        max_phases: 4,
        ..ExecutionPlannerContext::default()
    }
}

/// 節が 1 回だけ、`## Instructions` より前（objective の後）に出ることを確かめる。
fn assert_section_once_before_instructions(prompt: &str, section: &str, label: &str) {
    assert_eq!(
        prompt.matches(HEADING).count(),
        1,
        "{label}: acceptance heading must appear exactly once:\n{prompt}"
    );
    let at = prompt
        .find(section)
        .unwrap_or_else(|| panic!("{label}: acceptance section missing or changed:\n{prompt}"));
    let objective = prompt
        .find("## Objective\n")
        .unwrap_or_else(|| panic!("{label}: objective missing"));
    let instructions = prompt
        .find("## Instructions\n")
        .unwrap_or_else(|| panic!("{label}: instructions missing"));
    assert!(
        objective < at && at < instructions,
        "{label}: acceptance section must sit between the objective and the instructions"
    );
}

/// 不変条件（D1）: 実際の木の planner（`context.execution_planner.tree` がある run の `build_execution_plan_prompt`）の
/// prompt に、task の `acceptance[].text` が全部、`## Task acceptance criteria (checked by the final review)` の節に
/// `{i}. {text}` の行で、objective の後・`## Instructions` の前に 1 回だけ載る。
/// 既存の `planner_prompt_carries_depth_and_leaf_criteria` などの `planner_prompt_*` は /3 の形・深さ・上限の文だけを見て、
/// `prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule` は repo 固有の規約の節だけを見る。
/// どれも `task.acceptance` の text を探さないので、H の監査で見つかった 0/126 件（受入条件が planner に届かない）を検出できない。
#[test]
fn planner_prompt_acceptance_tree_planner_lists_every_criterion() {
    let task = task_with(TaskKind::Execute, &[], criteria());
    let context = planner_context(tree_planner(), None);
    let prompt = build_prompt(&task, &context, "run-planner-acceptance", "artifacts");
    assert!(
        prompt.contains("\"schema\":\"celeris.execution-plan/3\""),
        "fixture must select the tree planner"
    );
    assert_section_once_before_instructions(&prompt, &expected_section(), "tree planner");
    // fixture の task（受入条件 1 件 `c`）でも同じ節が出る。
    let fixture = prompt_for(TaskKind::Execute, &[], &context);
    assert!(fixture.contains(&format!("{HEADING}0. c\n\n")), "{fixture}");
}

/// 不変条件（D1）: 木でない execution planner（/1 と並列の /2）、`TaskKind::Plan`（`plan.json`）、案件の計画
/// （`MILESTONES_PLAN_LABEL`）と replan（`MILESTONES_REPLAN_LABEL`）の prompt にも、task の受入条件の text が全部、同じ節に
/// 載る。受入条件が空の task では節を消さず「無し」の 1 行を出す。
/// 既存の `planner_prompt_*` は木の planner と check の書き方の節しか見ず、`change_rules_generic_once_in_every_run_kind` は
/// 汎用規約の節の回数しか見ないので、これらの builder が受入条件を出さなくても落ちない。
#[test]
fn planner_prompt_acceptance_flat_planner_and_plan_kinds_list_every_criterion() {
    let milestones = task_core::MILESTONES_PLAN_LABEL;
    let replan = task_core::MILESTONES_REPLAN_LABEL;
    let cases: Vec<(&str, TaskKind, Vec<&str>, RunContext)> = vec![
        (
            "execution planner /1",
            TaskKind::Execute,
            vec![],
            planner_context(flat_planner(false), None),
        ),
        (
            "execution planner /2",
            TaskKind::Execute,
            vec![],
            planner_context(flat_planner(true), None),
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
    ];
    for (label, kind, labels, context) in cases {
        let task = task_with(kind, &labels, criteria());
        let prompt = build_prompt(&task, &context, "run-planner-acceptance", "artifacts");
        assert_section_once_before_instructions(&prompt, &expected_section(), label);
        let fixture = prompt_for(kind, &labels, &context);
        assert!(
            fixture.contains(&format!("{HEADING}0. c\n\n")),
            "{label}: fixture task:\n{fixture}"
        );
        let empty = task_with(kind, &labels, Vec::new());
        let prompt = build_prompt(&empty, &context, "run-planner-acceptance", "artifacts");
        assert_section_once_before_instructions(&prompt, &format!("{HEADING}無し\n\n"), label);
    }
}

/// 不変条件（D1・D3）: codex が使う `build_prompt` と、acp・pi が使う `build_prompt_with_skill_list` の両方で、木の planner と
/// `TaskKind::Plan` の prompt に task の受入条件の text が全部入る（どの harness の planner にも同じ節が届く）。
/// 既存の `change_rules_generic_reaches_*` は汎用規約の節だけを harness ごとに見て、`planner_prompt_*` は `build_prompt` しか
/// 呼ばないので、`build_prompt_with_skill_list` の経路で受入条件が欠けても検出できない。
#[test]
fn planner_prompt_acceptance_reaches_every_harness_input() {
    let cases = [
        (
            "tree planner",
            TaskKind::Execute,
            planner_context(tree_planner(), None),
        ),
        ("plan", TaskKind::Plan, RunContext::default()),
    ];
    for (label, kind, context) in cases {
        let task = task_with(kind, &[], criteria());
        let codex = build_prompt(&task, &context, "run-planner-acceptance", "artifacts");
        let acp_pi =
            build_prompt_with_skill_list(&task, &context, "run-planner-acceptance", "artifacts");
        for (harness, prompt) in [("build_prompt", &codex), ("with_skill_list", &acp_pi)] {
            for criterion in criteria() {
                assert!(
                    prompt.contains(&criterion.text),
                    "{label}/{harness}: missing `{}`:\n{prompt}",
                    criterion.text
                );
            }
            assert!(
                prompt.contains(&expected_section()),
                "{label}/{harness}:\n{prompt}"
            );
        }
    }
}

/// 不変条件（D2）: 3,000 文字の objective の末尾の印と、3,000 文字の受入条件の text の末尾の印が、木の planner と
/// `TaskKind::Plan` の prompt に切れずに載る（planner が依頼の末尾と最終レビューの条件の末尾まで読める）。
/// 既存の `prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule` は長い objective を worker の
/// prompt にしか入れず（planner の prompt は短い fixture の objective のまま）、末尾の印も worker の prompt にしか当てない。
/// `planner_prompt_*` は長い objective も受入条件も入れないので、planner の prompt が objective や受入条件を途中で切っても落ちない。
#[test]
fn prompt_full_objective_planner_keeps_objective_and_acceptance_tails() {
    let objective = format!("{}END-OF-OBJECTIVE", "あ".repeat(3000));
    let criterion = format!("{}END-OF-ACCEPTANCE", "い".repeat(3000));
    let cases = [
        (
            "tree planner",
            TaskKind::Execute,
            planner_context(tree_planner(), None),
        ),
        ("plan", TaskKind::Plan, RunContext::default()),
    ];
    for (label, kind, context) in cases {
        let mut task = task_with(
            kind,
            &[],
            vec![Criterion {
                text: criterion.clone(),
                check: Check::Reviewer,
            }],
        );
        task.objective = objective.clone();
        let prompt = build_prompt(&task, &context, "run-planner-tail", "artifacts");
        assert!(
            prompt.contains(&format!("## Objective\n{objective}\n")),
            "{label}: objective was truncated"
        );
        assert!(
            prompt.contains(&format!("{HEADING}0. {criterion}\n\n")),
            "{label}: acceptance criterion missing or truncated"
        );
    }
}

/// 不変条件（D1）: `acceptance_lines` に寄せた後も、final review の「Acceptance criteria of the task under review」と WU の run
/// の「最終レビューで確かめる Task の受け入れ条件（参考）」の節は同じ byte を出す（`{i}. {text}` の行と空行）。
/// 既存の試験は WU の参考の節の見出しの有無（`build_prompt_replaces_the_objective_and_acceptance_with_the_work_unit_when_present`）しか
/// 見ず、review の受入条件の節の書式を固定する試験は
/// 無いので、helper の書式が変わって 2 節の出力が変わっても落ちない。
#[test]
fn acceptance_lines_keep_review_and_work_unit_sections() {
    let lines = "0. ACC-ZERO test-affected と clippy と fmt が通る\n1. ACC-ONE 進捗の file が status: done\n\
                 2. ACC-TWO 人が報告を読んで承認する\n\n";
    let review = task_with(TaskKind::Review, &[], criteria());
    let context = RunContext {
        review: Some(ReviewRequest::default()),
        ..RunContext::default()
    };
    let prompt = build_prompt(&review, &context, "run-review", "artifacts");
    assert!(
        prompt.contains(&format!(
            "## Acceptance criteria of the task under review\n{lines}"
        )),
        "{prompt}"
    );
    assert!(!prompt.contains(HEADING), "{prompt}");

    let worker = task_with(TaskKind::Execute, &[], criteria());
    let context = RunContext {
        work_unit: Some(WorkUnitPromptContext {
            objective: "wu objective".to_string(),
            done_when: vec!["wu done".to_string()],
            ..WorkUnitPromptContext::default()
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&worker, &context, "run-wu", "artifacts");
    assert!(
        prompt.contains(&format!(
            "## Acceptance criteria (this WorkUnit)\n0. wu done\n\n\
             ## 最終レビューで確かめる Task の受け入れ条件（参考）\n{lines}"
        )),
        "{prompt}"
    );
    assert!(!prompt.contains(HEADING), "{prompt}");
}
