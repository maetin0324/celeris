//! 実 scheduler raw（`celeris.dagbench-scheduler-raw/1`、ADR 2026-10-25-recovery-scheduler-backend D3・実装付記 I6）。
//! 偽 transport で測定 dir を作り、writer が原票・成果物 hash・collector 分類を trial_key に結び、validator が
//! 不一致・改変・欠落を拒否すること、再収集が qsub も消費も起こさないこと、欠測を成功に補完しないことを確かめる。
//! ssh・qsub・外部 network は使わず、時間は注入時計で進める（sleep しない）。
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use celeris_campaign::dagbench::{
    durable,
    ext::{ExtBackend, SubmitError},
    metrics,
    pbs::{
        backend::{Arm, CONSUMED_LEDGER, PbsExt, RecoverySpec},
        intent::{IntentClock, IntentSpec},
        raw::{
            self, AttemptArtifacts, CollectorIdentity, EVENTS_FILE, GradeEvidence, RawWriter,
            StudyPins, TRIALS_FILE, TrialBinding, TrialInput, TrialStatus, config_of,
        },
        transport::{FakePbs, PbsTransport, QstatView, Scripted},
    },
};
use celeris_campaign::observe::CollectClass;
use sha2::{Digest, Sha256};

const T0: u64 = 1_800_000_000;
const OUTPUT: &[u8] = b"payload-output 17\n";
const LOGICAL: &str = "exp-uring";

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

