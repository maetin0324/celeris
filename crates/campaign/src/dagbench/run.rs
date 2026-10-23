//! `dagbench run` / `dagbench metrics` の本体（main.rs は薄く呼ぶだけ）。
//!
//! `run` は 1 点（scenario × point × seed × 系）を測定専用の出力 dir（raw dir）に流す。有料 LLM・実 job は 0。
//! 論理時計で進み、sleep しない。CPU/RSS は `/proc` を数回読むだけ（負荷は掛けない）。
//! baseline は同じ process の中で動かす（制御 process = driver process 全体の標本。ext-sim の書込みも含む上限値）。
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use super::{
    baseline::{Baseline, BaselineConfig, ExternalSpec, NodeSpec, Outcome, Resources, Scenario},
    celeris_side::{RunSpec, load_scenarios},
    ext::{ExtConfig, ExtSim},
    metrics::{
        RunHeader, append_campaign_submitted, append_control, append_sample, compute, write_tables,
    },
};

pub const BASELINE_SCOPE: &str = "driver_process_inclusive(baseline+ext-sim in one process)";
pub const CELERIS_SCOPE: &str = "driver_process_inclusive(dispatcher+ext-sim in one process)";

/// N 個の campaign を 1 つの DAG に並べる（node は `c{i}/<node>`、根の release_at が到着時刻）。
pub fn merged_scenario(spec: &RunSpec) -> Scenario {
    let mut nodes = Vec::new();
    for (i, c) in spec.campaign_ids().iter().enumerate() {
        for n in &spec.scenario.nodes {
            nodes.push(NodeSpec {
                id: format!("{c}/{}", n.id),
                kind: n.kind.clone(),
                deps: n.deps.iter().map(|d| format!("{c}/{d}")).collect(),
                work_secs: n.work_secs,
                release_at: if n.deps.is_empty() {
                    spec.arrival(i)
                } else {
                    0
                },
                external: n.external.as_ref().map(|e| ExternalSpec {
                    logical_job_id: RunSpec::logical_job(c, e),
                    post_wait_work_secs: e.post_wait_work_secs,
                }),
            });
        }
    }
    Scenario {
        id: spec.scenario.id.clone(),
        resources: Resources {
            worker_slots: spec.scenario.resources.worker_slots,
            external_job_slots: spec.scenario.resources.external_job_slots,
        },
        nodes,
    }
}

/// baseline を 1 点流して raw dir を書く。
pub fn run_baseline(spec: &RunSpec, raw: &Path) -> Result<(), String> {
    let t = Instant::now();
    let wall = || t.elapsed().as_millis() as u64;
    RunHeader::from_spec(spec, "baseline", BASELINE_SCOPE)
        .write(raw)
        .map_err(|e| e.to_string())?;
    for (i, c) in spec.campaign_ids().iter().enumerate() {
        append_campaign_submitted(raw, c, spec.arrival(i)).map_err(|e| e.to_string())?;
    }
    let mut ext = ExtSim::open(
        &raw.join("ext.jsonl"),
        ExtConfig {
            poll_secs: spec.poll_secs,
            job_slots: spec.scenario.resources.external_job_slots,
            faults: vec![],
        },
    )
    .map_err(|e| e.to_string())?;
    let mut b = Baseline::open(
        merged_scenario(spec),
        BaselineConfig {
            poll_secs: spec.poll_secs,
            ext_job_secs: spec.ext_job_secs,
        },
        &raw.join("state.json"),
    )
    .map_err(|e| format!("{e:?}"))?;
    b.set_worker_log(&raw.join("worker.jsonl"))
        .map_err(|e| e.to_string())?;
    b.set_logical_cap(spec.caps.max_logical_secs);
    if let Some(g) = &spec.grading {
        g.attach_baseline(&mut b);
    }
    let pid = std::process::id();
    append_sample(raw, pid, wall())?;
    let r = b.run(&mut ext).map_err(|e| format!("{e:?}"))?;
    append_sample(raw, pid, wall())?;
    if r != Outcome::Complete {
        return Err("baseline did not complete".into());
    }
    let _ = append_control; // 障害注入 runner（faults WU）が control.jsonl を書く。
    Ok(())
}

fn flag(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

fn default_scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

/// `celeris-campaign dagbench <run|metrics> …`（args[0] は "dagbench"）。
pub fn cli(args: &[String]) -> Result<String, String> {
    let sub = args.get(1).map(String::as_str).unwrap_or("");
    let text = |n: &str| flag(args, n).map(|p| p.to_string_lossy().into_owned());
    let out = flag(args, "--out").ok_or("--out <dir> is required")?;
    match sub {
        "run" => {
            let file = load_scenarios(&flag(args, "--scenarios").unwrap_or_else(default_scenarios))?;
            let seed = match text("--seed") {
                Some(s) => s.parse::<u64>().map_err(|e| format!("--seed: {e}"))?,
                None => file.seed,
            };
            let scenario = text("--scenario").ok_or("--scenario <id> is required")?;
            let point = text("--point").ok_or("--point <p> is required")?;
            let spec = RunSpec::resolve(&file, &scenario, &point, seed)?;
            match text("--arm").as_deref() {
                Some("baseline") => {
                    run_baseline(&spec, &out)?;
                    Ok(format!("wrote raw {} ({})", out.display(), spec.measurement_id().replace("-celeris", "-baseline")))
                }
                Some("celeris") => Err(
                    "--arm celeris needs an in-process task-api router (a dev-dependency); \
                     drive it with CelerisSide::drive and metrics::RunHeader (see tests/dagbench_metrics.rs)"
                        .into(),
                ),
                _ => Err("--arm baseline|celeris is required".into()),
            }
        }
        "metrics" => {
            let raw = flag(args, "--raw").ok_or("--raw <dir> is required")?;
            let m = compute(&raw)?;
            write_tables(&out, &m).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&m).map_err(|e| e.to_string())
        }
        "process-metrics" => {
            let raw = flag(args, "--raw").ok_or("--raw <dir> is required")?;
            let s = super::process::compute_process(&raw)?;
            serde_json::to_string_pretty(&s).map_err(|e| e.to_string())
        }
        _ => Err("usage: celeris-campaign dagbench run --scenario <id> --point <p> --arm baseline|celeris --out <dir> | dagbench metrics --raw <dir> --out <dir> | dagbench process-metrics --raw <dir> --out <dir>".into()),
    }
}
