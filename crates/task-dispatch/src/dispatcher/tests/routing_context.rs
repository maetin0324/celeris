//! 多目的 routing ADR（2026-10-04）§10 Phase 3: run 開始時の RoutingContext（`dispatcher/routing_context.rs`）。

use super::*;

use crate::dispatcher::routing_context::{
    RoutingContextInput, RoutingRunRole, build_routing_context, features_record,
};
use task_core::model_router::context::RoutingPhase;
use task_core::model_router::context_registry::{
    ContextRefError, InMemoryRoutingContextRegistry, RoutingContextRegistry,
};
use task_core::model_router::feedback::{FeatureStage, ROUTING_CONTEXT_VERSION};

fn work_unit(task: &Task) -> task_core::WorkUnitRow {
    let spec: task_core::WorkUnitSpec = serde_json::from_value(serde_json::json!({
        "key": "impl",
        "kind": "implement",
        "title": "実装",
        "objective": "SECRET-WU-OBJECTIVE を実装する",
        "checks": [{"cmd": "cargo test -p secret-crate", "expect_exit": 0}],
    }))
    .unwrap();
    let mut wu = task_core::WorkUnitRow::new(
        "wu-1".into(),
        task.id.to_string(),
        "plan-1".into(),
        1,
        spec,
        task_core::WorkUnitStatus::Running,
        "2026-10-05T00:00:00Z".into(),
    );
    wu.runs = 2;
    wu
}

