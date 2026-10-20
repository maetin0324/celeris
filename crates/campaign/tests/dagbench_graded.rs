//! 独立 grader 付きの仕事を baseline と Celeris に同じ条件で流す（有料 LLM・実クラスタ job 0）。
//!
//! 固定入力（生データ・評価側が取得した scheduler 記録・評価テスト）から作った実成果物を、評価側の grader
//! （`celeris-campaign grade`: 実装の評価テスト実行・scheduler 記録との突き合わせ・raw からの数値再計算）が検査する。
//! 最初の試行は数値を捏造した成果物で、grader が不合格にする → Celeris は既存の WorkUnit check 不合格 →
//! 次の run（repair）、baseline は同じ node の再実行 → 2 回目で grader 合格 → 完遂。
//! 成功宣言・常に true の check では通らない（`success_claim_only_is_rejected_by_the_same_grader`）。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{fs, path::Path, process::Command, sync::Arc};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        baseline::{Baseline, BaselineConfig, Outcome},
        celeris_side::{CelerisSide, Paths, RunSpec, load_scenarios, read_worker_ledger},
        ext::{ExtConfig, ExtSim, read_ledger},
        graded::Grading,
        metrics::{RunHeader, append_campaign_submitted, compute},
        run::{BASELINE_SCOPE, CELERIS_SCOPE, merged_scenario},
    },
    grader::GradeCase,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::{Status, TaskStore};

const PRISTINE_TEST: &str =
    "use demo::add;\n\n#[test]\nfn adds_small_numbers() {\n    assert_eq!(add(1, 2), 3);\n}\n";
