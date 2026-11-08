//! ADR 2026-11-07-health-review-integration 付記（2026-11-08、二度目の final review の差し戻しへの方針）の
//! 「流れ全体の試験（flow-tests）」: 偽の reviewer は `review.json` を自分で組まない。
//!
//! - reviewer の run では、dispatcher が渡した `RunRequest.task` と `RunRequest.context` から本物の
//!   `task_worker::claude_code::build_prompt`（claude_code adapter と同じ呼び方）で prompt を組む。
//! - 設計 review の prompt には `design_section` の節（brief の全欄の見出しと値・risk の level と reasons・方針の
//!   class と paths）と、Result 節の `design` の形があることを確かめる。final review の prompt には設計 review の
//!   節が無いことを確かめる。
//! - Result 節の例の JSON（バッククォートの 1 行）を取り出し、判定の値だけを合格に差し替えて `review.json` に書く
//!   （設計 review: `design.pass = true`・`findings = []`。final review: criterion ごとに `pass = true`、`repair` を
//!   除き、`health.findings = []`）。
//! - planner と WU の run は `health_review_scenario` の [`ScenarioAdapter`] に渡し、その prompt も記録する。
//! - 場合は 3 つ: 高 risk（合格で採用）、remote workspace（方針を読めず `unknown` → 合格で採用、人の承認に回らない）、
//!   通常の変更（設計 review の run が起きない）。
//!
//! health の diff 統計と blocking の finding による replan は `health_review_scenario_` が確かめるので、ここでは
//! 繰り返さない。`dispatcher/tests/mod.rs` から `#[path]` で読む。

use super::*;

use super::health_review_scenario::{
    NORMAL_PATH, Outcome, STORE_PATH, ScenarioAdapter, done, plan, run_flow_with,
};
use task_core::design_review::RiskLevel;
use task_worker::change_rules::REPO_RULES_HEADING;
use task_worker::change_rules::review::DESIGN_HEADING;
use task_worker::protocol::{ReviewOutput, ReviewRequest};

/// 受け入れ条件の判定の節の見出し（設計 review の prompt には出ない）。
const CRITERIA_HEADING: &str = "## Criteria you must judge in this run";
/// Result 節の見出しと 1 文目の頭（`change_rules::review::result_section`）。
const RESULT_HEADING: &str = "## Result\nWrite your verdicts to ";

/// adapter が受けた run の文脈と、その run に本物の `build_prompt` で組んだ prompt。
#[derive(Clone)]
struct PromptRun {
    context: task_worker::RunContext,
    prompt: String,
}

/// reviewer の run だけを自分で受け、ほかは [`ScenarioAdapter`] に渡す。prompt の不備は `problems` に残す
/// （adapter の中で panic すると試験の失敗の理由が run の失敗に埋もれる）。
struct FlowAdapter {
    inner: Arc<ScenarioAdapter>,
    runs: StdMutex<Vec<PromptRun>>,
    problems: StdMutex<Vec<String>>,
}

impl FlowAdapter {
    fn prompts(&self, pred: impl Fn(&task_worker::RunContext) -> bool) -> Vec<String> {
        self.runs
            .lock()
            .unwrap()
            .iter()
            .filter(|r| pred(&r.context))
            .map(|r| r.prompt.clone())
            .collect()
    }
    fn design_review_prompts(&self) -> Vec<String> {
        self.prompts(|c| c.review.as_ref().is_some_and(|r| r.design.is_some()))
    }
    fn final_review_prompts(&self) -> Vec<String> {
        self.prompts(|c| c.review.as_ref().is_some_and(|r| r.design.is_none()))
    }
    fn assert_no_problems(&self) {
        let problems = self.problems.lock().unwrap();
        assert!(
            problems.is_empty(),
            "reviewer prompt problems: {problems:#?}"
        );
    }
}

#[async_trait]
impl WorkerAdapter for FlowAdapter {
    fn id(&self) -> &str {
        self.inner.id()
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let prompt = task_worker::claude_code::build_prompt(
            &req.task,
            &req.context,
            run_id,
            &req.artifacts_rel(),
        );
        self.runs.lock().unwrap().push(PromptRun {
            context: req.context.clone(),
            prompt: prompt.clone(),
        });
        let Some(review) = &req.context.review else {
            return self.inner.run(req, run_id, limits, sink).await;
        };
        self.inner.seen.lock().unwrap().push(req.context.clone());
        let checked = if review.design.is_some() {
            design_prompt_problems(review, &prompt)
        } else {
            final_prompt_problems(&prompt)
        };
        let answer = answer_from_prompt(review, &prompt);
        let problems = match &answer {
            Ok(_) => checked,
            Err(e) => checked.into_iter().chain([e.clone()]).collect(),
        };
        if !problems.is_empty() {
            self.problems.lock().unwrap().extend(problems);
            return Ok(done("the prompt is incomplete"));
        }
        if let Ok(answer) = answer {
            std::fs::create_dir_all(&req.artifacts_dir).unwrap();
            std::fs::write(
                req.artifacts_dir.join(crate::review::REVIEW_FILE_NAME),
                answer.to_string(),
            )
            .unwrap();
        }
        Ok(done("reviewed"))
    }
}

