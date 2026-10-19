//! 代表 case の読み込み・検証の試験。
//! 固定 hash は `CAMPAIGN_UPDATE_CASES=1 cargo test -p celeris-campaign --test cases_load -- --ignored` で作り直す。
//! 通常は「置いてある case の hash が現物と一致する」ことを確かめる。
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    cases::{CaseKind, CaseSet, verify_file_sha256},
    manifest::Split,
    snapshot::{copy_tree, hash_tree},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).unwrap()))
}

fn rw(p: &Path, f: impl FnOnce(&mut Value)) {
    let mut v: Value = serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap();
    f(&mut v);
    fs::write(p, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
}

fn case_dirs(base: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    for s in ["dev", "final"] {
        if let Ok(rd) = fs::read_dir(base.join("cases").join(s)) {
            v.extend(
                rd.map(|e| e.unwrap().path())
                    .filter(|d| d.join("case.json").exists()),
            );
        }
    }
    v
}

/// 現物から hash を埋め直す（materials → kb → private の順）。
fn update_hashes(base: &Path) {
    for d in case_dirs(base) {
        let (mat, kb) = (
            hash_tree(&d.join("materials")).unwrap(),
            hash_tree(&d.join("kb")).unwrap(),
        );
        let cj = d.join("case.json");
        let rel = serde_json::from_str::<Value>(&fs::read_to_string(&cj).unwrap()).unwrap()["private_ref"]["path"]
            .as_str()
            .unwrap()
            .to_string();
        let ph = sha(&base.join("fixtures").join("grader-private").join(rel));
        rw(&cj, |v| {
            v["snapshot"]["materials_sha256"] = json!(mat);
            v["snapshot"]["kb_snapshot"] = json!(kb);
            v["private_ref"]["sha256"] = json!(ph);
        });
    }
}

/// 実 case 一式の写し。(tempdir, cases_root, grader_root)
fn copy() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    copy_tree(&root().join("cases"), &t.path().join("cases")).unwrap();
    copy_tree(
        &root().join("fixtures").join("grader-private"),
        &t.path().join("fixtures").join("grader-private"),
    )
    .unwrap();
    let (c, g) = (
        t.path().join("cases"),
        t.path().join("fixtures").join("grader-private"),
    );
    (t, c, g)
}
fn load(c: &Path, g: &Path) -> Result<CaseSet, String> {
    CaseSet::load(c, g)
}

fn err(r: Result<CaseSet, String>) -> String {
    r.expect_err("must be rejected")
}

#[test]
#[ignore = "固定 hash の作り直し用: CAMPAIGN_UPDATE_CASES=1 ... -- --ignored"]
fn update_pins() {
    if std::env::var("CAMPAIGN_UPDATE_CASES").as_deref() == Ok("1") {
        update_hashes(&root());
    }
}

#[test]
fn shipped_cases_are_pinned_and_load() {
    let set = load(
        &root().join("cases"),
        &root().join("fixtures").join("grader-private"),
    )
    .unwrap();
    let real: Vec<_> = set
        .cases
        .iter()
        .filter(|c| c.case.kind == CaseKind::Real)
        .collect();
    assert!((3..=5).contains(&real.len()), "real cases: {}", real.len());
    assert!(real.iter().any(|c| c.case.split == Split::Dev));
    assert!(real.iter().any(|c| c.case.split == Split::Final));
    let controls: Vec<_> = set
        .cases
        .iter()
        .filter(|c| c.case.kind == CaseKind::Control)
        .collect();
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0].case.split, Split::Dev);
    for c in &real {
        assert!(c.private.known_defects.len() >= 3, "{}", c.case.id);
    }
}

