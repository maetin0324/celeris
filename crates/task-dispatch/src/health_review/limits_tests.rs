//! ADR 2026-11-08-health-enforcement-loop D4.3: [`judge`] の試験（試験名は `health_limits_repeat_`）。events を組んで渡す
//! だけで、時計・I/O・検査器・LLM を使わない。
//!
//! 守る不変条件: 修復の後に同じ集合が残る・前の修復で消えた fingerprint が再び現れたときだけ Repeat、初回・別の集合・
//! 判定の外（Error）は Proceed、fingerprints は順と重複に依らない（受信箱の鍵の hash は task-core の 1 か所が求める）、
//! 人の再開（comment・answer・reopen）の後の最初の評価は Proceed。既存の `plan_gate::stall` の試験は `(key, 失敗の署名)` と commit の有無だけを見るので、fingerprint の集合の
//! 往復を検出できない。

use super::*;
use task_core::execution::RepairOrigin;
use task_core::{FailedWorkUnitCheck, PhaseCheckResult, Status};

const WU: &str = "build";
const PHASE: &str = "core";

fn evidence(repo_id: RepoId, verdict: HealthVerdict, blocking: &[&str]) -> Box<HealthEvidence> {
    Box::new(HealthEvidence {
        repo_id,
        base_sha: "base".into(),
        head_sha: "head".into(),
        head_tree: "tree".into(),
        policy_sha256: "policy".into(),
        checker: None,
        report_sha256: "report".into(),
        tool_version: "1".into(),
        verdict,
        blocking: blocking.iter().map(|s| s.to_string()).collect(),
        reason: None,
        reused_from: None,
        reviewable: true,
    })
}

fn verdict_of(blocking: &[&str]) -> HealthVerdict {
    if blocking.is_empty() {
        HealthVerdict::Pass
    } else {
        HealthVerdict::Fail
    }
}

/// WU `key` の check の health の評価（空の集合は Pass）。
fn wu_check(repo: RepoId, key: &str, blocking: &[&str]) -> Event {
    wu_check_with(key, evidence(repo, verdict_of(blocking), blocking))
}

fn wu_check_with(key: &str, health: Box<HealthEvidence>) -> Event {
    Event::WorkUnitCheckFinished {
        work_unit_id: format!("wu-{key}"),
        key: key.to_string(),
        run_id: "run".into(),
        index: 0,
        total: 1,
        cmd: HEALTH_CHECK_ID.into(),
        pass: health.verdict == HealthVerdict::Pass,
        exit: Some(0),
        timed_out: false,
        duration_ms: 1,
        health: Some(health),
    }
}

/// WU `key` の run を daemon が checks の不合格で retry にすり替えた（WU の再試行）。
fn wu_retry(key: &str) -> Event {
    Event::WorkUnitChecksFailed {
        run_id: "run".into(),
        work_unit_id: format!("wu-{key}"),
        key: key.to_string(),
        cwd: "/w".into(),
        failed: vec![FailedWorkUnitCheck {
            cmd: HEALTH_CHECK_ID.into(),
            expect_exit: 0,
            detail: "health".into(),
        }],
    }
}

/// 段 `phase` の統合の health の評価。
fn integration_check(repo: RepoId, phase: &str, blocking: &[&str]) -> Event {
    Event::IntegrationCheckFinished {
        work_unit_id: "integ".into(),
        key: task_core::integrate_key(phase),
        index: 0,
        total: 1,
        cmd: HEALTH_CHECK_ID.into(),
        pass: blocking.is_empty(),
        exit: Some(0),
        timed_out: false,
        duration_ms: 1,
        health: Some(evidence(repo, verdict_of(blocking), blocking)),
    }
}

fn repair(origin: RepairOrigin) -> Event {
    Event::RepairScheduled {
        work_unit_id: "repair".into(),
        key: "repair-1".into(),
        class: "health:size.responsibility-growth".into(),
        origin,
    }
}

/// task の review の必須検査の列（repo ごとの health の証跡）。
fn review(rows: Vec<(RepoId, Box<HealthEvidence>)>) -> Vec<Event> {
    rows.into_iter()
        .map(|(repo_id, health)| Event::RequiredChecksEvaluated {
            review_run: "review".into(),
            repo_id,
            target_sha: "base".into(),
            reviewed_sha: "head".into(),
            definition_sha256: "def".into(),
            results: vec![
                PhaseCheckResult::new("cargo test".into(), true, "ok".into()),
                PhaseCheckResult {
                    cmd: HEALTH_CHECK_ID.into(),
                    pass: health.verdict == HealthVerdict::Pass,
                    summary: "health".into(),
                    health: Some(health),
                },
            ],
        })
        .collect()
}

