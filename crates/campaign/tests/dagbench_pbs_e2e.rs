//! recovery 比較の opt-in PBS 入口の offline e2e（ADR 2026-10-25-recovery-scheduler-backend D2・D5・実装付記 I8・I12）。
//!
//! 偽 transport（stateful な偽 PBS cluster）と注入時計で、baseline / Celeris の両 arm に normal と A2
//! （保存済み Waiting 確認 → 所有 controller 停止 → 停止中に同じ job を観測 → 再開 → 同じ job を回収 → 独立採点）を
//! 同じ測定 dir・同じ spec で流し、物理 qsub と消費の数、raw の validator と再集計の一致を確かめる。
//! grade 失敗・unknown submit の停止・wrapper 環境の拒否も見る。ssh・qsub・外部 network は使わず sleep しない。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs, io,
    path::Path,
    sync::{Arc, Mutex},
};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        celeris_side::{CelerisSide, Paths, RunSpec},
        ext::{ExtBackend, ExtConfig},
        pbs::{
            backend::{Arm, CONSUMED_LEDGER, RecoverySpec},
            entry::{
                A2_FILE, BARRIERS_FILE, BackendChoice, BackendFlag, BaselineController, Condition,
                Controller, GatedExt, PbsChoice, SavedReader, Site, Study, TrialRun,
                create_measurement, refuse_wrapper_env, run_spec, run_trial,
            },
            intent::{INTENT_LEDGER, IntentClock},
            raw::{self, RawWriter, StudyPins, TrialStatus},
            transport::{CallOutcome, MarkerQuery, PbsCall, PbsReply, PbsTransport, QsubRequest},
        },
    },
    observe::Origin,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::{TaskStore, cluster_job::ClusterJobWaitStore};

const T0: u64 = 1_800_000_000;
const OUTPUT: &[u8] = b"recovery payload output seed=17\n";

/// sleep せず、要求された時刻へ跳ぶ時計。
struct FakeClock(Mutex<u64>);

impl IntentClock for FakeClock {
    fn now_unix(&self) -> u64 {
        *self.0.lock().expect("clock")
    }
    fn sleep_until(&self, t: u64) {
        let mut n = self.0.lock().expect("clock");
        *n = (*n).max(t);
    }
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[derive(Default)]
struct ClusterState {
    qsub: u32,
    qstat: u32,
    /// job id → (要求, 解放済み)
    jobs: BTreeMap<String, (QsubRequest, bool)>,
    qsub_timeout: bool,
    /// fetch が返す成果物（None なら OUTPUT）。
    output: Option<Vec<u8>>,
}

/// 偽 PBS cluster。job は完了解放 barrier（[`Site::release`]）が外れるまで `R`、外れたら `F`/Exit 0。
#[derive(Clone, Default)]
struct Cluster(Arc<Mutex<ClusterState>>);

impl Cluster {
    fn st(&self) -> std::sync::MutexGuard<'_, ClusterState> {
        self.0.lock().expect("cluster")
    }
    fn qsubs(&self) -> u32 {
        self.st().qsub
    }
}

fn reply(
    call: PbsCall,
    argv: Vec<String>,
    outcome: CallOutcome,
    stdout: String,
    stderr: &str,
) -> PbsReply {
    PbsReply {
        call,
        argv,
        started_utc: "2027-01-15T08:00:00Z".into(),
        finished_utc: "2027-01-15T08:00:00Z".into(),
        exit: Some(if outcome == CallOutcome::Ok { 0 } else { 1 }),
        stdout,
        stderr: stderr.into(),
        outcome,
    }
}

