//! dagbench の障害注入（`dagbench::fault`）: 3 種 × 2 方式（baseline / celeris-side）に同じ障害条件を与え、
//! 独立した外部側記録（ext-sim ledger）と観測 ledger（runner が書く `fault.jsonl`）から結果を確かめる。
//!
//! 制御 process は、この試験 binary 自身を `--exact child_control` で起こし直したもの（測定側が所有する子
//! process）。runner は握っている `Child` にだけ SIGKILL を送る。時間は制御 process の論理時計だけで進み、
//! 停止点は ext-sim の event 到達（`fault_fired` を ledger に確定させてから停止 hook で止まる）で決まる。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        baseline::{Baseline, BaselineConfig, Outcome, Status as BStatus, load_scenario},
        celeris_side::{
            CelerisSide, Paths, RunSpec, WorkerEntry, load_scenarios, read_worker_ledger,
        },
        ext::{Entry, ExtConfig, ExtSim, read_ledger},
        fault::{
            ControlCommand, FaultCase, FaultReport, FaultRunner, ObsEntry, RunnerOutcome,
            RunnerStop, WorkTable, block_until_killed, read_obs, report,
        },
    },
};
use serde_json::{Value, json};
use task_core::{Status, TaskId, TaskStore};

const SCENARIO: &str = "k2-mini";
/// baseline は 1 DAG instance の実行器なので、両方式とも campaign 1 本の点で同じ条件を与える。
const POINT: &str = "camp-1";
const CHILD_ENV: &str = "DAGBENCH_FAULT_CHILD";

fn scenarios() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

fn spec(seed: u64) -> RunSpec {
    let file = load_scenarios(&scenarios()).unwrap();
    RunSpec::resolve(&file, SCENARIO, POINT, seed).unwrap()
}

/// 正常 run の trigger event 出現回数: 論理 job ごとに受理・待機開始・完了が 1 回ずつ。
fn occurrences(s: &RunSpec) -> u64 {
    (s.point.concurrent_campaigns
        * s.scenario
            .nodes
            .iter()
            .filter(|n| n.external.is_some())
            .count()) as u64
}

// ---- 制御 process（子）の側 ----

/// task-api の実 router への同期の Transport（`tests/dagbench_celeris.rs` と同じ）。
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

/// 同じ DB file を開き直す router（制御 process の再起動ごとに作り直す）。
fn router_at(dir: &Path) -> axum::Router {
    let db = dir.join("celeris.db");
    let opts = api_common::EnvOptions {
        token: Some(api_common::TOKEN.to_string()),
        ..Default::default()
    };
    let settings = api_common::settings(
        &db,
        &dir.join("api-workspaces"),
        &dir.join("workspace"),
        &dir.join("knowledge"),
        opts,
    );
    let (_tx, rx) = tokio::sync::watch::channel(None);
    task_api::router(task_api::ApiState::new(settings, rx).unwrap())
}

fn baseline_paths(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (
        dir.join("state.json"),
        dir.join("ext.jsonl"),
        dir.join("worker.jsonl"),
    )
}

fn celeris_paths(dir: &Path) -> Paths {
    Paths::under(&dir.join("celeris.db"), dir)
}

/// 子 process の入口。runner が `CHILD_ENV` に条件を入れて起こしたときだけ動く（通常の試験実行では何もしない）。
#[test]
fn child_control() {
    let Ok(cfg) = std::env::var(CHILD_ENV) else {
        return;
    };
    let cfg: Value = serde_json::from_str(&cfg).unwrap();
    let dir = PathBuf::from(cfg["dir"].as_str().unwrap());
    let case: FaultCase = serde_json::from_value(cfg["case"].clone()).unwrap();
    let s = spec(case.seed);
    match cfg["system"].as_str().unwrap() {
        "baseline" => {
            let (state, ext_path, progress) = baseline_paths(&dir);
            let sc = load_scenario(&scenarios(), SCENARIO).unwrap();
            let mut ext = ExtSim::open(
                &ext_path,
                ExtConfig {
                    poll_secs: s.poll_secs,
                    job_slots: s.scenario.resources.external_job_slots,
                    faults: vec![case.ext_fault()],
                },
            )
            .unwrap();
            ext.set_stop_hook(block_until_killed());
            let mut b = Baseline::open(
                sc,
                BaselineConfig {
                    poll_secs: s.poll_secs,
                    ext_job_secs: s.ext_job_secs,
                },
                &state,
            )
            .unwrap();
            b.set_progress_log(&progress).unwrap();
            assert_eq!(b.run(&mut ext).unwrap(), Outcome::Complete);
        }
        "celeris" => {
            let paths = celeris_paths(&dir);
            let store: Arc<dyn TaskStore> =
                Arc::new(task_core::SqliteStore::open(&paths.db).unwrap());
            let transport = RouterTransport(router_at(&dir));
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            let report = rt.block_on(async {
                let mut side = CelerisSide::with_faults(
                    s.clone(),
                    paths,
                    store,
                    vec![case.ext_fault()],
                    Some(block_until_killed()),
                )
                .unwrap();
                side.drive(&transport).await
            });
            let report = report.unwrap_or_else(|e| panic!("drive failed: {e}"));
            for c in &report.campaigns {
                assert_eq!(c.status, "done", "{:?}", report.campaigns);
            }
        }
        other => panic!("unknown system {other}"),
    }
}

