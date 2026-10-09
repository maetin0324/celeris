//! ADR 2026-10-08-browser-prod-enablement D1.4: `ledger_status` の分岐（一時 file だけ。userns・網は不要）。

use super::*;

const PUBLIC_CASES: [&str; 7] = [
    "open_allowed_origin",
    "refuse_denied_origin",
    "resume_after_crash",
    "snapshot_has_refs",
    "click_by_ref",
    "screenshot_artifact",
    "download_to_artifacts",
];

fn write_ledger(dir: &Path, generated_for: Option<serde_json::Value>, version: &str) -> PathBuf {
    let path = dir.join("conformance.json");
    let results: Vec<_> = ["claude-code", "browser-specialist"]
        .into_iter()
        .map(|id| serde_json::json!({"backend_id": id, "version": version, "passed": PUBLIC_CASES}))
        .collect();
    let mut ledger = serde_json::json!({
        "schema": 1, "source": "celeris-browser-conformance", "results": results,
    });
    if let Some(g) = generated_for {
        ledger["generated_for"] = g;
    }
    std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    path
}

fn write_credential_ledger(dir: &Path, mode: &str) -> PathBuf {
    let path = dir.join("credential-conformance.json");
    let mut evidence = Vec::new();
    for (case, names) in [
        (
            browser_backend::FixtureCase::IsolationSuite,
            browser_backend::P4A_ISOLATION_TESTS.as_slice(),
        ),
        (
            browser_backend::FixtureCase::EgressNegativeSuite,
            browser_backend::P4A_EGRESS_NEGATIVE_TESTS.as_slice(),
        ),
        (browser_backend::FixtureCase::InjectionAttackSuite, &[][..]),
        (
            browser_backend::FixtureCase::AuthSectionObservationStop,
            &[][..],
        ),
    ] {
        for test in names {
            let credential_case = matches!(case, browser_backend::FixtureCase::IsolationSuite | browser_backend::FixtureCase::EgressNegativeSuite);
            evidence.push(serde_json::json!({"case": case, "test": test, "runtime": if (mode == "launcher" || mode == "complete") && credential_case { "launcher" } else { "daemon" }, "outcome": if mode == "failed" && *test == names[0] { "failed" } else if mode == "not_run" && *test == names[0] { "not_run" } else { "passed" }}));
        }
    }
    // Preserve the existing P4-B proof fixture while focusing on the new P4-A gate.
    for (case, names) in [
        (
            browser_backend::FixtureCase::InjectionAttackSuite,
            browser_backend::P4B_ATTACK_MARKS
                .iter()
                .map(|m| browser_backend::attack_evidence_name(m))
                .collect::<Vec<_>>(),
        ),
        (
            browser_backend::FixtureCase::AuthSectionObservationStop,
            browser_backend::P4B_H3_TESTS
                .iter()
                .map(|t| (*t).to_string())
                .collect(),
        ),
    ] {
        for test in names {
            evidence.push(serde_json::json!({"case": case, "test": test, "outcome": "passed", "runtime": "daemon"}));
        }
    }
    if mode == "missing" {
        evidence.retain(|item| {
            item["case"] != "isolation_suite"
                || item["test"] != browser_backend::P4A_ISOLATION_TESTS[0]
        });
    }
    let mut passed = PUBLIC_CASES
        .iter()
        .map(|case| (*case).to_string())
        .collect::<Vec<_>>();
    passed.extend([
        "isolation_suite".to_string(),
        "egress_negative_suite".to_string(),
        "injection_attack_suite".to_string(),
        "auth_section_observation_stop".to_string(),
    ]);
    let ledger = serde_json::json!({"schema": 1, "source": "celeris-browser-conformance", "results": [
        {"backend_id":"claude-code", "version":SUPPORTED_VERSION, "passed":passed, "evidence":evidence}
    ]});
    std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    path
}

fn generated_for(release: &str) -> serde_json::Value {
    serde_json::json!({
        "celeris_release": release,
        "celeris_sha": format!("{release}0000000000000000000000000000"),
        "agent_browser": SUPPORTED_VERSION,
        "fixture": "p4c-local-v1",
        "generated_at": "2026-10-08T00:00:00Z",
    })
}

