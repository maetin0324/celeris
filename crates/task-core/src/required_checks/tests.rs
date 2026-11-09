use super::*;
use health::CheckerSource;
use sha2::{Digest, Sha256};

/// health を要らない照合（health の項目の無い既存の試験の形）。
fn verify(
    events: &[Event],
    repo: RepoId,
    target: &str,
    reviewed: &str,
    required: &[String],
) -> Result<(), String> {
    verify_required_checks(
        events,
        repo,
        target,
        reviewed,
        required,
        HealthRequirement::NotRequired,
    )
}

fn digest(cmds: &[String]) -> String {
    definition_digest(cmds, None)
}

fn evidence(repo: RepoId, target: &str, reviewed: &str, results: &[(&str, bool)]) -> Event {
    Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: repo,
        target_sha: target.into(),
        reviewed_sha: reviewed.into(),
        definition_sha256: digest(&required()),
        results: results
            .iter()
            .map(|(cmd, pass)| PhaseCheckResult {
                cmd: (*cmd).into(),
                pass: *pass,
                summary: String::new(),
                health: None,
            })
            .collect(),
    }
}

fn required() -> Vec<String> {
    vec!["a".into(), "b".into()]
}

#[test]
fn bound_passing_evidence_is_accepted() {
    let repo = RepoId::new();
    let events = [evidence(repo, "t", "c", &[("a", true), ("b", true)])];
    assert_eq!(verify(&events, repo, "t", "c", &required()), Ok(()));
    assert!(verify(&events, repo, "t", "c", &[]).is_err());
}

#[test]
fn missing_failed_or_incomplete_evidence_is_rejected() {
    let repo = RepoId::new();
    assert!(verify(&[], repo, "t", "c", &[]).is_err());
    let failed = [evidence(repo, "t", "c", &[("a", true), ("b", false)])];
    assert!(
        verify(&failed, repo, "t", "c", &required())
            .unwrap_err()
            .contains("b")
    );
    // acceptance などで必須検査が置き換わり、結果に無い。
    let replaced = [evidence(repo, "t", "c", &[("a", true), ("x", true)])];
    assert!(
        verify(&replaced, repo, "t", "c", &required())
            .unwrap_err()
            .contains("b")
    );
}

#[test]
fn changed_candidate_base_or_repo_invalidates_evidence() {
    let repo = RepoId::new();
    let events = [evidence(repo, "t", "c", &[("a", true), ("b", true)])];
    assert!(verify(&events, repo, "t", "c2", &required()).is_err());
    assert!(verify(&events, repo, "t2", "c", &required()).is_err());
    assert!(verify(&events, RepoId::new(), "t", "c", &required()).is_err());
}

#[test]
fn the_latest_bound_evidence_wins() {
    let repo = RepoId::new();
    let events = [
        evidence(repo, "t", "c", &[("a", true), ("b", true)]),
        evidence(repo, "t", "c", &[("a", true), ("b", false)]),
    ];
    assert!(verify(&events, repo, "t", "c", &required()).is_err());
}

#[test]
fn only_same_binding_passes_are_reusable() {
    let repo = RepoId::new();
    let events = [evidence(repo, "t", "c", &[("a", true), ("b", false)])];
    let reuse = reusable_passes(&events, repo, "t", "c", &required());
    assert_eq!(reuse.into_iter().collect::<Vec<_>>(), vec!["a".to_string()]);
    assert!(reusable_passes(&events, repo, "t", "c2", &required()).is_empty());
    assert!(reusable_passes(&events, repo, "t2", "c", &required()).is_empty());
}

#[test]
fn definition_digest_depends_on_order_and_content() {
    let a = digest(&["x".into(), "y".into()]);
    assert_eq!(a, digest(&["x".into(), "y".into()]));
    assert_ne!(a, digest(&["y".into(), "x".into()]));
    assert_ne!(a, digest(&["x".into()]));
}

