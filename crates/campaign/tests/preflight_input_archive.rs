//! 評価側の固定入力 tgz（grade-case の `recompute.input_archive`）を投入前に確かめる:
//! preflight は archive の欠落・pin との sha256 不一致・公開 inputs との bytes 不一致・展開失敗・pinned_raw の不一致・
//! cells の再計算不能をそれぞれ `graders` の不足として拒否し、run_gated は transport（LLM run）に一度も触れない。
//! stage-grader-inputs（`inputs::stage_grader_inputs` と CLI）は正しい tgz を grade-case の隣に置き、
//! 欠落・hash 不一致では Err／非 0 終了して何も置かない（ADR 2026-10-23-k4-cells-validity-grading D3）。
//! 外部には出ない（偽 Host・偽 transport・合成 raw の tgz）。
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use celeris_campaign::{
    asset_assembly::create_tgz,
    celeris_exec::{CelerisExecutor, HttpResponse, Transport},
    cells_validity::synth,
    manifest::CampaignManifest,
    preflight::{Host, ResolvedProgram, check, run_gated},
    runner::RunOptions,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod daemon_identity;
mod preflight_setup;
use preflight_setup::*;

const K4: &str = "k4-cmp4-validity-analysis";

struct FakeHost {
    api: Vec<(&'static str, Value)>,
}

impl Host for FakeHost {
    fn runtime_probe(
        &self,
        r: &celeris_campaign::preflight_isolation::ProbeRequest,
    ) -> Result<celeris_campaign::runtime_probe::RuntimeProbe, String> {
        Ok(r.all_denied())
    }
    fn probe_runner(&self) -> Box<dyn celeris_campaign::cap_probe::ProbeRunner> {
        Box::new(OkRunner)
    }
    fn probe_daemon_worker(
        &self,
        _: &str,
        _: &str,
        spec: &celeris_campaign::cap_probe::WorkerProbeSpec,
    ) -> Result<celeris_campaign::cap_probe::WorkerProbeRun, String> {
        Ok(ok_worker_run(spec))
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
        if path == "/api/v1/config" {
            return Ok(daemon_identity::matching_daemon_body());
        }
        self.api
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| format!("GET {path}: HTTP 404"))
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
}

fn account_dir() -> &'static Path {
    static D: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    D.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

fn host() -> FakeHost {
    FakeHost {
        api: vec![
            (
                "/api/v1/projects/01ABC",
                json!({"project":{"id":"01ABC","auto_integrate":false},"repos":[]}),
            ),
            (
                "/api/v1/projects/01ABC/repos",
                json!({"items":[{"name":"scratch","is_primary":true,"location":{"kind":"local","path":"/x"}}]}),
            ),
            (
                "/api/v1/llm/accounts",
                json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":account_dir(),"logged_in":true}]}),
            ),
        ],
    }
}

/// 呼ばれたら数えるだけの transport（LLM run・task 作成の入口）。
struct CountingTransport(AtomicUsize);

impl Transport for &CountingTransport {
    fn request(&self, _: &str, _: &str, _: Option<&Value>) -> Result<HttpResponse, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("must not be called".into())
    }
}

fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).unwrap()))
}

fn read(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}

fn write(p: &Path, v: &Value) {
    fs::write(p, serde_json::to_string_pretty(v).unwrap() + "\n").unwrap();
}

fn k4_grade_dir(grade_root: &Path) -> PathBuf {
    grade_root.join("final").join(K4)
}

fn grade_json(grade_root: &Path) -> PathBuf {
    k4_grade_dir(grade_root).join(format!("{K4}.json"))
}

/// grade-case の隣の評価側 tgz。
fn grader_archive(s: &Setup) -> PathBuf {
    k4_grade_dir(&s.inp.grade_cases_root).join(K4_INPUT)
}

