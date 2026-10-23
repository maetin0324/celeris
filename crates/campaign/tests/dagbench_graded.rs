//! 独立 grader 付きの仕事を baseline と Celeris に同じ条件で流す（有料 LLM・実クラスタ job 0）。
//!
//! 固定入力（生データ・評価側が取得した scheduler 記録・評価テスト）から作った実成果物を、評価側の grader
//! （`celeris-campaign grade`: 実装の評価テスト実行・scheduler 記録との突き合わせ・raw からの数値再計算）が検査する。
//! 最初の試行は数値を捏造した成果物で、grader が不合格にする → Celeris は既存の WorkUnit check 不合格 →
//! 次の run（repair）、baseline は同じ node の再実行 → 2 回目で grader 合格 → 完遂。
//! 成功宣言・常に true の check では通らない（`success_claim_only_is_rejected_by_the_same_grader`）。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{fs, path::Path, sync::Arc};

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
};
use serde_json::Value;
use task_core::{Status, TaskStore};

use grading_fixture::{FABRICATED_REPORT, fixture, k3_grading, write};
#[path = "common/dagbench_grading.rs"]
mod grading_fixture;

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

fn both_arms_reject_then_repair(root: &Path, g: Grading) {
    let dir = root;
    // 負の対照: 捏造した成果物は同じ grader が不合格にする / 正しい成果物は合格にする。
    let probe = dir.join("probe");
    g.materialize(0, &probe).unwrap();
    let why = g.grade(&probe).unwrap_err();
    assert!(why.contains("\"recompute\""), "{why}");
    g.materialize(1, &probe).unwrap();
    g.grade(&probe).unwrap();

    let (braw, craw) = (dir.join("braw"), dir.join("craw"));
    let mut bg = g.clone();
    bg.stage_root = braw.join("artifacts");
    run_baseline_graded(&spec(Some(bg)), &braw);
    let mut cg = g.clone();
    cg.stage_root = craw.join("artifacts");
    let (cstatus, cstore) = run_celeris_graded(&spec(Some(cg.clone())), &craw);
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
    cg.grade(&cg.artifact_dir("c0")).unwrap();
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
