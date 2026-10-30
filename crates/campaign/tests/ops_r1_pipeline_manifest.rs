//! ops-r1 の condition 宣言（`contract.conditions`）と human-op の `--condition`（docs/ops/ops-r1-pipeline.md §3・§5）。
//! 旧系列（A0/B0/B1/C1・mlsys-main-4-v4）の manifest の sha256 と human-op 記録の record_id が変わらないことも固定する。
use std::{path::Path, process::Command};

use celeris_campaign::{
    human_ops::{ExperimentSet, HumanOpRecord, read_records},
    manifest::{CampaignManifest, ConfigId},
    ops_r1_conditions::Intake,
};
use serde_json::{Value, json};

const A_COND: &str = "mlsys-main-4-ops-r1-k4p1-A-direct-claude-code";
const B_COND: &str = "mlsys-main-4-ops-r1-k4p1-B-celeris-ops";
const CASE: &str = "k4-cmp4-validity-analysis";

fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"))
}

fn bin() -> Command {
    Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
}

/// ops-r1 の manifest（K4 pilot の写しから: B は routing なし・CoS 受付、契約 revision main-4-ops-r1）。
fn ops_r1_json() -> Value {
    let text = std::fs::read_to_string(fixtures().join("frozen-manifests/main-4-v4-pilot-k4.json"))
        .expect("read k4");
    let mut m: Value = serde_json::from_str(&text).expect("json");
    m["campaign_id"] = json!("mlsys-main-4-ops-r1-k4p1");
    m["configs"][1] = json!({"id":"B","harness":"celeris","harness_version":"94f86928dc13",
        "model":"claude-sonnet-5-5","model_version":"claude-sonnet-5-5","policy_version":"cos-v6"});
    m["experiment_sets"] = json!({"ops-r1": {"arms":["A","B"],"compare":[["A","B"]],
        "fixed":{"initial_info": m["snapshot"]["materials_sha256"].clone()}}});
    m["selected_sets"] = json!(["ops-r1"]);
    let sha = "a".repeat(64);
    m["contract"] = json!({"revision":"main-4-ops-r1",
        "pinned_versions":{
            "A":{"claude_code_version":"2.1.303","claude_code_sha256":sha},
            "B":{"claude_code_version":"2.1.303","claude_code_sha256":sha,"pricing_version":"p1","cos_policy_version":"cos-v6"}},
        "conditions":{
            "A":{"condition_id":A_COND,"intake":"direct_claude_code","trial_ids":["mlsys-main-4-ops-r1-k4p1-A-t001"]},
            "B":{"condition_id":B_COND,"intake":"cos_thread","trial_ids":["mlsys-main-4-ops-r1-k4p1-B-t001"]}}});
    m
}

fn parse(v: &Value) -> CampaignManifest {
    serde_json::from_value(v.clone()).expect("manifest")
}

fn validate_err(v: &Value) -> String {
    parse(v).validate().expect_err("must be rejected")
}

#[test]
fn ops_r1_pipeline_frozen_manifests_keep_published_sha256() {
    // docs/ops/mlsys-main-4-pilot-runbook.md の公開値（R2・K4 pilot・K2 pilot の campaign_sha256）。
    for (file, want) in [
        (
            "main-4-v4-r2-final.json",
            "3756ff02244a5d6c8ee4e1c04d5041487d7ebbba9ba762543bc429dfa2e51c65",
        ),
        (
            "main-4-v4-pilot-k4.json",
            "e9fb2320303a16d17e38ea6831d1d53b337bb5a869514a2a4bbfa39c0b6ceee0",
        ),
        (
            "main-4-v4-pilot-k2.json",
            "3447ea282a8c19696aeae9a1793f863782a11a3083c3727cfd652c91cd683a0e",
        ),
    ] {
        let m =
            CampaignManifest::load(&fixtures().join("frozen-manifests").join(file)).expect(file);
        assert_eq!(m.compute_sha256().expect("hash"), want, "{file}");
        assert!(!m.is_ops_r1());
        m.validate().expect(file);
        let v = serde_json::to_value(&m).expect("ser");
        assert!(
            v.get("conditions").is_none() && v.get("contract").is_none(),
            "{file}"
        );
    }
    // repo の dev pilot fixture は sha256 欄を持つ（validate が照合する）。
    let p = CampaignManifest::load(&fixtures().join("pilot.json")).expect("pilot");
    assert_eq!(
        p.compute_sha256().expect("hash"),
        "7af33ae7e5dac4e7ad7faabc366f1c50c6b4624e523e8530295e5d60b36837c0"
    );
}