// ---- 観測側（runner） ----

struct Run {
    dir: tempfile::TempDir,
    system: &'static str,
    report: FaultReport,
    obs: Vec<ObsEntry>,
    ext: Vec<Entry>,
    worker: Vec<WorkerEntry>,
}

fn runner(dir: &Path, system: &str, case: &FaultCase, stop: RunnerStop) -> FaultRunner {
    let file = load_scenarios(&scenarios()).unwrap();
    let cfg = json!({"dir": dir, "system": system, "case": case});
    FaultRunner {
        case: case.clone(),
        system: system.into(),
        cmd: ControlCommand {
            program: std::env::current_exe().unwrap(),
            args: vec![
                "--exact".into(),
                "child_control".into(),
                "--nocapture".into(),
            ],
            env: vec![(CHILD_ENV.into(), cfg.to_string())],
        },
        ext_ledger: dir.join("ext.jsonl"),
        obs_path: dir.join("fault.jsonl"),
        log_dir: dir.join("control-logs"),
        max_restarts: 3,
        wall_cap: Duration::from_secs(file.observation_caps.max_wall_secs),
        stop,
    }
}

fn work_table(s: &RunSpec) -> WorkTable {
    WorkTable {
        nodes: s
            .scenario
            .nodes
            .iter()
            .map(|n| {
                (
                    n.id.clone(),
                    (
                        n.work_secs,
                        n.external
                            .as_ref()
                            .map(|e| e.post_wait_work_secs)
                            .unwrap_or(0),
                    ),
                )
            })
            .collect(),
        job_secs: s.ext_job_secs,
    }
}

fn collect(dir: tempfile::TempDir, system: &'static str, case: &FaultCase) -> Run {
    let obs = read_obs(&dir.path().join("fault.jsonl")).unwrap();
    let ext = read_ledger(&dir.path().join("ext.jsonl")).unwrap();
    let worker = read_worker_ledger(&dir.path().join("worker.jsonl")).unwrap();
    let report = report(&obs, &ext, &worker, &work_table(&spec(case.seed))).unwrap();
    Run {
        dir,
        system,
        report,
        obs,
        ext,
        worker,
    }
}

/// 失敗時の診断: 制御 process の stderr の末尾と観測 ledger。
fn logs(dir: &Path) -> String {
    let mut out = String::new();
    if let Ok(rd) = std::fs::read_dir(dir.join("control-logs")) {
        let mut names: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        names.sort();
        for p in names {
            let t = std::fs::read_to_string(&p).unwrap_or_default();
            let tail: String = t
                .chars()
                .rev()
                .take(1500)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            out.push_str(&format!("== {}\n{tail}\n", p.display()));
        }
    }
    out.push_str(&std::fs::read_to_string(dir.join("fault.jsonl")).unwrap_or_default());
    out
}

fn run_system(system: &'static str, case: &FaultCase) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let out = runner(dir.path(), system, case, RunnerStop::Never)
        .run()
        .unwrap_or_else(|e| panic!("{system} {}: {e}\n{}", case.fault_id, logs(dir.path())));
    assert!(matches!(out, RunnerOutcome::Completed { .. }), "{out:?}");
    collect(dir, system, case)
}

