//! ADR-0069 D5: `GET /tasks/{id}/routing`（run ごとの routing の監査と、タスクの routing の出自）。

mod common;

use common::*;
use serde_json::json;
use task_core::model_router::shadow::{
    ShadowDailyCaps, ShadowReservation, ShadowReservationRequest, ShadowSettlement,
};
use task_core::{Event, Status, TaskId, TaskKind, TaskRouting, TaskStore, TierSource};

fn admin() -> [(&'static str, &'static str); 1] {
    [("authorization", "Bearer s3cret-token-value")]
}

fn env_with_token() -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    })
}

fn routing_decided(run_id: &str, escalation: Option<&str>) -> Event {
    serde_json::from_value(json!({
        "type": "routing_decided",
        "run_id": run_id,
        "record": {
            "org_node": "software-engineering",
            "harness": "coding",
            "decision": {
                "lane": "standard",
                "proposed": "standard",
                "source": "default",
                "rule_id": "standard/default",
                "policy_version": "v1",
                "features": {
                    "judgment": "low", "ambiguity": "low", "verifiability": "high",
                    "reversibility": "high", "consequence": "low", "context_size": "medium",
                    "tool_intensity": "medium", "expected_length": "medium", "cross_cutting": "low"
                },
                "reasons": ["no rule above matched"],
                "escalation": escalation
            },
            "resolution": {
                "lane": "standard",
                "adapter": "claude-code",
                "provider": "cc-1",
                "model_id": "model-std",
                "reasoning_effort": "medium"
            }
        }
    }))
    .expect("routing_decided event")
}

