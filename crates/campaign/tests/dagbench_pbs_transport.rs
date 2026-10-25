//! recovery の PBS transport（ADR 2026-10-25-recovery-scheduler-backend 実装付記 I2）。
//! 偽 transport の台本で応答の分類を、stub の ssh（実 ssh・qsub・network は使わない）で `SshPbs` の全体 timeout と
//! 引数の組み立てを確かめる。timeout は実時間でなく、stub が途中出力を書いた出来事（fifo）で注入時計を発火させる。
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use celeris_campaign::{
    dagbench::pbs::transport::{
        CallOutcome, CallTimer, FakeCounts, FakePbs, MarkerClass, MarkerQuery, PbsCall,
        PbsTransport, QstatClass, QstatView, QsubClass, QsubRequest, QueryFailure, Scripted,
        SshPbs, UnknownReason, classify_marker, classify_qstat, classify_qsub,
    },
    observe::{Origin, SchedulerTransport},
};

fn req() -> QsubRequest {
    QsubRequest {
        queue: "debug".into(),
        account: "NBBG".into(),
        select: "1:ncpus=4:mpiprocs=1".into(),
        place: "excl".into(),
        walltime: "00:30:00".into(),
        job_name: "rcv-baseline-n1".into(),
        trial_key: "m1.baseline.n1.a1".into(),
        submit_intent: format!("si-{}", "a".repeat(64)),
        script: "/work/NBB/rmaeda/recovery/job.sh".into(),
    }
}

fn query() -> MarkerQuery {
    let r = req();
    MarkerQuery {
        owner: "rmaeda".into(),
        trial_key: r.trial_key,
        submit_intent: r.submit_intent,
    }
}

fn record(id: &str, owner: &str, trial: &str, intent: &str) -> String {
    format!(
        "Job Id: {id}\n    Job_Name = rcv\n    Job_Owner = {owner}@sirius-login\n    job_state = F\n    queue = debug\n    Variable_List = PBS_O_HOME=/home/{owner},CELERIS_TRIAL_KEY={trial},\n\tCELERIS_SUBMIT_INTENT={intent}\n    Exit_status = 0\n\n"
    )
}

#[test]
fn qsub_request_is_non_rerunnable_and_carries_both_markers() {
    let argv = req().argv().expect("valid");
    assert_eq!(argv[0], "qsub");
    let r = argv.iter().position(|a| a == "-r").expect("-r");
    assert_eq!(argv[r + 1], "n");
    let v = argv.iter().position(|a| a == "-v").expect("-v");
    assert_eq!(
        argv[v + 1],
        format!(
            "CELERIS_TRIAL_KEY=m1.baseline.n1.a1,CELERIS_SUBMIT_INTENT=si-{}",
            "a".repeat(64)
        )
    );
    assert_eq!(
        argv.last().map(String::as_str),
        Some("/work/NBB/rmaeda/recovery/job.sh")
    );
    for bad in [
        QsubRequest {
            trial_key: "k,X=1".into(),
            ..req()
        },
        QsubRequest {
            queue: "-q".into(),
            ..req()
        },
        QsubRequest {
            script: "job.sh".into(),
            ..req()
        },
        QsubRequest {
            script: "/work/../etc/x".into(),
            ..req()
        },
        QsubRequest {
            select: "1;rm".into(),
            ..req()
        },
        QsubRequest {
            walltime: "1h".into(),
            ..req()
        },
    ] {
        assert!(bad.argv().is_err(), "{bad:?}");
    }
}

