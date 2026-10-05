use super::super::{DatasetV1, ExportOptions, evaluate, evaluate_with_estimator, export};
use super::*;
use crate::add::{NewTaskSpec, create_task};
use task_core::model_policy::RoutingRecord;
use task_core::model_router::estimator::sidecar::{EstimateValueV1, EstimatorDependencies};
use task_core::model_router::feedback::RoutingOutcome;
use task_core::model_router::policy::RoutingMode;
use task_core::model_router::shadow::{ShadowKind, ShadowReason, ShadowRecord, ShadowStatus};
use task_core::model_router::trace::{CandidateTrace, RoutingTraceV1};
use task_core::model_routing::LaneResolution;
use task_core::{Event, LaneCeiling, SqliteStore, Task, TaskStore, Tier};

fn task(store: &SqliteStore) -> Task {
    let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title":"secret title", "objective":"secret prompt",
        "acceptance":[{"type":"command", "cmd":"echo secret"}]
    }))
    .unwrap();
    create_task(store, spec, time::OffsetDateTime::now_utc()).unwrap()
}

/// Primary is model-a; the heuristic (highest eligible score) prefers model-b.
fn decided(task: &Task, run: &str) -> Event {
    let candidate = |model: &str, source: &str, score: f64| CandidateTrace {
        model_profile_id: model.into(),
        deployment_id: source.into(),
        score: Some(score),
        cash_usd: Some(0.1),
        effective_usd: Some(0.1),
        ..CandidateTrace::default()
    };
    Event::RoutingDecided {
        run_id: run.into(),
        record: Box::new(RoutingRecord {
            org_node: None,
            harness: None,
            decision: task_core::decide_for_task(task, &LaneCeiling::default()).unwrap(),
            resolution: LaneResolution {
                model_id: "model-a".into(),
                ..LaneResolution::default()
            },
            quota_reason: None,
            work_unit_id: None,
            escalation: None,
            optimizer: Some(RoutingTraceV1 {
                decision_id: format!("d-{run}"),
                parent_decision_id: None,
                task_id: Some(task.id.to_string()),
                work_unit_id: None,
                run_id: Some(run.into()),
                request_id: None,
                stage: "dispatch".into(),
                mode: RoutingMode::Shadow,
                policy_version: "p1".into(),
                catalog_version: "c1".into(),
                feature_version: "1".into(),
                estimator_version: "heuristic-1".into(),
                snapshot_id: "s1".into(),
                observed_at: None,
                requested_lane: Tier::Standard,
                selected_lane: Some(Tier::Standard),
                candidates: vec![
                    candidate("model-a", "source-a", 0.3),
                    candidate("model-b", "source-b", 0.9),
                    candidate("model-c", "source-c", 0.1),
                ],
                selected: Some("source-a".into()),
                fallback_order: vec![],
                reasons: vec![],
                source_id: Some("source-a".into()),
                model: Some("model-a".into()),
                account_id: None,
            }),
        }),
    }
}

fn outcome(run: &str, passed: bool) -> Event {
    Event::RoutingOutcomeRecorded {
        outcome: Box::new(RoutingOutcome {
            outcome_id: format!("o-{run}"),
            decision_id: format!("d-{run}"),
            run_id: Some(run.into()),
            request_id: None,
            evaluation_version: "1".into(),
            supersedes: None,
            acceptance_passed: Some(passed),
            review_passed: None,
            failed_criterion_ids: vec![],
            failure_class: None,
            cash_usd: Some(0.1),
            tokens: Some(10),
            wall_ms: Some(100),
            retries: Some(0),
            reward: None,
        }),
    }
}

fn estimator_shadow(
    run: &str,
    version: &str,
    status: ShadowStatus,
    reason: Option<ShadowReason>,
    detail: Option<&str>,
    model: Option<&str>,
    latency_ms: u64,
) -> Event {
    let record = ShadowRecord {
        shadow_id: format!("est-{run}-{version}"),
        primary_decision_id: format!("d-{run}"),
        kind: ShadowKind::Estimator,
        status,
        reason,
        detail: detail.map(str::to_owned),
        policy_version: version.into(),
        run_id: Some(run.into()),
        request_id: None,
        candidate_model: model.map(str::to_owned),
        candidate_source: None,
        input_tokens: None,
        output_tokens: None,
        output_sha256: None,
        cash_usd: None,
        effective_usd: None,
        latency_ms: Some(latency_ms),
        reservation_id: None,
    };
    record.validate().unwrap();
    Event::RoutingShadowRecorded {
        record: Box::new(record),
    }
}

fn descriptor() -> EstimatorDescriptor {
    EstimatorDescriptor {
        estimator_id: "route-test".into(),
        version: "1".into(),
        protocol_version: 1,
        needs_prompt: false,
        dependencies: EstimatorDependencies {
            needs_network: false,
            external_embeddings: false,
        },
    }
}

fn pair(calibration: Option<&str>) -> RouteLlmPairV1 {
    RouteLlmPairV1 {
        router: "bert".into(),
        strong_model: "model-a".into(),
        weak_model: "model-b".into(),
        calibration_version: calibration.map(str::to_owned),
    }
}

