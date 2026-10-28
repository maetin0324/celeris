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
            actor_kind: InterventionActorKind::Human,
            actor_id: Some("alice".into()),
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
            by: None,
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

#[test]
fn cos_export_keeps_thread_context_missing_usage_and_separate_shared_subtotals() {
    let s = SqliteStore::open_in_memory().unwrap();
    {
        let conn = s.lock().unwrap();
        conn.execute_batch(
            r#"INSERT INTO chat_threads(id,kind,title,project_id,status,created_at,updated_at) VALUES
             ('general','general','CoS',NULL,'open','1970-01-01T00:00:00Z','1970-01-01T00:00:00Z'),
             ('project','project','Project CoS','p1','open','1970-01-01T00:00:00Z','1970-01-01T00:00:00Z');
             INSERT INTO node_sessions(id,node_id,kind,adapter,session_id,thread_id,model,created_at,last_used_at,retired_at) VALUES
             ('old-session','cos','cos_chat','claude-code','old-harness-session','project','session-model','1970-01-01T00:00:00Z','1970-01-01T00:00:00Z','1970-01-01T00:00:01Z'),
             ('new-session','cos','cos_chat','claude-code','new-harness-session','project','new-model','1970-01-01T00:00:01Z','1970-01-01T00:00:01Z',NULL);
             INSERT INTO chat_runs(run_id,thread_id,input_message_id,state,session_row_id,resolved_config_json,started_at,finished_at,usage_json) VALUES
             ('general-run','general','m1','completed',NULL,'{"model":"general-model"}','1970-01-01T00:00:00Z','1970-01-01T00:00:01Z','{"input_tokens":11,"output_tokens":0}'),
             ('project-run','project','m2','completed','old-session','{"model":"run-model"}','1970-01-01T00:00:00Z','1970-01-01T00:00:01Z','{"input_tokens":23}'),
             ('missing-run','project','m3','failed','old-session','{}','1970-01-01T00:00:00Z','1970-01-01T00:00:01Z',NULL),
             ('invalid-run','general','m4','interrupted',NULL,'{}',NULL,'1970-01-01T00:00:01Z','{"input_tokens":"invalid"}'),
             ('active-run','project','m5','running',NULL,'{}','1970-01-01T00:00:00Z',NULL,NULL);"#
        ).unwrap();
    }
    // An existing, old projected row also acquires context without a rewrite/migration.
    let mut old = DecisionLogUsage::cos_turn("project-run", 1, None);
    old.input_tokens = Some(23);
    s.decision_log_record_usage(&old, now()).unwrap();
    let later = now() + time::Duration::days(2);
    assert_eq!(s.decision_log_project_cos_turns(later).unwrap(), 3);
    assert_eq!(s.decision_log_project_cos_turns(later).unwrap(), 0);
    let usage = s.decision_log_usage_list(None).unwrap();
    assert_eq!(usage.len(), 4);
    let project = usage
        .iter()
        .find(|u| u.thread_id.as_deref() == Some("project") && u.input_tokens.is_some())
        .unwrap();
    assert_eq!(project.thread_kind.as_deref(), Some("project"));
    assert_eq!(project.project_id.as_deref(), Some("p1"));
    assert_eq!(project.model.as_deref(), Some("run-model"));
    assert_eq!(project.session.as_deref(), Some("old-harness-session"));

    let params = ExportParams {
        campaign_id: "camp1".into(),
        manifest_hash: None,
        export_cursor: None,
    };
    let export = s.decision_log_export_campaign(&params).unwrap();
    let missing = export
        .records
        .iter()
        .find(|r| r["dedup_key"] == "decision:cos:missing-run:1")
        .unwrap();
    for field in [
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_creation_tokens",
        "list_cost_usd",
        "cash_cost_usd",
    ] {
        assert!(missing.get(field).unwrap().is_null(), "{field}");
    }
    assert_eq!(missing["model"], "session-model");
    assert_eq!(missing["thread_id"], "project");
    assert_eq!(missing["thread_kind"], "project");
    assert_eq!(missing["project_id"], "p1");
    assert_eq!(missing["session"], "old-harness-session");
    assert!(missing["campaign_id"].is_null());
    let general = export
        .records
        .iter()
        .find(|r| r["thread_id"] == "general" && r["input_tokens"] == 11)
        .unwrap();
    assert_eq!(general["model"], "general-model");
    assert!(general["session"].is_null());
    assert!(general["project_id"].is_null());
    assert_eq!(general["output_tokens"], 0);
    let by_kind = &export.summary.shared_overhead_by_kind;
    assert_eq!(by_kind["cos:general"].input_tokens.sum, Some(11.0));
    assert_eq!(by_kind["cos:project"].input_tokens.sum, Some(23.0));
    assert_eq!(by_kind["cos:general"].input_tokens.n_missing, 1);
    assert_eq!(by_kind["cos:project"].input_tokens.n_missing, 1);
    assert_eq!(by_kind["cos:project"].output_tokens.sum, None);
    assert_eq!(export.summary.shared_overhead.n_rows, 4);
    assert_eq!(export.summary.total.n_rows, 0);
    assert!(export.trials.is_empty());
    assert!(export.summary.by_arm.is_empty());
    let again = s.decision_log_export_campaign(&params).unwrap();
    assert_eq!(export.records_jsonl(), again.records_jsonl());
    assert_eq!(export.summary_json(), again.summary_json());
}

