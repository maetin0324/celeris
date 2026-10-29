//! recovery 比較の PBS backend（ADR 2026-10-25-recovery-scheduler-backend D2・D4・実装付記 I4・I5）。
//! 偽 transport を注入し、両 arm が同じ spec から同じ qsub 引数を作ること、controller 再起動後に同じ job ID を
//! 再追跡して新しい qsub を送らないこと、照合できない投入で停止することを、物理 qsub と消費の数で確かめる。
//! ssh・qsub・外部 network は使わず、時間は注入時計で進める（sleep しない）。
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use celeris_campaign::dagbench::{
    baseline::{Baseline, BaselineConfig, Outcome, Status, load_scenario},
    ext::{ExtBackend, JobState, Lookup, SubmitError},
    pbs::{
        backend::{Arm, CONSUMED_LEDGER, ConsumedRow, PbsExt, RecoverySpec},
        intent::{IntentClock, IntentSpec},
        transport::{
            CallOutcome, FakePbs, MarkerQuery, PbsCall, PbsReply, PbsTransport, QsubRequest,
            Scripted,
        },
    },
};
use celeris_campaign::observe::Origin;

const T0: u64 = 1_800_000_000;

/// sleep せず、要求された時刻へ跳ぶ時計。
struct FakeClock(Mutex<u64>);

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(T0)))
    }
}

impl IntentClock for FakeClock {
    fn now_unix(&self) -> u64 {
        *self.0.lock().expect("clock")
    }
    fn sleep_until(&self, t: u64) {
        let mut n = self.0.lock().expect("clock");
        *n = (*n).max(t);
    }
}

fn spec_json() -> serde_json::Value {
    serde_json::json!({
        "measurement_id": "rcv-m1",
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
            "expected_output_sha256": "c".repeat(64)
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
    })
}

fn spec() -> RecoverySpec {
    RecoverySpec::parse(&spec_json().to_string()).expect("spec")
}

fn trial(arm: Arm) -> String {
    format!("rcv-m1.{}.normal.r1", arm.as_str())
}

fn open(
    dir: &Path,
    spec: RecoverySpec,
    arm: Arm,
    epoch: &str,
    t: Arc<dyn PbsTransport + Send + Sync>,
    clock: Arc<FakeClock>,
) -> io::Result<PbsExt> {
    PbsExt::open(dir, spec, arm, &trial(arm), epoch, t, clock)
}

