//! ADR-0084 P4-C: browser backend の適合 fixture・routing・fallback・同一 task 評価。
//!
//! I/O を持たない。backend を実際に走らせた結果（[`ConformanceResult`]）を受け取り、
//! どの能力を名乗ってよいかと、task をどの backend に送るかを決める。
//! 既定は既存の loop（ACP / 明示の Claude、ADR-0080 H7 の推奨）で、専用 backend
//! （browser-specialist）の採用は人の決定（H7）まで `enabled = false` の登録に留める。
//! 機密の能力（credential 注入・identity 復元）は P4-A / P4-B の適合が無ければ名乗れない。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Navigate,
    Snapshot,
    Click,
    Fill,
    Screenshot,
    Download,
    LiveView,
    /// 機密: P4-B の trusted injection を要する。
    CredentialInjection,
    /// 機密: P4-A の隔離を要する。
    IdentityRestore,
}

impl Capability {
    pub fn is_sensitive(self) -> bool {
        matches!(self, Self::CredentialInjection | Self::IdentityRestore)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// 既存の harness の loop に agent-browser を持たせる（ACP / Claude）。
    ExistingLoop,
    /// browser 専用の backend（browser-specialist 課の harness）。
    BrowserSpecialist,
}

/// 適合 fixture の 1 件。能力ごとに要る件が決まる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureCase {
    OpenAllowedOrigin,
    RefuseDeniedOrigin,
    SnapshotHasRefs,
    ClickByRef,
    FillByRef,
    ScreenshotArtifact,
    DownloadToArtifacts,
    LiveViewFrames,
    ResumeAfterCrash,
    /// P4-A の隔離の試験一式（`browser_isolation`）。
    IsolationSuite,
    /// P4-A の egress 負例一式。
    EgressNegativeSuite,
    /// P4-B の注入攻撃一式（`celeris_credentiald::injection`）。
    InjectionAttackSuite,
    /// 認証区間中の観測停止（H3）。
    AuthSectionObservationStop,
}

/// 能力を名乗るのに通る必要のある fixture。
pub fn required_cases(cap: Capability) -> &'static [FixtureCase] {
    use FixtureCase as F;
    match cap {
        Capability::Navigate => &[
            F::OpenAllowedOrigin,
            F::RefuseDeniedOrigin,
            F::ResumeAfterCrash,
        ],
        Capability::Snapshot => &[F::SnapshotHasRefs],
        Capability::Click => &[F::ClickByRef],
        Capability::Fill => &[F::FillByRef],
        Capability::Screenshot => &[F::ScreenshotArtifact],
        Capability::Download => &[F::DownloadToArtifacts],
        Capability::LiveView => &[F::LiveViewFrames],
        Capability::CredentialInjection => &[
            F::IsolationSuite,
            F::EgressNegativeSuite,
            F::InjectionAttackSuite,
            F::AuthSectionObservationStop,
        ],
        Capability::IdentityRestore => &[
            F::IsolationSuite,
            F::EgressNegativeSuite,
            F::AuthSectionObservationStop,
        ],
    }
}

