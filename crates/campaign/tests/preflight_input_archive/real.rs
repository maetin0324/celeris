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
    let me: Vec<String> = std::env::args().collect();
    // 各 case は argv・exit・期待 exit を残し、全部を記録してから最後に一致を確かめる（途中で落ちても記録が残る）。
    let mut cases: Vec<Value> = Vec::new();
    let cli = |cases: &mut Vec<Value>, name: &str, args: Vec<String>, expect_exit: i32| {
        let o = Command::new(&bin).args(&args).output().unwrap();
        fs::write(dest.join(format!("{name}.stdout")), &o.stdout).unwrap();
        fs::write(dest.join(format!("{name}.stderr")), &o.stderr).unwrap();
        let exit = o.status.code();
        cases.push(json!({"name":name,"kind":"cli","argv":std::iter::once(bin.clone()).chain(args).collect::<Vec<_>>(),"exit":exit,"expect_exit":expect_exit,"matches":exit==Some(expect_exit),"summary":String::from_utf8_lossy(if o.status.success(){&o.stdout}else{&o.stderr}).chars().take(4000).collect::<String>()}));
    };
    // preflight は既存試験と同じく偽 Host の library 呼び出し（check と run_gated）。exit は不足ありで 1、なしで 0。
    // 拒否の case では transport（LLM run・task 作成）の呼び出し数と out の有無も残す。
    let pre = |cases: &mut Vec<Value>, s: &Setup, name: &str, needle: Option<&str>| {
        let r = check(&s.inp, &host());
        fs::write(
            dest.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&r).unwrap(),
        )
        .unwrap();
        let mut c = json!({"name":name,"kind":"library","argv":me.iter().cloned().chain([format!("--case={name}")]).collect::<Vec<_>>(),"exit":if r.ok {0} else {1},"expect_exit":if needle.is_some() {1} else {0},"paid_calls_made":r.paid_calls_made,"missing":r.missing.iter().map(|m| format!("{}: {}", m.check, m.detail)).collect::<Vec<_>>()});
        let mut ok = (r.ok == needle.is_none()) && r.paid_calls_made == 0;
        if let Some(needle) = needle {
            let m = CampaignManifest::load(&s.inp.manifest_path).unwrap();
            let input = m.load_input(&s.inp.manifest_path, None).unwrap();
            let out = s.dir.path().join("out");
            let t = CountingTransport(AtomicUsize::new(0));
            let exec =
                CelerisExecutor::new(&t, serde_json::Map::new(), 1800, out.join("cancel.jsonl"));
            let opts = RunOptions {
                global_ledger: None,
                out: out.clone(),
                ..Default::default()
            };
            let e = run_gated(&s.inp, &host(), &m, &input, &exec, &opts).err();
            let calls = t.0.load(Ordering::SeqCst);
            let named = r
                .missing
                .iter()
                .any(|m| m.check == "graders" && m.detail.contains(needle));
            let only_graders = r.missing.iter().all(|m| m.check == "graders");
            ok &= named
                && only_graders
                && calls == 0
                && !out.exists()
                && e.as_deref().is_some_and(|e| e.contains(needle));
            c["transport_calls"] = json!(calls);
            c["out_created"] = json!(out.exists());
            c["run_gated_error"] = json!(e);
            c["needle"] = json!(needle);
        }
        c["matches"] = json!(ok);
        cases.push(c);
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
    cli(&mut cases, "stage-positive", stage(), 0);
    pre(&mut cases, &s, "preflight-positive", None);
    let case = GradeCase::load(&grade_json(&s.inp.grade_cases_root)).unwrap();
    let rec = case.recompute.as_ref().unwrap();
    let raw = s.dir.path().join("raw");
    fs::create_dir_all(raw.join("inputs")).unwrap();
    celeris_campaign::asset_assembly::extract_tgz(&archive, &raw.join("inputs")).unwrap();
    let expected = cells_validity::compute(&raw, rec.cells.as_ref().unwrap()).unwrap();
    let counts = expected.counts();
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
    let report = |e: &cells_validity::Expected| Solution {
        csv: e.csv(),
        report: five.clone() + &e.metric_lines(),
    };
    let solution = report(&expected);
    let artifact = |name: &str, sol: &Solution| {
        let art = dest.join(name);
        fs::create_dir_all(art.join("results")).unwrap();
        fs::write(art.join("results/cells.csv"), &sol.csv).unwrap();
        fs::write(art.join("report.md"), &sol.report).unwrap();
        art
    };
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
    let art = artifact("positive-artifact", &solution);
    cli(&mut cases, "grade-positive", grade_args(&art), 0);
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
        cli(&mut cases, sub, args, 0);
    }
    // 独立負例: 1 つずつ別の誤りだけを入れた報告。
    let mut all_valid = expected.clone();
    for row in &mut all_valid.rows {
        row.valid = true;
        row.reasons.clear();
    }
    // 失敗 job（F/-20）の BenchFS W2 だけを有効にした報告（32/32）。
    let mut w2_valid = expected.clone();
    for row in &mut w2_valid.rows {
        if row.fs == "benchfs" && row.workload == "w2" {
            row.valid = true;
            row.reasons.clear();
        }
    }
    // 件数は正しく、有効セル 1 つの write_MiB_s だけを 2% 誤った報告。
    let mut wrong_value = expected.clone();
    *wrong_value
        .rows
        .iter_mut()
        .find(|r| r.valid && r.values.contains_key("write_MiB_s"))
        .expect("valid ior row")
        .values
        .get_mut("write_MiB_s")
        .expect("value") *= 1.02;
    // 欠測の 2 セルを表と分母から落とした報告（30/30）。
    let mut dropped = expected.clone();
    dropped.rows.retain(|r| r.valid);
    let negatives = [
        ("grade-all-valid-negative", &all_valid),
        ("grade-invalid-w2-as-valid-negative", &w2_valid),
        ("grade-numeric-error-negative", &wrong_value),
        ("grade-missing-dropped-from-denominator-negative", &dropped),
    ];
    let mut negative_counts = serde_json::Map::new();
    for (name, e) in negatives {
        let art = artifact(&name.replace("grade-", ""), &report(e));
        negative_counts.insert(name.into(), json!(e.counts()));
        cli(&mut cases, name, grade_args(&art), 1);
    }
    // Preserve the positive run while the rejection helper checks a fresh out.
    fs::rename(&out, s.dir.path().join("positive-out")).unwrap();
    fs::remove_file(&real_grader_archive).unwrap();
    pre(
        &mut cases,
        &s,
        "preflight-missing-negative",
        Some("missing next to the grade-case"),
    );
    let public = s.inp.inputs_src.as_ref().unwrap().join(&real_input);
    fs::remove_file(&public).unwrap();
    cli(&mut cases, "stage-missing-negative", stage(), 1);
    fs::write(&public, b"wrong sha").unwrap();
    cli(&mut cases, "stage-sha-negative", stage(), 1);
    // 不一致の stage は grade-case の隣に何も置かない。
    let staged_after_sha_negative = real_grader_archive.exists();
    fs::write(&real_grader_archive, b"wrong sha").unwrap();
    fs::copy(&archive, &public).unwrap();
    pre(
        &mut cases,
        &s,
        "preflight-sha-negative",
        Some("differs from the pinned"),
    );
    fs::copy(&archive, &real_grader_archive).unwrap();
    // 小さい再現用の資産だけ残す（fixture 全体は 1 GB を超える）。
    let kept = dest.join("fixture");
    fs::create_dir_all(&kept).unwrap();
    for p in [
        s.inp.manifest_path.clone(),
        ip.clone(),
        cp.clone(),
        grade_json(&s.inp.grade_cases_root),
    ] {
        fs::copy(&p, kept.join(p.file_name().unwrap())).unwrap();
    }
    let all_match = cases.iter().all(|c| c["matches"] == json!(true));
    fs::write(dest.join("audit.json"),serde_json::to_vec_pretty(&json!({"schema":"celeris.k4-offline-audit/2","real_archive":archive,"raw_sha256":sha(&archive),"case_input":{"name":real_input,"sha256":c["inputs"][0]["sha256"]},"audit_argv":me,"cases":cases,"all_match":all_match,"expected_counts":counts,"negative_counts":negative_counts,"staged_after_sha_negative":staged_after_sha_negative,"preflight_mode":"library check/run_gated with fake Host (no daemon, no LLM)","measurement_claim":false,"blocker":"Actual launcher/daemon/cap-env/release connection is pending; fixture host and simulated executor are not production evidence."})).unwrap()).unwrap();
    assert_eq!(counts["cells_total"], 32.0);
    assert_eq!(counts["cells_valid"], 30.0);
    assert!(!staged_after_sha_negative);
    assert!(
        all_match,
        "a case did not match its expected exit; see audit.json"
    );
}
