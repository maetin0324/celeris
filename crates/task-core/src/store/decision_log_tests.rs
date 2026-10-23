//! ADR 2026-10-15-decision-log-export の記録: 誤帰属なし・再処理で二重計上なし・別ラベル・
//! classifier_confidence と propensity の分離・旧 event の読み取り。
use time::OffsetDateTime;

use super::*;
use crate::decision_log::*;
use crate::decision_log_export::{ExportParams, build_campaign_export};
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

fn assist_record(
    status: crate::route_select::ClassifierStatus,
    jev: Option<&str>,
    model: Option<&str>,
    call_id: Option<&str>,
) -> crate::route_select::LaneAssistRecord {
    crate::route_select::LaneAssistRecord {
        mode: "decision_assisted".into(),
        classifier_status: status,
        real_api_verified: false,
        base_lane: "standard".into(),
        proposed_lane: jev.map(str::to_owned),
        fixed_lane: Some("standard".into()),
        jev_lane: jev.map(str::to_owned),
        jev_specificity: None,
        jev_confidence: None,
        jev_confidence_missing: false,
        response_model: model.map(str::to_owned),
        observed_snapshot: None,
        call_id: call_id.map(str::to_owned),
        final_lane: "standard".into(),
        basis: "classification_raise".into(),
        overridden_by: Some("ceiling".into()),
    }
}

#[test]
fn lane_stages_status_version_and_call_id_survive_store_reload_and_export() {
    use crate::route_select::ClassifierStatus;
    let s = SqliteStore::open_in_memory().unwrap();
    let call = DecisionLogUsage::jev_call_id("wu1", 2);
    assert_eq!(call, "decision:jev:wu1:2");
    // 同じ分類（同じ call_id）を 2 つの run（attempt_no は別）が使う。
    let mut runs = vec![];
    for (run, attempt) in [("r1", 0u32), ("r2", 1)] {
        let mut p = proposal(run);
        p.attempt_no = Some(attempt);
        p.apply_lane_assist(&assist_record(
            ClassifierStatus::JevOk,
            Some("frontier"),
            Some("jev-resp-model"),
            Some(&call),
        ));
        s.decision_log_record_proposal(&p, now()).unwrap();
        runs.push(s.decision_log_run_get(run).unwrap().unwrap());
        assert_eq!(runs[runs.len() - 1].proposal, p);
    }
    let p = &runs[0].proposal;
    assert_eq!(p.classifier_status.as_deref(), Some("jev_ok"));
    assert_eq!(p.classifier_call_id.as_deref(), Some("decision:jev:wu1:2"));
    assert_eq!(p.fixed_lane.as_deref(), Some("standard"));
    assert_eq!(p.jev_lane.as_deref(), Some("frontier"));
    assert_eq!(p.constrained_lane.as_deref(), Some("standard"));
    assert_eq!(p.lane_basis.as_deref(), Some("classification_raise"));
    let c = p.classifier.as_ref().unwrap();
    assert_eq!(c.kind, "jev");
    assert_eq!(c.model.as_deref(), Some("jev-resp-model"));
    assert_eq!(c.snapshot, None); // 応答 model から snapshot を作らない
    assert_eq!(c.confidence, None); // 記録に無い確信を作らない

    // 消費行は呼び出し単位で 1 行（2 run が参照しても 1 回しか数えない）。call_id は消費行の鍵と同一。
    let mut u = DecisionLogUsage::jev_call("wu1", 2, "t1", Some("camp1"), Some(0.5));
    assert_eq!(u.key.dedup_key(), call);
    u.list_cost_usd = Some(0.5);
    let params = ExportParams {
        campaign_id: "camp1".into(),
        manifest_hash: None,
        export_cursor: None,
    };
    let e = build_campaign_export(&params, &runs, &[u.clone(), u], &[]);
    assert_eq!(e.trials.len(), 2);
    for t in &e.trials {
        assert_eq!(t["classifier_status"], "jev_ok");
        assert_eq!(t["classifier_call_id"], "decision:jev:wu1:2");
        assert_eq!(t["classifier_model"], "jev-resp-model");
        assert_eq!(t["classifier_snapshot"], serde_json::Value::Null);
        assert_eq!(t["fixed_lane"], "standard");
        assert_eq!(t["jev_lane"], "frontier");
        assert_eq!(t["constrained_lane"], "standard");
    }
    assert_eq!(e.trials[0]["attempt_no"], 0);
    assert_eq!(e.trials[1]["attempt_no"], 1);
    let jev_cost: f64 = e
        .trials
        .iter()
        .map(|t| {
            t["task_level_consumption"]["list_cost_usd"]["sum"]
                .as_f64()
                .unwrap_or(0.0)
        })
        .sum();
    assert_eq!(jev_cost, 0.5); // 同じ使用行を重複させても 2 run に割って二重に出ない
    // 決定行（decision_id 単位）も同じ欄を持つ。
    let d = e
        .records
        .iter()
        .find(|r| r["record_type"] == "decision")
        .unwrap();
    assert_eq!(d["classifier_call_id"], "decision:jev:wu1:2");
}

