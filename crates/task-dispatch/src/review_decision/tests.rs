//! review_decision の表の試験（入力 → 提案）。
//!
//! 守る不変条件: 決定的 check の失敗一覧が空でなければ review を成功にしない（reviewer が合格でも）。失敗一覧は 3 経路
//! （最終 review・WU の checks・段の統合）で同じ規則で作る。delivery の結論は同じ候補（worker run・review run・
//! `Reviewing`）の行にだけ書く。repair は Execute の task で上限の内だけ。再同期は上限を超えたら止め、止めた記録から
//! 再開する。reviewer の合格は、target の進みが候補の差分と重ならないときだけ再利用する。
//! 既存の試験（`dispatcher/tests/review.rs`・`work_units.rs`・`health_enforce.rs` など）は daemon の経路を通して
//! 1 つずつの場合を見るだけで、境界（上限ちょうど・行の照合の各欄の不一致・止めた記録が無い場合）を表で押さえていない。

use super::*;
use task_core::execution::ReviewRepairHint;

fn verdict(idx: usize, pass: bool, reason: &str) -> Verdict {
    Verdict {
        criterion_idx: idx,
        pass,
        reason: reason.to_string(),
        repair_hint: None,
    }
}

fn criterion(check: Check) -> Criterion {
    Criterion {
        text: String::new(),
        check,
    }
}

fn cmd(c: &str) -> Check {
    Check::Command {
        cmd: c.to_string(),
        expect_exit: 0,
    }
}

fn result(check: Check, pass: bool, reason: &str) -> CheckResult {
    CheckResult {
        check,
        pass,
        reason: reason.to_string(),
        repair_hint: None,
    }
}

fn local_hint(class: &str) -> Option<ReviewRepairHint> {
    Some(ReviewRepairHint {
        scope: "local".to_string(),
        class: class.to_string(),
    })
}

#[test]
fn review_decision_failed_checks_table() {
    type Row = (
        &'static str,
        Vec<CheckResult>,
        Vec<(&'static str, &'static str)>,
    );
    let rows: Vec<Row> = vec![
        ("empty", vec![], vec![]),
        (
            "all pass",
            vec![result(cmd("a"), true, "ok"), result(cmd("b"), true, "ok")],
            vec![],
        ),
        (
            "keeps the order of the failures",
            vec![
                result(cmd("a"), false, "a broke"),
                result(cmd("b"), true, "ok"),
                result(cmd("c"), false, "c broke"),
            ],
            vec![("a", "a broke"), ("c", "c broke")],
        ),
    ];
    for (name, results, want) in rows {
        let got: Vec<(String, String)> = failed_checks(results)
            .into_iter()
            .map(|f| match f.check {
                Check::Command { cmd, .. } => (cmd, f.reason),
                other => (format!("{other:?}"), f.reason),
            })
            .collect();
        let want: Vec<(String, String)> = want
            .into_iter()
            .map(|(c, r)| (c.to_string(), r.to_string()))
            .collect();
        assert_eq!(got, want, "{name}");
    }
    // repair のヒントと check の種類は落とさない。
    let hinted = CheckResult {
        repair_hint: local_hint("format"),
        ..result(Check::Reviewer, false, "fmt")
    };
    let got = failed_checks([hinted]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].check, Check::Reviewer);
    assert_eq!(got[0].repair_hint, local_hint("format"));
}

#[test]
fn review_decision_verdict_results_map_criteria_and_implicit_checks() {
    let acceptance = vec![criterion(cmd("cargo test")), criterion(Check::Reviewer)];
    let health_id = task_core::required_checks::health::HEALTH_CHECK_ID;
    let health_fail = Verdict {
        repair_hint: Some(ReviewRepairHint {
            scope: health_id.to_string(),
            class: "other".to_string(),
        }),
        ..verdict(3, false, "health:rule:abc")
    };
    let verdicts = vec![
        verdict(0, false, "test failed"),
        verdict(1, true, "reviewer ok"),
        verdict(2, false, "plan missing"),
        health_fail,
    ];
    let checks: Vec<Check> = verdict_results(&acceptance, &verdicts)
        .map(|r| r.check)
        .collect();
    assert_eq!(
        checks,
        vec![
            cmd("cargo test"),
            Check::Reviewer,
            Check::Human,
            cmd(health_id)
        ]
    );
}

