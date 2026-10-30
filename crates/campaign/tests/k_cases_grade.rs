//! K1〜K4 の実 case ごとの `celeris.grade-case/1`（fixtures/grade-cases）の存在・固定・生成物の一致と、
//! 隠しテスト挿入つきの「既知の誤実装=不合格／修正版=合格」の model 無し試験。
//! cargo は textual stub に差し替える（BenchFS は submodule 未固定・UCX 要で本 repo では build できない）。
//! stub は挿入した隠し module の宣言と、修正の印（修正版にだけ在る記述）を見るだけで、実コンパイルではない。
#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use celeris_campaign::{
    cases::{CaseKind, CaseSet},
    grade_case_gen::{GenSource, generate},
    grade_runner::ProcessRunner,
    grader::{
        CommandOutput, CommandRunner, GradeCase, GradeOptions, Status, Verdict, grade,
        implementation,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn grade_root() -> PathBuf {
    root().join("fixtures/grade-cases")
}

fn set() -> CaseSet {
    CaseSet::load(
        &support_cases::cases_root(),
        &root().join("fixtures/grader-private"),
    )
    .expect("load")
}

fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).expect("read")))
}

fn split_dir(c: &celeris_campaign::cases::Case) -> &'static str {
    match c.split {
        celeris_campaign::manifest::Split::Dev => "dev",
        _ => "final",
    }
}

/// 隠しテストを足す case の生成元（hidden の実体は `fixtures/grade-cases/<split>/<id>/hidden/`）。
fn gen_source(id: &str, dir: &Path) -> Option<GenSource> {
    let (tests, repo) = match id {
        "k1-bf-fsync-campaign" => (
            vec![
                ".grader/run.sh",
                ".grader/k1_data_ops_hidden.rs",
                ".grader/k1_chunk_store_hidden.rs",
            ],
            "src",
        ),
        "k2-bf-xattr-restart-campaign" => (
            vec![".grader/run.sh", ".grader/k2_metadata_ops_hidden.rs"],
            "src",
        ),
        _ => return None,
    };
    Some(GenSource {
        repo: repo.into(),
        public_tests: vec![],
        hidden_tests: tests.into_iter().map(String::from).collect(),
        wrapper: Some(".grader/run.sh".into()),
        files_dir: dir.to_path_buf(),
    })
}