#[test]
fn qsub_classification_accepts_only_a_full_pbs_id() {
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::ok("123.sirius\n"))
        .script_qsub(Scripted::timeout(""))
        .script_qsub(Scripted::nonzero(255, "Connection closed by remote host"))
        .script_qsub(Scripted::nonzero(1, "qsub: Unknown queue"))
        .script_qsub(Scripted::ok("4242\n"))
        .script_qsub(Scripted::ok(""))
        .script_qsub(Scripted::ok("123.sirius\n124.sirius\n"))
        .script_qsub(Scripted::spawn_failed());
    let classes: Vec<QsubClass> = (0..8).map(|_| classify_qsub(&fake.qsub(&req()))).collect();
    assert_eq!(
        classes[0],
        QsubClass::Accepted {
            job_id: "123.sirius".into()
        }
    );
    let reasons: Vec<Option<UnknownReason>> = classes
        .iter()
        .map(|c| match c {
            QsubClass::Unknown { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasons[1..7],
        [
            Some(UnknownReason::Timeout),
            Some(UnknownReason::Disconnected),
            Some(UnknownReason::NonZero),
            Some(UnknownReason::InvalidId),
            Some(UnknownReason::InvalidId),
            Some(UnknownReason::InvalidId),
        ]
    );
    // 起動していないことが確実なものだけが送信前拒否。
    assert!(matches!(classes[7], QsubClass::RejectedBeforeSend { .. }));
    assert_eq!(fake.counts().qsub, 8);
    assert_eq!(fake.origin(), Origin::Synthetic);
}

#[test]
fn invalid_qsub_request_is_rejected_before_send_and_not_counted() {
    let fake = FakePbs::new();
    fake.script_qsub(Scripted::ok("1.sirius"));
    let r = fake.qsub(&QsubRequest {
        account: "a b".into(),
        ..req()
    });
    assert_eq!(r.outcome, CallOutcome::SpawnFailed);
    assert!(matches!(
        classify_qsub(&r),
        QsubClass::RejectedBeforeSend { .. }
    ));
    assert_eq!(fake.counts(), FakeCounts::default());
    assert!(fake.submitted().is_empty());
}

#[test]
fn qstat_classification_separates_record_unknown_job_and_failures() {
    let q = query();
    let fake = FakePbs::new();
    fake.script_qstat(Scripted::ok(&record(
        "123.sirius",
        "rmaeda",
        &q.trial_key,
        &q.submit_intent,
    )))
    .script_qstat(Scripted::nonzero(153, "qstat: Unknown Job Id 123.sirius"))
    .script_qstat(Scripted::timeout("Job Id: 123.sirius\n    job_st"))
    .script_qstat(Scripted::nonzero(
        255,
        "ssh: connect to host sirius: Connection refused",
    ))
    .script_qstat(Scripted::ok(&record(
        "999.sirius",
        "rmaeda",
        &q.trial_key,
        &q.submit_intent,
    )));
    let c: Vec<QstatClass> = (0..5)
        .map(|_| classify_qstat(&fake.qstat_full("123.sirius"), "123.sirius"))
        .collect();
    let QstatClass::Record { attrs } = &c[0] else {
        panic!("{:?}", c[0])
    };
    assert_eq!(attrs.get("job_state").map(String::as_str), Some("F"));
    assert!(attrs["Variable_List"].contains(&format!("CELERIS_SUBMIT_INTENT={}", q.submit_intent)));
    assert!(matches!(c[1], QstatClass::UnknownJob { .. }));
    for (i, want) in [
        (2, UnknownReason::Timeout),
        (3, UnknownReason::Disconnected),
        (4, UnknownReason::InvalidId),
    ] {
        assert!(
            matches!(&c[i], QstatClass::Failed { reason: QueryFailure::Unknown(r), .. } if *r == want),
            "{i}: {:?}",
            c[i]
        );
    }
    // PID 形の ID は照会しない。
    let r = fake.qstat_full("4242");
    assert!(matches!(
        classify_qstat(&r, "4242"),
        QstatClass::Failed {
            reason: QueryFailure::NotStarted,
            ..
        }
    ));
    assert_eq!(fake.counts().qstat_full, 5);
}

#[test]
fn marker_search_matches_own_jobs_exactly_and_unsupported_is_unknown() {
    let q = query();
    let out = [
        record("10.sirius", "rmaeda", &q.trial_key, &q.submit_intent),
        // 他人の job・別 trial・別 intent・前方一致だけの値は候補にしない。
        record("11.sirius", "other", &q.trial_key, &q.submit_intent),
        record("12.sirius", "rmaeda", "m1.baseline.n1.a2", &q.submit_intent),
        record(
            "13.sirius",
            "rmaeda",
            &q.trial_key,
            &format!("si-{}", "b".repeat(64)),
        ),
        record(
            "14.sirius",
            "rmaeda",
            &q.trial_key,
            &format!("{}0", q.submit_intent),
        ),
    ]
    .concat();
    let fake = FakePbs::new();
    fake.script_find(Scripted::ok(&out))
        .script_find(Scripted::ok(""))
        .script_find(Scripted::ok(&format!(
            "{}{}",
            record("10.sirius", "rmaeda", &q.trial_key, &q.submit_intent),
            record("20.sirius", "rmaeda", &q.trial_key, &q.submit_intent)
        )))
        .script_find(Scripted::unsupported())
        .script_find(Scripted::timeout(""))
        .script_find(Scripted::ok("qselect: garbage\n"));
    let c: Vec<MarkerClass> = (0..6)
        .map(|_| classify_marker(&fake.find_by_marker(&q), &q))
        .collect();
    assert_eq!(c[0], MarkerClass::Candidates(vec!["10.sirius".into()]));
    assert_eq!(c[1], MarkerClass::Candidates(vec![]));
    assert_eq!(
        c[2],
        MarkerClass::Candidates(vec!["10.sirius".into(), "20.sirius".into()])
    );
    for (i, want) in [
        (3, UnknownReason::Unsupported),
        (4, UnknownReason::Timeout),
        (5, UnknownReason::InvalidId),
    ] {
        assert!(
            matches!(&c[i], MarkerClass::Failed { reason: QueryFailure::Unknown(r), .. } if *r == want),
            "{i}: {:?}",
            c[i]
        );
    }
    assert_eq!(fake.counts().find_by_marker, 6);
    assert_eq!(fake.counts().qsub, 0);
}

#[test]
fn qstat_view_maps_only_qstat_and_keeps_raw_replies() {
    let q = query();
    let fake = FakePbs::new();
    fake.script_qstat(Scripted::ok(&record(
        "5.sirius",
        "rmaeda",
        &q.trial_key,
        &q.submit_intent,
    )))
    .script_qstat(Scripted::nonzero(153, "qstat: Unknown Job Id 6.sirius"));
    let view = QstatView::new(&fake, "sirius");
    let ok = view.qstat("pbs", "sirius", "5.sirius").expect("reply");
    assert_eq!(ok.cluster, "sirius");
    assert!(ok.raw.starts_with("Job Id: 5.sirius"));
    let e = view
        .qstat("pbs", "sirius", "6.sirius")
        .expect_err("unknown");
    assert!(e.contains("Unknown Job Id"), "{e}");
    assert!(view.qstat("pbs", "pegasus", "5.sirius").is_err());
    assert!(view.qstat("slurm", "sirius", "5.sirius").is_err());
    let raws = view.into_replies();
    assert_eq!(raws.len(), 2);
    assert!(
        raws.iter()
            .all(|r| r.call == PbsCall::QstatFull && !r.started_utc.is_empty())
    );
    assert_eq!(
        fake.counts(),
        FakeCounts {
            qsub: 0,
            qstat_full: 2,
            find_by_marker: 0
        }
    );
}

// --- SshPbs（stub の ssh） ---

/// 実行可能 file を別 process で書く（ETXTBSY 回避）。
fn write_executable(path: &Path, contents: &str) {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn writer");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(contents.as_bytes())
        .expect("write");
    assert!(child.wait().expect("wait").success());
}

/// 引数を 1 行ずつ `$ARGS` に残し、`$MODE` で振る舞いを変える stub ssh。
fn stub(dir: &Path) -> PathBuf {
    let p = dir.join("ssh");
    write_executable(
        &p,
        &format!(
            r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done > {args}
case $(cat {mode}) in
ok) echo 77.sirius ;;
fail) echo 'Connection closed' >&2; exit 255 ;;
hang) printf 'Job Id: 77.sir'; echo ready > {fifo}; exec sleep 3600 ;;
esac
"#,
            args = dir.join("args").display(),
            mode = dir.join("mode").display(),
            fifo = dir.join("fifo").display(),
        ),
    );
    p
}