#[test]
fn routing_estimator_shadow_report_records_coverage_and_limits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("estimator.sqlite");
    {
        let store = SqliteStore::open(&path).unwrap();
        let tasks: Vec<Task> = (0..5).map(|_| task(&store)).collect();
        let runs = [
            (0, "r1"),
            (0, "r2"),
            (1, "r3"),
            (2, "r4"),
            (3, "r5"),
            (4, "r6"),
        ];
        for (i, run) in runs {
            store
                .append_event(tasks[i].id, &decided(&tasks[i], run))
                .unwrap();
            store
                .append_event(tasks[i].id, &outcome(run, true))
                .unwrap();
        }
        let shadows = [
            // Differs from the heuristic (model-b) and equals the primary.
            (
                0,
                estimator_shadow(
                    "r1",
                    "route-test@1",
                    ShadowStatus::Completed,
                    None,
                    None,
                    Some("model-a"),
                    40,
                ),
            ),
            (
                0,
                estimator_shadow(
                    "r2",
                    "route-test@1",
                    ShadowStatus::Failed,
                    Some(ShadowReason::Timeout),
                    None,
                    None,
                    250,
                ),
            ),
            (
                1,
                estimator_shadow(
                    "r3",
                    "route-test@1",
                    ShadowStatus::Dropped,
                    Some(ShadowReason::Privacy),
                    Some("prompt_required: send_prompt=false secret"),
                    None,
                    0,
                ),
            ),
            // Another estimator version is not the pinned one.
            (
                2,
                estimator_shadow(
                    "r4",
                    "route-test@0",
                    ShadowStatus::Completed,
                    None,
                    None,
                    Some("model-a"),
                    30,
                ),
            ),
            // A choice outside the pinned pair is not compared.
            (
                3,
                estimator_shadow(
                    "r5",
                    "route-test@1",
                    ShadowStatus::Completed,
                    None,
                    None,
                    Some("model-c"),
                    50,
                ),
            ),
        ];
        for (i, event) in shadows {
            store.append_event(tasks[i].id, &event).unwrap();
        }
    }
    let before = std::fs::read(&path).unwrap();
    let options = ExportOptions {
        policy_hash: "p".into(),
        catalog_hash: "c".into(),
        estimator_hash: "e".into(),
        from_utc: None,
        until_utc: None,
        seed: 7,
    };
    let dataset = export(&path, &options).unwrap();
    assert_eq!(dataset.rows.len(), 6);
    let jsonl = dataset.jsonl().unwrap();
    assert!(jsonl.contains("\"detail_code\":\"prompt_required\""));
    assert!(!jsonl.contains("secret"));
    assert!(!jsonl.contains("send_prompt"));

    let input = EstimatorComparisonInputV1 {
        descriptor: descriptor(),
        pair: Some(pair(None)),
    };
    let report = evaluate_with_estimator(&dataset, None, None, Some(&input)).unwrap();
    let again = evaluate_with_estimator(&dataset, None, None, Some(&input)).unwrap();
    assert_eq!(report.json().unwrap(), again.json().unwrap());
    let est = report.estimator_comparison.as_ref().unwrap();
    assert_eq!(est.descriptor.as_ref(), Some(&descriptor()));
    assert_eq!(est.pair.as_ref(), Some(&pair(None)));
    assert!(!est.calibrated);
    assert_eq!(
        est.observed_estimator_versions,
        ["route-test@0", "route-test@1"]
    );
    assert_eq!(est.target_decisions, 6);
    assert_eq!(est.evaluated, 2);
    assert_eq!(est.coverage, 0.333333);
    assert_eq!(
        (
            est.completed,
            est.failed,
            est.timeout,
            est.dropped,
            est.prompt_required
        ),
        (2, 1, 1, 1, 1)
    );
    assert_eq!(est.overhead_ms_p50, Some(40));
    assert_eq!(est.overhead_ms_p95, Some(250));
    assert_eq!(est.overhead_ms_mean, Some(85.0));
    assert_eq!(
        (
            est.same_as_heuristic,
            est.differs_from_heuristic,
            est.same_as_primary
        ),
        (0, 1, 1)
    );
    // Uncalibrated raw pair score: never counted as quality success.
    assert_eq!((est.quality_observed, est.quality_unknown), (0, 1));
    assert_eq!(est.acceptance_success_rate, None);
    assert_eq!(
        est.incomparable_reasons,
        BTreeMap::from([
            ("estimator_version_mismatch".to_owned(), 1),
            ("not_evaluated".to_owned(), 1),
            ("outside_pinned_pair".to_owned(), 1),
            ("prompt_required".to_owned(), 1),
            ("timeout".to_owned(), 1),
            ("uncalibrated_raw_score".to_owned(), 1),
        ])
    );
    // Estimator shadows do not leak into the decision/execution shadow metrics.
    assert_eq!(report.policies["shadow_recorded"].timeout_rate, 0.0);
    assert_eq!(report.policies["shadow_recorded"].drop_rate, 0.0);

    // Once the pair is calibrated, the observed primary outcome counts.
    let calibrated = EstimatorComparisonInputV1 {
        descriptor: descriptor(),
        pair: Some(pair(Some("cal-1"))),
    };
    let report = evaluate_with_estimator(&dataset, None, None, Some(&calibrated)).unwrap();
    let est = report.estimator_comparison.unwrap();
    assert_eq!((est.quality_observed, est.quality_unknown), (1, 0));
    assert_eq!(est.acceptance_success_rate, Some(1.0));

    // Without a pinned estimator the comparison is still reported from the dataset alone.
    let plain = evaluate(&dataset, None).unwrap();
    assert!(plain.estimator_comparison.is_some());
    assert_eq!(before, std::fs::read(&path).unwrap());
}