#[test]
fn ops_r1_pipeline_manifest_loads_and_validates() {
    let dir = tempfile::tempdir().expect("dir");
    let p = dir.path().join("ops-r1.json");
    std::fs::write(&p, ops_r1_json().to_string()).expect("write");
    let m = CampaignManifest::load(&p).expect("load");
    m.validate().expect("valid ops-r1 manifest");
    assert!(m.is_ops_r1() && m.b_is_cos_thread());
    let (arm, a) = m.condition_arm(A_COND).expect("A");
    assert_eq!((arm, a.intake), (ConfigId::A, Intake::DirectClaudeCode));
    let (arm, b) = m.condition_arm(B_COND).expect("B");
    assert_eq!((arm, b.intake), (ConfigId::B, Intake::CosThread));
    assert_eq!(b.trial_ids, ["mlsys-main-4-ops-r1-k4p1-B-t001"]);
    assert!(m.condition_arm("nope").is_err());
    // 直列化に conditions が残り、読み直しても同じ hash（conditions は hash の材料）。
    let again: CampaignManifest =
        serde_json::from_value(serde_json::to_value(&m).expect("ser")).expect("de");
    assert_eq!(again, m);
    let mut other = ops_r1_json();
    other["contract"]["conditions"]["B"]["trial_ids"] = json!(["other"]);
    assert_ne!(
        parse(&other).compute_sha256().expect("h"),
        m.compute_sha256().expect("h")
    );
    let out = bin()
        .args(["validate", "--manifest"])
        .arg(&p)
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        m.compute_sha256().expect("h")
    );
}

#[test]
fn ops_r1_pipeline_manifest_rejects_bad_conditions() {
    let base = ops_r1_json();
    let mut v = base.clone();
    v["contract"]["conditions"]["B"]["intake"] = json!("direct_claude_code");
    assert!(validate_err(&v).contains("B.intake must be cos_thread"));
    let mut v = base.clone();
    v["contract"]["conditions"]["A"]["intake"] = json!("cos_thread");
    assert!(validate_err(&v).contains("A.intake must be direct_claude_code"));
    let mut v = base.clone();
    v["contract"]["conditions"]["B"]["condition_id"] = json!(A_COND);
    assert!(validate_err(&v).contains("unique"));
    let mut v = base.clone();
    v["contract"]["conditions"]["A"]["condition_id"] = json!(" ");
    assert!(validate_err(&v).contains("non-empty"));
    let mut v = base.clone();
    v["contract"]["conditions"]["B"]["trial_ids"] = json!([]);
    assert!(validate_err(&v).contains("repeats (1) entries"));
    let mut v = base.clone();
    v["contract"]["conditions"]["B"]["trial_ids"] = json!(["mlsys-main-4-ops-r1-k4p1-A-t001"]);
    assert!(validate_err(&v).contains("unique across arms"));
    let mut v = base.clone();
    v["contract"]["conditions"]
        .as_object_mut()
        .expect("obj")
        .remove("B");
    assert!(validate_err(&v).contains("cover exactly the active arms"));
    let mut v = base.clone();
    v["contract"]["conditions"]["A"]["extra"] = json!(1);
    assert!(serde_json::from_value::<CampaignManifest>(v).is_err());
    // CoS 受付の B に routing を固定しない（final の check_resolved）。
    let mut v = base.clone();
    v["configs"][1]["routing"] =
        json!({"lower_policy":"mlsys-min-1","online_update":false,"models":["claude-sonnet-5-5"]});
    assert!(validate_err(&v).contains("routing must not be pinned for the cos_thread intake"));
    // conditions を外すと B の routing 欠落は従来どおり拒否される（旧系列の検査は緩めない）。
    let mut v = base.clone();
    v["contract"]
        .as_object_mut()
        .expect("obj")
        .remove("conditions");
    assert!(validate_err(&v).contains("config B: routing must be pinned"));
}

