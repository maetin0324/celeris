//! ADR 2026-11-21-strategy-evidence-contract D5（U3）の試験（prefix `prompt_project_request_`）。案件に属する task の
//! worker・final review の reviewer・木の planner・WU の run が adapter に渡す `RunRequest` から `claude_code::build_prompt`
//! で組んだ prompt に、3,000 文字の `Project.request` の末尾の印と、案件の判断の記録と同じ sha256
//! （`project_wake::project_request_sha256` が呼ぶ `project_proposal::sha256_hex`）が 1 回ずつ載ることを、dispatcher の経路（store から task と案件を読み、
//! `RunContext` を組む所）で確かめる。案件に属さない task では節が無い。偽の adapter と一時 dir だけで、外部 network に出ない。

use super::prompt_ancestor_objective::{PromptCapture, RunKind};
use super::tree::{leaf, stage, tree_dispatcher, v3_plan};
use super::*;

const REQUEST_TAIL: &str = "PROJECT-REQUEST-TAIL 人の元の目的: 自動 publish は絶対にしない";
const HEADING: &str = "## 案件の依頼（Project.request の全文。人の元の目的の正本。sha256: ";

/// 3,000 文字の `Project.request`（末尾に印）を持つ案件を store に置き、案件判断の記録と同じ sha256 を返す。
fn long_request_project(store: &SqliteStore) -> (ProjectId, String) {
    let mut project = titled_project("strategy project");
    project.request = format!("{}{REQUEST_TAIL}", "人の依頼。".repeat(600));
    assert!(project.request.chars().count() > 3000);
    store.project_create(&project).unwrap();
    // `project_wake::project_request_sha256`（案件判断の記録の `request_sha256`）が呼ぶのと同じ関数。
    let sha = task_core::project_proposal::sha256_hex(&project.request);
    (project.id, sha)
}

/// 案件の依頼の見出し・`Project.request` の末尾の印・sha256 が、prompt にちょうど 1 回ずつ載るか（同じ text を 2 回出さ
/// ない）。外れた印を `"<label>: <印> x<回数>"` の形で返す（1 つの試験で全経路の外れを一度に出すため）。
fn misses(prompt: &str, label: &str, sha: &str) -> Vec<String> {
    [
        ("heading", HEADING),
        ("request tail", REQUEST_TAIL),
        ("sha256", sha),
    ]
    .iter()
    .filter_map(|(what, mark)| {
        let n = prompt.matches(mark).count();
        (n != 1).then(|| format!("{label}: project {what} x{n} (want x1)"))
    })
    .collect()
}

/// 不変条件（ADR 2026-11-21-strategy-evidence-contract D5.1・D5.2・D5.4）: 案件に属する task（`project_id` を持つ）の worker
/// の run と、その task の final review の reviewer の run の prompt に、`Project.request` の全文（3,000 文字の末尾の印）と、
/// 案件の判断の記録の `request_sha256` と同じ sha256 が 1 回ずつ載る。
/// 既存の試験で検出できない理由: `prompt_ancestor_objective_*` は祖先の task の text だけを、`prompt_full_objective_*` は run
/// 自身の task の objective だけを見る。案件を store に置いて dispatcher の経路を通す試験は無いので、prompt に案件の依頼が
/// 載らないこと（prompt-audit の欠け 5。15 run のすべて）を検出できない。
#[tokio::test]
async fn prompt_project_request_worker_and_reviewer_runs_carry_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, sha) = long_request_project(store.as_ref());
    let mut task = new_task(dir.path(), Check::Reviewer, 0);
    task.title = "project task".into();
    task.objective = "案件の task の目的".into();
    task.project_id = Some(project_id);
    store.insert(&task).unwrap();
    let adapter = Arc::new(PromptCapture::new(None));
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    run_until_task_terminal(&mut d, &store, task.id).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let workers = adapter.prompts(RunKind::Worker);
    assert_eq!(workers.len(), 1, "one worker run");
    let reviewers = adapter.prompts(RunKind::Reviewer);
    assert!(!reviewers.is_empty(), "the reviewer must run");
    let mut missed = misses(&workers[0], "worker", &sha);
    for p in &reviewers {
        missed.extend(misses(p, "reviewer", &sha));
    }
    assert!(
        missed.is_empty(),
        "{missed:#?}\nworker prompt:\n{}",
        workers[0]
    );
}

