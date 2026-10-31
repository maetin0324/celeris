//! ops-r2（K2 pilot）の manifest と preflight の受け入れ形（ADR 2026-10-31-ops-r2-preflight-fixes 付記 M1〜M4）。
//! 旧 private に依る検査（private の読み込み・lock・grade-case 実体）と task API 前提の検査（api-url・token・task-template）を
//! 外した代わりに、実在する入力（入力 manifest・case の公開部・manifest の sha256 参照）を検査する。外部・LLM には出ない。
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use celeris_campaign::{
    manifest::CampaignManifest,
    preflight::{Host, Inputs, ResolvedProgram, check},
};
use serde_json::{Value, json};

mod daemon_identity;
mod preflight_setup;
use preflight_setup::*;

const NEW_PRIVATE_SHA: &str = "e1bf18e72a921f227584867a4c0bc0664db58c3a1b1bfa7a0805832027cf8313";
const NEW_GRADE_CASE_SHA: &str = "8ae2a1706f1cb63afed8877921d486ffe12871a4b006f9dfe638aa6740616b1d";
const K2: &str = "k2-bf-xattr-restart-campaign";

fn frozen() -> PathBuf {
    root().join("fixtures/frozen-manifests")
}

struct Ops2Host;

impl Host for Ops2Host {
    fn runtime_probe(
        &self,
        r: &celeris_campaign::preflight_isolation::ProbeRequest,
    ) -> Result<celeris_campaign::runtime_probe::RuntimeProbe, String> {
        Ok(r.all_denied())
    }
    fn env_nonempty(&self, _: &str) -> bool {
        false
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        Ok(ResolvedProgram {
            path: p.to_path_buf(),
            version: "2.1.303 (Claude Code)".into(),
        })
    }
    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        panic!("ops-r2 preflight must not call the task API: GET {path}")
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
}

/// manifest・入力 manifest・cases を写した一時 dir と、api-url・token・task-template・materials/kb・canary・cap-env を
/// 渡さない `Inputs`（`--repo-src`・`--measure-config`・隔離の入力だけ）。
struct Setup {
    dir: tempfile::TempDir,
    inp: Inputs,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for f in ["ops-r2-k2.json", "ops-r2-k2-input-manifest.json"] {
        fs::copy(frozen().join(f), d.join(f)).unwrap();
    }
    let cases = d.join("cases");
    copy(&root().join("cases"), &cases);
    let repo = d.join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("README"), "k2 initial tree").unwrap();
    let measure = write_configs(d, &measure_config(d, "measure"), &normal_config(d));
    let inp = Inputs {
        manifest_path: d.join("ops-r2-k2.json"),
        input_override: None,
        cases_root: cases,
        // 旧 private の置き場。ops-r2 は読まない（存在しない path でも通ることを下の試験が確かめる）。
        grader_root: d.join("no-such-grader-root"),
        grade_cases_root: d.join("no-such-grade-cases"),
        a_program: "claude".into(),
        api_url: None,
        token_env: None,
        task_template: None,
        repo_src: Some(repo),
        materials_src: None,
        kb_src: None,
        inputs_src: None,
        a_account: None,
        measure: Some(measure),
        measurement_id: None,
        cap_env: None,
        variant_b0_sha256: None,
        worker_programs: Vec::new(),
        isolation: isolation(d),
    };
    Setup { dir, inp }
}

fn details(r: &celeris_campaign::preflight::Report) -> Vec<String> {
    r.missing
        .iter()
        .map(|m| format!("{}: {}", m.check, m.detail))
        .collect()
}

fn manifest_json() -> Value {
    serde_json::from_slice(&fs::read(frozen().join("ops-r2-k2.json")).unwrap()).unwrap()
}