/// backend を fixture で走らせた結果。`version` が変われば取り直す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConformanceResult {
    pub backend_id: String,
    pub version: String,
    pub passed: BTreeSet<FixtureCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDescriptor {
    pub id: String,
    pub kind: BackendKind,
    pub version: String,
    /// backend が名乗る能力（宣言）。
    pub declared: BTreeSet<Capability>,
    /// 人の決定（H7）で有効にされたか。既存の loop は既定で有効。
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConformanceGap {
    NoResult,
    StaleVersion {
        tested: String,
    },
    Missing {
        capability: Capability,
        cases: Vec<FixtureCase>,
    },
}

/// 宣言のうち、fixture で裏付けられた能力。機密の宣言が裏付けられなければ gap を返す
/// （機密の宣言を持つ backend は、それが通るまで routing に使わない）。
pub fn certify(
    backend: &BackendDescriptor,
    result: Option<&ConformanceResult>,
) -> Result<BTreeSet<Capability>, Vec<ConformanceGap>> {
    let Some(r) = result.filter(|r| r.backend_id == backend.id) else {
        return Err(vec![ConformanceGap::NoResult]);
    };
    if r.version != backend.version {
        return Err(vec![ConformanceGap::StaleVersion {
            tested: r.version.clone(),
        }]);
    }
    let mut gaps = Vec::new();
    let mut ok = BTreeSet::new();
    for cap in &backend.declared {
        let missing: Vec<FixtureCase> = required_cases(*cap)
            .iter()
            .copied()
            .filter(|c| !r.passed.contains(c))
            .collect();
        if missing.is_empty() {
            ok.insert(*cap);
        } else {
            gaps.push(ConformanceGap::Missing {
                capability: *cap,
                cases: missing,
            });
        }
    }
    if gaps.iter().any(
        |g| matches!(g, ConformanceGap::Missing { capability, .. } if capability.is_sensitive()),
    ) {
        return Err(gaps);
    }
    Ok(ok)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingRequest {
    pub required: BTreeSet<Capability>,
    /// task が明示した backend（ADR-0080 H7 の「明示 Claude」など）。
    pub explicit: Option<String>,
    /// 同じ task で前に失敗した backend（fallback で避ける）。
    pub failed: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub primary: String,
    /// 能力を失わない代替だけ（全部 `required` を満たす）。
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutingError {
    /// 要る能力を全部持つ有効な backend が無い。能力を落として代替しない。
    NoCapableBackend {
        missing: Vec<Capability>,
    },
    ExplicitUnavailable {
        backend: String,
    },
}

/// routing。順序: 明示 → browser-specialist → 既存の loop（同種内は id 順）。
/// 要る能力を 1 つでも欠く backend は primary にも fallback にもならない。
pub fn route(
    backends: &[BackendDescriptor],
    results: &BTreeMap<String, ConformanceResult>,
    req: &RoutingRequest,
) -> Result<RoutingDecision, RoutingError> {
    let mut capable: Vec<&BackendDescriptor> = backends
        .iter()
        .filter(|b| b.enabled && !req.failed.contains(&b.id))
        .filter(|b| {
            certify(b, results.get(&b.id))
                .map(|caps| req.required.is_subset(&caps))
                .unwrap_or(false)
        })
        .collect();
    capable.sort_by_key(|b| {
        (
            Some(&b.id) != req.explicit.as_ref(),
            b.kind != BackendKind::BrowserSpecialist,
            b.id.clone(),
        )
    });
    if let Some(explicit) = &req.explicit
        && capable.first().is_none_or(|b| &b.id != explicit)
    {
        return Err(RoutingError::ExplicitUnavailable {
            backend: explicit.clone(),
        });
    }
    let Some((first, rest)) = capable.split_first() else {
        let mut have = BTreeSet::new();
        for b in backends.iter().filter(|b| b.enabled) {
            if let Ok(c) = certify(b, results.get(&b.id)) {
                have.extend(c);
            }
        }
        return Err(RoutingError::NoCapableBackend {
            missing: req.required.difference(&have).copied().collect(),
        });
    };
    Ok(RoutingDecision {
        primary: first.id.clone(),
        fallbacks: rest.iter().map(|b| b.id.clone()).collect(),
    })
}

/// 同一 task を各 backend で走らせた結果（H7 の比較の材料）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SameTaskRun {
    pub backend_id: String,
    pub task_fixture: String,
    pub accepted: bool,
    pub policy_violations: u32,
    pub recoveries: u32,
    pub cost_usd: f64,
    pub wall_secs: u64,
}

/// 比較の順位。違反 0 を先に、受け入れ、少ない復旧、安い、速いの順。
/// 異なる task fixture の結果は混ぜない（`None`）。
pub fn rank_same_task(runs: &[SameTaskRun]) -> Option<Vec<String>> {
    let first = runs.first()?;
    if runs.iter().any(|r| r.task_fixture != first.task_fixture) {
        return None;
    }
    let mut v: Vec<&SameTaskRun> = runs.iter().collect();
    v.sort_by(|a, b| {
        (a.policy_violations > 0)
            .cmp(&(b.policy_violations > 0))
            .then(b.accepted.cmp(&a.accepted))
            .then(a.recoveries.cmp(&b.recoveries))
            .then(a.cost_usd.total_cmp(&b.cost_usd))
            .then(a.wall_secs.cmp(&b.wall_secs))
            .then(a.backend_id.cmp(&b.backend_id))
    });
    Some(v.into_iter().map(|r| r.backend_id.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: [Capability; 5] = [
        Capability::Navigate,
        Capability::Snapshot,
        Capability::Click,
        Capability::Fill,
        Capability::Screenshot,
    ];

    fn backend(id: &str, kind: BackendKind, caps: &[Capability]) -> BackendDescriptor {
        BackendDescriptor {
            id: id.into(),
            kind,
            version: "1".into(),
            declared: caps.iter().copied().collect(),
            enabled: true,
        }
    }

    fn passing(b: &BackendDescriptor) -> ConformanceResult {
        ConformanceResult {
            backend_id: b.id.clone(),
            version: b.version.clone(),
            passed: b
                .declared
                .iter()
                .flat_map(|c| required_cases(*c).iter().copied())
                .collect(),
        }
    }

    fn req(caps: &[Capability]) -> RoutingRequest {
        RoutingRequest {
            required: caps.iter().copied().collect(),
            explicit: None,
            failed: BTreeSet::new(),
        }
    }

    #[test]
    fn fixture_certifies_declared_capabilities() {
        let b = backend("acp", BackendKind::ExistingLoop, &BASIC);
        assert_eq!(certify(&b, Some(&passing(&b))).unwrap(), b.declared);
        assert_eq!(
            certify(&b, None).unwrap_err(),
            vec![ConformanceGap::NoResult]
        );
        let mut r = passing(&b);
        r.version = "0".into();
        assert!(matches!(
            certify(&b, Some(&r)).unwrap_err()[..],
            [ConformanceGap::StaleVersion { .. }]
        ));
        // 非機密の欠けはその能力だけを落とす
        let mut r = passing(&b);
        r.passed.remove(&FixtureCase::ClickByRef);
        let ok = certify(&b, Some(&r)).unwrap();
        assert!(!ok.contains(&Capability::Click));
        assert!(ok.contains(&Capability::Navigate));
    }

    #[test]
    fn sensitive_declaration_requires_p4a_and_p4b_conformance() {
        let mut caps = BASIC.to_vec();
        caps.push(Capability::CredentialInjection);
        let b = backend("spec", BackendKind::BrowserSpecialist, &caps);
        for missing in [
            FixtureCase::IsolationSuite,
            FixtureCase::EgressNegativeSuite,
            FixtureCase::InjectionAttackSuite,
            FixtureCase::AuthSectionObservationStop,
        ] {
            let mut r = passing(&b);
            r.passed.remove(&missing);
            // 機密の宣言が裏付けられない backend は丸ごと使わない
            let gaps = certify(&b, Some(&r)).unwrap_err();
            assert!(matches!(
                &gaps[..],
                [ConformanceGap::Missing {
                    capability: Capability::CredentialInjection,
                    ..
                }]
            ));
            let results = [(b.id.clone(), r)].into_iter().collect();
            assert!(
                route(
                    std::slice::from_ref(&b),
                    &results,
                    &req(&[Capability::Navigate])
                )
                .is_err()
            );
        }
        let rb = backend(
            "id",
            BackendKind::BrowserSpecialist,
            &[Capability::IdentityRestore],
        );
        let mut r = passing(&rb);
        r.passed.remove(&FixtureCase::IsolationSuite);
        assert!(certify(&rb, Some(&r)).is_err());
    }

    fn fleet() -> (Vec<BackendDescriptor>, BTreeMap<String, ConformanceResult>) {
        let mut full = BASIC.to_vec();
        full.push(Capability::LiveView);
        let bs = vec![
            backend("acp", BackendKind::ExistingLoop, &full),
            backend("claude", BackendKind::ExistingLoop, &BASIC),
            backend("specialist", BackendKind::BrowserSpecialist, &full),
        ];
        let rs = bs.iter().map(|b| (b.id.clone(), passing(b))).collect();
        (bs, rs)
    }

    #[test]
    fn routing_prefers_specialist_then_existing_loops_and_fallback_keeps_capabilities() {
        let (bs, rs) = fleet();
        let d = route(
            &bs,
            &rs,
            &req(&[Capability::Navigate, Capability::LiveView]),
        )
        .unwrap();
        assert_eq!(d.primary, "specialist");
        // claude は LiveView を持たないので fallback に入らない
        assert_eq!(d.fallbacks, vec!["acp".to_string()]);
        let d = route(&bs, &rs, &req(&[Capability::Navigate])).unwrap();
        assert_eq!(d.fallbacks, vec!["acp".to_string(), "claude".to_string()]);
    }

    #[test]
    fn specialist_disabled_until_decision_reuses_existing_loop() {
        let (mut bs, rs) = fleet();
        bs[2].enabled = false;
        let d = route(&bs, &rs, &req(&[Capability::Navigate])).unwrap();
        assert_eq!(d.primary, "acp");
        assert!(!d.fallbacks.contains(&"specialist".to_string()));
    }

    #[test]
    fn fallback_after_failure_never_drops_capability() {
        let (bs, rs) = fleet();
        let mut r = req(&[Capability::LiveView]);
        r.failed = ["specialist".to_string()].into_iter().collect();
        assert_eq!(route(&bs, &rs, &r).unwrap().primary, "acp");
        r.failed.insert("acp".into());
        assert_eq!(
            route(&bs, &rs, &r).unwrap_err(),
            RoutingError::NoCapableBackend { missing: vec![] }
        );
        let err = route(&bs, &rs, &req(&[Capability::Download])).unwrap_err();
        assert_eq!(
            err,
            RoutingError::NoCapableBackend {
                missing: vec![Capability::Download]
            }
        );
    }

    #[test]
    fn explicit_backend_is_honoured_or_refused() {
        let (bs, rs) = fleet();
        let mut r = req(&[Capability::Navigate]);
        r.explicit = Some("claude".into());
        let d = route(&bs, &rs, &r).unwrap();
        assert_eq!(d.primary, "claude");
        r.required.insert(Capability::LiveView);
        assert_eq!(
            route(&bs, &rs, &r).unwrap_err(),
            RoutingError::ExplicitUnavailable {
                backend: "claude".into()
            }
        );
    }

    #[test]
    fn same_task_evaluation_ranks_and_refuses_mixed_fixtures() {
        let run = |id: &str, ok: bool, viol: u32, cost: f64| SameTaskRun {
            backend_id: id.into(),
            task_fixture: "login-and-download".into(),
            accepted: ok,
            policy_violations: viol,
            recoveries: 0,
            cost_usd: cost,
            wall_secs: 60,
        };
        let runs = vec![
            run("a", true, 1, 0.1),
            run("b", true, 0, 0.5),
            run("c", false, 0, 0.1),
            run("d", true, 0, 0.2),
        ];
        assert_eq!(rank_same_task(&runs).unwrap(), vec!["d", "b", "c", "a"]);
        let mut mixed = runs.clone();
        mixed[0].task_fixture = "other".into();
        assert!(rank_same_task(&mixed).is_none());
        assert!(rank_same_task(&[]).is_none());
    }
}
