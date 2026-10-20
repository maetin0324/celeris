//! dagbench の反復実行入口（正式な test-harness 入口）。有料 LLM・実 job 0。
//!
//! 同じ引数（scenario / point / seed / 資源枠は scenarios.json / 故障 id / 出力先）で baseline と Celeris を同じ手順で流し、
//! raw dir から指標を再生成する。`--arm celeris` が CLI で使えない（task-api の router が dev-dependency）ための入口。
//!
//! ```text
//! DAGBENCH_ARM=celeris|baseline DAGBENCH_SCENARIO=k2-mini DAGBENCH_POINT=camp-1 [DAGBENCH_SEED=n] \
//! [DAGBENCH_FAULT=a1-restart-after-accept|a2-restart-after-wait|b-submit-response-lost|c-completion-duplicate] DAGBENCH_OUT=<dir> \
//!   cargo test -p celeris-campaign --test dagbench_harness -- --exact run_from_env --ignored --nocapture
//! ```
//! 故障条件は camp-1 の点だけ（制御 process は runner が起こす子で、この binary の `child_control`）。
//! 出力: `<out>/run.json` ほかの raw 一式と `<out>/metrics/control_measured_metrics.{json,tsv}`。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        baseline::{Baseline, BaselineConfig, Outcome, load_scenario},
        celeris_side::{CelerisSide, Paths, RunSpec, load_scenarios},
        ext::{ExtConfig, ExtSim, read_ledger},
        fault::{
            ControlCommand, FaultCase, FaultRunner, RunnerOutcome, RunnerStop, block_until_killed,
            read_obs,
        },
        metrics::{Metrics, RunHeader, append_control, append_sample, compute, write_tables},
        run::{BASELINE_SCOPE, CELERIS_SCOPE, run_baseline},
    },
};
use serde_json::{Value, json};
use task_core::TaskStore;

const CHILD_ENV: &str = "DAGBENCH_HARNESS_CHILD";

fn scenarios() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

/// 実行 1 回の入力。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Params {
    pub arm: String,
    pub scenario: String,
    pub point: String,
    pub seed: Option<u64>,
    pub fault: Option<String>,
    pub out: PathBuf,
}

