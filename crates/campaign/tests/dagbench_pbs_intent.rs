//! recovery の submit intent 台帳（ADR 2026-10-25-recovery-scheduler-backend D4・実装付記 I3）。
//! 偽 transport の台本で、応答喪失・0/1/複数候補・履歴消失・不一致・timeout・再起動・二重 submit 競合を確かめ、
//! どの場合も物理 qsub が 1 回以下であることを数える。ssh・qsub・外部 network は使わず、時間は注入時計で進める。
use std::{
    fs,
    io::Write,
    path::Path,
    sync::{Arc, Mutex, mpsc},
};

use celeris_campaign::{
    dagbench::pbs::{
        intent::{
            INTENT_LEDGER, IntentClock, IntentError, IntentEvent, IntentLedger, IntentSpec,
            IntentState, RetryCause, SubmitIdentity, SubmitOutcome, UnknownCause, UnresolvedReason,
        },
        transport::{
            FakePbs, MarkerQuery, PbsReply, PbsTransport, QsubRequest, Scripted, UnknownReason,
        },
    },
    observe::Origin,
};

const T0: u64 = 1_800_000_000;

/// sleep せず、要求された時刻へ跳ぶ時計。待った時刻を残す。
struct FakeClock {
    now: Mutex<u64>,
    sleeps: Mutex<Vec<u64>>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            now: Mutex::new(T0),
            sleeps: Mutex::new(Vec::new()),
        }
    }
    fn set(&self, t: u64) {
        *self.now.lock().expect("clock") = t;
    }
    fn sleeps(&self) -> Vec<u64> {
        self.sleeps.lock().expect("clock").clone()
    }
}

impl IntentClock for FakeClock {
    fn now_unix(&self) -> u64 {
        *self.now.lock().expect("clock")
    }
    fn sleep_until(&self, t: u64) {
        self.sleeps.lock().expect("clock").push(t);
        let mut n = self.now.lock().expect("clock");
        *n = (*n).max(t);
    }
}

fn identity(node: &str, attempt: u32) -> SubmitIdentity {
    SubmitIdentity {
        measurement_id: "rcv-m1".into(),
        arm: "baseline".into(),
        trial_key: "rcv-m1.baseline.normal.r1".into(),
        logical_job_id: "j1".into(),
        node: node.into(),
        workload_attempt: attempt,
    }
}

fn spec() -> IntentSpec {
    IntentSpec {
        identity: identity("n1", 1),
        owner: "rmaeda".into(),
        cluster: "sirius".into(),
        queue: "gen".into(),
        account: "NBBG".into(),
        select: "1:ncpus=24:mem=124gb:ngpus=1".into(),
        place: "exclhost".into(),
        walltime: "00:05:00".into(),
        job_name: "rcv-n1".into(),
        script: "/work/NBB/rmaeda/recovery/job.sh".into(),
        payload_sha256: "b".repeat(64),
        poll_secs: 30,
        trial_deadline_secs: 1800,
    }
}

/// `qstat -x -f` 形式の原票。`edit` で欄を差し替える。
fn record(id: &str, s: &IntentSpec, edit: &[(&str, &str)]) -> String {
    let mut kv: Vec<(String, String)> = vec![
        ("Job_Name".into(), s.job_name.clone()),
        ("Job_Owner".into(), format!("{}@sirius-login1", s.owner)),
        ("job_state".into(), "R".into()),
        ("queue".into(), s.queue.clone()),
        ("server".into(), "sirius".into()),
        ("Account_Name".into(), s.account.clone()),
        ("Rerunable".into(), "False".into()),
        ("Resource_List.place".into(), s.place.clone()),
        ("Resource_List.select".into(), s.select.clone()),
        ("Resource_List.walltime".into(), s.walltime.clone()),
        (
            "Variable_List".into(),
            format!(
                "PBS_O_HOME=/home/{},CELERIS_TRIAL_KEY={},CELERIS_SUBMIT_INTENT={}",
                s.owner,
                s.identity.trial_key,
                s.submit_intent_id()
            ),
        ),
        (
            "Submit_arguments".into(),
            format!("-q {} -A {} -r n {}", s.queue, s.account, s.script),
        ),
    ];
    for (k, v) in edit {
        match kv.iter_mut().find(|(kk, _)| kk == k) {
            Some(e) if v.is_empty() => e.0 = String::new(),
            Some(e) => e.1 = (*v).into(),
            None => kv.push(((*k).into(), (*v).into())),
        }
    }
    let mut out = format!("Job Id: {id}\n");
    for (k, v) in kv.into_iter().filter(|(k, _)| !k.is_empty()) {
        out.push_str(&format!("    {k} = {v}\n"));
    }
    out.push('\n');
    out
}

