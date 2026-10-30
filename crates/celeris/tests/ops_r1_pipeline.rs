//! ops-r1 の採点〜集計の全段（docs/ops/ops-r1-pipeline.md §4・§6）を A・B の stub 記録で通す。
//! A は stub の raw（`ops-r1-a-raw/1`）、B は stub の `ops-r1-intake/1` と、本物の store で書いた DB の写しから
//! `celeris-campaign ops-r1-trace`・`ops-r1-attribution` で出した記録。そこから ops-r1-assemble → stage-grader-inputs →
//! grade-out → check → export → human-op report → final-accounting → `scripts/campaign/check-out-consistency.py` を
//! 実 binary で走らせ、全段 exit 0、試行数・分母・終了理由・合否が段の間で一致すること、同じ raw から 2 回出した
//! 出力が byte 一致することを確かめる。LLM・daemon・本番 DB・~/.config/celeris は使わない。
#[path = "ops_r1/trace_fixture.rs"]
mod trace_fixture;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
};

use serde_json::{Value, json};
use trace_fixture::{Fixture, Shape, TRIAL as B_TRIAL, ts};

const A_COND: &str = "mlsys-main-4-ops-r1-k4p1-A-direct-claude-code";
const B_COND: &str = "mlsys-main-4-ops-r1-k4p1-B-celeris-ops";
const A_TRIAL: &str = "mlsys-main-4-ops-r1-k4p1-A-t001";
const MEASUREMENT: &str = "mlsys-main-4-ops-r1-k4p1";
const K4: &str = "k4-cmp4-validity-analysis";
const SONNET: &str = "claude-sonnet-5-5";
const OPUS: &str = "claude-opus-5-5";
const TGZ: &str = "main-v3-n2-43706.tgz";
/// 試行の終わり方（A の raw と B の打ち切り）と、成果物が採点に通るか。
#[derive(Clone, Copy)]
struct Scenario {
    a_stop: &'static str,
    a_exit: Option<i32>,
    a_correct: bool,
    b_correct: bool,
    /// B の打ち切り。trace fixture は受付 10s・done 160s・報告 170s（2026-10-29T01:00:00Z 起点）。
    b_cutoff: &'static str,
}

/// A は打ち切り（timeout）、B は報告まで届き採点にも通る。
const A_CUT_B_PASS: Scenario = Scenario {
    a_stop: "cutoff",
    a_exit: None,
    a_correct: false,
    b_correct: true,
    b_cutoff: "2026-10-29T11:00:00+09:00",
};

/// A は exit 0 で採点に通り、B は done（160s）より前に打ち切られる。
const A_PASS_B_CUT: Scenario = Scenario {
    a_stop: "exited",
    a_exit: Some(0),
    a_correct: true,
    b_correct: false,
    b_cutoff: "2026-10-29T01:02:00Z",
};

fn campaign_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../campaign")
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// celeris-campaign の binary（celeris の試験からは CARGO_BIN_EXE で引けないので、同じ target に build する）。
fn campaign_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let p = Path::new(env!("CARGO_BIN_EXE_celeris"))
            .parent()
            .expect("bin dir")
            .join("celeris-campaign");
        // 古い binary を使わないよう毎回 build する（新しければ何もしない）。
        let st = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args([
                "build",
                "-p",
                "celeris-campaign",
                "--bin",
                "celeris-campaign",
            ])
            .status()
            .expect("cargo build");
        assert!(st.success(), "cargo build celeris-campaign");
        assert!(p.exists(), "{} was not built", p.display());
        p
    })
    .clone()
}

fn bin(args: &[String]) -> Output {
    Command::new(campaign_bin())
        .args(args)
        .env_remove("CELERIS_CAMPAIGN_BIN")
        .output()
        .expect("spawn celeris-campaign")
}

fn ok(o: &Output, what: &str) -> String {
    assert!(
        o.status.success(),
        "{what}: exit {:?}\nstdout {}\nstderr {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn read(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn jsonl(p: &Path) -> Vec<Value> {
    fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("jsonl line"))
        .collect()
}

