//! ADR 2026-11-07-health-review-integration D5.2・D5.3・I6・I7 の試験。分類はディスパッチャと同じく、不合格の
//! verdict を `Check::Reviewer` の `FailedCheck` にして `classify_review_failure`（変えていない）に通す。

use task_core::model::Event;
use task_core::{
    Check, FailedCheck, RepairClass, RepairDecision, ReviewerRepairKind, classify_review_failure,
};
use task_worker::protocol::HealthFindingKind;

use super::*;

const TAG: &str = "reviewer(r1)";
const REL: &str = "artifacts/review.json";
/// reviewer の受け入れ条件。最小は 1（0 は決定的な条件の想定）。
const CRITERIA: [usize; 2] = [3, 1];

/// 2 つの条件とも合格の `review.json`。`health` は JSON の断片（`None` なら欄を書かない）。
fn review_json(health: Option<&str>) -> String {
    let verdicts = r#""verdicts":[{"criterion":1,"pass":true,"reason":"ok-1"},{"criterion":3,"pass":true,"reason":"ok-3"}]"#;
    match health {
        Some(h) => format!(r#"{{{verdicts},"health":{h}}}"#),
        None => format!("{{{verdicts}}}"),
    }
}

fn finding(kind: &str, severity: &str, repair: Option<&str>) -> String {
    let repair = repair
        .map(|scope| format!(r#","repair":{{"scope":"{scope}","class":"doc"}}"#))
        .unwrap_or_default();
    format!(
        r#"{{"kind":"{kind}","severity":"{severity}","path":"crates/x/src/lib.rs","reason":"{kind} found at crates/x/src/lib.rs:12"{repair}}}"#
    )
}

fn health(findings: &[String]) -> String {
    format!(r#"{{"findings":[{}]}}"#, findings.join(","))
}

/// `health_requested` なら policy の変更の無い候補の材料を渡す。
fn verdicts(text: &str, health_requested: bool) -> Result<Vec<Verdict>, String> {
    let input = grounded_input();
    Verdict::from_review_json(
        text,
        health_requested.then_some(&input),
        &CRITERIA,
        TAG,
        REL,
    )
}

/// ディスパッチャ（`review_verdict.rs` の `maybe_schedule_review_repair`）と同じ分類。不合格が無ければ `None`。
fn classify(verdicts: &[Verdict]) -> Option<RepairDecision> {
    let failing: Vec<FailedCheck> = verdicts
        .iter()
        .filter(|v| !v.pass)
        .map(|v| FailedCheck {
            check: Check::Reviewer,
            reason: v.reason.clone(),
            repair_hint: v.repair_hint.clone(),
        })
        .collect();
    (!failing.is_empty()).then(|| classify_review_failure(&failing))
}

fn is_local(decision: &Option<RepairDecision>) -> bool {
    matches!(
        decision,
        Some(RepairDecision::Repairable(RepairClass::ReviewerLocal(_)))
    )
}

#[test]
fn health_review_verdict_missing_health_fails_every_criterion() {
    for text in [review_json(None), review_json(Some("null"))] {
        let err = verdicts(&text, true).expect_err("health を渡した review で health が無い");
        assert_eq!(
            err,
            "reviewer(r1): health verdict missing or invalid in review.json"
        );
    }
    // 呼び出し側の `fail_all` と同じく、reviewer の全条件が同じ reason で不合格になる（review.rs の配線）。
    let all = verdicts(&review_json(None), true).unwrap_or_else(|reason| {
        CRITERIA
            .iter()
            .map(|&idx| Verdict {
                criterion_idx: idx,
                pass: false,
                reason: reason.clone(),
                repair_hint: None,
            })
            .collect()
    });
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .all(|v| !v.pass && v.reason.ends_with(HEALTH_VERDICT_INVALID))
    );
}

#[test]
fn health_review_verdict_unknown_kind_or_severity_fails_every_criterion() {
    let unknown_kind = health(&[finding("new_kind", "blocking", None)]);
    let unknown_severity = health(&[finding("removal_missing", "fatal", None)]);
    let not_an_object = r#""ok""#.to_string();
    for h in [unknown_kind, unknown_severity, not_an_object] {
        let err = verdicts(&review_json(Some(&h)), true).expect_err(&h);
        assert_eq!(err, format!("{TAG}: {HEALTH_VERDICT_INVALID}"), "{h}");
    }
    // verdicts そのものが壊れていれば、health を求めた review でも従来の reason のまま。
    let err = verdicts(r#"{"verdicts":"x","health":{"findings":[]}}"#, true).expect_err("bad");
    assert!(err.starts_with("reviewer(r1): artifacts/review.json is not a valid ReviewOutput: "));
}

#[test]
fn health_review_verdict_removal_missing_with_local_scope_repairs_locally() {
    let text = review_json(Some(&health(&[
        finding("removal_missing", "blocking", Some("local")),
        finding("test_change_unjustified", "blocking", Some("local")),
    ])));
    let v = verdicts(&text, true).expect("valid");
    let failing: Vec<&Verdict> = v.iter().filter(|v| !v.pass).collect();
    assert_eq!(failing.len(), 2);
    // reviewer の受け入れ条件の最小の index・ADR の reason・reviewer のヒントのまま。
    assert_eq!(failing[0].criterion_idx, 1);
    assert_eq!(
        failing[0].reason,
        "reviewer(r1): health:removal_missing: removal_missing found at crates/x/src/lib.rs:12"
    );
    assert_eq!(
        failing[0].repair_hint,
        Some(ReviewRepairHint {
            scope: "local".into(),
            class: "doc".into()
        })
    );
    assert_eq!(
        classify(&v),
        Some(RepairDecision::Repairable(RepairClass::ReviewerLocal(
            ReviewerRepairKind::Other
        )))
    );
}

#[test]
fn health_review_verdict_dual_source_of_truth_replans() {
    // reviewer が local と書いても、D5.1 の表で局所修復を許さない種別は replan。
    let text = review_json(Some(&health(&[finding(
        "dual_source_of_truth",
        "blocking",
        Some("local"),
    )])));
    let v = verdicts(&text, true).expect("valid");
    let failing: Vec<&Verdict> = v.iter().filter(|v| !v.pass).collect();
    assert_eq!(failing.len(), 1);
    assert_eq!(
        failing[0].reason,
        "reviewer(r1): health:dual_source_of_truth: dual_source_of_truth found at crates/x/src/lib.rs:12"
    );
    assert_eq!(
        failing[0].repair_hint.as_ref().map(|h| h.scope.as_str()),
        Some(SUBSTANTIVE_SCOPE)
    );
    assert_eq!(classify(&v), Some(RepairDecision::Substantive));
    // 局所の finding と混ざっても 1 件の substantive で replan。
    let mixed = review_json(Some(&health(&[
        finding("removal_missing", "blocking", Some("local")),
        finding("dual_source_of_truth", "blocking", None),
    ])));
    assert_eq!(
        classify(&verdicts(&mixed, true).expect("valid")),
        Some(RepairDecision::Substantive)
    );
}

/// 受け入れ条件 0: blocking の 7 種が D5.1 の表どおり局所修復か replan に分かれる。
#[test]
fn health_review_verdict_seven_kinds_follow_the_d5_1_table() {
    // (kind, repair.scope = "local" で局所修復になるか)。表を写した期待値で、`allows_local_repair` を使わない。
    let table = [
        ("symptom_fix_outside_owner", false),
        ("duplicate_implementation", false),
        ("local_hook_in_shared_interface", false),
        ("dual_source_of_truth", false),
        ("unapproved_fallback_or_switch", false),
        ("removal_missing", true),
        ("test_change_unjustified", true),
    ];
    assert_eq!(
        table.map(|(k, _)| k),
        HealthFindingKind::ALL.map(HealthFindingKind::as_str)
    );
    for (kind, local) in table {
        let with_local = review_json(Some(&health(&[finding(kind, "blocking", Some("local"))])));
        let decision = classify(&verdicts(&with_local, true).expect("valid"));
        assert_eq!(is_local(&decision), local, "{kind} with repair.scope=local");
        if !local {
            assert_eq!(decision, Some(RepairDecision::Substantive), "{kind}");
        }
        // repair が無い・local 以外なら、どの種別も replan。
        for repair in [None, Some("global")] {
            let text = review_json(Some(&health(&[finding(kind, "blocking", repair)])));
            let decision = classify(&verdicts(&text, true).expect("valid"));
            assert_eq!(
                decision,
                Some(RepairDecision::Substantive),
                "{kind} {repair:?}"
            );
        }
    }
}

/// repair を `None` にすると reason の `fmt`・`lint` の文面で局所修復と推測される（`classify_reviewer_reason`）。
/// substantive の種別は文面に依らず replan のまま。
#[test]
fn health_review_verdict_substantive_reason_mentioning_format_still_replans() {
    let h = r#"{"findings":[{"kind":"duplicate_implementation","severity":"blocking","path":"crates/x/src/lib.rs","reason":"the config format is parsed twice at crates/x/src/lib.rs:12"}]}"#;
    let v = verdicts(&review_json(Some(h)), true).expect("valid");
    assert_eq!(classify(&v), Some(RepairDecision::Substantive));
    // 比較: 同じ reason でヒントの無い reviewer の不合格は、既存の規則で format の局所修復と推測される。
    let guessed = classify_review_failure(&[FailedCheck {
        check: Check::Reviewer,
        reason: "reviewer(r1): health:duplicate_implementation: the config format is parsed twice"
            .into(),
        repair_hint: None,
    }]);
    assert_eq!(
        guessed,
        RepairDecision::Repairable(RepairClass::ReviewerLocal(ReviewerRepairKind::Format))
    );
}

#[test]
fn health_review_verdict_advisory_only_passes_and_is_recorded() {
    let text = review_json(Some(&health(&[
        finding("duplicate_implementation", "advisory", None),
        finding("removal_missing", "advisory", Some("local")),
    ])));
    let mut v = verdicts(&text, true).expect("valid");
    assert!(v.iter().all(|v| v.pass), "advisory は合否を変えない");
    assert_eq!(classify(&v), None);
    assert_eq!(v.len(), 4);
    assert_eq!(
        v[0].reason,
        "reviewer(r1): health:duplicate_implementation (advisory): duplicate_implementation found at crates/x/src/lib.rs:12"
    );
    assert_eq!(v[0].criterion_idx, 1);
    assert_eq!(v[0].repair_hint, None);
    // ディスパッチャと同じ安定 sort の後も、条件 1 の最後の verdict は reviewer 自身の判定（最後を採る読み手向け）。
    v.sort_by_key(|v| v.criterion_idx);
    let last_1 = v
        .iter()
        .rfind(|v| v.criterion_idx == 1)
        .expect("criterion 1");
    assert_eq!(last_1.reason, "reviewer(r1): ok-1");
    // `Event::ReviewVerdict` に写すと、次の run の `prior_review` に advisory が載る。
    let events: Vec<(u64, Event)> = v
        .iter()
        .enumerate()
        .map(|(i, v)| {
            (
                i as u64,
                Event::ReviewVerdict {
                    run_id: "run-1".into(),
                    criterion_idx: v.criterion_idx,
                    pass: v.pass,
                    reason: v.reason.clone(),
                },
            )
        })
        .collect();
    let prior = task_ops::derive::prior_review_from_events(&events);
    assert_eq!(prior.len(), 4);
    assert!(
        prior
            .iter()
            .any(|p| p.pass && p.reason.contains("health:removal_missing (advisory)"))
    );
    // advisory と blocking が混ざれば、blocking だけが合否を決める。
    let mixed = review_json(Some(&health(&[
        finding("dual_source_of_truth", "advisory", None),
        finding("removal_missing", "blocking", Some("local")),
    ])));
    let v = verdicts(&mixed, true).expect("valid");
    assert_eq!(v.iter().filter(|v| !v.pass).count(), 1);
    assert!(is_local(&classify(&v)));
}

#[test]
fn health_review_verdict_without_health_request_keeps_previous_behavior() {
    let expected = vec![
        Verdict {
            criterion_idx: 3,
            pass: true,
            reason: "reviewer(r1): ok-3".into(),
            repair_hint: None,
        },
        Verdict {
            criterion_idx: 1,
            pass: true,
            reason: "reviewer(r1): ok-1".into(),
            repair_hint: None,
        },
    ];
    // health が無い・空・blocking を含む review.json のどれでも、health を見ずに reviewer の判定だけを採る。
    let blocking = health(&[finding("dual_source_of_truth", "blocking", None)]);
    for text in [
        review_json(None),
        review_json(Some(r#"{"findings":[]}"#)),
        review_json(Some(&blocking)),
    ] {
        assert_eq!(verdicts(&text, false), Ok(expected.clone()), "{text}");
    }
    // 判定の無い条件・読めない review.json は従来の reason。
    let v = verdicts(
        r#"{"verdicts":[{"criterion":3,"pass":false,"reason":"bad","repair":{"scope":"local","class":"lint"}}]}"#,
        false,
    )
    .expect("valid");
    assert_eq!(v[0].reason, "reviewer(r1): bad");
    assert_eq!(
        v[0].repair_hint,
        Some(ReviewRepairHint {
            scope: "local".into(),
            class: "lint".into()
        })
    );
    assert_eq!(
        v[1].reason,
        "reviewer(r1): no verdict for criterion 1 in artifacts/review.json"
    );
    let unknown_kind = health(&[finding("new_kind", "blocking", None)]);
    let err = verdicts(&review_json(Some(&unknown_kind)), false).expect_err("invalid");
    assert!(err.starts_with("reviewer(r1): artifacts/review.json is not a valid ReviewOutput: "));
}

// G3: 既存の試験は repair 分類だけを確認し、根拠の有無を区別していなかった。
fn grounded_input() -> HealthReviewInput {
    use task_worker::protocol::{DiffStat, PathStat};
    HealthReviewInput {
        repos: vec![HealthRepoInput {
            diff: DiffStat {
                files: 1,
                paths: vec![PathStat {
                    path: "crates/x/src/lib.rs".into(),
                    insertions: 1,
                    deletions: 0,
                }],
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn finding_verdict(input: &HealthReviewInput, path: Option<&str>, reason: &str) -> Vec<Verdict> {
    let health = serde_json::json!({"findings": [{
        "kind": "removal_missing", "severity": "blocking", "path": path,
        "reason": reason, "repair": {"scope": "local", "class": "doc"}
    }]});
    Verdict::from_review_json(
        &review_json(Some(&health.to_string())),
        Some(input),
        &CRITERIA,
        TAG,
        REL,
    )
    .expect("valid review")
}

#[test]
fn health_enforce_llm_finding_grounded_line_blocks() {
    for reason in [
        "crates/x/src/lib.rs:12 retains old code",
        "see `crates/x/src/lib.rs#L2`",
    ] {
        let v = finding_verdict(&grounded_input(), Some("crates/x/src/lib.rs"), reason);
        assert_eq!(v.iter().filter(|v| !v.pass).count(), 1);
        assert!(is_local(&classify(&v)));
    }
}

#[test]
fn health_enforce_llm_finding_ungrounded_is_retained_as_advisory() {
    for reason in [
        "old code remains",
        "crates/x/src/lib.rs:0",
        "crates/x/src/lib.rs:12abc",
        "other/crates/x/src/lib.rs:12",
        "crates/x/src/lib.rs has 12 lines",
    ] {
        let mut v = finding_verdict(&grounded_input(), Some("crates/x/src/lib.rs"), reason);
        assert_eq!(classify(&v), None, "{reason}");
        assert_eq!(v.len(), 3);
        assert!(v[0].reason.contains("health:ungrounded"));
        assert!(v[0].reason.ends_with(reason));
        assert!(v[0].repair_hint.is_none());
        v.sort_by_key(|v| v.criterion_idx);
        assert_eq!(
            v.iter().rfind(|v| v.criterion_idx == 1).unwrap().reason,
            "reviewer(r1): ok-1"
        );
    }
}

#[test]
fn health_enforce_llm_finding_outside_diff_is_advisory() {
    for path in [None, Some(""), Some("other.rs")] {
        let v = finding_verdict(&grounded_input(), path, "other.rs:12 old code");
        assert_eq!(classify(&v), None);
        assert!(v[0].reason.contains("health:ungrounded"));
    }
}

#[test]
fn health_enforce_llm_finding_report_fingerprint_blocks_only_in_matching_repo() {
    use task_worker::protocol::HealthReportFinding;
    let mut input = grounded_input();
    input.repos[0].findings.reviewable = true;
    input.repos[0].findings.new.push(HealthReportFinding {
        rule_id: "deps.violation".into(),
        fingerprint: "deps.violation:abcdef".into(),
        severity: "error".into(),
        delta: "new".into(),
        path: Some("crates/x/src/lib.rs".into()),
    });
    let run = |i: &HealthReviewInput| {
        finding_verdict(i, Some("crates/x/src/lib.rs"), "see deps.violation:abcdef")
    };
    assert!(is_local(&classify(&run(&input))));
    input.repos[0].findings.reviewable = false;
    assert_eq!(classify(&run(&input)), None);
    input.repos[0].findings.reviewable = true;
    let mut other = input.repos[0].clone();
    other.diff.paths.clear();
    input.repos[0].findings.new.clear();
    input.repos.push(other);
    assert_eq!(classify(&run(&input)), None);
}

#[test]
fn health_enforce_llm_finding_owner_row_blocks_only_for_its_path() {
    use task_worker::protocol::OwnerRow;
    let mut input = grounded_input();
    input.repos[0].owner_rows.push(OwnerRow {
        path: "crates/x/src/lib.rs".into(),
        row: "| x | owner | entry |".into(),
    });
    let run = |i: &HealthReviewInput| {
        finding_verdict(
            i,
            Some("crates/x/src/lib.rs"),
            "contradicts | x | owner | entry |",
        )
    };
    assert!(is_local(&classify(&run(&input))));
    input.repos[0].owner_rows[0].path = "other.rs".into();
    assert_eq!(classify(&run(&input)), None);
    input.repos[0].owner_rows[0].path = "crates/x/src/lib.rs".into();
    input.repos[0].owner_rows[0].row.clear();
    assert_eq!(classify(&run(&input)), None);
}

#[test]
fn health_enforce_llm_finding_declared_removal_blocks() {
    let mut input = grounded_input();
    input.brief = Some(Box::new(
        serde_json::from_value(serde_json::json!({
            "root_cause": "duplicate state", "owner": "x", "options": [],
            "removals": ["legacy retry map"]
        }))
        .unwrap(),
    ));
    let run = |i: &HealthReviewInput| {
        finding_verdict(i, Some("crates/x/src/lib.rs"), "legacy retry map remains")
    };
    assert!(is_local(&classify(&run(&input))));
    input.brief.as_mut().unwrap().removals = vec!["".into()];
    assert_eq!(classify(&run(&input)), None);
}

#[test]
fn health_enforce_llm_finding_advisory_does_not_override_failed_criterion() {
    let text = review_json(Some(&health(&[finding(
        "removal_missing",
        "blocking",
        None,
    )])))
    .replace("\"pass\":true", "\"pass\":false");
    let v = Verdict::from_review_json(
        &text,
        Some(&HealthReviewInput::default()),
        &CRITERIA,
        TAG,
        REL,
    )
    .unwrap();
    assert!(v[0].pass && v[0].reason.contains("health:ungrounded"));
    assert_eq!(v.iter().filter(|v| !v.pass).count(), 2);
}