fn decision_request(id: &str, raised_on: crate::model::TaskId) -> crate::decision::DecisionRequest {
    use crate::decision::*;
    let opt = |k: &str| DecisionOption {
        key: k.into(),
        label: k.to_uppercase(),
        consequence: None,
    };
    DecisionRequest {
        id: id.into(),
        key: format!("k-{id}"),
        kind: DecisionKind::Choice,
        question: "which?".into(),
        options: vec![opt("a"), opt("b")],
        recommended: "a".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["self".into()],
        path: Vec::new(),
        raised_by: DecisionRaisedBy {
            task_id: raised_on,
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

/// ADR 2026-10-27-decision-log-intervention-actors: 人・案件 CoS・統括 CoS・operator・MCP が 1 件ずつ答える event 列から、
/// 主体別の件数と decision の待ち区間が決定的に出る。CoS・operator・MCP は人の介入に入らず、A2 より前の event は unknown。
#[test]
fn interventions_split_by_actor_and_decision_waits_close_on_answer() {
    use crate::model::{Event, Status};
    let s = SqliteStore::open_in_memory().unwrap();
    {
        let conn = s.lock().unwrap();
        conn.execute_batch(
            "INSERT INTO chat_threads(id,kind,title,project_id,status,created_at,updated_at) VALUES
             ('tg','general','CoS',NULL,'open','1970-01-01T00:00:00Z','1970-01-01T00:00:00Z'),
             ('tp','project','Project CoS','p1','open','1970-01-01T00:00:00Z','1970-01-01T00:00:00Z'),
             ('tl','legacy','old',NULL,'open','1970-01-01T00:00:00Z','1970-01-01T00:00:00Z');",
        )
        .unwrap();
    }
    let root = super::tests::sample_task(Status::Draft);
    s.create_task(&root, vec![]).unwrap();
    let mut child = super::tests::sample_task(Status::Draft);
    child.parent_id = Some(root.id);
    s.create_task(&child, vec![]).unwrap();
    let id = CampaignIdentity {
        campaign_id: "camp-a".into(),
        case_id: None,
        arm: None,
        repetition: None,
        manifest_hash: None,
        routing_setting: None,
        variant: None,
        variant_org: None,
    };
    s.task_campaign_set(&root.id.to_string(), &id, now())
        .unwrap();
    let answer = |id: &str, by: &str| Event::DecisionAnswered {
        id: id.into(),
        option: "a".into(),
        note: None,
        by: by.into(),
    };
    let requested = |id: &str, on| Event::DecisionRequested {
        decision: Box::new(decision_request(id, on)),
    };
    let root_events = [
        // 人: 質問への回答。
        Event::QuestionRaised {
            run_id: "run-1".into(),
            text: "which?".into(),
        },
        Event::Answered {
            question: "which?".into(),
            answer: "a".into(),
            by: Some("human:alice".into()),
        },
        // 案件 CoS: decision への回答。
        requested("D1", root.id),
        answer("D1", "cos:tp"),
        // operator: 統合依頼への回答。
        Event::IntegrationAnswered {
            request_id: "ir-1".into(),
            answer: "integrated".into(),
            note: None,
            by: Some("operator:claude-code".into()),
        },
        // MCP: 計画の承認。
        Event::PlanGateDecided {
            plan_id: "plan-1".into(),
            action: "approve".into(),
            by: "mcp:chatgpt".into(),
            note: None,
        },
        // A2 より前（主体なし）と、丸めた `human`・legacy thread の CoS は unknown。
        Event::Answered {
            question: "old".into(),
            answer: "b".into(),
            by: None,
        },
        Event::ApprovalDecided {
            by: "human".into(),
            approved: true,
            note: None,
        },
        Event::Edited {
            fields: vec!["title".into()],
            by: "cos:tl".into(),
        },
        // system: daemon が置き換えで閉じた統合依頼。
        Event::IntegrationAnswered {
            request_id: "ir-0".into(),
            answer: "superseded".into(),
            note: None,
            by: Some(crate::DAEMON_ACTOR.into()),
        },
        // 開いたままの decision。
        requested("D3", root.id),
    ];
    // 統括 CoS: child が出した decision に root の列で答える（decision id で対にする）。
    s.append_event(child.id, &requested("D2", child.id))
        .unwrap();
    for e in &root_events {
        s.append_event(root.id, e).unwrap();
    }
    s.append_event(root.id, &answer("D2", "cos:tg")).unwrap();
    // revise は介入だが、待ちを閉じ直さない。
    s.append_event(root.id, &answer("D1", "human:alice"))
        .unwrap();

    for _ in 0..2 {
        // 再投影しても行は増えない。
        s.decision_log_project_interventions(&root.id.to_string())
            .unwrap();
        s.decision_log_project_interventions(&child.id.to_string())
            .unwrap();
    }
    let ivs = s.decision_log_interventions_list(Some("camp-a")).unwrap();
    let mut by_actor = std::collections::BTreeMap::<&str, Vec<&str>>::new();
    for i in &ivs {
        by_actor
            .entry(i.actor_kind.as_str())
            .or_default()
            .push(i.actor_id.as_deref().unwrap_or("-"));
    }
    for v in by_actor.values_mut() {
        v.sort();
    }
    let expect: std::collections::BTreeMap<&str, Vec<&str>> = [
        ("human", vec!["alice", "alice"]),
        ("cos_project", vec!["tp"]),
        ("cos_general", vec!["tg"]),
        ("external_operator", vec!["claude-code"]),
        ("mcp_client", vec!["chatgpt"]),
        ("system", vec!["daemon"]),
        ("unknown", vec!["-", "cos:tl", "human"]),
    ]
    .into_iter()
    .collect();
    assert_eq!(by_actor, expect);
    let kind_of = |actor: InterventionActorKind| {
        ivs.iter()
            .filter(|i| i.actor_kind == actor)
            .map(|i| i.kind)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        kind_of(InterventionActorKind::McpClient),
        vec![InterventionKind::ResearchJudgment]
    );
    assert_eq!(
        kind_of(InterventionActorKind::ExternalOperator),
        vec![InterventionKind::Recovery]
    );

    // export: 人の介入は human の 2 件だけ、主体別は 7 種。
    let params = ExportParams {
        campaign_id: "camp-a".into(),
        manifest_hash: None,
        export_cursor: None,
    };
    let e = s.decision_log_export_campaign(&params).unwrap();
    let human: u64 = e
        .summary
        .human_interventions_by_kind
        .values()
        .map(|x| x.count)
        .sum();
    assert_eq!(human, 2);
    assert_eq!(
        e.summary.human_interventions_by_kind["research_judgment"].count,
        2
    );
    let counts: Vec<(&str, u64)> = e
        .summary
        .interventions_by_actor_kind
        .iter()
        .map(|(k, n)| (k.as_str(), *n))
        .collect();
    assert_eq!(
        counts,
        vec![
            ("cos_general", 1),
            ("cos_project", 1),
            ("external_operator", 1),
            ("human", 2),
            ("mcp_client", 1),
            ("system", 1),
            ("unknown", 3),
        ]
    );

    // 待ち区間: question 1・decision 3（D1 は CoS が閉じ、D2 は別 task の回答で閉じ、D3 は open）。
    let w = s.decision_log_human_waits("camp-a").unwrap();
    let summary: Vec<(String, bool, serde_json::Value, serde_json::Value)> = w
        .iter()
        .map(|x| {
            (
                x["wait_kind"].as_str().unwrap().to_string(),
                x["ended_at"].is_string(),
                x["end_actor_kind"].clone(),
                x["end_actor_id"].clone(),
            )
        })
        .collect();
    let mut expect = vec![
        ("question".to_string(), true, "human".into(), "alice".into()),
        (
            "decision".to_string(),
            true,
            "cos_project".into(),
            "tp".into(),
        ),
        (
            "decision".to_string(),
            false,
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
        (
            "decision".to_string(),
            true,
            "cos_general".into(),
            "tg".into(),
        ),
    ];
    // 出力は (task_id, 開始 seq) の順。child と root の並びは task id の大小で決まる。
    if child.id.to_string() < root.id.to_string() {
        let d2 = expect.remove(3);
        expect.insert(0, d2);
    }
    assert_eq!(summary, expect);
    let d2 = w
        .iter()
        .find(|x| x["task_id"] == child.id.to_string())
        .unwrap();
    assert!(
        d2["end_event"]
            .as_str()
            .unwrap()
            .starts_with(&format!("event:{}:", root.id))
    );
    assert_eq!(s.decision_log_human_waits("camp-a").unwrap(), w);
}

#[test]
fn actor_classification_keeps_unknown_for_rounded_or_missing_by() {
    use crate::chat::ChatThreadKind;
    let k = |by: Option<&str>| {
        InterventionActorKind::classify(by, |t| match t {
            "g" => Some(ChatThreadKind::General),
            "p" => Some(ChatThreadKind::Project),
            _ => None,
        })
    };
    let cases = [
        (None, InterventionActorKind::Unknown, None),
        (Some("human"), InterventionActorKind::Unknown, Some("human")),
        (
            Some("human:"),
            InterventionActorKind::Unknown,
            Some("human:"),
        ),
        (Some("cos"), InterventionActorKind::Unknown, Some("cos")),
        (
            Some("cos:zz"),
            InterventionActorKind::Unknown,
            Some("cos:zz"),
        ),
        (Some("human:bob"), InterventionActorKind::Human, Some("bob")),
        (Some("cos:g"), InterventionActorKind::CosGeneral, Some("g")),
        (
            Some("cos_project:x"),
            InterventionActorKind::CosProject,
            Some("x"),
        ),
        (
            Some("operator:op"),
            InterventionActorKind::ExternalOperator,
            Some("op"),
        ),
        (Some("mcp:c1"), InterventionActorKind::McpClient, Some("c1")),
        (
            Some("daemon"),
            InterventionActorKind::System,
            Some("daemon"),
        ),
        (
            Some("worker:r1"),
            InterventionActorKind::System,
            Some("worker:r1"),
        ),
        (
            Some("project-plan"),
            InterventionActorKind::System,
            Some("project-plan"),
        ),
    ];
    for (by, kind, id) in cases {
        assert_eq!(k(by), (kind, id.map(str::to_string)), "{by:?}");
    }
}

#[test]
fn integration_dispositions_project_recovery_with_each_actor() {
    let s = SqliteStore::open_in_memory().unwrap();
    let task = super::tests::sample_task(Status::Draft);
    s.create_task(&task, vec![]).unwrap();
    s.task_campaign_set(
        &task.id.to_string(),
        &CampaignIdentity {
            campaign_id: "integration-actors".into(),
            case_id: None,
            arm: None,
            repetition: None,
            manifest_hash: None,
            routing_setting: None,
            variant: None,
            variant_org: None,
        },
        now(),
    )
    .unwrap();
    for (actor, kind, id) in [
        ("human:web", InterventionActorKind::Human, "web"),
        (
            "operator:claude-code",
            InterventionActorKind::ExternalOperator,
            "claude-code",
        ),
        ("mcp:client", InterventionActorKind::McpClient, "client"),
        (
            "cos_general:thread",
            InterventionActorKind::CosGeneral,
            "thread",
        ),
        ("human:local", InterventionActorKind::Human, "local"),
    ] {
        let repo_id = crate::RepoId::new();
        for event in [
            Event::IntegrateTaskRequested {
                request_id: "request".into(),
                integrate_task_id: task.id,
                by: actor.into(),
            },
            Event::IntegrationMerged {
                repo_id,
                branch: "b".into(),
                head_sha: "h".into(),
                actor: actor.into(),
            },
            Event::IntegrationDiscarded {
                repo_id,
                branch: "b".into(),
                head_sha: "h".into(),
                reason: "r".into(),
                actor: actor.into(),
            },
            Event::IntegrationHeld {
                repo_id,
                branch: "b".into(),
                head_sha: "h".into(),
                reason: "r".into(),
                actor: actor.into(),
            },
            Event::IntegrationReopened {
                repo_id,
                branch: "b".into(),
                head_sha: "h".into(),
                reason: None,
                actor: actor.into(),
            },
        ] {
            s.append_event(task.id, &event).unwrap();
        }
        for _ in 0..2 {
            s.decision_log_project_interventions(&task.id.to_string())
                .unwrap();
        }
        let rows = s
            .decision_log_interventions_list(Some("integration-actors"))
            .unwrap();
        let found: Vec<_> = rows
            .iter()
            .filter(|r| r.actor_id.as_deref() == Some(id))
            .collect();
        assert_eq!(found.len(), 5);
        assert!(
            found
                .iter()
                .all(|r| r.kind == InterventionKind::Recovery && r.actor_kind == kind)
        );
    }
}
