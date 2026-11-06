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
            auto_integrate: false,
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
