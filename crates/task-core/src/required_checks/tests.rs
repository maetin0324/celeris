use super::*;

fn evidence(repo: RepoId, target: &str, reviewed: &str, results: &[(&str, bool)]) -> Event {
    Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: repo,
        target_sha: target.into(),
        reviewed_sha: reviewed.into(),
        definition_sha256: definition_digest(&[]),
        definition_repo_id: repo,
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
    assert_eq!(verify_required_checks(&events, repo, "t", "c", &[]), Ok(()));
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
    let reuse = reusable_passes(&events, repo, "t", "c");
    assert_eq!(reuse.into_iter().collect::<Vec<_>>(), vec!["a".to_string()]);
    assert!(reusable_passes(&events, repo, "t", "c2").is_empty());
    assert!(reusable_passes(&events, repo, "t2", "c").is_empty());
}

#[test]
fn definition_digest_depends_on_order_and_content() {
    let a = definition_digest(&["x".into(), "y".into()]);
    assert_eq!(a, definition_digest(&["x".into(), "y".into()]));
    assert_ne!(a, definition_digest(&["y".into(), "x".into()]));
    assert_ne!(a, definition_digest(&["x".into()]));
}
