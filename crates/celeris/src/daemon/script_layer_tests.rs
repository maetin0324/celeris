//! 付記 S5-2・S5-5: 起動時の復元の判定（[`restore_reason`]）。DB も isolate も使わない純粋関数の試験。

use std::collections::BTreeSet;

use celeris_script::dto::Capability;
use celeris_script::manifest::{ScriptManifest, Toolchain};
use task_core::script_compare::{
    ScriptDomain, ScriptDomainModeRow, ScriptGeneration, ScriptGenerationState, ScriptMode,
};

use super::restore_reason;
use crate::daemon::script_guard::reason_code;

fn row(domain: ScriptDomain, mode: ScriptMode) -> ScriptDomainModeRow {
    ScriptDomainModeRow {
        domain,
        mode,
        generation_id: (mode != ScriptMode::Rust).then(|| "g1".to_owned()),
        version: 3,
        canary_bps: None,
        updated_at: String::new(),
        updated_by: "human:test".into(),
    }
}

fn generation(state: ScriptGenerationState) -> ScriptGeneration {
    ScriptGeneration {
        id: "g1".into(),
        bundle_sha256: "a".repeat(64),
        host_api: "1.0.0".into(),
        schema_sha256: "0".repeat(64),
        manifest_json: "{}".into(),
        state,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn manifest(domains: &[&str], caps: &[Capability], checkpoints: &[u32]) -> ScriptManifest {
    ScriptManifest {
        schema: "celeris.script-manifest/1".into(),
        bundle_sha256: "a".repeat(64),
        host_api: "^1".into(),
        schema_sha256: "0".repeat(64),
        toolchain: Toolchain {
            typescript: "fixture".into(),
            esbuild: "fixture".into(),
            node: "fixture".into(),
        },
        source_commit: "fixture".into(),
        domains: domains.iter().map(|d| (*d).to_owned()).collect(),
        capabilities: caps.iter().copied().collect(),
        checkpoint_versions: checkpoints.to_vec(),
    }
}

/// 行・generation・load の結果・停止の理由・期待する code。
type Case<'a> = (
    &'a ScriptDomainModeRow,
    Option<ScriptGeneration>,
    Option<Result<ScriptManifest, String>>,
    Option<&'a str>,
    &'a str,
);

fn code(reason: Option<String>) -> String {
    reason
        .map(|r| reason_code(&r))
        .unwrap_or_else(|| "kept".into())
}

#[test]
fn script_restore_reason_keeps_usable_generation_modes() {
    let none = BTreeSet::new();
    let g = generation(ScriptGenerationState::Active);
    let ok = Ok(manifest(
        &["host_selftest", "lane_rules", "cos_digest"],
        &[Capability::CosDigestListRuns, Capability::CosDigestReadRun],
        &[],
    ));
    for mode in [ScriptMode::Compare, ScriptMode::Canary, ScriptMode::Ts] {
        for domain in [ScriptDomain::LaneRules, ScriptDomain::CosDigest] {
            assert_eq!(
                code(restore_reason(
                    &row(domain, mode),
                    Some(&g),
                    Some(&ok),
                    None,
                    &none
                )),
                "kept",
                "{domain:?} {mode:?}"
            );
        }
    }
    assert_eq!(
        restore_reason(
            &row(ScriptDomain::LaneRules, ScriptMode::Rust),
            None,
            None,
            None,
            &none
        ),
        None
    );
}

#[test]
fn script_restore_reason_codes_for_each_failure() {
    let none = BTreeSet::new();
    let active = generation(ScriptGenerationState::Active);
    let lane = row(ScriptDomain::LaneRules, ScriptMode::Compare);
    let digest = row(ScriptDomain::CosDigest, ScriptMode::Ts);
    let lane_only = Ok(manifest(&["host_selftest", "lane_rules"], &[], &[]));
    let cases: Vec<Case<'_>> = vec![
        (&lane, None, None, None, "startup_generation_missing"),
        (
            &lane,
            Some(generation(ScriptGenerationState::Invalid)),
            None,
            None,
            "startup_generation_invalid",
        ),
        (
            &lane,
            Some(generation(ScriptGenerationState::Draining)),
            None,
            None,
            "startup_generation_draining",
        ),
        (
            &lane,
            Some(active.clone()),
            None,
            None,
            "startup_bundle_unusable",
        ),
        (
            &lane,
            Some(active.clone()),
            Some(Err("manifest.json: No such file".into())),
            None,
            "startup_bundle_unusable",
        ),
        (
            &digest,
            Some(active.clone()),
            Some(lane_only.clone()),
            None,
            "startup_domain_not_declared",
        ),
        (
            &digest,
            Some(active.clone()),
            Some(Ok(manifest(
                &["host_selftest", "cos_digest"],
                &[Capability::CosDigestListRuns],
                &[],
            ))),
            None,
            "startup_capability_missing",
        ),
        (
            &lane,
            Some(active.clone()),
            Some(lane_only.clone()),
            Some("guard_unvalidated_effect"),
            "startup_guard_halted",
        ),
    ];
    for (row, g, loaded, halted, want) in cases {
        let got = restore_reason(row, g.as_ref(), loaded.as_ref(), halted, &none);
        assert_eq!(code(got.clone()), want, "{got:?}");
    }
}

/// 付記 S5-5: 保存済みの版の集合 V（作り物）を manifest が全部含まなければ戻す。
#[test]
fn script_restore_reason_checks_stored_checkpoint_versions() {
    let g = generation(ScriptGenerationState::Active);
    let lane = row(ScriptDomain::LaneRules, ScriptMode::Compare);
    let stored: BTreeSet<u32> = [1].into_iter().collect();
    let without = Ok(manifest(&["host_selftest", "lane_rules"], &[], &[]));
    let with = Ok(manifest(&["host_selftest", "lane_rules"], &[], &[1]));
    assert_eq!(
        code(restore_reason(
            &lane,
            Some(&g),
            Some(&without),
            None,
            &stored
        )),
        "startup_checkpoint_incompatible"
    );
    assert_eq!(
        code(restore_reason(&lane, Some(&g), Some(&with), None, &stored)),
        "kept"
    );
}