#[test]
fn missing_stage_values_stay_null_and_not_attempted_makes_no_classifier() {
    use crate::route_select::ClassifierStatus;
    let s = SqliteStore::open_in_memory().unwrap();
    let mut p = proposal("r1");
    p.apply_lane_assist(&assist_record(
        ClassifierStatus::NotAttempted,
        None,
        None,
        None,
    ));
    s.decision_log_record_proposal(&p, now()).unwrap();
    // 旧い形の run（lane 層の記録なし）。
    s.decision_log_record_proposal(&proposal("r-old"), now())
        .unwrap();
    let mut f = proposal("r-fb");
    f.apply_lane_assist(&assist_record(
        ClassifierStatus::FellBackToFixed {
            reason: "timeout".into(),
        },
        None,
        None,
        None,
    ));
    s.decision_log_record_proposal(&f, now()).unwrap();
    let runs: Vec<_> = ["r1", "r-old", "r-fb"]
        .iter()
        .map(|r| s.decision_log_run_get(r).unwrap().unwrap())
        .collect();
    assert_eq!(runs[0].proposal.classifier, None);
    assert_eq!(
        runs[0].proposal.classifier_status.as_deref(),
        Some("not_attempted")
    );
    assert_eq!(runs[1].proposal.classifier_status, None);
    assert_eq!(runs[1].proposal.fixed_lane, None);
    let fb = runs[2].proposal.classifier.as_ref().unwrap();
    assert_eq!(fb.fallback.as_deref(), Some("timeout"));
    assert_eq!(fb.model, None); // 設定 model で補わない
    let params = ExportParams {
        campaign_id: "camp1".into(),
        manifest_hash: None,
        export_cursor: None,
    };
    let e = build_campaign_export(&params, &runs, &[], &[]);
    let by = |id: &str| e.trials.iter().find(|t| t["run_id"] == id).unwrap();
    for k in [
        "classifier_status",
        "classifier_call_id",
        "fixed_lane",
        "jev_lane",
        "constrained_lane",
        "lane_basis",
        "classifier_model",
        "classifier_snapshot",
    ] {
        // 欠測は欄ごと落とさず null で出す（旧 run は全て null）。
        assert!(by("r-old").get(k).is_some_and(|v| v.is_null()), "{k}");
    }
    assert_eq!(by("r-fb")["classifier_status"], "fell_back_to_fixed");
    assert_eq!(by("r-fb")["classifier_fallback"], "timeout");
    assert!(by("r1")["classifier_kind"].is_null());
}

#[test]
fn human_waits_pair_question_and_approval_events_of_the_campaign_tree() {
    use crate::model::{Event, Status};
    let s = SqliteStore::open_in_memory().unwrap();
    let root = super::tests::sample_task(Status::Draft);
    s.create_task(&root, vec![]).unwrap();
    let mut child = super::tests::sample_task(Status::Draft);
    child.parent_id = Some(root.id);
    s.create_task(&child, vec![]).unwrap();
    let other = super::tests::sample_task(Status::Draft);
    s.create_task(&other, vec![]).unwrap();
    let id = CampaignIdentity {
        campaign_id: "camp-w".into(),
        case_id: Some("case-1".into()),
        arm: Some("B".into()),
        repetition: Some(1),
        manifest_hash: None,
        routing_setting: None,
        variant: None,
        variant_org: None,
    };
    s.task_campaign_set(&root.id.to_string(), &id, now())
        .unwrap();
    let q = Event::QuestionRaised {
        run_id: "run-1".into(),
        text: "which?".into(),
    };
    s.append_event(root.id, &q).unwrap();
    s.append_event(
        root.id,
        &Event::Answered {
            question: "which?".into(),
            answer: "a".into(),
        },
    )
    .unwrap();
    s.append_event(child.id, &Event::ApprovalRequested).unwrap(); // 決定なし → open
    s.append_event(other.id, &q).unwrap(); // 別 campaign の task は出さない
    let w = s.decision_log_human_waits("camp-w").unwrap();
    assert_eq!(w.len(), 2, "{w:?}");
    let q = w.iter().find(|x| x["wait_kind"] == "question").unwrap();
    assert_eq!(q["run_id"], "run-1");
    assert_eq!(q["task_id"], root.id.to_string());
    assert!(q["ended_at"].is_string() && q["end_event"].is_string());
    let a = w.iter().find(|x| x["wait_kind"] == "approval").unwrap();
    assert_eq!(a["task_id"], child.id.to_string());
    assert!(a["ended_at"].is_null(), "open は null（0 秒にしない）");
}
