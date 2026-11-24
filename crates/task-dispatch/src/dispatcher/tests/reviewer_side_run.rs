//! ADR 2026-11-22-parent-checkpoint-evaluator D2.3: task の status を変えない reviewer run（side review）の共通の起動口
//! （`health_review/design_gate.rs` の `start_side_review`・`on_side_review_finished`・`holds_side_review`）。
//!
//! 守る不変条件:
//! - 起動口を経た設計 review が、一般化の前と同じ events（起動から `DesignReviewed` まで）と同じ `RunRequest`
//!   （runs/<run_id>/request.json に書かれる形）を書く。期待値は一般化の前の base の code で取った sha256 で固定する。
//! - 終わりは完了の種類（`SideReviewKind`）を持つ `Completion::SideReview` で届き、種類の判定（設計 review は
//!   `design_verdict::settle_design_review`）に振り分けられる。
//! - 走っている設計 review の run は `abort_stale_runs` で止められない（`holds_side_review`）。
//!
//! 既存の `health_review_` の試験は採用・replan・人への gate の結果だけを見て、events の並びと request.json の byte 列、
//! 完了の channel の振り分けの口を直接は見ない。外部ネットワークに出ない。時計に依存しない（run の終わりは tick の
//! 間の短い sleep で待つ）。

use super::*;

use super::health_review_design_gate::{DIGEST, STORE_PATH, checker_repo, stub, v3_plan_with};
use sha2::{Digest, Sha256};

/// 設計 review の run を受けた `RunRequest` を全部残し、`review.json` に合格の `design` を書いて `done` で終える adapter。
/// `hang` なら `review.json` を書かずに長く終わらない（`abort_stale_runs` の試験）。
struct CaptureAdapter {
    requests: StdMutex<Vec<RunRequest>>,
    hang: bool,
}

#[async_trait]
impl WorkerAdapter for CaptureAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.requests.lock().unwrap().push(req.clone());
        if self.hang {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
        let json = serde_json::json!({"verdicts": [], "design": {"pass": true, "findings": []}});
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        std::fs::write(
            req.artifacts_dir.join(crate::review::REVIEW_FILE_NAME),
            json.to_string(),
        )
        .unwrap();
        Ok(done_outcome())
    }
}

