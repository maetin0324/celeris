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
        graded::Grading,
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
    #[serde(default)]
    pub grading: Option<Grading>,
}

impl Params {
    fn spec(&self) -> Result<RunSpec, String> {
        let file = load_scenarios(&scenarios())?;
        let mut spec = RunSpec::resolve(
            &file,
            &self.scenario,
            &self.point,
            self.seed.unwrap_or(file.seed),
        )?;
        if let Some(mut g) = self.grading.clone() {
            if spec.point.concurrent_campaigns != 1
                || g.max_attempts == 0
                || !spec
                    .scenario
                    .nodes
                    .iter()
                    .any(|n| n.id == g.node && n.external.is_none())
            {
                return Err(
                    "grading requires camp-1, a local graded node, and max_attempts > 0".into(),
                );
            }
            g.pin()?;
            g.stage_root = self.out.join("artifacts");
            g.work = self.out.join("grade-work");
            spec.grading = Some(Arc::new(g));
        }
        Ok(spec)
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
    RunHeader::from_spec(
        &s,
        &p.arm,
        if p.arm == "baseline" {
            BASELINE_SCOPE
        } else {
            CELERIS_SCOPE
        },
    )
    .write(&p.out)
    .unwrap();
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
            b.set_logical_cap(s.caps.max_logical_secs);
            if let Some(g) = &s.grading {
                g.attach_baseline(&mut b);
            }
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
    if p.out.join("run.json").exists() {
        return Err("output already contains a run; choose a fresh directory".into());
    }
    fs::create_dir_all(&p.out).map_err(|e| e.to_string())?;
    let s = p.spec()?;
    RunHeader::from_spec(
        &s,
        &p.arm,
        if p.arm == "baseline" {
            BASELINE_SCOPE
        } else {
            CELERIS_SCOPE
        },
    )
    .write(&p.out)
    .map_err(|e| e.to_string())?;
    let outcome = match &p.fault {
        None => run_normal(p, &s),
        Some(f) => run_faulted(p, &s, f),
    };
    let metrics = compute(&p.out);
    if let Ok(m) = &metrics {
        write_tables(&p.out.join("metrics"), m).map_err(|e| e.to_string())?;
    }
    outcome?;
    metrics
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
        grading: env("DAGBENCH_GRADING").map(|path| {
            serde_json::from_slice(&fs::read(path).expect("grading config")).expect("Grading JSON")
        }),
    };
    let m = run(&p).unwrap_or_else(|e| panic!("{e}"));
    println!("{}", serde_json::to_string_pretty(&m).unwrap());
}

