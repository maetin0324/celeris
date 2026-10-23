use super::*;
use crate::daemon::classify_stage::RequestClassifier;
use async_trait::async_trait;
use jev_classifier::{
    Answer, ApiKey, ApiKeyProvider, Classification, ClassifyOutcome, Specificity, WorkNature,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use task_core::SqliteStore;
use task_core::execution_plan::{WorkUnitContext, WorkUnitKind, WorkUnitSpec, WorkUnitStatus};

/// 呼ばれた回数を数え、`script` の先頭から結果を返す（尽きたら成功）。
struct Fake {
    calls: AtomicUsize,
    script: Mutex<Vec<Result<ClassifyOutcome, JevError>>>,
}

#[async_trait]
impl RequestClassifier for Fake {
    async fn classify(
        &self,
        _: &ClassifyInput,
        _: &dyn ApiKeyProvider,
    ) -> Result<ClassifyOutcome, JevError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut s = self.script.lock().expect("lock");
        if s.is_empty() {
            Ok(classified())
        } else {
            s.remove(0)
        }
    }
}

struct NoKey;
impl ApiKeyProvider for NoKey {
    fn api_key(&self) -> Option<ApiKey> {
        None
    }
}

fn classified() -> ClassifyOutcome {
    ClassifyOutcome::Classified {
        classification: Box::new(Classification {
            specificity: Answer {
                value: Specificity::Specified,
                probability: Some(0.9),
                classification_confidence: Some(0.9),
            },
            work_nature: Answer {
                value: WorkNature::Implementation,
                probability: None,
                classification_confidence: None,
            },
            failure_kind: None,
            response_model: None,
            model_snapshot: None,
            cost_usd: None,
            model_id: "m".into(),
            schema_version: "1".into(),
            prompt_version: "1".into(),
        }),
        cached: false,
    }
}

type Fixture = (
    ClassifyTick,
    Arc<Fake>,
    ClassificationBoard,
    SqliteStore,
    Task,
);

fn fixture(script: Vec<Result<ClassifyOutcome, JevError>>) -> Fixture {
    fixture_with(script, None)
}

/// `routing_override` は `POST /tasks` と同じ JSON（task 欄の routing 固定）。
fn fixture_with(
    script: Vec<Result<ClassifyOutcome, JevError>>,
    routing_override: Option<serde_json::Value>,
) -> Fixture {
    let store = SqliteStore::open_in_memory().expect("store");
    let spec: task_ops::add::NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "t", "objective": "fix the typo", "status": "ready", "acceptance": [{"type": "command", "cmd": "true"}]
    }))
    .expect("spec");
    let task =
        task_ops::add::create_task(&store, spec, time::OffsetDateTime::now_utc()).expect("task");
    if let Some(o) = routing_override {
        let o = serde_json::from_value(o).expect("routing_override");
        store
            .set_task_routing_override(task.id, &o, "2026-10-16")
            .expect("pin routing_override");
    }
    let fake = Arc::new(Fake {
        calls: AtomicUsize::new(0),
        script: Mutex::new(script),
    });
    let board = ClassificationBoard::default();
    let stage = ClassificationStage::new(
        fake.clone(),
        Arc::new(NoKey),
        CredentialGate {
            secret_id: "jev_api_key".into(),
            provider: "jev".into(),
            purpose: "x".into(),
        },
        board.clone(),
    );
    (
        ClassifyTick::with_stage(stage, board.clone()),
        fake,
        board,
        store,
        task,
    )
}