/// 合成 raw（`plan`）を `edit` で変えてから tgz にした bytes。
fn tgz(plan: &synth::Plan, edit: impl FnOnce(&Path)) -> Vec<u8> {
    let t = tempfile::tempdir().unwrap();
    let raw = t.path().join("inputs");
    synth::write(&raw, plan).unwrap();
    edit(&raw);
    let out = t.path().join(K4_INPUT);
    create_tgz(&raw, &out).unwrap();
    fs::read(out).unwrap()
}

/// 公開 case の `grade_case_ref.sha256` を grade-case 本体の現在の hash に合わせる（grade-case を書き換えた試験用）。
fn repin_grade_case_ref(s: &Setup) {
    let cp = s.inp.cases_root.join("final").join(K4).join("case.json");
    let mut c = read(&cp);
    c["grade_case_ref"]["sha256"] = json!(sha(&grade_json(&s.inp.grade_cases_root)));
    write(&cp, &c);
}

/// 評価側・公開側の両方を `bytes` の tgz に置き換え、grade-case と公開 case の固定を整合させる
/// （archive 自体の中身だけが原因で落ちることを確かめる）。
fn replace_archive_consistently(s: &Setup, bytes: &[u8]) {
    let ins = s.inp.inputs_src.as_ref().unwrap().join(K4_INPUT);
    fs::write(grader_archive(s), bytes).unwrap();
    fs::write(&ins, bytes).unwrap();
    let h = sha(&ins);
    let gp = grade_json(&s.inp.grade_cases_root);
    let mut g = read(&gp);
    g["recompute"]["input_archive"]["sha256"] = json!(h);
    write(&gp, &g);
    let cp = s.inp.cases_root.join("final").join(K4).join("case.json");
    let mut c = read(&cp);
    c["inputs"][0]["sha256"] = json!(h);
    write(&cp, &c);
    repin_grade_case_ref(s);
}

/// preflight が `needle` を含む `graders` の不足で止め（他の check は落ちない）、run_gated が transport に
/// 一度も触れず out も作らないことを確かめる。
fn assert_rejected_before_run(s: &Setup, needle: &str) {
    let r = check(&s.inp, &host());
    assert!(!r.ok, "preflight must reject: {needle}");
    assert_eq!(r.paid_calls_made, 0);
    assert!(
        r.missing
            .iter()
            .any(|m| m.check == "graders" && m.detail.contains(needle)),
        "want graders miss containing {needle:?}: {:?}",
        r.missing
    );
    assert!(
        r.missing.iter().all(|m| m.check == "graders"),
        "only the grader archive should be at fault: {:?}",
        r.missing
    );
    let m = CampaignManifest::load(&s.inp.manifest_path).unwrap();
    let input = m.load_input(&s.inp.manifest_path, None).unwrap();
    let out = s.dir.path().join("out");
    let t = CountingTransport(AtomicUsize::new(0));
    let exec = CelerisExecutor::new(&t, serde_json::Map::new(), 1800, out.join("cancel.jsonl"));
    let e = run_gated(
        &s.inp,
        &host(),
        &m,
        &input,
        &exec,
        &RunOptions {
            global_ledger: None,
            out: out.clone(),
            ..Default::default()
        },
    )
    .expect_err("run_gated must refuse");
    assert!(e.contains("preflight failed"), "{e}");
    assert!(e.contains(needle), "the refusal names the cause: {e}");
    assert_eq!(t.0.load(Ordering::SeqCst), 0, "no LLM run / task creation");
    assert!(!out.exists(), "no ledger or out dir");
}

#[test]
fn preflight_accepts_the_synthetic_k4_archive() {
    let s = setup();
    let r = check(&s.inp, &host());
    assert!(r.ok, "{:?}", r.missing);
    assert!(grader_archive(&s).is_file());
    let g = read(&grade_json(&s.inp.grade_cases_root));
    assert_eq!(
        g["recompute"]["input_archive"]["sha256"],
        json!(sha(&grader_archive(&s)))
    );
}

