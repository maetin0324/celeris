//! ADR 2026-11-08-health-enforcement-loop D1.2・D1.3・D3.1〜D3.4: 経路共通の入口 [`health_gate`] の試験（試験名は
//! `health_enforce_entry_`）。`gate_tests` の子 module で、同じ stub の検査器（base に commit した `repo_health.py` と
//! `health/engine.py`）を使う。外部ネットワークに出ず、LLM を呼ばない。
//!
//! 既存の `health_evidence_gate_*` は `run_health_check` の証跡の値（D2.3）を見る。これらは経路が見る分類
//! （[`HealthGateOutcome`]）と扱い（[`HealthGateDecision`]）、適用判定から評価までを 1 回で通す入口の振る舞いを見る。
//! 既存の試験は入口を通らないので、分類の取り違え（Error・Unavailable を合格や repair にする）を検出できない。

use std::path::Path;

use serde_json::json;
use task_core::RepoId;
use task_core::required_checks::health::{
    CHECKER_DIR, CheckerSource, HealthVerdict, NotApplicableReason, SOURCE_SIZE_REPORT,
    UnavailableReason,
};

use super::*;

fn request<'a>(dir: &'a Path, base: &'a str, head: &'a str) -> HealthGateRequest<'a> {
    HealthGateRequest {
        repo_id: RepoId::new(),
        worktree: dir,
        base,
        head,
        integrated_by_celeris: true,
        bundled: None,
        timeout: TIMEOUT,
    }
}

fn evaluated(outcome: &HealthGateOutcome) -> &HealthRun {
    match outcome {
        HealthGateOutcome::Evaluated(run) => run,
        other => panic!("not evaluated: {other:?}"),
    }
}

/// 不変条件: base に検査器のある repo の exit 0 は、入口が base の検査器で評価した Pass で、health の項目を足す。
#[test]
fn health_enforce_entry_pass() {
    let text = report("pass", 0, vec![], base_policy(false), vec![]);
    let (dir, base) = repo_with_checker(&stub_json(0, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");

    let outcome = health_gate(&request(dir.path(), &base, &head));

    let run = evaluated(&outcome);
    assert_eq!(run.evidence.verdict, HealthVerdict::Pass, "{outcome:?}");
    assert_eq!(
        run.evidence.checker.as_ref().map(|c| c.source),
        Some(CheckerSource::Base)
    );
    assert!(run.failures.is_empty());
    assert_eq!(outcome.decision(), HealthGateDecision::Pass);
    assert_eq!(outcome.item(), Some(&run.evidence));
}

/// 不変条件: policy の file だけの変更は、G2 D6.2 の既存の判定（同じ検査器の `policy_review_files`）で PolicyOnly に
/// なり、合格（Pass）でも repair でもない。
#[test]
fn health_enforce_entry_policy_only() {
    let text = policy_report();
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&text)), true);
    let head = commit(dir.path(), &[(PROFILE, "[review]\nx = 1\n")], "policy");

    let outcome = health_gate(&request(dir.path(), &base, &head));

    assert_eq!(
        outcome.decision(),
        HealthGateDecision::PolicyOnly,
        "{outcome:?}"
    );
    assert_eq!(
        evaluated(&outcome).evidence.verdict,
        HealthVerdict::PolicyOnly
    );
    assert!(evaluated(&outcome).failures.is_empty());
    assert!(outcome.item().is_some());
}

/// 不変条件: 決定的な検査の fail 級の finding（error・new か worsened・例外なし）だけが Fail の repair になり、rule ごとに
/// reason `health:<rule_id>:<hash>[,…]` と evidence の path、bucket `health:<rule_id>` を持つ。warning・existing・
/// 例外つきの finding は repair の材料に入らない。
#[test]
fn health_enforce_entry_fail_routes_to_repair() {
    let mut deps = finding("deps.violation", 2, "error", "new", false);
    deps["evidence"] = json!([{"path": "src/b.rs"}, {"path": SRC}]);
    let findings = vec![
        finding("size.x", 1, "error", "worsened", false),
        deps,
        finding("deps.violation", 1, "error", "new", false),
        finding("size.x", 3, "warning", "new", false),
        finding("size.x", 4, "error", "existing", false),
        finding("size.x", 5, "error", "new", true),
    ];
    let rules = vec![
        rule("deps.violation", "fail", ""),
        rule("size.x", "fail", ""),
    ];
    let text = report("fail", 1, findings, base_policy(false), rules);
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");

    let outcome = health_gate(&request(dir.path(), &base, &head));

    let run = evaluated(&outcome);
    assert_eq!(run.evidence.verdict, HealthVerdict::Fail, "{outcome:?}");
    let fp = |rule: &str, n: usize| format!("{rule}:{n:016x}");
    assert_eq!(
        run.evidence.blocking,
        vec![
            fp("deps.violation", 1),
            fp("deps.violation", 2),
            fp("size.x", 1)
        ]
    );
    let HealthGateDecision::Repair(repairs) = outcome.decision() else {
        panic!("not repair: {outcome:?}");
    };
    assert_eq!(repairs.len(), 2, "{repairs:?}");
    assert_eq!(repairs[0].bucket(), "health:deps.violation");
    assert_eq!(
        repairs[0].reason,
        format!(
            "health:{},health:{}（evidence: src/b.rs, {SRC}）",
            fp("deps.violation", 1),
            fp("deps.violation", 2)
        )
    );
    assert!(repairs[0].reason.starts_with("health:deps.violation:"));
    assert_eq!(
        repairs[0].paths,
        vec!["src/b.rs".to_string(), SRC.to_string()]
    );
    assert_eq!(repairs[1].bucket(), "health:size.x");
    assert_eq!(
        repairs[1].reason,
        format!("health:{}（evidence: {SRC}）", fp("size.x", 1))
    );
}