/// `needle` が `prompt` に何回出るか。
fn count(prompt: &str, needle: &str) -> usize {
    prompt.matches(needle).count()
}

/// 設計 review の prompt に、dispatcher が渡した `ReviewRequest.design` の中身が描かれていない箇所。
fn design_prompt_problems(review: &ReviewRequest, prompt: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(design) = review.design.as_deref() else {
        return vec!["not a design review".into()];
    };
    let mut expect = |needle: String| {
        if !prompt.contains(&needle) {
            problems.push(format!("the design review prompt lacks {needle:?}"));
        }
    };
    expect(design.plan_summary.trim_end().to_string());
    match &design.brief {
        Some(brief) => {
            for field in [
                "root_cause",
                "owner",
                "reuse",
                "options",
                "new_state",
                "new_branch",
                "new_fallback",
                "new_port",
                "removals",
                "invariants",
            ] {
                expect(format!("- {field}:"));
            }
            expect(brief.root_cause.clone());
            expect(brief.owner.clone());
            for option in &brief.options {
                expect(format!("`{}`（chosen={}）", option.name, option.chosen));
                expect(option.reason.clone());
            }
            for item in brief.new_state.iter().chain(&brief.removals) {
                expect(format!("  - {item}"));
            }
        }
        None => expect("brief 無し".into()),
    }
    let level = serde_json::to_value(design.risk.level).unwrap();
    expect(format!("- level: `{}`", level.as_str().unwrap_or_default()));
    for reason in &design.risk.reasons {
        expect(format!("`{reason}`"));
    }
    match &design.review_policy {
        Some(policy) => {
            for class in &policy.high_risk {
                expect(format!("`{}`: ", class.class));
                for path in &class.paths {
                    expect(format!("`{path}`"));
                }
            }
            for (name, source) in &policy.invariants {
                expect(format!("`{name}`: `{source}`"));
            }
        }
        None => expect("この repo の review 方針を読めなかった".into()),
    }
    if count(prompt, DESIGN_HEADING) != 1 {
        problems.push(format!(
            "the design review section appears {} times",
            count(prompt, DESIGN_HEADING)
        ));
    }
    if prompt.contains(CRITERIA_HEADING) {
        problems.push("the design review prompt asks for criterion verdicts".into());
    }
    problems
}

/// final review の prompt の不備（設計 review の節がある、受け入れ条件の判定の節が無い）。
fn final_prompt_problems(prompt: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if prompt.contains(DESIGN_HEADING) {
        problems.push("the final review prompt has the design review section".into());
    }
    if !prompt.contains(CRITERIA_HEADING) {
        problems.push("the final review prompt has no criteria to judge".into());
    }
    problems
}

/// Result 節の例の JSON（見出しの後で最初の、バッククォートで囲んだ `{…}` の 1 行）。
fn result_example(prompt: &str) -> Result<serde_json::Value, String> {
    let start = prompt
        .find(RESULT_HEADING)
        .ok_or("the prompt has no Result section")?;
    let line = prompt[start..]
        .lines()
        .find_map(|l| l.strip_prefix("`{").and_then(|r| r.strip_suffix("}`")))
        .ok_or("the Result section has no example JSON line")?;
    serde_json::from_str(&format!("{{{line}}}"))
        .map_err(|e| format!("the example JSON is not JSON: {e}"))
}