#[test]
fn ops_r2_manifest_has_the_approved_fixed_values() {
    let m = CampaignManifest::load(&frozen().join("ops-r2-k2.json")).unwrap();
    m.validate().unwrap();
    assert!(m.is_ops_r2() && m.is_ops_r1() && m.b_is_cos_thread());
    assert_eq!(m.problems, [K2]);
    // 1 試行 1,500,000 token・3600 秒、合計 3,000,000 token・7200 秒。
    let b = &m.budget;
    assert_eq!(
        (b.per_case.max_tokens, b.per_case.max_wall_secs),
        (1_500_000, 3600)
    );
    assert_eq!((b.max_tokens, b.max_wall_secs), (3_000_000, 7200));
    assert_eq!(m.termination.trial_timeout_secs, 3600);
    let c = m.contract.as_ref().unwrap();
    assert_eq!(c.revision, "main-4-ops-r2");
    let refs = &c.grader_refs[K2];
    assert_eq!(refs.private_sha256, NEW_PRIVATE_SHA);
    assert_eq!(refs.grade_case_sha256, NEW_GRADE_CASE_SHA);
    let a = &m.conditions()[&celeris_campaign::manifest::ConfigId::A];
    let bb = &m.conditions()[&celeris_campaign::manifest::ConfigId::B];
    assert!(a.condition_id.contains("-A1-") && bb.condition_id.contains("-B1-"));
    assert_eq!(bb.cos_model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        c.pinned_versions[&celeris_campaign::manifest::ConfigId::B]
            .cos_policy_version
            .as_deref(),
        Some("10")
    );
    // manifest は sha256 の参照だけを持ち、private の中身（hidden command など）を持たない。
    let text = fs::read_to_string(frozen().join("ops-r2-k2.json")).unwrap();
    assert!(!text.contains("hidden_command") && !text.contains("false_claim_traps"));
}

#[test]
fn ops_r2_preflight_passes_without_api_url_task_template_materials_kb_or_canaries() {
    let s = setup();
    let r = check(&s.inp, &Ops2Host);
    assert!(r.ok, "{:#?}", details(&r));
    assert_eq!(r.paid_calls_made, 0);
    for c in [
        "input-manifest",
        "case-inputs",
        "graders",
        "measure-env",
        "deny_readable",
        "budget",
    ] {
        assert!(
            r.checked.contains(&c),
            "{c} must still be checked: {:?}",
            r.checked
        );
    }
    // ops-r1 の経路が外した検査: lock（旧 private の hash を封印する）。
    assert!(!r.checked.contains(&"lock"));
}

#[test]
fn ops_r2_preflight_never_reads_the_old_private() {
    // repo の旧 private（漏洩済み）は case.json の新しい private_ref と hash が違う。ops-r1 の経路なら
    // `private file hash mismatch` で落ちたが、ops-r2 は読まず、採点にも使わない。
    let old = root()
        .join("fixtures/grader-private/final")
        .join(K2)
        .join("private.json");
    if old.exists() {
        use sha2::{Digest, Sha256};
        let got = format!("{:x}", Sha256::digest(fs::read(&old).unwrap()));
        assert_ne!(
            got, NEW_PRIVATE_SHA,
            "the old private is not the approved new one"
        );
    }
    let mut s = setup();
    // 旧 private の置き場を実在する旧 fixtures に向けても結果は同じ（読まれない）。
    s.inp.grader_root = root().join("fixtures/grader-private");
    let r = check(&s.inp, &Ops2Host);
    assert!(r.ok, "{:#?}", details(&r));
    assert!(
        !details(&r)
            .iter()
            .any(|d| d.contains("private file hash mismatch"))
    );
}

