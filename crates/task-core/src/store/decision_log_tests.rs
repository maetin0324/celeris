//! ADR 2026-10-15-decision-log-export の記録: 誤帰属なし・再処理で二重計上なし・別ラベル・
//! classifier_confidence と propensity の分離・旧 event の読み取り。
use time::OffsetDateTime;

use super::*;
use crate::decision_log::*;
use crate::execution::{BudgetKind, RunEnd};

fn now() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn cand(source: &str, model: &str) -> CandidateRef {
    CandidateRef {
        source_id: source.into(),
        model_id: model.into(),
        harness_id: "claude_code".into(),
    }
}

fn proposal(run: &str) -> DecisionLogProposal {
    DecisionLogProposal {
        run_id: run.into(),
        decision_id: Some("d1".into()),
        campaign_id: Some("camp1".into()),
        task_id: Some("t1".into()),
        proposed_lane: Some("frontier".into()),
        decided_lane: Some("standard".into()),
        proposed_candidates: vec![cand("src-a", "model-a"), cand("src-b", "model-b")],
        proposed_first: Some(cand("src-a", "model-a")),
        policy_version: Some("p1".into()),
        ..DecisionLogProposal::default()
    }
}

#[test]
fn outcome_attributes_to_executed_candidate_not_first_recommendation() {
    let s = SqliteStore::open_in_memory().unwrap();
    s.decision_log_record_proposal(&proposal("r1"), now())
        .unwrap();
    s.decision_log_record_execution(
        &DecisionLogExecution {
            run_id: "r1".into(),
            exec_source_id: Some("src-b".into()),
            exec_model: Some("model-b".into()),
            fallback_reason: Some("rate_limit".into()),
            ..Default::default()
        },
        now(),
    )
    .unwrap();
    s.decision_log_record_outcome(
        &DecisionLogOutcome::from_run_end("r1", RunEnd::Completed, Some(10)),
        now(),
    )
    .unwrap();

    assert!(
        s.decision_log_runs_by_exec("src-a", "model-a")
            .unwrap()
            .is_empty()
    );
    let hit = s.decision_log_runs_by_exec("src-b", "model-b").unwrap();
    assert_eq!(hit.len(), 1);
    let run = &hit[0];
    assert_eq!(run.exec_equals_proposed_first(), Some(false));
    assert_eq!(run.execution.fallback_reason.as_deref(), Some("rate_limit"));
    assert_eq!(run.proposal.proposed_first, Some(cand("src-a", "model-a")));
    assert_eq!(run.proposal.proposed_lane.as_deref(), Some("frontier"));
    assert_eq!(run.proposal.decided_lane.as_deref(), Some("standard"));
    assert_eq!(run.outcome.run_end, Some(RunEndLabel::Completed));
}

#[test]
fn group_writes_do_not_overwrite_other_groups_and_labels_stay_separate() {
    let s = SqliteStore::open_in_memory().unwrap();
    // 成果が先に来ても、後から来る提案が成果を消さない。
    s.decision_log_record_outcome(
        &DecisionLogOutcome::from_run_end("r1", RunEnd::Completed, None),
        now(),
    )
    .unwrap();
    s.decision_log_record_proposal(&proposal("r1"), now())
        .unwrap();
    let run = s.decision_log_run_get("r1").unwrap().unwrap();
    assert_eq!(run.outcome.run_end, Some(RunEndLabel::Completed));
    // run が completed でも WU 検証・campaign 合否は未判定のまま（代用しない）。
    assert_eq!(run.outcome.wu_verdict, None);
    assert_eq!(run.outcome.campaign_verdict, None);
    s.decision_log_record_outcome(
        &DecisionLogOutcome {
            run_id: "r1".into(),
            run_end: Some(RunEndLabel::Completed),
            wu_verdict: Some(WuVerdict::Failed),
            campaign_verdict: Some(CampaignVerdict::Unscored),
            ..Default::default()
        },
        now(),
    )
    .unwrap();
    let run = s.decision_log_run_get("r1").unwrap().unwrap();
    assert_eq!(run.outcome.wu_verdict, Some(WuVerdict::Failed));
    assert_eq!(
        run.outcome.campaign_verdict,
        Some(CampaignVerdict::Unscored)
    );
    assert_eq!(run.proposal.decision_id.as_deref(), Some("d1"));
}

