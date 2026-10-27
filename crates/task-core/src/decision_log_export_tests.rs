//! 固定 fixture 行での決定的な試験（実 LLM・外部通信なし）。
use super::decision_log_export::*;
use crate::decision_log::*;
use crate::execution::{BudgetKind, RunEnd};

fn run(id: &str, campaign: &str, end: RunEnd) -> DecisionLogRun {
    DecisionLogRun {
        proposal: DecisionLogProposal {
            run_id: id.into(),
            decision_id: Some(format!("d-{id}")),
            campaign_id: Some(campaign.into()),
            task_id: Some(format!("t-{campaign}")),
            arm: Some("A".into()),
            proposed_first: Some(CandidateRef {
                source_id: "s1".into(),
                model_id: "m1".into(),
                harness_id: "h".into(),
            }),
            classifier: Some(ClassifierRecord {
                kind: "jev".into(),
                model: None,
                snapshot: None,
                confidence: Some(0.9),
                probs: None,
                fallback: None,
            }),
            ..Default::default()
        },
        execution: DecisionLogExecution {
            run_id: id.into(),
            exec_source_id: Some("s2".into()),
            exec_model: Some("m2".into()),
            ..Default::default()
        },
        outcome: DecisionLogOutcome::from_run_end(id, end, Some(5)),
    }
}

fn usage(id: &str, campaign: &str, role: UsageRole) -> DecisionLogUsage {
    let mut u = DecisionLogUsage::new(
        UsageKey::Run {
            run_id: id.into(),
            usage_source: "proxy".into(),
        },
        role,
    );
    u.run_id = Some(id.into());
    u.campaign_id = Some(campaign.into());
    u
}

fn params(c: &str) -> ExportParams {
    ExportParams {
        campaign_id: c.into(),
        manifest_hash: Some("mh".into()),
        export_cursor: Some("cur".into()),
    }
}

#[test]
fn other_campaign_rows_do_not_mix() {
    let mut a = usage("r1", "c1", UsageRole::Worker);
    a.input_tokens = Some(10);
    let mut b = usage("r2", "c2", UsageRole::Worker);
    b.input_tokens = Some(1000);
    let iv = |id: &str, c: &str| DecisionLogIntervention {
        intervention_id: id.into(),
        kind: InterventionKind::Recovery,
        task_id: None,
        campaign_id: Some(c.into()),
        at: "t".into(),
        wall_ms: None,
        source_event: None,
    };
    let e = build_campaign_export(
        &params("c1"),
        &[
            run("r1", "c1", RunEnd::Completed),
            run("r2", "c2", RunEnd::Completed),
        ],
        &[a, b],
        &[iv("i1", "c1"), iv("i2", "c2")],
    );
    assert_eq!(e.summary.n_runs, 1);
    assert_eq!(e.summary.total.input_tokens.sum, Some(10.0));
    assert_eq!(e.summary.interventions_by_kind["recovery"].count, 1);
    assert_eq!(e.summary.excluded_rows, 3);
    assert!(!e.records_jsonl().contains("r2"));
}

#[test]
fn reprocessing_does_not_double_count_and_output_is_byte_identical() {
    let mut u = usage("r1", "c1", UsageRole::Worker);
    u.input_tokens = Some(10);
    u.list_cost_usd = Some(0.5);
    let runs = [run("r1", "c1", RunEnd::Completed)];
    let once = build_campaign_export(&params("c1"), &runs, &[u.clone()], &[]);
    let twice = build_campaign_export(
        &params("c1"),
        &[runs[0].clone(), runs[0].clone()],
        &[u.clone(), u.clone()],
        &[],
    );
    assert_eq!(twice.summary.total.input_tokens.sum, Some(10.0));
    assert_eq!(twice.summary.total.n_rows, 1);
    assert_eq!(once.records_jsonl(), twice.records_jsonl());
    // 再処理の重複は数えて見せる（それ以外の summary は同一）。
    assert_eq!(twice.summary.integrity.duplicates_identical, 2);
    let mut normalized = twice.summary.clone();
    normalized.integrity = IntegrityStats::default();
    assert_eq!(once.summary, normalized);
}