fn ledger(dir: &Path) -> IntentLedger {
    IntentLedger::open(dir, "epoch-1").expect("ledger")
}

fn states(dir: &Path) -> Vec<String> {
    ledger(dir)
        .rows()
        .expect("rows")
        .iter()
        .map(|r| {
            serde_json::to_value(&r.event).expect("json")["state"]
                .as_str()
                .expect("state")
                .to_string()
        })
        .collect()
}

fn unresolved(o: &SubmitOutcome) -> UnresolvedReason {
    match o {
        SubmitOutcome::Unresolved { reason, .. } => *reason,
        other => panic!("expected unresolved, got {other:?}"),
    }
}

/// qsub の中で台帳を読み、intent が先に永続化されていることを確かめる transport。
struct CheckBeforeQsub<'a> {
    inner: &'a FakePbs,
    dir: &'a Path,
    seen: Mutex<Vec<String>>,
}

impl PbsTransport for CheckBeforeQsub<'_> {
    fn origin(&self) -> Origin {
        self.inner.origin()
    }
    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        // lock の外から file を直接読む（同じ lock を取ると自分が待つ）。
        let text = fs::read_to_string(self.dir.join(INTENT_LEDGER)).expect("ledger on disk");
        self.seen.lock().expect("seen").extend(
            text.lines()
                .map(|l| {
                    serde_json::from_str::<serde_json::Value>(l).expect("row")["state"]
                        .as_str()
                        .expect("state")
                        .to_string()
                })
                .collect::<Vec<_>>(),
        );
        self.inner.qsub(req)
    }
    fn qstat_full(&self, job_id: &str) -> PbsReply {
        self.inner.qstat_full(job_id)
    }
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        self.inner.find_by_marker(m)
    }
}

#[test]
fn submit_intent_id_is_deterministic_and_separates_node_and_attempt() {
    let a = identity("n1", 1).submit_intent_id();
    assert_eq!(a, identity("n1", 1).submit_intent_id());
    assert!(a.starts_with("si-") && a.len() == 3 + 64, "{a}");
    assert_ne!(a, identity("n2", 1).submit_intent_id());
    assert_ne!(a, identity("n1", 2).submit_intent_id());
    let req = spec().request();
    assert_eq!(req.submit_intent, spec().submit_intent_id());
    assert_eq!(req.trial_key, "rcv-m1.baseline.normal.r1");
}

#[test]
fn intent_is_synced_before_qsub_and_accepted_job_is_recorded() {
    let d = tempfile::tempdir().expect("dir");
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::ok("4321.sirius\n"));
    let t = CheckBeforeQsub {
        inner: &fake,
        dir: d.path(),
        seen: Mutex::new(Vec::new()),
    };
    let clock = FakeClock::new();
    let out = ledger(d.path())
        .submit(&spec(), &t, &clock)
        .expect("submit");
    assert_eq!(
        out,
        SubmitOutcome::Accepted {
            job_id: "4321.sirius".into(),
            job_ref: "pbs:sirius:4321.sirius".into()
        }
    );
    assert_eq!(*t.seen.lock().expect("seen"), ["intent_recorded", "sent"]);
    assert_eq!(states(d.path()), ["intent_recorded", "sent", "accepted"]);
    assert_eq!(fake.counts().qsub, 1);
    assert_eq!(fake.counts().find_by_marker, 0);
    // 同じ intent をもう一度投入しても送らず再生する。
    let again = ledger(d.path())
        .submit(&spec(), &fake, &clock)
        .expect("again");
    assert_eq!(again, out);
    assert_eq!(fake.counts().qsub, 1);
    let rows = ledger(d.path()).rows().expect("rows");
    assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(rows.iter().all(|r| r.epoch == "epoch-1"));
    let IntentEvent::IntentRecorded {
        resource_sha256,
        deadline_unix,
        ..
    } = &rows[0].event
    else {
        panic!("first row")
    };
    assert_eq!(*resource_sha256, spec().resource_sha256());
    assert_eq!(*deadline_unix, T0 + 1800);
}

