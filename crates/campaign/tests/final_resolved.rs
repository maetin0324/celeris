//! final.json の解決検査: 未解決の placeholder・installed alias・C の routing 欠落は validate が落とす。
//! K2/K4 の時間は manifest の per_case 上限（30 分）に収まる。外部実行はしない。

#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::path::{Path, PathBuf};

use celeris_campaign::cases::CaseSet;
use celeris_campaign::manifest::CampaignManifest;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn committed() -> CampaignManifest {
    let p = root()
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final/final.json");
    CampaignManifest::load(&p).expect("final.json")
}

/// H1 後の置換を済ませた形。
fn resolved() -> CampaignManifest {
    let mut m = committed();
    for c in &mut m.configs {
        match c.id {
            celeris_campaign::manifest::ConfigId::A => {
                c.harness_version = "claude-code@2.1.0".into()
            }
            _ => c.harness_version = "0123456789ab".into(),
        }
        if let Some(r) = c.routing.as_mut()
            && r.evidence_table_id.is_some()
        {
            r.evidence_table_id = Some("evidence-v7".into());
        }
    }
    m
}

#[test]
fn committed_final_json_is_unresolved_and_rejected() {
    let e = committed().validate().expect_err("placeholder は未解決");
    assert!(e.contains("placeholder"), "{e}");
}

#[test]
fn resolved_final_json_validates() {
    resolved().validate().expect("resolved");
}

#[test]
fn alias_and_missing_routing_are_rejected() {
    let mut m = resolved();
    m.configs[0].harness_version = "claude-code@installed".into();
    assert!(m.validate().expect_err("alias").contains("alias"));
    let mut m = resolved();
    m.configs[2].routing = None;
    assert!(m.validate().expect_err("c routing").contains("routing"));
    let mut m = resolved();
    {
        let r = m.configs[2].routing.as_mut().unwrap();
        r.evidence_table_id = None;
        r.no_evidence_table = None;
    }
    assert!(
        m.validate()
            .expect_err("c evidence")
            .contains("evidence_table_id")
    );
    let mut m = resolved();
    m.configs[1].harness_version = "abc".into();
    assert!(m.validate().expect_err("short sha").contains("12-hex"));
    let mut m = resolved();
    m.configs[1].routing.as_mut().unwrap().online_update = Some(true);
    assert!(m.validate().expect_err("online").contains("online_update"));
}

#[test]
fn k2_k4_case_time_fits_manifest_per_case() {
    let set = CaseSet::load(
        &support_cases::cases_root(),
        &root().join("fixtures/grader-private"),
    )
    .expect("load");
    let m = committed();
    set.check_within_budget(&m.problems, &m.budget)
        .expect("fits");
    let mut tight = m.budget;
    tight.per_case.max_wall_secs = 1799;
    assert!(set.check_within_budget(&m.problems, &tight).is_err());
}