#[test]
fn missing_is_null_estimated_is_flagged_and_categories_stay_separate() {
    let mut a = usage("r1", "c1", UsageRole::Worker);
    a.input_tokens = Some(0); // 0 と観測
    a.list_cost_usd = Some(1.0);
    let b = usage("r2", "c1", UsageRole::Worker); // 全欠測
    let e = build_campaign_export(
        &params("c1"),
        &[
            run("r1", "c1", RunEnd::Completed),
            run("r2", "c1", RunEnd::Completed),
        ],
        &[a, b],
        &[],
    );
    let t = &e.summary.total;
    assert_eq!(t.input_tokens.sum, Some(0.0));
    assert_eq!(
        (t.input_tokens.n_observed, t.input_tokens.n_missing),
        (1, 1)
    );
    assert_eq!(t.list_cost_usd.n_estimated, 1);
    // 定価換算があっても実支出は欠測のまま（代用しない）。
    assert_eq!(t.cash_cost_usd.sum, None);
    assert_eq!(t.cash_cost_usd.n_missing, 2);
    assert_eq!(t.cluster_wait_ms.sum, None);
    // 行の無い役割は 0 でなく null。
    assert_eq!(e.summary.by_role["jev"].input_tokens.sum, None);
    assert!(e.summary.incomplete.contains(&"cash_cost_usd".to_string()));
    assert!(e.summary.incomplete.contains(&"quota".to_string()));
    let line = e
        .records
        .iter()
        .find(|r| r["dedup_key"] == "run:r2:proxy")
        .unwrap();
    assert!(line["input_tokens"].is_null());
    assert_eq!(line["list_cost_usd_estimated"], true);
}

#[test]
fn failures_stay_in_denominator_and_labels_are_separate() {
    let e = build_campaign_export(
        &params("c1"),
        &[
            run("r1", "c1", RunEnd::Failed { retryable: true }),
            run(
                "r2",
                "c1",
                RunEnd::BudgetExhausted {
                    kind: BudgetKind::WallClock,
                },
            ),
            run("r3", "c1", RunEnd::Cancelled),
            run("r4", "c1", RunEnd::Completed),
        ],
        &[],
        &[],
    );
    let s = &e.summary;
    assert_eq!(s.n_runs, 4);
    assert_eq!(s.runs_by_end["failed"], 1);
    assert_eq!(s.runs_by_end["timeout"], 1);
    assert_eq!(s.runs_by_end["cancelled"], 1);
    // run が completed でも WU・campaign は未記録（別ラベル）。
    assert_eq!(s.wu_verdicts["unrecorded"], 4);
    assert_eq!(s.campaign_verdicts["unrecorded"], 4);
    // 実行は s2/m2、提案先頭は s1/m1 → 誤帰属しない。
    assert_eq!(s.exec_vs_proposed["exec_differs"], 4);
}

#[test]
fn retry_and_role_tables_are_separate_and_jev_is_distinct() {
    let mut w = usage("r1", "c1", UsageRole::Worker);
    w.input_tokens = Some(5);
    let mut r = usage("r2", "c1", UsageRole::Worker);
    r.is_retry = true;
    r.input_tokens = Some(7);
    let mut j = DecisionLogUsage::new(
        UsageKey::Call {
            decision_id: "d-r1".into(),
            call_attempt: 1,
        },
        UsageRole::Jev,
    );
    j.decision_id = Some("d-r1".into());
    j.cash_cost_usd = Some(0.01);
    let e = build_campaign_export(
        &params("c1"),
        &[
            run("r1", "c1", RunEnd::Completed),
            run("r2", "c1", RunEnd::Completed),
        ],
        &[w, r, j],
        &[],
    );
    let s = &e.summary;
    assert_eq!(s.by_role["worker"].input_tokens.sum, Some(12.0));
    assert_eq!(s.by_retry["retry"].input_tokens.sum, Some(7.0));
    assert_eq!(s.by_retry["initial"].input_tokens.sum, Some(5.0));
    assert_eq!(s.by_role["jev"].cash_cost_usd.sum, Some(0.01));
    assert_eq!(s.by_arm["A"].n_rows, 3);
    assert_eq!(s.arms, vec!["A".to_string()]);
}

