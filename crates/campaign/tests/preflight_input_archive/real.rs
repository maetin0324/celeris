//! Opt-in audit: real K4 bytes; fake host/K2 environment is never production evidence.
use super::*;
use celeris_campaign::{
    cells_validity,
    executor::{ExecStatus, Executor, MockExecutor, TrialRequest},
    grader::GradeCase,
    runner,
};

struct Solution {
    csv: String,
    report: String,
}
impl Executor for Solution {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn simulated(&self) -> bool {
        true
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        let status = MockExecutor::default().execute(req)?;
        fs::create_dir_all(req.workspace.join("results")).map_err(|e| e.to_string())?;
        fs::write(req.workspace.join("results/cells.csv"), &self.csv).map_err(|e| e.to_string())?;
        fs::write(req.workspace.join("report.md"), &self.report).map_err(|e| e.to_string())?;
        Ok(status)
    }
}

#[test]
#[ignore = "requires K4_AUDIT_DIR and K4_REAL_ARCHIVE; no LLM or daemon"]
fn real_archive_offline_audit() {
    let dest = PathBuf::from(std::env::var("K4_AUDIT_DIR").expect("audit dir"));
    let archive = PathBuf::from(std::env::var("K4_REAL_ARCHIVE").expect("real archive"));
    fs::create_dir_all(&dest).unwrap();
    let mut s = setup();
    s.inp.repo_src = None;
    let cp = s.inp.cases_root.join("final").join(K4).join("case.json");
    let fixture_kb = read(&cp)["snapshot"]["kb_snapshot"].clone();
    copy(&root().join("cases/final").join(K4), cp.parent().unwrap());
    let mut c = read(&cp);
    c["snapshot"]["kb_snapshot"] = fixture_kb;
    write(&cp, &c);
    fs::copy(
        root()
            .join("fixtures/grade-cases/final")
            .join(K4)
            .join(format!("{K4}.json")),
        grade_json(&s.inp.grade_cases_root),
    )
    .unwrap();
    // 合成 raw の tgz（合成用の名前）を外し、実 case の名前（公開入力 v2）で実 archive を置く。
    let real_input = c["inputs"][0]["name"]
        .as_str()
        .expect("input name")
        .to_string();
    let real_grader_archive = k4_grade_dir(&s.inp.grade_cases_root).join(&real_input);
    fs::remove_file(grader_archive(&s)).unwrap();
    fs::remove_file(s.inp.inputs_src.as_ref().unwrap().join(K4_INPUT)).unwrap();
    fs::copy(
        &archive,
        s.inp.inputs_src.as_ref().unwrap().join(&real_input),
    )
    .unwrap();
    // Isolate K4; K2 initial tree and host observations remain fixture-only.
    let ip = s
        .inp
        .manifest_path
        .with_file_name("final-input-manifest.json");
    let mut input: routing_eval::manifest::Manifest =
        serde_json::from_slice(&fs::read(&ip).unwrap()).unwrap();
    input.problems.retain(|p| p.id == K4);
    input.sha256 = celeris_campaign::manifest::manifest_hash(&input).unwrap();
    fs::write(&ip, serde_json::to_vec_pretty(&input).unwrap()).unwrap();
    let mut m = read(&s.inp.manifest_path);
    m["problems"] = json!([K4]);
    m["secondary"]["subset"] = json!([K4]);
    m["campaign_id"] = json!("k4-real-offline-audit");
    m["input"]["sha256"] = json!(input.sha256);
    write(&s.inp.manifest_path, &m);
    let bin = std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary");
    let mut records = Vec::new();
    let mut cli = |name: &str, args: Vec<String>, success: bool| {
        let o = Command::new(&bin).args(&args).output().unwrap();
        fs::write(dest.join(format!("{name}.stdout")), &o.stdout).unwrap();
        fs::write(dest.join(format!("{name}.stderr")), &o.stderr).unwrap();
        records.push(json!({"name":name,"argv":std::iter::once(bin.clone()).chain(args).collect::<Vec<_>>(),"exit":o.status.code(),"summary":String::from_utf8_lossy(if o.status.success(){&o.stdout}else{&o.stderr})}));
        assert_eq!(
            o.status.success(),
            success,
            "{name}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    };
    let stage = || {
        vec![
            "stage-grader-inputs".into(),
            "--manifest".into(),
            s.inp.manifest_path.display().to_string(),
            "--grade-case-root".into(),
            s.inp.grade_cases_root.display().to_string(),
            "--inputs-src".into(),
            s.inp.inputs_src.as_ref().unwrap().display().to_string(),
        ]
    };
    cli("stage-positive", stage(), true);
    let pf = check(&s.inp, &host());
    fs::write(
        dest.join("preflight-positive.json"),
        serde_json::to_vec_pretty(&pf).unwrap(),
    )
    .unwrap();
    assert!(pf.ok, "{:?}", pf.missing);
    let case = GradeCase::load(&grade_json(&s.inp.grade_cases_root)).unwrap();
    let rec = case.recompute.as_ref().unwrap();
    let raw = s.dir.path().join("raw");
    fs::create_dir_all(raw.join("inputs")).unwrap();
    celeris_campaign::asset_assembly::extract_tgz(&archive, &raw.join("inputs")).unwrap();
    let expected = cells_validity::compute(&raw, rec.cells.as_ref().unwrap()).unwrap();
    assert_eq!(expected.counts()["cells_valid"], 30.0);
    let five: String = rec
        .metrics
        .iter()
        .map(|m| {
            let celeris_campaign::grader::case::Source::Json { files, .. } = &m.source else {
                panic!("json")
            };
            format!("metric {} = {}\n", m.name, files.len())
        })
        .collect();
    let solution = Solution {
        csv: expected.csv(),
        report: five.clone() + &expected.metric_lines(),
    };
    let art = dest.join("positive-artifact");
    fs::create_dir_all(art.join("results")).unwrap();
    fs::write(art.join("results/cells.csv"), &solution.csv).unwrap();
    fs::write(art.join("report.md"), &solution.report).unwrap();
    let grade_args = |art: &Path| {
        vec![
            "grade".into(),
            "--case".into(),
            grade_json(&s.inp.grade_cases_root).display().to_string(),
            "--artifact".into(),
            art.display().to_string(),
            "--work".into(),
            s.dir.path().join("grade-work").display().to_string(),
        ]
    };
    cli("grade-positive", grade_args(&art), true);
    let m = CampaignManifest::load(&s.inp.manifest_path).unwrap();
    let out = s.dir.path().join("out");
    let opts = runner::RunOptions {
        out: out.clone(),
        repo_src: s.inp.repo_src.clone(),
        materials_src: s.inp.materials_src.clone(),
        kb_src: s.inp.kb_src.clone(),
        inputs_src: s.inp.inputs_src.clone(),
        case_inputs: celeris_campaign::inputs::for_problems(
            &celeris_campaign::cases::CaseSet::load(&s.inp.cases_root, &s.inp.grader_root).unwrap(),
            &m.problems,
        ),
        ..Default::default()
    };
    run_gated(&s.inp, &host(), &m, &input, &solution, &opts).unwrap();
    let common = || {
        vec![
            "--manifest".into(),
            s.inp.manifest_path.display().to_string(),
            "--out".into(),
            out.display().to_string(),
            "--cases-root".into(),
            s.inp.cases_root.display().to_string(),
            "--grader-root".into(),
            s.inp.grader_root.display().to_string(),
            "--grade-case-root".into(),
            s.inp.grade_cases_root.display().to_string(),
        ]
    };
    for sub in ["grade-out", "check", "export"] {
        let mut args = vec![sub.into()];
        args.extend(common());
        if sub == "grade-out" {
            args.extend([
                "--cases".into(),
                s.inp.grade_cases_root.display().to_string(),
            ]);
        }
        cli(sub, args, true);
    }
    let bad = dest.join("all-valid-artifact");
    fs::create_dir_all(bad.join("results")).unwrap();
    let mut wrong = expected.clone();
    for row in &mut wrong.rows {
        row.valid = true;
    }
    fs::write(bad.join("results/cells.csv"), wrong.csv()).unwrap();
    fs::write(bad.join("report.md"), five + &wrong.metric_lines()).unwrap();
    cli("grade-all-valid-negative", grade_args(&bad), false);
    // Preserve the positive run while the rejection helper checks a fresh out.
    fs::rename(&out, s.dir.path().join("positive-out")).unwrap();
    fs::remove_file(&real_grader_archive).unwrap();
    assert_rejected_before_run(&s, "missing next to the grade-case");
    fs::write(
        dest.join("preflight-missing.json"),
        serde_json::to_vec_pretty(&check(&s.inp, &host())).unwrap(),
    )
    .unwrap();
    let public = s.inp.inputs_src.as_ref().unwrap().join(&real_input);
    fs::remove_file(&public).unwrap();
    cli("stage-missing-negative", stage(), false);
    fs::write(&public, b"wrong sha").unwrap();
    cli("stage-sha-negative", stage(), false);
    fs::write(&real_grader_archive, b"wrong sha").unwrap();
    fs::copy(&archive, &public).unwrap();
    assert_rejected_before_run(&s, "differs from the pinned");
    fs::write(
        dest.join("preflight-sha.json"),
        serde_json::to_vec_pretty(&check(&s.inp, &host())).unwrap(),
    )
    .unwrap();
    fs::copy(&archive, &real_grader_archive).unwrap();
    let retained = dest.join("fixture");
    copy(s.dir.path(), &retained);
    let kept = s.dir.keep();
    fs::write(dest.join("summary.json"),serde_json::to_vec_pretty(&json!({"schema":"celeris.k4-offline-audit/1","real_archive":archive,"raw_sha256":sha(&archive),"fixture_root":kept,"retained_fixture":retained,"positive_out_saved_as":"fixture/positive-out","audit_argv":std::env::args().collect::<Vec<_>>(),"commands":records,"preflight":{"mode":"library check/run_gated with fake Host","positive":true,"negative_missing":true,"negative_sha":true,"paid_calls_made":0},"blocker":"Actual launcher/daemon/cap-env/release connection is pending; fixture host and simulated executor are not production evidence."})).unwrap()).unwrap();
}