#[test]
fn routing_context_extracts_acceptance_tools_environment_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut task = new_task(dir.path(), Check::Human, 2);
    task.objective = "SECRET-OBJECTIVE-BODY token=sk-live-123".into();
    task.acceptance = vec![
        Criterion {
            text: "cargo test passes".into(),
            check: Check::Command {
                cmd: "SECRET_TOKEN=abc cargo test".into(),
                expect_exit: 0,
            },
        },
        Criterion {
            text: "reviewer approves".into(),
            check: Check::Reviewer,
        },
    ];
    task.assignee = Some("software-engineering".into());
    task.priority = 7;
    task.attempts = 3;
    task.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: "/work/x".into(),
        mode: None,
    };
    let wu = work_unit(&task);
    let profile = task_core::EffectiveProfile {
        tools: vec!["gh".into(), "cluster:pegasus".into(), "docker".into()],
        deny_tools: vec!["docker".into()],
        ..Default::default()
    };
    // review 不合格（reviewer 条件）1 回、検査だけの不合格 1 回、供給側 1 回、WU の検査不合格 1 回。
    let fail = |reason: &str| Event::Transitioned {
        from: Status::Reviewing,
        to: Status::Ready,
        reason: reason.into(),
    };
    let verdict = |idx: usize| Event::ReviewVerdict {
        run_id: "r0".into(),
        criterion_idx: idx,
        pass: false,
        reason: "no".into(),
    };
    let events = vec![
        verdict(1),
        fail("review_fail"),
        verdict(0),
        fail("review_fail"),
        fail("requeue"),
        Event::WorkUnitChecksFailed {
            run_id: "r1".into(),
            work_unit_id: "wu-1".into(),
            key: "impl".into(),
            cwd: "/w".into(),
            failed: vec![],
        },
        Event::WorkUnitChecksFailed {
            run_id: "r1".into(),
            work_unit_id: "other-wu".into(),
            key: "other".into(),
            cwd: "/w".into(),
            failed: vec![],
        },
    ];
    let input = RoutingContextInput {
        task: &task,
        work_unit: Some(&wu),
        run_id: "run-1",
        role: RoutingRunRole::Worker,
        harness: "claude-code",
        profile: Some(&profile),
        cluster: Some("pegasus"),
        events: &events,
    };
    let ctx = build_routing_context(&input);

    assert_eq!(ctx.task_id.as_deref(), Some(task.id.to_string().as_str()));
    assert_eq!(ctx.work_unit_id.as_deref(), Some("wu-1"));
    assert_eq!(ctx.run_id.as_deref(), Some("run-1"));
    assert_eq!(ctx.org_node.as_deref(), Some("software-engineering"));
    assert_eq!(ctx.role.as_deref(), Some("worker"));
    assert_eq!(ctx.harness.as_deref(), Some("claude-code"));
    assert_eq!(ctx.task_kind.as_deref(), Some("execute"));
    assert_eq!(ctx.phase, Some(RoutingPhase::Implementation));
    assert_eq!(
        ctx.acceptance_criteria,
        vec!["acceptance:0", "acceptance:1", "check:0"]
    );
    assert!(ctx.required_tools);
    assert_eq!(ctx.required_tool_ids, vec!["gh", "cluster:pegasus"]);
    assert_eq!(ctx.environment.locality.as_deref(), Some("cluster"));
    assert_eq!(ctx.environment.host.as_deref(), Some("pegasus"));
    assert_eq!(ctx.environment.external_network, None);
    assert!(ctx.input_tokens.is_some_and(|t| t > 0));
    assert_eq!(ctx.attempts, Some(2));
    assert_eq!(ctx.review_failures, Some(1));
    assert_eq!(ctx.check_failures, Some(2));
    assert_eq!(ctx.priority, Some(7));

    // 欄ごとの出自。
    let expect = [
        ("task_id", "task.id"),
        ("work_unit_id", "work_unit.id"),
        ("run_id", "dispatch.run_id"),
        ("org_node", "task.assignee"),
        ("role", "dispatch.run_role"),
        ("harness", "dispatch.adapter"),
        ("task_kind", "task.kind"),
        ("phase", "dispatch.run_role"),
        (
            "acceptance_criteria",
            "task.acceptance+work_unit.spec.checks",
        ),
        ("required_tools", "dispatch.adapter"),
        ("required_tool_ids", "org.profile.tools"),
        ("environment.locality", "task.workspace"),
        ("environment.host", "task.workspace.cluster"),
        (
            "input_tokens",
            "estimator:chars/4(work_unit.spec.objective+acceptance)",
        ),
        ("attempts", "work_unit.runs"),
        ("review_failures", "events:attempt_history(initial)"),
        (
            "check_failures",
            "events:attempt_history(initial)+work_unit_checks_failed",
        ),
        ("priority", "task.priority"),
    ];
    for (field, source) in expect {
        assert_eq!(
            ctx.field_provenance.get(field).map(String::as_str),
            Some(source),
            "{field}"
        );
    }
    assert_eq!(
        ctx.missing_fields,
        vec!["environment.external_network", "output_reserve"]
    );

    // feature event に本文・command・credential が入らない。
    let record = features_record("run-1", &ctx);
    assert_eq!(record.decision_id, "run-1");
    assert_eq!(record.context_version, ROUTING_CONTEXT_VERSION);
    assert_eq!(record.stage, Some(FeatureStage::Dispatch));
    let wire = serde_json::to_string(&Event::RoutingFeaturesRecorded {
        record: Box::new(record),
    })
    .unwrap();
    for secret in [
        "SECRET",
        "sk-live",
        "cargo test",
        "token=",
        "reviewer approves",
        "/work/x",
    ] {
        assert!(!wire.contains(secret), "{secret} leaked: {wire}");
    }

    // planner / reviewer の phase、WU なし・担当なし・profile なしは欠測として明示する。
    task.assignee = None;
    task.acceptance.clear();
    task.workspace = WorkspaceSpec::Local {
        path: dir.path().into(),
        mode: None,
    };
    for (role, phase) in [
        (RoutingRunRole::Planner, RoutingPhase::Planning),
        (RoutingRunRole::Reviewer, RoutingPhase::Review),
    ] {
        let ctx = build_routing_context(&RoutingContextInput {
            task: &task,
            work_unit: None,
            run_id: "run-2",
            role,
            harness: "paperqa",
            profile: None,
            cluster: None,
            events: &[],
        });
        assert_eq!(ctx.phase, Some(phase));
        assert!(!ctx.required_tools);
        assert_eq!(ctx.environment.locality.as_deref(), Some("local"));
        assert_eq!(ctx.attempts, Some(3));
        assert_eq!(
            ctx.field_provenance.get("attempts").map(String::as_str),
            Some("task.attempts")
        );
        for field in [
            "work_unit_id",
            "org_node",
            "acceptance_criteria",
            "required_tool_ids",
            "environment.external_network",
            "output_reserve",
        ] {
            assert!(ctx.missing_fields.iter().any(|m| m == field), "{field}");
        }
    }
}

