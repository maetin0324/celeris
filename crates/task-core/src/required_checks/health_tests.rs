use super::*;
use crate::{Event, PhaseCheckResult};
use std::collections::BTreeMap;

const BASE: &str = "b0";
const TREE: &str = "1111111111111111111111111111111111111111";
const SCRIPT_BLOB: &str = "2222222222222222222222222222222222222222";
const SIDE_BLOB: &str = "3333333333333333333333333333333333333333";

/// git の object を持つ偽の repo（`objects` は base の path → object id）。呼ばれた git の引数を `calls` に残す。
#[derive(Default)]
struct FakeRepo {
    not_a_repo: bool,
    git_broken: bool,
    ls_tree_fails: bool,
    /// `cat-file -e`・`rev-parse` が失敗するが `ls-tree` には出る path。
    unreadable: Vec<String>,
    objects: BTreeMap<String, String>,
    calls: Vec<Vec<String>>,
}

impl FakeRepo {
    fn with_checker() -> Self {
        let mut repo = FakeRepo::default();
        repo.put(CHECKER_DIR, TREE);
        repo.put(CHECKER_SCRIPT, SCRIPT_BLOB);
        repo.put(SOURCE_SIZE_REPORT, SIDE_BLOB);
        repo
    }

    fn put(&mut self, path: &str, id: &str) {
        self.objects.insert(path.to_string(), id.to_string());
    }

    fn readable(&self, object: &str) -> Option<&String> {
        let path = object.strip_prefix(BASE)?.strip_prefix(':')?;
        if self.unreadable.iter().any(|p| p == path) {
            return None;
        }
        self.objects.get(path)
    }

    fn git(&mut self, args: &[&str]) -> Result<String, String> {
        self.calls
            .push(args.iter().map(|arg| (*arg).to_string()).collect());
        if self.git_broken {
            return Err("git: command not found".into());
        }
        match args {
            ["--version"] => Ok("git version 2.47.0\n".into()),
            ["rev-parse", "--show-toplevel"] if self.not_a_repo => {
                Err("fatal: not a git repository".into())
            }
            ["rev-parse", "--show-toplevel"] => Ok("/work/repo\n".into()),
            ["cat-file", "-e", object] => self
                .readable(object)
                .map(|_| String::new())
                .ok_or_else(|| "fatal: path does not exist".into()),
            ["ls-tree", "--name-only", base, "--", path] => {
                if self.ls_tree_fails || *base != BASE {
                    return Err("fatal: not a tree object".into());
                }
                Ok(if self.objects.contains_key(*path) {
                    format!("{path}\n")
                } else {
                    String::new()
                })
            }
            ["rev-parse", "--verify", object] => self
                .readable(object)
                .map(|id| format!("{id}\n"))
                .ok_or_else(|| "fatal: Needed a single revision".into()),
            _ => Err(format!("unexpected git {args:?}")),
        }
    }
}

fn facts_of(repo: &mut FakeRepo, bundled: Option<&CheckerId>) -> HealthRepoFacts {
    repo_facts(true, BASE, bundled, |args| repo.git(args))
}

fn bundled() -> CheckerId {
    CheckerId {
        source: CheckerSource::Bundled,
        sha256: checker_sha256("4444444444444444444444444444444444444444", Some(SIDE_BLOB)),
    }
}

fn base_id(tree: &str, side: Option<&str>) -> CheckerId {
    CheckerId {
        source: CheckerSource::Base,
        sha256: checker_sha256(tree, side),
    }
}