/// 1 回の通しで変わらない入力（資産・manifest・A の raw・B の DB の写し・human-op の記録）。
struct Inputs {
    _dir: tempfile::TempDir,
    sc: Scenario,
    d: PathBuf,
    db: Fixture,
    manifest: PathBuf,
    input: PathBuf,
    cases: PathBuf,
    grade: PathBuf,
    ins: PathBuf,
    a_raw: PathBuf,
    a_art: PathBuf,
    b_art: PathBuf,
    b_intake: PathBuf,
    b_versions: PathBuf,
    human_ops: PathBuf,
    init: String,
}

/// ops-r1 の manifest（docs/ops/ops-r1-manifest.template.json の `REPLACE:` を試験の資産で埋めたもの）。
/// B の trial_id は trace fixture の試行 id。
fn manifest_json(success: Option<&str>) -> Value {
    let text = fs::read_to_string(repo().join("docs/ops/ops-r1-manifest.template.json"))
        .expect("template");
    let mut m: Value = serde_json::from_str(&text).expect("json");
    let k4 = read(&campaign_root().join("fixtures/frozen-manifests/main-4-v4-pilot-k4.json"));
    let input =
        read(&repo().join("agent-docs/progress/2026-10-16-mlsys-final/final-input-manifest.json"));
    m["input"]["sha256"] = input["sha256"].clone();
    m["snapshot"] = k4["snapshot"].clone();
    for arm in ["A", "B"] {
        m["contract"]["pinned_versions"][arm]["claude_code_sha256"] = json!("a".repeat(64));
    }
    m["contract"]["pinned_versions"]["B"]["pricing_version"] = json!("p1");
    m["contract"]["conditions"]["B"]["trial_ids"] = json!([B_TRIAL]);
    let set = &mut m["experiment_sets"]["ops-r1"]["fixed"];
    set["initial_info"] = k4["snapshot"]["materials_sha256"].clone();
    match success {
        Some(x) => set["success"] = json!(x),
        None => {
            if let Some(f) = set.as_object_mut() {
                f.remove("success");
            }
        }
    }
    m
}

