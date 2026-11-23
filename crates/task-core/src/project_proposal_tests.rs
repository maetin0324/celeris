use super::*;

const REQUEST: &str = "長期の依頼\n```celeris-outcomes\n[{\"id\":\"o1\",\"title\":\"中間\",\"acceptance\":[\"A\"]},\
{\"id\":\"o2\",\"title\":\"最終\",\"acceptance\":[\"B\",\"C\"],\"depends_on\":[\"o1\"]}]\n```\n";

fn snapshot() -> ProjectSnapshot {
    ProjectSnapshot {
        now: "2026-11-04T00:00:00Z".into(),
        truncated: false,
        project: SnapshotProject {
            id: "P".into(),
            title: "p".into(),
            status: "active".into(),
            request: REQUEST.into(),
            request_sha256: sha256_hex(REQUEST),
        },
        brief: None,
        outcomes: parse_outcomes(REQUEST).unwrap(),
        outcomes_invalid: false,
        roots: vec![],
        waits: vec![],
        stagnation: vec![],
        wakes: vec![],
        decisions: vec![],
        controller: SnapshotController {
            mode: "enabled".into(),
            phase: "active".into(),
            revision: 3,
            human_epoch: 1,
            retry_count: 0,
            progress_epoch: 0,
        },
        limits: SnapshotLimits::default(),
        roadmap: None,
        authority: None,
        checkpoints: Vec::new(),
    }
}

fn launch(snapshot: &ProjectSnapshot, acceptance: &[&str]) -> ProjectProposal {
    ProjectProposal {
        action: ProjectAction::LaunchRoot {
            root: RootSpec {
                key: "outcome:o1:1".into(),
                title: "o1".into(),
                objective: "do o1".into(),
                acceptance: acceptance
                    .iter()
                    .map(|t| RootCriterion {
                        text: (*t).into(),
                        check: RootCheck::Reviewer,
                    })
                    .collect(),
                stages_hint: vec![],
                continues_root_ids: vec![],
                outcome_ids: vec!["o1".into()],
            },
        },
        gap: ProposalGap {
            reached: vec![],
            open: vec!["o1".into(), "o2".into()],
            unknown: vec![],
            progress_epoch: 0,
        },
        explanation: ProposalExplanation {
            rule_id: "pc/outcome-open".into(),
            signals: vec![],
        },
        based_on: ProposalBasedOn {
            revision: snapshot.controller.revision,
            human_epoch: snapshot.controller.human_epoch,
            request_sha256: snapshot.project.request_sha256.clone(),
        },
    }
}

fn codes(r: Result<(), Vec<ProposalViolation>>) -> Vec<String> {
    r.err()
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.code)
        .collect()
}

#[test]
fn project_proposal_parses_outcome_block() {
    let o = parse_outcomes(REQUEST).unwrap();
    assert_eq!(o.len(), 2);
    assert_eq!(o[1].depends_on, ["o1"]);
    assert_eq!(o[0].source, "request_block");
    assert!(parse_outcomes("no block").unwrap().is_empty());
}

#[test]
fn project_proposal_rejects_bad_outcome_blocks() {
    for bad in [
        "```celeris-outcomes\n[{\"id\":\"o1\",\"title\":\"t\",\"acceptance\":[\"a\"]},{\"id\":\"o1\",\"title\":\"t\",\"acceptance\":[\"a\"]}]\n```",
        "```celeris-outcomes\n[{\"id\":\"a\",\"title\":\"t\",\"acceptance\":[\"x\"],\"depends_on\":[\"b\"]},{\"id\":\"b\",\"title\":\"t\",\"acceptance\":[\"x\"],\"depends_on\":[\"a\"]}]\n```",
        "```celeris-outcomes\n[{\"id\":\"a\",\"title\":\"t\",\"acceptance\":[]}]\n```",
        "```celeris-outcomes\n[{\"id\":\"A!\",\"title\":\"t\",\"acceptance\":[\"x\"]}]\n```",
        "```celeris-outcomes\n[{\"id\":\"a\",\"title\":\"t\",\"acceptance\":[\"x\"],\"extra\":1}]\n```",
        "```celeris-outcomes\n[]",
    ] {
        assert!(parse_outcomes(bad).is_err(), "{bad}");
    }
}