#[test]
fn review_decision_judge_review_table() {
    let acceptance = vec![criterion(cmd("cargo test")), criterion(Check::Reviewer)];
    type Row = (
        &'static str,
        Vec<Verdict>,
        Option<&'static str>,
        bool,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        ("no verdicts", vec![], None, true, None),
        (
            "all pass",
            vec![verdict(0, true, "ok"), verdict(1, true, "lgtm")],
            None,
            true,
            None,
        ),
        (
            "reviewer passed but the deterministic check failed",
            vec![verdict(0, false, "exit 1"), verdict(1, true, "lgtm")],
            Some("review discrepancy"),
            false,
            Some("exit 1"),
        ),
        (
            "both failed",
            vec![verdict(0, false, "exit 1"), verdict(1, false, "bad design")],
            None,
            false,
            Some("exit 1; bad design"),
        ),
        (
            "implicit condition failed",
            vec![
                verdict(0, true, "ok"),
                verdict(2, false, "summary.md missing"),
            ],
            None,
            false,
            Some("summary.md missing"),
        ),
    ];
    for (name, verdicts, discrepancy, passed, fail_message) in rows {
        let got = judge_review(ReviewFacts {
            acceptance: &acceptance,
            verdicts: &verdicts,
            discrepancy: discrepancy.map(str::to_string),
        });
        assert_eq!(got.passed, passed, "{name}");
        assert_eq!(got.fail_message.as_deref(), fail_message, "{name}");
        assert_eq!(got.discrepancy.as_deref(), discrepancy, "{name}");
    }
}

#[test]
fn review_decision_reviewer_criteria_passed_table() {
    let with_two = vec![
        criterion(Check::Reviewer),
        criterion(cmd("t")),
        criterion(Check::Reviewer),
    ];
    let without = vec![criterion(cmd("t"))];
    type Row<'a> = (
        &'static str,
        &'a [Criterion],
        Vec<Verdict>,
        Option<Vec<usize>>,
    );
    let rows: Vec<Row<'_>> = vec![
        (
            "no reviewer criterion",
            &without,
            vec![verdict(0, true, "")],
            None,
        ),
        (
            "all reviewer criteria pass (command fails)",
            &with_two,
            vec![
                verdict(0, true, ""),
                verdict(1, false, ""),
                verdict(2, true, ""),
            ],
            Some(vec![0, 2]),
        ),
        (
            "one reviewer criterion fails",
            &with_two,
            vec![verdict(0, true, ""), verdict(2, false, "")],
            None,
        ),
        (
            "a reviewer criterion has no verdict",
            &with_two,
            vec![verdict(0, true, "")],
            None,
        ),
    ];
    for (name, acceptance, verdicts, want) in rows {
        assert_eq!(
            reviewer_criteria_passed(acceptance, &verdicts),
            want,
            "{name}"
        );
    }
}

