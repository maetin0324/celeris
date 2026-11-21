//! ADR 2026-11-21-ancestor-objective-in-prompts D1・D2 の試験（prefix `prompt_ancestor_objective_`）。`RunContext.ancestors`
//! を持つ run の prompt を builder ごと（worker・WU・`TaskKind::Plan`・案件の計画と replan・木と木でない execution planner・
//! reviewer）に組み、祖先の objective と受入条件の全文が 1 回ずつ載ることと、祖先の無い root の task の prompt が変わらない
//! ことを確かめる。dispatcher が store から祖先を載せる経路は `task-dispatch` の同名 prefix の試験が見る。

use task_core::{Check, Criterion, Task, TaskKind};

use crate::claude_code::{build_prompt, build_prompt_with_skill_list};
use crate::protocol::{
    AncestorContext, ExecutionPlannerContext, ReviewRequest, RunContext, TreeAncestorContext,
    TreePlannerContext, WorkUnitPromptContext,
};

const HEADING: &str = "## 祖先の task の目的と受け入れ条件（全文。root が先頭、親が末尾。目標・禁止事項・不変条件はこの task にも掛かる）\n";
const ROOT_OBJECTIVE_TAIL: &str = "ROOT-OBJECTIVE-TAIL";
const ROOT_ACCEPTANCE_TAIL: &str = "ROOT-ACCEPTANCE-TAIL";
const PARENT_OBJECTIVE_TAIL: &str = "PARENT-OBJECTIVE-TAIL";
const PARENT_ACCEPTANCE_TAIL: &str = "PARENT-ACCEPTANCE-TAIL";

/// root と親の 2 段の祖先。どちらも 3,000 文字の objective と 3,000 文字の受入条件の text の末尾に印を持つ。
fn ancestors() -> Vec<AncestorContext> {
    let one = |title: &str, objective_tail: &str, acceptance_tail: &str| AncestorContext {
        title: title.to_string(),
        objective: format!("{}{objective_tail}", "う".repeat(3000)),
        acceptance: vec![
            Criterion {
                text: format!("{title} の受入条件 0"),
                check: Check::Reviewer,
            },
            Criterion {
                text: format!("{}{acceptance_tail}", "え".repeat(3000)),
                check: Check::Human,
            },
        ],
    };
    vec![
        one("root task", ROOT_OBJECTIVE_TAIL, ROOT_ACCEPTANCE_TAIL),
        one("parent task", PARENT_OBJECTIVE_TAIL, PARENT_ACCEPTANCE_TAIL),
    ]
}

fn task_with(kind: TaskKind, labels: &[&str]) -> Task {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = kind;
    task.labels = labels.iter().map(|l| l.to_string()).collect();
    task
}

fn work_unit() -> WorkUnitPromptContext {
    WorkUnitPromptContext {
        key: "fix".to_string(),
        title: "Fix".to_string(),
        objective: "WU の目的".to_string(),
        done_when: vec!["WU が終わる".to_string()],
        task_objective: "子 task の目的".to_string(),
        ..WorkUnitPromptContext::default()
    }
}

fn tree_planner() -> ExecutionPlannerContext {
    ExecutionPlannerContext {
        max_work_units: 8,
        tree: Some(TreePlannerContext {
            depth: 2,
            max_depth: 3,
            remaining_depth: 1,
            ancestors: vec![TreeAncestorContext {
                title: "parent task".to_string(),
                stage: Some("s1".to_string()),
            }],
            ..TreePlannerContext::default()
        }),
        ..ExecutionPlannerContext::default()
    }
}

/// builder ごとの (label, task, 祖先の無い context)。祖先は呼び出し側が `ancestors` に入れる。
fn cases() -> Vec<(&'static str, Task, RunContext)> {
    vec![
        (
            "worker",
            task_with(TaskKind::Execute, &[]),
            RunContext::default(),
        ),
        (
            "work unit",
            task_with(TaskKind::Execute, &[]),
            RunContext {
                work_unit: Some(work_unit()),
                ..RunContext::default()
            },
        ),
        (
            "plan",
            task_with(TaskKind::Plan, &[]),
            RunContext::default(),
        ),
        (
            "project plan",
            task_with(TaskKind::Plan, &[task_core::MILESTONES_PLAN_LABEL]),
            RunContext::default(),
        ),
        (
            "project replan",
            task_with(
                TaskKind::Plan,
                &[
                    task_core::MILESTONES_PLAN_LABEL,
                    task_core::MILESTONES_REPLAN_LABEL,
                ],
            ),
            RunContext::default(),
        ),
        (
            "tree planner",
            task_with(TaskKind::Execute, &[]),
            RunContext {
                execution_planner: Some(tree_planner()),
                ..RunContext::default()
            },
        ),
        (
            "flat planner",
            task_with(TaskKind::Execute, &[]),
            RunContext {
                execution_planner: Some(ExecutionPlannerContext {
                    max_work_units: 8,
                    ..ExecutionPlannerContext::default()
                }),
                ..RunContext::default()
            },
        ),
        (
            "reviewer",
            task_with(TaskKind::Review, &[]),
            RunContext {
                review: Some(ReviewRequest::default()),
                ..RunContext::default()
            },
        ),
    ]
}