/// 例の JSON の判定の値だけを合格に差し替えた `review.json`。形が run の種類に合わなければ `Err`。
fn answer_from_prompt(review: &ReviewRequest, prompt: &str) -> Result<serde_json::Value, String> {
    let mut example = result_example(prompt)?;
    if review.design.is_some() {
        if example["verdicts"] != serde_json::json!([]) {
            return Err(format!("the design example has verdicts: {example}"));
        }
        let shown = example.to_string();
        let design = example
            .get_mut("design")
            .and_then(|d| d.as_object_mut())
            .filter(|d| d.contains_key("pass") && d.contains_key("findings"))
            .ok_or_else(|| format!("the example has no design result: {shown}"))?;
        design.insert("pass".into(), serde_json::json!(true));
        design.insert("findings".into(), serde_json::json!([]));
    } else {
        if example.get("design").is_some() {
            return Err(format!("the final review example has design: {example}"));
        }
        let template = example["verdicts"]
            .get(0)
            .cloned()
            .ok_or_else(|| format!("the example has no verdict: {example}"))?;
        let verdicts: Vec<_> = review
            .criteria
            .iter()
            .map(|c| {
                let mut verdict = template.clone();
                verdict["criterion"] = serde_json::json!(c);
                verdict["pass"] = serde_json::json!(true);
                if let Some(v) = verdict.as_object_mut() {
                    v.remove("repair");
                }
                verdict
            })
            .collect();
        example["verdicts"] = serde_json::json!(verdicts);
        if review.health.is_some() != example.get("health").is_some() {
            return Err(format!(
                "health in the request and the example differ: {example}"
            ));
        }
        if let Some(health) = example.get_mut("health") {
            health["findings"] = serde_json::json!([]);
        }
    }
    let output: ReviewOutput = serde_json::from_value(example.clone())
        .map_err(|e| format!("the answer is not a ReviewOutput: {e}: {example}"))?;
    if output.design.is_some() != review.design.is_some() {
        return Err(format!(
            "the answer's design does not match the run: {example}"
        ));
    }
    Ok(example)
}

fn flow_adapter(inner: Arc<ScenarioAdapter>) -> Arc<FlowAdapter> {
    Arc::new(FlowAdapter {
        inner,
        runs: StdMutex::new(Vec::new()),
        problems: StdMutex::new(Vec::new()),
    })
}

/// `[review]` のある案件の repo で、planner が `change` を書く計画を出す流れを [`FlowAdapter`] で回す。
async fn run_prompt_flow(change: &'static str, new_state: bool) -> (Outcome, Arc<FlowAdapter>) {
    let inner = ScenarioAdapter::new(vec![plan(change, new_state)], change, vec![]);
    let adapter = flow_adapter(inner.clone());
    let out = run_flow_with(true, inner, adapter.clone()).await;
    (out, adapter)
}

/// 人が採用した計画の最初の WU の run だけ `yield.plan_issue` を返し、ほかの run は [`FlowAdapter`] に渡す。
struct PlanIssueOnce {
    flow: Arc<FlowAdapter>,
    yielded: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl WorkerAdapter for PlanIssueOnce {
    fn id(&self) -> &str {
        self.flow.id()
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        if req.context.work_unit.is_some()
            && !self.yielded.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.flow
                .inner
                .seen
                .lock()
                .unwrap()
                .push(req.context.clone());
            return Ok(RunOutcome {
                terminal: Terminal::Yielded {
                    checkpoint: serde_json::json!({
                        "plan_issue": "the remote module must be split before this unit",
                    }),
                    usage: None,
                },
                exit_code: Some(0),
            });
        }
        self.flow.run(req, run_id, limits, sink).await
    }
}

/// remote workspace（クラスタ `sirius` の `/work/x`、手元の写しは `<root>/<task_id>`）の task で、planner が `change`
/// を書き brief を付けた計画を出す流れを [`FlowAdapter`] で回す。ssh・rsync は偽のコマンド（`true`）に差し替え、外部
/// ネットワークに出ない（`session_resume` の remote workspace の試験と同じ設定）。
///
/// 今の Complexity Gate は remote workspace の task を `atomic/out-of-scope` にする（`WorkspaceSpec::local_mode` が
/// `Remote` に `Shared` を返す）ので、最初の dispatch では planner に届かない。remote の task が planner の計画を受ける
/// 本番の経路は、人が採用した計画（`PUT /tasks/{id}/execution-plan` = `task_ops::execution::adopt_plan`、origin
/// human）の WU が `yield.plan_issue` を返した後の replan なので、この試験はその経路で planner を起こす。
async fn run_remote_flow(change: &'static str) -> (Outcome, Arc<FlowAdapter>) {
    let inner = ScenarioAdapter::new(vec![plan(change, true)], change, vec![]);
    let flow = flow_adapter(inner.clone());
    let adapter = Arc::new(PlanIssueOnce {
        flow: flow.clone(),
        yielded: std::sync::atomic::AtomicBool::new(false),
    });
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = compound_task(dir.path());
    task.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/work/x"),
        mode: None,
    };
    store.insert(&task).unwrap();
    let human_plan: task_core::ExecutionPlanSpec =
        serde_json::from_str(&plan(change, false)).unwrap();
    task_ops::execution::adopt_plan(
        store.as_ref(),
        task.id,
        human_plan,
        task_core::PlanOrigin::Human,
        None,
        super::tree::tree_limits(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    d.config.clusters.insert(
        "sirius".into(),
        cluster_spec_with_auth("sirius", "sirius", "manual"),
    );
    d.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| true));
    d.set_cluster_ssh_command_override(vec!["true".to_string()]);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.limits = super::tree::tree_limits();
    run_until_idle(&mut d, 400).await;
    let seen = inner.seen.lock().unwrap().clone();
    let out = Outcome {
        store,
        task,
        seen,
        _repo: dir,
        _root: root,
    };
    (out, flow)
}

