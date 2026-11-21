//! ADR 2026-11-21-ancestor-objective-in-prompts の試験（prefix `prompt_ancestor_objective_`）。子 task（`parent_id` の
//! ある task）の worker・reviewer・planner・WU の run が adapter に渡す `RunRequest` から `claude_code::build_prompt` で
//! 組んだ prompt に、親 task の 3,000 文字の objective の末尾の印と受入条件の末尾の印が切れずに載ることを、dispatcher の
//! 経路（store から task を読み、`RunContext` を組む所）で確かめる。すべて偽の adapter と一時 dir だけで、外部 network
//! に出ない。

use std::sync::Mutex as StdMutex;

use super::tree::{leaf, stage, tree_dispatcher, v3_plan};
use super::*;

const OBJECTIVE_TAIL: &str = "ANCESTOR-OBJECTIVE-TAIL 親の禁止事項: Project.request を書き換えない";
const ACCEPTANCE_TAIL: &str = "ANCESTOR-ACCEPTANCE-TAIL 親の受入条件の末尾";

/// 3,000 文字の objective（末尾に印）と、3,000 文字の受入条件（末尾に印）を持つ親。`Draft` なので dispatch されない。
fn long_parent(dir: &std::path::Path) -> Task {
    let mut parent = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    parent.title = "strategy parent".into();
    parent.status = Status::Draft;
    parent.objective = format!("{}{OBJECTIVE_TAIL}", "親の目的。".repeat(600));
    assert!(parent.objective.chars().count() > 3000);
    parent.acceptance = vec![
        Criterion {
            text: "親の受入条件 0".into(),
            check: Check::Reviewer,
        },
        Criterion {
            text: format!("{}{ACCEPTANCE_TAIL}", "条件".repeat(1500)),
            check: Check::Human,
        },
    ];
    parent
}

/// run の種類（planner・WU・reviewer・worker）ごとの prompt。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunKind {
    Planner,
    WorkUnit,
    Reviewer,
    Worker,
}

/// 受け取った `RunRequest` から prompt を組んで記録する adapter。planner には `plan` を書き、reviewer には全件合格の
/// `review.json` を書き、worker・WU には `result.json` を書く。案件の依頼の試験（`prompt_project_request`）も使う。
pub(super) struct PromptCapture {
    plan: Option<String>,
    seen: StdMutex<Vec<(RunKind, String)>>,
}

impl PromptCapture {
    pub(super) fn new(plan: Option<String>) -> Self {
        PromptCapture {
            plan,
            seen: StdMutex::new(Vec::new()),
        }
    }