fn done_outcome() -> RunOutcome {
    RunOutcome {
        terminal: Terminal::Done {
            summary: "reviewed".into(),
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    }
}

struct Fixture {
    store: Arc<SqliteStore>,
    d: Dispatcher,
    task: Task,
    adapter: Arc<CaptureAdapter>,
    repo: tempfile::TempDir,
    root: tempfile::TempDir,
}

/// 高 risk の計画（`STORE_PATH` に触れ、brief が `new_state` を宣言する）の設計 review を待つ task。task の id と
/// 時刻は固定する（request.json の byte 列を比べるため）。
fn pending(hang: bool) -> Fixture {
    let repo = checker_repo(&stub("base"));
    let root = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = compound_task(repo.path());
    task.id = TaskId(ulid::Ulid::from_parts(0, 0x5eed));
    task.created_at = OffsetDateTime::UNIX_EPOCH;
    task.updated_at = OffsetDateTime::UNIX_EPOCH;
    task.workspace = WorkspaceSpec::Local {
        path: repo.path().to_path_buf(),
        mode: None,
    };
    store.create_task(&task, vec![]).unwrap();
    let spec: task_core::ExecutionPlanSpec =
        serde_json::from_str(&v3_plan_with(STORE_PATH, true)).unwrap();
    store
        .append_event(
            task.id,
            &Event::DesignReviewRequested {
                plan_sha256: task_core::execution_plan::plan_sha256(&spec),
                plan: Box::new(spec),
                risk: task_core::design_review::RiskAssessment {
                    level: task_core::design_review::RiskLevel::High,
                    reasons: vec![format!("path_class:store:{STORE_PATH}")],
                },
                policy_sha256: Some(DIGEST.to_string()),
            },
        )
        .unwrap();
    let adapter = Arc::new(CaptureAdapter {
        requests: StdMutex::new(Vec::new()),
        hang,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.limits = super::tree::tree_limits();
    Fixture {
        store,
        d,
        task,
        adapter,
        repo,
        root,
    }
}

impl Fixture {
    fn events(&self) -> Vec<Event> {
        self.store
            .events_for(self.task.id)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
    fn reviewer_run(&self) -> Option<String> {
        self.events().into_iter().find_map(|e| match e {
            Event::WorkerStarted {
                run_id,
                role: Some(RunRole::Reviewer),
                ..
            } => Some(run_id),
            _ => None,
        })
    }
    fn reviewed(&self) -> Vec<(String, bool)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::DesignReviewed {
                    review_run, pass, ..
                } => Some((review_run, pass)),
                _ => None,
            })
            .collect()
    }
    /// 実行ごとに変わる値（reviewer run の id・一時 dir の path）を置き換えた文。
    fn normalize(&self, text: &str, run_id: &str) -> String {
        text.replace(run_id, "<review-run>")
            .replace(&self.root.path().display().to_string(), "<root>")
            .replace(&self.repo.path().display().to_string(), "<repo>")
    }
}

async fn tick_until(f: &mut Fixture, done: impl Fn(&Fixture) -> bool) {
    for _ in 0..200 {
        f.d.tick().unwrap();
        if done(f) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition not reached: {:?}", f.events());
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 起動口を経た設計 review が、一般化の前と同じ events（`DesignReviewRequested` の後から `DesignReviewed` まで）と
/// 同じ `RunRequest` を書く。期待の sha256 は一般化の前の base の code（`start_design_review`・`run_design_reviewer`
/// の時点）でこの試験を走らせて取った値。run の id と一時 dir の path は置き換え、時刻・計測の値は落とす。
#[tokio::test]
async fn reviewer_side_run_design_review_writes_the_same_events_and_request() {
    let mut f = pending(false);
    tick_until(&mut f, |f| !f.reviewed().is_empty()).await;
    let run_id = f.reviewer_run().expect("the design review run started");
    assert_eq!(f.reviewed(), vec![(run_id.clone(), true)]);

    let events = f.events();
    let start = events
        .iter()
        .position(|e| matches!(e, Event::DesignReviewRequested { .. }))
        .unwrap();
    let end = events
        .iter()
        .position(|e| matches!(e, Event::DesignReviewed { .. }))
        .unwrap();
    let mut lines = Vec::new();
    for e in &events[start + 1..=end] {
        let mut v = serde_json::to_value(e).unwrap();
        strip_volatile(&mut v);
        lines.push(f.normalize(&v.to_string(), &run_id));
    }
    let events_text = lines.join("\n");

    let requests = f.adapter.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "one reviewer run reached the adapter");
    let mut snapshot = requests[0].clone();
    snapshot.context.routing_context_ref = None;
    let mut request = serde_json::to_value(&snapshot).unwrap();
    strip_volatile(&mut request);
    // 合成の `Review` task の id は run ごとに新しい。
    let request_text = f
        .normalize(&serde_json::to_string_pretty(&request).unwrap(), &run_id)
        .replace(&snapshot.task.id.to_string(), "<review-task>");
    let review = &request["context"]["review"];
    assert!(
        review["design"].is_object()
            && review["summary"] == ""
            && review["evidence"] == serde_json::json!([])
            && review["criteria"] == serde_json::json!([])
            && review.as_object().is_some_and(|o| o.len() == 4),
        "the request carries only the design section: {review}"
    );

    assert_eq!(
        (sha256_hex(&events_text), sha256_hex(&request_text)),
        (EVENTS_SHA256.to_string(), REQUEST_SHA256.to_string()),
        "events:\n{events_text}\nrequest:\n{request_text}"
    );
}

/// base の code（一般化の前）で取った、正規化した events の文の sha256。
const EVENTS_SHA256: &str = "04ff5244a8d6bb39560a1a75321f4b02e2d7ff6aba558791fc05649c9d615e80";
/// base の code（一般化の前）で取った、正規化した `RunRequest` の文の sha256。
const REQUEST_SHA256: &str = "a0d829ac94086ada12418eb53b9c0044428e04a8ecaa59dd110da51fa6d76e0d";

/// 完了の種類での振り分け: `Completion::SideReview`（`SideReviewKind::Design`）を channel に送ると、次の tick の
/// `drain_completions` が `on_side_review_finished` に渡し、`WorkerFinished`（role reviewer）を書いてから設計 review の判定
/// （`settle_design_review`）が `review.json` の `design` を読んで `DesignReviewed` を追記する。final review の判定
/// （`ReviewVerdict`）には流れない。run は起動口を経ずに送るので、振り分けの口だけを見る。
#[tokio::test]
async fn reviewer_side_run_completion_kind_routes_to_the_design_verdict() {
    let mut f = pending(false);
    let artifacts = f.d.artifacts_dir(&f.task, &f.d.task_dir(&f.task).unwrap());
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(
        artifacts.join(crate::review::REVIEW_FILE_NAME),
        serde_json::json!({"verdicts": [], "design": {"pass": false, "findings": []}}).to_string(),
    )
    .unwrap();
    f.d.coord
        .completion_sender()
        .send(Completion::SideReview(Box::new(
            super::super::design_gate::SideReviewEnd {
                task_id: f.task.id,
                run_id: "side-run-1".into(),
                provider: "p1".into(),
                kind: super::super::design_gate::SideReviewKind::Design,
                result: Ok(done_outcome()),
            },
        )))
        .unwrap();
    f.d.tick().unwrap();
    assert_eq!(f.reviewed(), vec![("side-run-1".to_string(), false)]);
    let events = f.events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::WorkerFinished { run_id, role: Some(RunRole::Reviewer), outcome, .. }
                if run_id == "side-run-1" && outcome == "done: reviewed"
        )),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ReviewVerdict { .. })),
        "a side review never reaches the final review verdict: {events:?}"
    );
}

