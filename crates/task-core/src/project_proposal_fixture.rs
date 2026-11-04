//! ADR 2026-11-04-project-controller D7: `control-ts/fixtures/project_controller/invariants.jsonl` を作る。
//! 行は「入力の snapshot」と「規則の変更では変わらない不変条件」（許される行動・起票の禁止・wait の phase）。
//! 方針の期待値はここに置かない（`control-ts/src/domains/project_controller.test.ts`。TS だけで変えられる）。
//! `cargo run -q -p task-core --example dump-project-controller-fixtures > control-ts/fixtures/project_controller/invariants.jsonl`

use serde::Serialize;

use super::*;

#[derive(Serialize)]
struct Row {
    name: &'static str,
    input: ProjectSnapshot,
    invariants: Invariants,
}

#[derive(Serialize)]
struct Invariants {
    allowed_kinds: Vec<&'static str>,
    must_not_launch_root: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    wait_phase: Option<&'static str>,
}

const REQUEST: &str = "fixture の依頼\n```celeris-outcomes\n[{\"id\":\"o1\",\"title\":\"中間\",\"acceptance\":[\"A1\"]},\
{\"id\":\"o2\",\"title\":\"最終\",\"acceptance\":[\"B1\",\"B2\"],\"depends_on\":[\"o1\"]}]\n```\n";

fn base() -> ProjectSnapshot {
    ProjectSnapshot {
        now: "2026-11-04T00:00:00Z".into(),
        truncated: false,
        project: SnapshotProject {
            id: "01M6JW1EFMF6T3CVCEH9ATR1PJ".into(),
            title: "fixture".into(),
            status: "active".into(),
            request: REQUEST.into(),
            request_sha256: sha256_hex(REQUEST),
            auto_integrate: false,
        },
        brief: None,
        outcomes: parse_outcomes(REQUEST).unwrap_or_default(),
        outcomes_invalid: false,
        roots: Vec::new(),
        waits: Vec::new(),
        stagnation: Vec::new(),
        wakes: Vec::new(),
        decisions: Vec::new(),
        controller: SnapshotController {
            mode: "enabled".into(),
            phase: "active".into(),
            revision: 7,
            human_epoch: 2,
            retry_count: 0,
            progress_epoch: 0,
        },
        limits: SnapshotLimits::default(),
        roadmap: None,
        authority: None,
    }
}

fn root(id: &str, key: &str, status: &str, passed: bool, integrated: bool) -> SnapshotRoot {
    let outcome = parse_root_key(key).map(|(o, _)| o.to_owned());
    SnapshotRoot {
        id: id.into(),
        status: status.into(),
        title: id.into(),
        controller_key: Some(key.into()),
        outcome_ids: outcome.into_iter().collect(),
        acceptance: Vec::new(),
        reviewer_passed: passed,
        integrated,
        failure_class: None,
        attempts: 0,
        open_waits: Vec::new(),
        leaf_done: 0,
        leaf_total: 1,
        continues_root_ids: Vec::new(),
    }
}

fn decision(
    kind: &str,
    outcome: Option<&str>,
    reason: Option<&str>,
    key: Option<&str>,
) -> SnapshotDecision {
    SnapshotDecision {
        id: format!("d-{kind}-{}", outcome.or(reason).or(key).unwrap_or("x")),
        kind: kind.into(),
        rule_id: "fixture".into(),
        effect_state: "applied".into(),
        root_key: key.map(str::to_owned),
        reason_code: reason.map(str::to_owned),
        outcome_id: outcome.map(str::to_owned),
        request_sha256: sha256_hex(REQUEST),
        progress_epoch: 0,
        at: "2026-11-04T00:00:00Z".into(),
    }
}

fn row(name: &'static str, input: ProjectSnapshot, kinds: &[&'static str], must_not: bool) -> Row {
    Row {
        name,
        input,
        invariants: Invariants {
            allowed_kinds: kinds.to_vec(),
            must_not_launch_root: must_not,
            wait_phase: None,
        },
    }
}

