//! 未実行・作成失敗の試行がある campaign は complete=false・real_performance=false
//! （pilot-2: 3 試行中 2 試行が 422 で未実行なのに complete=true だった。ADR 2026-10-17-mlsys-pilot2-fixes D6）。
use std::{fs, path::Path};

use celeris_campaign::{
    aggregate::Outcome,
    checker,
    executor::{ExecResult, ExecStatus, Executor, MockExecutor, TrialRequest},
    export,
    manifest::{CampaignManifest, ConfigId},
    runner::{RunOptions, run},
};
use serde_json::Value;

/// A は完走し、B/C は task 作成が 422 で失敗する実行器（実測扱い: simulated=false）。
struct Pilot2;

impl Executor for Pilot2 {
    fn name(&self) -> &'static str {
        "pilot2"
    }

    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        if req.config.id == ConfigId::A {
            return Ok(ExecStatus::Done(ExecResult {
                outcome: Outcome::Passed,
                tokens: Some(celeris_campaign::manifest::TokenUsage::full(10, 5, 0, 0)),
                cost_micro_usd: Some(1),
                wall_secs: 1,
            }));
        }
        Err("POST /api/v1/tasks: HTTP 422 a local workspace cannot be combined with the remote repo".into())
    }
}

fn fx() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn setup(exec: &dyn Executor) -> (tempfile::TempDir, CampaignManifest, std::path::PathBuf) {
    let d = tempfile::tempdir().expect("dir");
    let mp = fx().join("pilot.json");
    let m = CampaignManifest::load(&mp).expect("pilot");
    let i = m.load_input(&mp, None).expect("input");
    let out = d.path().join("out");
    run(
        &m,
        &i,
        exec,
        &RunOptions {
            global_ledger: None,
            out: out.clone(),
            repo_src: Some(fx().join("repo")),
            materials_src: Some(fx().join("materials")),
            kb_src: Some(fx().join("kb")),
            ..Default::default()
        },
    )
    .expect("run");
    (d, m, out)
}

#[test]
fn unexecuted_trials_make_check_and_export_incomplete() {
    let (d, m, out) = setup(&Pilot2);
    let rep = checker::check(&m, &out);
    assert!(rep.ok, "{:?}", rep.violations);
    assert!(!rep.complete, "2 arms never executed");
    assert!(
        rep.incomplete_reasons
            .iter()
            .any(|r| r.contains("never executed") && r.starts_with('B')),
        "{:?}",
        rep.incomplete_reasons
    );
    let rc = rep.recomputed.expect("recomputed");
    assert_eq!(rc.totals[&ConfigId::A].unexecuted, 0);
    assert!(rc.totals[&ConfigId::B].unexecuted > 0);
    let dest = d.path().join("export");
    let t = export::export(&m, &out, &dest).expect("export");
    assert_eq!(t["complete"], false);
    assert_eq!(t["real_performance"], false, "{t}");
    assert_eq!(t["evidence_kind"], "real");
    assert!(dest.join("table.incomplete.json").exists());
    assert!(!dest.join("table.json").exists());
}

/// 全試行が実行された（mock）campaign では、未実行の理由は出ない。
#[test]
fn fully_executed_campaign_has_no_unexecuted_cells() {
    let (_d, m, out) = setup(&MockExecutor::seeded());
    let rep = checker::check(&m, &out);
    assert!(rep.ok);
    assert!(
        rep.incomplete_reasons
            .iter()
            .all(|r| !r.contains("never executed")),
        "{:?}",
        rep.incomplete_reasons
    );
    let v: Value = serde_json::from_str(&fs::read_to_string(out.join("summary.json")).expect("s"))
        .expect("json");
    assert_eq!(v["budget"]["executor_errors"], 0);
}