/// 人への受信箱のうち、`task` に触れる項目の種類。
fn inbox_kinds_for(out: &Outcome) -> Vec<task_ops::human_inbox::InboxKind> {
    let inbox = task_ops::human_inbox::human_inbox(
        out.store.as_ref(),
        None,
        &task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                out._root.path().to_path_buf(),
                None,
            ),
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            max_requeues: 5,
            clusters: Default::default(),
        },
        OffsetDateTime::now_utc(),
        &|_, _| vec![],
    )
    .unwrap();
    inbox
        .items
        .iter()
        .filter(|item| item.task_ids().any(|id| id == out.task.id))
        .map(|item| item.kind)
        .collect()
}

/// 付記 (a)・流れ全体の試験 (1) 高 risk: store の class の path を書き `new_state` を宣言した計画は、本物の prompt
/// を読んだ reviewer の合格で採用される。設計 review の run は 1 回、planner の run は 1 回で、人の承認（`PlanGate`）
/// にも決定にも回らず、task は合格で終わる。
#[tokio::test]
async fn health_review_flow_high_risk_plan_is_adopted_after_the_prompt_review() {
    let (out, adapter) = run_prompt_flow(STORE_PATH, true).await;
    adapter.assert_no_problems();

    let prompts = adapter.design_review_prompts();
    assert_eq!(prompts.len(), 1, "one design review run");
    // 試験が計画に書いた値が prompt まで届く（`design_prompt_problems` は RunContext の値と prompt を照らす）。
    for needle in [
        "rows are rewritten in place",
        "rows.revision column",
        "`append a column`（chosen=true）",
        "- level: `high`",
        "`store`: `^crates/task-store/`",
    ] {
        assert!(prompts[0].contains(needle), "{needle:?} in {}", prompts[0]);
    }
    assert_eq!(out.design_review_runs(), 1);
    assert_eq!(out.planner_runs(), 1, "no planner retry");
    let reviewed = out
        .position(|e| matches!(e, Event::DesignReviewed { pass: true, .. }))
        .expect("the design review passes");
    let adopted = out
        .position(|e| matches!(e, Event::ExecutionPlanned { .. }))
        .expect("the plan is adopted");
    assert!(reviewed < adopted);
    assert!(
        out.position(|e| matches!(e, Event::DesignReviewed { pass: false, .. }))
            .is_none()
    );
    assert!(
        out.position(|e| matches!(
            e,
            Event::PlanApprovalRequested { .. }
                | Event::PlanGateDecided { .. }
                | Event::DecisionRequested { .. }
        ))
        .is_none(),
        "no human plan gate or decision: {:?}",
        out.events()
    );
    assert_eq!(out.work_unit_runs(), 1);
    assert_eq!(adapter.final_review_prompts().len(), 1);
    assert_eq!(out.status(), Status::Done, "{:?}", out.events());
}