#[test]
fn lost_reply_with_one_matching_candidate_is_reconciled() {
    let d = tempfile::tempdir().expect("dir");
    let s = spec();
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::timeout(""))
        .script_find(Scripted::ok(&record("77.sirius", &s, &[])))
        .script_qstat(Scripted::ok(&record("77.sirius", &s, &[])));
    let out = ledger(d.path())
        .submit(&s, &fake, &FakeClock::new())
        .expect("submit");
    assert_eq!(
        out,
        SubmitOutcome::Reconciled {
            job_id: "77.sirius".into(),
            job_ref: "pbs:sirius:77.sirius".into()
        }
    );
    assert_eq!(
        states(d.path()),
        ["intent_recorded", "sent", "unknown_submit", "reconciled"]
    );
    let rows = ledger(d.path()).rows().expect("rows");
    let IntentEvent::UnknownSubmit { cause, .. } = &rows[2].event else {
        panic!("unknown")
    };
    assert_eq!(*cause, UnknownCause::Qsub(UnknownReason::Timeout));
    let IntentEvent::Reconciled {
        replies,
        payload_sha256,
        ..
    } = &rows[3].event
    else {
        panic!("reconciled")
    };
    assert_eq!(replies.len(), 2, "find + qstat raw replies are kept");
    assert_eq!(*payload_sha256, s.payload_sha256);
    let c = fake.counts();
    assert_eq!((c.qsub, c.find_by_marker, c.qstat_full), (1, 1, 1));
    // 再投入しても qsub は増えない。
    let again = ledger(d.path())
        .submit(&s, &fake, &FakeClock::new())
        .expect("again");
    assert_eq!(again, out);
    assert_eq!(fake.counts().qsub, 1);
}

#[test]
fn invalid_or_disconnected_qsub_reply_is_unknown_not_unsubmitted() {
    for (qsub, reason) in [
        (Scripted::ok("12345\n"), UnknownReason::InvalidId),
        (Scripted::ok(""), UnknownReason::InvalidId),
        (
            Scripted::nonzero(255, "Connection closed"),
            UnknownReason::Disconnected,
        ),
        (Scripted::nonzero(1, "qsub: error"), UnknownReason::NonZero),
    ] {
        let d = tempfile::tempdir().expect("dir");
        let s = spec();
        let fake = FakePbs::new();
        fake.script_qsub(qsub)
            .script_find(Scripted::ok(&record("9.sirius", &s, &[])))
            .script_qstat(Scripted::ok(&record("9.sirius", &s, &[])));
        let out = ledger(d.path())
            .submit(&s, &fake, &FakeClock::new())
            .expect("submit");
        assert!(matches!(out, SubmitOutcome::Reconciled { .. }), "{out:?}");
        let rows = ledger(d.path()).rows().expect("rows");
        assert!(
            matches!(&rows[2].event, IntentEvent::UnknownSubmit { cause, .. } if *cause == UnknownCause::Qsub(reason)),
            "{:?}",
            rows[2].event
        );
        assert_eq!(fake.counts().qsub, 1);
    }
}