/// 固定 hash と生成物の作り直し: `CAMPAIGN_UPDATE_CASES=1 cargo test -p celeris-campaign --test k_cases_grade -- --ignored`
#[test]
#[ignore = "grade-case と grade_case_ref の作り直し用"]
fn update_grade_cases() {
    if std::env::var("CAMPAIGN_UPDATE_CASES").as_deref() != Ok("1") {
        return;
    }
    let set = set();
    for l in set.cases.iter().filter(|l| l.case.kind == CaseKind::Real) {
        let dir = grade_root().join(split_dir(&l.case)).join(&l.case.id);
        if let Some(src) = gen_source(&l.case.id, &dir) {
            generate(&l.case, &l.private, &src, &dir).expect("generate");
        }
        let cj = root()
            .join("cases")
            .join(split_dir(&l.case))
            .join(&l.case.id)
            .join("case.json");
        let mut v: Value = serde_json::from_str(&fs::read_to_string(&cj).unwrap()).unwrap();
        v["grade_case_ref"] = json!({
            "path": format!("{}/{}/{}.json", split_dir(&l.case), l.case.id, l.case.id),
            "sha256": sha(&dir.join(format!("{}.json", l.case.id))),
        });
        fs::write(&cj, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
    }
}

#[test]
fn every_real_case_has_a_pinned_loadable_grade_case_with_a_check() {
    let set = set();
    let real: Vec<_> = set
        .cases
        .iter()
        .filter(|l| l.case.kind == CaseKind::Real)
        .collect();
    assert!(real.len() >= 4, "K1〜K4 が揃っている: {}", real.len());
    for l in real {
        let r = l.case.grade_case_ref.as_ref().expect("grade_case_ref");
        let p = grade_root().join(&r.path);
        assert_eq!(
            sha(&p),
            r.sha256,
            "{}: grade-case hash が固定とずれた",
            l.case.id
        );
        let g = GradeCase::load(&p).expect("load grade-case");
        assert_eq!(g.schema, "celeris.grade-case/1");
        assert_eq!(g.id, l.case.id);
        assert!(
            g.implementation.is_some() || g.recompute.is_some(),
            "{}: 検査が 1 つも無い",
            l.case.id
        );
        // 実装工程のある case は実装検査、実験工程のある case は実験検査を持つ。
        assert_eq!(
            g.implementation.is_some(),
            l.case.stages.iter().any(|s| s == "implementation"),
            "{}",
            l.case.id
        );
        assert_eq!(
            g.experiment.is_some(),
            l.case.stages.iter().any(|s| s == "experiment"),
            "{}",
            l.case.id
        );
    }
}

#[test]
fn missing_or_altered_grade_case_makes_the_case_set_unusable() {
    // grade_case_ref の無い real case は loader が拒否する。
    let t = tempfile::tempdir().unwrap();
    let c = t.path().join("cases");
    copy(&support_cases::cases_root(), &c);
    let g = root().join("fixtures/grader-private");
    let cj = c.join("final/k4-cmp4-validity-analysis/case.json");
    let mut v: Value = serde_json::from_str(&fs::read_to_string(&cj).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("grade_case_ref");
    fs::write(&cj, v.to_string()).unwrap();
    let e = CaseSet::load(&c, &g).expect_err("grade_case_ref 無しは拒否");
    assert!(e.contains("grade_case_ref"), "{e}");
    // 短縮 hash の submodule は拒否する。
    copy(&support_cases::cases_root(), &t.path().join("c2"));
    let cj = t
        .path()
        .join("c2/final/k2-bf-xattr-restart-campaign/case.json");
    let mut v: Value = serde_json::from_str(&fs::read_to_string(&cj).unwrap()).unwrap();
    v["initial"]["submodules"] = json!(["lib/pluvio@9ecd51ee"]);
    fs::write(&cj, v.to_string()).unwrap();
    let e = CaseSet::load(&t.path().join("c2"), &g).expect_err("短縮 submodule は拒否");
    assert!(e.contains("submodule"), "{e}");
}

#[test]
fn committed_k1_k2_grade_cases_match_a_fresh_generation() {
    let set = set();
    for id in ["k1-bf-fsync-campaign", "k2-bf-xattr-restart-campaign"] {
        let l = set.get(id).expect("case");
        let dir = grade_root().join(split_dir(&l.case)).join(id);
        let out = tempfile::tempdir().unwrap();
        generate(
            &l.case,
            &l.private,
            &gen_source(id, &dir).unwrap(),
            out.path(),
        )
        .expect("gen");
        assert_eq!(
            fs::read_to_string(out.path().join(format!("{id}.json"))).unwrap(),
            fs::read_to_string(dir.join(format!("{id}.json"))).unwrap(),
            "{id}: 固定した grade-case が現在の case/private から作る物とずれた（作り直しが要る）"
        );
        let g = GradeCase::load(&dir.join(format!("{id}.json"))).unwrap();
        let imp = g.implementation.unwrap();
        assert!(
            imp.tests.is_empty(),
            "実装内の公開テストは file 単位で固定できない"
        );
        assert_eq!(imp.command[..2], ["sh", ".grader/run.sh"]);
        assert!(!imp.hidden_tests.is_empty() && !imp.must_pass.is_empty());
        let e = g.experiment.unwrap();
        assert_eq!(e.conditions["nodes"], json!(3));
    }
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &t);
        } else {
            fs::copy(e.path(), &t).unwrap();
        }
    }
}

// ---- 隠しテスト挿入 + 既知の誤実装/修正版 ----

/// `(試験名, 修正の印を探す file, 印, 挿入される module 名)`。
type Rule = (&'static str, &'static str, &'static str, &'static str);

