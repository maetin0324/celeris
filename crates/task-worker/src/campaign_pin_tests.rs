use super::*;
use crate::protocol::{CampaignSelected, PROTOCOL_VERSION, RunContext};
use task_core::decision_log::{CampaignIdentity, CampaignOrgPin, CampaignVariantRef};

fn hex(c: char) -> String {
    c.to_string().repeat(64)
}

fn pin(variant_id: &str) -> CampaignRunPin {
    CampaignRunPin {
        identity: CampaignIdentity {
            campaign_id: "c1".into(),
            case_id: None,
            arm: Some("B0".into()),
            repetition: Some(1),
            manifest_hash: None,
            routing_setting: None,
            variant: Some(CampaignVariantRef {
                comparison_id: "cmp".into(),
                id: variant_id.into(),
                content_sha256: hex('a'),
                selection_sha256: hex('b'),
            }),
            variant_org: Some(CampaignOrgPin {
                nodes: vec!["cos".into(), "coding".into()],
                coding_node: "coding".into(),
                effective_profile_sha256: hex('c'),
            }),
        },
        org_nodes_sha256: hex('d'),
        selected: CampaignSelected {
            harness_id: "claude-code".into(),
            source_id: "anthropic".into(),
            model_id: "sonnet".into(),
        },
        daemon_instance_id: "inst-1".into(),
        config_sha256: hex('e'),
    }
}

fn req(dir: &Path, campaign: Option<CampaignRunPin>) -> RunRequest {
    RunRequest {
        llm_route: None,
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        artifacts_dir: dir.join("artifacts"),
        workspace: dir.to_path_buf(),
        work_dir: None,
        context: RunContext {
            campaign,
            ..RunContext::default()
        },
    }
}

fn run_dir(dir: &Path, name: &str) -> PathBuf {
    let d = dir.join("runs").join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn variant_pin_is_stored_as_run_evidence_and_matches_on_resume() {
    let dir = tempfile::tempdir().unwrap();
    let r1 = run_dir(dir.path(), "r1");
    enforce(&r1, &req(dir.path(), Some(pin("spec-on"))))
        .await
        .unwrap();
    let evidence: CampaignRunPin =
        serde_json::from_slice(&std::fs::read(r1.join(PIN_FILE)).unwrap()).unwrap();
    assert_eq!(evidence, pin("spec-on"));
    assert!(dir.path().join("artifacts").join(PIN_FILE).exists());
    // continuation / retry: same pin → ok, and evidence is written for that run too
    let r2 = run_dir(dir.path(), "r2");
    enforce(&r2, &req(dir.path(), Some(pin("spec-on"))))
        .await
        .unwrap();
    assert!(r2.join(PIN_FILE).exists());
}

#[tokio::test]
async fn variant_pin_mismatch_on_resume_is_explicit_failure() {
    let dir = tempfile::tempdir().unwrap();
    enforce(
        &run_dir(dir.path(), "r1"),
        &req(dir.path(), Some(pin("spec-on"))),
    )
    .await
    .unwrap();
    let r2 = run_dir(dir.path(), "r2");
    let err = enforce(&r2, &req(dir.path(), Some(pin("spec-off"))))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("campaign_pin_mismatch"), "{err}");
    assert!(!r2.join(PIN_FILE).exists(), "no evidence for a refused run");
    // a differing observed field (daemon config) is also a mismatch
    let mut drift = pin("spec-on");
    drift.config_sha256 = hex('f');
    let err = enforce(&r2, &req(dir.path(), Some(drift)))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("campaign_pin_mismatch"), "{err}");
}

#[tokio::test]
async fn variant_pin_missing_on_resume_is_explicit_failure() {
    let dir = tempfile::tempdir().unwrap();
    enforce(
        &run_dir(dir.path(), "r1"),
        &req(dir.path(), Some(pin("spec-on"))),
    )
    .await
    .unwrap();
    let err = enforce(&run_dir(dir.path(), "r2"), &req(dir.path(), None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("campaign_pin_missing"), "{err}");
}

#[tokio::test]
async fn variant_pin_invalid_is_refused_and_plain_run_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let mut bad = pin("spec-on");
    bad.config_sha256 = "short".into();
    let err = enforce(&run_dir(dir.path(), "r1"), &req(dir.path(), Some(bad)))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("campaign_pin_invalid"), "{err}");
    let mut no_variant = pin("spec-on");
    no_variant.identity.variant = None;
    assert!(no_variant.validate().is_err());
    // plain: no pin, nothing stored → ok and no files written
    let r = run_dir(dir.path(), "r2");
    enforce(&r, &req(dir.path(), None)).await.unwrap();
    assert!(!r.join(PIN_FILE).exists());
    assert!(!dir.path().join("artifacts").join(PIN_FILE).exists());
}

#[test]
fn variant_pin_is_skipped_in_plain_context_json_and_roundtrips() {
    let plain = serde_json::to_value(RunContext::default()).unwrap();
    assert!(plain.get("campaign").is_none());
    let ctx = RunContext {
        campaign: Some(pin("spec-on")),
        ..RunContext::default()
    };
    let back: RunContext = serde_json::from_value(serde_json::to_value(&ctx).unwrap()).unwrap();
    assert_eq!(back, ctx);
}

#[tokio::test]
async fn variant_pin_mismatch_stops_run_before_spawn() {
    let dir = tempfile::tempdir().unwrap();
    enforce(
        &run_dir(dir.path(), "r1"),
        &req(dir.path(), Some(pin("spec-on"))),
    )
    .await
    .unwrap();
    let spec = crate::subprocess::SubprocessSpec {
        program: "/nonexistent/should-not-spawn".into(),
        args: vec![],
        env: vec![],
        container: None,
    };
    let sink = crate::adapter::NullSink;
    let err = crate::subprocess::run_subprocess(
        &spec,
        &req(dir.path(), Some(pin("spec-off"))),
        "r2",
        &crate::adapter::RunLimits {
            wall_clock: std::time::Duration::from_secs(5),
            idle_timeout: std::time::Duration::from_secs(5),
            kill_grace: std::time::Duration::from_secs(1),
        },
        &sink,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AdapterError::Other(m) if m.contains("campaign_pin_mismatch")));
}