fn unit(task: &Task, key: &str, seq: u32) -> WorkUnitRow {
    let spec = WorkUnitSpec {
        key: key.into(),
        kind: WorkUnitKind::Implement,
        title: key.into(),
        objective: format!("do {key}"),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    WorkUnitRow::new(
        format!("{}-{key}", task.id),
        task.id.to_string(),
        "plan".into(),
        seq,
        spec,
        WorkUnitStatus::Ready,
        "2026-10-15T00:00:00Z".into(),
    )
}

#[tokio::test]
async fn enabled_fills_the_board_for_a_ready_task_and_is_idempotent() {
    let (tick, fake, board, store, task) = fixture(vec![]);
    assert_eq!(tick.run(&store, RoutingMode::DecisionAssisted).await, 1);
    let item = board.get(&task.id.to_string()).expect("classified");
    assert_eq!(item.entry.status, ClassifierStatus::JevOk);
    // 2 回目は分類済みなので呼ばない。
    assert_eq!(tick.run(&store, RoutingMode::DecisionAssisted).await, 0);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

/// ADR 2026-10-22-jev-classification-before-launch D1: ready の WU に加え、pending の WU も ready になる前に
/// 分類する（同じ tick で ready になって起動しても分類済み）。統合 WU は LLM run を起こさないので分類しない。
#[tokio::test]
async fn pending_work_units_are_classified_ahead_but_integrate_units_are_not() {
    let (tick, _fake, board, store, task) = fixture(vec![]);
    let a = unit(&task, "a", 0);
    let mut b = unit(&task, "b", 1);
    b.status = WorkUnitStatus::Pending;
    let mut i = unit(&task, "i", 2);
    i.status = WorkUnitStatus::Pending;
    i.kind = WorkUnitKind::Integrate;
    i.spec.kind = WorkUnitKind::Integrate;
    store
        .work_units_apply(
            task.id,
            vec![a.clone(), b.clone(), i.clone()],
            vec![],
            vec![],
        )
        .expect("units");
    assert_eq!(tick.run(&store, RoutingMode::DecisionAssisted).await, 2);
    assert!(board.get(&a.id).is_some());
    assert!(
        board.get(&b.id).is_some(),
        "pending unit is classified ahead"
    );
    assert!(
        board.get(&i.id).is_none(),
        "integrate unit is not classified"
    );
    assert!(board.get(&task.id.to_string()).is_none());
}

/// 走っている WU があるだけの Running task でも、pending の WU を先読みする。計画の無い Running task は分類しない。
#[tokio::test]
async fn running_task_with_only_a_running_unit_still_classifies_its_pending_units() {
    let (tick, fake, board, store, task) = fixture(vec![]);
    let mut a = unit(&task, "a", 0);
    a.status = WorkUnitStatus::Running;
    let mut b = unit(&task, "b", 1);
    b.status = WorkUnitStatus::Pending;
    store
        .work_units_apply(task.id, vec![a.clone(), b.clone()], vec![], vec![])
        .expect("units");
    store
        .apply_transition(task.id, task_core::Trigger::Dispatch, None)
        .expect("running");
    tick.run(&store, RoutingMode::DecisionAssisted).await;
    assert!(board.get(&b.id).is_some());
    let calls = fake.calls.load(Ordering::SeqCst);
    // 分類済みの subject は再分類しない。
    assert_eq!(tick.run(&store, RoutingMode::DecisionAssisted).await, 0);
    assert_eq!(fake.calls.load(Ordering::SeqCst), calls);
}

/// 1 回の run で新しく分類する件数には上限がある（残りは次の loop）。
#[tokio::test]
async fn classifications_per_run_are_capped() {
    let (tick, _fake, board, store, task) = fixture(vec![]);
    let mut units = vec![unit(&task, "u0", 0)];
    for n in 1..12u32 {
        let mut u = unit(&task, &format!("u{n}"), n);
        u.status = WorkUnitStatus::Pending;
        units.push(u);
    }
    store
        .work_units_apply(task.id, units.clone(), vec![], vec![])
        .expect("units");
    assert_eq!(
        tick.run(&store, RoutingMode::DecisionAssisted).await,
        MAX_CLASSIFICATIONS_PER_RUN
    );
    assert_eq!(
        tick.run(&store, RoutingMode::DecisionAssisted).await,
        12 - MAX_CLASSIFICATIONS_PER_RUN
    );
    assert!(units.iter().all(|u| board.get(&u.id).is_some()));
}

#[tokio::test]
async fn fixed_mode_leaves_the_board_empty_and_never_calls_the_classifier() {
    let (tick, fake, board, store, task) = fixture(vec![]);
    assert_eq!(tick.run(&store, RoutingMode::Fixed).await, 0);
    assert!(board.get(&task.id.to_string()).is_none());
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

/// ADR 2026-10-16-mlsys-measure-task-routing: task 欄の mode が config の mode より優先する（分類段も task ごとに解決する）。
#[tokio::test]
async fn task_routing_override_beats_the_config_mode() {
    // config は decision_assisted、task は fixed に固定 → 分類器を呼ばず board も空。
    let (tick, fake, board, store, task) =
        fixture_with(vec![], Some(serde_json::json!({"mode": "fixed"})));
    assert_eq!(tick.run(&store, RoutingMode::DecisionAssisted).await, 0);
    assert!(board.get(&task.id.to_string()).is_none());
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    // config は fixed、task は decision_assisted に固定 → 分類する。
    let (tick, fake, board, store, task) = fixture_with(
        vec![],
        Some(serde_json::json!({"mode": "decision_assisted"})),
    );
    assert_eq!(tick.run(&store, RoutingMode::Fixed).await, 1);
    assert!(board.get(&task.id.to_string()).is_some());
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn disabled_config_builds_no_stage() {
    let built = ClassifyTick::build(
        &JevSectionConfig::default(),
        None,
        ClassificationBoard::default(),
    )
    .expect("build");
    assert!(built.is_none());
    let enabled = JevSectionConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(
        ClassifyTick::build(&enabled, None, ClassificationBoard::default())
            .expect("build")
            .is_some()
    );
}

#[tokio::test]
async fn credential_failures_are_reclassified_but_other_fallbacks_are_not() {
    let (tick, fake, board, store, task) = fixture(vec![Err(JevError::KeyMissing {
        secret_id: "jev_api_key".into(),
    })]);
    tick.run(&store, RoutingMode::DecisionAssisted).await;
    let item = board.get(&task.id.to_string()).expect("item");
    assert!(item.credential.is_some());
    // 鍵登録後（次の ready）は同じ subject を分類し直して JevOk になる。
    assert_eq!(tick.run(&store, RoutingMode::DecisionAssisted).await, 1);
    assert_eq!(
        board.get(&task.id.to_string()).expect("item").entry.status,
        ClassifierStatus::JevOk
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 2);

    // 429 などの fallback は鍵の問題ではないので分類し直さない。
    let (tick, fake, board, store, task) = fixture(vec![Ok(ClassifyOutcome::FallbackToFixed {
        reason: jev_classifier::FallbackReason::RateLimited,
    })]);
    tick.run(&store, RoutingMode::DecisionAssisted).await;
    assert_eq!(tick.run(&store, RoutingMode::DecisionAssisted).await, 0);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert!(
        board
            .get(&task.id.to_string())
            .expect("item")
            .credential
            .is_none()
    );
}