/// shadow の失敗・timeout は primary の outcome、attempts、review に影響しない。
#[tokio::test]
async fn routing_shadow_audit_keeps_primary_outcome_separate() {
    let env = env_with_token();
    let task = new_task(TaskKind::Execute, Status::Done);
    env.seed(&task);
    let id = task.id;
    let mut primary = serde_json::to_value(routing_decided("run-1", None)).unwrap();
    primary["record"]["optimizer"] = trace_json("decision-1", "dispatch", Some("run-1"), None);
    for event in [
        serde_json::from_value(primary).unwrap(),
        Event::WorkerFinished {
            run_id: "run-1".into(),
            outcome: "done: ok".into(),
            usage: None,
            role: None,
            metrics: Some(task_core::RunMetrics {
                wall_ms: 50,
                retries: 1,
                peak_context_tokens: None,
                turns: None,
            }),
            end: None,
        },
        serde_json::from_value(json!({
            "type": "routing_outcome_recorded", "outcome_id": "out-1",
            "decision_id": "decision-1", "run_id": "run-1",
            "evaluation_version": "routing-outcome/1", "acceptance_passed": true,
            "review_passed": true, "reward": 1.0
        }))
        .unwrap(),
    ] {
        env.store.append_event(id, &event).unwrap();
    }
    let now = time::OffsetDateTime::parse(
        "2026-10-05T10:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let reservation = env
        .store
        .routing_shadow_reserve(
            &ShadowDailyCaps {
                max_requests: 2,
                max_tokens: 1000,
                max_effective_usd: 1.0,
            },
            &ShadowReservationRequest {
                shadow_id: "shadow-timeout".into(),
                owner: "test".into(),
                worst_tokens: 500,
                worst_effective_usd: Some(0.2),
            },
            now,
        )
        .unwrap();
    let ShadowReservation::Reserved { reservation_id, .. } = reservation else {
        panic!("reservation denied")
    };
    env.store
        .routing_shadow_settle(&reservation_id, ShadowSettlement::TimedOut, now)
        .unwrap();
    for shadow in [
        json!({ "type": "routing_shadow_recorded", "shadow_id": "shadow-decision",
            "primary_decision_id": "decision-1", "run_id": "run-1", "kind": "decision",
            "status": "completed", "policy_version": "p4", "candidate_model": "candidate" }),
        json!({ "type": "routing_shadow_recorded", "shadow_id": "shadow-timeout",
            "primary_decision_id": "decision-1", "run_id": "run-1", "kind": "execution",
            "status": "failed", "reason": "timeout", "policy_version": "p4",
            "candidate_model": "candidate", "input_tokens": 20, "reservation_id": reservation_id }),
        json!({ "type": "routing_shadow_recorded", "shadow_id": "shadow-drop",
            "primary_decision_id": "decision-1", "run_id": "run-1", "kind": "execution",
            "status": "dropped", "reason": "queue_full", "policy_version": "p4" }),
    ] {
        env.store
            .append_event(id, &serde_json::from_value(shadow).unwrap())
            .unwrap();
    }
    let resp = send(
        &env.router(),
        get_with(&format!("/api/v1/tasks/{id}/routing"), &admin()),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let run = &resp.json()["runs"][0];
    assert_eq!(run["model"], "model-std");
    assert_eq!(run["routing_outcome"]["review_passed"], true);
    assert_eq!(run["retries"], 1);
    assert_eq!(run["wall_ms"], 50);
    let shadows = run["routing_shadow"].as_array().unwrap();
    assert_eq!(shadows.len(), 3);
    assert_eq!(shadows[0]["kind"], "decision");
    assert_eq!(shadows[0]["differs_from_primary"], true);
    assert_eq!(shadows[1]["status"], "failed");
    assert_eq!(shadows[1]["reason"], "timeout");
    assert_eq!(shadows[1]["input_tokens"], 20);
    assert_eq!(shadows[1]["reservation"]["state"], "timed_out");
    assert_eq!(shadows[1]["reservation"]["reserved_tokens"], 500);
    assert_eq!(shadows[1]["reservation"]["charged_tokens"], 500);
    assert_eq!(shadows[1]["reservation"]["reserved_effective_usd"], 0.2);
    assert_eq!(shadows[1]["reservation"]["charged_effective_usd"], 0.2);
    assert_eq!(shadows[2]["status"], "dropped");
    assert!(shadows[2].get("reservation").is_none());
    let stored = env.store.get(id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done);
    assert_eq!(stored.attempts, task.attempts);
}

/// Phase 5: estimator shadow は `routing_shadow[].estimator` と task の `estimator_shadow` 要約に出る。
/// coverage・失敗/timeout/dropped/prompt_required・推論 overhead・heuristic primary との差を返し、
/// primary の outcome・attempts・review は変えない。自由文の detail は写さない。
#[tokio::test]
async fn routing_estimator_shadow_report_records_coverage_and_limits() {
    let env = env_with_token();
    let task = new_task(TaskKind::Execute, Status::Done);
    env.seed(&task);
    let id = task.id;
    let mut primary = serde_json::to_value(routing_decided("run-1", None)).unwrap();
    primary["record"]["optimizer"] = trace_json("decision-1", "dispatch", Some("run-1"), None);
    for event in [
        serde_json::from_value(primary).unwrap(),
        Event::WorkerFinished {
            run_id: "run-1".into(),
            outcome: "done: ok".into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        },
        serde_json::from_value(json!({
            "type": "routing_outcome_recorded", "outcome_id": "out-1",
            "decision_id": "decision-1", "run_id": "run-1",
            "evaluation_version": "routing-outcome/1", "acceptance_passed": true,
            "review_passed": true, "reward": 1.0
        }))
        .unwrap(),
    ] {
        env.store.append_event(id, &event).unwrap();
    }
    let est = |shadow_id: &str, extra: serde_json::Value| {
        let mut v = json!({ "type": "routing_shadow_recorded", "shadow_id": shadow_id,
            "primary_decision_id": "decision-1", "run_id": "run-1", "kind": "estimator",
            "policy_version": "estimator:routellm-bert/0.2.2" });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    for shadow in [
        // Phase 4 の decision shadow は estimator 欄を持たない。
        json!({ "type": "routing_shadow_recorded", "shadow_id": "shadow-decision",
            "primary_decision_id": "decision-1", "run_id": "run-1", "kind": "decision",
            "status": "completed", "policy_version": "p4", "candidate_model": "candidate" }),
        est(
            "est-same",
            json!({ "status": "completed", "latency_ms": 10,
            "detail": "same_as_primary;heuristic:same_as_primary", "candidate_model": "model-std" }),
        ),
        est(
            "est-diff",
            json!({ "status": "completed", "latency_ms": 30,
            "detail": "differs_from_primary;heuristic:differs_from_primary",
            "candidate_model": "model-cheap" }),
        ),
        est(
            "est-timeout",
            json!({ "status": "failed", "reason": "timeout",
            "detail": "timeout", "latency_ms": 200 }),
        ),
        est(
            "est-prompt",
            json!({ "status": "dropped", "reason": "privacy",
            "detail": "prompt_required", "latency_ms": 2 }),
        ),
        est(
            "est-deps",
            json!({ "status": "dropped", "reason": "privacy",
            "detail": "dependencies_not_allowed" }),
        ),
        est(
            "est-upstream",
            json!({ "status": "failed", "reason": "upstream_error",
            "detail": "secret free text http://x", "policy_version": "estimator-bad" }),
        ),
        est(
            "est-busy",
            json!({ "status": "dropped", "reason": "concurrency_limit",
            "detail": "circuit_open" }),
        ),
    ] {
        env.store
            .append_event(id, &serde_json::from_value(shadow).unwrap())
            .unwrap();
    }
    let resp = send(
        &env.router(),
        get_with(&format!("/api/v1/tasks/{id}/routing"), &admin()),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    let run = &body["runs"][0];
    // primary は変わらない。
    assert_eq!(run["model"], "model-std");
    assert_eq!(run["routing_outcome"]["review_passed"], true);
    let shadows = run["routing_shadow"].as_array().unwrap();
    assert_eq!(shadows.len(), 8);
    assert!(shadows[0].get("estimator").is_none());
    let e = |i: usize| &shadows[i]["estimator"];
    assert_eq!(e(1)["estimator_id"], "routellm-bert");
    assert_eq!(e(1)["estimator_version"], "0.2.2");
    assert_eq!(e(1)["outcome"], "completed");
    assert_eq!(e(1)["vs_primary"], "same");
    assert_eq!(e(1)["vs_heuristic"], "same");
    assert_eq!(e(1)["overhead_ms"], 10);
    assert_eq!(shadows[1]["differs_from_primary"], false);
    assert_eq!(e(2)["vs_primary"], "differs");
    assert_eq!(e(2)["vs_heuristic"], "differs");
    assert_eq!(shadows[2]["differs_from_primary"], true);
    assert_eq!(e(3)["outcome"], "timeout");
    assert_eq!(e(3)["unavailable_reason"], "timeout");
    assert!(e(3).get("vs_primary").is_none());
    assert_eq!(e(4)["outcome"], "prompt_required");
    assert_eq!(e(4)["reason"], "privacy");
    assert_eq!(e(4)["dependencies"]["needs_prompt"], true);
    assert_eq!(e(5)["outcome"], "dropped");
    assert_eq!(e(5)["dependencies"]["not_allowed"], true);
    // 記録に無い依存は推定しない。
    assert!(e(5)["dependencies"].get("needs_network").is_none());
    assert!(e(5)["dependencies"].get("external_embeddings").is_none());
    assert_eq!(e(6)["outcome"], "failed");
    assert!(e(6).get("unavailable_reason").is_none());
    assert!(e(6).get("estimator_id").is_none());
    assert!(!resp.text().contains("secret free text"));
    assert_eq!(e(7)["outcome"], "dropped");
    assert_eq!(e(7)["unavailable_reason"], "circuit_open");

    let summary = &body["estimator_shadow"];
    assert_eq!(summary["targets"], 7);
    assert_eq!(summary["completed"], 2);
    assert_eq!(summary["failed"], 1);
    assert_eq!(summary["timeout"], 1);
    assert_eq!(summary["dropped"], 2);
    assert_eq!(summary["prompt_required"], 1);
    assert_eq!(summary["coverage"], 2.0 / 7.0);
    assert_eq!(summary["differs_from_primary"], 1);
    assert_eq!(summary["differs_from_heuristic"], 1);
    assert_eq!(summary["mean_overhead_ms"], 242.0 / 4.0);
    assert_eq!(summary["estimators"], json!(["routellm-bert/0.2.2"]));

    let stored = env.store.get(id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done);
    assert_eq!(stored.attempts, task.attempts);
}

/// run の監査（lane・model・規則・features・メトリクス・レビュー）と、捨てた担当が返る。
#[tokio::test]
async fn routing_returns_per_run_audit_and_dropped_assignee() {
    let env = env_with_token();
    let app = env.router();
    let mut task = new_task(TaskKind::Execute, Status::Done);
    task.assignee = Some("software-engineering".into());
    task.routing = Some(TaskRouting {
        tier_source: TierSource::Hint,
        dropped_assignee: Some("research".into()),
        ..TaskRouting::default()
    });
    env.seed(&task);
    let id = task.id;
    for (run, esc) in [
        ("run-1", None),
        ("run-2", Some("escalated: standard -> frontier")),
    ] {
        env.store
            .append_event(id, &routing_decided(run, esc))
            .expect("routing");
        env.store
            .append_event(
                id,
                &Event::WorkerFinished {
                    run_id: run.into(),
                    outcome: "done: ok".into(),
                    usage: None,
                    role: None,
                    metrics: Some(task_core::RunMetrics {
                        wall_ms: 1234,
                        retries: 1,
                        peak_context_tokens: None,
                        turns: None,
                    }),
                    end: None,
                },
            )
            .expect("finished");
    }

    let resp = send(
        &app,
        get_with(&format!("/api/v1/tasks/{id}/routing"), &admin()),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["task_id"], id.to_string());
    assert_eq!(body["assignee"], "software-engineering");
    assert_eq!(body["routing"]["dropped_assignee"], "research");
    assert_eq!(body["routing"]["tier_source"], "hint");
    let runs = body["runs"].as_array().cloned().expect("runs");
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0]["run_id"], "run-1");
    assert_eq!(runs[0]["org_node"], "software-engineering");
    assert_eq!(runs[0]["harness"], "coding");
    assert_eq!(runs[0]["lane"], "standard");
    assert_eq!(runs[0]["model"], "model-std");
    assert_eq!(runs[0]["rule_id"], "standard/default");
    assert_eq!(runs[0]["policy_version"], "v1");
    assert_eq!(runs[0]["features"]["judgment"], "low");
    assert_eq!(runs[0]["wall_ms"], 1234);
    assert_eq!(runs[0]["retries"], 1);
    assert!(runs[0].get("routing_shadow").is_none());
    assert!(runs[0].get("escalation").is_none());
    assert!(runs[0].get("optimizer").is_none());
    assert_eq!(runs[1]["escalation"], "escalated: standard -> frontier");
}

/// run の無いタスクは `runs: []`（200）。知らないタスクは 404、クエリは 400、トークン無しは 401。
#[tokio::test]
async fn routing_is_empty_without_runs_and_404_for_unknown_tasks() {
    let env = env_with_token();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);

    let resp = send(
        &app,
        get_with(&format!("/api/v1/tasks/{}/routing", task.id), &admin()),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["runs"], json!([]));
    assert!(body.get("routing").is_none() || body["routing"]["dropped_assignee"].is_null());

    let resp = send(
        &app,
        get_with(
            &format!("/api/v1/tasks/{}/routing", TaskId::new()),
            &admin(),
        ),
    )
    .await;
    assert_eq!(resp.status, 404, "{}", resp.text());

    let resp = send(
        &app,
        get_with(&format!("/api/v1/tasks/{}/routing?x=1", task.id), &admin()),
    )
    .await;
    assert_eq!(resp.status, 400, "{}", resp.text());

    let resp = send(&app, get(&format!("/api/v1/tasks/{}/routing", task.id))).await;
    assert_eq!(resp.status, 401, "{}", resp.text());
}

#[tokio::test]
async fn routing_projects_optional_optimizer_trace_without_changing_old_fields() {
    let env = env_with_token();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    let mut event = serde_json::to_value(routing_decided("run-trace", None)).unwrap();
    event["record"]["optimizer"] = json!({
        "decision_id": "decision-1",
        "parent_decision_id": null,
        "task_id": null,
        "work_unit_id": null,
        "run_id": "run-trace",
        "request_id": null,
        "stage": "dispatch",
        "mode": "legacy",
        "policy_version": "phase1-v1",
        "catalog_version": "phase1-v1",
        "feature_version": "1",
        "estimator_version": "1",
        "snapshot_id": "snapshot-1",
        "observed_at": null,
        "requested_lane": "standard",
        "selected_lane": "standard",
        "candidates": [],
        "selected": null,
        "fallback_order": [],
        "reasons": []
    });
    env.store
        .append_event(task.id, &serde_json::from_value(event).unwrap())
        .unwrap();
    let app = env.router();
    let resp = send(
        &app,
        get_with(&format!("/api/v1/tasks/{}/routing", task.id), &admin()),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let run = &resp.json()["runs"][0];
    assert_eq!(run["optimizer"]["decision_id"], "decision-1");
    assert_eq!(run["lane"], "standard");
    assert_eq!(run["model"], "model-std");
    assert_eq!(run["rule_id"], "standard/default");
}

fn trace_json(
    decision_id: &str,
    stage: &str,
    run_id: Option<&str>,
    request_id: Option<&str>,
) -> serde_json::Value {
    json!({
        "decision_id": decision_id,
        "run_id": run_id,
        "request_id": request_id,
        "stage": stage,
        "mode": "enforce",
        "policy_version": "p3",
        "catalog_version": "c1",
        "feature_version": "1",
        "estimator_version": "1",
        "snapshot_id": "snapshot-1",
        "requested_lane": "cheap",
        "selected_lane": "cheap",
        "candidates": [],
        "fallback_order": [],
        "reasons": []
    })
}

/// ADR 2026-10-04 Phase 3: run の dispatch 決定に proxy の要求（`routing_request_decided`）・実際の
/// source（dispatch では未確定だった model）・escalation の理由・最新の outcome を結ぶ。proxy log の無い
/// 要求は `audit_incomplete`。outcome 未追記と未レビュー（判定 null）を区別する。欄ごとに検査する。
#[tokio::test]
async fn routing_audit_links_run_request_and_actual_source() {
    let env = env_with_token();
    let task = new_task(TaskKind::Execute, Status::Done);
    env.seed(&task);
    let id = task.id;
    let decided = |run: &str, decision: &str| -> Event {
        let mut event = serde_json::to_value(routing_decided(
            run,
            Some("escalate cheap -> standard: 2 quality failures"),
        ))
        .unwrap();
        event["record"]["resolution"]["model_id"] = json!("");
        event["record"]["optimizer"] = trace_json(decision, "dispatch", Some(run), None);
        event["record"]["escalation"] = json!({
            "requested_lane": "cheap",
            "previous_lane": "cheap",
            "selected_lane": "standard",
            "reason": "2 quality failures at cheap",
            "counted_failures": 2,
            "interval_id": "initial"
        });
        serde_json::from_value(event).expect("routing_decided")
    };
    let request = |req: &str, decision: &str, parent: &str| -> Event {
        serde_json::from_value(json!({
            "type": "routing_request_decided",
            "request_id": req,
            "decision_id": decision,
            "parent_decision_id": parent,
            "trace": trace_json(decision, "proxy", None, Some(req)),
            "attempts": [
                {"source_id": "openai_compatible:qwen", "model": "qwen3", "fallback_reason": "rate_limit"},
                {"source_id": "claude-oauth:acc-1", "model": "claude-sonnet-5"}
            ],
            "fallback_reason": "rate_limit"
        }))
        .expect("routing_request_decided")
    };
    let outcome =
        |id: &str, run: &str, decision: &str, sup: Option<&str>, review: Option<bool>| -> Event {
            serde_json::from_value(json!({
                "type": "routing_outcome_recorded",
                "outcome_id": id,
                "decision_id": decision,
                "run_id": run,
                "evaluation_version": "routing-outcome/1",
                "supersedes": sup,
                "acceptance_passed": null,
                "review_passed": review,
                "reward": review.map(|p| if p { 0.9 } else { -0.1 })
            }))
            .expect("routing_outcome_recorded")
        };
    let features: Event = serde_json::from_value(json!({
        "type": "routing_features_recorded",
        "decision_id": "dec-run-1",
        "context_version": "routing-context/1",
        "features": {"phase": "implementation", "attempts": 2},
        "provenance": {"phase": "run.role"},
        "missing_fields": ["external_network"],
        "run_id": "run-1",
        "stage": "dispatch"
    }))
    .unwrap();
    for event in [
        decided("run-1", "dec-run-1"),
        features,
        request("req-1", "dec-req-1", "dec-run-1"),
        outcome("out-1", "run-1", "dec-run-1", None, None),
        outcome("out-2", "run-1", "dec-run-1", Some("out-1"), Some(true)),
        decided("run-2", "dec-run-2"),
        request("req-2", "dec-req-2", "dec-run-2"),
        decided("run-3", "dec-run-3"),
        outcome("out-3", "run-3", "dec-run-3", None, None),
    ] {
        env.store.append_event(id, &event).expect("append");
    }
    // req-1 だけ proxy log に行がある（実際の source/model はここが正）。
    {
        let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
        conn.execute(
            "INSERT INTO llm_proxy_requests (id, ts, source, account, requested_model, \
             upstream_model, latency_ms, status) \
             VALUES ('req-1', 1700000000, 'claude-oauth', 'acc-1', 'celeris/cheap', \
             'claude-sonnet-5', 120, 'ok')",
            [],
        )
        .expect("insert proxy log");
    }
    assert!(
        env.store
            .routing_correlation_set(
                "req-1",
                &task_core::store::RoutingCorrelation {
                    decision_id: Some("dec-req-1".into()),
                    run_id: Some("run-1".into()),
                    source_id: Some("claude-oauth:acc-1".into()),
                    model: Some("claude-sonnet-5".into()),
                    account: Some("acc-1".into()),
                    ..Default::default()
                },
            )
            .expect("correlation")
    );

    let app = env.router();
    let resp = send(
        &app,
        get_with(&format!("/api/v1/tasks/{id}/routing"), &admin()),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    let runs = body["runs"].as_array().cloned().expect("runs");
    assert_eq!(runs.len(), 3, "{runs:?}");

    // run-1: dispatch では model 未確定（resolution.model_id 空）→ request の proxy log から実際の model。
    let r1 = &runs[0];
    assert_eq!(r1["run_id"], "run-1");
    assert_eq!(r1["decision_id"], "dec-run-1");
    assert!(r1.get("model").is_none(), "{r1}");
    assert_eq!(r1["audit_incomplete"], false);
    let req = &r1["requests"][0];
    assert_eq!(req["request_id"], "req-1");
    assert_eq!(req["decision_id"], "dec-req-1");
    assert_eq!(req["parent_decision_id"], "dec-run-1");
    assert_eq!(req["fallback_reason"], "rate_limit");
    assert_eq!(req["attempts"].as_array().map(Vec::len), Some(2));
    assert_eq!(req["log"]["model"], "claude-sonnet-5");
    assert_eq!(req["actual"]["source_id"], "claude-oauth:acc-1");
    assert_eq!(req["actual"]["model"], "claude-sonnet-5");
    assert_eq!(req["actual"]["from"], "proxy_log");
    assert_eq!(r1["actual_sources"][0]["model"], "claude-sonnet-5");
    // escalation の理由（文字列と構造化した監査）
    assert_eq!(
        r1["escalation"],
        "escalate cheap -> standard: 2 quality failures"
    );
    assert_eq!(r1["escalation_audit"]["selected_lane"], "standard");
    assert_eq!(
        r1["escalation_audit"]["reason"],
        "2 quality failures at cheap"
    );
    assert_eq!(r1["escalation_audit"]["counted_failures"], 2);
    // dispatch 時点の特徴
    assert_eq!(
        r1["routing_features"]["context_version"],
        "routing-context/1"
    );
    assert_eq!(
        r1["routing_features"]["missing_fields"][0],
        "external_network"
    );
    // 最新の outcome（out-1 は out-2 に supersede された）
    assert_eq!(r1["routing_outcome"]["outcome_id"], "out-2");
    assert_eq!(r1["routing_outcome"]["supersedes"], "out-1");
    assert_eq!(r1["routing_outcome"]["review_passed"], true);
    assert_eq!(r1["outcome_state"], "judged");

    // run-2: 要求の proxy log が無い → audit_incomplete。実 source は試した最後の source。
    let r2 = &runs[1];
    assert_eq!(r2["run_id"], "run-2");
    assert_eq!(r2["audit_incomplete"], true);
    assert_eq!(r2["incomplete_reasons"], json!(["request_log_missing"]));
    assert_eq!(
        r2["requests"][0]["incomplete_reason"],
        "request_log_missing"
    );
    assert!(r2["requests"][0].get("log").is_none());
    assert_eq!(r2["requests"][0]["actual"]["model"], "claude-sonnet-5");
    assert_eq!(r2["requests"][0]["actual"]["from"], "request_attempts");
    // outcome 未追記
    assert!(r2.get("routing_outcome").is_none());
    assert_eq!(r2["outcome_state"], "not_recorded");

    // run-3: outcome はあるが未レビュー（review_passed は null で、false ではない）。
    let r3 = &runs[2];
    assert_eq!(r3["outcome_state"], "unreviewed");
    assert!(r3["routing_outcome"]["review_passed"].is_null());
    assert!(r3["routing_outcome"]["reward"].is_null());
    assert_eq!(r3["requests"], json!([]));
    assert_eq!(r3["audit_incomplete"], false);
}
