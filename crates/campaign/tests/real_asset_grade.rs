//! K2/K4 を実資産の手順で採点できるかの無 LLM 検証（ADR 2026-10-18 本測定移行 P1-3）。
//! (1) fixture: 同じ手順（初期 tree → 隠しテスト挿入 → 実行 → 分類、raw → 評価側再計算）を最小の正負 tree/raw で通す。
//! (2) env_failure と model_failure の分離。(3) grader/入力を変えたら lock と合わない（旧 lock で採点できない）。
//! (4) 実物検証: `CAMPAIGN_ASSET_DIR=<dir>` のときだけ asset store の実物で同じ手順を通す
//!     （`sh crates/campaign/scripts/verify-real-assets.sh <asset_dir>` が設定する。設定時は欠落・不一致を必ず失敗にする）。
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use celeris_campaign::{
    grade_runner::ProcessRunner,
    grader::{
        CommandOutput, CommandRunner, FailureClass, GradeCase, GradeOptions, GradeReport, Status,
        Verdict,
        case::{Agg, Source},
        grade,
    },
    lock::{GraderPin, LockedCase, LockedTest, TestVisibility},
    lock_exec::verify_grade_case,
    manifest::Split,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const K2: &str = "k2-bf-xattr-restart-campaign";
const K4: &str = "k4-cmp4-validity-analysis";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn case_dir(id: &str) -> PathBuf {
    root().join("fixtures/grade-cases/final").join(id)
}

fn load(id: &str) -> GradeCase {
    GradeCase::load(&case_dir(id).join(format!("{id}.json"))).expect("grade-case")
}

fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).expect("read")))
}

fn w(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, s).expect("write");
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for e in fs::read_dir(from).expect("read_dir") {
        let e = e.expect("entry");
        let t = to.join(e.file_name());
        if e.file_type().expect("type").is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            fs::copy(e.path(), &t).expect("copy");
        }
    }
}

// ---- 固定出力を返す実行器（env/model 分類の試験用） ----

struct Canned(Result<CommandOutput, String>);

