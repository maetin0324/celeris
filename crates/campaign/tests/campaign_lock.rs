//! campaign lock の共通契約（ADR 2026-10-17-campaign-lock）。公開 API（`lock` / `lock_tree`）だけで試験する。
use std::{fs, path::Path, process::Command};

use celeris_campaign::{
    lock::{CampaignLock, LockError, LockedInitial, TestVisibility, check_successor, verify_lock},
    lock_tree::{TreeProofError, archive_content_sha256, derive_initial, verify_archive},
};
use serde_json::{Value, json};

fn h(c: char) -> String {
    c.to_string().repeat(64)
}
fn g(c: char) -> String {
    c.to_string().repeat(40)
}

fn case_json(id: &str) -> Value {
    json!({
        "id": id, "split": "dev", "implementation": true,
        "initial": {
            "commit": g('a'), "tree": g('b'), "content_sha256": h('1'),
            "submodules": [
                {"path": "ext/b", "commit": g('c'), "content_sha256": h('2')},
                {"path": "ext/a", "commit": g('d'), "content_sha256": h('3')}
            ]
        },
        "inputs": [{"name": "data.tgz", "sha256": h('4')}, {"name": "aux.txt", "sha256": h('9')}],
        "grader": {"private_sha256": h('5'), "grade_case_sha256": h('6')},
        "tests": [
            {"path": "tests/pub.rs", "visibility": "public", "sha256": h('7')},
            {"path": "tests/hid.rs", "visibility": "hidden", "sha256": h('8')}
        ]
    })
}

fn cfg(id: &str, harness: &str) -> Value {
    json!({"id": id, "harness": harness, "harness_version": "1", "model": "m",
           "model_version": "1", "policy_version": "p1"})
}

fn lock_json() -> Value {
    json!({
        "schema": "celeris.campaign-lock/1", "version": "1", "measurement_id": "m-1",
        "campaign_sha256": h('c'), "materials_sha256": h('e'), "kb_snapshot": "kb@1",
        "cases": [case_json("p1"), case_json("p0")],
        "execution": {
            "configs": [cfg("C", "celeris"), cfg("A", "claude-code"), cfg("B", "celeris")],
            "budget": {"token_basis": "input_output", "max_tokens": 10000000, "max_wall_secs": 86400,
                       "max_disk_bytes": 1099511627776u64, "max_parallel": 4,
                       "per_case": {"max_tokens": 1000000, "max_wall_secs": 3600},
                       "per_arm": {"max_tokens": 5000000, "max_wall_secs": 43200}},
            "termination": {"trial_timeout_secs": 1800, "max_attempts_per_trial": 1, "stop_on_budget": true},
            "order": "abc_rotation", "seed": 0, "repeats": 1
        },
        "sha256": ""
    })
}

fn build(v: Value) -> CampaignLock {
    serde_json::from_value(v).expect("lock json")
}

fn sealed() -> CampaignLock {
    build(lock_json()).seal().expect("seal")
}

fn reseal(mut l: CampaignLock) -> CampaignLock {
    l.sha256 = String::new();
    l.seal().expect("seal")
}

#[test]
fn hash_is_stable_across_input_order_and_seal_rejects_edits() {
    let l = sealed();
    assert_eq!(l.sha256, l.lock_hash().expect("hash"));
    let mut r = l.clone();
    r.cases.reverse();
    for c in &mut r.cases {
        c.tests.reverse();
        c.inputs.reverse();
        c.initial.as_mut().expect("initial").submodules.reverse();
    }
    r.execution.configs.reverse();
    assert_eq!(r.lock_hash().expect("hash"), l.sha256);
    assert_eq!(r.normalized().lock_hash().expect("hash"), l.sha256);
    verify_lock(&l, &reseal(r)).expect("reordered lock verifies");

    // seal 後に中身だけ書き換えても通らない。
    let mut t = l.clone();
    t.cases[0].tests[0].sha256 = h('f');
    assert!(matches!(
        verify_lock(&l, &t),
        Err(LockError::SealMismatch(_))
    ));
    assert!(matches!(
        verify_lock(&t, &l),
        Err(LockError::SealMismatch(_))
    ));
    // JSON 往復で hash は変わらない。
    let back: CampaignLock =
        serde_json::from_str(&serde_json::to_string(&l).expect("ser")).expect("de");
    verify_lock(&l, &back).expect("round trip");
}