#[test]
fn failures_timeout_cancel_retry_are_kept_as_rows() {
    let s = SqliteStore::open_in_memory().unwrap();
    let ends = [
        (
            "r1",
            RunEnd::Failed { retryable: true },
            RunEndLabel::Failed,
        ),
        (
            "r2",
            RunEnd::BudgetExhausted {
                kind: BudgetKind::WallClock,
            },
            RunEndLabel::Timeout,
        ),
        ("r3", RunEnd::Cancelled, RunEndLabel::Cancelled),
        (
            "r4",
            RunEnd::BudgetExhausted {
                kind: BudgetKind::Turns,
            },
            RunEndLabel::BudgetExhausted,
        ),
    ];
    for (id, end, _) in ends {
        s.decision_log_record_outcome(&DecisionLogOutcome::from_run_end(id, end, None), now())
            .unwrap();
    }
    let mut retry = proposal("r5");
    retry.parent_run_id = Some("r1".into());
    retry.attempt_no = Some(1);
    s.decision_log_record_proposal(&retry, now()).unwrap();
    for (id, _, label) in ends {
        let run = s.decision_log_run_get(id).unwrap().unwrap();
        assert_eq!(run.outcome.run_end, Some(label), "{id}");
    }
    let r1 = s.decision_log_run_get("r1").unwrap().unwrap();
    assert_eq!(r1.outcome.run_retryable, Some(true));
    let r2 = s.decision_log_run_get("r2").unwrap().unwrap();
    assert!(r2.outcome.run_end_raw.unwrap().contains("wall_clock"));
    let r5 = s.decision_log_run_get("r5").unwrap().unwrap();
    assert!(r5.is_retry());
    assert_eq!(s.decision_log_runs_list(None).unwrap().len(), 5);
}

