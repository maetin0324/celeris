//! ADR 2026-10-11 llm-source-harness-selection: the reviewer run is launched on the route that
//! `select_route` admits (harness from the reviewer's harness request, then source×model and
//! account), and is not launched when no route can be admitted or bound.

use super::*;
use std::collections::BTreeMap;
use task_core::llm_route::{
    CandidateKey, HarnessId, HarnessRule, HarnessSpec, Registry, SourceSpec,
};
use task_worker::protocol::SelectedLlmRoute;

fn source(kind: &str, account: &str, model: &str) -> SourceSpec {
    SourceSpec {
        kind: kind.into(),
        api_format: "fake".into(),
        accounts: vec![account.into()],
        models: vec![model.into()],
        max_runs: None,
        account_max_runs: BTreeMap::new(),
        quota: None,
        endpoint: None,
        credential_ref: None,
    }
}

fn harness(kind: &str, source_kind: &str) -> HarnessSpec {
    HarnessSpec {
        kind: kind.into(),
        api_formats: ["fake".into()].into(),
        source_kinds: [source_kind.into()].into(),
        capabilities: Default::default(),
        command: None,
        args: vec![],
    }
}

fn key(source: &str, model: &str) -> CandidateKey {
    CandidateKey {
        source: source.into(),
        model: model.into(),
    }
}

/// The worker harness `h1` on `s1`, plus a reviewer harness `review_h` picked by the
/// `role = "reviewer"` rule on its own source `rs` (model `rm`, account `ra`).
/// `review_source_kind` is the source kind `review_h` accepts.
fn reviewer_registry(review_source_kind: &str) -> Registry {
    let mut r = Registry::default();
    r.sources.insert("s1".into(), source("fake", "a1", "m"));
    r.sources.insert("rs".into(), source("fake", "ra", "rm"));
    r.harnesses.insert("h1".into(), harness("instant", "fake"));
    r.harnesses
        .insert("review_h".into(), harness("review", review_source_kind));
    r.harness_selection.rules.push(HarnessRule {
        harness: "h1".into(),
        ..Default::default()
    });
    r.harness_selection.rules.push(HarnessRule {
        role: Some("reviewer".into()),
        harness: "review_h".into(),
        ..Default::default()
    });
    r.route_defaults.insert("h1".into(), vec![key("s1", "m")]);
    r.route_defaults
        .insert("review_h".into(), vec![key("rs", "rm")]);
    r
}

/// Writes a passing `review.json` and keeps the route it was started with.
struct CapturingReviewer {
    seen: Arc<StdMutex<Vec<Option<SelectedLlmRoute>>>>,
}

#[async_trait]
impl WorkerAdapter for CapturingReviewer {
    fn id(&self) -> &str {
        "review"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        assert_eq!(req.task.kind, TaskKind::Review);
        self.seen.lock().unwrap().push(req.llm_route.clone());
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        std::fs::write(
            req.artifacts_dir.join("review.json"),
            r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#,
        )
        .unwrap();
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "reviewed".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

fn worker_adapter() -> Arc<dyn WorkerAdapter> {
    Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: String::new(),
        delay: Duration::ZERO,
    })
}

/// `(run_id, adapter, model, provider, account)` of a reviewer `WorkerStarted`.
type ReviewerStart = (String, String, String, Option<String>, Option<String>);

fn reviewer_started(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<ReviewerStart> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerStarted {
                run_id,
                adapter,
                model,
                provider,
                account,
                role: Some(RunRole::Reviewer),
                ..
            } => Some((run_id, adapter, model, provider, account)),
            _ => None,
        })
        .collect()
}

/// Let the worker run finish and move the task to reviewing; then a few more review attempts.
async fn work_then_attempt_review(d: &mut Dispatcher, id: TaskId) {
    assert_eq!(d.tick().unwrap().dispatched, 1);
    await_worker_completion(d, id).await;
    for _ in 0..3 {
        d.tick().unwrap();
    }
}