/// register / release を数える registry（中身は in-memory の実装に任せる）。
#[derive(Default)]
struct RecordingRegistry {
    inner: InMemoryRoutingContextRegistry,
    registered: StdMutex<Vec<(String, String)>>,
    released: StdMutex<Vec<String>>,
}

impl RoutingContextRegistry for RecordingRegistry {
    fn register(
        &self,
        run_id: &str,
        ctx: task_core::model_router::context::RoutingContext,
        ttl: Duration,
        now: Instant,
    ) -> String {
        let r = self.inner.register(run_id, ctx, ttl, now);
        self.registered
            .lock()
            .unwrap()
            .push((run_id.to_string(), r.clone()));
        r
    }
    fn resolve(
        &self,
        reference: &str,
        now: Instant,
    ) -> Result<task_core::model_router::context::RoutingContext, ContextRefError> {
        self.inner.resolve(reference, now)
    }
    fn release(&self, run_id: &str) {
        self.released.lock().unwrap().push(run_id.to_string());
        self.inner.release(run_id);
    }
    fn prune(&self, now: Instant) -> usize {
        self.inner.prune(now)
    }
}

/// dispatcher は run を起こすとき context を run に結んで登録し、`RoutingDecided` と同じ decision_id で
/// `routing_features_recorded` を 1 回だけ追記し、run の終了で ref を release する。
#[tokio::test]
async fn routing_context_is_registered_recorded_once_and_released_at_run_end() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        ws.path(),
        Check::Command {
            cmd: "cargo test".into(),
            expect_exit: 0,
        },
        2,
    );
    task.objective = "crates/task-core/src/model.rs の typo を直す".into();
    task.genre = Some("coding".into());
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Hint,
        ..Default::default()
    });
    store.insert(&task).unwrap();
    let registry = Arc::new(RecordingRegistry::default());
    let mut d = dispatcher(store.clone(), three_lane_adapter(), 1);
    d.set_routing_context_registry(registry.clone());
    d.tick().unwrap();

    let events = store.events_for(task.id).unwrap();
    let record = routing_record(&events).expect("routing_decided");
    let decision_id = record
        .optimizer
        .as_ref()
        .map(|t| t.decision_id.clone())
        .expect("optimizer trace");
    let features: Vec<_> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::RoutingFeaturesRecorded { record } => Some((**record).clone()),
            _ => None,
        })
        .collect();
    assert_eq!(features.len(), 1, "{events:?}");
    assert_eq!(features[0].decision_id, decision_id);
    assert_eq!(features[0].stage, Some(FeatureStage::Dispatch));
    assert_eq!(features[0].context_version, ROUTING_CONTEXT_VERSION);
    assert_eq!(
        features[0].features["phase"],
        serde_json::json!("implementation")
    );
    assert!(
        !serde_json::to_string(&features[0])
            .unwrap()
            .contains("typo")
    );

    let registered = registry.registered.lock().unwrap().clone();
    assert_eq!(registered.len(), 1);
    let run_id = features[0].run_id.clone().expect("run id");
    assert_eq!(registered[0].0, run_id);
    let resolved = registry
        .resolve(&registered[0].1, Instant::now())
        .expect("ref resolves while the run is in flight (or already released)");
    assert_eq!(resolved.run_id.as_deref(), Some(run_id.as_str()));

    // run の終わり（完了の取り込み）で release される。
    for _ in 0..400 {
        if registry.released.lock().unwrap().contains(&run_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        d.tick().unwrap();
    }
    assert!(registry.released.lock().unwrap().contains(&run_id));
    assert_eq!(
        registry.resolve(&registered[0].1, Instant::now()),
        Err(ContextRefError::Invalid)
    );
    // 同じ run の features は 1 件だけ（後の tick で同じ decision_id を足さない）。
    let again = store
        .events_for(task.id)
        .unwrap()
        .iter()
        .filter(|(_, e)| matches!(e, Event::RoutingFeaturesRecorded { record } if record.decision_id == decision_id))
        .count();
    assert_eq!(again, 1);
}