const BASE_CSV: &str = "cond,xfer,bw\nbase,4k,50\nbase,4k,60\nbase,1m,999\n";
const GOOD_REPORT: &str = "# 結果\n\nmetric write_bw_mean = 110.0\nmetric write_bw_n = 3\nmetric base_bw_median = 55\nmetric speedup = 2.0\n";
/// 捏造: 生データから再計算できない speedup。
const FABRICATED_REPORT: &str = "# 結果\n\nmetric write_bw_mean = 110.0\nmetric write_bw_n = 3\nmetric base_bw_median = 55\nmetric speedup = 9.0\n";

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn write(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

/// 固定入力から作る成果物・case・評価側の scheduler 記録。
fn fixture(root: &Path, overrides: Vec<(String, String)>) -> Grading {
    let (case_dir, art, obs) = (
        root.join("case"),
        root.join("good"),
        root.join("observed.json"),
    );
    write(&case_dir.join("pristine/tests/add.rs"), PRISTINE_TEST);
    write(
        &art.join("repo/src/lib.rs"),
        "pub fn add(a: u64, b: u64) -> u64 { a + b }\n",
    );
    write(&art.join("repo/tests/add.rs"), PRISTINE_TEST);
    let (mut raw, mut seen) = (vec![], vec![]);
    for (i, bw) in [100.0, 110.0, 120.0].iter().enumerate() {
        let rel = format!("raw/r{}/ior_result.json", i + 1);
        let body = json!({"tests":[{"Results":[{"access":"write","bwMiB": bw}]}]}).to_string();
        write(&art.join(&rel), &body);
        let job = format!("{}.sirius", 42588 + i);
        write(
            &art.join(format!("evidence/{job}.qstat.txt")),
            &format!("Job Id: {job}\n    job_state = F\n    Exit_status = 0\n"),
        );
        write(&art.join(format!("evidence/{job}.log")), "IOR finished\n");
        let rawref = json!([{"path": rel, "sha256": sha(body.as_bytes())}]);
        seen.push(
            json!({"job_id": job, "exit_status": 0, "conditions": {"nodes": 3}, "raw": rawref}),
        );
        raw.push(json!({"job_id": job, "scheduler_record": format!("evidence/{job}.qstat.txt"),
            "log": format!("evidence/{job}.log"), "raw": rawref, "conditions": {"nodes": 3}, "evidence_kind": "real"}));
    }
    write(
        &art.join("evidence/jobs.json"),
        &json!({"jobs": raw}).to_string(),
    );
    write(
        &obs,
        &json!({"schema": "celeris.observed-jobs/1", "jobs": seen}).to_string(),
    );
    write(&art.join("raw/baseline.csv"), BASE_CSV);
    write(&art.join("report.md"), GOOD_REPORT);
    write(
        &art.join("review/findings.json"),
        &json!({"findings": []}).to_string(),
    );
    let files = [
        "raw/r1/ior_result.json",
        "raw/r2/ior_result.json",
        "raw/r3/ior_result.json",
    ];
    let json_src = json!({"json": {"files": files, "pointer": "/tests/0/Results/0/bwMiB"}});
    let case: GradeCase = serde_json::from_value(json!({
        "schema": "celeris.grade-case/1", "id": "dagbench-analyze", "split": "dev",
        "implementation": {
            "pristine_dir": "pristine", "repo": "repo",
            "tests": [{"path": "tests/add.rs", "sha256": sha(PRISTINE_TEST.as_bytes())}],
            "command": ["sh", "-c", "test -f tests/add.rs && echo 'test adds_small_numbers ... ok'"],
            "must_pass": ["adds_small_numbers"],
        },
        "experiment": {"min_jobs": 3, "conditions": {"nodes": 3}},
        "recompute": {
            "report": "report.md",
            "pinned_raw": {"raw/baseline.csv": sha(BASE_CSV.as_bytes())},
            "metrics": [
                {"name": "write_bw_mean", "agg": "mean", "tolerance_rel": 0.01, "source": json_src},
                {"name": "write_bw_n", "agg": "count", "tolerance_rel": 0.0, "source": json_src},
                {"name": "base_bw_median", "agg": "median", "tolerance_rel": 0.01,
                 "source": {"csv": {"files": ["raw/baseline.csv"], "column": "bw", "filter": {"xfer": "4k"}}}},
                {"name": "speedup", "tolerance_rel": 0.01,
                 "source": {"ratio": {"numerator": "write_bw_mean", "denominator": "base_bw_median"}}},
            ],
        },
            }))
    .unwrap();
    case.validate().unwrap();
    write(
        &case_dir.join("case.json"),
        &serde_json::to_string(&case).unwrap(),
    );
    Grading {
        node: "analyze".into(),
        grader_bin: std::env::var("CARGO_BIN_EXE_celeris-campaign")
            .expect("binary path")
            .into(),
        case: case_dir.join("case.json"),
        observed: Some(obs),
        work: root.join("grade-work"),
        stage_root: root.join("stage"),
        good_artifact: art,
        first_attempt_overrides: overrides,
        max_attempts: 3,
    }
}

fn spec(g: Option<Grading>) -> RunSpec {
    let file = load_scenarios(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json"),
    )
    .unwrap();
    let mut s = RunSpec::resolve(&file, "k2-mini", "camp-1", file.seed).unwrap();
    s.grading = g.map(Arc::new);
    s
}

struct RouterTransport(axum::Router);

impl Transport for RouterTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let app = self.0.clone();
        let req = match (method, body) {
            ("POST", Some(b)) => api_common::post_admin(path, b),
            ("GET", None) => api_common::get_admin(path),
            _ => return Err(format!("unsupported {method} {path}")),
        };
        std::thread::scope(|s| {
            s.spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let r = rt.block_on(api_common::send(&app, req));
                Ok(HttpResponse {
                    status: r.status.as_u16(),
                    body: serde_json::from_slice(&r.body).unwrap_or(Value::Null),
                })
            })
            .join()
            .map_err(|_| "transport thread panicked".to_string())?
        })
    }
}

fn run_baseline_graded(s: &RunSpec, raw: &Path) {
    let g = s.grading.clone().unwrap();
    fs::create_dir_all(raw).unwrap();
    RunHeader::from_spec(s, "baseline", BASELINE_SCOPE)
        .write(raw)
        .unwrap();
    append_campaign_submitted(raw, "c0", 0).unwrap();
    let mut ext = ExtSim::open(
        &raw.join("ext.jsonl"),
        ExtConfig {
            poll_secs: s.poll_secs,
            job_slots: s.scenario.resources.external_job_slots,
            faults: vec![],
        },
    )
    .unwrap();
    let mut b = Baseline::open(
        merged_scenario(s),
        BaselineConfig {
            poll_secs: s.poll_secs,
            ext_job_secs: s.ext_job_secs,
        },
        &raw.join("state.json"),
    )
    .unwrap();
    b.set_worker_log(&raw.join("worker.jsonl")).unwrap();
    let max = g.max_attempts;
    let v = g.clone();
    b.set_verifier(
        Box::new(move |id, attempt| {
            if id.rsplit('/').next() != Some(v.node.as_str()) {
                return Ok(());
            }
            let dir = v.artifact_dir("c0");
            v.materialize(attempt, &dir).map_err(|e| e.to_string())?;
            v.grade(&dir)
        }),
        max,
    );
    assert_eq!(b.run(&mut ext).unwrap(), Outcome::Complete);
}