#[test]
fn ops_r1_pipeline_submission_tools_reject_ops_r1_manifest() {
    let dir = tempfile::tempdir().expect("dir");
    let p = dir.path().join("ops-r1.json");
    std::fs::write(&p, ops_r1_json().to_string()).expect("write");
    let out_dir = dir.path().join("out");
    let cases: Vec<Vec<&str>> = vec![
        vec!["run", "--executor", "mock"],
        vec!["watch", "--executor", "celeris"],
        vec!["dry-run"],
        vec!["preflight"],
        vec!["config-check"],
        vec!["env-check"],
        vec!["cap-probe", "--case", CASE, "--arm", "b"],
    ];
    for args in cases {
        let out = bin()
            .args(&args)
            .arg("--manifest")
            .arg(&p)
            .arg("--out")
            .arg(&out_dir)
            .output()
            .expect("spawn");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} must fail");
        assert!(
            err.contains(&format!("{}: the ops-r1 manifest", args[0]))
                && err.contains("not a POST /tasks submission"),
            "{args:?}: {err}"
        );
        assert!(!out_dir.exists(), "{args:?} must not create the out dir");
    }
}

/// 時計と host を固定した human-op の呼び出し。
fn human_op(file: &Path, args: &[&str]) -> std::process::Output {
    bin()
        .arg("human-op")
        .args(args)
        .arg("--file")
        .arg(file)
        .args(["--now", "2026-10-30T01:00:00Z", "--host", "h1"])
        .output()
        .expect("spawn")
}

fn ok(out: std::process::Output) -> String {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn record_args<'a>(sel: &[&'a str], logical: &'a str) -> Vec<&'a str> {
    let mut v = vec!["record", "--campaign", "mlsys-main-4-ops-r1-k4p1"];
    v.extend_from_slice(sel);
    v.extend_from_slice(&[
        "--case",
        CASE,
        "--rep",
        "1",
        "--op-kind",
        "protocol_step",
        "--actor-kind",
        "human",
        "--actor",
        "evaluator",
        "--mode",
        "manual",
        "--intent",
        "protocol",
        "--logical-op",
        logical,
        "--started-at",
        "2026-10-30T00:00:00Z",
        "--ended-at",
        "2026-10-30T00:05:00Z",
    ]);
    v
}