/// 完走（制御側の永続 state）と、完了済みの作業が再実行されていないこと（所定状態からの再開）。
fn assert_completed_from_persisted_state(r: &Run) {
    let s = spec(r.report.seed);
    match r.system {
        "baseline" => {
            let st: Value = serde_json::from_str(
                &std::fs::read_to_string(r.dir.path().join("state.json")).unwrap(),
            )
            .unwrap();
            for n in &s.scenario.nodes {
                let status: BStatus =
                    serde_json::from_value(st["nodes"][&n.id]["status"].clone()).unwrap();
                assert_eq!(status, BStatus::Done, "{} not done", n.id);
            }
        }
        _ => {
            let store = task_core::SqliteStore::open(&r.dir.path().join("celeris.db")).unwrap();
            let roots: Vec<TaskId> = r
                .worker
                .iter()
                .filter(|e| e.event == "campaign_submitted")
                .map(|e| {
                    e.detail.as_ref().unwrap()["task_id"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap()
                })
                .collect();
            assert_eq!(
                roots.len(),
                s.point.concurrent_campaigns,
                "POST once per campaign"
            );
            for id in roots {
                assert_eq!(store.get(id).unwrap().unwrap().status, Status::Done);
            }
        }
    }
    // 各 node の完了は worker 側の記録でちょうど 1 回（完了済みの作業をやり直さない・二重に完了しない）。
    let mut finished: BTreeMap<String, usize> = BTreeMap::new();
    for e in r.worker.iter().filter(|e| e.event == "node_finished") {
        *finished.entry(e.node.clone()).or_default() += 1;
    }
    for n in &s.scenario.nodes {
        assert_eq!(
            finished.get(&n.id),
            Some(&1),
            "{} finished {:?}",
            r.system,
            finished
        );
    }
}

/// 独立した外部記録との整合（全障害共通）。
fn assert_external_consistent(r: &Run, case: &FaultCase) {
    let s = spec(case.seed);
    let rep = &r.report;
    let jobs = occurrences(&s) as usize;
    assert_eq!(rep.dataset, "control_measured");
    assert_eq!(rep.fault_id, case.fault_id);
    assert_eq!(rep.seed, case.seed);
    assert_eq!(
        rep.nth,
        1 + case.seed % occurrences(&s),
        "seeded trigger position"
    );
    assert!(rep.trigger_event_seq > 0);
    assert_eq!(rep.logical_jobs, jobs, "{}: {rep:?}", r.system);
    assert_eq!(
        rep.physical_launches, jobs,
        "{}: actual launches == logical jobs",
        r.system
    );
    assert_eq!(
        rep.consumed, jobs,
        "{}: each result consumed once",
        r.system
    );
    assert_eq!(rep.double_counted, 0, "{}: no double counting", r.system);
    assert_eq!(rep.interventions, 0, "no manual intervention was needed");
    assert!(r.ext.iter().all(|e| e.dataset == "control_measured"));
    assert!(r.obs.iter().all(|e| e.dataset == "control_measured"));
    // trigger の位置と seed は runner の観測 ledger にも残る。
    let t = r
        .obs
        .iter()
        .find(|e| e.event == "trigger_observed")
        .expect("trigger observed");
    assert_eq!(t.detail["seed"], case.seed);
    assert_eq!(t.detail["nth"], rep.nth);
    assert_eq!(t.detail["event"], case.trigger_event.as_str());
    assert!(t.detail["ext_seq"].as_u64().unwrap() > 0);
    assert_completed_from_persisted_state(r);
}

/// 同じ障害条件: 両方式で同じ seed・trigger event・nth（event の n 件目）。同時に ready になる外部 node の投入順は
/// 方式ごとに違うので、当たる論理 job は条件ではなく結果として ledger に記録する（両方式とも scenario の外部 node）。
fn assert_same_condition(a: &Run, b: &Run) {
    let strip = |s: &Option<String>| {
        s.as_deref()
            .map(|x| x.rsplit('/').next().unwrap_or(x).to_string())
    };
    assert_eq!(a.report.seed, b.report.seed);
    assert_eq!(a.report.nth, b.report.nth);
    let externals: Vec<String> = spec(a.report.seed)
        .scenario
        .nodes
        .iter()
        .filter_map(|n| n.external.as_ref().map(|e| e.logical_job_id.clone()))
        .collect();
    for r in [a, b] {
        let job = strip(&r.report.trigger_logical_job_id).expect("trigger job recorded");
        assert!(externals.contains(&job), "{job} not in {externals:?}");
    }
}

fn case(fault_id: &str) -> FaultCase {
    let file = load_scenarios(&scenarios()).unwrap();
    FaultCase::from_scenarios(
        &scenarios(),
        fault_id,
        file.seed,
        occurrences(&spec(file.seed)),
    )
    .unwrap()
}

fn both(c: &FaultCase) -> (Run, Run) {
    let (b, ce) = std::thread::scope(|s| {
        let b = s.spawn(|| run_system("baseline", c));
        let ce = s.spawn(|| run_system("celeris", c));
        (b.join().unwrap(), ce.join().unwrap())
    });
    assert_same_condition(&b, &ce);
    (b, ce)
}

fn assert_control_restarted(r: &Run) {
    let rep = &r.report;
    assert_eq!(rep.control_stops, 1, "{}: {:?}", r.system, r.obs);
    assert_eq!(rep.control_restarts, 1);
    assert_eq!(rep.runner_stops, 0);
    let stopped = r.obs.iter().find(|e| e.event == "control_stopped").unwrap();
    assert_eq!(stopped.detail["signal"], "SIGKILL");
    let exited = r
        .obs
        .iter()
        .find(|e| e.event == "control_exited" && e.detail["killed_by_runner"] == true)
        .unwrap();
    assert_eq!(exited.detail["signal"], 9, "killed by SIGKILL");
    // 殺したのは runner が起こした制御 process（同じ pid の control_started がある）。
    let pid = &stopped.detail["pid"];
    assert!(
        r.obs
            .iter()
            .any(|e| e.event == "control_started" && &e.detail["pid"] == pid)
    );
    assert!(rep.recovery_logical_secs.is_some(), "{}: {rep:?}", r.system);
    assert!(rep.recovery_wall_ms.is_some());
}

#[test]
fn a1_control_killed_after_accept_resumes_without_double_launch() {
    let c = case("a1-restart-after-accept");
    let (b, ce) = both(&c);
    for r in [&b, &ce] {
        assert_external_consistent(r, &c);
        assert_control_restarted(r);
    }
    // baseline は state の `submitted` を照会で解く（再投入しない）。Celeris は孤児回収で WorkUnit を再実行し、
    // 同じ論理 ID の再投入が既存の物理起動に寄る（失われた進捗 = 投入前の作業 1 回分）。
    assert_eq!(b.report.lost_progress_secs, 0, "{:?}", b.report);
    assert!(b.report.lookups >= 1);
    let target = ce.report.trigger_logical_job_id.clone();
    assert!(
        ce.ext
            .iter()
            .any(|e| e.event == "submit_deduped" && e.logical_job_id == target),
        "{:?}",
        ce.ext
    );
}

#[test]
fn pre_save_control_killed_after_wait_started_resumes() {
    let c = case("a0-restart-before-wait-save");
    let (b, ce) = both(&c);
    for r in [&b, &ce] {
        assert_external_consistent(r, &c);
        assert_control_restarted(r);
    }
}

#[test]
fn b_lost_submit_response_is_reconciled_by_logical_id() {
    let c = case("b-submit-response-lost");
    let (b, ce) = both(&c);
    for r in [&b, &ce] {
        assert_external_consistent(r, &c);
        assert_eq!(r.report.control_stops, 0);
        let target = r.report.trigger_logical_job_id.clone();
        // 応答喪失の後、論理 ID の照会で解いた（外部側のアクセス記録）。
        assert!(
            r.ext.iter().any(|e| e.event == "lookup"
                && e.logical_job_id == target
                && e.seq > r.report.trigger_event_seq),
            "{}",
            r.system
        );
        assert_eq!(
            r.report.lost_progress_secs, 0,
            "{}: {:?}",
            r.system, r.report
        );
        assert_eq!(r.report.recovery_logical_secs, Some(0), "{}", r.system);
    }
}

#[test]
fn c_duplicate_completion_notice_is_counted_once() {
    let c = case("c-completion-duplicate");
    let (b, ce) = both(&c);
    for r in [&b, &ce] {
        assert_external_consistent(r, &c);
        assert_eq!(r.report.control_stops, 0);
        let target = r.report.trigger_logical_job_id.clone();
        let copies = r
            .ext
            .iter()
            .filter(|e| e.event == "notice_sent" && e.logical_job_id == target)
            .count();
        assert_eq!(
            copies, c.copies as usize,
            "{}: duplicate was delivered",
            r.system
        );
        assert_eq!(r.report.consume_ignored, 0);
    }
}

/// 『制御（daemon）停止』と『評価 runner 停止』は観測 ledger 上で別の event 種別になる。
#[test]
fn runner_stop_is_distinguished_from_control_stop() {
    let c = case("a1-restart-after-accept");
    for system in ["baseline", "celeris"] {
        let dir = tempfile::tempdir().unwrap();
        let first = runner(dir.path(), system, &c, RunnerStop::AfterControlStopped)
            .run()
            .unwrap();
        assert_eq!(first, RunnerOutcome::RunnerStopped);
        let second = runner(dir.path(), system, &c, RunnerStop::Never)
            .run()
            .unwrap();
        assert_eq!(second, RunnerOutcome::Completed { restarts: 1 });
        let r = collect(dir, system, &c);
        assert_external_consistent(&r, &c);
        assert_eq!(r.report.control_stops, 1, "{system}");
        assert_eq!(r.report.runner_stops, 1, "{system}");
        assert_eq!(r.report.control_restarts, 1, "{system}");
        let kinds: Vec<&str> = r.obs.iter().map(|e| e.event.as_str()).collect();
        let ctl = kinds.iter().position(|k| *k == "control_stopped").unwrap();
        let rs = kinds.iter().position(|k| *k == "runner_stopped").unwrap();
        assert!(ctl < rs, "{kinds:?}");
        let stopped = &r.obs[rs];
        assert_eq!(stopped.detail["detected_at_resume"], true);
    }
}