const K1_RULES: &[Rule] = &[
    (
        "fsync_reply_uses_registered_reply_stream",
        "src/rpc/data_ops.rs",
        "reply_vectorized(Self::reply_stream_id()",
        "k1_hidden_tests",
    ),
    (
        "test_posix_explicit_fsync_reads_back",
        "src/storage/chunk_store.rs",
        "pub async fn fsync_chunk",
        "k1_hidden_tests",
    ),
    (
        "test_posix_size_xattr_survives_store_reopen",
        "src/storage/chunk_store.rs",
        "chunk 0 missing for size xattr",
        "k1_hidden_tests",
    ),
];
const K2_RULES: &[Rule] = &[(
    "lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks",
    "src/rpc/metadata_ops.rs",
    "fn lookup_metadata",
    "k2_hidden_tests",
)];

/// 実コンパイルの代わりの stub。`cargo test ... -- <名前>` を受け、隠し module が挿入されていて修正の印が
/// あれば `test <path>::<名前> ... ok`、無ければコンパイル失敗（exit 101）を模す。
struct StubCargo {
    script: PathBuf,
}

impl StubCargo {
    fn new(dir: &Path, rules: &[Rule]) -> Self {
        let table: String = rules
            .iter()
            .map(|(n, f, needle, m)| format!("{n}|{f}|{needle}|{m}\n"))
            .collect();
        let script = dir.join("stub-cargo");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nrules='{table}'\nseen=0; fail=0\nfor n in \"$@\"; do\n  if [ \"$seen\" = 1 ]; then\n    line=$(printf '%s' \"$rules\" | grep \"^$n|\")\n    file=$(printf '%s' \"$line\" | cut -d'|' -f2); needle=$(printf '%s' \"$line\" | cut -d'|' -f3); mod=$(printf '%s' \"$line\" | cut -d'|' -f4)\n    if ! grep -q \"^mod $mod;\" \"$file\"; then echo \"hidden module $mod was not inserted into $file\" >&2; fail=1\n    elif grep -qF \"$needle\" \"$file\"; then echo \"test x::$mod::$n ... ok\"\n    else echo \"test x::$mod::$n ... FAILED\"; echo 'error: stub compile/test failure' >&2; fail=1; fi\n  fi\n  [ \"$n\" = -- ] && seen=1\ndone\n[ \"$fail\" = 0 ] || exit 101\n"
            ),
        )
        .unwrap();
        Self { script }
    }
}

impl CommandRunner for StubCargo {
    fn run(&self, argv: &[String], cwd: &Path) -> Result<CommandOutput, String> {
        // wrapper の `exec "$@"` が `cargo ...` を実行するので、`cargo` を `sh <stub>` に置き換える
        // （実行 bit を立てない: file 書込み直後の exec が ETXTBSY になる競合を避ける）。
        let a: Vec<String> = argv
            .iter()
            .flat_map(|x| {
                if x == "cargo" {
                    vec!["sh".to_string(), self.script.display().to_string()]
                } else {
                    vec![x.clone()]
                }
            })
            .collect();
        ProcessRunner::default().run(&a, cwd)
    }
}

