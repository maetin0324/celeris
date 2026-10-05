//! Phase 4（ADR 2026-10-04-multi-objective-model-routing §7.2）: `celerisctl routing export` /
//! `routing evaluate` を実バイナリで回す。一時 DB に fixture event を入れ、2 回の実行で report が
//! バイト一致し、DB の内容と mtime が変わらず、`--baseline` が別欄で取り込まれることを確かめる。

use std::path::Path;
use std::process::{Command, Output};

use task_core::model_policy::RoutingRecord;
use task_core::model_router::estimator::sidecar::EstimatorDescriptor;
use task_core::model_router::feedback::RoutingOutcome;
use task_core::model_router::policy::RoutingMode;
use task_core::model_router::shadow::{ShadowKind, ShadowReason, ShadowRecord, ShadowStatus};
use task_core::model_router::trace::{CandidateTrace, RoutingTraceV1};
use task_core::model_routing::LaneResolution;
use task_core::{Event, LaneCeiling, SqliteStore, Task, TaskStore, Tier};
use task_ops::add::{NewTaskSpec, create_task};
use task_ops::routing_replay::RouteLlmPairV1;

fn ctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args(args)
        .env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        .env_remove("CELERIS_FOLLOWUPS_FILE")
        .env_remove("CELERIS_RUN_DB")
        .output()
        .unwrap_or_else(|e| panic!("spawn celerisctl: {e}"))
}

fn ok(out: &Output) {
    assert!(
        out.status.success(),
        "celerisctl failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn task(store: &SqliteStore) -> Task {
    let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "secret title", "objective": "secret prompt response credential",
        "acceptance": [{"type": "command", "cmd": "echo secret"}]
    }))
    .unwrap();
    create_task(store, spec, time::OffsetDateTime::now_utc()).unwrap()
}