impl PbsTransport for Cluster {
    fn origin(&self) -> Origin {
        Origin::Synthetic
    }
    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        let argv = req.argv().expect("argv");
        let mut s = self.st();
        s.qsub += 1;
        if s.qsub_timeout {
            return reply(
                PbsCall::Qsub,
                argv,
                CallOutcome::Timeout,
                String::new(),
                "timed out",
            );
        }
        let id = format!("{}.sirius", 4300 + s.qsub);
        s.jobs.insert(id.clone(), (req.clone(), false));
        reply(PbsCall::Qsub, argv, CallOutcome::Ok, format!("{id}\n"), "")
    }
    fn qstat_full(&self, job_id: &str) -> PbsReply {
        let argv = vec!["qstat".into(), "-x".into(), "-f".into(), job_id.into()];
        let mut s = self.st();
        s.qstat += 1;
        let Some((req, released)) = s.jobs.get(job_id) else {
            return reply(
                PbsCall::QstatFull,
                argv,
                CallOutcome::NonZero,
                String::new(),
                &format!("qstat: Unknown Job Id {job_id}"),
            );
        };
        let state = if *released { "F" } else { "R" };
        let mut kv = vec![
            ("Job_Name".to_string(), req.job_name.clone()),
            ("Job_Owner".into(), "rmaeda@sirius-login1".into()),
            ("job_state".into(), state.into()),
            ("queue".into(), req.queue.clone()),
            ("server".into(), "sirius".into()),
            ("Account_Name".into(), req.account.clone()),
            ("Rerunable".into(), "False".into()),
            ("Resource_List.place".into(), req.place.clone()),
            ("Resource_List.select".into(), req.select.clone()),
            ("Resource_List.walltime".into(), req.walltime.clone()),
            (
                "Variable_List".into(),
                format!(
                    "PBS_O_HOME=/home/rmaeda,CELERIS_TRIAL_KEY={},CELERIS_SUBMIT_INTENT={}",
                    req.trial_key, req.submit_intent
                ),
            ),
            (
                "Submit_arguments".into(),
                format!("-q {} -A {} -r n {}", req.queue, req.account, req.script),
            ),
        ];
        if state == "F" {
            kv.push(("Exit_status".into(), "0".into()));
        }
        let mut out = format!("Job Id: {job_id}\n");
        for (k, v) in kv {
            out.push_str(&format!("    {k} = {v}\n"));
        }
        out.push('\n');
        reply(PbsCall::QstatFull, argv, CallOutcome::Ok, out, "")
    }
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        reply(
            PbsCall::FindByMarker,
            vec!["find-by-marker".into(), m.submit_intent.clone()],
            CallOutcome::Unsupported,
            String::new(),
            "marker search capability not confirmed",
        )
    }
}

impl Site for Cluster {
    fn release(&self, job_id: &str, _reason: &str) -> io::Result<()> {
        let mut s = self.st();
        let j = s
            .jobs
            .get_mut(job_id)
            .ok_or_else(|| io::Error::other(format!("release: no job {job_id}")))?;
        j.1 = true;
        Ok(())
    }
    fn fetch_outputs(&self, job_id: &str, dest: &Path) -> io::Result<Vec<String>> {
        let s = self.st();
        if !s.jobs.get(job_id).is_some_and(|j| j.1) {
            return Err(io::Error::other(format!("{job_id} has not finished")));
        }
        fs::write(dest.join("out.bin"), s.output.as_deref().unwrap_or(OUTPUT))?;
        Ok(vec!["out.bin".into()])
    }
}

fn spec() -> RecoverySpec {
    let v = json!({
        "measurement_id": "rcv-e2e-1",
        "scheduler": "pbs",
        "cluster": "sirius",
        "owner": "rmaeda",
        "queue": "gen",
        "account": "NBBG",
        "resources": {"select": "1:ncpus=24:mem=124gb:ngpus=1", "place": "exclhost", "walltime_secs": 300},
        "rerunnable": false,
        "payload": {
            "script": "/work/NBB/rmaeda/recovery/job.sh",
            "script_sha256": "a".repeat(64),
            "input_sha256": "b".repeat(64),
            "expected_output_sha256": sha(OUTPUT)
        },
        "grader": {"binary_sha256": "d".repeat(64), "version": "g1"},
        "env": {"modules": ["gcc/13"], "env_sha256": "e".repeat(64)},
        "limits": {
            "external_slots": 1, "worker_slots": 1, "poll_secs": 30, "trial_deadline_secs": 1800,
            "ssh_timeout_secs": 60, "max_jobs": 4, "workload_attempts": 1
        },
        "conditions": ["normal", "a2"],
        "seed": 17,
        "rep": 1
    });
    RecoverySpec::parse(&v.to_string()).expect("spec")
}