#[tokio::test]
async fn route_select_dispatch_review_runs_on_the_selected_harness_model_and_account() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let worker = worker_adapter();
    let mut d = dispatcher(store.clone(), worker.clone(), 2);
    d.set_llm_route_registry(reviewer_registry("fake"));
    let seen = Arc::new(StdMutex::new(Vec::new()));
    d.set_harness_adapters(HashMap::from([
        (HarnessId::from("h1"), worker),
        (
            HarnessId::from("review_h"),
            Arc::new(CapturingReviewer { seen: seen.clone() }) as Arc<dyn WorkerAdapter>,
        ),
    ]));

    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);

    // The worker start input carries the selected route (ids only, no secret).
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![Some(SelectedLlmRoute {
            harness_id: "review_h".into(),
            source_id: "rs".into(),
            model_id: "rm".into(),
            account_id: "ra".into(),
            endpoint: Some(task_worker::protocol::LlmEndpointReference {
                base_url: None,
                api_format: "fake".into(),
            }),
            credential: None,
        })]
    );

    let started = reviewer_started(&store, task.id);
    assert_eq!(started.len(), 1, "{started:?}");
    let (review_run_id, adapter, model, source, account) = &started[0];
    assert_eq!(adapter, "review");
    assert_eq!(model, "rm");
    assert_eq!(source.as_deref(), Some("rs"));
    assert_eq!(account.as_deref(), Some("ra"));

    let events = events_of(&store, task.id);
    assert!(events.iter().any(|e| matches!(e,
        Event::HarnessSelected { harness_id } if harness_id == "review_h")));
    assert!(events.iter().any(|e| matches!(e,
        Event::LlmRouteSelected { source_id, model_id, account_id, .. }
            if source_id == "rs" && model_id == "rm" && account_id == "ra")));

    let conn = rusqlite::Connection::open(&db).unwrap();
    let row: (String, String, String, String, String, String) = conn
        .query_row(
            "SELECT role, selected_harness_id, selected_source_id, selected_model_id, \
             selected_account_id, model FROM runs WHERE run_id = ?1",
            [review_run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        (
            row.0.as_str(),
            row.1.as_str(),
            row.2.as_str(),
            row.3.as_str(),
            row.4.as_str(),
            row.5.as_str()
        ),
        ("reviewer", "review_h", "rs", "rm", "ra", "rm")
    );
}

#[tokio::test]
async fn route_select_dispatch_review_does_not_start_an_incompatible_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let worker = worker_adapter();
    let mut d = dispatcher(store.clone(), worker.clone(), 2);
    // `review_h` accepts no declared source kind: the reviewer route is refused before launch.
    d.set_llm_route_registry(reviewer_registry("other"));
    let seen = Arc::new(StdMutex::new(Vec::new()));
    d.set_harness_adapters(HashMap::from([
        (HarnessId::from("h1"), worker),
        (
            HarnessId::from("review_h"),
            Arc::new(CapturingReviewer { seen: seen.clone() }) as Arc<dyn WorkerAdapter>,
        ),
    ]));

    work_then_attempt_review(&mut d, task.id).await;

    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    assert!(seen.lock().unwrap().is_empty());
    assert!(reviewer_started(&store, task.id).is_empty());
    let events = events_of(&store, task.id);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::LlmRouteSelected { .. }))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ReviewVerdict { .. }))
    );
    // The refusal is recorded once, not on every deferred tick.
    let refused: Vec<String> = progress_msgs(&store, task.id)
        .into_iter()
        .filter(|m| m.contains("route refused") && m.contains("review_h"))
        .collect();
    assert_eq!(refused.len(), 1, "{refused:?}");
}

#[tokio::test]
async fn route_select_dispatch_review_does_not_start_without_a_harness_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let worker = worker_adapter();
    let mut d = dispatcher(store.clone(), worker.clone(), 2);
    // The route is admitted, but no adapter is built for `review_h`: fail closed.
    d.set_llm_route_registry(reviewer_registry("fake"));
    d.set_harness_adapters(HashMap::from([(HarnessId::from("h1"), worker)]));

    work_then_attempt_review(&mut d, task.id).await;

    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    assert!(reviewer_started(&store, task.id).is_empty());
    let msgs: Vec<String> = progress_msgs(&store, task.id)
        .into_iter()
        .filter(|m| m.contains("reviewer route cannot be launched"))
        .collect();
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("review_h"), "{msgs:?}");
}

/// A harness adapter that reports `id` (e.g. `claude-code`, which keeps a lead session) and runs
/// `inner`.
pub(super) struct IdAs {
    id: String,
    inner: Arc<dyn WorkerAdapter>,
}

impl IdAs {
    pub(super) fn wrap(id: &str, inner: Arc<dyn WorkerAdapter>) -> Arc<dyn WorkerAdapter> {
        Arc::new(IdAs {
            id: id.into(),
            inner,
        })
    }
}

#[async_trait]
impl WorkerAdapter for IdAs {
    fn id(&self) -> &str {
        &self.id
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let inner = self.inner.with_env(extra)?;
        Some(IdAs::wrap(&self.id, inner))
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.inner.run(req, run_id, limits, sink).await
    }
}