impl Inputs {
    fn new(sc: Scenario) -> Self {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path().to_path_buf();
        let (cases, grade, ins) = (d.join("cases"), d.join("grade-cases"), d.join("inputs-src"));
        let copy = |a: &Path, b: &Path| celeris_campaign::snapshot::copy_tree(a, b).expect("copy");
        copy(&campaign_root().join("cases"), &cases);
        copy(&campaign_root().join("fixtures/grade-cases"), &grade);
        fs::create_dir_all(&ins).expect("ins");
        celeris_campaign::cells_validity::synth::healthy()
            .install_k4_fixture(&grade, &cases, &ins)
            .expect("k4 fixture");
        // stage-grader-inputs が写し戻すことを見るため、grade-case の隣の公開入力 tgz を消しておく。
        fs::remove_file(grade.join("final").join(K4).join(TGZ)).expect("rm staged tgz");
        let input = d.join("final-input-manifest.json");
        fs::copy(
            repo().join("agent-docs/progress/2026-10-16-mlsys-final/final-input-manifest.json"),
            &input,
        )
        .expect("input manifest");
        let manifest = d.join("ops-r1.json");
        fs::write(&manifest, manifest_json(None).to_string()).expect("manifest");
        // success（grader の固定 hash）は lock から取って書き直す。
        let m = celeris_campaign::manifest::CampaignManifest::load(&manifest).expect("load");
        let lock = celeris_campaign::lock_exec::expected_lock(
            &celeris_campaign::lock_exec::LockSpec {
                cases_root: &cases,
                grader_root: &campaign_root().join("fixtures/grader-private"),
                grade_cases_root: &grade,
                measurement_id: None,
            },
            &m,
        )
        .expect("lock");
        fs::write(
            &manifest,
            manifest_json(Some(&lock.grader_set_sha256())).to_string(),
        )
        .expect("manifest");

        // 試行の成果物（初期 workspace = 公開入力だけ + report.md）。
        let (a_art, b_art) = (d.join("a-artifact"), d.join("b-artifact"));
        let mut init = String::new();
        for w in [&a_art, &b_art] {
            fs::create_dir_all(w.join("inputs")).expect("ws");
            fs::copy(ins.join(TGZ), w.join("inputs").join(TGZ)).expect("tgz");
            init = celeris_campaign::inputs::artifact_hash(w, K4).expect("hash");
        }
        // 採点に通る成果物は合成資料の正解（cells.csv と報告の件数）、通らないものは中身の無い報告。
        let plan = celeris_campaign::cells_validity::synth::healthy();
        let raw_root = d.join("expected-src");
        celeris_campaign::cells_validity::synth::write(&raw_root.join("inputs"), &plan)
            .expect("synth");
        let expected = celeris_campaign::cells_validity::compute(
            &raw_root,
            &celeris_campaign::cells_validity::synth::spec(&plan, "inputs"),
        )
        .expect("expected");
        for (w, correct, name) in [(&a_art, sc.a_correct, "A"), (&b_art, sc.b_correct, "B")] {
            if correct {
                fs::create_dir_all(w.join("results")).expect("results");
                fs::write(w.join("results/cells.csv"), expected.csv()).expect("csv");
                fs::write(w.join("report.md"), expected.metric_lines()).expect("report");
            } else {
                fs::write(w.join("report.md"), format!("# {name} report\n")).expect("report");
            }
        }

        let sha = "a".repeat(64);
        let a_raw = d.join("a-raw.json");
        let raw = json!({
            "schema": "ops-r1-a-raw/1", "trial_id": A_TRIAL, "condition_id": A_COND,
            "started_at": "2026-10-29T03:00:00Z", "ended_at": "2026-10-29T03:20:00Z",
            "stop": sc.a_stop, "exit_code": sc.a_exit,
            "claude_code": {"version": "2.1.303", "sha256": sha},
            "cost_usd": 1.25, "workspace_sha256_initial": init, "session_id": "sess-a",
            "models": [SONNET],
            "token_usage": {"input": 1000, "output": 200, "cache_read": 3000, "cache_creation": 400},
        });
        fs::write(&a_raw, raw.to_string()).expect("a raw");

        // B: 本物の store で書いた DB の写し。task の run と CoS の chat run に model・harness を入れる。
        let db = Fixture::new(Shape { report: true });
        {
            let conn = rusqlite::Connection::open(&db.copy).expect("open copy");
            conn.execute("UPDATE runs SET model=?1, adapter='claude_code'", [SONNET])
                .expect("runs model");
            conn.execute(
                "UPDATE chat_runs SET resolved_config_json=?1",
                [json!({"model": OPUS, "harness": "claude_code"}).to_string()],
            )
            .expect("chat_runs model");
        }
        let b_intake = d.join("b-intake.json");
        let intake = json!({
            "schema": "ops-r1-intake/1", "trial_id": B_TRIAL, "condition_id": B_COND,
            "thread_id": db.thread, "project_id": db.project, "message_id": db.intake_message,
            "message_seq": 1, "accepted_at": ts(10), "request_sha256": sha, "request_bytes": 10,
            "intake_mode": "operator_post", "client_message_id": format!("ops-r1-intake:{B_TRIAL}"),
            "actor": {"kind": "human", "id": "human:tester"},
        });
        fs::write(&b_intake, intake.to_string()).expect("intake");
        let b_versions = d.join("b-versions.json");
        let versions = json!({"release": "94f86928dc13",
            "claude_code": {"path": "", "sha256": sha, "version": "2.1.303 (Claude Code)"},
            "pricing_version": "p1", "cos_policy_version": "cos-v6"});
        fs::write(&b_versions, versions.to_string()).expect("versions");

        let mut me = Self {
            _dir: dir,
            sc,
            d,
            db,
            manifest,
            input,
            cases,
            grade,
            ins,
            a_raw,
            a_art,
            b_art,
            b_intake,
            b_versions,
            human_ops: PathBuf::new(),
            init,
        };
        me.human_ops = me.record_human_ops();
        me
    }

    fn lock_args(&self) -> Vec<String> {
        vec![
            "--cases-root".into(),
            s(&self.cases),
            "--grader-root".into(),
            s(&campaign_root().join("fixtures/grader-private")),
            "--grade-case-root".into(),
            s(&self.grade),
            "--measurement-id".into(),
            MEASUREMENT.into(),
        ]
    }

    fn manifest_args(&self) -> Vec<String> {
        vec![
            "--manifest".into(),
            s(&self.manifest),
            "--input-manifest".into(),
            s(&self.input),
        ]
    }