/// 不変条件: [`health_repairs`] は入力の順に依らず rule_id の順・fingerprint の順にまとめ、path を重複なく並べる。
/// evidence の path が無い finding は reason に path の括弧を付けない。
#[test]
fn health_enforce_entry_repairs_group_by_rule() {
    let failure = |rule: &str, hash: &str, paths: &[&str]| HealthFailure {
        rule_id: rule.to_string(),
        fingerprint: format!("{rule}:{hash}"),
        paths: paths.iter().map(|p| p.to_string()).collect(),
    };
    let repairs = health_repairs(&[
        failure("size.x", "bb", &[]),
        failure("deps.violation", "ff", &["a.rs", "c.rs"]),
        failure("deps.violation", "0a", &["c.rs"]),
    ]);

    assert_eq!(repairs.len(), 2);
    assert_eq!(repairs[0].rule_id, "deps.violation");
    assert_eq!(
        repairs[0].reason,
        "health:deps.violation:0a,health:deps.violation:ff（evidence: a.rs, c.rs）"
    );
    assert_eq!(repairs[0].bucket(), "health:deps.violation");
    assert_eq!(repairs[1].reason, "health:size.x:bb");
    assert!(repairs[1].paths.is_empty());
    assert!(health_repairs(&[]).is_empty());
}

/// 不変条件: exit 2（error・unknown・unsupported）・report が無い・壊れた report・時間切れ・fail 級の finding の無い
/// exit 1 は Error で、合格にも repair にもならない（J4・D3.4）。health の項目は verdict Error として足す。
#[test]
fn health_enforce_entry_error_is_not_pass() {
    let exit2 = report(
        "error",
        2,
        vec![],
        base_policy(false),
        vec![
            rule("size.x", "unsupported", "tool_missing:x"),
            rule("deps.cycle", "unknown", ""),
            rule("deps.violation", "error", "boom"),
        ],
    );
    let empty_fail = report(
        "fail",
        1,
        vec![finding("size.x", 1, "warning", "new", false)],
        base_policy(false),
        vec![rule("size.x", "fail", "")],
    );
    let cases = [
        (
            stub_json(2, Some(&exit2)),
            "unsupported:size.x(tool_missing:x)",
        ),
        (stub_json(2, Some(&exit2)), "unknown:deps.cycle"),
        (stub_json(2, Some(&exit2)), "error:deps.violation(boom)"),
        (stub_json(0, None), "report_missing:"),
        (stub_json(0, Some("not json")), "report_invalid:"),
        (stub_json(1, Some(&empty_fail)), "report_invalid:"),
        (
            json!({"exit": 0, "sleep": 30, "report": null}).to_string(),
            "checker_not_run:",
        ),
    ];
    for (stub, token) in cases {
        let (dir, base) = repo_with_checker(&stub, true);
        let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
        let mut req = request(dir.path(), &base, &head);
        if token == "checker_not_run:" {
            req.timeout = Duration::from_secs(3);
        }

        let outcome = health_gate(&req);

        let run = evaluated(&outcome);
        assert_eq!(
            run.evidence.verdict,
            HealthVerdict::Error,
            "{token}: {outcome:?}"
        );
        assert!(run.failures.is_empty(), "{token}");
        let HealthGateDecision::Error { reason } = outcome.decision() else {
            panic!("{token}: not error: {outcome:?}");
        };
        assert!(reason.contains(token), "{token}: {reason}");
        assert_eq!(
            outcome.item().map(|e| e.verdict),
            Some(HealthVerdict::Error),
            "{token}"
        );
    }
}

