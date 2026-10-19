//! 測定専用 daemon config の雛形（agent-docs/progress/2026-10-16-mlsys-final/measure-daemon.example.toml）が、
//! final.json の B/C `run_harnesses` すべてに検査宣言（cap-probe-<h>: kind=fake・env_of=<h>）を持ち、
//! 実試行の経路に probe 用 harness を混ぜないことを、cap_probe の宣言検証で確かめる。
use std::{fs, path::PathBuf};

use celeris_campaign::{cap_probe, manifest::CampaignManifest};

fn final_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final")
}

fn run_harnesses() -> Vec<String> {
    let m = CampaignManifest::load(&final_dir().join("final.json")).expect("final.json");
    let mut hs: Vec<String> = m
        .configs
        .iter()
        .filter(|c| c.id != celeris_campaign::manifest::ConfigId::A)
        .flat_map(|c| c.worker_harnesses())
        .collect();
    hs.sort();
    hs.dedup();
    hs
}

#[test]
fn template_declares_every_run_harness_probe() {
    let hs = run_harnesses();
    assert!(!hs.is_empty());
    let t = final_dir().join("measure-daemon.example.toml");
    assert_eq!(
        cap_probe::declaration_problems_of_file(&t, &hs),
        Vec::<String>::new()
    );
}

#[test]
fn template_has_no_undeclared_probe_or_real_candidate_probe() {
    let t = final_dir().join("measure-daemon.example.toml");
    let c: toml::Value = toml::from_str(&fs::read_to_string(t).unwrap()).unwrap();
    let hs = run_harnesses();
    let declared = c["harnesses"].as_table().unwrap();
    for (id, h) in declared {
        if let Some(base) = id.strip_prefix(cap_probe::PROBE_GENRE_PREFIX) {
            assert!(hs.iter().any(|x| x == base), "undeclared-use probe {id}");
            assert_eq!(h["kind"].as_str(), Some("fake"));
        } else {
            assert!(hs.contains(id), "harness {id} is not in run_harnesses");
        }
    }
}

#[test]
fn missing_probe_declaration_is_rejected() {
    let d = tempfile_dir();
    let t = d.join("m.toml");
    fs::write(&t, "[harnesses.claude-code]\nkind = \"claude_code\"\n").unwrap();
    let p = cap_probe::declaration_problems_of_file(&t, &["claude-code".to_string()]);
    assert!(
        p.iter().any(|m| m.contains("no harness_selection rule")),
        "{p:?}"
    );
}

fn tempfile_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("mde-{}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    d
}