#[test]
fn zero_candidates_recheck_every_poll_until_deadline_then_stop() {
    let d = tempfile::tempdir().expect("dir");
    let s = spec();
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::timeout(""));
    for _ in 0..100 {
        fake.script_find(Scripted::ok(""));
    }
    let clock = FakeClock::new();
    let l = ledger(d.path());
    let first = l.submit(&s, &fake, &clock).expect("submit");
    assert_eq!(
        first,
        SubmitOutcome::Pending {
            next_check_unix: T0 + 30,
            cause: RetryCause::NoCandidate
        }
    );
    // 期限前の照合要求では照会しない。
    clock.set(T0 + 29);
    let early = l
        .reconcile(&s.submit_intent_id(), &fake, &clock)
        .expect("early");
    assert!(matches!(early, SubmitOutcome::Pending { .. }), "{early:?}");
    assert_eq!(fake.counts().find_by_marker, 1);
    let out = l
        .resolve(&s.submit_intent_id(), &fake, &clock)
        .expect("resolve");
    assert_eq!(unresolved(&out), UnresolvedReason::DeadlineReached);
    // T0 で 1 回、以後 30 秒ごとに deadline（T0+1800）未満の T0+30..=T0+1770 で 59 回。
    assert_eq!(fake.counts().find_by_marker, 60);
    let sleeps = clock.sleeps();
    assert_eq!(sleeps.len(), 60);
    assert!(
        sleeps
            .iter()
            .enumerate()
            .all(|(i, t)| *t == T0 + 30 * (i as u64 + 1)),
        "{sleeps:?}"
    );
    assert_eq!(fake.counts().qsub, 1);
    assert_eq!(
        states(d.path()).last().map(String::as_str),
        Some("unresolved_stop")
    );
    // 停止後は qsub も照会もしない。
    let after = l.submit(&s, &fake, &clock).expect("after");
    assert_eq!(unresolved(&after), UnresolvedReason::DeadlineReached);
    assert_eq!(fake.counts().qsub, 1);
    assert_eq!(fake.counts().find_by_marker, 60);
}

#[test]
fn query_timeouts_retry_within_deadline_and_keep_raw_replies() {
    let d = tempfile::tempdir().expect("dir");
    let s = IntentSpec {
        trial_deadline_secs: 100,
        ..spec()
    };
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::timeout("partial"));
    for _ in 0..10 {
        fake.script_find(Scripted::timeout(""));
    }
    let clock = FakeClock::new();
    let l = ledger(d.path());
    l.submit(&s, &fake, &clock).expect("submit");
    let out = l
        .resolve(&s.submit_intent_id(), &fake, &clock)
        .expect("resolve");
    assert_eq!(unresolved(&out), UnresolvedReason::DeadlineReached);
    // T0, T0+30, T0+60, T0+90（T0+120 は deadline 後）。
    assert_eq!(fake.counts().find_by_marker, 4);
    assert_eq!(fake.counts().qsub, 1);
    let rows = l.rows().expect("rows");
    let retries: Vec<_> = rows
        .iter()
        .filter_map(|r| match &r.event {
            IntentEvent::ReconcileRetry { cause, replies, .. } => Some((*cause, replies.len())),
            _ => None,
        })
        .collect();
    assert_eq!(retries, vec![(RetryCause::QueryUnavailable, 1); 4]);
    let IntentEvent::UnknownSubmit { reply, .. } = &rows[2].event else {
        panic!("unknown")
    };
    assert_eq!(reply.as_ref().map(|r| r.stdout.as_str()), Some("partial"));
}

#[test]
fn multiple_candidates_stop_without_resubmit() {
    let d = tempfile::tempdir().expect("dir");
    let s = spec();
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::timeout(""))
        .script_find(Scripted::ok(&format!(
            "{}{}",
            record("10.sirius", &s, &[]),
            record("11.sirius", &s, &[])
        )));
    let out = ledger(d.path())
        .submit(&s, &fake, &FakeClock::new())
        .expect("submit");
    assert_eq!(unresolved(&out), UnresolvedReason::MultipleCandidates);
    let c = fake.counts();
    assert_eq!((c.qsub, c.qstat_full), (1, 0));
}