fn w(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

/// 実コンパイルの印だけを持つ最小の木（bad は印なし、fixed は印あり）。
fn synthetic_tree(root: &Path, rules: &[Rule], fixed: bool) {
    for (_, f, needle, _) in rules {
        let mut s = fs::read_to_string(root.join(f)).unwrap_or_default();
        s.push_str(&format!("// base\n{}\n", if fixed { *needle } else { "" }));
        w(&root.join(f), &s);
    }
}

fn grade_tree(id: &str, tree: &Path, rules: &[Rule]) -> celeris_campaign::grader::GradeReport {
    let set = set();
    let l = set.get(id).unwrap();
    let dir = grade_root().join(split_dir(&l.case)).join(id);
    let g = GradeCase::load(&dir.join(format!("{id}.json"))).unwrap();
    let t = tempfile::tempdir().unwrap();
    let stub = StubCargo::new(t.path(), rules);
    // 成果物 dir は `src/` に repo を持つ（case.json の fetch 手順）。
    let artifact = t.path().join("artifact");
    copy(tree, &artifact.join("src"));
    let scratch = t.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let opts = GradeOptions {
        case_dir: dir,
        work_root: scratch,
        runner: &stub,
        observed_jobs: None,
        initial_tree: None,
    };
    grade(&g, &artifact, &opts)
}

fn impl_check(r: &celeris_campaign::grader::GradeReport) -> (Status, String) {
    (
        r.checks.implementation.status,
        serde_json::to_string(&r.checks.implementation).unwrap_or_default(),
    )
}

fn known_bad_fails_and_fixed_passes(id: &str, rules: &[Rule], bad: &Path, fixed: &Path) {
    let bad_r = grade_tree(id, bad, rules);
    let (st, d) = impl_check(&bad_r);
    assert_eq!(st, Status::Fail, "{id}: 既知の誤実装は不合格のはず: {d}");
    assert_ne!(bad_r.verdict, Verdict::Pass);
    let fixed_r = grade_tree(id, fixed, rules);
    let (st, d) = impl_check(&fixed_r);
    assert_eq!(st, Status::Pass, "{id}: 修正版は合格のはず: {d}");
    // 隠しテストは成果物に無かったが、挿入されて走った（削除判定にならない）。
    let det = &fixed_r.checks.implementation.details["hidden_inserted"];
    assert!(det.as_array().is_some_and(|a| !a.is_empty()), "{det}");
    // 実験工程の証拠が無いので試験全体は合格にならない（実装検査だけを見ている）。
    assert_ne!(fixed_r.verdict, Verdict::Pass);
    let _ = implementation::marks; // grader の公開面に依存していることの確認
}

#[test]
fn k1_known_bad_fails_and_fixed_passes_with_hidden_insertion() {
    let t = tempfile::tempdir().unwrap();
    let (bad, fixed) = (t.path().join("bad"), t.path().join("fixed"));
    synthetic_tree(&bad, K1_RULES, false);
    synthetic_tree(&fixed, K1_RULES, true);
    known_bad_fails_and_fixed_passes("k1-bf-fsync-campaign", K1_RULES, &bad, &fixed);
}

#[test]
fn k2_known_bad_fails_and_fixed_passes_with_hidden_insertion() {
    let t = tempfile::tempdir().unwrap();
    let (bad, fixed) = (t.path().join("bad"), t.path().join("fixed"));
    synthetic_tree(&bad, K2_RULES, false);
    synthetic_tree(&fixed, K2_RULES, true);
    known_bad_fails_and_fixed_passes("k2-bf-xattr-restart-campaign", K2_RULES, &bad, &fixed);
}

fn p0_dir() -> PathBuf {
    PathBuf::from(std::env::var("CAMPAIGN_P0_DIR").unwrap_or_else(|_| {
        "/local/celeris/data/workspaces/01M3QKJZ8Y8TMSQ6HXQNFDFD9Z/artifacts".into()
    }))
}

fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    o.stdout
}