/// 試験の出来事（stub が途中出力を書いた）で期限切れにする時計。実時間は待たない。
struct EventTimer {
    fire: Mutex<mpsc::Receiver<()>>,
    calls: Mutex<Vec<Duration>>,
}

impl CallTimer for EventTimer {
    fn wait(&self, _event: &mpsc::Receiver<()>, limit: Duration) -> bool {
        self.calls.lock().expect("calls").push(limit);
        let _ = self.fire.lock().expect("fire").recv();
        false
    }
}

#[test]
fn ssh_pbs_builds_batchmode_command_with_validated_args() {
    let d = tempfile::tempdir().expect("dir");
    let prog = stub(d.path());
    std::fs::write(d.path().join("mode"), "ok").expect("mode");
    let t = SshPbs::new("sirius", Duration::from_secs(60))
        .expect("pbs")
        .with_program(&prog);
    assert_eq!(t.origin(), Origin::Live);
    let r = t.qsub(&req());
    assert_eq!(r.outcome, CallOutcome::Ok, "{r:?}");
    assert_eq!(
        classify_qsub(&r),
        QsubClass::Accepted {
            job_id: "77.sirius".into()
        }
    );
    let args = std::fs::read_to_string(d.path().join("args")).expect("args");
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(
        args[..8],
        [
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=60",
            "-o",
            "ControlMaster=no",
            "--",
            "sirius"
        ]
    );
    assert_eq!(args[8..], req().argv().expect("argv")[..]);
    assert_eq!(r.argv[1..], args[..]);

    std::fs::write(d.path().join("mode"), "fail").expect("mode");
    let r = t.qstat_full("77.sirius");
    assert_eq!((r.outcome, r.exit), (CallOutcome::NonZero, Some(255)));
    assert!(matches!(
        classify_qstat(&r, "77.sirius"),
        QstatClass::Failed {
            reason: QueryFailure::Unknown(UnknownReason::Disconnected),
            ..
        }
    ));
    assert_eq!(
        std::fs::read_to_string(d.path().join("args"))
            .expect("args")
            .lines()
            .skip(8)
            .collect::<Vec<_>>(),
        ["qstat", "-x", "-f", "77.sirius"]
    );

    // 安全文字でない cluster・ID・引数は ssh を起動しない。marker 検索は未確認のため起動せず Unsupported。
    assert!(SshPbs::new("-oProxyCommand=x", Duration::from_secs(60)).is_err());
    assert!(SshPbs::new("sirius", Duration::ZERO).is_err());
    std::fs::remove_file(d.path().join("args")).expect("rm");
    let r = t.qstat_full("77.sirius;id");
    assert_eq!(r.outcome, CallOutcome::SpawnFailed);
    let r = t.qsub(&QsubRequest {
        job_name: "$(id)".into(),
        ..req()
    });
    assert!(matches!(
        classify_qsub(&r),
        QsubClass::RejectedBeforeSend { .. }
    ));
    let r = t.find_by_marker(&query());
    assert_eq!(r.outcome, CallOutcome::Unsupported);
    assert!(matches!(
        classify_marker(&r, &query()),
        MarkerClass::Failed {
            reason: QueryFailure::Unknown(UnknownReason::Unsupported),
            ..
        }
    ));
    assert!(
        !d.path().join("args").exists(),
        "ssh must not have been started"
    );

    let missing = SshPbs::new("sirius", Duration::from_secs(60))
        .expect("pbs")
        .with_program(d.path().join("no-such-ssh"));
    assert!(matches!(
        classify_qsub(&missing.qsub(&req())),
        QsubClass::RejectedBeforeSend { .. }
    ));
}