#[test]
fn browser_ledger_gate_status_missing_without_path_or_file() {
    let dir = tempfile::tempdir().unwrap();
    let none = ledger_status(None, None, &HostAgentBrowser::NotChecked);
    assert_eq!(none.code, BrowserPrerequisiteCode::Missing);
    let absent = dir.path().join("conformance.json");
    let status = ledger_status(Some(&absent), None, &HostAgentBrowser::NotChecked);
    assert_eq!(status.code, BrowserPrerequisiteCode::Missing);
}

#[test]
fn browser_ledger_gate_status_invalid_for_corrupt_or_foreign_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conformance.json");
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(
        ledger_status(Some(&path), None, &HostAgentBrowser::NotChecked).code,
        BrowserPrerequisiteCode::Invalid
    );
    std::fs::write(
        &path,
        br#"{"schema":1,"source":"celeris-browser-conformance-scripted","results":[]}"#,
    )
    .unwrap();
    assert_eq!(
        ledger_status(Some(&path), None, &HostAgentBrowser::NotChecked).code,
        BrowserPrerequisiteCode::Invalid
    );
}

#[test]
fn browser_ledger_gate_status_stale_release_and_agent_browser() {
    let dir = tempfile::tempdir().unwrap();
    // generated_for の無い旧台帳は release の daemon からは古い。
    let old = write_ledger(dir.path(), None, SUPPORTED_VERSION);
    assert_eq!(
        ledger_status(
            Some(&old),
            Some("1a2b3c4d5e6f"),
            &HostAgentBrowser::NotChecked
        )
        .code,
        BrowserPrerequisiteCode::StaleRelease
    );
    // 開発 daemon（release なし）では旧台帳も有効。
    assert_eq!(
        ledger_status(Some(&old), None, &HostAgentBrowser::NotChecked).code,
        BrowserPrerequisiteCode::Ok
    );
    let other = write_ledger(
        dir.path(),
        Some(generated_for("ffffffffffff")),
        SUPPORTED_VERSION,
    );
    let status = ledger_status(
        Some(&other),
        Some("1a2b3c4d5e6f"),
        &HostAgentBrowser::NotChecked,
    );
    assert_eq!(status.code, BrowserPrerequisiteCode::StaleRelease);
    assert_eq!(
        status
            .generated_for
            .as_ref()
            .map(|g| g.celeris_release.as_str()),
        Some("ffffffffffff")
    );
    let old_browser = write_ledger(dir.path(), Some(generated_for("1a2b3c4d5e6f")), "0.37.0");
    assert_eq!(
        ledger_status(
            Some(&old_browser),
            Some("1a2b3c4d5e6f"),
            &HostAgentBrowser::NotChecked
        )
        .code,
        BrowserPrerequisiteCode::StaleAgentBrowser
    );
    let current = write_ledger(
        dir.path(),
        Some(generated_for("1a2b3c4d5e6f")),
        SUPPORTED_VERSION,
    );
    assert_eq!(
        ledger_status(
            Some(&current),
            Some("1a2b3c4d5e6f"),
            &HostAgentBrowser::Version("0.39.0".into())
        )
        .code,
        BrowserPrerequisiteCode::StaleAgentBrowser
    );
    assert_eq!(
        ledger_status(
            Some(&current),
            Some("1a2b3c4d5e6f"),
            &HostAgentBrowser::Missing
        )
        .code,
        BrowserPrerequisiteCode::AgentBrowserMissing
    );
}

#[test]
fn browser_ledger_gate_status_ok_without_credential_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_ledger(
        dir.path(),
        Some(generated_for("1a2b3c4d5e6f")),
        SUPPORTED_VERSION,
    );
    let status = ledger_status(
        Some(&path),
        Some("1a2b3c4d5e6f"),
        &HostAgentBrowser::Version(SUPPORTED_VERSION.into()),
    );
    assert_eq!(status.code, BrowserPrerequisiteCode::Ok);
    assert_eq!(
        status
            .public_backends
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["browser-specialist", "claude-code"]
    );
    assert!(status.credential_backends.is_empty());
    assert_eq!(status.task_code(false), BrowserPrerequisiteCode::Ok);
    assert_eq!(
        status.task_code(true),
        BrowserPrerequisiteCode::LedgerLacksCredential
    );
}