/// `qstat -x -f` 形式の原票。`edit` で欄を差し替える。
fn record(id: &str, s: &IntentSpec, state: &str, edit: &[(&str, &str)]) -> String {
    let mut kv: BTreeMap<String, String> = [
        ("Job_Name", s.job_name.clone()),
        ("Job_Owner", format!("{}@sirius-login1", s.owner)),
        ("job_state", state.into()),
        ("queue", s.queue.clone()),
        ("server", "sirius".into()),
        ("Account_Name", s.account.clone()),
        ("Rerunable", "False".into()),
        ("Resource_List.place", s.place.clone()),
        ("Resource_List.select", s.select.clone()),
        ("Resource_List.walltime", s.walltime.clone()),
        (
            "Variable_List",
            format!(
                "PBS_O_HOME=/home/{},CELERIS_TRIAL_KEY={},CELERIS_SUBMIT_INTENT={}",
                s.owner,
                s.identity.trial_key,
                s.submit_intent_id()
            ),
        ),
        (
            "Submit_arguments",
            format!("-q {} -A {} -r n {}", s.queue, s.account, s.script),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for (k, v) in edit {
        kv.insert((*k).into(), (*v).into());
    }
    let mut out = format!("Job Id: {id}\n");
    for (k, v) in kv {
        out.push_str(&format!("    {k} = {v}\n"));
    }
    out.push('\n');
    out
}

/// 投入された job を覚えて原票を返す偽 cluster。qstat に `finish_after` 回答えた job は `F` になる。
#[derive(Default)]
struct Cluster {
    inner: Mutex<ClusterState>,
}

#[derive(Default)]
struct ClusterState {
    finish_after: u32,
    jobs: BTreeMap<String, (QsubRequest, u32)>,
    qsub: u32,
    qstat: u32,
}

impl Cluster {
    fn new(finish_after: u32) -> Arc<Self> {
        let c = Self::default();
        c.inner.lock().expect("cluster").finish_after = finish_after;
        Arc::new(c)
    }
    fn qsubs(&self) -> u32 {
        self.inner.lock().expect("cluster").qsub
    }
    fn job_ids(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("cluster")
            .jobs
            .keys()
            .cloned()
            .collect()
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
        started_utc: "2026-10-25T00:00:00Z".into(),
        finished_utc: "2026-10-25T00:00:00Z".into(),
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
        let mut s = self.inner.lock().expect("cluster");
        s.qsub += 1;
        let id = format!("{}.sirius", 4300 + s.qsub);
        s.jobs.insert(id.clone(), (req.clone(), 0));
        reply(PbsCall::Qsub, argv, CallOutcome::Ok, format!("{id}\n"), "")
    }
    fn qstat_full(&self, job_id: &str) -> PbsReply {
        let argv = vec!["qstat".into(), "-x".into(), "-f".into(), job_id.into()];
        let mut s = self.inner.lock().expect("cluster");
        s.qstat += 1;
        let finish_after = s.finish_after;
        let Some((req, seen)) = s.jobs.get_mut(job_id) else {
            return reply(
                PbsCall::QstatFull,
                argv,
                CallOutcome::NonZero,
                String::new(),
                &format!("qstat: Unknown Job Id {job_id}"),
            );
        };
        *seen += 1;
        let state = if *seen > finish_after { "F" } else { "R" };
        let mut kv = vec![
            ("Job_Owner".to_string(), "rmaeda@sirius-login1".to_string()),
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
                    "CELERIS_TRIAL_KEY={},CELERIS_SUBMIT_INTENT={}",
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
        reply(PbsCall::QstatFull, argv, CallOutcome::Ok, out, "")
    }
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        reply(
            PbsCall::FindByMarker,
            vec!["find-by-marker".into(), m.submit_intent.clone()],
            CallOutcome::Unsupported,
            String::new(),
            "not confirmed",
        )
    }
}

fn consumed(dir: &Path) -> Vec<ConsumedRow> {
    let path = dir.join(CONSUMED_LEDGER);
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .expect("consumed")
        .lines()
        .map(|l| serde_json::from_str(l).expect("row"))
        .collect()
}

fn scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

fn baseline(dir: &Path) -> Baseline {
    let sc = load_scenario(&scenarios(), "k2-mini").expect("scenario");
    Baseline::open(
        sc,
        BaselineConfig {
            poll_secs: 30,
            ext_job_secs: 330,
        },
        &dir.join("state.json"),
    )
    .expect("baseline")
}

/// 資源・終了条件・attempt 上限は arm に依らず、qsub argv は identity の欄（-N と -v）以外一致する。
#[test]
fn pbs_backend_both_arms_build_the_same_qsub_from_one_spec() {
    let s = spec();
    let rb = s.qsub_request(Arm::Baseline, &trial(Arm::Baseline), "exp-uring");
    let rc = s.qsub_request(Arm::Celeris, &trial(Arm::Celeris), "exp-uring");
    assert_ne!(
        rb.submit_intent, rc.submit_intent,
        "identity differs per arm"
    );
    let strip = |r: &QsubRequest| {
        let argv = r.argv().expect("argv");
        let n = argv.iter().position(|a| a == "-N").expect("-N");
        let v = argv.iter().position(|a| a == "-v").expect("-v");
        argv.into_iter()
            .enumerate()
            .filter(|(i, _)| *i != n + 1 && *i != v + 1)
            .map(|(_, a)| a)
            .collect::<Vec<_>>()
    };
    assert_eq!(strip(&rb), strip(&rc));
    assert!(strip(&rb).contains(&"walltime=00:05:00".to_string()));
    assert!(strip(&rb).contains(&"select=1:ncpus=24:mem=124gb:ngpus=1".to_string()));
    assert_eq!(s.termination().poll_secs, 30);
    assert_eq!(s.termination().trial_deadline_secs, 1800);
    assert_eq!(s.workload_attempts(), 1);

    // 両 arm の backend が同じ測定 dir の pin で同じ spec hash を持ち、実際に送った要求も一致する。
    let dir = tempfile::tempdir().expect("dir");
    let clock = FakeClock::new();
    let fake = Arc::new(FakePbs::new());
    fake.script_qsub(Scripted::ok("101.sirius\n"));
    fake.script_qsub(Scripted::ok("102.sirius\n"));
    let mut b = open(
        dir.path(),
        s.clone(),
        Arm::Baseline,
        "e1",
        fake.clone(),
        clock.clone(),
    )
    .expect("baseline arm");
    let mut c = open(
        dir.path(),
        s.clone(),
        Arm::Celeris,
        "e1",
        fake.clone(),
        clock.clone(),
    )
    .expect("celeris arm");
    assert_eq!(b.spec_sha256(), c.spec_sha256());
    assert_eq!(b.spec_sha256(), s.sha256());
    b.submit("exp-uring", 0).expect("io").expect("submitted");
    assert!(b.record_consumed("exp-uring").expect("consume"));
    c.submit("exp-uring", 0).expect("io").expect("submitted");
    let sent = fake.submitted();
    assert_eq!(sent.len(), 2);
    assert_eq!(strip(&sent[0]), strip(&sent[1]));
    assert_eq!(sent[0], rb);
    assert_eq!(sent[1], rc);
    assert_eq!(fake.counts().qsub, 2);
}

/// 別の値の spec は pin と hash が合わず開けない。欄の欠落・未知の欄・attempt 上限 1 以外は読み込みエラー。
#[test]
fn pbs_backend_spec_hash_mismatch_and_invalid_specs_are_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let clock = FakeClock::new();
    let fake: Arc<FakePbs> = Arc::new(FakePbs::new());
    open(
        dir.path(),
        spec(),
        Arm::Baseline,
        "e1",
        fake.clone(),
        clock.clone(),
    )
    .expect("first");
    let mut other = spec();
    other.resources.walltime_secs = 600;
    let err = open(
        dir.path(),
        other,
        Arm::Celeris,
        "e1",
        fake.clone(),
        clock.clone(),
    )
    .err()
    .expect("refused");
    assert!(err.to_string().contains("differs from the pinned"), "{err}");

    let mut v = spec_json();
    v["limits"]["workload_attempts"] = 2.into();
    assert!(RecoverySpec::parse(&v.to_string()).is_err());
    let mut v = spec_json();
    v["limits"]
        .as_object_mut()
        .expect("obj")
        .remove("poll_secs");
    assert!(RecoverySpec::parse(&v.to_string()).is_err());
    let mut v = spec_json();
    v["retries"] = 3.into();
    assert!(RecoverySpec::parse(&v.to_string()).is_err());
    let mut v = spec_json();
    v["rerunnable"] = true.into();
    assert!(RecoverySpec::parse(&v.to_string()).is_err());
    assert_eq!(fake.counts().qsub, 0);
}

/// 受理後に controller を作り直す（新しい epoch・新しい backend）と、同じ job ID を qstat で再追跡し qsub しない。
/// 二重消費は消費台帳で拒否する。
#[test]
fn pbs_backend_restart_retracks_the_same_job_without_new_qsub() {
    let dir = tempfile::tempdir().expect("dir");
    let clock = FakeClock::new();
    let cluster = Cluster::new(1);
    let job = {
        let mut b = open(
            dir.path(),
            spec(),
            Arm::Baseline,
            "e1",
            cluster.clone(),
            clock.clone(),
        )
        .expect("open");
        let r = b.submit("exp-uring", 0).expect("io").expect("accepted");
        assert!(!r.deduped);
        r.physical_id
    };
    let mut b = open(
        dir.path(),
        spec(),
        Arm::Baseline,
        "e2",
        cluster.clone(),
        clock.clone(),
    )
    .expect("reopen");
    assert_eq!(
        b.lookup("exp-uring").expect("lookup"),
        Lookup::Known {
            physical_id: job.clone(),
            state: JobState::Running
        }
    );
    let r = b.submit("exp-uring", 0).expect("io").expect("same job");
    assert!(r.deduped);
    assert_eq!(r.physical_id, job);
    assert_eq!(cluster.qsubs(), 1, "no new qsub after restart");
    // advance は wall clock を poll 間隔まで進めてから qstat し、完了を通知する。
    let n = b.advance(30).expect("advance");
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].physical_id, job);
    assert!(b.record_consumed("exp-uring").expect("consume"));
    assert!(!b.record_consumed("exp-uring").expect("second consume"));
    let mut b = open(
        dir.path(),
        spec(),
        Arm::Baseline,
        "e3",
        cluster.clone(),
        clock.clone(),
    )
    .expect("reopen");
    assert!(!b.record_consumed("exp-uring").expect("after restart"));
    assert!(b.record_consumed("never-submitted").is_err());
    assert_eq!(consumed(dir.path()).len(), 1);
    assert_eq!(cluster.qsubs(), 1);
}

