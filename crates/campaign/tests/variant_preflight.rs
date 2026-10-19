//! 専門分担 variant の preflight 入口（GET のみ・有料実行 0 件）。manifest に variant 参照が無ければ従来どおり、
//! 有れば spec・B0・稼働 org が全て揃って一致しなければ拒否する。外部ネットワーク・LLM は使わない。
mod daemon_identity;
mod preflight_setup;

use std::{fs, path::Path};

use celeris_campaign::{
    cap_probe::{ProbeRunner, WorkerProbeRun, WorkerProbeSpec},
    lock::CampaignLock,
    manifest::CampaignManifest,
    preflight::{Host, Inputs, ResolvedProgram, check, expected_lock},
    variant::{REQUIRED_ROLES, VariantId, VariantSpec, effective_profile_sha256},
    variant_pin::{B0Inputs, baseline_pin},
};
use preflight_setup::*;
use serde_json::{Value, json};
use task_core::{model::Tier, profile::EffectiveProfile};

struct OrgHost {
    org: Value,
}

impl Host for OrgHost {
    fn probe_runner(&self) -> Box<dyn ProbeRunner> {
        Box::new(preflight_setup::OkRunner)
    }
    fn probe_daemon_worker(
        &self,
        _: &str,
        _: &str,
        spec: &WorkerProbeSpec,
    ) -> Result<WorkerProbeRun, String> {
        Ok(preflight_setup::ok_worker_run(spec))
    }
    fn env_nonempty(&self, name: &str) -> bool {
        name == "CELERIS_CAMPAIGN_TOKEN"
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        if p == Path::new("claude") {
            Ok(ResolvedProgram {
                path: p.to_path_buf(),
                version: "fake 1.0".into(),
            })
        } else {
            Err("not available".into())
        }
    }
    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        match path {
            "/api/v1/config" => Ok(daemon_identity::matching_daemon_body()),
            "/api/v1/llm/accounts" => Ok(
                json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":"/tmp","logged_in":true}]}),
            ),
            "/api/v1/projects/01ABC" => Ok(json!({"project": {"auto_integrate": false}})),
            "/api/v1/projects/01ABC/repos" => Ok(json!({"items": []})),
            "/api/v1/org" => Ok(self.org.clone()),
            _ => Err(format!("GET {path}: HTTP 404")),
        }
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
}

fn profile(node: &str) -> EffectiveProfile {
    EffectiveProfile {
        node_id: node.into(),
        chain: vec!["cos".into(), node.into()],
        harnesses_allowed: vec!["claude_code".into()],
        review_harness: Some("claude_code".into()),
        review_tier: Some(Tier::Standard),
        max_lane: Some(Tier::Standard),
        max_attempts: Some(3),
        ..Default::default()
    }
}

fn org(nodes: &[&str]) -> Value {
    json!({
        "items": nodes.iter().map(|n| json!({"id": n, "parent_id": null})).collect::<Vec<_>>(),
        "effective_profiles": nodes.iter().map(|n| profile(n)).collect::<Vec<_>>(),
    })
}

fn b0() -> B0Inputs {
    B0Inputs {
        coding_node: "coding".into(),
        role_models: REQUIRED_ROLES
            .iter()
            .map(|r| (r.to_string(), "m".to_string()))
            .collect(),
        org: org(&["cos", "coding", "research"]),
    }
}

fn write_b0(dir: &Path, b: &B0Inputs) {
    let v = json!({"coding_node": b.coding_node, "role_models": b.role_models, "org": b.org});
    fs::write(dir.join("variant-b0.json"), v.to_string()).unwrap();
}

/// B0（variant なし）から spec を作り、manifest に参照を書き、spec・B0 を manifest の隣に置く。
fn wire(s: &Setup, id: VariantId) -> (VariantSpec, Inputs) {
    let d = s.dir.path();
    let m = CampaignManifest::load(&s.inp.manifest_path).unwrap();
    let lock: CampaignLock = expected_lock(&s.inp, &m).unwrap();
    let base = baseline_pin(&m, &lock, &b0()).unwrap();
    let org_pin = match id {
        VariantId::SpecOn => base.org.clone(),
        VariantId::SpecOff => celeris_campaign::variant::OrgPin {
            nodes: vec!["cos".into(), "worker".into()],
            coding_node: "worker".into(),
            effective_profile_sha256: effective_profile_sha256(&profile("worker")).unwrap(),
        },
    };
    let spec = VariantSpec::new(id, org_pin, base.non_target)
        .seal()
        .unwrap();
    let mut v: Value = serde_json::from_slice(&fs::read(&s.inp.manifest_path).unwrap()).unwrap();
    v["variant"] = serde_json::to_value(spec.to_ref().unwrap()).unwrap();
    fs::write(&s.inp.manifest_path, v.to_string()).unwrap();
    fs::write(
        d.join("variant-spec.json"),
        serde_json::to_string(&spec).unwrap(),
    )
    .unwrap();
    write_b0(d, &b0());
    (spec, s.inp.clone())
}

fn variant_missing(r: &celeris_campaign::preflight::Report) -> Vec<String> {
    r.missing
        .iter()
        .filter(|m| m.check == "variant")
        .map(|m| m.detail.clone())
        .collect()
}

#[test]
fn no_variant_reference_keeps_the_old_preflight() {
    let s = setup();
    let r = check(
        &s.inp,
        &OrgHost {
            org: org(&["cos", "coding", "research"]),
        },
    );
    assert!(r.ok, "{:?}", r.missing);
    assert!(!r.checked.contains(&"variant"));
}

#[test]
fn both_variants_pass_and_every_drift_or_gap_is_rejected() {
    for id in [VariantId::SpecOn, VariantId::SpecOff] {
        let s = setup();
        let (spec, inp) = wire(&s, id);
        let nodes: Vec<&str> = spec.org.nodes.iter().map(String::as_str).collect();
        let ok = OrgHost { org: org(&nodes) };
        let r = check(&inp, &ok);
        assert!(r.ok && r.checked.contains(&"variant"), "{:?}", r.missing);
        assert_eq!(r.paid_calls_made, 0);

        // 稼働 org の drift（node 追加）。
        let mut more = nodes.clone();
        more.push("extra");
        let r = check(&inp, &OrgHost { org: org(&more) });
        assert!(!variant_missing(&r).is_empty(), "{:?}", r.missing);
        // GET org の情報不足。
        let r = check(
            &inp,
            &OrgHost {
                org: json!({"items": []}),
            },
        );
        assert!(!variant_missing(&r).is_empty());
        // spec ファイルの欠落。
        let p = s.dir.path().join("variant-spec.json");
        let saved = fs::read(&p).unwrap();
        fs::remove_file(&p).unwrap();
        assert!(!variant_missing(&check(&inp, &ok)).is_empty());
        // spec の内容改変（再 seal なし）。
        let mut t = serde_json::from_slice::<Value>(&saved).unwrap();
        t["non_target"]["kb_snapshot"] = json!("kb@other");
        fs::write(&p, t.to_string()).unwrap();
        assert!(!variant_missing(&check(&inp, &ok)).is_empty());
        fs::write(&p, &saved).unwrap();
        // B0 の欠落。
        let b = s.dir.path().join("variant-b0.json");
        fs::remove_file(&b).unwrap();
        assert!(!variant_missing(&check(&inp, &ok)).is_empty());
    }
}