#[test]
fn confidence_is_never_propensity_and_interventions_are_classified() {
    let e = build_campaign_export(
        &params("c1"),
        &[run("r1", "c1", RunEnd::Completed)],
        &[],
        &[],
    );
    let d = e
        .records
        .iter()
        .find(|r| r["record_type"] == "decision")
        .unwrap();
    assert_eq!(d["classifier_confidence"], 0.9);
    assert!(d["propensity"].is_null());
    assert!(d.get("success_probability").is_none());
    let iv = |id: &str, k| DecisionLogIntervention {
        intervention_id: id.into(),
        kind: k,
        task_id: Some("t-c1".into()), // campaign 未記載でも task で結ぶ
        campaign_id: None,
        at: "t".into(),
        wall_ms: Some(60_000),
        source_event: None,
    };
    let e = build_campaign_export(
        &params("c1"),
        &[run("r1", "c1", RunEnd::Completed)],
        &[],
        &[
            iv("a", InterventionKind::ResearchJudgment),
            iv("b", InterventionKind::ManualHandoff),
            iv("c", InterventionKind::Unclassified),
        ],
    );
    let k = &e.summary.interventions_by_kind;
    assert_eq!(k["research_judgment"].count, 1);
    assert_eq!(k["manual_handoff"].count, 1);
    assert_eq!(k["recovery"].count, 0);
    assert_eq!(k["unclassified"].count, 1);
    assert_eq!(k["research_judgment"].wall_ms.sum, Some(60_000.0));
}

#[test]
fn identity_and_raw_keys_survive_into_records_and_dto_roundtrip() {
    let mut r = run("r1", "c1", RunEnd::Completed);
    r.proposal.case_id = Some("case-1".into());
    r.proposal.repetition = Some(3);
    r.proposal.manifest_hash = Some("mh".into());
    let mut u = usage("r1", "c1", UsageRole::Worker);
    u.input_tokens = Some(5);
    // 正式 DTO は JSON を往復しても同じ（独自 fake JSON を挟まない）。
    let back: DecisionLogRun = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
    let e = build_campaign_export(&params("c1"), &[r], &[u], &[]);
    let rec = e
        .records
        .iter()
        .find(|v| v["record_type"] == "usage")
        .unwrap();
    assert_eq!(rec["run_id"], "r1");
    assert_eq!(rec["dedup_key"], "run:r1:proxy");
    assert_eq!(rec["usage_key"]["kind"], "run");
    assert_eq!(rec["usage_state"], "observed");
    assert_eq!(rec["run_known"], true);
    for (k, v) in [("case_id", "case-1"), ("manifest_hash", "mh"), ("arm", "A")] {
        assert_eq!(rec[k], v, "{k}");
    }
    assert_eq!(rec["repetition"], 3);
    assert_eq!(e.trials[0]["consumption_state"], "observed");
}

#[test]
fn consumption_states_distinguish_observed_missing_unmetered_not_executed_and_shared() {
    let mut obs = usage("r1", "c1", UsageRole::Worker);
    obs.input_tokens = Some(1);
    let miss = usage("r2", "c1", UsageRole::Worker);
    let mut not_run = run("r4", "c1", RunEnd::Completed);
    not_run.outcome.run_end = None;
    not_run.execution.exec_model = None;
    let mut shared = DecisionLogUsage::cos_turn("s", 1, Some("c1"));
    shared.input_tokens = Some(9);
    let e = build_campaign_export(
        &params("c1"),
        &[
            run("r1", "c1", RunEnd::Completed),
            run("r2", "c1", RunEnd::Completed),
            run("r3", "c1", RunEnd::Completed),
            not_run,
        ],
        &[obs, miss, shared],
        &[],
    );
    let state = |id: &str| {
        e.trials.iter().find(|t| t["run_id"] == id).unwrap()["consumption_state"].clone()
    };
    assert_eq!(state("r1"), "observed");
    assert_eq!(state("r2"), "missing");
    assert_eq!(state("r3"), "unmetered");
    assert_eq!(state("r4"), "not_executed");
    let states: Vec<_> = e
        .records
        .iter()
        .filter(|v| v["record_type"] == "usage")
        .map(|v| v["usage_state"].as_str().unwrap().to_string())
        .collect();
    assert!(states.contains(&"shared_unattributable".to_string()));
    assert!(states.contains(&"missing".to_string()));
    // 共有は直接合計に混ざらない。
    assert_eq!(e.summary.total.input_tokens.sum, Some(1.0));
}