#[test]
fn ops_r2_preflight_rejects_a_case_json_that_differs_from_the_manifest_refs() {
    let s = setup();
    let case = s.inp.cases_root.join("final").join(K2).join("case.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&case).unwrap()).unwrap();
    v["private_ref"]["sha256"] = json!("0".repeat(64));
    v["grade_case_ref"]["sha256"] = json!("1".repeat(64));
    fs::write(&case, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let r = check(&s.inp, &Ops2Host);
    assert!(!r.ok);
    let d = details(&r);
    assert!(
        d.iter()
            .any(|x| x.starts_with("graders:") && x.contains("private_ref.sha256")),
        "{d:?}"
    );
    assert!(
        d.iter()
            .any(|x| x.starts_with("graders:") && x.contains("grade_case_ref.sha256")),
        "{d:?}"
    );
}

#[test]
fn ops_r2_preflight_still_requires_real_inputs() {
    // 入力 manifest が無い・--repo-src が無い・測定 daemon の config が無い、は従来どおり不足になる。
    let mut s = setup();
    fs::remove_file(s.dir.path().join("ops-r2-k2-input-manifest.json")).unwrap();
    s.inp.repo_src = None;
    s.inp.measure = None;
    let d = details(&check(&s.inp, &Ops2Host));
    assert!(d.iter().any(|x| x.starts_with("input-manifest:")), "{d:?}");
    assert!(
        d.iter()
            .any(|x| x.starts_with("inputs:") && x.contains("--repo-src")),
        "{d:?}"
    );
    assert!(
        d.iter()
            .any(|x| x.starts_with("measure-env:") && x.contains("--measure-config")),
        "{d:?}"
    );
}

#[test]
fn ops_r2_manifest_grader_refs_are_checked_at_validate() {
    let mut v = manifest_json();
    v["contract"]["grader_refs"] = json!({});
    let m: CampaignManifest = serde_json::from_value(v).unwrap();
    assert!(
        m.validate()
            .unwrap_err()
            .contains("grader_refs is required")
    );

    let mut v = manifest_json();
    v["contract"]["grader_refs"][K2]["private_sha256"] = json!("short");
    let m: CampaignManifest = serde_json::from_value(v).unwrap();
    assert!(m.validate().unwrap_err().contains("64 hex"));

    let mut v = manifest_json();
    v["contract"]["grader_refs"]["other-case"] =
        json!({"private_sha256":"a".repeat(64),"grade_case_sha256":"b".repeat(64)});
    let m: CampaignManifest = serde_json::from_value(v).unwrap();
    assert!(
        m.validate()
            .unwrap_err()
            .contains("cover exactly the problems")
    );

    // ops-r1 の revision は grader_refs を持てない。
    let mut v = manifest_json();
    v["contract"]["revision"] = json!("main-4-ops-r1");
    let m: CampaignManifest = serde_json::from_value(v).unwrap();
    assert!(!m.is_ops_r2());
    assert!(m.validate().unwrap_err().contains("only for main-4-ops-r2"));
}

fn campaign(args: &[&str], manifest: &Path) -> std::process::Output {
    Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"))
        .args(args)
        .arg("--manifest")
        .arg(manifest)
        .arg("--cases-root")
        .arg(root().join("cases"))
        .output()
        .expect("run")
}

#[test]
fn cli_preflight_accepts_ops_r2_but_other_submit_commands_still_reject_it() {
    let m = frozen().join("ops-r2-k2.json");
    // preflight は受ける（不足は出るが、ops-r1 の拒否文でも旧 private・api-url・task-template・materials/kb の不足でもない）。
    let o = campaign(&["preflight"], &m);
    let text = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
    assert!(!text.contains("only for ops-r1-assemble"), "{text}");
    for gone in [
        "--api-url",
        "--task-template",
        "--materials-src",
        "--kb-src",
        "private file hash mismatch",
        "final-input-manifest.json",
    ] {
        assert!(!text.contains(gone), "{gone}: {text}");
    }
    assert!(text.contains("--measure-config"), "{text}");
    for cmd in ["run", "dry-run", "config-check", "env-check"] {
        let o = campaign(&[cmd], &m);
        assert!(!o.status.success());
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("only for ops-r1-assemble"),
            "{cmd}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}