fn rows() -> Vec<Row> {
    let mut out = Vec::new();
    out.push(row(
        "first_outcome_may_launch",
        base(),
        &["launch_root", "wait", "replan", "escalate"],
        false,
    ));
    for status in ["paused", "cancelled"] {
        let mut s = base();
        s.project.status = status.into();
        out.push(row(
            if status == "paused" {
                "paused_never_launches"
            } else {
                "cancelled_never_launches"
            },
            s,
            &["wait", "escalate"],
            true,
        ));
    }
    let mut s = base();
    s.outcomes = Vec::new();
    s.outcomes_invalid = true;
    out.push(row(
        "outcomes_invalid_never_launches",
        s,
        &["escalate", "wait"],
        true,
    ));
    let mut s = base();
    s.outcomes = Vec::new();
    out.push(row("no_outcomes_asks_cos", s, &["replan", "wait"], true));
    let mut s = base();
    s.outcomes = Vec::new();
    s.decisions = vec![decision("replan", None, Some("no_outcomes"), None)];
    out.push(row(
        "no_outcomes_asked_waits",
        s,
        &["wait", "escalate"],
        true,
    ));
    let mut s = base();
    s.roots = vec![
        root("R1", "outcome:o1:1", "done", true, true),
        root("R2", "outcome:o2:1", "done", true, true),
    ];
    s.decisions = vec![
        decision("mark_milestone", Some("o1"), None, None),
        decision("mark_milestone", Some("o2"), None, None),
    ];
    let mut r = row("achieved_never_launches", s, &["wait"], true);
    r.invariants.wait_phase = Some("achieved");
    out.push(r);
    let mut s = base();
    s.roots = vec![
        root("R1", "outcome:o1:1", "done", true, true),
        root("R2", "outcome:o2:1", "done", true, true),
    ];
    out.push(row(
        "reached_unrecorded_marks",
        s,
        &["mark_milestone", "wait"],
        true,
    ));
    let mut s = base();
    s.waits = vec![SnapshotWait {
        kind: "question".into(),
        id: "Q1".into(),
        task_id: "T1".into(),
        age_secs: 60,
    }];
    out.push(row(
        "human_wait_never_launches",
        s,
        &["wait", "escalate"],
        true,
    ));
    let mut s = base();
    s.roots = vec![root("R1", "outcome:o1:1", "running", false, false)];
    out.push(row(
        "running_root_never_duplicates",
        s,
        &["continue", "wait"],
        true,
    ));
    let mut s = base();
    s.roots = vec![root("R1", "outcome:o1:1", "done", true, false)];
    out.push(row(
        "unintegrated_never_launches",
        s,
        &["continue", "wait"],
        true,
    ));
    let mut s = base();
    let mut r1 = root("R1", "outcome:o1:1", "failed", false, false);
    r1.failure_class = Some("review_failed".into());
    s.roots = vec![r1];
    out.push(row(
        "review_failed_never_launches",
        s,
        &["replan", "wait", "escalate"],
        true,
    ));
    let mut s = base();
    let mut r1 = root("R1", "outcome:o1:1", "failed", false, false);
    r1.failure_class = Some("infra".into());
    s.roots = vec![r1];
    out.push(row(
        "infra_failed_never_launches",
        s,
        &["wait", "escalate", "replan"],
        true,
    ));
    let mut s = base();
    s.roots = vec![
        root("R1", "outcome:o1:1", "done", false, false),
        root("R2", "outcome:o1:2", "done", false, false),
    ];
    out.push(row(
        "attempts_exhausted_never_launches",
        s,
        &["escalate", "wait"],
        true,
    ));
    let mut s = base();
    s.limits.roots_remaining_today = 0;
    out.push(row(
        "daily_limit_never_launches",
        s,
        &["wait", "escalate"],
        true,
    ));
    out
}

/// fixture の本文（1 行 1 JSON、末尾改行）。
pub fn invariants_fixture_jsonl() -> Result<String, serde_json::Error> {
    let mut body = String::new();
    for r in rows() {
        body.push_str(&serde_json::to_string(&r)?);
        body.push('\n');
    }
    Ok(body)
}