#[test]
fn single_candidate_with_any_mismatch_stops() {
    let s = spec();
    let edits: &[&[(&str, &str)]] = &[
        &[("queue", "debug")],
        &[("Account_Name", "OTHER")],
        &[("Resource_List.select", "1:ncpus=4")],
        &[("Resource_List.place", "excl")],
        &[("Resource_List.walltime", "00:30:00")],
        &[("Rerunable", "True")],
        &[("server", "pegasus")],
        &[("Submit_arguments", "-q gen /work/NBB/rmaeda/other.sh")],
        &[("Submit_arguments", "")],
        &[("Account_Name", "")],
    ];
    for edit in edits {
        let d = tempfile::tempdir().expect("dir");
        let fake = FakePbs::new();
        fake.script_qsub(Scripted::timeout(""))
            .script_find(Scripted::ok(&record("5.sirius", &s, &[])))
            .script_qstat(Scripted::ok(&record("5.sirius", &s, edit)));
        let out = ledger(d.path())
            .submit(&s, &fake, &FakeClock::new())
            .expect("submit");
        assert_eq!(unresolved(&out), UnresolvedReason::Mismatch, "{edit:?}");
        assert_eq!(fake.counts().qsub, 1);
    }
    // owner 違いは marker 検索の段で候補から外れる（0 件 → 再照合待ち）。
    let d = tempfile::tempdir().expect("dir");
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::timeout(""))
        .script_find(Scripted::ok(&record(
            "5.sirius",
            &s,
            &[("Job_Owner", "other@sirius-login1")],
        )));
    let out = ledger(d.path())
        .submit(&s, &fake, &FakeClock::new())
        .expect("submit");
    assert!(
        matches!(
            out,
            SubmitOutcome::Pending {
                cause: RetryCause::NoCandidate,
                ..
            }
        ),
        "{out:?}"
    );
    assert_eq!(fake.counts().qsub, 1);
}

#[test]
fn history_loss_refusal_and_unsupported_search_stop() {
    let s = spec();
    let cases: Vec<(Vec<Scripted>, Vec<Scripted>, UnresolvedReason)> = vec![
        (
            vec![Scripted::ok(&record("5.sirius", &s, &[]))],
            vec![Scripted::nonzero(153, "qstat: Unknown Job Id 5.sirius")],
            UnresolvedReason::HistoryMissing,
        ),
        (
            vec![Scripted::nonzero(1, "qselect: Unauthorized Request")],
            vec![],
            UnresolvedReason::QueryRefused,
        ),
        (
            vec![Scripted::ok(&record("5.sirius", &s, &[]))],
            vec![Scripted::nonzero(1, "qstat: Unauthorized Request")],
            UnresolvedReason::QueryRefused,
        ),
        (
            vec![Scripted::unsupported()],
            vec![],
            UnresolvedReason::Unsupported,
        ),
        (
            vec![Scripted::ok("not a record\n")],
            vec![],
            UnresolvedReason::InvalidReply,
        ),
    ];
    for (find, qstat, want) in cases {
        let d = tempfile::tempdir().expect("dir");
        let fake = FakePbs::new();
        fake.script_qsub(Scripted::timeout(""));
        for f in find {
            fake.script_find(f);
        }
        for q in qstat {
            fake.script_qstat(q);
        }
        let out = ledger(d.path())
            .submit(&s, &fake, &FakeClock::new())
            .expect("submit");
        assert_eq!(unresolved(&out), want);
        assert_eq!(fake.counts().qsub, 1);
        let rows = ledger(d.path()).rows().expect("rows");
        let IntentEvent::UnresolvedStop { replies, .. } = &rows.last().expect("row").event else {
            panic!("stop")
        };
        assert!(!replies.is_empty(), "raw replies are kept for {want:?}");
    }
}

#[test]
fn rejected_before_send_is_recorded_and_never_resent() {
    let d = tempfile::tempdir().expect("dir");
    let s = IntentSpec {
        queue: "-bad".into(),
        ..spec()
    };
    let fake = FakePbs::new();
    let clock = FakeClock::new();
    let out = ledger(d.path()).submit(&s, &fake, &clock).expect("submit");
    assert_eq!(unresolved(&out), UnresolvedReason::RejectedBeforeSend);
    assert_eq!(
        states(d.path()),
        ["intent_recorded", "sent", "rejected_before_send"]
    );
    ledger(d.path()).submit(&s, &fake, &clock).expect("again");
    assert_eq!(fake.counts().qsub, 0);
    assert_eq!(states(d.path()).len(), 3);
}