#[test]
fn ops_r1_pipeline_human_op_records_and_lists_conditions() {
    let dir = tempfile::tempdir().expect("dir");
    let mp = dir.path().join("ops-r1.json");
    std::fs::write(&mp, ops_r1_json().to_string()).expect("write");
    let m = CampaignManifest::load(&mp).expect("load");
    let sha = m.compute_sha256().expect("h");
    let mps = mp.to_str().expect("utf8");
    let file = dir.path().join("human-ops.jsonl");
    for (cond, op) in [(A_COND, "a-start"), (B_COND, "b-intake")] {
        ok(human_op(
            &file,
            &record_args(&["--manifest", mps, "--condition", cond], op),
        ));
    }
    let recs = read_records(&file).expect("read");
    assert_eq!(recs.len(), 4, "op_start + op_end per condition");
    for r in &recs {
        r.validate().expect("valid");
        assert_eq!(r.experiment_set, ExperimentSet::OpsR1);
        assert_eq!(r.manifest_sha256, sha);
        let (want_arm, label) = match r.condition_id.as_deref() {
            Some(A_COND) => (ConfigId::A, "ops-r1-A"),
            Some(B_COND) => (ConfigId::B, "ops-r1-B"),
            o => panic!("condition_id {o:?}"),
        };
        assert_eq!((r.arm, r.arm_label()), (Some(want_arm), Some(label)));
    }
    let text = std::fs::read_to_string(&file).expect("read");
    assert!(text.contains("\"experiment_set\":\"ops-r1\"") && text.contains(B_COND));
    // 同じ事実の再送は行を増やさない（record_id は冪等）。
    ok(human_op(
        &file,
        &record_args(&["--manifest", mps, "--condition", B_COND], "b-intake"),
    ));
    assert_eq!(read_records(&file).expect("read").len(), 4);
    ok(human_op(&file, &["validate"]));

    // list --condition: その condition の記録だけ。arm_label は ops-r1-B。
    let v: Value = serde_json::from_str(&ok(human_op(
        &file,
        &["list", "--manifest", mps, "--condition", B_COND],
    )))
    .expect("json");
    let rs = v["records"].as_array().expect("records");
    assert_eq!(rs.len(), 2);
    assert!(
        rs.iter()
            .all(|r| r["arm_label"] == "ops-r1-B" && r["condition_id"] == B_COND)
    );
    assert_eq!(v["ops"][0]["duration_secs"], 300);
    assert_eq!(v["ops"][0]["arm_label"], "ops-r1-B");
    // --arm ops-r1-A でも絞れる（arm_label の比較）。
    let v: Value =
        serde_json::from_str(&ok(human_op(&file, &["list", "--arm", "ops-r1-A"]))).expect("json");
    assert_eq!(v["records"].as_array().expect("records").len(), 2);

    // correct --logical-op --condition: その condition の記録だけを取り消す。
    ok(human_op(
        &file,
        &[
            "correct",
            "--logical-op",
            "a-start",
            "--manifest",
            mps,
            "--condition",
            A_COND,
            "--retract",
            "--reason",
            "test",
        ],
    ));
    let v: Value = serde_json::from_str(&ok(human_op(
        &file,
        &["list", "--manifest", mps, "--condition", A_COND],
    )))
    .expect("json");
    assert_eq!(v["records"].as_array().expect("records").len(), 0);
    ok(human_op(&file, &["validate"]));

    // shared scope は experiment_set ops-r1 を受ける（試行に配らない）。
    let shared = [
        "record",
        "--campaign",
        "mlsys-main-4-ops-r1-k4p1",
        "--manifest",
        mps,
        "--scope",
        "shared",
        "--experiment-set",
        "ops-r1",
        "--op-kind",
        "protocol_step",
        "--actor-kind",
        "human",
        "--actor",
        "evaluator",
        "--mode",
        "manual",
        "--intent",
        "protocol",
        "--started-at",
        "2026-10-30T00:00:00Z",
    ];
    ok(human_op(&file, &shared));
    ok(human_op(&file, &["validate"]));

    // report: condition の記録は arm 別の表で ops-r1-A・ops-r1-B と表示される（event 側は空の records.jsonl）。
    let ev = dir.path().join("ev");
    std::fs::create_dir(&ev).expect("mkdir");
    std::fs::write(ev.join("records.jsonl"), "").expect("write");
    let rep = dir.path().join("rep");
    let out = bin()
        .args(["human-op", "report", "--file"])
        .arg(&file)
        .arg("--events")
        .arg(&ev)
        .arg("--out")
        .arg(&rep)
        .output()
        .expect("spawn");
    ok(out);
    let v: Value = serde_json::from_slice(&std::fs::read(rep.join("report.json")).expect("read"))
        .expect("json");
    let arms: Vec<&String> = v["arms"].as_object().expect("arms").keys().collect();
    // A の試行の記録は上の correct で取り消したので、B の表だけが残る。
    assert_eq!(arms, ["ops-r1-B"]);
    assert_eq!(v["arms"]["ops-r1-B"]["trials"], 1);
}

