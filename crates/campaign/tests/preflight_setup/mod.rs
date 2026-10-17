//! preflight 試験の共有部品: 解決済みの final 一式を一時 dir に作る（`preflight.rs` と `preflight_contract.rs`）。
#![allow(dead_code)]
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::preflight::Inputs;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const K4_INPUT: &str = "main-v3-n2-43706.tgz";

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// 解決済みの final 一式（manifest・入力・材料・template）を一時 dir に作る。
pub struct Setup {
    pub dir: tempfile::TempDir,
    pub inp: Inputs,
}

pub fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let t = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy(&p, &t);
        } else {
            fs::copy(&p, &t).unwrap();
        }
    }
}

pub fn resolved_manifest() -> Value {
    let p = root()
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final/final.json");
    let mut v: Value = serde_json::from_slice(&fs::read(p).unwrap()).unwrap();
    for c in v["configs"].as_array_mut().unwrap() {
        match c["id"].as_str().unwrap() {
            "A" => c["harness_version"] = json!("claude-code@2.1.0"),
            _ => c["harness_version"] = json!("0123456789ab"),
        }
        if c["routing"]["evidence_table_id"].is_string() {
            c["routing"]["evidence_table_id"] = json!("evidence-v7");
        }
    }
    v
}

pub fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let pd = root()
        .join("../../agent-docs")
        .join("progress/2026-10-16-mlsys-final");
    fs::copy(
        pd.join("final-input-manifest.json"),
        d.join("final-input-manifest.json"),
    )
    .unwrap();
    fs::write(d.join("final.json"), resolved_manifest().to_string()).unwrap();
    let (m, k) = (d.join("materials"), d.join("kb"));
    // 外部入力: 実物（本番の asset）は無いので、case を写して k4 の宣言 sha256 を試験用の中身に合わせる。
    let (cases, ins) = (d.join("cases"), d.join("inputs-src"));
    copy(&root().join("cases"), &cases);
    fs::create_dir_all(&ins).unwrap();
    let repo = d.join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    fs::write(ins.join(K4_INPUT), b"fake archive for the test").unwrap();
    let cj = cases.join("final/k4-cmp4-validity-analysis/case.json");
    let mut c: Value = serde_json::from_slice(&fs::read(&cj).unwrap()).unwrap();
    c["inputs"][0]["sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(b"fake archive for the test")
    ));
    fs::write(&cj, c.to_string()).unwrap();
    for c in ["k2-bf-xattr-restart-campaign", "k4-cmp4-validity-analysis"] {
        copy(
            &root().join("cases/final").join(c).join("materials"),
            &m.join(c),
        );
    }
    copy(
        &root().join("cases/final/k2-bf-xattr-restart-campaign/kb"),
        &k,
    );
    fs::write(
        d.join("task-template.json"),
        json!({"project_id":"01ABC","acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
    )
    .unwrap();
    let inp = Inputs {
        manifest_path: d.join("final.json"),
        input_override: None,
        cases_root: cases,
        grader_root: root().join("fixtures/grader-private"),
        grade_cases_root: root().join("fixtures/grade-cases"),
        a_program: "claude".into(),
        api_url: Some("http://127.0.0.1:8080".into()),
        token_env: Some("CELERIS_CAMPAIGN_TOKEN".into()),
        task_template: Some(d.join("task-template.json")),
        repo_src: Some(repo),
        materials_src: Some(m),
        kb_src: Some(k),
        inputs_src: Some(ins),
        a_account: Some("lab".into()),
    };
    Setup { dir, inp }
}
