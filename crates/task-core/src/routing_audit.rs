//! ADR-0069 D5（Phase 114）: run ごとの routing の監査記録（担当・harness・lane・model・features・規則・
//! policy の版）と、Phase 104 のメトリクス（cost・tokens・wall time・retries）とレビュー結果を
//! 突き合わせる純粋関数。イベントは古い順に渡す（`TaskStore::events_for` の順）。
//!
//! ADR 2026-10-04-multi-objective-model-routing Phase 3（§5・§6）: run の dispatch 決定に、構造化した
//! escalation の監査（`RoutingRecord.escalation`）、dispatch 時点の特徴 snapshot（`routing_features_recorded`）、
//! 最新の（supersede されていない）run 単位の `routing_outcome_recorded` を結ぶ。未レビューは
//! `outcome_state = unreviewed`（outcome の合否は null のまま。false とみなさない）、outcome が未追記なら
//! `not_recorded` と区別する。Phase 2 の trace も Phase 3 の event も持たない旧 run の新欄は None。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Event, RunRole, Task, TaskId, Tier};
use crate::model_policy::TaskFeatures;
use crate::model_router::feedback::{FeatureStage, RoutingFeaturesRecord, RoutingOutcome};
use crate::retry_policy::EscalationAudit;

/// run の routing outcome の状態（§6: 未判定を false や品質 0 とみなさないための区別）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingOutcomeState {
    /// `routing_outcome_recorded` がまだ無い（run 中・未投影）。
    NotRecorded,
    /// outcome はあるが review・acceptance の判定がどちらも null（未レビュー・中断）。
    Unreviewed,
    /// review か acceptance の判定がある。
    Judged,
}

/// レビューの結果（その run の後の `review_pass` / `review_fail`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub passed: bool,
    /// 不合格だった受け入れ条件の番号。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_criteria: Vec<usize>,
}

/// ワーカー run 1 件の routing の監査。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingAudit {
    pub task_id: TaskId,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<TaskFeatures>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewResult>,
    /// Phase 1 optimizer trace. Absent from events written before model routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimizer: Option<crate::model_router::trace::RoutingTraceV1>,
    /// Phase 3: dispatch の decision id（`optimizer.decision_id`）。feature・outcome はこの id で結ぶ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// Phase 3: run 間 escalation の構造化監査（`RoutingRecord.escalation`）。旧 event は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_audit: Option<EscalationAudit>,
    /// Phase 3: dispatch 時点の特徴 snapshot（stage dispatch の `routing_features_recorded`。最後の 1 件）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_features: Option<RoutingFeaturesRecord>,
    /// Phase 3: run 単位の最新の outcome（supersede された旧 outcome は出さない）。未追記は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_outcome: Option<RoutingOutcome>,
    /// Phase 3: outcome の状態。dispatch の decision id も outcome も無い旧 run は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_state: Option<RoutingOutcomeState>,
}

/// run 単位の outcome のうち、後の outcome に supersede されていない最後の 1 件。
fn latest_outcome<'a>(outcomes: &[&'a RoutingOutcome]) -> Option<&'a RoutingOutcome> {
    let superseded: std::collections::BTreeSet<&str> = outcomes
        .iter()
        .filter_map(|o| o.supersedes.as_deref())
        .collect();
    outcomes
        .iter()
        .rev()
        .find(|o| !superseded.contains(o.outcome_id.as_str()))
        .copied()
}