fn study() -> Study {
    Study {
        pins: StudyPins {
            campaign_id: "recovery-pbs-e2e".into(),
            manifest_sha256: "f".repeat(64),
            case_sha256: "1".repeat(64),
            source: "agent-platform@test".into(),
            binary_sha256: "2".repeat(64),
        },
        problem_id: "rcv".into(),
        trial_no: 1,
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
    task_api::router(task_api::ApiState::new(settings, rx).expect("api state"))
}

/// Celeris arm の controller。真実は測定専用 SQLite、停止は drive の future を捨てる（所有 controller の停止）。
struct CelerisController {
    spec: RunSpec,
    dir: std::path::PathBuf,
    stop: Arc<tokio::sync::Notify>,
}

impl Controller for CelerisController {
    fn run(&mut self, ext: GatedExt) -> Result<bool, String> {
        let paths = Paths::under(&self.dir.join("celeris.db"), &self.dir);
        let store: Arc<dyn TaskStore> =
            Arc::new(task_core::SqliteStore::open(&paths.db).map_err(|e| e.to_string())?);
        let transport = RouterTransport(router_at(&self.dir));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let gate = ext.gate();
        let stop = self.stop.clone();
        let stop_gate = gate.clone();
        let spec = self.spec.clone();
        rt.block_on(async move {
            let mut side = CelerisSide::with_backend(spec, paths, store, Box::new(ext))?;
            tokio::select! {
                r = side.drive(&transport) => {
                    if gate.is_stopped() {
                        return Ok(false);
                    }
                    let r = r?;
                    Ok(r.campaigns.iter().all(|c| c.status == "done"))
                }
                // 前の incarnation の permit で起きても、この gate が止めていなければ待ち直す。
                _ = async {
                    while !stop_gate.is_stopped() {
                        stop.notified().await;
                    }
                } => Ok(false),
            }
        })
    }

    fn saved_reader(&self) -> SavedReader {
        let db = self.dir.join("celeris.db");
        Box::new(move |physical| {
            let store =
                task_core::SqliteStore::open(&db).map_err(|e| io::Error::other(e.to_string()))?;
            let waits = store
                .cluster_job_waits_waiting()
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(waits
                .into_iter()
                .find(|w| w.jobs.iter().any(|j| j == physical))
                .map(|w| json!({"wait_id": w.wait_id, "job_id": physical, "source": "TaskStore"})))
        })
    }
}

struct Measurement {
    dir: tempfile::TempDir,
    cluster: Cluster,
    clock: Arc<FakeClock>,
    spec: RecoverySpec,
    study: Study,
}

impl Measurement {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("dir"),
            cluster: Cluster::default(),
            clock: Arc::new(FakeClock(Mutex::new(T0))),
            spec: spec(),
            study: study(),
        }
    }

    fn raw(&self) -> RawWriter {
        create_measurement(
            self.dir.path(),
            &self.spec,
            Arc::new(self.cluster.clone()),
            self.clock.clone(),
            "obs-1",
        )
        .expect("raw")
    }

    fn trial(&self, arm: Arm, condition: Condition) -> TrialRun<'_> {
        TrialRun {
            dir: self.dir.path(),
            spec: &self.spec,
            study: &self.study,
            arm,
            condition,
            transport: Arc::new(self.cluster.clone()),
            clock: self.clock.clone(),
            site: Arc::new(self.cluster.clone()),
        }
    }

    fn run(
        &self,
        raw: &mut RawWriter,
        arm: Arm,
        condition: Condition,
    ) -> Result<celeris_campaign::dagbench::pbs::entry::TrialReport, String> {
        let t = self.trial(arm, condition);
        let rs = run_spec(&self.spec).expect("run spec");
        let trial_dir = t.trial_dir();
        fs::create_dir_all(&trial_dir).expect("trial dir");
        match arm {
            Arm::Baseline => {
                let mut c = BaselineController::new(rs, &trial_dir);
                run_trial(&t, &mut c, raw, Box::new(|| {}))
            }
            Arm::Celeris => {
                let stop = Arc::new(tokio::sync::Notify::new());
                let mut c = CelerisController {
                    spec: rs,
                    dir: trial_dir,
                    stop: stop.clone(),
                };
                run_trial(&t, &mut c, raw, Box::new(move || stop.notify_one()))
            }
        }
    }
}

fn lines(p: &Path) -> Vec<Value> {
    fs::read_to_string(p)
        .map(|t| {
            t.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).expect("row"))
                .collect()
        })
        .unwrap_or_default()
}