    pub(super) fn prompts(&self, kind: RunKind) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for PromptCapture {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        let kind = if req.context.review.is_some() {
            RunKind::Reviewer
        } else if req.context.execution_planner.is_some() {
            RunKind::Planner
        } else if req.context.work_unit.is_some() {
            RunKind::WorkUnit
        } else {
            RunKind::Worker
        };
        let prompt =
            task_worker::claude_code::build_prompt(&req.task, &req.context, run_id, "artifacts");
        self.seen.lock().unwrap().push((kind.clone(), prompt));
        match kind {
            RunKind::Reviewer => {
                let verdicts: Vec<serde_json::Value> = (0..req.task.acceptance.len())
                    .map(|i| serde_json::json!({"criterion": i, "pass": true, "reason": "ok"}))
                    .collect();
                std::fs::write(
                    req.artifacts_dir.join("review.json"),
                    serde_json::json!({ "verdicts": verdicts }).to_string(),
                )
                .expect("write review.json");
            }
            RunKind::Planner => {
                if let Some(plan) = &self.plan {
                    std::fs::write(req.artifacts_dir.join("execution-plan.json"), plan)
                        .expect("write execution-plan.json");
                }
            }
            RunKind::WorkUnit | RunKind::Worker => {
                std::fs::write(
                    req.artifacts_dir.join("result.json"),
                    serde_json::json!({"summary": "ok", "evidence": []}).to_string(),
                )
                .expect("write result.json");
            }
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// 親の objective の末尾の印と受入条件の末尾の印が、prompt にちょうど 1 回ずつ載るか（同じ text を 2 回出さない）。
/// 外れた印を `"<label>: <印> x<回数>"` の形で返す（1 つの試験で全経路の外れを一度に出すため）。
fn misses(prompt: &str, label: &str) -> Vec<String> {
    [
        ("objective", OBJECTIVE_TAIL),
        ("acceptance", ACCEPTANCE_TAIL),
    ]
    .iter()
    .filter_map(|(what, tail)| {
        let n = prompt.matches(tail).count();
        (n != 1).then(|| format!("{label}: parent {what} tail x{n} (want x1)"))
    })
    .collect()
}

/// 不変条件（ADR 2026-11-21-ancestor-objective-in-prompts D1・D3）: `parent_id` のある子 task の worker の run と、
/// その子の final review の reviewer の run の prompt に、親の objective の全文（3,000 文字の末尾の印）と受入条件の全文
/// （3,000 文字の text の末尾の印）が 1 回ずつ載る。CoS が子の objective に親の文を写さなくても届く。
/// 既存の試験で検出できない理由: `prompt_full_objective_*`（task-worker・review・design）と `planner_prompt_acceptance_*`
/// は run 自身の task の objective と受入条件だけを見る。祖先の task を store に置いて dispatcher の経路を通す試験は
/// 無いので、`RunContext` に祖先が載らない（prompt-audit の欠け 1・4）ことを検出できない。
#[tokio::test]
async fn prompt_ancestor_objective_child_worker_and_reviewer_runs_carry_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let parent = long_parent(dir.path());
    store.insert(&parent).unwrap();
    let mut child = new_task(dir.path(), Check::Reviewer, 0);
    child.title = "child".into();
    child.objective = "子の目的".into();
    child.parent_id = Some(parent.id);
    store.insert(&child).unwrap();
    let adapter = Arc::new(PromptCapture::new(None));
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    run_until_task_terminal(&mut d, &store, child.id).await;
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Done);
    let workers = adapter.prompts(RunKind::Worker);
    assert_eq!(workers.len(), 1, "one worker run");
    let reviewers = adapter.prompts(RunKind::Reviewer);
    assert!(!reviewers.is_empty(), "the reviewer must run");
    let mut missed = misses(&workers[0], "child worker");
    for p in &reviewers {
        missed.extend(misses(p, "child reviewer"));
    }
    assert!(
        missed.is_empty(),
        "{missed:#?}\nworker prompt:\n{}",
        workers[0]
    );
}

/// 不変条件（ADR 2026-11-21-ancestor-objective-in-prompts D1・D2）: `parent_id` のある子 task が木の planner で計画され
/// るとき、planner の run と WU の run の prompt に、親の objective の全文と受入条件の全文が 1 回ずつ載る。子は木の root
/// （`tree` が無い）なので、`tree.parent_unit` を辿る木の祖先の行には親が出ない（prompt-audit の欠け 3）。
/// 既存の試験で検出できない理由: `planner_prompt_acceptance_*` と `prompt_full_objective_planner_*` は task 自身の受入
/// 条件と objective だけを、planner の木の祖先の試験（`planner_prompt_carries_depth_and_leaf_criteria`）は 300 文字の
/// 抜粋の短い印だけを見るので、祖先の末尾の欠け（欠け 2）と `parent_id` の祖先の欠け（欠け 3）を検出できない。
#[tokio::test]
async fn prompt_ancestor_objective_child_planner_and_work_unit_runs_carry_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let parent = long_parent(dir.path());
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
        missed.extend(misses(p, "child planner"));
    }
    for p in &units {
        missed.extend(misses(p, "child work unit"));
    }
    assert!(missed.is_empty(), "{missed:#?}");
}