/// 既存の baseline を ExtSim と同じ trait 経路で PbsExt に差し替え、保存点ごとに controller を作り直しても
/// 外部 job ごとに物理 qsub 1 回・消費 1 回で完走する。
#[test]
fn pbs_backend_baseline_restart_at_every_save_point_never_resubmits() {
    // 1 回で完走する場合の保存回数。
    let total = {
        let dir = tempfile::tempdir().expect("dir");
        let cluster = Cluster::new(2);
        let mut ext = open(
            dir.path(),
            spec(),
            Arm::Baseline,
            "e0",
            cluster.clone(),
            FakeClock::new(),
        )
        .expect("open");
        let mut b = baseline(dir.path());
        assert_eq!(b.run(&mut ext).expect("run"), Outcome::Complete);
        assert_eq!(cluster.qsubs(), 2);
        b.saves()
    };
    assert!(total > 5);
    let mut stopped_while_external = 0;
    for k in 1..=total {
        let dir = tempfile::tempdir().expect("dir");
        let clock = FakeClock::new();
        let cluster = Cluster::new(2);
        {
            let mut ext = open(
                dir.path(),
                spec(),
                Arm::Baseline,
                "e1",
                cluster.clone(),
                clock.clone(),
            )
            .expect("open");
            let mut b = baseline(dir.path());
            b.stop_after_saves(k);
            if b.run(&mut ext).expect("run") == Outcome::Complete {
                continue;
            }
            if b.state()
                .nodes
                .values()
                .any(|n| matches!(n.status, Status::Submitted | Status::Waiting))
            {
                stopped_while_external += 1;
            }
        }
        // controller を作り直す（新しい epoch・新しい backend・state から再開）。
        let mut ext = open(
            dir.path(),
            spec(),
            Arm::Baseline,
            "e2",
            cluster.clone(),
            clock.clone(),
        )
        .expect("reopen");
        let mut b = baseline(dir.path());
        assert_eq!(
            b.run(&mut ext).expect("resume"),
            Outcome::Complete,
            "stop at {k}"
        );
        assert!(b.state().nodes.values().all(|n| n.status == Status::Done));
        assert_eq!(
            cluster.qsubs(),
            2,
            "stop at {k}: one physical qsub per external job"
        );
        let rows = consumed(dir.path());
        assert_eq!(rows.len(), 2, "stop at {k}: each job consumed once");
        let mut refs: Vec<String> = rows.iter().map(|r| r.job_ref.clone()).collect();
        refs.sort();
        let want: Vec<String> = cluster
            .job_ids()
            .iter()
            .map(|j| format!("pbs:sirius:{j}"))
            .collect();
        assert_eq!(refs, want, "stop at {k}");
    }
    assert!(
        stopped_while_external > 0,
        "some stop points fall while a job is submitted or waiting"
    );
}