/// 初期 tree の baseline build（`--no-run`）は通る。評価 command には固定出力を返す。
impl CommandRunner for Canned {
    fn run(&self, argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
        if argv.iter().any(|a| a == "--no-run") {
            return Ok(CommandOutput {
                exit: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        self.0.clone()
    }
}

/// 初期 tree 自体が compile できない（baseline も同じ rustc error で落ちる）実行器。呼ばれた argv を記録する。
struct BrokenInitial {
    err: CommandOutput,
    calls: std::cell::RefCell<Vec<Vec<String>>>,
}

impl CommandRunner for BrokenInitial {
    fn run(&self, argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
        self.calls.borrow_mut().push(argv.to_vec());
        Ok(self.err.clone())
    }
}

fn out(exit: i32, stdout: &str, stderr: &str) -> Canned {
    Canned(Ok(CommandOutput {
        exit: Some(exit),
        stdout: stdout.into(),
        stderr: stderr.into(),
    }))
}

fn k2_artifact(root: &Path) -> PathBuf {
    let a = root.join("artifact");
    w(&a.join("src/lib.rs"), "// minimal\n");
    a
}

fn grade_k2(case_dir: &Path, runner: &dyn CommandRunner) -> GradeReport {
    let t = tempfile::tempdir().expect("tmp");
    let init = t.path().join("initial");
    w(&init.join("src/lib.rs"), "// initial\n");
    grade_k2_with(case_dir, runner, Some(init))
}

fn grade_k2_with(
    case_dir: &Path,
    runner: &dyn CommandRunner,
    initial_tree: Option<PathBuf>,
) -> GradeReport {
    let t = tempfile::tempdir().expect("tmp");
    let a = k2_artifact(t.path());
    let g = GradeCase::load(&case_dir.join(format!("{K2}.json"))).expect("load");
    let scratch = t.path().join("scratch");
    fs::create_dir_all(&scratch).expect("scratch");
    grade(
        &g,
        &a,
        &GradeOptions {
            case_dir: case_dir.to_path_buf(),
            work_root: scratch,
            runner,
            observed_jobs: None,
            initial_tree,
        },
    )
}

fn class(r: &GradeReport) -> Option<FailureClass> {
    r.failure_class
}

#[test]
fn build_environment_defects_are_env_failure_not_model_failure() {
    let cd = case_dir(K2);
    for (name, o) in [
        (
            "system library missing",
            out(
                101,
                "",
                "error: failed to run custom build command for `ucx-sys v0.1.0`\n  could not find system library 'ucx'\n",
            ),
        ),
        (
            "linker missing",
            out(101, "", "error: linker `cc` not found\n"),
        ),
        (
            "dependency fetch",
            out(
                101,
                "",
                "error: failed to get `tokio` as a dependency of package `benchfs`\n",
            ),
        ),
        (
            "offline dependency resolution",
            out(
                101,
                "",
                "error: no matching package named `async-backtrace` found\nnote: offline mode (via `--offline`) can sometimes cause surprising resolution failures\n",
            ),
        ),
        (
            "dependency build script cannot find libclang",
            out(
                101,
                "",
                "error: failed to run custom build command for `ucx1-sys v0.1.0`\n  thread 'main' panicked at bindgen-0.66.1/lib.rs:604:31:\n  Unable to find libclang\n",
            ),
        ),
        (
            "runner cannot spawn",
            Canned(Err("spawn sh: not found".into())),
        ),
    ] {
        let r = grade_k2(&cd, &o);
        assert_eq!(r.verdict, Verdict::Fail, "{name}");
        assert_eq!(class(&r), Some(FailureClass::EnvFailure), "{name}");
        assert!(r.checks.implementation.env_failure, "{name}");
        assert!(!r.final_pass);
    }
}

/// host-build2（2026-10-18）の実出力の形: 初期 tree が `--features transport-locusta` 無しでは compile できない。
fn k2_initial_tree_compile_error() -> CommandOutput {
    CommandOutput {
        exit: Some(101),
        stdout: String::new(),
        stderr: "error[E0433]: failed to resolve: use of unresolved module or unlinked crate `rrrpc`\n  --> src/ffi/io_buffer.rs:13:5\nerror[E0599]: no method named `locusta_transport_ref` found for reference `&std::rc::Rc<ConnectionPool>` in the current scope\n  --> src/ffi/io_buffer.rs:45:36\nerror: could not compile `benchfs` (lib test) due to 8 previous errors; 5 warnings emitted\n".into(),
    }
}

#[test]
fn initial_tree_compile_error_is_env_failure_not_model_failure() {
    let cd = case_dir(K2);
    let g = load(K2);
    let baseline = g
        .implementation
        .as_ref()
        .and_then(|i| i.baseline_command.clone())
        .expect("K2 は初期 tree の baseline build を持つ");
    let r = BrokenInitial {
        err: k2_initial_tree_compile_error(),
        calls: Default::default(),
    };
    let t = tempfile::tempdir().expect("tmp");
    let init = t.path().join("initial");
    w(&init.join("src/lib.rs"), "// initial\n");
    let rep = grade_k2_with(&cd, &r, Some(init));
    assert_eq!(rep.verdict, Verdict::Fail);
    assert_eq!(
        class(&rep),
        Some(FailureClass::EnvFailure),
        "初期 tree 由来の compile error をモデル失敗に数えない"
    );
    assert!(rep.checks.implementation.env_failure);
    assert!(
        rep.checks
            .implementation
            .reasons
            .iter()
            .any(|x| x.contains("initial tree does not build")),
        "{:?}",
        rep.checks.implementation.reasons
    );
    assert_eq!(
        rep.checks.implementation.details["baseline"]["initial_tree_builds"],
        json!(false)
    );
    let calls = r.calls.borrow();
    assert_eq!(calls.len(), 2, "評価と baseline の 2 回: {calls:?}");
    assert_eq!(calls[1], baseline, "baseline は隠しテスト挿入なしの build");
    // 初期 tree を渡されていない grader は出どころを決められない: モデル失敗に数えず採点し直させる。
    let rep = grade_k2_with(
        &cd,
        &out(
            101,
            "",
            "error[E0425]: cannot find function `lookup_metadata`\n",
        ),
        None,
    );
    assert_eq!(
        class(&rep),
        Some(FailureClass::Undetermined),
        "初期 tree 未指定は環境不良とも成果物の失敗とも決めない"
    );
    assert!(!rep.checks.implementation.env_failure);
    assert!(rep.checks.implementation.undetermined);
    assert!(
        rep.checks
            .implementation
            .reasons
            .iter()
            .any(|x| x.contains("cannot be attributed")),
        "{:?}",
        rep.checks.implementation.reasons
    );
}

#[test]
fn k2_build_command_enables_the_transport_feature_the_initial_tree_needs() {
    // 初期 commit 086c4618 の src/ffi/io_buffer.rs 等は rrrpc（optional, transport-locusta）を cfg なしで使う。
    // feature 無しの `cargo test --release --lib` は初期 tree 自体が compile できない（host-build2）。
    let g = load(K2);
    let imp = g.implementation.as_ref().expect("implementation");
    for (what, argv) in [
        ("command", &imp.command),
        ("baseline", imp.baseline_command.as_ref().expect("baseline")),
    ] {
        let i = argv.iter().position(|a| a == "--features").expect(what);
        assert_eq!(argv[i + 1], "transport-locusta", "{what}: {argv:?}");
        assert!(
            argv.iter().any(|a| a == "--release") && argv.iter().any(|a| a == "--lib"),
            "{what}"
        );
    }
    assert!(
        imp.baseline_command
            .as_ref()
            .expect("b")
            .iter()
            .any(|a| a == "--no-run")
    );
    assert!(
        !imp.baseline_command
            .as_ref()
            .expect("b")
            .iter()
            .any(|a| a.contains(".grader")),
        "baseline は隠しテストを挿入しない"
    );
    let cj: Value = serde_json::from_str(
        &fs::read_to_string(root().join("cases/final").join(K2).join("case.json")).expect("case"),
    )
    .expect("json");
    assert_eq!(
        cj["visible_command"],
        json!("cargo test --release --features transport-locusta --lib"),
        "試行に見せる command も同じ build 設定"
    );
    assert!(
        cj["required_capabilities"]
            .as_array()
            .expect("caps")
            .iter()
            .any(|c| c["id"] == "rdma-verbs"),
        "transport-locusta の mlx5/ibverbs header と library を能力として固定する"
    );
}

#[test]
fn model_defects_stay_model_failure_even_when_noise_mentions_the_environment() {
    let cd = case_dir(K2);
    for (name, o) in [
        (
            "hidden test FAILED",
            out(
                101,
                "test x::k2_hidden_tests::lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks ... FAILED\n",
                "",
            ),
        ),
        (
            "rustc error in the artifact",
            out(
                101,
                "",
                "error[E0425]: cannot find function `lookup_metadata`\n",
            ),
        ),
        (
            "rustc error plus a pkg-config note",
            out(
                101,
                "",
                "note: pkg-config found nothing\nerror[E0432]: unresolved import\n",
            ),
        ),
        (
            "panic in the test",
            out(
                101,
                "",
                "thread 'x' panicked at src/rpc/metadata_ops.rs:1:1\ncommand not found in map",
            ),
        ),
        ("silent nonzero exit", out(1, "", "")),
    ] {
        let r = grade_k2(&cd, &o);
        assert_eq!(r.verdict, Verdict::Fail, "{name}");
        assert_eq!(class(&r), Some(FailureClass::ModelFailure), "{name}");
        assert!(!r.checks.implementation.env_failure, "{name}");
    }
}

#[test]
fn passing_implementation_check_is_not_env_and_a_timeout_is_not_blamed_on_the_environment() {
    let ok = format!(
        "test x::k2_hidden_tests::{} ... ok\n",
        "lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks"
    );
    let r = grade_k2(&case_dir(K2), &out(0, &ok, ""));
    assert_eq!(r.checks.implementation.status, Status::Pass);
    // K2 は experiment 工程の証拠が別に要るので全体は Fail（成果物側の不足）だが、実装検査は通り環境不良でもない。
    assert!(!r.checks.implementation.env_failure);
    assert_eq!(class(&r), Some(FailureClass::ModelFailure));
    // timeout（exit なし）は成果物の暴走もあり得る。
    let to = Canned(Ok(CommandOutput {
        exit: None,
        stdout: String::new(),
        stderr: "[grade] killed after 1800s timeout".into(),
    }));
    let r = grade_k2(&case_dir(K2), &to);
    assert_eq!(class(&r), Some(FailureClass::ModelFailure));
}

#[test]
fn missing_or_altered_grader_side_assets_are_env_failure() {
    let t = tempfile::tempdir().expect("tmp");
    // 隠しテストの grader 側の写しを改変（asset store の取り違え・欠落）。
    let altered = t.path().join("altered");
    copy_dir(&case_dir(K2), &altered);
    let hidden = altered.join("hidden/.grader/k2_metadata_ops_hidden.rs");
    let mut b = fs::read(&hidden).expect("read");
    b.extend_from_slice(b"\n// tampered\n");
    fs::write(&hidden, b).expect("write");
    let r = grade_k2(&altered, &out(0, "", ""));
    assert_eq!(class(&r), Some(FailureClass::EnvFailure));
    // 欠落。
    let missing = t.path().join("missing");
    copy_dir(&case_dir(K2), &missing);
    fs::remove_file(missing.join("hidden/.grader/run.sh")).expect("rm");
    let r = grade_k2(&missing, &out(0, "", ""));
    assert_eq!(class(&r), Some(FailureClass::EnvFailure));
    // 成果物側の削除は成果物の失敗（grader 側は無事）。
    let t2 = tempfile::tempdir().expect("tmp");
    let g = load(K2);
    let r = grade(
        &g,
        &t2.path().join("no-artifact"),
        &GradeOptions {
            case_dir: case_dir(K2),
            work_root: t2.path().to_path_buf(),
            runner: &out(0, "", ""),
            observed_jobs: None,
            initial_tree: None,
        },
    );
    assert_eq!(class(&r), Some(FailureClass::ModelFailure));
}

// ---- K4: 評価側の再計算（fixture raw） ----

/// 評価側の素朴な再計算（grader の実装は使わない）。`corrupt` は先頭指標を何倍かにする。
fn oracle(g: &GradeCase, artifact: &Path, corrupt: f64) -> String {
    let mut s = String::new();
    for (i, m) in g
        .recompute
        .as_ref()
        .expect("recompute")
        .metrics
        .iter()
        .enumerate()
    {
        let Source::Json { files, pointer } = &m.source else {
            panic!("json 指標だけ")
        };
        let xs: Vec<f64> = files
            .iter()
            .map(|f| {
                let v: Value = serde_json::from_slice(&fs::read(artifact.join(f)).expect("raw"))
                    .expect("json");
                v.pointer(pointer).and_then(Value::as_f64).expect("value")
            })
            .collect();
        let mut x = match m.agg {
            Agg::Mean => xs.iter().sum::<f64>() / xs.len() as f64,
            Agg::Count => xs.len() as f64,
            a => panic!("未対応 {a:?}"),
        };
        if i == 0 {
            x *= corrupt;
        }
        s += &format!("metric {} = {x}\n", m.name);
    }
    s
}

fn put_pointer(root: &mut Value, pointer: &str, v: f64) {
    let mut cur = root;
    for k in pointer.split('/').skip(1) {
        cur = cur
            .as_object_mut()
            .expect("object")
            .entry(k.to_string())
            .or_insert_with(|| json!({}));
    }
    *cur = json!(v);
}

fn grade_recompute(g: &GradeCase, artifact: &Path) -> Status {
    let t = tempfile::tempdir().expect("tmp");
    let r = grade(
        g,
        artifact,
        &GradeOptions {
            case_dir: case_dir(K4),
            work_root: t.path().to_path_buf(),
            runner: &celeris_campaign::grader::NoRunner,
            observed_jobs: None,
            initial_tree: None,
        },
    );
    r.checks.recompute.status
}

/// 実 K4 の指標定義（files/pointer/agg/許容差）はそのまま、raw だけ合成して pin を作り直す。
#[test]
fn k4_fixture_raw_recompute_matches_and_rejects_a_wrong_or_tampered_report() {
    let mut g = load(K4);
    let t = tempfile::tempdir().expect("tmp");
    let art = t.path().join("artifact");
    let spec = g.recompute.as_mut().expect("recompute");
    let mut seen = std::collections::BTreeMap::new();
    for (n, m) in spec.metrics.iter().enumerate() {
        let Source::Json { files, pointer } = &m.source else {
            panic!("json 指標だけ")
        };
        for (j, f) in files.iter().enumerate() {
            let v = seen.entry(f.clone()).or_insert_with(|| json!({}));
            put_pointer(v, pointer, 10.0 + (n * 7 + j) as f64 * 0.5);
        }
    }
    spec.pinned_raw.clear();
    for (f, v) in &seen {
        let text = serde_json::to_string(v).expect("ser");
        w(&art.join(f), &text);
        spec.pinned_raw
            .insert(f.clone(), format!("{:x}", Sha256::digest(text.as_bytes())));
    }
    let report = |c: f64| w(&art.join("report.md"), &oracle(&g, &art, c));
    report(1.0);
    assert_eq!(
        grade_recompute(&g, &art),
        Status::Pass,
        "評価側の再計算と一致"
    );
    report(1.06);
    assert_eq!(grade_recompute(&g, &art), Status::Fail, "6% ずれた報告");
    // pin 後に raw を書き換えると、報告が新 raw と整合していても不合格。
    report(1.0);
    let f = seen.keys().next().expect("file");
    let mut v: Value = serde_json::from_slice(&fs::read(art.join(f)).expect("r")).expect("j");
    if let Some(Source::Json { pointer, .. }) = spec_first_source(&g) {
        put_pointer(&mut v, &pointer, 999.0);
    }
    fs::write(art.join(f), serde_json::to_string(&v).expect("ser")).expect("w");
    report(1.0);
    assert_eq!(grade_recompute(&g, &art), Status::Fail, "pin と違う raw");
}

fn spec_first_source(g: &GradeCase) -> Option<Source> {
    g.recompute
        .as_ref()?
        .metrics
        .first()
        .map(|m| m.source.clone())
}

// ---- lock: grader/テストを変えたら旧 lock と合わない ----

fn lock_case_for(id: &str, dir: &Path) -> LockedCase {
    let g = load(id);
    let imp = g.implementation.as_ref().expect("implementation");
    LockedCase {
        id: id.into(),
        split: Split::Final,
        implementation: true,
        initial: None,
        inputs: vec![],
        grader: GraderPin {
            private_sha256: "0".repeat(64),
            grade_case_sha256: sha(&dir.join(format!("{id}.json"))),
        },
        tests: imp
            .hidden_tests
            .iter()
            .map(|t| LockedTest {
                path: t.path.clone(),
                visibility: TestVisibility::Hidden,
                sha256: t.sha256.clone(),
            })
            .collect(),
    }
}

#[test]
fn changed_grade_case_or_hidden_test_no_longer_matches_the_old_lock() {
    let t = tempfile::tempdir().expect("tmp");
    let dir = t.path().join("k2");
    copy_dir(&case_dir(K2), &dir);
    let old = lock_case_for(K2, &dir);
    verify_grade_case(&old, &dir.join(format!("{K2}.json"))).expect("unchanged matches the lock");
    // 隠しテストを 1 byte 変える（grade-case の宣言 hash は変えない）→ 旧 lock と不一致。
    let h = dir.join("hidden/.grader/k2_metadata_ops_hidden.rs");
    let mut b = fs::read(&h).expect("r");
    b.push(b'\n');
    fs::write(&h, b).expect("w");
    assert!(verify_grade_case(&old, &dir.join(format!("{K2}.json"))).is_err());
    // grade-case 自体の変更（受け入れ試験名の差し替え）→ 旧 lock と不一致。
    let g = dir.join(format!("{K2}.json"));
    let text = fs::read_to_string(&g).expect("r");
    fs::write(&g, text.replace("\"nodes\": 3", "\"nodes\": 2")).expect("w");
    let err = verify_grade_case(&old, &g).expect_err("grade-case changed");
    assert!(err.contains("differs from the locked"), "{err}");
    // 新しい内容は新しい lock（別の測定 ID）にだけ載る。旧 lock の記録を書き換える経路は無い。
    let newer = lock_case_for(K2, &dir);
    assert_ne!(old.grader.grade_case_sha256, newer.grader.grade_case_sha256);
}

#[test]
fn committed_case_pins_equal_the_grade_case_files_on_disk() {
    // case.json の grade_case_ref と、実際の grade-case file の hash が一致する（変えたら更新を強いる）。
    for id in [K2, K4] {
        let cj: Value = serde_json::from_str(
            &fs::read_to_string(root().join("cases/final").join(id).join("case.json"))
                .expect("case.json"),
        )
        .expect("json");
        assert_eq!(
            cj["grade_case_ref"]["sha256"].as_str().expect("sha"),
            sha(&case_dir(id).join(format!("{id}.json"))),
            "{id}: grade-case を変えたら grade_case_ref と campaign lock を更新する"
        );
    }
}

// ---- 実物検証（CAMPAIGN_ASSET_DIR 設定時のみ。設定時は欠落を必ず失敗にする） ----

fn sh(dir: &Path, prog: &str, args: &[&str]) {
    let o = Command::new(prog)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("{prog}: {e}"));
    assert!(
        o.status.success(),
        "{prog} {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn k2_lock() -> celeris_campaign::lock::LockedInitial {
    serde_json::from_str(
        &fs::read_to_string(
            root()
                .join("cases/final")
                .join(K2)
                .join("initial.lock.json"),
        )
        .expect("lock"),
    )
    .expect("lock json")
}

/// 実 asset から K2 の初期 tree を組む。snapshot 欠落・未登録・改変は env_failure（build の前に止まる）。
fn assemble_k2(assets: &Path, dest: &Path) {
    let l = k2_lock();
    celeris_campaign::asset_assembly::assemble_initial(
        &l,
        assets,
        &assets.join("benchfs-086c4618.bundle"),
        dest,
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

fn real_grade(tree: &Path, initial: &Path, runner: &dyn CommandRunner) -> GradeReport {
    let t = tempfile::tempdir().expect("tmp");
    let art = t.path().join("artifact");
    copy_dir(tree, &art.join("src"));
    let g = load(K2);
    let scratch = t.path().join("scratch");
    fs::create_dir_all(&scratch).expect("scratch");
    grade(
        &g,
        &art,
        &GradeOptions {
            case_dir: case_dir(K2),
            work_root: scratch,
            runner,
            observed_jobs: None,
            initial_tree: Some(initial.to_path_buf()),
        },
    )
}

/// 実 asset の submodule snapshot（入れ子を含む）が lock の全 submodule 分あり、SHA256SUMS に載り hash が合う。
/// build の前に走らせ、不足は env_failure（モデル失敗ではない）として止める。
#[test]
fn real_k2_snapshots_complete_before_build() {
    let Ok(dir) = std::env::var("CAMPAIGN_ASSET_DIR") else {
        eprintln!("SKIPPED(実物検証): CAMPAIGN_ASSET_DIR 未設定");
        return;
    };
    celeris_campaign::asset_assembly::check_snapshots(&k2_lock(), Path::new(&dir))
        .unwrap_or_else(|e| panic!("{e}"));
}

/// `CAMPAIGN_ASSET_DIR`（asset store の k2 dir）で、実 BenchFS の初期 tree（既知の不正成果）と
/// 修正 patch 適用後（既知の正しい成果、`CAMPAIGN_K2_FIX_PATCH`）を同じ grader で採点する。
/// `CAMPAIGN_REAL_BUILD=1` なら実 cargo で build/test、無ければ build 工程は env 確認だけ（実行不能は成功にしない）。
#[test]
fn real_k2_assets_build_and_grade_known_bad_and_known_good() {
    let Ok(dir) = std::env::var("CAMPAIGN_ASSET_DIR") else {
        eprintln!(
            "SKIPPED(実物検証): CAMPAIGN_ASSET_DIR 未設定（verify-real-assets.sh が設定する）"
        );
        return;
    };
    let patch = PathBuf::from(
        std::env::var("CAMPAIGN_K2_FIX_PATCH").unwrap_or_else(|_| format!("{dir}/k2-fix.patch")),
    );
    assert!(
        patch.is_file(),
        "known-good patch missing: {}",
        patch.display()
    );
    // known-good は測定 ID k2-known-good-v2 の定義（private.json の known_good）に固定する。
    assert_eq!(
        sha(&patch),
        "6fc5d09a880ce405f4f81cbbf559df2b2bcf191946e14040c05705de63d22773",
        "known-good patch が private.json の known_good（k2-known-good-v2）と違う"
    );
    let t = tempfile::tempdir().expect("tmp");
    let bad = t.path().join("bad");
    assemble_k2(Path::new(&dir), &bad);
    let good = t.path().join("good");
    copy_dir(&bad, &good);
    sh(&good, "git", &["apply", patch.to_str().expect("u8")]);
    // 既知の不正成果は修正の印（lookup_metadata）が無く、正しい成果にはある。
    let mark = |p: &Path| {
        fs::read_to_string(p.join("src/rpc/metadata_ops.rs"))
            .expect("metadata_ops.rs")
            .contains("fn lookup_metadata")
    };
    assert!(!mark(&bad) && mark(&good), "正負 tree の印が逆");
    // by-ID synthetic path の xattr 経路（handlers.rs）は known-good だけにある（Sirius で ok の tree と同じ内容）。
    let byid = |p: &Path| {
        fs::read_to_string(p.join("src/rpc/handlers.rs"))
            .expect("handlers.rs")
            .contains("FileId::compute_path_hash(path)")
    };
    assert!(!byid(&bad) && byid(&good), "by-ID 経路の印が逆");
    // 初期 tree は optional な rrrpc を cfg なしで使う → 評価 command が transport-locusta を有効にしていること。
    let cargo = fs::read_to_string(bad.join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        cargo.contains("transport-locusta = [\"dep:rrrpc\""),
        "初期 tree の feature 定義が想定と違う"
    );
    assert!(
        fs::read_to_string(bad.join("src/ffi/io_buffer.rs"))
            .expect("io_buffer.rs")
            .contains("use rrrpc::"),
        "初期 tree の rrrpc 依存が想定と違う"
    );
    let imp = load(K2).implementation.expect("implementation");
    assert!(
        imp.command
            .windows(2)
            .any(|w| w[0] == "--features" && w[1] == "transport-locusta"),
        "評価 command が transport-locusta を有効にしない: {:?}",
        imp.command
    );
    if std::env::var("CAMPAIGN_REAL_BUILD").as_deref() == Ok("1") {
        // 初期 tree（= 既知の不正成果）自体が評価と同じ build 設定で compile できること。落ちれば case/asset の不良。
        let baseline = imp.baseline_command.as_ref().expect("baseline");
        let b = ProcessRunner::default()
            .run(baseline, &bad)
            .unwrap_or_else(|e| panic!("env_failure: baseline could not run: {e}"));
        assert_eq!(
            b.exit,
            Some(0),
            "env_failure: 初期 tree が build できない（モデル失敗ではない）: {}",
            b.stderr
                .chars()
                .rev()
                .take(4000)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
        let r_bad = real_grade(&bad, &bad, &ProcessRunner::default());
        let r_good = real_grade(&good, &bad, &ProcessRunner::default());
        let diag =
            |r: &GradeReport| serde_json::to_string(&r.checks.implementation).unwrap_or_default();
        assert_ne!(
            class(&r_bad),
            Some(FailureClass::EnvFailure),
            "build 環境不良で正負を判定できない: {}",
            diag(&r_bad)
        );
        assert_ne!(
            class(&r_good),
            Some(FailureClass::EnvFailure),
            "build 環境不良で正負を判定できない: {}",
            diag(&r_good)
        );
        assert_eq!(
            r_bad.checks.implementation.status,
            Status::Fail,
            "{}",
            diag(&r_bad)
        );
        assert_eq!(
            r_good.checks.implementation.status,
            Status::Pass,
            "{}",
            diag(&r_good)
        );
    }
}

/// `CAMPAIGN_K4_TGZ`（固定の原票 tgz）から評価側で再計算し、grade-case の pin・期待値と一致する。
#[test]
fn real_k4_raw_recompute_matches_the_grader() {
    let Ok(tgz) = std::env::var("CAMPAIGN_K4_TGZ") else {
        eprintln!("SKIPPED(実物検証): CAMPAIGN_K4_TGZ 未設定");
        return;
    };
    let tgz = PathBuf::from(tgz);
    assert!(tgz.is_file(), "raw missing: {}", tgz.display());
    assert_eq!(
        sha(&tgz),
        "128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1",
        "raw hash が case.json の固定と違う"
    );
    let g = load(K4);
    let t = tempfile::tempdir().expect("tmp");
    let art = t.path().join("artifact");
    fs::create_dir_all(art.join("inputs")).expect("mkdir");
    sh(
        t.path(),
        "tar",
        &[
            "-xzf",
            tgz.to_str().expect("u8"),
            "-C",
            art.join("inputs").to_str().expect("u8"),
        ],
    );
    for c in [1.0, 1.06] {
        w(&art.join("report.md"), &oracle(&g, &art, c));
        let want = if c == 1.0 { Status::Pass } else { Status::Fail };
        assert_eq!(grade_recompute(&g, &art), want, "corrupt x{c}");
    }
}