/// 不変条件（ADR 2026-11-21-strategy-evidence-contract D5.2）: 案件は対象 task の `project_id`、無ければ祖先の列で最初に
/// `project_id` を持つ祖先のもの。`project_id` を持つ親の下の子（自身は `project_id` を持たない）が木の planner で計画される
/// とき、planner の run と WU の run の prompt に、親の案件の `Project.request` の全文と sha256 が 1 回ずつ載る。
/// 既存の試験で検出できない理由: 上の試験は task 自身の `project_id` しか通らない。木の planner・WU の run（`run_extras` →
/// `prompt_ancestors::lineage`）で祖先の案件を辿る経路は、ほかに案件を置く試験が無い。
#[tokio::test]
async fn prompt_project_request_planner_and_work_unit_runs_carry_the_ancestor_project() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, sha) = long_request_project(store.as_ref());
    let mut parent = new_task(dir.path(), Check::Reviewer, 0);
    parent.title = "project parent".into();
    parent.status = Status::Draft;
    parent.project_id = Some(project_id);
    store.insert(&parent).unwrap();
    let mut child = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    child.title = "planned child".into();
    child.objective = "子の目的（計画する）".into();
    child.parent_id = Some(parent.id);
    child.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        }),
        ..Default::default()
    });
    assert!(child.project_id.is_none());
    store.insert(&child).unwrap();
    let plan = v3_plan(vec![stage("s1", false)], vec![leaf("a", "s1", &[])]);
    let adapter = Arc::new(PromptCapture::new(Some(plan)));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_task_terminal(&mut d, &store, child.id).await;
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Done);
    let planners = adapter.prompts(RunKind::Planner);
    assert!(!planners.is_empty(), "the planner must run");
    let units = adapter.prompts(RunKind::WorkUnit);
    assert!(!units.is_empty(), "the work unit must run");
    let mut missed = Vec::new();
    for p in &planners {
        missed.extend(misses(p, "planner", &sha));
    }
    for p in &units {
        missed.extend(misses(p, "work unit", &sha));
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// 不変条件（ADR 2026-11-21-strategy-evidence-contract D5.3・D5.6）: 案件に属さない task（自身にも祖先にも `project_id` が
/// 無い）の worker・reviewer の prompt には案件の依頼の節が無い。store に別の案件があっても、その依頼は載らない。
/// 既存の試験で検出できない理由: 上の 2 件は案件のある task だけを見るので、案件の無い task に別の案件の依頼が漏れても
/// （例えば store の最初の案件を使う誤り）落ちない。
#[tokio::test]
async fn prompt_project_request_task_without_project_has_no_section() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (_other_project, _) = long_request_project(store.as_ref());
    let mut parent = new_task(dir.path(), Check::Reviewer, 0);
    parent.status = Status::Draft;
    store.insert(&parent).unwrap();
    let mut task = new_task(dir.path(), Check::Reviewer, 0);
    task.parent_id = Some(parent.id);
    store.insert(&task).unwrap();
    let adapter = Arc::new(PromptCapture::new(None));
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    run_until_task_terminal(&mut d, &store, task.id).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let prompts: Vec<String> = adapter
        .prompts(RunKind::Worker)
        .into_iter()
        .chain(adapter.prompts(RunKind::Reviewer))
        .collect();
    assert!(prompts.len() >= 2, "worker and reviewer runs");
    for p in &prompts {
        assert!(
            !p.contains("## 案件の依頼") && !p.contains(REQUEST_TAIL),
            "a task without a project must not get a project request:\n{p}"
        );
    }
}