/// タスク 1 件のイベント列から、ワーカー run ごとの監査を作る（古い run が先）。
pub fn routing_audit(task: &Task, events: &[Event]) -> Vec<RoutingAudit> {
    let mut out: Vec<RoutingAudit> = Vec::new();
    // レビューの判定を最後のワーカー run に帰属させる。
    let mut last_worker: Option<usize> = None;
    let mut failed: Vec<usize> = Vec::new();
    let find = |out: &mut Vec<RoutingAudit>, run_id: &str| -> usize {
        match out.iter().position(|a| a.run_id == run_id) {
            Some(i) => i,
            None => {
                out.push(RoutingAudit {
                    task_id: task.id,
                    run_id: run_id.to_string(),
                    org_node: task.assignee.clone(),
                    harness: task.genre.clone(),
                    ..RoutingAudit::default()
                });
                out.len() - 1
            }
        }
    };
    for event in events {
        match event {
            Event::WorkerStarted {
                run_id,
                adapter,
                model,
                provider,
                account,
                role: None,
                ..
            } => {
                let i = find(&mut out, run_id);
                let a = &mut out[i];
                a.adapter = Some(adapter.clone());
                if !model.is_empty() {
                    a.model = Some(model.clone());
                }
                a.provider = provider.clone();
                a.account = account.clone();
                last_worker = Some(i);
                failed.clear();
            }
            Event::RoutingDecided { run_id, record } => {
                let i = find(&mut out, run_id);
                let a = &mut out[i];
                if record.org_node.is_some() {
                    a.org_node = record.org_node.clone();
                }
                if record.harness.is_some() {
                    a.harness = record.harness.clone();
                }
                a.lane = Some(record.resolution.lane.unwrap_or(record.decision.lane));
                if !record.resolution.model_id.is_empty() {
                    a.model = Some(record.resolution.model_id.clone());
                }
                a.reasoning_effort = record.resolution.reasoning_effort.clone();
                a.features = Some(record.decision.features);
                a.rule_id = Some(record.decision.rule_id.clone());
                a.policy_version = Some(record.decision.policy_version.clone());
                a.reasons = record.decision.reasons.clone();
                a.escalation = record.decision.escalation.clone();
                a.escalation_audit = record.escalation.clone();
                a.decision_id = record.optimizer.as_ref().map(|t| t.decision_id.clone());
                a.optimizer = record.optimizer.clone();
            }
            Event::WorkerFinished {
                run_id,
                outcome,
                usage,
                role,
                metrics,
                ..
            } if *role != Some(RunRole::Reviewer) => {
                let i = find(&mut out, run_id);
                let a = &mut out[i];
                a.outcome = Some(outcome.clone());
                if let Some(u) = usage {
                    a.cost_usd = u.cost_usd;
                    a.input_tokens = u.input_tokens;
                    a.output_tokens = u.output_tokens;
                }
                if let Some(m) = metrics {
                    a.wall_ms = Some(m.wall_ms);
                    a.retries = Some(m.retries);
                }
            }
            Event::ReviewVerdict {
                criterion_idx,
                pass: false,
                ..
            } => failed.push(*criterion_idx),
            Event::Transitioned { reason, .. }
                if reason == "review_pass" || reason == "review_fail" =>
            {
                if let Some(i) = last_worker {
                    out[i].review = Some(ReviewResult {
                        passed: reason == "review_pass",
                        failed_criteria: std::mem::take(&mut failed),
                    });
                }
            }
            _ => {}
        }
    }
    attach_feedback(&mut out, events);
    out
}

