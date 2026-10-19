//! A/B/C の試行が試行用の初期資料（materials/）と初期 KB（kb/）だけを参照することの試験。
//! 実行器に渡る workspace と kb は同じ木（試行の中）で、通常運用 KB・他試行の書き込み・grader 私有データは
//! 試行の木に現れない。外部 network・LLM は使わない（mock 実行器）。
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    executor::MockExecutor,
    manifest::CampaignManifest,
    runner::{RunOptions, run},
};
use serde_json::Value;

const PROD_KB_SECRET: &str = "PROD-KB-SECRET-5521";
const GRADER_SECRET: &str = "GRADER-PRIVATE-EXPECTED-9917";

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).expect("dir") {
        let p = e.expect("entry").path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn every_arm_sees_the_same_trial_kb_and_nothing_else() {
    let d = tempfile::tempdir().expect("dir");
    // 通常運用 KB と grader 私有データに見立てた、試行に渡してはいけない資料。
    let prod_kb = d.path().join("prod-kb");
    fs::create_dir_all(&prod_kb).expect("mkdir");
    fs::write(prod_kb.join("answer.md"), PROD_KB_SECRET).expect("write");
    let private = d.path().join("grader-private");
    fs::create_dir_all(&private).expect("mkdir");
    fs::write(private.join("expected.json"), GRADER_SECRET).expect("write");
    // 試行用の初期 KB は別に用意する（snapshot 元）。
    let kb_src = d.path().join("trial-kb");
    fs::create_dir_all(&kb_src).expect("mkdir");
    fs::write(kb_src.join("general.md"), "general facts only").expect("write");

    let mp = fx().join("pilot.json");
    let m = CampaignManifest::load(&mp).expect("pilot");
    let i = m.load_input(&mp, None).expect("input");
    let out = d.path().join("out");
    let o = RunOptions {
        out: out.clone(),
        repo_src: Some(fx().join("repo")),
        materials_src: Some(fx().join("materials")),
        kb_src: Some(kb_src.clone()),
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
        series: None,
    };
    run(&m, &i, &MockExecutor::seeded(), &o).expect("run");

    let recs: Vec<Value> = fs::read_to_string(out.join("trials.jsonl"))
        .expect("trials")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert!(recs.len() >= 3);
    let (mut arms, mut kb_initial) = (std::collections::BTreeSet::new(), Vec::new());
    for r in &recs {
        let ws = out.join(r["workspace"].as_str().expect("ws"));
        let kb = out.join(r["kb"].as_str().expect("kb"));
        // 実行器に渡る KB は workspace の中（materials/ と同じく、試行が実際に見られる場所）。
        assert_eq!(kb, ws.join("kb"), "trial kb must live inside its workspace");
        assert_eq!(
            fs::read_to_string(ws.join("kb/general.md")).expect("kb page"),
            "general facts only"
        );
        assert!(ws.join("materials/pluvio-report-draft.md").is_file());
        arms.insert(r["config"].to_string());
        kb_initial.push(r["kb_sha256_initial"].to_string());
        // 試行の木のどの file にも、通常運用 KB・grader 私有データの値は無い。
        let mut all = Vec::new();
        files(&ws, &mut all);
        for f in all {
            let t = fs::read_to_string(&f).unwrap_or_default();
            assert!(!t.contains(PROD_KB_SECRET), "{} leaks prod KB", f.display());
            assert!(
                !t.contains(GRADER_SECRET),
                "{} leaks grader data",
                f.display()
            );
            // 他の試行・out の他の場所を指す path も持ち込まない。
            assert!(
                !t.contains(&*out.to_string_lossy()) || f.ends_with("mock-result.txt"),
                "{} refers to out/",
                f.display()
            );
        }
        // 実行器（mock）が KB に書いた内容は、この試行の KB にだけある（元の KB snapshot・他試行へ戻らない）。
        assert!(ws.join("kb/mock-note.md").is_file());
    }
    assert_eq!(arms.len(), 3, "A/B/C all ran: {arms:?}");
    // 全 arm・全反復が同じ初期 KB から始まる。
    kb_initial.dedup();
    assert_eq!(kb_initial.len(), 1, "{kb_initial:?}");
    // 元の KB（snapshot 元・snapshot 複製）は試行の書き込みで変わらない。
    assert!(!kb_src.join("mock-note.md").exists());
    assert!(!out.join("snapshot/kb/mock-note.md").exists());
    // 試行 dir の他の試行の KB 書き込み（mock-note）が別試行の初期 hash に混ざらない。
    for r in &recs {
        assert_eq!(r["kb_sha256_initial"], recs[0]["kb_sha256_initial"]);
    }
}

#[test]
fn repo_that_already_has_kb_dir_collides_with_the_trial_kb() {
    let d = tempfile::tempdir().expect("dir");
    let repo = d.path().join("repo");
    fs::create_dir_all(repo.join("kb")).expect("mkdir");
    fs::write(repo.join("kb/x.md"), "x").expect("write");
    let mp = fx().join("pilot.json");
    let m = CampaignManifest::load(&mp).expect("pilot");
    let i = m.load_input(&mp, None).expect("input");
    let o = RunOptions {
        out: d.path().join("out"),
        repo_src: Some(repo),
        materials_src: Some(fx().join("materials")),
        kb_src: Some(fx().join("kb")),
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
        series: None,
    };
    let e = run(&m, &i, &MockExecutor::seeded(), &o).expect_err("collision");
    assert!(e.contains("kb/"), "{e}");
}
