//! 公開 case 定義が全腕の trial workspace の `materials/<case>/case.json` に届き、private_ref が漏れないこと、
//! objective が A・B・C で同じ参照先を指すこと（ADR 2026-10-20-mlsys-main2-fixes D2-4）。network・LLM なし。
#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    cases::CaseSet,
    executor::MockExecutor,
    inputs::{check_prompt_reference, public_case_path, public_cases},
    manifest::CampaignManifest,
    runner::{RunOptions, run},
};
use serde_json::Value;

const K4: &str = "k4-cmp4-validity-analysis";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn set() -> CaseSet {
    CaseSet::load(
        &support_cases::cases_root(),
        &root().join("fixtures/grader-private"),
    )
    .expect("load")
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("mkdir");
    for e in fs::read_dir(src).expect("read") {
        let p = e.expect("e").path();
        let to = dst.join(p.file_name().expect("name"));
        if p.is_dir() {
            copy_dir(&p, &to);
        } else {
            fs::copy(&p, &to).expect("copy");
        }
    }
}

#[test]
fn public_json_omits_the_private_and_grade_case_refs() {
    let s = set();
    let c = &s.get(K4).expect("k4").case;
    let j = c.public_json().expect("json");
    let v: Value = serde_json::from_str(&j).expect("parse");
    assert!(v.get("private_ref").is_none() && v.get("grade_case_ref").is_none());
    assert!(!j.contains(&c.private_ref.sha256), "private hash leaked");
    assert!(!j.contains(&c.private_ref.path), "private path leaked");
    assert_eq!(v["id"], K4);
    assert!(v["inputs"].is_array() && v["visible_command"].is_string());
}

#[test]
fn every_arm_workspace_gets_the_same_public_case_and_no_private_ref() {
    let d = tempfile::tempdir().expect("dir");
    let mut m = CampaignManifest::load(&root().join("fixtures/pilot.json")).expect("pilot");
    let problem = m.problems[0].clone();
    let i = m
        .load_input(&root().join("fixtures/pilot.json"), None)
        .expect("input");
    let mat = d.path().join("mat");
    copy_dir(&root().join("fixtures/materials"), &mat);
    fs::create_dir_all(mat.join(&problem)).expect("dir");
    fs::write(mat.join(&problem).join("task.md"), "task").expect("task");
    // 試験用に materials を足したので、manifest の pin を実体に合わせる。
    m.snapshot.materials_sha256 = celeris_campaign::snapshot::hash_tree(&mat).expect("hash");
    m.sha256 = m.compute_sha256().expect("sha");
    let json = set().get(K4).expect("k4").case.public_json().expect("json");
    let o = RunOptions {
        out: d.path().join("out"),
        repo_src: Some(root().join("fixtures/repo")),
        materials_src: Some(mat),
        kb_src: Some(root().join("fixtures/kb")),
        public_cases: BTreeMap::from([(problem.clone(), json.clone())]),
        ..Default::default()
    };
    run(&m, &i, &MockExecutor::seeded(), &o).expect("run");
    let text = fs::read_to_string(d.path().join("out/trials.jsonl")).expect("trials");
    let mut seen = 0;
    let mut initial = std::collections::BTreeSet::new();
    for l in text.lines() {
        let r: Value = serde_json::from_str(l).expect("json");
        initial.insert(r["workspace_sha256_initial"].to_string());
        let ws = d
            .path()
            .join("out")
            .join(r["workspace"].as_str().expect("ws"));
        let f = ws.join(public_case_path(r["problem_id"].as_str().expect("p")));
        if r["problem_id"] == problem.as_str() {
            assert_eq!(fs::read_to_string(&f).expect("delivered"), json);
            assert!(!json.contains("private_ref"));
            seen += 1;
        } else {
            assert!(!f.exists(), "undeclared problem got a case file");
        }
    }
    assert!(seen >= 3, "A, B and C each got it: {seen}");
    assert_eq!(
        initial.len(),
        1,
        "case file must not change the initial hash: {initial:?}"
    );
}

#[test]
fn objective_must_refer_to_the_delivered_path_for_every_arm() {
    let s = set();
    let want = public_case_path(K4);
    let ok = format!("{want} の定義に従え");
    check_prompt_reference(&s, &[(K4.into(), ok)]).expect("ok");
    let old = format!("{want}。公開 case 定義は crates/campaign/cases/final/{K4}/case.json");
    assert!(check_prompt_reference(&s, &[(K4.into(), old)]).is_err());
    assert!(check_prompt_reference(&s, &[(K4.into(), "no ref".into())]).is_err());
    // 公開 case 定義の対象は A・B・C で共通（腕ごとの分岐は無い）。
    let all = public_cases(&s, &[K4.to_string()]).expect("cases");
    assert_eq!(all.len(), 1);
}
