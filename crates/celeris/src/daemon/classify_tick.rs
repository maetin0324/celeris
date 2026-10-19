//! tick 側の連携（ADR 2026-10-15-jev-wire D3）: ready の work unit（計画の無い task は task 自身）を
//! `ClassificationStage` に通して `ClassificationBoard` に置く。
//!
//! dispatcher・store は board を読むだけで Jev を呼ばない。ここは daemon 層で、`Dispatcher::tick` の**直前**に
//! 呼ばれる。`routing.mode = fixed` / `[jev] enabled = false` なら何もしない（board は空のまま）。
//! 同じ subject は board に分類済みの entry があれば再分類しない（冪等）。ただし鍵未登録・認証失敗の entry は
//! 鍵が登録し直されたかもしれないので、次に ready になったとき（credential 待ちの解除後）に分類し直す。

use std::sync::Arc;

use jev_classifier::{ClassifyInput, JevClassifier, JevError};
use task_core::route_select::{ClassifierStatus, RoutingMode};
use task_core::{Task, TaskStore, WorkUnitRow, runnable_work_units};
use task_dispatch::{ClassificationBoard, CredentialGate};

use super::classify_stage::ClassificationStage;
use super::jev_key::SecretsDirKeyProvider;
use crate::config::JevSectionConfig;

/// 1 tick で見る task の上限（dispatcher の窓と同程度。分類は ready の先頭から）。
const WINDOW: usize = 32;

pub(crate) struct ClassifyTick {
    stage: ClassificationStage,
    board: ClassificationBoard,
}

impl ClassifyTick {
    /// `[jev] enabled = true` のときだけ作る。`board` は dispatcher の `classification_board()`。
    pub(crate) fn build(
        jev: &JevSectionConfig,
        secrets_dir: Option<std::path::PathBuf>,
        board: ClassificationBoard,
    ) -> Result<Option<Self>, JevError> {
        if !jev.enabled {
            return Ok(None);
        }
        let classifier = JevClassifier::new(jev.to_jev_config())?;
        let keys = SecretsDirKeyProvider::new(secrets_dir, jev.secret_id.clone());
        let credential = CredentialGate {
            secret_id: jev.secret_id.clone(),
            provider: "jev".into(),
            purpose: "Jev で依頼を分類する".into(),
        };
        Ok(Some(Self::with_stage(
            ClassificationStage::new(
                Arc::new(classifier),
                Arc::new(keys),
                credential,
                board.clone(),
            ),
            board,
        )))
    }

    pub(crate) fn with_stage(stage: ClassificationStage, board: ClassificationBoard) -> Self {
        Self { stage, board }
    }

    /// 分類した件数を返す。store の読み取り失敗は warn して続ける（dispatch は止めない）。
    /// mode は task ごとに解決する（task 欄 `routing_override` が config の `mode` より優先。ADR 2026-10-16-mlsys-measure-task-routing）:
    /// config が fixed でも decision_assisted に固定した task は分類し、config が decision_assisted でも fixed に
    /// 固定した task は分類しない。
    pub(crate) async fn run(&self, store: &dyn TaskStore, mode: RoutingMode) -> usize {
        let mut tasks = match store.ready_tasks(WINDOW) {
            Ok(t) => t,
            Err(error) => {
                tracing::warn!(%error, "jev: could not read ready tasks");
                return 0;
            }
        };
        match store.running_tasks_with_runnable_work_units(WINDOW) {
            Ok(t) => tasks.extend(t),
            Err(error) => tracing::warn!(%error, "jev: could not read runnable work units"),
        }
        let mut done = 0;
        for task in tasks {
            let routing_override = task_dispatch::task_routing_override(store, task.id);
            let mode =
                task_core::route_select::resolve_routing(mode, routing_override.as_ref()).mode;
            if mode == RoutingMode::Fixed {
                continue;
            }
            let units = store.work_units_for(task.id).unwrap_or_default();
            let subjects = if units.is_empty() {
                vec![(task.id.to_string(), task_input(&task))]
            } else {
                let ids = runnable_work_units(&units, 0, WINDOW);
                units
                    .iter()
                    .filter(|u| ids.contains(&u.id))
                    .map(|u| (u.id.clone(), unit_input(u, &units)))
                    .collect()
            };
            for (subject, input) in subjects {
                if self.needs_classification(&subject) {
                    self.stage.classify_for_mode(mode, &subject, &input).await;
                    done += 1;
                }
            }
        }
        done
    }

    fn needs_classification(&self, subject: &str) -> bool {
        match self.board.get(subject) {
            None => true,
            Some(item) => {
                item.credential.is_some()
                    && matches!(
                        &item.entry.status,
                        ClassifierStatus::FellBackToFixed { reason }
                            if reason == "key_missing" || reason == "auth"
                    )
            }
        }
    }
}

fn task_input(task: &Task) -> ClassifyInput {
    ClassifyInput {
        instruction: task.objective.clone(),
        acceptance: task.acceptance.iter().map(|c| c.text.clone()).collect(),
        required_artifacts: Vec::new(),
        dependency_summaries: Vec::new(),
        recent_failure: None,
    }
}

fn unit_input(unit: &WorkUnitRow, all: &[WorkUnitRow]) -> ClassifyInput {
    ClassifyInput {
        instruction: unit.spec.objective.clone(),
        acceptance: unit.spec.done_when.clone(),
        required_artifacts: unit.spec.outputs.clone(),
        dependency_summaries: all
            .iter()
            .filter(|u| {
                unit.depends_on.contains(&u.id) || unit.spec.depends_on.contains(&u.spec.key)
            })
            .map(|u| u.spec.title.clone())
            .collect(),
        recent_failure: None,
    }
}

#[cfg(test)]
mod tests {
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
        let task = task_ops::add::create_task(&store, spec, time::OffsetDateTime::now_utc())
            .expect("task");
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

    #[tokio::test]
    async fn only_the_runnable_work_unit_is_classified_by_its_unit_id() {
        let (tick, _fake, board, store, task) = fixture(vec![]);
        let a = unit(&task, "a", 0);
        let mut b = unit(&task, "b", 1);
        b.status = WorkUnitStatus::Pending;
        store
            .work_units_apply(task.id, vec![a.clone(), b.clone()], vec![], vec![])
            .expect("units");
        tick.run(&store, RoutingMode::DecisionAssisted).await;
        assert!(board.get(&a.id).is_some());
        assert!(board.get(&b.id).is_none(), "pending unit is not classified");
        assert!(board.get(&task.id.to_string()).is_none());
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
        let (tick, fake, board, store, task) =
            fixture(vec![Ok(ClassifyOutcome::FallbackToFixed {
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
}

#[cfg(test)]
mod e2e;