/// ADR 2026-11-24-planner-supply-failure-cooldown D4・D7-5: 設計 review の run が供給側の失敗（認証の失敗）で終わったら、
/// worker・planner と同じ `record_supply_failure` で cooldown にし（この run は pool の account を持たないので source の
/// 暗黙の account。`ProviderThrottled{reason: auth_failed}`）、`end` を `HarnessError{Supply}` にして判定に流さない
/// （`DesignReviewed` を足さない）。守る不変条件: 供給の失敗で設計 review の結果を作らず、同じ供給元を cooldown にする。
/// 既存の試験は side review の run を `Done` でしか終わらせず、修正前は cooldown が付かなかった。
#[tokio::test]
async fn reviewer_side_run_supply_failure_cools_down_and_reads_no_verdict() {
    let mut f = pending(false);
    f.d.coord
        .completion_sender()
        .send(Completion::SideReview(Box::new(
            super::super::design_gate::SideReviewEnd {
                task_id: f.task.id,
                run_id: "side-run-auth".into(),
                provider: "p1".into(),
                kind: super::super::design_gate::SideReviewKind::Design,
                result: Err(AdapterError::AuthFailed(
                    "Failed to authenticate: this login reached its time limit".into(),
                )),
            },
        )))
        .unwrap();
    f.d.tick().unwrap();
    assert!(
        f.reviewed().is_empty(),
        "no DesignReviewed: {:?}",
        f.reviewed()
    );
    let events = f.events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::WorkerFinished { run_id, role: Some(RunRole::Reviewer), end, .. }
                if run_id == "side-run-auth" && *end == Some(task_ops::tree::SUPPLY_FAILED_END)
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::ProviderThrottled { provider, reason: Some(r), .. }
                if provider == "p1" && r == "auth_failed"
        )),
        "{events:?}"
    );
}

/// `abort_stale_runs` の例外（`holds_side_review`）: 走っている設計 review の run は lease を持たない（task は `ready`）が
/// 止められない。別の run の id は真にならない。run が終わって `WorkerFinished` が積まれた後は真にならない。
#[tokio::test]
async fn reviewer_side_run_holds_the_running_design_review() {
    let mut f = pending(true);
    tick_until(&mut f, |f| f.reviewer_run().is_some()).await;
    let run_id = f.reviewer_run().unwrap();
    assert_eq!(
        f.store.get(f.task.id).unwrap().unwrap().status,
        Status::Ready
    );
    assert!(f.d.holds_side_review(&f.task, &run_id).unwrap());
    assert!(!f.d.holds_side_review(&f.task, "another-run").unwrap());
    f.d.abort_stale_runs().unwrap();
    assert!(
        f.d.supervisor.has_task_run(f.task.id),
        "the running design review run is not aborted"
    );
    f.store
        .append_event(
            f.task.id,
            &Event::WorkerFinished {
                run_id: run_id.clone(),
                outcome: "interrupted: test".into(),
                usage: None,
                role: Some(RunRole::Reviewer),
                metrics: None,
                end: None,
            },
        )
        .unwrap();
    assert!(!f.d.holds_side_review(&f.task, &run_id).unwrap());
}

/// 実行ごとに変わる時刻・計測の欄を落とす（`at`・`*_at`・`wall_ms`・`ts`）。
fn strip_volatile(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|k, _| !(k == "at" || k == "ts" || k == "wall_ms" || k.ends_with("_at")));
            for value in map.values_mut() {
                strip_volatile(value);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_volatile),
        _ => {}
    }
}