#[test]
fn duplicates_unpinned_versions_and_empty_tests_are_rejected() {
    type Edit = Box<dyn Fn(&mut Value)>;
    let edits: Vec<(&str, Edit)> = vec![
        ("dup case", Box::new(|v| v["cases"][1]["id"] = json!("p1"))),
        (
            "dup submodule path",
            Box::new(|v| v["cases"][0]["initial"]["submodules"][1]["path"] = json!("ext/b")),
        ),
        (
            "dup test path",
            Box::new(|v| v["cases"][0]["tests"][1]["path"] = json!("tests/pub.rs")),
        ),
        (
            "dup input",
            Box::new(|v| v["cases"][0]["inputs"][1]["name"] = json!("data.tgz")),
        ),
        (
            "empty tree",
            Box::new(|v| v["cases"][0]["initial"]["tree"] = json!("")),
        ),
        (
            "short commit",
            Box::new(|v| v["cases"][0]["initial"]["commit"] = json!("abc")),
        ),
        (
            "empty submodule hash",
            Box::new(|v| v["cases"][0]["initial"]["submodules"][0]["content_sha256"] = json!("")),
        ),
        (
            "empty input hash",
            Box::new(|v| v["cases"][0]["inputs"][0]["sha256"] = json!("")),
        ),
        (
            "empty grader",
            Box::new(|v| v["cases"][0]["grader"]["grade_case_sha256"] = json!("")),
        ),
        (
            "empty measurement",
            Box::new(|v| v["measurement_id"] = json!("")),
        ),
        ("empty version", Box::new(|v| v["version"] = json!(""))),
        ("empty kb", Box::new(|v| v["kb_snapshot"] = json!(""))),
        ("no cases", Box::new(|v| v["cases"] = json!([]))),
        (
            "wrong schema",
            Box::new(|v| v["schema"] = json!("celeris.campaign-lock/0")),
        ),
        (
            "escaping path",
            Box::new(|v| v["cases"][0]["initial"]["submodules"][0]["path"] = json!("../x")),
        ),
        (
            "no public test without an initial tree",
            Box::new(|v| {
                v["cases"][0]["initial"] = Value::Null;
                v["cases"][0]["tests"] = json!([lock_json()["cases"][0]["tests"][1]]);
            }),
        ),
        (
            "no hidden test",
            Box::new(|v| v["cases"][0]["tests"] = json!([lock_json()["cases"][0]["tests"][0]])),
        ),
        (
            "empty test list",
            Box::new(|v| v["cases"][0]["tests"] = json!([])),
        ),
    ];
    for (name, e) in &edits {
        let mut v = lock_json();
        e(&mut v);
        let l = build(v);
        assert!(l.validate().is_err(), "{name}: validate");
        assert!(l.clone().seal().is_err(), "{name}: seal");
    }
    // 版違い・測定 ID 違いは内容が同じでも拒否。
    let l = sealed();
    let mut v = lock_json();
    v["version"] = json!("2");
    let other = build(v).seal().expect("seal");
    assert!(matches!(
        verify_lock(&l, &other),
        Err(LockError::VersionMismatch { .. })
    ));
    let mut v = lock_json();
    v["measurement_id"] = json!("m-2");
    let other = build(v).seal().expect("seal");
    assert!(matches!(
        verify_lock(&l, &other),
        Err(LockError::MeasurementMismatch { .. })
    ));
    let t = &l.cases[0].tests;
    assert!(
        t.iter().any(|x| x.visibility == TestVisibility::Public)
            && t.iter().any(|x| x.visibility == TestVisibility::Hidden)
    );
}