/// 不変条件: remote workspace と Git 管理でない作業場所は NotApplicable で、health の項目を足さず、理由を証跡に残す。
/// remote workspace では作業ツリーを読まない（存在しない path でも NotApplicable）。
#[test]
fn health_enforce_entry_not_applicable() {
    let missing = Path::new("/nonexistent/celeris-health-entry");
    let mut remote = request(missing, "main", "HEAD");
    remote.integrated_by_celeris = false;
    let plain = tempfile::tempdir().unwrap();
    write(plain.path(), "README.md", "not a repo\n");
    let cases = [
        (remote, NotApplicableReason::RemoteWorkspace),
        (
            request(plain.path(), "main", "HEAD"),
            NotApplicableReason::NotAGitRepo,
        ),
    ];
    for (req, want) in cases {
        let outcome = health_gate(&req);

        let HealthGateOutcome::NotApplicable { reason, evidence } = &outcome else {
            panic!("{want:?}: {outcome:?}");
        };
        assert_eq!(*reason, want);
        assert_eq!(evidence.verdict, HealthVerdict::NotApplicable);
        assert_eq!(evidence.reason.as_deref(), Some(want.as_str()));
        assert_eq!(evidence.checker, None);
        assert_eq!(outcome.item(), None);
        assert_eq!(outcome.decision(), HealthGateDecision::NotApplicable);
    }
}

/// 不変条件: 当てるべきだが評価できない repo（base にも同梱にも検査器が無い・事実を読めない）は Unavailable で、
/// health の項目を verdict Error として足し、合格にも repair にもしない（D1.1 の fail closed・J4）。
#[test]
fn health_enforce_entry_unavailable_is_not_pass() {
    let dir = init_repo();
    let base = sh_git(dir.path(), &["rev-parse", "HEAD"]);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let unknown_base = "0".repeat(40);
    let cases = [
        (base.as_str(), "checker_missing"),
        (unknown_base.as_str(), "facts_unreadable:"),
    ];
    for (base, token) in cases {
        let outcome = health_gate(&request(dir.path(), base, &head));

        let HealthGateOutcome::Unavailable { reason, evidence } = &outcome else {
            panic!("{token}: {outcome:?}");
        };
        assert!(reason.to_string().starts_with(token), "{token}: {reason}");
        if token == "checker_missing" {
            assert_eq!(*reason, UnavailableReason::CheckerMissing);
        }
        assert_eq!(evidence.verdict, HealthVerdict::Error);
        assert_eq!(evidence.checker, None);
        assert_eq!(outcome.item(), Some(evidence));
        assert!(
            matches!(outcome.decision(), HealthGateDecision::Error { ref reason } if reason.starts_with(token)),
            "{token}: {outcome:?}"
        );
    }
}

/// 不変条件: Celeris でない repo（base に検査器が無く、test 台本が check-health を呼ばない）では、入口は引数で渡された
/// 同梱の検査器を選び、Celeris の repo と同じ判定（fail 級の finding → repair）をする。同梱の検査器を渡さなければ
/// Unavailable（process の同梱の設定は読まない）。
#[test]
fn health_enforce_entry_non_celeris_repo_uses_bundled_checker() {
    let failing = report(
        "fail",
        1,
        vec![finding("deps.violation", 7, "error", "new", false)],
        json!({"version": "1", "source": "builtin", "changed": false, "files": []}),
        vec![rule("deps.violation", "fail", "")],
    );
    let stub = stub_json(1, Some(&failing));
    let (source, source_base) = repo_with_checker(&stub, false);
    let bundle_root = tempfile::tempdir().unwrap();
    let bundle = bundle_root.path().join("repo-health");
    for (path, body) in checker_files(&stub, false) {
        write(&bundle, path, &body);
    }
    let tree = sh_git(
        source.path(),
        &["rev-parse", &format!("{source_base}:{CHECKER_DIR}")],
    );
    let blob = sh_git(
        source.path(),
        &["rev-parse", &format!("{source_base}:{SOURCE_SIZE_REPORT}")],
    );
    write(
        &bundle,
        CHECKER_TREE_FILE,
        &format!("repo-health {tree}\nsource-size-report {blob}\n"),
    );
    let bundled = read_bundled_checker(&bundle).expect("bundled checker");

    let dir = init_repo();
    write(dir.path(), "Makefile", "test:\n\tcargo test\n");
    let base = commit(dir.path(), &[], "non-celeris base");
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");

    let without = health_gate(&request(dir.path(), &base, &head));
    assert!(
        matches!(
            without,
            HealthGateOutcome::Unavailable {
                reason: UnavailableReason::CheckerMissing,
                ..
            }
        ),
        "{without:?}"
    );

    let mut req = request(dir.path(), &base, &head);
    req.bundled = Some(&bundled);
    let outcome = health_gate(&req);

    let run = evaluated(&outcome);
    assert_eq!(
        run.evidence.checker,
        Some(bundled.id.clone()),
        "{outcome:?}"
    );
    assert_eq!(bundled.id.source, CheckerSource::Bundled);
    assert_eq!(run.evidence.verdict, HealthVerdict::Fail);
    let HealthGateDecision::Repair(repairs) = outcome.decision() else {
        panic!("not repair: {outcome:?}");
    };
    assert_eq!(repairs.len(), 1);
    assert_eq!(repairs[0].bucket(), "health:deps.violation");
    assert_eq!(
        repairs[0].reason,
        format!("health:deps.violation:{:016x}（evidence: {SRC}）", 7)
    );
}