fn unavailable_reason(applicability: &HealthApplicability) -> String {
    match applicability {
        HealthApplicability::Unavailable { reason } => reason.to_string(),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[test]
fn health_evidence_applicability_ignores_project_and_repo_name() {
    // The facts have no field for a project, a repo name, self_improve, selfdeploy or the task mode: this
    // destructuring stops compiling if such a field is added.
    let HealthRepoFacts {
        git,
        integrated_by_celeris,
        base_checker,
        bundled_checker,
        unreadable,
    } = HealthRepoFacts {
        git: true,
        integrated_by_celeris: true,
        base_checker: None,
        bundled_checker: None,
        unreadable: None,
    };
    assert!(git && integrated_by_celeris);
    assert!(base_checker.is_none() && bundled_checker.is_none() && unreadable.is_none());

    // A repo carrying Celeris' own profile and a generic repo with the same checker get the same answer.
    let mut celeris = FakeRepo::with_checker();
    celeris.put(PROFILES_DIR, "5555555555555555555555555555555555555555");
    celeris.put(
        "scripts/dev/repo-health/profiles/celeris.toml",
        "6666666666666666666666666666666666666666",
    );
    let mut generic = FakeRepo::with_checker();
    let a = applicability(&facts_of(&mut celeris, None));
    let b = applicability(&facts_of(&mut generic, None));
    assert_eq!(a, b);
    assert_eq!(
        a,
        HealthApplicability::Applies {
            checker: base_id(TREE, Some(SIDE_BLOB)),
        }
    );
    // repo_facts asks git only about the base and the fixed checker paths (no name, no profile).
    let mut allowed: Vec<String> = [
        "rev-parse",
        "--show-toplevel",
        "--version",
        "cat-file",
        "-e",
        "ls-tree",
        "--name-only",
        "--",
        "--verify",
        BASE,
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    for path in [CHECKER_DIR, CHECKER_SCRIPT, SOURCE_SIZE_REPORT] {
        allowed.push(path.to_string());
        allowed.push(format!("{BASE}:{path}"));
    }
    for arg in celeris.calls.iter().chain(&generic.calls).flatten() {
        assert!(allowed.contains(arg), "unexpected git argument {arg:?}");
    }
    assert_eq!(celeris.calls, generic.calls);
}

#[test]
fn health_evidence_non_git_repo_is_not_applicable() {
    let mut repo = FakeRepo {
        not_a_repo: true,
        ..FakeRepo::default()
    };
    let facts = facts_of(&mut repo, Some(&bundled()));
    assert!(!facts.git && facts.unreadable.is_none());
    let result = applicability(&facts);
    assert_eq!(
        result,
        HealthApplicability::NotApplicable {
            reason: NotApplicableReason::NotAGitRepo,
        }
    );
    assert_eq!(result.reason().as_deref(), Some("not_a_git_repo"));
    // The base is not looked at once the directory is known not to be a repository.
    assert!(repo.calls.iter().all(|c| c[0] != "cat-file"));
}

#[test]
fn health_evidence_remote_workspace_is_not_applicable() {
    let mut called = 0;
    let facts = repo_facts(false, BASE, Some(&bundled()), |_| {
        called += 1;
        Ok(String::new())
    });
    assert_eq!(
        called, 0,
        "a remote workspace has no local tree to ask git about"
    );
    let result = applicability(&facts);
    assert_eq!(
        result,
        HealthApplicability::NotApplicable {
            reason: NotApplicableReason::RemoteWorkspace,
        }
    );
    assert_eq!(result.reason().as_deref(), Some("remote_workspace"));
    // A remote workspace stays out even if its facts claim a checker.
    let claimed = HealthRepoFacts {
        git: true,
        integrated_by_celeris: false,
        base_checker: Some(base_id(TREE, None)),
        bundled_checker: Some(bundled()),
        unreadable: None,
    };
    assert_eq!(applicability(&claimed), result);
}

#[test]
fn health_evidence_bundled_checker_applies_when_the_base_has_none() {
    let mut repo = FakeRepo::default();
    let facts = facts_of(&mut repo, Some(&bundled()));
    assert_eq!(facts.base_checker, None);
    assert_eq!(
        applicability(&facts),
        HealthApplicability::Applies { checker: bundled() }
    );
}

#[test]
fn health_evidence_base_checker_wins_over_bundled() {
    let mut repo = FakeRepo::with_checker();
    let result = applicability(&facts_of(&mut repo, Some(&bundled())));
    assert_eq!(
        result,
        HealthApplicability::Applies {
            checker: base_id(TREE, Some(SIDE_BLOB)),
        }
    );
    assert_eq!(result.reason(), None);
}

#[test]
fn health_evidence_no_checker_anywhere_is_checker_missing() {
    let mut repo = FakeRepo::default();
    let result = applicability(&facts_of(&mut repo, None));
    assert_eq!(
        result,
        HealthApplicability::Unavailable {
            reason: UnavailableReason::CheckerMissing,
        }
    );
    assert_eq!(result.reason().as_deref(), Some("checker_missing"));
}

#[test]
fn health_evidence_unreadable_facts_are_unavailable() {
    // Every case has a bundled checker: an unreadable base must not fall back to it (the base would win, J3).
    let cases: Vec<(&str, FakeRepo)> = vec![
        (
            "git cannot run",
            FakeRepo {
                git_broken: true,
                ..FakeRepo::default()
            },
        ),
        (
            "the base cannot be listed",
            FakeRepo {
                ls_tree_fails: true,
                ..FakeRepo::default()
            },
        ),
        ("the listed checker cannot be read", {
            let mut repo = FakeRepo::with_checker();
            repo.unreadable.push(CHECKER_SCRIPT.into());
            repo
        }),
        ("the checker tree id cannot be read", {
            let mut repo = FakeRepo::with_checker();
            repo.unreadable.push(CHECKER_DIR.into());
            repo
        }),
        ("the checker tree id is not hex", {
            let mut repo = FakeRepo::with_checker();
            repo.put(CHECKER_DIR, "not-an-id");
            repo
        }),
        ("the side file cannot be read", {
            let mut repo = FakeRepo::with_checker();
            repo.unreadable.push(SOURCE_SIZE_REPORT.into());
            repo
        }),
    ];
    for (name, mut repo) in cases {
        let result = applicability(&facts_of(&mut repo, Some(&bundled())));
        let reason = unavailable_reason(&result);
        assert!(reason.starts_with("facts_unreadable:"), "{name}: {reason}");
    }
    // base_has_file answers false only for confirmed absence.
    let mut empty = FakeRepo::default();
    assert_eq!(
        base_has_file(BASE, CHECKER_SCRIPT, |args| empty.git(args)),
        Ok(false)
    );
    let mut broken = FakeRepo {
        ls_tree_fails: true,
        ..FakeRepo::default()
    };
    assert!(base_has_file(BASE, CHECKER_SCRIPT, |args| broken.git(args)).is_err());
}

#[test]
fn health_evidence_repo_without_profile_still_applies() {
    // The base has the checker but no profiles/: the report uses the built-in generic profile.
    let mut repo = FakeRepo::with_checker();
    assert!(!repo.objects.contains_key(PROFILES_DIR));
    assert!(matches!(
        applicability(&facts_of(&mut repo, None)),
        HealthApplicability::Applies {
            checker: CheckerId {
                source: CheckerSource::Base,
                ..
            }
        }
    ));
    // Neither checker nor profile on the base: the bundled checker applies.
    let mut bare = FakeRepo::default();
    assert!(matches!(
        applicability(&facts_of(&mut bare, Some(&bundled()))),
        HealthApplicability::Applies {
            checker: CheckerId {
                source: CheckerSource::Bundled,
                ..
            }
        }
    ));
    // The policy of a repo without profiles is still identified (as builtin) and differs from a profiled one.
    let generic = PolicyIdentity {
        policy_version: "1",
        policy_source: "builtin",
        profile_resolved: "generic",
        profile_source: "builtin",
        profiles_tree: None,
        base_blobs: &[],
    };
    assert_eq!(policy_sha256(&generic).len(), 64);
    assert_ne!(
        policy_sha256(&generic),
        policy_sha256(&PolicyIdentity {
            profiles_tree: Some("5555555555555555555555555555555555555555"),
            ..generic
        })
    );
}

#[test]
fn health_evidence_checker_sha256_is_one_formula_for_base_and_bundled() {
    // The base version read through git equals the value the daemon computes from CHECKER_TREE's ids.
    let mut repo = FakeRepo::with_checker();
    let from_base = base_checker(BASE, |args| repo.git(args)).unwrap().unwrap();
    assert_eq!(from_base.sha256, checker_sha256(TREE, Some(SIDE_BLOB)));
    assert_eq!(from_base.source, CheckerSource::Base);
    // A base without the side file is still a checker, with a different version.
    let mut no_side = FakeRepo::with_checker();
    no_side.objects.remove(SOURCE_SIZE_REPORT);
    let without = base_checker(BASE, |args| no_side.git(args))
        .unwrap()
        .unwrap();
    assert_eq!(without.sha256, checker_sha256(TREE, None));
    let base = checker_sha256(TREE, Some(SIDE_BLOB));
    assert_eq!(base.len(), 64);
    assert_ne!(base, without.sha256);
    assert_ne!(base, checker_sha256(SCRIPT_BLOB, Some(SIDE_BLOB)));
    assert_ne!(base, checker_sha256(TREE, Some(SCRIPT_BLOB)));
    // Fields are length-prefixed: moving bytes between them changes the value.
    assert_ne!(
        checker_sha256("ab", Some("c")),
        checker_sha256("a", Some("bc"))
    );
}

#[test]
fn health_evidence_policy_sha256_changes_with_each_input() {
    let blobs = [Some(SCRIPT_BLOB.to_string()), None];
    let base = PolicyIdentity {
        policy_version: "1",
        policy_source: "base",
        profile_resolved: "celeris",
        profile_source: "repo",
        profiles_tree: Some(TREE),
        base_blobs: &blobs,
    };
    let digest = policy_sha256(&base);
    assert_eq!(digest, policy_sha256(&base));
    let reordered = [None, Some(SCRIPT_BLOB.to_string())];
    let other_blob = [Some(SIDE_BLOB.to_string()), None];
    let variants = [
        PolicyIdentity {
            policy_version: "2",
            ..base
        },
        PolicyIdentity {
            policy_source: "bootstrap",
            ..base
        },
        PolicyIdentity {
            profile_resolved: "generic",
            ..base
        },
        PolicyIdentity {
            profile_source: "cli",
            ..base
        },
        PolicyIdentity {
            profiles_tree: None,
            ..base
        },
        PolicyIdentity {
            base_blobs: &reordered,
            ..base
        },
        PolicyIdentity {
            base_blobs: &other_blob,
            ..base
        },
        PolicyIdentity {
            base_blobs: &blobs[..1],
            ..base
        },
    ];
    for variant in &variants {
        assert_ne!(policy_sha256(variant), digest, "{variant:?}");
    }
}

fn evidence(verdict: HealthVerdict) -> HealthEvidence {
    HealthEvidence {
        repo_id: crate::RepoId::new(),
        base_sha: "b".repeat(40),
        head_sha: "c".repeat(40),
        head_tree: "d".repeat(40),
        policy_sha256: "e".repeat(64),
        checker: Some(bundled()),
        report_sha256: "f".repeat(64),
        tool_version: "1.0".into(),
        verdict,
        blocking: vec!["size.responsibility-growth:abc".into()],
        reason: None,
        reused_from: None,
        reviewable: true,
    }
}

#[test]
fn health_evidence_old_event_json_without_health_reads() {
    let old = [
        r#"{"type":"work_unit_check_finished","work_unit_id":"w","key":"k","run_id":"r","index":0,"total":1,"cmd":"true","pass":true,"exit":0,"timed_out":false,"duration_ms":5}"#,
        r#"{"type":"integration_check_finished","work_unit_id":"w","key":"integrate-p1","index":0,"total":1,"cmd":"true","pass":false,"timed_out":true,"duration_ms":5}"#,
        r#"{"type":"required_checks_evaluated","review_run":"r","repo_id":"01M6YJF7HJ9J0EQHRM5SN5TWV4","target_sha":"t","reviewed_sha":"c","definition_sha256":"d","results":[{"cmd":"a","pass":true,"summary":""}]}"#,
        r#"{"type":"phase_integrated","phase":"p","work_unit_id":"w","merged":[],"head":"h","checks":[{"cmd":"a","pass":true,"summary":"ok"}]}"#,
    ];
    for json in old {
        let event: Event = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
        match &event {
            Event::WorkUnitCheckFinished { health, .. }
            | Event::IntegrationCheckFinished { health, .. } => assert_eq!(*health, None),
            Event::RequiredChecksEvaluated { results, .. } => {
                assert!(results.iter().all(|r| r.health.is_none()))
            }
            Event::PhaseIntegrated { checks, .. } => {
                assert!(checks.iter().all(|r| r.health.is_none()))
            }
            other => panic!("unexpected {other:?}"),
        }
        // None is not written back: the stored form of an old event does not change.
        let original: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&event).unwrap(), original);
    }
}

#[test]
fn health_evidence_round_trips_in_check_events() {
    let mut evidence = evidence(HealthVerdict::PolicyOnly);
    evidence.reused_from = Some("a".repeat(40));
    let review = Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: evidence.repo_id,
        target_sha: evidence.base_sha.clone(),
        reviewed_sha: evidence.head_sha.clone(),
        definition_sha256: "d".into(),
        results: vec![PhaseCheckResult {
            cmd: HEALTH_CHECK_ID.into(),
            pass: false,
            summary: String::new(),
            health: Some(Box::new(evidence.clone())),
        }],
    };
    let value = serde_json::to_value(&review).unwrap();
    let health = &value["results"][0]["health"];
    assert_eq!(health["verdict"], "policy_only");
    assert_eq!(health["checker"]["source"], "bundled");
    assert_eq!(
        serde_json::from_value::<Event>(value.clone()).unwrap(),
        review
    );
    let wu = Event::WorkUnitCheckFinished {
        work_unit_id: "w".into(),
        key: "k".into(),
        run_id: "r".into(),
        index: 0,
        total: 1,
        cmd: HEALTH_CHECK_ID.into(),
        pass: false,
        exit: Some(2),
        timed_out: false,
        duration_ms: 1,
        health: Some(Box::new(HealthEvidence {
            checker: None,
            report_sha256: String::new(),
            tool_version: String::new(),
            blocking: Vec::new(),
            verdict: HealthVerdict::Error,
            reason: Some("checker_missing".into()),
            ..evidence
        })),
    };
    let value = serde_json::to_value(&wu).unwrap();
    // Empty optional parts are omitted, the binding and the verdict stay.
    assert!(value["health"].get("checker").is_none());
    assert!(value["health"].get("blocking").is_none());
    assert_eq!(value["health"]["verdict"], "error");
    assert_eq!(serde_json::from_value::<Event>(value).unwrap(), wu);
}