    /// 人の操作の生記録（両 condition の試行開始の手順 1 件ずつ）。
    fn record_human_ops(&self) -> PathBuf {
        let file = self.d.join("human-ops.jsonl");
        for (cond, op, from, to) in [
            (
                A_COND,
                "a-start",
                "2026-10-29T02:58:00Z",
                "2026-10-29T03:00:00Z",
            ),
            (
                B_COND,
                "b-intake",
                "2026-10-29T00:59:00Z",
                "2026-10-29T01:00:10Z",
            ),
        ] {
            let a = args(&[
                "human-op",
                "record",
                "--file",
                &s(&file),
                "--campaign",
                MEASUREMENT,
                "--manifest",
                &s(&self.manifest),
                "--condition",
                cond,
                "--case",
                K4,
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
                op,
                "--started-at",
                from,
                "--ended-at",
                to,
            ]);
            ok(&bin(&a), "human-op record");
        }
        file
    }
}

/// 1 回の通しの出力。
struct Pass {
    root: PathBuf,
    out: PathBuf,
    records: PathBuf,
    trace: Value,
    attr: Value,
    grade: Value,
    check: Value,
    fa: Value,
    report: Value,
    table: Value,
    consistency: Value,
}

fn stage(i: &Inputs, cmd: &str, out: &Path, extra: &[String]) -> Output {
    let mut a = vec![cmd.to_string()];
    a.extend(i.manifest_args());
    a.extend(i.lock_args());
    a.extend(["--out".into(), s(out)]);
    a.extend(extra.iter().cloned());
    bin(&a)
}

fn pipeline(i: &Inputs, tag: &str) -> Pass {
    let root = i.d.join(format!("pass-{tag}"));
    fs::create_dir_all(&root).expect("pass dir");
    // B: DB の写しから trace と attribution。
    let trace_p = root.join("b-trace.json");
    let attr_p = root.join("b-attribution.json");
    let base = |cmd: &str, out: &Path| {
        args(&[
            cmd,
            "--db",
            &s(&i.db.copy),
            "--thread",
            &i.db.thread,
            "--trial-id",
            B_TRIAL,
            "--cutoff",
            i.sc.b_cutoff,
            "--out",
            &s(out),
        ])
    };
    ok(&bin(&base("ops-r1-trace", &trace_p)), "ops-r1-trace");
    let mut a = base("ops-r1-attribution", &attr_p);
    a.extend(["--condition-id".into(), B_COND.into()]);
    ok(&bin(&a), "ops-r1-attribution");

    // stage-grader-inputs: ops-r1 manifest で grade-case の隣へ公開入力 tgz を置く（2 回目は置いてあるので何もしない）。
    ok(
        &bin(&args(&[
            "stage-grader-inputs",
            "--manifest",
            &s(&i.manifest),
            "--grade-case-root",
            &s(&i.grade),
            "--inputs-src",
            &s(&i.ins),
        ])),
        "stage-grader-inputs",
    );
    assert_eq!(
        fs::read(i.grade.join("final").join(K4).join(TGZ)).expect("staged"),
        fs::read(i.ins.join(TGZ)).expect("src")
    );

    let out = root.join("out");
    let dl = root.join("dl");
    fs::create_dir_all(&dl).expect("dl");
    let records = dl.join("records.jsonl");
    let mut a = vec!["ops-r1-assemble".to_string()];
    a.extend(i.manifest_args());
    a.extend(i.lock_args());
    for (k, p) in [
        ("--a-raw", &i.a_raw),
        ("--a-artifact", &i.a_art),
        ("--b-intake", &i.b_intake),
        ("--b-trace", &trace_p),
        ("--b-attribution", &attr_p),
        ("--b-artifact", &i.b_art),
        ("--b-observed-versions", &i.b_versions),
        ("--out", &out),
        ("--records-out", &records),
    ] {
        a.extend([k.to_string(), s(p)]);
    }
    a.extend(["--b-workspace-sha256-initial".into(), i.init.clone()]);
    ok(&bin(&a), "ops-r1-assemble");

    let grade: Value = serde_json::from_str(&ok(
        &stage(i, "grade-out", &out, &["--cases".into(), s(&i.grade)]),
        "grade-out",
    ))
    .expect("grade-out json");
    let check_txt = ok(&stage(i, "check", &out, &[]), "check");
    let check_p = root.join("check.json");
    fs::write(&check_p, &check_txt).expect("check json");
    let check: Value = serde_json::from_str(&check_txt).expect("check json");

    let rec_args = vec!["--records".to_string(), s(&records)];
    let export = out.join("export");
    let mut ex = rec_args.clone();
    ex.extend(["--dest".into(), s(&export)]);
    ok(&stage(i, "export", &out, &ex), "export");

    let report_dir = root.join("human-ops-report");
    ok(
        &bin(&args(&[
            "human-op",
            "report",
            "--file",
            &s(&i.human_ops),
            "--events",
            &s(&out),
            "--events",
            &s(&dl),
            "--out",
            &s(&report_dir),
        ])),
        "human-op report",
    );

    let fa_txt = ok(
        &stage(i, "final-accounting", &out, &rec_args),
        "final-accounting",
    );
    let fa_p = root.join("final-accounting.json");
    fs::write(&fa_p, &fa_txt).expect("fa");
    let fa: Value = serde_json::from_str(&fa_txt).expect("fa json");

    let cons_p = root.join("consistency.json");
    let o = Command::new("python3")
        .arg(repo().join("scripts/campaign/check-out-consistency.py"))
        .args([
            "--out",
            &s(&out),
            "--check-json",
            &s(&check_p),
            "--export-dir",
            &s(&export),
            "--human-ops-report",
            &s(&report_dir),
            "--final-accounting",
            &s(&fa_p),
            "--records",
            &s(&records),
            "--json-out",
            &s(&cons_p),
        ])
        .output()
        .expect("python3");
    ok(&o, "check-out-consistency.py");

    let table = ["table.json", "table.incomplete.json"]
        .iter()
        .map(|f| export.join(f))
        .find(|p| p.exists())
        .map(|p| read(&p))
        .expect("export table");
    Pass {
        trace: read(&trace_p),
        attr: read(&attr_p),
        report: read(&report_dir.join("report.json")),
        consistency: read(&cons_p),
        root,
        out,
        records,
        grade,
        check,
        fa,
        table,
    }
}

fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        for e in fs::read_dir(&p).expect("read_dir") {
            let e = e.expect("entry").path();
            if e.is_dir() {
                stack.push(e);
            } else {
                let rel = e.strip_prefix(dir).expect("rel").display().to_string();
                m.insert(rel, fs::read(&e).expect("read"));
            }
        }
    }
    m
}

/// arm ごとの期待: (config, 終了 outcome, reason, executor の passed, 採点の final_pass)。
type Want = [(&'static str, &'static str, Option<&'static str>, bool, bool); 2];

fn want(sc: Scenario) -> Want {
    let a = if sc.a_stop == "cutoff" {
        ("A", "timeout", None, false, false)
    } else {
        ("A", "passed", None, true, sc.a_correct)
    };
    let b_reported = sc.b_cutoff != A_PASS_B_CUT.b_cutoff;
    let b = if b_reported {
        ("B", "passed", None, true, sc.b_correct)
    } else {
        ("B", "timeout", None, false, false)
    };
    [a, b]
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("not a count: {v}"))
}

/// 試行数・分母・終了理由・合否が段の間で一致すること。
fn assert_agree(i: &Inputs, p: &Pass) {
    let w = want(i.sc);
    let trials = jsonl(&p.out.join("trials.jsonl"));
    let grades = jsonl(&p.out.join("grades.jsonl"));
    // 分母: manifest の計画 cell は arm ごとに 1（ops-r1 の 1 case × 1 rep）。
    assert_eq!(trials.len(), 2, "trials.jsonl");
    assert_eq!(grades.len(), 2, "grades.jsonl");
    assert_eq!(n(&p.grade["graded"]), 2, "grade-out graded");
    assert_eq!(p.grade["ok"], true, "{}", p.grade);
    let finals = w.iter().filter(|x| x.4).count() as u64;
    assert_eq!(n(&p.grade["final_pass"]), finals, "grade-out final_pass");
    assert_eq!(p.check["ok"], true, "check: {}", p.check["violations"]);
    assert_eq!(
        p.check["complete"], true,
        "{}",
        p.check["incomplete_reasons"]
    );
    let fa_cells = p.fa["trials"]["cells"].as_array().expect("fa cells");
    assert_eq!(fa_cells.len(), 2, "final-accounting cells");
    let rows: BTreeMap<&str, &Value> = p.table["configs"]
        .as_array()
        .expect("export configs")
        .iter()
        .map(|r| (r["config"].as_str().expect("config"), r))
        .collect();
    assert_eq!(rows.len(), 2, "export rows");
    assert_eq!(p.table["complete"], true, "export complete");

    for (arm, outcome, reason, passed, final_pass) in w {
        let ctx = format!("arm {arm}");
        let t = trials
            .iter()
            .find(|t| t["config"] == arm)
            .unwrap_or_else(|| panic!("{ctx}: trial"));
        // 終了理由と合否（runner 形の生記録）。
        assert_eq!(t["outcome"], outcome, "{ctx}: outcome");
        assert_eq!(t["reason"].as_str(), reason, "{ctx}: reason");
        assert_eq!(t["passed"], passed, "{ctx}: passed");
        let g = grades
            .iter()
            .find(|g| g["trial"]["config"] == arm)
            .unwrap_or_else(|| panic!("{ctx}: grade"));
        assert_eq!(g["graded"], true, "{ctx}: graded");
        assert_eq!(g["grade"]["final_pass"], final_pass, "{ctx}: final_pass");
        let validated = passed && final_pass;

        // checker の再計算・export の表・human-op report の arm 表が同じ数を持つ。
        let c = &p.check["recomputed"]["totals"][arm];
        let e = rows[arm];
        let r = &p.report["arms"][format!("ops-r1-{arm}")];
        let is = |o: &str| u64::from(outcome == o);
        for (src, v) in [("checker", c), ("export", e)] {
            assert_eq!(n(&v["n"]), 1, "{ctx}: {src} n");
            assert_eq!(n(&v["ran_trials"]), 1, "{ctx}: {src} ran_trials");
            assert_eq!(n(&v["missing"]), 0, "{ctx}: {src} missing");
            assert_eq!(n(&v["passed"]), is("passed"), "{ctx}: {src} passed");
            assert_eq!(n(&v["timeout"]), is("timeout"), "{ctx}: {src} timeout");
            assert_eq!(n(&v["failed"]), is("failed"), "{ctx}: {src} failed");
            assert_eq!(
                n(&v["cancelled"]),
                is("cancelled"),
                "{ctx}: {src} cancelled"
            );
            assert_eq!(
                n(&v["final_passed"]),
                u64::from(validated),
                "{ctx}: {src} final_passed"
            );
        }
        assert_eq!(n(&e["failures"]), u64::from(!validated), "{ctx}: failures");
        assert_eq!(n(&r["trials"]), 1, "{ctx}: report trials");
        assert_eq!(
            r["executor_passed"].as_u64().unwrap_or(0),
            u64::from(passed),
            "{ctx}: report executor_passed"
        );
        assert_eq!(
            r["validated_success"].as_u64().unwrap_or(0),
            u64::from(validated),
            "{ctx}: report validated_success"
        );
        let rt = p.report["trials"]
            .as_array()
            .expect("report trials")
            .iter()
            .find(|x| x["arm"] == arm)
            .unwrap_or_else(|| panic!("{ctx}: report trial"));
        assert_eq!(
            rt["executor_status"]["value"], outcome,
            "{ctx}: report status"
        );
        assert_eq!(
            rt["completion"]["value"], validated,
            "{ctx}: report completion"
        );

        // final-accounting の cell は実行済みで、試行の tokens は trials.jsonl と同じ。
        let fc = fa_cells
            .iter()
            .find(|x| x["arm"] == arm)
            .unwrap_or_else(|| panic!("{ctx}: fa cell"));
        assert_eq!(fc["ran"], true, "{ctx}: fa ran");
        assert_eq!(fc["trial_tokens"], t["tokens"], "{ctx}: fa tokens");

        // check-out-consistency.py の独立な再計算も同じ。
        let cell = &p.consistency["cells"][format!("{arm}/{K4}/rep1")];
        assert_eq!(cell["ran"], true, "{ctx}: consistency ran");
        let counts = &p.consistency["counts"][arm];
        for k in ["n", "passed", "final_passed", "timeout"] {
            let vals: Vec<&Value> = counts[k].as_object().expect(k).values().collect();
            assert!(vals.len() >= 3, "{ctx}: {k} compared by {:?}", counts[k]);
            assert!(
                vals.iter().all(|v| *v == vals[0]),
                "{ctx}: {k} {:?}",
                counts[k]
            );
        }
        assert_eq!(counts["n"]["report.trials"], 1, "{ctx}: consistency n");
    }
    assert_eq!(p.consistency["consistent"], true);
    assert_eq!(p.consistency["mismatches"], json!([]));

    // B の記録（DB の写しから）と組み立てた試行の対応: 終了の段と消費。
    let b = trials.iter().find(|t| t["config"] == "B").expect("B");
    let stage = |name: &str| {
        p.trace["stages"]
            .as_array()
            .expect("stages")
            .iter()
            .find(|s| s["stage"] == name)
            .unwrap_or_else(|| panic!("trace stage {name}"))
            .clone()
    };
    let reported = !stage("report")["at"].is_null();
    assert_eq!(
        reported,
        b["outcome"] == "passed",
        "trace report vs B outcome"
    );
    let task_rows: Vec<&Value> = p.attr["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|r| r["source"] == "runs" && r["trial_id"] == B_TRIAL)
        .collect();
    let input: u64 = task_rows.iter().map(|r| n(&r["input_tokens"])).sum();
    assert_eq!(b["token_usage"]["input"], input, "B input tokens");
    let observed = b["observed"].as_array().expect("observed").len();
    assert_eq!(observed, task_rows.len(), "B observed runs");
    // CoS の chat run は試行の値に入れず、final-accounting の共有行に 1 度だけ出る。
    let cos_rows = p.attr["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|r| r["source"] == "chat_runs" && r["trial_id"] == B_TRIAL)
        .count() as u64;
    assert!(cos_rows > 0, "CoS rows");
    assert_eq!(
        n(&p.fa["shared_unattributable"]["cos"]["rows"]["n_rows"]),
        cos_rows,
        "fa CoS rows"
    );
    assert_eq!(p.fa["shared_unattributable"]["added_to_total"], false);
    let records = jsonl(&p.records);
    assert!(records.iter().any(|r| r["shared_overhead"] == true));
}

#[test]
fn ops_r1_pipeline_all_stages_agree_and_two_passes_are_byte_identical() {
    let i = Inputs::new(A_CUT_B_PASS);
    let p1 = pipeline(&i, "1");
    assert_agree(&i, &p1);
    let p2 = pipeline(&i, "2");
    assert_agree(&i, &p2);
    let (t1, t2) = (tree(&p1.root), tree(&p2.root));
    assert_eq!(
        t1.keys().collect::<Vec<_>>(),
        t2.keys().collect::<Vec<_>>(),
        "file sets differ"
    );
    // check-out-consistency.py の出力だけは照合した file の path を写す（pass dir が違う）。それ以外は bytes のまま比べる。
    let (r1, r2) = (s(&p1.root), s(&p2.root));
    for (k, v) in &t1 {
        let w = &t2[k];
        if k == "consistency.json" {
            let a = String::from_utf8_lossy(v).replace(&r1, "<pass>");
            let b = String::from_utf8_lossy(w).replace(&r2, "<pass>");
            assert_eq!(a, b, "{k} differs beyond the pass path");
        } else {
            assert!(v == w, "{k} differs between two passes from the same raw");
        }
    }
    for f in [
        "b-trace.json",
        "b-attribution.json",
        "out/trials.jsonl",
        "out/grades.jsonl",
        "out/summary.json",
        "out/export/table.json",
        "dl/records.jsonl",
        "human-ops-report/report.json",
        "final-accounting.json",
        "check.json",
    ] {
        assert!(t1.contains_key(f), "{f} was not produced");
    }
}

#[test]
fn ops_r1_pipeline_passing_a_and_cut_off_b_agree() {
    let i = Inputs::new(A_PASS_B_CUT);
    let p = pipeline(&i, "1");
    assert_agree(&i, &p);
}
