//! 独立観測 runner と所有 process の管理: role 再起動した実 process（External / 停止無応答の child）を相手に、
//! timeout・切断・応答しない child でも有界に停止・回収でき、所有外の sentinel process に触らないこと。
//! 順序は child の ready 行・IPC の応答・exit という出来事で決め、sleep では作らない（長い保険 deadline のみ）。
use std::{
    os::unix::net::UnixListener,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use celeris_campaign::dagbench::{
    ext::{ExtBackend, ExtConfig},
    ipc::IpcExt,
    process::*,
};
use serde_json::Value;

const TEST_ROLE_ENV: &str = "DAGBENCH_TEST_ROLE";

fn cfg() -> ExtConfig {
    ExtConfig {
        poll_secs: 30,
        job_slots: 2,
        faults: vec![],
    }
}

/// role 再起動の入口。通常の test run では何もしない（環境変数が無い）。
#[test]
fn role_entry() {
    let Ok(role) = std::env::var(TEST_ROLE_ENV) else {
        return;
    };
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let ready = || {
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = writeln!(o, "{READY_LINE}");
        let _ = o.flush();
    };
    let park = || loop {
        std::thread::park();
    };
    match role.as_str() {
        "external" => run_external_role_from_env().expect("external role"),
        // 受け付けるが決して応答しない。
        "mute" => {
            let l = UnixListener::bind(var(SOCKET_ENV)).expect("bind");
            ready();
            let mut held = Vec::new();
            for c in l.incoming().flatten() {
                held.push(c);
            }
        }
        // 受け付けてすぐ切る（切断）。
        "drop" => {
            let l = UnixListener::bind(var(SOCKET_ENV)).expect("bind");
            ready();
            for c in l.incoming().flatten() {
                drop(c);
            }
        }
        // graceful 要求（stdin close）を無視して止まらない。
        "stubborn" => {
            ready();
            park();
        }
        other => panic!("unknown role {other}"),
    }
}

fn role_cmd(role: &str, sock: &Path, ledger: &Path) -> Command {
    let mut c = Command::new(std::env::current_exe().expect("exe"));
    c.args(["--exact", "role_entry", "--nocapture", "--test-threads=1"])
        .env(TEST_ROLE_ENV, role)
        .env(SOCKET_ENV, sock)
        .env(LEDGER_ENV, ledger)
        .env(EXT_CFG_ENV, serde_json::to_string(&cfg()).expect("cfg"));
    c
}

fn wait_ready(p: &OwnedProc) {
    loop {
        let l = p.next_line(Duration::from_secs(60)).expect("ready line");
        if l.ends_with(READY_LINE) {
            return;
        }
    }
}

fn events_of(log: &EventLog, kind: &str) -> Vec<Value> {
    read_events(log.path())
        .expect("events")
        .into_iter()
        .filter(|e| e["kind"] == kind)
        .collect()
}

#[test]
fn ipc_bounded_shutdown() {
    let logd = tempfile::tempdir().expect("log dir");
    // unix socket の path は 107 byte までなので、socket だけ短い dir に置く。
    let sd = tempfile::Builder::new()
        .prefix("pr")
        .tempdir_in("/tmp")
        .expect("sock dir");
    let work = tempfile::tempdir().expect("work");
    let limits = Limits {
        wall: Duration::from_secs(120),
        logical_secs: 10_000,
        max_restarts: 2,
        ipc_io_timeout: Duration::from_millis(300),
        ipc_retries: 1,
        stop_grace: Duration::from_millis(300),
        kill_wait: Duration::from_secs(10),
    };
    let bound = limits.stop_grace + limits.kill_wait + 2 * limits.ipc_io_timeout * 2;
    let log = EventLog::create(logd.path()).expect("log");
    assert!(EventLog::create(Path::new("relative/dir")).is_err());
    let mut sup = Supervisor::new(log.clone(), limits.clone()).expect("sup");

    // 所有外の sentinel（test が直接起こす。supervisor は存在を知らない）。
    let mut sentinel = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("sentinel");
    let sentinel_pid = sentinel.id();

    let mut stop_ipc = |role: &str, name: &str| {
        let sock = sd.path().join(format!("{name}.sock"));
        let mut p = sup
            .spawn(
                Role::External,
                role_cmd(role, &sock, &work.path().join(format!("{name}.jsonl"))),
            )
            .expect("spawn");
        wait_ready(&p);
        let mut ipc = IpcExt::connect_unix(&sock, "control", limits.ipc_options());
        let t = Instant::now();
        let r = p
            .stop(Some(&mut || ipc.shutdown_server()), name)
            .expect("stop");
        assert!(t.elapsed() < bound, "{name}: {:?}", t.elapsed());
        assert!(p.try_exit().expect("wait").is_some(), "{name} reaped");
        r
    };

    // 応答しない server（timeout）→ kill。
    let r = stop_ipc("mute", "mute");
    assert_eq!((r.reason.as_str(), r.killed), ("killed", true), "{r:?}");
    assert!(r.graceful_error.is_some(), "{r:?}");
    // 切断される → kill。
    let r = stop_ipc("drop", "drop");
    assert_eq!((r.reason.as_str(), r.killed), ("killed", true), "{r:?}");
    assert!(r.graceful_error.is_some(), "{r:?}");

    // graceful 要求を無視する child（stdin close を無視）→ 期限で kill。
    let mut p = sup
        .spawn(
            Role::Control,
            role_cmd("stubborn", &sd.path().join("x"), &work.path().join("x")),
        )
        .expect("spawn");
    wait_ready(&p);
    let t = Instant::now();
    let r = p.stop(None, "unresponsive").expect("stop");
    assert_eq!((r.reason.as_str(), r.killed), ("killed", true), "{r:?}");
    assert!(
        r.graceful_error.is_none() && r.exit_signal == Some(9),
        "{r:?}"
    );
    assert!(t.elapsed() < bound);

    // 正常な External は Shutdown で graceful に終わる。観測 runner が control 不在で時計を進める。
    let sock = sd.path().join("ext.sock");
    let ledger = work.path().join("ext.jsonl");
    let mut ext = sup
        .spawn(Role::External, role_cmd("external", &sock, &ledger))
        .expect("spawn");
    wait_ready(&ext);
    // 健全な server への client は長い保険の timeout（短い timeout は上の異常系だけ）。
    let healthy = Limits {
        ipc_io_timeout: Duration::from_secs(60),
        ..limits.clone()
    };
    let mut control = IpcExt::connect_unix(&sock, "control", healthy.ipc_options());
    control.submit("j1", 100).expect("io").expect("accepted");
    let mut observer = IpcExt::connect_unix(&sock, "observer", healthy.ipc_options());
    let out = observe_until_complete(
        &mut observer,
        &["j1".to_string()],
        &limits,
        sup.wall_deadline(),
        &log,
    )
    .expect("observe");
    // observer は通知を消費・ack していない: outbox に 1 件残り、control が後から受け取る。
    assert_eq!((out.final_now, out.high_seq, out.unacked), (100, 1, 1));
    let got = control.advance(0).expect("advance");
    assert_eq!(got.len(), 1, "{got:?}");
    let r = ext
        .stop(Some(&mut || control.shutdown_server()), "done")
        .expect("stop");
    assert_eq!((r.reason.as_str(), r.killed), ("graceful", false), "{r:?}");
    assert_eq!(r.exit_code, Some(0));
    // ledger は sync 済みで外部完了が残る。
    let l = std::fs::read_to_string(&ledger).expect("ledger");
    assert!(l.contains("external_job_completed"), "{l}");

    // 所有外 sentinel は無事で、証跡のどこにも現れない。
    assert!(sentinel.try_wait().expect("try_wait").is_none());
    let events = read_events(log.path()).expect("events");
    assert!(!events.iter().any(|e| e["pid"] == sentinel_pid));
    assert_eq!(events_of(&log, "process_stopped").len(), 4);
    let started = events_of(&log, "process_started");
    assert_eq!(started[0]["role"], "observer");
    assert!(
        started[1..]
            .iter()
            .all(|e| e["owner_pid"] == std::process::id())
    );
    assert_eq!(events_of(&log, "limits").len(), 1);
    let b = events_of(&log, "barrier");
    assert_eq!(b[0]["barrier_seq"], 1);
    let seqs: Vec<u64> = events.iter().filter_map(|e| e["seq"].as_u64()).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    sentinel.kill().expect("kill sentinel");
    sentinel.wait().expect("reap sentinel");
}
