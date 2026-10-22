//! ADR 2026-10-11-llm-source-model-routing-evaluation D3: `GET`/`POST /routing/stages` と `GET /routing/evidence`。

mod common;

use common::*;
use serde_json::json;
use task_core::routing_evidence::{EvidenceKey, EvidenceRow};
use task_core::{Status, TaskKind, TaskStore};

fn row(n: u64, pass_lower: f64) -> EvidenceRow {
    EvidenceRow {
        quota_cost_per_pass: None,
        key: EvidenceKey {
            lane: String::new(),
            genre: "coding".into(),
            harness_id: "codex".into(),
            harness_version: "1.4.0".into(),
            source_id: "openai".into(),
            model_id: "gpt-5".into(),
        },
        evidence_version: String::new(),
        n,
        pass: n,
        pass_lower,
        pass_upper: 0.97,
        pass_boot_lower: pass_lower,
        pass_boot_upper: 0.97,
        wall_p90_s: Some(41.0),
        usd_per_pass: Some(0.3),
        quota_per_run: [("7d".to_string(), 0.002)].into(),
        timeout_rate: 0.0,
        low_confidence: false,
    }
}

#[tokio::test]
async fn stages_and_evidence_are_listed_and_enforce_requires_d1_evidence() {
    let env = TestEnv::new();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Draft);
    env.seed(&task);

    let empty = send(&app, get("/api/v1/routing/stages")).await;
    assert_eq!(empty.status.as_u16(), 200, "{}", empty.text());
    assert_eq!(empty.json()["stages"], json!([]));
    assert_problem(
        &send(&app, get("/api/v1/routing/stages?bogus=1")).await,
        400,
        "bad_request",
    );

    let body = |stage: &str| {
        json!({"task_id": task.id.to_string(), "genre": "coding", "harness_id": "codex",
               "stage": stage, "reason": "evidence reviewed"})
    };
    // 根拠表が無い → 409。
    assert_problem(
        &send(&app, post_json("/api/v1/routing/stages", &body("enforce"))).await,
        409,
        "routing_evidence_insufficient",
    );
    // 根拠表はあるが D1 を満たさない（Wilson 下限 < 0.5）→ 409。
    env.store
        .replace_routing_evidence("2026-10-13.1", &[row(30, 0.42)])
        .unwrap();
    assert_problem(
        &send(&app, post_json("/api/v1/routing/stages", &body("enforce"))).await,
        409,
        "routing_evidence_insufficient",
    );
    // 根拠表だけある組は shadow・since 無しで出る。
    let listed = send(&app, get("/api/v1/routing/stages")).await.json();
    assert_eq!(listed["stages"][0]["stage"], "shadow");
    assert_eq!(listed["stages"][0]["since"], json!(null));
    assert_eq!(
        listed["stages"][0]["latest_evidence_version"],
        "2026-10-13.1"
    );

    env.store
        .replace_routing_evidence("2026-10-13.2", &[row(30, 0.61)])
        .unwrap();
    let evidence = send(&app, get("/api/v1/routing/evidence?genre=coding")).await;
    assert_eq!(evidence.status.as_u16(), 200, "{}", evidence.text());
    let evidence = evidence.json();
    assert_eq!(evidence["groups"][0]["evidence_version"], "2026-10-13.2");
    assert_eq!(evidence["groups"][0]["rows"][0]["qualifies"], true);
    assert_eq!(evidence["groups"][0]["rows"][0]["pass_upper"], 0.97);
    let other = send(&app, get("/api/v1/routing/evidence?genre=research")).await;
    assert_eq!(other.json()["groups"], json!([]));

    // 空白だけの理由は 422、不明な task は 404、未知の欄は 400。
    let mut blank = body("enforce");
    blank["reason"] = json!("  ");
    assert_problem(
        &send(&app, post_json("/api/v1/routing/stages", &blank)).await,
        422,
        "validation",
    );
    let mut unknown = body("enforce");
    unknown["task_id"] = json!(task_core::TaskId::new().to_string());
    assert_problem(
        &send(&app, post_json("/api/v1/routing/stages", &unknown)).await,
        404,
        "task_not_found",
    );
    let mut extra = body("enforce");
    extra["actor"] = json!("daemon");
    assert_problem(
        &send(&app, post_json("/api/v1/routing/stages", &extra)).await,
        400,
        "bad_request",
    );

    let approved = send(&app, post_json("/api/v1/routing/stages", &body("enforce"))).await;
    assert_eq!(approved.status.as_u16(), 200, "{}", approved.text());
    let approved = approved.json();
    assert_eq!(approved["stage"], "enforce");
    assert_eq!(approved["evidence_version"], "2026-10-13.2");
    assert_eq!(approved["last_change"]["actor"], "human");
    assert_eq!(approved["last_change"]["from"], json!(null));
    assert_eq!(approved["last_change"]["task_id"], task.id.to_string());

    // shadow へ戻すのは根拠を問わない。直近の変更が出る。
    let back = send(&app, post_json("/api/v1/routing/stages", &body("shadow"))).await;
    assert_eq!(back.status.as_u16(), 200, "{}", back.text());
    let listed = send(&app, get("/api/v1/routing/stages?harness_id=codex"))
        .await
        .json();
    assert_eq!(listed["stages"][0]["stage"], "shadow");
    assert_eq!(listed["stages"][0]["last_change"]["from"], "enforce");
    assert_eq!(listed["stages"][0]["last_change"]["to"], "shadow");
    let events = env.store.events_for(task.id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(e, task_core::Event::RoutingStageChanged { .. }))
            .count(),
        2
    );
}