#[test]
fn same_measurement_id_content_change_is_rejected_and_successor_needs_new_id() {
    let l = sealed();
    type Edit = Box<dyn Fn(&mut Value)>;
    let edits: Vec<(&str, Edit)> = vec![
        (
            "hidden test",
            Box::new(|v| v["cases"][0]["tests"][1]["sha256"] = json!(h('f'))),
        ),
        (
            "grader",
            Box::new(|v| v["cases"][0]["grader"]["private_sha256"] = json!(h('f'))),
        ),
        (
            "input",
            Box::new(|v| v["cases"][0]["inputs"][0]["sha256"] = json!(h('f'))),
        ),
        (
            "submodule content",
            Box::new(|v| {
                v["cases"][0]["initial"]["submodules"][0]["content_sha256"] = json!(h('f'))
            }),
        ),
        (
            "materials",
            Box::new(|v| v["materials_sha256"] = json!(h('f'))),
        ),
        (
            "execution",
            Box::new(|v| v["execution"]["repeats"] = json!(3)),
        ),
        (
            "extra case",
            Box::new(|v| v["cases"].as_array_mut().expect("a").push(case_json("p2"))),
        ),
    ];
    for (name, e) in &edits {
        let mut v = lock_json();
        e(&mut v);
        let changed = build(v).seal().expect("seal");
        assert!(verify_lock(&l, &changed).is_err(), "{name}: verify");
        // 同じ測定 ID のままでは後継にならない。
        assert!(
            matches!(
                check_successor(&l, &changed),
                Err(LockError::NotSuccessor(_))
            ),
            "{name}: successor"
        );
        // 新しい測定 ID なら後継になるが、旧 lock は変わらず旧測定として検証できる。
        let mut c2 = changed.clone();
        c2.measurement_id = "m-2".into();
        let c2 = reseal(c2);
        check_successor(&l, &c2).expect("new id is a successor");
        verify_lock(&l, &sealed()).expect("old lock untouched");
        assert!(verify_lock(&l, &c2).is_err());
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn mkrepo(dir: &Path, files: &[(&str, &str)]) -> String {
    fs::create_dir_all(dir).expect("mkdir");
    git(dir, &["init", "-q"]);
    for (p, c) in files {
        let f = dir.join(p);
        fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
        fs::write(f, c).expect("write");
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "c"]);
    git(dir, &["rev-parse", "HEAD"])
}

/// 本体 + gitlink 1 つの repo と、その submodule repo。
struct Fixture {
    _tmp: tempfile::TempDir,
    main: std::path::PathBuf,
    sub: std::path::PathBuf,
    root: std::path::PathBuf,
    expected: LockedInitial,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tmp");
    let sub = tmp.path().join("sub");
    let sub_commit = mkrepo(&sub, &[("lib.rs", "pub fn a() {}\n"), ("d/x.txt", "x")]);
    let main = tmp.path().join("main");
    mkrepo(&main, &[("README", "hi\n"), ("src/m.rs", "fn main() {}\n")]);
    git(
        &main,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{sub_commit},vendor/sub"),
        ],
    );
    git(&main, &["commit", "-q", "-m", "gitlink"]);
    let sub2 = sub.clone();
    let expected = derive_initial(&main, "HEAD", &move |p| {
        (p == "vendor/sub").then(|| sub2.clone())
    })
    .expect("derive");
    // git 由来: git archive 展開 + submodule を別に展開（.git 無し）。
    let root = tmp.path().join("arch");
    fs::create_dir_all(root.join("vendor/sub")).expect("mkdir");
    extract(&main, &root);
    extract(&sub, &root.join("vendor/sub"));
    Fixture {
        _tmp: tmp,
        main,
        sub,
        root,
        expected,
    }
}

fn extract(repo: &Path, dest: &Path) {
    let a = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["archive", "HEAD"])
        .output()
        .expect("archive");
    assert!(a.status.success());
    let tar = dest.join("..").join(format!(
        "{}.tar",
        dest.file_name().expect("n").to_string_lossy()
    ));
    fs::write(&tar, &a.stdout).expect("write tar");
    let st = Command::new("tar")
        .arg("-xf")
        .arg(&tar)
        .arg("-C")
        .arg(dest)
        .status()
        .expect("tar");
    assert!(st.success());
    fs::remove_file(tar).expect("rm");
}