#[test]
fn changed_definition_invalidates_evidence_and_reuse() {
    let repo = RepoId::new();
    let events = [evidence(repo, "t", "c", &[("a", true), ("b", true)])];
    let changed = vec!["b".into(), "a".into()];
    assert!(
        verify(&events, repo, "t", "c", &changed)
            .unwrap_err()
            .contains("指紋")
    );
    assert!(reusable_passes(&events, repo, "t", "c", &changed).is_empty());
}

#[test]
fn second_repo_requires_its_own_complete_passing_evidence() {
    let first = RepoId::new();
    let second = RepoId::new();
    let mut events = vec![evidence(first, "t", "c", &[("a", true), ("b", true)])];
    assert!(verify(&events, second, "t", "c", &required()).is_err());
    assert!(reusable_passes(&events, second, "t", "c", &required()).is_empty());
    events.push(evidence(second, "t", "c", &[("a", true)]));
    assert!(
        verify(&events, second, "t", "c", &required())
            .unwrap_err()
            .contains("b")
    );
    events.push(evidence(second, "t", "c", &[("a", true), ("b", false)]));
    assert!(
        verify(&events, second, "t", "c", &required())
            .unwrap_err()
            .contains("不合格")
    );
    events.push(evidence(second, "t", "c", &[("a", true), ("b", true)]));
    assert!(verify(&events, second, "t", "c", &required()).is_ok());
}

#[test]
fn ambiguous_multiline_commands_and_pre_v2_evidence_cannot_be_reused() {
    assert_ne!(digest(&["a\nb".into()]), digest(&["a".into(), "b".into()]));
    let repo = RepoId::new();
    let mut event = evidence(repo, "t", "c", &[("a", true), ("b", true)]);
    if let Event::RequiredChecksEvaluated {
        definition_sha256, ..
    } = &mut event
    {
        // The old newline digest could describe results copied from another repository.
        *definition_sha256 = format!("{:x}", Sha256::digest(b"a\nb\n"));
    }
    assert!(verify(&[event.clone()], repo, "t", "c", &required()).is_err());
    assert!(reusable_passes(&[event], repo, "t", "c", &required()).is_empty());
}

#[test]
fn only_confirmed_absence_means_no_required_checks() {
    assert_eq!(
        candidate_required_checks("c", |args| match args[0] {
            "ls-tree" => Ok(String::new()),
            _ => Err("missing".into()),
        }),
        Ok(vec![])
    );
    assert!(candidate_required_checks("c", |_| Err("git unavailable".into())).is_err());
    assert!(
        candidate_required_checks("c", |args| match args[0] {
            "show" => Err("read failed".into()),
            _ => Ok("exists".into()),
        })
        .unwrap_err()
        .contains("read failed")
    );
    assert!(
        candidate_required_checks("c", |args| match args[0] {
            "ls-tree" => Ok(crate::workspace_config::WORKSPACE_CONFIG_PATH.into()),
            _ => Err("object unavailable".into()),
        })
        .is_err()
    );
}

#[test]
fn malformed_definition_is_rejected() {
    assert!(
        candidate_required_checks("c", |_| Ok("[commands\n".into()))
            .unwrap_err()
            .contains("invalid required checks")
    );
    assert_eq!(
        candidate_required_checks("c", |_| Ok("[commands]\ncheck = ['a', 'b']".into())),
        Ok(vec!["a".into(), "b".into()])
    );
}

// ---- ADR 2026-11-08-health-enforcement-loop D2.4〜D2.7 ----

const TREE: &str = "tree-c";
const CHECKER: &str = "checker-1";

fn checker(sha: &str) -> CheckerId {
    CheckerId {
        source: CheckerSource::Base,
        sha256: sha.into(),
    }
}