/// qsub の応答喪失を marker で照合できない（候補 0 件のまま deadline）と、Unresolved で止まる。lookup も
/// Unknown（再投入の許可）を返さず、再度の submit も qsub しない。
#[test]
fn pbs_backend_unknown_submit_stops_without_resubmitting() {
    let dir = tempfile::tempdir().expect("dir");
    let clock = FakeClock::new();
    let fake = Arc::new(FakePbs::new());
    fake.script_qsub(Scripted::timeout(""));
    for _ in 0..3 {
        fake.script_find(Scripted::ok(""));
    }
    let mut b = open(
        dir.path(),
        spec(),
        Arm::Baseline,
        "e1",
        fake.clone(),
        clock.clone(),
    )
    .expect("open");
    let id = b.submit_intent_id("exp-uring");
    match b.submit("exp-uring", 0).expect("io") {
        Err(SubmitError::Unresolved {
            submit_intent_id,
            reason,
        }) => {
            assert_eq!(submit_intent_id, id);
            assert!(reason.contains("DeadlineReached"), "{reason}");
        }
        other => panic!("expected unresolved, got {other:?}"),
    }
    assert!(
        clock.now_unix() >= T0 + 1800,
        "reconciled until the deadline"
    );
    let mut b = open(
        dir.path(),
        spec(),
        Arm::Baseline,
        "e2",
        fake.clone(),
        clock.clone(),
    )
    .expect("reopen");
    assert!(matches!(
        b.lookup("exp-uring").expect("lookup"),
        Lookup::Unresolved { .. }
    ));
    assert!(matches!(
        b.submit("exp-uring", 0).expect("io"),
        Err(SubmitError::Unresolved { .. })
    ));
    assert_eq!(fake.counts().qsub, 1);
    // 未解決の intent は枠を戻さない（別の job も投入しない）。
    assert_eq!(
        b.submit("exp-posix", 0).expect("io"),
        Err(SubmitError::NoSlot)
    );
    assert_eq!(fake.counts().qsub, 1);
}