#[test]
fn final_expectations_are_not_reachable_from_dev_layout() {
    let (r, g) = (
        root().join("cases"),
        root().join("fixtures").join("grader-private"),
    );
    let set = load(&r, &g).unwrap();
    // final の private は fixtures/grader-private/final にだけあり、cases/ の下に private.json は無い。
    assert!(
        !case_dirs(&root())
            .iter()
            .any(|d| d.join("private.json").exists())
    );
    for c in set.cases.iter().filter(|c| c.case.split == Split::Final) {
        assert!(c.case.private_ref.path.starts_with("final/"));
        assert!(g.join(&c.case.private_ref.path).is_file());
        // dev の木全体（case と grader/dev）に final の id・欠陥 id・期待値の語が出ない。
        let mut needles = vec![c.case.id.clone()];
        needles.extend(c.private.known_defects.iter().map(|d| d.id.clone()));
        needles.push(c.private.hidden_command.clone());
        for base in [r.join("dev"), g.join("dev")] {
            let mut stack = vec![base];
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(d).unwrap() {
                    let p = e.unwrap().path();
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        let t = fs::read_to_string(&p).unwrap();
                        for n in &needles {
                            assert!(!t.contains(n.as_str()), "{} leaks {n}", p.display());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn rejects_hash_mismatch_in_materials_kb_and_private() {
    for (rel, what) in [
        (
            "cases/dev/k3-locusta-ab-analysis/materials/task.md",
            "materials hash mismatch",
        ),
        (
            "cases/dev/k3-locusta-ab-analysis/kb/snapshot.md",
            "kb snapshot hash mismatch",
        ),
        (
            "fixtures/grader-private/dev/k3-locusta-ab-analysis/private.json",
            "private file hash mismatch",
        ),
    ] {
        let (t, c, g) = copy();
        let p = t.path().join(rel);
        let mut s = fs::read_to_string(&p).unwrap();
        s.push(' ');
        fs::write(&p, s).unwrap();
        let e = err(load(&c, &g));
        assert!(e.contains(what), "{rel}: {e}");
    }
}

#[test]
fn rejects_split_mixing() {
    // case.json の split が置き場所と食い違う
    let (t, c, g) = copy();
    rw(&c.join("dev/k3-locusta-ab-analysis/case.json"), |v| {
        v["split"] = json!("final")
    });
    let e = err(load(&c, &g));
    assert!(e.contains("split mixing") || e.contains("disagrees"), "{e}");
    drop(t);
    // private の split が case と食い違う
    let (t, c, g) = copy();
    rw(&g.join("dev/k3-locusta-ab-analysis/private.json"), |v| {
        v["split"] = json!("final")
    });
    update_hashes_in(&t);
    let e = err(load(&c, &g));
    assert!(e.contains("split mixing"), "{e}");
    // private_ref が別 split の directory を指す
    let (_t, c, g) = copy();
    rw(&c.join("dev/k1-bf-fsync-campaign/case.json"), |v| {
        v["private_ref"]["path"] = json!("final/k2-bf-xattr-restart-campaign/private.json")
    });
    let e = err(load(&c, &g));
    assert!(e.contains("split mixing"), "{e}");
}

fn update_hashes_in(t: &tempfile::TempDir) {
    update_hashes(t.path());
}

#[test]
fn rejects_dev_file_that_names_a_final_value() {
    let (t, c, g) = copy();
    fs::write(
        c.join("dev/k1-bf-fsync-campaign/materials/note.md"),
        "see k4-cmp4-validity-analysis\n",
    )
    .unwrap();
    update_hashes_in(&t);
    let e = err(load(&c, &g));
    assert!(e.contains("referenced from dev file"), "{e}");
    // grader 側の dev private に final の欠陥 id
    let (t, c, g) = copy();
    rw(&g.join("dev/k1-bf-fsync-campaign/private.json"), |v| {
        v["expected"]["leak"] = json!("k4-chfs-fsync-zero")
    });
    update_hashes_in(&t);
    let e = err(load(&c, &g));
    assert!(e.contains("referenced from dev file"), "{e}");
}

#[test]
fn rejects_grader_root_inside_cases_root() {
    let (t, c, _g) = copy();
    copy_tree(
        &t.path().join("fixtures").join("grader-private"),
        &c.join("fixtures").join("grader-private"),
    )
    .unwrap();
    let e = err(load(&c, &c.join("fixtures").join("grader-private")));
    assert!(e.contains("isolated"), "{e}");
}

#[test]
fn control_cannot_enter_measurement_or_final() {
    let set = load(
        &root().join("cases"),
        &root().join("fixtures").join("grader-private"),
    )
    .unwrap();
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let e = set
        .select_for_measurement(&ids(&["k0-fixture-control"]))
        .unwrap_err();
    assert!(e.contains("control"), "{e}");
    let e = set
        .select_for_measurement(&ids(&["k1-bf-fsync-campaign", "k0-fixture-control"]))
        .unwrap_err();
    assert!(e.contains("control"), "{e}");
    // control を final に置く指定は case 検証で拒否
    let (t, c, g) = copy();
    rw(&c.join("dev/k0-fixture-control/case.json"), |v| {
        v["split"] = json!("final");
        v["private_ref"]["path"] = json!("final/k0-fixture-control/private.json");
    });
    let _ = t;
    let e = err(load(&c, &g));
    assert!(e.contains("control case must be split=dev"), "{e}");
}

#[test]
fn measurement_selection_rejects_mixed_unknown_duplicate_empty() {
    let set = load(
        &root().join("cases"),
        &root().join("fixtures").join("grader-private"),
    )
    .unwrap();
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let ok = set
        .select_for_measurement(&ids(&["k1-bf-fsync-campaign", "k3-locusta-ab-analysis"]))
        .unwrap();
    assert_eq!(ok.len(), 2);
    let ok = set
        .select_for_measurement(&ids(&[
            "k2-bf-xattr-restart-campaign",
            "k4-cmp4-validity-analysis",
        ]))
        .unwrap();
    assert!(ok.iter().all(|c| c.case.split == Split::Final));
    let e = set
        .select_for_measurement(&ids(&["k1-bf-fsync-campaign", "k4-cmp4-validity-analysis"]))
        .unwrap_err();
    assert!(e.contains("split mixing"), "{e}");
    assert!(
        set.select_for_measurement(&ids(&["nope"]))
            .unwrap_err()
            .contains("unknown")
    );
    assert!(
        set.select_for_measurement(&ids(&["k1-bf-fsync-campaign", "k1-bf-fsync-campaign"]))
            .unwrap_err()
            .contains("duplicate")
    );
    assert!(
        set.select_for_measurement(&[])
            .unwrap_err()
            .contains("empty")
    );
}

#[test]
fn rejects_missing_or_malformed_required_fields() {
    type Edit = fn(&mut Value);
    let edits: Vec<(&str, &str, Edit)> = vec![
        ("evidence", "evidence kinds", |v| v["evidence"] = json!([])),
        ("visible_command", "visible_command", |v| {
            v["visible_command"] = json!(" ")
        }),
        ("experiment", "needs experiment conditions", |v| {
            v["experiment"] = Value::Null
        }),
        ("job evidence", "job_id", |v| {
            v["evidence"] = json!(["raw_data", "log"])
        }),
        ("commit", "malformed", |v| {
            v["initial"]["commit"] = json!("abc")
        }),
        ("repo_commit", "differs from initial.commit", |v| {
            v["snapshot"]["repo_commit"] = json!("0".repeat(40))
        }),
        ("walltime", "nodes/walltime", |v| {
            v["experiment"]["walltime_secs"] = json!(0)
        }),
        ("schema", "schema must be", |v| v["schema"] = json!("x/1")),
        ("unknown field", "unknown field", |v| v["extra"] = json!(1)),
    ];
    for (name, want, f) in edits {
        let (_t, c, g) = copy();
        rw(&c.join("dev/k1-bf-fsync-campaign/case.json"), f);
        let e = err(load(&c, &g));
        assert!(e.contains(want), "{name}: {e}");
    }
    // 入力だけの case: 外部入力の hash 欠落
    let (_t, c, g) = copy();
    rw(&c.join("dev/k3-locusta-ab-analysis/case.json"), |v| {
        v["inputs"][0]["sha256"] = json!("zz")
    });
    assert!(err(load(&c, &g)).contains("external input"));
    // private: 欠陥 list が空の real case
    let (t, c, g) = copy();
    rw(
        &g.join("final/k4-cmp4-validity-analysis/private.json"),
        |v| v["known_defects"] = json!([]),
    );
    update_hashes_in(&t);
    assert!(err(load(&c, &g)).contains("known_defects"));
}

#[test]
fn external_input_hash_is_verified() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("raw.tgz");
    fs::write(&f, b"raw bytes").unwrap();
    let good = format!("{:x}", Sha256::digest(b"raw bytes"));
    verify_file_sha256(&f, &good).unwrap();
    assert!(
        verify_file_sha256(&f, &"0".repeat(64))
            .unwrap_err()
            .contains("mismatch")
    );
    fs::write(&f, b"tampered").unwrap();
    assert!(verify_file_sha256(&f, &good).is_err());
}

#[test]
fn real_cases_pin_distinct_inputs_between_splits() {
    let set = load(
        &root().join("cases"),
        &root().join("fixtures").join("grader-private"),
    )
    .unwrap();
    let key = |c: &celeris_campaign::cases::LoadedCase| {
        let mut k: Vec<String> = c.case.inputs.iter().map(|i| i.sha256.clone()).collect();
        if let Some(i) = &c.case.initial {
            k.push(i.commit.clone());
            k.push(i.archive_sha256.clone());
        }
        k
    };
    for d in set
        .cases
        .iter()
        .filter(|c| c.case.split == Split::Dev && c.case.kind == CaseKind::Real)
    {
        for f in set.cases.iter().filter(|c| c.case.split == Split::Final) {
            for k in key(d) {
                assert!(
                    !key(f).contains(&k),
                    "{} and {} share {k}",
                    d.case.id,
                    f.case.id
                );
            }
        }
    }
}

#[test]
fn deferral_marker_dir_is_not_a_case() {
    // status.json だけの dir（延期記録）は case として読まれず、空 dir は従来どおりエラー。
    let root = std::env::temp_dir().join(format!("deferral-marker-{}", std::process::id()));
    let (cases, grader) = (root.join("cases"), root.join("grader"));
    std::fs::create_dir_all(cases.join("dev/short-ml-campaign")).unwrap();
    std::fs::create_dir_all(&grader).unwrap();
    std::fs::write(cases.join("dev/short-ml-campaign/status.json"), "{}").unwrap();
    let e = celeris_campaign::cases::CaseSet::load(&cases, &grader).unwrap_err();
    assert_eq!(
        e, "no cases found",
        "marker dir must be skipped, not parsed"
    );
    std::fs::create_dir_all(cases.join("dev/empty")).unwrap();
    let e = celeris_campaign::cases::CaseSet::load(&cases, &grader).unwrap_err();
    assert!(e.contains("case.json"), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}