fn spec() -> RecoverySpec {
    let v = serde_json::json!({
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

fn collector(arm: Arm) -> CollectorIdentity {
    CollectorIdentity {
        campaign_sha256: "f".repeat(64),
        config: config_of(arm),
        problem_id: "rcv-normal".into(),
        rep: 1,
        trial_no: 1,
    }
}

fn pins() -> StudyPins {
    StudyPins {
        campaign_id: "recovery-pbs-1".into(),
        manifest_sha256: "f".repeat(64),
        case_sha256: "1".repeat(64),
        source: "agent-platform@test".into(),
        binary_sha256: "2".repeat(64),
    }
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
        ("Exit_status", "0".into()),
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

/// 1 trial の測定（baseline arm）。`qsub` の台本で submit し、受理されたら 1 回消費する。
struct Measured {
    fake: Arc<FakePbs>,
    spec: IntentSpec,
    consumed: u32,
}

fn measure(dir: &Path, qsub: Scripted, finds: &[Scripted]) -> Measured {
    let clock = Arc::new(FakeClock(Mutex::new(T0)));
    let fake = Arc::new(FakePbs::new());
    fake.script_qsub(qsub);
    for f in finds {
        fake.script_find(f.clone());
    }
    let key = collector(Arm::Baseline).trial_key();
    let mut ext =
        PbsExt::open(dir, spec(), Arm::Baseline, &key, "m1", fake.clone(), clock).expect("open");
    let spec = ext.intent_spec(LOGICAL);
    let mut consumed = 0;
    match ext.submit(LOGICAL, 0).expect("io") {
        Ok(_) => {
            assert!(ext.record_consumed(LOGICAL).expect("consume"));
            consumed += 1;
        }
        Err(SubmitError::Unresolved { .. }) => {}
        Err(e) => panic!("unexpected submit error {e:?}"),
    }
    Measured {
        fake,
        spec,
        consumed,
    }
}

fn attempt(dir: &Path, content: &[u8]) -> AttemptArtifacts {
    let a = dir.join("attempt-1");
    fs::create_dir_all(&a).expect("mkdir");
    fs::write(a.join("out.bin"), content).expect("write");
    AttemptArtifacts {
        attempt: 1,
        dir: a,
        paths: vec!["out.bin".into()],
    }
}

fn grade(dir: &Path, passed: bool) -> GradeEvidence {
    let f = dir.join("grade.json");
    fs::write(&f, format!("{{\"passed\":{passed}}}")).expect("grade");
    GradeEvidence { file: f, passed }
}

fn input(art: AttemptArtifacts, grade: Option<GradeEvidence>) -> TrialInput {
    TrialInput {
        arm: Arm::Baseline,
        logical_job_id: LOGICAL.into(),
        collector: collector(Arm::Baseline),
        attempts: vec![art],
        grade,
    }
}

fn collect(dir: &Path, w: &mut RawWriter, t: &dyn PbsTransport, inp: &TrialInput) -> TrialBinding {
    let b = w
        .collect_trial(QstatView::new(t, "sirius"), inp)
        .expect("collect");
    w.write_run(pins()).expect("run.json");
    let _ = dir;
    b
}

fn lines(p: &Path) -> usize {
    fs::read_to_string(p).map_or(0, |t| t.lines().count())
}

fn missing_fields(b: &TrialBinding) -> Vec<&str> {
    b.missing.iter().map(|m| m.field.as_str()).collect()
}

/// 正常系の測定 dir（validated_completed まで）。
fn completed(dir: &Path) -> (Measured, TrialBinding) {
    let m = measure(dir, Scripted::ok("123.sirius"), &[]);
    m.fake
        .script_qstat(Scripted::ok(&record("123.sirius", &m.spec, "F", &[])));
    let mut w = RawWriter::create(dir, "obs-1").expect("raw");
    let inp = input(attempt(dir, OUTPUT), Some(grade(dir, true)));
    let b = collect(dir, &mut w, m.fake.as_ref(), &inp);
    (m, b)
}

/// 書き換えた結合行で trials.jsonl を置き換える（改ざんの再現）。
fn rewrite_trials(dir: &Path, f: impl Fn(&mut serde_json::Value)) {
    let p = dir.join(TRIALS_FILE);
    let text = fs::read_to_string(&p).expect("trials");
    let out: Vec<String> = text
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).expect("row");
            f(&mut v);
            v.to_string()
        })
        .collect();
    fs::write(&p, out.join("\n") + "\n").expect("rewrite");
}

#[test]
fn scheduler_raw_writer_binds_records_and_validator_accepts() {
    let dir = tempfile::tempdir().expect("dir");
    let (m, b) = completed(dir.path());
    assert_eq!(b.status, TrialStatus::ValidatedCompleted);
    assert_eq!(b.job_ref.as_deref(), Some("pbs:sirius:123.sirius"));
    assert_eq!(b.trial_key, collector(Arm::Baseline).trial_key());
    assert_eq!(b.collect_class, None);
    assert_eq!(b.exit_status, Some(0));
    assert!(b.missing.is_empty(), "{:?}", b.missing);
    let q = b.qstat.as_ref().expect("qstat ref");
    let raw_text =
        fs::read_to_string(dir.path().join(q.file.as_deref().expect("file"))).expect("read");
    assert_eq!(q.sha256.as_deref(), Some(sha(raw_text.as_bytes()).as_str()));
    assert!(!q.fetched_utc.is_empty());
    assert_eq!(b.target.queue, "gen");
    assert_eq!(b.target.account, "NBBG");
    let report = raw::validate(dir.path()).expect("valid raw");
    assert_eq!(report.run.schema, raw::SCHEDULER_RAW_SCHEMA);
    assert_eq!(report.run.dataset, "scheduler_measured");
    assert_eq!(report.run.clock, "wall");
    assert_eq!(report.run.llm_calls, 0);
    assert_eq!(report.run.real_jobs, 1);
    assert_eq!(report.trials.len(), 1);
    assert_eq!(m.fake.counts().qsub, 1);
    assert_eq!(m.consumed, 1);
    // 出力先は新規のみ。
    assert!(RawWriter::create(dir.path(), "obs-2").is_err());
}

#[test]
fn scheduler_raw_recollect_appends_without_qsub_or_consume() {
    let dir = tempfile::tempdir().expect("dir");
    let (m, first) = completed(dir.path());
    let consumed_before = fs::read(dir.path().join(CONSUMED_LEDGER)).expect("consumed");
    let first_file = first
        .qstat
        .as_ref()
        .and_then(|q| q.file.clone())
        .expect("file");
    let first_bytes = fs::read(dir.path().join(&first_file)).expect("raw");
    let before = m.fake.counts();
    let events_before = lines(&dir.path().join(EVENTS_FILE));

    m.fake
        .script_qstat(Scripted::ok(&record("123.sirius", &m.spec, "F", &[])));
    let mut w = RawWriter::reopen(dir.path(), "obs-2").expect("reopen");
    let inp = input(attempt(dir.path(), OUTPUT), Some(grade(dir.path(), true)));
    let again = w
        .collect_trial(QstatView::new(m.fake.as_ref(), "sirius"), &inp)
        .expect("recollect");

    let after = m.fake.counts();
    assert_eq!(after.qsub, before.qsub, "re-collection must not qsub");
    assert_eq!(after.qsub, 1);
    assert_eq!(after.find_by_marker, before.find_by_marker);
    assert_eq!(after.qstat_full, before.qstat_full + 1);
    assert_eq!(
        fs::read(dir.path().join(CONSUMED_LEDGER)).expect("consumed"),
        consumed_before,
        "re-collection must not record a consumption"
    );
    assert_eq!(lines(&dir.path().join(CONSUMED_LEDGER)), 1);
    assert_eq!(lines(&dir.path().join(EVENTS_FILE)), events_before + 1);
    assert_eq!(lines(&dir.path().join(TRIALS_FILE)), 2);
    assert_eq!(
        fs::read(dir.path().join(&first_file)).expect("raw"),
        first_bytes
    );
    let again_file = again
        .qstat
        .as_ref()
        .and_then(|q| q.file.clone())
        .expect("file");
    assert_ne!(again_file, first_file, "a new raw record is appended");
    let report = raw::validate(dir.path()).expect("valid after re-collection");
    assert_eq!(report.run.real_jobs, 1);
    assert_eq!(report.trials[&again.trial_key].seq, 2);
    assert_eq!(
        report.trials[&again.trial_key].status,
        TrialStatus::ValidatedCompleted
    );
}

#[test]
fn scheduler_raw_missing_evidence_is_null_with_reason_not_success() {
    // 履歴消失（gone）: 照会は届いたが scheduler が知らない。
    let dir = tempfile::tempdir().expect("dir");
    let m = measure(dir.path(), Scripted::ok("123.sirius"), &[]);
    m.fake
        .script_qstat(Scripted::nonzero(153, "qstat: Unknown Job Id 123.sirius"));
    let mut w = RawWriter::create(dir.path(), "obs").expect("raw");
    let inp = input(attempt(dir.path(), OUTPUT), Some(grade(dir.path(), true)));
    let b = collect(dir.path(), &mut w, m.fake.as_ref(), &inp);
    assert_eq!(b.status, TrialStatus::Unobserved);
    assert_eq!(b.collect_class, Some(CollectClass::Unobserved));
    assert_eq!((b.observed.clone(), b.exit_status), (None, None));
    assert!(missing_fields(&b).contains(&"observed"), "{:?}", b.missing);
    assert!(
        missing_fields(&b).contains(&"qstat.file"),
        "{:?}",
        b.missing
    );
    assert_eq!(
        raw::validate(dir.path()).expect("valid").trials[&b.trial_key].status,
        TrialStatus::Unobserved
    );

    // 取得失敗（timeout）: 原票は null と理由、分類は FetchFailed。
    let dir = tempfile::tempdir().expect("dir");
    let m = measure(dir.path(), Scripted::ok("123.sirius"), &[]);
    m.fake.script_qstat(Scripted::timeout(""));
    let mut w = RawWriter::create(dir.path(), "obs").expect("raw");
    let inp = input(attempt(dir.path(), OUTPUT), Some(grade(dir.path(), true)));
    let b = collect(dir.path(), &mut w, m.fake.as_ref(), &inp);
    assert_eq!(b.status, TrialStatus::Unobserved);
    assert_eq!(b.collect_class, Some(CollectClass::FetchFailed));
    let q = b.qstat.as_ref().expect("event ref");
    assert_eq!((q.file.clone(), q.sha256.clone()), (None, None));
    assert!(missing_fields(&b).contains(&"qstat.file"));
    raw::validate(dir.path()).expect("valid");

    // 採点入力欠落: 実行終了・成果物一致でも完遂にしない。
    let dir = tempfile::tempdir().expect("dir");
    let m = measure(dir.path(), Scripted::ok("123.sirius"), &[]);
    m.fake
        .script_qstat(Scripted::ok(&record("123.sirius", &m.spec, "F", &[])));
    let mut w = RawWriter::create(dir.path(), "obs").expect("raw");
    let b = collect(
        dir.path(),
        &mut w,
        m.fake.as_ref(),
        &input(attempt(dir.path(), OUTPUT), None),
    );
    assert_eq!(b.exit_status, Some(0));
    assert_eq!(b.grade, None);
    assert!(missing_fields(&b).contains(&"grade"));
    assert_eq!(b.status, TrialStatus::ExecutedUnvalidated);
    raw::validate(dir.path()).expect("valid");

    // F/Exit_status=0 と grade 合格でも、成果物が期待出力と違えば完遂にしない。
    let dir = tempfile::tempdir().expect("dir");
    let m = measure(dir.path(), Scripted::ok("123.sirius"), &[]);
    m.fake
        .script_qstat(Scripted::ok(&record("123.sirius", &m.spec, "F", &[])));
    let mut w = RawWriter::create(dir.path(), "obs").expect("raw");
    let inp = input(
        attempt(dir.path(), b"wrong output\n"),
        Some(grade(dir.path(), true)),
    );
    let b = collect(dir.path(), &mut w, m.fake.as_ref(), &inp);
    assert_eq!(b.exit_status, Some(0));
    assert!(!b.output_matches_expected);
    assert_eq!(b.status, TrialStatus::ExecutedUnvalidated);
    raw::validate(dir.path()).expect("valid");

    // 照合できない投入（応答喪失・検索未確認）: not_submitted にせず Unresolved、qstat も呼ばない。
    let dir = tempfile::tempdir().expect("dir");
    let m = measure(
        dir.path(),
        Scripted::timeout(""),
        &[Scripted::unsupported()],
    );
    assert_eq!(m.consumed, 0);
    let mut w = RawWriter::create(dir.path(), "obs").expect("raw");
    let inp = input(attempt(dir.path(), OUTPUT), Some(grade(dir.path(), true)));
    let b = collect(dir.path(), &mut w, m.fake.as_ref(), &inp);
    assert_eq!(b.status, TrialStatus::Unresolved);
    assert_eq!(b.collect_class, Some(CollectClass::Unobserved));
    assert_ne!(b.collect_class, Some(CollectClass::NotSubmitted));
    assert_eq!(b.intent_state.as_deref(), Some("unresolved_stop"));
    assert_eq!(b.job_ref, None);
    assert!(missing_fields(&b).contains(&"job_ref"));
    assert_eq!(m.fake.counts().qstat_full, 0);
    assert_eq!(m.fake.counts().qsub, 1);
    let report = raw::validate(dir.path()).expect("valid");
    assert_eq!(report.run.real_jobs, 0);
}

#[test]
fn scheduler_raw_rejects_collector_mismatch() {
    // 原票の marker が別 trial: collector が IdentityMismatch に分類し、成功にしない。
    let dir = tempfile::tempdir().expect("dir");
    let m = measure(dir.path(), Scripted::ok("123.sirius"), &[]);
    let other = format!(
        "PBS_O_HOME=/home/rmaeda,CELERIS_TRIAL_KEY=campaign-other,CELERIS_SUBMIT_INTENT={}",
        m.spec.submit_intent_id()
    );
    m.fake.script_qstat(Scripted::ok(&record(
        "123.sirius",
        &m.spec,
        "F",
        &[("Variable_List", &other)],
    )));
    let mut w = RawWriter::create(dir.path(), "obs").expect("raw");
    let inp = input(attempt(dir.path(), OUTPUT), Some(grade(dir.path(), true)));
    let b = collect(dir.path(), &mut w, m.fake.as_ref(), &inp);
    assert_eq!(b.collect_class, Some(CollectClass::IdentityMismatch));
    assert_eq!(b.status, TrialStatus::Unobserved);

    // 副記録 observed-jobs/1 が別 job を指す（hash も合わせた改ざん）→ 拒否。
    let dir = tempfile::tempdir().expect("dir");
    let (_m, b) = completed(dir.path());
    let obs = b.observed.clone().expect("observed");
    let p = dir.path().join(&obs.file);
    let mut doc: serde_json::Value =
        serde_json::from_slice(&fs::read(&p).expect("obs")).expect("json");
    doc["jobs"][0]["job_id"] = "999.sirius".into();
    let body = doc.to_string();
    fs::write(&p, &body).expect("write");
    let new_sha = sha(body.as_bytes());
    rewrite_trials(dir.path(), |v| {
        v["observed"]["sha256"] = new_sha.clone().into()
    });
    let e = raw::validate(dir.path()).expect_err("collector mismatch");
    assert!(e.contains("collector record"), "{e}");

    // 副記録 submit-audit/1 に台帳に無い job → 拒否。
    let dir = tempfile::tempdir().expect("dir");
    let (_m, b) = completed(dir.path());
    let extra = serde_json::json!({
        "schema": "celeris.submit-audit/1", "trial_key": b.trial_key,
        "scheduler": "pbs", "cluster": "sirius", "job_id": "124.sirius"
    });
    let ap = dir.path().join("submit-audit.jsonl");
    let mut text = fs::read_to_string(&ap).expect("audit");
    text.push_str(&format!("{extra}\n"));
    fs::write(&ap, text).expect("write");
    let e = raw::validate(dir.path()).expect_err("audit mismatch");
    assert!(e.contains("submit audit"), "{e}");

    // collector 分類を消して成功に書き換え → 証拠と合わず拒否。
    let dir = tempfile::tempdir().expect("dir");
    let m = measure(dir.path(), Scripted::ok("123.sirius"), &[]);
    m.fake
        .script_qstat(Scripted::nonzero(153, "qstat: Unknown Job Id 123.sirius"));
    let mut w = RawWriter::create(dir.path(), "obs").expect("raw");
    let inp = input(attempt(dir.path(), OUTPUT), Some(grade(dir.path(), true)));
    collect(dir.path(), &mut w, m.fake.as_ref(), &inp);
    rewrite_trials(dir.path(), |v| v["status"] = "validated_completed".into());
    let e = raw::validate(dir.path()).expect_err("forged success");
    assert!(e.contains("not supported by the evidence"), "{e}");

    // trial_key と collector identity の不一致（job ID/trial 不一致）→ 拒否。
    let dir = tempfile::tempdir().expect("dir");
    completed(dir.path());
    rewrite_trials(dir.path(), |v| {
        v["collector"]["problem_id"] = "other".into()
    });
    let e = raw::validate(dir.path()).expect_err("trial mismatch");
    assert!(e.contains("job ID/trial mismatch"), "{e}");

    // job_ref が台帳と違う → 拒否。
    let dir = tempfile::tempdir().expect("dir");
    completed(dir.path());
    rewrite_trials(dir.path(), |v| {
        v["job_ref"] = "pbs:sirius:124.sirius".into()
    });
    let e = raw::validate(dir.path()).expect_err("job mismatch");
    assert!(e.contains("job ID/trial mismatch"), "{e}");
}

#[test]
fn scheduler_raw_rejects_altered_or_missing_records() {
    // qstat 原票の 1 byte 改変。
    let dir = tempfile::tempdir().expect("dir");
    let (_m, b) = completed(dir.path());
    let f = dir
        .path()
        .join(b.qstat.as_ref().and_then(|q| q.file.clone()).expect("file"));
    let mut bytes = fs::read(&f).expect("raw");
    bytes[0] ^= 1;
    fs::write(&f, bytes).expect("write");
    let e = raw::validate(dir.path()).expect_err("altered");
    assert!(e.contains("altered"), "{e}");

    // 原票の欠落。
    let dir = tempfile::tempdir().expect("dir");
    let (_m, b) = completed(dir.path());
    fs::remove_file(
        dir.path()
            .join(b.qstat.as_ref().and_then(|q| q.file.clone()).expect("file")),
    )
    .expect("rm");
    let e = raw::validate(dir.path()).expect_err("missing");
    assert!(e.contains("missing"), "{e}");

    // 成果物の改変。
    let dir = tempfile::tempdir().expect("dir");
    completed(dir.path());
    fs::write(dir.path().join("attempt-1/out.bin"), b"changed").expect("write");
    let e = raw::validate(dir.path()).expect_err("artifact altered");
    assert!(e.contains("artifact"), "{e}");

    // 実 job 数・LLM calls の偽り。
    let dir = tempfile::tempdir().expect("dir");
    completed(dir.path());
    let run = dir.path().join(raw::RUN_FILE);
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(&run).expect("run")).expect("json");
    v["real_jobs"] = 2.into();
    fs::write(&run, v.to_string()).expect("write");
    let e = raw::validate(dir.path()).expect_err("real_jobs");
    assert!(e.contains("real_jobs"), "{e}");
    v["real_jobs"] = 1.into();
    v["llm_calls"] = 1.into();
    fs::write(&run, v.to_string()).expect("write");
    let e = raw::validate(dir.path()).expect_err("llm");
    assert!(e.contains("LLM"), "{e}");
}

#[test]
fn scheduler_raw_is_not_read_by_durable_reader_or_metrics() {
    let dir = tempfile::tempdir().expect("dir");
    completed(dir.path());
    let e = metrics::compute(dir.path()).expect_err("metrics must not read scheduler raw");
    assert!(e.contains(raw::SCHEDULER_RAW_SCHEMA), "{e}");
    let e = durable::verify(dir.path()).expect_err("durable must not read scheduler raw");
    assert!(e.contains(raw::SCHEDULER_RAW_SCHEMA), "{e}");

    // 旧 durable raw の run.json は本 validator が読まない。
    let run = dir.path().join(raw::RUN_FILE);
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(&run).expect("run")).expect("json");
    v["schema"] = metrics::DURABLE_RAW_SCHEMA.into();
    fs::write(&run, v.to_string()).expect("write");
    let e = raw::validate(dir.path()).expect_err("durable raw");
    assert!(e.contains(metrics::DURABLE_RAW_SCHEMA), "{e}");
    assert!(RawWriter::reopen(dir.path(), "obs").is_err());
}