impl Params {
    fn spec(&self) -> Result<RunSpec, String> {
        let file = load_scenarios(&scenarios())?;
        RunSpec::resolve(
            &file,
            &self.scenario,
            &self.point,
            self.seed.unwrap_or(file.seed),
        )
    }
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

/// 同じ DB file を開き直す router（制御 process の起動ごとに作り直す）。
fn router_at(dir: &Path) -> axum::Router {
    let opts = api_common::EnvOptions {
        token: Some(api_common::TOKEN.to_string()),
        ..Default::default()
    };
    let settings = api_common::settings(
        &dir.join("celeris.db"),
        &dir.join("api-workspaces"),
        &dir.join("workspace"),
        &dir.join("knowledge"),
        opts,
    );
    let (_tx, rx) = tokio::sync::watch::channel(None);
    task_api::router(task_api::ApiState::new(settings, rx).unwrap())
}

fn drive_celeris(
    spec: &RunSpec,
    dir: &Path,
    faults: Vec<celeris_campaign::dagbench::ext::FaultSpec>,
    hook: Option<celeris_campaign::dagbench::ext::StopHook>,
) -> Result<(), String> {
    let paths = Paths::under(&dir.join("celeris.db"), dir);
    let store: Arc<dyn TaskStore> =
        Arc::new(task_core::SqliteStore::open(&paths.db).map_err(|e| e.to_string())?);
    let transport = RouterTransport(router_at(dir));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let report = rt.block_on(async {
        let mut side = CelerisSide::with_faults(spec.clone(), paths, store, faults, hook)?;
        side.drive(&transport).await
    })?;
    match report.campaigns.iter().find(|c| c.status != "done") {
        Some(c) => Err(format!("campaign not done: {c:?}")),
        None => Ok(()),
    }
}

/// 制御 process（子）の入口。runner が `CHILD_ENV` に `Params` と `FaultCase` を入れて起こしたときだけ動く。
#[test]
fn child_control() {
    let Ok(cfg) = std::env::var(CHILD_ENV) else {
        return;
    };
    let cfg: Value = serde_json::from_str(&cfg).unwrap();
    let p: Params = serde_json::from_value(cfg["params"].clone()).unwrap();
    let case: FaultCase = serde_json::from_value(cfg["case"].clone()).unwrap();
    let s = p.spec().unwrap();
    match p.arm.as_str() {
        "baseline" => {
            let mut ext = ExtSim::open(
                &p.out.join("ext.jsonl"),
                ExtConfig {
                    poll_secs: s.poll_secs,
                    job_slots: s.scenario.resources.external_job_slots,
                    faults: vec![case.ext_fault()],
                },
            )
            .unwrap();
            ext.set_stop_hook(block_until_killed());
            let mut b = Baseline::open(
                load_scenario(&scenarios(), &p.scenario).unwrap(),
                BaselineConfig {
                    poll_secs: s.poll_secs,
                    ext_job_secs: s.ext_job_secs,
                },
                &p.out.join("state.json"),
            )
            .unwrap();
            b.set_progress_log(&p.out.join("worker.jsonl")).unwrap();
            assert_eq!(b.run(&mut ext).unwrap(), Outcome::Complete);
        }
        _ => drive_celeris(
            &s,
            &p.out,
            vec![case.ext_fault()],
            Some(block_until_killed()),
        )
        .unwrap_or_else(|e| panic!("drive failed: {e}")),
    }
}

/// 故障なしの通常 run（同じ process の中）。
fn run_normal(p: &Params, s: &RunSpec) -> Result<(), String> {
    match p.arm.as_str() {
        "baseline" => run_baseline(s, &p.out),
        "celeris" => {
            RunHeader::from_spec(s, "celeris", CELERIS_SCOPE)
                .write(&p.out)
                .map_err(|e| e.to_string())?;
            append_sample(&p.out, std::process::id(), 0)?;
            drive_celeris(s, &p.out, vec![], None)?;
            append_sample(&p.out, std::process::id(), 1)
        }
        other => Err(format!("unknown arm {other} (baseline|celeris)")),
    }
}

/// 故障あり: runner が制御 child を起こし、SIGKILL → 同じ永続 state で再起動、完走まで見届ける。
fn run_faulted(p: &Params, s: &RunSpec, fault: &str) -> Result<(), String> {
    if s.point.concurrent_campaigns != 1 {
        return Err("fault comparison is defined for the camp-1 point only".into());
    }
    let occurrences = (s
        .scenario
        .nodes
        .iter()
        .filter(|n| n.external.is_some())
        .count()) as u64;
    let case = FaultCase::from_scenarios(&scenarios(), fault, s.seed, occurrences)?;
    let file = load_scenarios(&scenarios())?;
    let scope = if p.arm == "baseline" {
        BASELINE_SCOPE
    } else {
        CELERIS_SCOPE
    };
    let cfg = json!({"params": p, "case": case});
    let out = FaultRunner {
        case: case.clone(),
        system: p.arm.clone(),
        cmd: ControlCommand {
            program: std::env::current_exe().map_err(|e| e.to_string())?,
            args: vec![
                "--exact".into(),
                "child_control".into(),
                "--nocapture".into(),
            ],
            env: vec![(CHILD_ENV.into(), cfg.to_string())],
        },
        ext_ledger: p.out.join("ext.jsonl"),
        obs_path: p.out.join("fault.jsonl"),
        log_dir: p.out.join("control-logs"),
        max_restarts: 3,
        wall_cap: Duration::from_secs(file.observation_caps.max_wall_secs),
        stop: RunnerStop::Never,
    }
    .run()?;
    if !matches!(out, RunnerOutcome::Completed { .. }) {
        return Err(format!("runner did not complete: {out:?}"));
    }
    RunHeader::from_spec(s, &p.arm, scope)
        .write(&p.out)
        .map_err(|e| e.to_string())?;
    // 観測 ledger の制御停止・再開を control.jsonl へ（時刻は trigger の論理時刻）。
    let obs = read_obs(&p.out.join("fault.jsonl")).map_err(|e| e.to_string())?;
    let trigger = obs
        .iter()
        .find(|e| e.event == "trigger_observed")
        .and_then(|e| e.detail["logical_secs"].as_u64())
        .unwrap_or(0);
    for e in &obs {
        match e.event.as_str() {
            "control_stopped" => {
                append_control(&p.out, "control_restart", trigger, Some(e.wall_ms), None)
                    .map_err(|e| e.to_string())?
            }
            "control_started" if e.detail["restart"] == true => {
                append_control(&p.out, "control_resumed", trigger, Some(e.wall_ms), None)
                    .map_err(|e| e.to_string())?
            }
            _ => {}
        }
    }
    Ok(())
}

/// 入口本体: 実行 → 指標再生成（raw だけから）。
pub fn run(p: &Params) -> Result<Metrics, String> {
    fs::create_dir_all(&p.out).map_err(|e| e.to_string())?;
    let s = p.spec()?;
    match &p.fault {
        None => run_normal(p, &s)?,
        Some(f) => run_faulted(p, &s, f)?,
    }
    let m = compute(&p.out)?;
    write_tables(&p.out.join("metrics"), &m).map_err(|e| e.to_string())?;
    Ok(m)
}

fn env(n: &str) -> Option<String> {
    std::env::var(n).ok().filter(|v| !v.is_empty())
}

/// 環境変数で指定する反復実行入口（`--ignored`）。
#[test]
#[ignore = "iterative entry: set DAGBENCH_* (see the module doc)"]
fn run_from_env() {
    let need = |n: &str| env(n).unwrap_or_else(|| panic!("{n} is required"));
    let p = Params {
        arm: need("DAGBENCH_ARM"),
        scenario: need("DAGBENCH_SCENARIO"),
        point: need("DAGBENCH_POINT"),
        seed: env("DAGBENCH_SEED").map(|v| v.parse().expect("DAGBENCH_SEED")),
        fault: env("DAGBENCH_FAULT"),
        out: PathBuf::from(need("DAGBENCH_OUT")),
    };
    let m = run(&p).unwrap_or_else(|e| panic!("{e}"));
    println!("{}", serde_json::to_string_pretty(&m).unwrap());
}

fn params(arm: &str, point: &str, fault: Option<&str>, out: &Path) -> Params {
    Params {
        arm: arm.into(),
        scenario: "k2-mini".into(),
        point: point.into(),
        seed: None,
        fault: fault.map(String::from),
        out: out.join(format!("{arm}-{}", fault.unwrap_or("normal"))),
    }
}

/// 同じ入口・同じ引数で両方式が流れ、同じ DAG の完了量・外部 job 数・二重起動なしの指標が出る。再実行で同じ表。
#[test]
fn both_arms_run_through_the_same_entry_and_regenerate() {
    let dir = tempfile::tempdir().unwrap();
    let mut ms = vec![];
    for arm in ["baseline", "celeris"] {
        let p = params(arm, "camp-1", None, dir.path());
        let m = run(&p).unwrap();
        assert_eq!(
            (m.dataset.as_str(), m.condition.as_str()),
            ("control_measured", "normal")
        );
        assert_eq!((m.llm_calls, m.paid_jobs), (0, 0));
        assert_eq!(m.duplicates.extra_physical_launches, 0);
        // 出力先の raw だけから表が再生成できて byte 単位で同じ。
        let again = p.out.join("again");
        write_tables(&again, &compute(&p.out).unwrap()).unwrap();
        for f in [
            "control_measured_metrics.json",
            "control_measured_metrics.tsv",
        ] {
            assert_eq!(
                fs::read(p.out.join("metrics").join(f)).unwrap(),
                fs::read(again.join(f)).unwrap()
            );
        }
        ms.push(m);
    }
    assert_eq!(ms[0].completed, ms[1].completed);
    assert_eq!(ms[0].scenario, ms[1].scenario);
}

/// 故障条件（制御 process の停止）も同じ入口の引数で与えられ、永続 state から完走する。
#[test]
fn fault_condition_is_a_parameter_of_the_same_entry() {
    let dir = tempfile::tempdir().unwrap();
    for (arm, fault) in [
        ("baseline", "a1-restart-after-accept"),
        ("celeris", "a1-restart-after-accept"),
        // 外部 job の ID・待機を永続化した後（wait 開始後）の制御 process 停止。job は外部 process で継続し、追跡して完遂する。
        ("baseline", "a2-restart-after-wait"),
        ("celeris", "a2-restart-after-wait"),
    ] {
        let p = params(arm, "camp-1", Some(fault), dir.path());
        let m = run(&p).unwrap();
        assert!(m.condition.starts_with("fault:"), "{}", m.condition);
        assert_eq!(m.completed.nodes, m.completed.nodes_expected, "{arm}");
        assert_eq!(m.duplicates.extra_physical_launches, 0, "{arm}");
        let ext = read_ledger(&p.out.join("ext.jsonl")).unwrap();
        assert!(ext.iter().any(|e| e.event == "fault_fired"), "{arm}");
    }
    // camp-1 以外の点では故障を拒否する。
    let p = params(
        "baseline",
        "center",
        Some("a1-restart-after-accept"),
        dir.path(),
    );
    assert!(run(&p).unwrap_err().contains("camp-1"));
}

/// 固定 scenario の stage 変換（位相レベルごとに 1 stage）は DAG に無い順序制約を足さない。
/// 検出器が空振りでないことを、レベルが揃わない DAG（a→c, b だけ独立で深い）で確かめる。
#[test]
fn stage_conversion_does_not_add_dependencies() {
    let file = load_scenarios(&scenarios()).unwrap();
    for sc in &file.scenarios {
        for pt in &file.fixed_points.points {
            let s = RunSpec::resolve(&file, &sc.id, &pt.id, file.seed).unwrap();
            assert_eq!(s.stage_extra_precedence(), vec![], "{} {}", sc.id, pt.id);
        }
    }
    let mut s = RunSpec::resolve(&file, "k2-mini", "camp-1", file.seed).unwrap();
    // review を verify にだけ依存させると review は level 3。level 4 以降の analyze は review の後ろに stage で
    // 並ぶが、DAG 上は review が analyze の祖先ではない（余計な順序）。
    for n in &mut s.scenario.nodes {
        if n.id == "review" {
            n.deps = vec!["verify".into()];
        }
    }
    let extra = s.stage_extra_precedence();
    assert!(
        extra.contains(&("review".into(), "analyze".into())),
        "{extra:?}"
    );
}