/// 不変条件（ADR 2026-11-21-ancestor-objective-in-prompts D1・D2）: `context.ancestors` を持つ run の prompt に、どの builder
/// でも（codex の `build_prompt` と acp・pi の `build_prompt_with_skill_list` の両方で）祖先の節が 1 回出て、root と親の
/// 3,000 文字の objective の末尾の印と、3,000 文字の受入条件の text の末尾の印がちょうど 1 回ずつ載る（木の planner の
/// 祖先の行は題名と段階だけで、同じ text を 2 回出さない）。root が親より先に出る。
/// 既存の試験で検出できない理由: `prompt_full_objective_*` と `planner_prompt_acceptance_*` は run 自身の task の objective と
/// 受入条件の末尾だけを見る。木の planner の祖先の試験（`execution_planner_prompt_tree_*` の深さ 3 の段）は 300 文字の
/// 抜粋の短い印しか見ないので、祖先の目的が先頭で切れても、受入条件が無くても落ちない。
#[test]
fn prompt_ancestor_objective_every_builder_carries_full_ancestor_text_once() {
    for (label, task, context) in cases() {
        let context = RunContext {
            ancestors: ancestors(),
            ..context
        };
        let codex = build_prompt(&task, &context, "run-ancestor", "artifacts");
        let acp_pi = build_prompt_with_skill_list(&task, &context, "run-ancestor", "artifacts");
        for (harness, prompt) in [("build_prompt", &codex), ("with_skill_list", &acp_pi)] {
            assert_eq!(
                prompt.matches(HEADING).count(),
                1,
                "{label}/{harness}: ancestors heading:\n{prompt}"
            );
            for tail in [
                ROOT_OBJECTIVE_TAIL,
                ROOT_ACCEPTANCE_TAIL,
                PARENT_OBJECTIVE_TAIL,
                PARENT_ACCEPTANCE_TAIL,
            ] {
                assert_eq!(
                    prompt.matches(tail).count(),
                    1,
                    "{label}/{harness}: `{tail}` must appear exactly once:\n{prompt}"
                );
            }
            let root = prompt.find("### 祖先 1（root）: root task\n");
            let parent = prompt.find("### 祖先 2（親）: parent task\n");
            assert!(
                root.is_some() && root < parent,
                "{label}/{harness}: root must come before the parent:\n{prompt}"
            );
        }
    }
}

/// 不変条件（ADR 2026-11-21-ancestor-objective-in-prompts D1）: 祖先の無い root の task（`context.ancestors` が空）の prompt
/// には祖先の節が無く、祖先を足した prompt から祖先の節を除くと 1 バイトも違わない（節は差し込むだけで、ほかの文を
/// 変えない）。`RunContext` の wire にも `ancestors` の欄が出ない。
/// 既存の試験で検出できない理由: 既存の prompt の試験は祖先を持つ context を組まないので、祖先の節が root の prompt に
/// 漏れても、節を足すときにほかの文を書き換えても、比べる相手が無い。
#[test]
fn prompt_ancestor_objective_root_task_prompt_is_unchanged() {
    for (label, task, context) in cases() {
        let root_prompt = build_prompt(&task, &context, "run-ancestor", "artifacts");
        assert!(
            !root_prompt.contains("## 祖先の task の目的と受け入れ条件"),
            "{label}: a root task must not get the ancestors section:\n{root_prompt}"
        );
        let with = RunContext {
            ancestors: ancestors(),
            ..context.clone()
        };
        let child_prompt = build_prompt(&task, &with, "run-ancestor", "artifacts");
        let start = child_prompt
            .find(HEADING)
            .unwrap_or_else(|| panic!("{label}: no ancestors section"));
        let end = start
            + child_prompt[start..]
                .find("PARENT-ACCEPTANCE-TAIL\n\n")
                .unwrap_or_else(|| panic!("{label}: no parent acceptance tail"))
            + "PARENT-ACCEPTANCE-TAIL\n\n".len();
        let stripped = format!("{}{}", &child_prompt[..start], &child_prompt[end..]);
        assert_eq!(
            stripped, root_prompt,
            "{label}: the ancestors section must be the only difference"
        );
        let wire = serde_json::to_value(&context).unwrap();
        assert!(
            wire.get("ancestors").is_none(),
            "{label}: an empty ancestors list must not appear on the wire: {wire}"
        );
    }
}