#[test]
fn preflight_rejects_a_missing_grader_archive() {
    let s = setup();
    fs::remove_file(grader_archive(&s)).unwrap();
    assert_rejected_before_run(&s, "is missing next to the grade-case");
}

#[test]
fn preflight_rejects_an_archive_that_differs_from_the_pinned_sha256() {
    let s = setup();
    let other = tgz(&synth::healthy(), |raw| {
        fs::write(raw.join("extra.txt"), "tampered").unwrap();
    });
    fs::write(grader_archive(&s), other).unwrap();
    assert_rejected_before_run(&s, "differs from the pinned");
}

#[test]
fn preflight_rejects_a_grader_archive_whose_bytes_differ_from_the_public_input() {
    let s = setup();
    // 評価側だけ別の tgz にし、grade-case の pin もそれに合わせる（grade-case 単体では整合している）。
    let other = tgz(&synth::healthy(), |raw| {
        fs::write(raw.join("extra.txt"), "grader-only").unwrap();
    });
    fs::write(grader_archive(&s), other).unwrap();
    let gp = grade_json(&s.inp.grade_cases_root);
    let mut g = read(&gp);
    g["recompute"]["input_archive"]["sha256"] = json!(sha(&grader_archive(&s)));
    write(&gp, &g);
    repin_grade_case_ref(&s);
    assert_rejected_before_run(&s, "differs from the public input's sha256");
}

#[test]
fn preflight_rejects_an_archive_that_cannot_be_extracted() {
    let s = setup();
    replace_archive_consistently(&s, b"this is not a gzip tar archive\n");
    assert_rejected_before_run(&s, "could not be extracted");
}

#[test]
fn preflight_rejects_an_archive_whose_raw_differs_from_pinned_raw() {
    let s = setup();
    let mut plan = synth::healthy();
    plan.cells.pop();
    replace_archive_consistently(&s, &tgz(&plan, |_| {}));
    assert_rejected_before_run(&s, "differs from pinned_raw");
}

#[test]
fn preflight_rejects_an_archive_from_which_cells_cannot_be_recomputed() {
    let s = setup();
    // planned-cells.json（pinned_raw）は同じだが、grade-case が参照する job 記録が無い。
    let bytes = tgz(&synth::healthy(), |raw| {
        fs::remove_file(raw.join("review/qstat-1001.txt")).unwrap();
    });
    replace_archive_consistently(&s, &bytes);
    assert_rejected_before_run(&s, "cells validity cannot be recomputed");
}

/// stage 用の一式: grade-case root（K4 だけ）・公開 case の写し・inputs-src。grade-case の隣の tgz は消してある。
struct Stage {
    _dir: tempfile::TempDir,
    grade: PathBuf,
    ins: PathBuf,
}

impl Stage {
    fn new() -> Stage {
        let dir = tempfile::tempdir().unwrap();
        let (grade, cases, ins) = (
            dir.path().join("grade-cases"),
            dir.path().join("cases"),
            dir.path().join("inputs-src"),
        );
        copy(&root().join("cases"), &cases);
        synth::healthy()
            .install_k4_fixture(&grade, &cases, &ins)
            .unwrap();
        fs::remove_file(k4_grade_dir(&grade).join(K4_INPUT)).unwrap();
        Stage {
            _dir: dir,
            grade,
            ins,
        }
    }
    fn staged(&self) -> PathBuf {
        k4_grade_dir(&self.grade).join(K4_INPUT)
    }
    fn pinned(&self) -> String {
        read(&grade_json(&self.grade))["recompute"]["input_archive"]["sha256"]
            .as_str()
            .unwrap()
            .to_string()
    }
    /// grade-case の隣に archive も途中 file も無い。
    fn assert_nothing_placed(&self) {
        let names: Vec<String> = fs::read_dir(k4_grade_dir(&self.grade))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![format!("{K4}.json")], "nothing staged");
    }
}