fn params(arm: &str, point: &str, fault: Option<&str>, out: &Path) -> Params {
    Params {
        arm: arm.into(),
        scenario: "k2-mini".into(),
        point: point.into(),
        grading: None,
        seed: None,
        fault: fault.map(String::from),
        out: out.join(format!("{arm}-{point}-{}", fault.unwrap_or("normal"))),
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

#[path = "common/dagbench_grading.rs"]
mod grading_fixture;

#[test]
fn graded_limits_and_failed_costs_use_the_formal_entry() {
    use grading_fixture::{FABRICATED_REPORT, fixture};
    let root = tempfile::tempdir().unwrap();
    for (limit, all_fail) in [(1, false), (2, false), (3, true)] {
        let inputs = root.path().join(format!("input-{limit}"));
        let mut g = fixture(
            &inputs,
            vec![("report.md".into(), FABRICATED_REPORT.into())],
        );
        g.max_attempts = limit;
        if all_fail {
            fs::write(g.good_artifact.join("report.md"), FABRICATED_REPORT).unwrap();
        }
        for arm in ["baseline", "celeris"] {
            let mut p = params(
                arm,
                "camp-1",
                None,
                &root.path().join(format!("limit-{limit}")),
            );
            p.grading = Some(g.clone());
            let result = run(&p);
            assert_eq!(result.is_ok(), limit == 2, "{arm} {limit}: {result:?}");
            let m = compute(&p.out).unwrap();
            assert_eq!(m.completed.campaigns, usize::from(limit == 2), "{m:?}");
            let attempts = p.out.join("artifacts/c0/analyze.attempts");
            assert_eq!(fs::read_dir(&attempts).unwrap().count(), limit as usize);
            for i in 0..limit {
                let v: Value = serde_json::from_slice(
                    &fs::read(attempts.join(i.to_string()).join("grade.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(v["passed"], limit == 2 && i == 1);
            }
            let w = celeris_campaign::dagbench::celeris_side::read_worker_ledger(
                &p.out.join("worker.jsonl"),
            )
            .unwrap();
            assert_eq!(
                w.iter()
                    .filter(|e| e.node == "analyze" && e.event == "execution_finished")
                    .count(),
                limit as usize
            );
            assert_eq!(
                w.iter()
                    .filter(|e| e.node == "analyze" && e.event == "validated_completed")
                    .count(),
                usize::from(limit == 2)
            );
            assert_eq!(m.lost_progress.rerun_node_starts, (limit - 1) as usize);
            let node_work = p
                .spec()
                .unwrap()
                .scenario
                .nodes
                .iter()
                .find(|n| n.id == "analyze")
                .unwrap()
                .work_secs;
            assert!(m.execution_busy_secs >= u64::from(limit) * node_work);
            if limit != 2 {
                // A legacy worker finish must never manufacture validated completion.
                let mut premature = w.last().unwrap().clone();
                premature.seq += 1;
                premature.node = "analyze".into();
                premature.event = "node_finished".into();
                celeris_campaign::dagbench::metrics::append_jsonl(
                    &p.out.join("worker.jsonl"),
                    &premature,
                )
                .unwrap();
                let again = compute(&p.out).unwrap();
                assert_eq!(again.completed, m.completed);
                assert_eq!(again.makespan, m.makespan);
                assert_eq!(again.ready_wait, m.ready_wait);
            }
        }
    }
}

/// Persistent real-input repetitions; never silently skip a missing K3 archive.
#[test]
#[ignore = "set DAGBENCH_SUITE_OUT to a fresh persistent output directory"]
fn real_k3_repetitions_from_formal_entry() {
    let root = PathBuf::from(std::env::var("DAGBENCH_SUITE_OUT").expect("DAGBENCH_SUITE_OUT"));
    assert!(!root.exists(), "fresh output directory required");
    fs::create_dir_all(&root).unwrap();
    let mut g = grading_fixture::k3_grading(&root.join("inputs")).expect("missing CAMPAIGN_K3_TGZ");
    // Cargo may rebuild the shared target binary while a repetition is in flight.
    // Freeze the exact evaluator used by every child and every arm.
    let frozen_grader = root.join("grader");
    fs::copy(&g.grader_bin, &frozen_grader).unwrap();
    g.grader_bin = frozen_grader;
    fs::write(
        root.join("grading.json"),
        serde_json::to_vec_pretty(&g).unwrap(),
    )
    .unwrap();
    for seed in [17, 23] {
        for fault in [
            None,
            Some("a1-restart-after-accept"),
            Some("b-submit-response-lost"),
            Some("c-completion-duplicate"),
        ] {
            for arm in ["baseline", "celeris"] {
                let mut p = params(arm, "camp-1", fault, &root.join(seed.to_string()));
                p.seed = Some(seed);
                p.grading = Some(g.clone());
                let m = run(&p).unwrap();
                assert_eq!(m.completed.nodes, m.completed.nodes_expected);
                assert!(m.lost_progress.rerun_node_starts >= 1);
                assert_eq!(
                    fs::read_dir(p.out.join("artifacts/c0/analyze.attempts"))
                        .unwrap()
                        .count(),
                    2
                );
                assert_eq!(
                    serde_json::to_value(&m).unwrap(),
                    serde_json::to_value(compute(&p.out).unwrap()).unwrap()
                );
            }
        }
    }
}

const GRADED_RESTART_ENV: &str = "DAGBENCH_GRADED_RESTART";

#[test]
fn graded_restart_child() {
    let Ok(config) = std::env::var(GRADED_RESTART_ENV) else {
        return;
    };
    let (p, stop): (Params, bool) = serde_json::from_str(&config).unwrap();
    fs::create_dir_all(&p.out).unwrap();
    let mut s = p.spec().unwrap();
    let budget = std::env::var("DAGBENCH_LOGICAL_CAP")
        .ok()
        .map(|v| v.parse::<u64>().unwrap());
    if let Some(cap) = budget {
        s.caps.max_logical_secs = cap;
    }
    if stop {
        RunHeader::from_spec(&s, &p.arm, "restart-test")
            .write(&p.out)
            .unwrap();
    }
    let outcome = if p.arm == "baseline" {
        let mut ext = ExtSim::open(
            &p.out.join("ext.jsonl"),
            ExtConfig {
                poll_secs: s.poll_secs,
                job_slots: s.scenario.resources.external_job_slots,
                faults: vec![],
            },
        )
        .unwrap();
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
        b.set_logical_cap(s.caps.max_logical_secs);
        s.grading.as_ref().unwrap().attach_baseline(&mut b);
        if stop {
            b.stop_after_verify_failure();
        }
        b.run(&mut ext)
            .map(|o| o == Outcome::Complete)
            .map_err(|e| format!("{e:?}"))
    } else {
        let paths = Paths::under(&p.out.join("celeris.db"), &p.out);
        let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open(&paths.db).unwrap());
        let transport = RouterTransport(router_at(&p.out));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut side = CelerisSide::new(s.clone(), paths, store).unwrap();
            if stop {
                side.stop_after_verify_failure();
            }
            side.drive(&transport)
                .await
                .map(|r| r.campaigns.iter().all(|c| c.status == "done"))
        })
    };
    if !stop && budget.is_none() && s.grading.as_ref().unwrap().max_attempts > 1 {
        assert_eq!(outcome, Ok(true));
    } else {
        assert_ne!(outcome, Ok(true));
    }
}

#[test]
fn failed_attempt_survives_controller_process_restart_on_both_arms() {
    use grading_fixture::{FABRICATED_REPORT, fixture};
    let root = tempfile::tempdir().unwrap();
    for limit in [1, 2] {
        let mut g = fixture(
            &root.path().join(format!("input-{limit}")),
            vec![("report.md".into(), FABRICATED_REPORT.into())],
        );
        g.max_attempts = limit;
        for arm in ["baseline", "celeris"] {
            let mut p = params(arm, "camp-1", None, &root.path().join(limit.to_string()));
            p.grading = Some(g.clone());
            let mut original = vec![];
            for stop in [true, false] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "graded_restart_child", "--nocapture"])
                    .env(
                        GRADED_RESTART_ENV,
                        serde_json::to_string(&(&p, stop)).unwrap(),
                    )
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{arm}/{limit}: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                let grade =
                    fs::read(p.out.join("artifacts/c0/analyze.attempts/0/grade.json")).unwrap();
                if stop {
                    original = grade;
                } else {
                    assert_eq!(grade, original);
                }
            }
            let w = celeris_campaign::dagbench::celeris_side::read_worker_ledger(
                &p.out.join("worker.jsonl"),
            )
            .unwrap();
            assert_eq!(
                w.iter()
                    .filter(|e| e.node == "analyze" && e.event == "node_started")
                    .count(),
                limit as usize,
                "{arm}/{limit}: {w:?}"
            );
            let m = compute(&p.out).unwrap();
            assert_eq!(
                m.completed.campaigns,
                usize::from(limit == 2),
                "{arm}/{limit}: {m:?}"
            );
        }
    }
}

#[test]
fn logical_budget_stop_survives_controller_restart() {
    let root = tempfile::tempdir().unwrap();
    let g = grading_fixture::fixture(&root.path().join("inputs"), vec![]);
    for arm in ["baseline", "celeris"] {
        let mut p = params(arm, "camp-1", None, root.path());
        p.grading = Some(g.clone());
        let mut starts = 0;
        for stop in [true, false] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "graded_restart_child", "--nocapture"])
                .env(
                    GRADED_RESTART_ENV,
                    serde_json::to_string(&(&p, stop)).unwrap(),
                )
                .env("DAGBENCH_LOGICAL_CAP", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{arm}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let w = celeris_campaign::dagbench::celeris_side::read_worker_ledger(
                &p.out.join("worker.jsonl"),
            )
            .unwrap();
            let count = w.iter().filter(|e| e.event == "node_started").count();
            if stop {
                starts = count;
            } else {
                assert_eq!(count, starts, "budget must remain exhausted");
            }
            assert_eq!(compute(&p.out).unwrap().completed.campaigns, 0);
        }
    }
}