fn decided(task: &Task, run: &str) -> Event {
    let decision = task_core::decide_for_task(task, &LaneCeiling::default()).unwrap();
    let candidate = |model: &str, source: &str, score: f64, cash: f64| CandidateTrace {
        model_profile_id: model.into(),
        deployment_id: source.into(),
        score: Some(score),
        cash_usd: Some(cash),
        effective_usd: Some(cash * 2.0),
        ..CandidateTrace::default()
    };
    Event::RoutingDecided {
        run_id: run.into(),
        record: Box::new(RoutingRecord {
            org_node: None,
            harness: None,
            decision,
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
                estimator_version: "e1".into(),
                snapshot_id: "s1".into(),
                observed_at: None,
                requested_lane: Tier::Standard,
                selected_lane: Some(Tier::Standard),
                candidates: vec![
                    candidate("model-a", "source-a", 0.3, 0.1),
                    candidate("model-b", "source-b", 0.9, 0.05),
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
            review_passed: Some(passed),
            failed_criterion_ids: if passed { vec![] } else { vec!["c0".into()] },
            failure_class: None,
            cash_usd: Some(0.1),
            tokens: Some(10),
            wall_ms: Some(500),
            retries: Some(0),
            reward: None,
        }),
    }
}

/// 1 file の控え（path と、存在すれば中身と mtime）。
type FileState = (String, Option<(Vec<u8>, std::time::SystemTime)>);

/// DB と WAL/SHM の中身と mtime の控え（存在しない側も記録する）。
fn snapshot(db: &Path) -> Vec<FileState> {
    ["", "-wal", "-shm", "-journal"]
        .iter()
        .map(|suffix| {
            let path = format!("{}{suffix}", db.display());
            let state = std::fs::metadata(&path)
                .ok()
                .map(|m| (std::fs::read(&path).unwrap(), m.modified().unwrap()));
            (path, state)
        })
        .collect()
}

#[test]
fn routing_cli_export_evaluate_is_read_only_and_reproducible() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("fixture.sqlite3");
    {
        let store = SqliteStore::open(&db).unwrap();
        let first = task(&store);
        let second = task(&store);
        store
            .append_event(first.id, &decided(&first, "r1"))
            .unwrap();
        store.append_event(first.id, &outcome("r1", true)).unwrap();
        store
            .append_event(second.id, &decided(&second, "r2"))
            .unwrap();
        store
            .append_event(second.id, &outcome("r2", false))
            .unwrap();
    }
    let baseline = temp.path().join("baseline.json");
    std::fs::write(
        &baseline,
        r#"{"name":"routerbench-fixture","models":{"model-b":{"quality":0.8,"cost_usd":0.2}}}"#,
    )
    .unwrap();
    let before = snapshot(&db);

    // --db を渡さなければ（CELERIS_DB 等の既定も解決せず）拒否する。
    let no_db = ctl(&[
        "routing",
        "export",
        "--out",
        temp.path().join("x").to_str().unwrap(),
    ]);
    assert!(!no_db.status.success());
    assert!(String::from_utf8_lossy(&no_db.stderr).contains("--db"));
    // 無い DB は作らない。
    let missing = temp.path().join("missing.sqlite3");
    let out = ctl(&[
        "--db",
        missing.to_str().unwrap(),
        "routing",
        "export",
        "--out",
        temp.path().join("x").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(!missing.exists());

    let mut reports = Vec::new();
    let mut markdowns = Vec::new();
    for i in 0..2 {
        let ds = temp.path().join(format!("ds{i}"));
        let report = temp.path().join(format!("report{i}.json"));
        let md = temp.path().join(format!("report{i}.md"));
        ok(&ctl(&[
            "routing",
            "export",
            "--db",
            db.to_str().unwrap(),
            "--out",
            ds.to_str().unwrap(),
            "--seed",
            "7",
            "--since",
            "2000-01-01T00:00:00Z",
            "--until",
            "2100-01-01T00:00:00Z",
        ]));
        let jsonl = std::fs::read_to_string(ds.join("dataset.jsonl")).unwrap();
        assert_eq!(jsonl.lines().count(), 2);
        assert!(!jsonl.contains("secret"));
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(ds.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["schema"], "celeris.routing.dataset.v1");
        assert_eq!(manifest["seed"], 7);
        ok(&ctl(&[
            "routing",
            "evaluate",
            "--dataset",
            ds.to_str().unwrap(),
            "--policy",
            "legacy",
            "--policy",
            "heuristic",
            "--baseline",
            baseline.to_str().unwrap(),
            "--out",
            report.to_str().unwrap(),
            "--markdown",
            md.to_str().unwrap(),
        ]));
        reports.push(std::fs::read(&report).unwrap());
        markdowns.push(std::fs::read(&md).unwrap());
    }
    assert_eq!(reports[0], reports[1], "report must be byte-identical");
    assert_eq!(markdowns[0], markdowns[1]);
    let after = snapshot(&db);
    for ((path, b), (_, a)) in before.iter().zip(&after) {
        assert!(
            b == a,
            "{path} changed: before={:?} after={:?}",
            b.as_ref().map(|(bytes, t)| (bytes.len(), t)),
            a.as_ref().map(|(bytes, t)| (bytes.len(), t))
        );
    }

    let report: serde_json::Value = serde_json::from_slice(&reports[0]).unwrap();
    assert_eq!(report["schema"], "celeris.routing.report.v1");
    let policies = report["policies"].as_object().unwrap();
    assert_eq!(
        policies.keys().collect::<Vec<_>>(),
        vec!["heuristic", "legacy"]
    );
    assert_eq!(report["policies"]["legacy"]["observed_outcomes"], 2);
    assert_eq!(report["policies"]["legacy"]["acceptance_success_rate"], 0.5);
    assert_eq!(report["policies"]["legacy"]["failed_criteria"]["c0"], 1);
    // 未選択モデルの品質は捏造しない。
    assert_eq!(report["policies"]["heuristic"]["observed_outcomes"], 0);
    assert_eq!(report["policies"]["heuristic"]["unknown_rate"], 1.0);
    assert_eq!(
        report["external_benchmark_baseline"]["name"],
        "routerbench-fixture"
    );
    assert_eq!(
        report["external_benchmark_baseline"]["models"]["model-b"]["quality"],
        0.8
    );
    let md = String::from_utf8(markdowns[0].clone()).unwrap();
    assert!(md.contains("routerbench-fixture"));
    assert!(md.contains("| legacy |"));
}

/// estimator shadow の記録イベント（kind=estimator。primary の decision `d-<run>` へ紐づく）。
fn estimator_shadow_event(
    run: &str,
    version: &str,
    status: ShadowStatus,
    reason: Option<ShadowReason>,
    model: Option<&str>,
    latency_ms: u64,
) -> Event {
    let record = ShadowRecord {
        shadow_id: format!("est-{run}"),
        primary_decision_id: format!("d-{run}"),
        kind: ShadowKind::Estimator,
        status,
        reason,
        detail: None,
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

#[test]
fn routing_cli_estimator_report_is_read_only_and_reproducible() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("estimator.sqlite3");
    {
        let store = SqliteStore::open(&db).unwrap();
        let first = task(&store);
        let second = task(&store);
        // primary は model-a。estimator shadow は completed（model-a、primary と同じ）、
        // timeout、prompt_required（dropped）の 3 種を記録する。
        store
            .append_event(first.id, &decided(&first, "r1"))
            .unwrap();
        store.append_event(first.id, &outcome("r1", true)).unwrap();
        store
            .append_event(
                first.id,
                &estimator_shadow_event(
                    "r1",
                    "route-test@1",
                    ShadowStatus::Completed,
                    None,
                    Some("model-a"),
                    40,
                ),
            )
            .unwrap();
        store
            .append_event(second.id, &decided(&second, "r2"))
            .unwrap();
        store
            .append_event(second.id, &outcome("r2", false))
            .unwrap();
        store
            .append_event(
                second.id,
                &estimator_shadow_event(
                    "r2",
                    "route-test@1",
                    ShadowStatus::Failed,
                    Some(ShadowReason::Timeout),
                    None,
                    250,
                ),
            )
            .unwrap();
    }
    let before = snapshot(&db);

    // pin した descriptor と pair（未校正の RouteLLM pair）。
    let descriptor = EstimatorDescriptor {
        estimator_id: "route-test".into(),
        version: "1".into(),
        protocol_version: 1,
        needs_prompt: false,
        dependencies: task_core::model_router::estimator::sidecar::EstimatorDependencies {
            needs_network: false,
            external_embeddings: false,
        },
    };
    let pin_descriptor = temp.path().join("estimator.json");
    std::fs::write(&pin_descriptor, serde_json::to_string(&descriptor).unwrap()).unwrap();
    let pair = RouteLlmPairV1 {
        router: "bert".into(),
        strong_model: "model-a".into(),
        weak_model: "model-b".into(),
        calibration_version: None,
    };
    let pin_pair = temp.path().join("pair.json");
    std::fs::write(&pin_pair, serde_json::to_string(&pair).unwrap()).unwrap();

    // --estimator を --policy estimator なしで使うと拒否する。
    let no_policy = ctl(&[
        "routing",
        "evaluate",
        "--dataset",
        temp.path().join("nope").to_str().unwrap(),
        "--estimator",
        pin_descriptor.to_str().unwrap(),
        "--out",
        temp.path().join("x").to_str().unwrap(),
    ]);
    assert!(!no_policy.status.success());
    assert!(String::from_utf8_lossy(&no_policy.stderr).contains("require --policy estimator"));
    // dataset の無い pin は DB を開かない（evaluate は DB を使わない）。
    let no_dataset = ctl(&[
        "routing",
        "evaluate",
        "--dataset",
        temp.path().join("nope").to_str().unwrap(),
        "--policy",
        "estimator",
        "--estimator",
        pin_descriptor.to_str().unwrap(),
        "--out",
        temp.path().join("x").to_str().unwrap(),
    ]);
    assert!(!no_dataset.status.success());

    let mut reports = Vec::new();
    let mut markdowns = Vec::new();
    for i in 0..2 {
        let ds = temp.path().join(format!("ds{i}"));
        let report = temp.path().join(format!("report{i}.json"));
        let md = temp.path().join(format!("report{i}.md"));
        ok(&ctl(&[
            "routing",
            "export",
            "--db",
            db.to_str().unwrap(),
            "--out",
            ds.to_str().unwrap(),
            "--seed",
            "7",
        ]));
        ok(&ctl(&[
            "routing",
            "evaluate",
            "--dataset",
            ds.to_str().unwrap(),
            "--policy",
            "estimator",
            "--estimator",
            pin_descriptor.to_str().unwrap(),
            "--pair",
            pin_pair.to_str().unwrap(),
            "--out",
            report.to_str().unwrap(),
            "--markdown",
            md.to_str().unwrap(),
        ]));
        reports.push(std::fs::read(&report).unwrap());
        markdowns.push(std::fs::read(&md).unwrap());
    }
    assert_eq!(
        reports[0], reports[1],
        "estimator report must be byte-identical"
    );
    assert_eq!(markdowns[0], markdowns[1]);
    let after = snapshot(&db);
    for ((path, b), (_, a)) in before.iter().zip(&after) {
        assert!(
            b == a,
            "{path} changed: before={:?} after={:?}",
            b.as_ref().map(|(bytes, t)| (bytes.len(), t)),
            a.as_ref().map(|(bytes, t)| (bytes.len(), t))
        );
    }

    let report: serde_json::Value = serde_json::from_slice(&reports[0]).unwrap();
    // --policy estimator のみ: policies 欄は空、estimator_comparison 欄が入る。
    assert!(report["policies"].as_object().unwrap().is_empty());
    let est = &report["estimator_comparison"];
    assert_eq!(est["descriptor"]["estimator_id"], "route-test");
    assert_eq!(est["pair"]["strong_model"], "model-a");
    assert_eq!(est["pair"]["weak_model"], "model-b");
    assert_eq!(est["calibrated"], false);
    assert_eq!(
        est["observed_estimator_versions"],
        serde_json::json!(["route-test@1"])
    );
    assert_eq!(est["target_decisions"], 2);
    assert_eq!(est["evaluated"], 1);
    assert_eq!(est["completed"], 1);
    assert_eq!(est["timeout"], 1);
    assert_eq!(est["overhead_ms_p50"], 40);
    assert_eq!(est["same_as_primary"], 1);
    // 未校正の pair: primary と同じ選択でも品質成功に数えない。
    assert_eq!(est["quality_observed"], 0);
    assert_eq!(est["quality_unknown"], 1);
    assert!(est["incomparable_reasons"]["timeout"].is_number());
    assert!(est["incomparable_reasons"]["uncalibrated_raw_score"].is_number());
    let md = String::from_utf8(markdowns[0].clone()).unwrap();
    assert!(md.contains("## Estimator shadow comparison"));
    assert!(md.contains("route-test @ 1"));
    assert!(md.contains("strong=model-a weak=model-b"));
    assert!(md.contains("coverage: 0.5"));
    assert!(md.contains("| timeout | 1 |"));
}

#[test]
fn routing_help_lists_export_and_evaluate() {
    let out = ctl(&["routing", "--help"]);
    ok(&out);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("export"));
    assert!(text.contains("evaluate"));
}
