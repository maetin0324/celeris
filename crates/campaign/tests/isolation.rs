use sha2::Digest;
use std::fs;

use celeris_campaign::{
    isolation::*,
    manifest::{Budget, ConfigId},
};

const FINAL_VALUE: &str = "FINAL-EXPECTED-7731.42";

fn sources(root: &std::path::Path) -> InitialSources {
    let (repo, mat, kb) = (root.join("s/repo"), root.join("s/mat"), root.join("s/kb"));
    for d in [&repo, &mat, &kb] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(repo.join("lib.rs"), "fn chunk() {}").unwrap();
    fs::create_dir_all(repo.join("grader")).unwrap();
    fs::write(repo.join("grader/pristine_test.rs"), "assert_eq!(x, 1)").unwrap();
    fs::write(repo.join("expected_final.json"), FINAL_VALUE).unwrap();
    fs::write(repo.join("defects.json"), "[\"known defect\"]").unwrap();
    fs::write(mat.join("paper.md"), "notes").unwrap();
    fs::write(kb.join("note.md"), "kb entry").unwrap();
    InitialSources {
        repo,
        materials: mat,
        kb,
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 1000,
            max_wall_secs: 60,
            max_disk_bytes: 1 << 20,
            max_parallel: 2,
            per_case: celeris_campaign::manifest::Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_000_000,
            },
            per_arm: celeris_campaign::manifest::Cap {
                max_tokens: 10_000_000_000,
                max_wall_secs: 1_000_000,
            },
        },
        withheld_names: vec![
            "grader".into(),
            "expected_final.json".into(),
            "defects.json".into(),
        ],
    }
}

fn view(t: &TrialDirs) -> TrialView {
    TrialView {
        id: t.id.clone(),
        root: t.root.clone(),
        // 初期 hash は config 間で同一なので秘密ではない。試行固有の hash（final 等）を渡す。
        owned_hashes: vec![format!("{:x}", sha2::Sha256::digest(t.id.as_bytes()))],
        final_values: vec![FINAL_VALUE.into()],
    }
}

#[test]
fn three_configs_get_identical_input_hashes() {
    let d = tempfile::tempdir().unwrap();
    let s = sources(d.path());
    let out = d.path().join("out");
    let ts: Vec<_> = [ConfigId::A, ConfigId::B, ConfigId::C]
        .into_iter()
        .map(|c| prepare_trial(&s, &out, c, "impl-x", 1).unwrap())
        .collect();
    let recs: Vec<_> = ts.iter().map(|t| t.record.clone()).collect();
    check_same_inputs(&recs).unwrap();
    // 記録が disk にも残る
    let saved: InputRecord =
        serde_json::from_slice(&fs::read(ts[2].root.join("inputs.json")).unwrap()).unwrap();
    assert_eq!(saved, recs[2]);
    let mut bad = recs.clone();
    bad[1].kb_sha256 = "0".into();
    assert!(check_same_inputs(&bad).is_err());
}

#[test]
fn kb_write_in_trial1_is_invisible_to_trial2_and_discarded() {
    let d = tempfile::tempdir().unwrap();
    let s = sources(d.path());
    let out = d.path().join("out");
    let t1 = prepare_trial(&s, &out, ConfigId::A, "p", 1).unwrap();
    fs::write(t1.kb.join("learned.md"), "answer: 42").unwrap();
    fs::write(t1.workspace.join("solution.rs"), "done").unwrap();
    let t2 = prepare_trial(&s, &out, ConfigId::A, "p", 2).unwrap();
    assert!(!t2.kb.join("learned.md").exists());
    assert!(!t2.workspace.join("solution.rs").exists());
    assert_eq!(t2.record.kb_sha256, t1.record.kb_sha256);
    // 元 snapshot も汚れていない
    assert!(!s.kb.join("learned.md").exists());
    let h = finish_trial(&t1).unwrap();
    assert_ne!(h, t1.record.kb_sha256);
    assert!(!t1.kb.exists());
}

#[test]
fn final_expectations_and_pristine_tests_are_not_in_workspace() {
    let d = tempfile::tempdir().unwrap();
    let s = sources(d.path());
    let t = prepare_trial(&s, &d.path().join("out"), ConfigId::B, "p", 1).unwrap();
    assert!(t.workspace.join("lib.rs").exists());
    assert!(!t.workspace.join("expected_final.json").exists());
    assert!(!t.workspace.join("defects.json").exists());
    assert!(!t.workspace.join("grader").exists());
    let leaks = verify_isolation(&[view(&t)]).unwrap();
    assert!(leaks.is_empty(), "{leaks:?}");
}

#[test]
fn verify_isolation_detects_intentional_leaks() {
    let d = tempfile::tempdir().unwrap();
    let s = sources(d.path());
    let out = d.path().join("out");
    let t1 = prepare_trial(&s, &out, ConfigId::A, "p", 1).unwrap();
    let t2 = prepare_trial(&s, &out, ConfigId::B, "p", 1).unwrap();
    let views = [view(&t1), view(&t2)];
    assert!(verify_isolation(&views).unwrap().is_empty());
    // 1) 他 trial の path
    fs::write(
        t2.workspace.join("a.txt"),
        format!("see {}", t1.root.display()),
    )
    .unwrap();
    // 2) 他 trial の hash
    fs::write(t2.kb.join("b.txt"), &views[0].owned_hashes[0]).unwrap();
    // 3) final 値
    fs::write(t1.workspace.join("c.txt"), format!("x {FINAL_VALUE} y")).unwrap();
    let leaks = verify_isolation(&views).unwrap();
    let has = |t: &str, f: &str, w: &str| {
        leaks
            .iter()
            .any(|l| l.in_trial == t && l.file.ends_with(f) && l.what.contains(w))
    };
    assert!(has(&t2.id, "a.txt", "path of trial"));
    assert!(has(&t2.id, "b.txt", "hash of trial"));
    assert!(has(&t1.id, "c.txt", "final value"));
}

#[test]
fn prepare_trial_keeps_inner_symlink_and_refuses_escaping_one() {
    let d = tempfile::tempdir().unwrap();
    let src = sources(d.path());
    std::os::unix::fs::symlink("lib.rs", src.repo.join("alias.rs")).unwrap();
    let t = prepare_trial(&src, &d.path().join("out"), ConfigId::A, "p1", 1).unwrap();
    let ln = t.workspace.join("alias.rs");
    assert!(fs::symlink_metadata(&ln).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(&ln).unwrap(), std::path::Path::new("lib.rs"));
    std::os::unix::fs::symlink("../../../x", src.repo.join("esc")).unwrap();
    assert!(prepare_trial(&src, &d.path().join("out2"), ConfigId::A, "p1", 1).is_err());
}