#[test]
fn project_proposal_accepts_a_full_launch_and_rejects_narrowing() {
    let s = snapshot();
    assert_eq!(validate(&s, &launch(&s, &["A"])), Ok(()));
    assert_eq!(validate(&s, &launch(&s, &["A", "extra"])), Ok(()));
    let c = codes(validate(&s, &launch(&s, &["A paraphrased"])));
    assert_eq!(c, ["root_acceptance_narrowed"]);
    assert!(SAFETY_VIOLATIONS.contains(&c[0].as_str()));
}

#[test]
fn project_proposal_rejects_stale_and_unowned_and_paused() {
    let mut s = snapshot();
    let mut p = launch(&s, &["A"]);
    p.based_on.human_epoch = 0;
    assert_eq!(codes(validate(&s, &p)), ["based_on_mismatch"]);
    let mut p = launch(&s, &["A"]);
    if let ProjectAction::LaunchRoot { root } = &mut p.action {
        root.key = "outcome:zz:1".into();
        root.outcome_ids = vec!["zz".into()];
    }
    assert_eq!(codes(validate(&s, &p)), ["root_key_not_owned"]);
    s.project.status = "paused".into();
    let p = launch(&s, &["A"]);
    assert_eq!(codes(validate(&s, &p)), ["launch_on_paused"]);
}

#[test]
fn project_proposal_round_trips_json_and_denies_unknown_fields() {
    let s = snapshot();
    let p = launch(&s, &["A"]);
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["action"]["kind"], "launch_root");
    assert_eq!(parse_and_validate(&s, &v).unwrap(), p);
    let mut extra = v.clone();
    extra["action"]["write_db"] = serde_json::json!(true);
    let err = parse_and_validate(&s, &extra).unwrap_err();
    assert_eq!(err[0].code, "invalid_output");
    assert_eq!(parse_root_key("outcome:o1:01"), None);
    assert_eq!(parse_root_key("outcome:o1:2"), Some(("o1", 2)));
    assert_eq!(
        root_idempotency_key("P", "outcome:o1:1"),
        "pc:P:outcome:o1:1"
    );
}

/// ADR 2026-11-05-phase3-p0e: Outcome の `checks` は block から読み、TS に渡す JSON には出さない。
#[test]
fn outcome_block_checks_are_parsed_and_hidden_from_the_snapshot_json() {
    let req = "x\n```celeris-outcomes\n[{\"id\":\"o1\",\"title\":\"t\",\"acceptance\":[\"A\"],\
\"checks\":[{\"cmd\":\"test -s out/o1.txt\"},{\"cmd\":\"exit 3\",\"expect_exit\":3}]}]\n```\n";
    let o = parse_outcomes(req).unwrap();
    assert_eq!(
        o[0].checks,
        vec![
            OutcomeCheck {
                cmd: "test -s out/o1.txt".into(),
                expect_exit: 0,
            },
            OutcomeCheck {
                cmd: "exit 3".into(),
                expect_exit: 3,
            },
        ]
    );
    let json = serde_json::to_value(&o[0]).unwrap();
    assert!(json.get("checks").is_none(), "{json}");
    assert!(parse_outcomes(REQUEST).unwrap()[0].checks.is_empty());
}