/// `(repo, t, c)` の候補を `CHECKER` で評価した合格の証跡。
fn pass_health(repo: RepoId) -> HealthEvidence {
    HealthEvidence {
        repo_id: repo,
        base_sha: "t".into(),
        head_sha: "c".into(),
        head_tree: TREE.into(),
        policy_sha256: "policy-1".into(),
        checker: Some(checker(CHECKER)),
        report_sha256: "report-1".into(),
        tool_version: "1.0".into(),
        verdict: HealthVerdict::Pass,
        blocking: Vec::new(),
        reason: None,
        reused_from: None,
        reviewable: true,
    }
}

fn key(e: &HealthEvidence) -> HealthReuseKey<'_> {
    HealthReuseKey {
        repo_id: e.repo_id,
        base_sha: &e.base_sha,
        head_tree: &e.head_tree,
        policy_sha256: &e.policy_sha256,
        checker_sha256: e.checker.as_ref().map_or("", |c| c.sha256.as_str()),
    }
}

fn health_item(health: Option<HealthEvidence>, pass: bool) -> PhaseCheckResult {
    PhaseCheckResult {
        cmd: HEALTH_CHECK_ID.into(),
        pass,
        summary: String::new(),
        health: health.map(Box::new),
    }
}

/// `(repo, t, c)` に束縛した review の証跡（必須検査 a・b は合格、health の項目は `item`、定義指紋は `CHECKER`）。
fn reviewed_with(repo: RepoId, item: PhaseCheckResult) -> Event {
    let mut results: Vec<PhaseCheckResult> = ["a", "b"]
        .iter()
        .map(|cmd| PhaseCheckResult::new((*cmd).into(), true, String::new()))
        .collect();
    results.push(item);
    Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: repo,
        target_sha: "t".into(),
        reviewed_sha: "c".into(),
        definition_sha256: definition_digest(&required(), Some(&checker(CHECKER))),
        results,
    }
}

/// health の項目の `pass` を verdict と食い違わせる（`pass` が true でも verdict が決めることを確かめる）。
fn reviewed(repo: RepoId, health: &HealthEvidence) -> Event {
    reviewed_with(repo, health_item(Some(health.clone()), true))
}

fn wu_check(health: &HealthEvidence) -> Event {
    Event::WorkUnitCheckFinished {
        work_unit_id: "w".into(),
        key: "k".into(),
        run_id: "run".into(),
        index: 0,
        total: 1,
        cmd: HEALTH_CHECK_ID.into(),
        pass: true,
        exit: Some(0),
        timed_out: false,
        duration_ms: 1,
        health: Some(Box::new(health.clone())),
    }
}

fn integration_check(health: &HealthEvidence) -> Event {
    Event::IntegrationCheckFinished {
        work_unit_id: "w".into(),
        key: "k".into(),
        index: 0,
        total: 1,
        cmd: HEALTH_CHECK_ID.into(),
        pass: true,
        exit: Some(0),
        timed_out: false,
        duration_ms: 1,
        health: Some(Box::new(health.clone())),
    }
}

fn verify_applies(events: &[Event], repo: RepoId) -> Result<(), String> {
    verify_required_checks(
        events,
        repo,
        "t",
        "c",
        &required(),
        HealthRequirement::Applies {
            checker: &checker(CHECKER),
            reviewed_tree: TREE,
        },
    )
}

/// 合格の証跡が 3 つの正本のどれにあっても写せ、`changed` を当てた鍵では写せないこと。
fn assert_change_invalidates(change: impl Fn(&mut HealthEvidence)) {
    let repo = RepoId::new();
    let recorded = pass_health(repo);
    let mut wanted = recorded.clone();
    change(&mut wanted);
    for events in [
        [reviewed(repo, &recorded)],
        [wu_check(&recorded)],
        [integration_check(&recorded)],
    ] {
        assert_eq!(reusable_health(&events, &key(&recorded)), Some(&recorded));
        assert_eq!(reusable_health(&events, &key(&wanted)), None);
    }
}