fn run_celeris_graded(s: &RunSpec, raw: &Path) -> (String, Status) {
    let env = api_common::admin_env();
    fs::create_dir_all(raw).unwrap();
    RunHeader::from_spec(s, "celeris", CELERIS_SCOPE)
        .write(raw)
        .unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open(&env.db_path).unwrap());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let report = rt.block_on(async {
        let mut side =
            CelerisSide::new(s.clone(), Paths::under(&env.db_path, raw), store.clone()).unwrap();
        side.drive(&RouterTransport(env.router())).await.unwrap()
    });
    let c = &report.campaigns[0];
    let st = store
        .get(c.task_id.parse().unwrap())
        .unwrap()
        .unwrap()
        .status;
    (c.status.clone(), st)
}

#[test]
fn fabricated_first_attempt_is_rejected_then_repaired_on_both_arms() {
    let dir = tempfile::tempdir().unwrap();
    let g = fixture(
        dir.path(),
        vec![("report.md".into(), FABRICATED_REPORT.into())],
    );
    both_arms_reject_then_repair(dir.path(), g);
}

/// 既存の実 case（K3: Locusta A/B 解析）の固定 grade-case・固定の実 raw（Sirius の IOR 結果 tgz、sha256 固定）から
/// 作った成果物を、同じ入口で両系に流す。最初の試行は報告の数値を 6% ずらした捏造で、実 grader（raw からの再計算）が
/// 不合格にする。原票 tgz が無い環境では実行不能として飛ばす。
#[test]
fn real_k3_case_is_graded_independently_on_both_arms() {
    let dir = tempfile::tempdir().unwrap();
    let Some(g) = k3_grading(dir.path()) else {
        eprintln!("SKIPPED(実行不能): K3 の原票 tgz が無い");
        return;
    };
    both_arms_reject_then_repair(dir.path(), g);
}

fn k3_grading(root: &Path) -> Option<Grading> {
    use celeris_campaign::grader::case::{Agg, Source};
    let tgz = std::path::PathBuf::from(std::env::var("CAMPAIGN_K3_TGZ").unwrap_or_else(|_| {
        "/local/celeris/data/workspaces/01M45ACZQWTW9BP8WVY8KAW9TC/artifacts/locusta-latency/raw/main-43223.tgz".into()
    }));
    if !tgz.is_file() {
        return None;
    }
    assert_eq!(
        sha(&fs::read(&tgz).unwrap()),
        "bdb155d4c1690ae5743621d129c270fcc53c919751d62c56d0c4395b74d2e90f",
        "原票の hash が case.json の固定とずれた"
    );
    let case = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/grade-cases/dev/k3-locusta-ab-analysis/k3-locusta-ab-analysis.json");
    let g = GradeCase::load(&case).unwrap();
    let good = root.join("good");
    fs::create_dir_all(good.join("inputs")).unwrap();
    assert!(
        Command::new("tar")
            .arg("-xzf")
            .arg(&tgz)
            .arg("-C")
            .arg(good.join("inputs"))
            .status()
            .unwrap()
            .success()
    );
    // 報告は raw から素朴に（平均・件数だけで）計算する。捏造は最初の指標だけ 6% ずらす。
    let (mut ok, mut bad) = (String::new(), String::new());
    for (i, m) in g.recompute.as_ref().unwrap().metrics.iter().enumerate() {
        let Source::Json { files, pointer } = &m.source else {
            panic!("K3 は json 指標だけ")
        };
        let xs: Vec<f64> = files
            .iter()
            .map(|f| {
                let v: Value = serde_json::from_slice(&fs::read(good.join(f)).unwrap()).unwrap();
                v.pointer(pointer).and_then(Value::as_f64).unwrap()
            })
            .collect();
        let x = match m.agg {
            Agg::Mean => xs.iter().sum::<f64>() / xs.len() as f64,
            Agg::Count => xs.len() as f64,
            a => panic!("未対応 {a:?}"),
        };
        ok.push_str(&format!("metric {} = {x}\n", m.name));
        let y = if i == 0 { x * 1.06 } else { x };
        bad.push_str(&format!("metric {} = {y}\n", m.name));
    }
    write(&good.join("report.md"), &ok);
    Some(Grading {
        node: "analyze".into(),
        grader_bin: std::env::var("CARGO_BIN_EXE_celeris-campaign")
            .expect("binary path")
            .into(),
        case,
        observed: None,
        work: root.join("grade-work"),
        stage_root: root.join("stage"),
        good_artifact: good,
        first_attempt_overrides: vec![("report.md".into(), bad)],
        max_attempts: 3,
    })
}