#[test]
fn duplicates_match_or_conflict_and_foreign_hash_and_late_usage_are_counted() {
    let mut a = usage("r1", "c1", UsageRole::Worker);
    a.input_tokens = Some(10);
    let mut a_conflict = a.clone();
    a_conflict.input_tokens = Some(99);
    let mut r_bad = run("r2", "c1", RunEnd::Completed);
    r_bad.proposal.manifest_hash = Some("other".into());
    let mut r_ok = run("r1", "c1", RunEnd::Completed);
    r_ok.proposal.manifest_hash = Some("mh".into());
    let mut late = usage("r9", "c1", UsageRole::Worker);
    late.input_tokens = Some(7);
    let mut of_bad = usage("r2", "c1", UsageRole::Worker);
    of_bad.input_tokens = Some(1000);
    let e = build_campaign_export(
        &params("c1"),
        &[r_ok.clone(), r_ok, r_bad],
        &[a.clone(), a, a_conflict, late, of_bad],
        &[],
    );
    let i = &e.summary.integrity;
    assert_eq!(i.duplicates_identical, 2); // run と usage が各 1
    assert_eq!(i.conflicts, vec!["run:r1:proxy".to_string()]);
    assert_eq!(i.excluded_manifest_mismatch, 2); // run r2 とその消費行
    assert_eq!(i.usage_without_run, 1);
    assert_eq!(e.summary.n_runs, 1);
    // 先に来た行を残し、食い違った後続は採らない。遅れた観測は record に残るが試行には結合しない。
    assert_eq!(e.summary.total.input_tokens.sum, Some(17.0));
    assert_eq!(e.trials[0]["run_consumption"]["input_tokens"]["sum"], 10.0);
    assert!(
        verify_connected(&e)
            .iter()
            .any(|p| p.contains("conflicting"))
    );
}

#[test]
fn unassigned_shared_cos_is_exported_once_without_allocation_to_trials() {
    let mut shared = DecisionLogUsage::cos_turn("chat-run", 1, None);
    shared.thread_kind = Some("project".into());
    shared.project_id = Some("p1".into());
    shared.input_tokens = Some(100);
    let mut foreign = DecisionLogUsage::cos_turn("foreign", 1, Some("c2"));
    foreign.input_tokens = Some(1000);
    let direct = usage("r1", "c1", UsageRole::Worker);
    let export = build_campaign_export(
        &params("c1"),
        &[
            run("r1", "c1", RunEnd::Completed),
            run("r2", "c1", RunEnd::Completed),
        ],
        &[direct, shared.clone(), shared, foreign],
        &[],
    );
    assert_eq!(export.summary.shared_overhead.n_rows, 1);
    assert_eq!(
        export.summary.shared_overhead_by_kind["cos:project"]
            .input_tokens
            .sum,
        Some(100.0)
    );
    assert_eq!(export.summary.total.input_tokens.sum, None);
    assert_eq!(export.summary.integrity.duplicates_identical, 1);
    assert_eq!(export.trials.len(), 2);
    for trial in &export.trials {
        assert!(trial.get("run_consumption").unwrap()["input_tokens"]["sum"].is_null());
        assert!(trial["task_level_consumption"]["input_tokens"]["sum"].is_null());
    }
    let rows: Vec<_> = export
        .records
        .iter()
        .filter(|r| r["role"] == "cos")
        .collect();
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["campaign_id"].is_null());
}