/// 両 arm × normal/A2 を同じ測定 dir で流す。物理 qsub 4・消費 4、A2 は同じ job を停止中に観測して再開後に回収し、
/// raw の validator が受け、再収集（read-only）で同じ結果が再集計される。
#[test]
fn pbs_e2e_normal_and_a2_both_arms_validate_and_recollect() {
    let m = Measurement::new();
    let mut raw = m.raw();
    let mut reports = Vec::new();
    for arm in [Arm::Baseline, Arm::Celeris] {
        for cond in [Condition::Normal, Condition::A2] {
            let before = m.cluster.qsubs();
            let r = m
                .run(&mut raw, arm, cond)
                .unwrap_or_else(|e| panic!("{arm:?}/{cond:?}: {e}"));
            assert_eq!(
                m.cluster.qsubs(),
                before + 1,
                "{arm:?}/{cond:?}: one physical qsub"
            );
            assert_eq!(
                r.binding.status,
                TrialStatus::ValidatedCompleted,
                "{arm:?}/{cond:?}"
            );
            assert_eq!(
                r.binding.job_ref.as_deref(),
                Some(format!("pbs:sirius:{}", r.job_id).as_str())
            );
            let t = m.trial(arm, cond);
            let barriers: Vec<String> = lines(&t.trial_dir().join(BARRIERS_FILE))
                .iter()
                .map(|b| b["event"].as_str().expect("event").to_string())
                .collect();
            match cond {
                Condition::Normal => {
                    assert_eq!(barriers, ["wait_saved", "released"], "{arm:?}");
                    assert!(r.a2.is_none());
                }
                Condition::A2 => {
                    assert_eq!(
                        barriers,
                        [
                            "wait_saved",
                            "controller_stopped",
                            "released",
                            "observed_finished_while_stopped",
                            "controller_resumed"
                        ],
                        "{arm:?}"
                    );
                    let a2 = r.a2.clone().expect("a2 evidence");
                    assert!(a2.controller_stopped);
                    assert_eq!(a2.job_id, r.job_id);
                    assert_eq!(a2.observed_finished_while_stopped, 1);
                    assert_eq!(
                        a2.intent_rows_before_stop, a2.intent_rows_after_resume,
                        "no new submit"
                    );
                    assert_eq!(a2.consumed_job_ref, format!("pbs:sirius:{}", r.job_id));
                    let file: Value = serde_json::from_slice(
                        &fs::read(t.trial_dir().join(A2_FILE)).expect("a2.json"),
                    )
                    .expect("json");
                    assert_eq!(file["job_id"], json!(r.job_id));
                }
            }
            reports.push(r);
        }
    }
    raw.write_run(study().pins).expect("run.json");
    assert_eq!(
        m.cluster.qsubs(),
        4,
        "max_jobs: one job per arm × condition"
    );
    let consumed = lines(&m.dir.path().join(CONSUMED_LEDGER));
    assert_eq!(consumed.len(), 4, "each job consumed once");
    let mut jobs: Vec<&str> = reports.iter().map(|r| r.job_id.as_str()).collect();
    jobs.dedup();
    assert_eq!(jobs.len(), 4, "arms do not share a physical job");

    let report = raw::validate(m.dir.path()).expect("valid raw");
    assert_eq!(report.run.real_jobs, 4);
    assert_eq!(report.run.llm_calls, 0);
    assert_eq!(report.trials.len(), 4);
    let first: BTreeMap<String, TrialStatus> = report
        .trials
        .iter()
        .map(|(k, b)| (k.clone(), b.status))
        .collect();
    assert!(
        first
            .values()
            .all(|s| *s == TrialStatus::ValidatedCompleted)
    );

    // 再収集: qstat と成果物の読み直しだけ。qsub も消費も増えず、raw から同じ結果が再集計される。
    let qsub_before = m.cluster.qsubs();
    let mut again = RawWriter::reopen(m.dir.path(), "obs-2").expect("reopen");
    for r in &reports {
        let (arm, cond) = match r.binding.arm.as_str() {
            "baseline" => (Arm::Baseline, &r.binding),
            _ => (Arm::Celeris, &r.binding),
        };
        let cond = if cond.collector["problem_id"]
            .as_str()
            .is_some_and(|p| p.ends_with("-a2"))
        {
            Condition::A2
        } else {
            Condition::Normal
        };
        let t = m.trial(arm, cond);
        let att = t.trial_dir().join("attempt-1");
        let inp = celeris_campaign::dagbench::pbs::raw::TrialInput {
            arm,
            logical_job_id: r.logical_job_id.clone(),
            collector: m.study.collector(&m.spec, arm, cond),
            attempts: vec![celeris_campaign::dagbench::pbs::raw::AttemptArtifacts {
                attempt: 1,
                dir: att,
                paths: vec!["out.bin".into()],
            }],
            grade: Some(celeris_campaign::dagbench::pbs::raw::GradeEvidence {
                file: t.trial_dir().join("grade.json"),
                passed: true,
            }),
        };
        let b = again
            .collect_trial(
                celeris_campaign::dagbench::pbs::transport::QstatView::new(&m.cluster, "sirius"),
                &inp,
            )
            .expect("recollect");
        assert_eq!(b.status, r.binding.status);
    }
    assert_eq!(
        m.cluster.qsubs(),
        qsub_before,
        "re-collection does not qsub"
    );
    assert_eq!(
        lines(&m.dir.path().join(CONSUMED_LEDGER)).len(),
        4,
        "re-collection does not consume"
    );
    let second = raw::validate(m.dir.path()).expect("valid after recollect");
    let again_status: BTreeMap<String, TrialStatus> = second
        .trials
        .iter()
        .map(|(k, b)| (k.clone(), b.status))
        .collect();
    assert_eq!(again_status, first, "re-aggregation from raw matches");
}