#[test]
fn git_derived_and_metadata_free_archive_verify_alike_and_tampering_is_rejected() {
    let f = fixture();
    assert_eq!(f.expected.submodules.len(), 1);
    verify_archive(&f.expected, &f.root).expect("archive without .git");

    // metadata の有無は理由にならない: root と submodule に .git を足しても通る。
    let with_meta = f._tmp.path().join("meta");
    fs::create_dir_all(&with_meta).expect("mkdir");
    git(&with_meta, &["init", "-q"]);
    let status = Command::new("cp")
        .arg("-a")
        .arg(format!("{}/.", f.root.display()))
        .arg(&with_meta)
        .status()
        .expect("cp");
    assert!(status.success());
    fs::write(
        with_meta.join("vendor/sub/.git"),
        "gitdir: ../../.git/modules/sub\n",
    )
    .expect("w");
    verify_archive(&f.expected, &with_meta).expect("archive with metadata");
    assert_eq!(
        archive_content_sha256(&with_meta, &["vendor/sub".into()]).expect("h"),
        f.expected.content_sha256
    );

    // 欠落
    let d = f._tmp.path().join("t1");
    Command::new("cp")
        .arg("-a")
        .arg(&f.root)
        .arg(&d)
        .status()
        .expect("cp");
    fs::remove_dir_all(d.join("vendor/sub")).expect("rm");
    assert!(matches!(
        verify_archive(&f.expected, &d),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
    // 差替え（別 repo の内容）
    let d = f._tmp.path().join("t2");
    Command::new("cp")
        .arg("-a")
        .arg(&f.root)
        .arg(&d)
        .status()
        .expect("cp");
    fs::write(d.join("vendor/sub/lib.rs"), "pub fn b() {}\n").expect("w");
    assert!(verify_archive(&f.expected, &d).is_err());
    // 1 byte 修正（本体）
    let d = f._tmp.path().join("t3");
    Command::new("cp")
        .arg("-a")
        .arg(&f.root)
        .arg(&d)
        .status()
        .expect("cp");
    fs::write(d.join("README"), "hi!\n").expect("w");
    assert!(matches!(
        verify_archive(&f.expected, &d),
        Err(TreeProofError::ContentMismatch(_))
    ));
    // 1 byte 修正（submodule）
    let d = f._tmp.path().join("t4");
    Command::new("cp")
        .arg("-a")
        .arg(&f.root)
        .arg(&d)
        .status()
        .expect("cp");
    fs::write(d.join("vendor/sub/d/x.txt"), "y").expect("w");
    assert!(verify_archive(&f.expected, &d).is_err());
    // 追加（submodule 内の余分な file、本体の余分な submodule 風 dir）
    let d = f._tmp.path().join("t5");
    Command::new("cp")
        .arg("-a")
        .arg(&f.root)
        .arg(&d)
        .status()
        .expect("cp");
    fs::write(d.join("vendor/sub/extra.txt"), "e").expect("w");
    assert!(verify_archive(&f.expected, &d).is_err());
    let d = f._tmp.path().join("t6");
    Command::new("cp")
        .arg("-a")
        .arg(&f.root)
        .arg(&d)
        .status()
        .expect("cp");
    fs::create_dir_all(d.join("vendor/other")).expect("mkdir");
    fs::write(d.join("vendor/other/lib.rs"), "pub fn a() {}\n").expect("w");
    assert!(verify_archive(&f.expected, &d).is_err());
    // 渡された repo を期待値にしない: 別 commit の repo は lock に照合されて拒否。
    celeris_campaign::lock_tree::verify_repo(&f.expected, &f.main).expect("pinned commit");
    let other = f._tmp.path().join("other");
    mkrepo(
        &other,
        &[("README", "hi!\n"), ("src/m.rs", "fn main() {}\n")],
    );
    assert!(celeris_campaign::lock_tree::verify_repo(&f.expected, &other).is_err());
    let _ = &f.sub;
}

const K2: &str = "cases/final/k2-bf-xattr-restart-campaign";
const ASSETS: &str = "/local/celeris/data/campaign-assets/k2";

#[test]
fn k2_initial_lock_is_a_valid_locked_initial_inside_a_lock() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(K2)
        .join("initial.lock.json");
    let init: LockedInitial =
        serde_json::from_str(&fs::read_to_string(p).expect("read")).expect("json");
    assert_eq!(init.submodules.len(), 5);
    let mut v = lock_json();
    v["cases"] = json!([{
        "id": "k2-bf-xattr-restart", "split": "final", "implementation": true,
        "initial": serde_json::to_value(&init).expect("v"),
        "inputs": [],
        "grader": {"private_sha256": h('5'), "grade_case_sha256": h('6')},
        "tests": [
            {"path": "tests/pub.rs", "visibility": "public", "sha256": h('7')},
            {"path": "tests/hid.rs", "visibility": "hidden", "sha256": h('8')}
        ]
    }]);
    let expected = build(v.clone()).seal().expect("K2 initial pins validly");
    // 同じ LockedInitial を別経路で組んだ actual は通り、submodule hash 1 つの差は拒否される。
    verify_lock(&expected, &build(v.clone()).seal().expect("seal")).expect("same");
    v["cases"][0]["initial"]["submodules"][2]["content_sha256"] = json!(h('0'));
    let bad = build(v).seal().expect("seal");
    assert!(matches!(
        verify_lock(&expected, &bad),
        Err(LockError::SubmoduleMismatch(_))
    ));
}

/// 資産（読み取り専用）が無い host では archive 照合は k2_pin.rs（資産が無ければ失敗する）に任せ、ここでは失敗にしない。
#[test]
fn k2_archive_verifies_when_approved_assets_exist() {
    let a = Path::new(ASSETS);
    let bundle = a.join("benchfs-086c4618.bundle");
    if !bundle.is_file() {
        eprintln!("approved K2 assets absent: archive verification is covered by k2_pin.rs");
        return;
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(K2)
        .join("initial.lock.json");
    let init: LockedInitial =
        serde_json::from_str(&fs::read_to_string(p).expect("read")).expect("json");
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    let st = Command::new("git")
        .args(["clone", "-q"])
        .arg(&bundle)
        .arg(&repo)
        .status()
        .expect("git");
    assert!(st.success());
    git(&repo, &["checkout", "-q", &init.commit]);
    let mut subs: Vec<_> = init.submodules.iter().collect();
    subs.sort_by_key(|s| s.path.matches('/').count());
    for s in subs {
        let tar = a
            .join("submodules")
            .join(celeris_campaign::asset_assembly::snapshot_name(s));
        if !tar.is_file() {
            eprintln!("SKIPPED(asset 未配置): {}", tar.display());
            return;
        }
        let dest = repo.join(&s.path);
        fs::create_dir_all(&dest).expect("mkdir");
        assert!(
            Command::new("tar")
                .arg("-xf")
                .arg(tar)
                .arg("-C")
                .arg(&dest)
                .status()
                .expect("tar")
                .success()
        );
    }
    verify_archive(&init, &repo).expect("with .git (clone)");
    // metadata 無し: .git を除いた写しでも同じ内容として通る。
    let bare = tmp.path().join("bare");
    fs::create_dir_all(&bare).expect("mkdir");
    assert!(
        Command::new("tar")
            .args(["-C"])
            .arg(&repo)
            .args(["--exclude=./.git", "-cf", "-", "."])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                let out = c.stdout.take().expect("out");
                let s = Command::new("tar")
                    .arg("-xf")
                    .arg("-")
                    .arg("-C")
                    .arg(&bare)
                    .stdin(out)
                    .status();
                c.wait()?;
                s
            })
            .expect("copy")
            .success()
    );
    verify_archive(&init, &bare).expect("without .git");
}