/// 合格でない証跡を、どの正本にあっても合格にも再利用にもしないこと。後から記録されたら前の合格も写さない。
fn assert_not_passed_or_reused(change: impl Fn(&mut HealthEvidence)) {
    let repo = RepoId::new();
    let mut bad = pass_health(repo);
    change(&mut bad);
    let err = verify_applies(&[reviewed(repo, &bad)], repo).unwrap_err();
    assert!(err.contains("health"), "{err}");
    for event in [
        reviewed(repo, &bad),
        wu_check(&bad),
        integration_check(&bad),
    ] {
        assert_eq!(reusable_health(&[event], &key(&bad)), None);
    }
    let earlier = pass_health(repo);
    let events = [wu_check(&earlier), integration_check(&bad)];
    assert_eq!(reusable_health(&events, &key(&earlier)), None);
}

#[test]
fn health_evidence_definition_digest_is_v3_with_checker() {
    let cmds = required();
    assert_eq!(
        definition_digest(&cmds, None),
        health::domain_sha256("celeris.required-checks/3", ["2", "a", "b"])
    );
    assert_eq!(
        definition_digest(&cmds, Some(&checker(CHECKER))),
        health::domain_sha256(
            "celeris.required-checks/3",
            ["2", "a", "b", HEALTH_CHECK_ID, CHECKER]
        )
    );
    // cmd の数が先に入るので、health の項目を cmd に並べた定義とは別の指紋になる。
    let lookalike = vec![HEALTH_CHECK_ID.to_string(), CHECKER.to_string()];
    assert_ne!(
        definition_digest(&lookalike, None),
        definition_digest(&[], Some(&checker(CHECKER)))
    );
}

#[test]
fn health_evidence_definition_digest_changes_with_checker_version() {
    let cmds = required();
    let v1 = definition_digest(&cmds, Some(&checker(CHECKER)));
    assert_ne!(v1, definition_digest(&cmds, Some(&checker("checker-2"))));
    assert_ne!(v1, definition_digest(&cmds, None));
    // 同じ commit の検査器なら base でも同梱でも同じ指紋（D2.3）。
    let bundled = CheckerId {
        source: CheckerSource::Bundled,
        sha256: CHECKER.into(),
    };
    assert_eq!(v1, definition_digest(&cmds, Some(&bundled)));
    let repo = RepoId::new();
    let events = [reviewed(repo, &pass_health(repo))];
    assert_eq!(verify_applies(&events, repo), Ok(()));
    let newer = verify_required_checks(
        &events,
        repo,
        "t",
        "c",
        &required(),
        HealthRequirement::Applies {
            checker: &checker("checker-2"),
            reviewed_tree: TREE,
        },
    );
    assert!(newer.unwrap_err().contains("指紋"));
}

#[test]
fn health_evidence_v2_definition_digest_is_invalid() {
    let repo = RepoId::new();
    let mut event = evidence(repo, "t", "c", &[("a", true), ("b", true)]);
    if let Event::RequiredChecksEvaluated {
        definition_sha256, ..
    } = &mut event
    {
        *definition_sha256 = health::domain_sha256("celeris.required-checks/2", ["a", "b"]);
    }
    assert!(verify(&[event.clone()], repo, "t", "c", &required()).is_err());
    assert!(reusable_passes(&[event], repo, "t", "c", &required()).is_empty());
}

#[test]
fn health_evidence_same_tree_reuses_pass() {
    let repo = RepoId::new();
    let recorded = pass_health(repo);
    // 同じ tree の別の commit（単一の WU の統合の HEAD）でも、鍵に commit が無いので写せる。
    let other_commit = HealthEvidence {
        head_sha: "c2".into(),
        ..recorded.clone()
    };
    for events in [
        [reviewed(repo, &recorded)],
        [wu_check(&recorded)],
        [integration_check(&recorded)],
    ] {
        assert_eq!(
            reusable_health(&events, &key(&other_commit)),
            Some(&recorded)
        );
    }
    // 写した証跡（reused_from あり）も、同じ束縛の最新の合格として写せる。
    let copied = HealthEvidence {
        reused_from: Some("c".into()),
        ..other_commit
    };
    let events = [wu_check(&recorded), integration_check(&copied)];
    assert_eq!(reusable_health(&events, &key(&recorded)), Some(&copied));
}