/// 独立 grade が不合格なら、実行が終わり同じ job を回収しても validated_completed にならない（両 arm・A2 でも）。
#[test]
fn pbs_e2e_grade_failure_is_not_validated_completed() {
    let m = Measurement::new();
    m.cluster.st().output = Some(b"wrong output\n".to_vec());
    let mut raw = m.raw();
    for (arm, cond) in [
        (Arm::Baseline, Condition::A2),
        (Arm::Celeris, Condition::Normal),
    ] {
        let r = m.run(&mut raw, arm, cond).expect("trial runs");
        assert_eq!(
            r.binding.status,
            TrialStatus::ExecutedUnvalidated,
            "{arm:?}/{cond:?}"
        );
        assert_eq!(r.binding.exit_status, Some(0));
        assert!(!r.binding.output_matches_expected);
        let g: Value = serde_json::from_slice(
            &fs::read(m.trial(arm, cond).trial_dir().join("grade.json")).expect("grade"),
        )
        .expect("json");
        assert_eq!(g["passed"], json!(false));
    }
    raw.write_run(study().pins).expect("run.json");
    let report = raw::validate(m.dir.path()).expect("valid raw");
    assert!(
        report
            .trials
            .values()
            .all(|b| b.status == TrialStatus::ExecutedUnvalidated)
    );
    assert_eq!(m.cluster.qsubs(), 2);
}

/// qsub の応答喪失を照合できない（marker 検索が未確認）と、trial は停止し再投入しない（not_submitted にしない）。
#[test]
fn pbs_e2e_unknown_submit_stops_both_arms_without_resubmit() {
    for arm in [Arm::Baseline, Arm::Celeris] {
        let m = Measurement::new();
        m.cluster.st().qsub_timeout = true;
        let mut raw = m.raw();
        let err = m
            .run(&mut raw, arm, Condition::Normal)
            .expect_err("unknown submit must stop the trial");
        assert!(!err.is_empty());
        assert_eq!(m.cluster.qsubs(), 1, "{arm:?}: no automatic resubmit");
        let states: Vec<String> = lines(&m.dir.path().join(INTENT_LEDGER))
            .iter()
            .filter_map(|r| r["state"].as_str().map(str::to_string))
            .collect();
        assert!(
            states.iter().any(|s| s == "unknown_submit"),
            "{arm:?}: {states:?}"
        );
        assert!(
            states.iter().all(|s| s != "rejected_before_send"),
            "{arm:?}: {states:?}"
        );
        assert!(lines(&m.dir.path().join(CONSUMED_LEDGER)).is_empty());
    }
}

