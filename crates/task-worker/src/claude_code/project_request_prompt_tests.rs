//! ADR 2026-11-21-strategy-evidence-contract D5（U3）の試験（prefix `prompt_project_request_`）。`RunContext.lineage.project`
//! を持つ run の prompt を builder ごと（worker・WU・`TaskKind::Plan`・案件の計画と replan・木と木でない execution planner・
//! reviewer。一覧は `ancestor_prompt_tests::cases`）に組み、`Project.request` の全文と sha256 が 1 回ずつ載ることと、案件に
//! 属さない task の prompt が変わらないことを確かめる。dispatcher が store から案件を載せる経路は `task-dispatch` の同名
//! prefix の試験が見る。

use super::ancestor_prompt_tests::cases;
use crate::claude_code::{build_prompt, build_prompt_with_skill_list};
use crate::protocol::{AncestorContext, ProjectRequestContext, RunContext, TaskLineageContext};

const HEADING_START: &str = "## 案件の依頼（Project.request の全文。人の元の目的の正本。sha256: ";
const REQUEST_TAIL: &str = "PROJECT-REQUEST-TAIL 人の元の目的の末尾";
const ANCESTORS_HEADING: &str = "## 祖先の task の目的と受け入れ条件";

/// 3,000 文字の `Project.request`（末尾に印）。sha256 は案件の判断の記録と同じ `project_proposal::sha256_hex`。
fn project() -> ProjectRequestContext {
    let request = format!("{}{REQUEST_TAIL}", "依".repeat(3000));
    ProjectRequestContext {
        project_id: "01M2WTS3DKNZBSZ2JMVB4CZMBW".to_string(),
        request_sha256: task_core::project_proposal::sha256_hex(&request),
        request,
    }
}

fn parent() -> Vec<AncestorContext> {
    vec![AncestorContext {
        title: "parent task".to_string(),
        objective: "親の目的".to_string(),
        acceptance: Vec::new(),
    }]
}

/// 不変条件（ADR 2026-11-21-strategy-evidence-contract D5.4）: `context.lineage.project` を持つ run の prompt に、どの builder
/// でも（codex の `build_prompt` と acp・pi の `build_prompt_with_skill_list` の両方で）案件の依頼の節が 1 回出て、3,000 文字の
/// `Project.request` の末尾の印と sha256 がちょうど 1 回ずつ載る。祖先があれば、案件の節は祖先の節より前に出る。
/// 既存の試験で検出できない理由: `prompt_ancestor_objective_*` は祖先の task の text だけを、`prompt_full_objective_*` は run
/// 自身の task の objective だけを見る。`Project.request` を持つ context を組む試験は無いので、prompt に案件の依頼が載らない
/// こと（prompt-audit の欠け 5。15 run のすべて）を検出できない。
#[test]
fn prompt_project_request_every_builder_carries_the_full_request_once() {
    let project = project();
    for (label, task, context) in cases() {
        let context = RunContext {
            lineage: TaskLineageContext {
                ancestors: parent(),
                project: Some(project.clone()),
                ..TaskLineageContext::default()
            },
            ..context
        };
        let codex = build_prompt(&task, &context, "run-project", "artifacts");
        let acp_pi = build_prompt_with_skill_list(&task, &context, "run-project", "artifacts");
        for (harness, prompt) in [("build_prompt", &codex), ("with_skill_list", &acp_pi)] {
            for mark in [HEADING_START, REQUEST_TAIL, project.request_sha256.as_str()] {
                assert_eq!(
                    prompt.matches(mark).count(),
                    1,
                    "{label}/{harness}: `{mark}` must appear exactly once:\n{prompt}"
                );
            }
            assert!(
                prompt.contains(&format!(
                    "{HEADING_START}{}）\n{}\n\n",
                    project.request_sha256, project.request
                )),
                "{label}/{harness}: the section must carry the whole request after its sha256:\n{prompt}"
            );
            let section = prompt.find(HEADING_START);
            let ancestors = prompt.find(ANCESTORS_HEADING);
            assert!(
                section.is_some() && section < ancestors,
                "{label}/{harness}: the project request must come before the ancestors:\n{prompt}"
            );
        }
    }
}

/// 不変条件（ADR 2026-11-21-strategy-evidence-contract D5.3・D5.6）: 案件に属さない task（`context.lineage.project` が `None`）
/// の prompt には案件の依頼の節が無く、案件を足した prompt から節を除くと 1 バイトも違わない（節は差し込むだけで、ほかの
/// 文を変えない）。wire の `lineage` にも `project` の欄が出ない。
/// 既存の試験で検出できない理由: `prompt_ancestor_objective_root_task_prompt_is_unchanged` は祖先の節だけを比べるので、案件の
/// 節が案件の無い task に漏れても、節を足すときにほかの文を書き換えても落ちない。
#[test]
fn prompt_project_request_task_without_project_is_unchanged() {
    let project = project();
    for (label, task, context) in cases() {
        for ancestors in [Vec::new(), parent()] {
            let without = RunContext {
                lineage: TaskLineageContext {
                    ancestors: ancestors.clone(),
                    ..TaskLineageContext::default()
                },
                ..context.clone()
            };
            let plain = build_prompt(&task, &without, "run-project", "artifacts");
            assert!(
                !plain.contains("## 案件の依頼"),
                "{label}: a task without a project must not get the section:\n{plain}"
            );
            let with = RunContext {
                lineage: TaskLineageContext {
                    project: Some(project.clone()),
                    ..without.lineage.clone()
                },
                ..without.clone()
            };
            let prompt = build_prompt(&task, &with, "run-project", "artifacts");
            let section = format!(
                "{HEADING_START}{}）\n{}\n\n",
                project.request_sha256, project.request
            );
            assert_eq!(
                prompt.replacen(&section, "", 1),
                plain,
                "{label}: the project section must be the only difference"
            );
            let wire = serde_json::to_value(&without).unwrap();
            assert!(
                wire.pointer("/lineage/project").is_none(),
                "{label}: no project on the wire: {wire}"
            );
            assert_eq!(
                wire.get("lineage").is_some(),
                !ancestors.is_empty(),
                "{label}: lineage appears only with ancestors: {wire}"
            );
        }
    }
}

/// 不変条件（ADR 2026-11-21-strategy-evidence-contract D5.1）: wire の `context.lineage.project` は `{project_id, request,
/// request_sha256}` で、`lineage` が案件だけを持つときも省略されず、往復で同じ値に戻る。
/// 既存の試験で検出できない理由: `run_context_*` の wire の試験は案件の欄を組まないので、`is_empty` が案件を見落として
/// 案件だけの `lineage` を wire から落としても落ちない。
#[test]
fn prompt_project_request_lineage_with_only_a_project_is_on_the_wire() {
    let project = project();
    let context = RunContext {
        lineage: TaskLineageContext {
            project: Some(project.clone()),
            ..TaskLineageContext::default()
        },
        ..RunContext::default()
    };
    let wire = serde_json::to_value(&context).unwrap();
    assert_eq!(
        wire.pointer("/lineage/project/request_sha256")
            .and_then(|v| v.as_str()),
        Some(project.request_sha256.as_str()),
        "{wire}"
    );
    assert_eq!(
        wire.pointer("/lineage/project/request")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().count()),
        Some(project.request.chars().count())
    );
    let back: RunContext = serde_json::from_value(wire).unwrap();
    assert_eq!(back, context);
}