#[test]
fn browser_ledger_credential_requires_complete_passed_isolation_and_egress_evidence() {
    for mode in ["complete", "missing", "failed", "not_run"] {
        let dir = tempfile::tempdir().unwrap();
        let path = write_credential_ledger(dir.path(), mode);
        let status = ledger_status(Some(&path), None, &HostAgentBrowser::NotChecked);
        assert_eq!(status.code, BrowserPrerequisiteCode::Ok, "mode={mode}");
        assert_eq!(
            status.public_backends,
            ["claude-code".to_string()].into_iter().collect()
        );
        if mode == "complete" {
            assert_eq!(
                status.credential_backends,
                ["claude-code".to_string()].into_iter().collect()
            );
            assert_eq!(status.task_code(true), BrowserPrerequisiteCode::Ok);
        } else {
            assert!(status.credential_backends.is_empty(), "mode={mode}");
            assert_eq!(
                status.task_code(true),
                BrowserPrerequisiteCode::LedgerLacksCredential
            );
        }
    }
}

#[test]
fn browser_ledger_gate_status_no_conformant_backend() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conformance.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "source": "celeris-browser-conformance",
            "results": [{"backend_id": "claude-code", "version": SUPPORTED_VERSION, "passed": ["open_allowed_origin"]}],
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        ledger_status(Some(&path), None, &HostAgentBrowser::NotChecked).code,
        BrowserPrerequisiteCode::NoConformantBackend
    );
}

#[test]
fn browser_ledger_gate_watch_recomputes_only_when_the_file_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conformance.json");
    let mut watch = LedgerWatch::fixed(Some(path.clone()), None, None);
    assert!(watch.refresh());
    assert_eq!(watch.status().code, BrowserPrerequisiteCode::Missing);
    assert!(!watch.refresh());
    write_ledger(dir.path(), None, SUPPORTED_VERSION);
    assert!(watch.refresh());
    assert_eq!(watch.status().code, BrowserPrerequisiteCode::Ok);
    assert!(!watch.refresh());
    std::fs::remove_file(&path).unwrap();
    assert!(watch.refresh());
    assert_eq!(watch.status().code, BrowserPrerequisiteCode::Missing);
}

#[test]
fn browser_ledger_gate_probe_reports_missing_executable() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        probe_agent_browser(&dir.path().join("no-such-agent-browser")),
        HostAgentBrowser::Missing
    );
}


#[test]
fn browser_ledger_launcher_daemon_only_evidence_does_not_certify() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_credential_ledger(dir.path(), "ok");
    let status = ledger_status(Some(&path), None, &HostAgentBrowser::NotChecked);
    assert!(status.public_backends.contains("claude-code"));
    assert!(status.credential_backends.is_empty());
    assert_eq!(status.task_code(true), BrowserPrerequisiteCode::LedgerLacksCredential);
}

#[test]
fn browser_ledger_launcher_fake_evidence_certifies_launcher_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_credential_ledger(dir.path(), "launcher");
    let status = ledger_status(Some(&path), None, &HostAgentBrowser::NotChecked);
    assert!(status.credential_backends.contains("claude-code"));
}

#[test]
fn browser_ledger_launcher_mixed_runtime_evidence_does_not_certify() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_credential_ledger(dir.path(), "launcher");
    let mut ledger: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let evidence = ledger["results"][0]["evidence"].as_array_mut().unwrap();
    if let Some(row) = evidence.iter_mut().find(|row| row["case"] == "isolation_suite") {
        row["runtime"] = serde_json::json!("daemon");
    }
    std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let status = ledger_status(Some(&path), None, &HostAgentBrowser::NotChecked);
    assert!(status.credential_backends.is_empty());
}