/// baseline も PbsExt の Unresolved を受けて Error で止まり、再起動しても再投入しない（node は Submitted のまま）。
#[test]
fn pbs_backend_baseline_stops_on_unresolved_submit() {
    let dir = tempfile::tempdir().expect("dir");
    let clock = FakeClock::new();
    let fake = Arc::new(FakePbs::new());
    fake.script_qsub(Scripted::timeout(""));
    fake.script_find(Scripted::unsupported());
    for epoch in ["e1", "e2"] {
        let mut ext = open(
            dir.path(),
            spec(),
            Arm::Baseline,
            epoch,
            fake.clone(),
            clock.clone(),
        )
        .expect("open");
        let mut b = baseline(dir.path());
        let err = b.run(&mut ext).expect_err("stops");
        assert!(format!("{err:?}").contains("unresolved"), "{err:?}");
        for id in ["exp-uring", "exp-posix"] {
            assert_ne!(b.state().nodes[id].status, Status::Done, "{id}");
        }
    }
    assert_eq!(fake.counts().qsub, 1, "one physical qsub in total");
    assert!(consumed(dir.path()).is_empty());
}

/// 受理後の履歴消失は Unknown（再投入）にせず Err（未観測）、原票が intent と食い違えば Unresolved。
#[test]
fn pbs_backend_history_loss_and_record_mismatch_are_not_unknown() {
    let dir = tempfile::tempdir().expect("dir");
    let clock = FakeClock::new();
    let fake = Arc::new(FakePbs::new());
    fake.script_qsub(Scripted::ok("101.sirius\n"));
    fake.script_qstat(Scripted::nonzero(153, "qstat: Unknown Job Id 101.sirius"));
    let mut b = open(
        dir.path(),
        spec(),
        Arm::Baseline,
        "e1",
        fake.clone(),
        clock.clone(),
    )
    .expect("open");
    let s = b.intent_spec("exp-uring");
    fake.script_qstat(Scripted::ok(&record(
        "101.sirius",
        &s,
        "R",
        &[("queue", "debug")],
    )));
    b.submit("exp-uring", 0).expect("io").expect("accepted");
    let err = b.lookup("exp-uring").expect_err("history missing");
    assert!(err.to_string().contains("history missing"), "{err}");
    match b.lookup("exp-uring").expect("lookup") {
        Lookup::Unresolved { reason, .. } => assert!(reason.contains("queue"), "{reason}"),
        other => panic!("expected unresolved, got {other:?}"),
    }
    assert_eq!(b.drain_replies().len(), 2, "qstat raw replies are kept");
    assert_eq!(fake.counts().qsub, 1);
}