#[test]
fn health_evidence_head_change_invalidates_pass() {
    assert_change_invalidates(|e| e.head_tree = "tree-c2".into());
    let repo = RepoId::new();
    let events = [reviewed(repo, &pass_health(repo))];
    let moved = verify_required_checks(
        &events,
        repo,
        "t",
        "c",
        &required(),
        HealthRequirement::Applies {
            checker: &checker(CHECKER),
            reviewed_tree: "tree-c2",
        },
    );
    assert!(moved.unwrap_err().contains("tree"));
    assert!(verify_applies(&events, repo).is_ok());
}

#[test]
fn health_evidence_base_change_invalidates_pass() {
    assert_change_invalidates(|e| e.base_sha = "t2".into());
}

#[test]
fn health_evidence_policy_change_invalidates_pass() {
    assert_change_invalidates(|e| e.policy_sha256 = "policy-2".into());
}

#[test]
fn health_evidence_checker_change_invalidates_pass() {
    assert_change_invalidates(|e| e.checker = Some(checker("checker-2")));
    // 定義指紋が今の検査器でも、health の項目が別の検査器の証跡なら合格にしない。
    let repo = RepoId::new();
    let stale = HealthEvidence {
        checker: Some(checker("checker-0")),
        ..pass_health(repo)
    };
    let err = verify_applies(&[reviewed(repo, &stale)], repo).unwrap_err();
    assert!(err.contains("検査器"), "{err}");
}

#[test]
fn health_evidence_repo_change_invalidates_pass() {
    assert_change_invalidates(|e| e.repo_id = RepoId::new());
    let repo = RepoId::new();
    let foreign = pass_health(RepoId::new());
    let err = verify_applies(&[reviewed(repo, &foreign)], repo).unwrap_err();
    assert!(err.contains("repo"), "{err}");
}

#[test]
fn health_evidence_report_digest_change_invalidates_pass() {
    let repo = RepoId::new();
    let first = pass_health(repo);
    let second = HealthEvidence {
        report_sha256: "report-2".into(),
        ..first.clone()
    };
    // 同じ束縛で後から別の report の証跡が記録されたら、前の合格は返さない。
    let events = [wu_check(&first), integration_check(&second)];
    assert_eq!(reusable_health(&events, &key(&first)), Some(&second));
    let failed = HealthEvidence {
        verdict: HealthVerdict::Fail,
        ..second.clone()
    };
    let events = [wu_check(&first), reviewed(repo, &failed)];
    assert_eq!(reusable_health(&events, &key(&first)), None);
    // report の無い証跡は写さず、delivery の証跡にもしない。
    let no_report = HealthEvidence {
        report_sha256: String::new(),
        ..first.clone()
    };
    let events = [wu_check(&first), wu_check(&no_report)];
    assert_eq!(reusable_health(&events, &key(&first)), None);
    let err = verify_applies(&[reviewed(repo, &no_report)], repo).unwrap_err();
    assert!(err.contains("report"), "{err}");
}

#[test]
fn health_evidence_fail_is_neither_passed_nor_reused() {
    assert_not_passed_or_reused(|e| {
        e.verdict = HealthVerdict::Fail;
        e.blocking = vec!["size.responsibility-growth:abc".into()];
    });
}

#[test]
fn health_evidence_error_is_neither_passed_nor_reused() {
    assert_not_passed_or_reused(|e| {
        e.verdict = HealthVerdict::Error;
        e.reason = Some("exit 2".into());
    });
}