#[test]
fn ssh_pbs_whole_call_timeout_kills_the_child_and_keeps_partial_output() {
    let d = tempfile::tempdir().expect("dir");
    let prog = stub(d.path());
    std::fs::write(d.path().join("mode"), "hang").expect("mode");
    let fifo = d.path().join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );
    let (fire, rx) = mpsc::channel();
    let timer = Arc::new(EventTimer {
        fire: Mutex::new(rx),
        calls: Mutex::new(Vec::new()),
    });
    let t = SshPbs::new("sirius", Duration::from_secs(60))
        .expect("pbs")
        .with_program(&prog)
        .with_timer(timer.clone());
    // stub が途中出力を書いて fifo に知らせたら期限切れにする（読み出しは出来事待ち）。
    let watcher = std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(std::fs::File::open(&fifo).expect("open fifo"))
            .read_line(&mut line)
            .expect("read fifo");
        assert_eq!(line.trim(), "ready");
        fire.send(()).expect("fire");
    });
    let r = t.qsub(&req());
    watcher.join().expect("watcher");
    assert_eq!(r.outcome, CallOutcome::Timeout, "{r:?}");
    assert_eq!(r.exit, None);
    assert_eq!(r.stdout, "Job Id: 77.sir");
    assert_eq!(
        classify_qsub(&r),
        QsubClass::Unknown {
            reason: UnknownReason::Timeout,
            detail: format!(
                "Timeout exit=None stdout={:?} stderr=\"\"",
                "Job Id: 77.sir"
            ),
        }
    );
    // 時計には呼出し全体の上限（60 秒以内の残り）が 1 度だけ渡る。
    let calls = timer.calls.lock().expect("calls").clone();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0] <= Duration::from_secs(60) && calls[0] > Duration::from_secs(50),
        "{calls:?}"
    );
}
