//! ADR 2026-10-04-multi-objective-model-routing Phase 2: `GET /llm/sources` の `deployments[]`
//! （鮮度・unknown・請求と機会費用）。時刻は固定（`source_state_view` に注入する）。

mod common;

use std::sync::Arc;

use common::*;
use task_api::llm_sources::source_state_view;
use task_api::{LlmSourceView, LlmSourcesReader, LlmSourcesView};
use task_core::model_router::cost::CostEstimate;
use task_core::model_router::profiles::{QuotaWindow, Reachability, SourceState};

/// 2026-10-05T00:00:00Z。
const NOW: i64 = 1_791_158_400;

fn subscription_state() -> SourceState {
    let mut s = SourceState::unobserved("claude-sub");
    s.observed_at = Some("2026-10-04T23:59:30Z".into());
    s.expires_at = Some("2026-10-05T00:05:00Z".into());
    s.reachability = Reachability::Up;
    s.latency_ms = Some(820.0);
    s.quota_windows = vec![
        QuotaWindow {
            account_id: "acct-1".into(),
            window_id: "5h".into(),
            unit: "fraction".into(),
            remaining: Some(0.6),
            limit: Some(1.0),
            reset_at: Some("2026-10-05T02:00:00Z".into()),
            measured: true,
            observed_at: Some("2026-10-04T23:59:30Z".into()),
            window_duration_s: None,
            estimated_consumption: None,
            reserve_value_usd: Some(5.0),
        },
        QuotaWindow {
            account_id: "acct-1".into(),
            window_id: "weekly".into(),
            unit: "fraction".into(),
            remaining: Some(0.4),
            limit: Some(1.0),
            reset_at: Some("2026-10-09T00:00:00Z".into()),
            measured: true,
            observed_at: Some("2026-10-04T23:59:30Z".into()),
            window_duration_s: None,
            estimated_consumption: None,
            reserve_value_usd: Some(5.0),
        },
    ];
    s
}

struct StateReader;

#[async_trait::async_trait]
impl LlmSourcesReader for StateReader {
    async fn view(&self, _now: i64) -> LlmSourcesView {
        let sub_cost = CostEstimate {
            cash_usd: Some(0.0),
            subscription_shadow_usd: Some(0.012),
            self_host_resource_usd: Some(0.0),
            effective_usd: Some(0.012),
            pressure: None,
            assumptions: vec!["shadow_from_window:5h".into()],
        };
        let mut local = SourceState::unobserved("qwen-local");
        local.in_use = Some(1);
        LlmSourcesView {
            sources: vec![
                LlmSourceView {
                    id: "claude-oauth".into(),
                    kind: "claude-oauth".into(),
                    enabled: true,
                    reachable: None,
                    unreachable_reason: None,
                    accounts: vec![],
                    last_hour_requests: 0,
                    last_hour_prompt_tokens: 0,
                    last_hour_completion_tokens: 0,
                    deployments: vec![source_state_view(
                        &subscription_state(),
                        Some(&sub_cost),
                        None,
                        NOW,
                    )],
                },
                LlmSourceView {
                    id: "openai_compatible:qwen".into(),
                    kind: "openai-compatible".into(),
                    enabled: true,
                    reachable: Some(true),
                    unreachable_reason: None,
                    accounts: vec![],
                    last_hour_requests: 1,
                    last_hour_prompt_tokens: 2,
                    last_hour_completion_tokens: 3,
                    deployments: vec![source_state_view(&local, None, Some(4), NOW)],
                },
            ],
            celeris_tiers: vec![],
        }
    }
}

#[tokio::test]
async fn routing_source_api_reports_freshness_and_cost_components() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        llm_sources: Some(Arc::new(StateReader)),
        ..Default::default()
    });
    let app = env.router();
    let resp = send(
        &app,
        get_with(
            "/api/v1/llm/sources",
            &[("authorization", &format!("Bearer {TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();

    // 旧欄は残る。
    assert_eq!(body["sources"][1]["last_hour_requests"], 1);
    assert_eq!(body["sources"][1]["reachable"], true);

    // 観測済みの subscription: 鮮度・latency・最小残量と最も早い reset。
    let sub = &body["sources"][0]["deployments"][0];
    assert_eq!(sub["deployment_id"], "claude-sub");
    assert_eq!(sub["freshness"]["observed_at"], "2026-10-04T23:59:30Z");
    assert_eq!(sub["freshness"]["age_secs"], 30);
    assert_eq!(sub["freshness"]["stale"], false);
    assert_eq!(sub["reachability"], "up");
    assert_eq!(sub["latency_ms"], 820.0);
    assert_eq!(sub["quota_remaining"], 0.4);
    assert_eq!(sub["quota_reset_at"], "2026-10-05T02:00:00Z");
    assert_eq!(sub["pressure"], serde_json::Value::Null);
    assert_eq!(sub["unknown"], serde_json::json!(["pressure"]));
    // 請求（billed）と機会費用（opportunity）は別の欄。
    assert_eq!(sub["cost"]["billed"]["cash_usd"], 0.0);
    assert_eq!(sub["cost"]["opportunity"]["shadow_usd"], 0.012);
    assert_eq!(sub["cost"]["opportunity"]["resource_usd"], 0.0);
    assert_eq!(sub["cost"]["effective_usd"], 0.012);
    assert!(sub["cost"]["billed"].get("shadow_usd").is_none());
    assert!(sub["cost"]["opportunity"].get("cash_usd").is_none());

    // 未観測の self-host: 鮮度は null かつ stale、未知は 0 で埋めず unknown に名前が並ぶ。
    let local = &body["sources"][1]["deployments"][0];
    assert_eq!(local["freshness"]["observed_at"], serde_json::Value::Null);
    assert_eq!(local["freshness"]["age_secs"], serde_json::Value::Null);
    assert_eq!(local["freshness"]["stale"], true);
    assert_eq!(local["reachability"], "unknown");
    assert_eq!(local["latency_ms"], serde_json::Value::Null);
    assert_eq!(local["quota_remaining"], serde_json::Value::Null);
    assert_eq!(local["pressure"], 0.25);
    assert_eq!(local["cost"], serde_json::Value::Null);
    assert_eq!(
        local["unknown"],
        serde_json::json!([
            "cash",
            "effective",
            "latency",
            "observed_at",
            "quota",
            "quota_reset",
            "resource",
            "shadow"
        ])
    );
}

#[test]
fn source_state_view_marks_expired_snapshot_stale() {
    let mut s = subscription_state();
    s.expires_at = Some("2026-10-04T23:59:59Z".into());
    let v = source_state_view(&s, None, None, NOW);
    assert!(v.freshness.stale);
    assert_eq!(v.freshness.age_secs, Some(30));
    // 同じ入力から同じ投影。
    assert_eq!(v, source_state_view(&s, None, None, NOW));
}