fn stage_cli(st: &Stage) -> std::process::Output {
    Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
        .args(["stage-grader-inputs", "--grade-case-root"])
        .arg(&st.grade)
        .arg("--inputs-src")
        .arg(&st.ins)
        .output()
        .unwrap()
}

#[test]
fn stage_grader_inputs_places_the_pinned_archive_next_to_the_grade_case() {
    let st = Stage::new();
    let done = celeris_campaign::inputs::stage_grader_inputs(&st.grade, &st.ins, None).unwrap();
    assert_eq!(done.len(), 1, "{done:?}");
    assert_eq!(sha(&st.staged()), st.pinned());
    // 既に同じ bytes があれば何もしない。
    let again = celeris_campaign::inputs::stage_grader_inputs(&st.grade, &st.ins, None).unwrap();
    assert!(again.is_empty(), "{again:?}");
    // only で選ばれていない case は触らない。
    let st = Stage::new();
    let other = ["k2-bf-xattr-restart-campaign".to_string()];
    let none =
        celeris_campaign::inputs::stage_grader_inputs(&st.grade, &st.ins, Some(&other)).unwrap();
    assert!(none.is_empty());
    st.assert_nothing_placed();
}

#[test]
fn stage_grader_inputs_rejects_a_missing_archive_and_places_nothing() {
    let st = Stage::new();
    fs::remove_file(st.ins.join(K4_INPUT)).unwrap();
    let e = celeris_campaign::inputs::stage_grader_inputs(&st.grade, &st.ins, None).unwrap_err();
    assert!(e.contains(K4) && e.contains("is not in"), "{e}");
    st.assert_nothing_placed();
}

#[test]
fn stage_grader_inputs_rejects_a_hash_mismatch_and_places_nothing() {
    let st = Stage::new();
    fs::write(st.ins.join(K4_INPUT), b"different bytes").unwrap();
    let e = celeris_campaign::inputs::stage_grader_inputs(&st.grade, &st.ins, None).unwrap_err();
    assert!(e.contains("differs from the grade-case pin"), "{e}");
    st.assert_nothing_placed();
    // 既に違う bytes が隣にあれば上書きしない。
    let st = Stage::new();
    fs::write(st.staged(), b"stale").unwrap();
    let e = celeris_campaign::inputs::stage_grader_inputs(&st.grade, &st.ins, None).unwrap_err();
    assert!(e.contains("not overwritten"), "{e}");
    assert_eq!(fs::read(st.staged()).unwrap(), b"stale");
}

#[test]
fn stage_grader_inputs_validates_every_case_before_copying_any() {
    // K4 は正しいが、同じ root の別 case の archive が無い → K4 も置かない。
    let st = Stage::new();
    let other = st.grade.join("final/zz-other-case");
    fs::create_dir_all(&other).unwrap();
    let mut g = read(&grade_json(&st.grade));
    g["id"] = json!("zz-other-case");
    g["recompute"]["input_archive"]["path"] = json!("absent.tgz");
    write(&other.join("zz-other-case.json"), &g);
    let e = celeris_campaign::inputs::stage_grader_inputs(&st.grade, &st.ins, None).unwrap_err();
    assert!(e.contains("zz-other-case"), "{e}");
    st.assert_nothing_placed();
}

#[test]
fn stage_grader_inputs_cli_stages_or_exits_nonzero_without_placing_anything() {
    let st = Stage::new();
    let o = stage_cli(&st);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains(K4_INPUT));
    assert_eq!(sha(&st.staged()), st.pinned());

    let st = Stage::new();
    fs::remove_file(st.ins.join(K4_INPUT)).unwrap();
    let o = stage_cli(&st);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("is not in"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    st.assert_nothing_placed();

    let st = Stage::new();
    fs::write(st.ins.join(K4_INPUT), b"different bytes").unwrap();
    let o = stage_cli(&st);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("differs from the grade-case pin"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    st.assert_nothing_placed();
}

#[path = "preflight_input_archive/real.rs"]
mod real;
