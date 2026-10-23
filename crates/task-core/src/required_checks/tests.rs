use super::*;

fn evidence(repo: RepoId, target: &str, reviewed: &str, results: &[(&str, bool)]) -> Event {
    Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: repo,
        target_sha: target.into(),
        reviewed_sha: reviewed.into(),
        definition_sha256: definition_digest(&required()),
        results: results
            .iter()
            .map(|(cmd, pass)| PhaseCheckResult {
                cmd: (*cmd).into(),
                pass: *pass,
                summary: String::new(),
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
    assert_eq!(
        verify_required_checks(&events, repo, "t", "c", &required()),
        Ok(())
    );
    assert!(verify_required_checks(&events, repo, "t", "c", &[]).is_err());
}

#[test]
fn missing_failed_or_incomplete_evidence_is_rejected() {
    let repo = RepoId::new();
    assert!(verify_required_checks(&[], repo, "t", "c", &[]).is_err());
    let failed = [evidence(repo, "t", "c", &[("a", true), ("b", false)])];
    assert!(
        verify_required_checks(&failed, repo, "t", "c", &required())
            .unwrap_err()
            .contains("b")
    );
    // acceptance などで必須検査が置き換わり、結果に無い。
    let replaced = [evidence(repo, "t", "c", &[("a", true), ("x", true)])];
    assert!(
        verify_required_checks(&replaced, repo, "t", "c", &required())
            .unwrap_err()
            .contains("b")
    );
}

#[test]
fn changed_candidate_base_or_repo_invalidates_evidence() {
    let repo = RepoId::new();
    let events = [evidence(repo, "t", "c", &[("a", true), ("b", true)])];
    assert!(verify_required_checks(&events, repo, "t", "c2", &required()).is_err());
    assert!(verify_required_checks(&events, repo, "t2", "c", &required()).is_err());
    assert!(verify_required_checks(&events, RepoId::new(), "t", "c", &required()).is_err());
}

#[test]
fn the_latest_bound_evidence_wins() {
    let repo = RepoId::new();
    let events = [
        evidence(repo, "t", "c", &[("a", true), ("b", true)]),
        evidence(repo, "t", "c", &[("a", true), ("b", false)]),
    ];
    assert!(verify_required_checks(&events, repo, "t", "c", &required()).is_err());
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
    let a = definition_digest(&["x".into(), "y".into()]);
    assert_eq!(a, definition_digest(&["x".into(), "y".into()]));
    assert_ne!(a, definition_digest(&["y".into(), "x".into()]));
    assert_ne!(a, definition_digest(&["x".into()]));
}

#[test]
fn changed_definition_invalidates_evidence_and_reuse() {
    let repo = RepoId::new();
    let events = [evidence(repo, "t", "c", &[("a", true), ("b", true)])];
    let changed = vec!["b".into(), "a".into()];
    assert!(
        verify_required_checks(&events, repo, "t", "c", &changed)
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
    assert!(verify_required_checks(&events, second, "t", "c", &required()).is_err());
    assert!(reusable_passes(&events, second, "t", "c", &required()).is_empty());
    events.push(evidence(second, "t", "c", &[("a", true)]));
    assert!(
        verify_required_checks(&events, second, "t", "c", &required())
            .unwrap_err()
            .contains("b")
    );
    events.push(evidence(second, "t", "c", &[("a", true), ("b", false)]));
    assert!(
        verify_required_checks(&events, second, "t", "c", &required())
            .unwrap_err()
            .contains("不合格")
    );
    events.push(evidence(second, "t", "c", &[("a", true), ("b", true)]));
    assert!(verify_required_checks(&events, second, "t", "c", &required()).is_ok());
}

#[test]
fn ambiguous_multiline_commands_and_pre_v2_evidence_cannot_be_reused() {
    assert_ne!(
        definition_digest(&["a\nb".into()]),
        definition_digest(&["a".into(), "b".into()])
    );
    let repo = RepoId::new();
    let mut event = evidence(repo, "t", "c", &[("a", true), ("b", true)]);
    if let Event::RequiredChecksEvaluated {
        definition_sha256, ..
    } = &mut event
    {
        // The old newline digest could describe results copied from another repository.
        *definition_sha256 = format!("{:x}", Sha256::digest(b"a\nb\n"));
    }
    assert!(verify_required_checks(&[event.clone()], repo, "t", "c", &required()).is_err());
    assert!(reusable_passes(&[event], repo, "t", "c", &required()).is_empty());
}