fn review_fail_retry() -> Event {
    transitioned(Status::Reviewing, Status::Ready, Trigger::ReviewFail.name())
}

fn transitioned(from: Status, to: Status, reason: &str) -> Event {
    Event::Transitioned {
        from,
        to,
        reason: reason.to_string(),
    }
}

fn numbered(events: Vec<Event>) -> Vec<(u64, Event)> {
    events
        .into_iter()
        .enumerate()
        .map(|(i, e)| (i as u64 + 1, e))
        .collect()
}

fn repeat(fingerprints: &[&str]) -> RepeatVerdict {
    RepeatVerdict::Repeat {
        fingerprints: fingerprints.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn health_limits_repeat_same_set_after_repair_repeats() {
    let repo = RepoId::new();
    // WU の check: 再試行の後も同じ集合が残る。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-b", "fp-a"]),
        wu_retry(WU),
        wu_check(repo, WU, &["fp-a", "fp-b"]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        repeat(&["fp-a", "fp-b"])
    );
    // 段の統合: 修復の後も同じ集合が残る（評価が修復の前に何件あっても、比べるのは最後の修復の前の評価）。
    let events = numbered(vec![
        integration_check(repo, PHASE, &["fp-x"]),
        integration_check(repo, PHASE, &["fp-a"]),
        repair(RepairOrigin::Integration),
        integration_check(repo, PHASE, &["fp-a"]),
    ]);
    assert_eq!(judge(&events, HealthScope::Phase(PHASE)), repeat(&["fp-a"]));
    // task の review: review の修復（RepairScheduled）と review の不合格による再実行のどちらも修復として数える。
    for between in [repair(RepairOrigin::Review), review_fail_retry()] {
        let mut events = review(vec![(repo, evidence(repo, HealthVerdict::Fail, &["fp-a"]))]);
        events.push(between);
        events.extend(review(vec![(
            repo,
            evidence(repo, HealthVerdict::Fail, &["fp-a"]),
        )]));
        assert_eq!(
            judge(&numbered(events), HealthScope::Review),
            repeat(&["fp-a"])
        );
    }
}

#[test]
fn health_limits_repeat_vanished_fingerprint_reappears() {
    let repo = RepoId::new();
    // 1 回目の修復で a が消えて b が出た。2 回目の修復で b が消えて a が戻った（a⇄b の往復）。最後の修復だけを見ると
    // b が消えただけなので、前のどの修復で消えたかも見る。
    let events = numbered(vec![
        integration_check(repo, PHASE, &["fp-a"]),
        repair(RepairOrigin::Integration),
        integration_check(repo, PHASE, &["fp-b"]),
        repair(RepairOrigin::Integration),
        integration_check(repo, PHASE, &["fp-a", "fp-c"]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::Phase(PHASE)),
        repeat(&["fp-a", "fp-c"]),
        "the escalated set is the whole current set"
    );
    // 修復を挟まずに消えた fingerprint（例: target の同期で消えた）の再出現は往復にしない。
    let events = numbered(vec![
        integration_check(repo, PHASE, &["fp-a"]),
        integration_check(repo, PHASE, &["fp-b"]),
        repair(RepairOrigin::Integration),
        integration_check(repo, PHASE, &["fp-a"]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::Phase(PHASE)),
        RepeatVerdict::Proceed
    );
}

#[test]
fn health_limits_repeat_different_set_proceeds() {
    let repo = RepoId::new();
    for (before, after) in [
        // 別の指摘になった（前のものは直った）。
        (&["fp-a"][..], &["fp-b"][..]),
        // 一部が直った（進展）。
        (&["fp-a", "fp-b"][..], &["fp-a"][..]),
        // 直らずに新しい指摘が増えた（同じ集合ではなく、消えた fingerprint も無い）。
        (&["fp-a"][..], &["fp-a", "fp-b"][..]),
    ] {
        let events = numbered(vec![
            wu_check(repo, WU, before),
            wu_retry(WU),
            wu_check(repo, WU, after),
        ]);
        assert_eq!(
            judge(&events, HealthScope::WorkUnit(WU)),
            RepeatVerdict::Proceed,
            "{before:?} -> {after:?}"
        );
    }
    // 今の評価が Pass なら往復ではない。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-a"]),
        wu_retry(WU),
        wu_check(repo, WU, &[]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
}

#[test]
fn health_limits_repeat_first_evaluation_proceeds() {
    let repo = RepoId::new();
    assert_eq!(judge(&[], HealthScope::Review), RepeatVerdict::Proceed);
    let events = numbered(vec![wu_check(repo, WU, &["fp-a"])]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
    // 修復を挟まない同じ集合の再評価（再 review・再統合）は往復ではない。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-a"]),
        wu_check(repo, WU, &["fp-a"]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
    // 今の評価の後に積まれた修復（今の失敗に対する retry）は判定を変えない。
    let events = numbered(vec![wu_check(repo, WU, &["fp-a"]), wu_retry(WU)]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
}

/// 不変条件: [`judge`] の fingerprints は評価の並び・重複に依らず昇順・重複なしで、受信箱の鍵の hash
/// （`task_core::health_repeat::fingerprint_set_sha256`。`Event::health_findings_repeated` が求める唯一の実装）も同じ値に
/// なる。task-core の試験は hash の関数だけを見るので、judge の出力をそのまま渡したときの一致は検出できない。
#[test]
fn health_limits_repeat_set_sha_does_not_depend_on_order() {
    let repo = RepoId::new();
    let judged = |blocking: &[&str]| {
        let events = numbered(vec![
            wu_check(repo, WU, blocking),
            wu_retry(WU),
            wu_check(repo, WU, blocking),
        ]);
        judge(&events, HealthScope::WorkUnit(WU))
    };
    let fingerprints_of = |blocking: &[&str]| match judged(blocking) {
        RepeatVerdict::Repeat { fingerprints } => fingerprints,
        RepeatVerdict::Proceed => panic!("expected a repeat for {blocking:?}"),
    };
    let sorted = fingerprints_of(&["fp-a", "fp-b", "fp-c"]);
    assert_eq!(sorted, vec!["fp-a", "fp-b", "fp-c"]);
    let shuffled = fingerprints_of(&["fp-c", "fp-a", "fp-b"]);
    let duplicated = fingerprints_of(&["fp-b", "fp-c", "fp-a", "fp-b"]);
    assert_eq!(shuffled, sorted);
    assert_eq!(duplicated, sorted);
    let key = task_core::health_repeat::fingerprint_set_sha256;
    assert_eq!(key(&shuffled), key(&sorted));
    assert_eq!(key(&duplicated), key(&sorted));
    assert_eq!(
        key(&sorted),
        key(&["fp-c".into(), "fp-b".into(), "fp-a".into()])
    );
}

#[test]
fn health_limits_repeat_human_resume_resets() {
    let repo = RepoId::new();
    let looped = vec![
        wu_check(repo, WU, &["fp-a"]),
        wu_retry(WU),
        wu_check(repo, WU, &["fp-a"]),
    ];
    assert_eq!(
        judge(&numbered(looped.clone()), HealthScope::WorkUnit(WU)),
        repeat(&["fp-a"])
    );
    for resumed in [
        // 走っている task への人の comment（`Trigger::Interrupt`）。
        transitioned(Status::Running, Status::Ready, Trigger::Interrupt.reason()),
        // Blocked の質問への回答（Blocked への comment も回答になる）。
        transitioned(Status::Blocked, Status::Ready, Trigger::Answer.name()),
        // 終わった task の reopen。
        transitioned(Status::Failed, Status::Ready, Trigger::Reopen.name()),
    ] {
        let mut events = looped.clone();
        events.push(resumed.clone());
        // 再開の直後（まだ評価が無い）と、再開の後の最初の評価は Proceed。
        assert_eq!(
            judge(&numbered(events.clone()), HealthScope::WorkUnit(WU)),
            RepeatVerdict::Proceed,
            "{resumed:?}"
        );
        events.push(wu_check(repo, WU, &["fp-a"]));
        assert_eq!(
            judge(&numbered(events.clone()), HealthScope::WorkUnit(WU)),
            RepeatVerdict::Proceed,
            "{resumed:?}"
        );
        // 再開の後にまた修復しても同じ集合が残れば、新しい連なりの往復。
        events.push(wu_retry(WU));
        events.push(wu_check(repo, WU, &["fp-a"]));
        assert_eq!(
            judge(&numbered(events), HealthScope::WorkUnit(WU)),
            repeat(&["fp-a"]),
            "{resumed:?}"
        );
    }
    // 人でない遷移（review の不合格の再実行）は連なりを切らない。
    let mut events = looped;
    events.push(transitioned(
        Status::Reviewing,
        Status::Ready,
        Trigger::ReviewFail.name(),
    ));
    assert_eq!(
        judge(&numbered(events), HealthScope::WorkUnit(WU)),
        repeat(&["fp-a"])
    );
}

#[test]
fn health_limits_repeat_error_rows_are_outside() {
    let repo = RepoId::new();
    // 今の評価が Error なら往復にしない（Error の扱いは経路の D3.4）。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-a"]),
        wu_retry(WU),
        wu_check(repo, WU, &["fp-a"]),
        wu_retry(WU),
        wu_check_with(WU, evidence(repo, HealthVerdict::Error, &[])),
    ]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
    // 前の評価の Error は読み飛ばす: 測れなかった行を「a が消えた」と数えない（Pass と扱うと a の再出現で Repeat に
    // なる）。修復の前の評価は a の行で、今の集合 {a, c} とは違うので Proceed。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-a"]),
        wu_retry(WU),
        wu_check_with(WU, evidence(repo, HealthVerdict::Error, &[])),
        wu_retry(WU),
        wu_check(repo, WU, &["fp-a", "fp-c"]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
    // Error・NotApplicable を挟んでも、最後の修復の前の測れた評価と同じ集合が残れば往復。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-a"]),
        wu_retry(WU),
        wu_check_with(WU, evidence(repo, HealthVerdict::NotApplicable, &[])),
        wu_retry(WU),
        wu_check(repo, WU, &["fp-a"]),
    ]);
    assert_eq!(judge(&events, HealthScope::WorkUnit(WU)), repeat(&["fp-a"]));
    // PolicyOnly は repair に回らないので、今の評価なら同じ集合でも Proceed。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-p"]),
        wu_retry(WU),
        wu_check_with(WU, evidence(repo, HealthVerdict::PolicyOnly, &["fp-p"])),
    ]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
}

#[test]
fn health_limits_repeat_scopes_and_repos_are_separate() {
    let (repo, other_repo) = (RepoId::new(), RepoId::new());
    // 別の WU の retry は、この WU の修復ではない。別の WU・段・review の評価も混ぜない。
    let events = numbered(vec![
        wu_check(repo, WU, &["fp-a"]),
        wu_retry("other"),
        integration_check(repo, PHASE, &["fp-a"]),
        wu_check(repo, WU, &["fp-a"]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::WorkUnit(WU)),
        RepeatVerdict::Proceed
    );
    assert_eq!(
        judge(&events, HealthScope::Phase(PHASE)),
        RepeatVerdict::Proceed
    );
    assert_eq!(judge(&events, HealthScope::Review), RepeatVerdict::Proceed);
    // review の修復は段の統合の修復ではない（origin で分ける）。
    let events = numbered(vec![
        integration_check(repo, PHASE, &["fp-a"]),
        repair(RepairOrigin::Review),
        integration_check(repo, PHASE, &["fp-a"]),
    ]);
    assert_eq!(
        judge(&events, HealthScope::Phase(PHASE)),
        RepeatVerdict::Proceed
    );
    // review は repo ごとに比べる: 片方の repo の Error で他方の fingerprint が消えたとは数えず、往復した repo の
    // 集合だけを上げる。
    let mut events = review(vec![
        (repo, evidence(repo, HealthVerdict::Fail, &["fp-a"])),
        (
            other_repo,
            evidence(other_repo, HealthVerdict::Fail, &["fp-b"]),
        ),
    ]);
    events.push(repair(RepairOrigin::Review));
    events.extend(review(vec![
        (repo, evidence(repo, HealthVerdict::Error, &[])),
        (
            other_repo,
            evidence(other_repo, HealthVerdict::Fail, &["fp-b"]),
        ),
    ]));
    assert_eq!(
        judge(&numbered(events), HealthScope::Review),
        repeat(&["fp-b"])
    );
}