#[test]
fn same_intent_with_other_resources_is_a_conflict_without_qsub() {
    let d = tempfile::tempdir().expect("dir");
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::ok("1.sirius"));
    let clock = FakeClock::new();
    ledger(d.path())
        .submit(&spec(), &fake, &clock)
        .expect("first");
    for other in [
        IntentSpec {
            walltime: "00:10:00".into(),
            ..spec()
        },
        IntentSpec {
            payload_sha256: "c".repeat(64),
            ..spec()
        },
    ] {
        let err = ledger(d.path()).submit(&other, &fake, &clock).unwrap_err();
        assert!(matches!(err, IntentError::Conflict(_)), "{err}");
    }
    assert_eq!(fake.counts().qsub, 1);
}

/// qsub の途中で owner が止まる transport（panic で process 停止を模す）。
struct DiesInQsub<'a>(&'a FakePbs);

impl PbsTransport for DiesInQsub<'_> {
    fn origin(&self) -> Origin {
        Origin::Synthetic
    }
    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        let _ = self.0.qsub(req);
        panic!("owner stopped while qsub was in flight");
    }
    fn qstat_full(&self, job_id: &str) -> PbsReply {
        self.0.qstat_full(job_id)
    }
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        self.0.find_by_marker(m)
    }
}

#[test]
fn restart_after_send_or_before_send_resumes_without_resubmit() {
    let s = spec();
    let after = tempfile::tempdir().expect("dir");
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::ok("88.sirius"));
    let p = after.path().to_path_buf();
    let s2 = s.clone();
    let died = std::thread::scope(|sc| {
        sc.spawn(|| {
            ledger(&p)
                .submit(&s2, &DiesInQsub(&fake), &FakeClock::new())
                .map(|_| ())
        })
        .join()
    });
    assert!(died.is_err(), "owner died in qsub");
    assert_eq!(fake.counts().qsub, 1);
    assert_eq!(states(after.path()), ["intent_recorded", "sent"]);
    let status = ledger(after.path()).replay().expect("replay");
    assert_eq!(status[&s.submit_intent_id()].state, IntentState::Sent);

    // 送信前に止まった台帳（intent_recorded だけ）と壊れた末尾行。
    let before = tempfile::tempdir().expect("dir");
    let text = fs::read_to_string(after.path().join(INTENT_LEDGER)).expect("read");
    let first = text.lines().next().expect("line");
    let mut f = fs::File::create(before.path().join(INTENT_LEDGER)).expect("create");
    writeln!(f, "{first}").expect("w");
    write!(f, "{{\"schema\":\"celeris.submit-int").expect("torn");
    drop(f);
    assert_eq!(
        ledger(before.path()).replay().expect("replay")[&s.submit_intent_id()].state,
        IntentState::Recorded
    );

    for dir in [after.path(), before.path()] {
        let fake2 = FakePbs::new();
        fake2
            .script_find(Scripted::ok(&record("88.sirius", &s, &[])))
            .script_qstat(Scripted::ok(&record("88.sirius", &s, &[])));
        let restarted = IntentLedger::open(dir, "epoch-2").expect("ledger");
        let out = restarted
            .submit(&s, &fake2, &FakeClock::new())
            .expect("resume");
        assert!(
            matches!(&out, SubmitOutcome::Reconciled { job_id, .. } if job_id == "88.sirius"),
            "{out:?}"
        );
        assert_eq!(fake2.counts().qsub, 0, "restart never resends qsub");
        let rows = restarted.rows().expect("rows");
        let unknown = rows
            .iter()
            .find(|r| matches!(r.event, IntentEvent::UnknownSubmit { .. }))
            .expect("unknown_submit");
        assert_eq!(unknown.epoch, "epoch-2");
        assert!(matches!(
            unknown.event,
            IntentEvent::UnknownSubmit {
                cause: UnknownCause::Interrupted,
                ..
            }
        ));
    }
    assert_eq!(fake.counts().qsub, 1);
}