/// 実 BenchFS の初期 commit（bundle から取り出す）に対する同じ確認。bundle・patch が無い環境では実施しない
/// （ここは「実行不能」であり合格ではない。進捗に明記）。
#[test]
fn real_benchfs_trees_from_the_bundle_follow_the_pinned_hashes_and_markers() {
    let bundle = p0_dir().join("p0-fix.bundle");
    let patch = p0_dir().join("p0-final.patch");
    if !bundle.is_file() || !patch.is_file() {
        eprintln!(
            "SKIPPED(実行不能): bundle/patch が無い: {}",
            p0_dir().display()
        );
        return;
    }
    let set = set();
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("bf");
    // 親 commit を欠く bundle なので clone は警告を出す。--no-checkout で commit だけ取り出す。
    let _ = Command::new("git")
        .args(["clone", "-q", "--no-checkout"])
        .arg(&bundle)
        .arg(&repo)
        .output();
    let extract = |commit: &str, to: &Path| {
        fs::create_dir_all(to).unwrap();
        let tar = git(&repo, &["archive", "--format=tar", commit]);
        let tf = t.path().join("x.tar");
        fs::write(&tf, &tar).unwrap();
        assert!(
            Command::new("tar")
                .arg("-xf")
                .arg(&tf)
                .arg("-C")
                .arg(to)
                .status()
                .unwrap()
                .success()
        );
        format!("{:x}", Sha256::digest(&tar))
    };
    let k1 = set.get("k1-bf-fsync-campaign").unwrap();
    let k2 = set.get("k2-bf-xattr-restart-campaign").unwrap();
    let (i1, i2) = (
        k1.case.initial.as_ref().unwrap(),
        k2.case.initial.as_ref().unwrap(),
    );
    let (k1_bad, k2_bad, k2_fixed) = (
        t.path().join("k1bad"),
        t.path().join("k2bad"),
        t.path().join("k2fixed"),
    );
    assert_eq!(
        extract(&i1.commit, &k1_bad),
        i1.archive_sha256,
        "K1 archive hash"
    );
    assert_eq!(
        extract(&i2.commit, &k2_bad),
        i2.archive_sha256,
        "K2 archive hash"
    );
    // K1 の修正版 = K2 の初期 commit（fix 済み）。
    let k1_fixed = t.path().join("k1fixed");
    extract(&i2.commit, &k1_fixed);
    extract(&i2.commit, &k2_fixed);
    // gitlink（submodule）の commit が case.json の固定値と一致する。
    for i in [i1, i2] {
        for m in &i.submodules {
            let (path, c) = m.split_once('@').unwrap();
            // 入れ子 submodule の gitlink は本体の木ではなく親 submodule の木にある。
            if i.submodules.iter().any(|o| {
                path.strip_prefix(o.split('@').next().unwrap())
                    .is_some_and(|r| r.starts_with('/'))
            }) {
                continue;
            }
            let ls = String::from_utf8(git(&repo, &["ls-tree", &i.commit, path])).unwrap();
            assert!(
                ls.contains(c) && ls.starts_with("160000 commit"),
                "{m}: {ls}"
            );
        }
        let tree = String::from_utf8(git(
            &repo,
            &["rev-parse", &format!("{}^{{tree}}", i.commit)],
        ))
        .unwrap();
        assert_eq!(tree.trim(), i.tree);
    }
    // K2 の修正版 = 初期 commit + p0-final.patch（git 管理外の木には -p1 で当たる）。
    assert!(
        Command::new("git")
            .arg("apply")
            .arg(&patch)
            .current_dir(&k2_fixed)
            .status()
            .unwrap()
            .success(),
        "p0-final.patch が K2 の初期 commit に当たらない"
    );
    // K2 の初期 commit（誤実装）には修正の印が無く、修正後には在る。
    assert!(
        !fs::read_to_string(k2_bad.join("src/rpc/metadata_ops.rs"))
            .unwrap()
            .contains("fn lookup_metadata")
    );
    assert!(
        fs::read_to_string(k2_fixed.join("src/rpc/metadata_ops.rs"))
            .unwrap()
            .contains("fn lookup_metadata")
    );
    known_bad_fails_and_fixed_passes("k1-bf-fsync-campaign", K1_RULES, &k1_bad, &k1_fixed);
    known_bad_fails_and_fixed_passes("k2-bf-xattr-restart-campaign", K2_RULES, &k2_bad, &k2_fixed);
}

// ---- K3/K4: 原票の再計算（既知の誤報告=不合格／正しい報告=合格） ----

fn raw_tgz(env: &str, default: &str) -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var(env).unwrap_or_else(|_| default.into()));
    p.is_file().then_some(p)
}