/// 既定は offline の ExtSim。recovery の投入は intent 台帳 1 本で、試行内 wrapper の環境があれば拒否する。
#[test]
fn pbs_e2e_entry_defaults_offline_and_refuses_wrapper_env() {
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        BackendFlag::from_args(&args(&["dagbench", "run"])),
        Ok(BackendFlag::ExtSim)
    );
    assert_eq!(
        BackendFlag::from_args(&args(&["dagbench", "run", "--backend", "pbs"])),
        Ok(BackendFlag::Pbs)
    );
    assert!(BackendFlag::from_args(&args(&["dagbench", "run", "--backend", "slurm"])).is_err());
    // CLI は PBS を実 transport で組まない（実接続は人の承認後、harness が transport を注入する）。
    let err = celeris_campaign::dagbench::run::cli(&args(&[
        "dagbench",
        "run",
        "--backend",
        "pbs",
        "--out",
        "/nonexistent",
    ]))
    .expect_err("cli does not build a real PBS transport");
    assert!(err.contains("BackendChoice::Pbs"), "{err}");
    // harness の backend 選択: ExtSim は offline の ledger、PBS は注入した transport の intent 台帳を開く。
    let dir = tempfile::tempdir().expect("dir");
    let mut sim = BackendChoice::ExtSim(ExtConfig {
        poll_secs: 30,
        job_slots: 1,
        faults: vec![],
    })
    .open(dir.path())
    .expect("ext-sim");
    assert_eq!(sim.now().expect("now"), 0);
    assert!(dir.path().join("ext.jsonl").exists());
    let m = Measurement::new();
    let mut pbs = BackendChoice::Pbs(Box::new(PbsChoice {
        spec: spec(),
        arm: Arm::Baseline,
        trial_key: "k".into(),
        epoch: "e1".into(),
        transport: Arc::new(m.cluster.clone()),
        clock: m.clock.clone(),
    }))
    .open(m.dir.path())
    .expect("pbs");
    assert!(matches!(
        pbs.lookup("never").expect("lookup"),
        celeris_campaign::dagbench::ext::Lookup::Unknown
    ));
    assert_eq!(m.cluster.qsubs(), 0);
    assert!(refuse_wrapper_env(|_| None).is_ok());
    let err = refuse_wrapper_env(|k| {
        (k == "CELERIS_SUBMIT_CMD").then(|| OsString::from("ssh sirius qsub"))
    })
    .expect_err("wrapper env refused");
    assert!(err.contains("CELERIS_SUBMIT_CMD"), "{err}");
}

/// A arm の wrapper `celeris-qsub` は recovery の job（CELERIS_SUBMIT_INTENT あり）から呼ばれると何も投入せずに止まる。
/// 印が無ければ従来どおり投入して受理記録を残す（A arm の用途は変えない）。
#[test]
fn pbs_e2e_celeris_qsub_refuses_inside_recovery_jobs() {
    let wrapper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/campaign/celeris-qsub");
    let dir = tempfile::tempdir().expect("dir");
    let fake = dir.path().join("fake-qsub");
    fs::write(
        &fake,
        "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/qsub.args\"\necho 4242.sirius\n",
    )
    .expect("fake");
    let audit = dir.path().join("audit.jsonl");
    let run = |intent: Option<&str>| {
        let mut c = std::process::Command::new("/bin/sh");
        c.arg(&wrapper)
            .arg("job.sh")
            .env("CELERIS_SUBMIT_AUDIT", &audit)
            .env("CELERIS_TRIAL_KEY", "campaign-k1")
            .env("CELERIS_SUBMIT_TARGET", "pbs:sirius")
            .env("CELERIS_SUBMIT_CMD", format!("/bin/sh {}", fake.display()))
            .env_remove("CELERIS_SUBMIT_INTENT");
        if let Some(i) = intent {
            c.env("CELERIS_SUBMIT_INTENT", i);
        }
        c.output().expect("wrapper")
    };
    let refused = run(Some("si-0123"));
    assert_eq!(refused.status.code(), Some(4));
    assert!(!dir.path().join("qsub.args").exists(), "nothing submitted");
    assert!(!audit.exists(), "nothing recorded");
    let ok = run(None);
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert_eq!(
        fs::read_to_string(&audit).expect("audit").lines().count(),
        1
    );
}