#[test]
fn review_decision_delivery_verdict_table() {
    let verdicts = vec![
        verdict(0, false, "exit 1"),
        verdict(1, true, "reviewer(rv): [needs-human] confirm the copy"),
    ];
    let row = |worker_run: &'static str, review_run: &'static str, reviewing: bool, idx: usize| {
        DeliveryRow {
            worker_run,
            review_run,
            reviewing,
            criterion_idx: idx,
        }
    };
    type Row = (
        &'static str,
        DeliveryRow<'static>,
        bool,
        Option<(bool, &'static str)>,
    );
    let both =
        "[needs-human] confirm the copy\nexit 1\nreviewer(rv): [needs-human] confirm the copy";
    let rows: Vec<Row> = vec![
        ("other worker run", row("w2", "rv", true, 1), true, None),
        ("other review run", row("w", "rv2", true, 1), true, None),
        ("row already decided", row("w", "rv", false, 1), true, None),
        (
            "no verdict for the row's criterion",
            row("w", "rv", true, 7),
            true,
            None,
        ),
        (
            "whole review passed",
            row("w", "rv", true, 1),
            true,
            Some((true, both)),
        ),
        (
            "row criterion passed but the review failed",
            row("w", "rv", true, 1),
            false,
            Some((false, both)),
        ),
        (
            "row criterion failed",
            row("w", "rv", true, 0),
            true,
            Some((false, "exit 1")),
        ),
    ];
    for (name, row, passed, want) in rows {
        let got = delivery_verdict(&row, "w", "rv", passed, &verdicts);
        let want = want.map(|(pass, detail)| DeliveryVerdict {
            pass,
            detail: detail.to_string(),
        });
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn review_decision_review_repair_class_table() {
    let not_evaluated = || result(Check::Reviewer, false, REVIEWER_NOT_EVALUATED_REASON);
    let local = |class: &str, reason: &str| CheckResult {
        repair_hint: local_hint(class),
        ..result(Check::Reviewer, false, reason)
    };
    // 期待は (reviewer の局所修復か, 失敗一覧の件数)。
    type Row = (
        &'static str,
        TaskKind,
        Vec<CheckResult>,
        Option<(bool, usize)>,
    );
    let rows: Vec<Row> = vec![
        (
            "plan tasks are not repaired",
            TaskKind::Plan,
            vec![local("format", "fmt")],
            None,
        ),
        (
            "reviewer local repair",
            TaskKind::Execute,
            vec![local("format", "fmt")],
            Some((true, 1)),
        ),
        (
            "a human check cannot be repaired",
            TaskKind::Execute,
            vec![result(Check::Human, false, "rejected")],
            None,
        ),
        (
            "mixed classes are substantive",
            TaskKind::Execute,
            vec![local("format", "fmt"), result(Check::Human, false, "no")],
            None,
        ),
        (
            "only the not-evaluated reviewer item: kept, and it is substantive",
            TaskKind::Execute,
            vec![not_evaluated()],
            None,
        ),
    ];
    for (name, kind, results, want) in rows {
        let got = review_repair_class(kind, failed_checks(results))
            .map(|(c, f)| (matches!(c, RepairClass::ReviewerLocal(_)), f.len()));
        assert_eq!(got, want, "{name}");
    }
    // 決定的な不合格があれば、reviewer run を起こさなかった項目は分類と失敗一覧から外れる。
    let health_id = task_core::required_checks::health::HEALTH_CHECK_ID;
    let with_health = vec![
        result(
            cmd(health_id),
            false,
            "health:size.responsibility-growth:abc",
        ),
        not_evaluated(),
    ];
    let got = review_repair_class(TaskKind::Execute, failed_checks(with_health));
    let (class, failing) = got.expect("health fail is repairable");
    assert_eq!(class, RepairClass::Health);
    assert_eq!(failing.len(), 1);
    assert_eq!(failing[0].check, cmd(health_id));
    // reviewer が実際に落とした項目（reason が違う）は外さない（混ざれば substantive）。
    let with_reviewer = vec![
        result(
            cmd(health_id),
            false,
            "health:size.responsibility-growth:abc",
        ),
        result(Check::Reviewer, false, "reviewer(rv): wrong design"),
    ];
    assert_eq!(
        review_repair_class(TaskKind::Execute, failed_checks(with_reviewer)),
        None
    );
}

#[test]
fn review_decision_repair_cap_reached_table() {
    let usage = |repairs, same_class| RepairUsage {
        repairs,
        max_repairs: 3,
        same_class,
        max_per_class: 2,
    };
    let rows = [
        ("none used", usage(0, 0), false),
        ("below both", usage(2, 1), false),
        ("task cap reached", usage(3, 0), true),
        ("class cap reached", usage(1, 2), true),
        ("both over", usage(4, 3), true),
    ];
    for (name, u, want) in rows {
        assert_eq!(repair_cap_reached(u), want, "{name}");
    }
    let zero = RepairUsage {
        repairs: 0,
        max_repairs: 0,
        same_class: 0,
        max_per_class: 5,
    };
    assert!(repair_cap_reached(zero), "max_repairs = 0 forbids repairs");
}

#[test]
fn review_decision_resync_gate_table() {
    let advanced = Event::ReviewTargetAdvanced {
        review_run: "rv-last".into(),
        repo_id: RepoId(ulid::Ulid(1)),
        reviewed_sha: "c2".into(),
        target_sha: "t2".into(),
        attempt: 3,
    };
    let halted_ev = Event::ReviewResyncHalted {
        review_run: "rv-halt".into(),
        repo_id: RepoId(ulid::Ulid(2)),
        reviewed_sha: "c1".into(),
        target_sha: "t1".into(),
        resyncs: 3,
    };
    let halted = |run: &str, repo: u128, reviewed: &str, target: &str, resyncs| HaltedReview {
        review_run: run.into(),
        repo_id: RepoId(ulid::Ulid(repo)),
        reviewed_sha: reviewed.into(),
        target_sha: target.into(),
        resyncs,
    };
    type Row<'a> = (
        &'static str,
        (u32, Option<&'a Event>, Option<&'a Event>),
        ResyncGate,
    );
    let rows: Vec<Row<'_>> = vec![
        (
            "nothing stale",
            (0, None, None),
            ResyncGate::Proceed { clear_halt: true },
        ),
        (
            "at the cap",
            (2, Some(&advanced), None),
            ResyncGate::Proceed { clear_halt: true },
        ),
        (
            "over the cap the first time",
            (3, Some(&advanced), None),
            ResyncGate::Halt {
                first: true,
                halted: halted("rv-last", 1, "c2", "t2", 3),
            },
        ),
        (
            "already halted: the halt record wins",
            (4, Some(&advanced), Some(&halted_ev)),
            ResyncGate::Halt {
                first: false,
                halted: halted("rv-halt", 2, "c1", "t1", 4),
            },
        ),
        (
            "over the cap without a record keeps the halt snapshot",
            (3, None, None),
            ResyncGate::Proceed { clear_halt: false },
        ),
    ];
    for (name, stale, want) in rows {
        assert_eq!(resync_gate(stale, 2), want, "{name}");
    }
}

#[test]
fn review_decision_target_quiet_table() {
    let rows = [
        ("just committed", 1_000, 1_000, 600, false),
        ("one second short", 1_599, 1_000, 600, false),
        ("exactly quiet", 1_600, 1_000, 600, true),
        ("long quiet", 10_000, 1_000, 600, true),
        ("commit in the future", 1_000, 2_000, 0, false),
        ("zero quiet", 1_000, 1_000, 0, true),
    ];
    for (name, now, committed, quiet, want) in rows {
        assert_eq!(target_quiet(now, committed, quiet), want, "{name}");
    }
}

#[test]
fn review_decision_reviewer_reuse_overlap_table() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let rows = [
        ("target did not move", s(&[]), s(&["a.rs"]), vec![]),
        ("disjoint", s(&["b.rs", "c.rs"]), s(&["a.rs"]), vec![]),
        (
            "overlap",
            s(&["a.rs", "b.rs"]),
            s(&["b.rs", "a.rs"]),
            vec!["a.rs", "b.rs"],
        ),
        ("task touched nothing", s(&["a.rs"]), s(&[]), vec![]),
    ];
    for (name, target_files, task_files, want) in rows {
        assert_eq!(
            reviewer_reuse_overlap(&target_files, &task_files),
            want,
            "{name}"
        );
    }
}