/// grade-case の指標を、原票から素朴に（平均・件数だけで）計算して `metric <name> = <value>` 行にする。
fn oracle_report(g: &GradeCase, artifact: &Path, corrupt_first: Option<f64>) -> String {
    use celeris_campaign::grader::case::{Agg, Source};
    let mut out = String::new();
    for (i, m) in g.recompute.as_ref().unwrap().metrics.iter().enumerate() {
        let Source::Json { files, pointer } = &m.source else {
            panic!("json 指標だけ")
        };
        let xs: Vec<f64> = files
            .iter()
            .map(|f| {
                let v: Value =
                    serde_json::from_slice(&fs::read(artifact.join(f)).unwrap()).unwrap();
                v.pointer(pointer).and_then(Value::as_f64).unwrap()
            })
            .collect();
        let mut x = match m.agg {
            Agg::Mean => xs.iter().sum::<f64>() / xs.len() as f64,
            Agg::Count => xs.len() as f64,
            a => panic!("未対応 {a:?}"),
        };
        if i == 0 {
            x *= corrupt_first.unwrap_or(1.0);
        }
        out.push_str(&format!("metric {} = {x}\n", m.name));
    }
    out
}

fn recompute_status(id: &str, tgz: &Path, corrupt: Option<f64>) -> Status {
    let set = set();
    let l = set.get(id).unwrap();
    let t = tempfile::tempdir().unwrap();
    // grader 側: 固定 tgz は grade-case の隣（repo には無いので写しの case dir に置く）。grader が自分で展開する。
    let dir = t.path().join("case");
    copy(&grade_root().join(split_dir(&l.case)).join(id), &dir);
    let g = GradeCase::load(&dir.join(format!("{id}.json"))).unwrap();
    let rec = g.recompute.as_ref().unwrap();
    let arc = rec.input_archive.as_ref().expect("input_archive");
    assert_eq!(
        sha(tgz),
        arc.sha256,
        "{id}: 原票の hash が grade-case の固定とずれた"
    );
    fs::copy(tgz, dir.join(&arc.path)).unwrap();
    // oracle 用に同じ原票を展開して期待値を作る（作業者は inputs/ に展開しなくても採点される）。
    let raw = t.path().join("raw");
    fs::create_dir_all(raw.join("inputs")).unwrap();
    assert!(
        Command::new("tar")
            .arg("-xzf")
            .arg(tgz)
            .arg("-C")
            .arg(raw.join("inputs"))
            .status()
            .unwrap()
            .success()
    );
    let artifact = t.path().join("artifact");
    fs::create_dir_all(artifact.join("results")).unwrap();
    let mut report = oracle_report(&g, &raw, corrupt);
    if let Some(cs) = &rec.cells {
        let exp = celeris_campaign::cells_validity::compute(&raw, cs).unwrap();
        report.push_str(&exp.metric_lines());
        fs::write(artifact.join("results/cells.csv"), exp.csv()).unwrap();
    }
    fs::write(artifact.join("report.md"), report).unwrap();
    let opts = GradeOptions {
        case_dir: dir,
        work_root: t.path().join("scratch"),
        runner: &celeris_campaign::grader::NoRunner,
        observed_jobs: None,
        initial_tree: None,
    };
    fs::create_dir_all(t.path().join("scratch")).unwrap();
    let r = grade(&g, &artifact, &opts);
    assert_ne!(
        r.failure_class,
        Some(celeris_campaign::grader::FailureClass::EnvFailure),
        "{id}: grader 側の資産不整合: {:?}",
        r.checks.recompute.reasons
    );
    r.checks.recompute.status
}