#[test]
fn reprocessing_does_not_add_rows_or_double_count() {
    let s = SqliteStore::open_in_memory().unwrap();
    for _ in 0..3 {
        s.decision_log_record_proposal(&proposal("r1"), now())
            .unwrap();
        s.decision_log_record_outcome(
            &DecisionLogOutcome::from_run_end("r1", RunEnd::Completed, Some(5)),
            now(),
        )
        .unwrap();
        let mut u = DecisionLogUsage::new(
            UsageKey::Run {
                run_id: "r1".into(),
                usage_source: "worker_finished".into(),
            },
            UsageRole::Worker,
        );
        u.campaign_id = Some("camp1".into());
        u.input_tokens = Some(100);
        u.output_tokens = Some(0);
        s.decision_log_record_usage(&u, now()).unwrap();
        let mut j = DecisionLogUsage::new(
            UsageKey::Call {
                decision_id: "d1".into(),
                call_attempt: 1,
            },
            UsageRole::Jev,
        );
        j.campaign_id = Some("camp1".into());
        j.cash_cost_usd = Some(0.002);
        s.decision_log_record_usage(&j, now()).unwrap();
        s.decision_log_record_intervention(&DecisionLogIntervention {
            intervention_id: "ev-42".into(),
            kind: InterventionKind::ResearchJudgment,
            task_id: Some("t1".into()),
            campaign_id: Some("camp1".into()),
            at: "2026-10-15T00:00:00Z".into(),
            wall_ms: None,
            source_event: Some("approval_decided".into()),
        })
        .unwrap();
    }
    assert_eq!(s.decision_log_runs_list(Some("camp1")).unwrap().len(), 1);
    let usage = s.decision_log_usage_list(Some("camp1")).unwrap();
    assert_eq!(usage.len(), 2);
    let worker = usage.iter().find(|u| u.role == UsageRole::Worker).unwrap();
    assert_eq!(worker.input_tokens, Some(100)); // 3 回再処理しても 300 にならない
    assert_eq!(worker.output_tokens, Some(0)); // 観測した 0
    assert_eq!(worker.cash_cost_usd, None); // 欠測は欠測のまま
    let jev = usage.iter().find(|u| u.role == UsageRole::Jev).unwrap();
    assert_eq!(
        jev.key,
        UsageKey::Call {
            decision_id: "d1".into(),
            call_attempt: 1
        }
    );
    assert_eq!(jev.cash_cost_usd, Some(0.002));
    assert_eq!(
        s.decision_log_interventions_list(Some("camp1"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn classifier_confidence_is_not_propensity() {
    let s = SqliteStore::open_in_memory().unwrap();
    let mut p = proposal("r1");
    p.routing_mode = Some("decision_assisted".into());
    p.classifier = Some(ClassifierRecord {
        kind: "jev".into(),
        model: Some("m".into()),
        snapshot: Some("m-2026-10-01".into()),
        confidence: Some(0.9),
        probs: Some(serde_json::json!({"a": 0.9, "b": 0.1})),
        fallback: None,
    });
    p.propensity = None;
    s.decision_log_record_proposal(&p, now()).unwrap();
    let got = s.decision_log_run_get("r1").unwrap().unwrap().proposal;
    assert_eq!(
        got.classifier.as_ref().and_then(|c| c.confidence),
        Some(0.9)
    );
    assert_eq!(got.propensity, None); // confidence で埋めない
    // 別々に持てる。
    p.run_id = "r2".into();
    p.propensity = Some(0.25);
    s.decision_log_record_proposal(&p, now()).unwrap();
    let got = s.decision_log_run_get("r2").unwrap().unwrap().proposal;
    assert_eq!(got.propensity, Some(0.25));
    assert_eq!(got.classifier.and_then(|c| c.confidence), Some(0.9));
    // 範囲外の confidence は拒否する。
    p.run_id = "r3".into();
    if let Some(c) = p.classifier.as_mut() {
        c.confidence = Some(1.5);
    }
    assert!(s.decision_log_record_proposal(&p, now()).is_err());
}

#[test]
fn old_routing_decided_event_without_new_fields_is_readable() {
    // 新欄（features・work_unit_id・escalation）を持たない旧 event。
    let old = serde_json::json!({
        "decision": {
            "lane": "standard", "proposed": "frontier", "source": "default",
            "rule_id": "standard/default", "policy_version": "lane-policy/1",
            "features": serde_json::json!({
                "judgment": "low", "ambiguity": "low", "verifiability": "high", "reversibility": "high",
                "consequence": "low", "context_size": "low", "tool_intensity": "low",
                "expected_length": "low", "cross_cutting": "low"
            })
        }
    });
    let record: Result<crate::model_policy::RoutingRecord, _> = serde_json::from_value(old);
    let record = record.expect("old event parses");
    let p = DecisionLogProposal::from_routing_record("r-old", "t-old", &record);
    assert_eq!(p.proposed_lane.as_deref(), Some("frontier"));
    assert_eq!(p.decided_lane.as_deref(), Some("standard"));
    assert_eq!(p.classifier, None);
    assert_eq!(p.propensity, None);
    let s = SqliteStore::open_in_memory().unwrap();
    s.decision_log_record_proposal(&p, now()).unwrap();
    let got = s.decision_log_run_get("r-old").unwrap().unwrap();
    assert_eq!(
        got.proposal.policy_version.as_deref(),
        Some("lane-policy/1")
    );
    assert_eq!(got.exec_equals_proposed_first(), None);
}

#[test]
fn wu_verdict_is_derived_not_assumed() {
    assert_eq!(WuVerdict::derive(None, None), WuVerdict::Unverified);
    assert_eq!(WuVerdict::derive(Some(true), None), WuVerdict::Passed);
    assert_eq!(
        WuVerdict::derive(Some(true), Some(false)),
        WuVerdict::Failed
    );
}