fn both_arms_reject_then_repair(root: &Path, g: Grading) {
    let dir = root;
    // 負の対照: 捏造した成果物は同じ grader が不合格にする / 正しい成果物は合格にする。
    let probe = dir.join("probe");
    g.materialize(0, &probe).unwrap();
    let why = g.grade(&probe).unwrap_err();
    assert!(why.contains("\"recompute\""), "{why}");
    g.materialize(1, &probe).unwrap();
    g.grade(&probe).unwrap();

    let s = spec(Some(g.clone()));
    let (braw, craw) = (dir.join("braw"), dir.join("craw"));
    run_baseline_graded(&s, &braw);
    let (cstatus, cstore) = run_celeris_graded(&s, &craw);
    assert_eq!((cstatus.as_str(), cstore), ("done", Status::Done));

    for raw in [&braw, &craw] {
        let m = compute(raw).unwrap();
        assert_eq!(m.completed.nodes, m.completed.nodes_expected, "{raw:?}");
        // 検証失敗の再作業が数えられる（analyze を 2 回始めた）。
        assert_eq!(m.lost_progress.rerun_node_starts, 1, "{raw:?}: {m:?}");
        assert_eq!(m.duplicates.extra_physical_launches, 0);
        assert_eq!((m.llm_calls, m.paid_jobs), (0, 0));
    }
    // 完遂した成果物は、評価側が独立に grader へ通して合格する（worker の自己申告ではない）。
    g.grade(&g.artifact_dir("c0")).unwrap();
    // Celeris は 2 回書き（最初は不合格 → repair）、baseline も同じ列の成果物を同じ grader で通した。
    let w = read_worker_ledger(&craw.join("worker.jsonl")).unwrap();
    let attempts: Vec<u64> = w
        .iter()
        .filter(|e| e.event == "artifact_written")
        .map(|e| e.detail.as_ref().unwrap()["attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(attempts, vec![0, 1]);
    let bw = read_worker_ledger(&braw.join("worker.jsonl")).unwrap();
    assert_eq!(bw.iter().filter(|e| e.event == "verify_failed").count(), 1);
    // 外部 job の起動は両系とも論理 job と同数（重複なし）。
    for raw in [&braw, &craw] {
        let ext = read_ledger(&raw.join("ext.jsonl")).unwrap();
        assert_eq!(
            ext.iter().filter(|e| e.event == "physical_launch").count(),
            2
        );
    }
}

#[test]
fn success_claim_only_is_rejected_by_the_same_grader() {
    let dir = tempfile::tempdir().unwrap();
    let g = fixture(dir.path(), vec![]);
    let claim = dir.path().join("claim");
    write(&claim.join("report.md"), "# 完了\n全て成功\n");
    write(
        &claim.join("result.json"),
        r#"{"summary":"all passed","outcome":"passed"}"#,
    );
    let why = g.grade(&claim).unwrap_err();
    assert!(why.contains("\"final_pass\": false"), "{why}");
}

/// 上限まで合格しない仕事は完遂にならない（baseline は Err、無限に再実行しない）。
#[test]
fn baseline_stops_when_the_attempt_limit_is_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let mut g = fixture(
        dir.path(),
        vec![("report.md".into(), FABRICATED_REPORT.into())],
    );
    g.max_attempts = 1;
    let s = spec(Some(g));
    let raw = dir.path().join("braw");
    let r = std::panic::catch_unwind(|| run_baseline_graded(&s, &raw));
    assert!(r.is_err(), "must not complete");
}