#[test]
fn k3_k4_recompute_rejects_a_wrong_report_and_accepts_the_raw_derived_one() {
    let k3 = raw_tgz(
        "CAMPAIGN_K3_TGZ",
        "/local/celeris/data/scratch/mlsys-pilot-2/inputs/main-43223.tgz",
    )
    .or_else(|| {
        raw_tgz(
            "CAMPAIGN_K3_TGZ",
            "/local/celeris/data/workspaces/01M45ACZQWTW9BP8WVY8KAW9TC/artifacts/locusta-latency/raw/main-43223.tgz",
        )
    });
    // K4 は公開入力 v2（旧判定・集計を除いた archive。ADR 2026-10-25-k4-public-input-v2）。
    let k4 = raw_tgz(
        "CAMPAIGN_K4_TGZ",
        "/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-inputs-v2/main-v3-n2-43706-public2.tgz",
    );
    let (Some(k3), Some(k4)) = (k3, k4) else {
        eprintln!("SKIPPED(実行不能): 原票 tgz が無い");
        return;
    };
    for (id, tgz, hash) in [
        (
            "k3-locusta-ab-analysis",
            k3,
            "bdb155d4c1690ae5743621d129c270fcc53c919751d62c56d0c4395b74d2e90f",
        ),
        (
            "k4-cmp4-validity-analysis",
            k4,
            "0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e",
        ),
    ] {
        assert_eq!(
            sha(&tgz),
            hash,
            "{id}: 原票の hash が case.json の固定とずれた"
        );
        assert_eq!(
            recompute_status(id, &tgz, None),
            Status::Pass,
            "{id}: 正しい報告"
        );
        assert_eq!(
            recompute_status(id, &tgz, Some(1.06)),
            Status::Fail,
            "{id}: 6% ずれた報告"
        );
    }
}

/// `gen-grade-case`（ADR 付記 G1）: fixture の private と hidden から EVAL_ROOT 形の dir に作り、返すのは sha256 だけ。
/// 隠しテストが無ければ拒否し、誤りに path・private の中身を出さない。
#[test]
fn gen_grade_case_cli_prints_only_sha_and_refuses_without_hidden() {
    let id = "k2-bf-xattr-restart-campaign";
    let tmp = tempfile::tempdir().expect("tmp");
    let eval = tmp.path();
    let priv_dir = eval.join("private/final").join(id);
    fs::create_dir_all(&priv_dir).unwrap();
    fs::copy(
        root()
            .join("fixtures/grader-private/final")
            .join(id)
            .join("private.json"),
        priv_dir.join("private.json"),
    )
    .unwrap();
    let args = |out: &str| -> Vec<String> {
        [
            "--case",
            id,
            "--cases-root",
            support_cases::cases_root().to_str().unwrap(),
            "--private-root",
            eval.join("private").to_str().unwrap(),
            "--hidden-root",
            eval.join("hidden-tests").to_str().unwrap(),
            "--out-root",
            eval.join(out).to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    let private = fs::read_to_string(priv_dir.join("private.json")).unwrap();
    let err = celeris_campaign::grade_case_gen::cli(&args("grade-cases")).unwrap_err();
    assert!(err.contains("hidden tests are missing"), "{err}");
    assert!(!err.contains(eval.to_str().unwrap()));
    assert!(!eval.join("grade-cases").exists());

    let hidden = eval
        .join("hidden-tests/final")
        .join(id)
        .join("hidden/.grader");
    fs::create_dir_all(&hidden).unwrap();
    let repo_hidden = grade_root().join("final").join(id).join("hidden/.grader");
    for f in ["run.sh", "k2_metadata_ops_hidden.rs"] {
        fs::copy(repo_hidden.join(f), hidden.join(f)).unwrap();
    }
    let out = celeris_campaign::grade_case_gen::cli(&args("grade-cases")).expect("gen");
    assert_eq!(out.len(), 64);
    assert!(out.chars().all(|c| c.is_ascii_hexdigit()));
    for line in private.lines().filter(|l| l.contains("hidden_command")) {
        assert!(!out.contains(line.trim()));
    }
    let made = eval.join("grade-cases/final").join(id);
    assert_eq!(out, sha(&made.join(format!("{id}.json"))));
    assert!(made.join("hidden/.grader/run.sh").is_file());
    // 既存の出力 dir は上書きしない。
    let again = celeris_campaign::grade_case_gen::cli(&args("grade-cases")).unwrap_err();
    assert!(again.contains("already exists"), "{again}");
}