#[test]
fn outcome_block_rejects_bad_checks() {
    let block = |checks: &str| {
        format!(
            "```celeris-outcomes\n[{{\"id\":\"o1\",\"title\":\"t\",\"acceptance\":[\"A\"],\"checks\":{checks}}}]\n```\n"
        )
    };
    assert!(parse_outcomes(&block(r#"[{"cmd":"  "}]"#)).is_err());
    assert!(parse_outcomes(&block(r#"[{"cmd":"true","source":"x"}]"#)).is_err());
    let long = format!(
        r#"[{{"cmd":"{}"}}]"#,
        "x".repeat(OUTCOME_CHECK_CMD_MAX_BYTES + 1)
    );
    assert!(parse_outcomes(&block(&long)).is_err());
    let many = format!(
        "[{}]",
        [r#"{"cmd":"true"}"#; OUTCOME_CHECKS_MAX + 1].join(",")
    );
    assert!(parse_outcomes(&block(&many)).is_err());
}

// ---------------------------------------------------------------- checkpoint（ADR 2026-11-22-parent-checkpoint-evaluator D2.2・D5.2）
// base の validate は `evaluate` を知らず、写していない評価を無視する行動も通した。ここで守る不変条件は、strategy root の
// due の trigger だけを 1 root 1 回で評価に出すことと、写していない評価を表の行動でしか処理できないこと。

fn checkpoint(root: &str, due: &[&str], running: Option<&str>) -> SnapshotCheckpoint {
    SnapshotCheckpoint {
        root_id: root.into(),
        strategy_ref: "artifact:S/strategy/s1/v1.json".into(),
        due: due
            .iter()
            .map(|k| SnapshotTrigger {
                key: (*k).into(),
                kind: "children".into(),
                at: None,
            })
            .collect(),
        running: running.map(str::to_owned),
        unmapped: vec![],
    }
}

fn unmapped(id: &str, allowed: Vec<SnapshotAllowedAction>) -> SnapshotEvaluation {
    SnapshotEvaluation {
        decision_id: id.into(),
        status: "checked".into(),
        effective: serde_json::json!({"verdict": "off_track", "next": "pivot"}),
        record_ref: format!("artifact:R/parent-evaluation/{id}.json"),
        allowed,
    }
}

fn act(
    snapshot: &ProjectSnapshot,
    action: ProjectAction,
    rule: &str,
    signal: Option<&str>,
) -> ProjectProposal {
    let mut p = launch(snapshot, &["A"]);
    p.action = action;
    p.explanation.rule_id = rule.into();
    p.explanation.signals = signal
        .map(|d| ProposalSignal {
            name: CHECKPOINT_SIGNAL.into(),
            detail: d.into(),
        })
        .into_iter()
        .collect();
    p
}

fn evaluate(root: &str, triggers: &[&str]) -> ProjectAction {
    ProjectAction::Evaluate {
        root_id: root.into(),
        triggers: triggers.iter().map(|t| (*t).to_string()).collect(),
    }
}

/// 不変条件（D2.2）: `evaluate` は snapshot の strategy root の due の trigger（1〜8 件、重複なし）だけを、実行中の判断の
/// 無い root に、active の案件でだけ求められる。
#[test]
fn project_controller_checkpoint_evaluate_validation() {
    let mut s = snapshot();
    s.checkpoints = vec![checkpoint("R", &["children:2", "budget:50"], None)];
    let ok = act(
        &s,
        evaluate("R", &["children:2", "budget:50"]),
        "pc/checkpoint/evaluate",
        None,
    );
    assert_eq!(validate(&s, &ok), Ok(()));
    let p = act(
        &s,
        evaluate("X", &["children:2"]),
        "pc/checkpoint/evaluate",
        None,
    );
    assert_eq!(codes(validate(&s, &p)), vec!["checkpoint_root_unknown"]);
    for bad in [
        &[][..],
        &["children:4"][..],
        &["children:2", "children:2"][..],
    ] {
        let p = act(&s, evaluate("R", bad), "pc/checkpoint/evaluate", None);
        assert_eq!(
            codes(validate(&s, &p)),
            vec!["checkpoint_not_due"],
            "{bad:?}"
        );
    }
    let nine: Vec<String> = (0..9).map(|i| format!("budget:{i}")).collect();
    let mut s9 = s.clone();
    s9.checkpoints[0].due = nine
        .iter()
        .map(|k| SnapshotTrigger {
            key: k.clone(),
            kind: "budget".into(),
            at: None,
        })
        .collect();
    let refs: Vec<&str> = nine.iter().map(String::as_str).collect();
    let p = act(&s9, evaluate("R", &refs), "pc/checkpoint/evaluate", None);
    assert_eq!(codes(validate(&s9, &p)), vec!["checkpoint_not_due"]);
    let mut open = s.clone();
    open.checkpoints[0].running = Some("D0".into());
    assert_eq!(
        codes(validate(&open, &ok)),
        vec!["checkpoint_evaluation_open"]
    );
    let mut paused = s.clone();
    paused.project.status = "paused".into();
    let r = validate(&paused, &ok).expect_err("paused");
    assert!(r.iter().any(|v| v.code == "launch_on_paused" && v.safety));
    // strategy root の無い snapshot（base の形）では evaluate は出せない。
    assert_eq!(
        codes(validate(&snapshot(), &ok)),
        vec!["checkpoint_root_unknown"]
    );
}

/// 不変条件（D5.2・D5.3）: 写していない評価があると、`evaluate` 以外の行動は `pc/checkpoint/act`・signal `checkpoint` で
/// その評価を名指し、表の行動（replan の reason_code・escalate の question_key・context_refs の範囲）に限られる。
#[test]
fn project_controller_checkpoint_act_must_map_unmapped() {
    let mut s = snapshot();
    let allowed = vec![
        SnapshotAllowedAction {
            kind: "replan".into(),
            reason_code: Some("checkpoint_pivot".into()),
            question_key: None,
        },
        SnapshotAllowedAction {
            kind: "escalate".into(),
            reason_code: None,
            question_key: Some("checkpoint:D1".into()),
        },
    ];
    let mut cp = checkpoint("R", &["children:4"], None);
    cp.unmapped = vec![unmapped("D1", allowed)];
    s.checkpoints = vec![cp];
    let replan = |reason: &str, refs: &[&str]| ProjectAction::Replan {
        reason_code: reason.into(),
        question: "q".into(),
        context_refs: refs.iter().map(|r| (*r).to_string()).collect(),
    };
    let escalate = |key: &str| ProjectAction::Escalate {
        category: "explicit_human".into(),
        question_key: key.into(),
        summary: "s".into(),
        options: vec![],
        evidence_refs: vec![],
    };
    let refs = [
        "task:R",
        "artifact:R/parent-evaluation/D1.json",
        "artifact:S/strategy/s1/v1.json",
    ];
    let good = act(
        &s,
        replan("checkpoint_pivot", &refs),
        CHECKPOINT_ACT_RULE,
        Some("D1"),
    );
    assert_eq!(validate(&s, &good), Ok(()));
    let good = act(
        &s,
        escalate("checkpoint:D1"),
        CHECKPOINT_ACT_RULE,
        Some("D1"),
    );
    assert_eq!(validate(&s, &good), Ok(()));
    // evaluate は写していない評価があっても出せる（D5.1 の順で evaluate が先）。
    let ev = act(
        &s,
        evaluate("R", &["children:4"]),
        "pc/checkpoint/evaluate",
        None,
    );
    assert_eq!(validate(&s, &ev), Ok(()));
    let bad = [
        act(
            &s,
            replan("checkpoint_pivot", &refs),
            "pc/outcome-open",
            Some("D1"),
        ),
        act(
            &s,
            replan("checkpoint_experiment", &refs),
            CHECKPOINT_ACT_RULE,
            Some("D1"),
        ),
        act(
            &s,
            replan("checkpoint_pivot", &["task:OTHER"]),
            CHECKPOINT_ACT_RULE,
            Some("D1"),
        ),
        act(
            &s,
            escalate("checkpoint:D9"),
            CHECKPOINT_ACT_RULE,
            Some("D1"),
        ),
        act(
            &s,
            escalate("checkpoint:D1"),
            CHECKPOINT_ACT_RULE,
            Some("D9"),
        ),
        act(&s, escalate("checkpoint:D1"), CHECKPOINT_ACT_RULE, None),
        act(
            &s,
            ProjectAction::Continue {
                watch_root_ids: vec![],
                next_wake_after_secs: 60,
            },
            CHECKPOINT_ACT_RULE,
            Some("D1"),
        ),
    ];
    for p in bad {
        assert_eq!(
            codes(validate(&s, &p)),
            vec!["checkpoint_action_mismatch"],
            "{:?}",
            p.action
        );
    }
    // 写していない評価が無ければ act の規則は使えない。
    let mut none = s.clone();
    none.checkpoints[0].unmapped.clear();
    let p = act(
        &none,
        escalate("checkpoint:D1"),
        CHECKPOINT_ACT_RULE,
        Some("D1"),
    );
    assert_eq!(
        codes(validate(&none, &p)),
        vec!["checkpoint_action_mismatch"]
    );
}

/// 不変条件（D9.2）: strategy root の無い snapshot の直列化に `checkpoints` の key が出ない（base の形と同じ）。
#[test]
fn project_controller_checkpoint_snapshot_without_strategy_roots_unchanged() {
    let v = serde_json::to_value(snapshot()).unwrap();
    assert!(v.get("checkpoints").is_none());
    let back: ProjectSnapshot = serde_json::from_value(v).unwrap();
    assert!(back.checkpoints.is_empty());
}

/// 不変条件（D5.2）: 写していない評価がある間も、計画を進めない guard の行動（act でない rule の `wait(waiting)` と
/// `escalate`）は通る。起票・CoS 入力（replan）・milestone・到達の記録（`wait(achieved)`）・continue は今までどおり
/// `checkpoint_action_mismatch` で落ちる（off_track・unknown の評価から計画は進まない）。
/// 既存の `..._act_must_map_unmapped` は act の rule の行動だけを見るので、guard の wait・escalate が落ちることを検出しない。
#[test]
fn project_controller_checkpoint_guard_while_unmapped_holds_pass_and_progress_fails() {
    let mut s = snapshot();
    let allowed = vec![SnapshotAllowedAction {
        kind: "replan".into(),
        reason_code: Some("checkpoint_pivot".into()),
        question_key: None,
    }];
    let mut cp = checkpoint("R", &[], None);
    cp.unmapped = vec![unmapped("D1", allowed)];
    s.checkpoints = vec![cp];
    let wait = |phase: WaitPhase| ProjectAction::Wait {
        waiting_on: vec![WaitingOn {
            kind: crate::project_brief::WaitingOnKind::Human,
            target: None,
            note: "n".into(),
        }],
        next_wake_after_secs: 3600,
        phase,
    };
    let mut paused = s.clone();
    paused.project.status = "paused".into();
    let p = act(
        &paused,
        wait(WaitPhase::Waiting),
        "pc/project-not-active",
        None,
    );
    assert_eq!(validate(&paused, &p), Ok(()), "paused の wait");
    let no_progress = act(
        &s,
        ProjectAction::Escalate {
            category: "explicit_human".into(),
            question_key: "no_progress:0".into(),
            summary: "s".into(),
            options: vec![],
            evidence_refs: vec![],
        },
        "pc/no-progress",
        None,
    );
    assert_eq!(
        validate(&s, &no_progress),
        Ok(()),
        "no-progress の escalate"
    );
    let daily = act(
        &s,
        wait(WaitPhase::Waiting),
        "pc/checkpoint/act/daily-limit",
        Some("D1"),
    );
    assert_eq!(validate(&s, &daily), Ok(()), "CoS 入力の日の上限の wait");
    let replan = ProjectAction::Replan {
        reason_code: "root_failed".into(),
        question: "q".into(),
        context_refs: vec!["task:R".into()],
    };
    let progress = [
        launch(&s, &["A"]),
        act(&s, replan, "pc/root-failed", None),
        act(
            &s,
            ProjectAction::MarkMilestone {
                outcome_id: "o1".into(),
                evidence_refs: vec!["task:R".into()],
            },
            "pc/milestone",
            None,
        ),
        act(&s, wait(WaitPhase::Achieved), "pc/achieved", None),
        act(
            &s,
            ProjectAction::Continue {
                watch_root_ids: vec!["R".into()],
                next_wake_after_secs: 60,
            },
            "pc/root-in-flight",
            None,
        ),
    ];
    for p in progress {
        assert!(
            codes(validate(&s, &p)).contains(&"checkpoint_action_mismatch".to_string()),
            "{:?}",
            p.action
        );
    }
}