/// 付記 (d) D1・D2・流れ全体の試験 (2) remote workspace: 方針は `Unavailable("remote workspace")` で、brief を付けた
/// 計画の risk は `unknown`（fail closed）になり、設計 review に回る。本物の prompt は方針を読めなかった旨とその理由を
/// 示し、Result 節に `design` の形を出す。reviewer が例の JSON の `design.pass = true` を書くと、設計 review の 1 run
/// で計画が採用される。planner の再試行も人の承認（`PlanGate`・決定・受信箱の項目）も起きない。planner は人の計画の
/// WU の `plan_issue` による replan で起きる（[`run_remote_flow`]）。採用の後（WU・final review）の挙動はここでは
/// 確かめない。
#[tokio::test]
async fn health_review_flow_remote_workspace_plan_is_adopted_after_one_design_review() {
    let (out, adapter) = run_remote_flow(NORMAL_PATH).await;
    adapter.assert_no_problems();

    let events = out.events();
    let risks: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::DesignReviewRequested { risk, .. } => Some(risk),
            _ => None,
        })
        .collect();
    assert_eq!(risks.len(), 1, "one design review request: {events:?}");
    assert_eq!(risks[0].level, RiskLevel::Unknown);
    assert!(
        risks[0]
            .reasons
            .iter()
            .any(|r| r == "review_policy_unavailable:remote workspace"),
        "{:?}",
        risks[0].reasons
    );

    let prompts = adapter.design_review_prompts();
    assert_eq!(prompts.len(), 1, "one design review run");
    for needle in [
        "- level: `unknown`",
        "この repo の review 方針を読めなかった。理由: `review_policy_unavailable:remote workspace`。",
        "設計 brief だけで判定する",
        "rows are rewritten in place",
        "`append a column`（chosen=true）",
    ] {
        assert!(prompts[0].contains(needle), "{needle:?} in {}", prompts[0]);
    }
    assert!(
        !prompts[0].contains("- owner 地図"),
        "no review policy is shown: {}",
        prompts[0]
    );
    let example = result_example(&prompts[0]).unwrap();
    assert_eq!(example["verdicts"], serde_json::json!([]), "{example}");
    assert!(
        example["design"].get("pass").is_some() && example["design"].get("findings").is_some(),
        "the Result section shows the design result: {example}"
    );

    assert_eq!(out.design_review_runs(), 1);
    assert_eq!(out.planner_runs(), 1, "no planner retry");
    let reviewed = out
        .position(|e| matches!(e, Event::DesignReviewed { pass: true, .. }))
        .expect("the design review passes");
    // 人の計画の採用（origin human）の後、replan の計画（origin planner）が設計 review の合格の後に採用される。
    let origins: Vec<(usize, task_core::PlanOrigin)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Event::ExecutionPlanned { origin, .. } => Some((i, *origin)),
            _ => None,
        })
        .collect();
    assert_eq!(
        origins.iter().map(|(_, o)| *o).collect::<Vec<_>>(),
        vec![task_core::PlanOrigin::Human, task_core::PlanOrigin::Planner],
        "{events:?}"
    );
    assert!(origins[0].0 < reviewed && reviewed < origins[1].0);
    assert!(
        out.position(|e| matches!(e, Event::DesignReviewed { pass: false, .. }))
            .is_none()
    );
    assert!(
        out.position(|e| matches!(
            e,
            Event::PlanApprovalRequested { .. }
                | Event::PlanGateDecided { .. }
                | Event::DecisionRequested { .. }
        ))
        .is_none(),
        "no human plan gate or decision: {events:?}"
    );
    assert_eq!(inbox_kinds_for(&out), vec![], "no inbox item for a human");
}

/// 付記 (b)・流れ全体の試験 (3) 通常の変更: 高 risk の class に触れない計画は設計 review の run なしに採用され、
/// WU の run の prompt に repo 固有の review 方針の節（`change_rules::repo_section`）がちょうど 1 回出る。final review
/// の prompt には出ない（方針は `ReviewRequest.health` で受け取る）。
#[tokio::test]
async fn health_review_flow_normal_change_puts_repo_rules_in_the_work_unit_prompt_only() {
    let (out, adapter) = run_prompt_flow(NORMAL_PATH, false).await;
    adapter.assert_no_problems();

    assert_eq!(out.design_review_runs(), 0, "no design review run");
    assert!(adapter.design_review_prompts().is_empty());
    let units: Vec<PromptRun> = adapter
        .runs
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.context.work_unit.is_some())
        .cloned()
        .collect();
    assert_eq!(units.len(), 1, "one work unit run");
    let policy = units[0]
        .context
        .review_policy()
        .expect("the work unit run carries the review policy");
    let prompt = &units[0].prompt;
    assert_eq!(count(prompt, REPO_RULES_HEADING), 1, "{prompt}");
    let section = &prompt[prompt.find(REPO_RULES_HEADING).unwrap()..];
    for class in &policy.high_risk {
        assert!(
            section.contains(&format!("`{}`: ", class.class)),
            "{section}"
        );
    }
    assert!(
        section.contains("`store`: `^crates/task-store/`"),
        "{section}"
    );

    let finals = adapter.final_review_prompts();
    assert_eq!(finals.len(), 1, "one final review run");
    assert_eq!(count(&finals[0], REPO_RULES_HEADING), 0, "{}", finals[0]);
    assert_eq!(out.status(), Status::Done, "{:?}", out.events());
}