#[test]
fn execution_pin_fixes_active_arms_only_and_carries_the_set_hash() {
    use celeris_campaign::{lock::ExecutionPin, manifest::CampaignManifest};
    let mut m: Value = json!({
        "schema": "celeris.campaign-manifest/1", "campaign_id": "c1", "version": "v1", "split": "final",
        "input": {"path": "input.json", "sha256": "00"}, "problems": ["p0"],
        "configs": [cfg("A", "claude-code"), cfg("B", "celeris")],
        "primary": ["A", "B"], "secondary": {"pair": ["B", "C"], "subset": []},
        "experiment_sets": {"main": {"arms": ["A", "B"], "compare": [["A", "B"]]}},
        "budget": {"token_basis": "input_output", "max_tokens": 1000, "max_wall_secs": 10, "max_disk_bytes": 1,
            "max_parallel": 1, "per_case": {"max_tokens": 1, "max_wall_secs": 1},
            "per_arm": {"max_tokens": 1, "max_wall_secs": 1}},
        "termination": {"trial_timeout_secs": 1, "max_attempts_per_trial": 1, "stop_on_budget": true},
        "snapshot": {"repo_commit": "a", "materials_sha256": h('e'), "kb_snapshot": "k"},
    });
    let ab: CampaignManifest = serde_json::from_value(m.clone()).expect("manifest");
    let pin = ExecutionPin::from_manifest(&ab);
    assert_eq!(pin.configs.len(), 2);
    assert_eq!(pin.sets_sha256, ab.sets_sha256().expect("sets hash"));
    // 宣言を替えると（B/C のみ）有効 arm も set hash も変わる。
    m["configs"] = json!([cfg("B", "celeris"), cfg("C", "celeris")]);
    m["experiment_sets"] = json!({"main": {"arms": ["B", "C"], "compare": [["B", "C"]]}});
    m["secondary"]["subset"] = json!(["p0"]);
    let bc: CampaignManifest = serde_json::from_value(m).expect("manifest");
    let pin_bc = ExecutionPin::from_manifest(&bc);
    assert_eq!(pin_bc.configs.len(), 2);
    assert_ne!(pin_bc.sets_sha256, pin.sets_sha256);
    assert_ne!(pin_bc, pin);
    // 宣言なし（従来）は set hash 欄を持たず、直列化にも出ない（既存 lock の hash は変わらない）。
    let legacy = ExecutionPin::from_manifest(&{
        let mut x = bc.clone();
        x.experiment_sets.clear();
        x.selected_sets.clear();
        x.configs = vec![];
        x
    });
    assert!(legacy.sets_sha256.is_empty());
    assert!(
        serde_json::to_value(&legacy)
            .expect("v")
            .get("sets_sha256")
            .is_none()
    );
}