#[test]
fn health_evidence_unknown_is_neither_passed_nor_reused() {
    // report/1 の rule の unknown は exit 2 で Error になる。
    assert_not_passed_or_reused(|e| {
        e.verdict = HealthVerdict::Error;
        e.blocking = vec!["layers.forbidden-import:unknown".into()];
        e.reason = Some("unknown".into());
    });
}

#[test]
fn health_evidence_unsupported_is_neither_passed_nor_reused() {
    // report/1 の rule の unsupported（必要な外部 tool が無い）も exit 2 で Error になる。
    assert_not_passed_or_reused(|e| {
        e.verdict = HealthVerdict::Error;
        e.reason = Some("unsupported".into());
    });
}

#[test]
fn health_evidence_policy_only_is_not_passed_without_bound_approval() {
    assert_not_passed_or_reused(|e| e.verdict = HealthVerdict::PolicyOnly);
    let repo = RepoId::new();
    let policy_only = HealthEvidence {
        verdict: HealthVerdict::PolicyOnly,
        ..pass_health(repo)
    };
    let err = verify_applies(&[reviewed(repo, &policy_only)], repo).unwrap_err();
    assert!(err.contains("承認"), "{err}");
}

#[test]
fn health_evidence_dirty_report_is_not_reused_or_delivered() {
    assert_not_passed_or_reused(|e| e.reviewable = false);
}

#[test]
fn health_evidence_phase_integrated_health_ignored() {
    let repo = RepoId::new();
    let recorded = pass_health(repo);
    let events = [Event::PhaseIntegrated {
        phase: "p".into(),
        work_unit_id: "w".into(),
        merged: Vec::new(),
        head: "c".into(),
        checks: vec![health_item(Some(recorded.clone()), true)],
    }];
    assert_eq!(reusable_health(&events, &key(&recorded)), None);
    assert!(verify_applies(&events, repo).is_err());
    // 正本（IntegrationCheckFinished）の後の写しの不合格も、正本の判定を変えない。
    let failed = HealthEvidence {
        verdict: HealthVerdict::Fail,
        ..recorded.clone()
    };
    let events = [
        integration_check(&recorded),
        Event::PhaseIntegrated {
            phase: "p".into(),
            work_unit_id: "w".into(),
            merged: Vec::new(),
            head: "c".into(),
            checks: vec![health_item(Some(failed), false)],
        },
    ];
    assert_eq!(reusable_health(&events, &key(&recorded)), Some(&recorded));
}

#[test]
fn health_evidence_applies_requires_health_item() {
    let repo = RepoId::new();
    let without_item = Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: repo,
        target_sha: "t".into(),
        reviewed_sha: "c".into(),
        definition_sha256: definition_digest(&required(), Some(&checker(CHECKER))),
        results: ["a", "b"]
            .iter()
            .map(|cmd| PhaseCheckResult::new((*cmd).into(), true, String::new()))
            .collect(),
    };
    let err = verify_applies(&[without_item], repo).unwrap_err();
    assert!(err.contains(HEALTH_CHECK_ID), "{err}");
    let empty_item = reviewed_with(repo, health_item(None, true));
    assert!(verify_applies(&[empty_item], repo).is_err());
    let events = [reviewed(repo, &pass_health(repo))];
    assert_eq!(verify_applies(&events, repo), Ok(()));
    // health を要らない照合は、検査器の版の入った指紋の証跡を受け取らない。
    assert!(verify(&events, repo, "t", "c", &required()).is_err());
}

#[test]
fn health_evidence_reusable_passes_never_copies_health() {
    let repo = RepoId::new();
    let event = Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: repo,
        target_sha: "t".into(),
        reviewed_sha: "c".into(),
        definition_sha256: digest(&required()),
        results: vec![
            PhaseCheckResult::new("a".into(), true, String::new()),
            health_item(Some(pass_health(repo)), true),
        ],
    };
    let reuse = reusable_passes(&[event], repo, "t", "c", &required());
    assert_eq!(reuse.into_iter().collect::<Vec<_>>(), vec!["a".to_string()]);
}