#[test]
fn routing_routellm_pair_adapter_preserves_unknown_models() {
    let estimate = |model: &str, index: Option<f64>, reasons: &[&str]| EstimateValueV1 {
        model_profile_id: model.into(),
        index,
        confidence: None,
        reasons: reasons.iter().map(|r| (*r).to_owned()).collect(),
    };
    let response = EstimateResponseV1 {
        request_id: "req-1".into(),
        estimator_id: "route-test".into(),
        version: "1".into(),
        estimates: vec![
            estimate("model-a", None, &["routellm:bert", "raw_pair_score=0.73"]),
            estimate("model-b", None, &["routellm:bert"]),
            // A sidecar index for a model outside the pair must not be transferred.
            estimate("model-c", Some(0.9), &["classifier_score"]),
        ],
        dependencies: EstimatorDependencies {
            needs_network: false,
            external_embeddings: false,
        },
    };
    let candidates: Vec<String> = ["model-a", "model-b", "model-c", "model-d"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let out = routellm_pair_estimates(&descriptor(), &pair(None), &response, &candidates).unwrap();
    let by: BTreeMap<_, _> = out
        .iter()
        .map(|e| (e.model_profile_id.as_str(), e))
        .collect();
    assert_eq!(by.len(), 4);
    for model in ["model-a", "model-b"] {
        assert_eq!(by[model].status, "uncalibrated");
        assert_eq!(by[model].index, None);
        assert_eq!(by[model].raw_pair_score, Some(0.73));
    }
    for model in ["model-c", "model-d"] {
        assert_eq!(by[model].status, "outside_pinned_pair");
        assert_eq!(by[model].index, None);
        assert_eq!(by[model].raw_pair_score, None);
    }

    let cal = routellm_pair_estimates(&descriptor(), &pair(Some("cal-1")), &response, &candidates)
        .unwrap();
    let by: BTreeMap<_, _> = cal
        .iter()
        .map(|e| (e.model_profile_id.as_str(), e))
        .collect();
    assert_eq!(by["model-a"].index, Some(0.73));
    assert_eq!(by["model-b"].index, Some(0.27));
    assert_eq!(by["model-c"].index, None);

    // No raw score: the pair stays unknown rather than guessed.
    let mut missing = response.clone();
    missing.estimates[0].reasons = vec!["routellm:bert".into()];
    let out = routellm_pair_estimates(&descriptor(), &pair(Some("cal-1")), &missing, &candidates)
        .unwrap();
    assert!(out.iter().all(|e| e.index.is_none()));
    assert!(
        out.iter()
            .filter(|e| e.model_profile_id == "model-a")
            .all(|e| e.status == "pair_score_missing")
    );

    // Response from another estimator version, or an invalid pair, is rejected.
    let mut other = response.clone();
    other.version = "2".into();
    assert!(routellm_pair_estimates(&descriptor(), &pair(None), &other, &candidates).is_err());
    let mut same = pair(None);
    same.weak_model = "model-a".into();
    assert!(routellm_pair_estimates(&descriptor(), &same, &response, &candidates).is_err());

    // The pinned descriptor and pair are saved in the report even with no estimator shadow.
    let dataset = DatasetV1 {
        manifest: super::super::ManifestV1 {
            schema: super::super::DATASET_SCHEMA.into(),
            policy_hash: "p".into(),
            catalog_hash: "c".into(),
            estimator_hash: "e".into(),
            from_utc: None,
            until_utc: None,
            extraction: String::new(),
            masking: String::new(),
            split: String::new(),
            seed: 1,
            rows: 0,
            missing_rate: BTreeMap::new(),
        },
        rows: vec![],
    };
    let input = EstimatorComparisonInputV1 {
        descriptor: descriptor(),
        pair: Some(pair(None)),
    };
    let report = evaluate_with_estimator(&dataset, None, None, Some(&input)).unwrap();
    let json = report.json().unwrap();
    assert!(json.contains("\"estimator_id\": \"route-test\""));
    assert!(json.contains("\"strong_model\": \"model-a\""));
    assert!(json.contains("\"weak_model\": \"model-b\""));
    let est = report.estimator_comparison.unwrap();
    assert_eq!(
        (est.target_decisions, est.evaluated, est.coverage),
        (0, 0, 0.0)
    );
    // Without any estimator input or shadow, the v1 report is unchanged.
    assert!(
        evaluate(&dataset, None)
            .unwrap()
            .estimator_comparison
            .is_none()
    );
}