/// Phase 3 の feature・outcome event を run に結ぶ。run_id で結び、無ければ decision id で結ぶ（推定はしない）。
fn attach_feedback(out: &mut [RoutingAudit], events: &[Event]) {
    let matches = |a: &RoutingAudit, run_id: Option<&str>, decision_id: &str| match run_id {
        Some(r) => a.run_id == r,
        None => a.decision_id.as_deref() == Some(decision_id),
    };
    for a in out.iter_mut() {
        for event in events {
            if let Event::RoutingFeaturesRecorded { record } = event
                && record.stage != Some(FeatureStage::Proxy)
                && record.request_id.is_none()
                && matches(a, record.run_id.as_deref(), &record.decision_id)
            {
                a.routing_features = Some((**record).clone());
            }
        }
        let outcomes: Vec<&RoutingOutcome> = events
            .iter()
            .filter_map(|e| match e {
                Event::RoutingOutcomeRecorded { outcome }
                    if outcome.request_id.is_none()
                        && matches(a, outcome.run_id.as_deref(), &outcome.decision_id) =>
                {
                    Some(&**outcome)
                }
                _ => None,
            })
            .collect();
        a.routing_outcome = latest_outcome(&outcomes).cloned();
        a.outcome_state = match &a.routing_outcome {
            Some(o) if o.review_passed.is_some() || o.acceptance_passed.is_some() => {
                Some(RoutingOutcomeState::Judged)
            }
            Some(_) => Some(RoutingOutcomeState::Unreviewed),
            None if a.decision_id.is_some() => Some(RoutingOutcomeState::NotRecorded),
            None => None,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RunMetrics, Status, TaskRouting, TierSource, Usage};
    use crate::model_policy::{LaneCeiling, decide_for_task};
    use crate::model_routing::LaneResolution;

    #[test]
    fn joins_routing_usage_wall_retries_and_review_per_run() {
        let mut t = crate::model_policy::tests::task("crates/x.rs の typo を直す", vec![]);
        t.assignee = Some("software-engineering".into());
        t.routing = Some(TaskRouting {
            tier_source: TierSource::Default,
            ..TaskRouting::default()
        });
        let decision = decide_for_task(&t, &LaneCeiling::default()).unwrap();
        let record = crate::model_policy::RoutingRecord {
            org_node: Some("software-engineering".into()),
            harness: Some("coding".into()),
            decision: decision.clone(),
            resolution: LaneResolution {
                lane: Some(Tier::Standard),
                adapter: "claude-code".into(),
                provider: Some("cc-1".into()),
                account: None,
                model_id: "model-std".into(),
                reasoning_effort: Some("medium".into()),
                selection: None,
            },
            quota_reason: None,
            work_unit_id: None,
            escalation: None,
            optimizer: None,
        };
        let events = vec![
            Event::WorkerStarted {
                run_id: "r1".into(),
                adapter: "claude-code".into(),
                model: "model-std".into(),
                provider: Some("cc-1".into()),
                account: None,
                role: None,
                task_role: None,
            },
            Event::RoutingDecided {
                run_id: "r1".into(),
                record: Box::new(record),
            },
            Event::WorkerFinished {
                run_id: "r1".into(),
                outcome: "done: ok".into(),
                usage: Some(Usage {
                    input_tokens: Some(100),
                    output_tokens: Some(20),
                    cost_usd: Some(0.5),
                    ..Usage::default()
                }),
                role: None,
                metrics: Some(RunMetrics {
                    wall_ms: 1234,
                    retries: 1,
                    peak_context_tokens: None,
                    turns: None,
                }),
                end: None,
            },
            // reviewer run の WorkerFinished は数えない
            Event::WorkerFinished {
                run_id: "rev".into(),
                outcome: "done: judged".into(),
                usage: None,
                role: Some(RunRole::Reviewer),
                metrics: None,
                end: None,
            },
            Event::ReviewVerdict {
                run_id: "rev".into(),
                criterion_idx: 1,
                pass: false,
                reason: "missing".into(),
            },
            Event::Transitioned {
                from: Status::Reviewing,
                to: Status::Ready,
                reason: "review_fail".into(),
            },
        ];
        let audit = routing_audit(&t, &events);
        assert_eq!(audit.len(), 1, "{audit:?}");
        let a = &audit[0];
        assert_eq!(a.run_id, "r1");
        assert_eq!(a.org_node.as_deref(), Some("software-engineering"));
        assert_eq!(a.harness.as_deref(), Some("coding"));
        assert_eq!(a.adapter.as_deref(), Some("claude-code"));
        assert_eq!(a.lane, Some(Tier::Standard));
        assert_eq!(a.model.as_deref(), Some("model-std"));
        assert_eq!(a.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(a.features, Some(decision.features));
        assert_eq!(a.rule_id.as_deref(), Some(decision.rule_id.as_str()));
        assert_eq!(a.policy_version.as_deref(), Some("lane-policy/1"));
        assert_eq!(a.cost_usd, Some(0.5));
        assert_eq!((a.input_tokens, a.output_tokens), (Some(100), Some(20)));
        assert_eq!((a.wall_ms, a.retries), (Some(1234), Some(1)));
        assert_eq!(
            a.review,
            Some(ReviewResult {
                passed: false,
                failed_criteria: vec![1]
            })
        );
    }

    /// 導入前のイベント（`RoutingDecided` 無し）だけでも run は並び、routing の欄が空になるだけ。
    /// `RoutingDecided` を知らない旧いイベント JSON もそのまま読める。
    #[test]
    fn legacy_events_without_routing_still_aggregate() {
        let t = crate::model_policy::tests::task("x", vec![]);
        let old: Event = serde_json::from_str(
            r#"{"type":"worker_finished","run_id":"r0","outcome":"done: ok","usage":null}"#,
        )
        .unwrap();
        let audit = routing_audit(&t, &[old]);
        assert_eq!(audit.len(), 1);
        assert!(audit[0].lane.is_none() && audit[0].rule_id.is_none());
        assert_eq!(audit[0].outcome.as_deref(), Some("done: ok"));
    }

    fn ev(v: serde_json::Value) -> Event {
        serde_json::from_value(v).unwrap()
    }

    /// Phase 3: features・escalation・最新 outcome を run に結び、supersede された outcome は出さない。
    /// 未レビュー（判定 null）と outcome 未追記を区別する。
    #[test]
    fn joins_features_escalation_and_latest_outcome_per_run() {
        let t = crate::model_policy::tests::task("x", vec![]);
        let decision = decide_for_task(&t, &LaneCeiling::default()).unwrap();
        let mut trace: crate::model_router::trace::RoutingTraceV1 =
            serde_json::from_value(serde_json::json!({
                "decision_id": "d1", "stage": "dispatch", "mode": "shadow",
                "policy_version": "p", "catalog_version": "c", "feature_version": "1",
                "estimator_version": "e", "snapshot_id": "s", "requested_lane": "cheap",
                "candidates": [], "fallback_order": [], "reasons": []
            }))
            .unwrap();
        trace.run_id = Some("r1".into());
        let record = |trace: Option<crate::model_router::trace::RoutingTraceV1>| {
            Box::new(crate::model_policy::RoutingRecord {
                org_node: None,
                harness: None,
                decision: decision.clone(),
                resolution: LaneResolution::default(),
                quota_reason: None,
                work_unit_id: None,
                escalation: Some(EscalationAudit {
                    requested_lane: Tier::Cheap,
                    previous_lane: Some(Tier::Cheap),
                    selected_lane: Tier::Standard,
                    reason: "2 quality failures".into(),
                    counted_failures: 2,
                    interval_id: "initial".into(),
                }),
                optimizer: trace,
            })
        };
        let outcome = |id: &str, run: &str, sup: Option<&str>, review: Option<bool>| {
            ev(serde_json::json!({
                "type": "routing_outcome_recorded", "outcome_id": id, "decision_id": "d1",
                "run_id": run, "evaluation_version": "routing-outcome/1",
                "supersedes": sup, "review_passed": review
            }))
        };
        let events = vec![
            Event::RoutingDecided {
                run_id: "r1".into(),
                record: record(Some(trace.clone())),
            },
            ev(serde_json::json!({
                "type": "routing_features_recorded", "decision_id": "d1",
                "context_version": "routing-context/1", "features": {"phase": "implementation"},
                "missing_fields": ["network"], "run_id": "r1", "stage": "dispatch"
            })),
            // proxy 時点の特徴は run の特徴にしない
            ev(serde_json::json!({
                "type": "routing_features_recorded", "decision_id": "p1",
                "context_version": "routing-context/1", "run_id": "r1",
                "request_id": "q1", "stage": "proxy"
            })),
            outcome("o1", "r1", None, None),
            outcome("o2", "r1", Some("o1"), Some(true)),
            // 要求単位の outcome は run の outcome にしない
            ev(serde_json::json!({
                "type": "routing_outcome_recorded", "outcome_id": "o3", "decision_id": "d1",
                "run_id": "r1", "request_id": "q1", "evaluation_version": "routing-outcome/1"
            })),
            Event::RoutingDecided {
                run_id: "r2".into(),
                record: record(Some({
                    let mut t2 = trace.clone();
                    t2.decision_id = "d2".into();
                    t2.run_id = Some("r2".into());
                    t2
                })),
            },
            Event::RoutingDecided {
                run_id: "r3".into(),
                record: record(None),
            },
        ];
        let audit = routing_audit(&t, &events);
        assert_eq!(audit.len(), 3, "{audit:?}");
        let a = &audit[0];
        assert_eq!(a.decision_id.as_deref(), Some("d1"));
        assert_eq!(
            a.escalation_audit.as_ref().map(|e| e.selected_lane),
            Some(Tier::Standard)
        );
        let f = a.routing_features.as_ref().expect("features");
        assert_eq!(f.decision_id, "d1");
        assert_eq!(f.missing_fields, vec!["network".to_string()]);
        let o = a.routing_outcome.as_ref().expect("outcome");
        assert_eq!(o.outcome_id, "o2");
        assert_eq!(o.review_passed, Some(true));
        assert_eq!(a.outcome_state, Some(RoutingOutcomeState::Judged));
        // decision はあるが outcome 未追記
        assert!(audit[1].routing_outcome.is_none());
        assert_eq!(
            audit[1].outcome_state,
            Some(RoutingOutcomeState::NotRecorded)
        );
        // Phase 2 trace の無い run は状態を出さない
        assert!(audit[2].outcome_state.is_none() && audit[2].decision_id.is_none());

        // 判定の無い outcome は unreviewed（false とみなさない）
        let unreviewed = routing_audit(&t, &events[..4]);
        assert_eq!(
            unreviewed[0].outcome_state,
            Some(RoutingOutcomeState::Unreviewed)
        );
        assert_eq!(
            unreviewed[0]
                .routing_outcome
                .as_ref()
                .map(|o| o.review_passed),
            Some(None)
        );
    }
}