/// qsub の中で、試験が許すまで止まる transport。
struct Gated<'a> {
    inner: &'a FakePbs,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl PbsTransport for Gated<'_> {
    fn origin(&self) -> Origin {
        Origin::Synthetic
    }
    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        let _ = self.entered.send(());
        let _ = self.release.lock().expect("gate").recv();
        self.inner.qsub(req)
    }
    fn qstat_full(&self, job_id: &str) -> PbsReply {
        self.inner.qstat_full(job_id)
    }
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        self.inner.find_by_marker(m)
    }
}

#[test]
fn concurrent_double_submit_sends_one_qsub() {
    let d = tempfile::tempdir().expect("dir");
    let s = spec();
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::ok("500.sirius"))
        .script_qsub(Scripted::ok("501.sirius"));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gated = Gated {
        inner: &fake,
        entered: entered_tx,
        release: Mutex::new(release_rx),
    };
    let (wait_tx, wait_rx) = mpsc::channel::<()>();
    let wait_tx = Mutex::new(wait_tx);
    let second = IntentLedger::open(d.path(), "epoch-b")
        .expect("ledger")
        .with_lock_wait_hook(Arc::new(move || {
            let _ = wait_tx.lock().expect("hook").send(());
        }));
    let first = IntentLedger::open(d.path(), "epoch-a").expect("ledger");
    let (a, b) = std::thread::scope(|sc| {
        let a = sc.spawn(|| first.submit(&s, &gated, &FakeClock::new()));
        // 1 本目が lock の中で qsub に入った。
        entered_rx.recv().expect("entered");
        let b = sc.spawn(|| second.submit(&s, &fake, &FakeClock::new()));
        // 2 本目が lock を待ち始めた（出来事）。
        wait_rx.recv().expect("second waits for the lock");
        release_tx.send(()).expect("release");
        (a.join().expect("a"), b.join().expect("b"))
    });
    let a = a.expect("a");
    assert_eq!(
        a,
        SubmitOutcome::Accepted {
            job_id: "500.sirius".into(),
            job_ref: "pbs:sirius:500.sirius".into()
        }
    );
    assert_eq!(b.expect("b"), a, "second path replays the first submit");
    assert_eq!(fake.counts().qsub, 1);
    assert_eq!(fake.submitted().len(), 1);
    assert_eq!(states(d.path()), ["intent_recorded", "sent", "accepted"]);
}

#[test]
fn ledger_rejects_rows_that_break_the_transition_rules() {
    let d = tempfile::tempdir().expect("dir");
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::ok("1.sirius"));
    ledger(d.path())
        .submit(&spec(), &fake, &FakeClock::new())
        .expect("submit");
    let path = d.path().join(INTENT_LEDGER);
    let text = fs::read_to_string(&path).expect("read");
    let first = text.lines().next().expect("line").to_string();
    // 2 本目の intent_recorded（同じ intent を 2 回記録しない）。
    fs::write(&path, format!("{text}{first}\n")).expect("w");
    let err = ledger(d.path()).replay().unwrap_err();
    assert!(err.to_string().contains("second intent_recorded"), "{err}");
    // accepted の後の reconciled。
    let mut v: serde_json::Value =
        serde_json::from_str(text.lines().nth(2).expect("acc")).expect("j");
    v["state"] = "reconciled".into();
    v["payload_sha256"] = "x".into();
    v["replies"] = serde_json::json!([]);
    v.as_object_mut().expect("o").remove("reply");
    fs::write(&path, format!("{text}{v}\n")).expect("w");
    let err = ledger(d.path()).replay().unwrap_err();
    assert!(err.to_string().contains("not allowed"), "{err}");
    // 失敗した台帳では送らない。
    assert!(
        ledger(d.path())
            .submit(&spec(), &fake, &FakeClock::new())
            .is_err()
    );
    assert_eq!(fake.counts().qsub, 1);
}