#[test]
fn ops_r1_pipeline_human_op_condition_rejections() {
    let dir = tempfile::tempdir().expect("dir");
    let mp = dir.path().join("ops-r1.json");
    std::fs::write(&mp, ops_r1_json().to_string()).expect("write");
    let mps = mp.to_str().expect("utf8");
    let sha = CampaignManifest::load(&mp)
        .expect("load")
        .compute_sha256()
        .expect("h");
    let file = dir.path().join("human-ops.jsonl");
    let fail = |sel: &[&str], want: &str| {
        let out = human_op(&file, &record_args(sel, "x"));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && err.contains(want),
            "{sel:?}: {err}"
        );
    };
    fail(
        &["--manifest", mps, "--condition", A_COND, "--arm", "A0"],
        "--condition と --arm は同時に書けない",
    );
    fail(
        &["--manifest-sha256", &sha, "--condition", A_COND],
        "--condition には --manifest（path）が要る",
    );
    fail(
        &["--manifest", mps, "--condition", "nope"],
        "is not declared",
    );
    let wrong = "0".repeat(64);
    fail(
        &[
            "--manifest",
            mps,
            "--manifest-sha256",
            &wrong,
            "--condition",
            A_COND,
        ],
        "campaign hash",
    );
    // 旧系列の manifest（conditions なし）では condition を引けない。
    let old = fixtures().join("frozen-manifests/main-4-v4-pilot-k4.json");
    fail(
        &[
            "--manifest",
            old.to_str().expect("utf8"),
            "--condition",
            A_COND,
        ],
        "is not declared",
    );
    let out = human_op(
        &file,
        &[
            "list",
            "--manifest",
            mps,
            "--condition",
            A_COND,
            "--arm",
            "A0",
        ],
    );
    assert!(!out.status.success());
    assert!(!file.exists(), "rejected records must not be written");
}

/// 旧系列の記録は condition_id を持たず、bytes と record_id は今までと同じ（record_id は固定値）。
#[test]
fn ops_r1_pipeline_old_human_op_records_unchanged() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("human-ops.jsonl");
    let sha = "e9fb2320303a16d17e38ea6831d1d53b337bb5a869514a2a4bbfa39c0b6ceee0";
    let mut args = record_args(&["--manifest-sha256", sha, "--arm", "B0"], "b0-op");
    args[2] = "mlsys-main-4-v4-pilot-k4";
    ok(human_op(&file, &args));
    let text = std::fs::read_to_string(&file).expect("read");
    assert!(!text.contains("condition_id"));
    let recs = read_records(&file).expect("read");
    let ids: Vec<&str> = recs.iter().map(|r| r.record_id.as_str()).collect();
    // 材料欄の正規化 JSON の sha256（compute_record_id。材料欄だけを Python で別に正規化・hash して同じ値になることを確かめた）。
    assert_eq!(
        ids,
        [
            "hop-d1606d625a80ff20fe5d280c5e70ff14e2b8979a78dfd52eb375f53b6f2ac32f",
            "hop-a2785e7a083f2d72a3d585eb6ac47edbbaa2f75cd345d465e35d43d149c2d7cd",
        ]
    );
    for (line, r) in text.lines().zip(&recs) {
        assert_eq!(r.experiment_set, ExperimentSet::MainAb);
        assert_eq!(r.arm_label(), Some("B0"));
        assert_eq!(r.condition_id, None);
        assert_eq!(
            serde_json::to_string(r).expect("ser"),
            line,
            "bytes unchanged"
        );
    }
    // 旧記録に condition_id を足すと検査で落ちる（ops-r1 の試行の記録だけが持つ）。
    let mut bad: HumanOpRecord = recs[0].clone();
    bad.condition_id = Some(A_COND.into());
    assert!(bad.validate().unwrap_err().contains("ops-r1 の試行の記録"));
    // ops-r1 の試行の記録は condition_id が要る。
    let mut bad: HumanOpRecord = recs[0].clone();
    bad.experiment_set = ExperimentSet::OpsR1;
    let bad = bad.with_record_id();
    assert!(bad.validate().unwrap_err().contains("condition_id が要る"));
}
